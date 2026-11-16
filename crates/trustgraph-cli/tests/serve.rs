//! `trustgraph serve` end to end over real loopback HTTP.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use serde_json::{Value, json};
use trustgraph_cli::serve::{self, Limits, ServeJournal, Server, Settings};
use trustgraph_engine::{Journal, JournalError, MemoryJournal, Record, Runtime};
use trustgraph_policy::STARTER;

const OPERATOR: &str = "leo";

/// The starter policy with `planner` replaced by `scout`, `roadmap` narrowed
/// to hr, and no tests (the starter's tests name `planner`).
const NEXT: &str = "version: 1
groups:
  hr: [nina]
  engineering: [sam, ravi]
  sales: [leo]
  customers: [acme]
agents: [mailer, scout]
sources:
  salaries: { readers: [hr], agents: [scout] }
  roadmap: { readers: [hr], agents: [scout] }
  price_sheet: { readers: [sales, customers], agents: [mailer] }
actions:
  post_to_slack: { agents: [scout], audience: [engineering] }
  email_customer:
    agents: [mailer]
    audience: [customers]
    approval: { reviewers: [sales], valid_for: 30m }
";

struct Harness<J: ServeJournal> {
    server: Arc<Server<J>>,
    base: String,
    port: u16,
    state: PathBuf,
    client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

fn settings(port: u16, state: &Path, limits: Limits) -> Settings {
    Settings {
        operator: OPERATOR.into(),
        port,
        state_dir: state.to_path_buf(),
        limits,
    }
}

async fn launch<J: ServeJournal>(make: impl FnOnce(u16) -> Arc<Server<J>>) -> Harness<J> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = make(port);
    let app = serve::router(server.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Harness {
        state: server.settings().state_dir.clone(),
        server,
        base: format!("http://127.0.0.1:{port}"),
        port,
        client: client(),
        task,
    }
}

async fn memory(state: &Path, limits: Limits) -> Harness<MemoryJournal> {
    let runtime = Runtime::open(MemoryJournal::new(), STARTER, 0).unwrap();
    let state = state.to_path_buf();
    launch(move |port| Server::new(runtime, settings(port, &state, limits)).unwrap()).await
}

impl<J: ServeJournal> Harness<J> {
    fn token(&self, agent: &str) -> String {
        std::fs::read_to_string(self.state.join("tokens").join(format!("{agent}.token"))).unwrap()
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> (u16, Value) {
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        assert_eq!(
            response.headers()["cache-control"],
            "no-store",
            "every response is no-store"
        );
        let body = response.json().await.unwrap_or(Value::Null);
        (status, body)
    }

    async fn post(&self, path: &str, token: Option<&str>, body: Value) -> (u16, Value) {
        let mut request = self.client.post(format!("{}{path}", self.base)).json(&body);
        if let Some(t) = token {
            request = request.bearer_auth(t);
        }
        self.send(request).await
    }

    async fn get(&self, path: &str, token: Option<&str>) -> (u16, Value) {
        let mut request = self.client.get(format!("{}{path}", self.base));
        if let Some(t) = token {
            request = request.bearer_auth(t);
        }
        self.send(request).await
    }

    /// Exchange the launch secret for the operator's session cookie.
    async fn login(&self) -> String {
        let secret = self.server.launch_secret().expect("unused launch secret");
        let response = self
            .client
            .post(format!("{}/session", self.base))
            .bearer_auth(&secret)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .to_string();
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
        cookie.split(';').next().unwrap().to_string()
    }

    async fn op_post(&self, path: &str, cookie: &str, body: Value) -> (u16, Value) {
        let request = self
            .client
            .post(format!("{}{path}", self.base))
            .header("cookie", cookie)
            .json(&body);
        self.send(request).await
    }

    async fn op_state(&self, cookie: &str) -> Value {
        let request = self
            .client
            .get(format!("{}/v1/operator/state", self.base))
            .header("cookie", cookie);
        let (status, body) = self.send(request).await;
        assert_eq!(status, 200, "{body}");
        body
    }

    async fn source(&self, token: &str, source: &str, content: &str) -> String {
        let (status, body) = self
            .post(
                "/v1/sources",
                Some(token),
                json!({"source": source, "content": content}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["artifact"].as_str().unwrap().to_string()
    }

    async fn output(&self, token: &str, inputs: &[&str], content: &str) -> String {
        let (status, body) = self
            .post(
                "/v1/outputs",
                Some(token),
                json!({"inputs": inputs, "step": "draft", "content": content}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body["artifact"].as_str().unwrap().to_string()
    }

    async fn check(&self, token: &str, artifact: &str, action: &str) -> Value {
        let (status, body) = self
            .post(
                "/v1/check",
                Some(token),
                json!({"artifact": artifact, "action": action, "recipients": null}),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        body
    }

    /// Stop serving and wait until nothing holds the server (and its journal).
    async fn stop(self) {
        let weak: Weak<Server<J>> = Arc::downgrade(&self.server);
        self.task.abort();
        let _ = self.task.await;
        drop(self.server);
        drop(self.client);
        for _ in 0..200 {
            if weak.strong_count() == 0 {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the server is still referenced after shutdown");
    }
}

#[tokio::test]
async fn full_flow_deny_allow_approve_revoke() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), Limits::default()).await;
    let planner = h.token("planner");
    let mailer = h.token("mailer");

    let salaries = h.source(&planner, "salaries", "salary table").await;
    let roadmap = h.source(&planner, "roadmap", "q3 roadmap").await;
    let both = h.output(&planner, &[&salaries, &roadmap], "summary").await;
    let denied = h.check(&planner, &both, "post_to_slack").await;
    assert_eq!(denied["verdict"], "deny", "{denied}");
    assert_eq!(denied["blocking"][0]["source"], "salaries");
    assert!(denied["decision"].is_string() && denied["reason"].is_string());

    let only = h.output(&planner, &[&roadmap], "roadmap summary").await;
    assert_eq!(
        h.check(&planner, &only, "post_to_slack").await["verdict"],
        "allow"
    );

    let (status, explained) = h
        .get(&format!("/v1/artifacts/{both}"), Some(&planner))
        .await;
    assert_eq!(status, 200);
    assert_eq!(explained["sources"], json!(["roadmap", "salaries"]));
    assert_eq!(explained["readers"], json!(["nina"]));

    let sheet = h.source(&mailer, "price_sheet", "prices").await;
    let email = h.output(&mailer, &[&sheet], "quote").await;
    let pending = h.check(&mailer, &email, "email_customer").await;
    assert_eq!(pending["verdict"], "needs_approval");
    let decision = pending["decision"].as_str().unwrap().to_string();

    let cookie = h.login().await;
    let state = h.op_state(&cookie).await;
    assert_eq!(state["operator"], OPERATOR);
    assert_eq!(state["policy"]["source"], "file");
    assert_eq!(state["policy"]["revision"], 1);
    let p = &state["pending"][0];
    assert_eq!(p["decision"], decision);
    assert_eq!(p["agent"], "mailer");
    assert_eq!(p["action"], "email_customer");
    assert_eq!(p["recipients"], json!(["acme"]));
    assert_eq!(p["sources"], json!(["price_sheet"]));
    assert!(p["reason"].is_string() && p["requested_at"].is_u64());

    let (status, approved) = h
        .op_post(
            "/v1/operator/approve",
            &cookie,
            json!({"decision": decision}),
        )
        .await;
    assert_eq!(status, 200, "{approved}");
    let allowed = h.check(&mailer, &email, "email_customer").await;
    assert_eq!(allowed["verdict"], "allow");
    assert!(allowed["expires_at"].is_u64());
    let state = h.op_state(&cookie).await;
    assert_eq!(state["pending"], json!([]));
    assert_eq!(state["approvals"][0]["decision"], decision);
    assert_eq!(state["approvals"][0]["agent"], "mailer");

    let (status, _) = h
        .op_post(
            "/v1/operator/revoke",
            &cookie,
            json!({"decision": decision}),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        h.check(&mailer, &email, "email_customer").await["verdict"],
        "needs_approval"
    );

    let (status, policy) = h.get("/v1/policy", Some(&mailer)).await;
    assert_eq!(status, 200);
    assert_eq!(policy["revision"], 1);
    assert!(policy["policy"].as_str().unwrap().contains("price_sheet"));
    let (status, schema) = h.get("/v1/policy/schema", Some(&mailer)).await;
    assert_eq!(status, 200);
    assert!(schema.is_object());
}

#[tokio::test]
async fn identity_comes_only_from_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), Limits::default()).await;
    let planner = h.token("planner");
    let mailer = h.token("mailer");
    let roadmap = h.source(&planner, "roadmap", "q3").await;
    let only = h.output(&planner, &[&roadmap], "summary").await;

    // Not listed for the source.
    let (status, body) = h
        .post(
            "/v1/sources",
            Some(&mailer),
            json!({"source": "salaries", "content": "x"}),
        )
        .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (403, Some("agent_cannot_use_source"))
    );
    // The mailer cannot build on the planner's sources.
    let (status, body) = h
        .post(
            "/v1/outputs",
            Some(&mailer),
            json!({"inputs": [only], "step": "s", "content": "c"}),
        )
        .await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (403, Some("agent_cannot_use_source"))
    );
    let (status, _) = h.get(&format!("/v1/artifacts/{only}"), Some(&mailer)).await;
    assert_eq!(status, 403);
    // Checking as the mailer is the mailer's check, whatever the artifact.
    assert_eq!(
        h.check(&mailer, &only, "post_to_slack").await["verdict"],
        "deny"
    );

    // No body field can name an agent.
    for (path, body) in [
        (
            "/v1/check",
            json!({"artifact": only, "action": "post_to_slack", "agent": "planner"}),
        ),
        (
            "/v1/outputs",
            json!({"inputs": [only], "step": "s", "content": "c", "agent": "planner"}),
        ),
        (
            "/v1/sources",
            json!({"source": "roadmap", "content": "x", "agent": "planner"}),
        ),
        (
            "/v1/policy/propose",
            json!({"policy": NEXT, "agent": "planner"}),
        ),
    ] {
        let (status, reply) = h.post(path, Some(&mailer), body).await;
        assert_eq!(status, 400, "{path}: {reply}");
        assert_eq!(reply["error"]["code"], "invalid_request");
    }

    // Missing, wrong and almost-right tokens.
    let wrong = format!("{}0", &planner[..63]);
    for token in [None, Some("nope"), Some(wrong.as_str())] {
        let (status, reply) = h
            .post(
                "/v1/check",
                token,
                json!({"artifact": only, "action": "post_to_slack"}),
            )
            .await;
        assert_eq!(status, 401, "{token:?}");
        assert_eq!(reply["error"]["code"], "unauthorized");
    }
    // Agents cannot hand their own requests to the operator routes.
    let (status, _) = h
        .post(
            "/v1/operator/approve",
            Some(&mailer),
            json!({"decision": "x"}),
        )
        .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn operator_routes_need_the_session_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), Limits::default()).await;
    let mailer = h.token("mailer");
    let secret = h.server.launch_secret().unwrap();

    // An agent token is not a launch secret, and probing does not burn it.
    let (status, _) = h.post("/session", Some(&mailer), json!({})).await;
    assert_eq!(status, 401);
    let routes: [(&str, Value); 4] = [
        ("/v1/operator/approve", json!({"decision": "d"})),
        ("/v1/operator/revoke", json!({"decision": "d"})),
        ("/v1/operator/activate", json!({"proposal": "p"})),
        ("/v1/operator/propose", json!({"policy": NEXT})),
    ];
    for token in [Some(mailer.as_str()), Some(secret.as_str()), None] {
        let (status, _) = h.get("/v1/operator/state", token).await;
        assert_eq!(status, 401);
        for (path, body) in &routes {
            let (status, reply) = h.post(path, token, body.clone()).await;
            assert_eq!(status, 401, "{path}");
            assert_eq!(reply["error"]["code"], "operator_session_required");
        }
    }
    assert_eq!(h.server.launch_secret().as_deref(), Some(secret.as_str()));

    let cookie = h.login().await;
    assert!(!cookie.contains(&secret));
    // Single use.
    let (status, _) = h.post("/session", Some(&secret), json!({})).await;
    assert_eq!(status, 401);
    // A forged cookie holding the launch secret is not a session.
    let forged = format!("{}={secret}", cookie.split('=').next().unwrap());
    let (status, _) = h
        .op_post("/v1/operator/propose", &forged, json!({"policy": NEXT}))
        .await;
    assert_eq!(status, 401);

    h.op_state(&cookie).await;
    let expected = [404, 404, 404, 200];
    for ((path, body), want) in routes.iter().zip(expected) {
        let (status, reply) = h.op_post(path, &cookie, body.clone()).await;
        assert_eq!(status, want, "{path}: {reply}");
    }
    // The page itself is plain and strict.
    let response = h.client.get(format!("{}/", h.base)).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-security-policy"], serve::CSP);
}

#[tokio::test]
async fn host_and_origin_are_enforced_on_every_route() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), Limits::default()).await;
    let mailer = h.token("mailer");
    let router = serve::router(h.server.clone());
    use tower::ServiceExt;
    let port = h.port;
    let good_host = format!("127.0.0.1:{port}");
    let cases = [
        (format!("127.0.0.1:{}", port.wrapping_add(1)), None),
        ("evil.test".to_string(), None),
        (format!("evil.test:{port}"), None),
        (good_host.clone(), Some("http://evil.test")),
        (
            good_host.clone(),
            Some(&format!("http://localhost:{port}")[..]),
        ),
    ];
    for path in ["/", "/session", "/v1/check", "/v1/operator/state", "/mcp"] {
        for (host, origin) in &cases {
            let mut request = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("host", host)
                .header("authorization", format!("Bearer {mailer}"))
                .header("content-type", "application/json");
            if let Some(origin) = origin {
                request = request.header("origin", *origin);
            }
            let response = router
                .clone()
                .oneshot(request.body(axum::body::Body::from("{}")).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), 403, "{path} {host} {origin:?}");
        }
    }
    let ok = axum::http::Request::builder()
        .uri("/")
        .header("host", &good_host)
        .header("origin", format!("http://{good_host}"))
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(router.oneshot(ok).await.unwrap().status(), 200);
}

#[tokio::test]
async fn activation_rotates_tokens_voids_approvals_and_narrows() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), Limits::default()).await;
    let planner = h.token("planner");
    let mailer = h.token("mailer");
    let roadmap = h.source(&planner, "roadmap", "q3").await;
    let only = h.output(&planner, &[&roadmap], "summary").await;
    assert_eq!(
        h.check(&planner, &only, "post_to_slack").await["verdict"],
        "allow"
    );
    let sheet = h.source(&mailer, "price_sheet", "prices").await;
    let email = h.output(&mailer, &[&sheet], "quote").await;
    let decision = h.check(&mailer, &email, "email_customer").await["decision"].clone();
    let cookie = h.login().await;
    let (status, _) = h
        .op_post(
            "/v1/operator/approve",
            &cookie,
            json!({"decision": decision}),
        )
        .await;
    assert_eq!(status, 200);

    // An invalid proposal never reaches the journal and explains itself.
    let (status, invalid) = h
        .post(
            "/v1/policy/propose",
            Some(&planner),
            json!({"policy": "version: 2"}),
        )
        .await;
    assert_eq!(status, 422);
    assert_eq!(invalid["error"]["code"], "invalid_policy");
    assert!(!invalid["diagnostics"].as_array().unwrap().is_empty());

    let (status, proposed) = h
        .post(
            "/v1/policy/propose",
            Some(&planner),
            json!({"policy": NEXT}),
        )
        .await;
    assert_eq!(status, 200, "{proposed}");
    let proposal = proposed["proposal"].as_str().unwrap().to_string();
    assert!(proposed["explanation"].is_string() && proposed["changes"].is_array());
    let (_, active) = h.get("/v1/policy", Some(&planner)).await;
    assert_eq!(active["revision"], 1, "proposing never changes the policy");

    let state = h.op_state(&cookie).await;
    assert_eq!(state["proposals"][0]["proposal"], proposal);
    assert_eq!(state["proposals"][0]["from"], "planner");
    assert!(state["proposals"][0]["widens"].is_boolean());

    let (status, activated) = h
        .op_post(
            "/v1/operator/activate",
            &cookie,
            json!({"proposal": proposal}),
        )
        .await;
    assert_eq!(status, 200, "{activated}");
    assert_eq!(activated["revision"], 2);
    let state = h.op_state(&cookie).await;
    assert_eq!(state["policy"]["revision"], 2);
    assert_eq!(state["policy"]["source"], "activated");
    assert_eq!(state["approvals"], json!([]), "activation voids approvals");

    let tokens = dir.path().join("tokens");
    assert!(!tokens.join("planner.token").exists());
    assert!(tokens.join("scout.token").exists());
    assert_eq!(
        h.token("mailer"),
        mailer,
        "kept for agents still in the policy"
    );
    let (status, _) = h.get("/v1/policy", Some(&planner)).await;
    assert_eq!(status, 401, "a removed agent's token stops working");
    assert_eq!(
        h.check(&mailer, &email, "email_customer").await["verdict"],
        "needs_approval"
    );

    // The narrowed roadmap applies to the output recorded before.
    let scout = h.token("scout");
    let (status, explained) = h.get(&format!("/v1/artifacts/{only}"), Some(&scout)).await;
    assert_eq!(status, 200);
    assert_eq!(explained["readers"], json!(["nina"]));
    let after = h.check(&scout, &only, "post_to_slack").await;
    assert_eq!(after["verdict"], "deny", "{after}");
}

#[tokio::test]
async fn restart_restores_artifacts_decisions_activation_and_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("trustgraph.policy.yaml");
    std::fs::write(&policy, STARTER).unwrap();
    let state = dir.path().join("state");
    let options = || serve::Options {
        policy: policy.clone(),
        operator: OPERATOR.into(),
        state: Some(state.clone()),
        allow_state_in_repository: false,
        limits: Limits::default(),
    };

    let h = launch(|port| serve::start(options(), port).unwrap()).await;
    let mailer = h.token("mailer");
    let cookie = h.login().await;
    let (_, proposed) = h
        .op_post("/v1/operator/propose", &cookie, json!({"policy": NEXT}))
        .await;
    let (status, _) = h
        .op_post(
            "/v1/operator/activate",
            &cookie,
            json!({"proposal": proposed["proposal"]}),
        )
        .await;
    assert_eq!(status, 200);
    let sheet = h.source(&mailer, "price_sheet", "prices").await;
    let email = h.output(&mailer, &[&sheet], "quote").await;
    let decision = h.check(&mailer, &email, "email_customer").await["decision"].clone();
    h.stop().await;

    let h = launch(|port| serve::start(options(), port).unwrap()).await;
    assert_eq!(h.token("mailer"), mailer, "tokens survive a restart");
    let cookie = h.login().await;
    let state = h.op_state(&cookie).await;
    assert_eq!(state["policy"]["source"], "activated");
    assert_eq!(state["policy"]["revision"], 2);
    assert_eq!(state["pending"][0]["decision"], decision);
    let (status, explained) = h
        .get(&format!("/v1/artifacts/{email}"), Some(&mailer))
        .await;
    assert_eq!(status, 200);
    assert_eq!(explained["sources"], json!(["price_sheet"]));
    assert!(dir.path().join("state/tokens/scout.token").exists());
    assert!(!dir.path().join("state/tokens/planner.token").exists());
    h.stop().await;

    // A changed policy file wins at the next start.
    std::fs::write(
        &policy,
        STARTER.replace("customers: [acme]", "customers: [acme, beta]"),
    )
    .unwrap();
    let h = launch(|port| serve::start(options(), port).unwrap()).await;
    let cookie = h.login().await;
    let state = h.op_state(&cookie).await;
    assert_eq!(state["policy"]["source"], "file");
    assert_eq!(state["policy"]["revision"], 3);
    h.stop().await;
}

#[tokio::test]
async fn start_refuses_bad_operators_and_repository_state() {
    let dir = tempfile::tempdir().unwrap();
    let policy = dir.path().join("p.yaml");
    std::fs::write(&policy, STARTER).unwrap();
    let options = |operator: &str, state: PathBuf| serve::Options {
        policy: policy.clone(),
        operator: operator.into(),
        state: Some(state),
        allow_state_in_repository: false,
        limits: Limits::default(),
    };
    for operator in ["mailer", "nobody", "sales"] {
        let error = serve::start(options(operator, dir.path().join("s")), 1)
            .err()
            .unwrap();
        assert_eq!(error.code, 2, "{operator}");
        assert!(error.message.contains("--operator"));
    }
    // A state directory inside the repository that holds the policy.
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    let error = serve::start(options(OPERATOR, dir.path().join("inside")), 1)
        .err()
        .unwrap();
    assert_eq!(error.code, 2);
    assert!(
        error.message.contains("--allow-state-in-repository"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn rate_limit_answers_429() {
    let dir = tempfile::tempdir().unwrap();
    let limits = Limits {
        rate_per_second: 0.01,
        burst: 3.0,
        ..Limits::default()
    };
    let h = memory(dir.path(), limits).await;
    let mailer = h.token("mailer");
    for _ in 0..3 {
        assert_eq!(h.get("/v1/policy", Some(&mailer)).await.0, 200);
    }
    let response = h
        .client
        .get(format!("{}/v1/policy", h.base))
        .bearer_auth(&mailer)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 429);
    assert!(response.headers().contains_key("retry-after"));
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "rate_limited");
    // Other agents have their own budget.
    assert_eq!(h.get("/v1/policy", Some(&h.token("planner"))).await.0, 200);
}

#[tokio::test]
async fn artifact_cap_answers_507_but_the_operator_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let limits = Limits {
        max_artifacts: 1,
        ..Limits::default()
    };
    let h = memory(dir.path(), limits).await;
    let mailer = h.token("mailer");
    h.source(&mailer, "price_sheet", "one").await;
    let (status, body) = h
        .post(
            "/v1/sources",
            Some(&mailer),
            json!({"source": "price_sheet", "content": "two"}),
        )
        .await;
    assert_eq!(status, 507);
    assert_eq!(body["error"]["code"], "quota_exceeded");
    let cookie = h.login().await;
    let (status, _) = h
        .op_post("/v1/operator/propose", &cookie, json!({"policy": NEXT}))
        .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn oversize_body_is_413_json() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), Limits::default()).await;
    let mailer = h.token("mailer");
    let big = "x".repeat(serve::BODY_LIMIT + 1);
    let (status, body) = h
        .post("/v1/policy/check", Some(&mailer), json!({"policy": big}))
        .await;
    assert_eq!(status, 413);
    assert_eq!(body["error"]["code"], "payload_too_large");
}

#[tokio::test]
async fn pure_helpers_match_the_cli_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), Limits::default()).await;
    let t = h.token("mailer");
    let (status, check) = h
        .post("/v1/policy/check", Some(&t), json!({"policy": STARTER}))
        .await;
    assert_eq!((status, &check["ok"]), (200, &json!(true)));
    assert!(check["diagnostics"].is_array());
    let (_, test) = h
        .post("/v1/policy/test", Some(&t), json!({"policy": STARTER}))
        .await;
    assert_eq!(test["ok"], true);
    assert_eq!(test["results"].as_array().unwrap().len(), 2);
    let (_, explain) = h
        .post("/v1/policy/explain", Some(&t), json!({"policy": STARTER}))
        .await;
    assert!(explain["text"].is_string());
    let (_, diff) = h
        .post("/v1/policy/diff", Some(&t), json!({"new": NEXT}))
        .await;
    assert_eq!(diff["ok"], true);
    assert!(diff["changes"].as_array().unwrap().len() > 1);
    let (status, sim) = h
        .post(
            "/v1/policy/simulate",
            Some(&t),
            json!({"policy": STARTER, "count": 2, "steps": 40}),
        )
        .await;
    assert_eq!(status, 200, "{sim}");
    assert_eq!(sim["ok"], true);
    assert_eq!(sim["count"], 2);
    let (status, _) = h
        .post(
            "/v1/policy/simulate",
            Some(&t),
            json!({"policy": STARTER, "count": 9}),
        )
        .await;
    assert_eq!(status, 422);
    let (status, invalid) = h
        .post("/v1/policy/check", Some(&t), json!({"policy": "nope"}))
        .await;
    assert_eq!((status, &invalid["ok"]), (200, &json!(false)));
}

/// A journal whose next append fails when the flag is set.
struct Flaky {
    inner: MemoryJournal,
    fail: Arc<AtomicBool>,
}

impl Journal for Flaky {
    fn load(&mut self) -> Result<Vec<Record>, JournalError> {
        self.inner.load()
    }
    fn append(&mut self, record: &Record) -> Result<(), JournalError> {
        if self.fail.swap(false, Ordering::SeqCst) {
            self.inner.fail_next_append();
        }
        self.inner.append(record)
    }
}

impl ServeJournal for Flaky {
    fn size(&self) -> Option<u64> {
        self.inner.size()
    }
}

#[tokio::test]
async fn journal_failure_stops_serving_with_503() {
    let dir = tempfile::tempdir().unwrap();
    let fail = Arc::new(AtomicBool::new(false));
    let journal = Flaky {
        inner: MemoryJournal::new(),
        fail: fail.clone(),
    };
    let runtime = Runtime::open(journal, STARTER, 0).unwrap();
    let state = dir.path().to_path_buf();
    let h = launch(move |port| {
        Server::new(runtime, settings(port, &state, Limits::default())).unwrap()
    })
    .await;
    let mailer = h.token("mailer");
    let cookie = h.login().await;
    h.source(&mailer, "price_sheet", "one").await;
    fail.store(true, Ordering::SeqCst);
    let (status, body) = h
        .post(
            "/v1/sources",
            Some(&mailer),
            json!({"source": "price_sheet", "content": "two"}),
        )
        .await;
    assert_eq!(status, 503);
    assert_eq!(body["error"]["code"], "restart_required");
    for (status, body) in [
        h.get("/v1/policy", Some(&mailer)).await,
        h.op_post("/v1/operator/propose", &cookie, json!({"policy": NEXT}))
            .await,
        h.get("/", None).await,
    ] {
        assert_eq!(status, 503);
        assert_eq!(body["error"]["code"], "restart_required");
    }
}

impl<J: ServeJournal> Harness<J> {
    /// One JSON-RPC request to `/mcp`; the status and the decoded reply.
    async fn mcp(&self, token: Option<&str>, method: &str, params: Value) -> (u16, Value) {
        let mut request = self
            .client
            .post(format!("{}/mcp", self.base))
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-06-18")
            .json(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
        if let Some(t) = token {
            request = request.bearer_auth(t);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        // A JSON body, or the last `data:` line of an event stream.
        let body = serde_json::from_str(&text).unwrap_or_else(|_| {
            text.lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .filter_map(|d| serde_json::from_str::<Value>(d.trim()).ok())
                .next_back()
                .unwrap_or(Value::Null)
        });
        (status, body)
    }

    async fn tool(&self, token: &str, name: &str, arguments: Value) -> Value {
        let (status, reply) = self
            .mcp(
                Some(token),
                "tools/call",
                json!({"name": name, "arguments": arguments}),
            )
            .await;
        assert_eq!(status, 200, "{reply}");
        reply["result"].clone()
    }
}

#[tokio::test]
async fn mcp_tools_share_the_api_and_its_identity() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), Limits::default()).await;
    let mailer = h.token("mailer");

    let (status, init) = h
        .mcp(
            Some(&mailer),
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"},
            }),
        )
        .await;
    assert_eq!(status, 200, "{init}");
    assert_eq!(init["result"]["serverInfo"]["name"], "trustgraph");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("act only on allow")
    );
    let (status, listed) = h.mcp(Some(&mailer), "tools/list", json!({})).await;
    assert_eq!(status, 200, "{listed}");
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, serve::mcp::TOOLS);

    // Without a token, or with a wrong one, nothing reaches the tools.
    for token in [None, Some("0".repeat(64))] {
        let (status, _) = h.mcp(token.as_deref(), "tools/list", json!({})).await;
        assert_eq!(status, 401);
    }

    let sheet = h
        .tool(
            &mailer,
            "register_source",
            json!({"source": "price_sheet", "content": "prices"}),
        )
        .await;
    assert_eq!(sheet["isError"], false, "{sheet}");
    let sheet = sheet["structuredContent"]["artifact"]
        .as_str()
        .unwrap()
        .to_string();
    let output = h
        .tool(
            &mailer,
            "record_output",
            json!({"inputs": [sheet], "step": "draft", "content": "quote"}),
        )
        .await;
    let output = output["structuredContent"]["artifact"]
        .as_str()
        .unwrap()
        .to_string();
    let via_mcp = h
        .tool(
            &mailer,
            "check",
            json!({"artifact": output, "action": "email_customer"}),
        )
        .await;
    let via_http = h.check(&mailer, &output, "email_customer").await;
    assert_eq!(via_mcp["structuredContent"], via_http, "same decision");
    assert_eq!(via_http["verdict"], "needs_approval");

    // No argument can name another agent, and no tool approves.
    let spoof = h
        .tool(
            &mailer,
            "check",
            json!({"artifact": output, "action": "email_customer", "agent": "planner"}),
        )
        .await;
    assert_eq!(spoof["isError"], true);
    assert_eq!(
        spoof["structuredContent"]["error"]["code"],
        "invalid_request"
    );
    for name in ["approve", "revoke", "activate", "policy_activate"] {
        let reply = h
            .tool(&mailer, name, json!({"decision": via_http["decision"]}))
            .await;
        assert_eq!(reply["isError"], true, "{name}");
    }
    let still = h.check(&mailer, &output, "email_customer").await;
    assert_eq!(still["verdict"], "needs_approval");

    // The planner's tools act as the planner.
    let planner = h.token("planner");
    let denied = h
        .tool(
            &planner,
            "register_source",
            json!({"source": "price_sheet", "content": "p"}),
        )
        .await;
    assert_eq!(
        denied["structuredContent"]["error"]["code"],
        "agent_cannot_use_source"
    );

    let proposed = h
        .tool(&planner, "policy_propose", json!({"policy": NEXT}))
        .await;
    assert_eq!(proposed["isError"], false, "{proposed}");
    let cookie = h.login().await;
    assert_eq!(h.op_state(&cookie).await["proposals"][0]["from"], "planner");
    let (_, policy) = h.get("/v1/policy", Some(&planner)).await;
    assert_eq!(policy["revision"], 1);

    let simulated = h
        .tool(
            &planner,
            "policy_simulate",
            json!({"policy": STARTER, "count": 1, "steps": 20}),
        )
        .await;
    assert_eq!(simulated["structuredContent"]["ok"], true, "{simulated}");
}
