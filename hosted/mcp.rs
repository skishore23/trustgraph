//! Anonymous MCP access to fictional, read-only policy experiments only.
use axum::Router;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    model::*,
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use trustgraph_runtime::story::{Scenario, Story, StoryPolicy};

#[derive(Clone)]
struct Demo;

fn tools() -> Vec<Tool> {
    let source_readers: serde_json::Map<_, _> = ["directory", "compensation", "budget", "approved-planning"]
        .into_iter()
        .map(|source| (source.to_owned(), json!({"type":"array","minItems":1,"maxItems":3,"uniqueItems":true,"items":{"enum":["hr","finance","engineering"]}})))
        .collect();
    let scenario = json!({"type":"object","additionalProperties":false,
        "properties":{
            "inputs":{"type":"array","minItems":1,"maxItems":4,"uniqueItems":true,"items":{"enum":["directory","compensation","budget","approved-planning"]}},
            "destination":{"enum":["engineering","hr","finance"]},
            "approval":{"enum":["missing","valid","changed","expired"]},
            "approval_age_minutes":{"type":"integer","minimum":0,"maximum":10080,"default":0},
            "complete_lineage":{"type":"boolean"}
        },"required":["inputs","destination","approval","complete_lineage"]});
    let policy = json!({"type":"object","additionalProperties":false,
        "properties":{
            "name":{"type":"string","minLength":1,"maxLength":80},
            "source_readers":{"type":"object","additionalProperties":false,
                "properties":source_readers,
                "required":["directory","compensation","budget","approved-planning"]},
            "approvers":{"type":"array","minItems":1,"maxItems":4,"uniqueItems":true,"items":{"enum":["maya","nina","leo","sam"]}},
            "approval_minutes":{"type":"integer","minimum":1,"maximum":1440}
        },"required":["name","source_readers","approvers","approval_minutes"]});
    [
        ("trustgraph_demo_workspace", "Inspect the fictional Northstar company, source permissions, reviewers, and default policy. Start here. No private data or live services.", json!({"type":"object","properties":{},"additionalProperties":false})),
        ("trustgraph_demo_simulate", "Compare default and optional proposed policies against a hypothetical scenario using the Rust verifier. Approval is a simulation assumption. No policy activation, generation, publication, or external writes.", json!({"type":"object","properties":{"scenario":scenario,"policy":policy},"required":["scenario"],"additionalProperties":false})),
    ].into_iter().map(|(name, description, schema)| {
        Tool::new(name, description, schema.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(true).destructive(false).idempotent(true).open_world(false))
    }).collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Simulation {
    scenario: Scenario,
    policy: Option<StoryPolicy>,
}

impl ServerHandler for Demo {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("trustgraph-demo", env!("CARGO_PKG_VERSION")))
            .with_instructions("Public fictional Northstar demo. Inspect the workspace, then simulate source permissions and approval assumptions using the real Rust verifier. All people, data, and approvals are hypothetical. No credentials, live agent execution, stored data, or external writes. These tools cannot authorize any real publication. Use the browser demo for the interactive walkthrough.")
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|tool| tool.name == name)
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
        let args = Value::Object(request.arguments.unwrap_or_default());
        let result = match request.name.as_ref() {
            "trustgraph_demo_workspace" if args.as_object().is_some_and(|args| args.is_empty()) => {
                Story::new().map(|story| story.view())
            }
            "trustgraph_demo_simulate" => {
                let input: Simulation = serde_json::from_value(args).map_err(|_| {
                    ErrorData::invalid_params(
                        "Use the advertised scenario and policy schema.",
                        None,
                    )
                })?;
                Story::new().and_then(|story| {
                    story.compare(&input.policy.unwrap_or_default(), &input.scenario)
                })
            }
            _ => {
                return Err(ErrorData::invalid_params(
                    "Unknown tool or unsupported arguments.",
                    None,
                ));
            }
        };
        Ok(match result {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::structured_error(
                json!({"error":error.to_string(),"external_writes":0}),
            ),
        }
        .into())
    }
}

pub fn router() -> Router {
    let mut hosts = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "trustgraph-demo.vercel.app".to_owned(),
    ];
    for variable in [
        "VERCEL_URL",
        "VERCEL_PROJECT_PRODUCTION_URL",
        "VERCEL_BRANCH_URL",
    ] {
        if let Ok(host) = std::env::var(variable) {
            hosts.push(host);
        }
    }
    let origins: Vec<_> = hosts
        .iter()
        .map(|host| format!("https://{host}:443"))
        .collect();
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(32_768)
        .with_allowed_hosts(hosts)
        .with_allowed_origins(origins);
    let service = StreamableHttpService::new(
        || Ok(Demo),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new()
        .route_service("/api/mcp", service.clone())
        .route_service("/mcp", service)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    async fn rpc(method: &str, params: Value) -> Value {
        let response = router()
            .oneshot(
                Request::post("/mcp")
                    .header("host", "localhost")
                    .header("accept", "application/json, text/event-stream")
                    .header("content-type", "application/json")
                    .header("mcp-protocol-version", "2025-03-26")
                    .body(Body::from(
                        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn anonymous_stateless_mcp_exposes_only_fictional_workspace_and_simulation() {
        let initialized = rpc("initialize", json!({"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1"}})).await;
        assert_eq!(
            initialized["result"]["serverInfo"]["name"],
            "trustgraph-demo"
        );
        let catalog = rpc("tools/list", json!({})).await;
        let tools = catalog["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert!(
            tools
                .iter()
                .all(|tool| tool["annotations"]["readOnlyHint"] == true)
        );
        let workspace = rpc(
            "tools/call",
            json!({"name":"trustgraph_demo_workspace","arguments":{}}),
        )
        .await;
        assert_eq!(
            workspace["result"]["structuredContent"]["external_writes"],
            0
        );
        let params = json!({"name":"trustgraph_demo_simulate","arguments":{"scenario":{"inputs":["directory","compensation"],"destination":"engineering","approval":"valid","complete_lineage":true}}});
        let simulated = rpc("tools/call", params.clone()).await;
        assert_eq!(
            simulated["result"]["structuredContent"]["active"]["code"],
            "RESTRICTED_SOURCE"
        );
        assert_eq!(rpc("tools/call", params).await, simulated);
        let denied = rpc(
            "tools/call",
            json!({"name":"trustgraph_submit_work","arguments":{"goal":"publish"}}),
        )
        .await;
        assert!(denied.get("error").is_some());
    }
}
