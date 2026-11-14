use serde::{Deserialize, Serialize};
use trustgraph_runtime::Receipt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    Demo,
    AclRace,
    RevocationRace,
    TimeoutRecovery,
    JournalRecovery,
    Expiry,
    AudienceChange,
    PolicyChange,
}
impl Scenario {
    pub const ALL: [Self; 8] = [
        Self::Demo,
        Self::AclRace,
        Self::RevocationRace,
        Self::TimeoutRecovery,
        Self::JournalRecovery,
        Self::Expiry,
        Self::AudienceChange,
        Self::PolicyChange,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    Generate { restricted: bool, key: String },
    Verify { artifact: String },
    Approve { artifact: String, key: String },
    Edit,
    Publish { artifact: String, key: String },
    NarrowAcl,
    Revoke,
    LoseAcknowledgment,
    LoseJournalAcknowledgment,
    Restart,
    Reconcile,
    ChangeAudience,
    ChangePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub at: u64,
    pub action: Action,
    pub outcome: serde_json::Value,
    pub accepted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trace {
    pub schema: u32,
    pub scheduler: String,
    pub seed: u64,
    pub scenario: Scenario,
    pub steps: Vec<Step>,
    pub receipts: Vec<Receipt>,
    pub publications: usize,
}
