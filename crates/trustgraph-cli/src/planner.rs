use crate::api::{Shared, World, now, with_world};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use trustgraph_core::{plan::*, *};
use trustgraph_runtime::*;

pub(crate) fn seed(w: &mut World, at: u64) -> Result<(), Error> {
    if !w.state().plans.contains_key("weekly-launch-update") {
        w.propose_plan(
            &"agent".into(),
            launch_plan("weekly-launch-update", true, "company".into()),
            at,
        )?;
    }
    Ok(())
}
pub(crate) fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/plans", get(list).post(propose))
        .route("/api/plans/{id}", get(detail))
        .route("/api/plans/{id}/repair", post(repair))
        .route("/api/plans/{id}/prepare", post(prepare))
        .route("/api/verification", post(check))
}
fn view(w: &World, record: &PlanRecord, actor: &PrincipalId, full: bool) -> Value {
    let report = analyze(&w.state().evidence, &record.plan, &record.submitted_by);
    let mut candidates: Vec<_> = w
        .state()
        .plan_runs
        .values()
        .filter(|r| r.plan_id == record.plan.id)
        .collect();
    candidates.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let runs:Vec<_>=candidates.into_iter().map(|run|json!({"run":run,"review_status":w.review_request(actor,&run.request).ok().map(|r|if let Some(op)=r.operation.as_ref().and_then(|id|w.state().operations.get(id)){match op.state{OperationState::Published=>"released",OperationState::OutcomeUnknown=>"outcome_unknown",OperationState::FailedConfirmed=>"release_failed",_=>"dispatching"}}else if r.changes_requested{"changes_requested"}else{"awaiting_review"})})).collect();

    let mut result = json!({"record":record,"report":report,"runs":runs});
    if full {
        result["repairs"] = json!(repairs(
            &w.state().evidence,
            &record.plan,
            &record.submitted_by
        ));
        result["audiences"] = json!(w.state().evidence.audiences.values().collect::<Vec<_>>());
        result["source_artifacts"] = json!(
            record
                .plan
                .steps
                .iter()
                .filter_map(|s| if let Op::Read { source } = &s.operation {
                    w.state()
                        .evidence
                        .sources
                        .get(source)
                        .and_then(|s| w.state().evidence.artifacts.get(&s.artifact))
                } else {
                    None
                })
                .collect::<Vec<_>>()
        );
    }
    result
}
async fn list(State(shared): State<Shared>, headers: HeaderMap) -> Response {
    with_world(&shared, &headers, |w, actor| {
        Ok(Json(
            json!({"plans":w.state().plans.values().filter(|r|w.read_plan(&actor,&r.plan.id).is_ok()).map(|r|view(w,r,&actor,false)).collect::<Vec<_>>()}),
        ))
    })
}
async fn detail(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        Ok(Json(view(w, w.read_plan(&actor, &id)?, &actor, true)))
    })
}
async fn propose(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(plan): Json<Plan>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let record = w.propose_plan(&actor, plan, now())?;
        Ok(Json(view(w, &record, &actor, true)))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairInput {
    kind: RepairKind,
}
async fn repair(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<RepairInput>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let record = w.repair_plan(&actor, &id, body.kind, now())?;
        Ok(Json(view(w, &record, &actor, true)))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareInput {
    plan_digest: digest::Digest,
    snapshot_digest: digest::Digest,
    key: String,
}
async fn prepare(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<PrepareInput>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let run = w.prepare_plan(
            &actor,
            &id,
            &body.plan_digest,
            &body.snapshot_digest,
            &body.key,
            now(),
        )?;
        Ok(Json(json!({"run":run})))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckInput {
    max_depth: u8,
    broken_rule: bool,
}
async fn check(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<CheckInput>,
) -> Response {
    with_world(&shared, &headers, |_, _| {
        let report = trustgraph_sim::checker::check(
            body.max_depth,
            if body.broken_rule {
                trustgraph_sim::checker::Rule::ReuseApprovalMutant
            } else {
                trustgraph_sim::checker::Rule::Production
            },
        )
        .map_err(Error::Invalid)?;
        Ok(Json(
            serde_json::to_value(report).expect("model report is serializable"),
        ))
    })
}
