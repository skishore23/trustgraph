use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
type Calls = Arc<Mutex<Vec<(String, Value)>>>;
const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
async fn workspace(headers: HeaderMap) -> Result<Json<Value>, StatusCode> {
    if headers["authorization"] != format!("Bearer {TOKEN}") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(Json(
        json!({"policy":{"name":"Reviewed changes"},"sources":[{"id":"shared"}],"jobs":[{"id":"job-1","status":"blocked","error":"restricted source","events":[]}]}),
    ))
}
async fn mutation(
    State(calls): State<Calls>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    if headers["authorization"] != format!("Bearer {TOKEN}") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    calls.lock().unwrap().push(("submit".into(), body));
    Ok(Json(json!({"id":"job-2"})))
}
async fn repair(
    State(calls): State<Calls>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    if headers["authorization"] != format!("Bearer {TOKEN}") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    calls.lock().unwrap().push(("repair".into(), json!(id)));
    Ok(Json(json!({"id":"job-3"})))
}
async fn simulate(
    State(calls): State<Calls>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    if headers["authorization"] != format!("Bearer {TOKEN}") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    calls.lock().unwrap().push(("simulate".into(), body));
    Ok(Json(
        json!({"status":"needs_approval","policy_activated":false}),
    ))
}
fn capability(dir: &std::path::Path) -> std::path::PathBuf {
    let file = dir.join("agent-token");
    std::fs::write(&file, TOKEN).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    file
}

#[tokio::test]
async fn hosted_agent_authentication_and_tools_stay_separate_from_operator() {
    let dir = tempfile::tempdir().unwrap();
    let token = capability(dir.path());
    let remote_dir = tempfile::tempdir().unwrap();
    let remote_token = capability(remote_dir.path());
    let remote_secret = "b".repeat(64);
    std::fs::write(&remote_token, &remote_secret).unwrap();
    let calls = Calls::default();
    let backend = Router::new()
        .route("/api/work", get(workspace).post(mutation))
        .route("/api/work/{id}/repair", post(repair))
        .route("/api/simulate", post(simulate))
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let backend = tokio::spawn(async move { axum::serve(listener, backend).await.unwrap() });
    assert!(trustgraph_cli::mcp::http_router(&base, &token, &token, None).is_err());
    for public in [
        "http://demo.example",
        "https://demo.example/mcp",
        "https://user@demo.example",
        "https://demo.example/?secret=x",
    ] {
        assert!(
            trustgraph_cli::mcp::http_router(&base, &token, &remote_token, Some(public)).is_err()
        );
    }
    let app = trustgraph_cli::mcp::http_router(
        &base,
        &token,
        &remote_token,
        Some("https://demo.example"),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"hosted-demo","version":"1"}}});
    for credential in [None, Some(TOKEN), Some("wrong")] {
        let mut request = client
            .post(&endpoint)
            .header("Accept", "application/json, text/event-stream")
            .json(&initialize);
        if let Some(value) = credential {
            request = request.bearer_auth(value);
        }
        let reply = request.send().await.unwrap();
        assert_eq!(reply.status(), reqwest::StatusCode::UNAUTHORIZED);
        assert_eq!(reply.headers()["www-authenticate"], "Bearer");
    }
    for (header, value) in [("Origin", "https://evil.example"), ("Host", "evil.example")] {
        let reply = client
            .post(&endpoint)
            .bearer_auth(&remote_secret)
            .header("Accept", "application/json, text/event-stream")
            .header(header, value)
            .json(&initialize)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), reqwest::StatusCode::FORBIDDEN);
    }
    let init: Value = client
        .post(&endpoint)
        .bearer_auth(&remote_secret)
        .header("Accept", "application/json, text/event-stream")
        .header("Host", "demo.example")
        .json(&initialize)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "trustgraph");
    async fn rpc(
        client: &reqwest::Client,
        url: &str,
        secret: &str,
        method: &str,
        params: Value,
    ) -> Value {
        client
            .post(url)
            .bearer_auth(secret)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-03-26")
            .json(&json!({"jsonrpc":"2.0","id":2,"method":method,"params":params}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    let listed = rpc(&client, &endpoint, &remote_secret, "tools/list", json!({})).await;
    let names: Vec<_> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "trustgraph_workspace",
            "trustgraph_submit_work",
            "trustgraph_status",
            "trustgraph_repair",
            "trustgraph_simulate"
        ]
    );
    let read = rpc(
        &client,
        &endpoint,
        &remote_secret,
        "tools/call",
        json!({"name":"trustgraph_workspace","arguments":{}}),
    )
    .await;
    assert_eq!(
        read["result"]["structuredContent"]["jobs"][0]["status"],
        "blocked"
    );
    let submit = rpc(&client, &endpoint, &remote_secret, "tools/call", json!({"name":"trustgraph_submit_work","arguments":{"goal":"admitted task","include_risk":true}})).await;
    assert_eq!(
        submit["result"]["structuredContent"]["review_url"],
        format!("{base}/agent/job-2")
    );
    let status = rpc(
        &client,
        &endpoint,
        &remote_secret,
        "tools/call",
        json!({"name":"trustgraph_status","arguments":{"id":"job-1"}}),
    )
    .await;
    assert_eq!(status["result"]["structuredContent"]["status"], "blocked");
    let repair = rpc(
        &client,
        &endpoint,
        &remote_secret,
        "tools/call",
        json!({"name":"trustgraph_repair","arguments":{"id":"job-1"}}),
    )
    .await;
    assert_eq!(repair["result"]["structuredContent"]["id"], "job-3");
    let simulation = rpc(&client, &endpoint, &remote_secret, "tools/call", json!({"name":"trustgraph_simulate","arguments":{"paths":["src/lib.rs"],"tests_passed":true,"lineage_complete":true,"approval":"missing"}})).await;
    assert_eq!(
        simulation["result"]["structuredContent"]["policy_activated"],
        false
    );
    let denied = rpc(
        &client,
        &endpoint,
        &remote_secret,
        "tools/call",
        json!({"name":"trustgraph_publish","arguments":{"id":"job-1"}}),
    )
    .await;
    assert!(denied.get("error").is_some());
    for path in ["/", "/api/work", "/api/approve", "/agent/job-1"] {
        assert_eq!(
            client
                .get(endpoint.replace("/mcp", path))
                .bearer_auth(&remote_secret)
                .send()
                .await
                .unwrap()
                .status(),
            reqwest::StatusCode::NOT_FOUND
        );
    }
    assert_eq!(calls.lock().unwrap().len(), 3);
    server.abort();
    backend.abort();
}

#[tokio::test]
async fn coding_agent_uses_live_api_over_real_stdio_without_human_authority() {
    let dir = tempfile::tempdir().unwrap();
    let token = capability(dir.path());
    let calls = Calls::default();
    let app = Router::new()
        .route("/api/work", get(workspace).post(mutation))
        .route("/api/work/{id}/repair", post(repair))
        .route("/api/simulate", post(simulate))
        .with_state(calls.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"));
    command
        .args(["mcp", "--url", &base, "--token-file"])
        .arg(token);
    let client = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 5);
    for name in [
        "trustgraph_approve",
        "trustgraph_publish",
        "trustgraph_activate_policy",
    ] {
        assert!(tools.iter().all(|t| t.name != name));
    }
    for (name, args, expected) in [
        (
            "trustgraph_workspace",
            json!({}),
            json!({"policy":{"name":"Reviewed changes"},"sources":[{"id":"shared"}],"jobs":[{"id":"job-1","status":"blocked","error":"restricted source","events":[]}]}),
        ),
        (
            "trustgraph_submit_work",
            json!({"goal":"admitted task","include_risk":true}),
            json!({"id":"job-2","review_url":format!("{base}/agent/job-2")}),
        ),
        (
            "trustgraph_status",
            json!({"id":"job-1"}),
            json!({"id":"job-1","status":"blocked","error":"restricted source","events":[],"review_url":format!("{base}/agent/job-1")}),
        ),
        (
            "trustgraph_repair",
            json!({"id":"job-1"}),
            json!({"id":"job-3","review_url":format!("{base}/agent/job-3")}),
        ),
        (
            "trustgraph_simulate",
            json!({"paths":["src/lib.rs"],"tests_passed":true,"lineage_complete":true,"approval":"missing"}),
            json!({"status":"needs_approval","policy_activated":false}),
        ),
    ] {
        let response = client
            .call_tool(
                CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(response.structured_content, Some(expected));
    }
    for (name, args) in [
        ("trustgraph_status", json!({"id":"../approve"})),
        (
            "trustgraph_submit_work",
            json!({"goal":"task","patch":"untrusted"}),
        ),
        ("trustgraph_workspace", json!({"token":"fake"})),
    ] {
        assert!(
            client
                .call_tool(
                    CallToolRequestParams::new(name)
                        .with_arguments(args.as_object().unwrap().clone())
                )
                .await
                .is_err()
        );
    }
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            (
                "submit".into(),
                json!({"goal":"admitted task","include_risk":true})
            ),
            ("repair".into(), json!("job-1")),
            (
                "simulate".into(),
                json!({"paths":["src/lib.rs"],"tests_passed":true,"lineage_complete":true,"approval":"missing"})
            )
        ]
    );
    client.cancel().await.unwrap();
    server.abort();
}
#[test]
fn adapter_restricts_destination_and_private_capability() {
    for url in [
        "https://example.com/",
        "http://127.0.0.1.evil.example/",
        "http://user:pass@127.0.0.1/",
        "http://127.0.0.1/mcp",
        "http://127.0.0.1/?token=secret",
        "http://127.0.0.1/#fragment",
    ] {
        assert!(trustgraph_cli::mcp::local_endpoint(url).is_err());
    }
    assert!(trustgraph_cli::mcp::local_endpoint("http://127.0.0.1:8080").is_ok());
    let dir = tempfile::tempdir().unwrap();
    let path = capability(dir.path());
    assert_eq!(trustgraph_cli::mcp::read_token(&path).unwrap(), TOKEN);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(trustgraph_cli::mcp::read_token(&path).is_err());
        let link = dir.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(trustgraph_cli::mcp::read_token(&link).is_err());
    }
}
#[tokio::test]
async fn adapter_never_retries_a_submission_after_a_lost_reply() {
    use axum::body::Body;
    use axum::response::Response;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let count = Arc::new(AtomicUsize::new(0));
    let captured = count.clone();
    let app = Router::new().route(
        "/api/work",
        post(move || {
            let count = captured.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Response::new(Body::from_stream(tokio_stream::once(Err::<String, _>(
                    std::io::Error::other("lost after acceptance"),
                ))))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let token = capability(dir.path());
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"));
    command
        .args(["mcp", "--url", &base, "--token-file"])
        .arg(token);
    let client = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    assert!(
        client
            .call_tool(
                CallToolRequestParams::new("trustgraph_submit_work")
                    .with_arguments(json!({"goal":"admitted task"}).as_object().unwrap().clone())
            )
            .await
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    client.cancel().await.unwrap();
    server.abort();
}
