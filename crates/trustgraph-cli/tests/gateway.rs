//! `trustgraph gateway` end to end over real loopback HTTP, with fake
//! upstream MCP servers (the `fake_upstream` example): `drive` over
//! streamable HTTP and `slack` over stdio.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, JsonObject};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;
use trustgraph_cli::serve::gateway::{self, Gateway, config};
use trustgraph_cli::serve::{self, Limits, ServeJournal, Server, Settings};
use trustgraph_engine::{FileJournal, Journal, JournalError, MemoryJournal, Record, Runtime};

const OPERATOR: &str = "nina";
const DRIVE_SECRET: &str = "drive-credential-5f0c2b9e71";
const SLACK_SECRET: &str = "slack-credential-a83d41c6e2";

const POLICY: &str = "version: 1
groups:
  hr: [nina]
  engineering: [sam, ravi]
  leadership: [maya]
agents: [researcher, planner, scout]
sources:
  salaries: { readers: [hr], agents: [researcher, planner] }
  roadmap: { readers: [hr, engineering, leadership], agents: [planner] }
  benchmarks: { readers: [hr, engineering, leadership], agents: [researcher, planner, scout] }
  unclassified: { readers: [hr], agents: [researcher, planner, scout] }
  unmatched: { readers: [hr], agents: [planner] }
actions:
  post_to_engineering: { agents: [planner, scout], audience: [engineering] }
  email_staff: { agents: [planner], audience: [engineering] }
  brief_leadership:
    agents: [planner]
    audience: [leadership]
    approval: { reviewers: [hr], valid_for: 1h }
";

/// The fake upstream binary, built by `cargo test` with the examples.
fn fake_upstream() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().unwrap().parent().unwrap();
    let path = debug.join("examples").join("fake_upstream");
    if !path.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = std::process::Command::new(cargo)
            .args([
                "build",
                "-p",
                "trustgraph-cli",
                "--example",
                "fake_upstream",
            ])
            .status()
            .unwrap();
        assert!(status.success(), "cannot build the fake upstream");
    }
    path
}

/// The `drive` upstream: the fake over streamable HTTP, needing DRIVE_SECRET.
struct Drive {
    _child: tokio::process::Child,
    url: String,
    log: PathBuf,
}

async fn drive(dir: &Path) -> Drive {
    let log = dir.join("drive.log");
    let mut child = tokio::process::Command::new(fake_upstream())
        .args(["--http", "127.0.0.1:0"])
        .env("FAKE_UPSTREAM_TOKEN", DRIVE_SECRET)
        .env("FAKE_UPSTREAM_LOG", &log)
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let url = line.strip_prefix("listening ").unwrap().to_string();
    Drive {
        _child: child,
        url,
        log,
    }
}

fn config_text(drive: &str, slack_log: &Path, session: &str) -> String {
    format!(
        r##"version: 1
session: {session}
timeout_seconds: 1
max_result_bytes: 65536
upstreams:
  drive:
    url: {drive}
    headers: {{ Authorization: "Bearer ${{env:DRIVE_TOKEN}}" }}
  slack:
    command: [{fake}, --stdio]
    env:
      SLACK_BOT_TOKEN: "${{env:SLACK_TOKEN}}"
      FAKE_UPSTREAM_LOG: {log}
  gone:
    url: http://127.0.0.1:9/mcp
tools:
  drive.read_file:
    reads:
      - when: {{ path: "HR/Compensation/**" }}
        source: salaries
      - when: {{ path: "Product/**" }}
        source: roadmap
      - when: {{ path: "Public/*" }}
        source: benchmarks
    otherwise: unmatched
  drive.slow_read:
    reads:
      - when: {{ path: "Product/**" }}
        source: roadmap
    otherwise: unmatched
  drive.search:
    passthrough: true
    arguments: [query]
  drive.slow:
    passthrough: true
    untracked: true
  drive.big:
    passthrough: true
    untracked: true
  slack.post_message:
    arguments: [channel, text, thread]
    acts:
      - when: {{ channel: "#engineering" }}
        action: post_to_engineering
        content: text
      - when: {{ channel: "#leadership" }}
        action: brief_leadership
        content: text
  slack.send_email:
    arguments: [to, subject, body]
    acts:
      - when: {{ to: {{ pattern: "*@corp.com", type: email }} }}
        action: email_staff
        content: body
  slack.env_names:
    passthrough: true
    untracked: true
  slack.env_home:
    passthrough: true
    untracked: true
  slack.leak:
    passthrough: true
    untracked: true
  drive.huge_schema:
    passthrough: true
    untracked: true
  gone.read_file:
    passthrough: true
    untracked: true
    arguments: [path]
"##,
        fake = fake_upstream().display(),
        log = slack_log.display(),
    )
}

const HUGE_SCHEMA_TOOL: &str = "  drive.huge_schema:\n    passthrough: true\n    untracked: true\n";

fn env(name: &str) -> Option<String> {
    match name {
        "DRIVE_TOKEN" => Some(DRIVE_SECRET.into()),
        "SLACK_TOKEN" => Some(SLACK_SECRET.into()),
        _ => None,
    }
}

/// Lines of an upstream call log naming `tool`.
fn calls(log: &Path, tool: &str) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.split(' ').next() == Some(tool))
        .map(str::to_string)
        .collect()
}

struct Gw<J: ServeJournal> {
    gateway: Arc<Gateway<J>>,
    base: String,
    state: PathBuf,
    task: tokio::task::JoinHandle<()>,
    http: reqwest::Client,
    slack_log: PathBuf,
    drive: Drive,
    /// Every tool result an agent saw, as JSON text.
    seen: Mutex<Vec<String>>,
}

fn settings(port: u16, state: &Path, limits: Limits) -> Settings {
    Settings {
        operator: OPERATOR.into(),
        port,
        state_dir: state.to_path_buf(),
        limits,
    }
}

async fn launch<J: ServeJournal>(
    dir: &Path,
    session: &str,
    make: impl FnOnce(u16) -> Arc<Server<J>>,
) -> Gw<J> {
    launch_with(dir, session, make, |text| text).await
}

async fn launch_with<J: ServeJournal>(
    dir: &Path,
    session: &str,
    make: impl FnOnce(u16) -> Arc<Server<J>>,
    edit: impl FnOnce(String) -> String,
) -> Gw<J> {
    let drive = drive(dir).await;
    let slack_log = dir.join("slack.log");
    let config = config::parse(&edit(config_text(&drive.url, &slack_log, session))).unwrap();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = make(port);
    let gateway = gateway::attach(server.clone(), config, &env).unwrap();
    let app = gateway::router(gateway.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Gw {
        state: server.settings().state_dir.clone(),
        gateway,
        base: format!("http://127.0.0.1:{port}"),
        task,
        http: reqwest::Client::builder().no_proxy().build().unwrap(),
        slack_log,
        drive,
        seen: Mutex::new(Vec::new()),
    }
}

async fn memory(dir: &Path, session: &str) -> Gw<MemoryJournal> {
    memory_with(dir, session, Limits::default()).await
}

async fn memory_with(dir: &Path, session: &str, limits: Limits) -> Gw<MemoryJournal> {
    let state = dir.join("state");
    launch(dir, session, move |port| {
        let runtime = Runtime::open(MemoryJournal::new(), POLICY, 0).unwrap();
        Server::new(runtime, settings(port, &state, limits)).unwrap()
    })
    .await
}

async fn on_disk(dir: &Path, session: &str) -> Gw<FileJournal> {
    let state = dir.join("state");
    launch(dir, session, move |port| {
        std::fs::create_dir_all(&state).unwrap();
        let journal = FileJournal::open(&state).unwrap();
        let runtime = Runtime::open(journal, POLICY, 1).unwrap();
        Server::new(runtime, settings(port, &state, Limits::default())).unwrap()
    })
    .await
}

/// One agent's MCP connection to the gateway.
struct Agent {
    client: RunningService<RoleClient, ()>,
}

impl<J: ServeJournal> Gw<J> {
    fn token(&self, agent: &str) -> String {
        std::fs::read_to_string(self.state.join("tokens").join(format!("{agent}.token"))).unwrap()
    }

    async fn connect(&self, agent: &str) -> Agent {
        let config =
            StreamableHttpClientTransportConfig::with_uri(format!("{}/gateway/mcp", self.base))
                .auth_header(self.token(agent));
        let client = ().serve(StreamableHttpClientTransport::from_config(config)).await;
        Agent {
            client: client.unwrap(),
        }
    }

    /// Call a tool; the result as JSON (`isError`, `content`, `structuredContent`).
    async fn call(&self, agent: &Agent, name: &str, arguments: Value) -> Value {
        let result = agent
            .client
            .peer()
            .call_tool(
                CallToolRequestParams::new(name.to_string()).with_arguments(
                    arguments
                        .as_object()
                        .cloned()
                        .unwrap_or_else(JsonObject::new),
                ),
            )
            .await
            .unwrap();
        let value = serde_json::to_value(&result).unwrap();
        self.seen.lock().unwrap().push(value.to_string());
        value
    }

    async fn read(&self, agent: &Agent, path: &str) -> Value {
        self.call(agent, "drive__read_file", json!({"path": path}))
            .await
    }

    async fn post(&self, agent: &Agent, channel: &str, text: &str) -> Value {
        self.call(
            agent,
            "slack__post_message",
            json!({"channel": channel, "text": text}),
        )
        .await
    }

    async fn login(&self) -> String {
        let secret = self.gateway.server().launch_secret().unwrap();
        let response = self
            .http
            .post(format!("{}/session", self.base))
            .bearer_auth(secret)
            .send()
            .await
            .unwrap();
        let body: Value = response.json().await.unwrap();
        body["session"].as_str().unwrap().to_string()
    }

    async fn op_state(&self, session: &str) -> Value {
        self.http
            .get(format!("{}/v1/operator/state", self.base))
            .header("x-trustgraph-operator", session)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn op_post(&self, path: &str, session: &str, body: Value) -> (u16, Value) {
        let response = self
            .http
            .post(format!("{}{path}", self.base))
            .header("x-trustgraph-operator", session)
            .json(&body)
            .send()
            .await
            .unwrap();
        (
            response.status().as_u16(),
            response.json().await.unwrap_or(Value::Null),
        )
    }

    /// The sources the operator page says each live session carries.
    async fn sessions(&self, operator: &str) -> Vec<(String, String, Vec<String>)> {
        let state = self.op_state(operator).await;
        state["gateway"]["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                let sources = s["sources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_string())
                    .collect();
                (
                    s["session"].as_str().unwrap().to_string(),
                    s["agent"].as_str().unwrap().to_string(),
                    sources,
                )
            })
            .collect()
    }
}

fn text(result: &Value) -> String {
    result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn error_code(result: &Value) -> String {
    assert_eq!(result["isError"], true, "expected a tool error: {result}");
    result["structuredContent"]["error"]["code"]
        .as_str()
        .unwrap()
        .to_string()
}

fn ok(result: &Value) -> String {
    assert_ne!(result["isError"], true, "expected success: {result}");
    text(result)
}

// ----- the hand-off story -----

#[tokio::test]
async fn a_handed_off_salary_read_blocks_the_planners_post() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;

    let researcher = h.connect("researcher").await;
    let salaries = h.read(&researcher, "HR/Compensation/salaries.csv").await;
    assert_eq!(ok(&salaries), "nina: 120k, sam: 95k");
    let handoff = h.call(&researcher, gateway::HANDOFF_TOOL, json!({})).await;
    ok(&handoff);
    let token = handoff["structuredContent"]["handoff"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(handoff["structuredContent"]["carries"], json!(["salaries"]));

    let planner = h.connect("planner").await;
    let received = h
        .call(&planner, gateway::RECEIVE_TOOL, json!({"token": token}))
        .await;
    assert_eq!(
        received["structuredContent"]["carries"],
        json!(["salaries"])
    );
    let roadmap = h.read(&planner, "Product/roadmap.md").await;
    assert_eq!(ok(&roadmap), "Q3: ship the new billing page");
    let post = h
        .post(&planner, "#engineering", "Q3 plan and pay bands")
        .await;
    assert_eq!(error_code(&post), "denied");
    let message = text(&post);
    assert!(message.contains("salaries"), "{message}");
    assert!(message.contains("not forwarded"), "{message}");
    assert!(
        calls(&h.slack_log, "post_message").is_empty(),
        "the upstream post_message was never called"
    );
}

#[tokio::test]
async fn a_roadmap_only_post_is_forwarded_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let post = h.post(&planner, "#engineering", "Q3: billing page").await;
    assert_eq!(ok(&post), "posted to #engineering");
    let posted = calls(&h.slack_log, "post_message");
    assert_eq!(posted.len(), 1, "{posted:?}");
    assert!(posted[0].contains("Q3: billing page"));
}

#[tokio::test]
async fn a_held_action_is_forwarded_on_retry_after_approval() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let held = h.post(&planner, "#leadership", "Q3 plan").await;
    assert_eq!(error_code(&held), "needs_approval");
    let decision = held["structuredContent"]["error"]["decision"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(text(&held).contains("held for approval"), "{held}");
    assert!(calls(&h.slack_log, "post_message").is_empty());

    let operator = h.login().await;
    let state = h.op_state(&operator).await;
    assert_eq!(state["pending"][0]["decision"], decision.as_str());
    let (status, body) = h
        .op_post(
            "/v1/operator/approve",
            &operator,
            json!({"decision": decision}),
        )
        .await;
    assert_eq!(status, 200, "{body}");

    let retry = h.post(&planner, "#leadership", "Q3 plan").await;
    assert_eq!(ok(&retry), "posted to #leadership");
    assert_eq!(calls(&h.slack_log, "post_message").len(), 1);
    // A different text is a different output: it needs its own approval.
    let other = h.post(&planner, "#leadership", "Q4 plan").await;
    assert_eq!(error_code(&other), "needs_approval");
    assert_eq!(calls(&h.slack_log, "post_message").len(), 1);
}

#[tokio::test]
async fn nothing_read_means_no_action() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    let post = h.post(&planner, "#engineering", "hello").await;
    assert_eq!(error_code(&post), "nothing_read");
    let handoff = h.call(&planner, gateway::HANDOFF_TOOL, json!({})).await;
    assert_eq!(error_code(&handoff), "nothing_read");
    assert!(calls(&h.slack_log, "post_message").is_empty());
}

#[tokio::test]
async fn allow_without_reads_forwards_unchecked() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let h = launch_with(
        dir.path(),
        "agent",
        move |port| {
            let runtime = Runtime::open(MemoryJournal::new(), POLICY, 0).unwrap();
            Server::new(runtime, settings(port, &state, Limits::default())).unwrap()
        },
        |text| {
            text.replace(
                "  slack.send_email:",
                "    allow_without_reads: true\n  slack.send_email:",
            )
        },
    )
    .await;
    let planner = h.connect("planner").await;
    let post = h.post(&planner, "#engineering", "hello").await;
    assert_eq!(ok(&post), "posted to #engineering");
    // Once something is read, the check applies again.
    ok(&h.read(&planner, "HR/Compensation/salaries.csv").await);
    let post = h.post(&planner, "#engineering", "pay").await;
    assert_eq!(error_code(&post), "denied");
    assert_eq!(calls(&h.slack_log, "post_message").len(), 1);
}

// ----- what an agent may call -----

#[tokio::test]
async fn an_uncleared_read_and_an_unknown_tool_look_the_same_and_reach_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let researcher = h.connect("researcher").await;

    let listed: Vec<String> = researcher
        .client
        .peer()
        .list_all_tools()
        .await
        .unwrap()
        .iter()
        .map(|t| t.name.to_string())
        .collect();
    let mut expected = vec![
        "trustgraph__handoff",
        "trustgraph__receive",
        "drive__big",
        "drive__read_file",
        "drive__search",
        "drive__slow",
        "drive__slow_read",
        "slack__env_home",
        "slack__env_names",
        "slack__leak",
        "slack__post_message",
        "slack__send_email",
    ];
    let mut listed_sorted = listed.clone();
    listed_sorted.sort();
    expected.sort();
    assert_eq!(
        listed_sorted, expected,
        "unlisted upstream tools are invisible"
    );

    // Upstream descriptions reach agents without control characters.
    let tools = researcher.client.peer().list_all_tools().await.unwrap();
    let post = tools
        .iter()
        .find(|t| t.name == "slack__post_message")
        .unwrap();
    let description = post.description.as_deref().unwrap();
    assert!(
        !description.chars().any(char::is_control),
        "{description:?}"
    );

    // roadmap is not the researcher's: refused before the upstream is called.
    let uncleared = h.read(&researcher, "Product/roadmap.md").await;
    let unknown = h.call(&researcher, "drive__nope", json!({})).await;
    let unlisted = h
        .call(&researcher, "slack__admin_delete_all", json!({}))
        .await;
    let raw_name = h
        .call(
            &researcher,
            "read_file",
            json!({"path": "Product/roadmap.md"}),
        )
        .await;
    for result in [&uncleared, &unknown, &unlisted, &raw_name] {
        assert_eq!(error_code(result), "tool_not_available");
        assert_eq!(result["structuredContent"], uncleared["structuredContent"]);
    }
    assert!(
        calls(&h.drive.log, "read_file").is_empty(),
        "drive was not called"
    );
    assert!(calls(&h.slack_log, "admin_delete_all").is_empty());
}

#[tokio::test]
async fn when_rules_cannot_be_bypassed() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let operator = h.login().await;
    // Each of these must fall to `otherwise` (unmatched), never to roadmap.
    let attempts = [
        json!("Product/../HR/Compensation/salaries.csv"),
        json!("Product\\..\\HR\\Compensation\\salaries.csv"),
        json!("product/roadmap.md"),
        json!("PRODUCT/roadmap.md"),
        json!("Product/road\u{0}map.md"),
        json!("Product/\nroadmap.md"),
        json!(["Product/roadmap.md", "HR/Compensation/salaries.csv"]),
        json!([]),
        json!(5),
        json!({"path": "Product/roadmap.md"}),
    ];
    for path in attempts {
        let planner = h.connect("planner").await;
        h.call(&planner, "drive__read_file", json!({ "path": path }))
            .await;
        let carried: Vec<Vec<String>> = h
            .sessions(&operator)
            .await
            .into_iter()
            .filter(|(_, agent, sources)| agent == "planner" && !sources.is_empty())
            .map(|(_, _, sources)| sources)
            .collect();
        assert!(
            carried.iter().all(|s| s == &vec!["unmatched".to_string()]),
            "{path}: {carried:?}"
        );
        planner.client.cancel().await.unwrap();
    }
    // An action call that matches no rule is blocked, never forwarded.
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    for channel in ["#Engineering", "#engineering\n", "#eng"] {
        let post = h.post(&planner, channel, "hi").await;
        assert_eq!(error_code(&post), "blocked", "{channel:?}");
    }
    let missing = h
        .call(
            &planner,
            "slack__post_message",
            json!({"channel": "#engineering"}),
        )
        .await;
    assert_eq!(error_code(&missing), "invalid_arguments");
    assert!(calls(&h.slack_log, "post_message").is_empty());
}

#[tokio::test]
async fn passthrough_results_are_unclassified_reads() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let operator = h.login().await;
    let planner = h.connect("planner").await;
    ok(&h
        .call(&planner, "drive__search", json!({"query": "x"}))
        .await);
    let sessions = h.sessions(&operator).await;
    assert_eq!(sessions[0].2, vec!["unclassified"]);
    // unclassified is HR-only, so a post built on it is denied.
    let post = h.post(&planner, "#engineering", "results").await;
    assert_eq!(error_code(&post), "denied");
}

// ----- failures -----

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
async fn a_read_that_cannot_be_recorded_is_withheld() {
    let dir = tempfile::tempdir().unwrap();
    let fail = Arc::new(AtomicBool::new(false));
    let state = dir.path().join("state");
    let flag = fail.clone();
    let h = launch(dir.path(), "connection", move |port| {
        let journal = Flaky {
            inner: MemoryJournal::new(),
            fail: flag,
        };
        let runtime = Runtime::open(journal, POLICY, 0).unwrap();
        Server::new(runtime, settings(port, &state, Limits::default())).unwrap()
    })
    .await;
    let planner = h.connect("planner").await;
    fail.store(true, Ordering::SeqCst);
    let result = h.read(&planner, "HR/Compensation/salaries.csv").await;
    assert_eq!(error_code(&result), "not_recorded");
    assert!(!result.to_string().contains("120k"), "{result}");
    assert_eq!(
        calls(&h.drive.log, "read_file").len(),
        1,
        "the upstream answered; the gateway withheld it"
    );
}

#[tokio::test]
async fn upstream_failures_fail_closed_and_the_gateway_keeps_serving() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    let slow = h.call(&planner, "drive__slow", json!({})).await;
    assert_eq!(error_code(&slow), "upstream_timeout");
    let big = h.call(&planner, "drive__big", json!({})).await;
    assert_eq!(error_code(&big), "upstream_result_withheld");
    let gone = h
        .call(&planner, "gone__read_file", json!({"path": "x"}))
        .await;
    assert_eq!(error_code(&gone), "upstream_unreachable", "{gone}");
    assert_eq!(
        text(&gone),
        "TrustGraph gateway: the upstream `gone` cannot be reached (upstream_unreachable)"
    );
    // Still serving.
    assert_eq!(
        ok(&h.read(&planner, "Product/roadmap.md").await),
        "Q3: ship the new billing page"
    );
    let operator = h.login().await;
    assert_eq!(h.sessions(&operator).await[0].2, vec!["roadmap"]);
}

// ----- hand-off tokens -----

#[derive(Default)]
struct TestClock {
    wall: AtomicU64,
    monotonic: AtomicU64,
}

impl serve::Clock for TestClock {
    fn wall(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn monotonic(&self) -> Duration {
        Duration::from_secs(self.monotonic.load(Ordering::SeqCst))
    }
}

#[tokio::test]
async fn hand_off_tokens_cannot_be_forged_edited_reused_late_or_taken_by_the_uncleared() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(TestClock::default());
    let state = dir.path().join("state");
    let shared = clock.clone();
    let h = launch(dir.path(), "connection", move |port| {
        let runtime = Runtime::open(MemoryJournal::new(), POLICY, 0).unwrap();
        Server::with_clock(runtime, settings(port, &state, Limits::default()), shared).unwrap()
    })
    .await;
    let researcher = h.connect("researcher").await;
    ok(&h.read(&researcher, "HR/Compensation/salaries.csv").await);
    let handoff = h.call(&researcher, gateway::HANDOFF_TOOL, json!({})).await;
    let token = handoff["structuredContent"]["handoff"]
        .as_str()
        .unwrap()
        .to_string();

    let planner = h.connect("planner").await;
    let mut edited = token.clone().into_bytes();
    let i = 10;
    edited[i] = if edited[i] == b'0' { b'1' } else { b'0' };
    let edited = String::from_utf8(edited).unwrap();
    let forged = format!("tgh1.{}.{}", "7b7d".repeat(4), "00".repeat(32));
    for bad in [
        edited.as_str(),
        forged.as_str(),
        "",
        "tgh1..",
        "not a token",
    ] {
        let result = h
            .call(&planner, gateway::RECEIVE_TOOL, json!({"token": bad}))
            .await;
        assert_eq!(error_code(&result), "invalid_handoff", "{bad}");
    }

    // The uncleared: scout may not use salaries.
    let scout = h.connect("scout").await;
    let refused = h
        .call(&scout, gateway::RECEIVE_TOOL, json!({"token": token}))
        .await;
    assert_eq!(error_code(&refused), "not_cleared");
    assert!(text(&refused).contains("salaries"));
    let operator = h.login().await;
    assert!(
        h.sessions(&operator)
            .await
            .iter()
            .all(|(_, agent, sources)| agent != "scout" || sources.is_empty()),
        "nothing was added to the scout's session"
    );

    // Valid now; not after 24 hours.
    clock.monotonic.store(24 * 60 * 60 - 1, Ordering::SeqCst);
    let fine = h
        .call(&planner, gateway::RECEIVE_TOOL, json!({"token": token}))
        .await;
    ok(&fine);
    clock.monotonic.store(24 * 60 * 60 + 1, Ordering::SeqCst);
    let late_planner = h.connect("planner").await;
    let late = h
        .call(
            &late_planner,
            gateway::RECEIVE_TOOL,
            json!({"token": token}),
        )
        .await;
    assert_eq!(error_code(&late), "invalid_handoff");
}

// ----- sessions -----

#[tokio::test]
async fn connection_sessions_are_per_connection_and_end_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let h = on_disk(dir.path(), "connection").await;
    let operator = h.login().await;
    let first = h.connect("planner").await;
    ok(&h.read(&first, "HR/Compensation/salaries.csv").await);
    let second = h.connect("planner").await;
    ok(&h.read(&second, "Product/roadmap.md").await);
    assert_eq!(
        ok(&h.post(&second, "#engineering", "roadmap").await),
        "posted to #engineering",
        "the second connection did not read salaries"
    );
    assert_eq!(
        error_code(&h.post(&first, "#engineering", "pay").await),
        "denied"
    );
    assert_eq!(h.sessions(&operator).await.len(), 2);

    // A closed connection ends its session.
    second.client.cancel().await.unwrap();
    let mut left = h.sessions(&operator).await;
    for _ in 0..50 {
        if left.len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        left = h.sessions(&operator).await;
    }
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].2, vec!["salaries"]);

    first.client.cancel().await.unwrap();
    let state = h.state.clone();
    let h = restart(h, dir.path(), "connection").await;
    assert_eq!(h.state, state);
    let operator = h.login().await;
    assert!(h.sessions(&operator).await.is_empty(), "restart ends them");
}

#[tokio::test]
async fn agent_sessions_survive_reconnects_and_restarts_until_the_operator_ends_them() {
    let dir = tempfile::tempdir().unwrap();
    let h = on_disk(dir.path(), "agent").await;
    let first = h.connect("planner").await;
    ok(&h.read(&first, "HR/Compensation/salaries.csv").await);
    first.client.cancel().await.unwrap();

    let second = h.connect("planner").await;
    ok(&h.read(&second, "Product/roadmap.md").await);
    assert_eq!(
        error_code(&h.post(&second, "#engineering", "roadmap").await),
        "denied",
        "a reconnect does not shed what was read"
    );
    second.client.cancel().await.unwrap();

    let h = restart(h, dir.path(), "agent").await;
    let operator = h.login().await;
    let sessions = h.sessions(&operator).await;
    assert_eq!(
        sessions,
        vec![(
            "agent:planner".to_string(),
            "planner".to_string(),
            vec!["roadmap".to_string(), "salaries".to_string()]
        )]
    );
    let third = h.connect("planner").await;
    assert_eq!(
        error_code(&h.post(&third, "#engineering", "roadmap").await),
        "denied",
        "a restart does not shed it either"
    );

    // The agent has no way to end its own session.
    for name in [
        "trustgraph__reset",
        "trustgraph__end_session",
        "end_session",
    ] {
        let result = h.call(&third, name, json!({})).await;
        assert_eq!(error_code(&result), "tool_not_available");
    }
    let token = h.token("planner");
    let agent_try = h
        .http
        .post(format!("{}/v1/operator/end-session", h.base))
        .bearer_auth(&token)
        .json(&json!({"session": "agent:planner"}))
        .send()
        .await
        .unwrap();
    assert_eq!(agent_try.status().as_u16(), 401);

    // The operator can.
    let (status, body) = h
        .op_post(
            "/v1/operator/end-session",
            &operator,
            json!({"session": "agent:planner"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(h.sessions(&operator).await.is_empty());
    // Ending it closed the connection: the client initialises again.
    let fourth = h.connect("planner").await;
    ok(&h.read(&fourth, "Product/roadmap.md").await);
    assert_eq!(
        ok(&h.post(&fourth, "#engineering", "roadmap").await),
        "posted to #engineering"
    );
    let (status, _) = h
        .op_post(
            "/v1/operator/end-session",
            &operator,
            json!({"session": "agent:nobody"}),
        )
        .await;
    assert_eq!(status, 404);
}

/// Stop `h` (every connection must be closed) and start again on the same
/// state directory.
async fn restart(h: Gw<FileJournal>, dir: &Path, session: &str) -> Gw<FileJournal> {
    let Gw { gateway, task, .. } = h;
    task.abort();
    let _ = task.await;
    let server = gateway.server().clone();
    drop(gateway);
    for _ in 0..100 {
        if Arc::strong_count(&server) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        Arc::strong_count(&server),
        1,
        "the old server is still held"
    );
    drop(server);
    on_disk(dir, session).await
}

#[tokio::test]
async fn another_agent_cannot_use_a_connection_it_did_not_open() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let operator = h.login().await;
    let session = h.sessions(&operator).await[0].0.clone();
    assert!(session.starts_with("conn:"));
    // Open a session as the planner over raw HTTP, then present it with the scout's token.
    let init = h
        .http
        .post(format!("{}/gateway/mcp", h.base))
        .bearer_auth(h.token("planner"))
        .header("accept", "application/json, text/event-stream")
        .json(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "t", "version": "0"}}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(init.status().as_u16(), 200);
    let mcp = init.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    let stolen = h
        .http
        .post(format!("{}/gateway/mcp", h.base))
        .bearer_auth(h.token("scout"))
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &mcp)
        .header("mcp-protocol-version", "2025-06-18")
        .json(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}))
        .send()
        .await
        .unwrap();
    assert_eq!(stolen.status().as_u16(), 404);
}

#[tokio::test]
async fn gateway_calls_spend_the_agents_rate_limit() {
    let dir = tempfile::tempdir().unwrap();
    let limits = Limits {
        rate_per_second: 0.001,
        burst: 6.0,
        ..Limits::default()
    };
    let h = memory_with(dir.path(), "connection", limits).await;
    let token = h.token("planner");
    let mut statuses = Vec::new();
    for _ in 0..10 {
        let response = h
            .http
            .post(format!("{}/gateway/mcp", h.base))
            .bearer_auth(&token)
            .header("accept", "application/json, text/event-stream")
            .json(
                &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "t", "version": "0"}}}),
            )
            .send()
            .await
            .unwrap();
        statuses.push(response.status().as_u16());
    }
    assert_eq!(&statuses[..6], &[200; 6]);
    assert!(statuses[6..].iter().all(|s| *s == 429), "{statuses:?}");
    // Another agent's budget is its own.
    let scout = h.connect("scout").await;
    ok(&h.read(&scout, "Public/benchmarks.md").await);
}

#[tokio::test]
async fn the_operator_page_shows_closed_tools_after_a_narrowing_activation() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let operator = h.login().await;
    let state = h.op_state(&operator).await;
    assert_eq!(state["gateway"]["mode"], "connection");
    assert_eq!(state["gateway"]["problems"], json!([]));
    // A policy without brief_leadership.
    let narrowed = POLICY.replace(
        "  brief_leadership:
    agents: [planner]
    audience: [leadership]
    approval: { reviewers: [hr], valid_for: 1h }
",
        "",
    );
    let (status, proposed) = h
        .op_post(
            "/v1/operator/propose",
            &operator,
            json!({"policy": narrowed}),
        )
        .await;
    assert_eq!(status, 200, "{proposed}");
    let (status, body) = h
        .op_post(
            "/v1/operator/activate",
            &operator,
            json!({"proposal": proposed["proposal"]}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let state = h.op_state(&operator).await;
    let problems = state["gateway"]["problems"].to_string();
    assert!(problems.contains("brief_leadership"), "{problems}");
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let post = h.post(&planner, "#engineering", "roadmap").await;
    assert_eq!(error_code(&post), "tool_misconfigured");
    assert!(calls(&h.slack_log, "post_message").is_empty());
}

// ----- credentials -----

/// Every file under `dir`, as text.
fn all_files(dir: &Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&all_files(&path));
        } else {
            out.push_str(&String::from_utf8_lossy(&std::fs::read(&path).unwrap()));
        }
    }
    out
}

#[tokio::test]
async fn upstream_credentials_never_reach_agents_logs_or_the_journal() {
    let dir = tempfile::tempdir().unwrap();
    let drive = drive(dir.path()).await;
    let slack_log = dir.path().join("slack.log");
    let config_path = dir.path().join("gateway.yaml");
    // The real binary refuses to start when an upstream tool's schema is
    // over the cap, so leave that tool out here.
    std::fs::write(
        &config_path,
        config_text(&drive.url, &slack_log, "connection").replace(HUGE_SCHEMA_TOOL, ""),
    )
    .unwrap();
    let policy_path = dir.path().join("policy.yaml");
    std::fs::write(&policy_path, POLICY).unwrap();
    let state = dir.path().join("state");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"))
        .arg("gateway")
        .args(["--config".as_ref(), config_path.as_os_str()])
        .args(["--policy".as_ref(), policy_path.as_os_str()])
        .args(["--operator", OPERATOR])
        .args(["--state".as_ref(), state.as_os_str()])
        .args(["--port", "0"])
        .env("DRIVE_TOKEN", DRIVE_SECRET)
        .env("SLACK_TOKEN", SLACK_SECRET)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let output = Arc::new(Mutex::new(String::new()));
    let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let mut base = String::new();
    while let Some(line) = stdout.next_line().await.unwrap() {
        output.lock().unwrap().push_str(&format!("{line}\n"));
        if let Some(url) = line.strip_prefix("TrustGraph gateway: ") {
            base = url.to_string();
        }
        if line.starts_with("Sessions:") {
            break;
        }
    }
    assert!(!base.is_empty(), "{}", output.lock().unwrap());
    let sink = output.clone();
    let stderr = child.stderr.take().unwrap();
    let errors = tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            sink.lock().unwrap().push_str(&format!("{line}\n"));
        }
    });

    let token = std::fs::read_to_string(state.join("tokens/planner.token")).unwrap();
    let config = StreamableHttpClientTransportConfig::with_uri(format!("{base}/gateway/mcp"))
        .auth_header(token);
    let client = ().serve(StreamableHttpClientTransport::from_config(config)).await.unwrap();
    let mut seen = Vec::new();
    for (name, arguments) in [
        ("drive__read_file", json!({"path": "Product/roadmap.md"})),
        (
            "drive__read_file",
            json!({"path": "HR/Compensation/salaries.csv"}),
        ),
        ("drive__slow", json!({})),
        ("drive__big", json!({})),
        ("gone__read_file", json!({"path": "x"})),
        ("slack__env_names", json!({})),
        (
            "slack__post_message",
            json!({"channel": "#engineering", "text": "t"}),
        ),
    ] {
        let result = client
            .peer()
            .call_tool(
                CallToolRequestParams::new(name)
                    .with_arguments(arguments.as_object().cloned().unwrap()),
            )
            .await
            .unwrap();
        seen.push(serde_json::to_string(&result).unwrap());
    }
    let tools = client.peer().list_all_tools().await.unwrap();
    seen.push(serde_json::to_string(&tools).unwrap());

    // The stdio upstream got only its own environment.
    let names: Vec<String> = serde_json::from_str::<Value>(&seen[5]).unwrap()["content"][0]["text"]
        .as_str()
        .unwrap()
        .split(',')
        .map(str::to_string)
        .collect();
    for name in &names {
        assert!(
            [
                "PATH",
                "HOME",
                "SLACK_BOT_TOKEN",
                "FAKE_UPSTREAM_LOG",
                "__CF_USER_TEXT_ENCODING"
            ]
            .contains(&name.as_str()),
            "the upstream saw `{name}`: {names:?}"
        );
    }
    assert!(names.contains(&"SLACK_BOT_TOKEN".to_string()));

    client.cancel().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    child.kill().await.unwrap();
    let _ = errors.await;
    let output = output.lock().unwrap().clone();
    assert!(
        output.contains("fake_upstream: starting with credential [redacted]"),
        "the careless upstream's log line is forwarded, redacted: {output}"
    );
    let journal = all_files(&state);
    for secret in [DRIVE_SECRET, SLACK_SECRET] {
        assert!(
            !output.contains(secret),
            "in the gateway's output: {output}"
        );
        assert!(!journal.contains(secret), "in the state directory");
        for result in &seen {
            assert!(!result.contains(secret), "in a result: {result}");
        }
    }
}

// ----- the stdio bridge -----

#[tokio::test]
async fn the_stdio_bridge_relays_to_a_running_gateway() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let token_file = h.state.join("tokens/planner.token");
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"));
    command
        .args(["gateway", "stdio", "--url"])
        .arg(format!("{}/gateway/mcp", h.base))
        .arg("--token-file")
        .arg(&token_file);
    let bridge = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = bridge.peer().list_all_tools().await.unwrap();
    assert!(tools.iter().any(|t| t.name == "drive__read_file"));
    let read = bridge
        .peer()
        .call_tool(
            CallToolRequestParams::new("drive__read_file").with_arguments(
                json!({"path": "Product/roadmap.md"})
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
    let read = serde_json::to_value(&read).unwrap();
    assert_eq!(text(&read), "Q3: ship the new billing page");
    let post = bridge
        .peer()
        .call_tool(
            CallToolRequestParams::new("slack__post_message").with_arguments(
                json!({"channel": "#engineering", "text": "via stdio"})
                    .as_object()
                    .cloned()
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        text(&serde_json::to_value(&post).unwrap()),
        "posted to #engineering"
    );
    bridge.cancel().await.unwrap();
}

// ----- the config -----

#[test]
fn config_errors_are_reported_together_with_paths() {
    let bad = r#"version: 2
session: sometimes
colour: blue
upstreams:
  Drive: { url: "ftp://x" }
  slack: { command: [npx], url: "http://x" }
  docs: { url: "https://docs/${env:TOKEN}" }
  files: { url: "http://f", headers: { Authorization: "Bearer ${TOKEN}" } }
tools:
  nowhere.read: { passthrough: true }
  files.read_file:
    reads:
      - when: { path: "a/**" }
        source: a
  files.post:
    acts:
      - action: post
    otherwise: allow
  files.both: { passthrough: true, reads: [] }
  files.search: { passthrough: false }
"#;
    let problems = config::parse(bad).unwrap_err();
    let paths: Vec<&str> = problems.iter().map(|p| p.path.as_str()).collect();
    for expected in [
        "version",
        "session",
        "colour",
        "upstreams.Drive",
        "upstreams.slack",
        "upstreams.docs.url",
        "upstreams.files.headers.Authorization",
        "tools.nowhere.read",
        "tools.files.read_file.otherwise",
        "tools.files.post.otherwise",
        "tools.files.both",
        "tools.files.search.passthrough",
    ] {
        assert!(
            paths.contains(&expected),
            "{expected} missing from {problems:#?}"
        );
    }
}

#[test]
fn config_is_checked_against_the_policy() {
    let text = "version: 1
upstreams:
  drive: { url: \"http://127.0.0.1:1/mcp\" }
tools:
  drive.read_file:
    reads:
      - when: { path: \"x/**\" }
        source: secrets
    otherwise: salaries
  drive.post:
    arguments: [text]
    acts:
      - action: shout
  drive.search: { passthrough: true }
";
    let config = config::parse(text).unwrap();
    let no_unclassified = POLICY.replace(
        "  unclassified: { readers: [hr], agents: [researcher, planner, scout] }\n",
        "",
    );
    let policy = trustgraph_policy::parse(&no_unclassified).policy.unwrap();
    let problems = config::check_against(&config, &policy);
    let paths: Vec<&str> = problems.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "tools.drive.post.acts[0].action",
            "tools.drive.read_file.reads[0].source",
            "tools.drive.search.passthrough",
        ]
    );
}

#[test]
fn globs_and_matching_fail_closed() {
    assert!(config::glob(
        "HR/Compensation/**",
        "HR/Compensation/a/b.csv"
    ));
    assert!(config::glob("Product/*", "Product/roadmap.md"));
    assert!(!config::glob("Product/*", "Product/a/roadmap.md"));
    assert!(config::glob("Product/**", "Product/a/roadmap.md"));
    assert!(config::glob("#eng?", "#engx"));
    assert!(!config::glob("Product/**", "product/x"));
    assert!(!config::glob("[ab]", "a"), "brackets are literal");
    assert!(config::glob("[ab]", "[ab]"));
    for unsafe_value in [
        json!("Product/../HR"),
        json!("..\\HR"),
        json!("a\u{0}b"),
        json!("a\tb"),
        json!(1),
        json!(null),
        json!("x".repeat(config::MAX_MATCH_BYTES + 1)),
    ] {
        assert!(
            config::safe_for_matching(&unsafe_value).is_none(),
            "{unsafe_value}"
        );
    }
    assert_eq!(
        config::safe_for_matching(&json!("Product/..roadmap")),
        Some("Product/..roadmap")
    );
    // `*`, `?` and `**` never cross list, header or markup separators.
    for (pattern, text) in [
        ("*@corp.com", "x@evil.com, y@corp.com"),
        ("*@corp.com", "x@evil.com;y@corp.com"),
        ("*@corp.com", "x@evil.com y@corp.com"),
        ("*@corp.com", "x@evil.com\ty@corp.com"),
        ("*@corp.com", "\"x@evil.com\"y@corp.com"),
        ("*@corp.com", "<x@evil.com>y@corp.com"),
        ("**@corp.com", "a/x@evil.com,y@corp.com"),
        ("#eng?", "#eng,"),
    ] {
        assert!(!config::glob(pattern, text), "{pattern} {text}");
    }
    assert!(config::glob("**@corp.com", "a/b@corp.com"));
    for odd in [
        "%2e%2e/x",
        "~/x",
        "a\\b",
        "/abs",
        "./x",
        "a//b",
        "a/./b",
        "a/",
        "r\u{f6}admap",
    ] {
        assert!(
            config::safe_path_for_matching(&json!(odd)).is_none(),
            "{odd}"
        );
        assert!(config::safe_for_matching(&json!(odd)).is_some(), "{odd}");
    }
    assert_eq!(
        config::safe_path_for_matching(&json!("Product/roadmap.md")),
        Some("Product/roadmap.md")
    );
}

#[test]
fn when_types_arguments_and_shadowed_rules() {
    let text = "version: 1
session: agent
upstreams:
  drive: { url: \"https://drive.example.com/mcp\" }
tools:
  drive.read_file:
    arguments: [revision]
    reads:
      - when: { path: \"Product/**\" }
        source: roadmap
      - when: { path: \"Product/**\", revision: \"1\" }
        source: benchmarks
    otherwise: unmatched
  drive.post:
    arguments: [to, body]
    acts:
      - when: { to: { glob: \"/srv/*\", type: path } }
        action: post_to_engineering
        content: body
";
    let config = config::parse(text).unwrap();
    let read = &config.tools["drive__read_file"];
    assert_eq!(
        read.arguments
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["path", "revision"],
        "the rules' arguments plus the listed ones"
    );
    let config::Kind::Acts { rules, .. } = &config.tools["drive__post"].kind else {
        panic!()
    };
    assert_eq!(rules[0].when[0].kind, config::MatchType::Path);
    let warnings = config::warnings(&config);
    let paths: Vec<&str> = warnings.iter().map(|w| w.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "tools.drive.post.arguments",
            "tools.drive.read_file.reads[1]"
        ],
        "{warnings:#?}"
    );
    assert!(warnings[0].message.contains("body"), "{}", warnings[0]);
    // A path-typed argument with a leading `/` never matches.
    let args = json!({"to": "/srv/x", "body": "b"});
    assert!(!config::when_matches(
        &rules[0].when,
        args.as_object().unwrap()
    ));
    // Two names equal ignoring case, and an unknown `when` key, are errors.
    let bad = text
        .replace("arguments: [to, body]", "arguments: [to, body, To]")
        .replace("type: path", "type: path, kind: x");
    let problems = config::parse(&bad).unwrap_err();
    let paths: Vec<&str> = problems.iter().map(|p| p.path.as_str()).collect();
    assert!(
        paths.contains(&"tools.drive.post.arguments[2]"),
        "{problems:#?}"
    );
    assert!(
        paths.contains(&"tools.drive.post.acts[0].when.to.kind"),
        "{problems:#?}"
    );
}

#[test]
fn gateway_check_prints_a_plain_summary() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("gateway.yaml");
    std::fs::write(
        &config_path,
        config_text(
            "https://drive.example.com/mcp",
            Path::new("/dev/null"),
            "agent",
        )
        .replace(
            "  slack.send_email:",
            "    allow_without_reads: true\n  slack.send_email:",
        ),
    )
    .unwrap();
    let policy_path = dir.path().join("policy.yaml");
    std::fs::write(&policy_path, POLICY).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"))
        .args(["gateway", "check", "--config"])
        .arg(&config_path)
        .arg("--policy")
        .arg(&policy_path)
        .env_remove("DRIVE_TOKEN")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    for expected in [
        "Sessions: agent",
        "drive__read_file reads a source",
        "when `path` matches `HR/Compensation/**`: `salaries` (usable by planner, researcher)",
        "otherwise: blocked, never forwarded",
        "not checked when nothing was read",
        "drive__slow is forwarded untouched and its results are NOT tracked",
        "warning: not set here, and required where the gateway runs: DRIVE_TOKEN, SLACK_TOKEN",
    ] {
        assert!(stdout.contains(expected), "{expected:?} not in:\n{stdout}");
    }

    std::fs::write(
        &config_path,
        "version: 1\nupstreams: {}\ntools: []\nextra: 1\n",
    )
    .unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"))
        .args(["gateway", "check", "--config"])
        .arg(&config_path)
        .arg("--policy")
        .arg(&policy_path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1));
    assert!(stdout.contains("3 problem(s)"), "{stdout}");
}

#[test]
fn gateway_check_compares_arguments_with_what_the_upstream_publishes() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("gateway.yaml");
    let policy_path = dir.path().join("policy.yaml");
    std::fs::write(&policy_path, POLICY).unwrap();
    let text = config_text(
        "http://drive.example.com/mcp",
        Path::new("/dev/null"),
        "agent",
    );
    let run = |text: &str| {
        std::fs::write(&config_path, text).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"))
            .args(["gateway", "check", "--config"])
            .arg(&config_path)
            .arg("--policy")
            .arg(&policy_path)
            .env_remove("DRIVE_TOKEN")
            .env("SLACK_TOKEN", SLACK_SECRET)
            .output()
            .unwrap();
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).to_string(),
        )
    };
    let (code, stdout) = run(&text);
    assert_eq!(code, Some(0), "{stdout}");
    for expected in [
        "forwards only the arguments: channel, text, thread; a call with any other is refused",
        "upstream arguments NOT forwardable: blocks",
        "upstream arguments NOT forwardable: bcc",
        "note: upstream `drive` was not contacted",
        "warning: upstreams.drive.url: plain http:// to a host that is not loopback",
    ] {
        assert!(stdout.contains(expected), "{expected:?} not in:\n{stdout}");
    }
    // An allow-listed argument the upstream tool does not take is an error.
    let (code, stdout) = run(&text.replace(
        "arguments: [to, subject, body]",
        "arguments: [to, subject, body, cc]",
    ));
    assert_eq!(code, Some(1), "{stdout}");
    assert!(
        stdout.contains(
            "tools.slack.send_email.arguments: `cc` is not an argument the upstream tool takes"
        ),
        "{stdout}"
    );
}

// ----- security review regressions -----

fn decision_of(result: &Value) -> String {
    result["structuredContent"]["error"]["decision"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Every call the gateway forwarded to the slack upstream.
fn slack_calls<J: ServeJournal>(h: &Gw<J>) -> Vec<String> {
    std::fs::read_to_string(&h.slack_log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// The source sets of the planner's live sessions, as the operator sees them.
async fn planner_sources<J: ServeJournal>(h: &Gw<J>, operator: &str) -> Vec<Vec<String>> {
    h.sessions(operator)
        .await
        .into_iter()
        .filter(|(_, agent, sources)| agent == "planner" && !sources.is_empty())
        .map(|(_, _, sources)| sources)
        .collect()
}

/// Open an MCP session over raw HTTP.
async fn raw_init<J: ServeJournal>(h: &Gw<J>, agent: &str) -> reqwest::Response {
    h.http
        .post(format!("{}/gateway/mcp", h.base))
        .bearer_auth(h.token(agent))
        .header("accept", "application/json, text/event-stream")
        .json(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "t", "version": "0"}}}),
        )
        .send()
        .await
        .unwrap()
}

async fn raw_post<J: ServeJournal>(
    h: &Gw<J>,
    agent: &str,
    mcp: &str,
    body: Value,
) -> (u16, String) {
    let response = h
        .http
        .post(format!("{}/gateway/mcp", h.base))
        .bearer_auth(h.token(agent))
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", mcp)
        .header("mcp-protocol-version", "2025-06-18")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.text().await.unwrap_or_default())
}

#[tokio::test]
async fn c1_extra_and_case_variant_arguments_are_refused_and_never_forwarded() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    // The reviewer's reproduction: checked as a post to #engineering.
    let reproduced = h
        .call(
            &planner,
            "slack__post_message",
            json!({"Channel": "#all-company", "blocks": "SALARY nina 120k",
                   "channel": "#engineering", "text": "hi"}),
        )
        .await;
    assert_eq!(error_code(&reproduced), "invalid_arguments", "{reproduced}");
    let message = text(&reproduced);
    assert!(
        message.contains("Channel") || message.contains("blocks"),
        "{message}"
    );
    // Only the case variant.
    let variant = h
        .call(
            &planner,
            "slack__post_message",
            json!({"channel": "#engineering", "CHANNEL": "#all-company", "text": "hi"}),
        )
        .await;
    assert_eq!(error_code(&variant), "invalid_arguments", "{variant}");
    // An argument the upstream knows but the config does not forward.
    let bcc = h
        .call(
            &planner,
            "slack__send_email",
            json!({"to": "sam@corp.com", "body": "Q3", "bcc": "x@evil.com"}),
        )
        .await;
    assert_eq!(error_code(&bcc), "invalid_arguments", "{bcc}");
    assert!(text(&bcc).contains("bcc"), "{bcc}");
    assert!(slack_calls(&h).is_empty(), "{:?}", slack_calls(&h));

    // A read with an extra argument is refused too, before the upstream.
    let extra = h
        .call(
            &planner,
            "drive__read_file",
            json!({"path": "Public/benchmarks.md", "file_id": "HR-salaries"}),
        )
        .await;
    assert_eq!(error_code(&extra), "invalid_arguments", "{extra}");
    assert_eq!(calls(&h.drive.log, "read_file").len(), 1);

    // Arguments that are not an object never reach the upstream.
    let init = raw_init(&h, "planner").await;
    let mcp = init.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    raw_post(
        &h,
        "planner",
        &mcp,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    let (_, body) = raw_post(
        &h,
        "planner",
        &mcp,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
               "params": {"name": "slack__post_message", "arguments": ["#engineering", "hi"]}}),
    )
    .await;
    assert!(!body.contains("posted"), "{body}");
    assert!(slack_calls(&h).is_empty());
}

#[tokio::test]
async fn c1_recipient_globs_do_not_cross_separators_and_arrays_must_all_match() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    for to in [
        json!("x@evil.com, y@corp.com"),
        json!("x@evil.com;y@corp.com"),
        json!("x@evil.com y@corp.com"),
        json!("<x@evil.com>y@corp.com"),
        json!(["a@corp.com", "b@evil.com"]),
        json!([]),
        json!([1]),
        json!({"to": "a@corp.com"}),
    ] {
        let sent = h
            .call(
                &planner,
                "slack__send_email",
                json!({"to": to, "body": "Q3"}),
            )
            .await;
        // The upstream declares `to` a string: a list or an object is
        // refused before matching; a string that does not match is blocked.
        let expected = if to.is_string() {
            "blocked"
        } else {
            "invalid_arguments"
        };
        assert_eq!(error_code(&sent), expected, "{to}: {sent}");
    }
    let sent = h
        .call(
            &planner,
            "slack__send_email",
            json!({"to": ["a@corp.com", "b@corp.com"], "body": "Q3"}),
        )
        .await;
    assert_eq!(error_code(&sent), "invalid_arguments", "{sent}");
    assert!(
        text(&sent)
            .contains("`to` is of JSON type array, but the upstream tool declares it string"),
        "{sent}"
    );
    assert!(slack_calls(&h).is_empty(), "{:?}", slack_calls(&h));
}

#[tokio::test]
async fn m6_values_whose_json_type_contradicts_the_upstream_schema_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    for text_value in [
        json!({"channel": "#all-company"}),
        json!(["#all-company"]),
        json!(1),
        json!(1.5),
        json!(true),
        json!(null),
    ] {
        let sent = h
            .call(
                &planner,
                "slack__post_message",
                json!({"channel": "#engineering", "text": text_value}),
            )
            .await;
        assert_eq!(
            error_code(&sent),
            "invalid_arguments",
            "{text_value}: {sent}"
        );
        assert!(text(&sent).contains("`text` is of JSON type"), "{sent}");
    }
    let read = h
        .call(
            &planner,
            "drive__read_file",
            json!({"path": ["Product/roadmap.md"]}),
        )
        .await;
    assert_eq!(error_code(&read), "invalid_arguments", "{read}");
    assert!(slack_calls(&h).is_empty(), "{:?}", slack_calls(&h));
    assert_eq!(calls(&h.drive.log, "read_file").len(), 1);
    ok(&h.post(&planner, "#engineering", "roadmap only").await);
}

#[test]
fn m6_the_schema_type_check_is_small_and_strict() {
    let tool = |properties: Value| {
        rmcp::model::Tool::new(
            "t",
            "t",
            json!({"type": "object", "properties": properties})
                .as_object()
                .cloned()
                .unwrap(),
        )
    };
    let args = |v: Value| v.as_object().cloned().unwrap();
    let t = tool(json!({
        "s": {"type": "string"},
        "i": {"type": "integer"},
        "n": {"type": "number"},
        "b": {"type": "boolean"},
        "o": {"type": ["string", "null"]},
        "a": {"type": "array", "items": {"type": "string"}},
        "free": {"description": "no type: not checked"},
    }));
    for good in [
        json!({"s": "x", "i": 3, "n": 3, "b": false}),
        json!({"n": 2.5, "o": null, "a": ["x", "y"], "free": {"any": 1}}),
        json!({"o": "x", "a": [], "undeclared": 1}),
    ] {
        assert!(
            gateway::check_schema_types(&t, &args(good.clone())).is_ok(),
            "{good}"
        );
    }
    for bad in [
        json!({"s": {"channel": "#all-company"}}),
        json!({"s": ["x"]}),
        json!({"s": 1}),
        json!({"i": "3"}),
        json!({"i": 3.5}),
        json!({"n": "3"}),
        json!({"b": "true"}),
        json!({"o": 1}),
        json!({"a": "x"}),
        json!({"a": ["x", 1]}),
        json!({"a": [["x"]]}),
    ] {
        assert!(
            gateway::check_schema_types(&t, &args(bad.clone())).is_err(),
            "{bad}"
        );
    }
}

/// Recipient values a mail parser may read as an address outside the
/// domain (the third review's reproduction), and invisible-character tricks.
const SPOOFED_RECIPIENTS: &[&str] = &[
    "x@evil.com(@corp.com",
    "x@evil.com@corp.com",
    "x%evil.com@corp.com",
    "evil.com!x@corp.com",
    "=?utf-8?q?x@evil.com?=@corp.com",
    "x@evil.com\u{2063}@corp.com",
    "\"x@evil.com\"@corp.com",
    "x@EVIL.COM?@corp.com",
    "x@[1.2.3.4]@corp.com",
    "sam@corp.com\u{200b}",
    "sam\u{00ad}@corp.com",
    "sam@corp.com\u{e0041}",
    "sam\u{3164}@corp.com",
    "sam@corp.com.",
    ".sam@corp.com",
    "sam@sub.corp.com",
    "sam@corp.com.evil.com",
];

async fn email_to<J: ServeJournal>(h: &Gw<J>, agent: &Agent, to: Value) -> Value {
    h.call(agent, "slack__send_email", json!({"to": to, "body": "Q3"}))
        .await
}

#[tokio::test]
async fn i1_spoofed_recipients_are_refused_under_type_email() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    for to in SPOOFED_RECIPIENTS {
        let sent = email_to(&h, &planner, json!(to)).await;
        assert_eq!(error_code(&sent), "blocked", "{to:?}: {sent}");
    }
    assert!(slack_calls(&h).is_empty(), "{:?}", slack_calls(&h));
    for to in ["sam@corp.com", "x+y@CORP.com", "first.last-1@corp.com"] {
        let sent = email_to(&h, &planner, json!(to)).await;
        assert_eq!(ok(&sent), "sent", "{to}");
    }
    assert_eq!(slack_calls(&h).len(), 3);
}

#[tokio::test]
async fn i1_acts_rules_match_exactly_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let h = launch_with(
        dir.path(),
        "connection",
        move |port| {
            let runtime = Runtime::open(MemoryJournal::new(), POLICY, 0).unwrap();
            Server::new(runtime, settings(port, &state, Limits::default())).unwrap()
        },
        |text| {
            text.replace(
                r#"to: { pattern: "*@corp.com", type: email }"#,
                r#"to: "sam@corp.com""#,
            )
        },
    )
    .await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    for to in SPOOFED_RECIPIENTS.iter().copied().chain([
        "SAM@corp.com",
        "sam@CORP.COM",
        " sam@corp.com",
        "x@corp.com",
    ]) {
        let sent = email_to(&h, &planner, json!(to)).await;
        assert_eq!(error_code(&sent), "blocked", "{to:?}: {sent}");
    }
    assert!(slack_calls(&h).is_empty(), "{:?}", slack_calls(&h));
    let sent = email_to(&h, &planner, json!("sam@corp.com")).await;
    assert_eq!(ok(&sent), "sent");
}

#[test]
fn i1_matcher_types_and_their_config_errors() {
    let email = |p: &str| config::Matcher {
        argument: "to".into(),
        pattern: p.into(),
        kind: config::MatchType::Email,
    };
    for to in SPOOFED_RECIPIENTS {
        assert!(
            !config::value_matches(&email("*@corp.com"), &json!(to)),
            "{to:?}"
        );
        assert!(
            !config::value_matches(&email("sam@corp.com"), &json!(to)),
            "{to:?}"
        );
    }
    assert!(config::value_matches(
        &email("*@corp.com"),
        &json!("sam@Corp.Com")
    ));
    assert!(config::value_matches(
        &email("sam@corp.com"),
        &json!("sam@CORP.com")
    ));
    assert!(!config::value_matches(
        &email("sam@corp.com"),
        &json!("Sam@corp.com")
    ));
    // A list matches only when every element does.
    assert!(!config::value_matches(&email("*@corp.com"), &json!([])));
    assert!(config::value_matches(
        &email("*@corp.com"),
        &json!(["a@corp.com", "b_c@corp.com"])
    ));
    assert!(!config::value_matches(
        &email("*@corp.com"),
        &json!(["sam@corp.com", "x@evil.com@corp.com"])
    ));
    assert!(!config::value_matches(
        &email("*@corp.com"),
        &json!(["a@corp.com", 1])
    ));
    let exact = config::Matcher {
        argument: "to".into(),
        pattern: "sam@corp.com".into(),
        kind: config::MatchType::Exact,
    };
    assert!(config::value_matches(
        &exact,
        &json!(["sam@corp.com", "sam@corp.com"])
    ));
    assert!(!config::value_matches(
        &exact,
        &json!(["sam@corp.com", "x@corp.com"])
    ));
    assert!(!config::value_matches(&exact, &json!("SAM@corp.com")));
    assert!(!config::value_matches(
        &exact,
        &json!({"to": "sam@corp.com"})
    ));
    // Invisible characters never match, whatever the type.
    for kind in [
        config::MatchType::Exact,
        config::MatchType::Glob,
        config::MatchType::Path,
    ] {
        let m = config::Matcher {
            argument: "a".into(),
            pattern: "x*".into(),
            kind,
        };
        for odd in [
            "x\u{200b}",
            "x\u{00ad}",
            "x\u{3164}",
            "x\u{e0001}",
            "x\u{2028}",
            "x\u{feff}",
        ] {
            assert!(!config::value_matches(&m, &json!(odd)), "{kind:?} {odd:?}");
        }
    }
    let text = |when: &str| {
        format!(
            "version: 1
upstreams:
  slack: {{ command: [x] }}
tools:
  slack.send_email:
    arguments: [to, body]
    acts:
      - when: {when}
        action: email_staff
"
        )
    };
    for (when, needle) in [
        (r#"{ to: "*@corp.com" }"#, "type: email"),
        (
            r#"{ to: { pattern: "*@corp.com", type: exact } }"#,
            "type: email",
        ),
        (r#"{ to: "?am@corp.com" }"#, "type: glob"),
        (
            r#"{ to: { pattern: "x@evil.com@corp.com", type: email } }"#,
            "email pattern",
        ),
        (
            r#"{ to: { pattern: "*@*.com", type: email } }"#,
            "email pattern",
        ),
        (
            r#"{ to: { pattern: "a", type: text } }"#,
            "no longer a type",
        ),
        (r#"{ to: { pattern: "a", glob: "b" } }"#, "not both"),
        ("{ to: \"sam@corp.com\\u200b\" }", "invisible"),
    ] {
        let problems = config::parse(&text(when)).unwrap_err();
        assert!(
            problems.iter().any(
                |p| p.path.starts_with("tools.slack.send_email.acts[0].when.to")
                    && p.message.contains(needle)
            ),
            "{when}: {problems:#?}"
        );
    }
    // Typed patterns parse; a glob on an action and a free argument warn.
    let config = config::parse(&text(r#"{ to: { glob: "*@corp.com", type: glob } }"#)).unwrap();
    let warnings: Vec<String> = config::warnings(&config)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("acts[0].when.to") && w.contains("type: glob")),
        "{warnings:#?}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("send_email.arguments") && w.contains("body")),
        "{warnings:#?}"
    );
    let config = config::parse(&text("{}")).unwrap();
    let warnings: Vec<String> = config::warnings(&config)
        .iter()
        .map(ToString::to_string)
        .collect();
    assert!(
        warnings.iter().any(|w| w.contains("acts[0]")
            && w.contains(
                "every call to this tool is treated as `email_staff` whatever its arguments"
            )),
        "{warnings:#?}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("to, body") || w.contains("body, to")),
        "{warnings:#?}"
    );
    // A reads rule still defaults to `type: path`.
    let reads = "version: 1
upstreams:
  drive: { url: \"https://d.example.com/mcp\" }
tools:
  drive.read_file:
    reads:
      - when: { path: \"Product/**\" }
        source: roadmap
    otherwise: unmatched
";
    let config = config::parse(reads).unwrap();
    let config::Kind::Reads { rules, .. } = &config.tools["drive__read_file"].kind else {
        panic!()
    };
    assert_eq!(rules[0].when[0].kind, config::MatchType::Path);
}

#[test]
fn i1_gateway_check_prints_the_matchers_and_warns() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("gateway.yaml");
    std::fs::write(
        &config_path,
        config_text(
            "https://drive.example.com/mcp",
            Path::new("/dev/null"),
            "agent",
        )
        .replace(
            "        action: brief_leadership\n",
            "        action: brief_leadership\n      - action: post_to_engineering\n",
        ),
    )
    .unwrap();
    let policy_path = dir.path().join("policy.yaml");
    std::fs::write(&policy_path, POLICY).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"))
        .args(["gateway", "check", "--offline", "--config"])
        .arg(&config_path)
        .arg("--policy")
        .arg(&policy_path)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    for expected in [
        "when `channel` is exactly `#engineering`: `post_to_engineering`",
        "when `to` is a plain email address at exactly `corp.com`: `email_staff`",
        "always (every call, whatever its arguments): `post_to_engineering`",
        "warning: tools.slack.post_message.acts[2]: no `when`: every call to this tool is treated as `post_to_engineering` whatever its arguments",
        "warning: tools.slack.post_message.arguments: not constrained by any `when`: text, thread",
        "warning: tools.slack.send_email.arguments: not constrained by any `when`: body, subject",
    ] {
        assert!(stdout.contains(expected), "{expected:?} not in:\n{stdout}");
    }
}

#[tokio::test]
async fn c1_odd_and_encoded_paths_fall_to_otherwise() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let operator = h.login().await;
    for path in [
        "Product/%2e%2e/HR/Compensation/salaries.csv",
        "Product/..%2FHR/salaries.csv",
        "/Product/roadmap.md",
        "./Product/roadmap.md",
        "Product//roadmap.md",
        "Product/./roadmap.md",
        "Product/~/roadmap.md",
        "Product/r\u{f6}admap.md",
        "Product/roadmap.md\u{202e}",
    ] {
        let planner = h.connect("planner").await;
        h.read(&planner, path).await;
        let carried = planner_sources(&h, &operator).await;
        assert!(
            carried.iter().all(|s| s == &vec!["unmatched".to_string()]),
            "{path}: {carried:?}"
        );
        planner.client.cancel().await.unwrap();
    }
}

#[tokio::test]
async fn c2_an_approval_releases_exactly_one_identical_call() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let args = json!({"channel": "#leadership", "text": "Q3 roadmap summary"});
    let held = h.call(&planner, "slack__post_message", args.clone()).await;
    assert_eq!(error_code(&held), "needs_approval");
    let decision = decision_of(&held);

    let operator = h.login().await;
    let (status, body) = h
        .op_post(
            "/v1/operator/approve",
            &operator,
            json!({"decision": decision}),
        )
        .await;
    assert_eq!(status, 200, "{body}");

    // A retry with any argument changed is a different call: a new decision.
    let changed = h
        .call(
            &planner,
            "slack__post_message",
            json!({"channel": "#leadership", "text": "Q3 roadmap summary", "thread": "1"}),
        )
        .await;
    assert_eq!(error_code(&changed), "needs_approval", "{changed}");
    assert_ne!(decision_of(&changed), decision);
    assert!(slack_calls(&h).is_empty());

    // The identical retry is forwarded once.
    let retry = h.call(&planner, "slack__post_message", args.clone()).await;
    assert_eq!(ok(&retry), "posted to #leadership");
    assert_eq!(slack_calls(&h).len(), 1);
    // The approval is used up: the same call again is held again.
    let again = h.call(&planner, "slack__post_message", args.clone()).await;
    assert_eq!(error_code(&again), "needs_approval", "{again}");
    assert_eq!(slack_calls(&h).len(), 1);
    let state = h.op_state(&operator).await;
    assert!(
        state["approvals"].as_array().unwrap().is_empty(),
        "{}",
        state["approvals"]
    );
}

fn canonical_sha(arguments: &Value) -> String {
    use sha2::Digest;
    let text = serde_json::to_string(arguments).unwrap();
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn c2_the_operator_sees_the_arguments_of_a_held_call() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let long = format!("Q3 <b>plan</b>\u{1b}[2J{}", "x".repeat(2500));
    let args = json!({"channel": "#leadership", "text": long});
    let held = h.call(&planner, "slack__post_message", args.clone()).await;
    assert_eq!(error_code(&held), "needs_approval");
    let operator = h.login().await;
    let state = h.op_state(&operator).await;
    let pending = &state["pending"][0];
    let call = &pending["gateway"];
    assert_eq!(call["tool"], "slack__post_message", "{pending}");
    assert_eq!(call["held"], true, "{pending}");
    assert_eq!(pending["can_approve"], true);
    assert_eq!(call["sha256"], canonical_sha(&args));
    let arguments = call["arguments"].as_array().unwrap();
    assert_eq!(arguments[0]["name"], "channel");
    assert_eq!(arguments[0]["value"], "#leadership");
    assert_eq!(arguments[1]["name"], "text");
    let shown = arguments[1]["value"].as_str().unwrap();
    assert_eq!(shown.chars().count(), 2000);
    assert!(long.starts_with(shown));
    assert_eq!(
        arguments[1]["more"].as_u64().unwrap() as usize,
        long.chars().count() - 2000
    );
}

#[tokio::test]
async fn c2_after_a_restart_a_held_call_must_be_retried_before_it_can_be_approved() {
    let dir = tempfile::tempdir().unwrap();
    let h = on_disk(dir.path(), "agent").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let args = json!({"channel": "#leadership", "text": "Q3 plan, held in memory only"});
    let held = h.call(&planner, "slack__post_message", args.clone()).await;
    let decision = decision_of(&held);
    // The arguments are kept in memory only, never in the journal.
    assert!(!all_files(&h.state).contains("held in memory only"));
    planner.client.cancel().await.unwrap();
    let h = restart(h, dir.path(), "agent").await;
    let operator = h.login().await;
    let state = h.op_state(&operator).await;
    let pending = &state["pending"][0];
    assert_eq!(pending["decision"], decision.as_str());
    assert_eq!(pending["can_approve"], false, "{pending}");
    assert_eq!(pending["gateway"]["held"], false, "{pending}");
    assert!(
        pending["cannot_approve"].as_str().unwrap().contains("retr"),
        "{pending}"
    );
    let (status, _) = h
        .op_post(
            "/v1/operator/approve",
            &operator,
            json!({"decision": decision}),
        )
        .await;
    assert_ne!(status, 200, "approving a call nobody can see is refused");
    // The agent retries; now it can be approved, and the retry after that goes.
    let planner = h.connect("planner").await;
    let again = h.call(&planner, "slack__post_message", args.clone()).await;
    assert_eq!(decision_of(&again), decision);
    let state = h.op_state(&operator).await;
    assert_eq!(state["pending"][0]["can_approve"], true);
    let (status, body) = h
        .op_post(
            "/v1/operator/approve",
            &operator,
            json!({"decision": decision}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        ok(&h.call(&planner, "slack__post_message", args).await),
        "posted to #leadership"
    );
}

#[tokio::test]
async fn ending_an_agent_session_closes_its_live_connections() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "agent").await;
    let init = raw_init(&h, "planner").await;
    let mcp = init.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    raw_post(
        &h,
        "planner",
        &mcp,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    let read = |id: u64| {
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
            "name": "drive__read_file", "arguments": {"path": "HR/Compensation/salaries.csv"}}})
    };
    let (status, body) = raw_post(&h, "planner", &mcp, read(2)).await;
    assert_eq!(status, 200);
    assert!(body.contains("120k"), "{body}");
    let operator = h.login().await;
    let (status, body) = h
        .op_post(
            "/v1/operator/end-session",
            &operator,
            json!({"session": "agent:planner"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    // The same connection can no longer call tools: it must initialise again.
    let (status, body) = raw_post(&h, "planner", &mcp, read(3)).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(calls(&h.drive.log, "read_file").len(), 1);
}

#[tokio::test]
async fn in_agent_mode_an_mcp_session_belongs_to_the_agent_that_opened_it() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "agent").await;
    let init = raw_init(&h, "planner").await;
    assert_eq!(init.status().as_u16(), 200);
    let mcp = init.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_string();
    let (status, _) = raw_post(
        &h,
        "scout",
        &mcp,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    )
    .await;
    assert_eq!(status, 404);
    let deleted = h
        .http
        .delete(format!("{}/gateway/mcp", h.base))
        .bearer_auth(h.token("scout"))
        .header("mcp-session-id", &mcp)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status().as_u16(), 404);
    let (status, _) = raw_post(
        &h,
        "planner",
        &mcp,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    assert!(status < 300, "{status}");
}

#[tokio::test]
async fn the_operator_page_lists_open_connections_and_the_cap() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "agent").await;
    let operator = h.login().await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let state = h.op_state(&operator).await;
    let connections = state["gateway"]["connections"].as_array().unwrap().clone();
    assert_eq!(connections.len(), 1, "{}", state["gateway"]);
    assert_eq!(connections[0]["agent"], "planner");
    assert!(connections[0]["last_active"].as_u64().is_some());
    for _ in 1..gateway::MAX_CONNECTIONS_PER_AGENT {
        assert_eq!(raw_init(&h, "planner").await.status().as_u16(), 200);
    }
    let refused = raw_init(&h, "planner").await;
    assert_eq!(refused.status().as_u16(), 429);
    assert!(refused.text().await.unwrap().contains("operator page"));
    let state = h.op_state(&operator).await;
    assert_eq!(state["gateway"]["at_cap"], json!(["planner"]));
    // End one connection: a new one is accepted again.
    let id = connections[0]["connection"].as_str().unwrap().to_string();
    let (status, body) = h
        .op_post(
            "/v1/operator/end-connection",
            &operator,
            json!({"connection": id}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(raw_init(&h, "planner").await.status().as_u16(), 200);
}

#[tokio::test]
async fn a_stdio_upstream_gets_a_private_home_and_a_minimal_path() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    let out = ok(&h.call(&planner, "slack__env_home", json!({})).await);
    let (home, path) = out.split_once('\n').unwrap();
    assert_ne!(Some(home.to_string()), std::env::var("HOME").ok());
    assert_ne!(Some(path.to_string()), std::env::var("PATH").ok());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        assert_eq!(std::fs::read_dir(home).unwrap().count(), 0, "empty");
    }
}

#[tokio::test]
async fn upstream_results_with_a_credential_or_over_the_cap_look_the_same() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    let leak = h.call(&planner, "slack__leak", json!({})).await;
    let big = h.call(&planner, "drive__big", json!({})).await;
    assert_eq!(error_code(&leak), "upstream_result_withheld");
    assert_eq!(error_code(&big), "upstream_result_withheld");
    assert!(!leak.to_string().contains(SLACK_SECRET));
}

#[tokio::test]
async fn upstream_schema_text_is_cleaned_and_a_huge_schema_closes_its_tool() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    let tools = planner.client.peer().list_all_tools().await.unwrap();
    assert!(!tools.iter().any(|t| t.name == "drive__huge_schema"));
    let post = tools
        .iter()
        .find(|t| t.name == "slack__post_message")
        .unwrap();
    let schema = serde_json::to_string(&*post.input_schema).unwrap();
    assert!(
        !schema.contains("\\u001b") && !schema.contains("\\u0007"),
        "{schema}"
    );
    let huge = h.call(&planner, "drive__huge_schema", json!({})).await;
    assert!(huge["isError"] == true, "{huge}");
}

#[tokio::test]
async fn a_refused_hand_off_leaves_the_receiver_unable_to_act() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let refused = h
        .call(
            &planner,
            gateway::RECEIVE_TOOL,
            json!({"token": "tgh1.00.00"}),
        )
        .await;
    assert_eq!(error_code(&refused), "invalid_handoff");
    let post = h.post(&planner, "#engineering", "roadmap").await;
    assert_eq!(error_code(&post), "session_blocked", "{post}");
    assert!(slack_calls(&h).is_empty());
}

#[tokio::test]
async fn m2_in_connection_mode_a_refused_hand_off_blocks_the_whole_agent_until_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let operator = h.login().await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let refused = h
        .call(&planner, gateway::RECEIVE_TOOL, json!({"token": "garbage"}))
        .await;
    assert_eq!(error_code(&refused), "invalid_handoff");
    // A new connection is a new session, but the agent is blocked.
    let fresh = h.connect("planner").await;
    ok(&h.read(&fresh, "Product/roadmap.md").await);
    let post = h.post(&fresh, "#engineering", "roadmap only").await;
    assert_eq!(error_code(&post), "agent_blocked", "{post}");
    assert!(slack_calls(&h).is_empty());
    // Another agent is not affected.
    let scout = h.connect("scout").await;
    ok(&h.read(&scout, "Public/benchmarks.md").await);
    ok(&h.post(&scout, "#engineering", "bench").await);
    let state = h.op_state(&operator).await;
    let blocked: Vec<&str> = state["gateway"]["blocked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(blocked.contains(&"agent:planner"), "{blocked:?}");
    // Kept across a restart.
    let saved = gateway::load_blocked(&h.state.join("gateway.blocked")).unwrap();
    assert!(saved.contains("agent:planner"), "{saved:?}");
    // The operator clears it; the fresh connection may act, the refusing
    // connection's own session stays blocked.
    let (status, body) = h
        .op_post(
            "/v1/operator/end-session",
            &operator,
            json!({"session": "agent:planner"}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    ok(&h.post(&fresh, "#engineering", "roadmap only").await);
    let post = h.post(&planner, "#engineering", "roadmap").await;
    assert_eq!(error_code(&post), "session_blocked", "{post}");
}

fn bare_server(state: &Path) -> Arc<Server<MemoryJournal>> {
    let runtime = Runtime::open(MemoryJournal::new(), POLICY, 0).unwrap();
    Server::new(runtime, settings(1, state, Limits::default())).unwrap()
}

#[tokio::test]
async fn m1_a_corrupt_or_unreadable_blocked_file_stops_the_gateway() {
    let dir = tempfile::tempdir().unwrap();
    let drive = drive(dir.path()).await;
    let text = config_text(&drive.url, &dir.path().join("s.log"), "agent");
    let state = dir.path().join("state");
    let blocked = state.join("gateway.blocked");
    for bad in [
        "",
        "not json",
        "[\"agent:planner\"",
        "{\"agent:planner\": true}",
        "[1]",
    ] {
        let server = bare_server(&state);
        std::fs::write(&blocked, bad).unwrap();
        let problems = gateway::attach(server, config::parse(&text).unwrap(), &env)
            .err()
            .unwrap_or_else(|| panic!("started with {bad:?}"));
        assert!(
            problems.iter().any(|p| p.contains("gateway.blocked")),
            "{problems:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&blocked).unwrap(),
            bad,
            "kept as is"
        );
    }
    // A directory or a link where the file should be is refused too.
    std::fs::remove_file(&blocked).unwrap();
    std::fs::create_dir(&blocked).unwrap();
    assert!(gateway::attach(bare_server(&state), config::parse(&text).unwrap(), &env).is_err());
    std::fs::remove_dir(&blocked).unwrap();
    #[cfg(unix)]
    {
        let target = dir.path().join("elsewhere");
        std::fs::write(&target, "[]").unwrap();
        std::os::unix::fs::symlink(&target, &blocked).unwrap();
        assert!(gateway::attach(bare_server(&state), config::parse(&text).unwrap(), &env).is_err());
        std::fs::remove_file(&blocked).unwrap();
    }
    // A missing file is an empty set; a good one is kept.
    std::fs::write(&blocked, r#"["agent:planner","conn:x"]"#).unwrap();
    let server = bare_server(&state);
    assert!(gateway::attach(server, config::parse(&text).unwrap(), &env).is_ok());
    assert_eq!(
        std::fs::read_to_string(&blocked).unwrap(),
        r#"["agent:planner"]"#
    );
}

#[test]
fn m1_the_blocked_file_is_written_atomically_and_never_through_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.blocked");
    let set: std::collections::BTreeSet<String> = ["agent:planner".to_string()].into();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let target = dir.path().join("target");
        std::fs::write(&target, "untouched").unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("gateway.blocked.tmp")).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        gateway::save_blocked(&path, &set).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched");
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert!(metadata.is_file(), "the link was replaced by a file");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }
    gateway::save_blocked(&path, &set).unwrap();
    assert_eq!(gateway::load_blocked(&path).unwrap(), set);
    assert!(!dir.path().join("gateway.blocked.tmp").exists());
    assert!(
        gateway::load_blocked(&dir.path().join("missing"))
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_connection_ended_mid_call_leaves_no_session_behind() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "connection").await;
    let operator = h.login().await;
    let planner = Arc::new(h.connect("planner").await);
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let session = h.sessions(&operator).await[0].0.clone();
    let p2 = planner.clone();
    let call = tokio::spawn(async move {
        let _ = p2
            .client
            .peer()
            .call_tool(
                CallToolRequestParams::new("drive__slow_read").with_arguments(
                    json!({"path": "Product/roadmap.md"})
                        .as_object()
                        .cloned()
                        .unwrap(),
                ),
            )
            .await;
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let (status, body) = h
        .op_post(
            "/v1/operator/end-session",
            &operator,
            json!({"session": session}),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let _ = tokio::time::timeout(Duration::from_secs(3), call).await;
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let left = h.sessions(&operator).await;
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn config_refuses_what_the_review_found() {
    let text = "version: 1
session: connection
upstreams:
  drive: { url: \"https://user:pw@drive.example.com/mcp\" }
  slack: { command: [x] }
tools:
  slack.post:
    acts:
      - when: { channel: \"#eng\" }
        action: post_to_engineering
    allow_without_reads: true
";
    let problems = config::parse(text).unwrap_err();
    let paths: Vec<&str> = problems.iter().map(|p| p.path.as_str()).collect();
    for expected in [
        "upstreams.drive.url",
        "tools.slack.post.arguments",
        "tools.slack.post.allow_without_reads",
    ] {
        assert!(
            paths.contains(&expected),
            "{expected} missing from {problems:#?}"
        );
    }
}

#[test]
fn otherwise_must_be_at_least_as_restrictive_as_every_rule() {
    let text = "version: 1
upstreams:
  drive: { url: \"http://127.0.0.1:1/mcp\" }
tools:
  drive.read_file:
    reads:
      - when: { path: \"Product/**\" }
        source: roadmap
    otherwise: salaries
";
    let config = config::parse(text).unwrap();
    let policy = trustgraph_policy::parse(POLICY).policy.unwrap();
    let problems = config::check_against(&config, &policy);
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert_eq!(problems[0].path, "tools.drive.read_file.otherwise");
    assert!(
        problems[0].message.contains("researcher"),
        "{}",
        problems[0]
    );
}

#[tokio::test]
async fn a_short_secret_is_refused_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let drive = drive(dir.path()).await;
    let config = config::parse(&config_text(
        &drive.url,
        &dir.path().join("s.log"),
        "connection",
    ))
    .unwrap();
    let state = dir.path().join("state");
    let runtime = Runtime::open(MemoryJournal::new(), POLICY, 0).unwrap();
    let server = Server::new(runtime, settings(1, &state, Limits::default())).unwrap();
    let short = |name: &str| match name {
        "SLACK_TOKEN" => Some("short".to_string()),
        other => env(other),
    };
    let problems = gateway::attach(server, config, &short).err().unwrap();
    assert!(
        problems.iter().any(|p| p.contains("SLACK_TOKEN")),
        "{problems:?}"
    );
}

#[test]
fn the_bridge_refuses_a_readable_token_file_and_a_cleartext_remote_url() {
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().join("planner.token");
    std::fs::write(&token, "a".repeat(64)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o644)).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"))
            .args([
                "gateway",
                "stdio",
                "--url",
                "http://127.0.0.1:9/gateway/mcp",
                "--token-file",
            ])
            .arg(&token)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("readable"));
        std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"))
        .args([
            "gateway",
            "stdio",
            "--url",
            "http://gateway.example.com/gateway/mcp",
            "--token-file",
        ])
        .arg(&token)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--allow-insecure"));
}

#[tokio::test]
async fn c2_concurrent_identical_retries_release_one_call() {
    let dir = tempfile::tempdir().unwrap();
    let h = memory(dir.path(), "agent").await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let args = json!({"channel": "#leadership", "text": "Q3 plan"});
    let held = h.call(&planner, "slack__post_message", args.clone()).await;
    let operator = h.login().await;
    let (status, _) = h
        .op_post(
            "/v1/operator/approve",
            &operator,
            json!({"decision": decision_of(&held)}),
        )
        .await;
    assert_eq!(status, 200);
    // Four connections of the same agent session race the same call.
    let (a, b, c, d) = (
        h.connect("planner").await,
        h.connect("planner").await,
        h.connect("planner").await,
        h.connect("planner").await,
    );
    let results = tokio::join!(
        h.call(&a, "slack__post_message", args.clone()),
        h.call(&b, "slack__post_message", args.clone()),
        h.call(&c, "slack__post_message", args.clone()),
        h.call(&d, "slack__post_message", args.clone()),
    );
    let results = [results.0, results.1, results.2, results.3];
    let forwarded = results.iter().filter(|r| r["isError"] != true).count();
    assert_eq!(forwarded, 1, "{results:?}");
    assert_eq!(slack_calls(&h).len(), 1);
}

#[tokio::test]
async fn m8_after_concurrent_retries_the_losers_call_stays_approvable() {
    let dir = tempfile::tempdir().unwrap();
    let h = Arc::new(memory(dir.path(), "agent").await);
    let operator = h.login().await;
    let planner = h.connect("planner").await;
    ok(&h.read(&planner, "Product/roadmap.md").await);
    let args = json!({"channel": "#leadership", "text": "Q3 plan"});
    let mut connections = Vec::new();
    for _ in 0..3 {
        connections.push(Arc::new(h.connect("planner").await));
    }
    for round in 0..10 {
        let held = h.call(&planner, "slack__post_message", args.clone()).await;
        assert_eq!(error_code(&held), "needs_approval", "{held}");
        let decision = decision_of(&held);
        let (status, body) = h
            .op_post(
                "/v1/operator/approve",
                &operator,
                json!({"decision": decision}),
            )
            .await;
        assert_eq!(status, 200, "round {round}: {body}");
        let mut tasks = Vec::new();
        for c in &connections {
            let (h, c, args) = (h.clone(), c.clone(), args.clone());
            tasks.push(tokio::spawn(async move {
                h.call(&c, "slack__post_message", args).await
            }));
        }
        let mut forwarded = 0;
        for t in tasks {
            if t.await.unwrap()["isError"] != true {
                forwarded += 1;
            }
        }
        assert_eq!(forwarded, 1, "round {round}");
        assert_eq!(slack_calls(&h).len(), round + 1);
        // Every loser was held again: the operator still sees exactly what
        // it would forward, so the next approval can be given.
        let state = h.op_state(&operator).await;
        let pending = state["pending"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["decision"] == decision.as_str())
            .unwrap_or_else(|| panic!("round {round}: not pending: {state}"))
            .clone();
        assert_eq!(pending["gateway"]["held"], true, "round {round}: {pending}");
        assert_eq!(pending["can_approve"], true, "round {round}: {pending}");
    }
}
