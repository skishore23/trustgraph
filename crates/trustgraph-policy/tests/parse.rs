use std::collections::BTreeSet;
use std::time::Instant;

use trustgraph_policy::{Diagnostic, Severity, parse};

const SPEC: &str = include_str!("fixtures/spec-example.policy.yaml");

fn set<const N: usize>(items: [&str; N]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn errors(yaml: &str) -> Vec<Diagnostic> {
    parse(yaml)
        .diagnostics
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .collect()
}

fn assert_single(yaml: &str, code: &str, path: &str) {
    let errs = errors(yaml);
    assert!(
        errs.iter().any(|d| d.code == code && d.path == path),
        "expected {code} at {path:?}, got {errs:?}"
    );
    for d in &errs {
        assert!(!d.message.contains("serde"), "leaks serde: {}", d.message);
    }
}

#[test]
fn spec_example_parses() {
    let parsed = parse(SPEC);
    assert_eq!(parsed.diagnostics, vec![]);
    let policy = parsed.policy.expect("policy");
    assert_eq!(
        policy.people(&set(["hr", "engineering"])),
        set(["nina", "sam", "ravi"])
    );
    assert_eq!(
        policy.actions["email_customer"]
            .approval
            .as_ref()
            .unwrap()
            .valid_for_seconds,
        1800
    );
    assert_eq!(policy.tests.len(), 2);
}

#[test]
fn all_errors_reported_at_once() {
    let yaml = r#"
version: 1
groups: {hr: [nina], engineering: [sam]}
agents: [planner, mailer]
sorces: {}
sources:
  salaries: {readers: [hrr], agents: [planner]}
actions:
  email_customer:
    agents: [mailer]
    audience: [engineering]
    approval: {reviewers: [hr], valid_for: 30x}
"#;
    let errs = errors(yaml);
    let mut got: Vec<_> = errs.iter().map(|d| (d.code, d.path.as_str())).collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            (
                "invalid_duration",
                "actions.email_customer.approval.valid_for"
            ),
            ("unknown_field", "sorces"),
            ("unknown_name", "sources.salaries.readers[0]"),
        ]
    );
    assert!(parse(yaml).policy.is_none());
}

#[test]
fn suggests_close_names() {
    let yaml = r#"
version: 1
groups: {engineering: [sam]}
agents: [planner]
sources:
  s: {readers: [enginering], agents: [planner]}
"#;
    let errs = errors(yaml);
    assert_eq!(errs.len(), 1);
    assert!(
        errs[0]
            .suggestion
            .as_deref()
            .unwrap()
            .contains("engineering")
    );
}

#[test]
fn oversized_input_is_rejected_fast() {
    let big = "a".repeat(10 * 1024 * 1024);
    let start = Instant::now();
    let errs = errors(&big);
    assert!(start.elapsed().as_secs() < 1);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].code, "too_large");
}

#[test]
fn alias_bomb_does_not_hang() {
    let mut yaml = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
    for i in 1..9 {
        let p = i - 1;
        let refs = vec![format!("*a{p}"); 10].join(", ");
        yaml.push_str(&format!("a{i}: &a{i} [{refs}]\n"));
    }
    let start = Instant::now();
    let parsed = parse(&yaml);
    assert!(start.elapsed().as_secs() < 1, "took {:?}", start.elapsed());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    );
    assert!(parsed.policy.is_none());
}

#[test]
fn wrong_types_have_paths() {
    let base = "version: 1\ngroups: {hr: [nina]}\nagents: [planner]\n";
    assert_single(
        &format!("{base}sources:\n  s: {{readers: 5}}\n"),
        "wrong_type",
        "sources.s.readers",
    );
    assert_single(
        "version: 1\ngroups: {hr: null}\n",
        "wrong_type",
        "groups.hr",
    );
    // YAML 1.2: bare `on` is a string, so a boolean key needs `true`.
    assert_single(
        "version: 1\ngroups: {true: [a]}\n",
        "wrong_type",
        "groups.true",
    );
    assert_single(
        "version: 1\ngroups: {on: [a]}\nagents: [true]\n",
        "wrong_type",
        "agents[0]",
    );
    let dup = parse("version: 1\nsources: {}\nsources: {}\n");
    assert!(dup.policy.is_none());
    let d = &dup.diagnostics[0];
    assert_eq!(d.severity, Severity::Error);
    assert!(d.code == "duplicate_key" || d.code == "yaml_syntax");
    assert_eq!(d.path, "sources");
    assert!(!d.message.contains("serde"));
}

#[test]
fn names_are_strict() {
    for bad in ["HR", "two words", "münchen"] {
        let yaml = format!("version: 1\ngroups:\n  \"{bad}\": [nina]\n");
        assert_single(&yaml, "invalid_name", &format!("groups.{bad}"));
    }
    assert_single(
        "version: 1\ngroups: {planner: [nina]}\nagents: [planner]\n",
        "name_collision",
        "agents[0]",
    );
    assert_single(
        "version: 1\ngroups: {hr: [nina]}\nagents: [nina]\n",
        "name_collision",
        "agents[0]",
    );
    assert_single("version: 1\ngroups: {hr: []}\n", "empty_group", "groups.hr");
}

#[test]
fn version_and_sets_and_tests_are_checked() {
    assert_single("version: 2\n", "unsupported_version", "version");
    let base = "version: 1\ngroups: {hr: [nina]}\nagents: [planner]\n";
    assert_single(
        &format!("{base}sources:\n  s: {{readers: []}}\n"),
        "empty_set",
        "sources.s.readers",
    );
    for bad in ["0s", "31d", "5", "m", "-1m", "99999999999999999999d"] {
        let yaml = format!(
            "{base}actions:\n  a:\n    agents: [planner]\n    audience: [hr]\n    approval: {{reviewers: [hr], valid_for: \"{bad}\"}}\n"
        );
        assert_single(&yaml, "invalid_duration", "actions.a.approval.valid_for");
    }
    let ok = format!(
        "{base}actions:\n  a:\n    agents: [planner]\n    audience: [hr]\n    approval: {{reviewers: [hr], valid_for: 30d}}\n"
    );
    assert!(errors(&ok).is_empty());
    let tests = format!(
        "{base}sources:\n  s: {{readers: [hr]}}\nactions:\n  a: {{agents: [planner], audience: [hr]}}\ntests:\n  - {{name: t, output_from: [], agent: nobody, action: nope, recipients: [zzz], expect: deny}}\n  - {{name: t, output_from: [s], agent: planner, action: a, expect: allow}}\n"
    );
    let errs = errors(&tests);
    let mut got: Vec<_> = errs.iter().map(|d| (d.code, d.path.as_str())).collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            ("duplicate_test", "tests[1].name"),
            ("empty_set", "tests[0].output_from"),
            ("unknown_name", "tests[0].action"),
            ("unknown_name", "tests[0].agent"),
            ("unknown_name", "tests[0].recipients[0]"),
        ]
    );
}

#[test]
fn syntax_errors_are_reported() {
    let errs = errors("version: [1\n");
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].code, "yaml_syntax");
    assert!(!errs[0].message.contains("serde"));
}

#[test]
fn digest_is_stable_across_key_order() {
    let a = parse("version: 1\ngroups: {hr: [nina], eng: [sam]}\nagents: [b, a]\n")
        .policy
        .unwrap();
    let b = parse("agents: [a, b]\ngroups: {eng: [sam], hr: [nina]}\nversion: 1\n")
        .policy
        .unwrap();
    assert_eq!(a.digest(), b.digest());
    let c = parse("version: 1\ngroups: {hr: [nina]}\nagents: [a, b]\n")
        .policy
        .unwrap();
    assert_ne!(a.digest(), c.digest());
}

#[test]
fn to_yaml_round_trips() {
    let p = parse(SPEC).policy.unwrap();
    let again = parse(&p.to_yaml());
    assert_eq!(again.diagnostics, vec![]);
    assert_eq!(again.policy, Some(p));
}
