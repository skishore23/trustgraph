//! The state machine. Every handler validates before it mutates, so a
//! `Rejected` reply leaves the state as it was apart from the clock. An event
//! that fails `Event::validate_shape` does not even move the clock.
//!
//! Two deciders must agree before anything is allowed: the pure
//! `trustgraph_policy::decide` and the framework `Graph`, which tracks lineage
//! and approvals. `decide` alone may deny; neither alone may allow.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use trustgraph_core::decision::{Gate, approval_live};
use trustgraph_core::digest;
use trustgraph_framework::{
    ActionIntent, Approval, ArtifactId, Decision, Error, Graph, OperationDefinition, OutputValue,
    PrincipalId, SchemaId,
};
use trustgraph_policy::{Policy, Query, Verdict, decide, diff, explain};

#[cfg(feature = "fault-injection")]
use crate::event::Fault;
use crate::event::{Event, Reply};
use crate::rules::{
    action_rule, clip, decision_reply, echo, graph_error, graph_readers, principals, reject,
    validate_policy,
};

/// Stored proposals per class (agent-proposed; operator-proposed). The
/// policy file has a class of its own that holds only its latest version.
const MAX_PROPOSALS_PER_CLASS: usize = 32;
/// Stored proposals per proposing agent, within the agent class.
const MAX_PROPOSALS_PER_AGENT: usize = 8;
/// Remembered `NeedsApproval` decisions; `Check` is agent-callable.
const MAX_DECISIONS: usize = 1024;
/// Remembered `NeedsApproval` decisions per agent, within `MAX_DECISIONS`.
const MAX_DECISIONS_PER_AGENT: usize = 64;
/// The engine never sees content, so every port carries the same nominal type.
const SCHEMA: &str = "any";
const OUTPUT_PORT: &str = "out";
const RECEIPT: &str = "trustgraph-engine";

/// A `NeedsApproval` decision, kept so it can be approved or revoked.
#[derive(Debug, Clone, Serialize)]
struct Remembered {
    agent: String,
    artifact: String,
    action: String,
    recipients: Option<BTreeSet<String>>,
    intent: ActionIntent,
    /// Insertion order, for deterministic eviction.
    seq: u64,
    /// When it was last asked, and the policy's reason. For the operator's
    /// view only; they do not affect replies, so `state_digest` skips them.
    #[serde(skip)]
    requested_at: u64,
    #[serde(skip)]
    reason: String,
}

/// Who stored a proposal. Each origin kind is its own class: agents share
/// one (with a per-agent cap), the operator has one, and the policy file
/// holds a single slot, so no class can push out another's entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalOrigin {
    /// `ProposePolicy` with `agent: None`.
    Operator,
    /// `ProposeFromFile`: the policy file differed from the active policy at start.
    File,
    /// `ProposePolicy` from this agent.
    Agent(String),
}

#[derive(Debug, Clone)]
struct Proposal {
    id: String,
    policy: Policy,
    origin: ProposalOrigin,
}

/// A remembered `NeedsApproval` decision with no live approval: what the
/// operator is asked to approve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingDecision {
    pub decision: String,
    pub agent: String,
    pub action: String,
    pub artifact: String,
    /// The people the output would reach.
    pub recipients: Vec<String>,
    /// Source names the output derives from.
    pub sources: Vec<String>,
    pub reason: String,
    pub requested_at: u64,
}

/// A live approval: granted, not revoked and not expired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveApproval {
    pub decision: String,
    pub agent: String,
    pub action: String,
    pub recipients: Vec<String>,
    pub reviewer: String,
    pub expires_at: u64,
}

/// A stored proposal, compared with the active policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredProposal {
    pub proposal: String,
    pub origin: ProposalOrigin,
    pub widens: bool,
    pub changes: Vec<trustgraph_policy::Change>,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize)]
struct Granted {
    reviewer: String,
    issued_at: u64,
    valid_for: u64,
    expires_at: u64,
    voided: bool,
    approval: Approval,
}

#[derive(Debug, Clone, Serialize)]
struct Activation {
    revision: u64,
    digest: String,
    /// `None` for `Genesis`.
    operator: Option<String>,
    at: u64,
}

/// Key of a recorded output: `(agent, step, inputs, sha256)`.
type OutputKey = (String, String, Vec<String>, String);

/// The deterministic runtime core. See the crate docs.
#[derive(Debug, Default)]
pub struct Engine {
    #[cfg(feature = "fault-injection")]
    fault: Fault,
    graph: Graph,
    policy: Option<Policy>,
    revision: u64,
    last_at: u64,
    activation: Option<Activation>,
    /// Source artifact id -> source name.
    sources: BTreeMap<String, String>,
    /// `(source name, sha256)` -> artifact id.
    source_ids: BTreeMap<(String, String), String>,
    outputs: BTreeMap<OutputKey, String>,
    /// Every action name ever given a Graph rule.
    rule_names: BTreeSet<String>,
    decisions: BTreeMap<String, Remembered>,
    next_seq: u64,
    approvals: BTreeMap<String, Granted>,
    /// Oldest first.
    proposals: Vec<Proposal>,
}

type Handled = Result<Reply, Reply>;

impl Engine {
    /// An engine with no policy. Until `Genesis`, every other event is
    /// `Rejected { code: "no_policy" }`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply one event at time `at` (seconds). Never panics.
    pub fn apply(&mut self, at: u64, event: &Event) -> Reply {
        if at < self.last_at {
            return reject(
                "clock_regressed",
                format!("time {at} is before the previous event at {}", self.last_at),
            );
        }
        if let Err(rejected) = event.validate_shape() {
            return rejected;
        }
        self.last_at = at;
        if self.policy.is_none() && !matches!(event, Event::Genesis { .. }) {
            return reject("no_policy", "no policy is active; send Genesis first");
        }
        let handled = match event {
            Event::Genesis { policy } => {
                validate_policy(policy).and_then(|policy| self.activate(policy, None, at))
            }
            Event::RegisterSource { source, sha256 } => self.register_source(source, sha256),
            Event::RecordOutput {
                agent,
                step,
                inputs,
                sha256,
            } => self.record_output(agent, step, inputs, sha256),
            Event::Check {
                agent,
                artifact,
                action,
                recipients,
            } => self.check(at, agent, artifact, action, recipients.as_ref()),
            Event::Approve { decision, reviewer } => self.approve(at, decision, reviewer),
            Event::Revoke { decision, reviewer } => self.revoke(decision, reviewer),
            Event::ProposePolicy { agent, policy } => self.propose(agent.as_deref(), policy),
            Event::ActivatePolicy { proposal, operator } => {
                self.activate_proposal(at, proposal, operator)
            }
            Event::ProposeFromFile { policy } => self.propose_from_file(policy),
        };
        handled.unwrap_or_else(|rejected| rejected)
    }

    pub fn policy(&self) -> Option<&Policy> {
        self.policy.as_ref()
    }

    /// 0 before `Genesis`; incremented by every activation.
    pub fn policy_revision(&self) -> u64 {
        self.revision
    }

    /// True when the active policy came from a `Genesis` (the policy file),
    /// false when an `ActivatePolicy` installed it, or before any policy.
    pub fn policy_from_genesis(&self) -> bool {
        self.activation
            .as_ref()
            .is_some_and(|a| a.operator.is_none())
    }

    /// The artifact id of an already registered `(source, sha256)`.
    pub fn source_artifact(&self, source: &str, sha256: &str) -> Option<&str> {
        self.source_ids
            .get(&(source.to_string(), sha256.to_string()))
            .map(String::as_str)
    }

    /// The artifact id of an already recorded output with exactly this key.
    pub fn output_artifact(
        &self,
        agent: &str,
        step: &str,
        inputs: &[String],
        sha256: &str,
    ) -> Option<&str> {
        let key: OutputKey = (agent.into(), step.into(), inputs.to_vec(), sha256.into());
        self.outputs.get(&key).map(String::as_str)
    }

    /// True when a proposal with this id (policy digest) is stored.
    pub fn has_proposal(&self, id: &str) -> bool {
        self.proposals.iter().any(|p| p.id == id)
    }

    /// Who stored the proposal with this id, when one is stored.
    pub fn proposal_origin(&self, id: &str) -> Option<&ProposalOrigin> {
        self.proposals
            .iter()
            .find(|p| p.id == id)
            .map(|p| &p.origin)
    }

    /// Number of artifacts (sources and outputs) the engine holds.
    pub fn artifact_count(&self) -> usize {
        self.graph.artifacts().len()
    }

    /// Remembered `NeedsApproval` decisions without a live approval at `at`,
    /// oldest request first.
    pub fn pending_decisions(&self, at: u64) -> Vec<PendingDecision> {
        let mut out: Vec<_> = self
            .decisions
            .iter()
            .filter(|(id, _)| !self.approval_is_live(id, at))
            .map(|(id, d)| PendingDecision {
                decision: id.clone(),
                agent: d.agent.clone(),
                action: d.action.clone(),
                artifact: d.artifact.clone(),
                recipients: d.intent.recipients.iter().map(|p| p.0.clone()).collect(),
                sources: self
                    .lineage(&d.artifact)
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
                reason: d.reason.clone(),
                requested_at: d.requested_at,
            })
            .collect();
        out.sort_by(|a, b| (a.requested_at, &a.decision).cmp(&(b.requested_at, &b.decision)));
        out
    }

    /// Approvals that are live at `at`, soonest expiry first.
    pub fn live_approvals(&self, at: u64) -> Vec<LiveApproval> {
        let mut out: Vec<_> = self
            .approvals
            .iter()
            .filter(|(id, _)| self.approval_is_live(id, at))
            .filter_map(|(id, g)| {
                let d = self.decisions.get(id)?;
                Some(LiveApproval {
                    decision: id.clone(),
                    agent: d.agent.clone(),
                    action: d.action.clone(),
                    recipients: d.intent.recipients.iter().map(|p| p.0.clone()).collect(),
                    reviewer: g.reviewer.clone(),
                    expires_at: g.expires_at,
                })
            })
            .collect();
        out.sort_by(|a, b| (a.expires_at, &a.decision).cmp(&(b.expires_at, &b.decision)));
        out
    }

    /// Stored proposals, oldest first, each diffed against the active policy.
    pub fn stored_proposals(&self) -> Vec<StoredProposal> {
        let Some(active) = self.policy.as_ref() else {
            return Vec::new();
        };
        self.proposals
            .iter()
            .map(|p| {
                let changes = diff(active, &p.policy);
                StoredProposal {
                    proposal: p.id.clone(),
                    origin: p.origin.clone(),
                    widens: changes.widens(),
                    changes: changes.changes,
                    explanation: explain(&p.policy),
                }
            })
            .collect()
    }

    fn approval_is_live(&self, decision: &str, at: u64) -> bool {
        self.approvals
            .get(decision)
            .is_some_and(|g| approval_live(g.issued_at, g.valid_for, at, g.voided))
    }

    /// SHA-256 over everything that affects future replies: the policy digest,
    /// revision, clock, artifacts, lineage, source permissions, action rules,
    /// remembered decisions, approvals with expiry, and proposals.
    pub fn state_digest(&self) -> String {
        #[derive(Serialize)]
        struct State<'a> {
            policy: Option<String>,
            revision: u64,
            last_at: u64,
            activation: &'a Option<Activation>,
            artifacts: &'a BTreeMap<ArtifactId, trustgraph_framework::Artifact>,
            executions:
                &'a BTreeMap<trustgraph_framework::ExecutionId, trustgraph_framework::Execution>,
            permissions: BTreeMap<&'a String, Option<trustgraph_framework::SourcePermission>>,
            sources: Vec<(&'a String, &'a String, &'a String)>,
            outputs: Vec<(&'a OutputKey, &'a String)>,
            rules: &'a BTreeSet<String>,
            decisions: &'a BTreeMap<String, Remembered>,
            next_seq: u64,
            approvals: &'a BTreeMap<String, Granted>,
            proposals: Vec<(&'a String, &'a ProposalOrigin)>,
        }
        let permissions = self
            .sources
            .keys()
            .map(|id| {
                let id_ref = ArtifactId(id.clone());
                let permission = self
                    .graph
                    .sources(&id_ref)
                    .ok()
                    .and_then(|mut m| m.remove(&id_ref));
                (id, permission)
            })
            .collect();
        let state = State {
            policy: self.policy.as_ref().map(Policy::digest),
            revision: self.revision,
            last_at: self.last_at,
            activation: &self.activation,
            artifacts: self.graph.artifacts(),
            executions: self.graph.executions(),
            permissions,
            sources: self
                .source_ids
                .iter()
                .map(|((name, sha), id)| (name, sha, id))
                .collect(),
            outputs: self.outputs.iter().collect(),
            rules: &self.rule_names,
            decisions: &self.decisions,
            next_seq: self.next_seq,
            approvals: &self.approvals,
            proposals: self.proposals.iter().map(|p| (&p.id, &p.origin)).collect(),
        };
        digest::structured("engine-state-v2", &state).0
    }

    /// Source names an artifact derives from, or `None` for an unknown id.
    /// Names stay even after the active policy drops the source.
    /// Also `None` when the lineage exceeds the Graph's limits.
    pub fn lineage(&self, artifact: &str) -> Option<BTreeSet<String>> {
        self.source_names(artifact).ok()
    }

    /// `unknown_artifact` for an unknown id, `lineage_limit` when the Graph
    /// cannot walk the lineage.
    fn source_names(&self, artifact: &str) -> Result<BTreeSet<String>, Reply> {
        let id = ArtifactId(artifact.to_string());
        let artifacts = self.graph.artifacts();
        if !artifacts.contains_key(&id) {
            return Err(reject(
                "unknown_artifact",
                format!("no artifact `{}`", clip(artifact)),
            ));
        }
        let sources = self.graph.sources(&id).map_err(graph_error)?;
        sources
            .keys()
            .map(|id| artifacts.get(id).map(|a| a.logical_id.clone()))
            .collect::<Option<_>>()
            .ok_or_else(|| reject("engine_mismatch", "a lineage source is missing"))
    }

    /// The same engine with a deliberate bug, for the simulator only.
    #[cfg(feature = "fault-injection")]
    #[doc(hidden)]
    pub fn with_fault(mut self, fault: Fault) -> Self {
        self.fault = fault;
        self
    }

    /// False only under an injected fault that leaves Graph readers stale.
    fn refreshes_readers(&self) -> bool {
        #[cfg(feature = "fault-injection")]
        if matches!(
            self.fault,
            Fault::SkipReaderRefresh | Fault::GraphOnlyStaleReaders
        ) {
            return false;
        }
        true
    }

    /// True only under an injected fault that keeps approvals across activation.
    fn keeps_approvals(&self) -> bool {
        #[cfg(feature = "fault-injection")]
        if self.fault == Fault::KeepApprovalsAcrossPolicyChange {
            return true;
        }
        false
    }

    fn active(&self) -> Result<&Policy, Reply> {
        self.policy
            .as_ref()
            .ok_or_else(|| reject("no_policy", "no policy is active; send Genesis first"))
    }

    fn activate(&mut self, policy: Policy, operator: Option<String>, at: u64) -> Handled {
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| reject("revision_exhausted", "the policy revision cannot grow"))?;

        let refresh = self.refreshes_readers();
        let names: BTreeSet<String> = self
            .rule_names
            .iter()
            .chain(policy.actions.keys())
            .cloned()
            .collect();
        // The Graph cannot be cloned, so check everything it could refuse
        // before changing anything. Revision overflow cannot happen past the
        // check above: each activation bumps a source or rule revision at most
        // once, so neither can exceed the engine's own revision.
        let refused = |what: &str| {
            reject(
                "engine_mismatch",
                format!("the dataflow graph would refuse {what}; nothing changed"),
            )
        };
        if refresh {
            for id in self.sources.keys() {
                let id = ArtifactId(id.clone());
                if !self.graph.sources(&id).is_ok_and(|s| s.contains_key(&id)) {
                    return Err(refused("a source permission update"));
                }
            }
        }
        for name in &names {
            let rule = action_rule(&policy, name);
            let approval_ok = rule
                .approval
                .as_ref()
                .is_none_or(|a| !a.reviewers.is_empty() && a.ttl_seconds > 0);
            if name.trim().is_empty() || !approval_ok {
                return Err(refused("an action rule"));
            }
        }

        let unexpected = |error: Error| {
            reject(
                "engine_mismatch",
                format!("unexpected graph error: {}", clip(&error.to_string())),
            )
        };
        if refresh {
            for (id, name) in &self.sources {
                self.graph
                    .update_source_permissions(
                        &ArtifactId(id.clone()),
                        graph_readers(&policy, name),
                    )
                    .map_err(unexpected)?;
            }
        }
        for name in &names {
            self.graph
                .set_action_rule(name.clone(), action_rule(&policy, name))
                .map_err(unexpected)?;
        }
        self.rule_names = names;

        if !self.keeps_approvals() {
            for granted in self.approvals.values() {
                self.graph.revoke_approval(&granted.approval);
            }
            self.approvals.clear();
            self.decisions.clear();
        }

        let digest = policy.digest();
        self.policy = Some(policy);
        self.revision = revision;
        self.activation = Some(Activation {
            revision,
            digest: digest.clone(),
            operator,
            at,
        });
        Ok(Reply::Activated { revision, digest })
    }

    fn register_source(&mut self, source: &str, sha: &str) -> Handled {
        let policy = self.active()?;
        if !policy.sources.contains_key(source) {
            return Err(reject(
                "unknown_source",
                format!("`{}` is not a source in the active policy", clip(source)),
            ));
        }
        let key = (source.to_string(), sha.to_string());
        if let Some(id) = self.source_ids.get(&key) {
            return Ok(Reply::Artifact { id: id.clone() });
        }
        let readers = graph_readers(policy, source);
        let id = self
            .graph
            .register_source(
                source,
                SchemaId(SCHEMA.into()),
                sha.as_bytes().to_vec(),
                readers,
            )
            .map_err(graph_error)?
            .0;
        self.sources.insert(id.clone(), source.to_string());
        self.source_ids.insert(key, id.clone());
        Ok(Reply::Artifact { id })
    }

    fn record_output(&mut self, agent: &str, step: &str, inputs: &[String], sha: &str) -> Handled {
        let policy = self.active()?;
        if !policy.agents.contains(agent) {
            return Err(reject(
                "unknown_agent",
                format!("`{}` is not an agent in the active policy", clip(agent)),
            ));
        }
        for input in inputs {
            if !self
                .graph
                .artifacts()
                .contains_key(&ArtifactId(input.clone()))
            {
                return Err(reject(
                    "unknown_artifact",
                    format!("no artifact `{}`", clip(input)),
                ));
            }
        }
        for input in inputs {
            let names = self.source_names(input)?;
            if let Some(name) = names.iter().find(|n| {
                !policy
                    .sources
                    .get(*n)
                    .is_some_and(|s| s.agents.contains(agent))
            }) {
                let [agent, name] = echo([agent, name]);
                return Err(reject(
                    "agent_cannot_use_source",
                    format!("agent `{agent}` may not use source `{name}`"),
                ));
            }
        }

        let key: OutputKey = (agent.into(), step.into(), inputs.to_vec(), sha.into());
        if let Some(id) = self.outputs.get(&key) {
            return Ok(Reply::Artifact { id: id.clone() });
        }
        let ports = (0..inputs.len())
            .map(|i| (format!("in{i}"), SchemaId(SCHEMA.into())))
            .collect();
        let operation = self
            .graph
            .register_operation(OperationDefinition {
                name: step.into(),
                // One contract per (step, input count, agent), so contracts never collide.
                version: format!("{}/{agent}", inputs.len()),
                executor: PrincipalId(agent.into()),
                inputs: ports,
                outputs: BTreeMap::from([(OUTPUT_PORT.into(), SchemaId(SCHEMA.into()))]),
            })
            .map_err(graph_error)?;
        let bound = inputs
            .iter()
            .enumerate()
            .map(|(i, id)| (format!("in{i}"), ArtifactId(id.clone())))
            .collect();
        let ticket = self
            .graph
            .begin_execution(
                &operation,
                bound,
                PrincipalId(agent.into()),
                serde_json::Value::Null,
            )
            .map_err(graph_error)?;
        let values = BTreeMap::from([(
            OUTPUT_PORT.to_string(),
            OutputValue {
                logical_id: step.into(),
                bytes: sha.as_bytes().to_vec(),
            },
        )]);
        let outputs = self
            .graph
            .finish_execution(ticket, values, RECEIPT)
            .map_err(graph_error)?;
        let id = outputs
            .get(OUTPUT_PORT)
            .ok_or_else(|| reject("engine_mismatch", "the graph returned no output"))?
            .0
            .clone();
        self.outputs.insert(key, id.clone());
        Ok(Reply::Artifact { id })
    }

    fn check(
        &mut self,
        at: u64,
        agent: &str,
        artifact: &str,
        action: &str,
        recipients: Option<&BTreeSet<String>>,
    ) -> Handled {
        let output_from = self.source_names(artifact)?;
        let policy = self.active()?;
        let outcome = decide(
            policy,
            &Query {
                output_from,
                agent: agent.into(),
                action: action.into(),
                recipients: recipients.cloned(),
            },
        );
        let query_id =
            || digest::structured("engine-query-v1", &(artifact, agent, action, recipients)).0;
        let intent = |audience: &BTreeSet<String>| ActionIntent {
            actor: PrincipalId(agent.into()),
            action: action.into(),
            artifact: ArtifactId(artifact.into()),
            destination: action.into(),
            recipients: principals(policy.people(recipients.unwrap_or(audience))),
            parameters: serde_json::Value::Null,
        };

        // Deliberately wrong, for the simulator only: answer from the Graph
        // alone without the `decide` gate. Compiled out of normal builds.
        #[cfg(feature = "fault-injection")]
        if self.fault == Fault::GraphOnlyStaleReaders {
            let Some(rule) = policy.actions.get(action) else {
                return Ok(decision_reply(query_id(), outcome));
            };
            return Ok(
                match self.graph.check_action(&intent(&rule.audience), None, at) {
                    Decision::Allow { context } => Reply::Decision {
                        id: context.digest,
                        verdict: Verdict::Allow,
                        code: Gate::Allowed.code().into(),
                        reason: "allowed".into(),
                        blocking: Vec::new(),
                        expires_at: None,
                    },
                    _ => decision_reply(query_id(), outcome),
                },
            );
        }

        if outcome.verdict == Verdict::Deny {
            return Ok(decision_reply(query_id(), outcome));
        }
        // `decide` permits it; the Graph must say the same thing.
        let Some(rule) = policy.actions.get(action) else {
            return Ok(self.mismatch(query_id(), outcome.verdict, "no such action"));
        };
        let intent = intent(&rule.audience);
        let graph = self.graph.check_action(&intent, None, at);
        match (outcome.verdict, graph) {
            (Verdict::Allow, Decision::Allow { context }) => {
                Ok(decision_reply(context.digest, outcome))
            }
            (Verdict::NeedsApproval, Decision::NeedsApproval { context }) => {
                let id = context.digest;
                if let Some(granted) = self.approvals.get(&id)
                    && approval_live(granted.issued_at, granted.valid_for, at, granted.voided)
                    && matches!(
                        self.graph
                            .check_action(&intent, Some(&granted.approval), at),
                        Decision::Allow { .. }
                    )
                {
                    return Ok(Reply::Decision {
                        id,
                        verdict: Verdict::Allow,
                        code: Gate::Allowed.code().into(),
                        reason: format!("allowed: approved by `{}`", granted.reviewer),
                        blocking: Vec::new(),
                        expires_at: Some(granted.expires_at),
                    });
                }
                self.remember(
                    at,
                    id.clone(),
                    Remembered {
                        agent: agent.into(),
                        artifact: artifact.into(),
                        action: action.into(),
                        recipients: recipients.cloned(),
                        intent,
                        seq: 0,
                        requested_at: at,
                        reason: outcome.reason.clone(),
                    },
                );
                Ok(decision_reply(id, outcome))
            }
            (verdict, graph) => {
                let said = match graph {
                    Decision::Allow { .. } => "allow".to_string(),
                    Decision::NeedsApproval { .. } => "needs_approval".to_string(),
                    Decision::Deny { reason } => format!("deny ({})", clip(&reason)),
                };
                Ok(self.mismatch(query_id(), verdict, &said))
            }
        }
    }

    fn mismatch(&self, id: String, verdict: Verdict, graph: &str) -> Reply {
        Reply::Decision {
            id,
            verdict: Verdict::Deny,
            code: "engine_mismatch".into(),
            reason: format!(
                "denied: the policy says {} but the dataflow graph says {graph}",
                verdict.as_str()
            ),
            blocking: Vec::new(),
            expires_at: None,
        }
    }

    /// The remembered decision and its action's expanded reviewers.
    fn pending(&self, decision: &str) -> Result<(&Remembered, BTreeSet<String>, u64), Reply> {
        let unknown = || {
            reject(
                "unknown_decision",
                format!("no pending decision `{}`", clip(decision)),
            )
        };
        let remembered = self.decisions.get(decision).ok_or_else(unknown)?;
        let policy = self.active()?;
        let rule = policy
            .actions
            .get(&remembered.action)
            .and_then(|a| a.approval.as_ref())
            .ok_or_else(unknown)?;
        Ok((
            remembered,
            policy.people(&rule.reviewers),
            rule.valid_for_seconds,
        ))
    }

    fn approve(&mut self, at: u64, decision: &str, reviewer: &str) -> Handled {
        let (remembered, reviewers, valid_for) = self.pending(decision)?;
        let policy = self.active()?;
        let output_from = self.source_names(&remembered.artifact)?;
        let outcome = decide(
            policy,
            &Query {
                output_from,
                agent: remembered.agent.clone(),
                action: remembered.action.clone(),
                recipients: remembered.recipients.clone(),
            },
        );
        if outcome.verdict != Verdict::NeedsApproval {
            return Err(reject(
                "decision_not_pending",
                format!("the policy now says {}", outcome.verdict.as_str()),
            ));
        }
        if !reviewers.contains(reviewer)
            || !outcome.readers.contains(reviewer)
            || reviewer == remembered.agent
        {
            return Err(reject(
                "reviewer_not_allowed",
                format!(
                    "`{}` must be a listed reviewer, a reader of the output and not the agent",
                    clip(reviewer)
                ),
            ));
        }
        if let Some(granted) = self.approvals.get(decision)
            && approval_live(granted.issued_at, granted.valid_for, at, granted.voided)
        {
            return Err(reject(
                "already_approved",
                format!(
                    "approved by `{}` until {}",
                    clip(&granted.reviewer),
                    granted.expires_at
                ),
            ));
        }
        let expires_at = at
            .checked_add(valid_for)
            .ok_or_else(|| reject("invalid_time", "the approval would expire past u64::MAX"))?;
        let intent = remembered.intent.clone();
        let approval = self
            .graph
            .approve(&intent, PrincipalId(reviewer.into()), at)
            .map_err(graph_error)?;
        let granted = Granted {
            reviewer: reviewer.into(),
            issued_at: at,
            valid_for,
            expires_at,
            voided: false,
            approval,
        };
        if let Some(old) = self.approvals.insert(decision.into(), granted) {
            self.graph.revoke_approval(&old.approval);
        }
        Ok(Reply::Approved {
            decision: decision.into(),
            expires_at,
        })
    }

    fn revoke(&mut self, decision: &str, reviewer: &str) -> Handled {
        let (_, reviewers, _) = self.pending(decision)?;
        if !reviewers.contains(reviewer) {
            return Err(reject(
                "reviewer_not_allowed",
                format!("`{}` is not a reviewer for this action", clip(reviewer)),
            ));
        }
        let granted = self
            .approvals
            .get_mut(decision)
            .ok_or_else(|| reject("no_approval", "this decision has no approval to revoke"))?;
        granted.voided = true;
        self.graph.revoke_approval(&granted.approval);
        Ok(Reply::Revoked {
            decision: decision.into(),
        })
    }

    fn propose(&mut self, agent: Option<&str>, text: &str) -> Handled {
        let active = self.active()?;
        if let Some(agent) = agent
            && !active.agents.contains(agent)
        {
            return Err(reject(
                "unknown_agent",
                format!("`{}` is not an agent in the active policy", clip(agent)),
            ));
        }
        let origin = match agent {
            Some(agent) => ProposalOrigin::Agent(agent.to_string()),
            None => ProposalOrigin::Operator,
        };
        let proposed = validate_policy(text)?;
        self.store_proposal(origin, proposed)
    }

    fn propose_from_file(&mut self, text: &str) -> Handled {
        let proposed = validate_policy(text)?;
        self.store_proposal(ProposalOrigin::File, proposed)
    }

    /// Store a validated proposal and answer with its diff from the active
    /// policy. An agent re-proposing a stored digest changes nothing, so it
    /// can neither take over nor move another origin's entry. The operator
    /// proposing any stored digest makes it the operator's; the file
    /// proposing an agent's digest makes it the file's, and leaves the
    /// operator's own entry as it is.
    /// Quotas: 32 for the operator, 32 for all agents and 8 per agent within
    /// them, and ONE for the policy file: a newer file version replaces the
    /// older file proposal. Eviction removes the proposer's own oldest entry
    /// first.
    fn store_proposal(&mut self, origin: ProposalOrigin, proposed: Policy) -> Handled {
        let active = self.active()?;
        let id = proposed.digest();
        let changes = diff(active, &proposed);
        let reply = Reply::Proposed {
            id: id.clone(),
            widens: changes.widens(),
            changes: changes.changes,
            explanation: explain(&proposed),
        };
        let from_file = origin == ProposalOrigin::File;
        if from_file {
            // The file's one slot: an older version goes.
            self.proposals
                .retain(|p| p.origin != ProposalOrigin::File || p.id == id);
        }
        if let Some(existing) = self.proposals.iter().find(|p| p.id == id) {
            let keep = match (&origin, &existing.origin) {
                (ProposalOrigin::Agent(_), _) => true,
                (ProposalOrigin::File, e) => !matches!(e, ProposalOrigin::Agent(_)),
                (ProposalOrigin::Operator, _) => false,
            };
            if keep {
                return Ok(reply);
            }
        }
        self.proposals.retain(|p| p.id != id);
        self.proposals.push(Proposal {
            id,
            policy: proposed,
            origin: origin.clone(),
        });
        match &origin {
            ProposalOrigin::Agent(agent) => {
                let own =
                    |p: &Proposal| matches!(&p.origin, ProposalOrigin::Agent(a) if a == agent);
                if self.proposals.iter().filter(|p| own(p)).count() > MAX_PROPOSALS_PER_AGENT
                    && let Some(oldest) = self.proposals.iter().position(own)
                {
                    self.proposals.remove(oldest);
                }
                let agents = self
                    .proposals
                    .iter()
                    .filter(|p| matches!(p.origin, ProposalOrigin::Agent(_)))
                    .count();
                if agents > MAX_PROPOSALS_PER_CLASS
                    && let Some(victim) = self.agent_proposal_victim()
                {
                    self.proposals.remove(victim);
                }
            }
            ProposalOrigin::Operator => {
                let operator = |p: &Proposal| p.origin == ProposalOrigin::Operator;
                if self.proposals.iter().filter(|p| operator(p)).count() > MAX_PROPOSALS_PER_CLASS
                    && let Some(oldest) = self.proposals.iter().position(operator)
                {
                    self.proposals.remove(oldest);
                }
            }
            // Older file versions were removed above.
            ProposalOrigin::File => {}
        }
        Ok(reply)
    }

    /// The oldest agent-class proposal of the agent that holds the most of
    /// them (ties: the agent whose oldest entry is oldest). Deterministic.
    fn agent_proposal_victim(&self) -> Option<usize> {
        let mut counts: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for (index, p) in self.proposals.iter().enumerate() {
            if let ProposalOrigin::Agent(agent) = &p.origin {
                counts.entry(agent.as_str()).or_insert((0, index)).0 += 1;
            }
        }
        counts
            .values()
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|(_, first)| *first)
    }

    /// Remember a `NeedsApproval` decision. Each agent keeps at most 64 and
    /// everyone together at most 1,024. When the asking agent is at its own
    /// cap, its own oldest entry without a live approval goes (or else its
    /// own oldest). When the global cap is hit, the asking agent's own
    /// entries go first, then anyone's, entries without a live approval
    /// before approved ones. An evicted entry takes its approval with it. A
    /// later `Approve` of an evicted id is `unknown_decision`; a re-`Check`
    /// restores it.
    fn remember(&mut self, at: u64, id: String, mut entry: Remembered) {
        entry.seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        if !self.decisions.contains_key(&id) {
            let agent = entry.agent.clone();
            let own = self.decisions.values().filter(|d| d.agent == agent).count();
            let victim = if own >= MAX_DECISIONS_PER_AGENT {
                self.decision_victim(at, Some(&agent), true)
                    .or_else(|| self.decision_victim(at, Some(&agent), false))
            } else if self.decisions.len() >= MAX_DECISIONS {
                self.decision_victim(at, Some(&agent), true)
                    .or_else(|| self.decision_victim(at, None, true))
                    .or_else(|| self.decision_victim(at, Some(&agent), false))
                    .or_else(|| self.decision_victim(at, None, false))
            } else {
                None
            };
            if let Some(victim) = victim {
                self.decisions.remove(&victim);
                if let Some(granted) = self.approvals.remove(&victim) {
                    self.graph.revoke_approval(&granted.approval);
                }
            }
        }
        self.decisions.insert(id, entry);
    }

    /// The oldest remembered decision, of `agent` when given, skipping those
    /// with a live approval when `skip_live`.
    fn decision_victim(&self, at: u64, agent: Option<&str>, skip_live: bool) -> Option<String> {
        self.decisions
            .iter()
            .filter(|(_, d)| agent.is_none_or(|a| d.agent == a))
            .filter(|(key, _)| !(skip_live && self.approval_is_live(key, at)))
            .min_by_key(|(key, d)| (d.seq, *key))
            .map(|(key, _)| key.clone())
    }

    fn activate_proposal(&mut self, at: u64, proposal: &str, operator: &str) -> Handled {
        let policy = self
            .proposals
            .iter()
            .find(|p| p.id == proposal)
            .map(|p| p.policy.clone())
            .ok_or_else(|| {
                reject(
                    "unknown_proposal",
                    format!("no stored proposal `{}`", clip(proposal)),
                )
            })?;
        self.activate(policy, Some(operator.into()), at)
    }
}

#[cfg(test)]
mod tests {
    use trustgraph_policy::parse;

    use super::*;

    const SPEC: &str =
        include_str!("../../trustgraph-policy/tests/fixtures/spec-example.policy.yaml");

    /// The parser never yields a policy the Graph refuses, so build one by hand:
    /// an approval whose reviewers expand to nobody.
    #[test]
    fn activation_the_graph_refuses_changes_nothing() {
        let mut engine = Engine::new();
        engine.apply(
            0,
            &Event::Genesis {
                policy: SPEC.into(),
            },
        );
        let sheet = engine.apply(
            1,
            &Event::RegisterSource {
                source: "price_sheet".into(),
                sha256: "a".repeat(64),
            },
        );
        assert!(matches!(sheet, Reply::Artifact { .. }));
        let before = engine.state_digest();

        let mut bad = parse(SPEC).policy.unwrap();
        bad.sources.get_mut("price_sheet").unwrap().readers.clear();
        bad.actions
            .get_mut("email_customer")
            .unwrap()
            .approval
            .as_mut()
            .unwrap()
            .reviewers = BTreeSet::from(["ghost".to_string()]);
        let reply = engine.activate(bad, Some("leo".into()), 1);
        assert!(
            matches!(&reply, Err(Reply::Rejected { code, .. }) if code == "engine_mismatch"),
            "{reply:?}"
        );
        assert_eq!(engine.policy_revision(), 1);
        assert_eq!(engine.state_digest(), before);
    }
}
