//! The simulator's shadow model and the invariant checks that use it.
//!
//! The shadow is deliberately simple and independent of the engine's
//! internals: it knows only what the simulator sent, the replies it got, the
//! pure `decide` function and the engine's public accessors. Who may read an
//! artifact is also computed a second way, straight from the policy text
//! ([`text_readers`]), so neither `decide` nor the engine grades itself.

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

/// The people who may read an output derived from `sources`, read straight
/// from the policy text: those in every source's expanded `readers`. A source
/// the policy does not name has no readers.
pub(super) fn text_readers(policy: &Policy, sources: &BTreeSet<String>) -> BTreeSet<String> {
    let mut readers: Option<BTreeSet<String>> = None;
    for name in sources {
        let people = policy
            .sources
            .get(name)
            .map(|s| policy.people(&s.readers))
            .unwrap_or_default();
        readers = Some(match readers {
            None => people,
            Some(r) => r.intersection(&people).cloned().collect(),
        });
    }
    readers.unwrap_or_default()
}

/// The people `ask` sends to under `policy`: its explicit recipients
/// expanded, or the action's whole audience when it names none.
pub(super) fn resolved_people(policy: &Policy, ask: &Ask) -> BTreeSet<String> {
    let names = match &ask.recipients {
        Some(names) => names.clone(),
        None => policy
            .actions
            .get(&ask.action)
            .map(|a| a.audience.clone())
            .unwrap_or_default(),
    };
    policy.people(&names)
}

/// True when an approval granted for `granted` covers `ask` under `policy`:
/// the same resolved intent (agent, artifact, action and recipient people),
/// the framework's context binding. How the recipients were written does not
/// matter; who they are does.
pub(super) fn same_intent(policy: &Policy, granted: &Ask, ask: &Ask) -> bool {
    granted.agent == ask.agent
        && granted.artifact == ask.artifact
        && granted.action == ask.action
        && resolved_people(policy, granted) == resolved_people(policy, ask)
}

/// Invariant 4 for one earlier `Allow` re-sent after an activation: true when
/// some recipient is no longer a reader and the reply is not a `Deny`.
pub(super) fn narrowing_broken(
    recipients: &BTreeSet<String>,
    readers: &BTreeSet<String>,
    reply: &Reply,
) -> bool {
    let still_readers = !recipients.is_empty() && recipients.is_subset(readers);
    let denied = matches!(
        reply,
        Reply::Decision {
            verdict: Verdict::Deny,
            ..
        }
    );
    !still_readers && !denied
}

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
    /// Artifacts recorded as outputs, not registered as sources.
    derived: BTreeSet<String>,
    /// `NeedsApproval` decisions under the current revision.
    pub(super) pending: BTreeMap<String, Ask>,
    pub(super) stale: Vec<String>,
    grants: BTreeMap<String, Grant>,
    pub(super) allowed: Vec<Ask>,
    pub(super) recent_checks: Vec<Ask>,
    /// `(proposal id, policy text, the policy it encodes)`, oldest first.
    pub(super) proposals: Vec<(String, String, Policy)>,
    /// Coverage keys reached by the last calls; the simulator drains them.
    pub(super) notes: Vec<&'static str>,
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
            derived: BTreeSet::new(),
            pending: BTreeMap::new(),
            stale: Vec::new(),
            grants: BTreeMap::new(),
            allowed: Vec::new(),
            recent_checks: Vec::new(),
            proposals: Vec::new(),
            notes: Vec::new(),
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
        resolved_people(&self.policy, ask)
    }

    /// The artifact's readers from the policy text, or `None` if unknown.
    fn readers_of(&self, artifact: &str) -> Option<BTreeSet<String>> {
        self.lineage
            .get(artifact)
            .map(|sources| text_readers(&self.policy, sources))
    }

    /// True when every recipient may read the artifact under the active
    /// policy text (not `decide`).
    pub(super) fn covered(&self, ask: &Ask) -> bool {
        let people = self.recipients(ask);
        self.readers_of(&ask.artifact)
            .is_some_and(|readers| !people.is_empty() && people.is_subset(&readers))
    }

    /// Invariant 4 for `reply`, a re-sent earlier `Allow`.
    pub(super) fn narrowing_finding(&self, ask: &Ask, reply: &Reply) -> Option<Finding> {
        let people = self.recipients(ask);
        let readers = self.readers_of(&ask.artifact).unwrap_or_default();
        narrowing_broken(&people, &readers, reply).then(|| {
            (
                NARROWING_APPLIES,
                format!(
                    "{ask:?} was allowed; recipients {people:?} are not all readers {readers:?} now, yet the runtime says {reply:?}"
                ),
            )
        })
    }

    /// True when some output derives from `source`.
    pub(super) fn has_derived_from(&self, source: &str) -> bool {
        self.derived
            .iter()
            .any(|id| self.lineage.get(id).is_some_and(|l| l.contains(source)))
    }

    /// Checks with an approval that is live now under the current revision,
    /// and when that approval stops being live.
    pub(super) fn live_grants(&self, now: u64) -> Vec<(Ask, u64)> {
        self.grants
            .values()
            .filter(|g| g.revision == self.revision && g.live(now))
            .map(|g| (g.ask.clone(), g.issued_at + g.valid_for))
            .collect()
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

    /// A `RegisterSource` or `RecordOutput` (`derived`) reply, its expected
    /// lineage and the lineage the engine reports for the returned id.
    pub(super) fn on_artifact(
        &mut self,
        reply: &Reply,
        derived: bool,
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
        if derived {
            self.derived.insert(id.clone());
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
            id,
            verdict,
            code,
            expires_at,
            ..
        } = reply
        else {
            return vec![(
                NO_LEAK,
                format!("{ask:?}: expected a decision, got {reply:?}"),
            )];
        };
        let readers = self.readers_of(&ask.artifact).unwrap_or_default();
        if ask.recipients.is_some() {
            self.notes.push("explicit_recipients_check");
        }
        if *verdict == Verdict::Deny && code == "agent_cannot_use_source" {
            self.notes.push("agent_denied_source");
        }
        if let Some(g) = self.grants.get(id)
            && g.revision == self.revision
            && *verdict != Verdict::Allow
        {
            if g.revoked {
                self.notes.push("revoke_effective");
            } else if !g.live(now) {
                self.notes.push("approval_expired_recheck");
            }
        }
        let said = |why: &str| {
            format!(
                "{ask:?}: runtime said {} ({code}), decide says {} ({}): {why}",
                verdict.as_str(),
                outcome.verdict.as_str(),
                outcome.code
            )
        };
        // `decide` reports readers only once agent, action and sources resolve.
        let resolved = self.policy.agents.contains(&ask.agent)
            && self.policy.actions.contains_key(&ask.action)
            && self
                .lineage
                .get(&ask.artifact)
                .is_some_and(|l| l.iter().all(|s| self.policy.sources.contains_key(s)));
        if resolved && outcome.readers != readers {
            return vec![(
                NO_LEAK,
                format!(
                    "decide disagrees with policy text: {ask:?}: decide readers {:?}, policy text {readers:?}",
                    outcome.readers
                ),
            )];
        }
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
        if people.is_empty() || !people.is_subset(&readers) {
            findings.push((
                NO_LEAK,
                said(&format!(
                    "recipients {people:?} are not all readers {readers:?} of the policy text"
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
                Some(g) if !same_intent(&self.policy, &g.ask, ask) => findings.push((
                    APPROVAL_SCOPED,
                    said(&format!("relies on an approval granted for {:?}", g.ask)),
                )),
                Some(g) if *expires_at != Some(g.issued_at + g.valid_for) => findings.push((
                    APPROVAL_SCOPED,
                    said(&format!(
                        "expires at {expires_at:?}, the approval at {}",
                        g.issued_at + g.valid_for
                    )),
                )),
                Some(_) => self.notes.push("approved_allow"),
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

    /// A `Revoke` reply. A valid reviewer's revocation of a live approval
    /// under the current revision must succeed.
    pub(super) fn on_revoke(
        &mut self,
        now: u64,
        decision: &str,
        reviewer: &str,
        reply: &Reply,
    ) -> Vec<Finding> {
        if let Reply::Revoked { decision } = reply {
            if let Some(grant) = self.grants.get_mut(decision) {
                grant.revoked = true;
            }
            return Vec::new();
        }
        match self.grants.get(decision) {
            Some(g)
                if g.revision == self.revision
                    && g.live(now)
                    && self.reviewers(&g.ask).iter().any(|r| r == reviewer) =>
            {
                vec![(
                    APPROVAL_SCOPED,
                    format!("revoke {decision} by {reviewer} of a live approval: {reply:?}"),
                )]
            }
            _ => Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    fn decision(verdict: Verdict) -> Reply {
        Reply::Decision {
            id: "d".into(),
            verdict,
            code: "c".into(),
            reason: String::new(),
            blocking: Vec::new(),
            expires_at: None,
        }
    }

    #[test]
    fn narrowing_fires_on_allow_to_a_non_reader_only() {
        let readers = set(&["hr"]);
        let outside = set(&["hr", "sam"]);
        assert!(narrowing_broken(
            &outside,
            &readers,
            &decision(Verdict::Allow)
        ));
        assert!(narrowing_broken(
            &outside,
            &readers,
            &decision(Verdict::NeedsApproval)
        ));
        assert!(!narrowing_broken(
            &outside,
            &readers,
            &decision(Verdict::Deny)
        ));
        assert!(!narrowing_broken(
            &set(&["hr"]),
            &readers,
            &decision(Verdict::Allow)
        ));
        assert!(narrowing_broken(
            &BTreeSet::new(),
            &readers,
            &decision(Verdict::Allow)
        ));
    }

    fn slack() -> Policy {
        trustgraph_policy::parse(include_str!(
            "../../../../examples/policies/slack-digest.policy.yaml"
        ))
        .policy
        .unwrap()
    }

    fn ask(agent: &str, artifact: &str, recipients: Option<&[&str]>) -> Ask {
        Ask {
            agent: agent.into(),
            artifact: artifact.into(),
            action: "post_all_hands".into(),
            recipients: recipients.map(set),
        }
    }

    #[test]
    fn same_intent_compares_resolved_people_not_spelling() {
        let policy = slack();
        let everyone = ask("digest_bot", "a", Some(&["everyone"]));
        assert!(same_intent(
            &policy,
            &everyone,
            &ask("digest_bot", "a", None)
        ));
        let people = ["maya", "omar", "sam", "ravi", "nina"];
        assert!(same_intent(
            &policy,
            &everyone,
            &ask("digest_bot", "a", Some(&people))
        ));
        assert!(!same_intent(
            &policy,
            &everyone,
            &ask("digest_bot", "a", Some(&["leadership"]))
        ));
        assert!(!same_intent(
            &policy,
            &everyone,
            &ask("digest_bot", "b", None)
        ));
        assert!(!same_intent(
            &policy,
            &everyone,
            &ask("other_bot", "a", None)
        ));
    }

    fn allowed(expires_at: u64) -> Reply {
        Reply::Decision {
            id: "d".into(),
            verdict: Verdict::Allow,
            code: "allowed".into(),
            reason: String::new(),
            blocking: Vec::new(),
            expires_at: Some(expires_at),
        }
    }

    /// A shadow with one handbook artifact `a` and an approval of decision
    /// `d` granted for `granted` at time 10.
    fn approved_shadow(granted: Ask) -> Shadow {
        let policy = slack();
        let mut shadow = Shadow::new(policy.clone(), "");
        shadow.on_artifact(
            &Reply::Artifact { id: "a".into() },
            false,
            set(&["handbook"]),
            |_| Some(set(&["handbook"])),
        );
        shadow.pending.insert("d".into(), granted);
        let approved = Reply::Approved {
            decision: "d".into(),
            expires_at: 3610,
        };
        assert!(shadow.on_approve(10, "d", "nina", &approved).is_empty());
        shadow
    }

    #[test]
    fn an_approval_covers_the_same_people_however_they_are_named() {
        let mut shadow = approved_shadow(ask("digest_bot", "a", Some(&["everyone"])));
        let findings = shadow.on_check(20, &ask("digest_bot", "a", None), &allowed(3610));
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(shadow.notes, ["approved_allow"]);
    }

    #[test]
    fn an_approval_for_other_people_does_not_cover_an_allow() {
        let mut shadow = approved_shadow(ask("digest_bot", "a", Some(&["hr"])));
        let findings = shadow.on_check(20, &ask("digest_bot", "a", None), &allowed(3610));
        assert!(
            findings
                .iter()
                .any(|(invariant, detail)| *invariant == APPROVAL_SCOPED
                    && detail.contains("relies on an approval granted for")),
            "{findings:?}"
        );
    }

    #[test]
    fn text_readers_intersect_and_missing_sources_read_nothing() {
        let policy = trustgraph_policy::parse(include_str!(
            "../../../../examples/policies/slack-digest.policy.yaml"
        ))
        .policy
        .unwrap();
        assert_eq!(
            text_readers(&policy, &set(&["eng_updates", "comp_review"])),
            set(&["maya", "omar"])
        );
        assert!(text_readers(&policy, &set(&["handbook", "gone"])).is_empty());
        assert!(text_readers(&policy, &BTreeSet::new()).is_empty());
    }
}
