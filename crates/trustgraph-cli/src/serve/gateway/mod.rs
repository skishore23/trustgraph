//! `trustgraph gateway`: `serve` plus an MCP endpoint, `/gateway/mcp`, that
//! sits between agents and the MCP servers they use.
//!
//! The gateway holds the upstreams' credentials, exposes only the tools
//! `gateway.yaml` lists, records what each agent's session reads, and checks
//! every action before forwarding it. It shares the server's runtime,
//! journal, agent tokens, limits and operator page; nothing here decides
//! anything the engine does not.
//!
//! The rules that make it fail closed:
//! - What is checked is exactly what is forwarded. A call may carry only the
//!   arguments its tool lists (`arguments`); anything else is refused. The
//!   output an action is checked as is the canonical JSON of every argument
//!   it forwards.
//! - A read result is returned only after it is registered as its source and
//!   recorded in the session (`SessionCarry`); otherwise it is withheld.
//! - An action is forwarded only on an `allow` from a completed check of an
//!   output derived from everything the session carries. An allow that an
//!   approval gave uses the approval up: one approval releases one call.
//! - A session that has read nothing is refused (`nothing_read`) unless the
//!   tool says `allow_without_reads: true` (agent mode only); then it is
//!   forwarded unchecked and logged.
//! - A tool that is not listed, or whose source the agent may not use, gets
//!   the same "not available" error and the upstream is not called.
//! - Upstream failures of any kind are short tool errors with no detail.

pub mod bridge;
pub mod config;
mod handler;
pub mod handoff;
mod upstream;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::Router;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use serde_json::{Value, json};
use trustgraph_engine::{Event, FileJournal, Reply};
use trustgraph_policy::Policy;

use self::config::{Config, Kind, SessionMode, Tool, UNCLASSIFIED};
use self::handoff::{LIFETIME_SECONDS, Payload, Signer};
use self::upstream::{CallError, Upstreams};
use super::{
    ApiError, CheckRequest, ConnectionView, GatewayView, OutputRequest, ServeJournal, Server,
    SourceRequest, sha256_hex,
};

/// The gateway's own tools, exposed to every agent.
pub const HANDOFF_TOOL: &str = "trustgraph__handoff";
pub const RECEIVE_TOOL: &str = "trustgraph__receive";
/// Live MCP connections per agent, in both modes. At the cap a new
/// connection is refused; none is ever evicted or idled out.
pub const MAX_CONNECTIONS_PER_AGENT: usize = 64;
/// Calls held for approval whose arguments are kept, in memory only.
pub const MAX_HELD_CALLS: usize = 256;
/// Characters of each argument value shown to the operator.
pub const SHOWN_CHARS: usize = 2000;
/// Largest upstream input schema, serialized; a tool with a larger one is closed.
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024;
/// Sessions that may not act, kept across restarts (agent mode).
const BLOCKED_FILE: &str = "gateway.blocked";

/// A tool error as the agent sees it.
#[derive(Debug, Clone)]
pub struct ToolFail {
    pub code: String,
    pub message: String,
    pub extra: Option<Value>,
}

impl ToolFail {
    fn new(code: &str, message: impl Into<String>) -> Self {
        ToolFail {
            code: code.into(),
            message: message.into(),
            extra: None,
        }
    }

    /// The one answer for a tool that does not exist and for one this agent
    /// may not call, so tools cannot be probed.
    fn not_available() -> Self {
        Self::new(
            "tool_not_available",
            "no tool with that name is available to this agent",
        )
    }

    fn misconfigured(tool: &Tool) -> Self {
        Self::new(
            "tool_misconfigured",
            format!(
                "`{}` does not match the active policy or what its upstream publishes, so it is closed; the operator must fix gateway.yaml, the policy or the upstream",
                tool.exposed
            ),
        )
    }

    fn upstream(error: CallError, upstream: &str) -> Self {
        Self::new(error.code(), error.message(upstream))
    }

    pub fn result(&self) -> CallToolResult {
        let mut body = json!({"error": {"code": self.code, "message": self.message}});
        if let Some(extra) = &self.extra
            && let Some(map) = extra.as_object()
        {
            for (k, v) in map {
                body["error"][k] = v.clone();
            }
        }
        let mut result = CallToolResult::error(vec![ContentBlock::text(format!(
            "TrustGraph gateway: {} ({})",
            self.message, self.code
        ))]);
        result.structured_content = Some(body);
        result
    }
}

impl From<ApiError> for ToolFail {
    fn from(e: ApiError) -> Self {
        ToolFail::new(&e.code, e.message)
    }
}

/// One live MCP connection.
#[derive(Debug, Clone)]
struct Conn {
    /// The operator page's handle; not the MCP session id.
    id: String,
    agent: String,
    /// The gateway session it serves: `conn:…` or `agent:<name>`.
    session: String,
    started_at: u64,
    last_active: u64,
}

#[derive(Default)]
struct Connections {
    /// MCP session id -> connection.
    by_mcp: HashMap<String, Conn>,
}

impl Connections {
    fn live_session(&self, session: &str) -> bool {
        self.by_mcp.values().any(|c| c.session == session)
    }
}

/// A call held for approval: what it would forward. Memory only.
#[derive(Debug, Clone)]
struct HeldCall {
    tool: String,
    action: String,
    content: Option<String>,
    arguments: JsonObject,
    sha256: String,
}

#[derive(Default)]
struct Held {
    calls: BTreeMap<String, HeldCall>,
    /// Decision ids, oldest first.
    order: VecDeque<String>,
}

/// What the server's operator routes see of the gateway.
pub(crate) struct Shared {
    config: Config,
    conns: Mutex<Connections>,
    manager: Arc<LocalSessionManager>,
    held: Mutex<Held>,
    /// Sessions that refused a hand-off and may not act until ended.
    blocked: Mutex<BTreeSet<String>>,
    blocked_path: PathBuf,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Shared {
    fn conns(&self) -> std::sync::MutexGuard<'_, Connections> {
        lock(&self.conns)
    }

    fn is_blocked(&self, session: &str) -> bool {
        lock(&self.blocked).contains(session)
    }

    /// Mark `session` as unable to act, kept across restarts.
    fn block(&self, session: &str) {
        let mut blocked = lock(&self.blocked);
        if blocked.insert(session.to_string()) {
            save_blocked_or_warn(&self.blocked_path, &blocked);
        }
    }

    fn unblock(&self, session: &str) -> bool {
        let mut blocked = lock(&self.blocked);
        let was = blocked.remove(session);
        if was {
            save_blocked_or_warn(&self.blocked_path, &blocked);
        }
        was
    }

    fn hold(&self, decision: &str, call: HeldCall) {
        let mut held = lock(&self.held);
        if held.calls.insert(decision.to_string(), call).is_none() {
            held.order.push_back(decision.to_string());
        }
        while held.order.len() > MAX_HELD_CALLS {
            if let Some(old) = held.order.pop_front() {
                held.calls.remove(&old);
            }
        }
    }

    fn release(&self, decision: &str) {
        let mut held = lock(&self.held);
        if held.calls.remove(decision).is_some() {
            held.order.retain(|d| d != decision);
        }
    }

    /// Close an MCP session in the background.
    fn close_mcp(&self, mcp: &str) {
        let manager = self.manager.clone();
        let mcp = mcp.to_string();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                use rmcp::transport::streamable_http_server::session::SessionManager;
                let _ = manager.close_session(&mcp.into()).await;
            });
        }
    }
}

/// Largest `gateway.blocked` read.
const MAX_BLOCKED_BYTES: u64 = 4 << 20;

/// The blocked set kept across restarts: `agent:` entries (an agent-mode
/// session, or, in connection mode, an agent blocked as a whole).
/// Connection sessions end with the process. A missing file is an empty
/// set; any other problem (unreadable, a link, not a regular file, not a
/// JSON list of strings) is an error: starting anyway would unblock.
pub fn load_blocked(path: &Path) -> Result<BTreeSet<String>, String> {
    use std::io::Read;
    let fail = |why: &dyn std::fmt::Display| {
        format!(
            "cannot read {}: {why}; the gateway does not start, because ignoring it would unblock every session it blocks. Fix the file, or remove it to unblock them all",
            path.display()
        )
    };
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(fail(&e)),
    };
    let metadata = file.metadata().map_err(|e| fail(&e))?;
    if !metadata.is_file() || metadata.len() > MAX_BLOCKED_BYTES {
        return Err(fail(&"not a regular file under 4 MiB"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_BLOCKED_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| fail(&e))?;
    let set: BTreeSet<String> = serde_json::from_slice(&bytes)
        .map_err(|e| fail(&format!("not a JSON list of session ids ({e})")))?;
    Ok(set
        .into_iter()
        .filter(|s| s.starts_with("agent:"))
        .collect())
}

/// Write the blocked set atomically: a new 0600 temp file (never through a
/// link), synced, then renamed over the old one.
pub fn save_blocked(path: &Path, blocked: &BTreeSet<String>) -> std::io::Result<()> {
    use std::io::Write;
    let text = serde_json::to_vec(blocked).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("blocked.tmp");
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut file = crate::serve::auth::private_create(&tmp)?;
    file.write_all(&text)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

fn save_blocked_or_warn(path: &Path, blocked: &BTreeSet<String>) {
    if let Err(e) = save_blocked(path, blocked) {
        eprintln!(
            "trustgraph gateway: cannot write {}: {e}; a blocked session is still blocked until this process ends",
            path.display()
        );
    }
}

/// The blocked-set key of an agent as a whole.
fn agent_key(agent: &str) -> String {
    format!("agent:{agent}")
}

/// An argument value as the operator sees it: its text when it is a
/// string, else its canonical JSON; at most `SHOWN_CHARS` characters.
fn shown_argument(name: &str, value: &Value) -> Value {
    let text = match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(&canonical(other)).unwrap_or_default(),
    };
    let total = text.chars().count();
    let shown: String = text.chars().take(SHOWN_CHARS).collect();
    json!({"name": name, "value": shown, "more": total.saturating_sub(SHOWN_CHARS)})
}

impl GatewayView for Shared {
    fn mode(&self) -> &'static str {
        self.config.session.as_str()
    }

    fn problems(&self, policy: &Policy) -> Vec<String> {
        config::check_against(&self.config, policy)
            .iter()
            .map(|p| format!("{p} — that tool is closed until it is fixed"))
            .collect()
    }

    fn connections(&self) -> Vec<ConnectionView> {
        let mut out: Vec<ConnectionView> = self
            .conns()
            .by_mcp
            .values()
            .map(|c| ConnectionView {
                connection: c.id.clone(),
                agent: c.agent.clone(),
                session: c.session.clone(),
                started_at: c.started_at,
                last_active: c.last_active,
            })
            .collect();
        out.sort_by(|a, b| {
            (&a.agent, a.started_at, &a.connection).cmp(&(&b.agent, b.started_at, &b.connection))
        });
        out
    }

    fn at_cap(&self) -> Vec<String> {
        let mut count: BTreeMap<&str, usize> = BTreeMap::new();
        let conns = self.conns();
        for c in conns.by_mcp.values() {
            *count.entry(c.agent.as_str()).or_default() += 1;
        }
        count
            .into_iter()
            .filter(|(_, n)| *n >= MAX_CONNECTIONS_PER_AGENT)
            .map(|(agent, _)| agent.to_string())
            .collect()
    }

    fn close(&self, session: &str) -> bool {
        let closed: Vec<String> = {
            let mut conns = self.conns();
            let mcps: Vec<String> = conns
                .by_mcp
                .iter()
                .filter(|(_, c)| c.session == session)
                .map(|(m, _)| m.clone())
                .collect();
            for mcp in &mcps {
                conns.by_mcp.remove(mcp);
            }
            mcps
        };
        for mcp in &closed {
            self.close_mcp(mcp);
        }
        let unblocked = self.unblock(session);
        !closed.is_empty() || unblocked
    }

    fn close_connection(&self, connection: &str) -> bool {
        let mcp = {
            let mut conns = self.conns();
            let mcp = conns
                .by_mcp
                .iter()
                .find(|(_, c)| c.id == connection)
                .map(|(m, _)| m.clone());
            if let Some(mcp) = &mcp {
                conns.by_mcp.remove(mcp);
            }
            mcp
        };
        match mcp {
            Some(mcp) => {
                self.close_mcp(&mcp);
                true
            }
            None => false,
        }
    }

    fn blocked(&self) -> Vec<String> {
        lock(&self.blocked).iter().cloned().collect()
    }

    fn held_call(&self, decision: &str) -> Option<Value> {
        let held = lock(&self.held);
        let call = held.calls.get(decision)?;
        let arguments: Vec<Value> = canonical_map(&call.arguments)
            .iter()
            .map(|(name, value)| shown_argument(name, value))
            .collect();
        Some(json!({
            "tool": call.tool,
            "action": call.action,
            "content": call.content,
            "held": true,
            "arguments": arguments,
            "sha256": call.sha256,
        }))
    }
}

pub struct Gateway<J: ServeJournal> {
    server: Arc<Server<J>>,
    shared: Arc<Shared>,
    upstreams: Upstreams,
    signer: Signer,
}

/// Canonical JSON: object keys sorted at every level, so the same arguments
/// always hash the same.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            canonical_map(map)
                .into_iter()
                .map(|(k, v)| (k.clone(), v))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

fn canonical_map(map: &JsonObject) -> BTreeMap<&String, Value> {
    map.iter().map(|(k, v)| (k, canonical(v))).collect()
}

/// The output of an action call: the canonical JSON (sorted keys, no
/// whitespace) of every argument it forwards.
pub fn canonical_arguments(arguments: &JsonObject) -> Vec<u8> {
    // Written by hand so key order never depends on serde_json's features.
    fn write(value: &Value, out: &mut String) {
        match value {
            Value::Object(map) => {
                out.push('{');
                let sorted: BTreeMap<&String, &Value> = map.iter().collect();
                for (i, (k, v)) in sorted.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String(k.clone()).to_string());
                    out.push(':');
                    write(v, out);
                }
                out.push('}');
            }
            Value::Array(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write(v, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(&Value::Object(arguments.clone()), &mut out);
    out.into_bytes()
}

/// What a checked action call may do.
enum Forward {
    /// Allowed; `Some(decision)` when an approval allowed it and was used up.
    Checked(Option<String>),
    /// `allow_without_reads` and the session has read nothing.
    Unchecked,
}

/// Problems of one listed tool against the tool its upstream publishes,
/// and the upstream arguments it does not forward.
pub fn schema_report(
    tool: &Tool,
    published: Option<&rmcp::model::Tool>,
) -> (Vec<String>, Vec<String>) {
    let mut problems = Vec::new();
    let Some(published) = published else {
        problems.push(format!(
            "tools.{}: the upstream does not offer a tool named `{}`",
            tool.key, tool.name
        ));
        return (problems, Vec::new());
    };
    let size = serde_json::to_vec(&*published.input_schema).map_or(usize::MAX, |b| b.len());
    if size > MAX_SCHEMA_BYTES {
        problems.push(format!(
            "tools.{}: the upstream's input schema is {size} bytes, over the {MAX_SCHEMA_BYTES}-byte cap",
            tool.key
        ));
        return (problems, Vec::new());
    }
    let Some(properties) = published
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
    else {
        return (problems, Vec::new());
    };
    for name in &tool.arguments {
        if !properties.contains_key(name) {
            problems.push(format!(
                "tools.{}.arguments: `{name}` is not an argument the upstream tool takes (it takes: {})",
                tool.key,
                properties.keys().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    let unforwarded = properties
        .keys()
        .filter(|k| !tool.arguments.contains(*k))
        .cloned()
        .collect();
    (problems, unforwarded)
}

/// Build the gateway over a server: check the config against the active
/// policy, resolve secrets with `lookup`, open the hand-off key, end
/// sessions that do not survive a restart, and hook into the operator page.
pub fn attach<J: ServeJournal>(
    server: Arc<Server<J>>,
    config: Config,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Arc<Gateway<J>>, Vec<String>> {
    {
        let rt = server.lock().map_err(|e| vec![e.message])?;
        let policy = rt
            .engine()
            .policy()
            .ok_or_else(|| vec!["the journal has no active policy".to_string()])?;
        let problems = config::check_against(&config, policy);
        if !problems.is_empty() {
            return Err(problems.iter().map(ToString::to_string).collect());
        }
    }
    let upstreams = upstream::resolve(&config, lookup)?;
    let signer = Signer::open(&server.settings.state_dir)
        .map_err(|e| vec![format!("cannot open the hand-off key: {e}")])?;
    end_stale_sessions(&server, config.session).map_err(|e| vec![e.message])?;
    let mut manager = LocalSessionManager::default();
    if config.session == SessionMode::Connection {
        // A session must not end because it idled: a client that quietly
        // reconnected would shed what it read. Connections end when the
        // client closes them, the operator ends them, or on restart. (In
        // agent mode an idle connection may close: the session is the
        // agent's and is kept.)
        manager.session_config.keep_alive = None;
    }
    let blocked_path = server.settings.state_dir.join(BLOCKED_FILE);
    let blocked = load_blocked(&blocked_path).map_err(|e| vec![e])?;
    save_blocked(&blocked_path, &blocked).map_err(|e| {
        vec![format!(
            "cannot write {}: {e}; the gateway does not start without a place to keep blocked sessions",
            blocked_path.display()
        )]
    })?;
    let shared = Arc::new(Shared {
        config,
        conns: Mutex::new(Connections::default()),
        manager: Arc::new(manager),
        held: Mutex::new(Held::default()),
        blocked: Mutex::new(blocked),
        blocked_path,
    });
    if server
        .gateway
        .set(shared.clone() as Arc<dyn GatewayView>)
        .is_err()
    {
        return Err(vec!["a gateway is already attached to this server".into()]);
    }
    let gateway = Arc::new(Gateway {
        server,
        shared,
        upstreams,
        signer,
    });
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        for conn in gateway.upstreams.all() {
            let conn = conn.clone();
            handle.spawn(async move { conn.warm().await });
        }
        let weak = Arc::downgrade(&gateway);
        handle.spawn(async move { reconcile(weak).await });
    }
    Ok(gateway)
}

/// `connection` sessions end with the process; `agent` sessions survive it.
/// At start, journal the end of every session the mode does not keep.
fn end_stale_sessions<J: ServeJournal>(
    server: &Server<J>,
    mode: SessionMode,
) -> Result<(), ApiError> {
    let mut rt = server.lock()?;
    let stale: Vec<String> = rt
        .engine()
        .sessions()
        .into_iter()
        .map(|s| s.session)
        .filter(|id| mode == SessionMode::Connection || !id.starts_with("agent:"))
        .collect();
    for session in stale {
        server.handle(&mut rt, Event::EndSession { session })?;
    }
    Ok(())
}

/// Every two seconds: forget connections whose MCP session rmcp closed (the
/// client's DELETE, or a dropped connection), and end every `conn:` session
/// that no live connection serves, including one a call still in flight
/// would otherwise have left behind.
async fn reconcile<J: ServeJournal>(gateway: std::sync::Weak<Gateway<J>>) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let Some(gateway) = gateway.upgrade() else {
            return;
        };
        let live: BTreeSet<String> = gateway
            .shared
            .manager
            .sessions
            .read()
            .await
            .keys()
            .map(|k| k.to_string())
            .collect();
        gateway
            .shared
            .conns()
            .by_mcp
            .retain(|mcp, _| live.contains(mcp));
        let g = gateway.clone();
        let _ = tokio::task::spawn_blocking(move || g.reap()).await;
    }
}

impl<J: ServeJournal> Gateway<J> {
    pub fn server(&self) -> &Arc<Server<J>> {
        &self.server
    }

    pub fn config(&self) -> &Config {
        &self.shared.config
    }

    /// End every `conn:` engine session no live connection serves. Under
    /// the runtime lock, then the connection table, as `carry` takes them,
    /// so a session cannot gain reads after it is judged gone.
    fn reap(&self) {
        let Ok(mut rt) = self.server.lock() else {
            return;
        };
        let orphans: Vec<String> = {
            let conns = self.shared.conns();
            rt.engine()
                .sessions()
                .into_iter()
                .map(|s| s.session)
                .filter(|s| s.starts_with("conn:") && !conns.live_session(s))
                .collect()
        };
        for session in orphans {
            let _ = self.server.handle(&mut rt, Event::EndSession { session });
        }
    }

    /// Run `f` on a blocking thread: the runtime lock is never held across an await.
    async fn blocking<T: Send + 'static>(
        self: &Arc<Self>,
        f: impl FnOnce(&Gateway<J>) -> Result<T, ToolFail> + Send + 'static,
    ) -> Result<T, ToolFail> {
        let gateway = self.clone();
        tokio::task::spawn_blocking(move || f(&gateway))
            .await
            .unwrap_or_else(|_| Err(ToolFail::new("internal", "the request failed")))
    }

    // ----- connections -----

    /// Bind a new MCP session to the agent that opened it. `false` when the
    /// agent already has the most live connections allowed.
    fn bind(&self, mcp: &str, agent: &str) -> bool {
        let now = self.server.time.now();
        let mut conns = self.shared.conns();
        let open = conns.by_mcp.values().filter(|c| c.agent == agent).count();
        if open >= MAX_CONNECTIONS_PER_AGENT {
            return false;
        }
        let session = match self.shared.config.session {
            SessionMode::Connection => format!("conn:{}", &super::auth::random_secret()[..24]),
            SessionMode::Agent => format!("agent:{agent}"),
        };
        conns.by_mcp.insert(
            mcp.to_string(),
            Conn {
                id: format!("c-{}", &super::auth::random_secret()[..16]),
                agent: agent.to_string(),
                session,
                started_at: now,
                last_active: now,
            },
        );
        true
    }

    /// True when `mcp` is a live connection this agent opened; then its
    /// last activity is now.
    fn may_use(&self, mcp: &str, agent: &str) -> bool {
        let now = self.server.time.now();
        let mut conns = self.shared.conns();
        match conns.by_mcp.get_mut(mcp) {
            Some(c) if c.agent == agent => {
                c.last_active = now;
                true
            }
            _ => false,
        }
    }

    /// The gateway session of one call.
    fn session_for(&self, agent: &str, mcp: Option<&str>) -> Result<String, ToolFail> {
        match self.shared.config.session {
            SessionMode::Agent => Ok(format!("agent:{agent}")),
            SessionMode::Connection => {
                let missing = || {
                    ToolFail::new(
                        "no_session",
                        "this gateway tracks reads per MCP session: connect with a protocol version that has sessions (2025-11-25 or earlier) and keep the connection",
                    )
                };
                let mcp = mcp.ok_or_else(missing)?;
                let conns = self.shared.conns();
                match conns.by_mcp.get(mcp) {
                    Some(c) if c.agent == agent => Ok(c.session.clone()),
                    _ => Err(missing()),
                }
            }
        }
    }

    /// A `conn:` session is live while a connection serves it; agent
    /// sessions always are. Called under the runtime lock.
    fn session_live(&self, session: &str) -> Result<(), ToolFail> {
        if session.starts_with("conn:") && !self.shared.conns().live_session(session) {
            return Err(ToolFail::new(
                "session_ended",
                "this connection was closed while the call was in flight; nothing was recorded and the result is withheld",
            ));
        }
        Ok(())
    }

    // ----- one tool call -----

    pub async fn call(
        self: &Arc<Self>,
        agent: String,
        mcp: Option<String>,
        name: String,
        arguments: JsonObject,
    ) -> CallToolResult {
        let outcome = async {
            let session = self.session_for(&agent, mcp.as_deref())?;
            match name.as_str() {
                HANDOFF_TOOL => {
                    if !arguments.is_empty() {
                        return Err(ToolFail::new("invalid_arguments", "takes no arguments"));
                    }
                    let value = self.blocking(move |g| g.handoff(&agent, &session)).await?;
                    Ok(structured(value))
                }
                RECEIVE_TOOL => {
                    let token = match (arguments.len(), arguments.get("token")) {
                        (1, Some(Value::String(t))) => t.clone(),
                        _ => {
                            return Err(ToolFail::new(
                                "invalid_arguments",
                                "send exactly one argument, `token`: a hand-off token",
                            ));
                        }
                    };
                    let shared = self.shared.clone();
                    let receiver = session.clone();
                    let whole = agent_key(&agent);
                    let value = self
                        .blocking(move |g| g.receive(&agent, &session, &token))
                        .await
                        .inspect_err(|_| {
                            // Refusing a hand-off must never leave the
                            // receiver freer than accepting it. In
                            // connection mode a new connection is a new
                            // session, so the agent as a whole is blocked
                            // too, until the operator clears it.
                            shared.block(&receiver);
                            if receiver != whole {
                                shared.block(&whole);
                            }
                        })?;
                    Ok(structured(value))
                }
                _ => {
                    let tool = self
                        .shared
                        .config
                        .tools
                        .get(&name)
                        .cloned()
                        .ok_or_else(ToolFail::not_available)?;
                    config::check_arguments(&tool.arguments, &arguments).map_err(|why| {
                        ToolFail::new(
                            "invalid_arguments",
                            format!("{why}; the call was not forwarded"),
                        )
                    })?;
                    self.published_ok(&tool).await?;
                    self.call_tool(agent, session, tool, arguments).await
                }
            }
        }
        .await;
        outcome.unwrap_or_else(|fail| fail.result())
    }

    /// The upstream still publishes `tool`, with a schema under the cap that
    /// takes every argument the config forwards.
    async fn published_ok(&self, tool: &Tool) -> Result<(), ToolFail> {
        let upstream = self
            .upstreams
            .get(&tool.upstream)
            .ok_or_else(ToolFail::not_available)?;
        let published = upstream
            .tools()
            .await
            .map_err(|e| ToolFail::upstream(e, &tool.upstream))?;
        let found = published.iter().find(|t| t.name == tool.name);
        if found.is_none() {
            return Err(ToolFail::not_available());
        }
        let (problems, _) = schema_report(tool, found);
        if !problems.is_empty() {
            for p in &problems {
                eprintln!("trustgraph gateway: {p}; the tool is closed");
            }
            return Err(ToolFail::misconfigured(tool));
        }
        Ok(())
    }

    async fn call_tool(
        self: &Arc<Self>,
        agent: String,
        session: String,
        tool: Tool,
        arguments: JsonObject,
    ) -> Result<CallToolResult, ToolFail> {
        match &tool.kind {
            Kind::Reads { rules, otherwise } => {
                let source = rules
                    .iter()
                    .find(|r| config::when_matches(&r.when, &arguments))
                    .map_or(otherwise, |r| &r.source)
                    .clone();
                self.read(agent, session, tool, source, arguments).await
            }
            Kind::Passthrough { untracked: true } => {
                let tool2 = tool.clone();
                self.blocking(move |g| g.tool_ok(&tool2)).await?;
                Ok(self.forward(&tool, arguments).await?.0)
            }
            Kind::Passthrough { untracked: false } => {
                self.read(agent, session, tool, UNCLASSIFIED.to_string(), arguments)
                    .await
            }
            Kind::Acts {
                rules,
                allow_without_reads,
            } => {
                let Some(rule) = rules
                    .iter()
                    .find(|r| config::when_matches(&r.when, &arguments))
                    .cloned()
                else {
                    return Err(ToolFail::new(
                        "blocked",
                        "this call matches no rule of the gateway config, so it is blocked; it was not forwarded",
                    ));
                };
                if let Some(name) = &rule.content
                    && !arguments.contains_key(name)
                {
                    return Err(ToolFail::new(
                        "invalid_arguments",
                        format!(
                            "the call has no `{name}` argument, which is its content; it was not forwarded"
                        ),
                    ));
                }
                let held = HeldCall {
                    tool: tool.exposed.clone(),
                    action: rule.action.clone(),
                    content: rule.content.clone(),
                    sha256: sha256_hex(&canonical_arguments(&arguments)),
                    arguments: arguments.clone(),
                };
                let allow_without_reads = *allow_without_reads;
                let tool2 = tool.clone();
                let forward = self
                    .blocking(move |g| {
                        g.decide(&agent, &session, &tool2, held, allow_without_reads)
                    })
                    .await?;
                match forward {
                    Forward::Unchecked => eprintln!(
                        "trustgraph gateway: `{}` was called with nothing read and forwarded without a check (allow_without_reads)",
                        tool.exposed
                    ),
                    Forward::Checked(Some(decision)) => self.shared.release(&decision),
                    Forward::Checked(None) => {}
                }
                Ok(self.forward(&tool, arguments).await?.0)
            }
        }
    }

    async fn forward(
        &self,
        tool: &Tool,
        arguments: JsonObject,
    ) -> Result<(CallToolResult, Vec<u8>), ToolFail> {
        let upstream = self
            .upstreams
            .get(&tool.upstream)
            .ok_or_else(ToolFail::not_available)?;
        upstream
            .call(&tool.name, arguments)
            .await
            .map_err(|e| ToolFail::upstream(e, &tool.upstream))
    }

    async fn read(
        self: &Arc<Self>,
        agent: String,
        session: String,
        tool: Tool,
        source: String,
        arguments: JsonObject,
    ) -> Result<CallToolResult, ToolFail> {
        {
            let (agent, tool, source) = (agent.clone(), tool.clone(), source.clone());
            self.blocking(move |g| g.authorize_read(&agent, &tool, &source))
                .await?;
        }
        let (result, bytes) = self.forward(&tool, arguments).await?;
        let sha = sha256_hex(&bytes);
        let named = source.clone();
        self.blocking(move |g| g.record_read(&agent, &session, &named, sha))
            .await
            .map_err(|fail| {
                ToolFail::new(
                    "not_recorded",
                    format!(
                        "the result could not be recorded as `{source}`, so it is withheld: {} ({})",
                        fail.message, fail.code
                    ),
                )
            })?;
        Ok(result)
    }

    // ----- under the runtime lock -----

    /// The active policy still has every name `tool` uses.
    fn tool_ok(&self, tool: &Tool) -> Result<(), ToolFail> {
        let rt = self.server.lock()?;
        let policy = rt
            .engine()
            .policy()
            .ok_or_else(|| ToolFail::new("internal", "no active policy"))?;
        if config::tool_problems(tool, policy).is_empty() {
            Ok(())
        } else {
            Err(ToolFail::misconfigured(tool))
        }
    }

    /// Before calling the upstream: the tool matches the policy, the agent
    /// may use `source`, and the agent has budget to record the result.
    fn authorize_read(&self, agent: &str, tool: &Tool, source: &str) -> Result<(), ToolFail> {
        let rt = self.server.lock()?;
        let policy = rt
            .engine()
            .policy()
            .ok_or_else(|| ToolFail::new("internal", "no active policy"))?;
        if !config::tool_problems(tool, policy).is_empty() {
            return Err(ToolFail::misconfigured(tool));
        }
        let cleared = policy
            .sources
            .get(source)
            .is_some_and(|s| s.agents.contains(agent));
        if !cleared {
            return Err(ToolFail::not_available());
        }
        self.server.agent_quota(&rt, agent, true)?;
        Ok(())
    }

    /// Register the result as `source` and add it to the session.
    fn record_read(
        &self,
        agent: &str,
        session: &str,
        source: &str,
        sha256: String,
    ) -> Result<(), ToolFail> {
        let artifact = self.server.register_source(
            agent,
            SourceRequest {
                source: source.to_string(),
                sha256: Some(sha256),
                content: None,
            },
        )?;
        let artifact = artifact["artifact"]
            .as_str()
            .ok_or_else(|| ToolFail::new("internal", "no artifact id"))?
            .to_string();
        self.carry(agent, session, vec![artifact])
    }

    /// Add `artifacts` to the session, skipping any whose sources it
    /// already carries (the lineage, not the bytes, decides).
    fn carry(&self, agent: &str, session: &str, artifacts: Vec<String>) -> Result<(), ToolFail> {
        let mut rt = self.server.lock()?;
        self.session_live(session)?;
        let engine = rt.engine();
        let mut carried: BTreeSet<String> = engine
            .session(session)
            .map(|s| s.sources.into_iter().collect())
            .unwrap_or_default();
        let mut new = Vec::new();
        for artifact in artifacts {
            let sources = engine
                .lineage(&artifact)
                .ok_or_else(|| ToolFail::new("unknown_artifact", "no such artifact"))?;
            if !sources.is_subset(&carried) {
                carried.extend(sources);
                new.push(artifact);
            }
        }
        if new.is_empty() {
            return Ok(());
        }
        self.server.agent_quota(&rt, agent, false)?;
        let event = Event::SessionCarry {
            session: session.to_string(),
            agent: agent.to_string(),
            artifacts: new,
        };
        match self.server.agent_handle(&mut rt, agent, event)? {
            Reply::Session { .. } => Ok(()),
            Reply::Rejected { code, message } => Err(ToolFail::new(&code, message)),
            _ => Err(ToolFail::new(
                "internal",
                "the engine sent an unknown reply",
            )),
        }
    }

    /// Check an action call: record the canonical JSON of its arguments as
    /// an output of everything the session carries, then check it. `Ok`
    /// only on allow (or, with `allow_without_reads`, when the session has
    /// read nothing). An allow from an approval uses the approval up.
    fn decide(
        &self,
        agent: &str,
        session: &str,
        tool: &Tool,
        call: HeldCall,
        allow_without_reads: bool,
    ) -> Result<Forward, ToolFail> {
        if self.shared.is_blocked(session) {
            return Err(ToolFail::new(
                "session_blocked",
                "this session was handed a token it refused, so it may not act; the operator must end the session on the operator page",
            ));
        }
        if self.shared.is_blocked(&agent_key(agent)) {
            return Err(ToolFail::new(
                "agent_blocked",
                "this agent was handed a token it refused, so none of its connections may act; the operator must clear the block on the operator page",
            ));
        }
        let carried = {
            let rt = self.server.lock()?;
            self.session_live(session)?;
            let policy = rt
                .engine()
                .policy()
                .ok_or_else(|| ToolFail::new("internal", "no active policy"))?;
            if !config::tool_problems(tool, policy).is_empty() {
                return Err(ToolFail::misconfigured(tool));
            }
            rt.engine()
                .session(session)
                .map(|s| s.artifacts)
                .unwrap_or_default()
        };
        if carried.is_empty() {
            if allow_without_reads {
                return Ok(Forward::Unchecked);
            }
            return Err(ToolFail::new(
                "nothing_read",
                "this session has read nothing through the gateway, so there is nothing to check the action against; it was not forwarded",
            ));
        }
        let output = self
            .server
            .record_output(
                agent,
                OutputRequest {
                    inputs: carried,
                    step: format!("gateway:{}", tool.exposed),
                    sha256: Some(call.sha256.clone()),
                    content: None,
                },
            )
            .map_err(|e| match e.code.as_str() {
                "unknown_artifact" => ToolFail::new(
                    "session_not_usable",
                    "this session carries a source this agent may no longer use; it was not forwarded",
                ),
                _ => e.into(),
            })?;
        let artifact = output["artifact"]
            .as_str()
            .ok_or_else(|| ToolFail::new("internal", "no artifact id"))?
            .to_string();
        let action = call.action.clone();
        let decision = self.server.check_once(
            agent,
            CheckRequest {
                artifact,
                action: action.clone(),
                recipients: None,
            },
        )?;
        let id = decision["decision"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let reason = decision["reason"].as_str().unwrap_or_default().to_string();
        let extra = Some(json!({"decision": id, "verdict": decision["verdict"].clone()}));
        match decision["verdict"].as_str() {
            Some("allow") => Ok(Forward::Checked(
                (decision["consumed"] == true).then_some(id),
            )),
            Some("needs_approval") => {
                self.shared.hold(&id, call);
                Err(ToolFail {
                    code: "needs_approval".into(),
                    message: format!(
                        "`{action}` was not forwarded: {reason}. It is held for approval as decision {}; an approval releases one call: retry exactly the same call after a reviewer approves it",
                        id.get(..12).unwrap_or(&id)
                    ),
                    extra,
                })
            }
            _ => Err(ToolFail {
                code: "denied".into(),
                message: format!("`{action}` was not forwarded: {reason}"),
                extra,
            }),
        }
    }

    fn handoff(&self, agent: &str, session: &str) -> Result<Value, ToolFail> {
        let rt = self.server.lock()?;
        let now = self.server.now(&rt);
        let carried = rt
            .engine()
            .session(session)
            .filter(|s| !s.artifacts.is_empty())
            .ok_or_else(|| {
                ToolFail::new(
                    "nothing_read",
                    "this session has read nothing through the gateway; there is nothing to hand off",
                )
            })?;
        let expires_at = now.saturating_add(LIFETIME_SECONDS);
        let token = self.signer.sign(&Payload {
            agent: agent.to_string(),
            seq: rt.last_seq(),
            expires_at,
            artifacts: carried.artifacts,
        });
        Ok(json!({
            "handoff": token,
            "carries": carried.sources,
            "expires_at": expires_at,
        }))
    }

    fn receive(&self, agent: &str, session: &str, token: &str) -> Result<Value, ToolFail> {
        let invalid = || {
            ToolFail::new(
                "invalid_handoff",
                "that is not a valid hand-off token from this gateway, or it has expired; this session may no longer act until the operator ends it",
            )
        };
        let payload = self.signer.verify(token).ok_or_else(invalid)?;
        let sources: BTreeSet<String> = {
            let rt = self.server.lock()?;
            let now = self.server.now(&rt);
            let policy = rt
                .engine()
                .policy()
                .ok_or_else(|| ToolFail::new("internal", "no active policy"))?;
            if payload.expires_at <= now
                || payload.seq > rt.last_seq()
                || !policy.agents.contains(&payload.agent)
            {
                return Err(invalid());
            }
            let mut sources = BTreeSet::new();
            for artifact in &payload.artifacts {
                sources.extend(rt.engine().lineage(artifact).ok_or_else(invalid)?);
            }
            // Refuse before recording anything when any source is not this agent's.
            if let Some(name) = sources.iter().find(|name| {
                !policy
                    .sources
                    .get(*name)
                    .is_some_and(|s| s.agents.contains(agent))
            }) {
                return Err(ToolFail::new(
                    "not_cleared",
                    format!(
                        "refused: this hand-off carries `{name}`, which this agent is not cleared for; this session may no longer act until the operator ends it"
                    ),
                ));
            }
            sources
        };
        self.carry(agent, session, payload.artifacts)?;
        Ok(json!({"received": true, "carries": sources}))
    }

    /// At start: every reachable upstream's tools against the config. The
    /// problems; unreachable upstreams are reported on stderr and checked
    /// again on every call.
    pub async fn verify_upstreams(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (name, upstream) in self.upstreams.iter() {
            let published = match upstream.tools().await {
                Ok(tools) => tools,
                Err(_) => {
                    eprintln!(
                        "trustgraph gateway: upstream `{name}` is not reachable at start; its tools' arguments are checked when it connects, and the tools stay closed until they match"
                    );
                    continue;
                }
            };
            for tool in self
                .shared
                .config
                .tools
                .values()
                .filter(|t| &t.upstream == name)
            {
                let found = published.iter().find(|t| t.name == tool.name);
                problems.extend(schema_report(tool, found).0);
            }
        }
        problems
    }
}

fn structured(value: Value) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(value.to_string())]);
    result.structured_content = Some(value);
    result
}

/// Every route of `serve` plus `/gateway/mcp`.
pub fn router<J: ServeJournal>(gateway: Arc<Gateway<J>>) -> Router {
    let server = gateway.server.clone();
    super::router_with(server, handler::router(gateway))
}

// ----- `trustgraph gateway` and `trustgraph gateway check` -----

/// Read and parse `gateway.yaml`; every problem as a line.
pub fn load_config(path: &Path) -> Result<Config, Vec<String>> {
    let io = |e: &dyn std::fmt::Display| vec![format!("cannot read {}: {e}", path.display())];
    let metadata = std::fs::metadata(path).map_err(|e| io(&e))?;
    if !metadata.is_file() || metadata.len() > config::MAX_CONFIG_BYTES as u64 {
        return Err(io(&"not a regular file under the size limit"));
    }
    let text = std::fs::read_to_string(path).map_err(|e| io(&e))?;
    config::parse(&text).map_err(|problems| problems.iter().map(ToString::to_string).collect())
}

/// `trustgraph gateway`: returns the process exit code.
pub async fn main(options: super::Options, config_path: PathBuf, port: u16) -> i32 {
    let config = match load_config(&config_path) {
        Ok(c) => c,
        Err(problems) => {
            eprintln!("error: {} is not valid:", config_path.display());
            for p in problems {
                eprintln!("  {p}");
            }
            return 1;
        }
    };
    for warning in config::warnings(&config) {
        eprintln!("warning: {warning}");
    }
    let stale = upstream::remove_stale_homes(&std::env::temp_dir());
    if stale > 0 {
        eprintln!(
            "trustgraph gateway: removed {stale} private upstream HOME directories a stopped gateway left behind"
        );
    }
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: cannot listen on 127.0.0.1:{port}: {e}");
            return 2;
        }
    };
    let port = listener.local_addr().map_or(port, |a| a.port());
    let server: Arc<Server<FileJournal>> = match super::start(options, port) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {}", e.message);
            return e.code;
        }
    };
    let lookup = |name: &str| std::env::var(name).ok();
    let gateway = match attach(server.clone(), config, &lookup) {
        Ok(g) => g,
        Err(problems) => {
            eprintln!(
                "error: {} does not work with the active policy or environment:",
                config_path.display()
            );
            for p in problems {
                eprintln!("  {p}");
            }
            return 1;
        }
    };
    let code = serve_gateway(&gateway, &server, &config_path, listener, port).await;
    // Every exit path stops the stdio children and removes their private
    // HOME directories before the process ends.
    gateway.upstreams.shutdown().await;
    code
}

async fn serve_gateway(
    gateway: &Arc<Gateway<FileJournal>>,
    server: &Server<FileJournal>,
    config_path: &Path,
    listener: tokio::net::TcpListener,
    port: u16,
) -> i32 {
    let problems = gateway.verify_upstreams().await;
    if !problems.is_empty() {
        eprintln!(
            "error: {} does not match what its upstreams publish:",
            config_path.display()
        );
        for p in problems {
            eprintln!("  {p}");
        }
        return 1;
    }
    if let Err(code) = super::banner(server, port, "TrustGraph gateway", "/gateway/mcp") {
        return code;
    }
    println!(
        "Sessions: {}; tools exposed: {}",
        gateway.config().session.as_str(),
        gateway.config().tools.len() + 2
    );
    super::run(listener, router(gateway.clone())).await
}

/// `trustgraph gateway check`: validate `gateway.yaml` against a policy and
/// say in plain English what it does. Unless `offline`, upstreams whose
/// variables are set here are contacted (stdio ones are run) to check
/// argument names. Returns the exit code.
pub async fn check(config_path: &Path, policy_path: &Path, offline: bool) -> i32 {
    let config = match load_config(config_path) {
        Ok(c) => c,
        Err(problems) => {
            println!(
                "{} has {} problem(s):",
                config_path.display(),
                problems.len()
            );
            for p in problems {
                println!("  {p}");
            }
            return 1;
        }
    };
    let text = match std::fs::read_to_string(policy_path) {
        Ok(t) => t,
        Err(e) => {
            println!("cannot read {}: {e}", policy_path.display());
            return 2;
        }
    };
    let Some(policy) = trustgraph_policy::parse(&text).policy else {
        println!(
            "{} is not a valid policy; run `trustgraph policy check` on it",
            policy_path.display()
        );
        return 1;
    };
    let mut problems: Vec<String> = config::check_against(&config, &policy)
        .iter()
        .map(ToString::to_string)
        .collect();
    let (published, contact) = if offline {
        let contact = Contact {
            notes: vec![
                "note: --offline: no upstream was contacted and no stdio command was run, so the checks against what upstreams publish (tool names, argument names, schema size) were skipped".into(),
            ],
            missing: config::env_names(&config)
                .into_iter()
                .filter(|n| std::env::var_os(n).is_none())
                .collect(),
            ..Contact::default()
        };
        let published = config.upstreams.keys().map(|n| (n.clone(), None)).collect();
        (published, contact)
    } else {
        contact_upstreams(&config).await
    };
    for tool in config.tools.values() {
        if let Some(Some(tools)) = published.get(&tool.upstream) {
            let found = tools.iter().find(|t| t.name == tool.name);
            problems.extend(schema_report(tool, found).0);
        }
    }
    problems.extend(contact.problems);
    if !problems.is_empty() {
        println!(
            "{} does not match {} or its upstreams ({} problem(s)):",
            config_path.display(),
            policy_path.display(),
            problems.len()
        );
        for p in problems {
            println!("  {p}");
        }
        return 1;
    }
    for line in summary(&config, &policy, &published) {
        println!("{line}");
    }
    for warning in config::warnings(&config) {
        println!("warning: {warning}");
    }
    for line in contact.notes {
        println!("{line}");
    }
    if !contact.missing.is_empty() {
        println!(
            "warning: not set here, and required where the gateway runs: {}",
            contact.missing.join(", ")
        );
    }
    0
}

#[derive(Default)]
struct Contact {
    problems: Vec<String>,
    notes: Vec<String>,
    missing: Vec<String>,
}

/// The tools of every upstream that can be reached from here (by name;
/// `None` when it was not contacted), with what kept the others out.
async fn contact_upstreams(
    config: &Config,
) -> (BTreeMap<String, Option<Vec<rmcp::model::Tool>>>, Contact) {
    let mut contact = Contact {
        missing: config::env_names(config)
            .into_iter()
            .filter(|n| std::env::var_os(n).is_none())
            .collect(),
        ..Contact::default()
    };
    let mut reachable = config.clone();
    reachable.upstreams.retain(|name, upstream| {
        let names: Vec<String> = match upstream {
            config::Upstream::Http { headers, .. } => headers,
            config::Upstream::Stdio { env, .. } => env,
        }
        .iter()
        .flat_map(|(_, t)| t.names().map(str::to_string).collect::<Vec<_>>())
        .collect();
        let ready = names.iter().all(|n| std::env::var_os(n).is_some());
        if !ready {
            contact.notes.push(format!(
                "note: upstream `{name}` was not contacted (its variables are not set here), so its argument names were not checked"
            ));
        }
        ready
    });
    let mut published: BTreeMap<String, Option<Vec<rmcp::model::Tool>>> =
        config.upstreams.keys().map(|n| (n.clone(), None)).collect();
    let lookup = |name: &str| std::env::var(name).ok();
    let upstreams = match upstream::resolve(&reachable, &lookup) {
        Ok(u) => u,
        Err(problems) => {
            contact.problems.extend(problems);
            return (published, contact);
        }
    };
    for (name, upstream) in upstreams.iter() {
        match upstream.tools().await {
            Ok(tools) => {
                published.insert(name.clone(), Some(tools));
            }
            Err(e) => contact.notes.push(format!(
                "note: upstream `{name}` could not be contacted ({}), so its argument names were not checked",
                e.code()
            )),
        }
    }
    (published, contact)
}

fn people(names: &BTreeSet<String>) -> String {
    if names.is_empty() {
        "no agent".into()
    } else {
        names.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

fn when_text(when: &[config::Matcher]) -> String {
    if when.is_empty() {
        "always (every call, whatever its arguments)".into()
    } else {
        let parts: Vec<String> = when
            .iter()
            .map(|m| match m.kind {
                config::MatchType::Exact => format!("`{}` is exactly `{}`", m.argument, m.pattern),
                config::MatchType::Email if m.pattern.starts_with("*@") => format!(
                    "`{}` is a plain email address at exactly `{}`",
                    m.argument,
                    &m.pattern[2..]
                ),
                config::MatchType::Email => {
                    format!("`{}` is the email address `{}`", m.argument, m.pattern)
                }
                config::MatchType::Glob => {
                    format!("`{}` matches the glob `{}`", m.argument, m.pattern)
                }
                config::MatchType::Path => format!("`{}` matches `{}`", m.argument, m.pattern),
            })
            .collect();
        format!("when {}", parts.join(" and "))
    }
}

fn names(set: &BTreeSet<String>) -> String {
    if set.is_empty() {
        "none".into()
    } else {
        set.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// The plain-English summary `gateway check` prints. `published` holds the
/// tools of each upstream that was contacted.
pub fn summary(
    config: &Config,
    policy: &Policy,
    published: &BTreeMap<String, Option<Vec<rmcp::model::Tool>>>,
) -> Vec<String> {
    let mut lines = vec![format!(
        "Sessions: {} ({}).",
        config.session.as_str(),
        match config.session {
            SessionMode::Connection =>
                "each MCP connection is one session; it assumes the host opens a new connection per task with a fresh agent context, and that the agent cannot reconnect at will: a client that reconnects by itself starts with an empty session. Use `session: agent` when the host cannot guarantee that",
            SessionMode::Agent =>
                "one session per agent for the gateway's lifetime; only the operator can end it",
        }
    )];
    lines.push(format!(
        "Upstreams: {}.",
        config
            .upstreams
            .iter()
            .map(|(name, u)| match u {
                config::Upstream::Http { url, .. } => format!("{name} ({url})"),
                config::Upstream::Stdio { command, .. } => format!("{name} (runs {})", command[0]),
            })
            .collect::<Vec<_>>()
            .join(", ")
    ));
    lines.push(format!(
        "Agents see {} tools; every other upstream tool is invisible and uncallable:",
        config.tools.len() + 2
    ));
    let agents_of = |source: &str| {
        policy
            .sources
            .get(source)
            .map(|s| s.agents.clone())
            .unwrap_or_default()
    };
    for tool in config.tools.values() {
        match &tool.kind {
            Kind::Reads { rules, otherwise } => {
                lines.push(format!(
                    "  {} reads a source; its result is recorded before it is returned:",
                    tool.exposed
                ));
                for rule in rules {
                    lines.push(format!(
                        "    {}: `{}` (usable by {})",
                        when_text(&rule.when),
                        rule.source,
                        people(&agents_of(&rule.source))
                    ));
                }
                lines.push(format!(
                    "    otherwise: `{otherwise}` (usable by {})",
                    people(&agents_of(otherwise))
                ));
            }
            Kind::Acts {
                rules,
                allow_without_reads,
            } => {
                lines.push(format!(
                    "  {} is an action, checked against everything the session read before it is forwarded; the output checked is every forwarded argument:",
                    tool.exposed
                ));
                for rule in rules {
                    let audience = policy
                        .actions
                        .get(&rule.action)
                        .map(|a| policy.people(&a.audience))
                        .unwrap_or_default();
                    lines.push(format!(
                        "    {}: `{}`{} (reaches {})",
                        when_text(&rule.when),
                        rule.action,
                        rule.content
                            .as_deref()
                            .map_or(String::new(), |c| format!(", content `{c}`")),
                        people(&audience)
                    ));
                }
                lines.push("    otherwise: blocked, never forwarded".into());
                if *allow_without_reads {
                    lines.push(
                        "    not checked when nothing was read: a call from a session that has read nothing is forwarded without a check, and logged".into(),
                    );
                } else {
                    lines.push(
                        "    a call from a session that has read nothing is refused (nothing_read)"
                            .into(),
                    );
                }
            }
            Kind::Passthrough { untracked: true } => lines.push(format!(
                "  {} is forwarded untouched and its results are NOT tracked",
                tool.exposed
            )),
            Kind::Passthrough { untracked: false } => lines.push(format!(
                "  {} is forwarded untouched; its results are recorded as `{UNCLASSIFIED}` (usable by {})",
                tool.exposed,
                people(&agents_of(UNCLASSIFIED))
            )),
        }
        lines.push(format!(
            "    forwards only the arguments: {}; a call with any other is refused",
            names(&tool.arguments)
        ));
        if let Some(Some(tools)) = published.get(&tool.upstream)
            && let Some(found) = tools.iter().find(|t| t.name == tool.name)
        {
            let unforwarded: BTreeSet<String> =
                schema_report(tool, Some(found)).1.into_iter().collect();
            if !unforwarded.is_empty() {
                lines.push(format!(
                    "    upstream arguments NOT forwardable: {}",
                    names(&unforwarded)
                ));
            }
        }
    }
    lines.push(format!(
        "  {HANDOFF_TOOL} and {RECEIVE_TOOL} pass what a session carries to another agent's session"
    ));
    lines
}
