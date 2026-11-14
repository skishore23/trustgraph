use trustgraph_core::*;
use trustgraph_runtime::{journal::MemoryJournal, publisher::LocalPublisher, *};
fn world() -> Coordinator<MemoryJournal, LocalPublisher> {
    Coordinator::open(MemoryJournal::default(), LocalPublisher::default()).unwrap()
}
fn submission(w: &mut Coordinator<MemoryJournal, LocalPublisher>) -> ReviewRequest {
    let artifact = w.generate(&"agent".into(), false, "submission").unwrap();
    w.submit_request(
        &"agent".into(),
        Submission {
            key: "launch".into(),
            title: "Share the roadmap update".into(),
            summary: "Prepared for the company channel.".into(),
            artifact,
            audience: "company".into(),
        },
        10,
    )
    .unwrap()
}
fn context(
    w: &Coordinator<MemoryJournal, LocalPublisher>,
    request: &ReviewRequest,
) -> digest::Digest {
    let mut evidence = w.state().evidence.clone();
    evidence.approvals.clear();
    verify(
        &evidence,
        &Intent {
            artifact: request.artifact().clone(),
            audience: request.audience.clone(),
            requester: "maya".into(),
        },
        11,
    )
    .context()
    .unwrap()
    .digest()
}
#[test]
fn agent_intake_is_persistent_idempotent_and_scoped_to_reviewers() {
    let mut w = world();
    let r = submission(&mut w);
    let input = Submission {
        key: r.id.clone(),
        title: r.title.clone(),
        summary: r.summary.clone(),
        artifact: r.artifact().clone(),
        audience: r.audience.clone(),
    };
    assert_eq!(
        w.submit_request(&"agent".into(), input.clone(), 12)
            .unwrap(),
        r
    );
    assert!(matches!(
        w.submit_request(
            &"maya".into(),
            Submission {
                key: "human".into(),
                ..input
            },
            12
        ),
        Err(Error::Unauthorized)
    ));
    assert_eq!(w.visible_requests(&"maya".into()).len(), 1);
    assert_eq!(w.visible_requests(&"agent".into()).len(), 1);
    assert!(w.visible_requests(&"sam".into()).is_empty());
    let recovered = Coordinator::open(w.journal, w.publisher).unwrap();
    assert_eq!(recovered.review_request(&"maya".into(), &r.id).unwrap(), &r);
}
#[test]
fn returning_work_records_feedback_revokes_old_approval_and_requires_agent_revision() {
    let mut w = world();
    let r = submission(&mut w);
    let ctx = context(&w, &r);
    let intent = Intent {
        artifact: r.artifact().clone(),
        audience: r.audience.clone(),
        requester: "maya".into(),
    };
    w.approve(&"maya".into(), &intent, &ctx, "earlier", 11)
        .unwrap();
    w.request_changes(
        &"maya".into(),
        &r.id,
        1,
        "Use a clearer opening sentence.",
        "feedback",
        12,
    )
    .unwrap();
    assert!(verify(&w.state().evidence, &intent, 12).has(Code::ApprovalRevoked));
    assert!(
        w.release_request(&"maya".into(), &r.id, 1, &ctx, 12)
            .is_err()
    );
    assert!(
        w.request_changes(
            &"agent".into(),
            &r.id,
            1,
            "self review",
            "agent-feedback",
            12
        )
        .is_err()
    );
    let revised = w
        .edit(
            &"agent".into(),
            r.artifact(),
            "An updated roadmap announcement.",
            "revision",
        )
        .unwrap();
    let r = w
        .revise_request(&"agent".into(), &r.id, 1, &revised, 13)
        .unwrap();
    assert!(!r.changes_requested);
    assert_eq!(r.revision(), 2);
    assert_eq!(r.feedback.len(), 1);
    assert!(
        w.release_request(&"maya".into(), &r.id, 1, &ctx, 14)
            .is_err()
    );
    let ctx = context(&w, &r);
    let PublishResult::Operation { operation } = w
        .release_request(&"maya".into(), &r.id, 2, &ctx, 14)
        .unwrap()
    else {
        panic!("release blocked")
    };
    assert_eq!(operation.state, OperationState::Published);
    assert_eq!(
        operation.bundle.evidence.artifacts[&revised].content,
        "An updated roadmap announcement."
    );
}
#[test]
fn release_is_fenced_idempotent_and_agent_cannot_approve() {
    let mut w = world();
    let r = submission(&mut w);
    let ctx = context(&w, &r);
    assert!(
        w.release_request(&"agent".into(), &r.id, 1, &ctx, 11)
            .is_err()
    );
    let first = w
        .release_request(&"maya".into(), &r.id, 1, &ctx, 11)
        .unwrap();
    let second = w
        .release_request(&"arun".into(), &r.id, 1, &ctx, 12)
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(w.publisher.sends, 1);
    assert!(
        w.request_changes(&"maya".into(), &r.id, 1, "Too late", "late", 13)
            .is_err()
    );
    assert!(
        w.release_request(
            &"maya".into(),
            &r.id,
            1,
            &digest::content(b"wrong context"),
            13
        )
        .is_err()
    );
}

#[test]
fn intake_retry_cannot_return_an_unreadable_later_revision() {
    let mut w = world();
    let r = submission(&mut w);
    let original = Submission {
        key: r.id.clone(),
        title: r.title.clone(),
        summary: r.summary.clone(),
        artifact: r.artifact().clone(),
        audience: r.audience.clone(),
    };
    let private = w
        .generate(&"agent".into(), true, "private-revision")
        .unwrap();
    w.revise_request(&"agent".into(), &r.id, 1, &private, 12)
        .unwrap();
    w.change_acl(
        &"maya".into(),
        &"launch-plan".into(),
        trustgraph_runtime::fixtures::readers(&["maya", "arun"]),
        "remove-agent",
    )
    .unwrap();
    assert!(matches!(
        w.submit_request(&"agent".into(), original, 13),
        Err(Error::NotFound)
    ));
}

struct LoseRequestLink {
    inner: MemoryJournal,
}
impl trustgraph_runtime::journal::Journal for LoseRequestLink {
    fn events(&self) -> Result<Vec<Event>, Error> {
        Ok(self.inner.entries.clone())
    }
    fn append(&mut self, event: &Event) -> Result<(), Error> {
        if matches!(event, Event::RequestReleased { .. }) {
            return Err(Error::Storage("lost separate release linkage write".into()));
        }
        trustgraph_runtime::journal::Journal::append(&mut self.inner, event)
    }
}
#[test]
fn release_linkage_survives_loss_of_a_separate_post_dispatch_write() {
    let mut original = world();
    let r = submission(&mut original);
    let ctx = context(&original, &r);
    let mut w = Coordinator::open(
        LoseRequestLink {
            inner: original.journal,
        },
        original.publisher,
    )
    .unwrap();
    let _ = w.release_request(&"maya".into(), &r.id, 1, &ctx, 12);
    assert_eq!(w.publisher.accepted().len(), 1);
    let mut recovered = Coordinator::open(w.journal.inner, w.publisher).unwrap();
    assert!(
        recovered
            .review_request(&"maya".into(), &r.id)
            .unwrap()
            .operation
            .is_some()
    );
    assert!(
        recovered
            .request_changes(&"maya".into(), &r.id, 1, "Too late", "late-return", 13)
            .is_err()
    );
    let revised = recovered
        .edit(
            &"agent".into(),
            r.artifact(),
            "A later draft.",
            "after-release",
        )
        .unwrap();
    assert!(
        recovered
            .revise_request(&"agent".into(), &r.id, 1, &revised, 13)
            .is_err()
    );
    assert_eq!(recovered.publisher.accepted().len(), 1);
}
