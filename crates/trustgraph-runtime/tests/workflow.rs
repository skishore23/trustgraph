use trustgraph_core::{digest, *};
use trustgraph_runtime::{journal::*, publisher::*, *};

fn world() -> Coordinator<MemoryJournal, LocalPublisher> {
    Coordinator::open(MemoryJournal::default(), LocalPublisher::default()).unwrap()
}
fn intent(artifact: ArtifactId) -> Intent {
    Intent {
        artifact,
        audience: "company".into(),
        requester: "maya".into(),
    }
}
fn approved(
    world: &mut Coordinator<impl Journal, impl Publisher>,
    key: &str,
) -> (Intent, digest::Digest) {
    let id = world.generate(&"maya".into(), false, key).unwrap();
    let i = intent(id);
    let context = verify(&world.state().evidence, &i, 10)
        .context()
        .unwrap()
        .digest();
    world
        .approve(&"maya".into(), &i, &context, key, 10)
        .unwrap();
    (i, context)
}
fn operation(result: PublishResult) -> Operation {
    match result {
        PublishResult::Operation { operation } => *operation,
        other => panic!("expected operation: {other:?}"),
    }
}

#[test]
fn demo_uses_exact_approved_bytes_and_refuses_changed_draft() {
    let mut w = world();
    let restricted = intent(w.generate(&"maya".into(), true, "restricted").unwrap());
    assert!(verify(&w.state().evidence, &restricted, 10).has(Code::RestrictedSource));
    let (mut i, ctx) = approved(&mut w, "clean");
    let original = i.clone();
    i.artifact = w
        .edit(
            &"maya".into(),
            &i.artifact,
            "A reviewed launch update.",
            "edit",
        )
        .unwrap();
    assert!(verify(&w.state().evidence, &i, 11).has(Code::StaleApproval));
    assert!(matches!(
        w.publish(&i, &ctx, "blocked", 11).unwrap(),
        PublishResult::Blocked { .. }
    ));
    let op = operation(w.publish(&original, &ctx, "post-1", 11).unwrap());
    assert_eq!(op.state, OperationState::Published);
    let ack = w.publisher.accepted().get("post-1").unwrap();
    assert_eq!(
        ack.payload.content,
        w.state().evidence.artifacts[&original.artifact].content
    );
    assert_eq!(ack.payload.hash, op.context.artifact_hash);
    assert_eq!(ack.payload.destination, "sim://northstar/company");
    assert_eq!(w.publisher.accepted().len(), 1);
    assert!(trustgraph_core::bundle::replay(&op.bundle).is_ok());
    let mut tampered = op.bundle.clone();
    tampered
        .evidence
        .artifacts
        .get_mut(&original.artifact)
        .unwrap()
        .content
        .push('!');
    assert!(trustgraph_core::bundle::replay(&tampered).is_err());
}
#[test]
fn idempotency_keys_cannot_change_payload_and_never_dispatch_twice() {
    let mut w = world();
    let (i, ctx) = approved(&mut w, "clean");
    let first = w.publish(&i, &ctx, "same", 11).unwrap();
    assert_eq!(w.publish(&i, &ctx, "same", 12).unwrap(), first);
    assert_eq!(w.publisher.sends, 1);
    let mut other = i.clone();
    other.audience = "product".into();
    assert!(matches!(
        w.publish(&other, &ctx, "same", 12),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        w.generate(&"maya".into(), true, "clean"),
        Err(Error::Conflict)
    ));
    assert_eq!(w.publisher.accepted().len(), 1);
}
#[test]
fn timeout_after_acceptance_reconciles_same_operation_without_resending() {
    let mut w = world();
    let (i, ctx) = approved(&mut w, "clean");
    w.publisher.next_fault = PublisherFault::AcceptThenTimeout;
    let op = operation(w.publish(&i, &ctx, "uncertain", 11).unwrap());
    assert_eq!(op.state, OperationState::OutcomeUnknown);
    let op = w.reconcile(&"maya".into(), "uncertain", 12).unwrap();
    assert_eq!(op.state, OperationState::Published);
    assert_eq!(w.publisher.sends, 1);
    assert_eq!(w.publisher.accepted().len(), 1);
}
#[test]
fn timeout_without_acceptance_stays_unknown_and_is_not_retried() {
    let mut w = world();
    let (i, ctx) = approved(&mut w, "clean");
    w.publisher.next_fault = PublisherFault::TimeoutBeforeAccept;
    operation(w.publish(&i, &ctx, "uncertain", 11).unwrap());
    assert_eq!(
        w.reconcile(&"maya".into(), "uncertain", 12).unwrap().state,
        OperationState::OutcomeUnknown
    );
    w.publish(&i, &ctx, "uncertain", 13).unwrap();
    assert_eq!(w.publisher.sends, 1);
    assert!(w.publisher.accepted().is_empty());
}
#[test]
fn verification_is_repeated_after_acl_change_before_dispatch() {
    let mut w = world();
    let (i, ctx) = approved(&mut w, "clean");
    w.change_acl(
        &"maya".into(),
        &"roadmap".into(),
        fixtures::readers(&["maya", "arun"]),
        "narrow",
    )
    .unwrap();
    assert!(
        matches!(w.publish(&i, &ctx, "post", 11).unwrap(), PublishResult::Blocked {decision} if decision.has(Code::RestrictedSource))
    );
    assert!(w.publisher.accepted().is_empty());
}
#[test]
fn agent_cannot_approve_or_change_acl_and_sales_cannot_read_private_draft() {
    let mut w = world();
    let (i, ctx) = approved(&mut w, "clean");
    assert!(matches!(
        w.approve(&"agent".into(), &i, &ctx, "agent", 11),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        w.change_acl(
            &"sam".into(),
            &"launch-plan".into(),
            fixtures::readers(&["sam"]),
            "leak"
        ),
        Err(Error::Unauthorized)
    ));
    let private = w.generate(&"maya".into(), true, "private").unwrap();
    assert!(!lineage::can_read(
        &w.state().evidence,
        &"sam".into(),
        &private
    ));
    assert!(matches!(
        w.edit(&"sam".into(), &private, "steal", "copy"),
        Err(Error::Unauthorized)
    ));
}
#[test]
fn uncertain_journal_write_poisons_coordinator_until_recovery() {
    let mut w = world();
    let (i, ctx) = approved(&mut w, "clean");
    w.journal.fault = Some(WriteFault::AfterCommit);
    assert!(w.publish(&i, &ctx, "post", 11).is_err());
    assert!(w.publisher.accepted().is_empty());
    assert!(matches!(
        w.publish(&i, &ctx, "post", 12),
        Err(Error::RecoveryRequired)
    ));
    let mut recovered = Coordinator::open(w.journal, w.publisher).unwrap();
    let op = recovered.reconcile(&"maya".into(), "post", 13).unwrap();
    assert_eq!(op.state, OperationState::FailedConfirmed);
    assert!(recovered.publisher.accepted().is_empty());
}
#[test]
fn durable_state_and_provider_ledger_survive_real_disk_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path();
    let mut w = Coordinator::open(
        DiskJournal::open(path).unwrap(),
        LocalPublisher::open(path.join("publisher")).unwrap(),
    )
    .unwrap();
    let (i, ctx) = approved(&mut w, "disk");
    w.publisher.next_fault = PublisherFault::AcceptThenTimeout;
    operation(w.publish(&i, &ctx, "disk-post", 11).unwrap());
    assert!(DiskJournal::open(path).is_err());
    drop(w);
    let mut recovered = Coordinator::open(
        DiskJournal::open(path).unwrap(),
        LocalPublisher::open(path.join("publisher")).unwrap(),
    )
    .unwrap();
    let op = recovered
        .reconcile(&"maya".into(), "disk-post", 12)
        .unwrap();
    assert_eq!(op.state, OperationState::Published);
    assert_eq!(recovered.publisher.accepted().len(), 1);
    assert_eq!(recovered.publisher.sends, 0);
}
#[test]
fn torn_or_modified_journal_never_allows_new_publication() {
    use std::io::Write;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path();
    drop(Coordinator::open(DiskJournal::open(path).unwrap(), LocalPublisher::default()).unwrap());
    std::fs::OpenOptions::new()
        .append(true)
        .open(path.join("journal.jsonl"))
        .unwrap()
        .write_all(b"{\"partial\"")
        .unwrap();
    assert!(DiskJournal::open(path).is_err());
}

#[test]
fn restart_after_send_before_outcome_commit_reconciles_without_second_send() {
    let mut w = world();
    let (i, ctx) = approved(&mut w, "recovery");
    w.journal.fault = Some(WriteFault::BeforeCommit);
    w.journal.writes_before_fault = 2;
    assert!(w.publish(&i, &ctx, "sent", 11).is_err());
    assert_eq!(w.publisher.accepted().len(), 1);
    let mut recovered = Coordinator::open(w.journal, w.publisher).unwrap();
    assert_eq!(
        recovered.state().operations["sent"].state,
        OperationState::OutcomeUnknown
    );
    assert_eq!(
        recovered
            .reconcile(&"maya".into(), "sent", 12)
            .unwrap()
            .state,
        OperationState::Published
    );
    assert_eq!(recovered.publisher.sends, 1);
}

#[test]
fn editing_an_older_version_creates_a_new_version_without_overwriting() {
    let mut w = world();
    let parent = w.generate(&"maya".into(), false, "original").unwrap();
    let first = w
        .edit(&"maya".into(), &parent, "First edit.", "one")
        .unwrap();
    let second = w
        .edit(&"maya".into(), &parent, "Second edit.", "two")
        .unwrap();
    assert_eq!(w.state().evidence.artifacts[&first].version, 2);
    assert_eq!(w.state().evidence.artifacts[&second].version, 3);
    assert_eq!(
        w.edit(&"maya".into(), &parent, "First edit.", "one")
            .unwrap(),
        first
    );
    assert_eq!(w.state().evidence.artifacts[&first].content, "First edit.");
}

#[test]
fn content_store_uses_exact_hashes_and_detects_modified_blob_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path();
    let mut w =
        Coordinator::open(DiskJournal::open(path).unwrap(), LocalPublisher::default()).unwrap();
    let artifact = w.generate(&"maya".into(), false, "blob").unwrap();
    let record = &w.state().evidence.artifacts[&artifact];
    let blob = path.join("blobs").join(&record.hash.0);
    assert_eq!(std::fs::read(&blob).unwrap(), record.content.as_bytes());
    drop(w);
    std::fs::write(&blob, b"changed bytes").unwrap();
    assert!(DiskJournal::open(path).is_err());
}
