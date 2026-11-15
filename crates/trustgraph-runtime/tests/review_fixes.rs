//! Regression tests for publication fencing, operator resolution and policy bounds.
use serde_json::json;
use trustgraph_core::*;
use trustgraph_runtime::{
    journal::MemoryJournal,
    publisher::{LocalPublisher, PublisherFault},
    story::{Command, Story},
    *,
};

type World = Coordinator<MemoryJournal, LocalPublisher>;
const BANK_POLICY: &str = include_str!("fixtures/bank-policy.yaml");

fn world() -> World {
    Coordinator::open(MemoryJournal::default(), LocalPublisher::default()).unwrap()
}
fn approved(w: &mut World, key: &str) -> (Intent, digest::Digest) {
    let artifact = w.generate(&"maya".into(), false, key).unwrap();
    let intent = Intent {
        artifact,
        audience: "company".into(),
        requester: "maya".into(),
    };
    let context = verify(&w.state().evidence, &intent, 10)
        .context()
        .unwrap()
        .digest();
    w.approve(&"maya".into(), &intent, &context, key, 10)
        .unwrap();
    (intent, context)
}
fn state_of(result: PublishResult) -> OperationState {
    match result {
        PublishResult::Operation { operation } => operation.state,
        other => panic!("expected operation: {other:?}"),
    }
}
fn reopen(w: World) -> World {
    Coordinator::open(w.journal, w.publisher).unwrap()
}

#[test]
fn a_fresh_key_cannot_republish_published_bytes_to_the_same_destination() {
    let mut w = world();
    let (intent, context) = approved(&mut w, "once");
    assert_eq!(
        state_of(w.publish(&intent, &context, "first", 11).unwrap()),
        OperationState::Published
    );
    assert!(matches!(
        w.publish(&intent, &context, "second", 11),
        Err(Error::Conflict)
    ));
    // The same key remains an idempotent retry.
    assert_eq!(
        state_of(w.publish(&intent, &context, "first", 11).unwrap()),
        OperationState::Published
    );
    let mut w = reopen(w);
    assert!(matches!(
        w.publish(&intent, &context, "third", 12),
        Err(Error::Conflict)
    ));
    assert_eq!(w.publisher.sends, 1);
}

#[test]
fn outcome_unknown_fences_new_keys_until_an_approver_confirms_failure() {
    let mut w = world();
    let (intent, context) = approved(&mut w, "uncertain");
    w.publisher.next_fault = PublisherFault::TimeoutBeforeAccept;
    assert_eq!(
        state_of(w.publish(&intent, &context, "lost", 11).unwrap()),
        OperationState::OutcomeUnknown
    );
    assert!(matches!(
        w.publish(&intent, &context, "retry", 11),
        Err(Error::Conflict)
    ));
    // Guard survives journal replay.
    let mut w = reopen(w);
    assert!(matches!(
        w.publish(&intent, &context, "retry", 11),
        Err(Error::Conflict)
    ));
    // Agents and non-approvers cannot resolve; unknown keys are not found.
    assert!(matches!(
        w.confirm_failed(&"agent".into(), "lost", 12),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        w.confirm_failed(&"sam".into(), "lost", 12),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        w.confirm_failed(&"maya".into(), "missing", 12),
        Err(Error::NotFound)
    ));
    let resolved = w.confirm_failed(&"maya".into(), "lost", 12).unwrap();
    assert_eq!(resolved.state, OperationState::FailedConfirmed);
    assert_eq!(
        w.confirm_failed(&"maya".into(), "lost", 13).unwrap().state,
        OperationState::FailedConfirmed
    );
    let mut w = reopen(w);
    assert_eq!(
        w.state().operations["lost"].state,
        OperationState::FailedConfirmed
    );
    assert_eq!(
        state_of(w.publish(&intent, &context, "retry", 13).unwrap()),
        OperationState::Published
    );
    // Published operations cannot be marked failed.
    assert!(matches!(
        w.confirm_failed(&"maya".into(), "retry", 14),
        Err(Error::Conflict)
    ));
}

#[test]
fn failure_cannot_be_confirmed_when_the_publisher_has_the_delivery() {
    let mut w = world();
    let (intent, context) = approved(&mut w, "delivered");
    w.publisher.next_fault = PublisherFault::AcceptThenTimeout;
    assert_eq!(
        state_of(w.publish(&intent, &context, "late", 11).unwrap()),
        OperationState::OutcomeUnknown
    );
    assert!(matches!(
        w.confirm_failed(&"maya".into(), "late", 12),
        Err(Error::Conflict)
    ));
    assert_eq!(
        w.reconcile(&"maya".into(), "late", 12).unwrap().state,
        OperationState::Published
    );
}

fn submit(w: &mut World) -> ReviewRequest {
    let artifact = w.generate(&"agent".into(), false, "handoff").unwrap();
    w.submit_request(
        &"agent".into(),
        Submission {
            key: "handoff".into(),
            title: "Share the roadmap".into(),
            summary: "For the company channel.".into(),
            artifact,
            audience: "company".into(),
        },
        10,
    )
    .unwrap()
}
fn request_context(w: &World, request: &ReviewRequest) -> (Intent, digest::Digest) {
    let intent = Intent {
        artifact: request.artifact().clone(),
        audience: request.audience.clone(),
        requester: "maya".into(),
    };
    let context = verify(&w.state().evidence, &intent, 10)
        .context()
        .unwrap()
        .digest();
    (intent, context)
}

#[test]
fn low_level_publication_fences_a_later_request_release() {
    let mut w = world();
    let request = submit(&mut w);
    let (intent, context) = request_context(&w, &request);
    w.approve(&"maya".into(), &intent, &context, "direct", 10)
        .unwrap();
    assert_eq!(
        state_of(w.publish(&intent, &context, "direct", 11).unwrap()),
        OperationState::Published
    );
    let approvals = w.state().evidence.approvals.len();
    assert!(matches!(
        w.release_request(&"maya".into(), &request.id, 1, &context, 11),
        Err(Error::Conflict)
    ));
    // Refused before a new approval is recorded.
    assert_eq!(w.state().evidence.approvals.len(), approvals);
    assert_eq!(w.publisher.sends, 1);
}

#[test]
fn returned_requests_block_lower_level_approval_and_publication() {
    let mut w = world();
    let request = submit(&mut w);
    let (intent, context) = request_context(&w, &request);
    w.approve(&"maya".into(), &intent, &context, "before", 10)
        .unwrap();
    w.request_changes(&"maya".into(), &request.id, 1, "Revise it.", "fb", 11)
        .unwrap();
    assert!(matches!(
        w.approve(&"maya".into(), &intent, &context, "bypass", 12),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        w.publish(&intent, &context, "bypass", 12),
        Err(Error::Conflict)
    ));
    assert_eq!(w.publisher.sends, 0);
}

#[test]
fn reviewers_cannot_move_a_destination_or_disable_the_last_authority() {
    let mut w = world();
    let mut moved = w.state().evidence.audiences[&AudienceId::from("company")].clone();
    moved.destination = "sim://northstar/another-channel".into();
    assert!(matches!(
        w.change_audience(&"maya".into(), moved),
        Err(Error::Unauthorized)
    ));
    // The northstar administrator cannot be disabled.
    assert!(matches!(
        w.disable_principal(&"arun".into(), &"maya".into()),
        Err(Error::Invalid(_))
    ));
    w.disable_principal(&"maya".into(), &"sam".into()).unwrap();

    let mut e = fixtures::northstar();
    e.policy.approvers = fixtures::readers(&["arun"]);
    let mut w =
        Coordinator::open_with_evidence(MemoryJournal::default(), LocalPublisher::default(), e)
            .unwrap();
    assert!(matches!(
        w.disable_principal(&"arun".into(), &"arun".into()),
        Err(Error::Invalid(_))
    ));
    assert!(w.state().evidence.principals[&PrincipalId::from("arun")].enabled);
}

#[test]
fn reimporting_a_source_with_different_readers_conflicts() {
    let mut w = world();
    let readers = fixtures::readers(&["maya", "arun"]);
    w.import_source(
        &"maya".into(),
        "memo".into(),
        "Memo".into(),
        readers.clone(),
    )
    .unwrap();
    assert!(
        w.import_source(&"maya".into(), "memo".into(), "Memo".into(), readers)
            .is_ok()
    );
    assert!(matches!(
        w.import_source(
            &"maya".into(),
            "memo".into(),
            "Memo".into(),
            fixtures::readers(&["maya", "arun", "sam"])
        ),
        Err(Error::Conflict)
    ));
}

#[test]
fn policy_activation_never_re_grants_narrowed_sources_or_recipients() {
    let mut w = Coordinator::open_with_evidence(
        MemoryJournal::default(),
        LocalPublisher::default(),
        fixtures::bank(),
    )
    .unwrap();
    let baseline = w.propose_policy(&"agent".into(), BANK_POLICY, 10).unwrap();
    // Explicit narrowing: Sam loses the requirements source and the repository.
    w.change_acl(
        &"arun".into(),
        &"engineering-requirements".into(),
        fixtures::readers(&["maya", "arun", "agent"]),
        "narrow-requirements",
    )
    .unwrap();
    let mut repo = w.state().evidence.audiences[&AudienceId::from("engineering-repo")].clone();
    repo.recipients = fixtures::readers(&["maya"]);
    w.change_audience(&"arun".into(), repo).unwrap();
    assert!(matches!(
        w.activate_policy(&"arun".into(), &baseline.spec.id, 1),
        Err(Error::Invalid(_))
    ));
    let narrowed = BANK_POLICY
        .replace("id: bank-baseline-v1", "id: bank-narrowed")
        .replace(
            "  risk: [arun, agent]",
            "  risk: [arun, agent]\n  leads: [maya]",
        )
        .replace(
            "  engineering-requirements:\n    readers: [engineering, risk]",
            "  engineering-requirements:\n    readers: [leads, risk]",
        )
        .replace(
            "  engineering-repo:\n    audience: [engineering]",
            "  engineering-repo:\n    audience: [leads]",
        );
    let version = w.propose_policy(&"agent".into(), &narrowed, 11).unwrap();
    w.activate_policy(&"arun".into(), &version.spec.id, 1)
        .unwrap();
    let e = &w.state().evidence;
    let source = &e.sources[&SourceId::from("engineering-requirements")];
    assert!(
        !e.acls[&source.acl]
            .readers
            .contains(&PrincipalId::from("sam"))
    );
    let repo = &e.audiences[&AudienceId::from("engineering-repo")];
    assert_eq!(repo.recipients, fixtures::readers(&["maya"]));
    assert_eq!(repo.destination, "sim://bank/engineering-repo");
    let expected = w.state().clone();
    assert_eq!(reopen(w).state(), &expected);
}

fn command(value: serde_json::Value) -> Command {
    serde_json::from_value(value).unwrap()
}

#[test]
fn story_rejects_non_canonical_digests_without_panicking() {
    let mut story = Story::new().unwrap();
    let view = story
        .execute(command(
            json!({"action":"generate","approved_inputs":true,"expected_revision":1}),
        ))
        .unwrap();
    let artifact = view["current"]["artifact"]["id"].clone();
    for expected in ["aaaaaaaaaaaaaaa€", "€", "", "ABCDEF"] {
        for action in [
            json!({"action":"approve","reviewer":"maya","artifact":artifact,"expected":expected}),
            json!({"action":"release","artifact":artifact,"expected":expected}),
        ] {
            assert!(matches!(
                story.execute(command(action)),
                Err(Error::Invalid(_))
            ));
        }
    }
}

#[test]
fn story_replay_without_views_matches_execute() {
    let steps = || {
        vec![
            json!({"action":"generate","approved_inputs":false,"expected_revision":1}),
            json!({"action":"generate","approved_inputs":true,"expected_revision":1}),
        ]
    };
    let mut executed = Story::new().unwrap();
    let mut last = None;
    for step in steps() {
        last = Some(executed.execute(command(step)).unwrap());
    }
    let mut replayed = Story::new().unwrap();
    for step in steps() {
        replayed.replay(command(step)).unwrap();
    }
    assert_eq!(Some(replayed.view()), last);
    // Approval and release execute as a human governance principal.
    let v = replayed.view();
    let digest = v["current"]["assessment"]["context_digest"].clone();
    let artifact = v["current"]["artifact"]["id"].clone();
    replayed
        .replay(command(
            json!({"action":"approve","reviewer":"maya","artifact":artifact,"expected":digest}),
        ))
        .unwrap();
    replayed
        .replay(command(
            json!({"action":"release","artifact":artifact,"expected":digest}),
        ))
        .unwrap();
    let v = replayed.view();
    assert_eq!(v["current"]["operation"]["state"], "PUBLISHED");
    assert_eq!(v["current"]["operation"]["intent"]["requester"], "maya");
}
