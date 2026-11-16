//! `trustgraph serve`: the policy runtime behind a loopback HTTP API, an MCP
//! endpoint and a plain operator page.
//!
//! Identity is the whole point. An agent is known ONLY by its bearer token
//! (one per agent, in `<state>/tokens/`); no request body names an agent.
//! The operator is known ONLY by a session cookie obtained once through the
//! private launch file; the reviewer sent to the engine is always the
//! configured `--operator`. Agent tokens never reach an operator route, and
//! no agent route or MCP tool can approve, revoke or activate.
//!
//! Every request must name the loopback `Host` with this server's port, and
//! a browser `Origin`, when present, must be the same origin. A journal
//! failure makes the outcome unknown: the server then answers every request
//! with 503 `restart_required` until the process is restarted.

pub mod mcp;
mod tokens;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

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
use trustgraph_engine::{Event, FileJournal, Journal, MemoryJournal, Reply, Runtime};
use trustgraph_live::auth;
use trustgraph_policy::{Policy, Severity, lint, parse, run_tests};

use crate::policy_cmd::text as policy_text;

pub use tokens::random_token;

/// Largest accepted request body, in bytes.
pub const BODY_LIMIT: usize = 512 * 1024;
/// How much of an oversize body is read and discarded before answering 413.
const DRAIN_LIMIT: usize = 8 * 1024 * 1024;
pub const DEFAULT_MAX_ARTIFACTS: usize = 100_000;
pub const DEFAULT_MAX_JOURNAL_BYTES: u64 = 1 << 30;
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
    /// Sustained requests per second per agent.
    pub rate_per_second: f64,
    /// Requests an agent may make at once before the rate applies.
    pub burst: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_artifacts: DEFAULT_MAX_ARTIFACTS,
            max_journal_bytes: DEFAULT_MAX_JOURNAL_BYTES,
            rate_per_second: 50.0,
            burst: 200.0,
        }
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

/// The server: one runtime behind one mutex, plus the identity tables.
pub struct Server<J: ServeJournal> {
    settings: Settings,
    runtime: Mutex<Runtime<J>>,
    /// `(agent, token)` for every agent of the active policy with a token.
    tokens: RwLock<Vec<(String, String)>>,
    sessions: auth::OwnerSessions,
    buckets: Mutex<HashMap<String, Bucket>>,
    /// Why the server stopped serving; set once, never cleared.
    broken: Mutex<Option<String>>,
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

impl<J: ServeJournal> Server<J> {
    /// A server over an opened runtime. Writes the token files of every agent
    /// in the active policy; any token problem is an error.
    pub fn new(runtime: Runtime<J>, settings: Settings) -> std::io::Result<Arc<Self>> {
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
        let sessions = auth::OwnerSessions::new(&settings.port.to_string(), auth::SESSION_LIFETIME);
        Ok(Arc::new(Server {
            settings,
            runtime: Mutex::new(runtime),
            tokens: RwLock::new(tokens),
            sessions,
            buckets: Mutex::new(HashMap::new()),
            broken: Mutex::new(None),
        }))
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
        let secret = self
            .sessions
            .launch_secret()
            .ok_or_else(|| std::io::Error::other("the launch secret was already used"))?;
        self.sessions.write_launch_file(
            &path,
            &format!(
                "<!doctype html><meta name=referrer content=no-referrer><meta http-equiv=refresh content=\"0;url=http://127.0.0.1:{}/#/connect/{secret}\">",
                self.settings.port
            ),
        )?;
        Ok(path)
    }

    /// The unused launch secret, for tests and the launch file.
    pub fn launch_secret(&self) -> Option<String> {
        self.sessions.launch_secret()
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

    /// Seconds since the epoch, never before the last journaled record.
    fn now(rt: &Runtime<J>) -> u64 {
        unix_now().max(rt.last_at())
    }

    fn handle(&self, rt: &mut Runtime<J>, event: Event) -> Result<Reply, ApiError> {
        let at = Self::now(rt);
        rt.handle(at, event).map_err(|e| {
            self.mark_broken(format!("journal failure: {e}"));
            ApiError::restart_required()
        })
    }

    /// Quotas checked before an agent write is journaled.
    fn agent_quota(&self, rt: &Runtime<J>, new_artifact: bool) -> Result<(), ApiError> {
        let limits = &self.settings.limits;
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
        let [agent] = found[..] else {
            return Err(missing());
        };
        let agent = agent.to_string();
        drop(tokens);
        self.charge(&agent)?;
        Ok(agent)
    }

    fn charge(&self, agent: &str) -> Result<(), ApiError> {
        let limits = &self.settings.limits;
        let mut buckets = self.buckets.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        let bucket = buckets.entry(agent.to_string()).or_insert(Bucket {
            tokens: limits.burst,
            at: now,
        });
        let refill = now.duration_since(bucket.at).as_secs_f64() * limits.rate_per_second;
        bucket.tokens = (bucket.tokens + refill).min(limits.burst);
        bucket.at = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Ok(());
        }
        let wait = ((1.0 - bucket.tokens) / limits.rate_per_second)
            .ceil()
            .max(1.0);
        let mut error = ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many requests; retry later",
        );
        error.retry_after = Some(wait as u64);
        Err(error)
    }

    fn require_operator(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        if self.sessions.valid(headers) {
            Ok(())
        } else {
            Err(ApiError::unauthorized(
                "operator_session_required",
                "open the operator launch file to sign in",
            ))
        }
    }

    // ----- agent operations (HTTP and MCP share these) -----

    pub fn register_source(&self, agent: &str, request: SourceRequest) -> ApiResult {
        let sha = digest_of(request.sha256, request.content)?;
        let mut rt = self.lock()?;
        let policy = rt
            .engine()
            .policy()
            .ok_or_else(|| ApiError::internal("no active policy"))?;
        let Some(source) = policy.sources.get(&request.source) else {
            return Err(ApiError::rejected(
                "unknown_source".into(),
                "that is not a source in the active policy".into(),
            ));
        };
        if !source.agents.contains(agent) {
            return Err(ApiError::rejected(
                "agent_cannot_use_source".into(),
                format!("agent `{agent}` may not use this source"),
            ));
        }
        self.agent_quota(&rt, true)?;
        let reply = self.handle(
            &mut rt,
            Event::RegisterSource {
                source: request.source,
                sha256: sha,
            },
        )?;
        reply_value(reply)
    }

    pub fn record_output(&self, agent: &str, request: OutputRequest) -> ApiResult {
        let sha = digest_of(request.sha256, request.content)?;
        let mut rt = self.lock()?;
        self.agent_quota(&rt, true)?;
        let reply = self.handle(
            &mut rt,
            Event::RecordOutput {
                agent: agent.into(),
                step: request.step,
                inputs: request.inputs,
                sha256: sha,
            },
        )?;
        reply_value(reply)
    }

    pub fn check(&self, agent: &str, request: CheckRequest) -> ApiResult {
        let mut rt = self.lock()?;
        self.agent_quota(&rt, false)?;
        let reply = self.handle(
            &mut rt,
            Event::Check {
                agent: agent.into(),
                artifact: request.artifact,
                action: request.action,
                recipients: request.recipients.map(|r| r.into_iter().collect()),
            },
        )?;
        reply_value(reply)
    }

    pub fn explain_artifact(&self, agent: &str, artifact: &str) -> ApiResult {
        let rt = self.lock()?;
        let engine = rt.engine();
        let Some(sources) = engine.lineage(artifact) else {
            return Err(ApiError::rejected(
                "unknown_artifact".into(),
                "no such artifact".into(),
            ));
        };
        let policy = engine
            .policy()
            .ok_or_else(|| ApiError::internal("no active policy"))?;
        let allowed = sources.iter().all(|name| {
            policy
                .sources
                .get(name)
                .is_some_and(|s| s.agents.contains(agent))
        });
        if !allowed {
            return Err(ApiError::rejected(
                "agent_cannot_use_source".into(),
                format!("agent `{agent}` may not use every source of this artifact"),
            ));
        }
        Ok(json!({
            "artifact": artifact,
            "readers": readers(policy, &sources),
            "sources": sources,
        }))
    }

    pub fn propose(&self, agent: Option<&str>, request: PolicyRequest) -> ApiResult {
        validate_policy(&request.policy)?;
        let mut rt = self.lock()?;
        if agent.is_some() {
            self.agent_quota(&rt, false)?;
        }
        let reply = self.handle(
            &mut rt,
            Event::ProposePolicy {
                agent: agent.map(str::to_string),
                policy: request.policy,
            },
        )?;
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
        let now = Self::now(&rt);
        let engine = rt.engine();
        let pending: Vec<Value> = engine
            .pending_decisions(now)
            .into_iter()
            .map(|p| {
                json!({
                    "decision": p.decision,
                    "agent": p.agent,
                    "action": p.action,
                    "recipients": p.recipients,
                    "sources": p.sources,
                    "reason": p.reason,
                    "requested_at": p.requested_at,
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
                json!({
                    "proposal": p.proposal,
                    "from": p.agent.as_deref().unwrap_or("operator"),
                    "widens": p.widens,
                    "changes": p.changes,
                    "explanation": p.explanation,
                })
            })
            .collect();
        Ok(json!({
            "operator": self.settings.operator,
            "now": now,
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
        let mut rt = self.lock()?;
        let reviewer = self.settings.operator.clone();
        let reply = self.handle(&mut rt, Event::Approve { decision, reviewer })?;
        reply_value(reply)
    }

    fn revoke(&self, decision: String) -> ApiResult {
        let mut rt = self.lock()?;
        let reviewer = self.settings.operator.clone();
        let reply = self.handle(&mut rt, Event::Revoke { decision, reviewer })?;
        reply_value(reply)
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
                    s.authenticate_agent(request.headers())?;
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
            s.authenticate_agent(request.headers())?;
            let body: DiffRequest = read_json(request).await?;
            blocking(&s, move |s| s.policy_diff(body)).await
        }
        .await,
    )
}

async fn session<J: ServeJournal>(State(s): Shared<J>, headers: HeaderMap) -> Response {
    if s.sessions.valid(&headers) {
        return axum::Json(json!({"authenticated": true})).into_response();
    }
    let Some(cookie) = auth::bearer(&headers).and_then(|secret| s.sessions.exchange(secret)) else {
        return ApiError::unauthorized(
            "invalid_launch_secret",
            "the launch link is invalid or already used; restart trustgraph serve for a new one",
        )
        .into_response();
    };
    let mut response = axum::Json(json!({"authenticated": true})).into_response();
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        response.headers_mut().insert(header::SET_COOKIE, v);
    }
    response
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
        .merge(mcp::router(server.clone()))
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
    let parsed = parse(&text);
    let Some(policy) = parsed.policy else {
        let lines: Vec<String> = parsed
            .diagnostics
            .iter()
            .map(|d| format!("{} {}: {}", d.code, d.path, d.message))
            .collect();
        return Err(start_error(
            1,
            format!("the policy file is invalid:\n{}", lines.join("\n")),
        ));
    };
    if !policy
        .groups
        .values()
        .any(|members| members.contains(&options.operator))
    {
        return Err(start_error(
            2,
            format!(
                "--operator `{}` is not a person in any group of {}; name a person listed under `groups`",
                options.operator,
                options.policy.display()
            ),
        ));
    }
    let state = match options.state {
        Some(state) => state,
        None => default_state()?,
    };
    tokens::private_dir(&state)
        .map_err(|e| start_error(2, format!("cannot create {}: {e}", state.display())))?;
    let policy_dir = options
        .policy
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut repositories: Vec<PathBuf> = Vec::new();
    repositories.extend(git_root(policy_dir));
    if let Ok(cwd) = std::env::current_dir() {
        repositories.extend(git_root(&cwd));
    }
    let repository_refs: Vec<&Path> = repositories.iter().map(PathBuf::as_path).collect();
    auth::state_outside_repositories(&state, &repository_refs, options.allow_state_in_repository)
        .map_err(|e| start_error(2, e.to_string()))?;
    let journal = FileJournal::open(&state)
        .map_err(|e| start_error(2, format!("cannot open the journal: {e}")))?;
    let runtime = Runtime::open(journal, &text, unix_now()).map_err(|e| match e {
        trustgraph_engine::OpenError::InvalidPolicy(diagnostics) => {
            let lines: Vec<String> = diagnostics
                .iter()
                .map(|d| format!("{} {}: {}", d.code, d.path, d.message))
                .collect();
            start_error(
                1,
                format!("the policy file is invalid:\n{}", lines.join("\n")),
            )
        }
        other => start_error(2, format!("cannot open the journal: {other}")),
    })?;
    Server::new(
        runtime,
        Settings {
            operator: options.operator,
            port,
            state_dir: state,
            limits: options.limits,
        },
    )
    .map_err(|e| start_error(2, format!("cannot write agent tokens: {e}")))
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
    let launch = match server.write_launch_file() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("error: cannot write the operator launch file: {e}");
            return 2;
        }
    };
    let (revision, digest, source) = match server.policy_summary() {
        Ok(summary) => summary,
        Err(e) => {
            eprintln!("error: {}", e.message);
            return 2;
        }
    };
    let short = digest.get(..12).unwrap_or(&digest);
    println!("TrustGraph serve: http://127.0.0.1:{port}");
    println!("MCP endpoint: http://127.0.0.1:{port}/mcp");
    println!("Operator sign-in (single use): {}", launch.display());
    println!("Agent tokens: {}", server.token_dir().display());
    println!(
        "Policy in force: revision {revision} ({}), digest {short}",
        if source == "file" {
            "from the policy file"
        } else {
            "activated in service"
        }
    );
    println!("Operator: {}", server.settings().operator);
    let app = router(server);
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
