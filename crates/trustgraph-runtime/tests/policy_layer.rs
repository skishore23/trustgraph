use trustgraph_runtime::{journal::MemoryJournal, policy::*, publisher::LocalPublisher, *};
const YAML: &str = include_str!("../../../examples/bank-policy.yaml");

#[test]
fn policy_versions_are_explicit_atomic_and_cannot_expand_source_authority() {
    let mut w = Coordinator::open_with_evidence(
        MemoryJournal::default(),
        LocalPublisher::default(),
        fixtures::bank(),
    )
    .unwrap();
    let version = w
        .propose_policy(
            &"agent".into(),
            &format!("{YAML}\n# Untracked comment must not be displayed\n"),
            10,
        )
        .unwrap();
    assert!(!version.yaml.contains("Untracked comment"));
    assert!(
        w.change_acl(
            &"maya".into(),
            &"risk-investigation".into(),
            fixtures::readers(&["maya", "arun", "agent"]),
            "above-ceiling"
        )
        .is_err()
    );
    assert_eq!(w.state().evidence.policy.revision, 1);
    assert!(
        w.activate_policy(&"agent".into(), &version.spec.id, 1)
            .is_err()
    );
    assert!(
        w.activate_policy(&"maya".into(), &version.spec.id, 1)
            .is_err()
    );
    let report = test_policy(
        &w.state().evidence,
        &version.spec,
        &w.state().source_authority,
        &w.state().destination_authority,
    )
    .unwrap();
    assert!(report.passed, "{report:?}");
    assert_eq!(report.cases.len(), 5);
    let before = w.journal.entries.len();
    w.activate_policy(&"arun".into(), &version.spec.id, 1)
        .unwrap();
    assert_eq!(w.journal.entries.len(), before + 1);
    assert_eq!(w.state().evidence.policy.revision, 2);
    assert_eq!(
        w.state().evidence.policy.configuration,
        Some(version.digest.clone())
    );
    assert!(
        w.activate_policy(&"arun".into(), &version.spec.id, 1)
            .is_err()
    );
    let widened = YAML
        .replace("id: bank-baseline-v1", "id: widened")
        .replace("readers: [risk]", "readers: [engineering, risk]");
    assert!(w.propose_policy(&"agent".into(), &widened, 12).is_err());
    let replay = Coordinator::open(w.journal.clone(), LocalPublisher::default()).unwrap();
    assert_eq!(replay.state(), w.state());
}

#[test]
fn unsupported_rules_missing_evidence_and_disabled_approval_are_rejected() {
    for bad in [
        YAML.replace("missing_lineage_blocks", "guess_if_lineage_missing"),
        YAML.replace("requires_approval: true", "requires_approval: false"),
        YAML.replace("max_age_seconds: 1800", "max_age_seconds: 0"),
        YAML.replace(
            "engineering: [maya, sam, agent]",
            "engineering: [maya, agent]",
        ),
        format!("{YAML}\narbitrary_code: true\n"),
    ] {
        let mut w = Coordinator::open_with_evidence(
            MemoryJournal::default(),
            LocalPublisher::default(),
            fixtures::bank(),
        )
        .unwrap();
        assert!(matches!(
            w.propose_policy(&"agent".into(), &bad, 10),
            Err(Error::Invalid(_))
        ));
        assert!(w.state().policy_versions.is_empty());
        assert!(w.propose_policy(&"agent".into(), YAML, 11).is_ok());
    }
}
