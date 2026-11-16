//! `trustgraph serve`: the policy runtime behind a loopback HTTP API, an MCP
//! endpoint and a plain operator page.
//!
//! Identity is the whole point. An agent is known ONLY by its bearer token
//! (one per agent, in `<state>/tokens/`); no request body names an agent.
//! The operator is known ONLY by the `X-TrustGraph-Operator` session id that
//! the single-use launch file's secret is exchanged for (never a cookie:
//! cookies ignore ports). One operator session is live at a time. The
//! reviewer sent to the engine is always the configured `--operator`. A
//! request with an `Authorization` header never reaches an operator route,
//! and no agent route or MCP tool can approve, revoke or activate.
//!
//! An artifact an agent may not use answers exactly like one that does not
//! exist (404 `unknown_artifact`), and neither is journaled. Requests that
//! would change nothing or that the engine would refuse for their shape are
//! answered without journaling, so agents spend their journal budget only on
//! real work.
//!
//! Time: `now` is the larger of the wall clock and the last journaled time
//! at start, plus the time elapsed since: the larger of monotonic and
//! wall-clock elapsed time, never less than before. Sleep (which monotonic
//! time skips) and forward wall-clock jumps make things expire sooner; a
//! backward jump is ignored. Operator sessions use the same time.
//!
//! Every request must name the loopback `Host` with this server's port, and
//! a browser `Origin`, when present, must be the same origin. A journal
//! failure makes the outcome unknown: the server then answers every request
//! with 503 `restart_required` until the process is restarted.

mod auth;
pub mod gateway;
pub mod mcp;
mod tokens;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use trustgraph_engine::{
    Engine, Event, FileJournal, FileOutcome, Journal, MemoryJournal, PolicyFile, ProposalOrigin,
    Reply, Runtime,
};
use trustgraph_policy::{ChangeKind, Policy, Severity, lint, parse, run_tests};

use crate::policy_cmd::text as policy_text;

pub use tokens::random_token;

/// Largest accepted request body, in bytes.
pub const BODY_LIMIT: usize = 512 * 1024;
/// How much of an oversize body is read and discarded before answering 413.
const DRAIN_LIMIT: usize = 8 * 1024 * 1024;
pub const DEFAULT_MAX_ARTIFACTS: usize = 100_000;
pub const DEFAULT_MAX_JOURNAL_BYTES: u64 = 1 << 30;
/// Journal bytes one agent may cause over the state directory's lifetime.
pub const DEFAULT_AGENT_JOURNAL_BYTES: u64 = 64 << 20;
/// Artifacts one agent may create over the state directory's lifetime.
pub const DEFAULT_AGENT_ARTIFACTS: u64 = 10_000;
/// How far the wall clock may be behind the journal at start before
/// approvals are refused.
pub const CLOCK_BEHIND_TOLERANCE: u64 = 300;
pub const DEFAULT_PORT: u16 = 8787;
/// `policy_simulate` bounds: the server runs it on the caller's behalf.
pub const MAX_SIMULATE_COUNT: u64 = 8;
pub const MAX_SIMULATE_STEPS: u32 = 120;
pub const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

pub const PAGE_HTML: &str = include_str!("../../../../web/serve.html");
pub const PAGE_CSS: &str = include_str!("../../../../web/serve.css");
pub const PAGE_JS: &str = include_str!("../../../../web/serve.js");

/// A journal the server can size for its quota.
pub trait ServeJournal: Journal + Send + 'static {
    /// Bytes the journal takes; `None` when unknown, which counts as full.
    fn size(&self) -> Option<u64>;
}

impl ServeJournal for FileJournal {
    fn size(&self) -> Option<u64> {
        self.len_bytes().ok()
    }
}

impl ServeJournal for MemoryJournal {
    fn size(&self) -> Option<u64> {
        Some(self.len_bytes())
    }
}

/// Caps on what untrusted agents can make the server do.
#[derive(Debug, Clone)]
pub struct Limits {
    pub max_artifacts: usize,
    pub max_journal_bytes: u64,
    /// Journal bytes each agent may cause over the state directory's
    /// lifetime (rebuilt from the journal at start).
    pub agent_journal_bytes: u64,
    /// Artifacts each agent may create over the state directory's lifetime.
    pub agent_artifacts: u64,
    /// Sustained requests per second per agent.
    pub rate_per_second: f64,
    /// Requests an agent may make at once before the rate applies.
    pub burst: f64,
    /// Per agent, for the pure policy helpers other than simulate.
    pub helper_rate_per_second: f64,
    /// Per agent, for `policy_simulate`.
    pub simulate_rate_per_second: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_artifacts: DEFAULT_MAX_ARTIFACTS,
            max_journal_bytes: DEFAULT_MAX_JOURNAL_BYTES,
            agent_journal_bytes: DEFAULT_AGENT_JOURNAL_BYTES,
            agent_artifacts: DEFAULT_AGENT_ARTIFACTS,
            rate_per_second: 50.0,
            burst: 200.0,
            helper_rate_per_second: 10.0,
            simulate_rate_per_second: 2.0,
        }
    }
}

/// The server's time source. `trustgraph serve` uses `SystemClock`; tests
/// drive their own.
pub trait Clock: Send + Sync {
    /// Seconds since the Unix epoch, as the wall clock says now.
    fn wall(&self) -> u64;
    /// Time since an arbitrary fixed point; never goes backwards.
    fn monotonic(&self) -> Duration;
}

/// The system wall clock and `Instant`.
pub struct SystemClock(Instant);

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock(Instant::now())
    }
}

impl Clock for SystemClock {
    fn wall(&self) -> u64 {
        unix_now()
    }
    fn monotonic(&self) -> Duration {
        self.0.elapsed()
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    /// The person every approval, revocation and activation is made as.
    pub operator: String,
    /// The loopback port requests must name in `Host`.
    pub port: u16,
    pub state_dir: PathBuf,
    pub limits: Limits,
}

/// An error as the API returns it: `{"error": {"code", "message"}}`.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub diagnostics: Option<Value>,
    pub retry_after: Option<u64>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            code: code.into(),
            message: message.into(),
            diagnostics: None,
            retry_after: None,
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }

    fn unauthorized(code: &str, message: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, code, message)
    }

    fn restart_required() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "restart_required",
            "the journal failed and the outcome of a request is unknown; restart trustgraph serve",
        )
    }

    fn quota(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INSUFFICIENT_STORAGE, "quota_exceeded", message)
    }

    fn internal(message: &str) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    /// The one answer for an artifact that does not exist AND for one the
    /// agent may not use, so ids of other agents' content cannot be probed.
    fn unknown_artifact() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "unknown_artifact",
            "no artifact with that id that this agent may use",
        )
    }

    /// An engine `Rejected` reply.
    fn rejected(code: String, message: String) -> Self {
        let status = match code.as_str() {
            "unknown_artifact" | "unknown_decision" | "unknown_proposal" => StatusCode::NOT_FOUND,
            "agent_cannot_use_source" | "reviewer_not_allowed" => StatusCode::FORBIDDEN,
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        };
        ApiError {
            status,
            code,
            message,
            diagnostics: None,
            retry_after: None,
        }
    }

    pub fn body(&self) -> Value {
        let mut body = json!({"error": {"code": self.code, "message": self.message}});
        if let Some(d) = &self.diagnostics {
            body["diagnostics"] = d.clone();
        }
        body
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, axum::Json(self.body())).into_response();
        let headers = response.headers_mut();
        if let Some(seconds) = self.retry_after
            && let Ok(v) = HeaderValue::from_str(&seconds.to_string())
        {
            headers.insert(header::RETRY_AFTER, v);
        }
        if self.status == StatusCode::UNAUTHORIZED {
            headers.insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

type ApiResult = Result<Value, ApiError>;

struct Bucket {
    tokens: f64,
    at: Instant,
}

/// Which rate limit a request is charged to. Every agent request is charged
/// to `Requests`; the pure helpers also to their own, lower lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Lane {
    Requests,
    Helper,
    Simulate,
}

/// What one agent has made the server store: its journaled records since
/// the journal began (rebuilt at start), plus the sources it registered
/// while this process ran.
#[derive(Debug, Default, Clone, Copy)]
struct Usage {
    journal_bytes: u64,
    artifacts: u64,
}

/// The server's time, in seconds: `base` plus the elapsed time since start,
/// where elapsed is the largest of monotonic elapsed, wall-clock elapsed and
/// any elapsed value seen before. Monotonic time stops during sleep on
/// macOS and Linux, so wall time makes sleep count; the high-water mark
/// keeps a backward wall jump from undoing a forward one.
struct ServerTime {
    clock: Arc<dyn Clock>,
    base: u64,
    start_monotonic: Duration,
    start_wall: u64,
    elapsed: AtomicU64,
}

impl ServerTime {
    fn new(clock: Arc<dyn Clock>, base: u64) -> Self {
        ServerTime {
            base,
            start_monotonic: clock.monotonic(),
            start_wall: clock.wall(),
            elapsed: AtomicU64::new(0),
            clock,
        }
    }

    fn now(&self) -> u64 {
        let monotonic = self
            .clock
            .monotonic()
            .saturating_sub(self.start_monotonic)
            .as_secs();
        let wall = self.clock.wall().saturating_sub(self.start_wall);
        let seen = monotonic.max(wall);
        let elapsed = self.elapsed.fetch_max(seen, Ordering::SeqCst).max(seen);
        self.base.saturating_add(elapsed)
    }
}

/// The server: one runtime behind one mutex, plus the identity tables.
pub struct Server<J: ServeJournal> {
    settings: Settings,
    runtime: Mutex<Runtime<J>>,
    /// `(agent, token)` for every agent of the active policy with a token.
    tokens: RwLock<Vec<(String, String)>>,
    sessions: auth::OwnerSessions,
    buckets: Mutex<HashMap<(String, Lane), Bucket>>,
    /// Per agent; only touched while the runtime lock is held.
    usage: Mutex<HashMap<String, Usage>>,
    clock: Arc<dyn Clock>,
    time: ServerTime,
    /// When nonzero, the journal time the wall clock must reach before
    /// approvals are accepted again.
    behind_until: AtomicU64,
    /// Lines `main` prints at start about the policy file.
    report: Vec<String>,
    /// Why the server stopped serving; set once, never cleared.
    broken: Mutex<Option<String>>,
    /// The gateway's view for the operator page, when one is attached.
    gateway: OnceLock<Arc<dyn GatewayView>>,
}

/// What the operator routes need from an attached gateway.
pub(crate) trait GatewayView: Send + Sync {
    /// `connection` or `agent`.
    fn mode(&self) -> &'static str;
    /// Problems of the gateway config against `policy`, as lines.
    fn problems(&self, policy: &Policy) -> Vec<String>;
    /// Every live MCP connection, in both modes.
    fn connections(&self) -> Vec<ConnectionView>;
    /// Agents holding the most open connections allowed.
    fn at_cap(&self) -> Vec<String>;
    /// Close every MCP connection of `session` and forget that it may not
    /// act; false when it had no connection.
    fn close(&self, session: &str) -> bool;
    /// Close one MCP connection by its id; false when there is none.
    fn close_connection(&self, connection: &str) -> bool;
    /// What the operator sees of a call held for approval as `decision`:
    /// the tool and its forwarded arguments, from memory. `None` when the
    /// gateway no longer has it (it restarted, or dropped it).
    fn held_call(&self, decision: &str) -> Option<Value>;
}

/// One live MCP connection to the gateway, as the operator page shows it.
pub(crate) struct ConnectionView {
    /// An id for the operator page; not the MCP session id.
    pub connection: String,
    pub agent: String,
    pub session: String,
    pub started_at: u64,
    pub last_active: u64,
}

/// Why a call the gateway held cannot be approved after a restart.
const CALL_NOT_HELD: &str = "the gateway no longer has this call's arguments (it restarted, or dropped the call), so you cannot see what you would approve; the agent must retry the call before it can be approved";

/// The tool of a pending decision when the gateway recorded its output.
fn gateway_tool(engine: &Engine, artifact: &str) -> Option<String> {
    engine
        .output_step(artifact)?
        .strip_prefix("gateway:")
        .map(str::to_string)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Readers of anything derived from all of `sources`: the intersection of
/// each source's people. A source missing from `policy` has no readers.
fn readers(policy: &Policy, sources: &BTreeSet<String>) -> BTreeSet<String> {
    let mut out: Option<BTreeSet<String>> = None;
    for name in sources {
        let people = policy
            .sources
            .get(name)
            .map(|s| policy.people(&s.readers))
            .unwrap_or_default();
        out = Some(match out {
            None => people,
            Some(prev) => prev.intersection(&people).cloned().collect(),
        });
    }
    out.unwrap_or_default()
}

/// Parse, lint and test a policy text, as activation does; the diagnostics
/// as JSON when anything is an error.
fn validate_policy(text: &str) -> Result<(), ApiError> {
    let parsed = parse(text);
    let mut diagnostics = parsed.diagnostics;
    if let Some(policy) = &parsed.policy {
        diagnostics.extend(lint(policy));
        for test in run_tests(policy).into_iter().filter(|t| !t.pass) {
            diagnostics.push(trustgraph_policy::Diagnostic {
                severity: Severity::Error,
                path: "tests".into(),
                code: "test_failed",
                message: format!(
                    "test `{}` expects {} but the policy says {}",
                    test.name,
                    test.expect.as_str(),
                    test.outcome.verdict.as_str()
                ),
                suggestion: None,
            });
        }
    }
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    if parsed.policy.is_some() && errors == 0 {
        return Ok(());
    }
    let mut error = ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_policy",
        format!("the policy has {errors} error(s); see diagnostics"),
    );
    error.diagnostics = Some(json!(diagnostics));
    Err(error)
}

fn digest_of(sha256: Option<String>, content: Option<String>) -> Result<String, ApiError> {
    match (sha256, content) {
        (Some(sha), None) => Ok(sha),
        (None, Some(content)) => Ok(sha256_hex(content.as_bytes())),
        _ => Err(ApiError::bad_request(
            "send exactly one of `sha256` or `content`",
        )),
    }
}

/// The engine's shape bounds, checked before the runtime lock and journal.
fn shape(event: &Event) -> Result<(), ApiError> {
    event
        .validate_shape()
        .or_else(|reply| reply_value(reply).map(|_| ()))
}

fn reply_value(reply: Reply) -> ApiResult {
    Ok(match reply {
        Reply::Artifact { id } => json!({"artifact": id}),
        Reply::Decision {
            id,
            verdict,
            code,
            reason,
            blocking,
            expires_at,
        } => json!({
            "decision": id,
            "verdict": verdict,
            "code": code,
            "reason": reason,
            "blocking": blocking,
            "expires_at": expires_at,
        }),
        Reply::Approved {
            decision,
            expires_at,
        } => json!({"decision": decision, "approved": true, "expires_at": expires_at}),
        Reply::Revoked { decision } => json!({"decision": decision, "revoked": true}),
        Reply::Proposed {
            id,
            widens,
            changes,
            explanation,
        } => json!({
            "proposal": id,
            "widens": widens,
            "changes": changes,
            "explanation": explanation,
        }),
        Reply::Activated { revision, digest } => {
            json!({"revision": revision, "digest": digest})
        }
        Reply::Rejected { code, message } => return Err(ApiError::rejected(code, message)),
        _ => return Err(ApiError::internal("the engine sent an unknown reply")),
    })
}

/// `POST /v1/sources`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRequest {
    pub source: String,
    pub sha256: Option<String>,
    pub content: Option<String>,
}

/// `POST /v1/outputs`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputRequest {
    pub inputs: Vec<String>,
    pub step: String,
    pub sha256: Option<String>,
    pub content: Option<String>,
}

/// `POST /v1/check`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRequest {
    pub artifact: String,
    pub action: String,
    #[serde(default)]
    pub recipients: Option<Vec<String>>,
}

/// `{"policy": "<yaml>"}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRequest {
    pub policy: String,
}

/// `POST /v1/policy/diff`: `old` defaults to the active policy.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffRequest {
    #[serde(default)]
    pub old: Option<String>,
    pub new: String,
}

/// `POST /v1/policy/simulate`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulateRequest {
    pub policy: String,
    #[serde(default)]
    pub seed: u64,
    #[serde(default = "default_count")]
    pub count: u64,
    #[serde(default = "default_steps")]
    pub steps: u32,
}

fn default_count() -> u64 {
    MAX_SIMULATE_COUNT
}

fn default_steps() -> u32 {
    MAX_SIMULATE_STEPS
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionRequest {
    decision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposalRequest {
    proposal: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionRequest {
    session: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionRequest {
    connection: String,
}

impl<J: ServeJournal> Server<J> {
    /// A server over an opened runtime. Writes the token files of every agent
    /// in the active policy; any token problem is an error.
    pub fn new(runtime: Runtime<J>, settings: Settings) -> std::io::Result<Arc<Self>> {
        Self::with_clock(runtime, settings, Arc::new(SystemClock::default()))
    }

    /// `new` with its own time source.
    pub fn with_clock(
        runtime: Runtime<J>,
        settings: Settings,
        clock: Arc<dyn Clock>,
    ) -> std::io::Result<Arc<Self>> {
        Self::build(runtime, settings, clock, Vec::new())
    }

    fn build(
        runtime: Runtime<J>,
        settings: Settings,
        clock: Arc<dyn Clock>,
        report: Vec<String>,
    ) -> std::io::Result<Arc<Self>> {
        tokens::private_dir(&settings.state_dir)?;
        let agents = runtime
            .engine()
            .policy()
            .map(|p| p.agents.clone())
            .unwrap_or_default();
        let (tokens, problems) = tokens::sync(&settings.state_dir, &agents);
        if !problems.is_empty() {
            return Err(std::io::Error::other(problems.join("; ")));
        }
        let wall = clock.wall();
        let journaled = runtime.last_at();
        let behind_until = if journaled > wall.saturating_add(CLOCK_BEHIND_TOLERANCE) {
            eprintln!(
                "warning: the system clock ({wall}) is {} s behind the last journaled time ({journaled}); approvals are refused until the clock catches up",
                journaled - wall
            );
            journaled
        } else {
            0
        };
        // Per-agent budgets are lifetime budgets for this state directory.
        let usage = runtime
            .agent_usage()
            .iter()
            .map(|(agent, u)| {
                let usage = Usage {
                    journal_bytes: u.journal_bytes,
                    artifacts: u.artifacts,
                };
                (agent.clone(), usage)
            })
            .collect();
        Ok(Arc::new(Server {
            settings,
            runtime: Mutex::new(runtime),
            tokens: RwLock::new(tokens),
            sessions: auth::OwnerSessions::new(auth::SESSION_LIFETIME),
            buckets: Mutex::new(HashMap::new()),
            usage: Mutex::new(usage),
            time: ServerTime::new(clock.clone(), wall.max(journaled)),
            clock,
            behind_until: AtomicU64::new(behind_until),
            report,
            broken: Mutex::new(None),
            gateway: OnceLock::new(),
        }))
    }

    /// What `main` prints at start about the policy file.
    pub fn startup_report(&self) -> &[String] {
        &self.report
    }

    /// True when the policy in force is the policy file as it is now.
    pub fn file_in_force(&self) -> bool {
        self.lock().is_ok_and(|rt| {
            rt.policy_source() == trustgraph_engine::PolicySource::File
                && matches!(
                    rt.file_outcome(),
                    FileOutcome::Genesis | FileOutcome::Unchanged | FileOutcome::Activated { .. }
                )
        })
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn token_dir(&self) -> PathBuf {
        tokens::dir(&self.settings.state_dir)
    }

    /// Write the single-use operator launch file, `<state>/operator.html`.
    pub fn write_launch_file(&self) -> std::io::Result<PathBuf> {
        let path = self.settings.state_dir.join("operator.html");
        let secret = self.sessions.launch_secret();
        self.sessions.write_launch_file(
            &path,
            &format!(
                "<!doctype html><meta name=referrer content=no-referrer><meta http-equiv=refresh content=\"0;url=http://127.0.0.1:{}/#/connect/{secret}\">",
                self.settings.port
            ),
        )?;
        Ok(path)
    }

    /// The current, unused launch secret, for tests and the launch file.
    pub fn launch_secret(&self) -> Option<String> {
        Some(self.sessions.launch_secret())
    }

    /// `revision`, `digest` and `source` of the active policy.
    pub fn policy_summary(&self) -> Result<(u64, String, &'static str), ApiError> {
        let rt = self.lock()?;
        let digest = rt.engine().policy().map(Policy::digest).unwrap_or_default();
        Ok((
            rt.engine().policy_revision(),
            digest,
            rt.policy_source().as_str(),
        ))
    }

    /// Bytes in the journal; `None` when unknown or the server stopped.
    pub fn journal_size(&self) -> Option<u64> {
        self.lock().ok()?.journal().size()
    }

    fn broken(&self) -> Option<String> {
        self.broken
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn mark_broken(&self, why: String) {
        let mut broken = self.broken.lock().unwrap_or_else(|p| p.into_inner());
        if broken.is_none() {
            eprintln!("trustgraph serve: stopped serving: {why}; restart required");
            *broken = Some(why);
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Runtime<J>>, ApiError> {
        if self.broken().is_some() {
            return Err(ApiError::restart_required());
        }
        let guard = self.runtime.lock().map_err(|_| {
            self.mark_broken("a request failed while holding the runtime".into());
            ApiError::restart_required()
        })?;
        if self.broken().is_some() {
            return Err(ApiError::restart_required());
        }
        Ok(guard)
    }

    /// Seconds: the server's time (see `ServerTime`), never before the last
    /// journaled record.
    fn now(&self, rt: &Runtime<J>) -> u64 {
        self.time.now().max(rt.last_at())
    }

    /// True while the wall clock is still behind the journal it found at start.
    fn clock_behind(&self) -> bool {
        let until = self.behind_until.load(Ordering::SeqCst);
        if until == 0 {
            return false;
        }
        if self.clock.wall() >= until {
            self.behind_until.store(0, Ordering::SeqCst);
            eprintln!(
                "trustgraph serve: the system clock caught up with the journal; approvals work again"
            );
            return false;
        }
        true
    }

    fn handle(&self, rt: &mut Runtime<J>, event: Event) -> Result<Reply, ApiError> {
        let at = self.now(rt);
        rt.handle(at, event).map_err(|e| {
            self.mark_broken(format!("journal failure: {e}"));
            ApiError::restart_required()
        })
    }

    fn usage(&self, agent: &str) -> Usage {
        self.usage
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(agent)
            .copied()
            .unwrap_or_default()
    }

    /// Quotas checked before an agent write is journaled: the server's and
    /// the agent's own.
    fn agent_quota(
        &self,
        rt: &Runtime<J>,
        agent: &str,
        new_artifact: bool,
    ) -> Result<(), ApiError> {
        let limits = &self.settings.limits;
        let usage = self.usage(agent);
        if usage.journal_bytes >= limits.agent_journal_bytes {
            return Err(ApiError::quota(format!(
                "agent `{agent}` has written its limit of {} journal bytes in this state directory",
                limits.agent_journal_bytes
            )));
        }
        if new_artifact && usage.artifacts >= limits.agent_artifacts {
            return Err(ApiError::quota(format!(
                "agent `{agent}` has created its limit of {} artifacts in this state directory",
                limits.agent_artifacts
            )));
        }
        match rt.journal().size() {
            Some(size) if size < limits.max_journal_bytes => {}
            _ => {
                return Err(ApiError::quota(format!(
                    "the journal has reached its limit of {} bytes",
                    limits.max_journal_bytes
                )));
            }
        }
        if new_artifact && rt.engine().artifact_count() >= limits.max_artifacts {
            return Err(ApiError::quota(format!(
                "the server holds its limit of {} artifacts",
                limits.max_artifacts
            )));
        }
        Ok(())
    }

    /// `handle` for an agent's event, charging what it stored to the agent.
    fn agent_handle(
        &self,
        rt: &mut Runtime<J>,
        agent: &str,
        event: Event,
    ) -> Result<Reply, ApiError> {
        let bytes = rt.journal().size().unwrap_or(0);
        let artifacts = rt.engine().artifact_count();
        let reply = self.handle(rt, event)?;
        let written = rt
            .journal()
            .size()
            .unwrap_or(u64::MAX)
            .saturating_sub(bytes);
        let created = rt.engine().artifact_count().saturating_sub(artifacts) as u64;
        let mut usage = self.usage.lock().unwrap_or_else(|p| p.into_inner());
        let entry = usage.entry(agent.to_string()).or_default();
        entry.journal_bytes = entry.journal_bytes.saturating_add(written);
        entry.artifacts = entry.artifacts.saturating_add(created);
        Ok(reply)
    }

    /// The source names `artifact` derives from, when it exists and `agent`
    /// may use every one of them; otherwise the uniform `unknown_artifact`.
    /// Both outcomes walk the lineage and the policy the same way.
    fn visible_sources(
        engine: &Engine,
        policy: &Policy,
        agent: &str,
        artifact: &str,
    ) -> Result<BTreeSet<String>, ApiError> {
        let lineage = engine.lineage(artifact);
        let known = lineage.is_some();
        let sources = lineage.unwrap_or_default();
        let allowed = sources.iter().fold(true, |ok, name| {
            ok & policy
                .sources
                .get(name)
                .is_some_and(|s| s.agents.contains(agent))
        });
        if known & allowed & !sources.is_empty() {
            Ok(sources)
        } else {
            Err(ApiError::unknown_artifact())
        }
    }

    /// The agent whose token is the request's bearer credential, charged one
    /// request against its rate limit.
    pub fn authenticate_agent(&self, headers: &HeaderMap) -> Result<String, ApiError> {
        let missing =
            || ApiError::unauthorized("unauthorized", "a valid agent bearer token is required");
        let presented = auth::bearer(headers).ok_or_else(missing)?;
        let tokens = self.tokens.read().unwrap_or_else(|p| p.into_inner());
        // Compare against every token so timing reveals nothing.
        let mut found: Vec<&str> = Vec::new();
        for (agent, token) in tokens.iter() {
            if auth::constant_time_eq(token.as_bytes(), presented.as_bytes()) {
                found.push(agent);
            }
        }
        // A token held by two agents identifies nobody.
        let [agent] = found[..] else {
            return Err(missing());
        };
        let agent = agent.to_string();
        drop(tokens);
        self.charge(&agent, Lane::Requests)?;
        Ok(agent)
    }

    /// Charge one call of the pure helper `name` (`check`, `test`,
    /// `explain`, `diff`, `schema`, `simulate`) to its own, lower rate limit.
    pub fn charge_helper(&self, agent: &str, name: &str) -> Result<(), ApiError> {
        let lane = if name == "simulate" {
            Lane::Simulate
        } else {
            Lane::Helper
        };
        self.charge(agent, lane)
    }

    fn charge(&self, agent: &str, lane: Lane) -> Result<(), ApiError> {
        let limits = &self.settings.limits;
        let (rate, burst) = match lane {
            Lane::Requests => (limits.rate_per_second, limits.burst),
            Lane::Helper => (
                limits.helper_rate_per_second,
                limits.helper_rate_per_second.max(1.0),
            ),
            Lane::Simulate => (
                limits.simulate_rate_per_second,
                limits.simulate_rate_per_second.max(1.0),
            ),
        };
        let mut buckets = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        let bucket = buckets.entry((agent.to_string(), lane)).or_insert(Bucket {
            tokens: burst,
            at: now,
        });
        let refill = now.duration_since(bucket.at).as_secs_f64() * rate;
        bucket.tokens = (bucket.tokens + refill).min(burst);
        bucket.at = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Ok(());
        }
        let wait = ((1.0 - bucket.tokens) / rate).ceil().max(1.0);
        let mut error = ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many requests; retry later",
        );
        error.retry_after = Some(wait as u64);
        Err(error)
    }

    /// The operator session header, and no `Authorization` header: an agent
    /// token never reaches an operator route, even next to a valid session.
    fn require_operator(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        let required = || {
            ApiError::unauthorized(
                "operator_session_required",
                "sign in by opening the operator launch file",
            )
        };
        if headers.contains_key(header::AUTHORIZATION) {
            return Err(required());
        }
        match self.sessions.check(headers, self.time.now()) {
            auth::Session::Valid => Ok(()),
            auth::Session::Replaced => Err(ApiError::unauthorized(
                "operator_session_replaced",
                "Signed out: a newer operator session was started. If that was not you, stop the server.",
            )),
            auth::Session::Invalid => Err(required()),
        }
    }

    // ----- agent operations (HTTP and MCP share these) -----

    pub fn register_source(&self, agent: &str, request: SourceRequest) -> ApiResult {
        let sha = digest_of(request.sha256, request.content)?;
        let event = Event::RegisterSource {
            source: request.source,
            sha256: sha,
        };
        shape(&event)?;
        let Event::RegisterSource { source, sha256 } = &event else {
            return Err(ApiError::internal("unexpected event"));
        };
        let mut rt = self.lock()?;
        let policy = rt
            .engine()
            .policy()
            .ok_or_else(|| ApiError::internal("no active policy"))?;
        let Some(rule) = policy.sources.get(source) else {
            return Err(ApiError::rejected(
                "unknown_source".into(),
                "that is not a source in the active policy".into(),
            ));
        };
        if !rule.agents.contains(agent) {
            return Err(ApiError::rejected(
                "agent_cannot_use_source".into(),
                format!("agent `{agent}` may not use this source"),
            ));
        }
        // Already registered: the same answer, nothing journaled or charged.
        if let Some(id) = rt.engine().source_artifact(source, sha256) {
            return Ok(json!({"artifact": id}));
        }
        self.agent_quota(&rt, agent, true)?;
        let reply = self.agent_handle(&mut rt, agent, event)?;
        reply_value(reply)
    }

    pub fn record_output(&self, agent: &str, request: OutputRequest) -> ApiResult {
        let sha = digest_of(request.sha256, request.content)?;
        let event = Event::RecordOutput {
            agent: agent.into(),
            step: request.step,
            inputs: request.inputs,
            sha256: sha,
        };
        shape(&event)?;
        let Event::RecordOutput {
            step,
            inputs,
            sha256,
            ..
        } = &event
        else {
            return Err(ApiError::internal("unexpected event"));
        };
        let mut rt = self.lock()?;
        let engine = rt.engine();
        let policy = engine
            .policy()
            .ok_or_else(|| ApiError::internal("no active policy"))?;
        // Walk every input, so the answer takes the same work whichever
        // input is unknown or not the agent's.
        let visible = inputs.iter().fold(true, |ok, input| {
            ok & Self::visible_sources(engine, policy, agent, input).is_ok()
        });
        if !visible {
            return Err(ApiError::unknown_artifact());
        }
        if let Some(id) = engine.output_artifact(agent, step, inputs, sha256) {
            return Ok(json!({"artifact": id}));
        }
        self.agent_quota(&rt, agent, true)?;
        let reply = self.agent_handle(&mut rt, agent, event)?;
        reply_value(reply)
    }

    pub fn check(&self, agent: &str, request: CheckRequest) -> ApiResult {
        let mut rt = self.lock()?;
        self.check_in(&mut rt, agent, request)
    }

    /// `check` for one gateway call: when the allow comes from a live
    /// approval, that approval is revoked (journaled) under the same lock,
    /// so it releases exactly one call. `consumed` says whether it did.
    pub(crate) fn check_once(&self, agent: &str, request: CheckRequest) -> ApiResult {
        let mut rt = self.lock()?;
        let mut value = self.check_in(&mut rt, agent, request)?;
        if value["verdict"] != "allow" {
            return Ok(value);
        }
        let decision = value["decision"].as_str().unwrap_or_default().to_string();
        let now = self.now(&rt);
        let Some(approval) = rt
            .engine()
            .live_approvals(now)
            .into_iter()
            .find(|a| a.decision == decision)
        else {
            value["consumed"] = json!(false);
            return Ok(value);
        };
        let event = Event::Revoke {
            decision,
            reviewer: approval.reviewer,
        };
        match self.handle(&mut rt, event)? {
            Reply::Revoked { .. } => {
                value["consumed"] = json!(true);
                Ok(value)
            }
            _ => Err(ApiError::new(
                StatusCode::CONFLICT,
                "approval_not_consumed",
                "the approval could not be used up, so the call was not forwarded",
            )),
        }
    }

    fn check_in(&self, rt: &mut Runtime<J>, agent: &str, request: CheckRequest) -> ApiResult {
        let event = Event::Check {
            agent: agent.into(),
            artifact: request.artifact,
            action: request.action,
            recipients: request.recipients.map(|r| r.into_iter().collect()),
        };
        shape(&event)?;
        let Event::Check { artifact, .. } = &event else {
            return Err(ApiError::internal("unexpected event"));
        };
        let engine = rt.engine();
        let policy = engine
            .policy()
            .ok_or_else(|| ApiError::internal("no active policy"))?;
        Self::visible_sources(engine, policy, agent, artifact)?;
        self.agent_quota(rt, agent, false)?;
        let reply = self.agent_handle(rt, agent, event)?;
        reply_value(reply)
    }

    pub fn explain_artifact(&self, agent: &str, artifact: &str) -> ApiResult {
        let rt = self.lock()?;
        let engine = rt.engine();
        let policy = engine
            .policy()
            .ok_or_else(|| ApiError::internal("no active policy"))?;
        let sources = Self::visible_sources(engine, policy, agent, artifact)?;
        Ok(json!({
            "artifact": artifact,
            "readers": readers(policy, &sources),
            "sources": sources,
        }))
    }

    pub fn propose(&self, agent: Option<&str>, request: PolicyRequest) -> ApiResult {
        validate_policy(&request.policy)?;
        let event = Event::ProposePolicy {
            agent: agent.map(str::to_string),
            policy: request.policy,
        };
        shape(&event)?;
        let mut rt = self.lock()?;
        let reply = match agent {
            Some(agent) => {
                self.agent_quota(&rt, agent, false)?;
                self.agent_handle(&mut rt, agent, event)?
            }
            None => self.handle(&mut rt, event)?,
        };
        reply_value(reply)
    }

    pub fn policy_get(&self) -> ApiResult {
        let rt = self.lock()?;
        let engine = rt.engine();
        let policy = engine
            .policy()
            .ok_or_else(|| ApiError::internal("no active policy"))?;
        Ok(json!({
            "policy": policy.to_yaml(),
            "revision": engine.policy_revision(),
            "digest": policy.digest(),
        }))
    }

    pub fn policy_diff(&self, request: DiffRequest) -> ApiResult {
        let old = match request.old {
            Some(old) => old,
            None => {
                let rt = self.lock()?;
                rt.engine()
                    .policy()
                    .map(Policy::to_yaml)
                    .ok_or_else(|| ApiError::internal("no active policy"))?
            }
        };
        Ok(policy_text::diff(&old, &request.new))
    }

    // ----- operator operations -----

    fn operator_state(&self) -> ApiResult {
        let rt = self.lock()?;
        let now = self.now(&rt);
        let engine = rt.engine();
        let operator = &self.settings.operator;
        let policy = engine.policy();
        let pending: Vec<Value> = engine
            .pending_decisions(now)
            .into_iter()
            .map(|p| {
                let reviewers: BTreeSet<String> = policy
                    .and_then(|policy| {
                        let rule = policy.actions.get(&p.action)?.approval.as_ref()?;
                        Some(policy.people(&rule.reviewers))
                    })
                    .unwrap_or_default();
                let sources: BTreeSet<String> = p.sources.iter().cloned().collect();
                let reads =
                    policy.is_some_and(|policy| readers(policy, &sources).contains(operator));
                // A call the gateway holds: what it would forward, from memory.
                let call = self.gateway.get().and_then(|g| {
                    let tool = gateway_tool(engine, &p.artifact)?;
                    Some(
                        g.held_call(&p.decision)
                            .unwrap_or_else(|| json!({"tool": tool, "held": false})),
                    )
                });
                let unseen = call.as_ref().is_some_and(|c| c["held"] != true);
                // The engine's own rule; the page only hides what would fail.
                let blocked = if !reviewers.contains(operator) {
                    Some("you are not a reviewer for this action")
                } else if !reads {
                    Some("you may not read this output, so you cannot approve it")
                } else if &p.agent == operator {
                    Some("an agent cannot approve its own request")
                } else if unseen {
                    Some(CALL_NOT_HELD)
                } else {
                    None
                };
                json!({
                    "gateway": call,
                    "decision": p.decision,
                    "agent": p.agent,
                    "action": p.action,
                    "recipients": p.recipients,
                    "sources": p.sources,
                    "reason": p.reason,
                    "requested_at": p.requested_at,
                    "reviewers": reviewers,
                    "can_approve": blocked.is_none(),
                    "cannot_approve": blocked,
                })
            })
            .collect();
        let approvals: Vec<Value> = engine
            .live_approvals(now)
            .into_iter()
            .map(|a| {
                json!({
                    "decision": a.decision,
                    "agent": a.agent,
                    "action": a.action,
                    "recipients": a.recipients,
                    "expires_at": a.expires_at,
                })
            })
            .collect();
        let proposals: Vec<Value> = engine
            .stored_proposals()
            .into_iter()
            .map(|p| {
                let (origin, from) = match &p.origin {
                    ProposalOrigin::Operator => ("operator", "operator"),
                    ProposalOrigin::File => ("file", "file"),
                    ProposalOrigin::Agent(agent) => ("agent", agent.as_str()),
                };
                json!({
                    "proposal": p.proposal,
                    "origin": origin,
                    "from": from,
                    "widens": p.widens,
                    "changes": p.changes,
                    "explanation": p.explanation,
                })
            })
            .collect();
        let gateway = self.gateway.get().map(|g| {
            let mut sessions: BTreeMap<String, Value> = engine
                .sessions()
                .into_iter()
                .map(|s| {
                    let row = json!({
                        "session": s.session,
                        "agent": s.agent,
                        "started_at": s.started_at,
                        "sources": s.sources,
                    });
                    (s.session, row)
                })
                .collect();
            let connections = g.connections();
            for c in &connections {
                // A connection-mode session exists before it reads anything.
                if c.session.starts_with("conn:") {
                    sessions.entry(c.session.clone()).or_insert_with(|| {
                        json!({
                            "session": c.session,
                            "agent": c.agent,
                            "started_at": c.started_at,
                            "sources": [],
                        })
                    });
                }
            }
            let connections: Vec<Value> = connections
                .iter()
                .map(|c| {
                    json!({
                        "connection": c.connection,
                        "agent": c.agent,
                        "session": c.session,
                        "started_at": c.started_at,
                        "last_active": c.last_active,
                    })
                })
                .collect();
            json!({
                "mode": g.mode(),
                "problems": policy.map(|p| g.problems(p)).unwrap_or_default(),
                "sessions": sessions.into_values().collect::<Vec<_>>(),
                "connections": connections,
                "max_connections_per_agent": gateway::MAX_CONNECTIONS_PER_AGENT,
                "at_cap": g.at_cap(),
            })
        });
        Ok(json!({
            "operator": self.settings.operator,
            "now": now,
            "gateway": gateway,
            "clock_behind": self.behind_until.load(Ordering::SeqCst) != 0,
            "policy": {
                "revision": engine.policy_revision(),
                "digest": engine.policy().map(Policy::digest),
                "source": rt.policy_source().as_str(),
            },
            "pending": pending,
            "approvals": approvals,
            "proposals": proposals,
        }))
    }

    fn approve(&self, decision: String) -> ApiResult {
        if self.clock_behind() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "clock_behind",
                "the system clock is behind the journal; approvals are refused until it catches up",
            ));
        }
        let mut rt = self.lock()?;
        // A call the gateway holds is approved only while the operator can
        // see what it would forward.
        if let Some(g) = self.gateway.get() {
            let now = self.now(&rt);
            let unseen = rt
                .engine()
                .pending_decisions(now)
                .iter()
                .find(|p| p.decision == decision)
                .is_some_and(|p| {
                    gateway_tool(rt.engine(), &p.artifact).is_some()
                        && g.held_call(&decision).is_none()
                });
            if unseen {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "call_not_held",
                    CALL_NOT_HELD,
                ));
            }
        }
        let reviewer = self.settings.operator.clone();
        let reply = self.handle(&mut rt, Event::Approve { decision, reviewer })?;
        reply_value(reply)
    }

    /// Close one gateway MCP connection; the session it served is kept.
    fn end_connection(&self, connection: String) -> ApiResult {
        if self
            .gateway
            .get()
            .is_some_and(|g| g.close_connection(&connection))
        {
            Ok(json!({"connection": connection, "ended": true}))
        } else {
            Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "unknown_connection",
                "no open gateway connection with that id",
            ))
        }
    }

    fn revoke(&self, decision: String) -> ApiResult {
        let mut rt = self.lock()?;
        let reviewer = self.settings.operator.clone();
        let reply = self.handle(&mut rt, Event::Revoke { decision, reviewer })?;
        reply_value(reply)
    }

    /// End a gateway session: forget what it carries and close every MCP
    /// connection it has (in `agent` mode, all of that agent's), so the
    /// client must initialise again. Only the operator can.
    fn end_session(&self, session: String) -> ApiResult {
        let mut rt = self.lock()?;
        let carried = rt.engine().session(&session).is_some();
        let connected = self.gateway.get().is_some_and(|g| g.close(&session));
        if carried {
            let event = Event::EndSession {
                session: session.clone(),
            };
            match self.handle(&mut rt, event)? {
                Reply::SessionEnded { .. } => {}
                other => return reply_value(other),
            }
        }
        if !carried && !connected {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "unknown_session",
                "no live session with that id",
            ));
        }
        Ok(json!({"session": session, "ended": true}))
    }

    fn activate(&self, proposal: String) -> ApiResult {
        let mut rt = self.lock()?;
        let operator = self.settings.operator.clone();
        let reply = self.handle(&mut rt, Event::ActivatePolicy { proposal, operator })?;
        let value = reply_value(reply)?;
        // Still under the runtime lock: token files follow the new agents.
        let agents = rt
            .engine()
            .policy()
            .map(|p| p.agents.clone())
            .unwrap_or_default();
        let (tokens, problems) = tokens::sync(&self.settings.state_dir, &agents);
        for problem in &problems {
            eprintln!("trustgraph serve: {problem}");
        }
        *self.tokens.write().unwrap_or_else(|p| p.into_inner()) = tokens;
        Ok(value)
    }
}

/// Run `f` on a blocking thread; the runtime lock is never held across an await.
async fn blocking<J: ServeJournal>(
    server: &Arc<Server<J>>,
    f: impl FnOnce(&Server<J>) -> ApiResult + Send + 'static,
) -> ApiResult {
    let server = server.clone();
    tokio::task::spawn_blocking(move || f(&server))
        .await
        .unwrap_or_else(|_| Err(ApiError::internal("the request failed")))
}

/// Read a JSON body of at most `BODY_LIMIT` bytes into `T`.
pub async fn read_json<T: serde::de::DeserializeOwned>(request: Request) -> Result<T, ApiError> {
    let too_large = || {
        ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
            format!("the request body is larger than {BODY_LIMIT} bytes"),
        )
    };
    let json_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    if !json_type {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "send `Content-Type: application/json`",
        ));
    }
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    // Read past the limit (up to `DRAIN_LIMIT`, then stop) so a client still
    // sending gets the 413 instead of a reset connection.
    if declared.is_some_and(|n| n > DRAIN_LIMIT as u64) {
        return Err(too_large());
    }
    let mut body = request.into_body();
    let mut bytes = Vec::new();
    let mut total = 0usize;
    while let Some(frame) = body.frame().await {
        let frame =
            frame.map_err(|_| ApiError::bad_request("the request body could not be read"))?;
        if let Ok(data) = frame.into_data() {
            total = total.saturating_add(data.len());
            if total <= BODY_LIMIT {
                bytes.extend_from_slice(&data);
            } else if total > DRAIN_LIMIT {
                break;
            }
        }
    }
    if total > BODY_LIMIT {
        return Err(too_large());
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::bad_request(format!("the request body is not valid: {e}")))
}

fn respond(result: ApiResult) -> Response {
    match result {
        Ok(value) => axum::Json(value).into_response(),
        Err(error) => error.into_response(),
    }
}

/// Host, Origin and health for every route, and the headers every response carries.
async fn guard<J: ServeJournal>(
    State(server): State<Arc<Server<J>>>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let mut response = match auth::loopback_host(headers, server.settings.port) {
        None => ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden_host",
            "requests must be sent to this server's loopback address and port",
        )
        .into_response(),
        Some(host) if !auth::same_origin(headers, host) => ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden_origin",
            "cross-origin requests are refused",
        )
        .into_response(),
        Some(_) if server.broken().is_some() => ApiError::restart_required().into_response(),
        Some(_) => next.run(request).await,
    };
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    response
}

type Shared<J> = State<Arc<Server<J>>>;

macro_rules! agent_post {
    ($name:ident, $body:ty, $op:ident) => {
        async fn $name<J: ServeJournal>(State(s): Shared<J>, request: Request) -> Response {
            respond(
                async {
                    let agent = s.authenticate_agent(request.headers())?;
                    let body: $body = read_json(request).await?;
                    blocking(&s, move |s| s.$op(&agent, body)).await
                }
                .await,
            )
        }
    };
}

agent_post!(post_source, SourceRequest, register_source);
agent_post!(post_output, OutputRequest, record_output);
agent_post!(post_check, CheckRequest, check);

async fn get_artifact<J: ServeJournal>(
    State(s): Shared<J>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    respond(
        async {
            let agent = s.authenticate_agent(&headers)?;
            blocking(&s, move |s| s.explain_artifact(&agent, &id)).await
        }
        .await,
    )
}

async fn post_propose<J: ServeJournal>(State(s): Shared<J>, request: Request) -> Response {
    respond(
        async {
            let agent = s.authenticate_agent(request.headers())?;
            let body: PolicyRequest = read_json(request).await?;
            blocking(&s, move |s| s.propose(Some(&agent), body)).await
        }
        .await,
    )
}

async fn get_policy<J: ServeJournal>(State(s): Shared<J>, headers: HeaderMap) -> Response {
    respond(
        async {
            s.authenticate_agent(&headers)?;
            blocking(&s, |s| s.policy_get()).await
        }
        .await,
    )
}

async fn get_schema<J: ServeJournal>(State(s): Shared<J>, headers: HeaderMap) -> Response {
    respond(
        s.authenticate_agent(&headers)
            .and_then(|agent| s.charge_helper(&agent, "schema"))
            .map(|_| trustgraph_policy::schema()),
    )
}

/// The pure policy helpers. None of them touch the runtime's state.
pub fn policy_helper(name: &str, body: Value) -> ApiResult {
    let parse_body = |body: Value| -> Result<PolicyRequest, ApiError> {
        serde_json::from_value(body).map_err(|e| ApiError::bad_request(e.to_string()))
    };
    Ok(match name {
        "check" => policy_text::check(&parse_body(body)?.policy),
        "test" => policy_text::test(&parse_body(body)?.policy),
        "explain" => policy_text::explain(&parse_body(body)?.policy),
        "simulate" => {
            let r: SimulateRequest =
                serde_json::from_value(body).map_err(|e| ApiError::bad_request(e.to_string()))?;
            if !(1..=MAX_SIMULATE_COUNT).contains(&r.count)
                || !(1..=MAX_SIMULATE_STEPS).contains(&r.steps)
                || r.seed.checked_add(r.count).is_none()
            {
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_request",
                    format!(
                        "count must be 1 to {MAX_SIMULATE_COUNT} and steps 1 to {MAX_SIMULATE_STEPS}"
                    ),
                ));
            }
            policy_text::simulate(&r.policy, r.seed, r.count, r.steps)
        }
        _ => return Err(ApiError::internal("unknown helper")),
    })
}

macro_rules! helper_post {
    ($name:ident, $helper:literal) => {
        async fn $name<J: ServeJournal>(State(s): Shared<J>, request: Request) -> Response {
            respond(
                async {
                    let agent = s.authenticate_agent(request.headers())?;
                    s.charge_helper(&agent, $helper)?;
                    let body: Value = read_json(request).await?;
                    blocking(&s, move |_| policy_helper($helper, body)).await
                }
                .await,
            )
        }
    };
}

helper_post!(post_policy_check, "check");
helper_post!(post_policy_test, "test");
helper_post!(post_policy_explain, "explain");
helper_post!(post_policy_simulate, "simulate");

async fn post_policy_diff<J: ServeJournal>(State(s): Shared<J>, request: Request) -> Response {
    respond(
        async {
            let agent = s.authenticate_agent(request.headers())?;
            s.charge_helper(&agent, "diff")?;
            let body: DiffRequest = read_json(request).await?;
            blocking(&s, move |s| s.policy_diff(body)).await
        }
        .await,
    )
}

/// Exchange the launch secret for THE operator session (ending any other)
/// and write a new single-use launch file for the next sign-in.
async fn session<J: ServeJournal>(State(s): Shared<J>, headers: HeaderMap) -> Response {
    let now = s.time.now();
    let Some((id, expires_at)) =
        auth::bearer(&headers).and_then(|secret| s.sessions.exchange(secret, now))
    else {
        return ApiError::unauthorized(
            "invalid_launch_secret",
            "this sign-in link is not valid or was already used. If you did not use it, stop the server: someone else holds the operator session",
        )
        .into_response();
    };
    let server = s.clone();
    let written = tokio::task::spawn_blocking(move || server.write_launch_file()).await;
    match written {
        Ok(Ok(path)) => eprintln!(
            "trustgraph serve: operator signed in; a new single-use sign-in link is at {}",
            path.display()
        ),
        Ok(Err(e)) => {
            eprintln!("trustgraph serve: cannot write the next operator launch file: {e}")
        }
        Err(_) => eprintln!("trustgraph serve: cannot write the next operator launch file"),
    }
    axum::Json(json!({"session": id, "expires_at": expires_at})).into_response()
}

async fn operator_state<J: ServeJournal>(State(s): Shared<J>, headers: HeaderMap) -> Response {
    respond(
        async {
            s.require_operator(&headers)?;
            blocking(&s, |s| s.operator_state()).await
        }
        .await,
    )
}

async fn operator_approve<J: ServeJournal>(State(s): Shared<J>, request: Request) -> Response {
    respond(
        async {
            s.require_operator(request.headers())?;
            let body: DecisionRequest = read_json(request).await?;
            blocking(&s, move |s| s.approve(body.decision)).await
        }
        .await,
    )
}

async fn operator_revoke<J: ServeJournal>(State(s): Shared<J>, request: Request) -> Response {
    respond(
        async {
            s.require_operator(request.headers())?;
            let body: DecisionRequest = read_json(request).await?;
            blocking(&s, move |s| s.revoke(body.decision)).await
        }
        .await,
    )
}

async fn operator_activate<J: ServeJournal>(State(s): Shared<J>, request: Request) -> Response {
    respond(
        async {
            s.require_operator(request.headers())?;
            let body: ProposalRequest = read_json(request).await?;
            blocking(&s, move |s| s.activate(body.proposal)).await
        }
        .await,
    )
}

async fn operator_end_session<J: ServeJournal>(State(s): Shared<J>, request: Request) -> Response {
    respond(
        async {
            s.require_operator(request.headers())?;
            let body: SessionRequest = read_json(request).await?;
            blocking(&s, move |s| s.end_session(body.session)).await
        }
        .await,
    )
}

async fn operator_end_connection<J: ServeJournal>(
    State(s): Shared<J>,
    request: Request,
) -> Response {
    respond(
        async {
            s.require_operator(request.headers())?;
            let body: ConnectionRequest = read_json(request).await?;
            blocking(&s, move |s| s.end_connection(body.connection)).await
        }
        .await,
    )
}

async fn operator_propose<J: ServeJournal>(State(s): Shared<J>, request: Request) -> Response {
    respond(
        async {
            s.require_operator(request.headers())?;
            let body: PolicyRequest = read_json(request).await?;
            blocking(&s, move |s| s.propose(None, body)).await
        }
        .await,
    )
}

fn asset(content_type: &'static str, body: &'static str) -> Response {
    ([(header::CONTENT_TYPE, content_type)], body).into_response()
}

async fn not_found() -> Response {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "no such route").into_response()
}

/// Every route, behind the Host/Origin/health guard.
pub fn router<J: ServeJournal>(server: Arc<Server<J>>) -> Router {
    router_with(server, Router::new())
}

/// `router` with `extra` routes (the gateway's) behind the same guard.
fn router_with<J: ServeJournal>(server: Arc<Server<J>>, extra: Router<Arc<Server<J>>>) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { asset("text/html; charset=utf-8", PAGE_HTML) }),
        )
        .route("/serve.css", get(|| async { asset("text/css", PAGE_CSS) }))
        .route(
            "/serve.js",
            get(|| async { asset("text/javascript", PAGE_JS) }),
        )
        .route("/session", post(session::<J>))
        .route("/v1/sources", post(post_source::<J>))
        .route("/v1/outputs", post(post_output::<J>))
        .route("/v1/check", post(post_check::<J>))
        .route("/v1/artifacts/{id}", get(get_artifact::<J>))
        .route("/v1/policy", get(get_policy::<J>))
        .route("/v1/policy/schema", get(get_schema::<J>))
        .route("/v1/policy/propose", post(post_propose::<J>))
        .route("/v1/policy/check", post(post_policy_check::<J>))
        .route("/v1/policy/test", post(post_policy_test::<J>))
        .route("/v1/policy/explain", post(post_policy_explain::<J>))
        .route("/v1/policy/diff", post(post_policy_diff::<J>))
        .route("/v1/policy/simulate", post(post_policy_simulate::<J>))
        .route("/v1/operator/state", get(operator_state::<J>))
        .route("/v1/operator/approve", post(operator_approve::<J>))
        .route("/v1/operator/revoke", post(operator_revoke::<J>))
        .route("/v1/operator/activate", post(operator_activate::<J>))
        .route("/v1/operator/propose", post(operator_propose::<J>))
        .route("/v1/operator/end-session", post(operator_end_session::<J>))
        .route(
            "/v1/operator/end-connection",
            post(operator_end_connection::<J>),
        )
        .merge(mcp::router(server.clone()))
        .merge(extra)
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(server.clone(), guard::<J>))
        .with_state(server)
}

// ----- starting the real server -----

/// What `start` needs from the command line.
#[derive(Debug, Clone)]
pub struct Options {
    pub policy: PathBuf,
    pub operator: String,
    pub state: Option<PathBuf>,
    pub allow_state_in_repository: bool,
    /// Apply a policy file that differs from the active policy at start,
    /// instead of storing it as a proposal for the operator.
    pub activate_policy_file: bool,
    pub limits: Limits,
}

/// Why the server did not start, with the exit code it maps to.
#[derive(Debug)]
pub struct StartError {
    pub code: i32,
    pub message: String,
}

fn start_error(code: i32, message: impl Into<String>) -> StartError {
    StartError {
        code,
        message: message.into(),
    }
}

/// The nearest ancestor of `start` (inclusive) that holds a `.git` entry.
fn git_root(start: &Path) -> Option<PathBuf> {
    let start = std::fs::canonicalize(start).ok()?;
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

fn read_policy(path: &Path) -> Result<String, StartError> {
    let io =
        |e: &dyn std::fmt::Display| start_error(2, format!("cannot read {}: {e}", path.display()));
    let metadata = std::fs::metadata(path).map_err(|e| io(&e))?;
    if !metadata.is_file() {
        return Err(io(&"not a regular file"));
    }
    if metadata.len() > trustgraph_policy::MAX_BYTES as u64 {
        return Err(io(&"the file is larger than the policy size limit"));
    }
    std::fs::read_to_string(path).map_err(|e| io(&e))
}

fn default_state() -> Result<PathBuf, StartError> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|home| PathBuf::from(home).join(".trustgraph").join("serve"))
        .ok_or_else(|| start_error(2, "HOME is not set; pass --state DIR"))
}

/// Validate the options, open the journal and build the server. `port` is
/// the port the caller already bound.
pub fn start(options: Options, port: u16) -> Result<Arc<Server<FileJournal>>, StartError> {
    let text = read_policy(&options.policy)?;
    // Validated by the runtime: an invalid file is an error only where it
    // would be applied (an empty journal, or --activate-policy-file).
    let file_policy = parse(&text).policy;
    let state = match options.state {
        Some(state) => state,
        None => default_state()?,
    };
    tokens::private_dir(&state)
        .map_err(|e| start_error(2, format!("cannot create {}: {e}", state.display())))?;
    let state = std::fs::canonicalize(&state)
        .map_err(|e| start_error(2, format!("cannot resolve {}: {e}", state.display())))?;
    let policy_dir = options
        .policy
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut repositories: Vec<PathBuf> = Vec::new();
    let policy_repository = git_root(policy_dir);
    repositories.extend(policy_repository.clone());
    if let Ok(cwd) = std::env::current_dir() {
        repositories.extend(git_root(&cwd));
    }
    let repository_refs: Vec<&Path> = repositories.iter().map(PathBuf::as_path).collect();
    auth::state_outside_repositories(&state, &repository_refs, options.allow_state_in_repository)
        .map_err(|e| start_error(2, e.to_string()))?;
    let journal = FileJournal::open(&state)
        .map_err(|e| start_error(2, format!("cannot open the journal: {e}")))?;
    let mode = if options.activate_policy_file {
        PolicyFile::Activate
    } else {
        PolicyFile::Propose
    };
    let runtime = Runtime::open_with(journal, &text, unix_now(), mode).map_err(|e| match e {
        trustgraph_engine::OpenError::InvalidPolicy(diagnostics) => start_error(
            1,
            format!(
                "the policy file is invalid:\n{}",
                diagnostic_lines(&diagnostics).join("\n")
            ),
        ),
        other => start_error(2, format!("cannot open the journal: {other}")),
    })?;
    // The operator must be a person of the policy actually in force, which
    // need not be the file's.
    let active = runtime
        .engine()
        .policy()
        .ok_or_else(|| start_error(2, "the journal has no active policy"))?;
    if !active
        .groups
        .values()
        .any(|members| members.contains(&options.operator))
    {
        return Err(start_error(
            2,
            format!(
                "--operator `{}` is not a person in any group of the active policy (revision {}); name a person listed under `groups`",
                options.operator,
                runtime.engine().policy_revision()
            ),
        ));
    }
    let mut report = file_report(runtime.file_outcome(), active, file_policy.as_ref());
    if let Some(repository) = policy_repository {
        report.push(format!(
            "warning: the policy file is inside the Git repository {}; agents working there can edit it. Changes to it never apply without the operator.",
            repository.display()
        ));
    }
    Server::build(
        runtime,
        Settings {
            operator: options.operator,
            port,
            state_dir: state,
            limits: options.limits,
        },
        Arc::new(SystemClock::default()),
        report,
    )
    .map_err(|e| start_error(2, format!("cannot write agent tokens: {e}")))
}

/// `WIDENS subject: summary` lines, as `trustgraph policy diff` prints them.
fn diff_lines(old: &Policy, new: &Policy) -> Vec<String> {
    let changes = trustgraph_policy::diff(old, new).changes;
    if changes.is_empty() {
        return vec!["  No changes.".into()];
    }
    changes
        .iter()
        .map(|c| {
            let tag = match c.kind {
                ChangeKind::Widens => "WIDENS ",
                ChangeKind::Narrows => "narrows",
                ChangeKind::Neutral => "neutral",
            };
            format!("  {tag} {}: {}", c.subject, c.summary)
        })
        .collect()
}

fn diagnostic_lines(diagnostics: &[trustgraph_policy::Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|d| format!("{} {}: {}", d.code, d.path, d.message))
        .collect()
}

/// What to tell the person at the terminal about the policy file. `file`
/// is the parsed file, `None` when it does not parse.
fn file_report(outcome: &FileOutcome, active: &Policy, file: Option<&Policy>) -> Vec<String> {
    let how = "To apply it, activate it on the operator page, or restart once with --activate-policy-file.";
    let changes = |from: &Policy| file.map(|file| diff_lines(from, file)).unwrap_or_default();
    let mut lines = Vec::new();
    match outcome {
        FileOutcome::Genesis | FileOutcome::Unchanged => {}
        FileOutcome::Proposed { .. } => {
            lines.push("The policy file differs from the active policy. It was NOT applied; it is stored as the proposal from the file (replacing any older one). Changes it would make:".into());
            lines.extend(changes(active));
            lines.push(how.into());
        }
        FileOutcome::AlreadyRecorded { stored, .. } => {
            let what = match stored {
                Some(ProposalOrigin::File) => {
                    "It was NOT applied; it is still stored as the proposal from the file, from an earlier start.".to_string()
                }
                Some(ProposalOrigin::Operator) => {
                    "It was NOT applied; the same policy is stored as a proposal by the operator.".to_string()
                }
                Some(ProposalOrigin::Agent(agent)) => format!(
                    "It was NOT applied; the same policy is stored as a proposal by agent `{agent}`."
                ),
                None => "It was NOT applied. It was applied or proposed at an earlier start, is no longer stored as a proposal, and is not proposed again.".to_string(),
            };
            lines.push(format!(
                "The policy file differs from the active policy. {what} Changes it would make:"
            ));
            lines.extend(changes(active));
            lines.push(if stored.is_some() {
                how.into()
            } else {
                "To apply it, restart once with --activate-policy-file.".into()
            });
        }
        FileOutcome::Activated { previous } => {
            lines.push("Applied the policy file (--activate-policy-file). Changes from the previous policy:".into());
            lines.extend(changes(previous));
        }
        FileOutcome::Invalid { diagnostics } => {
            lines.push("warning: the policy file is invalid, so it was ignored: the server runs on the active policy and nothing was proposed. Fix the file and restart, or propose a change on the operator page. Diagnostics:".into());
            lines.extend(
                diagnostic_lines(diagnostics)
                    .into_iter()
                    .map(|line| format!("  {line}")),
            );
        }
    }
    lines
}

/// `trustgraph serve`: returns the process exit code.
pub async fn main(options: Options, port: u16) -> i32 {
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: cannot listen on 127.0.0.1:{port}: {e}");
            return 2;
        }
    };
    let port = listener.local_addr().map_or(port, |a| a.port());
    let server = match start(options, port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {}", e.message);
            return e.code;
        }
    };
    if let Err(code) = banner(&server, port, "TrustGraph serve", "/mcp") {
        return code;
    }
    run(listener, router(server)).await
}

/// Write the launch file and print what the person at the terminal needs.
fn banner(server: &Server<FileJournal>, port: u16, title: &str, mcp: &str) -> Result<(), i32> {
    let launch = match server.write_launch_file() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("error: cannot write the operator launch file: {e}");
            return Err(2);
        }
    };
    let (revision, digest, source) = match server.policy_summary() {
        Ok(summary) => summary,
        Err(e) => {
            eprintln!("error: {}", e.message);
            return Err(2);
        }
    };
    let short = digest.get(..12).unwrap_or(&digest);
    println!("{title}: http://127.0.0.1:{port}");
    println!("MCP endpoint: http://127.0.0.1:{port}{mcp}");
    println!("Operator sign-in (single use): {}", launch.display());
    println!("Agent tokens: {}", server.token_dir().display());
    println!(
        "Policy in force: revision {revision} ({}), digest {short}",
        if source != "file" {
            "activated in service"
        } else if server.file_in_force() {
            "from the policy file"
        } else {
            "from an earlier version of the policy file"
        }
    );
    println!("Operator: {}", server.settings().operator);
    for line in server.startup_report() {
        println!("{line}");
    }
    Ok(())
}

/// Serve `app` until Ctrl-C; the exit code.
async fn run(listener: tokio::net::TcpListener, app: Router) -> i32 {
    match axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The operator page is served under a strict CSP, so it must not
    /// depend on inline script or style.
    #[test]
    fn page_has_no_inline_script_or_style() {
        let html = PAGE_HTML.to_ascii_lowercase();
        assert!(!html.contains("<style"), "inline <style>");
        assert!(!html.contains(" style="), "inline style attribute");
        assert!(!html.contains("javascript:"));
        for attr in [
            " onclick",
            " onload",
            " onsubmit",
            " oninput",
            " onchange",
            " onerror",
        ] {
            assert!(!html.contains(attr), "inline handler {attr}");
        }
        let mut rest = html.as_str();
        while let Some(i) = rest.find("<script") {
            let tag_end = rest[i..].find('>').unwrap() + i;
            let tag = &rest[i..tag_end];
            assert!(tag.contains("src="), "inline <script>: {tag}");
            assert!(
                rest[tag_end + 1..].trim_start().starts_with("</script>"),
                "script body must be empty"
            );
            rest = &rest[tag_end..];
        }
        assert!(html.contains("src=\"/serve.js\""));
        assert!(html.contains("href=\"/serve.css\""));
        // The script builds the page without inline styles or HTML strings.
        assert!(!PAGE_JS.contains("innerHTML"));
        assert!(!PAGE_JS.contains(".style."));
        assert!(!PAGE_JS.contains("setAttribute('style'"));
        assert!(!PAGE_JS.contains("eval("));
        assert!(!PAGE_JS.contains("window.confirm"));
    }

    #[test]
    fn a_token_held_by_two_agents_authenticates_nobody() {
        let state = tempfile::tempdir().unwrap();
        let runtime = Runtime::open(MemoryJournal::new(), trustgraph_policy::STARTER, 0).unwrap();
        let server = Server::new(
            runtime,
            Settings {
                operator: "leo".into(),
                port: 1,
                state_dir: state.path().to_path_buf(),
                limits: Limits::default(),
            },
        )
        .unwrap();
        let shared = "ab".repeat(32);
        *server.tokens.write().unwrap() = vec![
            ("mailer".into(), shared.clone()),
            ("planner".into(), shared.clone()),
            ("other".into(), "cd".repeat(32)),
        ];
        let mut headers = HeaderMap::new();
        let bearer = |t: &str| HeaderValue::from_str(&format!("Bearer {t}")).unwrap();
        headers.insert(header::AUTHORIZATION, bearer(&shared));
        let error = server.authenticate_agent(&headers).unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        headers.insert(header::AUTHORIZATION, bearer(&"cd".repeat(32)));
        assert_eq!(server.authenticate_agent(&headers).unwrap(), "other");
    }

    #[test]
    fn readers_intersect_and_missing_sources_read_nothing() {
        let policy = parse(trustgraph_policy::STARTER).policy.unwrap();
        let set = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<BTreeSet<_>>();
        assert_eq!(
            readers(&policy, &set(&["salaries", "roadmap"])),
            set(&["nina"])
        );
        assert!(readers(&policy, &set(&["gone"])).is_empty());
    }
}
