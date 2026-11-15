use std::collections::{BTreeMap, BTreeSet};

use serde_yaml::{Mapping, Value};

use crate::model::{
    Action, ApprovalRule, Case, Diagnostic, Parsed, Policy, Severity, Source, Verdict,
};

/// Largest accepted policy file, checked before any parsing.
pub const MAX_BYTES: usize = 262_144;
const MAX_VALID_FOR_SECONDS: u64 = 30 * 24 * 60 * 60;

const TOP_KEYS: &[&str] = &["version", "groups", "agents", "sources", "actions", "tests"];
const SOURCE_KEYS: &[&str] = &["readers", "agents"];
const ACTION_KEYS: &[&str] = &["agents", "audience", "approval"];
const APPROVAL_KEYS: &[&str] = &["reviewers", "valid_for"];
const CASE_KEYS: &[&str] = &[
    "name",
    "output_from",
    "agent",
    "action",
    "recipients",
    "expect",
];

/// Parse and validate a policy file, collecting every problem in one pass.
pub fn parse(yaml: &str) -> Parsed {
    let mut cx = Cx::default();
    if yaml.len() > MAX_BYTES {
        cx.error(
            "",
            "too_large",
            format!(
                "policy file is {} bytes; the limit is {MAX_BYTES}",
                yaml.len()
            ),
        );
        return cx.finish(None);
    }
    let root: Value = match serde_yaml::from_str(yaml) {
        Ok(v) => v,
        Err(e) => {
            let message = e.to_string();
            let (code, path) = match duplicate_key(&message) {
                Some(key) => ("duplicate_key", key),
                None => ("yaml_syntax", String::new()),
            };
            cx.error(&path, code, message);
            return cx.finish(None);
        }
    };
    let policy = cx.policy(&root);
    cx.finish(policy)
}

/// serde_yaml reports repeated keys as `... duplicate entry with key "k" ...`.
fn duplicate_key(message: &str) -> Option<String> {
    let rest = message.split("duplicate entry with key ").nth(1)?;
    let rest = rest.strip_prefix('"')?;
    Some(rest.split('"').next()?.to_string())
}

#[derive(Default)]
struct Cx {
    diagnostics: Vec<Diagnostic>,
}

impl Cx {
    fn finish(self, policy: Option<Policy>) -> Parsed {
        let has_error = self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error);
        Parsed {
            policy: if has_error { None } else { policy },
            diagnostics: self.diagnostics,
        }
    }

    fn error(&mut self, path: &str, code: &'static str, message: String) {
        self.error_with(path, code, message, None);
    }

    fn error_with(
        &mut self,
        path: &str,
        code: &'static str,
        message: String,
        suggestion: Option<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            severity: Severity::Error,
            path: path.to_string(),
            code,
            message,
            suggestion,
        });
    }

    fn wrong_type(&mut self, path: &str, expected: &str, found: &Value) {
        self.error(
            path,
            "wrong_type",
            format!("expected {expected}, found {}", type_name(found)),
        );
    }

    fn missing(&mut self, path: &str, key: &str) {
        self.error(
            &join(path, key),
            "missing_field",
            format!("required field `{key}` is missing"),
        );
    }

    /// Return the mapping, reporting `wrong_type` otherwise.
    fn map<'a>(&mut self, v: &'a Value, path: &str) -> Option<&'a Mapping> {
        match v {
            Value::Mapping(m) => Some(m),
            other => {
                self.wrong_type(path, "a mapping", other);
                None
            }
        }
    }

    /// Entries of a mapping whose keys must be strings.
    fn entries<'a>(&mut self, m: &'a Mapping, path: &str) -> Vec<(&'a str, &'a Value)> {
        let mut out = Vec::new();
        for (k, v) in m {
            match k {
                Value::String(s) => out.push((s.as_str(), v)),
                other => {
                    let shown = key_text(other);
                    self.error(
                        &join(path, &shown),
                        "wrong_type",
                        format!("expected a string key, found {}", type_name(other)),
                    );
                }
            }
        }
        out
    }

    /// Report keys outside `allowed`; return the string-keyed entries.
    fn checked_entries<'a>(
        &mut self,
        m: &'a Mapping,
        path: &str,
        allowed: &[&str],
    ) -> BTreeMap<&'a str, &'a Value> {
        let mut out = BTreeMap::new();
        for (k, v) in self.entries(m, path) {
            if allowed.contains(&k) {
                out.insert(k, v);
            } else {
                let suggestion =
                    closest(k, allowed.iter().copied()).map(|c| format!("did you mean `{c}`?"));
                self.error_with(
                    &join(path, k),
                    "unknown_field",
                    format!("unknown field `{k}`; allowed: {}", allowed.join(", ")),
                    suggestion,
                );
            }
        }
        out
    }

    fn name(&mut self, name: &str, path: &str) {
        if !valid_name(name) {
            self.error(
                path,
                "invalid_name",
                format!(
                    "`{name}` is not a valid name; use lowercase letters, digits, `_`, `.`, `@`, `-` \
                     (start with a letter or digit, at most 64 characters)"
                ),
            );
        }
    }

    /// A list of strings; non-list or non-string items are `wrong_type`.
    fn strings(&mut self, v: &Value, path: &str) -> Vec<(usize, String)> {
        let Value::Sequence(items) = v else {
            self.wrong_type(path, "a list of names", v);
            return Vec::new();
        };
        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate() {
            match item {
                Value::String(s) => out.push((i, s.clone())),
                other => self.wrong_type(&format!("{path}[{i}]"), "a string", other),
            }
        }
        out
    }

    /// A list whose entries must all be in `pool`.
    fn refs(
        &mut self,
        v: &Value,
        path: &str,
        pool: &BTreeSet<String>,
        what: &str,
        non_empty: bool,
    ) -> BTreeSet<String> {
        let items = self.strings(v, path);
        let is_list = matches!(v, Value::Sequence(_));
        let mut out = BTreeSet::new();
        for (i, s) in items {
            if pool.contains(&s) {
                out.insert(s);
            } else {
                self.unknown(&format!("{path}[{i}]"), &s, pool, what);
            }
        }
        if non_empty && is_list && matches!(v, Value::Sequence(s) if s.is_empty()) {
            self.error(path, "empty_set", "this list must not be empty".to_string());
        }
        out
    }

    fn unknown(&mut self, path: &str, name: &str, pool: &BTreeSet<String>, what: &str) {
        let suggestion =
            closest(name, pool.iter().map(String::as_str)).map(|c| format!("did you mean `{c}`?"));
        self.error_with(
            path,
            "unknown_name",
            format!("`{name}` is not {what}"),
            suggestion,
        );
    }

    /// A single string that must be in `pool`.
    fn one_ref(&mut self, v: &Value, path: &str, pool: &BTreeSet<String>, what: &str) -> String {
        match v {
            Value::String(s) => {
                if !pool.contains(s) {
                    self.unknown(path, s, pool, what);
                }
                s.clone()
            }
            other => {
                self.wrong_type(path, "a string", other);
                String::new()
            }
        }
    }

    fn policy(&mut self, root: &Value) -> Option<Policy> {
        let top = self.map(root, "")?;
        let fields = self.checked_entries(top, "", TOP_KEYS);

        let version = match fields.get("version") {
            None => {
                self.missing("", "version");
                0
            }
            Some(Value::Number(n)) if n.as_u64() == Some(1) => 1,
            Some(other) => {
                self.error(
                    "version",
                    "unsupported_version",
                    format!(
                        "unsupported version {}; only 1 is supported",
                        key_text(other)
                    ),
                );
                0
            }
        };

        let groups = self.groups(fields.get("groups").copied());
        let people: BTreeSet<String> = groups.values().flatten().cloned().collect();
        let agents = self.agents(fields.get("agents").copied(), &groups, &people);

        let mut referable: BTreeSet<String> = groups.keys().cloned().collect();
        referable.extend(people.iter().cloned());

        let sources = self.sources(fields.get("sources").copied(), &referable, &agents);
        let actions = self.actions(fields.get("actions").copied(), &referable, &agents);
        let tests = self.cases(
            fields.get("tests").copied(),
            &referable,
            &agents,
            &sources,
            &actions,
        );

        Some(Policy {
            version,
            groups,
            agents,
            sources,
            actions,
            tests,
        })
    }

    fn groups(&mut self, v: Option<&Value>) -> BTreeMap<String, BTreeSet<String>> {
        let mut out = BTreeMap::new();
        let Some(v) = v else { return out };
        let Some(m) = self.map(v, "groups") else {
            return out;
        };
        for (name, members) in self.entries(m, "groups") {
            let path = join("groups", name);
            self.name(name, &path);
            let mut set = BTreeSet::new();
            for (i, person) in self.strings(members, &path) {
                self.name(&person, &format!("{path}[{i}]"));
                set.insert(person);
            }
            if matches!(members, Value::Sequence(s) if s.is_empty()) {
                self.error(
                    &path,
                    "empty_group",
                    format!("group `{name}` has no members"),
                );
            }
            out.insert(name.to_string(), set);
        }
        out
    }

    fn agents(
        &mut self,
        v: Option<&Value>,
        groups: &BTreeMap<String, BTreeSet<String>>,
        people: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let Some(v) = v else { return out };
        for (i, name) in self.strings(v, "agents") {
            let path = format!("agents[{i}]");
            self.name(&name, &path);
            if groups.contains_key(&name) || people.contains(&name) {
                self.error(
                    &path,
                    "name_collision",
                    format!("`{name}` is both an agent and a group or person"),
                );
            }
            out.insert(name);
        }
        out
    }

    fn sources(
        &mut self,
        v: Option<&Value>,
        referable: &BTreeSet<String>,
        agents: &BTreeSet<String>,
    ) -> BTreeMap<String, Source> {
        let mut out = BTreeMap::new();
        let Some(v) = v else { return out };
        let Some(m) = self.map(v, "sources") else {
            return out;
        };
        for (name, body) in self.entries(m, "sources") {
            let path = join("sources", name);
            self.name(name, &path);
            let Some(bm) = self.map(body, &path) else {
                continue;
            };
            let f = self.checked_entries(bm, &path, SOURCE_KEYS);
            let readers = match f.get("readers") {
                Some(v) => self.refs(
                    v,
                    &join(&path, "readers"),
                    referable,
                    "a group or person in this policy",
                    true,
                ),
                None => {
                    self.missing(&path, "readers");
                    BTreeSet::new()
                }
            };
            let agents_set = match f.get("agents") {
                Some(v) => self.refs(
                    v,
                    &join(&path, "agents"),
                    agents,
                    "an agent declared in `agents`",
                    false,
                ),
                None => BTreeSet::new(),
            };
            out.insert(
                name.to_string(),
                Source {
                    readers,
                    agents: agents_set,
                },
            );
        }
        out
    }

    fn actions(
        &mut self,
        v: Option<&Value>,
        referable: &BTreeSet<String>,
        agents: &BTreeSet<String>,
    ) -> BTreeMap<String, Action> {
        let mut out = BTreeMap::new();
        let Some(v) = v else { return out };
        let Some(m) = self.map(v, "actions") else {
            return out;
        };
        for (name, body) in self.entries(m, "actions") {
            let path = join("actions", name);
            self.name(name, &path);
            let Some(bm) = self.map(body, &path) else {
                continue;
            };
            let f = self.checked_entries(bm, &path, ACTION_KEYS);
            let agent_set = match f.get("agents") {
                Some(v) => self.refs(
                    v,
                    &join(&path, "agents"),
                    agents,
                    "an agent declared in `agents`",
                    true,
                ),
                None => {
                    self.missing(&path, "agents");
                    BTreeSet::new()
                }
            };
            let audience = match f.get("audience") {
                Some(v) => self.refs(
                    v,
                    &join(&path, "audience"),
                    referable,
                    "a group or person in this policy",
                    true,
                ),
                None => {
                    self.missing(&path, "audience");
                    BTreeSet::new()
                }
            };
            let approval = f
                .get("approval")
                .and_then(|v| self.approval(v, &join(&path, "approval"), referable));
            out.insert(
                name.to_string(),
                Action {
                    agents: agent_set,
                    audience,
                    approval,
                },
            );
        }
        out
    }

    fn approval(
        &mut self,
        v: &Value,
        path: &str,
        referable: &BTreeSet<String>,
    ) -> Option<ApprovalRule> {
        let m = self.map(v, path)?;
        let f = self.checked_entries(m, path, APPROVAL_KEYS);
        let reviewers = match f.get("reviewers") {
            Some(v) => self.refs(
                v,
                &join(path, "reviewers"),
                referable,
                "a group or person in this policy",
                true,
            ),
            None => {
                self.missing(path, "reviewers");
                BTreeSet::new()
            }
        };
        let valid_for_seconds = match f.get("valid_for") {
            Some(v) => self.duration(v, &join(path, "valid_for")),
            None => {
                self.missing(path, "valid_for");
                0
            }
        };
        Some(ApprovalRule {
            reviewers,
            valid_for_seconds,
        })
    }

    fn duration(&mut self, v: &Value, path: &str) -> u64 {
        let Value::String(s) = v else {
            self.wrong_type(path, "a duration string such as `30m`", v);
            return 0;
        };
        match parse_duration(s) {
            Some(secs) => secs,
            None => {
                self.error_with(
                    path,
                    "invalid_duration",
                    format!(
                        "`{s}` is not a valid duration; use <integer><s|m|h|d> between 1s and 30d"
                    ),
                    Some("for example `30m`, `2h` or `1d`".to_string()),
                );
                0
            }
        }
    }

    fn cases(
        &mut self,
        v: Option<&Value>,
        referable: &BTreeSet<String>,
        agents: &BTreeSet<String>,
        sources: &BTreeMap<String, Source>,
        actions: &BTreeMap<String, Action>,
    ) -> Vec<Case> {
        let mut out = Vec::new();
        let Some(v) = v else { return out };
        let Value::Sequence(items) = v else {
            self.wrong_type("tests", "a list of test cases", v);
            return out;
        };
        let source_names: BTreeSet<String> = sources.keys().cloned().collect();
        let action_names: BTreeSet<String> = actions.keys().cloned().collect();
        let mut seen = BTreeSet::new();
        for (i, item) in items.iter().enumerate() {
            let path = format!("tests[{i}]");
            let Some(m) = self.map(item, &path) else {
                continue;
            };
            let f = self.checked_entries(m, &path, CASE_KEYS);
            let name = match f.get("name") {
                Some(Value::String(s)) => {
                    if !seen.insert(s.clone()) {
                        self.error(
                            &join(&path, "name"),
                            "duplicate_test",
                            format!("another test is already named `{s}`"),
                        );
                    }
                    s.clone()
                }
                Some(other) => {
                    self.wrong_type(&join(&path, "name"), "a string", other);
                    String::new()
                }
                None => {
                    self.missing(&path, "name");
                    String::new()
                }
            };
            let output_from = match f.get("output_from") {
                Some(v) => self.refs(
                    v,
                    &join(&path, "output_from"),
                    &source_names,
                    "a source in this policy",
                    true,
                ),
                None => {
                    self.missing(&path, "output_from");
                    BTreeSet::new()
                }
            };
            let agent = match f.get("agent") {
                Some(v) => self.one_ref(
                    v,
                    &join(&path, "agent"),
                    agents,
                    "an agent declared in `agents`",
                ),
                None => {
                    self.missing(&path, "agent");
                    String::new()
                }
            };
            let action = match f.get("action") {
                Some(v) => self.one_ref(
                    v,
                    &join(&path, "action"),
                    &action_names,
                    "an action in this policy",
                ),
                None => {
                    self.missing(&path, "action");
                    String::new()
                }
            };
            let recipients = f.get("recipients").map(|v| {
                self.refs(
                    v,
                    &join(&path, "recipients"),
                    referable,
                    "a group or person in this policy",
                    true,
                )
            });
            let expect = match f.get("expect") {
                Some(Value::String(s)) => match s.as_str() {
                    "allow" => Verdict::Allow,
                    "needs_approval" => Verdict::NeedsApproval,
                    "deny" => Verdict::Deny,
                    _ => {
                        self.error(
                            &join(&path, "expect"),
                            "wrong_type",
                            format!("expected one of allow, needs_approval, deny; found `{s}`"),
                        );
                        Verdict::Deny
                    }
                },
                Some(other) => {
                    self.wrong_type(
                        &join(&path, "expect"),
                        "one of allow, needs_approval, deny",
                        other,
                    );
                    Verdict::Deny
                }
                None => {
                    self.missing(&path, "expect");
                    Verdict::Deny
                }
            };
            out.push(Case {
                name,
                output_from,
                agent,
                action,
                recipients,
                expect,
            });
        }
        out
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
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Sequence(_) => "a list",
        Value::Mapping(_) => "a mapping",
        Value::Tagged(_) => "a tagged value",
    }
}

fn key_text(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => type_name(other).to_string(),
    }
}

fn valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    s.len() <= 64
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '@' | '-')
        })
}

fn parse_duration(s: &str) -> Option<u64> {
    let unit = s.chars().last()?;
    let digits = &s[..s.len() - unit.len_utf8()];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = digits.parse().ok()?;
    let mult = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return None,
    };
    let secs = n.checked_mul(mult)?;
    (1..=MAX_VALID_FOR_SECONDS).contains(&secs).then_some(secs)
}

/// The nearest candidate within edit distance 2 (ties go to the first).
fn closest<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .map(|c| (edit_distance(name, c), c))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}
