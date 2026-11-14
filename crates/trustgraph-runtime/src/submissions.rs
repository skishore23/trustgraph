//! Journaled agent handoffs and human review. Request metadata never grants
//! publication authority; release still calls the exact-context Rust gate.
use crate::{engine::validate_key, journal::Journal, publisher::Publisher, *};
use trustgraph_core::*;

impl<J: Journal, P: Publisher> Coordinator<J, P> {
    pub fn submit_request(
        &mut self,
        actor: &PrincipalId,
        input: Submission,
        now: u64,
    ) -> Result<ReviewRequest, Error> {
        if self.actor(actor)?.role != Role::Agent {
            return Err(Error::Unauthorized);
        }
        validate_key(&input.key)?;
        if input.key.len() > 64
            || input.title.trim().is_empty()
            || input.title.len() > 160
            || input.summary.len() > 2000
        {
            return Err(Error::Invalid("request needs a title up to 160 bytes, summary up to 2000 bytes, and key up to 64 bytes".into()));
        }
        if !lineage::can_read(&self.state.evidence, actor, &input.artifact)
            || !self.state.evidence.audiences.contains_key(&input.audience)
        {
            return Err(Error::NotFound);
        }
        if let Some(existing) = self.state.requests.get(&input.key) {
            return if existing.title == input.title
                && existing.summary == input.summary
                && existing.submitted_by == *actor
                && existing.revisions[0].artifact == input.artifact
                && existing.audience == input.audience
            {
                Ok(self.review_request(actor, &input.key)?.clone())
            } else {
                Err(Error::Conflict)
            };
        }
        let request = ReviewRequest {
            id: input.key,
            title: input.title,
            summary: input.summary,
            submitted_by: actor.clone(),
            audience: input.audience,
            revisions: vec![RequestRevision {
                artifact: input.artifact,
                at: now,
            }],
            feedback: vec![],
            changes_requested: false,
            operation: None,
            created_at: now,
            updated_at: now,
        };
        self.commit(Event::RequestSubmitted {
            request: Box::new(request.clone()),
        })?;
        Ok(request)
    }
    pub fn request_changes(
        &mut self,
        actor: &PrincipalId,
        id: &str,
        revision: u64,
        body: &str,
        key: &str,
        now: u64,
    ) -> Result<ReviewRequest, Error> {
        self.reviewer(actor)?;
        validate_key(key)?;
        let request = self.review_request(actor, id)?;
        if body.trim().is_empty() || body.len() > 4000 {
            return Err(Error::Invalid("feedback needs 1 to 4000 bytes".into()));
        }
        if let Some(existing) = request.feedback.iter().find(|f| f.id == key) {
            let artifact = revision
                .checked_sub(1)
                .and_then(|r| request.revisions.get(r as usize))
                .map(|r| &r.artifact);
            return if existing.actor == *actor
                && existing.body == body
                && artifact == Some(&existing.artifact)
            {
                Ok(request.clone())
            } else {
                Err(Error::Conflict)
            };
        }
        if request.revision() != revision || request.operation.is_some() {
            return Err(Error::Conflict);
        }
        let feedback = Feedback {
            id: key.into(),
            actor: actor.clone(),
            body: body.into(),
            artifact: request.artifact().clone(),
            at: now,
        };
        self.commit(Event::RequestReturned {
            id: id.into(),
            feedback,
        })?;
        Ok(self.state.requests[id].clone())
    }
    pub fn revise_request(
        &mut self,
        actor: &PrincipalId,
        id: &str,
        revision: u64,
        artifact: &ArtifactId,
        now: u64,
    ) -> Result<ReviewRequest, Error> {
        if self.actor(actor)?.role != Role::Agent {
            return Err(Error::Unauthorized);
        }
        let request = self.review_request(actor, id)?;
        if request.submitted_by != *actor {
            return Err(Error::Unauthorized);
        }
        if request.revision() == revision.saturating_add(1) && request.artifact() == artifact {
            return Ok(request.clone());
        }
        if request.operation.is_some()
            || request.revision() != revision
            || request.artifact() == artifact
        {
            return Err(Error::Conflict);
        }
        if !lineage::can_read(&self.state.evidence, actor, artifact) {
            return Err(Error::NotFound);
        }
        self.commit(Event::RequestRevised {
            id: id.into(),
            artifact: artifact.clone(),
            at: now,
        })?;
        Ok(self.state.requests[id].clone())
    }
    pub fn release_request(
        &mut self,
        actor: &PrincipalId,
        id: &str,
        revision: u64,
        context: &digest::Digest,
        now: u64,
    ) -> Result<PublishResult, Error> {
        self.reviewer(actor)?;
        let request = self.review_request(actor, id)?.clone();
        crate::plans::enforce_request(&self.state, &request)?;
        if request.revision() != revision || request.changes_requested {
            return Err(Error::Conflict);
        }
        let key = release_key(&request.id, revision);
        if let Some(operation) = self.state.operations.get(&key).cloned() {
            if operation.context.digest() != *context {
                return Err(Error::Conflict);
            }
            if request.operation.is_none() {
                self.commit(Event::RequestReleased {
                    id: id.into(),
                    operation: key,
                })?;
            }
            return Ok(PublishResult::Operation {
                operation: Box::new(operation),
            });
        }
        let intent = Intent {
            artifact: request.artifact().clone(),
            audience: request.audience,
            requester: actor.clone(),
        };
        self.approve(actor, &intent, context, &key, now)?;
        let result = self.publish(&intent, context, &key, now)?;
        if matches!(result, PublishResult::Operation { .. })
            && self.state.requests[id].operation.is_none()
        {
            self.commit(Event::RequestReleased {
                id: id.into(),
                operation: key,
            })?;
        }
        Ok(result)
    }
    pub fn visible_requests(&self, actor: &PrincipalId) -> Vec<&ReviewRequest> {
        self.state
            .requests
            .values()
            .filter(|r| self.review_request(actor, &r.id).is_ok())
            .collect()
    }
    pub fn review_request(&self, actor: &PrincipalId, id: &str) -> Result<&ReviewRequest, Error> {
        self.actor(actor)?;
        let request = self.state.requests.get(id).ok_or(Error::NotFound)?;
        if (request.submitted_by != *actor && self.reviewer(actor).is_err())
            || request
                .revisions
                .iter()
                .any(|r| !lineage::can_read(&self.state.evidence, actor, &r.artifact))
        {
            return Err(Error::NotFound);
        }
        Ok(request)
    }
}
pub(crate) fn reduce(state: &mut State, event: &Event) -> Result<(), Error> {
    match event {
        Event::RequestSubmitted { request } => {
            if state.requests.contains_key(&request.id) || request.revisions.len() != 1 {
                return Err(Error::Conflict);
            }
            state.requests.insert(request.id.clone(), *request.clone());
        }
        Event::RequestReturned { id, feedback } => {
            let request = state.requests.get_mut(id).ok_or(Error::NotFound)?;
            if request.operation.is_some()
                || request.artifact() != &feedback.artifact
                || request.feedback.iter().any(|f| f.id == feedback.id)
            {
                return Err(Error::Conflict);
            }
            // Returning a request revokes matching approvals in the same durable
            // event, including approvals created via the lower-level API.
            for approval in state.evidence.approvals.values().filter(|a| {
                a.context.artifact == feedback.artifact && a.context.audience.id == request.audience
            }) {
                state.evidence.revoked.insert(approval.id.clone());
            }
            request.feedback.push(feedback.clone());
            request.changes_requested = true;
            request.updated_at = feedback.at;
        }
        Event::RequestRevised { id, artifact, at } => {
            let request = state.requests.get_mut(id).ok_or(Error::NotFound)?;
            if request.operation.is_some()
                || request.artifact() == artifact
                || !state.evidence.artifacts.contains_key(artifact)
            {
                return Err(Error::Conflict);
            }
            request.revisions.push(RequestRevision {
                artifact: artifact.clone(),
                at: *at,
            });
            request.changes_requested = false;
            request.updated_at = *at;
        }
        Event::RequestReleased { id, operation } => {
            let request = state.requests.get_mut(id).ok_or(Error::NotFound)?;
            let op = state.operations.get(operation).ok_or(Error::NotFound)?;
            if request
                .operation
                .as_ref()
                .is_some_and(|key| key != operation)
                || request.changes_requested
                || op.intent.artifact != *request.artifact()
                || op.intent.audience != request.audience
            {
                return Err(Error::Conflict);
            }
            request.operation = Some(operation.clone());
            request.updated_at = op.updated_at;
        }
        _ => return Err(Error::Conflict),
    }
    Ok(())
}

fn release_key(id: &str, revision: u64) -> String {
    format!(
        "release-{}",
        digest::structured("request-release", &(id, revision)).0
    )
}

/// The reservation and request association are one journal event. Replaying
/// older journals also reconstructs a missing post-dispatch linkage.
pub(crate) fn link_operation(state: &mut State, operation: &Operation) -> Result<(), Error> {
    for request in state
        .requests
        .values_mut()
        .filter(|r| release_key(&r.id, r.revision()) == operation.key)
    {
        if request.changes_requested
            || request.artifact() != &operation.intent.artifact
            || request.audience != operation.intent.audience
            || request
                .operation
                .as_ref()
                .is_some_and(|key| key != &operation.key)
        {
            return Err(Error::Conflict);
        }
        request.operation = Some(operation.key.clone());
        request.updated_at = operation.updated_at;
    }
    Ok(())
}
