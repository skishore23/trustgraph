//! Connections to the upstream MCP servers: streamable HTTP or a child
//! process's stdio, connected on first use and again after a failure.
//!
//! Credentials stay here. Header and environment values are [`Secret`]s of
//! at least 12 characters; an upstream child gets only its configured
//! environment, a minimal `PATH` and a private, empty `HOME` (0700, locked
//! while in use, removed when the connection ends or the gateway shuts
//! down, and by the next gateway's startup if a killed one left it); its
//! stderr is forwarded with every secret
//! value replaced; and an error an agent sees never carries transport
//! detail, only the upstream's name. A child runs as the gateway's own OS
//! user, so it can still read whatever that user can (see docs/gateway.md).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
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

use super::config::{Config, MIN_SECRET_CHARS, Secret, Upstream};

/// Replaces every secret value in text the gateway prints.
#[derive(Default)]
pub struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut secrets: Vec<String> = secrets
            .into_iter()
            .filter(|s| !s.is_empty())
            .flat_map(|s| {
                // Also its JSON-escaped form, as it appears inside a result.
                let escaped = serde_json::to_string(&s).unwrap_or_default();
                let escaped = escaped
                    .get(1..escaped.len().saturating_sub(1))
                    .unwrap_or_default()
                    .to_string();
                if escaped != s && !escaped.is_empty() {
                    vec![s, escaped]
                } else {
                    vec![s]
                }
            })
            .collect();
        secrets.dedup();
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
    /// No answer in time. The action may still have happened upstream.
    Timeout,
    Failed,
    /// Over the size cap, malformed, or holding a credential. One code and
    /// one message for all three, so the error is no guess oracle.
    Withheld,
}

impl CallError {
    pub fn code(&self) -> &'static str {
        match self {
            CallError::Unreachable => "upstream_unreachable",
            CallError::Timeout => "upstream_timeout",
            CallError::Failed => "upstream_error",
            CallError::Withheld => "upstream_result_withheld",
        }
    }

    pub fn message(&self, upstream: &str) -> String {
        match self {
            CallError::Unreachable => format!("the upstream `{upstream}` cannot be reached"),
            CallError::Timeout => format!(
                "the upstream `{upstream}` did not answer in time; an action may still have happened there, and a retry sends it again"
            ),
            CallError::Failed => format!("the upstream `{upstream}` returned an error"),
            CallError::Withheld => format!(
                "the upstream `{upstream}` returned a result the gateway withholds (too large, malformed or unsafe to return)"
            ),
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

/// The name prefix of every private `HOME` in the temp directory.
pub const HOME_PREFIX: &str = "trustgraph-upstream-";
/// The lock file of a private `HOME`: beside it, so the `HOME` stays empty.
fn lock_path(home: &Path) -> PathBuf {
    let mut name = home.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    home.with_file_name(name)
}

/// A stdio upstream's `HOME`: a new, empty directory only the gateway's
/// user can open, removed when the connection is dropped. Beside it is a
/// lock file the gateway keeps locked (an exclusive OS lock, released when
/// the process ends however it ends), so a later gateway's startup cleanup
/// can tell a live one from one a killed process left behind.
struct PrivateHome {
    path: PathBuf,
    _lock: std::fs::File,
}

impl PrivateHome {
    fn create(upstream: &str) -> std::io::Result<Self> {
        Self::create_in(&std::env::temp_dir(), upstream)
    }

    /// The lock is made and taken before the directory exists, so no
    /// cleanup ever sees the directory unlocked.
    fn create_in(base: &Path, upstream: &str) -> std::io::Result<Self> {
        let id = &crate::serve::auth::random_secret()[..16];
        let path = base.join(format!("{HOME_PREFIX}{upstream}-{id}"));
        let lock = crate::serve::auth::private_create(&lock_path(&path))?;
        if let Err(e) = lock.try_lock() {
            let _ = std::fs::remove_file(lock_path(&path));
            return Err(std::io::Error::other(e));
        }
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        if let Err(e) = builder.create(&path) {
            let _ = std::fs::remove_file(lock_path(&path));
            return Err(e);
        }
        Ok(PrivateHome { path, _lock: lock })
    }
}

impl Drop for PrivateHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        let _ = std::fs::remove_file(lock_path(&self.path));
    }
}

/// Remove the private `HOME` directories under `base` that a gateway left
/// behind (it was killed, or crashed): every `trustgraph-upstream-*`
/// directory, not a link, owned by this user, whose lock file exists and is
/// not locked by a live gateway. A directory without a lock file is kept:
/// nothing proves no one uses it. Returns how many were removed.
pub fn remove_stale_homes(base: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(base) else {
        return 0;
    };
    // This process's user: the owner of a file it creates.
    #[cfg(unix)]
    let me = {
        use std::os::unix::fs::MetadataExt;
        let probe = base.join(format!(
            "trustgraph-owner-probe-{}",
            &crate::serve::auth::random_secret()[..16]
        ));
        let uid = crate::serve::auth::private_create(&probe)
            .and_then(|f| f.metadata())
            .map(|m| m.uid())
            .ok();
        let _ = std::fs::remove_file(&probe);
        match uid {
            Some(uid) => Some(uid),
            None => return 0,
        }
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_str().is_some_and(|n| n.starts_with(HOME_PREFIX)) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if Some(metadata.uid()) != me {
                continue;
            }
        }
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let Ok(lock) = options.open(lock_path(&path)) else {
            continue;
        };
        if !lock.metadata().is_ok_and(|m| m.is_file()) || lock.try_lock().is_err() {
            continue;
        }
        // Held while removing, so no gateway can take it meanwhile.
        if std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
            let _ = std::fs::remove_file(lock_path(&path));
        }
        drop(lock);
    }
    removed
}

/// The program to run, found on the gateway's `PATH` when it is a bare
/// name, and the minimal `PATH` the child gets: the program's own
/// directory, then the system directories.
fn program_and_path(program: &str) -> (PathBuf, String) {
    let mut found = PathBuf::from(program);
    if !program.contains('/')
        && let Some(paths) = std::env::var_os("PATH")
        && let Some(hit) = std::env::split_paths(&paths)
            .map(|dir| dir.join(program))
            .find(|p| p.is_file())
    {
        found = hit;
    }
    let mut dirs: Vec<String> = Vec::new();
    if let Some(dir) = found.parent().and_then(Path::to_str)
        && !dir.is_empty()
    {
        let dir = std::fs::canonicalize(dir)
            .ok()
            .and_then(|d| d.to_str().map(str::to_string))
            .unwrap_or_else(|| dir.to_string());
        dirs.push(dir);
    }
    for system in ["/usr/bin", "/bin"] {
        if !dirs.iter().any(|d| d == system) {
            dirs.push(system.to_string());
        }
    }
    (found, dirs.join(":"))
}

struct Connection {
    service: RunningService<RoleClient, ()>,
    tools: Vec<Tool>,
    /// Dropped after the service, which kills the child.
    _home: Option<PrivateHome>,
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
    for name in super::config::env_names(config) {
        if let Some(value) = lookup(&name)
            && value.chars().count() < MIN_SECRET_CHARS
        {
            problems.push(format!(
                "the environment variable `{name}` is shorter than {MIN_SECRET_CHARS} characters; every `${{env:…}}` value is treated as a secret and redacted by its text, which is meaningless for a short one"
            ));
        }
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

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Arc<UpstreamConn>)> {
        self.by_name.iter()
    }

    /// Close every connection: stop each stdio child and remove its private
    /// `HOME`. A later call connects again.
    pub async fn shutdown(&self) {
        for conn in self.by_name.values() {
            conn.reset().await;
        }
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
        let mut home = None;
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
                // Never the gateway's own HOME (its state directory and
                // the operator's files live under it) or its full PATH.
                let private = PrivateHome::create(&self.name).map_err(|e| {
                    self.log("cannot create a private HOME", &e);
                    CallError::Unreachable
                })?;
                let (program, path) = program_and_path(&command[0]);
                let mut cmd = tokio::process::Command::new(program);
                cmd.args(&command[1..])
                    .env_clear()
                    .env("PATH", path)
                    .env("HOME", &private.path);
                home = Some(private);
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
        Ok(Connection {
            service,
            tools,
            _home: home,
        })
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

    /// The upstream's tools, connecting first when needed. Descriptions,
    /// titles and schemas are passed through the redactor: they are
    /// upstream text.
    pub async fn tools(&self) -> Result<Vec<Tool>, CallError> {
        let mut tools = self.peer().await?.1;
        for tool in &mut tools {
            if let Some(d) = &tool.description {
                tool.description = Some(self.redactor.redact(d).into());
            }
            if let Some(t) = &tool.title {
                tool.title = Some(self.redactor.redact(t));
            }
            let schema = serde_json::to_vec(&*tool.input_schema).unwrap_or_default();
            if self.redactor.leaks(&schema) {
                let text = self.redactor.redact(&String::from_utf8_lossy(&schema));
                if let Ok(clean) = serde_json::from_str::<JsonObject>(&text) {
                    tool.input_schema = Arc::new(clean);
                }
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
                return Err(CallError::Withheld);
            }
        };
        let bytes = serde_json::to_vec(&result).map_err(|e| {
            self.log(&format!("`{tool}`"), &e);
            CallError::Withheld
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
            return Err(CallError::Withheld);
        }
        if self.redactor.leaks(&bytes) {
            self.log(
                &format!("`{tool}`"),
                &"the result holds an upstream credential; it is withheld",
            );
            return Err(CallError::Withheld);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_redactor_matches_a_secret_and_its_json_escaped_form() {
        let secret = r#"pa"ss\word-0123456"#.to_string();
        let redactor = Redactor::new([secret.clone()]);
        let json = serde_json::to_vec(&serde_json::json!({"text": secret})).unwrap();
        assert!(redactor.leaks(&json), "{}", String::from_utf8_lossy(&json));
        assert!(redactor.leaks(secret.as_bytes()));
        assert_eq!(redactor.redact(&format!("x {secret} y")), "x [redacted] y");
        assert!(!redactor.leaks(b"nothing here"));
    }

    #[test]
    fn stale_homes_are_removed_and_live_ones_kept() {
        let base = tempfile::tempdir().unwrap();
        let live = PrivateHome::create_in(base.path(), "slack").unwrap();
        assert!(lock_path(&live.path).is_file());
        assert_eq!(std::fs::read_dir(&live.path).unwrap().count(), 0, "empty");
        // A gateway that was killed: its lock file is no longer locked.
        let stale = base.path().join(format!("{HOME_PREFIX}old-0000"));
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(lock_path(&stale), "").unwrap();
        std::fs::write(stale.join("leftover"), "x").unwrap();
        // No lock file: nothing proves it unused, so it is kept.
        let unknown = base.path().join(format!("{HOME_PREFIX}legacy-1111"));
        std::fs::create_dir(&unknown).unwrap();
        // Not ours by name, and a link by the name: kept.
        let other = base.path().join("other-dir");
        std::fs::create_dir(&other).unwrap();
        std::fs::write(lock_path(&other), "").unwrap();
        #[cfg(unix)]
        {
            let link = base.path().join(format!("{HOME_PREFIX}link"));
            std::os::unix::fs::symlink(&other, &link).unwrap();
            std::fs::write(lock_path(&link), "").unwrap();
        }

        assert_eq!(remove_stale_homes(base.path()), 1);
        assert!(!stale.exists() && !lock_path(&stale).exists());
        assert!(live.path.exists(), "a live gateway's HOME is kept");
        assert!(unknown.exists());
        assert!(other.exists());
        let live_path = live.path.clone();
        drop(live);
        assert!(!live_path.exists(), "removed when the connection ends");
        assert!(!lock_path(&live_path).exists());
    }

    #[test]
    fn a_child_gets_its_program_directory_and_the_system_ones_on_path() {
        let (program, path) = program_and_path("/opt/tools/bin/server");
        assert_eq!(program, PathBuf::from("/opt/tools/bin/server"));
        assert_eq!(path, "/opt/tools/bin:/usr/bin:/bin");
    }
}
