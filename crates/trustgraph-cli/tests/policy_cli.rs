use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn tg(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trustgraph"))
        .current_dir(dir)
        .arg("policy")
        .args(args)
        .output()
        .expect("run trustgraph")
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exit code")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).expect("utf8")
}

fn json(o: &Output) -> Value {
    serde_json::from_str(&stdout(o))
        .unwrap_or_else(|e| panic!("not one JSON doc: {e}: {}", stdout(o)))
}

const BASE: &str = "version: 1
groups:
  engineering: [sam]
  hr: [nina]
agents: [planner]
sources:
  salaries:
    readers: [hr]
    agents: [planner]
actions:
  post:
    agents: [planner]
    audience: [hr]
tests:
  - name: ok
    output_from: [salaries]
    agent: planner
    action: post
    expect: allow
";

#[test]
fn init_then_check_and_test_pass() {
    let d = tempfile::tempdir().unwrap();
    let o = tg(d.path(), &["init"]);
    assert_eq!(code(&o), 0, "{}", stdout(&o));
    assert!(stdout(&o).contains("Wrote"));
    let f = "trustgraph.policy.yaml";
    assert_eq!(code(&tg(d.path(), &["check", f])), 0);
    assert_eq!(code(&tg(d.path(), &["test", f])), 0);
    let o = tg(d.path(), &["check", f, "--json"]);
    assert_eq!(code(&o), 0);
    let v = json(&o);
    assert_eq!(v["ok"], true);
    assert_eq!(v["diagnostics"].as_array().unwrap().len(), 0);
}

#[test]
fn init_refuses_to_overwrite() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(code(&tg(d.path(), &["init"])), 0);
    let o = tg(d.path(), &["init"]);
    assert_eq!(code(&o), 2);
    let o = tg(d.path(), &["init", "--json"]);
    assert_eq!(code(&o), 2);
    let v = json(&o);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "exists");
    assert_eq!(code(&tg(d.path(), &["init", "--force"])), 0);
}

#[test]
fn check_json_reports_path_code_suggestion() {
    let d = tempfile::tempdir().unwrap();
    // typo of an existing group name
    std::fs::write(
        d.path().join("p.yaml"),
        BASE.replace("readers: [hr]", "readers: [enginering]"),
    )
    .unwrap();
    let o = tg(d.path(), &["check", "p.yaml", "--json"]);
    assert_eq!(code(&o), 1);
    let v = json(&o);
    assert_eq!(v["ok"], false);
    let diag = &v["diagnostics"][0];
    assert_eq!(diag["code"], "unknown_name");
    assert_eq!(diag["path"], "sources.salaries.readers[0]");
    assert!(diag["suggestion"].as_str().unwrap().contains("engineering"));
    let o = tg(d.path(), &["check", "p.yaml"]);
    assert_eq!(code(&o), 1);
    let s = stdout(&o);
    assert!(
        s.starts_with("error sources.salaries.readers[0] unknown_name:"),
        "{s}"
    );
    assert!(s.contains("did you mean"));
    assert!(s.contains("failed: 1 error"));
}

#[test]
fn failing_inline_test_exits_1() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("p.yaml"),
        BASE.replace("expect: allow", "expect: deny"),
    )
    .unwrap();
    let o = tg(d.path(), &["test", "p.yaml"]);
    assert_eq!(code(&o), 1);
    assert!(
        stdout(&o).contains("FAIL ok: expected deny, got allow"),
        "{}",
        stdout(&o)
    );
    let o = tg(d.path(), &["test", "p.yaml", "--json"]);
    assert_eq!(code(&o), 1);
    let v = json(&o);
    assert_eq!(v["ok"], false);
    assert_eq!(v["results"][0]["pass"], false);
    assert_eq!(v["results"][0]["expect"], "deny");
    assert_eq!(v["results"][0]["outcome"]["verdict"], "allow");
}

#[test]
fn test_with_parse_error_emits_diagnostics() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("p.yaml"), "version: 2\n").unwrap();
    let o = tg(d.path(), &["test", "p.yaml", "--json"]);
    assert_eq!(code(&o), 1);
    let v = json(&o);
    assert_eq!(v["ok"], false);
    assert_eq!(v["results"].as_array().unwrap().len(), 0);
    assert_eq!(v["diagnostics"][0]["code"], "unsupported_version");
}

#[test]
fn explain_outputs_text() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("p.yaml"), BASE).unwrap();
    let o = tg(d.path(), &["explain", "p.yaml"]);
    assert_eq!(code(&o), 0);
    assert!(stdout(&o).contains("salaries"));
    let v = json(&tg(d.path(), &["explain", "p.yaml", "--json"]));
    assert_eq!(v["ok"], true);
    assert!(v["text"].as_str().unwrap().contains("salaries"));
    std::fs::write(d.path().join("bad.yaml"), "version: 2\n").unwrap();
    let o = tg(d.path(), &["explain", "bad.yaml", "--json"]);
    assert_eq!(code(&o), 1);
    assert_eq!(json(&o)["ok"], false);
}

#[test]
fn diff_marks_widening() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("old.yaml"), BASE).unwrap();
    std::fs::write(
        d.path().join("new.yaml"),
        BASE.replace("readers: [hr]", "readers: [hr, engineering]"),
    )
    .unwrap();
    let o = tg(d.path(), &["diff", "old.yaml", "new.yaml", "--json"]);
    assert_eq!(code(&o), 0);
    let v = json(&o);
    assert_eq!(v["widens"], true);
    assert!(!v["changes"].as_array().unwrap().is_empty());
    let o = tg(d.path(), &["diff", "old.yaml", "new.yaml"]);
    assert_eq!(code(&o), 0);
    assert!(stdout(&o).contains("WIDENS"), "{}", stdout(&o));
    // reverse narrows
    let v = json(&tg(d.path(), &["diff", "new.yaml", "old.yaml", "--json"]));
    assert_eq!(v["widens"], false);
}

#[test]
fn diff_parse_error_names_file() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("old.yaml"), BASE).unwrap();
    std::fs::write(d.path().join("new.yaml"), "version: 2\n").unwrap();
    let o = tg(d.path(), &["diff", "old.yaml", "new.yaml", "--json"]);
    assert_eq!(code(&o), 1);
    let v = json(&o);
    assert_eq!(v["ok"], false);
    assert_eq!(v["file"], "new.yaml");
}

#[test]
fn schema_validates_shape() {
    let d = tempfile::tempdir().unwrap();
    for args in [vec!["schema"], vec!["schema", "--json"]] {
        let o = tg(d.path(), &args);
        assert_eq!(code(&o), 0);
        let v = json(&o);
        assert!(v["properties"]["sources"].is_object());
    }
}

#[test]
fn missing_file_exits_2() {
    let d = tempfile::tempdir().unwrap();
    for cmd in ["check", "test", "explain"] {
        let o = tg(d.path(), &[cmd, "nope.yaml"]);
        assert_eq!(code(&o), 2, "{cmd}");
        assert!(String::from_utf8_lossy(&o.stderr).starts_with("error: cannot read nope.yaml:"));
        let o = tg(d.path(), &[cmd, "nope.yaml", "--json"]);
        assert_eq!(code(&o), 2);
        let v = json(&o);
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "io");
    }
    assert_eq!(code(&tg(d.path(), &["diff", "nope.yaml", "nope2.yaml"])), 2);
}

#[test]
fn directory_exits_2() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir(d.path().join("dir")).unwrap();
    assert_eq!(code(&tg(d.path(), &["check", "dir"])), 2);
    let o = tg(d.path(), &["check", "dir", "--json"]);
    assert_eq!(code(&o), 2);
    assert_eq!(json(&o)["error"]["code"], "io");
}

#[test]
fn oversize_file_is_too_large_diagnostic() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("big.yaml"), vec![b'#'; 300_000]).unwrap();
    let o = tg(d.path(), &["check", "big.yaml", "--json"]);
    assert_eq!(code(&o), 1);
    assert_eq!(json(&o)["diagnostics"][0]["code"], "too_large");
}

#[test]
fn non_utf8_exits_2() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("b.yaml"), [0xff, 0xfe, 0x00]).unwrap();
    let o = tg(d.path(), &["check", "b.yaml", "--json"]);
    assert_eq!(code(&o), 2);
    assert_eq!(json(&o)["error"]["code"], "io");
}
