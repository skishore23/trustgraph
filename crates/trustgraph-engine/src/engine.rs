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

/// Stored proposals per class (agent-proposed, operator-proposed).
const MAX_PROPOSALS_PER_CLASS: usize = 32;
/// Remembered `NeedsApproval` decisions; `Check` is agent-callable.
const MAX_DECISIONS: usize = 1024;
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
}

#[derive(Debug, Clone)]
struct Proposal {
    id: String,
    policy: Policy,
    /// Proposed with `agent: None` (the operator path), at least once.
    operator: bool,
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
            proposals: Vec<(&'a String, bool)>,
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
            proposals: self.proposals.iter().map(|p| (&p.id, p.operator)).collect(),
        };
        digest::structured("engine-state-v1", &state).0
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
        let proposed = validate_policy(text)?;
        let id = proposed.digest();
        let changes = diff(active, &proposed);
        let reply = Reply::Proposed {
            id: id.clone(),
            widens: changes.widens(),
            changes: changes.changes,
            explanation: explain(&proposed),
        };
        // Agents and the operator have separate quotas, so agents cannot push
        // the operator's proposals out. A digest the operator proposed stays
        // operator-owned.
        let operator = agent.is_none() || self.proposals.iter().any(|p| p.id == id && p.operator);
        self.proposals.retain(|p| p.id != id);
        self.proposals.push(Proposal {
            id,
            policy: proposed,
            operator,
        });
        let in_class = self.proposals.iter().filter(|p| p.operator == operator);
        if in_class.count() > MAX_PROPOSALS_PER_CLASS
            && let Some(oldest) = self.proposals.iter().position(|p| p.operator == operator)
        {
            self.proposals.remove(oldest);
        }
        Ok(reply)
    }

    /// Remember a `NeedsApproval` decision. When full, evict the oldest entry
    /// without a live approval (or else the oldest), with its approval. A later
    /// `Approve` of an evicted id is `unknown_decision`; a re-`Check` restores it.
    fn remember(&mut self, at: u64, id: String, mut entry: Remembered) {
        entry.seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        if !self.decisions.contains_key(&id) && self.decisions.len() >= MAX_DECISIONS {
            let live = |key: &String| {
                self.approvals
                    .get(key)
                    .is_some_and(|g| approval_live(g.issued_at, g.valid_for, at, g.voided))
            };
            let oldest = |keep_live: bool| {
                self.decisions
                    .iter()
                    .filter(|(key, _)| !(keep_live && live(key)))
                    .min_by_key(|(key, d)| (d.seq, *key))
                    .map(|(key, _)| key.clone())
            };
            if let Some(victim) = oldest(true).or_else(|| oldest(false)) {
                self.decisions.remove(&victim);
                if let Some(granted) = self.approvals.remove(&victim) {
                    self.graph.revoke_approval(&granted.approval);
                }
            }
        }
        self.decisions.insert(id, entry);
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
