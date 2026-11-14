use trustgraph_sim::*;

#[test]
fn full_demo_contains_permission_block_stale_approval_and_one_exact_publication() {
    let trace = run(7, Scenario::Demo).unwrap();
    let output = serde_json::to_string(&trace).unwrap();
    assert!(output.contains("RESTRICTED_SOURCE"));
    assert!(output.contains("STALE_APPROVAL"));
    assert_eq!(trace.publications, 1);
    assert_eq!(trace.receipts.len(), 1);
    assert_eq!(trace.receipts[0].destination, "sim://northstar/company");
}
#[test]
fn seeded_races_cover_both_orders_without_unauthorized_publication() {
    for scenario in [Scenario::AclRace, Scenario::RevocationRace] {
        let mut results = std::collections::BTreeSet::new();
        for seed in 0..64 {
            let trace = run(seed, scenario).unwrap();
            assert_eq!(trace, run(seed, scenario).unwrap());
            let action_index = trace
                .steps
                .iter()
                .position(|s| matches!(s.action, Action::NarrowAcl | Action::Revoke))
                .unwrap();
            let publication_index = trace
                .steps
                .iter()
                .position(|s| matches!(s.action, Action::Publish { .. }))
                .unwrap();
            let expected = usize::from(publication_index < action_index);
            assert_eq!(
                trace.publications, expected,
                "seed {seed}, scenario {scenario:?}"
            );
            results.insert(expected);
        }
        assert_eq!(results, [0, 1].into());
    }
}
#[test]
fn crash_and_provider_recovery_observe_original_operation_only() {
    let timeout = run(1, Scenario::TimeoutRecovery).unwrap();
    assert!(
        serde_json::to_string(&timeout)
            .unwrap()
            .contains("OUTCOME_UNKNOWN")
    );
    assert_eq!(timeout.publications, 1);
    assert_eq!(timeout.receipts.len(), 1);
    let journal = run(1, Scenario::JournalRecovery).unwrap();
    assert_eq!(journal.publications, 0);
    assert!(
        serde_json::to_string(&journal)
            .unwrap()
            .contains("FAILED_CONFIRMED")
    );
}
#[test]
fn expired_approval_and_changed_audience_or_policy_cannot_publish() {
    for scenario in [
        Scenario::Expiry,
        Scenario::AudienceChange,
        Scenario::PolicyChange,
    ] {
        let trace = run(9, scenario).unwrap();
        assert_eq!(trace.publications, 0);
        assert!(trace.steps.len() >= 3);
    }
}
