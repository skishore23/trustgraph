use reqwest::{Client, StatusCode};
use serde_json::{Value, json};

#[tokio::test]
async fn bank_walkthrough_uses_role_gates_and_never_mutates_the_workspace_journal() {
    let temp = tempfile::tempdir().unwrap();
    let app = trustgraph_cli::api::bank_router(temp.path()).unwrap();
    let original = std::fs::read(temp.path().join("journal.jsonl")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = Client::new();
    let url = format!("{base}/api/demo");
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let maya: Value = client
        .get(&url)
        .bearer_auth("demo-maya")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(maya["simple"]["visible"], false);
    assert!(!maya.to_string().contains("mixed-test"));
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("demo-maya")
            .json(&json!({"action":"simulate_mixed"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let blocked: Value = client
        .post(&url)
        .bearer_auth("demo-arun")
        .json(&json!({"action":"simulate_mixed"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(blocked["simple"]["assessment"]["status"], "blocked");
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("demo-agent")
            .json(&json!({"action":"approve_patch","expected":maya["medium"]["expected"]}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let approved: Value = client
        .post(&url)
        .bearer_auth("demo-maya")
        .json(&json!({"action":"approve_patch","expected":maya["medium"]["expected"]}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(approved["medium"]["assessment"]["status"], "ready");
    client
        .post(&url)
        .bearer_auth("demo-agent")
        .json(&json!({"action":"change_patch"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        client
            .post(&url)
            .bearer_auth("demo-maya")
            .json(&json!({"action":"approve_patch","expected":maya["medium"]["expected"]}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    let stale: Value = client
        .post(&url)
        .bearer_auth("demo-agent")
        .json(&json!({"action":"simulate_patch"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        stale["medium"]["assessment"]["decision"]["violations"][0]["code"],
        "STALE_APPROVAL"
    );
    client
        .post(&url)
        .bearer_auth("demo-maya")
        .json(&json!({"action":"approve_baseline","expected":maya["complex"]["expected"]}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    client
        .post(&url)
        .bearer_auth("demo-arun")
        .json(&json!({"action":"mark_impact","source":"case-analysis"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let impact: Value = client
        .post(&url)
        .bearer_auth("demo-arun")
        .json(&json!({"action":"evaluate_impact"}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(impact["complex"]["phase"], "evaluated");
    assert_eq!(impact["publisher_calls"], 0);
    assert_eq!(
        std::fs::read(temp.path().join("journal.jsonl")).unwrap(),
        original
    );
    let reset: Value = client
        .post(format!("{url}/reset"))
        .bearer_auth("demo-arun")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reset["complex"]["phase"], "baseline");
    assert_eq!(reset["complex"]["approvals"], json!([]));
    assert_eq!(reset["medium"]["artifact"]["version"], 1);
    assert_eq!(
        std::fs::read(temp.path().join("journal.jsonl")).unwrap(),
        original
    );
    server.abort();
}
