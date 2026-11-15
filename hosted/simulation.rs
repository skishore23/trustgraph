//! Bounded virtual-time coordinator scenarios, without network or real publishing.
use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use trustgraph_sim::Scenario;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    scenario: Scenario,
    seed: u16,
}
pub async fn demo(Json(request): Json<Request>) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        trustgraph_sim::run(request.seed as u64, request.scenario)
    })
    .await;
    let (status, value) = match result {
        Ok(Ok(trace)) => (
            StatusCode::OK,
            json!({"trace":trace,"external_writes":0,"scope":"Seeded virtual-time simulation of the existing journaled coordinator. Not the generic SDK, actual threads, network faults, or a production incident."}),
        ),
        Ok(Err(e)) => (StatusCode::BAD_REQUEST, json!({"error":e})),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":"Simulation did not complete"}),
        ),
    };
    (status, [(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request as HttpRequest,
    };
    use tower::ServiceExt;
    async fn request(value: serde_json::Value) -> (StatusCode, serde_json::Value) {
        let response = crate::router()
            .oneshot(
                HttpRequest::post("/api/simulation")
                    .header("content-type", "application/json")
                    .body(Body::from(value.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    #[tokio::test]
    async fn all_scenarios_run_and_replay_with_no_external_writes() {
        for scenario in Scenario::ALL {
            let payload = json!({"scenario":scenario,"seed":7});
            let (status, a) = request(payload.clone()).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(a["external_writes"], 0);
            assert!(!a["trace"]["steps"].as_array().unwrap().is_empty());
            assert_eq!(a, request(payload).await.1);
        }
    }
    #[tokio::test]
    async fn seed_limit_is_enforced_before_simulation() {
        let response = crate::router()
            .oneshot(
                HttpRequest::post("/api/simulation")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"scenario":"demo","seed":65536}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_client_error());
    }
}
