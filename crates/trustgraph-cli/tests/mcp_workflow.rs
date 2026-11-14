use reqwest::{Client, StatusCode};
use serde_json::{Value, json};

async fn rpc(client: &Client, base: &str, token: &str, method: &str, params: Value) -> Value {
    client
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-11-25")
        .bearer_auth(token)
        .json(&json!({"jsonrpc":"2.0","id":20,"method":method,"params":params}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}
async fn call(client: &Client, base: &str, token: &str, name: &str, args: Value) -> Value {
    rpc(
        client,
        base,
        token,
        "tools/call",
        json!({"name":name,"arguments":args}),
    )
    .await
}

#[tokio::test]
async fn mcp_requires_a_scoped_capability_and_lists_no_human_authority() {
    let temp = tempfile::tempdir().unwrap();
    let app = trustgraph_cli::api::consulting_router(temp.path()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = Client::new();
    let message = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"trustgraph-test","version":"1"}}});
    let response = client
        .post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .json(&message)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let token = std::fs::read_to_string(temp.path().join("mcp-token")).unwrap();
    let config: Value = client
        .get(format!("{base}/api/mcp-config"))
        .bearer_auth("demo-maya")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(config["mcpServers"]["trustgraph"]["args"][0], "mcp");
    assert!(
        config["mcpServers"]["trustgraph"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == &json!(format!("{base}/mcp")))
    );
    assert!(!config.to_string().contains(&token));
    let post = |message: Value| {
        client
            .post(format!("{base}/mcp"))
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .bearer_auth(token.trim())
            .json(&message)
    };
    let initialized: Value = post(message)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(initialized["result"]["capabilities"]["tools"], json!({}));
    let tools: Value = post(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let tools = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 10);
    for tool in tools {
        assert!(
            !["approve", "publish", "release", "set_acl"]
                .iter()
                .any(|word| tool["name"].as_str().unwrap().contains(word))
        );
    }
    assert_eq!(
        post(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}))
            .header("Origin", "https://foreign.example")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}))
            .header("Host", "foreign.example:8080")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(temp.path().join("mcp-token"))
                .unwrap()
                .permissions()
                .mode()
                & 0o077,
            0
        );
    }
    server.abort();
}

#[tokio::test]
async fn actual_mcp_tools_connect_blocked_plan_repairs_human_review_and_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let app = trustgraph_cli::api::consulting_router(temp.path()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let c = Client::new();
    let token = std::fs::read_to_string(temp.path().join("mcp-token")).unwrap();
    let workspace = call(&c, &base, &token, "trustgraph_workspace", json!({})).await;
    assert_eq!(workspace["result"]["structuredContent"]["actor"], "agent");
    assert_eq!(
        workspace["result"]["structuredContent"]["sources"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    let initial=call(&c,&base,&token,"trustgraph_simulate",json!({"sources":["market-brief","helios-data-room","orion-benchmark"],"audience":"atlas-committee"})).await;
    let initial = &initial["result"]["structuredContent"];
    assert_eq!(initial["report"]["ready_to_generate"], false);
    assert_eq!(initial["report"]["conflict"]["recipient"], "sam");
    let mut plan = initial["plan"].clone();
    plan["id"] = json!("deal.memo");
    let proposed = call(
        &c,
        &base,
        &token,
        "trustgraph_propose",
        json!({"plan":plan}),
    )
    .await;
    assert_eq!(
        proposed["result"]["structuredContent"]["record"]["submitted_by"],
        "agent"
    );
    let analyzed = call(
        &c,
        &base,
        &token,
        "trustgraph_analyze",
        json!({"id":"deal.memo"}),
    )
    .await;
    let analyzed = &analyzed["result"]["structuredContent"];
    assert_eq!(analyzed["repairs"].as_array().unwrap().len(), 2);
    let refused=call(&c,&base,&token,"trustgraph_prepare",json!({"id":"deal.memo","plan_digest":analyzed["report"]["plan_digest"],"snapshot_digest":analyzed["report"]["snapshot_digest"],"key":"blocked"})).await;
    assert_eq!(refused["result"]["isError"], true);
    let repaired = call(
        &c,
        &base,
        &token,
        "trustgraph_repair",
        json!({"id":"deal.memo","kind":"fresh_context"}),
    )
    .await;
    let repaired = &repaired["result"]["structuredContent"];
    let args = json!({"id":repaired["record"]["plan"]["id"],"plan_digest":repaired["report"]["plan_digest"],"snapshot_digest":repaired["report"]["snapshot_digest"],"key":"factory.work.order.1"});
    let mut stale = args.clone();
    stale["plan_digest"] = analyzed["report"]["plan_digest"].clone();
    assert_eq!(
        call(&c, &base, &token, "trustgraph_prepare", stale).await["result"]["structuredContent"]["http_status"],
        409
    );
    let prepared = call(&c, &base, &token, "trustgraph_prepare", args.clone()).await;
    assert_eq!(
        prepared,
        call(&c, &base, &token, "trustgraph_prepare", args).await
    );
    let request = prepared["result"]["structuredContent"]["run"]["request"]
        .as_str()
        .unwrap();
    assert!(
        prepared["result"]["structuredContent"]["review_path"]
            .as_str()
            .unwrap()
            .contains(request)
    );
    let status = call(
        &c,
        &base,
        &token,
        "trustgraph_status",
        json!({"request_id":request}),
    )
    .await;
    let status = &status["result"]["structuredContent"];
    assert_eq!(status["status"], "awaiting_review");
    assert!(
        !status["artifact"]["content"]
            .as_str()
            .unwrap()
            .contains("ORION")
    );
    for (name, args) in [
        ("trustgraph_approve", json!({})),
        ("trustgraph_publish", json!({})),
        ("trustgraph_workspace", json!({"actor":"maya"})),
    ] {
        let response = call(&c, &base, &token, name, args).await;
        assert!(response.get("error").is_some() || response["result"]["isError"] == true);
    }
    let release = json!({"revision":1,"context_digest":status["context_digest"]});
    assert_eq!(
        c.post(format!("{base}/api/requests/{request}/release"))
            .bearer_auth("demo-agent")
            .json(&release)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    c.post(format!("{base}/api/requests/{request}/release"))
        .bearer_auth("demo-maya")
        .json(&release)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        call(
            &c,
            &base,
            &token,
            "trustgraph_status",
            json!({"request_id":request})
        )
        .await["result"]["structuredContent"]["status"],
        "released"
    );
    server.abort();
}

#[tokio::test]
async fn real_stdio_child_uses_the_running_app_without_a_second_journal_owner() {
    use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
    let temp = tempfile::tempdir().unwrap();
    let app = trustgraph_cli::api::consulting_router(temp.path()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"));
    command
        .arg("mcp")
        .arg("--url")
        .arg(endpoint)
        .arg("--token-file")
        .arg(temp.path().join("mcp-token"));
    let transport = TokioChildProcess::new(command).unwrap();
    let client = ().serve(transport).await.unwrap();
    assert_eq!(client.list_all_tools().await.unwrap().len(), 10);
    let result = client
        .call_tool(
            CallToolRequestParams::new("trustgraph_workspace").with_arguments(Default::default()),
        )
        .await
        .unwrap();
    assert_eq!(
        result.structured_content.as_ref().unwrap()["workspace"],
        "aster-advisory"
    );
    client.cancel().await.unwrap();
    server.abort();
}

#[test]
fn bridge_rejects_nonlocal_urls_and_credential_bearing_destinations() {
    for url in [
        "https://example.com/mcp",
        "http://127.0.0.1.evil.example/mcp",
        "http://user:pass@127.0.0.1:8080/mcp",
        "http://127.0.0.1:8080/api/approvals",
        "http://127.0.0.1:8080/mcp?token=secret",
        "http://127.0.0.1:8080/mcp#fragment",
    ] {
        assert!(trustgraph_cli::mcp::local_endpoint(url).is_err());
    }
    assert!(trustgraph_cli::mcp::local_endpoint("http://127.0.0.1:8080/mcp").is_ok());
}

#[test]
fn private_agent_capability_is_reused_when_the_workspace_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let first = trustgraph_cli::api::consulting_router(temp.path()).unwrap();
    let token = trustgraph_cli::mcp::read_token(&temp.path().join("mcp-token")).unwrap();
    drop(first);
    let _reopened = trustgraph_cli::api::consulting_router(temp.path()).unwrap();
    assert_eq!(
        trustgraph_cli::mcp::read_token(&temp.path().join("mcp-token")).unwrap(),
        token
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            temp.path().join("mcp-token"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(trustgraph_cli::mcp::read_token(&temp.path().join("mcp-token")).is_err());
    }
}

#[tokio::test]
async fn stdio_bridge_does_not_resend_when_a_prepare_reply_is_lost_after_commit() {
    use axum::{
        body::{Body, to_bytes},
        extract::Request,
        middleware::{self, Next},
        response::Response,
    };
    use rmcp::{ServiceExt, model::CallToolRequestParams, transport::TokioChildProcess};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let temp = tempfile::tempdir().unwrap();
    let app = trustgraph_cli::api::consulting_router(temp.path()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let fault_calls = calls.clone();
    let app = app.layer(middleware::from_fn(move |request: Request, next: Next| {
        let calls = fault_calls.clone();
        async move {
            let (parts, body) = request.into_parts();
            let bytes = to_bytes(body, 262_144).await.unwrap();
            let message = serde_json::from_slice::<Value>(&bytes).unwrap_or_default();
            let lose = parts.uri.path() == "/mcp"
                && message["method"] == "tools/call"
                && message["params"]["name"] == "trustgraph_prepare";
            if lose {
                calls.fetch_add(1, Ordering::SeqCst);
            }
            let response = next
                .run(Request::from_parts(parts, Body::from(bytes)))
                .await;
            if !lose {
                return response;
            }
            // The domain handler has committed. Consume its reply, then fail the
            // actual HTTP body so neither MCP client sees the successful result.
            let (parts, body) = response.into_parts();
            let _ = to_bytes(body, 4_194_304).await.unwrap();
            Response::from_parts(
                parts,
                Body::from_stream(tokio_stream::once(Err::<String, _>(std::io::Error::other(
                    "deliberately dropped post-commit reply",
                )))),
            )
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let token = std::fs::read_to_string(temp.path().join("mcp-token")).unwrap();
    let c = Client::new();
    let preview = call(
        &c,
        &base,
        &token,
        "trustgraph_simulate",
        json!({"sources":["market-brief","helios-data-room"],"audience":"atlas-committee"}),
    )
    .await;
    let preview = &preview["result"]["structuredContent"];
    call(
        &c,
        &base,
        &token,
        "trustgraph_propose",
        json!({"plan":preview["plan"]}),
    )
    .await;
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_trustgraph"));
    command
        .arg("mcp")
        .arg("--url")
        .arg(format!("{base}/mcp"))
        .arg("--token-file")
        .arg(temp.path().join("mcp-token"));
    let client = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let args = json!({"id":preview["plan"]["id"],"plan_digest":preview["report"]["plan_digest"],"snapshot_digest":preview["report"]["snapshot_digest"],"key":"lost.reply"});
    let result = client
        .call_tool(
            CallToolRequestParams::new("trustgraph_prepare")
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await;
    assert!(result.is_err());
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "an ambiguous mutation must never be resent by the bridge"
    );
    let requests: Value = c
        .get(format!("{base}/api/requests"))
        .bearer_auth("demo-agent")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(requests["requests"].as_array().unwrap().len(), 1);
    assert_eq!(requests["requests"][0]["status"], "awaiting_review");
    client.cancel().await.unwrap();
    server.abort();
}
