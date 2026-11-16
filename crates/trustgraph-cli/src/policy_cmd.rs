//! `trustgraph policy ...`: write, check, test, explain and diff a
//! `trustgraph.policy.yaml`.
//!
//! Exit codes: 0 success, 1 the policy has errors or a test failed, 2 the
//! command could not run (I/O failure, file exists). With `--json` stdout
//! carries exactly one JSON document and nothing else.

use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{Value, json};
use trustgraph_policy::{
    ChangeKind, Diagnostic, MAX_BYTES, Policy, STARTER, Severity, diff, explain, lint, parse,
    run_tests, schema,
};
use trustgraph_sim::policy_sim::{Campaign, campaign};

#[derive(Subcommand)]
pub enum PolicyCommand {
    /// Write a commented starter policy file.
    Init {
        #[arg(long, default_value = "trustgraph.policy.yaml")]
        path: PathBuf,
        /// Overwrite the file if it already exists.
        #[arg(long)]
        force: bool,
        #[arg(long)]
        json: bool,
    },
    /// Print the JSON Schema for policy files.
    Schema {
        #[arg(long)]
        json: bool,
    },
    /// Validate a policy file and report errors and warnings.
    Check {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Run the policy's inline test cases.
    Test {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Describe in plain English what a policy allows.
    Explain {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Show who gains or loses access between two policy files.
    Diff {
        old: PathBuf,
        new: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Run the policy through seeded random scenarios and check the safety invariants.
    Simulate {
        file: PathBuf,
        /// First seed; runs use seed, seed+1, ...
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// Number of runs (1-10000).
        #[arg(long, default_value_t = 64)]
        count: u64,
        /// Steps per run (1-5000).
        #[arg(long, default_value_t = 200)]
        steps: u32,
        #[arg(long)]
        json: bool,
    },
}

macro_rules! out {
    ($o:expr, $($a:tt)*) => {{ let _ = write!($o, $($a)*); }};
}
macro_rules! outln {
    ($o:expr) => {{ $o.push('\n'); }};
    ($o:expr, $($a:tt)*) => {{ let _ = writeln!($o, $($a)*); }};
}

/// Run a policy command and return the process exit code.
pub fn run(command: PolicyCommand) -> i32 {
    let mut out = String::new();
    let code = dispatch(&mut out, command);
    match emit(&mut std::io::stdout().lock(), &out) {
        Ok(()) => code,
        Err(_) => 2,
    }
}

/// The single place command output reaches stdout; a closed pipe is an
/// error value, never a panic.
fn emit(writer: &mut impl Write, text: &str) -> std::io::Result<()> {
    writer.write_all(text.as_bytes())?;
    writer.flush()
}

fn dispatch(out: &mut String, command: PolicyCommand) -> i32 {
    match command {
        PolicyCommand::Init { path, force, json } => init(out, &path, force, json),
        PolicyCommand::Schema { .. } => {
            outln!(out, "{}", pretty(&schema()));
            0
        }
        PolicyCommand::Check { file, json } => check(out, &file, json),
        PolicyCommand::Test { file, json } => test(out, &file, json),
        PolicyCommand::Explain { file, json } => explain_cmd(out, &file, json),
        PolicyCommand::Diff { old, new, json } => diff_cmd(out, &old, &new, json),
        PolicyCommand::Simulate {
            file,
            seed,
            count,
            steps,
            json,
        } => simulate_cmd(out, &file, seed, count, steps, json),
    }
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON value serializes")
}

fn fail(out: &mut String, json: bool, code: &str, message: String) -> i32 {
    if json {
        outln!(
            out,
            "{}",
            pretty(&json!({"ok": false, "error": {"code": code, "message": message}}))
        );
    } else {
        let _ = writeln!(std::io::stderr(), "error: {message}");
    }
    2
}

/// Write `STARTER` to a fresh sibling temp file, then put it at `path`:
/// `rename` (replacing whatever is there, including a symlink itself) with
/// `force`, otherwise a hard link that fails if anything exists at `path`,
/// dangling symlinks included. The temp file is always removed.
fn write_starter(path: &Path, force: bool) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("path has no file name"))?
        .to_string_lossy();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let mut attempt = 0u32;
    let (temp, mut file) = loop {
        let temp = dir.join(format!(
            ".{name}.{}.{nanos}.{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(f) => break (temp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    };
    let result = file
        .write_all(STARTER.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            if force {
                std::fs::rename(&temp, path)
            } else {
                std::fs::hard_link(&temp, path)
            }
        });
    if !force || result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn init(out: &mut String, path: &Path, force: bool, json: bool) -> i32 {
    let shown = path.display();
    match write_starter(path, force) {
        Ok(()) => {
            if json {
                outln!(
                    out,
                    "{}",
                    pretty(&json!({"ok": true, "path": path.to_string_lossy()}))
                );
            } else {
                outln!(out, "Wrote {shown}");
            }
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => fail(
            out,
            json,
            "exists",
            format!("{shown} already exists; pass --force to overwrite"),
        ),
        Err(e) => fail(out, json, "io", format!("cannot write {shown}: {e}")),
    }
}

/// What reading and parsing one file produced.
struct Loaded {
    policy: Option<Policy>,
    /// Parse diagnostics, then lints when parsing produced a policy.
    diagnostics: Vec<Diagnostic>,
}

impl Loaded {
    fn has_errors(&self) -> bool {
        has_errors(&self.diagnostics)
    }
}

fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|d| d.severity == Severity::Error)
}

/// Read a regular file of at most `MAX_BYTES`. Oversize input is reported as
/// the `too_large` diagnostic rather than read; anything else that goes wrong
/// is an I/O error message.
fn load(file: &Path) -> Result<Loaded, String> {
    let text = match read_text(file)? {
        Ok(text) => text,
        Err(diagnostic) => {
            return Ok(Loaded {
                policy: None,
                diagnostics: vec![diagnostic],
            });
        }
    };
    let parsed = parse(&text);
    let mut diagnostics = parsed.diagnostics;
    if let Some(policy) = &parsed.policy {
        diagnostics.extend(lint(policy));
    }
    Ok(Loaded {
        policy: parsed.policy,
        diagnostics,
    })
}

/// The safe-read half of `load`: the file's text, or the `too_large`
/// diagnostic, or an I/O error message.
fn read_text(file: &Path) -> Result<Result<String, Diagnostic>, String> {
    let io = |e: &dyn std::fmt::Display| format!("cannot read {}: {e}", file.display());
    // Check before opening: opening a FIFO would block until a writer shows up.
    let before = std::fs::metadata(file).map_err(|e| io(&e))?;
    if !before.is_file() {
        return Err(io(&"not a regular file"));
    }
    let handle = std::fs::File::open(file).map_err(|e| io(&e))?;
    // Guard against the path being swapped between the two calls.
    let meta = handle.metadata().map_err(|e| io(&e))?;
    if !meta.is_file() {
        return Err(io(&"not a regular file"));
    }
    let mut bytes = Vec::new();
    handle
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io(&e))?;
    if bytes.len() > MAX_BYTES {
        let size = (meta.len() as usize).max(bytes.len());
        return Ok(Err(Diagnostic {
            severity: Severity::Error,
            path: String::new(),
            code: "too_large",
            message: format!("policy file is {size} bytes; the limit is {MAX_BYTES}"),
            suggestion: None,
        }));
    }
    String::from_utf8(bytes)
        .map(Ok)
        .map_err(|_| io(&"file is not valid UTF-8"))
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn print_diagnostics(out: &mut String, diagnostics: &[Diagnostic]) {
    for d in diagnostics {
        let severity = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let path = if d.path.is_empty() { "(root)" } else { &d.path };
        out!(out, "{severity} {path} {}: {}", d.code, d.message);
        if let Some(s) = &d.suggestion {
            out!(out, " ({s})");
        }
        outln!(out);
    }
}

fn summary(diagnostics: &[Diagnostic]) -> String {
    let errors = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = diagnostics.len() - errors;
    format!(
        "{}: {}, {}",
        if errors == 0 { "ok" } else { "failed" },
        plural(errors, "error"),
        plural(warnings, "warning")
    )
}

fn check(out: &mut String, file: &Path, json: bool) -> i32 {
    let loaded = match load(file) {
        Ok(l) => l,
        Err(m) => return fail(out, json, "io", m),
    };
    let ok = !loaded.has_errors();
    if json {
        outln!(
            out,
            "{}",
            pretty(&json!({"ok": ok, "diagnostics": loaded.diagnostics}))
        );
    } else {
        print_diagnostics(out, &loaded.diagnostics);
        outln!(out, "{}", summary(&loaded.diagnostics));
    }
    i32::from(!ok)
}

/// Print the diagnostics of a file that failed to load cleanly.
fn report_invalid(
    out: &mut String,
    loaded: &Loaded,
    json: bool,
    extra: Option<(&str, Value)>,
    results: bool,
) -> i32 {
    if json {
        let mut doc = json!({"ok": false, "diagnostics": loaded.diagnostics});
        if results {
            doc["results"] = json!([]);
        }
        if let Some((k, v)) = extra {
            doc[k] = v;
        }
        outln!(out, "{}", pretty(&doc));
    } else {
        if let Some((_, Value::String(name))) = &extra {
            outln!(out, "{name}:");
        }
        print_diagnostics(out, &loaded.diagnostics);
        outln!(out, "{}", summary(&loaded.diagnostics));
    }
    1
}

fn test(out: &mut String, file: &Path, json: bool) -> i32 {
    let loaded = match load(file) {
        Ok(l) => l,
        Err(m) => return fail(out, json, "io", m),
    };
    let policy = match &loaded.policy {
        Some(p) if !loaded.has_errors() => p,
        _ => return report_invalid(out, &loaded, json, None, true),
    };
    let results = run_tests(policy);
    let failed = results.iter().filter(|r| !r.pass).count();
    if json {
        outln!(
            out,
            "{}",
            pretty(&json!({
                "ok": failed == 0,
                "diagnostics": loaded.diagnostics,
                "results": results,
            }))
        );
    } else {
        for r in &results {
            if r.pass {
                outln!(out, "PASS {}", r.name);
            } else {
                outln!(
                    out,
                    "FAIL {}: expected {}, got {} \u{2014} {}",
                    r.name,
                    r.expect.as_str(),
                    r.outcome.verdict.as_str(),
                    r.outcome.reason
                );
            }
        }
        outln!(out, "{} passed, {} failed", results.len() - failed, failed);
    }
    i32::from(failed > 0)
}

fn explain_cmd(out: &mut String, file: &Path, json: bool) -> i32 {
    let loaded = match load(file) {
        Ok(l) => l,
        Err(m) => return fail(out, json, "io", m),
    };
    let policy = match &loaded.policy {
        Some(p) if !loaded.has_errors() => p,
        _ => return report_invalid(out, &loaded, json, None, false),
    };
    let text = explain(policy);
    if json {
        outln!(out, "{}", pretty(&json!({"ok": true, "text": text})));
    } else {
        out!(out, "{text}");
    }
    0
}

fn diff_cmd(out: &mut String, old: &Path, new: &Path, json: bool) -> i32 {
    let mut policies = Vec::new();
    for file in [old, new] {
        let loaded = match load(file) {
            Ok(l) => l,
            Err(m) => return fail(out, json, "io", m),
        };
        match loaded.policy {
            Some(ref p) if !loaded.has_errors() => policies.push(p.clone()),
            _ => {
                let name = file.display().to_string();
                return report_invalid(out, &loaded, json, Some(("file", json!(name))), false);
            }
        }
    }
    let result = diff(&policies[0], &policies[1]);
    if json {
        outln!(
            out,
            "{}",
            pretty(&json!({
                "ok": true,
                "widens": result.widens(),
                "changes": result.changes,
            }))
        );
    } else if result.is_empty() {
        outln!(out, "No changes.");
    } else {
        for c in &result.changes {
            let tag = match c.kind {
                ChangeKind::Widens => "WIDENS ",
                ChangeKind::Narrows => "narrows",
                ChangeKind::Neutral => "neutral",
            };
            outln!(out, "{tag} {}: {}", c.subject, c.summary);
        }
    }
    0
}

const MAX_COUNT: u64 = 10_000;
const MAX_STEPS: u32 = 5_000;
/// Violations printed in human mode before the rest are summarised.
const PRINTED_VIOLATIONS: usize = 20;

fn simulate_cmd(
    out: &mut String,
    file: &Path,
    seed: u64,
    count: u64,
    steps: u32,
    json: bool,
) -> i32 {
    if !(1..=MAX_COUNT).contains(&count) {
        let m = format!("--count must be between 1 and {MAX_COUNT}");
        return fail(out, json, "usage", m);
    }
    if !(1..=MAX_STEPS).contains(&steps) {
        let m = format!("--steps must be between 1 and {MAX_STEPS}");
        return fail(out, json, "usage", m);
    }
    let text = match read_text(file) {
        Ok(Ok(text)) => text,
        Ok(Err(diagnostic)) => {
            let loaded = Loaded {
                policy: None,
                diagnostics: vec![diagnostic],
            };
            return report_invalid(out, &loaded, json, None, false);
        }
        Err(m) => return fail(out, json, "io", m),
    };
    match campaign(&text, seed, count, steps) {
        Ok(c) => {
            let (text, code) = render_simulation(&file.display().to_string(), &c, json);
            out.push_str(&text);
            code
        }
        Err(diagnostics) => {
            let loaded = Loaded {
                policy: None,
                diagnostics,
            };
            report_invalid(out, &loaded, json, None, false)
        }
    }
}

/// `12800` -> `12,800`.
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut s = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            s.push(',');
        }
        s.push(c);
    }
    s
}

/// Render a finished campaign: the output text and the exit code.
fn render_simulation(file: &str, c: &Campaign, json: bool) -> (String, i32) {
    let mut out = String::new();
    let ok = c.violations.is_empty();
    let decision = |k: &str| c.decisions.get(k).copied().unwrap_or(0);
    if json {
        let violations: Vec<Value> = c
            .violations
            .iter()
            .map(|(seed, v)| {
                json!({"seed": seed, "invariant": v.invariant, "step": v.step, "detail": v.detail})
            })
            .collect();
        outln!(
            out,
            "{}",
            pretty(&json!({
                "ok": ok,
                "first_seed": c.first_seed,
                "count": c.count,
                "steps": c.steps,
                "events": c.events,
                "decisions": {
                    "allow": decision("allow"),
                    "deny": decision("deny"),
                    "needs_approval": decision("needs_approval"),
                },
                "violations": violations,
                "digest": c.digest,
            }))
        );
        return (out, i32::from(!ok));
    }
    outln!(
        out,
        "{} runs, {} events: {} allow, {} deny, {} needs_approval \u{2014} {}",
        thousands(c.count),
        thousands(c.events),
        thousands(decision("allow")),
        thousands(decision("deny")),
        thousands(decision("needs_approval")),
        plural(c.violations.len(), "violation"),
    );
    for (seed, v) in c.violations.iter().take(PRINTED_VIOLATIONS) {
        outln!(
            out,
            "VIOLATION {} seed={seed} step={}: {}",
            v.invariant,
            v.step,
            v.detail
        );
        outln!(
            out,
            "replay: trustgraph policy simulate {file} --seed {seed} --count 1 --steps {}",
            c.steps
        );
    }
    if c.violations.len() > PRINTED_VIOLATIONS {
        outln!(
            out,
            "\u{2026} and {} more",
            c.violations.len() - PRINTED_VIOLATIONS
        );
    }
    (out, i32::from(!ok))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use trustgraph_sim::policy_sim::Violation;

    fn campaign_with(n: u64) -> Campaign {
        Campaign {
            first_seed: 0,
            count: 64,
            steps: 200,
            events: 12_800,
            decisions: BTreeMap::from([
                ("allow".to_string(), 3_912),
                ("deny".to_string(), 7_204),
                ("needs_approval".to_string(), 1_684),
            ]),
            violations: (0..n)
                .map(|i| {
                    (
                        100 + i,
                        Violation {
                            invariant: "no_leak",
                            step: 93,
                            detail: format!("detail {i}"),
                        },
                    )
                })
                .collect(),
            digest: "d".to_string(),
        }
    }

    #[test]
    fn clean_campaign_summary_line() {
        let (text, code) = render_simulation("p.yaml", &campaign_with(0), false);
        assert_eq!(code, 0);
        assert_eq!(
            text,
            "64 runs, 12,800 events: 3,912 allow, 7,204 deny, 1,684 needs_approval \u{2014} 0 violations\n"
        );
    }

    #[test]
    fn human_output_caps_violations_and_shows_replay() {
        let (text, code) = render_simulation("p.yaml", &campaign_with(25), false);
        assert_eq!(code, 1);
        assert!(text.contains("\u{2014} 25 violations\n"));
        assert_eq!(text.matches("VIOLATION ").count(), 20);
        assert!(text.contains("VIOLATION no_leak seed=100 step=93: detail 0\n"));
        assert!(text.contains(
            "replay: trustgraph policy simulate p.yaml --seed 100 --count 1 --steps 200\n"
        ));
        assert!(text.ends_with("\u{2026} and 5 more\n"));
        assert!(!text.contains("detail 20\n"));
    }

    #[test]
    fn json_output_lists_every_violation() {
        let (text, code) = render_simulation("p.yaml", &campaign_with(25), true);
        assert_eq!(code, 1);
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["violations"].as_array().unwrap().len(), 25);
        assert_eq!(v["violations"][24]["seed"], 124);
        assert_eq!(v["violations"][0]["invariant"], "no_leak");
        assert_eq!(v["decisions"]["needs_approval"], 1_684);
    }
}
