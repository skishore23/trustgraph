//! A sign-in-free presentation surface with only fictional data and a local publisher.
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use trustgraph_runtime::{
    Error,
    story::{Command, Scenario, Story, StoryPolicy},
};

type Shared = Arc<Mutex<Story>>;
pub fn router() -> Result<Router, Error> {
    Ok(Router::new()
        .route("/", get(index))
        .route(
            "/story.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/story.css"),
                )
            }),
        )
        .route(
            "/story.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/story.js"),
                )
            }),
        )
        .route("/api/story", get(view).post(command))
        .route("/api/story/compare", post(compare))
        .route("/api/story/reset", post(reset))
        .with_state(Arc::new(Mutex::new(Story::new()?))))
}
async fn index() -> Response {
    ([(header::CONTENT_SECURITY_POLICY,"default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'"),(header::CACHE_CONTROL,"no-store")],Html(include_str!("../../../web/story.html"))).into_response()
}
fn access(headers: &HeaderMap) -> Result<(), Error> {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    if !host.starts_with("127.0.0.1:") && !host.starts_with("localhost:") {
        return Err(Error::Unauthorized);
    }
    if let Some(origin) = headers.get(header::ORIGIN)
        && origin.to_str().ok() != Some(format!("http://{host}").as_str())
    {
        return Err(Error::Unauthorized);
    }
    Ok(())
}
fn respond(result: Result<Value, Error>) -> Response {
    match result {
        Ok(value) => ([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response(),
        Err(e) => {
            let status = match e {
                Error::Conflict => StatusCode::CONFLICT,
                Error::Unauthorized => StatusCode::FORBIDDEN,
                Error::Invalid(_) => StatusCode::BAD_REQUEST,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            };
            (status, Json(json!({"error":e.to_string()}))).into_response()
        }
    }
}
async fn view(State(s): State<Shared>, headers: HeaderMap) -> Response {
    respond((|| {
        access(&headers)?;
        Ok(s.lock().map_err(|_| Error::RecoveryRequired)?.view())
    })())
}
async fn command(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(command): Json<Command>,
) -> Response {
    respond((|| {
        access(&headers)?;
        s.lock()
            .map_err(|_| Error::RecoveryRequired)?
            .execute(command)
    })())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Comparison {
    policy: StoryPolicy,
    scenario: Scenario,
}
async fn compare(
    State(s): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Comparison>,
) -> Response {
    respond((|| {
        access(&headers)?;
        s.lock()
            .map_err(|_| Error::RecoveryRequired)?
            .compare(&body.policy, &body.scenario)
    })())
}
async fn reset(State(s): State<Shared>, headers: HeaderMap) -> Response {
    respond((|| {
        access(&headers)?;
        let mut story = s.lock().map_err(|_| Error::RecoveryRequired)?;
        *story = Story::new()?;
        Ok(story.view())
    })())
}
