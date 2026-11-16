//! MCP over streamable HTTP at `/mcp`, on the same port as the HTTP API.
//!
//! Every request is authenticated by the agent's bearer token (and charged
//! to its rate limit) before rmcp sees it; the agent's name travels to the
//! tool handler only inside the request's extensions, never in arguments.
//! The server is stateless: no MCP session can carry one agent's identity to
//! another request. No tool can approve, revoke or activate.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::request::Parts;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};

use super::{
    ApiError, BODY_LIMIT, CheckRequest, DiffRequest, MAX_SIMULATE_COUNT, MAX_SIMULATE_STEPS,
    OutputRequest, PolicyRequest, ServeJournal, Server, SourceRequest, policy_helper,
};

/// The authenticated agent of one MCP request.
#[derive(Debug, Clone)]
struct Agent(String);

/// The tool names, in the order they are listed.
pub const TOOLS: [&str; 12] = [
    "register_source",
    "record_output",
    "check",
    "explain_artifact",
    "policy_get",
    "policy_schema",
    "policy_check",
    "policy_test",
    "policy_explain",
    "policy_diff",
    "policy_simulate",
    "policy_propose",
];

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn tools() -> Vec<Tool> {
    let name = json!({"type": "string", "minLength": 1, "maxLength": 256});
    let sha = json!({"type": "string", "pattern": "^[0-9a-f]{64}$", "description": "SHA-256 of the content, 64 lowercase hex characters"});
    let content = json!({"type": "string", "description": "The text itself; the server hashes it and does not store it"});
    let policy = json!({"type": "string", "description": "A trustgraph.policy.yaml text"});
    let read_only = |t: Tool| {
        t.with_annotations(
            ToolAnnotations::new()
                .read_only(true)
                .destructive(false)
                .idempotent(true)
                .open_world(false),
        )
    };
    let writes = |t: Tool| {
        t.with_annotations(
            ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(true)
                .open_world(false),
        )
    };
    let tool = |n: &'static str, d: &'static str, schema: Value| {
        Tool::new(n, d, schema.as_object().cloned().unwrap_or_default())
    };
    vec![
        writes(tool(
            "register_source",
            "Call when you read a source named in the policy, before you use it: send the source name and its content or sha256. Returns the artifact id to list as an input of anything you write from it.",
            object(
                json!({"source": name, "sha256": sha, "content": content}),
                &["source"],
            ),
        )),
        writes(tool(
            "record_output",
            "Call after you produce something, listing the artifact ids it was made from. Returns the output's artifact id; check it before sending it anywhere.",
            object(
                json!({
                    "inputs": {"type": "array", "minItems": 1, "maxItems": 64, "items": name},
                    "step": name,
                    "sha256": sha,
                    "content": content,
                }),
                &["inputs", "step"],
            ),
        )),
        writes(tool(
            "check",
            "Ask whether an output may go through an action to its recipients. Call immediately before acting and act only on allow; needs_approval means a human must approve first, deny means do not act.",
            object(
                json!({
                    "artifact": name,
                    "action": name,
                    "recipients": {"type": ["array", "null"], "maxItems": 256, "items": name, "description": "People or groups; null or absent means the action's whole audience"},
                }),
                &["artifact", "action"],
            ),
        )),
        read_only(tool(
            "explain_artifact",
            "Show which sources an artifact derives from and who may read it under the active policy.",
            object(json!({"artifact": name}), &["artifact"]),
        )),
        read_only(tool(
            "policy_get",
            "Return the active policy as YAML with its revision and digest.",
            object(json!({}), &[]),
        )),
        read_only(tool(
            "policy_schema",
            "Return the JSON Schema of the trustgraph.policy.yaml format.",
            object(json!({}), &[]),
        )),
        read_only(tool(
            "policy_check",
            "Validate a policy text and list its errors and warnings. Changes nothing.",
            object(json!({"policy": policy}), &["policy"]),
        )),
        read_only(tool(
            "policy_test",
            "Run the inline tests of a policy text. Changes nothing.",
            object(json!({"policy": policy}), &["policy"]),
        )),
        read_only(tool(
            "policy_explain",
            "Describe in plain English what a policy text allows. Changes nothing.",
            object(json!({"policy": policy}), &["policy"]),
        )),
        read_only(tool(
            "policy_diff",
            "Show who gains or loses access between two policy texts; old defaults to the active policy. Changes nothing.",
            object(json!({"old": policy, "new": policy}), &["new"]),
        )),
        read_only(tool(
            "policy_simulate",
            "Run a policy text through seeded random scenarios on the real runtime and report any safety-invariant violations. Changes nothing.",
            object(
                json!({
                    "policy": policy,
                    "seed": {"type": "integer", "minimum": 0},
                    "count": {"type": "integer", "minimum": 1, "maximum": MAX_SIMULATE_COUNT},
                    "steps": {"type": "integer", "minimum": 1, "maximum": MAX_SIMULATE_STEPS},
                }),
                &["policy"],
            ),
        )),
        writes(tool(
            "policy_propose",
            "Submit a policy text for the operator to review once check, test and simulate are clean. It never changes the active policy; only the operator can activate it.",
            object(json!({"policy": policy}), &["policy"]),
        )),
    ]
}

/// The MCP face of one server.
struct Tools<J: ServeJournal> {
    server: Arc<Server<J>>,
}

impl<J: ServeJournal> Clone for Tools<J> {
    fn clone(&self) -> Self {
        Tools {
            server: self.server.clone(),
        }
    }
}

fn args<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ApiError> {
    serde_json::from_value(value).map_err(|e| {
        ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "invalid_request",
            format!("the arguments are not valid: {e}"),
        )
    })
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactArgs {
    artifact: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

impl<J: ServeJournal> Tools<J> {
    /// Run one tool as `agent`. The same server methods as the HTTP API.
    fn call(
        server: &Server<J>,
        agent: &str,
        name: &str,
        arguments: Value,
    ) -> Result<Value, ApiError> {
        match name {
            "register_source" => server.register_source(agent, args::<SourceRequest>(arguments)?),
            "record_output" => server.record_output(agent, args::<OutputRequest>(arguments)?),
            "check" => server.check(agent, args::<CheckRequest>(arguments)?),
            "explain_artifact" => {
                let a: ArtifactArgs = args(arguments)?;
                server.explain_artifact(agent, &a.artifact)
            }
            "policy_get" => {
                let _: NoArgs = args(arguments)?;
                server.policy_get()
            }
            "policy_schema" => {
                let _: NoArgs = args(arguments)?;
                Ok(trustgraph_policy::schema())
            }
            "policy_check" => policy_helper("check", arguments),
            "policy_test" => policy_helper("test", arguments),
            "policy_explain" => policy_helper("explain", arguments),
            "policy_simulate" => policy_helper("simulate", arguments),
            "policy_diff" => server.policy_diff(args::<DiffRequest>(arguments)?),
            "policy_propose" => server.propose(Some(agent), args::<PolicyRequest>(arguments)?),
            _ => Err(ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "unknown_tool",
                "no such tool",
            )),
        }
    }
}

impl<J: ServeJournal> ServerHandler for Tools<J> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("trustgraph", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "TrustGraph decides where your output may go based on what you read. Register each source you read, record each output with the artifacts it came from, and call check immediately before acting; act only on allow. You cannot approve, revoke or change the active policy; policy_propose hands a change to the operator.",
            )
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
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // Fail closed: without the middleware's identity there is no agent.
        let agent = context
            .extensions
            .get::<Parts>()
            .and_then(|parts| parts.extensions.get::<Agent>())
            .cloned()
            .ok_or_else(|| ErrorData::invalid_request("an agent bearer token is required", None))?;
        let name = request.name.to_string();
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let server = self.server.clone();
        let outcome =
            tokio::task::spawn_blocking(move || Self::call(&server, &agent.0, &name, arguments))
                .await
                .unwrap_or_else(|_| Err(ApiError::internal("the request failed")));
        let (value, is_error) = match outcome {
            Ok(value) => (value, false),
            Err(error) => {
                let mut body = error.body();
                body["status"] = json!(error.status.as_u16());
                (body, true)
            }
        };
        let mut result = CallToolResult::success(vec![ContentBlock::text(value.to_string())]);
        result.is_error = Some(is_error);
        result.structured_content = Some(value);
        Ok(result.into())
    }
}

/// Authenticate the agent and charge its rate limit before rmcp sees the request.
async fn authenticate<J: ServeJournal>(
    State(server): State<Arc<Server<J>>>,
    mut request: Request,
    next: Next,
) -> Response {
    match server.authenticate_agent(request.headers()) {
        Ok(agent) => {
            request.extensions_mut().insert(Agent(agent));
            next.run(request).await
        }
        Err(error) => error.into_response(),
    }
}

pub fn router<J: ServeJournal>(server: Arc<Server<J>>) -> Router<Arc<Server<J>>> {
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(BODY_LIMIT)
        .enforce_origin_validation();
    let tools = Tools {
        server: server.clone(),
    };
    let service = StreamableHttpService::new(
        move || Ok(tools.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(server, authenticate::<J>))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_exactly_the_twelve_and_none_can_approve() {
        let listed: Vec<String> = tools().iter().map(|t| t.name.to_string()).collect();
        assert_eq!(listed, TOOLS);
        for tool in tools() {
            let name = tool.name.to_lowercase();
            for word in ["approve", "revoke", "activate"] {
                assert!(!name.contains(word), "{name}");
            }
            let description = tool.description.as_deref().unwrap_or_default();
            assert!(!description.is_empty() && description.len() < 300, "{name}");
        }
        let check = tools().into_iter().find(|t| t.name == "check").unwrap();
        assert!(
            check
                .description
                .unwrap()
                .to_lowercase()
                .contains("call immediately before acting and act only on allow")
        );
    }
}
