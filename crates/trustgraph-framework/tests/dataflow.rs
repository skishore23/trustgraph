use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use trustgraph_framework::*;

fn readers(names: &[&str]) -> Readers {
    names.iter().map(|name| PrincipalId::from(*name)).collect()
}
fn ports(names: &[&str]) -> Ports {
    names
        .iter()
        .map(|name| (name.to_string(), SchemaId::from("text/v1")))
        .collect()
}
fn operation(graph: &mut Graph, inputs: &[&str], outputs: &[&str]) -> OperationId {
    graph
        .register_operation(OperationDefinition {
            name: "transform".into(),
            version: "1".into(),
            executor: "executor".into(),
            inputs: ports(inputs),
            outputs: ports(outputs),
        })
        .unwrap()
}
fn source(graph: &mut Graph, name: &str, allowed: &[&str]) -> ArtifactId {
    graph
        .register_source(
            name,
            "text/v1".into(),
            name.as_bytes().to_vec(),
            readers(allowed),
        )
        .unwrap()
}
fn run(
    graph: &mut Graph,
    op: &OperationId,
    inputs: &[(&str, &ArtifactId)],
    outputs: &[&str],
) -> BTreeMap<String, ArtifactId> {
    let inputs = inputs
        .iter()
        .map(|(port, artifact)| (port.to_string(), (*artifact).clone()))
        .collect();
    let ticket = graph
        .begin_execution(op, inputs, "actor".into(), json!({}))
        .unwrap();
    graph
        .finish_execution(
            ticket,
            outputs
                .iter()
                .map(|port| {
                    (
                        port.to_string(),
                        OutputValue {
                            logical_id: port.to_string(),
                            bytes: b"result".to_vec(),
                        },
                    )
                })
                .collect(),
            "trusted host receipt",
        )
        .unwrap()
}
fn intent(artifact: &ArtifactId) -> ActionIntent {
    ActionIntent {
        actor: "actor".into(),
        action: "send".into(),
        artifact: artifact.clone(),
        destination: "internal".into(),
        recipients: readers(&["reviewer"]),
        parameters: json!({"subject":"hello"}),
    }
}
fn rule(graph: &mut Graph, approve: bool) {
    graph
        .set_action_rule(
            "send",
            ActionRule {
                actors: readers(&["actor"]),
                approval: approve.then(|| ApprovalRequirement {
                    reviewers: readers(&["reviewer"]),
                    ttl_seconds: 60,
                }),
            },
        )
        .unwrap();
}
const ALL: &[&str] = &["actor", "executor", "reviewer", "sales"];
const RESTRICTED: &[&str] = &["actor", "executor", "reviewer"];

#[test]
fn restrictions_follow_multiple_steps_branch_merge_and_repeated_inputs() {
    let mut g = Graph::new();
    let broad = source(&mut g, "broad", ALL);
    let narrow = source(&mut g, "narrow", RESTRICTED);
    let op = operation(&mut g, &["a", "b"], &["quote", "audit"]);
    let first = run(
        &mut g,
        &op,
        &[("a", &broad), ("b", &narrow)],
        &["quote", "audit"],
    );
    for output in first.values() {
        assert_eq!(g.readers(output).unwrap(), readers(RESTRICTED));
    }
    let second = run(
        &mut g,
        &op,
        &[("a", &first["quote"]), ("b", &first["quote"])],
        &["quote", "audit"],
    );
    assert_eq!(g.readers(&second["quote"]).unwrap(), readers(RESTRICTED));
    assert!(
        g.executions()
            .values()
            .any(|e| e.inputs.len() == 2 && e.inputs["a"] == e.inputs["b"])
    );
    let impacted = g.impact(&narrow).unwrap();
    assert!(!impacted.contains(&broad));
    assert!(impacted.contains(&second["quote"]));
    assert!(impacted.contains(&first["audit"]));
}

#[test]
fn input_types_ports_and_disclosure_are_checked_before_execution() {
    let mut g = Graph::new();
    let op = operation(&mut g, &["document"], &["summary"]);
    let secret = source(&mut g, "secret", &["actor"]);
    assert!(matches!(
        g.begin_execution(
            &op,
            BTreeMap::from([("document".into(), secret)]),
            "actor".into(),
            json!({})
        ),
        Err(Error::Permission(_))
    ));
    let wrong = g
        .register_source("image", "image/v1".into(), vec![1, 2], readers(ALL))
        .unwrap();
    assert!(matches!(
        g.begin_execution(
            &op,
            BTreeMap::from([("document".into(), wrong)]),
            "actor".into(),
            json!({})
        ),
        Err(Error::Contract(_))
    ));
    assert!(matches!(
        g.begin_execution(&op, BTreeMap::new(), "actor".into(), json!({})),
        Err(Error::Contract(_))
    ));
    assert!(g.executions().is_empty());
}

#[test]
fn finish_rejects_stale_permissions_and_commits_no_partial_output() {
    let mut g = Graph::new();
    let input = source(&mut g, "input", ALL);
    let op = operation(&mut g, &["in"], &["one", "two"]);
    let ticket = g
        .begin_execution(
            &op,
            BTreeMap::from([("in".into(), input.clone())]),
            "actor".into(),
            json!({}),
        )
        .unwrap();
    g.update_source_permissions(&input, readers(RESTRICTED))
        .unwrap();
    let values = || {
        BTreeMap::from([
            (
                "one".into(),
                OutputValue {
                    logical_id: "one".into(),
                    bytes: vec![],
                },
            ),
            (
                "two".into(),
                OutputValue {
                    logical_id: "two".into(),
                    bytes: vec![],
                },
            ),
        ])
    };
    assert!(matches!(
        g.finish_execution(ticket, values(), "receipt"),
        Err(Error::Stale(_))
    ));
    assert_eq!(g.artifacts().len(), 1);
    let ticket = g
        .begin_execution(
            &op,
            BTreeMap::from([("in".into(), input)]),
            "actor".into(),
            json!({}),
        )
        .unwrap();
    let mut bad = values();
    bad.get_mut("two").unwrap().logical_id.clear();
    assert!(g.finish_execution(ticket, bad, "receipt").is_err());
    assert_eq!(g.artifacts().len(), 1);
    assert!(g.executions().is_empty());
}

#[test]
fn approvals_bind_exact_action_context_and_do_not_widen_readers() {
    let mut g = Graph::new();
    let artifact = source(&mut g, "record", RESTRICTED);
    rule(&mut g, true);
    let original = intent(&artifact);
    assert!(matches!(
        g.check_action(&original, None, 10),
        Decision::NeedsApproval { .. }
    ));
    let approval = g.approve(&original, "reviewer".into(), 10).unwrap();
    assert!(matches!(
        g.check_action(&original, Some(&approval), 11),
        Decision::Allow { .. }
    ));
    let mut changed = original.clone();
    changed.parameters = json!({"subject":"changed"});
    assert!(matches!(
        g.check_action(&changed, Some(&approval), 11),
        Decision::Deny { .. }
    ));
    changed = original.clone();
    changed.recipients = readers(&["sales"]);
    assert!(g.approve(&changed, "reviewer".into(), 11).is_err());
    assert!(matches!(
        g.check_action(&changed, Some(&approval), 11),
        Decision::Deny { .. }
    ));
    changed = original.clone();
    changed.destination = "other-internal".into();
    assert!(matches!(
        g.check_action(&changed, Some(&approval), 11),
        Decision::Deny { .. }
    ));
    assert!(matches!(
        g.check_action(&original, Some(&approval), 70),
        Decision::Deny { .. }
    ));
    assert!(matches!(
        g.check_action(&original, Some(&approval), 9),
        Decision::Deny { .. }
    ));
    assert!(g.revoke_approval(&approval));
    assert!(matches!(
        g.check_action(&original, Some(&approval), 11),
        Decision::Deny { .. }
    ));
}

#[test]
fn only_relevant_source_changes_invalidate_approval() {
    let mut g = Graph::new();
    let relevant = source(&mut g, "relevant", ALL);
    let unrelated = source(&mut g, "unrelated", ALL);
    let op = operation(&mut g, &["in"], &["out"]);
    let output = run(&mut g, &op, &[("in", &relevant)], &["out"])["out"].clone();
    rule(&mut g, true);
    let request = intent(&output);
    let approval = g.approve(&request, "reviewer".into(), 1).unwrap();
    g.update_source_permissions(&unrelated, readers(RESTRICTED))
        .unwrap();
    assert!(matches!(
        g.check_action(&request, Some(&approval), 2),
        Decision::Allow { .. }
    ));
    g.set_action_rule(
        "export",
        ActionRule {
            actors: readers(&["actor"]),
            approval: None,
        },
    )
    .unwrap();
    assert!(matches!(
        g.check_action(&request, Some(&approval), 2),
        Decision::Allow { .. }
    ));
    g.update_source_permissions(&relevant, readers(RESTRICTED))
        .unwrap();
    assert!(matches!(
        g.check_action(&request, Some(&approval), 2),
        Decision::Deny { .. }
    ));
    // Even changing back is a new authorization revision.
    g.update_source_permissions(&relevant, readers(ALL))
        .unwrap();
    assert!(matches!(
        g.check_action(&request, Some(&approval), 2),
        Decision::Deny { .. }
    ));
}

#[test]
fn source_versions_are_immutable_and_new_bytes_do_not_rewrite_history() {
    let mut g = Graph::new();
    let old = source(&mut g, "source", ALL);
    let new = g
        .register_source(
            "source",
            "text/v1".into(),
            b"new bytes".to_vec(),
            readers(ALL),
        )
        .unwrap();
    assert_ne!(old, new);
    assert_eq!(g.artifacts()[&old].bytes, b"source");
    assert!(
        g.register_source(
            "source",
            "text/v1".into(),
            b"source".to_vec(),
            readers(RESTRICTED)
        )
        .is_err()
    );
}

#[test]
fn approval_is_optional_by_rule_and_unknown_actions_or_audiences_fail_closed() {
    let mut g = Graph::new();
    let artifact = source(&mut g, "record", ALL);
    let mut request = intent(&artifact);
    assert!(matches!(
        g.check_action(&request, None, 0),
        Decision::Deny { .. }
    ));
    rule(&mut g, false);
    assert!(matches!(
        g.check_action(&request, None, 0),
        Decision::Allow { .. }
    ));
    request.recipients = BTreeSet::new();
    assert!(matches!(
        g.check_action(&request, None, 0),
        Decision::Deny { .. }
    ));
}

#[test]
fn approval_requires_independent_authorized_reader_and_rule_changes_invalidate() {
    let mut g = Graph::new();
    let artifact = source(&mut g, "record", ALL);
    rule(&mut g, true);
    let request = intent(&artifact);
    assert!(g.approve(&request, "stranger".into(), 0).is_err());
    let approval = g.approve(&request, "reviewer".into(), 0).unwrap();
    g.set_action_rule(
        "send",
        ActionRule {
            actors: readers(&["actor"]),
            approval: Some(ApprovalRequirement {
                reviewers: readers(&["reviewer", "actor"]),
                ttl_seconds: 61,
            }),
        },
    )
    .unwrap();
    assert!(g.approve(&request, "actor".into(), 1).is_err());
    assert!(matches!(
        g.check_action(&request, Some(&approval), 1),
        Decision::Deny { .. }
    ));
}

#[test]
fn decision_is_reproducible_for_same_state_intent_and_time() {
    let mut g = Graph::new();
    let artifact = source(&mut g, "record", ALL);
    rule(&mut g, false);
    let request = intent(&artifact);
    assert_eq!(
        g.check_action(&request, None, 100),
        g.check_action(&request, None, 100)
    );
}

#[test]
fn ticket_cannot_finish_or_cancel_other_graphs_execution() {
    fn prepare() -> (Graph, ExecutionTicket) {
        let mut graph = Graph::new();
        let input = source(&mut graph, "source", ALL);
        let op = operation(&mut graph, &["in"], &["out"]);
        let ticket = graph
            .begin_execution(
                &op,
                BTreeMap::from([("in".into(), input)]),
                "actor".into(),
                json!({}),
            )
            .unwrap();
        (graph, ticket)
    }
    let (_a, ticket_a) = prepare();
    let (mut b, ticket_b) = prepare();
    assert!(matches!(
        b.cancel_execution(ticket_a),
        Err(Error::Unknown(_))
    ));
    assert!(b.cancel_execution(ticket_b).is_ok());
    let (_a, ticket_a) = prepare();
    let (mut b, ticket_b) = prepare();
    let values = || {
        BTreeMap::from([(
            "out".into(),
            OutputValue {
                logical_id: "out".into(),
                bytes: vec![],
            },
        )])
    };
    assert!(matches!(
        b.finish_execution(ticket_a, values(), "receipt"),
        Err(Error::Unknown(_))
    ));
    assert!(b.finish_execution(ticket_b, values(), "receipt").is_ok());
}

#[test]
fn operation_name_and_version_cannot_be_rebound() {
    let mut graph = Graph::new();
    let id = operation(&mut graph, &["in"], &["out"]);
    let original = graph.operations()[&id].clone();
    assert_eq!(id, graph.register_operation(original.clone()).unwrap());
    let mut changed = original;
    changed.executor = "another-provider".into();
    assert!(graph.register_operation(changed.clone()).is_err());
    changed.version = "2".into();
    assert_ne!(id, graph.register_operation(changed).unwrap());
}

#[test]
fn graph_rejects_too_deep_composition_before_execution() {
    let mut graph = Graph::new();
    let mut current = source(&mut graph, "source", ALL);
    let op = operation(&mut graph, &["in"], &["out"]);
    for _ in 0..64 {
        current = run(&mut graph, &op, &[("in", &current)], &["out"])["out"].clone();
    }
    assert!(graph.readers(&current).is_ok());
    assert!(matches!(
        graph.begin_execution(
            &op,
            BTreeMap::from([("in".into(), current)]),
            "actor".into(),
            json!({})
        ),
        Err(Error::Lineage(_))
    ));
}
