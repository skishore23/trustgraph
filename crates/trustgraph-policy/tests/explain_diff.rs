use std::collections::BTreeSet;

use trustgraph_policy::{ChangeKind, Policy, diff, explain, parse};

const SPEC: &str = include_str!("fixtures/spec-example.policy.yaml");

fn set<const N: usize>(items: [&str; N]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn spec() -> Policy {
    parse(SPEC).policy.expect("spec example parses")
}

#[test]
fn explain_spec_example() {
    let text = explain(&spec());
    assert!(
        text.contains("salaries: output can reach hr (nina). Usable by agent planner.\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "email_customer: mailer may send to customers (acme) after approval by sales (leo), valid 30m.\n"
        ),
        "{text}"
    );
    assert!(text.ends_with('\n'));
}

#[test]
fn explain_without_approval_or_agents() {
    let mut p = spec();
    p.sources.get_mut("roadmap").unwrap().agents.clear();
    let text = explain(&p);
    assert!(
        text.contains(
            "roadmap: output can reach engineering (ravi, sam), hr (nina). Usable by no agent.\n"
        ),
        "{text}"
    );
    assert!(
        text.contains("post_to_slack: planner may send to engineering (ravi, sam).\n"),
        "{text}"
    );
}

#[test]
fn adding_a_reader_widens() {
    let mut new = spec();
    new.sources
        .get_mut("salaries")
        .unwrap()
        .readers
        .insert("engineering".into());
    let d = diff(&spec(), &new);
    assert!(d.widens());
    assert_eq!(d.changes.len(), 1);
    let c = &d.changes[0];
    assert_eq!(c.kind, ChangeKind::Widens);
    assert_eq!(c.subject, "source salaries");
    assert_eq!(c.gained, set(["ravi", "sam"]));
    assert!(c.lost.is_empty());
    assert!(
        c.summary.contains("readers gained ravi, sam"),
        "{}",
        c.summary
    );
}

#[test]
fn removing_approval_widens() {
    let mut new = spec();
    new.actions.get_mut("email_customer").unwrap().approval = None;
    let d = diff(&spec(), &new);
    assert!(d.widens());
    assert_eq!(d.changes.len(), 1);
    assert_eq!(d.changes[0].kind, ChangeKind::Widens);
    assert_eq!(d.changes[0].subject, "action email_customer");
}

#[test]
fn group_membership_change_propagates() {
    let mut new = spec();
    new.groups.get_mut("hr").unwrap().insert("ravi".into());
    let d = diff(&spec(), &new);
    assert!(d.widens());
    let c = d
        .changes
        .iter()
        .find(|c| c.subject == "source salaries")
        .expect("salaries change");
    assert_eq!(c.kind, ChangeKind::Widens);
    assert_eq!(c.gained, set(["ravi"]));
}

#[test]
fn narrowing_does_not_widen() {
    let mut new = spec();
    new.sources
        .get_mut("roadmap")
        .unwrap()
        .readers
        .remove("engineering");
    let d = diff(&spec(), &new);
    assert!(!d.widens());
    assert!(!d.is_empty());
    assert_eq!(d.changes.len(), 1);
    assert_eq!(d.changes[0].kind, ChangeKind::Narrows);
    assert_eq!(d.changes[0].lost, set(["ravi", "sam"]));
}

#[test]
fn identical_is_empty() {
    let d = diff(&spec(), &spec());
    assert!(d.is_empty());
    assert!(!d.widens());
}

#[test]
fn test_only_edit_is_neutral() {
    let mut new = spec();
    new.tests.pop();
    let d = diff(&spec(), &new);
    assert!(!d.widens());
    assert_eq!(d.changes.len(), 1);
    assert_eq!(d.changes[0].kind, ChangeKind::Neutral);
    assert_eq!(d.changes[0].subject, "tests");
}

#[test]
fn approval_rule_changes() {
    let mut longer = spec();
    longer
        .actions
        .get_mut("email_customer")
        .unwrap()
        .approval
        .as_mut()
        .unwrap()
        .valid_for_seconds = 3600;
    assert!(diff(&spec(), &longer).widens());
    let back = diff(&longer, &spec());
    assert!(!back.widens());
    assert_eq!(back.changes[0].kind, ChangeKind::Narrows);

    let mut added = spec();
    added.actions.get_mut("post_to_slack").unwrap().approval =
        Some(trustgraph_policy::ApprovalRule {
            reviewers: set(["hr"]),
            valid_for_seconds: 60,
        });
    let d = diff(&spec(), &added);
    assert!(!d.widens());
    assert_eq!(d.changes[0].kind, ChangeKind::Narrows);
}

#[test]
fn added_and_removed_sources() {
    let mut new = spec();
    let s = new.sources.remove("roadmap").unwrap();
    new.sources.insert("zeta".into(), s);
    let d = diff(&spec(), &new);
    assert!(d.widens());
    assert_eq!(d.changes.len(), 2);
    assert_eq!(d.changes[0].subject, "source roadmap");
    assert_eq!(d.changes[0].kind, ChangeKind::Narrows);
    assert_eq!(d.changes[1].subject, "source zeta");
    assert_eq!(d.changes[1].kind, ChangeKind::Widens);
}
