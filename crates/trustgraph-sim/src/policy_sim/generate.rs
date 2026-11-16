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

/// How many kinds of policy mutation `Names::mutate` draws from.
pub(super) const MUTATIONS: u64 = 6;

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
        let kind = rng.below(MUTATIONS);
        self.mutate_with(policy, kind, rng)
    }

    /// Mutation `kind` (0..MUTATIONS): remove a reader, add a reader, remove
    /// a source, remove an approval, shorten a `valid_for`, add a person to
    /// a group. Each changes the policy whenever such a change exists.
    pub(super) fn mutate_with(&self, policy: &Policy, kind: u64, rng: &mut Rng) -> Policy {
        let mut p = policy.clone();
        p.tests.clear();
        let gated = |p: &Policy, min: u64| -> Vec<String> {
            p.actions
                .iter()
                .filter(|(_, a)| {
                    a.approval
                        .as_ref()
                        .is_some_and(|r| r.valid_for_seconds >= min)
                })
                .map(|(name, _)| name.clone())
                .collect()
        };
        match kind {
            0 => {
                let pairs: Vec<(String, String)> = p
                    .sources
                    .iter()
                    .flat_map(|(name, s)| s.readers.iter().map(|r| (name.clone(), r.clone())))
                    .collect();
                if let Some((name, reader)) = rng.pick(&pairs)
                    && let Some(source) = p.sources.get_mut(name)
                {
                    source.readers.remove(reader);
                }
            }
            1 => {
                // Only names the policy defines, or it would not parse.
                let known: BTreeSet<String> = p
                    .groups
                    .iter()
                    .flat_map(|(group, members)| std::iter::once(group).chain(members))
                    .cloned()
                    .collect();
                let pairs: Vec<(String, String)> = p
                    .sources
                    .iter()
                    .flat_map(|(name, s)| {
                        known
                            .iter()
                            .filter(|r| !s.readers.contains(*r))
                            .map(|r| (name.clone(), r.clone()))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                if let Some((name, reader)) = rng.pick(&pairs)
                    && let Some(source) = p.sources.get_mut(name)
                {
                    source.readers.insert(reader.clone());
                }
            }
            2 => {
                let sources: Vec<String> = p.sources.keys().cloned().collect();
                if let Some(name) = rng.pick(&sources) {
                    p.sources.remove(name);
                }
            }
            3 => {
                if let Some(action) = rng.pick(&gated(&p, 0)).and_then(|n| p.actions.get_mut(n)) {
                    action.approval = None;
                }
            }
            4 => {
                if let Some(rule) = rng
                    .pick(&gated(&p, 2))
                    .and_then(|n| p.actions.get_mut(n))
                    .and_then(|a| a.approval.as_mut())
                {
                    rule.valid_for_seconds /= 2;
                }
            }
            _ => {
                let pairs: Vec<(String, String)> = p
                    .groups
                    .iter()
                    .flat_map(|(group, members)| {
                        self.people
                            .iter()
                            .filter(|person| !members.contains(*person))
                            .map(|person| (group.clone(), person.clone()))
                            .collect::<Vec<_>>()
                    })
                    .collect();
                if let Some((group, person)) = rng.pick(&pairs)
                    && let Some(members) = p.groups.get_mut(group)
                {
                    members.insert(person.clone());
                }
            }
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trustgraph_policy::{Severity, parse};

    #[test]
    fn every_mutation_kind_changes_the_policy_and_stays_parseable() {
        for yaml in [
            include_str!("../../../../examples/policies/slack-digest.policy.yaml"),
            include_str!("../../../trustgraph-policy/tests/fixtures/spec-example.policy.yaml"),
        ] {
            let policy = parse(yaml).policy.unwrap();
            let names = Names::new(&policy);
            let mut cleared = policy.clone();
            cleared.tests.clear();
            for kind in 0..MUTATIONS {
                for seed in 0..16 {
                    let mutated = names.mutate_with(&policy, kind, &mut Rng::new(seed));
                    assert_ne!(
                        mutated.digest(),
                        cleared.digest(),
                        "kind {kind} seed {seed}"
                    );
                    let parsed = parse(&mutated.to_yaml());
                    assert!(
                        parsed
                            .diagnostics
                            .iter()
                            .all(|d| d.severity != Severity::Error),
                        "kind {kind} seed {seed}: {:?}",
                        parsed.diagnostics
                    );
                    assert_eq!(parsed.policy.map(|p| p.digest()), Some(mutated.digest()));
                }
            }
        }
    }
}
