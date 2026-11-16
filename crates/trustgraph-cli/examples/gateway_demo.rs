//! Two agents through a running `trustgraph gateway` (see
//! examples/gateway/README.md): the researcher reads the salary file and
//! hands off; the planner receives, reads the roadmap and tries to post.
//!
//!     gateway_demo --tokens ~/tg-gateway/tokens [--url http://127.0.0.1:8787/gateway/mcp]

use std::path::PathBuf;

use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

async fn connect(
    url: &str,
    tokens: &std::path::Path,
    agent: &str,
) -> RunningService<RoleClient, ()> {
    let token = std::fs::read_to_string(tokens.join(format!("{agent}.token")))
        .expect("cannot read the agent's token file");
    let config = StreamableHttpClientTransportConfig::with_uri(url).auth_header(token.trim());
    ().serve(StreamableHttpClientTransport::from_config(config))
        .await
        .expect("cannot connect to the gateway")
}

/// Call a tool and print one line; the structured result.
async fn call(
    client: &RunningService<RoleClient, ()>,
    who: &str,
    tool: &str,
    arguments: Value,
    label: &str,
) -> Value {
    let result = client
        .peer()
        .call_tool(
            CallToolRequestParams::new(tool.to_string())
                .with_arguments(arguments.as_object().cloned().unwrap_or_default()),
        )
        .await
        .expect("the call failed");
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    let shown = if tool == "trustgraph__handoff" && result.is_error != Some(true) {
        "a hand-off token".to_string()
    } else {
        text
    };
    let error = if result.is_error == Some(true) {
        "error: "
    } else {
        ""
    };
    let label = if label.is_empty() {
        String::new()
    } else {
        format!(" {label}")
    };
    println!("{who} {tool}{label}: {error}{shown}");
    result.structured_content.unwrap_or(Value::Null)
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut url = "http://127.0.0.1:8787/gateway/mcp".to_string();
    let mut tokens = None;
    let mut i = 0;
    while i < args.len() {
        match (args[i].as_str(), args.get(i + 1)) {
            ("--url", Some(v)) => url = v.clone(),
            ("--tokens", Some(v)) => tokens = Some(PathBuf::from(v)),
            _ => {
                eprintln!("usage: gateway_demo --tokens DIR [--url URL]");
                std::process::exit(2);
            }
        }
        i += 2;
    }
    let Some(tokens) = tokens else {
        eprintln!("usage: gateway_demo --tokens DIR [--url URL]");
        std::process::exit(2);
    };

    let researcher = connect(&url, &tokens, "researcher").await;
    call(
        &researcher,
        "researcher",
        "drive__read_file",
        json!({"path": "HR/Compensation/salaries.csv"}),
        "HR/Compensation/salaries.csv",
    )
    .await;
    let handoff = call(
        &researcher,
        "researcher",
        "trustgraph__handoff",
        json!({}),
        "",
    )
    .await;
    let token = handoff["handoff"].as_str().unwrap_or_default().to_string();

    let planner = connect(&url, &tokens, "planner").await;
    call(
        &planner,
        "planner",
        "trustgraph__receive",
        json!({"token": token}),
        "",
    )
    .await;
    call(
        &planner,
        "planner",
        "drive__read_file",
        json!({"path": "Product/roadmap.md"}),
        "Product/roadmap.md",
    )
    .await;
    call(
        &planner,
        "planner",
        "slack__post_message",
        json!({"channel": "#engineering", "text": "Q3 plan and pay bands"}),
        "#engineering",
    )
    .await;
    let _ = researcher.cancel().await;
    let _ = planner.cancel().await;
}
