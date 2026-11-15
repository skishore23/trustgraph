//! Permission-preserving typed dataflow for trusted application hosts.
//!
//! Functions execute outside this library. The host registers their contracts,
//! records the supplied inputs and commits their outputs. Every output inherits
//! *all* input restrictions. Type identifiers are nominal runtime contracts, not
//! Rust type-level proofs or validation of payload schemas.
//!
//! No I/O or wall clock is hidden here. Caller-provided identities, audiences,
//! source permissions and execution receipts must come from a trusted adapter.
//! This kernel cannot observe unrecorded reads or prevent an adapter bypass.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use trustgraph_core::digest;

macro_rules! id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.into())
            }
        }
    };
}
id!(ArtifactId);
id!(OperationId);
id!(ExecutionId);
id!(SchemaId);
id!(PrincipalId);
pub type Readers = BTreeSet<PrincipalId>;
pub type Ports = BTreeMap<String, SchemaId>;
pub type Inputs = BTreeMap<String, ArtifactId>;

/// A registered, versioned operation contract. Executable code belongs to its adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationDefinition {
    pub name: String,
    pub version: String,
    pub executor: PrincipalId,
    pub inputs: Ports,
    pub outputs: Ports,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Origin {
    Source,
    Output {
        execution: ExecutionId,
        port: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub logical_id: String,
    pub schema: SchemaId,
    pub content_hash: String,
    pub bytes: Vec<u8>,
    pub origin: Origin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Execution {
    pub id: ExecutionId,
    pub operation: OperationId,
    pub actor: PrincipalId,
    pub inputs: Inputs,
    pub outputs: BTreeMap<String, ArtifactId>,
    pub parameters: serde_json::Value,
    /// Host attestation, not cryptographic proof that the operation ran.
    pub receipt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourcePermission {
    pub revision: u64,
    pub readers: Readers,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequirement {
    pub reviewers: Readers,
    pub ttl_seconds: u64,
}

/// Action rules can add restrictions and obligations, never widen source access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRule {
    pub actors: Readers,
    pub approval: Option<ApprovalRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionIntent {
    pub actor: PrincipalId,
    pub action: String,
    pub artifact: ArtifactId,
    pub destination: String,
    /// Must be resolved by the trusted destination adapter, not the model.
    pub recipients: Readers,
    pub parameters: serde_json::Value,
}

/// Relevant inputs to one decision, suitable for an audit display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DecisionContext {
    pub digest: String,
    pub intent: ActionIntent,
    pub source_permissions: BTreeMap<ArtifactId, SourcePermission>,
    pub action_rule_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Decision {
    Allow { context: DecisionContext },
    NeedsApproval { context: DecisionContext },
    Deny { reason: String },
}

/// An opaque approval reference. Only `Graph::approve` can issue it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Approval {
    id: String,
    context: String,
    reviewer: PrincipalId,
    issued_at: u64,
    expires_at: u64,
}
impl Approval {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

/// An opaque handle to an admitted computation. Not serializable or forgeable.
#[derive(Debug)]
pub struct ExecutionTicket {
    id: ExecutionId,
    owner: Arc<()>,
}

/// Output payloads are supplied once; port types come from the registered contract.
#[derive(Debug, Clone)]
pub struct OutputValue {
    pub logical_id: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("unknown {0}")]
    Unknown(String),
    #[error("invalid contract: {0}")]
    Contract(String),
    #[error("permission denied: {0}")]
    Permission(String),
    #[error("evidence is stale: {0}")]
    Stale(String),
    #[error("invalid lineage: {0}")]
    Lineage(String),
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
struct Pending {
    operation: OperationId,
    inputs: Inputs,
    actor: PrincipalId,
    parameters: serde_json::Value,
    sources: BTreeMap<ArtifactId, SourcePermission>,
}

/// In-process reference kernel. Mutations belong to a trusted application host.
/// Read-only accessors expose evidence; no unchecked deserialization is provided.
#[derive(Debug, Default)]
pub struct Graph {
    identity: Arc<()>,
    artifacts: BTreeMap<ArtifactId, Artifact>,
    operations: BTreeMap<OperationId, OperationDefinition>,
    executions: BTreeMap<ExecutionId, Execution>,
    permissions: BTreeMap<ArtifactId, SourcePermission>,
    pending: BTreeMap<ExecutionId, Pending>,
    rules: BTreeMap<String, (u64, ActionRule)>,
    approvals: BTreeMap<String, Approval>,
    sequence: u64,
}

fn hash(domain: &str, value: &impl Serialize) -> String {
    digest::structured(domain, value).0
}
fn nonempty(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(Error::Contract(format!("{field} must not be empty")))
    } else {
        Ok(())
    }
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn artifacts(&self) -> &BTreeMap<ArtifactId, Artifact> {
        &self.artifacts
    }
    pub fn operations(&self) -> &BTreeMap<OperationId, OperationDefinition> {
        &self.operations
    }
    pub fn executions(&self) -> &BTreeMap<ExecutionId, Execution> {
        &self.executions
    }

    /// Import exact source bytes. A changed source creates a new artifact revision;
    /// existing descendants continue to reference the version actually supplied.
    pub fn register_source(
        &mut self,
        logical_id: impl Into<String>,
        schema: SchemaId,
        bytes: Vec<u8>,
        readers: Readers,
    ) -> Result<ArtifactId> {
        let logical_id = logical_id.into();
        nonempty(&logical_id, "logical ID")?;
        nonempty(&schema.0, "schema")?;
        let content_hash = digest::content(&bytes).0;
        let id = ArtifactId(hash(
            "framework-source-v1",
            &(&logical_id, &schema, &content_hash),
        ));
        if let Some(existing) = self.permissions.get(&id) {
            if existing.readers != readers {
                return Err(Error::Contract(
                    "existing source permissions require explicit update".into(),
                ));
            }
            return Ok(id);
        }
        self.artifacts.insert(
            id.clone(),
            Artifact {
                id: id.clone(),
                logical_id,
                schema,
                content_hash,
                bytes,
                origin: Origin::Source,
            },
        );
        self.permissions.insert(
            id.clone(),
            SourcePermission {
                revision: 1,
                readers,
            },
        );
        Ok(id)
    }

    pub fn register_operation(&mut self, definition: OperationDefinition) -> Result<OperationId> {
        nonempty(&definition.name, "operation name")?;
        nonempty(&definition.version, "operation version")?;
        nonempty(&definition.executor.0, "executor")?;
        if definition.inputs.is_empty() || definition.outputs.is_empty() {
            return Err(Error::Contract("operations require at least one input and output; register independent values as sources".into()));
        }
        for (port, schema) in definition.inputs.iter().chain(definition.outputs.iter()) {
            nonempty(port, "port")?;
            nonempty(&schema.0, "schema")?;
        }
        if self.operations.values().any(|existing| {
            existing.name == definition.name
                && existing.version == definition.version
                && existing != &definition
        }) {
            return Err(Error::Contract(
                "operation name/version already has a different contract; increment its version"
                    .into(),
            ));
        }
        let id = OperationId(hash("framework-operation-v1", &definition));
        self.operations.entry(id.clone()).or_insert(definition);
        Ok(id)
    }

    /// Check every input before the host discloses it to the executor. The returned
    /// ticket does not execute code, read files or call an LLM.
    pub fn begin_execution(
        &mut self,
        operation: &OperationId,
        inputs: Inputs,
        actor: PrincipalId,
        parameters: serde_json::Value,
    ) -> Result<ExecutionTicket> {
        let definition = self
            .operations
            .get(operation)
            .ok_or_else(|| Error::Unknown("operation".into()))?;
        if inputs.keys().ne(definition.inputs.keys()) {
            return Err(Error::Contract(
                "input ports must exactly match operation".into(),
            ));
        }
        let mut sources = BTreeMap::new();
        for (port, id) in &inputs {
            let artifact = self
                .artifacts
                .get(id)
                .ok_or_else(|| Error::Unknown("input artifact".into()))?;
            if artifact.schema != definition.inputs[port] {
                return Err(Error::Contract(format!("schema mismatch on input {port}")));
            }
            let (readers, dependencies) = self.lineage(id)?;
            if !readers.contains(&actor) || !readers.contains(&definition.executor) {
                return Err(Error::Permission(format!(
                    "actor or executor cannot read input {port}"
                )));
            }
            sources.extend(dependencies);
        }
        // Reserve one artifact level/node for each new output. All outputs share
        // the same inputs, so each has the same bounded ancestry.
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeMap::new();
        for input in inputs.values() {
            self.visit(input, &mut visiting, &mut visited, &mut sources, 1)?;
        }
        if visited.len() >= 256 {
            return Err(Error::Lineage("artifact limit exceeded".into()));
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| Error::Contract("sequence exhausted".into()))?;
        let id = ExecutionId(hash(
            "framework-execution-v1",
            &(self.sequence, operation, &inputs, &actor, &parameters),
        ));
        self.pending.insert(
            id.clone(),
            Pending {
                operation: operation.clone(),
                inputs,
                actor,
                parameters,
                sources,
            },
        );
        Ok(ExecutionTicket {
            id,
            owner: Arc::clone(&self.identity),
        })
    }

    /// Commit a computation atomically after validating exact output ports and
    /// unchanged source permissions. Every output retains every supplied input.
    pub fn finish_execution(
        &mut self,
        ticket: ExecutionTicket,
        values: BTreeMap<String, OutputValue>,
        receipt: impl Into<String>,
    ) -> Result<BTreeMap<String, ArtifactId>> {
        if !Arc::ptr_eq(&ticket.owner, &self.identity) {
            return Err(Error::Unknown(
                "execution ticket belongs to another graph".into(),
            ));
        }
        let pending = self
            .pending
            .remove(&ticket.id)
            .ok_or_else(|| Error::Unknown("execution ticket".into()))?;
        let definition = &self.operations[&pending.operation];
        if values.keys().ne(definition.outputs.keys()) {
            return Err(Error::Contract(
                "output ports must exactly match operation".into(),
            ));
        }
        for (source, previous) in &pending.sources {
            if self.permissions.get(source) != Some(previous) {
                return Err(Error::Stale(
                    "source permissions changed during execution".into(),
                ));
            }
        }
        let receipt = receipt.into();
        nonempty(&receipt, "execution receipt")?;
        let mut artifacts = Vec::new();
        let mut outputs = BTreeMap::new();
        for (port, value) in values {
            nonempty(&value.logical_id, "output logical ID")?;
            let schema = definition.outputs[&port].clone();
            let content_hash = digest::content(&value.bytes).0;
            let id = ArtifactId(hash(
                "framework-output-v1",
                &(&ticket.id, &port, &value.logical_id, &schema, &content_hash),
            ));
            outputs.insert(port.clone(), id.clone());
            artifacts.push(Artifact {
                id,
                logical_id: value.logical_id,
                schema,
                content_hash,
                bytes: value.bytes,
                origin: Origin::Output {
                    execution: ticket.id.clone(),
                    port,
                },
            });
        }
        self.executions.insert(
            ticket.id.clone(),
            Execution {
                id: ticket.id,
                operation: pending.operation,
                actor: pending.actor,
                inputs: pending.inputs,
                outputs: outputs.clone(),
                parameters: pending.parameters,
                receipt,
            },
        );
        for artifact in artifacts {
            self.artifacts.insert(artifact.id.clone(), artifact);
        }
        Ok(outputs)
    }

    pub fn cancel_execution(&mut self, ticket: ExecutionTicket) -> Result<()> {
        if !Arc::ptr_eq(&ticket.owner, &self.identity) {
            return Err(Error::Unknown(
                "execution ticket belongs to another graph".into(),
            ));
        }
        self.pending
            .remove(&ticket.id)
            .map(|_| ())
            .ok_or_else(|| Error::Unknown("execution ticket".into()))
    }

    /// Change the restrictions on an existing source version, not its bytes.
    pub fn update_source_permissions(
        &mut self,
        source: &ArtifactId,
        readers: Readers,
    ) -> Result<()> {
        let permission = self
            .permissions
            .get_mut(source)
            .ok_or_else(|| Error::Unknown("source".into()))?;
        if permission.readers != readers {
            permission.revision = permission
                .revision
                .checked_add(1)
                .ok_or_else(|| Error::Contract("permission revision exhausted".into()))?;
            permission.readers = readers;
        }
        Ok(())
    }

    pub fn readers(&self, artifact: &ArtifactId) -> Result<Readers> {
        self.lineage(artifact).map(|(readers, _)| readers)
    }

    /// Forward impact from one artifact, including itself, without pulling in sibling inputs.
    pub fn impact(&self, changed: &ArtifactId) -> Result<BTreeSet<ArtifactId>> {
        if !self.artifacts.contains_key(changed) {
            return Err(Error::Unknown("artifact".into()));
        }
        let mut affected = BTreeSet::from([changed.clone()]);
        loop {
            let before = affected.len();
            for execution in self.executions.values() {
                if execution
                    .inputs
                    .values()
                    .any(|input| affected.contains(input))
                {
                    affected.extend(execution.outputs.values().cloned());
                }
            }
            if affected.len() == before {
                return Ok(affected);
            }
        }
    }

    /// Trusted policy administration. Rule revisions only invalidate this action.
    pub fn set_action_rule(&mut self, action: impl Into<String>, rule: ActionRule) -> Result<()> {
        let action = action.into();
        nonempty(&action, "action")?;
        if let Some(requirement) = &rule.approval
            && (requirement.reviewers.is_empty() || requirement.ttl_seconds == 0)
        {
            return Err(Error::Contract(
                "approval needs reviewers and a positive lifetime".into(),
            ));
        }
        let revision = match self.rules.get(&action) {
            Some((revision, old)) if *old == rule => *revision,
            Some((revision, _)) => revision
                .checked_add(1)
                .ok_or_else(|| Error::Contract("rule revision exhausted".into()))?,
            None => 1,
        };
        self.rules.insert(action, (revision, rule));
        Ok(())
    }

    fn context(&self, intent: &ActionIntent) -> Result<(DecisionContext, &ActionRule)> {
        nonempty(&intent.destination, "destination")?;
        if intent.recipients.is_empty() {
            return Err(Error::Permission(
                "destination audience is unknown or empty".into(),
            ));
        }
        let (revision, rule) = self
            .rules
            .get(&intent.action)
            .ok_or_else(|| Error::Permission("no action rule".into()))?;
        if !rule.actors.contains(&intent.actor) {
            return Err(Error::Permission("actor cannot perform action".into()));
        }
        let (readers, sources) = self.lineage(&intent.artifact)?;
        if !readers.contains(&intent.actor)
            || !trustgraph_core::permissions::can_flow(&readers, &intent.recipients)
        {
            return Err(Error::Permission(
                "actor or destination is outside inherited readers".into(),
            ));
        }
        let digest = hash("framework-decision-v1", &(intent, &sources, revision, rule));
        Ok((
            DecisionContext {
                digest,
                intent: intent.clone(),
                source_permissions: sources,
                action_rule_revision: *revision,
            },
            rule,
        ))
    }

    /// Always recheck immediately before a trusted adapter performs an effect.
    /// This is not an atomic transaction with an external system.
    pub fn check_action(
        &self,
        intent: &ActionIntent,
        approval: Option<&Approval>,
        now: u64,
    ) -> Decision {
        let (context, rule) = match self.context(intent) {
            Ok(value) => value,
            Err(error) => {
                return Decision::Deny {
                    reason: error.to_string(),
                };
            }
        };
        match (&rule.approval, approval) {
            (None, _) => Decision::Allow { context },
            (Some(_), None) => Decision::NeedsApproval { context },
            (Some(requirement), Some(approval)) => {
                let valid = self.approvals.get(&approval.id) == Some(approval)
                    && approval.context == context.digest
                    && approval.issued_at <= now
                    && now < approval.expires_at
                    && requirement.reviewers.contains(&approval.reviewer);
                if valid {
                    Decision::Allow { context }
                } else {
                    Decision::Deny {
                        reason: "approval is stale, expired, revoked or mismatched".into(),
                    }
                }
            }
        }
    }

    /// Reviewer identity must be authenticated by the integrating host.
    pub fn approve(
        &mut self,
        intent: &ActionIntent,
        reviewer: PrincipalId,
        now: u64,
    ) -> Result<Approval> {
        let (context, rule) = self.context(intent)?;
        let requirement = rule
            .approval
            .as_ref()
            .ok_or_else(|| Error::Contract("this action does not require approval".into()))?;
        if !requirement.reviewers.contains(&reviewer) || reviewer == intent.actor {
            return Err(Error::Permission(
                "reviewer is unauthorized or is the requester".into(),
            ));
        }
        if !self.readers(&intent.artifact)?.contains(&reviewer) {
            return Err(Error::Permission("reviewer cannot read artifact".into()));
        }
        let expires_at = now
            .checked_add(requirement.ttl_seconds)
            .ok_or_else(|| Error::Contract("approval expiration overflow".into()))?;
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| Error::Contract("sequence exhausted".into()))?;
        let id = hash(
            "framework-approval-v1",
            &(self.sequence, &context.digest, &reviewer, now, expires_at),
        );
        let approval = Approval {
            id,
            context: context.digest,
            reviewer,
            issued_at: now,
            expires_at,
        };
        self.approvals.insert(approval.id.clone(), approval.clone());
        Ok(approval)
    }

    pub fn revoke_approval(&mut self, approval: &Approval) -> bool {
        self.approvals.remove(&approval.id).is_some()
    }

    fn lineage(
        &self,
        id: &ArtifactId,
    ) -> Result<(Readers, BTreeMap<ArtifactId, SourcePermission>)> {
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeMap::new();
        let mut sources = BTreeMap::new();
        self.visit(id, &mut visiting, &mut visited, &mut sources, 0)?;
        let mut values = sources.values();
        let mut readers = values
            .next()
            .ok_or_else(|| Error::Lineage("no source dependencies".into()))?
            .readers
            .clone();
        for permission in values {
            readers = trustgraph_core::permissions::intersect(&readers, &permission.readers);
        }
        Ok((readers, sources))
    }

    fn visit(
        &self,
        id: &ArtifactId,
        visiting: &mut BTreeSet<ArtifactId>,
        visited: &mut BTreeMap<ArtifactId, usize>,
        sources: &mut BTreeMap<ArtifactId, SourcePermission>,
        depth: usize,
    ) -> Result<usize> {
        if depth > 64 {
            return Err(Error::Lineage("depth limit exceeded".into()));
        }
        if let Some(height) = visited.get(id) {
            if depth + height > 64 {
                return Err(Error::Lineage("depth limit exceeded".into()));
            }
            return Ok(*height);
        }
        if visited.len() + visiting.len() >= 256 {
            return Err(Error::Lineage("artifact limit exceeded".into()));
        }
        if !visiting.insert(id.clone()) {
            return Err(Error::Lineage("cycle".into()));
        }
        let artifact = self
            .artifacts
            .get(id)
            .ok_or_else(|| Error::Unknown("artifact".into()))?;
        if digest::content(&artifact.bytes).0 != artifact.content_hash {
            return Err(Error::Lineage("content digest mismatch".into()));
        }
        let height = match &artifact.origin {
            Origin::Source => {
                sources.insert(
                    id.clone(),
                    self.permissions
                        .get(id)
                        .ok_or_else(|| Error::Unknown("source permissions".into()))?
                        .clone(),
                );
                0
            }
            Origin::Output { execution, port } => {
                let execution = self
                    .executions
                    .get(execution)
                    .ok_or_else(|| Error::Unknown("execution".into()))?;
                if execution.outputs.get(port) != Some(id) {
                    return Err(Error::Lineage("producer mismatch".into()));
                }
                let mut max_height = 0;
                for input in execution.inputs.values() {
                    max_height =
                        max_height.max(self.visit(input, visiting, visited, sources, depth + 1)?);
                }
                max_height + 1
            }
        };
        visiting.remove(id);
        visited.insert(id.clone(), height);
        Ok(height)
    }
}
