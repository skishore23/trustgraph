use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use trustgraph_runtime::story::StoryPolicy;

async fn request(
    app: axum::Router,
    path: &str,
    body: Option<Value>,
    origin: Option<&str>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .uri(path)
        .header("Host", "127.0.0.1:8081");
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    let request = if let Some(body) = body {
        request
            .method("POST")
            .header("Content-Type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        request.body(Body::empty()).unwrap()
    };
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn demo_opens_without_signin_and_comparisons_have_no_release_effects() {
    let app = trustgraph_cli::story::router().unwrap();
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let html = String::from_utf8(
        to_bytes(response.into_body(), 100_000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("THE GOVERNANCE STORY"));
    assert!(!html.contains("operator.html"));
    let (status, before) = request(app.clone(), "/api/story", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let body = json!({"policy":StoryPolicy::default(),"scenario":{"inputs":["directory","compensation"],"destination":"engineering","approval":"valid","complete_lineage":true}});
    let (status, a) = request(app.clone(), "/api/story/compare", Some(body.clone()), None).await;
    assert_eq!(status, StatusCode::OK);
    let (_, b) = request(app.clone(), "/api/story/compare", Some(body), None).await;
    assert_eq!(a, b);
    assert_eq!(a["external_writes"], 0);
    assert_eq!(
        request(app.clone(), "/api/story", None, None).await.1,
        before
    );
    assert_eq!(
        request(
            app,
            "/api/story/reset",
            Some(json!({})),
            Some("https://outside.example")
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}
