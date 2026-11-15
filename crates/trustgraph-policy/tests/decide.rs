use std::collections::BTreeSet;

use trustgraph_policy::decide::{Query, decide, run_tests};
use trustgraph_policy::lint::lint;
use trustgraph_policy::{Policy, Severity, Verdict, parse};

const SPEC: &str = include_str!("fixtures/spec-example.policy.yaml");

fn set<const N: usize>(items: [&str; N]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn spec() -> Policy {
    parse(SPEC).policy.expect("spec example parses")
}

fn query<const N: usize>(sources: [&str; N], agent: &str, action: &str) -> Query {
    Query {
        output_from: set(sources),
        agent: agent.to_string(),
        action: action.to_string(),
        recipients: None,
    }
}

#[test]
fn salary_summary_is_denied_for_slack() {
    let out = decide(
        &spec(),
        &query(["salaries", "roadmap"], "planner", "post_to_slack"),
    );
    assert_eq!(out.verdict, Verdict::Deny);
    assert_eq!(out.code, "audience_outside_readers");
    assert_eq!(out.blocking.len(), 1);
    assert_eq!(out.blocking[0].source, "salaries");
    assert_eq!(out.blocking[0].readers, set(["hr"]));
    assert_eq!(out.blocking[0].excluded, set(["ravi", "sam"]));
    assert_eq!(
        out.reason,
        "denied: `salaries` is readable by hr only; engineering (ravi, sam) is not included"
    );
    assert_eq!(out.readers, set(["nina"]));
}

#[test]
fn roadmap_summary_is_allowed() {
    let out = decide(&spec(), &query(["roadmap"], "planner", "post_to_slack"));
    assert_eq!(out.verdict, Verdict::Allow);
    assert_eq!(out.code, "allowed");
    assert_eq!(out.readers, set(["nina", "ravi", "sam"]));
    assert!(out.blocking.is_empty());
}

#[test]
fn price_sheet_email_needs_approval() {
    let out = decide(&spec(), &query(["price_sheet"], "mailer", "email_customer"));
    assert_eq!(out.verdict, Verdict::NeedsApproval);
    assert_eq!(out.code, "needs_approval");
}

#[test]
fn wrong_agent_is_denied() {
    let out = decide(&spec(), &query(["roadmap"], "mailer", "post_to_slack"));
    assert_eq!(out.verdict, Verdict::Deny);
    assert_eq!(out.code, "agent_not_listed_for_action");
    let out = decide(&spec(), &query(["price_sheet"], "planner", "post_to_slack"));
    assert_eq!(out.code, "agent_cannot_use_source");
}

#[test]
fn unknowns_are_denied() {
    let p = spec();
    let out = decide(&p, &query(["roadmap"], "planner", "nope"));
    assert_eq!((out.verdict, out.code), (Verdict::Deny, "unknown_action"));
    assert!(out.reason.contains("`nope`"), "{}", out.reason);
    let out = decide(&p, &query(["roadmap"], "ghost", "post_to_slack"));
    assert_eq!(out.code, "unknown_agent");
    assert!(out.reason.contains("`ghost`"), "{}", out.reason);
    let out = decide(&p, &query(["nothing"], "planner", "post_to_slack"));
    assert_eq!(out.code, "unknown_source");
    assert!(out.reason.contains("`nothing`"), "{}", out.reason);
    let out = decide(&p, &query([], "planner", "post_to_slack"));
    assert_eq!(out.code, "empty_output");
    assert!(out.readers.is_empty());
}

#[test]
fn explicit_recipients_must_be_in_audience() {
    let p = spec();
    let mut q = query(["roadmap"], "planner", "post_to_slack");
    q.recipients = Some(set(["nina"]));
    assert_eq!(decide(&p, &q).code, "recipients_outside_audience");
    q.recipients = Some(set(["sam"]));
    assert_eq!(decide(&p, &q).verdict, Verdict::Allow);
    // Unresolvable or empty recipient lists never pass vacuously.
    q.recipients = Some(set(["bogus"]));
    assert_eq!(decide(&p, &q).code, "recipients_outside_audience");
    q.recipients = Some(BTreeSet::new());
    assert_eq!(decide(&p, &q).code, "recipients_outside_audience");
}

#[test]
fn person_in_two_groups() {
    let mut p = spec();
    p.groups
        .get_mut("engineering")
        .unwrap()
        .insert("nina".into());
    let mut q = query(["salaries", "roadmap"], "planner", "post_to_slack");
    q.recipients = Some(set(["nina"]));
    assert_eq!(decide(&p, &q).verdict, Verdict::Allow);
}

#[test]
fn inline_tests_run() {
    let mut p = spec();
    let results = run_tests(&p);
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| r.pass), "{results:?}");
    p.tests[0].expect = Verdict::Allow;
    let results = run_tests(&p);
    assert_eq!(results.iter().filter(|r| !r.pass).count(), 1);
    assert!(!results[0].pass);
    assert_eq!(results[0].name, p.tests[0].name);
}

#[test]
fn lints_fire() {
    assert!(lint(&spec()).is_empty(), "{:?}", lint(&spec()));

    let find = |p: &Policy, code: &str, path: &str, sev: Severity| {
        let found = lint(p);
        assert!(
            found
                .iter()
                .any(|d| d.code == code && d.path == path && d.severity == sev),
            "expected {code} at {path}, got {found:?}"
        );
    };

    let mut p = spec();
    p.tests.clear();
    find(&p, "no_tests", "tests", Severity::Warning);

    let mut p = spec();
    p.sources.get_mut("salaries").unwrap().agents.clear();
    find(&p, "source_unusable", "sources.salaries", Severity::Warning);

    let mut p = spec();
    p.sources.get_mut("roadmap").unwrap().readers = set(["hr"]);
    find(
        &p,
        "action_unsatisfiable",
        "actions.post_to_slack",
        Severity::Warning,
    );

    let mut p = spec();
    p.actions
        .get_mut("email_customer")
        .unwrap()
        .approval
        .as_mut()
        .unwrap()
        .reviewers = set(["hr"]);
    find(
        &p,
        "reviewers_cannot_read",
        "actions.email_customer.approval.reviewers",
        Severity::Error,
    );

    let mut p = spec();
    p.groups.insert("ghost".into(), set(["zed"]));
    find(&p, "unused_group", "groups.ghost", Severity::Warning);

    let mut p = spec();
    p.agents.insert("idle".into());
    find(&p, "unused_agent", "agents.idle", Severity::Warning);
}

fn assert_audience_denied(p: &Policy) {
    let out = decide(p, &query(["roadmap"], "planner", "post_to_slack"));
    assert_eq!(out.verdict, Verdict::Deny, "{out:?}");
    assert_eq!(out.code, "recipients_outside_audience");
}

#[test]
fn audience_resolving_to_nobody_is_denied() {
    let mut p = spec();
    p.actions.get_mut("post_to_slack").unwrap().audience.clear();
    assert_audience_denied(&p);

    let mut p = spec();
    p.actions
        .get_mut("post_to_slack")
        .unwrap()
        .audience
        .insert("bogus".into());
    assert_audience_denied(&p);

    let mut p = spec();
    p.groups.get_mut("engineering").unwrap().clear();
    assert_audience_denied(&p);
}

#[test]
fn source_with_no_readers_never_allows() {
    let mut p = spec();
    p.sources.get_mut("roadmap").unwrap().readers.clear();
    let out = decide(&p, &query(["roadmap"], "planner", "post_to_slack"));
    assert_eq!(out.verdict, Verdict::Deny);
    assert_eq!(out.code, "audience_outside_readers");
}
