//! The engine's input and output vocabulary.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use trustgraph_policy::{Blocking, Change, Verdict};

/// One input to the engine. Only the four agent-callable variants may come
/// from an agent; the rest come from the system or the operator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    /// System only: the first record, or an operator restart with a new file.
    Genesis {
        policy: String,
    },
    RegisterSource {
        source: String,
        sha256: String,
    },
    RecordOutput {
        agent: String,
        step: String,
        inputs: Vec<String>,
        sha256: String,
    },
    Check {
        agent: String,
        artifact: String,
        action: String,
        recipients: Option<BTreeSet<String>>,
    },
    Approve {
        decision: String,
        reviewer: String,
    },
    Revoke {
        decision: String,
        reviewer: String,
    },
    ProposePolicy {
        agent: Option<String>,
        policy: String,
    },
    ActivatePolicy {
        proposal: String,
        operator: String,
    },
}

impl Event {
    /// True for the events an agent token may send. None of them can change
    /// the active policy.
    pub fn agent_callable(&self) -> bool {
        matches!(
            self,
            Event::RegisterSource { .. }
                | Event::RecordOutput { .. }
                | Event::Check { .. }
                | Event::ProposePolicy { .. }
        )
    }
}

/// The engine's answer to one event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Artifact {
        id: String,
    },
    Decision {
        id: String,
        verdict: Verdict,
        code: String,
        reason: String,
        blocking: Vec<Blocking>,
        expires_at: Option<u64>,
    },
    Approved {
        decision: String,
        expires_at: u64,
    },
    Revoked {
        decision: String,
    },
    Proposed {
        id: String,
        widens: bool,
        changes: Vec<Change>,
        explanation: String,
    },
    Activated {
        revision: u64,
        digest: String,
    },
    Rejected {
        code: String,
        message: String,
    },
}

/// Deliberate bugs the simulator injects to prove its invariants catch them.
/// `Fault::None` is the only value production code uses. Only built with the
/// `fault-injection` feature, which normal builds leave off.
#[cfg(feature = "fault-injection")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Fault {
    #[default]
    None,
    /// Leave every source's Graph readers unchanged on policy activation.
    SkipReaderRefresh,
    /// Keep approvals and remembered decisions across policy activation.
    KeepApprovalsAcrossPolicyChange,
    /// Skip the reader refresh on activation, like `SkipReaderRefresh`, and
    /// answer `Check` from the Graph alone without the `decide` gate, so a
    /// narrowing yields a real `Allow`. It exists only so the simulator can
    /// prove it catches a real leak with its own oracle.
    GraphOnlyStaleReaders,
}
