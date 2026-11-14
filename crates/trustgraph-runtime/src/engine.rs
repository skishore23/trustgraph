use crate::{
    journal::Journal,
    publisher::{Acknowledgment, Outcome, Payload, Publisher},
    *,
};
use trustgraph_core::{bundle::VerifiedBundle, *};

/// One owned coordinator serializes state changes and dispatch. Callers cannot
/// obtain a mutable evidence reference. Journal failures require reconstruction.
pub struct Coordinator<J, P> {
    pub journal: J,
    pub publisher: P,
    pub(crate) state: State,
    healthy: bool,
}
impl<J: Journal, P: Publisher> Coordinator<J, P> {
    pub fn open(journal: J, publisher: P) -> Result<Self, Error> {
        Self::open_with_evidence(journal, publisher, fixtures::northstar())
    }
    /// The supplied genesis is used only for an empty journal. Persisted evidence
    /// always wins; callers selecting a scenario must verify its workspace.
    pub fn open_with_evidence(
        mut journal: J,
        publisher: P,
        genesis: Evidence,
    ) -> Result<Self, Error> {
        let mut events = journal.events()?;
        if events.is_empty() {
            let event = Event::Initialized {
                evidence: Box::new(genesis),
            };
            journal.append(&event)?;
            events.push(event);
        }
        let Event::Initialized { evidence } = &events[0] else {
            return Err(Error::Storage("missing genesis".into()));
        };
        let mut state = State {
            evidence: *evidence.clone(),
            operations: Default::default(),
            requests: Default::default(),
            plans: Default::default(),
            plan_runs: Default::default(),
            policy_versions: Default::default(),
            source_authority: evidence
                .sources
                .iter()
                .map(|(id, s)| {
                    (
                        id.clone(),
                        evidence
                            .acls
                            .get(&s.acl)
                            .map(|a| a.readers.clone())
                            .unwrap_or_default(),
                    )
                })
                .collect(),
            destination_authority: evidence.audiences.clone(),
        };
        for event in events.iter().skip(1) {
            reduce(&mut state, event)?;
        }
        let mut coordinator = Self {
            journal,
            publisher,
            state,
            healthy: true,
        };
        // A reservation cannot have sent. A dispatch record might have sent.
        let unfinished: Vec<_> = coordinator
            .state
            .operations
            .values()
            .filter(|op| {
                matches!(
                    op.state,
                    OperationState::Reserved | OperationState::Dispatching
                )
            })
            .cloned()
            .collect();
        for mut operation in unfinished {
            operation.state = if operation.state == OperationState::Reserved {
                OperationState::FailedConfirmed
            } else {
                OperationState::OutcomeUnknown
            };
            coordinator.commit(Event::OperationRecorded {
                operation: Box::new(operation),
            })?;
        }
        Ok(coordinator)
    }
    pub fn state(&self) -> &State {
        &self.state
    }
    fn ensure_healthy(&self) -> Result<(), Error> {
        if self.healthy {
            Ok(())
        } else {
            Err(Error::RecoveryRequired)
        }
    }
    pub(crate) fn actor(&self, actor: &PrincipalId) -> Result<&Principal, Error> {
        self.ensure_healthy()?;
        self.state
            .evidence
            .principals
            .get(actor)
            .filter(|p| p.enabled && p.id == *actor && p.workspace == self.state.evidence.workspace)
            .ok_or(Error::Unauthorized)
    }
    pub(crate) fn reviewer(&self, actor: &PrincipalId) -> Result<(), Error> {
        let p = self.actor(actor)?;
        if p.role != Role::Human || !self.state.evidence.policy.approvers.contains(actor) {
            return Err(Error::Unauthorized);
        }
        Ok(())
    }
    pub(crate) fn commit(&mut self, event: Event) -> Result<(), Error> {
        self.ensure_healthy()?;
        let mut next = self.state.clone();
        reduce(&mut next, &event)?;
        if let Err(error) = self.journal.append(&event) {
            self.healthy = false;
            return Err(error);
        }
        self.state = next;
        Ok(())
    }
    /// A deterministic synthetic generator. Every supplied task input is
    /// assembled here; callers cannot submit a self-declared model manifest.
    pub fn generate(
        &mut self,
        actor: &PrincipalId,
        restricted: bool,
        key: &str,
    ) -> Result<ArtifactId, Error> {
        self.actor(actor)?;
        validate_key(key)?;
        let mut inputs = vec![ArtifactId::from("task-prompt"), "roadmap".into()];
        if restricted {
            inputs.push("launch-plan".into());
        }
        for id in &inputs {
            if !lineage::can_read(&self.state.evidence, actor, id) {
                return Err(Error::Unauthorized);
            }
        }
        let content = format!(
            "Northstar launch update\n\n{}",
            inputs
                .iter()
                .skip(1)
                .map(|id| self.state.evidence.artifacts[id].content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        );
        let id = ArtifactId(format!("draft-{key}"));
        self.create_derived(actor, id, inputs, content, None, "synthetic-launch-v1")
    }
    pub fn edit(
        &mut self,
        actor: &PrincipalId,
        parent: &ArtifactId,
        content: &str,
        key: &str,
    ) -> Result<ArtifactId, Error> {
        self.actor(actor)?;
        validate_key(key)?;
        if content.is_empty() || content.len() > 65_536 {
            return Err(Error::Invalid(
                "draft must contain 1 to 65536 UTF-8 bytes".into(),
            ));
        }
        if !lineage::can_read(&self.state.evidence, actor, parent) {
            return Err(Error::Unauthorized);
        }
        let original = self
            .state
            .evidence
            .artifacts
            .get(parent)
            .ok_or(Error::NotFound)?
            .clone();
        self.create_derived(
            actor,
            ArtifactId(format!("edit-{key}")),
            vec![parent.clone()],
            content.into(),
            Some(original),
            "human-edit-v1",
        )
    }
    pub(crate) fn create_derived(
        &mut self,
        actor: &PrincipalId,
        id: ArtifactId,
        inputs: Vec<ArtifactId>,
        content: String,
        parent: Option<Artifact>,
        provider: &str,
    ) -> Result<ArtifactId, Error> {
        let tid = TransformationId(format!("transform-{}", id.0));
        let manifest = Manifest {
            complete: true,
            provider: provider.into(),
            template_revision: "launch-1".into(),
            inputs: inputs
                .into_iter()
                .enumerate()
                .map(|(order, artifact)| InputUse {
                    occurrence: format!("{}-input-{order}", tid.0),
                    role: if artifact.0 == "task-prompt" {
                        InputRole::Prompt
                    } else if parent.is_some() {
                        InputRole::PreviousDraft
                    } else {
                        InputRole::Document
                    },
                    artifact,
                    order,
                })
                .collect(),
        };
        let transformation = Transformation {
            id: tid.clone(),
            workspace: self.state.evidence.workspace.clone(),
            output: id.clone(),
            manifest_hash: digest::structured("context-manifest", &manifest),
            manifest,
        };
        let mut artifact = Artifact::new(
            id.clone(),
            self.state.evidence.workspace.clone(),
            content,
            Origin::Transformation(tid),
        );
        if let Some(parent) = parent {
            artifact.logical_id = parent.logical_id;
            artifact.version = if let Some(existing) = self.state.evidence.artifacts.get(&id) {
                existing.version
            } else {
                self.state
                    .evidence
                    .artifacts
                    .values()
                    .filter(|a| a.logical_id == artifact.logical_id)
                    .map(|a| a.version)
                    .max()
                    .unwrap_or(parent.version)
                    .checked_add(1)
                    .ok_or(Error::Conflict)?
            };
        }
        if let Some(existing) = self.state.evidence.artifacts.get(&id) {
            return if existing == &artifact
                && self.state.evidence.transformations.get(&transformation.id)
                    == Some(&transformation)
            {
                Ok(id)
            } else {
                Err(Error::Conflict)
            };
        }
        // Refuse malformed or over-limit new lineage before storing it.
        let mut candidate = self.state.evidence.clone();
        candidate.artifacts.insert(id.clone(), artifact.clone());
        candidate
            .transformations
            .insert(transformation.id.clone(), transformation.clone());
        if !lineage::can_read(&candidate, actor, &id) {
            return Err(Error::Invalid(
                "generated lineage is incomplete or exceeds bounds".into(),
            ));
        }
        self.commit(Event::DraftCreated {
            artifact,
            transformation,
        })?;
        Ok(id)
    }
    pub fn approve(
        &mut self,
        actor: &PrincipalId,
        intent: &Intent,
        expected: &digest::Digest,
        key: &str,
        now: u64,
    ) -> Result<ApprovalId, Error> {
        self.reviewer(actor)?;
        crate::plans::enforce_intent(&self.state, intent)?;
        validate_key(key)?;
        let mut snapshot = self.state.evidence.clone();
        snapshot.approvals.clear();
        let mut review_intent = intent.clone();
        review_intent.requester = actor.clone();
        let decision = verify(&snapshot, &review_intent, now);
        let context = decision
            .context()
            .ok_or_else(|| Error::Invalid("source or audience gate blocks approval".into()))?
            .clone();
        if context.digest() != *expected {
            return Err(Error::Conflict);
        }
        let id = ApprovalId(format!("approval-{key}"));
        if let Some(existing) = self.state.evidence.approvals.get(&id) {
            return if existing.signer == *actor && existing.context == context {
                Ok(id)
            } else {
                Err(Error::Conflict)
            };
        }
        let expires_at = now
            .checked_add(snapshot.policy.max_approval_lifetime)
            .ok_or(Error::Conflict)?;
        self.commit(Event::Approved {
            approval: Approval {
                id: id.clone(),
                signer: actor.clone(),
                context,
                issued_at: now,
                expires_at,
            },
        })?;
        Ok(id)
    }
    pub fn revoke(&mut self, actor: &PrincipalId, id: &ApprovalId) -> Result<(), Error> {
        self.reviewer(actor)?;
        if !self.state.evidence.approvals.contains_key(id) {
            return Err(Error::NotFound);
        }
        if self.state.evidence.revoked.contains(id) {
            return Ok(());
        }
        self.commit(Event::Revoked { id: id.clone() })
    }
    pub fn change_acl(
        &mut self,
        actor: &PrincipalId,
        source: &SourceId,
        readers: Readers,
        key: &str,
    ) -> Result<(), Error> {
        self.reviewer(actor)?;
        if !self
            .state
            .source_authority
            .get(source)
            .is_some_and(|ceiling| readers.is_subset(ceiling))
        {
            return Err(Error::Unauthorized);
        }
        validate_key(key)?;
        if readers.iter().any(|id| {
            !self
                .state
                .evidence
                .principals
                .get(id)
                .is_some_and(|p| p.enabled && p.workspace == self.state.evidence.workspace)
        }) {
            return Err(Error::Invalid("unknown ACL principal".into()));
        }
        let current = self
            .state
            .evidence
            .sources
            .get(source)
            .ok_or(Error::NotFound)?;
        let old = &self.state.evidence.acls[&current.acl];
        let id = AclId(format!("acl-{key}"));
        if let Some(existing) = self.state.evidence.acls.get(&id) {
            return if existing.readers == readers && current.acl == id {
                Ok(())
            } else {
                Err(Error::Conflict)
            };
        }
        let acl = Acl {
            id,
            workspace: self.state.evidence.workspace.clone(),
            revision: old.revision.checked_add(1).ok_or(Error::Conflict)?,
            readers,
        };
        self.commit(Event::AclUpdated {
            source: source.clone(),
            acl,
        })
    }
    pub fn change_audience(
        &mut self,
        actor: &PrincipalId,
        mut audience: Audience,
    ) -> Result<(), Error> {
        self.reviewer(actor)?;
        if !self
            .state
            .destination_authority
            .get(&audience.id)
            .is_some_and(|ceiling| audience.recipients.is_subset(&ceiling.recipients))
        {
            return Err(Error::Unauthorized);
        }
        let old = self
            .state
            .evidence
            .audiences
            .get(&audience.id)
            .ok_or(Error::NotFound)?;
        if audience.workspace != old.workspace
            || audience.revision != old.revision
            || !audience.destination.starts_with("sim://northstar/")
        {
            return Err(Error::Conflict);
        }
        audience.revision = audience.revision.checked_add(1).ok_or(Error::Conflict)?;
        self.commit(Event::AudienceUpdated { audience })
    }
    pub fn change_policy(&mut self, actor: &PrincipalId) -> Result<(), Error> {
        self.reviewer(actor)?;
        let mut policy = self.state.evidence.policy.clone();
        policy.revision = policy.revision.checked_add(1).ok_or(Error::Conflict)?;
        self.commit(Event::PolicyUpdated { policy })
    }
    pub fn disable_principal(
        &mut self,
        actor: &PrincipalId,
        id: &PrincipalId,
    ) -> Result<(), Error> {
        self.reviewer(actor)?;
        let mut principal = self
            .state
            .evidence
            .principals
            .get(id)
            .ok_or(Error::NotFound)?
            .clone();
        principal.enabled = false;
        self.commit(Event::PrincipalUpdated { principal })
    }
    pub fn publish(
        &mut self,
        intent: &Intent,
        expected: &digest::Digest,
        key: &str,
        now: u64,
    ) -> Result<PublishResult, Error> {
        self.actor(&intent.requester)?;
        crate::plans::enforce_intent(&self.state, intent)?;
        validate_key(key)?;
        if let Some(existing) = self.state.operations.get(key) {
            return if existing.intent == *intent && existing.context.digest() == *expected {
                Ok(PublishResult::Operation {
                    operation: Box::new(existing.clone()),
                })
            } else {
                Err(Error::Conflict)
            };
        }
        let verified = match VerifiedBundle::check(&self.state.evidence, intent, now) {
            Ok(bundle) => bundle,
            Err(decision) => {
                return Ok(PublishResult::Blocked { decision });
            }
        };
        if verified.context().digest() != *expected {
            return Err(Error::Conflict);
        }
        let mut operation = Operation {
            key: key.into(),
            intent: intent.clone(),
            context: verified.context().clone(),
            approval: verified.approval().clone(),
            bundle: verified.export().clone(),
            state: OperationState::Reserved,
            receipt: None,
            updated_at: now,
        };
        self.commit(Event::OperationRecorded {
            operation: Box::new(operation.clone()),
        })?;
        operation.state = OperationState::Dispatching;
        self.commit(Event::OperationRecorded {
            operation: Box::new(operation.clone()),
        })?;
        // &mut self plus process ownership keeps policy mutations out of this
        // verification-to-send interval. Bytes come from the checked bundle.
        let payload = Payload {
            operation: key.into(),
            destination: operation.context.audience.destination.clone(),
            content: operation.bundle.evidence.artifacts[&intent.artifact]
                .content
                .clone(),
            hash: operation.context.artifact_hash.clone(),
        };
        match self.publisher.send(&payload) {
            Ok((Outcome::Accepted, Some(ack))) if ack.payload == payload => {
                operation.state = OperationState::Published;
                operation.receipt = Some(receipt(&operation, &ack, now));
            }
            Ok((Outcome::Rejected, _)) => operation.state = OperationState::FailedConfirmed,
            _ => operation.state = OperationState::OutcomeUnknown,
        }
        self.commit(Event::OperationRecorded {
            operation: Box::new(operation.clone()),
        })?;
        Ok(PublishResult::Operation {
            operation: Box::new(operation),
        })
    }
    pub fn reconcile(
        &mut self,
        actor: &PrincipalId,
        key: &str,
        now: u64,
    ) -> Result<Operation, Error> {
        self.actor(actor)?;
        let mut operation = self
            .state
            .operations
            .get(key)
            .ok_or(Error::NotFound)?
            .clone();
        if operation.intent.requester != *actor
            && !self.state.evidence.policy.approvers.contains(actor)
        {
            return Err(Error::Unauthorized);
        }
        if operation.state != OperationState::OutcomeUnknown {
            return Ok(operation);
        }
        if let Some(ack) = self.publisher.lookup(key)? {
            let content = &operation.bundle.evidence.artifacts[&operation.intent.artifact].content;
            if ack.payload.destination != operation.context.audience.destination
                || ack.payload.hash != operation.context.artifact_hash
                || ack.payload.operation != key
                || ack.payload.content != *content
            {
                return Err(Error::Conflict);
            }
            operation.state = OperationState::Published;
            operation.updated_at = now;
            operation.receipt = Some(receipt(&operation, &ack, now));
            self.commit(Event::OperationRecorded {
                operation: Box::new(operation.clone()),
            })?;
        }
        Ok(operation)
    }
}
pub(crate) fn validate_key(key: &str) -> Result<(), Error> {
    if key.is_empty()
        || key.len() > 96
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(Error::Invalid(
            "key must be 1 to 96 ASCII letters, digits, hyphens, underscores or dots".into(),
        ));
    }
    Ok(())
}
fn receipt(operation: &Operation, ack: &Acknowledgment, now: u64) -> Receipt {
    Receipt {
        operation: operation.key.clone(),
        context_digest: operation.context.digest(),
        artifact_hash: operation.context.artifact_hash.clone(),
        destination: operation.context.audience.destination.clone(),
        provider_id: ack.provider_id.clone(),
        observed_at: now,
        simulated: true,
    }
}

fn reduce(state: &mut State, event: &Event) -> Result<(), Error> {
    let e = &mut state.evidence;
    match event {
        Event::PolicyProposed { .. } | Event::PolicyActivated { .. } => {
            return crate::policy::reduce(state, event);
        }
        Event::PlanProposed { .. }
        | Event::PlanRunReserved { .. }
        | Event::PlanRunPrepared { .. } => return crate::plans::reduce(state, event),
        Event::RequestSubmitted { .. }
        | Event::RequestReturned { .. }
        | Event::RequestRevised { .. }
        | Event::RequestReleased { .. } => return crate::submissions::reduce(state, event),
        Event::Initialized { .. } => return Err(Error::Conflict),
        Event::DraftCreated {
            artifact,
            transformation,
        } => {
            if e.artifacts.contains_key(&artifact.id)
                || e.transformations.contains_key(&transformation.id)
                || e.artifacts
                    .values()
                    .any(|a| a.logical_id == artifact.logical_id && a.version == artifact.version)
            {
                return Err(Error::Conflict);
            }
            e.artifacts.insert(artifact.id.clone(), artifact.clone());
            e.transformations
                .insert(transformation.id.clone(), transformation.clone());
        }
        Event::Approved { approval } => {
            if e.approvals
                .insert(approval.id.clone(), approval.clone())
                .is_some()
            {
                return Err(Error::Conflict);
            }
        }
        Event::Revoked { id } => {
            if !e.approvals.contains_key(id) {
                return Err(Error::NotFound);
            }
            e.revoked.insert(id.clone());
        }
        Event::AclUpdated { source, acl } => {
            if !state
                .source_authority
                .get(source)
                .is_some_and(|ceiling| acl.readers.is_subset(ceiling))
            {
                return Err(Error::Conflict);
            }
            let source = e.sources.get_mut(source).ok_or(Error::NotFound)?;
            if e.acls.contains_key(&acl.id)
                || acl.workspace != e.workspace
                || acl.revision != e.acls[&source.acl].revision + 1
            {
                return Err(Error::Conflict);
            }
            source.acl = acl.id.clone();
            e.acls.insert(acl.id.clone(), acl.clone());
        }
        Event::AudienceUpdated { audience } => {
            if !state
                .destination_authority
                .get(&audience.id)
                .is_some_and(|ceiling| audience.recipients.is_subset(&ceiling.recipients))
            {
                return Err(Error::Conflict);
            }
            let old = e.audiences.get(&audience.id).ok_or(Error::NotFound)?;
            if audience.workspace != e.workspace || audience.revision != old.revision + 1 {
                return Err(Error::Conflict);
            }
            e.audiences.insert(audience.id.clone(), audience.clone());
        }
        Event::PolicyUpdated { policy } => {
            if policy.workspace != e.workspace || policy.revision != e.policy.revision + 1 {
                return Err(Error::Conflict);
            }
            e.policy = policy.clone();
        }
        Event::PrincipalUpdated { principal } => {
            if principal.workspace != e.workspace || !e.principals.contains_key(&principal.id) {
                return Err(Error::Conflict);
            }
            e.principals.insert(principal.id.clone(), principal.clone());
        }
        Event::OperationRecorded { operation } => {
            if operation.context != *operation.bundle.decision.context().ok_or(Error::Conflict)?
                || operation.intent != operation.bundle.intent
                || trustgraph_core::bundle::replay(&operation.bundle).is_err()
            {
                return Err(Error::Conflict);
            }
            if let Some(old) = state.operations.get(&operation.key) {
                let mut allowed = old.clone();
                allowed.state = operation.state;
                allowed.receipt = operation.receipt.clone();
                allowed.updated_at = operation.updated_at;
                let transition = matches!(
                    (old.state, operation.state),
                    (
                        OperationState::Reserved,
                        OperationState::Dispatching | OperationState::FailedConfirmed
                    ) | (
                        OperationState::Dispatching,
                        OperationState::Published
                            | OperationState::FailedConfirmed
                            | OperationState::OutcomeUnknown
                    ) | (OperationState::OutcomeUnknown, OperationState::Published)
                );
                if allowed != **operation || !transition {
                    return Err(Error::Conflict);
                }
            } else if operation.state != OperationState::Reserved {
                return Err(Error::Conflict);
            }
            if (operation.state == OperationState::Published) != operation.receipt.is_some() {
                return Err(Error::Conflict);
            }
            if let Some(r) = &operation.receipt
                && (r.operation != operation.key
                    || r.context_digest != operation.context.digest()
                    || r.artifact_hash != operation.context.artifact_hash
                    || r.destination != operation.context.audience.destination
                    || r.provider_id.is_empty())
            {
                return Err(Error::Conflict);
            }
            state
                .operations
                .insert(operation.key.clone(), *operation.clone());
            crate::submissions::link_operation(state, operation)?;
        }
    }
    Ok(())
}
