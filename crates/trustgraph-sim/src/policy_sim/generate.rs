//! Seeded choices: the PRNG, the names a run draws from, and policy mutations.

use std::collections::BTreeSet;

use trustgraph_policy::Policy;

/// An agent the policy does not name.
pub(super) const ROGUE_AGENT: &str = "rogue_agent";
/// A person in no group.
pub(super) const OUTSIDER: &str = "outsider";
/// An action the policy does not name.
pub(super) const UNKNOWN_ACTION: &str = "unknown_action";
/// An artifact id the runtime never issued.
pub(super) const UNKNOWN_ARTIFACT: &str = "unknown_artifact_id";

/// splitmix64: small, seedable and dependency-free.
#[derive(Debug, Clone)]
pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub(super) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-enough in `0..n`; 0 when `n` is 0.
    pub(super) fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }

    pub(super) fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    pub(super) fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }
        let index = usize::try_from(self.below(items.len() as u64)).unwrap_or(0);
        items.get(index)
    }
}

/// A valid lowercase-hex sha256 digest, distinct per `n`.
pub(super) fn sha(n: u64) -> String {
    format!("{n:064x}")
}

/// Every name a run draws from, fixed from the starting policy plus a few
/// names the policy does not know.
#[derive(Debug, Clone)]
pub(super) struct Names {
    agents: Vec<String>,
    /// Group members, then the outsider.
    people: Vec<String>,
    groups: Vec<String>,
    pub(super) sources: Vec<String>,
    actions: Vec<String>,
}

impl Names {
    pub(super) fn new(policy: &Policy) -> Self {
        let mut people: BTreeSet<String> = policy.groups.values().flatten().cloned().collect();
        people.insert(OUTSIDER.into());
        Names {
            agents: policy.agents.iter().cloned().collect(),
            people: people.into_iter().collect(),
            groups: policy.groups.keys().cloned().collect(),
            sources: policy.sources.keys().cloned().collect(),
            actions: policy.actions.keys().cloned().collect(),
        }
    }

    pub(super) fn agent(&self, rng: &mut Rng) -> String {
        match rng.chance(8) {
            true => ROGUE_AGENT.into(),
            false => rng
                .pick(&self.agents)
                .map_or(ROGUE_AGENT.into(), Clone::clone),
        }
    }

    pub(super) fn action(&self, rng: &mut Rng) -> String {
        match rng.chance(5) {
            true => UNKNOWN_ACTION.into(),
            false => rng
                .pick(&self.actions)
                .map_or(UNKNOWN_ACTION.into(), Clone::clone),
        }
    }

    /// A person, an agent (who may not review its own request) or the outsider.
    pub(super) fn person(&self, rng: &mut Rng) -> String {
        let total = (self.people.len() + self.agents.len()) as u64;
        let index = usize::try_from(rng.below(total)).unwrap_or(0);
        self.people
            .iter()
            .chain(&self.agents)
            .nth(index)
            .cloned()
            .unwrap_or_else(|| OUTSIDER.into())
    }

    /// A group name or a person.
    fn reader(&self, rng: &mut Rng) -> String {
        match rng.chance(50) {
            true => rng.pick(&self.groups).map_or(OUTSIDER.into(), Clone::clone),
            false => rng.pick(&self.people).map_or(OUTSIDER.into(), Clone::clone),
        }
    }

    /// Mostly the action's whole audience, sometimes one or two named recipients.
    pub(super) fn recipients(&self, rng: &mut Rng) -> Option<BTreeSet<String>> {
        if rng.chance(60) {
            return None;
        }
        let count = 1 + rng.below(2);
        Some((0..count).map(|_| self.reader(rng)).collect())
    }

    /// One random change to `policy`, with its inline tests cleared so that
    /// a stale test cannot make the proposal invalid.
    pub(super) fn mutate(&self, policy: &Policy, rng: &mut Rng) -> Policy {
        let mut p = policy.clone();
        p.tests.clear();
        let sources: Vec<String> = p.sources.keys().cloned().collect();
        let gated: Vec<String> = p
            .actions
            .iter()
            .filter(|(_, a)| a.approval.is_some())
            .map(|(name, _)| name.clone())
            .collect();
        match rng.below(6) {
            0 => {
                if let Some(name) = rng.pick(&sources) {
                    let readers: Vec<String> = p.sources[name].readers.iter().cloned().collect();
                    if let Some(reader) = rng.pick(&readers)
                        && let Some(source) = p.sources.get_mut(name)
                    {
                        source.readers.remove(reader);
                    }
                }
            }
            1 => {
                if let Some(name) = rng.pick(&sources).cloned() {
                    let reader = self.reader(rng);
                    if let Some(source) = p.sources.get_mut(&name) {
                        source.readers.insert(reader);
                    }
                }
            }
            2 => {
                if let Some(name) = rng.pick(&sources) {
                    p.sources.remove(name);
                }
            }
            3 => {
                if let Some(action) = rng.pick(&gated).and_then(|n| p.actions.get_mut(n)) {
                    action.approval = None;
                }
            }
            4 => {
                if let Some(name) = rng.pick(&gated)
                    && let Some(rule) = p.actions.get_mut(name).and_then(|a| a.approval.as_mut())
                {
                    rule.valid_for_seconds = (rule.valid_for_seconds / 2).max(1);
                }
            }
            _ => {
                if let Some(group) = rng.pick(&self.groups).cloned() {
                    let person = rng.pick(&self.people).map_or(OUTSIDER.into(), Clone::clone);
                    if let Some(members) = p.groups.get_mut(&group) {
                        members.insert(person);
                    }
                }
            }
        }
        p
    }
}
