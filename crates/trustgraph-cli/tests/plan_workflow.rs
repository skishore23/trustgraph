use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
#[tokio::test]
async fn unsafe_plan_repair_generation_human_review_and_release_are_connected() {
    let temp = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = trustgraph_cli::api::router(temp.path()).unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let c = Client::new();
    let get = |path: &str| c.get(format!("{base}/api{path}")).bearer_auth("demo-maya");
    let post = |path: &str, body: Value| {
        c.post(format!("{base}/api{path}"))
            .bearer_auth("demo-maya")
            .json(&body)
    };
    let blocked: Value = get("/plans/weekly-launch-update")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(blocked["report"]["conflict"]["recipient"], "sam");
    assert_eq!(blocked["repairs"].as_array().unwrap().len(), 2);
    let fixed: Value = post(
        "/plans/weekly-launch-update/repair",
        json!({"kind":"fresh_context"}),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    let id = fixed["record"]["plan"]["id"].as_str().unwrap();
    assert_eq!(fixed["report"]["ready_to_generate"], true);
    let run:Value=post(&format!("/plans/{id}/prepare"),json!({"plan_digest":fixed["report"]["plan_digest"],"snapshot_digest":fixed["report"]["snapshot_digest"],"key":"http-prepare"})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    let request = run["run"]["request"].as_str().unwrap();
    let review: Value = get(&format!("/requests/{request}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(review["status"], "awaiting_review");
    assert!(
        !review["artifact"]["content"]
            .as_str()
            .unwrap()
            .contains("CONFIDENTIAL")
    );
    let outcome: Value = post(
        &format!("/requests/{request}/release"),
        json!({"revision":1,"context_digest":review["context_digest"]}),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(outcome["status"], "released");
    let model: Value = post("/verification", json!({"max_depth":6,"broken_rule":true}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(model["passed"], false);
    assert!(model["counterexample"].as_array().unwrap().len() >= 3);
    assert_eq!(
        c.get(format!("{base}/api/plans/weekly-launch-update"))
            .bearer_auth("demo-sam")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    server.abort();
}
