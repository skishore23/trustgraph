use crate::api::{Shared, now, with_world};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use trustgraph_core::*;
use trustgraph_runtime::{Error, policy};
pub(crate) fn routes() -> Router<Shared> {
    Router::new()
        .route("/api/policies", get(list).post(propose))
        .route("/api/policies/validate", post(validate))
        .route("/api/policies/{id}/test", post(test))
        .route("/api/policies/{id}/activate", post(activate))
}
fn allowed(w: &crate::api::World, actor: &PrincipalId) -> Result<(), Error> {
    if !["maya", "arun", "agent"].contains(&actor.0.as_str())
        || !policy::can_inspect(&w.state().evidence, actor)
    {
        Err(Error::Unauthorized)
    } else {
        Ok(())
    }
}
async fn list(State(shared): State<Shared>, headers: HeaderMap) -> Response {
    with_world(&shared, &headers, |w, actor| {
        allowed(w, &actor)?;
        Ok(Json(
            json!({"versions":w.state().policy_versions.values().collect::<Vec<_>>(),"active":w.state().evidence.policy,"can_activate":actor.0==policy::administrator(&w.state().evidence),"administrator":policy::administrator(&w.state().evidence),"rules":policy::RULES,"template":if w.state().evidence.workspace.0=="bank"{Some(include_str!("../../../examples/bank-policy.yaml"))}else{None},"test_suite_available":w.state().evidence.workspace.0=="bank","authority_note":"Policy administrators may see policy references, but source-content access is checked separately. Packages cannot exceed source/repository authority from workspace initialization."}),
        ))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    yaml: String,
}
async fn propose(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Proposal>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        allowed(w, &actor)?;
        Ok(Json(
            json!({"version":w.propose_policy(&actor,&body.yaml,now())?}),
        ))
    })
}
async fn validate(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Proposal>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        allowed(w, &actor)?;
        let spec = policy::parse(&body.yaml)?;
        let snapshot = policy::compile(
            &w.state().evidence,
            &spec,
            &w.state().source_authority,
            &w.state().destination_authority,
        )?;
        Ok(Json(
            json!({"valid":true,"spec":spec,"candidate":snapshot,"activated":false}),
        ))
    })
}
async fn test(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        allowed(w, &actor)?;
        let version = w.state().policy_versions.get(&id).ok_or(Error::NotFound)?;
        Ok(Json(
            json!({"version":id,"digest":version.digest,"report":policy::test_policy(&w.state().evidence,&version.spec,&w.state().source_authority,&w.state().destination_authority)?}),
        ))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Activation {
    expected_revision: u64,
}
async fn activate(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Activation>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        w.activate_policy(&actor, &id, body.expected_revision)?;
        Ok(Json(
            json!({"active":w.state().evidence.policy,"old_approvals":"cannot match the new policy/ACL/audience snapshot"}),
        ))
    })
}
