use std::collections::BTreeMap;

use trustgraph_engine::{Event, Fault, MemoryJournal, Reply, Runtime};
use trustgraph_policy::Verdict;
use trustgraph_sim::policy_sim::{Run, campaign, run, run_agents_only, run_with_fault};

const EXAMPLES: [(&str, &str); 5] = [
    (
        "customer-email",
        include_str!("../../../examples/policies/customer-email.policy.yaml"),
    ),
    (
        "multi-agent-research",
        include_str!("../../../examples/policies/multi-agent-research.policy.yaml"),
    ),
    (
        "pull-request-bot",
        include_str!("../../../examples/policies/pull-request-bot.policy.yaml"),
    ),
    (
        "slack-digest",
        include_str!("../../../examples/policies/slack-digest.policy.yaml"),
    ),
    (
        "support-replies",
        include_str!("../../../examples/policies/support-replies.policy.yaml"),
    ),
];
const SLACK: &str = EXAMPLES[3].1;

#[test]
fn examples_hold_under_simulation() {
    // One thread per example: each campaign takes several seconds in debug.
    let results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = EXAMPLES
            .iter()
            .map(|(name, yaml)| scope.spawn(move || (*name, campaign(yaml, 0, 64, 200).unwrap())))
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (name, result) in results {
        assert!(
            result.violations.is_empty(),
            "{name}: {} violations, first {:?}",
            result.violations.len(),
            result.violations.first()
        );
        assert!(result.events > 10_000, "{name}: {} events", result.events);
        for key in ["allow", "deny", "needs_approval"] {
            assert!(
                result.decisions.get(key).copied().unwrap_or(0) > 0,
                "{name}: no `{key}` decisions in {:?}",
                result.decisions
            );
        }
        assert_eq!(
            result.decisions.keys().collect::<Vec<_>>(),
            ["allow", "deny", "needs_approval"]
        );
    }
}

#[test]
fn runs_exercise_faults() {
    for (name, yaml) in EXAMPLES {
        let (mut crashes, mut changes) = (0, 0);
        for seed in 0..8 {
            let r = run(yaml, seed, 200).unwrap();
            crashes += r.crashes;
            changes += r.policy_changes;
        }
        assert!(crashes > 0, "{name}: no crashes");
        assert!(changes > 0, "{name}: no policy changes");
    }
}

#[test]
fn same_seed_same_run() {
    let a = run(SLACK, 7, 200).unwrap();
    let b = run(SLACK, 7, 200).unwrap();
    assert_eq!(a.state_digest, b.state_digest);
    assert_eq!(a.events, b.events);
    assert_eq!(a, b);
}

#[test]
fn same_arguments_same_campaign() {
    let a = campaign(SLACK, 3, 4, 100).unwrap();
    let b = campaign(SLACK, 3, 4, 100).unwrap();
    assert_eq!(a, b);
    let c = campaign(SLACK, 4, 4, 100).unwrap();
    assert_ne!(a.digest, c.digest);
}

#[test]
fn different_seeds_differ() {
    let a = run(SLACK, 1, 200).unwrap();
    let b = run(SLACK, 2, 200).unwrap();
    assert_ne!(a.events, b.events);
}

/// The first seed in `0..64` of any example whose faulted run reports one of `invariants`.
fn caught(fault: Fault, invariants: &[&str]) -> Option<(&'static str, Run)> {
    for (name, yaml) in EXAMPLES {
        for seed in 0..64 {
            let r = run_with_fault(yaml, seed, 200, fault).unwrap();
            if r.violations
                .iter()
                .any(|v| invariants.contains(&v.invariant))
            {
                return Some((name, r));
            }
        }
    }
    None
}

#[test]
fn skip_reader_refresh_is_caught() {
    let found = caught(Fault::SkipReaderRefresh, &["narrowing_applies", "no_leak"]);
    assert!(found.is_some(), "SkipReaderRefresh went unnoticed");
}

#[test]
fn kept_approvals_are_caught() {
    let found = caught(Fault::KeepApprovalsAcrossPolicyChange, &["approval_scoped"]);
    assert!(
        found.is_some(),
        "KeepApprovalsAcrossPolicyChange went unnoticed"
    );
}

#[test]
fn invalid_policy_is_an_error() {
    assert!(run("version: 2", 0, 10).is_err());
}

#[test]
fn mutations_change_decisions() {
    // Some check's verdict differs before and after a policy activation.
    for seed in 0..64 {
        let r = run(SLACK, seed, 200).unwrap();
        let mut epoch = 0u32;
        let mut last: BTreeMap<String, (Verdict, u32)> = BTreeMap::new();
        for ((_, event), reply) in r.events.iter().zip(&r.replies) {
            match (event, reply) {
                (Event::ActivatePolicy { .. }, Reply::Activated { .. }) => epoch += 1,
                (Event::Check { .. }, Reply::Decision { verdict, .. }) => {
                    let key = format!("{event:?}");
                    if let Some((before, at)) = last.get(&key)
                        && *at < epoch
                        && before != verdict
                    {
                        return;
                    }
                    last.insert(key, (*verdict, epoch));
                }
                _ => {}
            }
        }
    }
    panic!("no check changed its verdict across an activation");
}

#[test]
fn agent_events_never_change_policy() {
    let r = run_agents_only(SLACK, 3, 500).unwrap();
    assert!(r.violations.is_empty(), "{:?}", r.violations);
    assert_eq!(r.policy_changes, 0);
    assert_eq!(r.crashes, 0);
    assert!(r.events.iter().all(|(_, e)| e.agent_callable()));
    assert!(
        r.events
            .iter()
            .any(|(_, e)| matches!(e, Event::ProposePolicy { agent: Some(_), .. })),
        "agents never tried to propose a policy"
    );
    // Replayed into a fresh runtime, the same events give the same replies
    // and never move the revision.
    let mut runtime = Runtime::open(MemoryJournal::new(), SLACK, 0).unwrap();
    for ((at, event), expected) in r.events.iter().zip(&r.replies) {
        let reply = runtime.handle(*at, event.clone()).unwrap();
        assert_eq!(&reply, expected);
        assert_eq!(runtime.engine().policy_revision(), 1);
    }
}

#[test]
fn violation_is_reproducible() {
    let (name, first) = caught(Fault::KeepApprovalsAcrossPolicyChange, &["approval_scoped"])
        .expect("a violating seed");
    let yaml = EXAMPLES.iter().find(|(n, _)| *n == name).unwrap().1;
    let again = run_with_fault(
        yaml,
        first.seed,
        first.steps,
        Fault::KeepApprovalsAcrossPolicyChange,
    )
    .unwrap();
    assert!(!again.violations.is_empty());
    assert_eq!(again.violations, first.violations);
    assert_eq!(again, first);
}
