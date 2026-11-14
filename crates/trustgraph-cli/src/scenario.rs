//! A business-facing projection of the real plan compiler, not a second policy engine.
use crate::api::{ApiResult, Shared, World, now, with_world};
use axum::{
    Json, Router,
    extract::State,
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use trustgraph_core::{
    digest::{self, Digest},
    plan::*,
    *,
};
use trustgraph_runtime::Error;

const DOCUMENTS: [(&str, &str, &str, &str, &str); 3] = [
    (
        "market-brief",
        "Industrial market outlook",
        "Aster research",
        "Client-shareable",
        "Demand cycles and the diligence questions that matter.",
    ),
    (
        "helios-data-room",
        "Helios diligence pack",
        "Atlas engagement",
        "Deal confidential",
        "$240m offer, $27m EBITDA and a customer renewal risk.",
    ),
    (
        "orion-benchmark",
        "Orion operating benchmark",
        "Previous client · Orion",
        "Internal use only",
        "Private downside assumptions from a prior engagement.",
    ),
];
const BANK_DOCUMENTS: [(&str, &str, &str, &str, &str); 3] = [
    (
        "engineering-requirements",
        "Reconciliation requirements",
        "Engineering",
        "Both departments",
        "Exact minor-unit matching, duplicate protection and tests.",
    ),
    (
        "approved-specification",
        "Approved implementation specification",
        "Engineering + Risk",
        "Both departments",
        "Explicit engineering guidance approved for both teams.",
    ),
    (
        "risk-investigation",
        "Confidential fraud investigation",
        "Fraud & Risk",
        "Risk only",
        "Synthetic investigation; not authorized for Engineering.",
    ),
];
fn documents(
    w: &World,
) -> &'static [(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)] {
    if w.state().evidence.workspace.0 == "bank" {
        &BANK_DOCUMENTS
    } else {
        &DOCUMENTS
    }
}
fn available(w: &World) -> Result<(), Error> {
    if !["aster-advisory", "bank"].contains(&w.state().evidence.workspace.0.as_str()) {
        return Err(Error::NotFound);
    }
    Ok(())
}
fn can_prepare(w: &World, actor: &PrincipalId) -> bool {
    let e = &w.state().evidence;
    e.principals
        .get(actor)
        .is_some_and(|p| p.enabled && (p.role == Role::Agent || e.policy.approvers.contains(actor)))
}
fn source_options(w: &World, actor: &PrincipalId) -> Vec<Value> {
    let e = &w.state().evidence;
    documents(w).iter().filter(|(id, ..)| e.sources.get(&SourceId::from(*id)).is_some_and(|s| lineage::can_read(e, actor, &s.artifact)))
        .map(|(id, title, owner, classification, description)| json!({"id":id,"title":title,"owner":owner,"classification":classification,"description":description})).collect()
}
pub(crate) fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/scenario", get(scenario))
        .route("/api/scenario/simulate", post(simulate))
        .route("/api/scenario/prepare", post(prepare))
}
async fn scenario(State(shared): State<Shared>, headers: HeaderMap) -> Response {
    with_world(&shared, &headers, |w, actor| {
        available(w)?;
        let options = source_options(w, &actor);
        let e = &w.state().evidence;
        if e.workspace.0 == "bank" {
            let audiences:Vec<_>=e.audiences.values().map(|a|json!({"id":a.id,"title":if a.id.0=="engineering-repo"{"Engineering repository"}else{"Risk review room"},"description":if a.id.0=="engineering-repo"{"Patch and tests for Engineering"}else{"Restricted Risk discussion"},"recipients":a.recipients})).collect();
            return Ok(Json(
                json!({"kind":"bank","title":"Payment reconciliation change","client":"BANK MODERNIZATION","engagement":"ENGINEERING + RISK","description":"One agent works for both departments. Test whether its proposed patch may enter Engineering’s repository.","fictional":true,"sources":options,"audiences":audiences,"people":{"maya":{"name":"Maya","role":"Engineering reviewer","department":"Engineering"},"arun":{"name":"Arun","role":"Risk owner · policy admin","department":"Risk"},"sam":{"name":"Sam","role":"Engineering developer","department":"Engineering"},"agent":{"name":"Coding agent","role":"Works in both scopes","department":"Agent"}},"defaults":{"sources":options.iter().map(|s|s["id"].clone()).collect::<Vec<_>>(),"audience":"engineering-repo"},"can_prepare":can_prepare(w,&actor),"generation":"local_fixture"}),
            ));
        }
        let audiences: Vec<_> = e.audiences.values().map(|a| json!({"id":a.id,"title":if a.id.0=="atlas-committee" {"Atlas investment committee"} else {"Aster engagement team"},"description":if a.id.0=="atlas-committee" {"Client-facing decision memo"} else {"Internal partner discussion"},"recipients":a.recipients})).collect();
        Ok(Json(
            json!({"title":"Should Atlas commit $240m?","client":"Atlas Capital","engagement":"Helios acquisition","description":"Aster Advisory must prepare an acquisition recommendation. Test whether the evidence can reach the people making the decision.","fictional":true,"sources":options,"audiences":audiences,"people":{"maya":{"name":"Maya","role":"Engagement partner"},"arun":{"name":"Arun","role":"Deal manager"},"sam":{"name":"Sam","role":"Atlas CFO · client"},"agent":{"name":"Diligence agent","role":"Prepares the memo"}},"defaults":{"sources":options.iter().map(|s|s["id"].clone()).collect::<Vec<_>>(),"audience":"atlas-committee"},"can_prepare":can_prepare(w,&actor)}),
        ))
    })
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    sources: Vec<SourceId>,
    audience: AudienceId,
}

fn build(w: &World, actor: &PrincipalId, selection: &Selection) -> Result<Plan, Error> {
    available(w)?;
    // Missing and inaccessible references have the same response. Do not let
    // source-selection errors become an existence oracle for private documents.
    let e = &w.state().evidence;
    for source in &selection.sources {
        if !documents(w).iter().any(|(id, ..)| *id == source.0)
            || !e
                .sources
                .get(source)
                .is_some_and(|s| lineage::can_read(e, actor, &s.artifact))
        {
            return Err(Error::NotFound);
        }
    }
    let ids: BTreeSet<_> = selection.sources.iter().cloned().collect();
    if ids.is_empty() || ids.len() > 3 || ids.len() != selection.sources.len() {
        return Err(Error::Invalid(
            "select one or more available evidence documents, without duplicates".into(),
        ));
    }
    if !e.audiences.contains_key(&selection.audience) {
        return Err(Error::Invalid("select a configured destination".into()));
    }
    let prompt = e
        .sources
        .get(&SourceId::from("task-prompt"))
        .ok_or(Error::NotFound)?;
    if !lineage::can_read(e, actor, &prompt.artifact) {
        return Err(Error::NotFound);
    }
    let mut steps: Vec<_> = ids
        .into_iter()
        .chain([SourceId::from("task-prompt")])
        .map(|source| Step {
            id: format!("read-{}", source.0),
            operation: Op::Read { source },
        })
        .collect();
    let inputs = steps.iter().map(|s| s.id.clone()).collect();
    steps.push(Step {
        id: "memo".into(),
        operation: Op::Generate {
            inputs,
            provider: Provider::LocalFixture,
        },
    });
    steps.push(Step {
        id: "partner-review".into(),
        operation: Op::Approve {
            artifact: "memo".into(),
            audience: selection.audience.clone(),
        },
    });
    steps.push(Step {
        id: "deliver".into(),
        operation: Op::Publish {
            artifact: "memo".into(),
            audience: selection.audience.clone(),
            approval: "partner-review".into(),
        },
    });
    let mut plan = Plan {
        id: "acquisition-memo".into(),
        goal: "Review the Helios acquisition recommendation for Atlas Capital".into(),
        steps,
    };
    if e.workspace.0 == "bank" {
        plan.id = "reconciliation-change".into();
        plan.goal = "Implement and test reconciliation; prepare a patch for human review".into();
    }
    plan.id = format!("deal-{}", &plan.digest().0[..32]);
    Ok(plan)
}
fn preview(w: &World, actor: &PrincipalId, selection: &Selection) -> ApiResult {
    let plan = build(w, actor, selection)?;
    let e = &w.state().evidence;
    let report = analyze(e, &plan, &"agent".into());
    let audience = &e.audiences[&selection.audience];
    let mut conflicts = vec![];
    let mut matrix = vec![];
    // Includes the immutable recorded instruction: its permissions also propagate.
    let visible = source_options(w, actor);
    for id in visible
        .iter()
        .filter_map(|s| s["id"].as_str())
        .chain(["task-prompt"])
    {
        let source = &e.sources[&SourceId::from(id)];
        let readers = lineage::derived_readers(e, &source.artifact)
            .map_err(|_| Error::Invalid("source evidence is incomplete".into()))?;
        let selected = id == "task-prompt" || selection.sources.contains(&SourceId::from(id));
        let cells: Vec<_> = audience
            .recipients
            .iter()
            .map(|recipient| {
                let allowed = readers.contains(recipient);
                if selected && !allowed {
                    conflicts.push(json!({"source":id,"recipient":recipient}));
                }
                json!({"recipient":recipient,"allowed":allowed})
            })
            .collect();
        matrix.push(
            json!({"source":id,"selected":selected,"instruction":id=="task-prompt","cells":cells}),
        );
    }
    let key = run_key(&report);
    let run_id = format!(
        "run-{}",
        &digest::structured("plan-run-key", &(&plan.id, &key)).0[..32]
    );
    let prepared_review = w.state().plan_runs.get(&run_id).and_then(|run| {
        w.review_request(actor, &run.request).ok().map(|r| {
            let status = if let Some(op) = r
                .operation
                .as_ref()
                .and_then(|id| w.state().operations.get(id))
            {
                match op.state {
                    trustgraph_runtime::OperationState::Published => "released",
                    trustgraph_runtime::OperationState::OutcomeUnknown => "outcome_unknown",
                    _ => "release_stopped",
                }
            } else if r.changes_requested {
                "changes_requested"
            } else {
                "awaiting_review"
            };
            json!({"request":r.id,"status":status})
        })
    });
    Ok(Json(
        json!({"plan":plan,"report":report,"matrix":matrix,"conflicts":conflicts,"audience":audience,"prepared_review":prepared_review}),
    ))
}
fn run_key(report: &Report) -> String {
    format!(
        "scenario-{}",
        &digest::structured(
            "scenario-run",
            &(&report.plan_digest, &report.snapshot_digest)
        )
        .0[..32]
    )
}
async fn simulate(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(selection): Json<Selection>,
) -> Response {
    with_world(&shared, &headers, |w, actor| preview(w, &actor, &selection))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    selection: Selection,
    plan_digest: Digest,
    snapshot_digest: Digest,
}
async fn prepare(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Prepare>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        available(w)?;
        if !can_prepare(w, &actor) {
            return Err(Error::Unauthorized.into());
        }
        let plan = build(w, &actor, &body.selection)?;
        let report = analyze(&w.state().evidence, &plan, &"agent".into());
        if !report.ready_to_generate {
            return Err(Error::Invalid(
                "the selected evidence cannot reach this audience; choose a permitted alternative"
                    .into(),
            )
            .into());
        }
        if report.plan_digest != body.plan_digest || report.snapshot_digest != body.snapshot_digest
        {
            return Err(Error::Conflict.into());
        }
        let key = run_key(&report);
        let id = plan.id.clone();
        w.propose_plan(&"agent".into(), plan, now())?;
        let run = w.prepare_plan(
            &actor,
            &id,
            &body.plan_digest,
            &body.snapshot_digest,
            &key,
            now(),
        )?;
        Ok(Json(json!({"run":run})))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use std::sync::{Arc, Mutex};
    use trustgraph_runtime::{
        Coordinator, fixtures, journal::DiskJournal, publisher::LocalPublisher,
    };

    #[tokio::test]
    async fn permission_snapshot_drift_rejects_stale_prepare_and_creates_a_distinct_fresh_run() {
        let temp = tempfile::tempdir().unwrap();
        let journal = DiskJournal::open(temp.path()).unwrap();
        let world = Coordinator::open_with_evidence(
            journal,
            LocalPublisher::default(),
            fixtures::consulting(),
        )
        .unwrap();
        let shared = Arc::new(Mutex::new(world));
        let actor = PrincipalId::from("maya");
        let selection = Selection {
            sources: vec!["market-brief".into(), "helios-data-room".into()],
            audience: "atlas-committee".into(),
        };
        let headers: HeaderMap = [
            ("host", "127.0.0.1:8080"),
            ("authorization", "Bearer demo-maya"),
        ]
        .into_iter()
        .map(|(k, v)| {
            (
                k.parse::<axum::http::HeaderName>().unwrap(),
                v.parse().unwrap(),
            )
        })
        .collect();
        let report = {
            let w = shared.lock().unwrap();
            analyze(
                &w.state().evidence,
                &build(&w, &actor, &selection).unwrap(),
                &"agent".into(),
            )
        };
        let make_body = |r: &Report| Prepare {
            selection: selection.clone(),
            plan_digest: r.plan_digest.clone(),
            snapshot_digest: r.snapshot_digest.clone(),
        };
        let first = prepare(
            State(shared.clone()),
            headers.clone(),
            Json(make_body(&report)),
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let first: Value = serde_json::from_slice(
            &axum::body::to_bytes(first.into_body(), 1_000_000)
                .await
                .unwrap(),
        )
        .unwrap();
        {
            let mut w = shared.lock().unwrap();
            w.change_acl(
                &actor,
                &"market-brief".into(),
                fixtures::readers(&["maya", "arun", "sam", "agent"]),
                "renewed-source-rights",
            )
            .unwrap();
        }
        let stale = prepare(
            State(shared.clone()),
            headers.clone(),
            Json(make_body(&report)),
        )
        .await;
        assert_eq!(stale.status(), StatusCode::CONFLICT);
        assert_eq!(shared.lock().unwrap().state().plan_runs.len(), 1);
        let fresh = {
            let w = shared.lock().unwrap();
            analyze(
                &w.state().evidence,
                &build(&w, &actor, &selection).unwrap(),
                &"agent".into(),
            )
        };
        assert_ne!(fresh.snapshot_digest, report.snapshot_digest);
        let second = prepare(State(shared.clone()), headers, Json(make_body(&fresh))).await;
        assert_eq!(second.status(), StatusCode::OK);
        let second: Value = serde_json::from_slice(
            &axum::body::to_bytes(second.into_body(), 1_000_000)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_ne!(first["run"]["id"], second["run"]["id"]);
        assert_eq!(shared.lock().unwrap().state().plan_runs.len(), 2);
    }
}
