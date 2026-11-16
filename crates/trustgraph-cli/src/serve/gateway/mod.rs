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
//! - A read result is returned only after it is registered as its source and
//!   recorded in the session (`SessionCarry`); otherwise it is withheld.
//! - An action is forwarded only on an `allow` from a completed check of an
//!   output derived from everything the session carries. A session that has
//!   read nothing is refused (`nothing_read`) unless the tool says
//!   `allow_without_reads: true`; then it is forwarded unchecked and logged.
//! - A tool that is not listed, or whose source the agent may not use, gets
//!   the same "not available" error and the upstream is not called.
//! - Upstream failures of any kind are short tool errors with no detail.

pub mod bridge;
pub mod config;
mod handler;
pub mod handoff;
mod upstream;

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    ApiError, CheckRequest, GatewayView, OutputRequest, ServeJournal, Server, SourceRequest,
    sha256_hex,
};

/// The gateway's own tools, exposed to every agent.
pub const HANDOFF_TOOL: &str = "trustgraph__handoff";
pub const RECEIVE_TOOL: &str = "trustgraph__receive";
/// Live MCP connections per agent in `connection` mode.
pub const MAX_CONNECTIONS_PER_AGENT: usize = 64;

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
                "`{}` names a source or action the active policy does not have, so it is closed; the operator must fix gateway.yaml or the policy",
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

/// One live MCP connection in `connection` mode.
#[derive(Debug, Clone)]
struct Conn {
    agent: String,
    mcp: String,
    started_at: u64,
}

#[derive(Default)]
struct Connections {
    /// MCP session id -> gateway session id.
    by_mcp: HashMap<String, String>,
    /// Gateway session id -> connection.
    sessions: BTreeMap<String, Conn>,
}

/// What the server's operator routes see of the gateway.
pub(crate) struct Shared {
    config: Config,
    conns: Mutex<Connections>,
    manager: Arc<LocalSessionManager>,
}

impl Shared {
    fn conns(&self) -> std::sync::MutexGuard<'_, Connections> {
        self.conns.lock().unwrap_or_else(|p| p.into_inner())
    }
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

    fn connections(&self) -> Vec<(String, String, u64)> {
        self.conns()
            .sessions
            .iter()
            .map(|(id, c)| (id.clone(), c.agent.clone(), c.started_at))
            .collect()
    }

    fn close(&self, session: &str) -> bool {
        let conn = {
            let mut conns = self.conns();
            let conn = conns.sessions.remove(session);
            if let Some(c) = &conn {
                conns.by_mcp.remove(&c.mcp);
            }
            conn
        };
        let Some(conn) = conn else {
            return false;
        };
        self.close_mcp(&conn.mcp);
        true
    }
}

impl Shared {
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
        Value::Object(map) => {
            let sorted: BTreeMap<&String, Value> =
                map.iter().map(|(k, v)| (k, canonical(v))).collect();
            Value::Object(sorted.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// The output content of an action call: the named argument (its text when
/// it is a string), or all arguments.
fn content_bytes(arguments: &JsonObject, name: Option<&str>) -> Result<Vec<u8>, ToolFail> {
    match name {
        None => Ok(
            serde_json::to_vec(&canonical(&Value::Object(arguments.clone()))).unwrap_or_default(),
        ),
        Some(name) => match arguments.get(name) {
            Some(Value::String(text)) => Ok(text.as_bytes().to_vec()),
            Some(other) => Ok(serde_json::to_vec(&canonical(other)).unwrap_or_default()),
            None => Err(ToolFail::new(
                "invalid_arguments",
                format!(
                    "the call has no `{name}` argument, which is its output; it was not forwarded"
                ),
            )),
        },
    }
}

/// What a checked action call may do.
enum Forward {
    Checked,
    /// `allow_without_reads` and the session has read nothing.
    Unchecked,
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
        // client closes them, the operator ends them, or on restart.
        manager.session_config.keep_alive = None;
    }
    let shared = Arc::new(Shared {
        config,
        conns: Mutex::new(Connections::default()),
        manager: Arc::new(manager),
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

/// Every two seconds: a connection whose MCP session rmcp closed (the
/// client's DELETE, or a dropped connection) ends its gateway session.
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
        let closed: Vec<String> = {
            let mut conns = gateway.shared.conns();
            let gone: Vec<(String, String)> = conns
                .by_mcp
                .iter()
                .filter(|(mcp, _)| !live.contains(*mcp))
                .map(|(m, s)| (m.clone(), s.clone()))
                .collect();
            for (mcp, session) in &gone {
                conns.by_mcp.remove(mcp);
                conns.sessions.remove(session);
            }
            gone.into_iter().map(|(_, s)| s).collect()
        };
        if closed.is_empty() {
            continue;
        }
        let server = gateway.server.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let Ok(mut rt) = server.lock() else {
                return;
            };
            for session in closed {
                if rt.engine().session(&session).is_some() {
                    let _ = server.handle(&mut rt, Event::EndSession { session });
                }
            }
        })
        .await;
    }
}

impl<J: ServeJournal> Gateway<J> {
    pub fn server(&self) -> &Arc<Server<J>> {
        &self.server
    }

    pub fn config(&self) -> &Config {
        &self.shared.config
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
        if self.shared.config.session != SessionMode::Connection {
            return true;
        }
        let started_at = self.server.time.now();
        let mut conns = self.shared.conns();
        let open = conns.sessions.values().filter(|c| c.agent == agent).count();
        if open >= MAX_CONNECTIONS_PER_AGENT {
            return false;
        }
        let session = format!("conn:{}", &super::auth::random_secret()[..24]);
        conns.by_mcp.insert(mcp.to_string(), session.clone());
        conns.sessions.insert(
            session,
            Conn {
                agent: agent.to_string(),
                mcp: mcp.to_string(),
                started_at,
            },
        );
        true
    }

    /// True when `mcp` may be used by `agent`: in `connection` mode it must
    /// be a session this agent opened.
    fn may_use(&self, mcp: &str, agent: &str) -> bool {
        if self.shared.config.session != SessionMode::Connection {
            return true;
        }
        let conns = self.shared.conns();
        conns
            .by_mcp
            .get(mcp)
            .and_then(|s| conns.sessions.get(s))
            .is_some_and(|c| c.agent == agent)
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
                let session = conns.by_mcp.get(mcp).ok_or_else(missing)?;
                match conns.sessions.get(session) {
                    Some(c) if c.agent == agent => Ok(session.clone()),
                    _ => Err(missing()),
                }
            }
        }
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
                    let value = self
                        .blocking(move |g| g.receive(&agent, &session, &token))
                        .await?;
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
                    self.call_tool(agent, session, tool, arguments).await
                }
            }
        }
        .await;
        outcome.unwrap_or_else(|fail| fail.result())
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
                let sha = sha256_hex(&content_bytes(&arguments, rule.content.as_deref())?);
                let allow_without_reads = *allow_without_reads;
                let tool2 = tool.clone();
                let forward = self
                    .blocking(move |g| {
                        g.decide(
                            &agent,
                            &session,
                            &tool2,
                            &rule.action,
                            sha,
                            allow_without_reads,
                        )
                    })
                    .await?;
                if let Forward::Unchecked = forward {
                    eprintln!(
                        "trustgraph gateway: `{}` was called with nothing read and forwarded without a check (allow_without_reads)",
                        tool.exposed
                    );
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

    /// Check an action call: record its content as an output of everything
    /// the session carries, then check it. `Ok` only on allow (or, with
    /// `allow_without_reads`, when the session has read nothing).
    fn decide(
        &self,
        agent: &str,
        session: &str,
        tool: &Tool,
        action: &str,
        sha256: String,
        allow_without_reads: bool,
    ) -> Result<Forward, ToolFail> {
        let carried = {
            let rt = self.server.lock()?;
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
                    sha256: Some(sha256),
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
        let decision = self.server.check(
            agent,
            CheckRequest {
                artifact,
                action: action.to_string(),
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
            Some("allow") => Ok(Forward::Checked),
            Some("needs_approval") => Err(ToolFail {
                code: "needs_approval".into(),
                message: format!(
                    "`{action}` was not forwarded: {reason}. It is held for approval as decision {}; retry the same call after a reviewer approves it",
                    id.get(..12).unwrap_or(&id)
                ),
                extra,
            }),
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
                "that is not a valid hand-off token from this gateway, or it has expired",
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
                        "refused: this hand-off carries `{name}`, which this agent is not cleared for"
                    ),
                ));
            }
            sources
        };
        self.carry(agent, session, payload.artifacts)?;
        Ok(json!({"received": true, "carries": sources}))
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
    if let Err(code) = super::banner(&server, port, "TrustGraph gateway", "/gateway/mcp") {
        return code;
    }
    println!(
        "Sessions: {}; tools exposed: {}",
        gateway.config().session.as_str(),
        gateway.config().tools.len() + 2
    );
    super::run(listener, router(gateway)).await
}

/// `trustgraph gateway check`: validate `gateway.yaml` against a policy and
/// say in plain English what it does. Returns the exit code.
pub fn check(config_path: &Path, policy_path: &Path) -> i32 {
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
    let problems = config::check_against(&config, &policy);
    if !problems.is_empty() {
        println!(
            "{} does not match {} ({} problem(s)):",
            config_path.display(),
            policy_path.display(),
            problems.len()
        );
        for p in problems {
            println!("  {p}");
        }
        return 1;
    }
    for line in summary(&config, &policy) {
        println!("{line}");
    }
    let missing: Vec<String> = config::env_names(&config)
        .into_iter()
        .filter(|n| std::env::var_os(n).is_none())
        .collect();
    if !missing.is_empty() {
        println!(
            "warning: not set here, and required where the gateway runs: {}",
            missing.join(", ")
        );
    }
    0
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
        "always".into()
    } else {
        let parts: Vec<String> = when
            .iter()
            .map(|m| format!("`{}` matches `{}`", m.argument, m.glob))
            .collect();
        format!("when {}", parts.join(" and "))
    }
}

/// The plain-English summary `gateway check` prints.
pub fn summary(config: &Config, policy: &Policy) -> Vec<String> {
    let mut lines = vec![format!(
        "Sessions: {} ({}).",
        config.session.as_str(),
        match config.session {
            SessionMode::Connection =>
                "each MCP connection is one session; it assumes the host opens a new connection per task and the agent cannot reconnect at will",
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
                    "  {} is an action, checked against everything the session read before it is forwarded:",
                    tool.exposed
                ));
                for rule in rules {
                    let audience = policy
                        .actions
                        .get(&rule.action)
                        .map(|a| policy.people(&a.audience))
                        .unwrap_or_default();
                    lines.push(format!(
                        "    {}: `{}` with {} as the output (reaches {})",
                        when_text(&rule.when),
                        rule.action,
                        rule.content
                            .as_deref()
                            .map_or("all arguments".to_string(), |c| format!("`{c}`")),
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
    }
    lines.push(format!(
        "  {HANDOFF_TOOL} and {RECEIVE_TOOL} pass what a session carries to another agent's session"
    ));
    lines
}
