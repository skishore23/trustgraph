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
    let d = diff(&spec(), &longer);
    assert!(d.widens());
    assert_eq!(d.changes.len(), 1);
    let back = diff(&longer, &spec());
    assert!(!back.widens());
    assert_eq!(back.changes.len(), 1);
    assert_eq!(back.changes[0].kind, ChangeKind::Narrows);

    let mut added = spec();
    added.actions.get_mut("post_to_slack").unwrap().approval =
        Some(trustgraph_policy::ApprovalRule {
            reviewers: set(["hr"]),
            valid_for_seconds: 60,
        });
    let d = diff(&spec(), &added);
    assert!(!d.widens());
    assert_eq!(d.changes.len(), 1);
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
    assert_eq!(d.changes[1].gained, set(["nina", "ravi", "sam"]));
    assert!(
        d.changes[1].summary.contains("agents planner"),
        "{}",
        d.changes[1].summary
    );
    assert!(
        d.changes[0].summary.contains("agents planner"),
        "{}",
        d.changes[0].summary
    );
}

fn only(d: &trustgraph_policy::Diff) -> &trustgraph_policy::Change {
    assert_eq!(d.changes.len(), 1, "{:?}", d.changes);
    &d.changes[0]
}

fn check(c: &trustgraph_policy::Change, kind: ChangeKind, subject: &str, g: &[&str], l: &[&str]) {
    assert_eq!(c.kind, kind, "{c:?}");
    assert_eq!(c.subject, subject, "{c:?}");
    assert_eq!(c.gained, g.iter().map(|s| s.to_string()).collect(), "{c:?}");
    assert_eq!(c.lost, l.iter().map(|s| s.to_string()).collect(), "{c:?}");
}

#[test]
fn source_agent_gained_and_lost() {
    let mut new = spec();
    new.sources
        .get_mut("salaries")
        .unwrap()
        .agents
        .insert("mailer".into());
    let d = diff(&spec(), &new);
    check(
        only(&d),
        ChangeKind::Widens,
        "source salaries",
        &["mailer"],
        &[],
    );
    assert!(d.widens());

    let mut new = spec();
    new.sources
        .get_mut("roadmap")
        .unwrap()
        .agents
        .remove("planner");
    let d = diff(&spec(), &new);
    check(
        only(&d),
        ChangeKind::Narrows,
        "source roadmap",
        &[],
        &["planner"],
    );
    assert!(!d.widens());
}

#[test]
fn action_agent_gained_and_lost() {
    let mut new = spec();
    new.actions
        .get_mut("post_to_slack")
        .unwrap()
        .agents
        .insert("mailer".into());
    let d = diff(&spec(), &new);
    check(
        only(&d),
        ChangeKind::Widens,
        "action post_to_slack",
        &["mailer"],
        &[],
    );

    let mut new = spec();
    new.actions
        .get_mut("email_customer")
        .unwrap()
        .agents
        .clear();
    let d = diff(&spec(), &new);
    check(
        only(&d),
        ChangeKind::Narrows,
        "action email_customer",
        &[],
        &["mailer"],
    );
}

#[test]
fn reviewer_swap_widens_and_narrows() {
    let mut new = spec();
    let rule = new
        .actions
        .get_mut("email_customer")
        .unwrap()
        .approval
        .as_mut()
        .unwrap();
    rule.reviewers = set(["hr"]);
    let d = diff(&spec(), &new);
    assert!(d.widens());
    assert_eq!(d.changes.len(), 2, "{:?}", d.changes);
    check(
        &d.changes[0],
        ChangeKind::Widens,
        "action email_customer",
        &["nina"],
        &[],
    );
    check(
        &d.changes[1],
        ChangeKind::Narrows,
        "action email_customer",
        &[],
        &["leo"],
    );
}

#[test]
fn reviewer_group_membership_change() {
    // ravi joins sales: he may now read price_sheet and approve email_customer.
    let mut new = spec();
    new.groups.get_mut("sales").unwrap().insert("ravi".into());
    let d = diff(&spec(), &new);
    assert_eq!(d.changes.len(), 2, "{:?}", d.changes);
    check(
        &d.changes[0],
        ChangeKind::Widens,
        "source price_sheet",
        &["ravi"],
        &[],
    );
    check(
        &d.changes[1],
        ChangeKind::Widens,
        "action email_customer",
        &["ravi"],
        &[],
    );
    assert!(
        d.changes[1].summary.contains("approval reviewers"),
        "{}",
        d.changes[1].summary
    );
}

#[test]
fn group_change_reaches_action_audience() {
    // leo joins engineering: roadmap readers and the slack audience grow.
    let mut new = spec();
    new.groups
        .get_mut("engineering")
        .unwrap()
        .insert("leo".into());
    let d = diff(&spec(), &new);
    assert_eq!(d.changes.len(), 2, "{:?}", d.changes);
    check(
        &d.changes[0],
        ChangeKind::Widens,
        "source roadmap",
        &["leo"],
        &[],
    );
    check(
        &d.changes[1],
        ChangeKind::Widens,
        "action post_to_slack",
        &["leo"],
        &[],
    );
}

#[test]
fn person_moved_between_groups() {
    // ravi leaves engineering for hr: salaries gains him, slack loses him,
    // roadmap (hr + engineering) is unchanged.
    let mut new = spec();
    new.groups.get_mut("engineering").unwrap().remove("ravi");
    new.groups.get_mut("hr").unwrap().insert("ravi".into());
    let d = diff(&spec(), &new);
    assert!(d.widens());
    assert_eq!(d.changes.len(), 2, "{:?}", d.changes);
    check(
        &d.changes[0],
        ChangeKind::Widens,
        "source salaries",
        &["ravi"],
        &[],
    );
    check(
        &d.changes[1],
        ChangeKind::Narrows,
        "action post_to_slack",
        &[],
        &["ravi"],
    );
}

#[test]
fn readers_swap_gives_widens_then_narrows() {
    let mut new = spec();
    let s = new.sources.get_mut("salaries").unwrap();
    s.readers = set(["engineering"]);
    let d = diff(&spec(), &new);
    assert_eq!(d.changes.len(), 2, "{:?}", d.changes);
    check(
        &d.changes[0],
        ChangeKind::Widens,
        "source salaries",
        &["ravi", "sam"],
        &[],
    );
    check(
        &d.changes[1],
        ChangeKind::Narrows,
        "source salaries",
        &[],
        &["nina"],
    );
}

#[test]
fn added_action_summary_names_agents_and_approval() {
    let mut new = spec();
    new.actions.insert(
        "notify".into(),
        trustgraph_policy::Action {
            agents: set(["planner"]),
            audience: set(["sales"]),
            approval: Some(trustgraph_policy::ApprovalRule {
                reviewers: set(["hr"]),
                valid_for_seconds: 60,
            }),
        },
    );
    new.actions.insert(
        "open_notify".into(),
        trustgraph_policy::Action {
            agents: set(["mailer", "planner"]),
            audience: set(["sales"]),
            approval: None,
        },
    );
    let d = diff(&spec(), &new);
    assert_eq!(d.changes.len(), 2, "{:?}", d.changes);
    // "notify" sorts before "open_notify"
    check(
        &d.changes[0],
        ChangeKind::Widens,
        "action notify",
        &["leo"],
        &[],
    );
    assert!(
        d.changes[0].summary.contains("agents planner"),
        "{}",
        d.changes[0].summary
    );
    assert!(
        d.changes[0].summary.contains("approval by nina, valid 1m"),
        "{}",
        d.changes[0].summary
    );
    check(
        &d.changes[1],
        ChangeKind::Widens,
        "action open_notify",
        &["leo"],
        &[],
    );
    assert!(
        d.changes[1].summary.contains("agents mailer, planner"),
        "{}",
        d.changes[1].summary
    );
    assert!(
        d.changes[1].summary.contains("no approval"),
        "{}",
        d.changes[1].summary
    );
}

#[test]
fn removed_action_summary_names_agents_and_approval() {
    let mut new = spec();
    new.actions.remove("email_customer");
    let d = diff(&spec(), &new);
    check(
        only(&d),
        ChangeKind::Narrows,
        "action email_customer",
        &[],
        &["acme"],
    );
    let s = &d.changes[0].summary;
    assert!(s.contains("agents mailer"), "{s}");
    assert!(s.contains("approval by leo, valid 30m"), "{s}");
}

#[test]
fn added_source_summary_names_agents() {
    let mut new = spec();
    new.sources.insert(
        "notes".into(),
        trustgraph_policy::Source {
            readers: set(["sales"]),
            agents: set(["mailer", "planner"]),
        },
    );
    let d = diff(&spec(), &new);
    check(only(&d), ChangeKind::Widens, "source notes", &["leo"], &[]);
    let s = &d.changes[0].summary;
    assert!(s.contains("leo"), "{s}");
    assert!(s.contains("agents mailer, planner"), "{s}");
}

#[test]
fn group_only_edit_is_neutral() {
    let mut new = spec();
    new.groups.insert("contractors".into(), set(["zed"]));
    let d = diff(&spec(), &new);
    check(only(&d), ChangeKind::Neutral, "groups", &[], &[]);
    assert!(!d.widens());
    assert!(!d.is_empty());
}
