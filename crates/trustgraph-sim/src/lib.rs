//! Seeded virtual-time scenarios over the actual governance coordinator.
//! This scheduler models command order and declared I/O failures; it does not
//! intercept OS threads, HTTP packets, or database internals.
pub mod checker;
pub mod policy_sim;
pub mod trace;
use std::collections::BTreeMap;
pub use trace::*;
use trustgraph_core::{bundle, digest::Digest, *};
use trustgraph_runtime::{
    journal::{MemoryJournal, WriteFault},
    publisher::{LocalPublisher, PublisherFault},
    *,
};

type World = Coordinator<MemoryJournal, LocalPublisher>;

/// Scheduler version 1: ascending virtual timestamp, seeded LCG rank, then
/// insertion number. The saved trace records the actual resolved action order.
pub fn run(seed: u64, scenario: Scenario) -> Result<Trace, String> {
    Ok(run_with_bundles(seed, scenario)?.0)
}

pub fn run_with_bundles(
    seed: u64,
    scenario: Scenario,
) -> Result<(Trace, Vec<bundle::Bundle>), String> {
    let mut random = seed;
    let mut schedule: Vec<_> = actions(scenario)
        .into_iter()
        .enumerate()
        .map(|(ticket, (at, action))| {
            random = random
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (at, random, ticket, action)
        })
        .collect();
    schedule.sort_by_key(|(at, rank, ticket, _)| (*at, *rank, *ticket));
    let mut world = Some(
        World::open(MemoryJournal::default(), LocalPublisher::default())
            .map_err(|e| e.to_string())?,
    );
    let mut contexts = BTreeMap::<ArtifactId, Digest>::new();
    let mut trace = Trace {
        schema: 1,
        scheduler: "trustgraph-scheduler-1".into(),
        seed,
        scenario,
        steps: vec![],
        receipts: vec![],
        publications: 0,
    };
    for (at, _, _, action) in schedule {
        let outcome = step(&mut world, &mut contexts, &action, at);
        let outcome = match outcome {
            Ok(value) => value,
            Err(error)
                if scenario == Scenario::JournalRecovery
                    && matches!(action, Action::Publish { .. }) =>
            {
                serde_json::json!({"storage_fault": error.to_string()})
            }
            Err(error) => return Err(format!("{scenario:?} at {at}: {error}")),
        };
        let w = world.as_ref().expect("world restored");
        // Every accepted provider payload must have the exact journaled evidence
        // that authorized it. Counts alone would miss payload substitution.
        for (key, ack) in w.publisher.accepted() {
            let op = w
                .state()
                .operations
                .get(key)
                .ok_or("publisher effect has no durable operation")?;
            bundle::replay(&op.bundle).map_err(str::to_string)?;
            if op.bundle.decision.context() != Some(&op.context) || op.intent != op.bundle.intent {
                return Err("operation binding differs from its verified bundle".into());
            }
            if op.context.artifact_hash != ack.payload.hash
                || op.context.audience.destination != ack.payload.destination
                || op.bundle.evidence.artifacts[&op.intent.artifact].content != ack.payload.content
            {
                return Err("provider payload differs from approved context".into());
            }
        }
        trace.steps.push(Step {
            at,
            action,
            outcome,
            accepted: w.publisher.accepted().len(),
        });
    }
    let w = world.unwrap();
    trace.publications = w.publisher.accepted().len();
    trace.receipts = w
        .state()
        .operations
        .values()
        .filter_map(|op| op.receipt.clone())
        .collect();
    let bundles = w
        .state()
        .operations
        .values()
        .map(|op| op.bundle.clone())
        .collect();
    Ok((trace, bundles))
}

fn step(
    world: &mut Option<World>,
    contexts: &mut BTreeMap<ArtifactId, Digest>,
    action: &Action,
    at: u64,
) -> Result<serde_json::Value, Error> {
    if matches!(action, Action::Restart) {
        let previous = world.take().unwrap();
        *world = Some(World::open(previous.journal, previous.publisher)?);
        return Ok(serde_json::json!({"recovered": true}));
    }
    let w = world.as_mut().unwrap();
    let actor = PrincipalId::from("maya");
    let make_intent = |artifact: &str| Intent {
        artifact: artifact.into(),
        audience: "company".into(),
        requester: actor.clone(),
    };
    let value = match action {
        Action::Generate { restricted, key } => {
            serde_json::json!({"artifact": w.generate(&actor, *restricted, key)?})
        }
        Action::Verify { artifact } => {
            serde_json::to_value(verify(&w.state().evidence, &make_intent(artifact), at)).unwrap()
        }
        Action::Approve { artifact, key } => {
            let i = make_intent(artifact);
            let mut e = w.state().evidence.clone();
            e.approvals.clear();
            let context = verify(&e, &i, at)
                .context()
                .ok_or_else(|| Error::Invalid("cannot approve source-unsafe draft".into()))?
                .digest();
            let id = w.approve(&actor, &i, &context, key, at)?;
            contexts.insert(i.artifact, context);
            serde_json::json!({"approval": id})
        }
        Action::Edit => {
            let id = w.edit(
                &actor,
                &"draft-safe".into(),
                "Northstar is preparing a reviewed launch update. More details will follow.",
                "revised",
            )?;
            contexts.insert(
                id.clone(),
                contexts[&ArtifactId::from("draft-safe")].clone(),
            );
            serde_json::json!({"artifact": id})
        }
        Action::Publish { artifact, key } => {
            let i = make_intent(artifact);
            let expected = contexts
                .get(&i.artifact)
                .ok_or_else(|| Error::Invalid("no captured context".into()))?;
            let result = w.publish(&i, expected, key, at)?;
            // Keep traces small; exported bundles remain available through the CLI/API.
            match result {
                PublishResult::Blocked { decision } => serde_json::to_value(decision).unwrap(),
                PublishResult::Operation { operation } => {
                    serde_json::json!({"operation": operation.key, "status": operation.state, "receipt": operation.receipt})
                }
            }
        }
        Action::NarrowAcl => {
            w.change_acl(
                &actor,
                &"roadmap".into(),
                fixtures::readers(&["maya", "arun", "agent"]),
                "narrow",
            )?;
            serde_json::json!({"acl_narrowed": true})
        }
        Action::Revoke => {
            w.revoke(&actor, &"approval-safe".into())?;
            serde_json::json!({"revoked": true})
        }
        Action::LoseAcknowledgment => {
            w.publisher.next_fault = PublisherFault::AcceptThenTimeout;
            serde_json::json!({"fault": "accept_then_timeout"})
        }
        Action::LoseJournalAcknowledgment => {
            w.journal.fault = Some(WriteFault::AfterCommit);
            serde_json::json!({"fault": "journal_commit_ack_lost"})
        }
        Action::Reconcile => {
            let op = w.reconcile(&actor, "post", at)?;
            serde_json::json!({"operation": op.key, "status": op.state, "receipt": op.receipt})
        }
        Action::ChangeAudience => {
            let mut audience = w.state().evidence.audiences[&AudienceId::from("company")].clone();
            // Reviewers may narrow membership within the ceiling but never move
            // the destination; any membership change invalidates the approval.
            audience.recipients = fixtures::readers(&["maya", "arun"]);
            w.change_audience(&actor, audience)?;
            serde_json::json!({"audience_changed": true})
        }
        Action::ChangePolicy => {
            w.change_policy(&actor)?;
            serde_json::json!({"policy_changed": true})
        }
        Action::Restart => unreachable!(),
    };
    Ok(value)
}
fn actions(scenario: Scenario) -> Vec<(u64, Action)> {
    use Action::*;
    let publish = |artifact: &str| Publish {
        artifact: artifact.into(),
        key: "post".into(),
    };
    let mut actions = vec![
        (
            0,
            Generate {
                restricted: false,
                key: "safe".into(),
            },
        ),
        (
            1,
            Approve {
                artifact: "draft-safe".into(),
                key: "safe".into(),
            },
        ),
    ];
    match scenario {
        Scenario::Demo => {
            return vec![
                (
                    0,
                    Generate {
                        restricted: true,
                        key: "restricted".into(),
                    },
                ),
                (
                    1,
                    Verify {
                        artifact: "draft-restricted".into(),
                    },
                ),
                (
                    2,
                    Generate {
                        restricted: false,
                        key: "safe".into(),
                    },
                ),
                (
                    3,
                    Verify {
                        artifact: "draft-safe".into(),
                    },
                ),
                (
                    4,
                    Approve {
                        artifact: "draft-safe".into(),
                        key: "safe".into(),
                    },
                ),
                (5, Edit),
                (
                    6,
                    Verify {
                        artifact: "edit-revised".into(),
                    },
                ),
                (7, publish("edit-revised")),
                (
                    8,
                    Approve {
                        artifact: "edit-revised".into(),
                        key: "revised".into(),
                    },
                ),
                (9, publish("edit-revised")),
                (10, Reconcile),
            ];
        }
        Scenario::AclRace => actions.extend([(10, NarrowAcl), (10, publish("draft-safe"))]),
        Scenario::RevocationRace => actions.extend([(10, Revoke), (10, publish("draft-safe"))]),
        Scenario::TimeoutRecovery => actions.extend([
            (2, LoseAcknowledgment),
            (3, publish("draft-safe")),
            (4, Restart),
            (5, Reconcile),
        ]),
        Scenario::JournalRecovery => actions.extend([
            (2, LoseJournalAcknowledgment),
            (3, publish("draft-safe")),
            (4, Restart),
            (5, Reconcile),
        ]),
        Scenario::Expiry => actions.push((3601, publish("draft-safe"))),
        Scenario::AudienceChange => {
            actions.extend([(2, ChangeAudience), (3, publish("draft-safe"))])
        }
        Scenario::PolicyChange => actions.extend([(2, ChangePolicy), (3, publish("draft-safe"))]),
    }
    actions
}
