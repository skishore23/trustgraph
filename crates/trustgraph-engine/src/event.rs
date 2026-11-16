//! The engine's input and output vocabulary.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use trustgraph_policy::{Blocking, Change, MAX_BYTES, Verdict};

use crate::rules::{MAX_INPUTS, MAX_RECIPIENTS, field, reject, sha256};

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
    /// System only, written by `Runtime::open`: the policy file differs from
    /// the active policy, so it is stored as an operator-class proposal
    /// marked as coming from the file. It never changes the active policy.
    ProposeFromFile {
        policy: String,
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

    /// The cheap bounds that do not depend on engine state: every name or id
    /// is 1 to 256 bytes and not blank, digests are 64 lowercase hex
    /// characters, an output has 1 to 64 inputs, a check names at most 256
    /// recipients and a policy text is at most `trustgraph_policy::MAX_BYTES`.
    ///
    /// `Runtime::handle` calls this before journaling and `Engine::apply`
    /// before anything else, so an event that fails it is never written and
    /// changes nothing, not even the engine's clock.
    pub fn validate_shape(&self) -> Result<(), Reply> {
        match self {
            Event::Genesis { policy } | Event::ProposeFromFile { policy } => policy_text(policy),
            Event::RegisterSource {
                source,
                sha256: sha,
            } => {
                field("source", source)?;
                sha256(sha)
            }
            Event::RecordOutput {
                agent,
                step,
                inputs,
                sha256: sha,
            } => {
                field("agent", agent)?;
                field("step", step)?;
                sha256(sha)?;
                if inputs.is_empty() || inputs.len() > MAX_INPUTS {
                    return Err(reject(
                        "invalid_inputs",
                        format!(
                            "an output needs 1 to {MAX_INPUTS} inputs, not {}",
                            inputs.len()
                        ),
                    ));
                }
                inputs.iter().try_for_each(|input| field("inputs", input))
            }
            Event::Check {
                agent,
                artifact,
                action,
                recipients,
            } => {
                field("agent", agent)?;
                field("artifact", artifact)?;
                field("action", action)?;
                let Some(names) = recipients else {
                    return Ok(());
                };
                if names.len() > MAX_RECIPIENTS {
                    return Err(reject(
                        "invalid_field",
                        format!("at most {MAX_RECIPIENTS} recipients"),
                    ));
                }
                names.iter().try_for_each(|name| field("recipients", name))
            }
            Event::Approve { decision, reviewer } | Event::Revoke { decision, reviewer } => {
                field("decision", decision)?;
                field("reviewer", reviewer)
            }
            Event::ProposePolicy { agent, policy } => {
                if let Some(agent) = agent {
                    field("agent", agent)?;
                }
                policy_text(policy)
            }
            Event::ActivatePolicy { proposal, operator } => {
                field("proposal", proposal)?;
                field("operator", operator)
            }
        }
    }
}

fn policy_text(text: &str) -> Result<(), Reply> {
    if text.len() > MAX_BYTES {
        return Err(reject(
            "invalid_policy",
            format!("policy is {} bytes; the limit is {MAX_BYTES}", text.len()),
        ));
    }
    Ok(())
}

/// The engine's answer to one event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
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
