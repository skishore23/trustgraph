use reqwest::{Client, StatusCode};
use serde_json::{Value, json};

#[tokio::test]
async fn agent_handoff_feedback_revision_and_release_work_through_http() {
    let temp = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = trustgraph_cli::api::router(temp.path()).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = Client::new();
    let post = |path: &str, actor: &str, body: Value| {
        client
            .post(format!("{base}/api{path}"))
            .bearer_auth(format!("demo-{actor}"))
            .json(&body)
    };
    let get = |path: &str, actor: &str| {
        client
            .get(format!("{base}/api{path}"))
            .bearer_auth(format!("demo-{actor}"))
    };
    let list: Value = get("/requests", "maya")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list["requests"].as_array().unwrap().len(), 4);
    let sam: Value = get("/requests", "sam")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(sam["requests"].as_array().unwrap().is_empty());
    assert_eq!(
        get("/requests/sample-launch", "sam")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let draft: Value = post(
        "/drafts/generate",
        "agent",
        json!({"restricted":false,"key":"http-intake"}),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    let intake = json!({"key":"http-request","title":"An agent-prepared update","summary":"Ready for review.","artifact":draft["artifact"],"audience":"company"});
    let submitted: Value = post("/requests", "agent", intake.clone())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(submitted["request"]["submitted_by"], "agent");
    post("/requests", "agent", intake)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        post(
            "/requests/http-request/release",
            "agent",
            json!({"revision":1,"context_digest":submitted["context_digest"]})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::FORBIDDEN
    );
    let returned: Value = post(
        "/requests/http-request/changes",
        "maya",
        json!({"revision":1,"body":"Please clarify the opening.","key":"return-1"}),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(returned["status"], "changes_requested");
    assert_eq!(
        post(
            "/requests/http-request/release",
            "maya",
            json!({"revision":1,"context_digest":submitted["context_digest"]})
        )
        .send()
        .await
        .unwrap()
        .status(),
        StatusCode::CONFLICT
    );
    let revision: Value = post(
        &format!(
            "/artifacts/{}/versions",
            draft["artifact"].as_str().unwrap()
        ),
        "agent",
        json!({"content":"The revised announcement.","key":"http-revision"}),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    let updated: Value = post(
        "/requests/http-request/revisions",
        "agent",
        json!({"revision":1,"artifact":revision["artifact"]}),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(updated["status"], "awaiting_review");
    assert_eq!(
        updated["request"]["feedback"][0]["body"],
        "Please clarify the opening."
    );
    let body = json!({"revision":2,"context_digest":updated["context_digest"]});
    let released: Value = post("/requests/http-request/release", "maya", body.clone())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(released["status"], "released");
    assert_eq!(released["artifact"]["content"], "The revised announcement.");
    let repeat: Value = post("/requests/http-request/release", "arun", body)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(released["operation"], repeat["operation"]);
    server.abort();
}
