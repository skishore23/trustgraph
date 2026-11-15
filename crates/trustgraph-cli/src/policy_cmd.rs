//! `trustgraph policy ...`: write, check, test, explain and diff a
//! `trustgraph.policy.yaml`.
//!
//! Exit codes: 0 success, 1 the policy has errors or a test failed, 2 the
//! command could not run (I/O failure, file exists). With `--json` stdout
//! carries exactly one JSON document and nothing else.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::{Value, json};
use trustgraph_policy::{
    ChangeKind, Diagnostic, MAX_BYTES, Policy, STARTER, Severity, diff, explain, lint, parse,
    run_tests, schema,
};

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
}

/// Run a policy command and return the process exit code.
pub fn run(command: PolicyCommand) -> i32 {
    let code = match command {
        PolicyCommand::Init { path, force, json } => init(&path, force, json),
        PolicyCommand::Schema { .. } => {
            println!("{}", pretty(&schema()));
            0
        }
        PolicyCommand::Check { file, json } => check(&file, json),
        PolicyCommand::Test { file, json } => test(&file, json),
        PolicyCommand::Explain { file, json } => explain_cmd(&file, json),
        PolicyCommand::Diff { old, new, json } => diff_cmd(&old, &new, json),
    };
    let _ = std::io::stdout().flush();
    code
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON value serializes")
}

fn fail(json: bool, code: &str, message: String) -> i32 {
    if json {
        println!(
            "{}",
            pretty(&json!({"ok": false, "error": {"code": code, "message": message}}))
        );
    } else {
        eprintln!("error: {message}");
    }
    2
}

fn init(path: &Path, force: bool, json: bool) -> i32 {
    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let shown = path.display();
    let result = options
        .open(path)
        .and_then(|mut f| f.write_all(STARTER.as_bytes()));
    match result {
        Ok(()) => {
            if json {
                println!("{}", pretty(&json!({"ok": true, "path": path})));
            } else {
                println!("Wrote {shown}");
            }
            0
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => fail(
            json,
            "exists",
            format!("{shown} already exists; pass --force to overwrite"),
        ),
        Err(e) => fail(json, "io", format!("cannot write {shown}: {e}")),
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
    let io = |e: &dyn std::fmt::Display| format!("cannot read {}: {e}", file.display());
    let handle = std::fs::File::open(file).map_err(|e| io(&e))?;
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
        return Ok(Loaded {
            policy: None,
            diagnostics: vec![Diagnostic {
                severity: Severity::Error,
                path: String::new(),
                code: "too_large",
                message: format!("policy file is {size} bytes; the limit is {MAX_BYTES}"),
                suggestion: None,
            }],
        });
    }
    let text = String::from_utf8(bytes).map_err(|_| io(&"file is not valid UTF-8"))?;
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

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn print_diagnostics(diagnostics: &[Diagnostic]) {
    for d in diagnostics {
        let severity = match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let path = if d.path.is_empty() { "(root)" } else { &d.path };
        print!("{severity} {path} {}: {}", d.code, d.message);
        if let Some(s) = &d.suggestion {
            print!(" ({s})");
        }
        println!();
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

fn check(file: &Path, json: bool) -> i32 {
    let loaded = match load(file) {
        Ok(l) => l,
        Err(m) => return fail(json, "io", m),
    };
    let ok = !loaded.has_errors();
    if json {
        println!(
            "{}",
            pretty(&json!({"ok": ok, "diagnostics": loaded.diagnostics}))
        );
    } else {
        print_diagnostics(&loaded.diagnostics);
        println!("{}", summary(&loaded.diagnostics));
    }
    i32::from(!ok)
}

/// Print the diagnostics of a file that failed to load cleanly.
fn report_invalid(loaded: &Loaded, json: bool, extra: Option<(&str, Value)>, results: bool) -> i32 {
    if json {
        let mut doc = json!({"ok": false, "diagnostics": loaded.diagnostics});
        if results {
            doc["results"] = json!([]);
        }
        if let Some((k, v)) = extra {
            doc[k] = v;
        }
        println!("{}", pretty(&doc));
    } else {
        if let Some((_, Value::String(name))) = &extra {
            println!("{name}:");
        }
        print_diagnostics(&loaded.diagnostics);
        println!("{}", summary(&loaded.diagnostics));
    }
    1
}

fn test(file: &Path, json: bool) -> i32 {
    let loaded = match load(file) {
        Ok(l) => l,
        Err(m) => return fail(json, "io", m),
    };
    let policy = match &loaded.policy {
        Some(p) if !loaded.has_errors() => p,
        _ => return report_invalid(&loaded, json, None, true),
    };
    let results = run_tests(policy);
    let failed = results.iter().filter(|r| !r.pass).count();
    if json {
        println!(
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
                println!("PASS {}", r.name);
            } else {
                println!(
                    "FAIL {}: expected {}, got {} \u{2014} {}",
                    r.name,
                    r.expect.as_str(),
                    r.outcome.verdict.as_str(),
                    r.outcome.reason
                );
            }
        }
        println!("{} passed, {} failed", results.len() - failed, failed);
    }
    i32::from(failed > 0)
}

fn explain_cmd(file: &Path, json: bool) -> i32 {
    let loaded = match load(file) {
        Ok(l) => l,
        Err(m) => return fail(json, "io", m),
    };
    let policy = match &loaded.policy {
        Some(p) if !loaded.has_errors() => p,
        _ => return report_invalid(&loaded, json, None, false),
    };
    let text = explain(policy);
    if json {
        println!("{}", pretty(&json!({"ok": true, "text": text})));
    } else {
        print!("{text}");
    }
    0
}

fn diff_cmd(old: &Path, new: &Path, json: bool) -> i32 {
    let mut policies = Vec::new();
    for file in [old, new] {
        let loaded = match load(file) {
            Ok(l) => l,
            Err(m) => return fail(json, "io", m),
        };
        match loaded.policy {
            Some(ref p) if !loaded.has_errors() => policies.push(p.clone()),
            _ => {
                let name = file.display().to_string();
                return report_invalid(&loaded, json, Some(("file", json!(name))), false);
            }
        }
    }
    let result = diff(&policies[0], &policies[1]);
    if json {
        println!(
            "{}",
            pretty(&json!({
                "ok": true,
                "widens": result.widens(),
                "changes": result.changes,
            }))
        );
    } else if result.is_empty() {
        println!("No changes.");
    } else {
        for c in &result.changes {
            let tag = match c.kind {
                ChangeKind::Widens => "WIDENS ",
                ChangeKind::Narrows => "narrows",
                ChangeKind::Neutral => "neutral",
            };
            println!("{tag} {}: {}", c.subject, c.summary);
        }
    }
    0
}
