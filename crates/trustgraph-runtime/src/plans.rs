//! Immutable plan proposals, resumable generation and runtime contract checks.
use crate::{engine::validate_key, journal::Journal, publisher::Publisher, *};
use trustgraph_core::{plan::*, *};

impl<J: Journal, P: Publisher> Coordinator<J, P> {
    pub fn propose_plan(
        &mut self,
        actor: &PrincipalId,
        plan: Plan,
        now: u64,
    ) -> Result<PlanRecord, Error> {
        if self.actor(actor)?.role != Role::Agent {
            return Err(Error::Unauthorized);
        }
        // Authorize every referenced source independently of the compiler's
        // first diagnostic; a shape error must not hide a later private read.
        for step in &plan.steps {
            if let Op::Read { source } = &step.operation
                && let Some(source) = self.state.evidence.sources.get(source)
                && !lineage::can_read(&self.state.evidence, actor, &source.artifact)
            {
                return Err(Error::Unauthorized);
            }
        }
        validate_key(&plan.id)?;
        if plan.id.len() > 64 || plan.steps.len() > 32 || plan.goal.len() > 512 {
            return Err(Error::Invalid("plan exceeds language bounds".into()));
        }
        if analyze(&self.state.evidence, &plan, actor)
            .conflict
            .is_some_and(|c| c.code == PlanCode::CallerUnauthorized)
        {
            return Err(Error::Unauthorized);
        }
        if let Some(existing) = self.state.plans.get(&plan.id) {
            return if existing.plan == plan && existing.submitted_by == *actor {
                Ok(self.read_plan(actor, &plan.id)?.clone())
            } else {
                Err(Error::Conflict)
            };
        }
        let record = PlanRecord {
            plan,
            submitted_by: actor.clone(),
            created_by: actor.clone(),
            parent: None,
            created_at: now,
        };
        self.commit(Event::PlanProposed {
            record: Box::new(record.clone()),
        })?;
        Ok(record)
    }
    pub fn read_plan(&self, actor: &PrincipalId, id: &str) -> Result<&PlanRecord, Error> {
        self.actor(actor)?;
        let record = self.state.plans.get(id).ok_or(Error::NotFound)?;
        if record.submitted_by != *actor && self.reviewer(actor).is_err() {
            return Err(Error::NotFound);
        }
        for step in &record.plan.steps {
            if let Op::Read { source } = &step.operation
                && let Some(source) = self.state.evidence.sources.get(source)
                && !lineage::can_read(&self.state.evidence, actor, &source.artifact)
            {
                return Err(Error::NotFound);
            }
        }
        Ok(record)
    }
    pub fn repair_plan(
        &mut self,
        actor: &PrincipalId,
        id: &str,
        kind: RepairKind,
        now: u64,
    ) -> Result<PlanRecord, Error> {
        let parent = self.read_plan(actor, id)?.clone();
        let option = repairs(&self.state.evidence, &parent.plan, &parent.submitted_by)
            .into_iter()
            .find(|r| r.kind == kind)
            .ok_or_else(|| {
                Error::Invalid("no permitted repair of this kind under current evidence".into())
            })?;
        if let Some(existing) = self.state.plans.get(&option.plan.id) {
            return if existing.plan == option.plan {
                Ok(self.read_plan(actor, &option.plan.id)?.clone())
            } else {
                Err(Error::Conflict)
            };
        }
        let record = PlanRecord {
            plan: option.plan,
            submitted_by: parent.submitted_by,
            created_by: actor.clone(),
            parent: Some(id.into()),
            created_at: now,
        };
        self.commit(Event::PlanProposed {
            record: Box::new(record.clone()),
        })?;
        Ok(record)
    }
    pub fn prepare_plan(
        &mut self,
        actor: &PrincipalId,
        id: &str,
        expected: &digest::Digest,
        snapshot: &digest::Digest,
        key: &str,
        now: u64,
    ) -> Result<PlanRun, Error> {
        validate_key(key)?;
        let record = self.read_plan(actor, id)?.clone();
        let report = analyze(&self.state.evidence, &record.plan, &record.submitted_by);
        if !report.ready_to_generate {
            return Err(Error::Invalid(
                "plan is blocked; inspect its compiler conflict before execution".into(),
            ));
        }
        if report.plan_digest != *expected || report.snapshot_digest != *snapshot {
            return Err(Error::Conflict);
        }
        let run_id = format!(
            "run-{}",
            &digest::structured("plan-run-key", &(id, key)).0[..32]
        );
        let mut run = if let Some(existing) = self.state.plan_runs.get(&run_id) {
            if existing.plan_digest != *expected || existing.snapshot_digest != *snapshot {
                return Err(Error::Conflict);
            }
            if existing.stage == RunStage::AwaitingReview {
                return Ok(existing.clone());
            }
            existing.clone()
        } else {
            let mut bindings = std::collections::BTreeMap::new();
            for step in &record.plan.steps {
                if let Op::Read { source } = &step.operation {
                    bindings.insert(
                        step.id.clone(),
                        self.state.evidence.sources[source].artifact.clone(),
                    );
                } else if matches!(step.operation, Op::Generate { .. }) {
                    bindings.insert(step.id.clone(), generated_id(&run_id, &step.id));
                }
            }
            let run = PlanRun {
                id: run_id.clone(),
                plan_id: id.into(),
                plan_digest: expected.clone(),
                snapshot_digest: snapshot.clone(),
                started_by: actor.clone(),
                operating_agent: record.submitted_by.clone(),
                bindings,
                request: format!(
                    "review-{}",
                    &digest::structured("plan-review-key", &run_id).0[..32]
                ),
                stage: RunStage::Preparing,
                created_at: now,
            };
            self.commit(Event::PlanRunReserved {
                run: Box::new(run.clone()),
            })?;
            run
        };
        for step in &record.plan.steps {
            if let Op::Generate {
                inputs,
                provider: Provider::LocalFixture,
            } = &step.operation
            {
                let inputs: Vec<_> = inputs
                    .iter()
                    .map(|id| run.bindings.get(id).cloned().ok_or(Error::Conflict))
                    .collect::<Result<_, _>>()?;
                for input in &inputs {
                    if !lineage::can_read(&self.state.evidence, &record.submitted_by, input) {
                        return Err(Error::Unauthorized);
                    }
                }
                let content = fixtures::render_plan_output(&self.state.evidence, &inputs);
                let artifact = generated_id(&run.id, &step.id);
                let artifact = self.create_derived(
                    &record.submitted_by,
                    artifact,
                    inputs,
                    content,
                    None,
                    "plan-local-fixture-v1",
                )?;
                run.bindings.insert(step.id.clone(), artifact);
            }
        }
        let Some(Step {
            operation: Op::Publish {
                artifact, audience, ..
            },
            ..
        }) = record.plan.steps.last()
        else {
            return Err(Error::Conflict);
        };
        self.submit_request(&record.submitted_by,Submission{key:run.request.clone(),title:review_title(&record.plan.goal),summary:format!("Plan goal: {}\nGenerated from a compiled plan. Human approval must cover this actual result and destination.",record.plan.goal),artifact:run.bindings.get(artifact).cloned().ok_or(Error::Conflict)?,audience:audience.clone()},now)?;
        self.commit(Event::PlanRunPrepared {
            id: run.id.clone(),
            bindings: run.bindings.clone(),
        })?;
        Ok(self.state.plan_runs[&run.id].clone())
    }
}
/// Invoked by every request release, including lower-level HTTP review routes.
/// A prepared plan cannot silently become a different artifact or destination.
pub(crate) fn enforce_request(state: &State, request: &ReviewRequest) -> Result<(), Error> {
    for run in state
        .plan_runs
        .values()
        .filter(|run| run.request == request.id)
    {
        if run.stage != RunStage::AwaitingReview {
            return Err(Error::Conflict);
        }
        let record = state.plans.get(&run.plan_id).ok_or(Error::Conflict)?;
        if record.plan.digest() != run.plan_digest
            || !analyze(&state.evidence, &record.plan, &run.operating_agent).ready_to_generate
        {
            return Err(Error::Invalid(
                "the current plan or source permissions no longer permit this run".into(),
            ));
        }
        for step in &record.plan.steps {
            match &step.operation {
                Op::Read { source } => {
                    if state.evidence.sources.get(source).map(|s| &s.artifact)
                        != run.bindings.get(&step.id)
                    {
                        return Err(Error::Conflict);
                    }
                }
                Op::Generate { inputs, .. } => {
                    let artifact = run
                        .bindings
                        .get(&step.id)
                        .and_then(|id| state.evidence.artifacts.get(id))
                        .ok_or(Error::Conflict)?;
                    let Origin::Transformation(id) = &artifact.origin else {
                        return Err(Error::Conflict);
                    };
                    let t = state
                        .evidence
                        .transformations
                        .get(id)
                        .ok_or(Error::Conflict)?;
                    let expected: Vec<_> = inputs
                        .iter()
                        .map(|i| run.bindings.get(i).ok_or(Error::Conflict))
                        .collect::<Result<_, _>>()?;
                    if t.manifest.provider != "plan-local-fixture-v1"
                        || !t.manifest.complete
                        || t.manifest
                            .inputs
                            .iter()
                            .map(|i| &i.artifact)
                            .collect::<Vec<_>>()
                            != expected
                    {
                        return Err(Error::Conflict);
                    }
                }
                Op::Publish {
                    artifact, audience, ..
                } => {
                    if run.bindings.get(artifact) != Some(request.artifact())
                        || audience != &request.audience
                    {
                        return Err(Error::Invalid("the review output or destination differs from the prepared plan; start a new run".into()));
                    }
                }
                Op::Approve { .. } => {}
            }
        }
    }
    Ok(())
}

/// Lower-level approval/publication endpoints share the same plan boundary.
pub(crate) fn enforce_intent(state: &State, intent: &Intent) -> Result<(), Error> {
    let mut seen = std::collections::BTreeSet::new();
    let mut pending = vec![intent.artifact.clone()];
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(Artifact {
            origin: Origin::Transformation(t),
            ..
        }) = state.evidence.artifacts.get(&id)
            && let Some(t) = state.evidence.transformations.get(t)
        {
            pending.extend(t.manifest.inputs.iter().map(|i| i.artifact.clone()));
        }
    }
    for run in state.plan_runs.values() {
        let record = state.plans.get(&run.plan_id).ok_or(Error::Conflict)?;
        let generated = record
            .plan
            .steps
            .iter()
            .filter(|s| matches!(s.operation, Op::Generate { .. }))
            .map(|s| {
                run.bindings
                    .get(&s.id)
                    .cloned()
                    .unwrap_or_else(|| generated_id(&run.id, &s.id))
            });
        if !generated.into_iter().any(|id| seen.contains(&id)) {
            continue;
        }
        if run.stage != RunStage::AwaitingReview {
            return Err(Error::Conflict);
        }
        let request = state.requests.get(&run.request).ok_or(Error::Conflict)?;
        if request.artifact() != &intent.artifact || request.audience != intent.audience {
            return Err(Error::Invalid(
                "the proposed action differs from the prepared plan".into(),
            ));
        }
        enforce_request(state, request)?;
    }
    Ok(())
}

fn generated_id(run: &str, step: &str) -> ArtifactId {
    ArtifactId(format!(
        "plan-{run}-{}",
        &digest::structured("plan-step-key", &step).0[..12]
    ))
}
fn review_title(goal: &str) -> String {
    if goal.len() <= 160 {
        return goal.into();
    }
    let end = goal
        .char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .take_while(|end| *end <= 157)
        .last()
        .unwrap_or(0);
    format!("{}…", &goal[..end])
}
pub(crate) fn reduce(state: &mut State, event: &Event) -> Result<(), Error> {
    match event {
        Event::PlanProposed { record } => {
            if state.plans.contains_key(&record.plan.id) {
                return Err(Error::Conflict);
            }
            state.plans.insert(record.plan.id.clone(), *record.clone());
        }
        Event::PlanRunReserved { run } => {
            if state.plan_runs.contains_key(&run.id)
                || run.stage != RunStage::Preparing
                || !state
                    .plans
                    .get(&run.plan_id)
                    .is_some_and(|p| p.plan.digest() == run.plan_digest)
            {
                return Err(Error::Conflict);
            }
            state.plan_runs.insert(run.id.clone(), *run.clone());
        }
        Event::PlanRunPrepared { id, bindings } => {
            let run = state.plan_runs.get_mut(id).ok_or(Error::NotFound)?;
            if run.stage != RunStage::Preparing || !state.requests.contains_key(&run.request) {
                return Err(Error::Conflict);
            }
            run.bindings = bindings.clone();
            run.stage = RunStage::AwaitingReview;
        }
        _ => return Err(Error::Conflict),
    }
    Ok(())
}
