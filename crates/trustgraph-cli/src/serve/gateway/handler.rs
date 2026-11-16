//! `/gateway/mcp`: the MCP face of the gateway, over rmcp's stateful
//! streamable HTTP transport.
//!
//! Every request is authenticated by the agent's bearer token (and charged
//! to its rate limit) before rmcp sees it. In `connection` mode each MCP
//! session is bound to the agent that opened it, and only that agent may use
//! it. Only tools are offered: no resources, prompts, sampling or
//! elicitation. The protocol versions offered are those with sessions.

use std::borrow::Cow;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};

use super::{Gateway, HANDOFF_TOOL, RECEIVE_TOOL};
use crate::serve::{ApiError, BODY_LIMIT, ServeJournal, Server};

const SESSION_HEADER: &str = "mcp-session-id";
/// Longest upstream tool description passed to agents, in characters.
const MAX_DESCRIPTION: usize = 1024;
/// Largest upstream input schema passed to agents, serialized.
const MAX_SCHEMA_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
struct Agent(String);

pub(super) struct Handler<J: ServeJournal> {
    gateway: Arc<Gateway<J>>,
}

impl<J: ServeJournal> Clone for Handler<J> {
    fn clone(&self) -> Self {
        Handler {
            gateway: self.gateway.clone(),
        }
    }
}

/// An upstream description, which is untrusted text: control characters
/// stripped, length capped.
fn description(text: Option<&str>) -> String {
    let clean: String = text
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_DESCRIPTION)
        .collect();
    clean.trim().to_string()
}

fn builtins() -> Vec<Tool> {
    let object = |properties: Value, required: &[&str]| -> JsonObject {
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
        .as_object()
        .cloned()
        .unwrap_or_default()
    };
    vec![
        Tool::new(
            HANDOFF_TOOL,
            "Hand what this session has read to another agent: returns an opaque token. Give it to the next agent, which passes it to trustgraph__receive. What you read follows your output.",
            object(json!({}), &[]),
        ),
        Tool::new(
            RECEIVE_TOOL,
            "Add a hand-off token from another agent to this session, so what it read counts as read here too. Refused when this agent is not cleared for something it carries.",
            object(
                json!({"token": {"type": "string", "description": "A token from trustgraph__handoff"}}),
                &["token"],
            ),
        ),
    ]
}

impl<J: ServeJournal> ServerHandler for Handler<J> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "trustgraph-gateway",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "These tools pass through the TrustGraph gateway. What you read through them is recorded, and every action is checked against it before it runs; a refused action did not run. To pass what you read to another agent, call trustgraph__handoff and give it the token; it calls trustgraph__receive.",
            )
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        // Versions with MCP sessions, which `connection` mode needs.
        Cow::Owned(vec![
            ProtocolVersion::V_2025_11_25,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_03_26,
        ])
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = builtins();
        let config = self.gateway.config();
        for upstream in self.gateway.upstreams.all() {
            // An upstream that cannot be reached lists nothing; its tools
            // still fail closed when called.
            let Ok(offered) = upstream.tools().await else {
                continue;
            };
            for tool in config.tools.values() {
                if !self
                    .gateway
                    .upstreams
                    .get(&tool.upstream)
                    .is_some_and(|u| Arc::ptr_eq(u, upstream))
                {
                    continue;
                }
                let Some(found) = offered.iter().find(|t| t.name == tool.name) else {
                    continue;
                };
                let schema_fits = serde_json::to_vec(&*found.input_schema)
                    .is_ok_and(|b| b.len() <= MAX_SCHEMA_BYTES);
                let schema = if schema_fits {
                    (*found.input_schema).clone()
                } else {
                    json!({"type": "object"})
                        .as_object()
                        .cloned()
                        .unwrap_or_default()
                };
                tools.push(Tool::new(
                    tool.exposed.clone(),
                    description(found.description.as_deref()),
                    schema,
                ));
            }
        }
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Fail closed: without the middleware's identity there is no agent.
        let parts = context.extensions.get::<Parts>();
        let agent = parts
            .and_then(|p| p.extensions.get::<Agent>())
            .cloned()
            .ok_or_else(|| ErrorData::invalid_request("an agent bearer token is required", None))?;
        let mcp = parts
            .and_then(|p| p.headers.get(SESSION_HEADER))
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let result = self
            .gateway
            .call(
                agent.0,
                mcp,
                request.name.to_string(),
                request.arguments.unwrap_or_default(),
            )
            .await;
        Ok(result.into())
    }
}

/// Authenticate, charge the rate limit, and keep each MCP session to the
/// agent that opened it.
async fn authenticate<J: ServeJournal>(
    State(gateway): State<Arc<Gateway<J>>>,
    mut request: Request,
    next: Next,
) -> Response {
    let server: &Server<J> = &gateway.server;
    let agent = match server.authenticate_agent(request.headers()) {
        Ok(agent) => agent,
        Err(error) => return error.into_response(),
    };
    let presented = request
        .headers()
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    if let Some(mcp) = &presented
        && !gateway.may_use(mcp, &agent)
    {
        return ApiError::new(
            StatusCode::NOT_FOUND,
            "unknown_session",
            "no MCP session with that id for this agent",
        )
        .into_response();
    }
    request.extensions_mut().insert(Agent(agent.clone()));
    let response = next.run(request).await;
    if presented.is_none()
        && let Some(created) = response
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
        && !gateway.bind(&created, &agent)
    {
        gateway.shared.close_mcp(&created);
        return ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_sessions",
            "this agent has the most open gateway sessions allowed; close some, or the operator can end them on the operator page",
        )
        .into_response();
    }
    response
}

pub(super) fn router<J: ServeJournal>(gateway: Arc<Gateway<J>>) -> Router<Arc<Server<J>>> {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(true)
        .with_max_request_body_bytes(BODY_LIMIT)
        .enforce_origin_validation();
    let handler = Handler {
        gateway: gateway.clone(),
    };
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
        gateway.shared.manager.clone(),
        config,
    );
    Router::new()
        .route_service("/gateway/mcp", service)
        .layer(middleware::from_fn_with_state(gateway, authenticate::<J>))
}
