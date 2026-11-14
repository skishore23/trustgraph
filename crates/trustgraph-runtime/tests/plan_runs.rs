use trustgraph_core::{plan::*, *};
use trustgraph_runtime::{
    journal::{MemoryJournal, WriteFault},
    publisher::LocalPublisher,
    *,
};
fn world() -> Coordinator<MemoryJournal, LocalPublisher> {
    Coordinator::open(MemoryJournal::default(), LocalPublisher::default()).unwrap()
}
#[test]
fn unsafe_plan_has_no_generation_effect_and_repairs_prepare_a_real_review() {
    let mut w = world();
    let p = launch_plan("launch", true, "company".into());
    w.propose_plan(&"agent".into(), p.clone(), 0).unwrap();
    let r = analyze(&w.state().evidence, &p, &"agent".into());
    let count = w.state().evidence.artifacts.len();
    assert!(
        w.prepare_plan(
            &"maya".into(),
            &p.id,
            &r.plan_digest,
            &r.snapshot_digest,
            "unsafe",
            1
        )
        .is_err()
    );
    assert_eq!(w.state().evidence.artifacts.len(), count);
    let fixed = w
        .repair_plan(&"maya".into(), &p.id, RepairKind::FreshContext, 1)
        .unwrap();
    let r = analyze(&w.state().evidence, &fixed.plan, &"agent".into());
    let run = w
        .prepare_plan(
            &"maya".into(),
            &fixed.plan.id,
            &r.plan_digest,
            &r.snapshot_digest,
            "safe",
            2,
        )
        .unwrap();
    assert_eq!(run.stage, RunStage::AwaitingReview);
    assert!(w.publisher.accepted().is_empty());
    let request = w
        .review_request(&"maya".into(), &run.request)
        .unwrap()
        .clone();
    assert!(matches!(
        verify(
            &w.state().evidence,
            &Intent {
                artifact: request.artifact().clone(),
                audience: request.audience.clone(),
                requester: "maya".into()
            },
            2
        ),
        Decision::PendingApproval { .. }
    ));
    let origin = &w.state().evidence.artifacts[request.artifact()].origin;
    let Origin::Transformation(t) = origin else {
        panic!("not generated")
    };
    let inputs = &w.state().evidence.transformations[t].manifest.inputs;
    assert!(
        inputs
            .iter()
            .all(|i| i.artifact != ArtifactId::from("launch-plan"))
    );
    let ctx = verify(
        &w.state().evidence,
        &Intent {
            artifact: request.artifact().clone(),
            audience: request.audience.clone(),
            requester: "maya".into(),
        },
        2,
    )
    .context()
    .unwrap()
    .digest();
    let PublishResult::Operation { operation } = w
        .release_request(&"maya".into(), &request.id, 1, &ctx, 3)
        .unwrap()
    else {
        panic!("blocked")
    };
    assert_eq!(operation.state, OperationState::Published);
    assert_eq!(
        w.prepare_plan(
            &"maya".into(),
            &fixed.plan.id,
            &r.plan_digest,
            &r.snapshot_digest,
            "safe",
            4
        )
        .unwrap(),
        run
    );
    assert_eq!(w.publisher.sends, 1);
}
#[test]
fn run_reservation_recovers_without_duplicate_generation_or_review() {
    let mut w = world();
    let p = launch_plan("safe", false, "company".into());
    w.propose_plan(&"agent".into(), p.clone(), 0).unwrap();
    let r = analyze(&w.state().evidence, &p, &"agent".into());
    w.journal.fault = Some(WriteFault::AfterCommit);
    assert!(
        w.prepare_plan(
            &"agent".into(),
            &p.id,
            &r.plan_digest,
            &r.snapshot_digest,
            "restart",
            1
        )
        .is_err()
    );
    let mut w = Coordinator::open(w.journal, w.publisher).unwrap();
    let run = w
        .prepare_plan(
            &"agent".into(),
            &p.id,
            &r.plan_digest,
            &r.snapshot_digest,
            "restart",
            2,
        )
        .unwrap();
    let count = w.state().evidence.artifacts.len();
    w.prepare_plan(
        &"agent".into(),
        &p.id,
        &r.plan_digest,
        &r.snapshot_digest,
        "restart",
        3,
    )
    .unwrap();
    assert_eq!(w.state().evidence.artifacts.len(), count);
    assert_eq!(w.state().requests.len(), 1);
    assert_eq!(run.stage, RunStage::AwaitingReview);
}
#[test]
fn a_review_cannot_substitute_the_plan_output_or_publish_after_acl_drift() {
    let mut w = world();
    let p = launch_plan("safe", false, "company".into());
    w.propose_plan(&"agent".into(), p.clone(), 0).unwrap();
    let r = analyze(&w.state().evidence, &p, &"agent".into());
    let run = w
        .prepare_plan(
            &"agent".into(),
            &p.id,
            &r.plan_digest,
            &r.snapshot_digest,
            "execute",
            1,
        )
        .unwrap();
    let request = w.state().requests[&run.request].clone();
    let edit = w
        .edit(
            &"agent".into(),
            request.artifact(),
            "Different bytes.",
            "substitution",
        )
        .unwrap();
    w.revise_request(&"agent".into(), &request.id, 1, &edit, 2)
        .unwrap();
    let intent = Intent {
        artifact: edit,
        audience: request.audience,
        requester: "maya".into(),
    };
    let ctx = verify(&w.state().evidence, &intent, 2)
        .context()
        .unwrap()
        .digest();
    assert!(
        w.release_request(&"maya".into(), &request.id, 2, &ctx, 3)
            .is_err()
    );
    assert!(w.publisher.accepted().is_empty());
    let mut w = world();
    w.propose_plan(&"agent".into(), p.clone(), 0).unwrap();
    let r = analyze(&w.state().evidence, &p, &"agent".into());
    let run = w
        .prepare_plan(
            &"agent".into(),
            &p.id,
            &r.plan_digest,
            &r.snapshot_digest,
            "acl",
            1,
        )
        .unwrap();
    let request = w.state().requests[&run.request].clone();
    let intent = Intent {
        artifact: request.artifact().clone(),
        audience: request.audience.clone(),
        requester: "maya".into(),
    };
    let ctx = verify(&w.state().evidence, &intent, 1)
        .context()
        .unwrap()
        .digest();
    w.change_acl(
        &"maya".into(),
        &"roadmap".into(),
        trustgraph_runtime::fixtures::readers(&["maya", "arun", "agent"]),
        "narrow-plan",
    )
    .unwrap();
    assert!(
        w.release_request(&"maya".into(), &request.id, 1, &ctx, 3)
            .is_err()
    );
    assert!(w.publisher.accepted().is_empty());
}

#[test]
fn every_prepare_commit_boundary_resumes_once_and_partial_outputs_cannot_bypass_review() {
    for fault in [WriteFault::BeforeCommit, WriteFault::AfterCommit] {
        for boundary in 0..4 {
            let mut w = world();
            let p = launch_plan("recovery", false, "company".into());
            w.propose_plan(&"agent".into(), p.clone(), 0).unwrap();
            let r = analyze(&w.state().evidence, &p, &"agent".into());
            w.journal.fault = Some(fault);
            w.journal.writes_before_fault = boundary;
            let _ = w.prepare_plan(
                &"agent".into(),
                &p.id,
                &r.plan_digest,
                &r.snapshot_digest,
                "commit-boundaries",
                1,
            );
            let mut recovered = Coordinator::open(w.journal, w.publisher).unwrap();
            if recovered
                .state()
                .plan_runs
                .values()
                .any(|run| run.stage == RunStage::Preparing)
                && let Some(a) = recovered
                    .state()
                    .evidence
                    .artifacts
                    .values()
                    .find(|a| a.id.0.starts_with("plan-run-"))
                    .cloned()
            {
                let intent = Intent {
                    artifact: a.id,
                    audience: "product".into(),
                    requester: "maya".into(),
                };
                let ctx = verify(&recovered.state().evidence, &intent, 2)
                    .context()
                    .unwrap()
                    .digest();
                assert!(
                    recovered
                        .approve(&"maya".into(), &intent, &ctx, "partial-bypass", 2)
                        .is_err(),
                    "partial output escaped at {fault:?}/{boundary}"
                );
            }
            recovered
                .prepare_plan(
                    &"agent".into(),
                    &p.id,
                    &r.plan_digest,
                    &r.snapshot_digest,
                    "commit-boundaries",
                    3,
                )
                .unwrap();
            assert_eq!(recovered.state().requests.len(), 1);
            assert_eq!(
                recovered
                    .state()
                    .evidence
                    .artifacts
                    .values()
                    .filter(|a| a.id.0.starts_with("plan-run-"))
                    .count(),
                1
            );
            assert!(recovered.publisher.accepted().is_empty());
        }
    }
}

#[test]
fn malformed_plan_cannot_hide_an_unreadable_source_after_an_earlier_error() {
    let mut w = world();
    w.change_acl(
        &"maya".into(),
        &"launch-plan".into(),
        trustgraph_runtime::fixtures::readers(&["maya", "arun"]),
        "hide-from-agent",
    )
    .unwrap();
    let mut p = launch_plan("invalid-private", true, "company".into());
    p.steps[0].id = "".into();
    assert!(matches!(
        w.propose_plan(&"agent".into(), p, 0),
        Err(Error::Unauthorized)
    ));
}
#[test]
fn a_long_unicode_goal_can_prepare_a_bounded_review_title() {
    let mut w = world();
    let mut p = launch_plan("long-goal", false, "company".into());
    p.goal = "界".repeat(120);
    w.propose_plan(&"agent".into(), p.clone(), 0).unwrap();
    let r = analyze(&w.state().evidence, &p, &"agent".into());
    assert!(r.ready_to_generate);
    let run = w
        .prepare_plan(
            &"agent".into(),
            &p.id,
            &r.plan_digest,
            &r.snapshot_digest,
            "unicode",
            1,
        )
        .unwrap();
    assert!(w.state().requests[&run.request].title.len() <= 160);
}
