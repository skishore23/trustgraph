//! The engine's input and output vocabulary.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use trustgraph_framework::SchemaId;
use trustgraph_policy::{Blocking, Change, MAX_BYTES, Verdict};

use crate::rules::{MAX_INPUTS, MAX_RECIPIENTS, field, reject, sha256};

/// Nominal adapter contract. It never grants permission or declassifies data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputContract {
    pub version: String,
    pub inputs: Vec<SchemaId>,
    pub output: SchemaId,
}

/// Versioned nominal schemas use `Name@version`; old records use `any`.
pub fn validate_schema(schema: &SchemaId) -> Result<(), Reply> {
    field("schema", &schema.0)?;
    if schema.0 == "any" {
        return Ok(());
    }
    let valid = schema.0.rsplit_once('@').is_some_and(|(name, version)| {
        !name.is_empty()
            && name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._-/".contains(&c))
            && !version.is_empty()
            && version.bytes().all(|c| c.is_ascii_digit())
            && version.parse::<u32>().is_ok_and(|v| v > 0)
    });
    if valid {
        Ok(())
    } else {
        Err(reject(
            "invalid_schema",
            "use `Name@version` with a positive integer version, or `any`",
        ))
    }
}

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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<SchemaId>,
    },
    RecordOutput {
        agent: String,
        step: String,
        inputs: Vec<String>,
        sha256: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        contract: Option<OutputContract>,
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
    /// the active policy, so it is stored as the file's one proposal,
    /// replacing any older file proposal. It never changes the active policy.
    ProposeFromFile {
        policy: String,
    },
    /// The gateway, for an agent: gateway session `session`, which belongs
    /// to `agent`, now also carries `artifacts` (what the agent read through
    /// the gateway or received in a hand-off). It is recorded so a restart
    /// can rebuild the session; it changes no decision.
    SessionCarry {
        session: String,
        agent: String,
        artifacts: Vec<String>,
    },
    /// System or operator: forget gateway session `session`.
    EndSession {
        session: String,
    },
}

impl Event {
    /// True for the events an agent token may send (`SessionCarry` only
    /// through the gateway). None of them can change the active policy.
    pub fn agent_callable(&self) -> bool {
        matches!(
            self,
            Event::RegisterSource { .. }
                | Event::RecordOutput { .. }
                | Event::Check { .. }
                | Event::ProposePolicy { .. }
                | Event::SessionCarry { .. }
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
                schema,
            } => {
                field("source", source)?;
                if let Some(schema) = schema {
                    validate_schema(schema)?;
                }
                sha256(sha)
            }
            Event::RecordOutput {
                agent,
                step,
                inputs,
                sha256: sha,
                contract,
            } => {
                field("agent", agent)?;
                field("step", step)?;
                sha256(sha)?;
                if let Some(contract) = contract {
                    field("contract.version", &contract.version)?;
                    if contract.inputs.len() != inputs.len() {
                        return Err(reject(
                            "invalid_contract",
                            "contract must describe every input",
                        ));
                    }
                    contract.inputs.iter().try_for_each(validate_schema)?;
                    validate_schema(&contract.output)?;
                }
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
            Event::SessionCarry {
                session,
                agent,
                artifacts,
            } => {
                field("session", session)?;
                field("agent", agent)?;
                if artifacts.is_empty() || artifacts.len() > MAX_INPUTS {
                    return Err(reject(
                        "invalid_inputs",
                        format!(
                            "a session carries 1 to {MAX_INPUTS} artifacts at a time, not {}",
                            artifacts.len()
                        ),
                    ));
                }
                artifacts
                    .iter()
                    .try_for_each(|artifact| field("artifacts", artifact))
            }
            Event::EndSession { session } => field("session", session),
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
    /// `SessionCarry`: everything the session now carries, sorted.
    Session {
        session: String,
        artifacts: Vec<String>,
    },
    SessionEnded {
        session: String,
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
