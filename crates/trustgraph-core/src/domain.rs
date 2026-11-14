use crate::digest::{self, Digest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

macro_rules! ids {
    ($($name:ident),*) => {$ (
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl From<&str> for $name { fn from(s: &str) -> Self { Self(s.into()) } }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }
        }
    )*};
}
ids!(
    WorkspaceId,
    PrincipalId,
    ArtifactId,
    SourceId,
    AclId,
    TransformationId,
    AudienceId,
    ApprovalId
);
pub type Readers = BTreeSet<PrincipalId>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Human,
    Agent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Principal {
    pub id: PrincipalId,
    pub workspace: WorkspaceId,
    pub role: Role,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Origin {
    Source(SourceId),
    Transformation(TransformationId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: ArtifactId,
    pub logical_id: String,
    pub version: u64,
    pub workspace: WorkspaceId,
    pub media_type: String,
    pub content: String,
    pub hash: Digest,
    pub origin: Origin,
}
impl Artifact {
    pub fn new(id: ArtifactId, workspace: WorkspaceId, content: String, origin: Origin) -> Self {
        Self {
            logical_id: id.0.clone(),
            id,
            version: 1,
            workspace,
            media_type: "text/plain; charset=utf-8".into(),
            hash: digest::content(content.as_bytes()),
            content,
            origin,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acl {
    pub id: AclId,
    pub workspace: WorkspaceId,
    pub revision: u64,
    pub readers: Readers,
}

/// Current permission pointer for an immutable source content revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub workspace: WorkspaceId,
    pub artifact: ArtifactId,
    pub acl: AclId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputRole {
    Document,
    PreviousDraft,
    Prompt,
    ToolOutput,
    Memory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputUse {
    pub occurrence: String,
    pub artifact: ArtifactId,
    pub role: InputRole,
    pub order: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub complete: bool,
    pub provider: String,
    pub template_revision: String,
    pub inputs: Vec<InputUse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transformation {
    pub id: TransformationId,
    pub workspace: WorkspaceId,
    pub output: ArtifactId,
    pub manifest: Manifest,
    pub manifest_hash: Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audience {
    pub id: AudienceId,
    pub workspace: WorkspaceId,
    pub destination: String,
    pub revision: u64,
    pub recipients: Readers,
    /// Unresolved external memberships must never be interpreted as empty sets.
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub workspace: WorkspaceId,
    pub revision: u64,
    pub approvers: Readers,
    pub max_approval_lifetime: u64,
    /// Omitted for legacy snapshots, preserving their canonical policy hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub workspace: WorkspaceId,
    pub artifact: ArtifactId,
    pub artifact_hash: Digest,
    pub audience: Audience,
    pub action: String,
    pub policy_hash: Digest,
    pub lineage_hash: Digest,
    pub acl_hash: Digest,
}
impl Context {
    pub fn digest(&self) -> Digest {
        digest::structured("publication-context", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub id: ApprovalId,
    pub signer: PrincipalId,
    pub context: Context,
    pub issued_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub workspace: WorkspaceId,
    pub complete: bool,
    pub principals: BTreeMap<PrincipalId, Principal>,
    pub artifacts: BTreeMap<ArtifactId, Artifact>,
    pub sources: BTreeMap<SourceId, Source>,
    pub acls: BTreeMap<AclId, Acl>,
    pub transformations: BTreeMap<TransformationId, Transformation>,
    pub audiences: BTreeMap<AudienceId, Audience>,
    pub policy: Policy,
    pub approvals: BTreeMap<ApprovalId, Approval>,
    pub revoked: BTreeSet<ApprovalId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intent {
    pub artifact: ArtifactId,
    pub audience: AudienceId,
    pub requester: PrincipalId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Code {
    RestrictedSource,
    IncompleteLineage,
    InvalidSchema,
    CrossWorkspace,
    ContentTampered,
    StaleApproval,
    SignerUnauthorized,
    ApprovalRevoked,
    ApprovalExpired,
    AudienceUnresolved,
    CallerUnauthorized,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub code: Code,
    pub witness: Option<ArtifactId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Decision {
    Blocked {
        violations: Vec<Violation>,
    },
    PendingApproval {
        context: Context,
    },
    Ready {
        context: Context,
        approval: ApprovalId,
    },
}
impl Decision {
    pub fn context(&self) -> Option<&Context> {
        match self {
            Self::PendingApproval { context } | Self::Ready { context, .. } => Some(context),
            _ => None,
        }
    }
    pub fn has(&self, code: Code) -> bool {
        matches!(self, Self::Blocked {violations} if violations.iter().any(|v| v.code == code))
    }
}
