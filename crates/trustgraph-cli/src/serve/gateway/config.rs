//! `gateway.yaml`: which upstream MCP servers the gateway reaches, and what
//! each exposed tool is to the policy (a read of a source, an action, or a
//! passthrough).
//!
//! Strict: unknown keys are errors, and every problem is reported at once
//! with its path, like the policy parser. `${env:NAME}` is the only
//! interpolation and only in upstream `headers` and `env` values; resolved
//! values are [`Secret`]s, which never print.
//!
//! Every tool has an argument allow-list (`arguments`); a call carrying any
//! other argument is refused, so what is checked is exactly what is
//! forwarded. Matching is fail-closed by construction (see
//! [`safe_for_matching`] and [`safe_path_for_matching`]): an argument that
//! is not a plain string (or a list of them), or holds a `..` segment or a
//! control character, never matches a `when` rule, so the call falls to
//! `otherwise`.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde_yaml::{Mapping, Value};
use trustgraph_policy::Policy;

/// Default cap on one upstream tool result, serialized, in bytes.
pub const DEFAULT_MAX_RESULT_BYTES: usize = 1 << 20;
/// Default timeout of every upstream call.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Longest argument string a `when` glob is matched against.
pub const MAX_MATCH_BYTES: usize = 4096;
/// Longest `when` glob.
pub const MAX_GLOB_BYTES: usize = 512;
/// Largest accepted config file.
pub const MAX_CONFIG_BYTES: usize = 256 * 1024;
/// The prefix of the gateway's own tools; no upstream may use it.
pub const BUILTIN: &str = "trustgraph";

const TOP_KEYS: &[&str] = &[
    "version",
    "session",
    "upstreams",
    "tools",
    "max_result_bytes",
    "timeout_seconds",
];
const HTTP_KEYS: &[&str] = &["url", "headers"];
const STDIO_KEYS: &[&str] = &["command", "env"];
const READS_KEYS: &[&str] = &["reads", "otherwise", "arguments"];
const ACTS_KEYS: &[&str] = &["acts", "otherwise", "allow_without_reads", "arguments"];
const PASSTHROUGH_KEYS: &[&str] = &["passthrough", "untracked", "arguments"];
const MATCHER_KEYS: &[&str] = &["glob", "type"];
/// Longest argument name.
pub const MAX_ARGUMENT_NAME: usize = 128;
/// Shortest `${env:…}` value accepted: redacting a shorter one by
/// substring would be meaningless.
pub const MIN_SECRET_CHARS: usize = 12;
const READ_RULE_KEYS: &[&str] = &["when", "source"];
const ACT_RULE_KEYS: &[&str] = &["when", "action", "content"];

/// A value resolved from the environment. It never prints.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[redacted]")
    }
}

/// A header or environment value: literal text and `${env:NAME}` parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Env(String),
}

impl Template {
    /// The environment variables it names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().filter_map(|p| match p {
            Part::Env(name) => Some(name.as_str()),
            Part::Text(_) => None,
        })
    }

    /// Resolve with `lookup`; the first missing name on failure.
    pub fn resolve(&self, lookup: &dyn Fn(&str) -> Option<String>) -> Result<Secret, String> {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Env(name) => out.push_str(&lookup(name).ok_or_else(|| name.clone())?),
            }
        }
        Ok(Secret(out))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionMode {
    /// One session per MCP connection (`Mcp-Session-Id`).
    #[default]
    Connection,
    /// One session per agent token for the gateway's lifetime.
    Agent,
}

impl SessionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionMode::Connection => "connection",
            SessionMode::Agent => "agent",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Upstream {
    Http {
        url: String,
        headers: Vec<(String, Template)>,
    },
    Stdio {
        command: Vec<String>,
        env: Vec<(String, Template)>,
    },
}

/// One argument glob of a `when` rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matcher {
    pub argument: String,
    pub glob: String,
    /// `type: path` (the default in `reads` rules): odd path forms never match.
    pub path: bool,
}

#[derive(Debug, Clone)]
pub struct ReadRule {
    pub when: Vec<Matcher>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct ActRule {
    pub when: Vec<Matcher>,
    pub action: String,
    /// The argument shown to the operator as the content: a label only. The
    /// checked output is always the canonical JSON of every forwarded argument.
    pub content: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Kind {
    Reads {
        rules: Vec<ReadRule>,
        otherwise: String,
    },
    Acts {
        rules: Vec<ActRule>,
        allow_without_reads: bool,
    },
    Passthrough {
        untracked: bool,
    },
}

#[derive(Debug, Clone)]
pub struct Tool {
    /// `upstream.tool`, as in the config.
    pub key: String,
    pub upstream: String,
    /// The tool's name on the upstream.
    pub name: String,
    /// `upstream__tool`, as agents see it.
    pub exposed: String,
    pub kind: Kind,
    /// The only argument names a call may carry.
    pub arguments: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub session: SessionMode,
    pub upstreams: BTreeMap<String, Upstream>,
    /// By exposed name.
    pub tools: BTreeMap<String, Tool>,
    pub max_result_bytes: usize,
    pub timeout: Duration,
}

/// One problem, with where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

#[derive(Default)]
struct Cx {
    problems: Vec<Problem>,
}

impl Cx {
    fn error(&mut self, path: &str, message: impl Into<String>) {
        self.problems.push(Problem {
            path: path.to_string(),
            message: message.into(),
        });
    }

    fn map<'a>(&mut self, v: &'a Value, path: &str) -> Option<&'a Mapping> {
        match v {
            Value::Mapping(m) => Some(m),
            other => {
                self.error(
                    path,
                    format!("expected a mapping, found {}", type_name(other)),
                );
                None
            }
        }
    }

    /// String-keyed entries; keys outside `allowed` are errors.
    fn entries<'a>(
        &mut self,
        m: &'a Mapping,
        path: &str,
        allowed: Option<&[&str]>,
    ) -> BTreeMap<String, &'a Value> {
        let mut out = BTreeMap::new();
        for (k, v) in m {
            let Value::String(k) = k else {
                self.error(
                    path,
                    format!("expected a string key, found {}", type_name(k)),
                );
                continue;
            };
            match allowed {
                Some(allowed) if !allowed.contains(&k.as_str()) => self.error(
                    &join(path, k),
                    format!("unknown key `{k}`; allowed: {}", allowed.join(", ")),
                ),
                _ => {
                    out.insert(k.clone(), v);
                }
            }
        }
        out
    }

    fn string(&mut self, v: &Value, path: &str) -> Option<String> {
        match v {
            Value::String(s) if !s.is_empty() && !s.chars().any(char::is_control) => {
                Some(s.clone())
            }
            Value::String(_) => {
                self.error(path, "must be non-empty text without control characters");
                None
            }
            other => {
                self.error(
                    path,
                    format!("expected a string, found {}", type_name(other)),
                );
                None
            }
        }
    }

    fn bool(&mut self, v: &Value, path: &str) -> Option<bool> {
        match v {
            Value::Bool(b) => Some(*b),
            other => {
                self.error(
                    path,
                    format!("expected true or false, found {}", type_name(other)),
                );
                None
            }
        }
    }

    fn template(&mut self, v: &Value, path: &str) -> Option<Template> {
        let text = self.string(v, path)?;
        match parse_template(&text) {
            Ok(t) => Some(t),
            Err(message) => {
                self.error(path, message);
                None
            }
        }
    }

    fn templates(&mut self, v: &Value, path: &str) -> Vec<(String, Template)> {
        let Some(m) = self.map(v, path) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (name, value) in self.entries(m, path, None) {
            let here = join(path, &name);
            if name.is_empty() || name.chars().any(|c| c.is_control() || c == '=') {
                self.error(&here, "not a valid name");
                continue;
            }
            if let Some(t) = self.template(value, &here) {
                out.push((name, t));
            }
        }
        out
    }

    /// A `when` mapping: `argument: glob`, or `argument: {glob, type}` with
    /// `type` `path` or `text` (`default_path` when not given).
    fn when(&mut self, v: Option<&&Value>, path: &str, default_path: bool) -> Vec<Matcher> {
        let Some(v) = v else {
            return Vec::new();
        };
        let path = join(path, "when");
        let Some(m) = self.map(v, &path) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (argument, value) in self.entries(m, &path, None) {
            let here = join(&path, &argument);
            if !valid_argument_name(&argument) {
                self.error(&here, "not a valid argument name");
                continue;
            }
            let (glob, is_path) = match value {
                Value::Mapping(spec) => {
                    let keys = self.entries(spec, &here, Some(MATCHER_KEYS));
                    let is_path = match keys.get("type") {
                        None => default_path,
                        Some(Value::String(t)) if t == "path" => true,
                        Some(Value::String(t)) if t == "text" => false,
                        Some(_) => {
                            self.error(&join(&here, "type"), "must be `path` or `text`");
                            continue;
                        }
                    };
                    let Some(glob) = keys.get("glob") else {
                        self.error(&join(&here, "glob"), "required: the glob to match");
                        continue;
                    };
                    (self.string(glob, &join(&here, "glob")), is_path)
                }
                other => (self.string(other, &here), default_path),
            };
            match glob {
                Some(glob) if glob.len() > MAX_GLOB_BYTES => {
                    self.error(&here, format!("a glob is at most {MAX_GLOB_BYTES} bytes"))
                }
                Some(glob) => out.push(Matcher {
                    argument,
                    glob,
                    path: is_path,
                }),
                None => {}
            }
        }
        out
    }

    /// An `arguments:` list of names, no two equal ignoring case; `None`
    /// when the key is absent.
    fn arguments(&mut self, v: Option<&&Value>, path: &str) -> Option<BTreeSet<String>> {
        let v = v?;
        let path = join(path, "arguments");
        let Value::Sequence(items) = v else {
            self.error(&path, "expected a list of argument names");
            return Some(BTreeSet::new());
        };
        let mut out = BTreeSet::new();
        let mut folded = BTreeSet::new();
        for (i, item) in items.iter().enumerate() {
            let here = format!("{path}[{i}]");
            let Some(name) = self.string(item, &here) else {
                continue;
            };
            if !valid_argument_name(&name) {
                self.error(&here, "not a valid argument name");
            } else if !folded.insert(name.to_lowercase()) {
                self.error(
                    &here,
                    format!("`{name}` is listed twice (names are compared ignoring case)"),
                );
            } else {
                out.insert(name);
            }
        }
        Some(out)
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "nothing",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Sequence(_) => "a list",
        Value::Mapping(_) => "a mapping",
        Value::Tagged(_) => "a tagged value",
    }
}

fn parse_template(text: &str) -> Result<Template, String> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        if start > 0 {
            parts.push(Part::Text(rest[..start].to_string()));
        }
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or("an unclosed `${`; write `${env:NAME}`")?;
        let inner = &after[..end];
        let name = inner
            .strip_prefix("env:")
            .ok_or("only `${env:NAME}` is interpolated")?;
        let valid = name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(format!("`{name}` is not an environment variable name"));
        }
        parts.push(Part::Env(name.to_string()));
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest.to_string()));
    }
    Ok(Template { parts })
}

/// Upstream names: lowercase letters, digits, `-` and single `_`, starting
/// with a letter, so `upstream__tool` splits one way only.
fn valid_upstream(name: &str) -> bool {
    name.len() <= 32
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && !name.contains("__")
        && !name.ends_with('_')
}

/// An argument name: non-empty text without control characters.
fn valid_argument_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_ARGUMENT_NAME && !name.chars().any(char::is_control)
}

/// The `user:password@host:port` part of a URL.
fn url_authority(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or_default()
}

/// True for an `http://` URL whose host is not loopback: what it carries
/// (credentials included) travels in clear.
pub fn cleartext_remote(url: &str) -> bool {
    if !url.starts_with("http://") {
        return false;
    }
    let authority = url_authority(url);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|ip| ip.is_loopback());
    !loopback
}

/// What an MCP client accepts in a tool name.
fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
}

/// Parse `gateway.yaml`. Every problem is returned, with its path.
pub fn parse(text: &str) -> Result<Config, Vec<Problem>> {
    let mut cx = Cx::default();
    if text.len() > MAX_CONFIG_BYTES {
        cx.error(
            "",
            format!("the file is larger than {MAX_CONFIG_BYTES} bytes"),
        );
        return Err(cx.problems);
    }
    let root: Value = match serde_yaml::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            cx.error("", format!("not valid YAML: {e}"));
            return Err(cx.problems);
        }
    };
    let Some(root) = cx.map(&root, "") else {
        return Err(cx.problems);
    };
    let top = cx.entries(root, "", Some(TOP_KEYS));
    match top.get("version") {
        Some(Value::Number(n)) if n.as_u64() == Some(1) => {}
        Some(_) => cx.error("version", "must be 1"),
        None => cx.error("version", "required; write `version: 1`"),
    }
    let session = match top.get("session") {
        None => SessionMode::Connection,
        Some(Value::String(s)) if s == "connection" => SessionMode::Connection,
        Some(Value::String(s)) if s == "agent" => SessionMode::Agent,
        Some(_) => {
            cx.error("session", "must be `connection` or `agent`");
            SessionMode::Connection
        }
    };
    let max_result_bytes = match top.get("max_result_bytes") {
        None => DEFAULT_MAX_RESULT_BYTES,
        Some(Value::Number(n)) => match n.as_u64() {
            Some(n) if (1024..=64 << 20).contains(&n) => n as usize,
            _ => {
                cx.error("max_result_bytes", "must be 1024 to 67108864");
                DEFAULT_MAX_RESULT_BYTES
            }
        },
        Some(v) => {
            cx.error(
                "max_result_bytes",
                format!("expected a number, found {}", type_name(v)),
            );
            DEFAULT_MAX_RESULT_BYTES
        }
    };
    let timeout = match top.get("timeout_seconds") {
        None => DEFAULT_TIMEOUT,
        Some(Value::Number(n)) => match n.as_u64() {
            Some(n) if (1..=600).contains(&n) => Duration::from_secs(n),
            _ => {
                cx.error("timeout_seconds", "must be 1 to 600");
                DEFAULT_TIMEOUT
            }
        },
        Some(v) => {
            cx.error(
                "timeout_seconds",
                format!("expected a number, found {}", type_name(v)),
            );
            DEFAULT_TIMEOUT
        }
    };

    let mut upstreams = BTreeMap::new();
    match top.get("upstreams") {
        None => cx.error("upstreams", "required: at least one upstream MCP server"),
        Some(v) => {
            if let Some(m) = cx.map(v, "upstreams") {
                let entries = cx.entries(m, "upstreams", None);
                if entries.is_empty() {
                    cx.error("upstreams", "list at least one upstream MCP server");
                }
                for (name, value) in entries {
                    let path = join("upstreams", &name);
                    if !valid_upstream(&name) || name == BUILTIN {
                        cx.error(
                            &path,
                            "an upstream name is lowercase letters, digits, `-` and single `_`, starts with a letter, and is not `trustgraph`",
                        );
                        continue;
                    }
                    if let Some(u) = upstream(&mut cx, value, &path) {
                        upstreams.insert(name, u);
                    }
                }
            }
        }
    }

    let mut tools = BTreeMap::new();
    match top.get("tools") {
        None => cx.error("tools", "required: list the tools agents may call"),
        Some(v) => {
            if let Some(m) = cx.map(v, "tools") {
                for (key, value) in cx.entries(m, "tools", None) {
                    let path = join("tools", &key);
                    let Some((up, name)) = key.split_once('.') else {
                        cx.error(&path, "name a tool as `upstream.tool`");
                        continue;
                    };
                    if !upstreams.contains_key(up) {
                        if valid_upstream(up) {
                            cx.error(&path, format!("no upstream named `{up}`"));
                        } else {
                            cx.error(&path, "name a tool as `upstream.tool`");
                        }
                        continue;
                    }
                    let exposed = format!("{up}__{name}");
                    if !valid_tool_name(name) || !valid_tool_name(&exposed) {
                        cx.error(
                            &path,
                            "a tool name is letters, digits, `_`, `-` and `.`, at most 128 characters with its upstream",
                        );
                        continue;
                    }
                    let Some((kind, arguments)) = tool_kind(&mut cx, value, &path, session) else {
                        continue;
                    };
                    tools.insert(
                        exposed.clone(),
                        Tool {
                            key: key.clone(),
                            upstream: up.to_string(),
                            name: name.to_string(),
                            exposed,
                            kind,
                            arguments,
                        },
                    );
                }
            }
        }
    }
    if cx.problems.is_empty() {
        Ok(Config {
            session,
            upstreams,
            tools,
            max_result_bytes,
            timeout,
        })
    } else {
        Err(cx.problems)
    }
}

fn upstream(cx: &mut Cx, value: &Value, path: &str) -> Option<Upstream> {
    let m = cx.map(value, path)?;
    let keys = cx.entries(m, path, None);
    if keys.contains_key("url") == keys.contains_key("command") {
        cx.error(
            path,
            "give exactly one of `url` (streamable HTTP) or `command` (stdio)",
        );
        return None;
    }
    if keys.contains_key("url") {
        let keys = cx.entries(m, path, Some(HTTP_KEYS));
        let url_path = join(path, "url");
        let url = cx.string(keys.get("url")?, &url_path)?;
        if url.contains("${") {
            cx.error(
                &url_path,
                "`${env:…}` is only interpolated in `headers`; keep secrets out of the URL",
            );
            return None;
        }
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            cx.error(&url_path, "must start with https:// or http://");
            return None;
        }
        if url_authority(&url).contains('@') {
            cx.error(
                &url_path,
                "must not hold a user name or password; put credentials in `headers`",
            );
            return None;
        }
        let headers = keys
            .get("headers")
            .map(|v| cx.templates(v, &join(path, "headers")))
            .unwrap_or_default();
        Some(Upstream::Http { url, headers })
    } else {
        let keys = cx.entries(m, path, Some(STDIO_KEYS));
        let command_path = join(path, "command");
        let Some(Value::Sequence(items)) = keys.get("command") else {
            cx.error(
                &command_path,
                "expected a list: the program and its arguments",
            );
            return None;
        };
        let mut command = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let here = format!("{command_path}[{i}]");
            match cx.string(item, &here) {
                Some(s) if s.contains("${") => cx.error(
                    &here,
                    "`${env:…}` is only interpolated in `env`; pass secrets as environment variables",
                ),
                Some(s) => command.push(s),
                None => {}
            }
        }
        if items.is_empty() {
            cx.error(&command_path, "name the program to run");
        }
        let env = keys
            .get("env")
            .map(|v| cx.templates(v, &join(path, "env")))
            .unwrap_or_default();
        (command.len() == items.len() && !command.is_empty())
            .then_some(Upstream::Stdio { command, env })
    }
}

fn tool_kind(
    cx: &mut Cx,
    value: &Value,
    path: &str,
    session: SessionMode,
) -> Option<(Kind, BTreeSet<String>)> {
    let m = cx.map(value, path)?;
    let keys = cx.entries(m, path, None);
    let kinds = ["reads", "acts", "passthrough"]
        .iter()
        .filter(|k| keys.contains_key(**k))
        .count();
    if kinds != 1 {
        cx.error(
            path,
            "give exactly one of `reads`, `acts` or `passthrough: true`",
        );
        return None;
    }
    if keys.contains_key("reads") {
        let keys = cx.entries(m, path, Some(READS_KEYS));
        let rules_path = join(path, "reads");
        let Some(Value::Sequence(items)) = keys.get("reads") else {
            cx.error(&rules_path, "expected a list of `{when, source}` rules");
            return None;
        };
        let mut rules = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let here = format!("{rules_path}[{i}]");
            let Some(rm) = cx.map(item, &here) else {
                continue;
            };
            let rk = cx.entries(rm, &here, Some(READ_RULE_KEYS));
            let when = cx.when(rk.get("when"), &here, true);
            let Some(source) = rk.get("source") else {
                cx.error(&join(&here, "source"), "required: the source this read is");
                continue;
            };
            if let Some(source) = cx.string(source, &join(&here, "source")) {
                rules.push(ReadRule { when, source });
            }
        }
        let otherwise_path = join(path, "otherwise");
        let Some(otherwise) = keys.get("otherwise") else {
            cx.error(
                &otherwise_path,
                "required for a reads tool: the source a call that matches no rule is (make it a restrictive one)",
            );
            return None;
        };
        let otherwise = cx.string(otherwise, &otherwise_path)?;
        // By default the arguments the rules name; a list may add more.
        let mut arguments = cx
            .arguments(keys.get("arguments"), path)
            .unwrap_or_default();
        for m in rules.iter().flat_map(|r| &r.when) {
            let clash = arguments
                .iter()
                .any(|a| a != &m.argument && a.to_lowercase() == m.argument.to_lowercase());
            if clash {
                cx.error(
                    &join(path, "arguments"),
                    format!(
                        "`{}` differs only in case from a listed argument",
                        m.argument
                    ),
                );
            }
            arguments.insert(m.argument.clone());
        }
        Some((Kind::Reads { rules, otherwise }, arguments))
    } else if keys.contains_key("acts") {
        let keys = cx.entries(m, path, Some(ACTS_KEYS));
        let rules_path = join(path, "acts");
        let Some(Value::Sequence(items)) = keys.get("acts") else {
            cx.error(
                &rules_path,
                "expected a list of `{when, action, content}` rules",
            );
            return None;
        };
        let mut rules = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let here = format!("{rules_path}[{i}]");
            let Some(rm) = cx.map(item, &here) else {
                continue;
            };
            let rk = cx.entries(rm, &here, Some(ACT_RULE_KEYS));
            let when = cx.when(rk.get("when"), &here, false);
            let content = rk
                .get("content")
                .and_then(|v| cx.string(v, &join(&here, "content")));
            let Some(action) = rk.get("action") else {
                cx.error(
                    &join(&here, "action"),
                    "required: the policy action this call is",
                );
                continue;
            };
            if let Some(action) = cx.string(action, &join(&here, "action")) {
                rules.push(ActRule {
                    when,
                    action,
                    content,
                });
            }
        }
        if items.is_empty() {
            cx.error(&rules_path, "list at least one rule");
        }
        match keys.get("otherwise") {
            None => {}
            Some(Value::String(s)) if s == "block" => {}
            Some(_) => cx.error(
                &join(path, "otherwise"),
                "must be `block`: an action call that matches no rule is never forwarded",
            ),
        }
        let allow_without_reads = match keys.get("allow_without_reads") {
            None => false,
            Some(v) => cx.bool(v, &join(path, "allow_without_reads"))?,
        };
        if allow_without_reads && session != SessionMode::Agent {
            cx.error(
                &join(path, "allow_without_reads"),
                "requires `session: agent`: in connection mode a new connection would be a way to shed what was read",
            );
        }
        let Some(arguments) = cx.arguments(keys.get("arguments"), path) else {
            cx.error(
                &join(path, "arguments"),
                "required for an acts tool: list every argument a call may carry, e.g. `arguments: [channel, text]`; a call with any other argument is refused",
            );
            return None;
        };
        for (i, rule) in rules.iter().enumerate() {
            let here = format!("{rules_path}[{i}]");
            for m in &rule.when {
                if !arguments.contains(&m.argument) {
                    cx.error(
                        &join(&join(&here, "when"), &m.argument),
                        format!("`{}` is not in `arguments`", m.argument),
                    );
                }
            }
            if let Some(content) = &rule.content
                && !arguments.contains(content)
            {
                cx.error(
                    &join(&here, "content"),
                    format!("`{content}` is not in `arguments`"),
                );
            }
        }
        Some((
            Kind::Acts {
                rules,
                allow_without_reads,
            },
            arguments,
        ))
    } else {
        let keys = cx.entries(m, path, Some(PASSTHROUGH_KEYS));
        let here = join(path, "passthrough");
        if !cx.bool(keys.get("passthrough")?, &here)? {
            cx.error(&here, "write `passthrough: true`, or leave the tool out");
            return None;
        }
        let untracked = match keys.get("untracked") {
            None => false,
            Some(v) => cx.bool(v, &join(path, "untracked"))?,
        };
        let arguments = cx
            .arguments(keys.get("arguments"), path)
            .unwrap_or_default();
        Some((Kind::Passthrough { untracked }, arguments))
    }
}

/// The source name passthrough results are registered as.
pub const UNCLASSIFIED: &str = "unclassified";

/// Problems of `config` against `policy`: every source and action it names
/// must exist, and a tracked passthrough needs an `unclassified` source.
pub fn check_against(config: &Config, policy: &Policy) -> Vec<Problem> {
    let mut problems = Vec::new();
    for tool in config.tools.values() {
        problems.extend(tool_problems(tool, policy));
    }
    problems
}

/// Problems of one tool against `policy`; a tool with any fails closed.
pub fn tool_problems(tool: &Tool, policy: &Policy) -> Vec<Problem> {
    let mut problems = Vec::new();
    let path = join("tools", &tool.key);
    let mut source = |name: &str, path: String| {
        if !policy.sources.contains_key(name) {
            problems.push(Problem {
                path,
                message: format!("`{name}` is not a source in the active policy"),
            });
        }
    };
    match &tool.kind {
        Kind::Reads { rules, otherwise } => {
            for (i, rule) in rules.iter().enumerate() {
                source(&rule.source, format!("{path}.reads[{i}].source"));
            }
            source(otherwise, join(&path, "otherwise"));
            problems.extend(otherwise_problems(&path, rules, otherwise, policy));
        }
        Kind::Acts { rules, .. } => {
            for (i, rule) in rules.iter().enumerate() {
                if !policy.actions.contains_key(&rule.action) {
                    problems.push(Problem {
                        path: format!("{path}.acts[{i}].action"),
                        message: format!("`{}` is not an action in the active policy", rule.action),
                    });
                }
            }
        }
        Kind::Passthrough { untracked } => {
            if !untracked && !policy.sources.contains_key(UNCLASSIFIED) {
                problems.push(Problem {
                    path: join(&path, "passthrough"),
                    message: "the policy has no `unclassified` source to register its results as; add one, or write `untracked: true` to forward results with no tracking".into(),
                });
            }
        }
    }
    problems
}

/// `otherwise` must be at least as restrictive as every rule's source:
/// classification trusts the upstream's own naming (a link or alias may
/// point elsewhere), so an unmatched call must never be freer than a
/// matched one. Its readers and its agents must each be a subset of every
/// rule source's.
fn otherwise_problems(
    path: &str,
    rules: &[ReadRule],
    otherwise: &str,
    policy: &Policy,
) -> Vec<Problem> {
    let mut problems = Vec::new();
    let Some(fallback) = policy.sources.get(otherwise) else {
        return problems;
    };
    let fallback_readers = policy.people(&fallback.readers);
    for rule in rules {
        let Some(source) = policy.sources.get(&rule.source) else {
            continue;
        };
        let extra_readers: Vec<String> = fallback_readers
            .difference(&policy.people(&source.readers))
            .cloned()
            .collect();
        let extra_agents: Vec<String> = fallback
            .agents
            .difference(&source.agents)
            .cloned()
            .collect();
        let mut why = Vec::new();
        if !extra_readers.is_empty() {
            why.push(format!("its readers include {}", extra_readers.join(", ")));
        }
        if !extra_agents.is_empty() {
            why.push(format!("its agents include {}", extra_agents.join(", ")));
        }
        if !why.is_empty() {
            problems.push(Problem {
                path: join(path, "otherwise"),
                message: format!(
                    "`{otherwise}` must be at least as restrictive as `{}`, but {}, who may not use `{}`; name the most restrictive source here",
                    rule.source,
                    why.join(" and "),
                    rule.source
                ),
            });
        }
    }
    problems
}

/// Warnings that do not stop the gateway: rules no call can reach, and
/// upstreams reached in clear over the network.
pub fn warnings(config: &Config) -> Vec<Problem> {
    let mut out = Vec::new();
    for (name, upstream) in &config.upstreams {
        if let Upstream::Http { url, .. } = upstream
            && cleartext_remote(url)
        {
            out.push(Problem {
                path: format!("upstreams.{name}.url"),
                message: "plain http:// to a host that is not loopback: its credentials and every call travel in clear; use https://".into(),
            });
        }
    }
    for tool in config.tools.values() {
        let path = join("tools", &tool.key);
        let (whens, key): (Vec<&[Matcher]>, &str) = match &tool.kind {
            Kind::Reads { rules, .. } => {
                (rules.iter().map(|r| r.when.as_slice()).collect(), "reads")
            }
            Kind::Acts { rules, .. } => (rules.iter().map(|r| r.when.as_slice()).collect(), "acts"),
            Kind::Passthrough { .. } => continue,
        };
        for (j, later) in whens.iter().enumerate() {
            // An earlier rule whose matchers are all in this one matches
            // every call this one would: this one is never reached.
            if let Some(i) = (0..j).find(|&i| whens[i].iter().all(|m| later.contains(m))) {
                out.push(Problem {
                    path: format!("{path}.{key}[{j}]"),
                    message: format!("never reached: `{key}[{i}]` matches every call it would"),
                });
            }
        }
    }
    out
}

/// Every environment variable the config names.
pub fn env_names(config: &Config) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for upstream in config.upstreams.values() {
        let templates = match upstream {
            Upstream::Http { headers, .. } => headers,
            Upstream::Stdio { env, .. } => env,
        };
        for (_, t) in templates {
            names.extend(t.names().map(str::to_string));
        }
    }
    names
}

/// True when `value` may be matched against a `when` glob: a string of at
/// most `MAX_MATCH_BYTES` bytes with no control character (NUL included)
/// and no `..` segment (split on `/` and `\`). Anything else never matches,
/// so it falls to `otherwise`: the upstream may normalise a path the
/// gateway sees as `Product/../HR/x` into one the rule did not mean.
pub fn safe_for_matching(value: &serde_json::Value) -> Option<&str> {
    let text = value.as_str()?;
    let safe = text.len() <= MAX_MATCH_BYTES
        && !text.chars().any(char::is_control)
        && !text.split(['/', '\\']).any(|segment| segment == "..");
    safe.then_some(text)
}

/// [`safe_for_matching`], and for an argument declared `type: path`: ASCII
/// only, no `%`, `~` or `\`, no leading `/` or `./`, and no empty, `.` or
/// `..` segment. An upstream may decode, expand or normalise any of these
/// into a path the rule did not mean, so they never match.
pub fn safe_path_for_matching(value: &serde_json::Value) -> Option<&str> {
    let text = safe_for_matching(value)?;
    let safe = text.is_ascii()
        && !text.contains(['%', '~', '\\'])
        && !text.starts_with('/')
        && !text
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..");
    safe.then_some(text)
}

/// Characters `**` never matches: list and header separators, whitespace,
/// markup and quotes, and control characters.
fn stops_globstar(c: char) -> bool {
    matches!(c, ',' | ';' | '<' | '>' | '"') || c.is_whitespace() || c.is_control()
}

/// Characters `*` and `?` never match: those, and `/`.
fn stops_star(c: char) -> bool {
    c == '/' || stops_globstar(c)
}

/// Glob match, case-sensitive: `*` matches any run and `?` one character,
/// but neither matches `/`, `,`, `;`, whitespace, `<`, `>`, `"` or a control
/// character; `**` is `*` that also crosses `/`. Every other character is
/// literal.
pub fn glob(pattern: &str, text: &str) -> bool {
    enum Tok {
        Lit(char),
        One,
        Star,
        Globstar,
    }
    let mut toks = Vec::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        toks.push(match c {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                Tok::Globstar
            }
            '*' => Tok::Star,
            '?' => Tok::One,
            c => Tok::Lit(c),
        });
    }
    let t: Vec<char> = text.chars().collect();
    let n = t.len();
    // next[j]: the tokens after this one match t[j..].
    let mut next = vec![false; n + 1];
    next[n] = true;
    for tok in toks.iter().rev() {
        let mut here = vec![false; n + 1];
        for j in (0..=n).rev() {
            here[j] = match tok {
                Tok::Lit(c) => j < n && t[j] == *c && next[j + 1],
                Tok::One => j < n && !stops_star(t[j]) && next[j + 1],
                Tok::Star => next[j] || (j < n && !stops_star(t[j]) && here[j + 1]),
                Tok::Globstar => next[j] || (j < n && !stops_globstar(t[j]) && here[j + 1]),
            };
        }
        next = here;
    }
    next[0]
}

/// One matcher against one argument value: a safe string that matches, or
/// a non-empty list whose every element is one. Objects, numbers and
/// anything else never match.
pub fn value_matches(m: &Matcher, value: &serde_json::Value) -> bool {
    let one = |v: &serde_json::Value| {
        let safe = if m.path {
            safe_path_for_matching(v)
        } else {
            safe_for_matching(v)
        };
        safe.is_some_and(|text| glob(&m.glob, text))
    };
    match value {
        serde_json::Value::String(_) => one(value),
        serde_json::Value::Array(items) => !items.is_empty() && items.iter().all(one),
        _ => false,
    }
}

/// The rule matchers of one call: every named argument must be present and
/// match its glob.
pub fn when_matches(
    when: &[Matcher],
    arguments: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    when.iter().all(|m| {
        arguments
            .get(&m.argument)
            .is_some_and(|value| value_matches(m, value))
    })
}

/// An argument name as it may appear in an error: control characters
/// escaped, at most 64 characters.
fn shown_name(name: &str) -> String {
    let mut out: String = name.chars().take(64).flat_map(char::escape_debug).collect();
    if name.chars().count() > 64 {
        out.push('…');
    }
    out
}

/// A call's arguments against the tool's allow-list: every name listed,
/// and no two equal ignoring case. The error names the offending argument.
pub fn check_arguments(
    allowed: &BTreeSet<String>,
    arguments: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    for name in arguments.keys() {
        if !allowed.contains(name) {
            let listed: Vec<&str> = allowed.iter().map(String::as_str).collect();
            return Err(format!(
                "`{}` is not an argument the gateway forwards for this tool (it forwards: {})",
                shown_name(name),
                if listed.is_empty() {
                    "none".to_string()
                } else {
                    listed.join(", ")
                }
            ));
        }
    }
    let mut folded = BTreeSet::new();
    for name in arguments.keys() {
        if !folded.insert(name.to_lowercase()) {
            return Err(format!(
                "two arguments are named `{}` ignoring case",
                shown_name(name)
            ));
        }
    }
    Ok(())
}
