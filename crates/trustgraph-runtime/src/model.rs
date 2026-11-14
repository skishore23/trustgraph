use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use trustgraph_core::{bundle::Bundle, digest::Digest, *};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OperationState {
    Reserved,
    Dispatching,
    Published,
    FailedConfirmed,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub operation: String,
    pub context_digest: Digest,
    pub artifact_hash: Digest,
    pub destination: String,
    pub provider_id: String,
    pub observed_at: u64,
    pub simulated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub key: String,
    pub intent: Intent,
    pub context: Context,
    pub approval: ApprovalId,
    pub bundle: Bundle,
    pub state: OperationState,
    pub receipt: Option<Receipt>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub evidence: Evidence,
    pub operations: BTreeMap<String, Operation>,
    #[serde(default)]
    pub requests: BTreeMap<String, ReviewRequest>,
    #[serde(default)]
    pub plans: BTreeMap<String, PlanRecord>,
    #[serde(default)]
    pub plan_runs: BTreeMap<String, PlanRun>,
    #[serde(default)]
    pub policy_versions: BTreeMap<String, crate::policy::PolicyVersion>,
    #[serde(default)]
    pub source_authority: BTreeMap<SourceId, Readers>,
    #[serde(default)]
    pub destination_authority: BTreeMap<AudienceId, Audience>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRecord {
    pub plan: trustgraph_core::plan::Plan,
    pub submitted_by: PrincipalId,
    pub created_by: PrincipalId,
    pub parent: Option<String>,
    pub created_at: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStage {
    Preparing,
    AwaitingReview,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanRun {
    pub id: String,
    pub plan_id: String,
    pub plan_digest: Digest,
    pub snapshot_digest: Digest,
    pub started_by: PrincipalId,
    pub operating_agent: PrincipalId,
    pub bindings: BTreeMap<String, ArtifactId>,
    pub request: String,
    pub stage: RunStage,
    pub created_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestRevision {
    pub artifact: ArtifactId,
    pub at: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feedback {
    pub id: String,
    pub actor: PrincipalId,
    pub body: String,
    pub artifact: ArtifactId,
    pub at: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRequest {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub submitted_by: PrincipalId,
    pub audience: AudienceId,
    pub revisions: Vec<RequestRevision>,
    pub feedback: Vec<Feedback>,
    pub changes_requested: bool,
    pub operation: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}
impl ReviewRequest {
    pub fn artifact(&self) -> &ArtifactId {
        &self
            .revisions
            .last()
            .expect("request has a revision")
            .artifact
    }
    pub fn revision(&self) -> u64 {
        self.revisions.len() as u64
    }
}
#[derive(Debug, Clone)]
pub struct Submission {
    pub key: String,
    pub title: String,
    pub summary: String,
    pub artifact: ArtifactId,
    pub audience: AudienceId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    SourceImported {
        artifact: Artifact,
        source: Source,
        acl: Acl,
        imported_by: PrincipalId,
    },
    PolicyProposed {
        version: Box<crate::policy::PolicyVersion>,
    },
    PolicyActivated {
        snapshot: Box<crate::policy::PolicySnapshot>,
        activated_by: PrincipalId,
    },
    PlanProposed {
        record: Box<PlanRecord>,
    },
    PlanRunReserved {
        run: Box<PlanRun>,
    },
    PlanRunPrepared {
        id: String,
        bindings: BTreeMap<String, ArtifactId>,
    },
    RequestSubmitted {
        request: Box<ReviewRequest>,
    },
    RequestReturned {
        id: String,
        feedback: Feedback,
    },
    RequestRevised {
        id: String,
        artifact: ArtifactId,
        at: u64,
    },
    RequestReleased {
        id: String,
        operation: String,
    },
    Initialized {
        evidence: Box<Evidence>,
    },
    DraftCreated {
        artifact: Artifact,
        transformation: Transformation,
    },
    Approved {
        approval: Approval,
    },
    Revoked {
        id: ApprovalId,
    },
    AclUpdated {
        source: SourceId,
        acl: Acl,
    },
    AudienceUpdated {
        audience: Audience,
    },
    PolicyUpdated {
        policy: Policy,
    },
    PrincipalUpdated {
        principal: Principal,
    },
    OperationRecorded {
        operation: Box<Operation>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("storage unavailable or journal inconsistent: {0}")]
    Storage(String),
    #[error("caller unauthorized")]
    Unauthorized,
    #[error("request conflicts with an immutable identity or revision")]
    Conflict,
    #[error("record not found or not accessible")]
    NotFound,
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("coordinator must be reopened after uncertain storage outcome")]
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PublishResult {
    Blocked { decision: Box<Decision> },
    Operation { operation: Box<Operation> },
}
