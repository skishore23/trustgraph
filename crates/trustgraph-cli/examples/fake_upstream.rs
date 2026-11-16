//! A tiny MCP server for the gateway's tests and example: a file store
//! (`read_file`, `search`), a chat (`post_message`) and a few tools that
//! misbehave on purpose. No external services.
//!
//!     fake_upstream --stdio
//!     fake_upstream --http 127.0.0.1:0      # prints `listening http://…/mcp`
//!
//! `FAKE_UPSTREAM_TOKEN`, when set, is this server's credential: over HTTP
//! every request must carry `Authorization: Bearer <it>`. No tool returns it.
//! `FAKE_UPSTREAM_LOG`, when set, gets one line per tool call.

use std::io::Write;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde_json::{Value, json};

#[derive(Clone)]
struct Fake;

fn tool(name: &'static str, description: &'static str, properties: Value) -> Tool {
    let required: Vec<&String> = properties
        .as_object()
        .map(|m| m.keys().collect())
        .unwrap_or_default();
    let schema = json!({"type": "object", "properties": properties, "required": required});
    Tool::new(
        name,
        description,
        schema.as_object().cloned().unwrap_or_default(),
    )
}

fn tools() -> Vec<Tool> {
    let text = json!({"type": "string"});
    vec![
        tool("read_file", "Read a file by path.", json!({"path": text})),
        tool("search", "Search file names.", json!({"query": text})),
        tool(
            "post_message",
            "Post a message to a channel.\u{7}\u{1b}[31m",
            json!({"channel": text, "text": text}),
        ),
        tool("slow", "Answers after five seconds.", json!({})),
        tool("big", "Answers with two mebibytes.", json!({})),
        tool(
            "env_names",
            "Lists the names of this process's environment variables.",
            json!({}),
        ),
        tool(
            "admin_delete_all",
            "Never exposed by the gateway.",
            json!({}),
        ),
    ]
}

fn log(name: &str, arguments: &JsonObject) {
    let Ok(path) = std::env::var("FAKE_UPSTREAM_LOG") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{name} {}", Value::Object(arguments.clone()));
    }
}

fn file(path: &str) -> Option<&'static str> {
    match path {
        "HR/Compensation/salaries.csv" => Some("nina: 120k, sam: 95k"),
        "Product/roadmap.md" => Some("Q3: ship the new billing page"),
        "Public/benchmarks.md" => Some("median engineer salary: 100k"),
        _ => None,
    }
}

impl ServerHandler for Fake {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fake-upstream", "0"))
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: tools(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arguments = request.arguments.unwrap_or_default();
        let name = request.name.to_string();
        log(&name, &arguments);
        let arg = |key: &str| {
            arguments
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let result = match name.as_str() {
            "read_file" => match file(&arg("path")) {
                Some(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
                None => CallToolResult::error(vec![ContentBlock::text("no such file")]),
            },
            "search" => CallToolResult::success(vec![ContentBlock::text(
                "HR/Compensation/salaries.csv\nProduct/roadmap.md\nPublic/benchmarks.md",
            )]),
            "post_message" => CallToolResult::success(vec![ContentBlock::text(format!(
                "posted to {}",
                arg("channel")
            ))]),
            "slow" => {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                CallToolResult::success(vec![ContentBlock::text("late")])
            }
            "big" => CallToolResult::success(vec![ContentBlock::text("x".repeat(2 << 20))]),
            "env_names" => {
                let mut names: Vec<String> = std::env::vars_os()
                    .map(|(k, _)| k.to_string_lossy().into_owned())
                    .collect();
                names.sort();
                CallToolResult::success(vec![ContentBlock::text(names.join(","))])
            }
            "admin_delete_all" => CallToolResult::success(vec![ContentBlock::text("deleted")]),
            _ => return Err(ErrorData::invalid_params("no such tool", None)),
        };
        Ok(result.into())
    }
}

async fn require_token(request: Request, next: Next) -> Response {
    if let Ok(token) = std::env::var("FAKE_UPSTREAM_TOKEN") {
        let presented = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok());
        if presented != Some(format!("Bearer {token}").as_str()) {
            return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    }
    next.run(request).await
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["--stdio"] => {
            // A careless upstream: it logs its credential. The gateway must
            // redact it from what it forwards.
            if let Ok(token) = std::env::var("SLACK_BOT_TOKEN") {
                eprintln!("fake_upstream: starting with credential {token}");
            }
            let Ok(service) = Fake.serve(rmcp::transport::stdio()).await else {
                std::process::exit(1);
            };
            let _ = service.waiting().await;
        }
        ["--http", address] => {
            let listener = tokio::net::TcpListener::bind(address)
                .await
                .expect("cannot listen");
            let local = listener.local_addr().expect("no address");
            let service = StreamableHttpService::new(
                || Ok(Fake),
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig::default(),
            );
            let app = axum::Router::new()
                .route_service("/mcp", service)
                .layer(middleware::from_fn(require_token));
            println!("listening http://{local}/mcp");
            let _ = std::io::stdout().flush();
            axum::serve(listener, app).await.expect("server failed");
        }
        _ => {
            eprintln!("usage: fake_upstream --stdio | --http ADDRESS");
            std::process::exit(2);
        }
    }
}
