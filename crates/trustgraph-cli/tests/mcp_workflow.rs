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
