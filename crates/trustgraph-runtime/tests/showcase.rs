use serde_json::{Value, json};
use trustgraph_runtime::showcase::{Command, Showcase};

fn run(demo: &mut Showcase, actor: &str, command: Value) -> Value {
    demo.execute(
        &actor.into(),
        serde_json::from_value::<Command>(command).unwrap(),
        100,
    )
    .unwrap()
}
fn candidate<'a>(view: &'a Value, id: &str) -> &'a Value {
    &view["complex"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["artifact"] == id)
        .unwrap()["assessment"]
}
fn approve_baseline(demo: &mut Showcase) -> Value {
    let view = demo.view(&"maya".into(), 100).unwrap();
    run(
        demo,
        "maya",
        json!({"action":"approve_baseline","expected":view["complex"]["expected"]}),
    )
}

#[test]
fn restricted_generation_blocks_and_fresh_generation_has_no_old_context() {
    let mut demo = Showcase::new().unwrap();
    let private = run(&mut demo, "arun", json!({"action":"simulate_mixed"}));
    assert_eq!(private["simple"]["assessment"]["status"], "blocked");
    assert_eq!(
        private["simple"]["assessment"]["decision"]["violations"][0]["code"],
        "RESTRICTED_SOURCE"
    );
    let public = demo.view(&"maya".into(), 100).unwrap();
    assert_eq!(public["simple"]["visible"], false);
    assert!(!public.to_string().contains("mixed-test"));
    assert!(
        demo.execute(&"maya".into(), Command::SimulateMixed, 100)
            .is_err()
    );
    let fresh = run(&mut demo, "arun", json!({"action":"fresh_generation"}));
    assert_eq!(fresh["fresh"]["assessment"]["status"], "needs_approval");
    let inputs = &fresh["fresh"]["artifact"]["transformation"]["manifest"]["inputs"];
    let ids: Vec<_> = inputs
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["artifact"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["engineering-requirements", "task-prompt"]);
    assert_eq!(fresh["publisher_calls"], 0);
}

#[test]
fn exact_approval_is_preserved_but_cannot_authorize_an_edit_or_new_destination() {
    let mut demo = Showcase::new().unwrap();
    let v1 = demo.view(&"maya".into(), 100).unwrap();
    assert!(
        demo.execute(
            &"agent".into(),
            serde_json::from_value(
                json!({"action":"approve_patch","expected":v1["medium"]["expected"]})
            )
            .unwrap(),
            100
        )
        .is_err()
    );
    let approved = run(
        &mut demo,
        "maya",
        json!({"action":"approve_patch","expected":v1["medium"]["expected"]}),
    );
    assert_eq!(approved["medium"]["assessment"]["status"], "ready");
    let old = approved["medium"]["approvals"][0].clone();
    let edited = run(&mut demo, "agent", json!({"action":"change_patch"}));
    assert_ne!(
        v1["medium"]["artifact"]["hash"],
        edited["medium"]["artifact"]["hash"]
    );
    assert!(
        demo.execute(
            &"maya".into(),
            serde_json::from_value(
                json!({"action":"approve_patch","expected":v1["medium"]["expected"]})
            )
            .unwrap(),
            100
        )
        .is_err()
    );
    let stale = run(&mut demo, "agent", json!({"action":"simulate_patch"}));
    assert_eq!(stale["medium"]["assessment"]["information_permitted"], true);
    assert_eq!(stale["medium"]["assessment"]["approval_matches"], false);
    assert_eq!(
        stale["medium"]["assessment"]["decision"]["violations"][0]["code"],
        "STALE_APPROVAL"
    );
    assert_eq!(stale["medium"]["reviewer_authorized"], true);
    assert_eq!(stale["medium"]["approvals"][0], old);
    let renewed = run(
        &mut demo,
        "maya",
        json!({"action":"approve_patch","expected":stale["medium"]["expected"]}),
    );
    assert_eq!(renewed["medium"]["assessment"]["status"], "ready");
    run(&mut demo, "agent", json!({"action":"change_destination"}));
    let changed = run(&mut demo, "agent", json!({"action":"simulate_patch"}));
    assert_eq!(
        changed["medium"]["assessment"]["information_permitted"],
        true
    );
    assert_eq!(changed["medium"]["assessment"]["approval_matches"], false);
    assert_eq!(changed["publisher_calls"], 0);
}

#[test]
fn forward_impact_distinguishes_forbidden_stale_and_independent_ready_work() {
    let mut demo = Showcase::new().unwrap();
    let baseline = approve_baseline(&mut demo);
    for id in [
        "release-bundle",
        "risk-note",
        "compatibility-patch",
        "documentation-fix",
    ] {
        assert_eq!(candidate(&baseline, id)["status"], "ready");
    }
    let marked = run(
        &mut demo,
        "maya",
        json!({"action":"mark_impact","source":"case-analysis"}),
    );
    assert_eq!(marked["complex"]["phase"], "marked");
    let affected = marked["complex"]["affected"].as_array().unwrap();
    for id in [
        "case-analysis",
        "regression-fixtures",
        "regression-report",
        "release-bundle",
        "risk-note",
    ] {
        assert!(affected.contains(&json!(id)));
    }
    for id in [
        "api-spec",
        "compatibility-patch",
        "compatibility-report",
        "documentation-fix",
    ] {
        assert!(!affected.contains(&json!(id)));
    }
    assert_eq!(marked["complex"]["candidates"][0]["needs_recheck"], true);
    let evaluated = run(&mut demo, "maya", json!({"action":"evaluate_impact"}));
    assert_eq!(candidate(&evaluated, "release-bundle")["status"], "blocked");
    assert_eq!(
        candidate(&evaluated, "release-bundle")["approval_matches"],
        false
    );
    assert_eq!(
        candidate(&evaluated, "risk-note")["information_permitted"],
        true
    );
    assert_eq!(
        candidate(&evaluated, "risk-note")["status"],
        "needs_approval_renewal"
    );
    for id in ["compatibility-patch", "documentation-fix"] {
        assert_eq!(candidate(&evaluated, id)["status"], "ready");
        assert_eq!(
            candidate(&baseline, id)["context_digest"],
            candidate(&evaluated, id)["context_digest"]
        );
    }
    assert_eq!(
        evaluated["complex"]["active_source_readers"],
        json!(["agent", "arun", "maya", "sam"])
    );
    assert_eq!(evaluated["publisher_calls"], 0);
    assert_eq!(
        baseline["complex"]["approvals"],
        evaluated["complex"]["approvals"]
    );
    let restored = run(&mut demo, "maya", json!({"action":"clear_impact"}));
    assert_eq!(candidate(&restored, "release-bundle")["status"], "ready");
}

#[test]
fn changing_the_other_source_traverses_a_different_branch_and_expiry_is_real() {
    let mut demo = Showcase::new().unwrap();
    approve_baseline(&mut demo);
    run(
        &mut demo,
        "arun",
        json!({"action":"mark_impact","source":"api-spec"}),
    );
    let view = run(&mut demo, "arun", json!({"action":"evaluate_impact"}));
    for id in ["compatibility-patch", "documentation-fix", "release-bundle"] {
        assert_eq!(candidate(&view, id)["status"], "blocked");
    }
    assert_eq!(candidate(&view, "risk-note")["status"], "ready");
    assert!(
        !view["complex"]["affected"]
            .as_array()
            .unwrap()
            .contains(&json!("regression-fixtures"))
    );
    run(&mut demo, "arun", json!({"action":"clear_impact"}));
    let expired = demo.view(&"maya".into(), 3700).unwrap();
    assert_eq!(
        candidate(&expired, "release-bundle")["approval_matches"],
        true
    );
    assert_eq!(
        candidate(&expired, "release-bundle")["approval_valid"],
        false
    );
    assert_eq!(
        candidate(&expired, "release-bundle")["decision"]["violations"][0]["code"],
        "APPROVAL_EXPIRED"
    );
    assert!(demo.view(&"unknown".into(), 100).is_err());
}
