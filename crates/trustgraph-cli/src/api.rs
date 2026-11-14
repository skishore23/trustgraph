use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::Path as FilePath,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use trustgraph_core::{digest::Digest, *};
use trustgraph_runtime::{journal::DiskJournal, publisher::LocalPublisher, *};

pub(crate) type World = Coordinator<DiskJournal, LocalPublisher>;
pub(crate) type Shared = Arc<Mutex<World>>;
pub(crate) type ApiResult = Result<Json<Value>, ApiError>;

pub fn router(directory: &FilePath) -> Result<Router, Error> {
    build_router(directory, "northstar")
}
pub fn consulting_router(directory: &FilePath) -> Result<Router, Error> {
    build_router(directory, "consulting")
}
pub fn bank_router(directory: &FilePath) -> Result<Router, Error> {
    build_router(directory, "bank")
}
fn build_router(directory: &FilePath, scenario: &str) -> Result<Router, Error> {
    // Acquire ownership before opening the simulated provider's private ledger.
    let journal = DiskJournal::open(directory)?;
    let publisher = LocalPublisher::open(directory.join("publisher"))?;
    let genesis = match scenario {
        "bank" => fixtures::bank(),
        "consulting" => fixtures::consulting(),
        _ => fixtures::northstar(),
    };
    let expected_workspace = genesis.workspace.clone();
    let mut coordinator = Coordinator::open_with_evidence(journal, publisher, genesis)?;
    if coordinator.state().evidence.workspace != expected_workspace {
        return Err(Error::Invalid(
            "this directory belongs to a different scenario; select a separate data directory"
                .into(),
        ));
    }
    if scenario == "northstar" {
        crate::inbox::seed(&mut coordinator, now())?;
        crate::planner::seed(&mut coordinator, now())?;
    }
    if scenario == "bank"
        && !coordinator
            .state()
            .policy_versions
            .contains_key("bank-baseline-v1")
    {
        coordinator.propose_policy(
            &"agent".into(),
            include_str!("../../../examples/bank-policy.yaml"),
            now(),
        )?;
    }
    let state = Arc::new(Mutex::new(coordinator));
    let config = Arc::new(
        json!({"mcpServers":{"trustgraph":{"command":std::env::current_exe().map_err(|_|Error::Storage("cannot locate executable".into()))?.to_string_lossy(),"args":["mcp","--url","http://127.0.0.1:8080/mcp","--token-file",std::fs::canonicalize(directory).map_err(|_|Error::Storage("cannot locate data directory".into()))?.join("mcp-token").to_string_lossy()]}}}),
    );
    let app = Router::new()
        .route("/", get(index))
        .merge(crate::inbox::routes())
        .merge(crate::planner::routes())
        .merge(crate::scenario::routes())
        .merge(crate::policies::routes())
        .merge(crate::showcase::routes()?)
        .route(
            "/showcase.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/showcase.js"),
                )
            }),
        )
        .route(
            "/showcase.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/showcase.css"),
                )
            }),
        )
        .route(
            "/policies.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/policies.js"),
                )
            }),
        )
        .route(
            "/simulator.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/simulator.js"),
                )
            }),
        )
        .route(
            "/simulator.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/simulator.css"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/app.js"),
                )
            }),
        )
        .route(
            "/plans.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/plans.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/style.css"),
                )
            }),
        )
        .route("/api/state", get(state_view))
        .route(
            "/api/mcp-config",
            get(mcp_config).layer(axum::Extension(config)),
        )
        .route("/api/drafts/generate", post(generate))
        .route("/api/artifacts/{id}/versions", post(edit))
        .route("/api/artifacts/{id}/lineage", get(lineage_view))
        .route("/api/verify", post(verify_intent))
        .route("/api/review", post(review))
        .route("/api/approvals", post(approve))
        .route("/api/approvals/{id}/revoke", post(revoke))
        .route("/api/publish", post(publish))
        .route("/api/operations/{key}", get(operation_view))
        .route("/api/operations/{key}/reconcile", post(reconcile))
        .route("/api/operations/{key}/bundle", get(bundle_view))
        .with_state(state);
    let mcp = crate::mcp::routes(app.clone(), directory)?;
    Ok(app.merge(mcp))
}
async fn mcp_config(
    State(shared): State<Shared>,
    headers: HeaderMap,
    axum::Extension(config): axum::Extension<Arc<Value>>,
) -> Response {
    with_world(&shared, &headers, |_, _| {
        let mut config = (*config).clone();
        config["mcpServers"]["trustgraph"]["args"][2] = json!(format!(
            "http://{}/mcp",
            headers[header::HOST].to_str().unwrap_or("127.0.0.1:8080")
        ));
        Ok(Json(config))
    })
}
async fn index() -> Response {
    ([(header::CONTENT_SECURITY_POLICY, "default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'"), (header::CACHE_CONTROL, "no-store")], Html(include_str!("../../../web/index.html"))).into_response()
}
pub(crate) struct ApiError(Error);
impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            Error::Unauthorized => StatusCode::FORBIDDEN,
            Error::NotFound => StatusCode::NOT_FOUND,
            Error::Conflict => StatusCode::CONFLICT,
            Error::Invalid(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        (status, Json(json!({"error": self.0.to_string()}))).into_response()
    }
}
fn actor(headers: &HeaderMap, w: &World) -> Result<PrincipalId, (StatusCode, &'static str)> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !(host.starts_with("127.0.0.1:") || host.starts_with("localhost:")) {
        return Err((StatusCode::FORBIDDEN, "loopback host required"));
    }
    if let Some(origin) = headers.get(header::ORIGIN)
        && origin.to_str().ok() != Some(format!("http://{host}").as_str())
    {
        return Err((StatusCode::FORBIDDEN, "same origin required"));
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer demo-"));
    let Some(id) = token
        .filter(|id| ["maya", "arun", "sam", "agent"].contains(id))
        .map(PrincipalId::from)
    else {
        return Err((StatusCode::UNAUTHORIZED, "select a synthetic demo identity"));
    };
    if !w
        .state()
        .evidence
        .principals
        .get(&id)
        .is_some_and(|p| p.enabled)
    {
        return Err((StatusCode::FORBIDDEN, "identity disabled"));
    }
    Ok(id)
}
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_secs()
}
pub(crate) fn with_world(
    shared: &Shared,
    headers: &HeaderMap,
    work: impl FnOnce(&mut World, PrincipalId) -> ApiResult,
) -> Response {
    let Ok(mut world) = shared.lock() else {
        return ApiError(Error::RecoveryRequired).into_response();
    };
    let id = match actor(headers, &world) {
        Ok(id) => id,
        Err((status, message)) => return (status, Json(json!({"error":message}))).into_response(),
    };
    match work(&mut world, id) {
        Ok(value) => ([(header::CACHE_CONTROL, "no-store")], value).into_response(),
        Err(error) => error.into_response(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generate {
    restricted: bool,
    key: String,
}
async fn generate(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Generate>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        Ok(Json(
            json!({"artifact": w.generate(&actor, body.restricted, &body.key)?}),
        ))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    content: String,
    key: String,
}
async fn edit(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Edit>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        Ok(Json(
            json!({"artifact": w.edit(&actor, &ArtifactId(id), &body.content, &body.key)?}),
        ))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    artifact: ArtifactId,
    audience: AudienceId,
}
impl Candidate {
    fn intent(&self, actor: PrincipalId) -> Intent {
        Intent {
            artifact: self.artifact.clone(),
            audience: self.audience.clone(),
            requester: actor,
        }
    }
}
async fn verify_intent(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Candidate>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        Ok(Json(
            serde_json::to_value(verify(&w.state().evidence, &body.intent(actor), now())).unwrap(),
        ))
    })
}
async fn review(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Candidate>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let mut e = w.state().evidence.clone();
        e.approvals.clear();
        let decision = verify(&e, &body.intent(actor), now());
        let Some(context) = decision.context() else {
            return Ok(Json(json!({"decision":decision})));
        };
        Ok(Json(
            json!({"context": context, "context_digest": context.digest(), "content":e.artifacts[&body.artifact].content}),
        ))
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bound {
    artifact: ArtifactId,
    audience: AudienceId,
    context_digest: Digest,
    key: String,
}
impl Bound {
    fn intent(&self, actor: PrincipalId) -> Intent {
        Intent {
            artifact: self.artifact.clone(),
            audience: self.audience.clone(),
            requester: actor,
        }
    }
}
async fn approve(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Bound>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        Ok(Json(
            json!({"approval":w.approve(&actor, &body.intent(actor.clone()), &body.context_digest, &body.key, now())?}),
        ))
    })
}
async fn revoke(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        w.revoke(&actor, &ApprovalId(id))?;
        Ok(Json(json!({"revoked":true})))
    })
}
fn op_summary(op: &Operation) -> Value {
    json!({"key":op.key, "intent":op.intent, "state":op.state, "context":op.context, "receipt":op.receipt, "bundle_digest":op.bundle.digest})
}
async fn publish(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Bound>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let result = w.publish(&body.intent(actor), &body.context_digest, &body.key, now())?;
        Ok(Json(match result {
            PublishResult::Blocked { decision } => json!({"result":"BLOCKED", "decision":decision}),
            PublishResult::Operation { operation } => {
                json!({"result":"OPERATION", "operation":op_summary(&operation)})
            }
        }))
    })
}
fn visible_operation<'a>(
    w: &'a World,
    actor: &PrincipalId,
    key: &str,
) -> Result<&'a Operation, Error> {
    let op = w.state().operations.get(key).ok_or(Error::NotFound)?;
    if !lineage::can_read(&w.state().evidence, actor, &op.intent.artifact) {
        return Err(Error::NotFound);
    }
    Ok(op)
}
async fn operation_view(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        Ok(Json(op_summary(visible_operation(w, &actor, &key)?)))
    })
}
async fn bundle_view(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        Ok(Json(
            serde_json::to_value(&visible_operation(w, &actor, &key)?.bundle).unwrap(),
        ))
    })
}
async fn reconcile(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        visible_operation(w, &actor, &key)?;
        Ok(Json(op_summary(&w.reconcile(&actor, &key, now())?)))
    })
}
async fn state_view(State(shared): State<Shared>, headers: HeaderMap) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let e = &w.state().evidence;
        let artifacts: Vec<_> = e
            .artifacts
            .values()
            .filter(|a| lineage::can_read(e, &actor, &a.id))
            .collect();
        let approvals: Vec<_> = e
            .approvals
            .values()
            .filter(|a| lineage::can_read(e, &actor, &a.context.artifact))
            .collect();
        let operations: Vec<_> = w
            .state()
            .operations
            .values()
            .filter(|op| lineage::can_read(e, &actor, &op.intent.artifact))
            .map(op_summary)
            .collect();
        Ok(Json(
            json!({"workspace":e.workspace, "actor":actor, "mode":"synthetic-local", "artifacts":artifacts, "audiences":e.audiences.values().collect::<Vec<_>>(), "approvals":approvals, "operations":operations}),
        ))
    })
}
async fn lineage_view(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    with_world(&shared, &headers, |w, actor| {
        let e = &w.state().evidence;
        let root = ArtifactId(id);
        if !lineage::can_read(e, &actor, &root) {
            return Err(Error::NotFound.into());
        }
        let mut pending = vec![root];
        let mut seen = BTreeSet::new();
        let mut artifacts = vec![];
        let mut transformations = vec![];
        while let Some(id) = pending.pop() {
            if !seen.insert(id.clone()) {
                continue;
            }
            let a = &e.artifacts[&id];
            artifacts.push(a);
            if let Origin::Transformation(t) = &a.origin {
                let t = &e.transformations[t];
                transformations.push(t);
                pending.extend(t.manifest.inputs.iter().map(|i| i.artifact.clone()));
            }
        }
        Ok(Json(
            json!({"artifacts":artifacts,"transformations":transformations}),
        ))
    })
}
