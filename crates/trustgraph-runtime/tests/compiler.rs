use trustgraph_core::{plan::*, *};
use trustgraph_runtime::fixtures;

#[test]
fn compiler_names_a_concrete_unauthorized_recipient_and_dependency_path() {
    let e = fixtures::northstar();
    let p = launch_plan("launch", true, "company".into());
    let report = analyze(&e, &p, &"agent".into());
    let c = report.conflict.unwrap();
    assert_eq!(c.code, PlanCode::AudienceConflict);
    assert_eq!(c.step, "publish");
    assert_eq!(c.recipient, Some("sam".into()));
    assert_eq!(c.source, Some("launch-plan".into()));
    assert_eq!(c.path, vec!["read-launch-plan", "generate", "publish"]);
}
#[test]
fn both_repairs_recompile_without_changing_acl_or_claiming_equal_content() {
    let e = fixtures::northstar();
    let before = e.clone();
    let p = launch_plan("launch", true, "company".into());
    let options = repairs(&e, &p, &"agent".into());
    assert_eq!(options.len(), 2);
    assert!(options.iter().all(|r| r.report.ready_to_generate));
    assert_eq!(e, before);
    let narrow = options
        .iter()
        .find(|r| r.kind == RepairKind::NarrowAudience)
        .unwrap();
    assert!(
        matches!(&narrow.plan.steps.last().unwrap().operation,Op::Publish{audience,..} if audience==&AudienceId::from("product"))
    );
    let clean = options
        .iter()
        .find(|r| r.kind == RepairKind::FreshContext)
        .unwrap();
    assert!(!clean.plan.steps.iter().any(
        |s| matches!(&s.operation,Op::Read{source} if source==&SourceId::from("launch-plan"))
    ));
}
#[test]
fn approval_is_an_obligation_after_generation_and_must_match_publication() {
    let e = fixtures::northstar();
    let mut p = launch_plan("safe", false, "company".into());
    let report = analyze(&e, &p, &"agent".into());
    assert!(report.ready_to_generate);
    assert_eq!(report.obligations.len(), 1);
    assert_eq!(report.obligations[0].artifact, "generate");
    let n = p.steps.len();
    if let Op::Approve { audience, .. } = &mut p.steps[n - 2].operation {
        *audience = "product".into();
    }
    assert_eq!(
        analyze(&e, &p, &"agent".into()).conflict.unwrap().code,
        PlanCode::ApprovalBindingMismatch
    );
}
#[test]
fn bad_references_missing_prompt_and_private_metadata_fail_closed() {
    let e = fixtures::northstar();
    let p = launch_plan("launch", true, "company".into());
    let denied = analyze(&e, &p, &"sam".into()).conflict.unwrap();
    assert_eq!(denied.code, PlanCode::CallerUnauthorized);
    assert_eq!(denied.source, None);
    assert!(denied.path.is_empty());
    let mut p = launch_plan("bad", false, "company".into());
    if let Op::Generate { inputs, .. } = &mut p.steps[2].operation {
        inputs.push("generate".into());
    }
    assert_eq!(
        analyze(&e, &p, &"agent".into()).conflict.unwrap().code,
        PlanCode::UnavailableReference
    );
    let mut p = launch_plan("prompt", false, "company".into());
    if let Op::Generate { inputs, .. } = &mut p.steps[2].operation {
        inputs.retain(|i| i != "read-prompt");
    }
    assert_eq!(
        analyze(&e, &p, &"agent".into()).conflict.unwrap().code,
        PlanCode::MissingPrompt
    );
}

#[test]
fn every_fresh_generation_records_its_prompt_as_a_direct_input() {
    let e = fixtures::northstar();
    let mut p = launch_plan("nested", false, "company".into());
    let n = p.steps.len();
    p.steps.insert(
        n - 2,
        Step {
            id: "second-generation".into(),
            operation: Op::Generate {
                inputs: vec!["generate".into()],
                provider: Provider::LocalFixture,
            },
        },
    );
    for step in &mut p.steps {
        match &mut step.operation {
            Op::Approve { artifact, .. } | Op::Publish { artifact, .. } => {
                *artifact = "second-generation".into()
            }
            _ => {}
        }
    }
    assert_eq!(
        analyze(&e, &p, &"agent".into()).conflict.unwrap().code,
        PlanCode::MissingPrompt
    );
}
