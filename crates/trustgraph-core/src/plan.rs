//! A small, explicitly bounded language for analyzing recorded information flow.
use crate::{
    digest::{self, Digest},
    *,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub goal: String,
    pub steps: Vec<Step>,
}
impl Plan {
    pub fn digest(&self) -> Digest {
        digest::structured("agent-plan", self)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub id: String,
    #[serde(flatten)]
    pub operation: Op,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Op {
    Read {
        source: SourceId,
    },
    Generate {
        inputs: Vec<String>,
        provider: Provider,
    },
    Approve {
        artifact: String,
        audience: AudienceId,
    },
    Publish {
        artifact: String,
        audience: AudienceId,
        approval: String,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    LocalFixture,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlanCode {
    InvalidShape,
    UnavailableReference,
    TypeMismatch,
    MissingLineage,
    CallerUnauthorized,
    AudienceUnknown,
    AudienceConflict,
    ApprovalBindingMismatch,
    MissingPrompt,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub code: PlanCode,
    pub step: String,
    pub recipient: Option<PrincipalId>,
    pub source: Option<ArtifactId>,
    pub path: Vec<String>,
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisStep {
    pub id: String,
    pub operation: String,
    pub inputs: Vec<String>,
    pub readers: Readers,
    pub sources: Vec<ArtifactId>,
    pub effect: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Obligation {
    pub step: String,
    pub artifact: String,
    pub audience: AudienceId,
    pub description: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub ready_to_generate: bool,
    pub plan_digest: Digest,
    pub snapshot_digest: Digest,
    pub steps: Vec<AnalysisStep>,
    pub obligations: Vec<Obligation>,
    pub conflict: Option<Conflict>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairKind {
    NarrowAudience,
    FreshContext,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repair {
    pub kind: RepairKind,
    pub title: String,
    pub explanation: String,
    pub plan: Plan,
    pub report: Report,
}

#[derive(Clone)]
struct ArtifactValue {
    readers: Readers,
    paths: std::collections::BTreeMap<ArtifactId, Vec<String>>,
    generated: bool,
}
#[derive(Clone)]
enum Value {
    Artifact(ArtifactValue),
    Approval {
        artifact: String,
        audience: AudienceId,
    },
}
fn problem(code: PlanCode, step: &str, message: &str) -> Box<Conflict> {
    Box::new(Conflict {
        code,
        step: step.into(),
        recipient: None,
        source: None,
        path: vec![],
        message: message.into(),
    })
}
fn value<'a>(
    values: &'a std::collections::BTreeMap<String, Value>,
    id: &str,
    step: &str,
) -> Result<&'a ArtifactValue, Box<Conflict>> {
    match values.get(id) {
        Some(Value::Artifact(a)) => Ok(a),
        Some(_) => Err(problem(
            PlanCode::TypeMismatch,
            step,
            "An artifact input was expected.",
        )),
        None => Err(problem(
            PlanCode::UnavailableReference,
            step,
            "References must name an earlier step; missing and cyclic dependencies are rejected.",
        )),
    }
}
fn audience<'a>(
    e: &'a Evidence,
    id: &AudienceId,
    step: &str,
) -> Result<&'a Audience, Box<Conflict>> {
    crate::verifier::resolve_audience(e, id)
        .ok()
        .ok_or_else(|| {
            problem(
                PlanCode::AudienceUnknown,
                step,
                "The destination must have a complete, known recipient set.",
            )
        })
}
pub fn analyze(e: &Evidence, plan: &Plan, actor: &PrincipalId) -> Report {
    let source_hashes: Vec<_> = e
        .sources
        .values()
        .map(|s| (&s.id, e.artifacts.get(&s.artifact).map(|a| &a.hash)))
        .collect();
    let mut report = Report {
        ready_to_generate: false,
        plan_digest: plan.digest(),
        snapshot_digest: digest::structured(
            "plan-input-snapshot",
            &(
                &e.workspace,
                e.complete,
                &e.sources,
                &e.acls,
                &e.audiences,
                &e.policy,
                &e.principals,
                source_hashes,
            ),
        ),
        steps: vec![],
        obligations: vec![],
        conflict: None,
    };
    match compile(e, plan, actor, &mut report) {
        Ok(()) => report.ready_to_generate = true,
        Err(conflict) => report.conflict = Some(*conflict),
    }
    report
}
fn compile(
    e: &Evidence,
    plan: &Plan,
    actor: &PrincipalId,
    report: &mut Report,
) -> Result<(), Box<Conflict>> {
    if !e
        .principals
        .get(actor)
        .is_some_and(|p| p.id == *actor && p.enabled && p.workspace == e.workspace)
    {
        return Err(problem(
            PlanCode::CallerUnauthorized,
            "plan",
            "The caller is not authorized to analyze this plan.",
        ));
    }
    if !e.complete {
        return Err(problem(
            PlanCode::MissingLineage,
            "plan",
            "Evidence is incomplete.",
        ));
    }
    let valid_id = |id: &str| {
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    };
    if !valid_id(&plan.id)
        || plan.goal.trim().is_empty()
        || plan.goal.len() > 512
        || !(4..=32).contains(&plan.steps.len())
    {
        return Err(problem(
            PlanCode::InvalidShape,
            "plan",
            "Use a named plan with 4 to 32 operations and a goal up to 512 bytes.",
        ));
    }
    let n = plan.steps.len();
    if !matches!(plan.steps[n - 2].operation, Op::Approve { .. })
        || !matches!(plan.steps[n - 1].operation, Op::Publish { .. })
        || plan
            .steps
            .iter()
            .filter(|s| matches!(s.operation, Op::Approve { .. }))
            .count()
            != 1
        || plan
            .steps
            .iter()
            .filter(|s| matches!(s.operation, Op::Publish { .. }))
            .count()
            != 1
    {
        return Err(problem(
            PlanCode::InvalidShape,
            "plan",
            "This language requires one human approval immediately before the sole final publication.",
        ));
    }
    let mut values = std::collections::BTreeMap::new();
    for step in &plan.steps {
        if !valid_id(&step.id) || values.contains_key(&step.id) {
            return Err(problem(
                PlanCode::InvalidShape,
                &step.id,
                "Step identities must be unique and nonempty.",
            ));
        }
        let (output, operation, inputs, effect) = match &step.operation {
            Op::Read { source } => {
                let source_id = source;
                let source = e.sources.get(source).ok_or_else(|| {
                    problem(
                        PlanCode::MissingLineage,
                        &step.id,
                        "A source reference is unresolved.",
                    )
                })?;
                if &source.id != source_id
                    || source.workspace != e.workspace
                    || !e
                        .artifacts
                        .get(&source.artifact)
                        .is_some_and(|a| a.origin == Origin::Source(source_id.clone()))
                {
                    return Err(problem(
                        PlanCode::MissingLineage,
                        &step.id,
                        "Source references do not agree with their artifact revision.",
                    ));
                }
                let readers =
                    crate::lineage::derived_readers(e, &source.artifact).map_err(|_| {
                        problem(
                            PlanCode::MissingLineage,
                            &step.id,
                            "Source evidence is incomplete or inconsistent.",
                        )
                    })?;
                if !readers.contains(actor) {
                    return Err(problem(
                        PlanCode::CallerUnauthorized,
                        &step.id,
                        "The caller cannot read every input. Private source details are hidden.",
                    ));
                }
                (
                    Value::Artifact(ArtifactValue {
                        readers,
                        paths: [(source.artifact.clone(), vec![step.id.clone()])].into(),
                        generated: false,
                    }),
                    "read",
                    vec![],
                    "Read a recorded source revision",
                )
            }
            Op::Generate {
                inputs,
                provider: Provider::LocalFixture,
            } => {
                if inputs.is_empty() || inputs.len() > 64 {
                    return Err(problem(
                        PlanCode::MissingLineage,
                        &step.id,
                        "Generation needs 1 to 64 explicitly recorded inputs.",
                    ));
                }
                let mut readers: Option<Readers> = None;
                let mut paths = std::collections::BTreeMap::new();
                for input in inputs {
                    let a = value(&values, input, &step.id)?;
                    readers = Some(match readers {
                        None => a.readers.clone(),
                        Some(r) => r.intersection(&a.readers).cloned().collect(),
                    });
                    for (source, path) in &a.paths {
                        let mut path = path.clone();
                        path.push(step.id.clone());
                        paths.entry(source.clone()).or_insert(path);
                    }
                }
                if !inputs.iter().any(|id|plan.steps.iter().find(|s|&s.id==id).is_some_and(|s|matches!(&s.operation,Op::Read{source} if source==&SourceId::from("task-prompt"))))
                {
                    return Err(problem(
                        PlanCode::MissingPrompt,
                        &step.id,
                        "The task prompt must be an explicit recorded input to every fresh model context.",
                    ));
                }
                (
                    Value::Artifact(ArtifactValue {
                        readers: readers.unwrap_or_default(),
                        paths,
                        generated: true,
                    }),
                    "generate",
                    inputs.clone(),
                    "Create a fresh local fixture context; no external provider transfer",
                )
            }
            Op::Approve {
                artifact,
                audience: id,
            } => {
                let a = value(&values, artifact, &step.id)?;
                audience(e, id, &step.id)?;
                if !a.generated {
                    return Err(problem(
                        PlanCode::TypeMismatch,
                        &step.id,
                        "Human review must refer to the generated result.",
                    ));
                }
                report.obligations.push(Obligation{step:step.id.clone(),artifact:artifact.clone(),audience:id.clone(),description:"After generation, a human must approve the actual bytes and exact destination. This step is not approval in advance.".into()});
                (
                    Value::Approval {
                        artifact: artifact.clone(),
                        audience: id.clone(),
                    },
                    "approve",
                    vec![artifact.clone()],
                    "Pause for authenticated human approval of actual content",
                )
            }
            Op::Publish {
                artifact,
                audience: id,
                approval,
            } => {
                let a = value(&values, artifact, &step.id)?;
                let target = audience(e, id, &step.id)?;
                match values.get(approval) {
                    Some(Value::Approval {
                        artifact: approved,
                        audience,
                    }) if approved == artifact && audience == id => {}
                    Some(Value::Approval { .. }) => {
                        return Err(problem(
                            PlanCode::ApprovalBindingMismatch,
                            &step.id,
                            "Approval and publication must refer to the same generated artifact and audience.",
                        ));
                    }
                    Some(_) => {
                        return Err(problem(
                            PlanCode::TypeMismatch,
                            &step.id,
                            "Publication needs an approval obligation.",
                        ));
                    }
                    None => {
                        return Err(problem(
                            PlanCode::UnavailableReference,
                            &step.id,
                            "Publication references a missing approval step.",
                        ));
                    }
                }
                if let Some(recipient) = target.recipients.difference(&a.readers).next() {
                    let witness = a
                        .paths
                        .keys()
                        .find(|artifact| {
                            crate::lineage::derived_readers(e, artifact)
                                .is_ok_and(|r| !r.contains(recipient))
                        })
                        .cloned();
                    let mut path = witness
                        .as_ref()
                        .and_then(|id| a.paths.get(id))
                        .cloned()
                        .unwrap_or_default();
                    path.push(step.id.clone());
                    return Err(Box::new(Conflict{code:PlanCode::AudienceConflict,step:step.id.clone(),recipient:Some(recipient.clone()),source:witness,path,message:"A destination recipient cannot read a source that the generated artifact depends on.".into()}));
                }
                (
                    Value::Artifact(a.clone()),
                    "publish",
                    vec![artifact.clone(), approval.clone()],
                    "Recheck current evidence and dispatch only the approved bytes",
                )
            }
        };
        let (readers, sources) = match &output {
            Value::Artifact(a) => (a.readers.clone(), a.paths.keys().cloned().collect()),
            Value::Approval { artifact, .. } => {
                let a = value(&values, artifact, &step.id)?;
                (a.readers.clone(), a.paths.keys().cloned().collect())
            }
        };
        report.steps.push(AnalysisStep {
            id: step.id.clone(),
            operation: operation.into(),
            inputs,
            readers,
            sources,
            effect: effect.into(),
        });
        values.insert(step.id.clone(), output);
    }
    Ok(())
}
pub fn repairs(e: &Evidence, plan: &Plan, actor: &PrincipalId) -> Vec<Repair> {
    let report = analyze(e, plan, actor);
    if !report
        .conflict
        .as_ref()
        .is_some_and(|c| c.code == PlanCode::AudienceConflict)
    {
        return vec![];
    }
    let Some(Step {
        operation:
            Op::Publish {
                artifact,
                audience: target,
                ..
            },
        ..
    }) = plan.steps.last()
    else {
        return vec![];
    };
    let mut options = vec![];
    for audience in e.audiences.values().filter(|a| a.id != *target) {
        let mut candidate = plan.clone();
        for step in &mut candidate.steps {
            match &mut step.operation {
                Op::Approve { audience: id, .. } | Op::Publish { audience: id, .. } => {
                    *id = audience.id.clone()
                }
                _ => {}
            }
        }
        candidate.id = format!("repair-{}", &candidate.digest().0[..12]);
        let analysis = analyze(e, &candidate, actor);
        if analysis.ready_to_generate {
            options.push(Repair{kind:RepairKind::NarrowAudience,title:"Send to a permitted audience".into(),explanation:"Keep the proposed inputs and change the destination. No source permission is widened.".into(),plan:candidate,report:analysis});
            break;
        }
    }
    let Some(target) = e.audiences.get(target) else {
        return options;
    };
    let mut candidate = plan.clone();
    let safe_inputs: Vec<_> = candidate
        .steps
        .iter()
        .filter_map(|s| {
            if let Op::Read { source } = &s.operation {
                e.sources
                    .get(source)
                    .filter(|source| {
                        crate::lineage::derived_readers(e, &source.artifact)
                            .is_ok_and(|r| target.recipients.is_subset(&r))
                    })
                    .map(|_| s.id.clone())
            } else {
                None
            }
        })
        .collect();
    let document_present = candidate.steps.iter().any(|s| {
        safe_inputs.contains(&s.id)
            && matches!(&s.operation,Op::Read{source} if source!=&SourceId::from("task-prompt"))
    });
    if document_present {
        if let Some(Step {
            operation: Op::Generate { inputs, .. },
            ..
        }) = candidate.steps.iter_mut().find(|s| s.id == *artifact)
        {
            *inputs = safe_inputs;
        }
        // Backward reachability removes the private reads and prior generation
        // branches instead of leaving a contaminated conversation in the plan.
        let mut keep = std::collections::BTreeSet::new();
        let mut pending = vec![candidate.steps.last().unwrap().id.clone()];
        while let Some(id) = pending.pop() {
            if !keep.insert(id.clone()) {
                continue;
            }
            if let Some(step) = candidate.steps.iter().find(|s| s.id == id) {
                match &step.operation {
                    Op::Generate { inputs, .. } => pending.extend(inputs.clone()),
                    Op::Approve { artifact, .. } => pending.push(artifact.clone()),
                    Op::Publish {
                        artifact, approval, ..
                    } => pending.extend([artifact.clone(), approval.clone()]),
                    _ => {}
                }
            }
        }
        candidate.steps.retain(|s| keep.contains(&s.id));
        candidate.id = format!("fresh-{}", &candidate.digest().0[..12]);
        let analysis = analyze(e, &candidate, actor);
        if analysis.ready_to_generate {
            options.push(Repair{kind:RepairKind::FreshContext,title:"Generate a fresh audience-safe draft".into(),explanation:"Start a new context using only permitted inputs. The result may contain different information; the old draft is not reused.".into(),plan:candidate,report:analysis});
        }
    }
    options
}

pub fn launch_plan(id: &str, restricted: bool, audience: AudienceId) -> Plan {
    let mut steps = vec![
        Step {
            id: "read-roadmap".into(),
            operation: Op::Read {
                source: "roadmap".into(),
            },
        },
        Step {
            id: "read-prompt".into(),
            operation: Op::Read {
                source: "task-prompt".into(),
            },
        },
    ];
    if restricted {
        steps.push(Step {
            id: "read-launch-plan".into(),
            operation: Op::Read {
                source: "launch-plan".into(),
            },
        });
    }
    let inputs = steps.iter().map(|s| s.id.clone()).collect();
    steps.extend([
        Step {
            id: "generate".into(),
            operation: Op::Generate {
                inputs,
                provider: Provider::LocalFixture,
            },
        },
        Step {
            id: "approve".into(),
            operation: Op::Approve {
                artifact: "generate".into(),
                audience: audience.clone(),
            },
        },
        Step {
            id: "publish".into(),
            operation: Op::Publish {
                artifact: "generate".into(),
                audience,
                approval: "approve".into(),
            },
        },
    ]);
    Plan {
        id: id.into(),
        goal: "Post this week’s launch update".into(),
        steps,
    }
}
