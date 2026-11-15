//! Exhaustive exploration of a declared finite abstraction, not of external systems.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    Production,
    ReuseApprovalMutant,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    GeneratePublic,
    GenerateMixed,
    Edit,
    Approve,
    CompanyAudience,
    ProductAudience,
    DropLineage,
    Publish,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceStep {
    pub action: Action,
    pub artifact: String,
    pub audience: String,
    pub approved: bool,
    pub decision: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckReport {
    pub model: String,
    pub rule: Rule,
    pub max_depth: u8,
    pub states: usize,
    pub transitions: usize,
    pub publications: usize,
    pub passed: bool,
    pub complete_within_bounds: bool,
    pub properties: Vec<String>,
    pub counterexample: Option<Vec<TraceStep>>,
    pub scope: String,
}
use std::collections::{BTreeSet, VecDeque};
use trustgraph_core::{Decision, digest, *};
use trustgraph_runtime::fixtures;
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    None,
    Public,
    Mixed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Stamp {
    kind: Kind,
    version: u8,
    audience: u8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct State {
    kind: Kind,
    version: u8,
    audience: u8,
    complete: bool,
    approval: Option<Stamp>,
}
fn artifact(kind: Kind, version: u8) -> ArtifactId {
    ArtifactId(format!(
        "model-{}-{version}",
        match kind {
            Kind::None => "none",
            Kind::Public => "public",
            Kind::Mixed => "mixed",
        }
    ))
}
fn intent(s: State) -> Intent {
    Intent {
        artifact: artifact(s.kind, s.version),
        audience: if s.audience == 0 {
            "company".into()
        } else {
            "product".into()
        },
        requester: "maya".into(),
    }
}
fn model() -> Evidence {
    let mut e = fixtures::northstar();
    e.approvals.clear();
    for kind in [Kind::Public, Kind::Mixed] {
        for version in 0..2 {
            let id = artifact(kind, version);
            let tid = TransformationId(format!("transform-{}", id.0));
            let inputs: Vec<_> = if version == 1 {
                vec![artifact(kind, 0)]
            } else if kind == Kind::Mixed {
                vec!["task-prompt".into(), "roadmap".into(), "launch-plan".into()]
            } else {
                vec!["task-prompt".into(), "roadmap".into()]
            };
            let manifest = Manifest {
                complete: true,
                provider: "bounded-model".into(),
                template_revision: "finite-1".into(),
                inputs: inputs
                    .into_iter()
                    .enumerate()
                    .map(|(order, artifact)| InputUse {
                        occurrence: format!("{}-{order}", id.0),
                        artifact,
                        role: if version == 1 {
                            InputRole::PreviousDraft
                        } else {
                            InputRole::Document
                        },
                        order,
                    })
                    .collect(),
            };
            let t = Transformation {
                id: tid.clone(),
                workspace: e.workspace.clone(),
                output: id.clone(),
                manifest_hash: digest::structured("context-manifest", &manifest),
                manifest,
            };
            let mut a = Artifact::new(
                id.clone(),
                e.workspace.clone(),
                format!(
                    "{:?} modeled announcement{}",
                    kind,
                    if version == 1 { "!" } else { "" }
                ),
                Origin::Transformation(tid.clone()),
            );
            a.logical_id = format!("model-{kind:?}");
            a.version = version as u64 + 1;
            e.artifacts.insert(id, a);
            e.transformations.insert(tid, t);
        }
    }
    e
}
fn materialize(base: &Evidence, s: State) -> Evidence {
    let mut e = base.clone();
    if let Some(stamp) = s.approval {
        let approved = State {
            kind: stamp.kind,
            version: stamp.version,
            audience: stamp.audience,
            complete: true,
            approval: None,
        };
        let context = verify(base, &intent(approved), 10)
            .context()
            .expect("only source-permitted model states can be approved")
            .clone();
        let a = Approval {
            id: "model-approval".into(),
            signer: "maya".into(),
            context,
            issued_at: 0,
            expires_at: 1000,
        };
        e.approvals.insert(a.id.clone(), a);
    }
    e.complete = s.complete;
    e
}
fn declared_readers(base: &Evidence, kind: Kind) -> Readers {
    let mut sources = vec!["task-prompt", "roadmap"];
    if kind == Kind::Mixed {
        sources.push("launch-plan");
    }
    sources
        .into_iter()
        .map(|id| {
            base.acls[&base.sources[&SourceId::from(id)].acl]
                .readers
                .clone()
        })
        .reduce(|a, b| a.intersection(&b).cloned().collect())
        .unwrap_or_default()
}
fn decision_label(d: &Decision) -> String {
    match d {
        Decision::Ready { .. } => "READY".into(),
        Decision::PendingApproval { .. } => "PENDING_APPROVAL".into(),
        Decision::Blocked { violations } => format!("BLOCKED: {:?}", violations[0].code),
    }
}
fn describe(action: Action, s: State, decision: String) -> TraceStep {
    TraceStep {
        action,
        artifact: if s.kind == Kind::None {
            "none".into()
        } else {
            artifact(s.kind, s.version).0
        },
        audience: if s.audience == 0 {
            "company".into()
        } else {
            "product".into()
        },
        approved: s.approval.is_some(),
        decision,
    }
}
/// Breadth-first search gives the shortest discovered failure trace. State
/// deduplication is valid because this abstraction has fixed time and no hidden I/O.
pub fn check(max_depth: u8, rule: Rule) -> Result<CheckReport, String> {
    if !(1..=8).contains(&max_depth) {
        return Err("model depth must be between 1 and 8".into());
    }
    let base = model();
    let mut report=CheckReport{model:"trustgraph-finite-1".into(),rule,max_depth,states:0,transitions:0,publications:0,passed:true,complete_within_bounds:false,properties:vec!["No unauthorized recipient".into(),"Exact artifact and destination approval".into(),"Missing lineage fails closed".into(),"Restricted inputs cannot widen readership".into()],counterexample:None,scope:"3 human readers plus the agent, 2 document source sets plus the recorded prompt, 2 audiences, 2 immutable versions per source set, one approval, complete/missing lineage, one terminal publication. Time is fixed; external providers, databases, threads, expiry and network behavior are outside this model.".into()};
    for version in 0..2 {
        let public = lineage::derived_readers(&base, &artifact(Kind::Public, version))
            .map_err(|_| "invalid public model")?;
        let private = lineage::derived_readers(&base, &artifact(Kind::Mixed, version))
            .map_err(|_| "invalid restricted model")?;
        if !private.is_subset(&public)
            || private.contains(&PrincipalId::from("sam"))
            || !public.contains(&PrincipalId::from("sam"))
        {
            return Err("readership monotonicity failed in the finite input world".into());
        }
    }
    let initial = State {
        kind: Kind::None,
        version: 0,
        audience: 0,
        complete: true,
        approval: None,
    };
    let mut seen: BTreeSet<_> = [initial].into();
    let mut queue = VecDeque::from([(initial, Vec::<TraceStep>::new())]);
    let actions = [
        Action::GeneratePublic,
        Action::GenerateMixed,
        Action::Edit,
        Action::Approve,
        Action::CompanyAudience,
        Action::ProductAudience,
        Action::DropLineage,
        Action::Publish,
    ];
    while let Some((state, trace)) = queue.pop_front() {
        report.states += 1;
        if trace.len() >= max_depth as usize {
            continue;
        }
        for action in actions {
            report.transitions += 1;
            let mut next = state;
            if action == Action::Publish {
                if state.kind == Kind::None {
                    continue;
                }
                let decision = verify(&materialize(&base, state), &intent(state), 10);
                let allowed = match rule {
                    Rule::Production => matches!(decision, Decision::Ready { .. }),
                    Rule::ReuseApprovalMutant => state.approval.is_some(),
                };
                if allowed {
                    report.publications += 1;
                    // Independent predicates over the fixture's declared source
                    // ACLs and audience membership (plain std set operations,
                    // not the lineage walker) and the exact version/destination stamp.
                    let readers = declared_readers(&base, state.kind);
                    let recipients = &base.audiences[&intent(state).audience].recipients;
                    let violation = if !recipients.is_subset(&readers) {
                        Some("UNAUTHORIZED_AUDIENCE")
                    } else if state.approval
                        != Some(Stamp {
                            kind: state.kind,
                            version: state.version,
                            audience: state.audience,
                        })
                    {
                        Some("APPROVAL_DRIFT")
                    } else if !state.complete {
                        Some("MISSING_LINEAGE")
                    } else {
                        None
                    };
                    if let Some(violation) = violation {
                        let mut failure = trace.clone();
                        failure.push(describe(
                            action,
                            state,
                            format!(
                                "VIOLATION: {violation}; production gate: {}",
                                decision_label(&decision)
                            ),
                        ));
                        report.passed = false;
                        report.counterexample = Some(failure);
                        return Ok(report);
                    }
                }
                continue;
            }
            match action {
                Action::GeneratePublic => {
                    next.kind = Kind::Public;
                    next.version = 0;
                    next.complete = true;
                }
                Action::GenerateMixed => {
                    next.kind = Kind::Mixed;
                    next.version = 0;
                    next.complete = true;
                }
                Action::Edit => {
                    if next.kind == Kind::None || next.version == 1 {
                        continue;
                    }
                    next.version = 1;
                }
                Action::Approve => {
                    if next.kind == Kind::None {
                        continue;
                    }
                    let mut e = materialize(&base, next);
                    e.approvals.clear();
                    if verify(&e, &intent(next), 10).context().is_none() {
                        continue;
                    }
                    next.approval = Some(Stamp {
                        kind: next.kind,
                        version: next.version,
                        audience: next.audience,
                    });
                }
                Action::CompanyAudience => next.audience = 0,
                Action::ProductAudience => next.audience = 1,
                Action::DropLineage => next.complete = false,
                Action::Publish => unreachable!(),
            }
            if seen.insert(next) {
                let mut path = trace.clone();
                let label = if next.kind == Kind::None {
                    "NO_ARTIFACT".into()
                } else {
                    decision_label(&verify(&materialize(&base, next), &intent(next), 10))
                };
                path.push(describe(action, next, label));
                queue.push_back((next, path));
            }
        }
    }
    report.complete_within_bounds = true;
    Ok(report)
}
