//! `Policy::people` is the one group expansion that `decide`, the engine,
//! lint, diff and the simulator's oracle all rely on, so it is tested
//! directly here, including against an independent reference.

use std::collections::{BTreeMap, BTreeSet};

use trustgraph_policy::{Policy, parse};

const SPEC: &str = include_str!("fixtures/spec-example.policy.yaml");

fn set<const N: usize>(items: [&str; N]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn spec() -> Policy {
    parse(SPEC).policy.expect("the spec example parses")
}

#[test]
fn a_group_expands_to_its_members() {
    let policy = spec();
    assert_eq!(policy.people(&set(["engineering"])), set(["ravi", "sam"]));
    assert_eq!(policy.people(&set(["hr"])), set(["nina"]));
}

#[test]
fn a_person_stands_for_itself() {
    let policy = spec();
    assert_eq!(policy.people(&set(["sam"])), set(["sam"]));
    assert_eq!(policy.people(&set(["acme"])), set(["acme"]));
}

#[test]
fn unknown_names_and_agents_are_dropped() {
    let policy = spec();
    assert_eq!(policy.people(&set(["ghost"])), BTreeSet::new());
    // Agents are principals, never people.
    assert_eq!(policy.people(&set(["planner", "mailer"])), BTreeSet::new());
    // Source and action names are not people either.
    assert_eq!(
        policy.people(&set(["salaries", "post_to_slack"])),
        BTreeSet::new()
    );
    assert_eq!(
        policy.people(&set(["ghost", "planner", "sam"])),
        set(["sam"])
    );
}

#[test]
fn several_names_give_the_union() {
    let policy = spec();
    assert_eq!(
        policy.people(&set(["hr", "engineering", "leo", "ghost"])),
        set(["leo", "nina", "ravi", "sam"])
    );
    // A person also reached through their group is listed once.
    assert_eq!(
        policy.people(&set(["engineering", "sam"])),
        set(["ravi", "sam"])
    );
}

#[test]
fn empty_input_gives_nobody() {
    assert_eq!(spec().people(&BTreeSet::new()), BTreeSet::new());
}

#[test]
fn expansion_is_idempotent() {
    let policy = spec();
    for names in [
        set(["hr", "engineering"]),
        set(["sales", "acme", "planner"]),
        set(["ghost"]),
    ] {
        let once = policy.people(&names);
        assert_eq!(policy.people(&once), once, "{names:?}");
    }
}

/// splitmix64; enough randomness for a property loop without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn subset(&mut self, pool: &[String], max: usize) -> BTreeSet<String> {
        let count = self.below(max + 1);
        (0..count)
            .map(|_| pool[self.below(pool.len())].clone())
            .collect()
    }
}

fn pool(prefix: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{prefix}{i}")).collect()
}

/// A random policy shaped as `parse` allows: group names, people and agents
/// come from disjoint pools, so no group contains a group.
fn random_policy(rng: &mut Rng) -> Policy {
    let group_names = pool("g", 4);
    let people = pool("p", 6);
    let mut groups = BTreeMap::new();
    for name in &group_names {
        if rng.below(4) != 0 {
            let mut members = rng.subset(&people, 4);
            if members.is_empty() {
                members.insert(people[rng.below(people.len())].clone());
            }
            groups.insert(name.clone(), members);
        }
    }
    Policy {
        version: 1,
        groups,
        agents: rng.subset(&pool("a", 3), 3),
        sources: BTreeMap::new(),
        actions: BTreeMap::new(),
        tests: Vec::new(),
    }
}

/// Written from the documented meaning, not from the implementation: the
/// people a set of names reaches are the members of every named group plus
/// every named person who belongs to some group.
fn reference(policy: &Policy, names: &BTreeSet<String>) -> BTreeSet<String> {
    let everyone: BTreeSet<&String> = policy.groups.values().flatten().collect();
    let mut out = BTreeSet::new();
    for name in names {
        match policy.groups.get(name) {
            Some(members) => {
                for member in members {
                    out.insert(member.clone());
                }
            }
            None if everyone.contains(name) => {
                out.insert(name.clone());
            }
            None => {}
        }
    }
    out
}

#[test]
fn people_matches_an_independent_reference() {
    // Names drawn from groups, people, agents and names nobody defined.
    let mut names_pool = pool("g", 4);
    names_pool.extend(pool("p", 7));
    names_pool.extend(pool("a", 3));
    names_pool.extend(pool("x", 2));
    for seed in 0..2_000 {
        let mut rng = Rng(seed);
        let policy = random_policy(&mut rng);
        let names = rng.subset(&names_pool, 6);
        let got = policy.people(&names);
        assert_eq!(got, reference(&policy, &names), "seed {seed}: {names:?}");
        assert_eq!(policy.people(&got), got, "seed {seed}: not idempotent");
        assert!(
            got.iter().all(|p| !policy.groups.contains_key(p)),
            "seed {seed}: a group name came out as a person"
        );
        assert!(
            got.iter().all(|p| !policy.agents.contains(p)),
            "seed {seed}: an agent came out as a person"
        );
    }
}
