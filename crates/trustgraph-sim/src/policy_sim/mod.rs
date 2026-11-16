//! A seeded simulator that drives the real policy runtime.
//!
//! It runs `Runtime<MemoryJournal>`, the same code `serve` runs, through
//! random interleavings of agent, host and operator events, clock jumps,
//! policy changes, crashes and torn journal tails, and checks five safety
//! invariants after every step:
//!
//! 1. `no_leak`: every decision equals the pure `trustgraph_policy::decide`,
//!    and every `Allow` names only readers of the output.
//! 2. `approval_scoped`: no `Allow` relies on an approval from before the
//!    latest activation, or one that was revoked or expired.
//! 3. `replay_equivalent`: a crash and reopen reach the same state and the
//!    same next decisions.
//! 4. `narrowing_applies`: after an activation, earlier allowed checks whose
//!    recipients are no longer readers are denied.
//! 5. `agents_cannot_change_policy`: the policy changes only on an
//!    `ActivatePolicy` that returned `Activated`.
//!
//! A run is a pure function of `(policy_yaml, seed, steps)`. It stops after
//! the first step that breaks an invariant.

mod generate;
mod invariants;

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use trustgraph_core::digest;
use trustgraph_engine::{Engine, Event, JournalError, MemoryJournal, OpenError, Reply, Runtime};
use trustgraph_policy::{Diagnostic, Policy, Severity, Verdict, parse};

use generate::{Names, Rng, UNKNOWN_ARTIFACT, sha};
use invariants::{AGENTS_CANNOT_CHANGE_POLICY, Ask, Finding, REPLAY_EQUIVALENT, Shadow};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    pub invariant: &'static str,
    /// 0 is the setup that registers every source.
    pub step: u32,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Run {
    pub seed: u64,
    pub steps: u32,
    /// Every event the runtime answered, with its time; `replies` is parallel.
    /// An event whose journal write failed has no reply and is not listed.
    /// Events later dropped by a torn tail stay listed (they were answered),
    /// so counts over `events` and `replies` include them.
    pub events: Vec<(u64, Event)>,
    pub replies: Vec<Reply>,
    pub violations: Vec<Violation>,
    /// The runtime's `state_digest()` at the end; empty if it could not reopen.
    pub state_digest: String,
    /// Clean crashes, lost writes and torn tails, each followed by a reopen.
    pub crashes: u32,
    /// `ActivatePolicy` events that returned `Activated`.
    pub policy_changes: u32,
    /// How often each path that makes a clean run meaningful was reached;
    /// every key in `COVERAGE` is present.
    pub coverage: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Campaign {
    pub first_seed: u64,
    pub count: u64,
    pub steps: u32,
    pub events: u64,
    /// Decision replies by verdict: exactly "allow", "deny", "needs_approval".
    pub decisions: BTreeMap<String, u64>,
    pub violations: Vec<(u64, Violation)>,
    /// Stable hash over every run's `(seed, state_digest, violations)`.
    pub digest: String,
    /// `Run::coverage` summed over the runs.
    pub coverage: BTreeMap<String, u64>,
}

/// The coverage keys every `Run` reports.
pub const COVERAGE: [&str; 18] = [
    "approved_allow",
    "approval_expired_recheck",
    "expiry_boundary_before",
    "expiry_boundary_at",
    "revoke_effective",
    "identical_reactivation_with_live_approvals",
    "recheck_after_identical_reactivation",
    "narrowing_with_derived_outputs",
    "source_removed_with_derived_outputs",
    "explicit_recipients_check",
    "agent_denied_source",
    "crash_clean",
    "crash_lost_write",
    "crash_torn_tail",
    "torn_approve",
    "torn_revoke",
    "torn_activate",
    "crash_without_resend",
];

/// The engine fault a run injects. Without the `fault-injection` feature
/// there is only "no fault", and the engine has no fault code at all.
#[cfg(feature = "fault-injection")]
type EngineFault = trustgraph_engine::Fault;
#[cfg(not(feature = "fault-injection"))]
#[derive(Debug, Clone, Copy)]
struct EngineFault;

#[cfg(feature = "fault-injection")]
const NO_FAULT: EngineFault = trustgraph_engine::Fault::None;
#[cfg(not(feature = "fault-injection"))]
const NO_FAULT: EngineFault = EngineFault;

fn open_runtime(
    journal: MemoryJournal,
    policy_yaml: &str,
    at: u64,
    fault: EngineFault,
) -> Result<Runtime<MemoryJournal>, OpenError> {
    #[cfg(feature = "fault-injection")]
    return Runtime::open_with_fault(journal, policy_yaml, at, fault);
    #[cfg(not(feature = "fault-injection"))]
    {
        let EngineFault = fault;
        Runtime::open(journal, policy_yaml, at)
    }
}

pub fn run(policy_yaml: &str, seed: u64, steps: u32) -> Result<Run, Vec<Diagnostic>> {
    simulate(policy_yaml, seed, steps, NO_FAULT, false)
}

/// `run` against a deliberately broken engine, to prove the invariants bite.
/// Only with the non-default `fault-injection` feature.
#[cfg(feature = "fault-injection")]
#[doc(hidden)]
pub fn run_with_fault(
    policy_yaml: &str,
    seed: u64,
    steps: u32,
    fault: trustgraph_engine::Fault,
) -> Result<Run, Vec<Diagnostic>> {
    simulate(policy_yaml, seed, steps, fault, false)
}

/// `run` with only agent-callable events and no crashes or operator.
#[doc(hidden)]
pub fn run_agents_only(policy_yaml: &str, seed: u64, steps: u32) -> Result<Run, Vec<Diagnostic>> {
    simulate(policy_yaml, seed, steps, NO_FAULT, true)
}

pub fn campaign(
    policy_yaml: &str,
    first_seed: u64,
    count: u64,
    steps: u32,
) -> Result<Campaign, Vec<Diagnostic>> {
    let mut decisions: BTreeMap<String, u64> =
        [Verdict::Allow, Verdict::Deny, Verdict::NeedsApproval]
            .iter()
            .map(|v| (v.as_str().to_string(), 0))
            .collect();
    let mut events = 0;
    let mut coverage: BTreeMap<String, u64> = COVERAGE.iter().map(|k| (k.to_string(), 0)).collect();
    let mut violations = Vec::new();
    let mut summary = Vec::new();
    for seed in first_seed..first_seed.saturating_add(count) {
        let r = run(policy_yaml, seed, steps)?;
        events += r.events.len() as u64;
        for reply in &r.replies {
            if let Reply::Decision { verdict, .. } = reply {
                *decisions.entry(verdict.as_str().into()).or_default() += 1;
            }
        }
        for (key, n) in &r.coverage {
            *coverage.entry(key.clone()).or_default() += n;
        }
        violations.extend(r.violations.iter().map(|v| (seed, v.clone())));
        summary.push((seed, r.state_digest, r.violations));
    }
    let digest = digest::structured(
        "policy-sim-campaign-v1",
        &(first_seed, count, steps, &summary),
    )
    .0;
    Ok(Campaign {
        first_seed,
        count,
        steps,
        events,
        decisions,
        violations,
        digest,
        coverage,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Record,
    Check,
    Recheck,
    Approve,
    Revoke,
    Advance,
    Propose,
    Register,
    Crash,
    LostWrite,
    Tear,
}

/// Weights per step kind: about a dozen crashes per 200-step run.
const ALL: [(Kind, u64); 11] = [
    (Kind::Record, 32),
    (Kind::Check, 56),
    (Kind::Recheck, 16),
    (Kind::Approve, 24),
    (Kind::Revoke, 8),
    (Kind::Advance, 12),
    (Kind::Propose, 12),
    (Kind::Register, 6),
    (Kind::Crash, 3),
    (Kind::LostWrite, 3),
    (Kind::Tear, 4),
];
const AGENTS_ONLY: [(Kind, u64); 6] = [
    (Kind::Record, 20),
    (Kind::Check, 40),
    (Kind::Recheck, 10),
    (Kind::Advance, 5),
    (Kind::Propose, 10),
    (Kind::Register, 5),
];

fn draw(rng: &mut Rng, table: &[(Kind, u64)]) -> Kind {
    let mut r = rng.below(table.iter().map(|(_, w)| w).sum());
    for (kind, weight) in table {
        if r < *weight {
            return *kind;
        }
        r -= weight;
    }
    table[0].0
}

/// Who sends an event. Agents only ever send agent-callable events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Actor {
    Agent,
    Host,
    Operator,
}

/// The revision and the active policy. Compared structurally: equal policies
/// have equal digests, and hashing in debug builds is slow.
fn policy_state(engine: &Engine) -> (u64, Option<Policy>) {
    (engine.policy_revision(), engine.policy().cloned())
}

fn simulate(
    policy_yaml: &str,
    seed: u64,
    steps: u32,
    fault: EngineFault,
    agents_only: bool,
) -> Result<Run, Vec<Diagnostic>> {
    let runtime =
        open_runtime(MemoryJournal::new(), policy_yaml, 0, fault).map_err(|error| match error {
            OpenError::InvalidPolicy(diagnostics) => diagnostics,
            other => vec![Diagnostic {
                severity: Severity::Error,
                path: String::new(),
                code: "open_failed",
                message: other.to_string(),
                suggestion: None,
            }],
        })?;
    // `open` validated the text, so this only fails if parse and open disagree.
    let parsed = parse(policy_yaml);
    let policy = parsed.policy.ok_or_else(|| {
        let mut diagnostics = parsed.diagnostics;
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            path: String::new(),
            code: "open_failed",
            message: "the runtime opened a policy that does not parse".into(),
            suggestion: None,
        });
        diagnostics
    })?;
    let longest_valid_for = policy
        .actions
        .values()
        .filter_map(|a| a.approval.as_ref().map(|r| r.valid_for_seconds))
        .max()
        .unwrap_or(3600);
    let mut sim = Sim {
        runtime: Some(runtime),
        genesis: policy_yaml.into(),
        fault,
        agents_only,
        rng: Rng::new(seed),
        names: Names::new(&policy),
        longest_valid_for,
        now: 0,
        step: 0,
        shadow: Shadow::new(policy, policy_yaml),
        before_last: None,
        keep_snapshot: false,
        out: Run {
            seed,
            steps,
            events: Vec::new(),
            replies: Vec::new(),
            violations: Vec::new(),
            state_digest: String::new(),
            crashes: 0,
            policy_changes: 0,
            coverage: COVERAGE.iter().map(|k| (k.to_string(), 0)).collect(),
        },
    };
    let table: &[(Kind, u64)] = if agents_only { &AGENTS_ONLY } else { &ALL };
    // The next step's kind is drawn one step early, so that the step before a
    // torn tail can keep what the state was before its last record.
    let mut next = draw(&mut sim.rng, table);
    sim.keep_snapshot = next == Kind::Tear;
    sim.setup();
    sim.cross_check();
    for step in 1..=steps {
        if !sim.out.violations.is_empty() || sim.runtime.is_none() {
            break;
        }
        sim.step = step;
        let kind = next;
        next = draw(&mut sim.rng, table);
        sim.keep_snapshot = next == Kind::Tear;
        sim.now += sim.rng.below(4);
        sim.act(kind);
        sim.cross_check();
    }
    if let Some(runtime) = &sim.runtime {
        sim.out.state_digest = runtime.engine().state_digest();
    }
    Ok(sim.out)
}

struct Sim {
    /// `None` only while crashed, or after a reopen failed.
    runtime: Option<Runtime<MemoryJournal>>,
    /// The policy text the runtime was started with; every reopen uses it.
    genesis: String,
    fault: EngineFault,
    agents_only: bool,
    rng: Rng,
    names: Names,
    longest_valid_for: u64,
    now: u64,
    step: u32,
    shadow: Shadow,
    /// The shadow and state digest from just before the last journaled record.
    before_last: Option<(Shadow, String)>,
    keep_snapshot: bool,
    out: Run,
}

impl Sim {
    fn engine(&self) -> &Engine {
        self.runtime.as_ref().expect("the runtime is open").engine()
    }

    fn note(&mut self, key: &'static str) {
        *self.out.coverage.entry(key.into()).or_default() += 1;
    }

    /// Record findings, and the coverage notes the shadow made meanwhile.
    fn flag(&mut self, findings: Vec<Finding>) {
        for key in std::mem::take(&mut self.shadow.notes) {
            self.note(key);
        }
        for (invariant, detail) in findings {
            self.out.violations.push(Violation {
                invariant,
                step: self.step,
                detail,
            });
        }
    }

    /// Send one event and check invariant 5 around it.
    fn submit(&mut self, actor: Actor, event: Event) -> Result<Reply, JournalError> {
        assert!(
            actor != Actor::Agent || event.agent_callable(),
            "agents only send agent-callable events"
        );
        let runtime = self.runtime.as_mut().expect("the runtime is open");
        let before = policy_state(runtime.engine());
        let seq = runtime.last_seq();
        let snapshot = self
            .keep_snapshot
            .then(|| (self.shadow.clone(), runtime.engine().state_digest()));
        let result = runtime.handle(self.now, event.clone());
        let after = policy_state(runtime.engine());
        // Refused before the journal (bad shape, too large): it never happened,
        // so the snapshot before the last journaled record stays as it was.
        let journaled = runtime.last_seq() != seq;
        let activated = matches!(event, Event::ActivatePolicy { .. })
            && matches!(result, Ok(Reply::Activated { .. }));
        if before != after && !activated {
            self.flag(vec![(
                AGENTS_CANNOT_CHANGE_POLICY,
                format!(
                    "{event:?} moved the policy from revision {} to {}",
                    before.0, after.0
                ),
            )]);
        }
        if let Ok(reply) = &result {
            if journaled {
                self.before_last = snapshot;
            }
            self.out.events.push((self.now, event));
            self.out.replies.push(reply.clone());
            if activated {
                self.out.policy_changes += 1;
            }
        }
        result
    }

    /// `submit` where a journal error is itself a violation.
    fn send(&mut self, actor: Actor, event: Event) -> Option<Reply> {
        match self.submit(actor, event) {
            Ok(reply) => Some(reply),
            Err(error) => {
                self.flag(vec![(
                    REPLAY_EQUIVALENT,
                    format!("unexpected journal error: {error}"),
                )]);
                None
            }
        }
    }

    fn check(&mut self, ask: Ask) -> Option<Reply> {
        let reply = self.send(Actor::Agent, ask.event())?;
        let findings = self.shadow.on_check(self.now, &ask, &reply);
        self.flag(findings);
        Some(reply)
    }

    /// The runtime's policy must be the shadow's after every step.
    fn cross_check(&mut self) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        let engine = runtime.engine();
        if engine.policy_revision() != self.shadow.revision
            || engine.policy() != Some(&self.shadow.policy)
        {
            self.flag(vec![(
                AGENTS_CANNOT_CHANGE_POLICY,
                format!(
                    "the runtime's policy is revision {} {:?}, expected revision {} {}",
                    engine.policy_revision(),
                    engine.policy().map(|p| p.digest()),
                    self.shadow.revision,
                    self.shadow.digest
                ),
            )]);
        }
    }

    fn setup(&mut self) {
        let sources = self.names.sources.clone();
        for (n, source) in (1..).zip(sources) {
            self.register(source, sha(n));
        }
    }

    fn act(&mut self, kind: Kind) {
        match kind {
            Kind::Record => self.record(),
            Kind::Check => {
                let ask = self.random_ask();
                self.check(ask);
            }
            Kind::Recheck => self.recheck(),
            Kind::Approve => self.approve(),
            Kind::Revoke => self.revoke(),
            Kind::Advance => self.advance(),
            Kind::Propose => self.propose(),
            Kind::Register => {
                let source = self.rng.pick(&self.names.sources).cloned();
                if let Some(source) = source {
                    let digest = sha(self.rng.next_u64());
                    self.register(source, digest);
                }
            }
            Kind::Crash => self.clean_crash(),
            Kind::LostWrite => self.lost_write(),
            Kind::Tear => self.torn_tail(),
        }
    }

    fn register(&mut self, source: String, sha256: String) {
        let event = Event::RegisterSource {
            schema: None,
            source: source.clone(),
            sha256,
        };
        if let Some(reply) = self.send(Actor::Host, event) {
            let engine = self.runtime.as_ref().expect("the runtime is open").engine();
            let findings = self
                .shadow
                .on_artifact(&reply, false, BTreeSet::from([source]), |id| {
                    engine.lineage(id)
                });
            self.flag(findings);
        }
    }

    fn record(&mut self) {
        if self.shadow.artifacts.is_empty() {
            return;
        }
        let agent = self.names.agent(&mut self.rng);
        let mut inputs: Vec<String> = Vec::new();
        for _ in 0..=self.rng.below(3) {
            let id = self.rng.pick(&self.shadow.artifacts).cloned();
            if let Some(id) = id
                && !inputs.contains(&id)
            {
                inputs.push(id);
            }
        }
        let expected = self.shadow.lineage_of(&inputs);
        let event = Event::RecordOutput {
            contract: None,
            agent,
            step: format!("step{}", self.rng.below(4)),
            inputs,
            sha256: sha(self.rng.below(16)),
        };
        if let Some(reply) = self.send(Actor::Agent, event) {
            if matches!(&reply, Reply::Rejected { code, .. } if code == "agent_cannot_use_source") {
                self.note("agent_denied_source");
            }
            let engine = self.runtime.as_ref().expect("the runtime is open").engine();
            let findings = self
                .shadow
                .on_artifact(&reply, true, expected, |id| engine.lineage(id));
            self.flag(findings);
        }
    }

    fn random_ask(&mut self) -> Ask {
        let agent = self.names.agent(&mut self.rng);
        let artifact = match self.rng.chance(3) {
            true => None,
            false => self.rng.pick(&self.shadow.artifacts).cloned(),
        };
        Ask {
            agent,
            artifact: artifact.unwrap_or_else(|| UNKNOWN_ARTIFACT.into()),
            action: self.names.action(&mut self.rng),
            recipients: self.names.recipients(&mut self.rng),
        }
    }

    /// Move the clock: usually a random jump, sometimes to the last second of
    /// a live approval and then to the second it expires, re-checking at each.
    fn advance(&mut self) {
        let live = self.shadow.live_grants(self.now);
        let boundary = if self.rng.chance(40) {
            self.rng.pick(&live).cloned()
        } else {
            None
        };
        let Some((ask, end)) = boundary else {
            self.now += self.rng.below(2 * self.longest_valid_for + 1);
            return;
        };
        let allowed = |reply: &Option<Reply>| {
            matches!(
                reply,
                Some(Reply::Decision {
                    verdict: Verdict::Allow,
                    ..
                })
            )
        };
        self.now = end - 1;
        if allowed(&self.check(ask.clone())) {
            self.note("expiry_boundary_before");
        }
        self.now = end;
        if !allowed(&self.check(ask)) {
            self.note("expiry_boundary_at");
        }
    }

    /// Sometimes tear the journal right after `event` when it succeeds, so
    /// the dropped record is that event. True when it tore.
    fn send_maybe_torn(&mut self, event: Event) -> (Option<Reply>, bool) {
        let tear = self.rng.chance(10);
        let keep = self.keep_snapshot;
        self.keep_snapshot |= tear;
        let reply = self.send(Actor::Operator, event);
        self.keep_snapshot = keep;
        let ok = matches!(
            reply,
            Some(Reply::Approved { .. } | Reply::Revoked { .. } | Reply::Activated { .. })
        );
        (reply, tear && ok)
    }

    /// Re-send an approved, pending or allowed check, as a host would.
    fn recheck(&mut self) {
        let grants = self.shadow.grant_asks();
        let pending: Vec<Ask> = self.shadow.pending.values().cloned().collect();
        let ask = if !grants.is_empty() && self.rng.chance(60) {
            self.rng.pick(&grants).cloned()
        } else if !pending.is_empty() && self.rng.chance(50) {
            self.rng.pick(&pending).cloned()
        } else {
            self.rng.pick(&self.shadow.allowed).cloned()
        };
        let ask = ask.unwrap_or_else(|| self.random_ask());
        self.check(ask);
    }

    /// A reviewer for `ask`: often a valid one, otherwise anyone.
    fn reviewer(&mut self, ask: Option<&Ask>, valid_percent: u64) -> String {
        let valid = ask.map(|a| self.shadow.reviewers(a)).unwrap_or_default();
        if !valid.is_empty() && self.rng.chance(valid_percent) {
            return self.rng.pick(&valid).cloned().unwrap_or_default();
        }
        self.names.person(&mut self.rng)
    }

    fn approve(&mut self) {
        let pending: Vec<String> = self.shadow.pending.keys().cloned().collect();
        let roll = self.rng.below(100);
        let decision = if roll < 80 && !pending.is_empty() {
            self.rng.pick(&pending).cloned()
        } else if roll < 95 {
            self.rng.pick(&self.shadow.stale).cloned()
        } else {
            None
        };
        let decision = decision.unwrap_or_else(|| sha(self.rng.next_u64()));
        let ask = self.shadow.pending.get(&decision).cloned();
        let reviewer = self.reviewer(ask.as_ref(), 60);
        let event = Event::Approve {
            decision: decision.clone(),
            reviewer: reviewer.clone(),
        };
        let (reply, tear) = self.send_maybe_torn(event);
        let Some(reply) = reply else {
            return;
        };
        let findings = self
            .shadow
            .on_approve(self.now, &decision, &reviewer, &reply);
        self.flag(findings);
        if tear {
            self.torn_tail();
            return;
        }
        if let (Reply::Approved { .. }, Some(ask)) = (&reply, ask)
            && self.rng.chance(50)
        {
            self.check(ask);
        }
    }

    fn revoke(&mut self) {
        let mut ids = self.shadow.grant_ids();
        if ids.is_empty() || self.rng.chance(30) {
            ids.extend(self.shadow.pending.keys().cloned());
        }
        let Some(decision) = self.rng.pick(&ids).cloned() else {
            return;
        };
        let ask = self.shadow.ask_for(&decision);
        let reviewer = self.reviewer(ask.as_ref(), 70);
        let event = Event::Revoke {
            decision: decision.clone(),
            reviewer: reviewer.clone(),
        };
        let (reply, tear) = self.send_maybe_torn(event);
        let Some(reply) = reply else {
            return;
        };
        let findings = self
            .shadow
            .on_revoke(self.now, &decision, &reviewer, &reply);
        self.flag(findings);
        if tear {
            self.torn_tail();
            return;
        }
        // A host re-checks after a revocation; it must no longer be allowed.
        if let (Reply::Revoked { .. }, Some(ask)) = (&reply, ask)
            && self.rng.chance(50)
        {
            self.check(ask);
        }
    }

    /// Propose the active policy unchanged or mutated, by an agent or the
    /// operator; the operator sometimes activates it or an earlier proposal.
    fn propose(&mut self) {
        let (yaml, policy) = match self.rng.chance(25) {
            true => (self.shadow.yaml.clone(), self.shadow.policy.clone()),
            false => {
                let policy = self.names.mutate(&self.shadow.policy, &mut self.rng);
                (policy.to_yaml(), policy)
            }
        };
        let by_agent = self.agents_only || self.rng.chance(35);
        let (actor, agent) = match by_agent {
            true => (Actor::Agent, Some(self.names.agent(&mut self.rng))),
            false => (Actor::Operator, None),
        };
        let event = Event::ProposePolicy {
            agent,
            policy: yaml.clone(),
        };
        let Some(reply) = self.send(actor, event) else {
            return;
        };
        self.shadow.on_proposed(&reply, yaml, policy);
        if self.agents_only || !self.rng.chance(60) {
            return;
        }
        let id = match &reply {
            Reply::Proposed { id, .. } if self.rng.chance(80) => Some(id.clone()),
            _ => self
                .rng
                .pick(&self.shadow.proposals)
                .map(|(id, ..)| id.clone()),
        };
        if let Some(id) = id {
            self.activate(id);
        }
    }

    fn activate(&mut self, id: String) {
        let old = self.shadow.policy.clone();
        let old_digest = self.shadow.digest.clone();
        let live: Vec<Ask> = self
            .shadow
            .live_grants(self.now)
            .into_iter()
            .map(|(ask, _)| ask)
            .collect();
        let event = Event::ActivatePolicy {
            proposal: id.clone(),
            operator: "operator".into(),
        };
        let (reply, tear) = self.send_maybe_torn(event);
        let Some(reply) = reply else {
            return;
        };
        if !matches!(reply, Reply::Activated { .. }) {
            return;
        }
        let findings = self.shadow.on_activated(&reply, &id);
        self.flag(findings);
        if tear {
            self.torn_tail();
            return;
        }
        let identical = self.shadow.digest == old_digest;
        if identical && !live.is_empty() {
            self.note("identical_reactivation_with_live_approvals");
        }
        for (name, source) in &old.sources {
            if !self.shadow.has_derived_from(name) {
                continue;
            }
            match self.shadow.policy.sources.get(name) {
                None => self.note("source_removed_with_derived_outputs"),
                Some(now)
                    if !old
                        .people(&source.readers)
                        .is_subset(&self.shadow.policy.people(&now.readers)) =>
                {
                    self.note("narrowing_with_derived_outputs")
                }
                Some(_) => {}
            }
        }
        // Invariant 4: earlier allowed checks whose recipients are no longer
        // all readers of the policy text are re-sent, and so are approved ones
        // and those with an approval live until now (invariant 2). The rest
        // stay remembered as they are.
        let (mut resend, keep): (Vec<Ask>, Vec<Ask>) = std::mem::take(&mut self.shadow.allowed)
            .into_iter()
            .partition(|ask| !self.shadow.covered(ask) || self.shadow.approved(ask));
        self.shadow.allowed = keep;
        for ask in &live {
            if !resend.contains(ask) {
                resend.push(ask.clone());
            }
        }
        for ask in resend {
            let Some(reply) = self.check(ask.clone()) else {
                return;
            };
            if identical && live.contains(&ask) {
                self.note("recheck_after_identical_reactivation");
            }
            if let Some(finding) = self.shadow.narrowing_finding(&ask, &reply) {
                self.flag(vec![finding]);
            }
        }
    }

    /// Reopen from `journal` with the original policy text and expect
    /// `digest`. False when the runtime could not reopen.
    fn reopen(&mut self, journal: MemoryJournal, digest: &str) -> bool {
        self.out.crashes += 1;
        match open_runtime(journal, &self.genesis, self.now, self.fault) {
            Ok(runtime) => {
                let reopened = runtime.engine().state_digest();
                self.runtime = Some(runtime);
                if reopened != digest {
                    self.flag(vec![(
                        REPLAY_EQUIVALENT,
                        format!("reopened at state {reopened}, expected {digest}"),
                    )]);
                }
                true
            }
            Err(error) => {
                self.flag(vec![(REPLAY_EQUIVALENT, format!("reopen failed: {error}"))]);
                false
            }
        }
    }

    fn take_journal(&mut self) -> MemoryJournal {
        self.runtime
            .take()
            .expect("the runtime is open")
            .into_journal()
    }

    /// Crash after the last durable record; the next decisions must not
    /// change. Sometimes nothing is re-sent first, so the last record before
    /// the crash is whatever the previous step wrote.
    fn clean_crash(&mut self) {
        self.note("crash_clean");
        let asks = match self.rng.chance(40) {
            true => {
                self.note("crash_without_resend");
                Vec::new()
            }
            false => self.shadow.recent_checks.clone(),
        };
        let before: Vec<Option<Reply>> = asks.iter().map(|a| self.check(a.clone())).collect();
        let digest = self.engine().state_digest();
        let journal = self.take_journal();
        if !self.reopen(journal, &digest) {
            return;
        }
        for (ask, expected) in asks.into_iter().zip(before) {
            let got = self.check(ask.clone());
            if got != expected {
                self.flag(vec![(
                    REPLAY_EQUIVALENT,
                    format!("{ask:?} answered {expected:?} before the crash, {got:?} after"),
                )]);
            }
        }
    }

    /// Crash, then the first write after the restart fails: nothing changes.
    fn lost_write(&mut self) {
        self.note("crash_lost_write");
        let digest = self.engine().state_digest();
        let mut journal = self.take_journal();
        journal.fail_next_append();
        if !self.reopen(journal, &digest) {
            return;
        }
        let ask = self.random_ask();
        if let Ok(reply) = self.submit(Actor::Agent, ask.event()) {
            self.flag(vec![(
                REPLAY_EQUIVALENT,
                format!("{ask:?} answered {reply:?} although its write failed"),
            )]);
        }
        let after = self.engine().state_digest();
        if after != digest {
            self.flag(vec![(
                REPLAY_EQUIVALENT,
                format!("state moved to {after} although the write failed"),
            )]);
        }
    }

    /// The last record never became durable: the state is the one before it,
    /// and the simulator forgets what that record did.
    fn torn_tail(&mut self) {
        let Some((shadow, digest)) = self.before_last.take() else {
            return;
        };
        self.note("crash_torn_tail");
        match self.out.events.last().zip(self.out.replies.last()) {
            Some(((_, Event::Approve { .. }), Reply::Approved { .. })) => self.note("torn_approve"),
            Some(((_, Event::Revoke { .. }), Reply::Revoked { .. })) => self.note("torn_revoke"),
            Some(((_, Event::ActivatePolicy { .. }), Reply::Activated { .. })) => {
                self.note("torn_activate")
            }
            _ => {}
        }
        let mut journal = self.take_journal();
        journal.tear_tail();
        if self.reopen(journal, &digest) {
            self.shadow = shadow;
        }
    }
}
