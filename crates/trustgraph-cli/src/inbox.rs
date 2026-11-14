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
use trustgraph_core::*;
use trustgraph_runtime::*;

pub(crate) fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/requests", get(list).post(submit))
        .route("/api/requests/{id}", get(detail))
        .route("/api/requests/{id}/changes", post(changes))
        .route("/api/requests/{id}/revisions", post(revise))
        .route("/api/requests/{id}/release", post(release))
        .route("/api/workspace", get(workspace))
}

pub(crate) fn seed(w: &mut World, at: u64) -> Result<(), Error> {
    let actor = PrincipalId::from("agent");
    for (key, title, summary, restricted, audience) in [
        (
            "sample-launch",
            "Share the Northstar launch announcement",
            "A company-wide announcement covering launch timing and the new workspace.",
            true,
            "company",
        ),
        (
            "sample-product",
            "Post a product team launch preview",
            "A preview of the launch plan for the Product team, including timing and pricing.",
            true,
            "product",
        ),
        (
            "sample-roadmap",
            "Share the roadmap update",
            "A short update on the collaborative planning workspace for everyone at Northstar.",
            false,
            "company",
        ),
    ] {
        if w.state().requests.contains_key(key) {
            continue;
        }
        let artifact = w.generate(&actor, restricted, key)?;
        w.submit_request(
            &actor,
            Submission {
                key: key.into(),
                title: title.into(),
                summary: summary.into(),
                artifact,
                audience: audience.into(),
            },
            at,
        )?;
    }
    if !w.state().requests.contains_key("sample-revised") {
        let parent = w.generate(&actor, false, "sample-revised-original")?;
        let intent = Intent {
            artifact: parent.clone(),
            audience: "company".into(),
            requester: "maya".into(),
        };
        let mut e = w.state().evidence.clone();
        e.approvals.clear();
        let context = verify(&e, &intent, at)
            .context()
            .ok_or(Error::Conflict)?
            .digest();
        w.approve(&"maya".into(), &intent, &context, "sample-original", at)?;
        let artifact=w.edit(&actor,&parent,"Northstar is building a collaborative planning workspace. We are bringing shared plans, decisions, and team updates together in one place. More launch details will follow after review.","sample-revised")?;
        w.submit_request(&actor,Submission{key:"sample-revised".into(),title:"Publish the revised launch summary".into(),summary:"The agent revised the approved wording. The latest version needs another review.".into(),artifact,audience:"company".into()},at)?;
    }
    Ok(())
}

fn request_view(w: &World, r: &ReviewRequest, actor: &PrincipalId, full: bool) -> Value {
    let e = &w.state().evidence;
    let intent = Intent {
        artifact: r.artifact().clone(),
        audience: r.audience.clone(),
        requester: actor.clone(),
    };
    let decision = verify(e, &intent, now());
    let op = r
        .operation
        .as_ref()
        .and_then(|key| w.state().operations.get(key));
    let status = if let Some(op) = op {
        match op.state {
            OperationState::Published => "released",
            OperationState::OutcomeUnknown | OperationState::Dispatching => "outcome_unknown",
            _ => "release_failed",
        }
    } else if r.changes_requested {
        "changes_requested"
    } else if matches!(decision, Decision::Blocked { .. }) {
        "needs_attention"
    } else {
        "awaiting_review"
    };
    let audience = &e.audiences[&r.audience];
    let mut output = json!({"request":r,"status":status,"decision":decision,"destination":audience,"sample":r.id.starts_with("sample-"),"artifact_version":e.artifacts[r.artifact()].version});
    if full {
        let mut clean = e.clone();
        clean.approvals.clear();
        let review = verify(&clean, &intent, now());
        output["review_context"] = json!(review.context());
        output["context_digest"] = json!(review.context().map(Context::digest));
        output["artifact"] = json!(e.artifacts[r.artifact()]);
        output["can_review"] =
            json!(e.policy.approvers.contains(actor) && e.principals[actor].role == Role::Human);
        output["operation"] =
            json!(op.map(|op| json!({"key":op.key,"state":op.state,"receipt":op.receipt})));
        let mut pending = vec![r.artifact().clone()];
        let mut seen = std::collections::BTreeSet::new();
        let mut sources = vec![];
        let mut transforms = vec![];
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let a = &e.artifacts[&id];
            match &a.origin {
                Origin::Source(id) => {
                    let s = &e.sources[id];
                    sources.push(json!({"artifact":a,"acl":e.acls[&s.acl],"instruction":id.0=="task-prompt"}));
                }
                Origin::Transformation(id) => {
                    let t = &e.transformations[id];
                    transforms.push(t);
                    pending.extend(t.manifest.inputs.iter().map(|i| i.artifact.clone()));
                }
            }
        }
        output["sources"] = json!(sources);
        output["transformations"] = json!(transforms);
        output["approvals"] = json!(
            e.approvals
                .values()
                .filter(|a| r
                    .revisions
                    .iter()
                    .any(|rev| rev.artifact == a.context.artifact)
                    && a.context.audience.id == r.audience)
                .collect::<Vec<_>>()
        );
    }
    output
}
async fn list(State(shared): State<Shared>, headers: HeaderMap) -> Response {
    with_world(&shared, &headers, |w, actor| {
        Ok(Json(
            json!({"requests":w.visible_requests(&actor).into_iter().map(|r|request_view(w,r,&actor,false)).collect::<Vec<_>>()}),
        ))
    })
}
async fn detail(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let r = w.review_request(&actor, &id)?;
        Ok(Json(request_view(w, r, &actor, true)))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Intake {
    key: String,
    title: String,
    summary: String,
    artifact: ArtifactId,
    audience: AudienceId,
}
async fn submit(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Intake>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let r = w.submit_request(
            &actor,
            Submission {
                key: body.key,
                title: body.title,
                summary: body.summary,
                artifact: body.artifact,
                audience: body.audience,
            },
            now(),
        )?;
        Ok(Json(request_view(w, &r, &actor, true)))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Return {
    revision: u64,
    body: String,
    key: String,
}
async fn changes(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Return>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let r = w.request_changes(&actor, &id, body.revision, &body.body, &body.key, now())?;
        Ok(Json(request_view(w, &r, &actor, true)))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    revision: u64,
    artifact: ArtifactId,
}
async fn revise(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Revision>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let r = w.revise_request(&actor, &id, body.revision, &body.artifact, now())?;
        Ok(Json(request_view(w, &r, &actor, true)))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Release {
    revision: u64,
    context_digest: digest::Digest,
}
async fn release(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Release>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        w.release_request(&actor, &id, body.revision, &body.context_digest, now())?;
        let r = w.review_request(&actor, &id)?;
        Ok(Json(request_view(w, r, &actor, true)))
    })
}
async fn workspace(State(shared): State<Shared>, headers: HeaderMap) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let e = &w.state().evidence;
        Ok(Json(
            json!({"name":if e.workspace.0 == "bank" { "Bank modernization" } else if e.workspace.0 == "aster-advisory" { "Aster Advisory" } else { "Northstar" },"actor":actor,"policy":e.policy,"principals":e.principals.values().collect::<Vec<_>>(),"mode":"local-preview"}),
        ))
    })
}
