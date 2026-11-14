use reqwest::{Client, StatusCode};
use serde_json::{Value, json};

#[tokio::test]
async fn review_http_flow_enforces_roles_and_serves_exact_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = trustgraph_cli::api::router(temp.path()).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = Client::new();
    let response = client
        .get(format!("{base}/api/state"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let post = |route: &str, actor: &str, body: Value| {
        client
            .post(format!("{base}{route}"))
            .bearer_auth(format!("demo-{actor}"))
            .json(&body)
    };
    let private: Value = post(
        "/api/drafts/generate",
        "maya",
        json!({"restricted":true,"key":"private"}),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    let private_id = private["artifact"].as_str().unwrap();
    let decision: Value = post(
        "/api/verify",
        "maya",
        json!({"artifact":private_id,"audience":"company"}),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(decision["violations"][0]["code"], "RESTRICTED_SOURCE");
    let sales: Value = client
        .get(format!("{base}/api/state"))
        .bearer_auth("demo-sam")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!sales.to_string().contains("CONFIDENTIAL"));
    assert!(!sales.to_string().contains("launch-plan"));
    let hidden = client
        .get(format!("{base}/api/artifacts/{private_id}/lineage"))
        .bearer_auth("demo-sam")
        .send()
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    let draft: Value = post(
        "/api/drafts/generate",
        "maya",
        json!({"restricted":false,"key":"safe"}),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let id = draft["artifact"].as_str().unwrap();
    let review: Value = post(
        "/api/review",
        "maya",
        json!({"artifact":id,"audience":"company"}),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let context = review["context_digest"].as_str().unwrap();
    let request = json!({"artifact":id,"audience":"company","context_digest":context,"key":"safe"});
    assert_eq!(
        post("/api/approvals", "agent", request.clone())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    post("/api/approvals", "maya", request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    // A safe publication bundle must not include unrelated private workspace data.
    let safe_request =
        json!({"artifact":id,"audience":"company","context_digest":context,"key":"public-safe"});
    post("/api/publish", "maya", safe_request)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let safe_bundle: Value = client
        .get(format!("{base}/api/operations/public-safe/bundle"))
        .bearer_auth("demo-sam")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!safe_bundle.to_string().contains("launch-plan"));
    assert!(!safe_bundle.to_string().contains("CONFIDENTIAL"));
    let edited: Value = post(
        &format!("/api/artifacts/{id}/versions"),
        "maya",
        json!({"content":"The new reviewed message.","key":"edited"}),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let edit_id = edited["artifact"].as_str().unwrap();
    let stale: Value = post(
        "/api/verify",
        "maya",
        json!({"artifact":edit_id,"audience":"company"}),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(stale["violations"][0]["code"], "STALE_APPROVAL");
    let review: Value = post(
        "/api/review",
        "maya",
        json!({"artifact":edit_id,"audience":"company"}),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    let context = review["context_digest"].as_str().unwrap();
    let request =
        json!({"artifact":edit_id,"audience":"company","context_digest":context,"key":"revised"});
    post("/api/approvals", "maya", request.clone())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let result: Value = post("/api/publish", "maya", request.clone())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result["operation"]["state"], "PUBLISHED");
    assert_eq!(
        result["operation"]["receipt"]["destination"],
        "sim://northstar/company"
    );
    let duplicate: Value = post("/api/publish", "maya", request)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(result, duplicate);
    let bundle: trustgraph_core::bundle::Bundle = client
        .get(format!("{base}/api/operations/revised/bundle"))
        .bearer_auth("demo-maya")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    trustgraph_core::bundle::replay(&bundle).unwrap();
    assert_eq!(
        bundle.evidence.artifacts[&trustgraph_core::ArtifactId::from(edit_id)].content,
        "The new reviewed message."
    );
    let cross_origin = post(
        "/api/drafts/generate",
        "maya",
        json!({"restricted":false,"key":"csrf"}),
    )
    .header("Origin", "https://untrusted.example")
    .send()
    .await
    .unwrap();
    assert_eq!(cross_origin.status(), StatusCode::FORBIDDEN);
    server.abort();
}
