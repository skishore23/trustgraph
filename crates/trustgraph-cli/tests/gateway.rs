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
actions:
  post_to_engineering: { agents: [planner, scout], audience: [engineering] }
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
    otherwise: salaries
  drive.search:
    passthrough: true
  drive.slow:
    passthrough: true
    untracked: true
  drive.big:
    passthrough: true
    untracked: true
  slack.post_message:
    acts:
      - when: {{ channel: "#engineering" }}
        action: post_to_engineering
        content: text
      - when: {{ channel: "#leadership" }}
        action: brief_leadership
        content: text
  slack.env_names:
    passthrough: true
    untracked: true
  gone.read_file:
    passthrough: true
    untracked: true
"##,
        fake = fake_upstream().display(),
        log = slack_log.display(),
    )
}

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
        "connection",
        move |port| {
            let runtime = Runtime::open(MemoryJournal::new(), POLICY, 0).unwrap();
            Server::new(runtime, settings(port, &state, Limits::default())).unwrap()
        },
        |text| {
            text.replace(
                "  slack.env_names:",
                "    allow_without_reads: true\n  slack.env_names:",
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
        "slack__env_names",
        "slack__post_message",
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
    // Each of these must fall to `otherwise` (salaries), never to roadmap.
    let attempts = [
        json!("Product/../HR/Compensation/salaries.csv"),
        json!("Product\\..\\HR\\Compensation\\salaries.csv"),
        json!("product/roadmap.md"),
        json!("PRODUCT/roadmap.md"),
        json!("Product/road\u{0}map.md"),
        json!("Product/\nroadmap.md"),
        json!(["Product/roadmap.md"]),
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
            carried.iter().all(|s| s == &vec!["salaries".to_string()]),
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
    assert_eq!(error_code(&big), "upstream_result_too_large");
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
    ok(&h.read(&third, "Product/roadmap.md").await);
    assert_eq!(
        ok(&h.post(&third, "#engineering", "roadmap").await),
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
    std::fs::write(
        &config_path,
        config_text(&drive.url, &slack_log, "connection"),
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
            "  slack.env_names:",
            "    allow_without_reads: true\n  slack.env_names:",
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
