use trustgraph_sim::checker::*;
#[test]
fn production_gate_passes_every_reachable_state_within_declared_bounds() {
    let report = check(6, Rule::Production).unwrap();
    assert!(report.passed);
    assert!(report.complete_within_bounds);
    assert!(report.states > 30);
    assert!(report.transitions > 100);
    assert!(report.publications > 0);
    assert_eq!(report.properties.len(), 4);
    assert!(report.counterexample.is_none());
    assert_eq!(report, check(6, Rule::Production).unwrap());
}
#[test]
fn deliberate_approval_reuse_produces_a_shortest_replayable_counterexample() {
    let report = check(6, Rule::ReuseApprovalMutant).unwrap();
    assert!(!report.passed);
    let trace = report.counterexample.unwrap();
    assert!(trace.len() <= 4);
    assert_eq!(trace.last().unwrap().action, Action::Publish);
    assert!(trace.last().unwrap().decision.contains("VIOLATION"));
    assert!(
        trace
            .iter()
            .any(|s| matches!(s.action, Action::Edit | Action::GenerateMixed))
    );
    assert!(check(0, Rule::Production).is_err());
    assert!(check(20, Rule::Production).is_err());
}
