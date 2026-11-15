//! Public, stateless demo. Every request replays a bounded fictional story through
//! the real Rust engine. Client history is demo input, never authorization for a
//! live publisher. No credentials, durable state, or external writes are used.
pub mod framework;
pub mod mcp;

use axum::{
    Json, Router,
    extract::DefaultBodyLimit,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde::Deserialize;
use serde_json::{Value, json};
use trustgraph_runtime::{
    Error,
    story::{Command, Scenario, Story, StoryPolicy},
};

const MAX_COMMANDS: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Comparison {
    policy: StoryPolicy,
    scenario: Scenario,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DemoRequest {
    #[serde(default)]
    history: Vec<Command>,
    command: Option<Command>,
    comparison: Option<Comparison>,
}

fn evaluate(request: DemoRequest) -> Result<Value, Error> {
    if request.history.len() + usize::from(request.command.is_some()) > MAX_COMMANDS {
        return Err(Error::Invalid(
            "This demo supports 64 steps per story. Restart the story to continue.".into(),
        ));
    }
    if request.command.is_some() && request.comparison.is_some() {
        return Err(Error::Invalid(
            "Choose one demo operation per request.".into(),
        ));
    }
    let mut story = Story::new()?;
    for command in request.history {
        story.execute(command)?;
    }
    if let Some(command) = request.command {
        story.execute(command)
    } else if let Some(comparison) = request.comparison {
        story.compare(&comparison.policy, &comparison.scenario)
    } else {
        Ok(story.view())
    }
}

async fn demo(Json(request): Json<DemoRequest>) -> Response {
    let result = tokio::task::spawn_blocking(move || evaluate(request)).await;
    let (status, value) = match result {
        Ok(Ok(value)) => (StatusCode::OK, value),
        Ok(Err(error)) => {
            let status = match error {
                Error::Unauthorized => StatusCode::FORBIDDEN,
                Error::Conflict => StatusCode::CONFLICT,
                Error::Invalid(_) => StatusCode::BAD_REQUEST,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            };
            (status, json!({"error": error.to_string()}))
        }
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": "The demo could not complete this step. Please retry."}),
        ),
    };
    (status, [(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
}

pub fn router() -> Router {
    Router::new()
        .route("/api/demo", post(demo))
        .route("/api/framework", post(framework::demo))
        .layer(DefaultBodyLimit::max(131_072))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    async fn request(body: Value) -> (StatusCode, Value) {
        let response = router()
            .oneshot(
                Request::post("/api/demo")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn replay_preserves_boundaries_review_release_and_revision_without_shared_state() {
        let private = json!({"action":"generate","approved_inputs":false,"expected_revision":1});
        let shared = json!({"action":"generate","approved_inputs":true,"expected_revision":1});
        let mut history = vec![private, shared];
        let (status, view) = request(json!({"history":history})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            view["versions"][0]["assessment"]["code"],
            "RESTRICTED_SOURCE"
        );
        assert_eq!(view["current"]["assessment"]["code"], "APPROVAL_REQUIRED");
        let artifact = view["current"]["artifact"]["id"].clone();
        let expected = view["current"]["assessment"]["context_digest"].clone();
        let approve =
            json!({"action":"approve","reviewer":"maya","artifact":artifact,"expected":expected});
        let (status, ready) = request(json!({"history":history,"command":approve})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ready["current"]["assessment"]["code"], "READY");
        history.push(approve);
        let release = json!({"action":"release","artifact":artifact,"expected":expected});
        let (status, released) = request(json!({"history":history,"command":release})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(released["current"]["operation"]["state"], "PUBLISHED");
        assert_eq!(released["external_writes"], 0);
        history.push(release);
        assert_eq!(request(json!({"history":history})).await.1, released);
        let (status, revised) =
            request(json!({"history":history,"command":{"action":"revise","artifact":artifact}}))
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(revised["current"]["assessment"]["code"], "STALE_APPROVAL");
        let (_, fresh_visitor) = request(json!({"history":[]})).await;
        assert!(fresh_visitor["current"].is_null());
        assert_eq!(fresh_visitor["external_writes"], 0);
    }

    #[tokio::test]
    async fn comparisons_are_repeatable_and_do_not_activate_policy() {
        let body = json!({"history":[],"comparison":{"policy":StoryPolicy::default(),"scenario":Scenario::default()}});
        let (status, first) = request(body.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(request(body).await.1, first);
        assert_eq!(first["policy_activated"], false);
        assert_eq!(first["external_writes"], 0);
    }

    #[tokio::test]
    async fn rejects_overlong_histories_and_stale_commands() {
        let generate = json!({"action":"generate","approved_inputs":true,"expected_revision":1});
        assert_eq!(
            request(json!({"history":vec![generate;65]})).await.0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(request(json!({"command":{"action":"generate","approved_inputs":true,"expected_revision":99}})).await.0, StatusCode::CONFLICT);
    }
}
