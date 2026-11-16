//! The simulator's shadow model and the invariant checks that use it.
//!
//! The shadow is deliberately simple and independent of the engine's
//! internals: it knows only what the simulator sent, the replies it got, the
//! pure `decide` function and the engine's public accessors.

use std::collections::{BTreeMap, BTreeSet};

use trustgraph_engine::{Event, Reply};
use trustgraph_policy::{Outcome, Policy, Query, Verdict, decide};

pub(super) const NO_LEAK: &str = "no_leak";
pub(super) const APPROVAL_SCOPED: &str = "approval_scoped";
pub(super) const REPLAY_EQUIVALENT: &str = "replay_equivalent";
pub(super) const NARROWING_APPLIES: &str = "narrowing_applies";
pub(super) const AGENTS_CANNOT_CHANGE_POLICY: &str = "agents_cannot_change_policy";

/// An invariant name and what broke it.
pub(super) type Finding = (&'static str, String);

/// Checks re-sent after a crash.
const RECENT_CHECKS: usize = 5;
/// Allowed checks re-sent after an activation.
const REMEMBERED_ALLOWS: usize = 8;
/// Decision ids from earlier revisions, offered to the operator to approve.
const STALE_DECISIONS: usize = 16;
const PROPOSALS: usize = 64;

/// One `Check` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Ask {
    pub(super) agent: String,
    pub(super) artifact: String,
    pub(super) action: String,
    pub(super) recipients: Option<BTreeSet<String>>,
}

impl Ask {
    pub(super) fn event(&self) -> Event {
        Event::Check {
            agent: self.agent.clone(),
            artifact: self.artifact.clone(),
            action: self.action.clone(),
            recipients: self.recipients.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct Grant {
    ask: Ask,
    issued_at: u64,
    valid_for: u64,
    /// The policy revision it was issued under.
    revision: u64,
    revoked: bool,
}

impl Grant {
    fn live(&self, now: u64) -> bool {
        !self.revoked && self.issued_at <= now && now - self.issued_at < self.valid_for
    }
}

#[derive(Debug, Clone)]
pub(super) struct Shadow {
    pub(super) policy: Policy,
    /// The text of the active policy.
    pub(super) yaml: String,
    pub(super) digest: String,
    pub(super) revision: u64,
    /// Artifact ids in the order they were issued.
    pub(super) artifacts: Vec<String>,
    lineage: BTreeMap<String, BTreeSet<String>>,
    /// `NeedsApproval` decisions under the current revision.
    pub(super) pending: BTreeMap<String, Ask>,
    pub(super) stale: Vec<String>,
    grants: BTreeMap<String, Grant>,
    pub(super) allowed: Vec<Ask>,
    pub(super) recent_checks: Vec<Ask>,
    /// `(proposal id, policy text, the policy it encodes)`, oldest first.
    pub(super) proposals: Vec<(String, String, Policy)>,
}

fn push_capped<T: PartialEq>(list: &mut Vec<T>, item: T, cap: usize) {
    list.retain(|x| *x != item);
    list.push(item);
    if list.len() > cap {
        list.remove(0);
    }
}

impl Shadow {
    pub(super) fn new(policy: Policy, yaml: &str) -> Self {
        Shadow {
            digest: policy.digest(),
            policy,
            yaml: yaml.into(),
            revision: 1,
            artifacts: Vec::new(),
            lineage: BTreeMap::new(),
            pending: BTreeMap::new(),
            stale: Vec::new(),
            grants: BTreeMap::new(),
            allowed: Vec::new(),
            recent_checks: Vec::new(),
            proposals: Vec::new(),
        }
    }

    /// The union of the inputs' source names, as the simulator recorded them.
    pub(super) fn lineage_of(&self, inputs: &[String]) -> BTreeSet<String> {
        inputs
            .iter()
            .filter_map(|id| self.lineage.get(id))
            .flatten()
            .cloned()
            .collect()
    }

    fn outcome(&self, ask: &Ask) -> Option<Outcome> {
        let output_from = self.lineage.get(&ask.artifact)?.clone();
        Some(decide(
            &self.policy,
            &Query {
                output_from,
                agent: ask.agent.clone(),
                action: ask.action.clone(),
                recipients: ask.recipients.clone(),
            },
        ))
    }

    /// The people a check would send to under the active policy.
    fn recipients(&self, ask: &Ask) -> BTreeSet<String> {
        let names = match &ask.recipients {
            Some(names) => names.clone(),
            None => self
                .policy
                .actions
                .get(&ask.action)
                .map(|a| a.audience.clone())
                .unwrap_or_default(),
        };
        self.policy.people(&names)
    }

    /// True when every recipient may read the artifact under the active policy.
    pub(super) fn covered(&self, ask: &Ask) -> bool {
        let people = self.recipients(ask);
        self.outcome(ask)
            .is_some_and(|o| !people.is_empty() && people.is_subset(&o.readers))
    }

    /// The people who may approve this check's action under the active policy.
    pub(super) fn reviewers(&self, ask: &Ask) -> Vec<String> {
        self.policy
            .actions
            .get(&ask.action)
            .and_then(|a| a.approval.as_ref())
            .map(|rule| self.policy.people(&rule.reviewers).into_iter().collect())
            .unwrap_or_default()
    }

    pub(super) fn grant_asks(&self) -> Vec<Ask> {
        self.grants.values().map(|g| g.ask.clone()).collect()
    }

    /// True when some approval was ever granted for this check.
    pub(super) fn approved(&self, ask: &Ask) -> bool {
        self.grants.values().any(|g| g.ask == *ask)
    }

    pub(super) fn grant_ids(&self) -> Vec<String> {
        self.grants.keys().cloned().collect()
    }

    pub(super) fn ask_for(&self, decision: &str) -> Option<Ask> {
        self.pending
            .get(decision)
            .or_else(|| self.grants.get(decision).map(|g| &g.ask))
            .cloned()
    }

    /// A `RegisterSource` or `RecordOutput` reply, its expected lineage and
    /// the lineage the engine reports for the returned id.
    pub(super) fn on_artifact(
        &mut self,
        reply: &Reply,
        expected: BTreeSet<String>,
        engine: impl FnOnce(&str) -> Option<BTreeSet<String>>,
    ) -> Vec<Finding> {
        let Reply::Artifact { id } = reply else {
            return Vec::new();
        };
        let mut findings = Vec::new();
        let reported = engine(id);
        if reported.as_ref() != Some(&expected) {
            findings.push((
                NO_LEAK,
                format!("artifact {id}: lineage {reported:?}, expected {expected:?}"),
            ));
        }
        if self.lineage.insert(id.clone(), expected).is_none() {
            self.artifacts.push(id.clone());
        }
        findings
    }

    /// Invariants 1 and 2 for one `Check` reply at time `now`.
    pub(super) fn on_check(&mut self, now: u64, ask: &Ask, reply: &Reply) -> Vec<Finding> {
        push_capped(&mut self.recent_checks, ask.clone(), RECENT_CHECKS);
        let Some(outcome) = self.outcome(ask) else {
            return match reply {
                Reply::Rejected { code, .. } if code == "unknown_artifact" => Vec::new(),
                other => vec![(
                    NO_LEAK,
                    format!("{ask:?} on an unknown artifact: {other:?}"),
                )],
            };
        };
        let Reply::Decision {
            id, verdict, code, ..
        } = reply
        else {
            return vec![(
                NO_LEAK,
                format!("{ask:?}: expected a decision, got {reply:?}"),
            )];
        };
        let said = |why: &str| {
            format!(
                "{ask:?}: runtime said {} ({code}), decide says {} ({}): {why}",
                verdict.as_str(),
                outcome.verdict.as_str(),
                outcome.code
            )
        };
        if code == "engine_mismatch" {
            return vec![(NO_LEAK, said("the runtime's two deciders disagree"))];
        }
        if *verdict != Verdict::Allow {
            if *verdict != outcome.verdict || code != outcome.code {
                return vec![(NO_LEAK, said("verdicts differ"))];
            }
            if *verdict == Verdict::NeedsApproval {
                self.pending.insert(id.clone(), ask.clone());
            }
            return Vec::new();
        }

        let mut findings = Vec::new();
        let people = self.recipients(ask);
        if people.is_empty() || !people.is_subset(&outcome.readers) {
            findings.push((
                NO_LEAK,
                said(&format!(
                    "recipients {people:?} are not all readers {:?}",
                    outcome.readers
                )),
            ));
        }
        match outcome.verdict {
            Verdict::Allow if code == outcome.code => {}
            Verdict::NeedsApproval if code == "allowed" => match self.grants.get(id) {
                None => findings.push((NO_LEAK, said("no approval was ever granted"))),
                Some(g) if g.revision != self.revision => findings.push((
                    APPROVAL_SCOPED,
                    said(&format!(
                        "relies on an approval from revision {}, now {}",
                        g.revision, self.revision
                    )),
                )),
                Some(g) if g.revoked => {
                    findings.push((APPROVAL_SCOPED, said("relies on a revoked approval")))
                }
                Some(g) if !g.live(now) => findings.push((
                    APPROVAL_SCOPED,
                    said(&format!(
                        "relies on an approval issued at {} for {}s, now {now}",
                        g.issued_at, g.valid_for
                    )),
                )),
                Some(_) => {}
            },
            _ => findings.push((NO_LEAK, said("verdicts differ"))),
        }
        push_capped(&mut self.allowed, ask.clone(), REMEMBERED_ALLOWS);
        findings
    }

    /// Invariant 2 for one `Approve` reply.
    pub(super) fn on_approve(
        &mut self,
        now: u64,
        decision: &str,
        reviewer: &str,
        reply: &Reply,
    ) -> Vec<Finding> {
        let Reply::Approved { expires_at, .. } = reply else {
            return Vec::new();
        };
        let wrong = |why: String| {
            vec![(
                APPROVAL_SCOPED,
                format!("approve {decision} by {reviewer}: {why}"),
            )]
        };
        let Some(ask) = self.pending.get(decision).cloned() else {
            return wrong("not a pending decision under the current policy".into());
        };
        let Some(outcome) = self.outcome(&ask) else {
            return wrong("the artifact is unknown".into());
        };
        let Some(rule) = self
            .policy
            .actions
            .get(&ask.action)
            .and_then(|a| a.approval.as_ref())
        else {
            return wrong("the action needs no approval".into());
        };
        let valid_for = rule.valid_for_seconds;
        let valid = outcome.verdict == Verdict::NeedsApproval
            && self.policy.people(&rule.reviewers).contains(reviewer)
            && outcome.readers.contains(reviewer)
            && reviewer != ask.agent;
        if !valid {
            return wrong(format!("not a valid reviewer for {ask:?}"));
        }
        if *expires_at != now + valid_for {
            return wrong(format!("expires at {expires_at}, not {}", now + valid_for));
        }
        self.grants.insert(
            decision.into(),
            Grant {
                ask,
                issued_at: now,
                valid_for,
                revision: self.revision,
                revoked: false,
            },
        );
        Vec::new()
    }

    pub(super) fn on_revoke(&mut self, reply: &Reply) {
        if let Reply::Revoked { decision } = reply
            && let Some(grant) = self.grants.get_mut(decision)
        {
            grant.revoked = true;
        }
    }

    pub(super) fn on_proposed(&mut self, reply: &Reply, yaml: String, policy: Policy) {
        if let Reply::Proposed { id, .. } = reply {
            self.proposals.retain(|(known, ..)| known != id);
            self.proposals.push((id.clone(), yaml, policy));
            if self.proposals.len() > PROPOSALS {
                self.proposals.remove(0);
            }
        }
    }

    /// An `Activated` reply for proposal `id`: every approval and pending
    /// decision belongs to the old revision from now on.
    pub(super) fn on_activated(&mut self, reply: &Reply, id: &str) -> Vec<Finding> {
        let Reply::Activated { revision, digest } = reply else {
            return Vec::new();
        };
        let wrong =
            |why: String| vec![(AGENTS_CANNOT_CHANGE_POLICY, format!("activate {id}: {why}"))];
        let Some((_, yaml, policy)) = self
            .proposals
            .iter()
            .find(|(known, ..)| known == id)
            .cloned()
        else {
            return wrong("the simulator never saw this proposal".into());
        };
        if *revision != self.revision + 1 || *digest != policy.digest() || *digest != id {
            return wrong(format!(
                "activated revision {revision} digest {digest}, expected {} {}",
                self.revision + 1,
                policy.digest()
            ));
        }
        self.policy = policy;
        self.yaml = yaml;
        self.digest = digest.clone();
        self.revision = *revision;
        for id in std::mem::take(&mut self.pending).into_keys() {
            push_capped(&mut self.stale, id, STALE_DECISIONS);
        }
        Vec::new()
    }
}
