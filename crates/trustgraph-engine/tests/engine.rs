use std::collections::BTreeSet;

#[cfg(feature = "fault-injection")]
use trustgraph_engine::Fault;
use trustgraph_engine::{Engine, Event, ProposalOrigin, Reply};
use trustgraph_policy::{Query, Verdict, decide, parse};

const SPEC: &str = include_str!("../../trustgraph-policy/tests/fixtures/spec-example.policy.yaml");
const EXAMPLES: [&str; 5] = [
    include_str!("../../../examples/policies/customer-email.policy.yaml"),
    include_str!("../../../examples/policies/multi-agent-research.policy.yaml"),
    include_str!("../../../examples/policies/pull-request-bot.policy.yaml"),
    include_str!("../../../examples/policies/slack-digest.policy.yaml"),
    include_str!("../../../examples/policies/support-replies.policy.yaml"),
];

/// A valid lowercase-hex sha256 digest, distinct per `n`.
fn sha(n: u32) -> String {
    format!("{n:064x}")
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| n.to_string()).collect()
}

/// The spec example with `roadmap` readable by hr only, and its tests updated.
fn narrowed() -> String {
    SPEC.replace("readers: [hr, engineering]", "readers: [hr]")
        .replace(
            "action: post_to_slack\n    expect: allow",
            "action: post_to_slack\n    expect: deny",
        )
}

/// The spec example without the `roadmap` source (and without tests, which name it).
fn without_roadmap() -> String {
    let body = SPEC.split("\ntests:").next().unwrap();
    let removed = body.replace(
        "  roadmap:\n    readers: [hr, engineering]\n    agents: [planner]\n",
        "",
    );
    assert_ne!(removed, body, "fixture changed; update without_roadmap");
    removed
}

fn genesis(policy: &str) -> Engine {
    let mut engine = Engine::new();
    let reply = engine.apply(
        0,
        &Event::Genesis {
            policy: policy.into(),
        },
    );
    assert!(
        matches!(reply, Reply::Activated { revision: 1, .. }),
        "{reply:?}"
    );
    engine
}

fn artifact(reply: Reply) -> String {
    match reply {
        Reply::Artifact { id } => id,
        other => panic!("expected an artifact, got {other:?}"),
    }
}

fn register(engine: &mut Engine, at: u64, source: &str, n: u32) -> String {
    artifact(engine.apply(
        at,
        &Event::RegisterSource {
            schema: None,
            source: source.into(),
            sha256: sha(n),
        },
    ))
}

fn record(engine: &mut Engine, at: u64, agent: &str, inputs: &[&String], n: u32) -> String {
    artifact(engine.apply(
        at,
        &Event::RecordOutput {
            contract: None,
            agent: agent.into(),
            step: "summarize".into(),
            inputs: inputs.iter().map(|s| s.to_string()).collect(),
            sha256: sha(n),
        },
    ))
}

fn check_event(agent: &str, artifact: &str, action: &str) -> Event {
    Event::Check {
        agent: agent.into(),
        artifact: artifact.into(),
        action: action.into(),
        recipients: None,
    }
}

fn check(engine: &mut Engine, at: u64, agent: &str, artifact: &str, action: &str) -> Reply {
    engine.apply(at, &check_event(agent, artifact, action))
}

fn verdict(reply: &Reply) -> Verdict {
    match reply {
        Reply::Decision { verdict, .. } => *verdict,
        other => panic!("expected a decision, got {other:?}"),
    }
}

fn decision_id(reply: &Reply) -> String {
    match reply {
        Reply::Decision { id, .. } => id.clone(),
        other => panic!("expected a decision, got {other:?}"),
    }
}

fn rejected_code(reply: &Reply) -> &str {
    match reply {
        Reply::Rejected { code, .. } => code,
        other => panic!("expected a rejection, got {other:?}"),
    }
}

fn propose(engine: &mut Engine, at: u64, policy: &str) -> Reply {
    engine.apply(
        at,
        &Event::ProposePolicy {
            agent: None,
            policy: policy.into(),
        },
    )
}

fn propose_and_activate(engine: &mut Engine, at: u64, policy: &str) {
    let id = match propose(engine, at, policy) {
        Reply::Proposed { id, .. } => id,
        other => panic!("expected a proposal, got {other:?}"),
    };
    let reply = engine.apply(
        at,
        &Event::ActivatePolicy {
            proposal: id,
            operator: "leo".into(),
        },
    );
    assert!(matches!(reply, Reply::Activated { .. }), "{reply:?}");
}

/// A mailer output from price_sheet, checked once so the decision is remembered.
fn pending_email(engine: &mut Engine) -> (String, String) {
    let sheet = register(engine, 1, "price_sheet", 3);
    let out = record(engine, 2, "mailer", &[&sheet], 10);
    let reply = check(engine, 3, "mailer", &out, "email_customer");
    assert_eq!(verdict(&reply), Verdict::NeedsApproval, "{reply:?}");
    (out, decision_id(&reply))
}

fn approve(engine: &mut Engine, at: u64, decision: &str, reviewer: &str) -> Reply {
    engine.apply(
        at,
        &Event::Approve {
            decision: decision.into(),
            reviewer: reviewer.into(),
        },
    )
}

#[test]
fn end_to_end_matches_the_spec_example() {
    let mut engine = genesis(SPEC);
    let salaries = register(&mut engine, 1, "salaries", 1);
    let roadmap = register(&mut engine, 1, "roadmap", 2);
    assert_eq!(
        register(&mut engine, 1, "roadmap", 2),
        roadmap,
        "re-registering the same digest is idempotent"
    );

    let both = record(&mut engine, 2, "planner", &[&salaries, &roadmap], 10);
    assert_eq!(engine.lineage(&both), Some(set(&["roadmap", "salaries"])));
    match check(&mut engine, 3, "planner", &both, "post_to_slack") {
        Reply::Decision {
            verdict,
            code,
            blocking,
            expires_at,
            ..
        } => {
            assert_eq!(verdict, Verdict::Deny);
            assert_eq!(code, "audience_outside_readers");
            let sources: Vec<_> = blocking.iter().map(|b| b.source.as_str()).collect();
            assert_eq!(sources, ["salaries"]);
            assert_eq!(expires_at, None);
        }
        other => panic!("{other:?}"),
    }

    let only = record(&mut engine, 4, "planner", &[&roadmap], 11);
    match check(&mut engine, 5, "planner", &only, "post_to_slack") {
        Reply::Decision { verdict, code, .. } => {
            assert_eq!(verdict, Verdict::Allow);
            assert_eq!(code, "allowed");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn approval_flow() {
    let mut engine = genesis(SPEC);
    let (out, decision) = pending_email(&mut engine);
    assert_eq!(
        approve(&mut engine, 10, &decision, "leo"),
        Reply::Approved {
            decision: decision.clone(),
            expires_at: 10 + 1800,
        }
    );
    assert_eq!(
        rejected_code(&approve(&mut engine, 11, &decision, "leo")),
        "already_approved"
    );

    match check(&mut engine, 1809, "mailer", &out, "email_customer") {
        Reply::Decision {
            id,
            verdict,
            expires_at,
            ..
        } => {
            assert_eq!(id, decision);
            assert_eq!(verdict, Verdict::Allow);
            assert_eq!(expires_at, Some(1810));
        }
        other => panic!("{other:?}"),
    }
    let expired = check(&mut engine, 1810, "mailer", &out, "email_customer");
    assert_eq!(verdict(&expired), Verdict::NeedsApproval, "{expired:?}");

    // An expired approval may be renewed.
    assert!(matches!(
        approve(&mut engine, 1811, &decision, "leo"),
        Reply::Approved {
            expires_at: 3611,
            ..
        }
    ));
    assert_eq!(
        rejected_code(&approve(&mut engine, 1812, "nope", "leo")),
        "unknown_decision"
    );
}

#[test]
fn reviewer_rules() {
    let mut engine = genesis(SPEC);
    let (_, decision) = pending_email(&mut engine);
    for reviewer in ["nina", "mailer", "acme"] {
        assert_eq!(
            rejected_code(&approve(&mut engine, 10, &decision, reviewer)),
            "reviewer_not_allowed",
            "{reviewer}"
        );
    }
}

#[test]
fn revoke_voids() {
    let mut engine = genesis(SPEC);
    let (out, decision) = pending_email(&mut engine);
    approve(&mut engine, 10, &decision, "leo");
    assert_eq!(
        verdict(&check(&mut engine, 11, "mailer", &out, "email_customer")),
        Verdict::Allow
    );

    let revoke = |reviewer: &str| Event::Revoke {
        decision: decision.clone(),
        reviewer: reviewer.into(),
    };
    assert_eq!(
        rejected_code(&engine.apply(12, &revoke("nina"))),
        "reviewer_not_allowed"
    );
    assert_eq!(
        engine.apply(12, &revoke("leo")),
        Reply::Revoked {
            decision: decision.clone()
        }
    );
    assert_eq!(
        verdict(&check(&mut engine, 13, "mailer", &out, "email_customer")),
        Verdict::NeedsApproval
    );
}

#[test]
fn narrowing_applies_to_existing_outputs() {
    let mut engine = genesis(SPEC);
    let roadmap = register(&mut engine, 1, "roadmap", 2);
    let out = record(&mut engine, 2, "planner", &[&roadmap], 10);
    assert_eq!(
        verdict(&check(&mut engine, 3, "planner", &out, "post_to_slack")),
        Verdict::Allow
    );

    propose_and_activate(&mut engine, 4, &narrowed());
    assert_eq!(engine.policy_revision(), 2);
    match check(&mut engine, 5, "planner", &out, "post_to_slack") {
        Reply::Decision { verdict, code, .. } => {
            assert_eq!(verdict, Verdict::Deny);
            assert_eq!(code, "audience_outside_readers");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn removed_source_denies() {
    let mut engine = genesis(SPEC);
    let roadmap = register(&mut engine, 1, "roadmap", 2);
    let out = record(&mut engine, 2, "planner", &[&roadmap], 10);

    propose_and_activate(&mut engine, 3, &without_roadmap());
    assert_eq!(engine.lineage(&out), Some(set(&["roadmap"])));
    match check(&mut engine, 4, "planner", &out, "post_to_slack") {
        Reply::Decision { verdict, code, .. } => {
            assert_eq!(verdict, Verdict::Deny);
            assert_eq!(code, "unknown_source");
        }
        other => panic!("{other:?}"),
    }
    let again = engine.apply(
        5,
        &Event::RegisterSource {
            schema: None,
            source: "roadmap".into(),
            sha256: sha(2),
        },
    );
    assert_eq!(rejected_code(&again), "unknown_source");
    let derived = engine.apply(
        6,
        &Event::RecordOutput {
            contract: None,
            agent: "planner".into(),
            step: "again".into(),
            inputs: vec![roadmap],
            sha256: sha(20),
        },
    );
    assert_eq!(rejected_code(&derived), "agent_cannot_use_source");
}

#[test]
fn removed_then_readded_source() {
    let mut engine = genesis(SPEC);
    let roadmap = register(&mut engine, 1, "roadmap", 2);
    let out = record(&mut engine, 2, "planner", &[&roadmap], 10);

    propose_and_activate(&mut engine, 3, &without_roadmap());
    assert_eq!(
        verdict(&check(&mut engine, 4, "planner", &out, "post_to_slack")),
        Verdict::Deny
    );

    propose_and_activate(&mut engine, 5, &narrowed());
    match check(&mut engine, 6, "planner", &out, "post_to_slack") {
        Reply::Decision { verdict, code, .. } => {
            assert_eq!(verdict, Verdict::Deny);
            assert_eq!(code, "audience_outside_readers", "new readers apply");
        }
        other => panic!("{other:?}"),
    }

    propose_and_activate(&mut engine, 7, SPEC);
    assert_eq!(
        verdict(&check(&mut engine, 8, "planner", &out, "post_to_slack")),
        Verdict::Allow
    );
    assert_eq!(register(&mut engine, 9, "roadmap", 2), roadmap);
}

#[test]
fn policy_change_voids_approvals() {
    #[allow(unused_mut)]
    let mut cases = vec![("no fault", Engine::new(), false)];
    #[cfg(feature = "fault-injection")]
    cases.push((
        "KeepApprovalsAcrossPolicyChange",
        Engine::new().with_fault(Fault::KeepApprovalsAcrossPolicyChange),
        true,
    ));
    for (fault, mut engine, survives) in cases {
        engine.apply(
            0,
            &Event::Genesis {
                policy: SPEC.into(),
            },
        );
        let (out, decision) = pending_email(&mut engine);
        approve(&mut engine, 10, &decision, "leo");

        // Re-activating the identical policy still voids every approval.
        let changes = match propose(&mut engine, 11, SPEC) {
            Reply::Proposed { changes, .. } => changes,
            other => panic!("{other:?}"),
        };
        assert!(changes.is_empty());
        propose_and_activate(&mut engine, 11, SPEC);

        if !survives {
            assert_eq!(
                rejected_code(&engine.apply(
                    11,
                    &Event::Revoke {
                        decision: decision.clone(),
                        reviewer: "leo".into(),
                    }
                )),
                "unknown_decision"
            );
        }

        let after = check(&mut engine, 12, "mailer", &out, "email_customer");
        let expected = if survives {
            Verdict::Allow
        } else {
            Verdict::NeedsApproval
        };
        assert_eq!(verdict(&after), expected, "{fault}");
    }
}

#[test]
fn proposal_never_changes_active_policy() {
    let mut engine = genesis(SPEC);
    let digest = engine.policy().unwrap().digest();

    match propose(&mut engine, 1, &narrowed()) {
        Reply::Proposed {
            id,
            widens,
            changes,
            explanation,
        } => {
            assert_eq!(id, parse(&narrowed()).policy.unwrap().digest());
            assert!(!widens);
            assert!(!changes.is_empty());
            assert!(explanation.contains("roadmap"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(engine.policy_revision(), 1);
    assert_eq!(engine.policy().unwrap().digest(), digest);

    let failing_test = SPEC.replace("readers: [hr, engineering]", "readers: [hr]");
    for bad in ["version: 2\n", "not: [valid", failing_test.as_str()] {
        assert_eq!(
            rejected_code(&propose(&mut engine, 2, bad)),
            "invalid_policy"
        );
    }
    let huge = "#".repeat(trustgraph_policy::MAX_BYTES + 1);
    assert_eq!(
        rejected_code(&propose(&mut engine, 2, &huge)),
        "invalid_policy"
    );
    assert_eq!(engine.policy_revision(), 1);
    assert_eq!(engine.policy().unwrap().digest(), digest);

    let unknown = engine.apply(
        3,
        &Event::ActivatePolicy {
            proposal: sha(9),
            operator: "leo".into(),
        },
    );
    assert_eq!(rejected_code(&unknown), "unknown_proposal");
}

#[test]
fn only_genesis_before_policy() {
    let mut engine = Engine::new();
    let events = [
        Event::RegisterSource {
            schema: None,
            source: "salaries".into(),
            sha256: sha(1),
        },
        Event::RecordOutput {
            contract: None,
            agent: "planner".into(),
            step: "s".into(),
            inputs: vec![sha(1)],
            sha256: sha(2),
        },
        check_event("planner", &sha(1), "post_to_slack"),
        Event::Approve {
            decision: sha(1),
            reviewer: "leo".into(),
        },
        Event::Revoke {
            decision: sha(1),
            reviewer: "leo".into(),
        },
        Event::ProposePolicy {
            agent: None,
            policy: SPEC.into(),
        },
        Event::ActivatePolicy {
            proposal: sha(1),
            operator: "leo".into(),
        },
    ];
    for event in &events {
        assert_eq!(
            rejected_code(&engine.apply(1, event)),
            "no_policy",
            "{event:?}"
        );
    }
    assert!(engine.policy().is_none());
    assert_eq!(engine.policy_revision(), 0);
    assert!(matches!(
        engine.apply(
            2,
            &Event::Genesis {
                policy: SPEC.into()
            }
        ),
        Reply::Activated { revision: 1, .. }
    ));
}

#[test]
fn clock_regression_is_rejected_without_state_change() {
    let mut engine = genesis(SPEC);
    let roadmap = register(&mut engine, 100, "roadmap", 2);
    let before = engine.state_digest();
    let reply = engine.apply(
        99,
        &Event::RecordOutput {
            contract: None,
            agent: "planner".into(),
            step: "s".into(),
            inputs: vec![roadmap.clone()],
            sha256: sha(3),
        },
    );
    assert_eq!(rejected_code(&reply), "clock_regressed");
    assert_eq!(engine.state_digest(), before);
    // The same time is not a regression.
    record(&mut engine, 100, "planner", &[&roadmap], 3);
    assert_ne!(engine.state_digest(), before);
}

#[test]
fn agent_callable_classification() {
    let s = || String::from("x");
    let all = [
        Event::Genesis { policy: s() },
        Event::RegisterSource {
            schema: None,
            source: s(),
            sha256: s(),
        },
        Event::RecordOutput {
            contract: None,
            agent: s(),
            step: s(),
            inputs: vec![],
            sha256: s(),
        },
        check_event("x", "x", "x"),
        Event::Approve {
            decision: s(),
            reviewer: s(),
        },
        Event::Revoke {
            decision: s(),
            reviewer: s(),
        },
        Event::ProposePolicy {
            agent: None,
            policy: s(),
        },
        Event::ActivatePolicy {
            proposal: s(),
            operator: s(),
        },
        Event::ProposeFromFile { policy: s() },
    ];
    let callable: Vec<bool> = all.iter().map(Event::agent_callable).collect();
    assert_eq!(
        callable,
        [false, true, true, true, false, false, true, false, false]
    );
}

fn script() -> Vec<(u64, Event)> {
    let mut engine = genesis(SPEC);
    let salaries = register(&mut engine, 1, "salaries", 1);
    let roadmap = register(&mut engine, 1, "roadmap", 2);
    let sheet = register(&mut engine, 1, "price_sheet", 3);
    let both = record(&mut engine, 2, "planner", &[&salaries, &roadmap], 10);
    let email = record(&mut engine, 2, "mailer", &[&sheet], 11);
    let decision = decision_id(&check(&mut engine, 3, "mailer", &email, "email_customer"));
    vec![
        (
            0,
            Event::Genesis {
                policy: SPEC.into(),
            },
        ),
        (
            1,
            Event::RegisterSource {
                schema: None,
                source: "salaries".into(),
                sha256: sha(1),
            },
        ),
        (
            1,
            Event::RegisterSource {
                schema: None,
                source: "roadmap".into(),
                sha256: sha(2),
            },
        ),
        (
            1,
            Event::RegisterSource {
                schema: None,
                source: "price_sheet".into(),
                sha256: sha(3),
            },
        ),
        (
            2,
            Event::RecordOutput {
                contract: None,
                agent: "planner".into(),
                step: "summarize".into(),
                inputs: vec![salaries, roadmap.clone()],
                sha256: sha(10),
            },
        ),
        (
            2,
            Event::RecordOutput {
                contract: None,
                agent: "mailer".into(),
                step: "summarize".into(),
                inputs: vec![sheet],
                sha256: sha(11),
            },
        ),
        (3, check_event("planner", &both, "post_to_slack")),
        (3, check_event("mailer", &email, "email_customer")),
        (
            4,
            Event::Approve {
                decision: decision.clone(),
                reviewer: "leo".into(),
            },
        ),
        (5, check_event("mailer", &email, "email_customer")),
        (
            6,
            Event::ProposePolicy {
                agent: Some("planner".into()),
                policy: narrowed(),
            },
        ),
        (
            7,
            Event::ActivatePolicy {
                proposal: parse(&narrowed()).policy.unwrap().digest(),
                operator: "leo".into(),
            },
        ),
        (8, check_event("mailer", &email, "email_customer")),
        (
            8,
            Event::RecordOutput {
                contract: None,
                agent: "planner".into(),
                step: "again".into(),
                inputs: vec![roadmap],
                sha256: sha(12),
            },
        ),
        (
            9,
            Event::Revoke {
                decision,
                reviewer: "leo".into(),
            },
        ),
        (2, check_event("planner", &both, "post_to_slack")),
    ]
}

#[test]
fn replay_is_deterministic() {
    let events = script();
    let run = || {
        let mut engine = Engine::new();
        let replies: Vec<Reply> = events.iter().map(|(at, e)| engine.apply(*at, e)).collect();
        (replies, engine.state_digest())
    };
    let (first, digest) = run();
    let (second, digest2) = run();
    assert_eq!(first, second);
    assert_eq!(digest, digest2);
    assert_eq!(digest.len(), 64);
    // The script exercises real work, not only rejections.
    assert!(matches!(first[8], Reply::Approved { .. }), "{:?}", first[8]);
    assert!(matches!(first[11], Reply::Activated { revision: 2, .. }));
    assert_eq!(rejected_code(&first[15]), "clock_regressed");
}

#[test]
fn garbage_never_panics() {
    let long = "x".repeat(10_000);
    // 128 two-byte characters: passes the 256-byte field limit, so it is echoed.
    let wide = "ж".repeat(128);
    for g in ["", long.as_str(), "€", wide.as_str()] {
        let mut engine = genesis(SPEC);
        register(&mut engine, 1, "roadmap", 2);
        let s = || g.to_string();
        let events = [
            Event::Genesis { policy: s() },
            Event::RegisterSource {
                schema: None,
                source: s(),
                sha256: s(),
            },
            Event::RecordOutput {
                contract: None,
                agent: s(),
                step: s(),
                inputs: vec![s()],
                sha256: s(),
            },
            Event::Check {
                agent: s(),
                artifact: s(),
                action: s(),
                recipients: Some(BTreeSet::from([s()])),
            },
            Event::Approve {
                decision: s(),
                reviewer: s(),
            },
            Event::Revoke {
                decision: s(),
                reviewer: s(),
            },
            Event::ProposePolicy {
                agent: Some(s()),
                policy: s(),
            },
            Event::ProposePolicy {
                agent: None,
                policy: s(),
            },
            Event::ActivatePolicy {
                proposal: s(),
                operator: s(),
            },
        ];
        for event in &events {
            match engine.apply(2, event) {
                Reply::Rejected { message, .. } => {
                    assert!(message.len() <= 200, "{message}");
                    assert!(!message.contains(&"x".repeat(65)), "{message}");
                    assert!(message.matches('ж').count() <= 64, "{message}");
                }
                other => panic!("{g:?} {event:?} gave {other:?}"),
            }
        }
        assert_eq!(engine.policy_revision(), 1);
    }
}

#[test]
fn record_output_checks_in_order() {
    let mut engine = genesis(SPEC);
    let salaries = register(&mut engine, 1, "salaries", 1);
    let rec = |agent: &str, inputs: Vec<String>, sha256: String| Event::RecordOutput {
        contract: None,
        agent: agent.into(),
        step: "s".into(),
        inputs,
        sha256,
    };
    let cases = [
        (
            rec("nobody", vec![salaries.clone()], "ABC".into()),
            "invalid_digest",
        ),
        (
            rec("nobody", vec![salaries.clone()], sha(5)),
            "unknown_agent",
        ),
        (rec("planner", vec![], sha(5)), "invalid_inputs"),
        (
            rec("planner", vec![salaries.clone(); 65], sha(5)),
            "invalid_inputs",
        ),
        (rec("planner", vec![sha(77)], sha(5)), "unknown_artifact"),
        (
            rec("mailer", vec![salaries.clone()], sha(5)),
            "agent_cannot_use_source",
        ),
    ];
    for (event, code) in &cases {
        assert_eq!(rejected_code(&engine.apply(2, event)), *code, "{event:?}");
    }
    let first = record(&mut engine, 3, "planner", &[&salaries], 5);
    assert_eq!(record(&mut engine, 4, "planner", &[&salaries], 5), first);
    assert_ne!(record(&mut engine, 4, "planner", &[&salaries], 6), first);

    // A new version of a source is a new artifact; both stay usable.
    let v2 = register(&mut engine, 5, "salaries", 9);
    assert_ne!(v2, salaries);
    record(&mut engine, 6, "planner", &[&salaries, &v2], 7);
}

#[test]
fn engine_matches_decide_on_every_example() {
    let mut seen = BTreeSet::new();
    for text in EXAMPLES {
        let policy = parse(text).policy.expect("example parses");
        let mut engine = genesis(text);
        let mut at = 1;
        let mut sources = Vec::new();
        for (i, name) in policy.sources.keys().enumerate() {
            sources.push((name.clone(), register(&mut engine, at, name, i as u32)));
        }
        let mut artifacts: Vec<String> = sources.iter().map(|(_, id)| id.clone()).collect();
        let mut n = 1000;
        for agent in &policy.agents {
            let usable: Vec<&String> = sources
                .iter()
                .filter(|(name, _)| policy.sources[name].agents.contains(agent))
                .map(|(_, id)| id)
                .collect();
            let mut subsets: Vec<Vec<&String>> = usable.iter().map(|a| vec![*a]).collect();
            for (i, a) in usable.iter().enumerate() {
                for b in &usable[i + 1..] {
                    subsets.push(vec![*a, *b]);
                }
            }
            for inputs in subsets {
                at += 1;
                n += 1;
                artifacts.push(record(&mut engine, at, agent, &inputs, n));
            }
        }
        let mut compared = 0;
        for agent in &policy.agents {
            for artifact in &artifacts {
                let output_from = engine.lineage(artifact).expect("known artifact");
                for (action_name, action) in &policy.actions {
                    let mut options: Vec<Option<BTreeSet<String>>> = vec![None];
                    options.extend(action.audience.iter().map(|n| Some(set(&[n]))));
                    for recipients in options {
                        at += 1;
                        let reply = engine.apply(
                            at,
                            &Event::Check {
                                agent: agent.clone(),
                                artifact: artifact.clone(),
                                action: action_name.clone(),
                                recipients: recipients.clone(),
                            },
                        );
                        let expected = decide(
                            &policy,
                            &Query {
                                output_from: output_from.clone(),
                                agent: agent.clone(),
                                action: action_name.clone(),
                                recipients,
                            },
                        );
                        match reply {
                            Reply::Decision {
                                verdict,
                                code,
                                reason,
                                blocking,
                                ..
                            } => {
                                assert_eq!(verdict, expected.verdict, "{agent} {action_name}");
                                assert_eq!(code, expected.code, "{agent} {action_name}");
                                assert_eq!(reason, expected.reason);
                                assert_eq!(blocking, expected.blocking);
                                seen.insert(verdict);
                            }
                            other => panic!("{other:?}"),
                        }
                        compared += 1;
                    }
                }
            }
        }
        assert!(compared > 0);
    }
    assert_eq!(
        seen,
        BTreeSet::from([Verdict::Allow, Verdict::NeedsApproval, Verdict::Deny])
    );
}

/// The simulator's leak fault: stale Graph readers and no `decide` gate turn a
/// narrowing into a real `Allow`. Without the fault the same check is denied.
#[test]
fn graph_only_stale_readers_leaks_after_narrowing() {
    #[allow(unused_mut)]
    let mut cases = vec![("no fault", Engine::new(), Verdict::Deny)];
    #[cfg(feature = "fault-injection")]
    cases.push((
        "GraphOnlyStaleReaders",
        Engine::new().with_fault(Fault::GraphOnlyStaleReaders),
        Verdict::Allow,
    ));
    for (fault, mut engine, expected) in cases {
        engine.apply(
            0,
            &Event::Genesis {
                policy: SPEC.into(),
            },
        );
        let roadmap = register(&mut engine, 1, "roadmap", 2);
        let out = record(&mut engine, 2, "planner", &[&roadmap], 10);
        assert_eq!(
            verdict(&check(&mut engine, 3, "planner", &out, "post_to_slack")),
            Verdict::Allow
        );
        propose_and_activate(&mut engine, 4, &narrowed());
        assert_eq!(
            verdict(&check(&mut engine, 5, "planner", &out, "post_to_slack")),
            expected,
            "{fault}"
        );
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn skipped_reader_refresh_shows_as_mismatch_never_allow() {
    let mut engine = Engine::new().with_fault(Fault::SkipReaderRefresh);
    engine.apply(0, &Event::Genesis { policy: narrowed() });
    let roadmap = register(&mut engine, 1, "roadmap", 2);
    let out = record(&mut engine, 2, "planner", &[&roadmap], 10);
    assert_eq!(
        verdict(&check(&mut engine, 3, "planner", &out, "post_to_slack")),
        Verdict::Deny
    );
    // The policy widens roadmap, but the Graph still holds the old readers.
    propose_and_activate(&mut engine, 4, SPEC);
    match check(&mut engine, 5, "planner", &out, "post_to_slack") {
        Reply::Decision { verdict, code, .. } => {
            assert_eq!(verdict, Verdict::Deny);
            assert_eq!(code, "engine_mismatch");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn engine_is_default_and_events_round_trip() {
    let engine = Engine::default();
    assert_eq!(engine.policy_revision(), 0);
    let event = check_event("planner", &sha(1), "post_to_slack");
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"check\""), "{json}");
    assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
    let extra = json.replace("{", "{\"bogus\":1,");
    assert!(serde_json::from_str::<Event>(&extra).is_err());
}

#[test]
fn rejection_messages_echo_at_most_64_characters_in_total() {
    let agent = "q".repeat(60);
    let source = "z".repeat(60);
    let policy = format!(
        "version: 1\ngroups:\n  g: [p]\nagents: [{agent}, b]\nsources:\n  {source}:\n    readers: [g]\n    agents: [b]\nactions:\n  act:\n    agents: [b]\n    audience: [g]\n"
    );
    let mut engine = genesis(&policy);
    let input = register(&mut engine, 1, &source, 1);
    let reply = engine.apply(
        2,
        &Event::RecordOutput {
            contract: None,
            agent: agent.clone(),
            step: "s".into(),
            inputs: vec![input],
            sha256: sha(2),
        },
    );
    let Reply::Rejected { code, message } = reply else {
        panic!("{reply:?}")
    };
    assert_eq!(code, "agent_cannot_use_source");
    let echoed = message.matches(['q', 'z']).count();
    assert!(echoed <= 64, "{echoed} echoed characters in {message}");
}

#[test]
fn remembered_decisions_are_capped_per_agent() {
    let mut engine = genesis(SPEC);
    let sheet = register(&mut engine, 1, "price_sheet", 3);
    let mut outputs = Vec::new();
    let mut ids = Vec::new();
    for i in 0..100u32 {
        let out = record(&mut engine, 3, "mailer", &[&sheet], 100 + i);
        let reply = check(&mut engine, 3, "mailer", &out, "email_customer");
        assert_eq!(verdict(&reply), Verdict::NeedsApproval);
        ids.push(decision_id(&reply));
        outputs.push(out);
        if i == 0 {
            // A live approval protects the oldest decision from eviction.
            assert!(matches!(
                approve(&mut engine, 3, &ids[0], "leo"),
                Reply::Approved { .. }
            ));
        }
    }
    assert_eq!(
        verdict(&check(
            &mut engine,
            4,
            "mailer",
            &outputs[0],
            "email_customer"
        )),
        Verdict::Allow
    );
    let mut unknown = 0;
    let mut approved = 0;
    for id in &ids[1..] {
        match approve(&mut engine, 5, id, "leo") {
            Reply::Approved { .. } => approved += 1,
            Reply::Rejected { code, .. } if code == "unknown_decision" => unknown += 1,
            other => panic!("{other:?}"),
        }
    }
    // 64 remembered for the mailer: the approved oldest plus the 63 newest.
    assert_eq!(approved, 63);
    assert_eq!(unknown, 36);
    for id in &ids[1..37] {
        assert_eq!(
            rejected_code(&approve(&mut engine, 6, id, "leo")),
            "unknown_decision"
        );
    }
    // A re-check re-creates an evicted decision with the same id.
    let again = check(&mut engine, 7, "mailer", &outputs[1], "email_customer");
    assert_eq!(decision_id(&again), ids[1]);
    assert_eq!(verdict(&again), Verdict::NeedsApproval);
}

#[test]
fn one_agent_flooding_checks_cannot_evict_another_agents_decision() {
    let mut engine = genesis(&scope_policy());
    let sheet = register(&mut engine, 1, "price_sheet", 3);
    let theirs = record(&mut engine, 2, "mailer2", &[&sheet], 7);
    let decision = decision_id(&check(&mut engine, 2, "mailer2", &theirs, "email_customer"));
    for i in 0..300u32 {
        let out = record(&mut engine, 3, "mailer", &[&sheet], 100 + i);
        check(&mut engine, 3, "mailer", &out, "email_customer");
    }
    let pending = engine.pending_decisions(4);
    assert_eq!(pending.iter().filter(|p| p.agent == "mailer").count(), 64);
    assert!(pending.iter().any(|p| p.decision == decision));
    assert!(matches!(
        approve(&mut engine, 4, &decision, "leo"),
        Reply::Approved { .. }
    ));
}

#[test]
fn remembered_decisions_are_capped_in_all() {
    let names: Vec<String> = (0..20).map(|i| format!("m{i}")).collect();
    let list = names.join(", ");
    let policy = SPEC
        .replace(
            "agents: [planner, mailer]",
            &format!("agents: [planner, {list}]"),
        )
        .replace("agents: [mailer]", &format!("agents: [{list}]"));
    let mut engine = genesis(&policy);
    let sheet = register(&mut engine, 1, "price_sheet", 3);
    let mut n = 100;
    for agent in &names {
        for _ in 0..60 {
            n += 1;
            let out = record(&mut engine, 2, agent, &[&sheet], n);
            assert_eq!(
                verdict(&check(&mut engine, 2, agent, &out, "email_customer")),
                Verdict::NeedsApproval
            );
        }
    }
    let pending = engine.pending_decisions(3);
    assert_eq!(pending.len(), 1024);
    // Agents that asked after the cap was reached kept their own newest.
    for agent in &names {
        let own = pending.iter().filter(|p| &p.agent == agent).count();
        assert!(own > 0 && own <= 60, "{agent}: {own}");
    }
}

#[test]
fn agent_proposals_cannot_evict_operator_proposals() {
    let operator_id = match propose(&mut genesis(SPEC), 1, &narrowed()) {
        Reply::Proposed { id, .. } => id,
        other => panic!("{other:?}"),
    };
    let mut engine = genesis(SPEC);
    assert!(matches!(
        propose(&mut engine, 1, &narrowed()),
        Reply::Proposed { .. }
    ));
    let mut agent_ids = Vec::new();
    for i in 0..100 {
        let text = SPEC.replace("valid_for: 30m", &format!("valid_for: {}s", 100 + i));
        match engine.apply(
            2,
            &Event::ProposePolicy {
                agent: Some("planner".into()),
                policy: text,
            },
        ) {
            Reply::Proposed { id, .. } => agent_ids.push(id),
            other => panic!("{other:?}"),
        }
    }
    let activate = |engine: &mut Engine, id: &str| {
        engine.apply(
            3,
            &Event::ActivatePolicy {
                proposal: id.into(),
                operator: "leo".into(),
            },
        )
    };
    // The planner keeps its own 8 newest.
    assert_eq!(
        rejected_code(&activate(&mut engine, &agent_ids[91])),
        "unknown_proposal"
    );
    assert!(matches!(
        activate(&mut engine, &agent_ids[92]),
        Reply::Activated { .. }
    ));
    assert!(matches!(
        activate(&mut engine, &operator_id),
        Reply::Activated { .. }
    ));
}

fn agent_propose(engine: &mut Engine, at: u64, agent: &str, policy: &str) -> String {
    match engine.apply(
        at,
        &Event::ProposePolicy {
            agent: Some(agent.into()),
            policy: policy.into(),
        },
    ) {
        Reply::Proposed { id, .. } => id,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_proposal_flood_cannot_evict_other_agents_the_operator_or_the_file() {
    let mut engine = genesis(&scope_policy());
    let variant =
        |n: u32| scope_policy().replace("valid_for: 30m", &format!("valid_for: {}s", 100 + n));
    let theirs = agent_propose(&mut engine, 1, "mailer2", &variant(1));
    let operator = match propose(&mut engine, 1, &variant(2)) {
        Reply::Proposed { id, .. } => id,
        other => panic!("{other:?}"),
    };
    let file = match engine.apply(1, &Event::ProposeFromFile { policy: variant(3) }) {
        Reply::Proposed { id, .. } => id,
        other => panic!("{other:?}"),
    };
    let before = engine.stored_proposals();
    // Re-proposing someone else's digest changes neither origin nor order.
    for id_text in [variant(1), variant(2), variant(3)] {
        agent_propose(&mut engine, 2, "mailer", &id_text);
    }
    assert_eq!(engine.stored_proposals(), before);
    for i in 0..200 {
        agent_propose(&mut engine, 3, "mailer", &variant(10 + i));
    }
    for id in [&theirs, &operator, &file] {
        assert!(engine.has_proposal(id), "{id}");
    }
    let stored = engine.stored_proposals();
    let origin = |id: &str| {
        stored
            .iter()
            .find(|p| p.proposal == id)
            .unwrap()
            .origin
            .clone()
    };
    assert_eq!(origin(&theirs), ProposalOrigin::Agent("mailer2".into()));
    assert_eq!(origin(&operator), ProposalOrigin::Operator);
    assert_eq!(origin(&file), ProposalOrigin::File);
    let mailer = stored
        .iter()
        .filter(|p| p.origin == ProposalOrigin::Agent("mailer".into()))
        .count();
    assert_eq!(mailer, 8);
}

#[test]
fn same_digest_from_both_classes_is_stored_once_as_operator() {
    let mut engine = genesis(SPEC);
    let shared = match engine.apply(
        1,
        &Event::ProposePolicy {
            agent: Some("planner".into()),
            policy: narrowed(),
        },
    ) {
        Reply::Proposed { id, .. } => id,
        other => panic!("{other:?}"),
    };
    propose(&mut engine, 1, &narrowed());
    for i in 0..40 {
        let text = SPEC.replace("valid_for: 30m", &format!("valid_for: {}s", 100 + i));
        engine.apply(
            2,
            &Event::ProposePolicy {
                agent: Some("planner".into()),
                policy: text,
            },
        );
    }
    let reply = engine.apply(
        3,
        &Event::ActivatePolicy {
            proposal: shared,
            operator: "leo".into(),
        },
    );
    assert!(matches!(reply, Reply::Activated { .. }), "{reply:?}");
}

#[test]
fn too_deep_lineage_is_lineage_limit() {
    let mut engine = genesis(SPEC);
    let mut last = register(&mut engine, 1, "roadmap", 2);
    let mut depth = 0;
    let code = loop {
        match engine.apply(
            2,
            &Event::RecordOutput {
                contract: None,
                agent: "planner".into(),
                step: "s".into(),
                inputs: vec![last.clone()],
                sha256: sha(1000 + depth),
            },
        ) {
            Reply::Artifact { id } => last = id,
            Reply::Rejected { code, .. } => break code,
            other => panic!("{other:?}"),
        }
        depth += 1;
        assert!(depth <= 100, "no depth limit");
    };
    assert_eq!(code, "lineage_limit");
    assert_eq!(depth, 64);
    // The deepest accepted output is still checkable.
    assert_eq!(
        verdict(&check(&mut engine, 3, "planner", &last, "post_to_slack")),
        Verdict::Allow
    );
}

/// The spec example with a second customer and a second mailer agent.
fn scope_policy() -> String {
    SPEC.replace("customers: [acme]", "customers: [acme, globex]")
        .replace(
            "agents: [planner, mailer]",
            "agents: [planner, mailer, mailer2]",
        )
        .replace("agents: [mailer]", "agents: [mailer, mailer2]")
}

/// Genesis on `policy`, one approved mailer output; returns (engine, sheet, output, decision).
fn approved_email(policy: &str) -> (Engine, String, String, String) {
    let mut engine = genesis(policy);
    let sheet = register(&mut engine, 1, "price_sheet", 3);
    let out = record(&mut engine, 2, "mailer", &[&sheet], 10);
    let decision = decision_id(&check(&mut engine, 3, "mailer", &out, "email_customer"));
    assert!(matches!(
        approve(&mut engine, 4, &decision, "leo"),
        Reply::Approved { .. }
    ));
    assert_eq!(
        verdict(&check(&mut engine, 5, "mailer", &out, "email_customer")),
        Verdict::Allow
    );
    (engine, sheet, out, decision)
}

#[test]
fn approval_does_not_cover_a_subset_of_recipients() {
    let (mut engine, _, out, decision) = approved_email(&scope_policy());
    let reply = engine.apply(
        6,
        &Event::Check {
            agent: "mailer".into(),
            artifact: out,
            action: "email_customer".into(),
            recipients: Some(set(&["acme"])),
        },
    );
    assert_eq!(verdict(&reply), Verdict::NeedsApproval);
    assert_ne!(decision_id(&reply), decision);
}

#[test]
fn approval_does_not_cover_another_output_from_the_same_source() {
    let (mut engine, sheet, _, decision) = approved_email(&scope_policy());
    let other = record(&mut engine, 6, "mailer", &[&sheet], 11);
    let reply = check(&mut engine, 7, "mailer", &other, "email_customer");
    assert_eq!(verdict(&reply), Verdict::NeedsApproval);
    assert_ne!(decision_id(&reply), decision);
}

#[test]
fn approval_does_not_cover_another_agent() {
    let (mut engine, _, out, decision) = approved_email(&scope_policy());
    let reply = check(&mut engine, 6, "mailer2", &out, "email_customer");
    assert_eq!(verdict(&reply), Verdict::NeedsApproval);
    assert_ne!(decision_id(&reply), decision);
}

#[test]
fn changing_only_the_reviewers_voids_the_approval() {
    let (mut engine, _, out, _) = approved_email(&scope_policy());
    let changed = scope_policy().replace("reviewers: [sales]", "reviewers: [sales, acme]");
    assert_ne!(changed, scope_policy());
    propose_and_activate(&mut engine, 6, &changed);
    assert_eq!(
        verdict(&check(&mut engine, 7, "mailer", &out, "email_customer")),
        Verdict::NeedsApproval
    );
}

#[test]
fn revoke_without_approval_is_no_approval() {
    let mut engine = genesis(SPEC);
    let (_, decision) = pending_email(&mut engine);
    let reply = engine.apply(
        5,
        &Event::Revoke {
            decision,
            reviewer: "leo".into(),
        },
    );
    assert_eq!(rejected_code(&reply), "no_approval");
}

/// Direct engine use matches the runtime: a shape rejection moves nothing,
/// not even the clock, so replaying a journal without it gives the same state.
#[test]
fn shape_rejection_leaves_the_clock_alone() {
    let mut engine = genesis(SPEC);
    let digest = engine.state_digest();
    let reply = engine.apply(
        50,
        &Event::RecordOutput {
            contract: None,
            agent: "planner".into(),
            step: "summarize".into(),
            inputs: Vec::new(),
            sha256: sha(1),
        },
    );
    assert!(matches!(&reply, Reply::Rejected { code, .. } if code == "invalid_inputs"));
    assert_eq!(engine.state_digest(), digest);
    for event in [
        Event::Approve {
            decision: String::new(),
            reviewer: "leo".into(),
        },
        Event::ActivatePolicy {
            proposal: "p".into(),
            operator: "o".repeat(257),
        },
        Event::ProposePolicy {
            agent: Some("  ".into()),
            policy: SPEC.into(),
        },
    ] {
        assert!(event.validate_shape().is_err(), "{event:?}");
        assert!(matches!(engine.apply(60, &event), Reply::Rejected { .. }));
        assert_eq!(engine.state_digest(), digest, "{event:?}");
    }
    assert!(
        Event::Check {
            agent: "planner".into(),
            artifact: "x".into(),
            action: "post_to_slack".into(),
            recipients: Some(set(&["nina"])),
        }
        .validate_shape()
        .is_ok()
    );
}

/// The engine's own tests build with the feature (self dev-dependency).
#[test]
fn fault_injection_constant_is_on_in_engine_tests() {
    const { assert!(trustgraph_engine::FAULT_INJECTION) };
}

#[test]
fn operator_views_list_pending_approved_and_proposals() {
    let mut engine = genesis(SPEC);
    assert!(engine.policy_from_genesis());
    let (out, decision) = pending_email(&mut engine);
    assert_eq!(engine.artifact_count(), 2);
    let digest = engine.state_digest();

    let pending = engine.pending_decisions(3);
    assert_eq!(pending.len(), 1);
    let p = &pending[0];
    assert_eq!(
        (p.decision.as_str(), p.agent.as_str(), p.action.as_str()),
        (decision.as_str(), "mailer", "email_customer")
    );
    assert_eq!(p.artifact, out);
    assert_eq!(p.recipients, ["acme"]);
    assert_eq!(p.sources, ["price_sheet"]);
    assert_eq!(p.requested_at, 3);
    assert!(!p.reason.is_empty());
    assert!(engine.live_approvals(3).is_empty());
    assert_eq!(engine.state_digest(), digest, "views change nothing");

    approve(&mut engine, 10, &decision, "leo");
    assert!(engine.pending_decisions(10).is_empty());
    let live = engine.live_approvals(10);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].decision, decision);
    assert_eq!(live[0].reviewer, "leo");
    assert_eq!(live[0].expires_at, 1810);
    assert_eq!(live[0].recipients, ["acme"]);
    // Expired: pending again, not live.
    assert!(engine.live_approvals(1810).is_empty());
    assert_eq!(engine.pending_decisions(1810).len(), 1);

    let agent_proposal = engine.apply(
        11,
        &Event::ProposePolicy {
            agent: Some("planner".into()),
            policy: narrowed(),
        },
    );
    assert!(matches!(agent_proposal, Reply::Proposed { .. }));
    // The first `readers: [hr] ` is the salaries source.
    let widened = SPEC.replacen("readers: [hr] ", "readers: [hr, sales] ", 1);
    assert!(matches!(
        propose(&mut engine, 12, &widened),
        Reply::Proposed { widens: true, .. }
    ));
    let stored = engine.stored_proposals();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].origin, ProposalOrigin::Agent("planner".into()));
    assert!(!stored[0].widens);
    assert_eq!(stored[1].origin, ProposalOrigin::Operator);
    assert!(stored[1].widens);
    assert!(!stored[1].explanation.is_empty());

    propose_and_activate(&mut engine, 13, &narrowed());
    assert!(!engine.policy_from_genesis());
    assert!(engine.pending_decisions(13).is_empty());
    assert!(engine.live_approvals(13).is_empty());
}

fn carry(engine: &mut Engine, at: u64, session: &str, agent: &str, artifacts: &[&String]) -> Reply {
    engine.apply(
        at,
        &Event::SessionCarry {
            session: session.into(),
            agent: agent.into(),
            artifacts: artifacts.iter().map(|a| a.to_string()).collect(),
        },
    )
}

#[test]
fn gateway_sessions_carry_what_the_agent_may_use_and_change_no_decision() {
    let mut engine = genesis(SPEC);
    let salaries = register(&mut engine, 1, "salaries", 1);
    let roadmap = register(&mut engine, 1, "roadmap", 2);
    let prices = register(&mut engine, 1, "price_sheet", 3);

    let reply = carry(&mut engine, 2, "s1", "planner", &[&salaries]);
    assert_eq!(
        reply,
        Reply::Session {
            session: "s1".into(),
            artifacts: vec![salaries.clone()],
        }
    );
    let Reply::Session { artifacts, .. } = carry(&mut engine, 3, "s1", "planner", &[&roadmap])
    else {
        panic!("expected a session");
    };
    assert_eq!(artifacts.len(), 2);
    let session = engine.session("s1").unwrap();
    assert_eq!(session.agent, "planner");
    assert_eq!(session.started_at, 2);
    assert_eq!(session.sources, vec!["roadmap", "salaries"]);

    // Every refusal leaves the sessions as they were.
    let before = engine.sessions();
    let mailer_reads_salaries = carry(&mut engine, 4, "s2", "mailer", &[&salaries]);
    assert_eq!(
        rejected_code(&mailer_reads_salaries),
        "agent_cannot_use_source"
    );
    let other_agent = carry(&mut engine, 4, "s1", "mailer", &[&prices]);
    assert_eq!(rejected_code(&other_agent), "session_agent_mismatch");
    let unknown = carry(&mut engine, 4, "s1", "planner", &[&"nope".to_string()]);
    assert_eq!(rejected_code(&unknown), "unknown_artifact");
    let stranger = carry(&mut engine, 4, "s3", "stranger", &[&roadmap]);
    assert_eq!(rejected_code(&stranger), "unknown_agent");
    let empty = carry(&mut engine, 4, "s1", "planner", &[]);
    assert_eq!(rejected_code(&empty), "invalid_inputs");
    assert_eq!(engine.sessions(), before);

    // A session changes no decision: the check is about the output's lineage.
    let both = record(&mut engine, 5, "planner", &[&salaries, &roadmap], 9);
    let digest = engine.state_digest();
    let ended = engine.apply(
        6,
        &Event::EndSession {
            session: "s1".into(),
        },
    );
    assert_eq!(
        ended,
        Reply::SessionEnded {
            session: "s1".into()
        }
    );
    assert_ne!(engine.state_digest(), digest, "sessions are state");
    assert!(engine.session("s1").is_none());
    let again = engine.apply(
        6,
        &Event::EndSession {
            session: "s1".into(),
        },
    );
    assert_eq!(rejected_code(&again), "unknown_session");
    let decision = engine.apply(7, &check_event("planner", &both, "post_to_slack"));
    assert!(
        matches!(
            decision,
            Reply::Decision {
                verdict: Verdict::Deny,
                ..
            }
        ),
        "{decision:?}"
    );
}

#[test]
fn gateway_sessions_are_capped() {
    let mut engine = genesis(SPEC);
    let roadmap = register(&mut engine, 1, "roadmap", 2);
    for n in 0..64 {
        let reply = carry(&mut engine, 2, &format!("s{n}"), "planner", &[&roadmap]);
        assert!(matches!(reply, Reply::Session { .. }), "{reply:?}");
    }
    let over = carry(&mut engine, 2, "s64", "planner", &[&roadmap]);
    assert_eq!(rejected_code(&over), "too_many_sessions");
    // An existing session still takes more.
    let salaries = register(&mut engine, 3, "salaries", 1);
    let more = carry(&mut engine, 3, "s0", "planner", &[&salaries]);
    assert!(matches!(more, Reply::Session { .. }), "{more:?}");
}
