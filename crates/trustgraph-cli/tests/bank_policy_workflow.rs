use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
#[tokio::test]
async fn bank_scopes_policy_tests_and_human_activation_are_connected() {
    let temp = tempfile::tempdir().unwrap();
    let app = trustgraph_cli::api::bank_router(temp.path()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let c = Client::new();
    let risk: Value = c
        .get(format!("{base}/api/scenario"))
        .bearer_auth("demo-arun")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(risk["sources"].as_array().unwrap().len(), 3);
    let eng: Value = c
        .get(format!("{base}/api/scenario"))
        .bearer_auth("demo-maya")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!eng.to_string().contains("risk-investigation"));
    let blocked:Value=c.post(format!("{base}/api/scenario/simulate")).bearer_auth("demo-agent").json(&json!({"sources":["engineering-requirements","approved-specification","risk-investigation"],"audience":"engineering-repo"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(blocked["report"]["ready_to_generate"], false);
    assert_eq!(
        blocked["report"]["conflict"]["source"],
        "risk-investigation"
    );
    let report: Value = c
        .post(format!("{base}/api/policies/bank-baseline-v1/test"))
        .bearer_auth("demo-agent")
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["report"]["passed"], true);
    let token = std::fs::read_to_string(temp.path().join("mcp-token")).unwrap();
    let mcp:Value=c.post(format!("{base}/mcp")).bearer_auth(&token).header("Accept","application/json, text/event-stream").header("MCP-Protocol-Version","2025-11-25").json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"trustgraph_policy_test","arguments":{"id":"bank-baseline-v1"}}})).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(mcp["result"]["structuredContent"]["report"]["passed"], true);
    let denied:Value=c.post(format!("{base}/mcp")).bearer_auth(&token).header("Accept","application/json, text/event-stream").header("MCP-Protocol-Version","2025-11-25").json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"trustgraph_policy_activate","arguments":{"id":"bank-baseline-v1"}}})).send().await.unwrap().json().await.unwrap();
    assert!(denied.get("error").is_some());
    assert_eq!(
        c.get(format!("{base}/api/policies"))
            .bearer_auth("demo-maya")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let activate = format!("{base}/api/policies/bank-baseline-v1/activate");
    assert_eq!(
        c.post(&activate)
            .bearer_auth("demo-agent")
            .json(&json!({"expected_revision":1}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    c.post(&activate)
        .bearer_auth("demo-arun")
        .json(&json!({"expected_revision":1}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        c.post(&activate)
            .bearer_auth("demo-arun")
            .json(&json!({"expected_revision":1}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        c.get(format!("{base}/api/policies"))
            .bearer_auth("demo-sam")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    server.abort();
}
