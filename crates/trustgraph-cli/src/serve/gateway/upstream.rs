//! Connections to the upstream MCP servers: streamable HTTP or a child
//! process's stdio, connected on first use and again after a failure.
//!
//! Credentials stay here. Header and environment values are [`Secret`]s;
//! an upstream child gets only its configured environment plus `PATH` and
//! `HOME`; its stderr is forwarded with every secret value replaced; and an
//! error an agent sees never carries transport detail, only the upstream's
//! name.

use std::collections::{BTreeMap, HashMap};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderName, HeaderValue};
use rmcp::RoleClient;
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, JsonObject, Tool};
use rmcp::service::{Peer, RunningService};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use tokio::io::AsyncBufReadExt;

use super::config::{Config, Secret, Upstream};

/// Replaces every secret value in text the gateway prints.
#[derive(Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut secrets: Vec<String> = secrets.into_iter().filter(|s| !s.is_empty()).collect();
        // Longest first, so a secret that contains another is replaced whole.
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        Redactor { secrets }
    }

    /// True when `bytes` hold any secret value.
    pub fn leaks(&self, bytes: &[u8]) -> bool {
        self.secrets
            .iter()
            .any(|s| bytes.windows(s.len()).any(|w| w == s.as_bytes()))
    }

    pub fn redact(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.secrets {
            out = out.replace(secret.as_str(), "[redacted]");
        }
        out
    }
}

/// Why an upstream call did not return a result. The agent sees only the
/// short text of each; detail goes to the log, redacted.
#[derive(Debug)]
pub enum CallError {
    Unreachable,
    Timeout,
    Failed,
    Oversize,
    Malformed,
}

impl CallError {
    pub fn code(&self) -> &'static str {
        match self {
            CallError::Unreachable => "upstream_unreachable",
            CallError::Timeout => "upstream_timeout",
            CallError::Failed => "upstream_error",
            CallError::Oversize => "upstream_result_too_large",
            CallError::Malformed => "upstream_malformed",
        }
    }

    pub fn message(&self, upstream: &str) -> String {
        match self {
            CallError::Unreachable => format!("the upstream `{upstream}` cannot be reached"),
            CallError::Timeout => format!("the upstream `{upstream}` did not answer in time"),
            CallError::Failed => format!("the upstream `{upstream}` returned an error"),
            CallError::Oversize => {
                format!("the upstream `{upstream}` returned a result over the size limit")
            }
            CallError::Malformed => {
                format!("the upstream `{upstream}` returned a result the gateway cannot use")
            }
        }
    }
}

/// How to reach one upstream, with its secrets resolved.
enum Spec {
    Http {
        url: String,
        headers: HashMap<HeaderName, HeaderValue>,
    },
    Stdio {
        command: Vec<String>,
        env: Vec<(String, Secret)>,
    },
}

struct Connection {
    service: RunningService<RoleClient, ()>,
    tools: Vec<Tool>,
}

pub struct UpstreamConn {
    name: String,
    spec: Spec,
    timeout: Duration,
    max_result_bytes: usize,
    redactor: Arc<Redactor>,
    live: tokio::sync::Mutex<Option<Connection>>,
}

/// Every upstream of one config.
pub struct Upstreams {
    by_name: BTreeMap<String, Arc<UpstreamConn>>,
}

/// Resolve every upstream's secrets with `lookup`. A missing variable is
/// an error naming it (never its value).
pub fn resolve(
    config: &Config,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<Upstreams, Vec<String>> {
    let mut problems = Vec::new();
    let mut secrets = Vec::new();
    let mut by_name = BTreeMap::new();
    let mut specs = Vec::new();
    for (name, upstream) in &config.upstreams {
        let spec = match upstream {
            Upstream::Http { url, headers } => {
                let mut resolved = HashMap::new();
                for (header, template) in headers {
                    let here = format!("upstreams.{name}.headers.{header}");
                    let value = match template.resolve(lookup) {
                        Ok(v) => v,
                        Err(missing) => {
                            problems.push(format!(
                                "{here}: the environment variable `{missing}` is not set"
                            ));
                            continue;
                        }
                    };
                    let (Ok(header), Ok(value)) = (
                        HeaderName::try_from(header.as_str()),
                        HeaderValue::from_str(value.expose()),
                    ) else {
                        problems.push(format!("{here}: not a valid HTTP header"));
                        continue;
                    };
                    let mut value = value;
                    value.set_sensitive(true);
                    secrets.extend(template.names().filter_map(lookup));
                    resolved.insert(header, value);
                }
                Spec::Http {
                    url: url.clone(),
                    headers: resolved,
                }
            }
            Upstream::Stdio { command, env } => {
                let mut resolved = Vec::new();
                for (var, template) in env {
                    match template.resolve(lookup) {
                        Ok(value) => {
                            secrets.extend(template.names().filter_map(lookup));
                            resolved.push((var.clone(), value));
                        }
                        Err(missing) => problems.push(format!(
                            "upstreams.{name}.env.{var}: the environment variable `{missing}` is not set"
                        )),
                    }
                }
                Spec::Stdio {
                    command: command.clone(),
                    env: resolved,
                }
            }
        };
        specs.push((name.clone(), spec));
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let redactor = Arc::new(Redactor::new(secrets));
    for (name, spec) in specs {
        by_name.insert(
            name.clone(),
            Arc::new(UpstreamConn {
                name,
                spec,
                timeout: config.timeout,
                max_result_bytes: config.max_result_bytes,
                redactor: redactor.clone(),
                live: tokio::sync::Mutex::new(None),
            }),
        );
    }
    Ok(Upstreams { by_name })
}

impl Upstreams {
    pub fn get(&self, name: &str) -> Option<&Arc<UpstreamConn>> {
        self.by_name.get(name)
    }

    pub fn all(&self) -> impl Iterator<Item = &Arc<UpstreamConn>> {
        self.by_name.values()
    }
}

impl UpstreamConn {
    fn log(&self, what: &str, detail: &dyn std::fmt::Display) {
        eprintln!(
            "trustgraph gateway: upstream `{}`: {what}: {}",
            self.name,
            self.redactor.redact(&detail.to_string())
        );
    }

    async fn connect(&self) -> Result<Connection, CallError> {
        let service = match &self.spec {
            Spec::Http { url, headers } => {
                let config = StreamableHttpClientTransportConfig::with_uri(url.as_str())
                    .custom_headers(headers.clone())
                    // Big enough that an oversize result arrives and is
                    // refused by the cap with a clear error; still bounded.
                    .max_sse_event_size(self.max_result_bytes.saturating_mul(2).max(16 << 20));
                let transport = StreamableHttpClientTransport::from_config(config);
                ().serve(transport).await.map_err(|e| {
                    self.log("cannot connect", &e);
                    CallError::Unreachable
                })?
            }
            Spec::Stdio { command, env } => {
                let mut cmd = tokio::process::Command::new(&command[0]);
                cmd.args(&command[1..]).env_clear();
                for var in ["PATH", "HOME"] {
                    if let Some(value) = std::env::var_os(var) {
                        cmd.env(var, value);
                    }
                }
                for (var, value) in env {
                    cmd.env(var, value.expose());
                }
                cmd.kill_on_drop(true);
                let (process, stderr) = TokioChildProcess::builder(cmd)
                    .stderr(Stdio::piped())
                    .spawn()
                    .map_err(|e| {
                        self.log("cannot start", &e);
                        CallError::Unreachable
                    })?;
                if let Some(stderr) = stderr {
                    let redactor = self.redactor.clone();
                    let name = self.name.clone();
                    tokio::spawn(async move {
                        let mut lines = tokio::io::BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            eprintln!("upstream `{name}`: {}", redactor.redact(&line));
                        }
                    });
                }
                ().serve(process).await.map_err(|e| {
                    self.log("cannot initialize", &e);
                    CallError::Unreachable
                })?
            }
        };
        let tools = tokio::time::timeout(self.timeout, service.peer().list_all_tools()).await;
        let tools = match tools {
            Ok(Ok(tools)) => tools,
            Ok(Err(e)) => {
                self.log("cannot list tools", &e);
                return Err(CallError::Failed);
            }
            Err(_) => {
                self.log("cannot list tools", &"timed out");
                return Err(CallError::Timeout);
            }
        };
        Ok(Connection { service, tools })
    }

    /// The live peer and tool list, connecting when there is none.
    async fn peer(&self) -> Result<(Peer<RoleClient>, Vec<Tool>), CallError> {
        let mut live = self.live.lock().await;
        if let Some(c) = live.as_ref()
            && !c.service.is_closed()
        {
            return Ok((c.service.peer().clone(), c.tools.clone()));
        }
        *live = None;
        let connection = tokio::time::timeout(self.timeout, self.connect())
            .await
            .unwrap_or_else(|_| {
                self.log("cannot connect", &"timed out");
                Err(CallError::Timeout)
            })?;
        let out = (connection.service.peer().clone(), connection.tools.clone());
        *live = Some(connection);
        Ok(out)
    }

    /// Drop the connection so the next call connects again.
    async fn reset(&self) {
        if let Some(c) = self.live.lock().await.take() {
            let _ = c.service.cancel().await;
        }
    }

    /// The upstream's tools, connecting first when needed. Descriptions
    /// are passed through the redactor: they are upstream text.
    pub async fn tools(&self) -> Result<Vec<Tool>, CallError> {
        let mut tools = self.peer().await?.1;
        for tool in &mut tools {
            if let Some(d) = &tool.description {
                tool.description = Some(self.redactor.redact(d).into());
            }
        }
        Ok(tools)
    }

    /// Call one tool with a timeout; the result, checked against the size
    /// cap, with its serialized bytes.
    pub async fn call(
        &self,
        tool: &str,
        arguments: JsonObject,
    ) -> Result<(CallToolResult, Vec<u8>), CallError> {
        let (peer, _) = self.peer().await?;
        let params = CallToolRequestParams::new(tool.to_string()).with_arguments(arguments);
        let outcome = tokio::time::timeout(self.timeout, peer.call_tool_once(params)).await;
        let result = match outcome {
            Err(_) => {
                self.log(&format!("`{tool}`"), &"timed out");
                return Err(CallError::Timeout);
            }
            Ok(Err(e)) => {
                self.log(&format!("`{tool}`"), &e);
                if matches!(
                    e,
                    rmcp::ServiceError::TransportClosed | rmcp::ServiceError::TransportSend(_)
                ) {
                    self.reset().await;
                    return Err(CallError::Unreachable);
                }
                return Err(CallError::Failed);
            }
            Ok(Ok(CallToolResponse::Complete(result))) => result,
            Ok(Ok(_)) => {
                self.log(
                    &format!("`{tool}`"),
                    &"answered with input_required or a task, which the gateway does not support",
                );
                return Err(CallError::Malformed);
            }
        };
        let bytes = serde_json::to_vec(&result).map_err(|e| {
            self.log(&format!("`{tool}`"), &e);
            CallError::Malformed
        })?;
        if bytes.len() > self.max_result_bytes {
            self.log(
                &format!("`{tool}`"),
                &format!(
                    "result of {} bytes is over the {}-byte cap",
                    bytes.len(),
                    self.max_result_bytes
                ),
            );
            return Err(CallError::Oversize);
        }
        if self.redactor.leaks(&bytes) {
            self.log(
                &format!("`{tool}`"),
                &"the result holds an upstream credential; it is withheld",
            );
            return Err(CallError::Malformed);
        }
        Ok((result, bytes))
    }

    /// Connect in the background at start, so problems show early.
    pub async fn warm(&self) {
        if let Ok(tools) = self.tools().await {
            eprintln!(
                "trustgraph gateway: upstream `{}`: connected, {} tools",
                self.name,
                tools.len()
            );
        }
    }
}
