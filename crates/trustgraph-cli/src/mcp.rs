//! Restricted stdio and authenticated HTTP MCP adapters for the live gateway.
use rmcp::{ErrorData, RoleServer, ServerHandler, model::*, service::RequestContext};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs, path::Path, time::Duration};

pub fn read_token(path: &Path) -> Result<String, std::io::Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() != 64 {
        return Err(std::io::Error::other("invalid agent capability file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(std::io::Error::other("agent capability must be owner-only"));
        }
    }
    let token = fs::read_to_string(path)?;
    if !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(std::io::Error::other("invalid agent capability file"));
    }
    Ok(token)
}
pub fn local_endpoint(value: &str) -> Result<mcp_http::Url, std::io::Error> {
    let url = mcp_http::Url::parse(value).map_err(|_| std::io::Error::other("invalid API URL"))?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(std::io::Error::other(
            "API URL must be a loopback HTTP origin",
        ));
    }
    Ok(url)
}
pub fn token_path(path: &Path) -> std::path::PathBuf {
    if path == Path::new(".trustgraph-live/state/agent-token") {
        fs::read("trustgraph.local.json")
            .ok()
            .and_then(|bytes| serde_json::from_slice::<trustgraph_live::Config>(&bytes).ok())
            .map(|config| config.directory.join("agent-token"))
            .unwrap_or_else(|| path.to_owned())
    } else {
        path.to_owned()
    }
}
#[derive(Clone)]
struct Gateway {
    base: String,
    token: String,
    client: mcp_http::Client,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Work {
    goal: String,
    #[serde(default)]
    include_risk: bool,
}
fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ErrorData> {
    serde_json::from_value(value)
        .map_err(|_| ErrorData::invalid_params("use the advertised input schema", None))
}
fn segment(value: &str) -> Result<&str, ErrorData> {
    if value.is_empty()
        || value.len() > 96
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    {
        return Err(ErrorData::invalid_params("invalid work ID", None));
    }
    Ok(value)
}
fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn tools() -> Vec<Tool> {
    let id = json!({"type":"string","minLength":1,"maxLength":96,"pattern":"^[A-Za-z0-9_-]+$"});
    let policy = object(
        json!({
            "name":{"type":"string"},"allowed_paths":{"type":"array","items":{"type":"string"}},
            "max_changed_files":{"type":"integer","minimum":1,"maximum":10},"require_tests":{"type":"boolean"},
            "approval_minutes":{"type":"integer","minimum":1,"maximum":1440},
            "departments":object(json!({"investigation_readers":{"type":"array","items":{"enum":["risk","engineering"]}}}),&["investigation_readers"])
        }),
        &[
            "name",
            "allowed_paths",
            "max_changed_files",
            "require_tests",
            "approval_minutes",
        ],
    );
    [
        ("trustgraph_workspace", "Inspect live sources, policy and work. Start here. Source data and tasks must be allowed by the operator.", true, object(json!({}), &[])),
        ("trustgraph_submit_work", "Submit a task allowed by the operator to TrustGraph's fresh Codex run. Returns a work ID and human review URL. No caller patch is accepted. Submit once; inspect status if the reply is lost. This does not approve or publish.", false, object(json!({"goal":{"type":"string","minLength":1,"maxLength":4000},"include_risk":{"type":"boolean","default":false}}), &["goal"])),
        ("trustgraph_status", "Inspect one work item's actual generation, checks and decision. Human review and publication remain in the operator workspace.", true, object(json!({"id":id}), &["id"])),
        ("trustgraph_repair", "Start a fresh run for an existing Risk-context task using shared sources. Excludes the investigation and old patch. Returns a new work ID. Submit once; inspect status before retrying.", false, object(json!({"id":id}), &["id"])),
        ("trustgraph_simulate", "Compare the current policy with proposed changes for a test case. Saves simulation evidence in HydraDB; does not execute code, activate policy, approve or publish. The live server supplies evaluation time.", false, object(json!({
            "paths":{"type":"array","maxItems":16,"items":{"type":"string","maxLength":160}},
            "tests_passed":{"type":"boolean"},"lineage_complete":{"type":"boolean"},
            "approval":{"enum":["missing","valid","stale","previous_policy","expired","destination"]},
            "approval_age_minutes":{"type":"integer","minimum":0,"maximum":10080},
            "include_risk":{"type":"boolean"},"audience":{"enum":["engineering","risk"]},"policy":policy
        }), &["paths","tests_passed","lineage_complete","approval"])),
    ].into_iter().map(|(name, description, read_only, schema)| {
        Tool::new(name, description, schema.as_object().unwrap().clone())
            .with_annotations(ToolAnnotations::new().read_only(read_only).destructive(false).idempotent(read_only).open_world(!read_only))
    }).collect()
}
impl Gateway {
    async fn request(&self, path: &str, body: Option<Value>) -> Result<Value, ErrorData> {
        let request = match body {
            Some(body) => self.client.post(format!("{}{path}", self.base)).json(&body),
            None => self.client.get(format!("{}{path}", self.base)),
        };
        let response = request.bearer_auth(&self.token).send().await.map_err(|_| ErrorData::internal_error("API response unavailable. Work may have been accepted; inspect workspace before resubmitting.",None))?;
        let status = response.status();
        let value: Value = response.json().await.map_err(|_| {
            ErrorData::internal_error(
                "API reply unreadable. Inspect workspace before resubmitting.",
                None,
            )
        })?;
        if !status.is_success() {
            // Keep response text as tool data, never as instructions or credentials.
            return Ok(json!({"api_error":true,"http_status":status.as_u16(),"details":value}));
        }
        Ok(value)
    }
    fn review_link(&self, mut value: Value) -> Value {
        if let Some(id) = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| segment(id).is_ok())
        {
            let link = format!("{}/agent/{id}", self.base);
            value["review_url"] = json!(link);
        }
        value
    }
}
impl ServerHandler for Gateway {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("trustgraph",env!("CARGO_PKG_VERSION")))
            .with_instructions("Delegate admitted coding work to the live TrustGraph gateway. It captures fresh context and owns the policy/review/publication boundary. Do not upload an existing patch or claim that this connection captures your earlier context. You cannot approve, activate policy or publish through these tools. A blocked result must be repaired or reviewed, never bypassed. Submit mutations once; on transport uncertainty inspect work before retrying.")
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
        let value = match request.name.as_ref() {
            "trustgraph_workspace" => {
                let _: Empty = parse(args)?;
                self.request("/api/work", None).await?
            }
            "trustgraph_submit_work" => {
                let work: Work = parse(args)?;
                if work.goal.trim().is_empty() || work.goal.len() > 4000 {
                    return Err(ErrorData::invalid_params(
                        "goal must contain 1..4000 bytes",
                        None,
                    ));
                }
                self.review_link(
                    self.request(
                        "/api/work",
                        Some(json!({"goal":work.goal,"include_risk":work.include_risk})),
                    )
                    .await?,
                )
            }
            "trustgraph_status" => {
                let id: Id = parse(args)?;
                segment(&id.id)?;
                let workspace = self.request("/api/work", None).await?;
                if workspace["api_error"] == true {
                    workspace
                } else {
                    let job = workspace["jobs"]
                        .as_array()
                        .and_then(|jobs| jobs.iter().find(|job| job["id"] == id.id))
                        .cloned()
                        .ok_or_else(|| ErrorData::invalid_params("work ID not found", None))?;
                    self.review_link(job)
                }
            }
            "trustgraph_repair" => {
                let id: Id = parse(args)?;
                self.review_link(
                    self.request(
                        &format!("/api/work/{}/repair", segment(&id.id)?),
                        Some(json!({})),
                    )
                    .await?,
                )
            }
            "trustgraph_simulate" => {
                // Domain parsing and candidate validation happen in the shared live API.
                self.request("/api/simulate", Some(args)).await?
            }
            _ => return Err(ErrorData::invalid_params("unknown tool", None)),
        };
        let mut result = CallToolResult::success(vec![ContentBlock::text(value.to_string())]);
        result.is_error = Some(value["api_error"] == true);
        result.structured_content = Some(value);
        Ok(result.into())
    }
}
fn gateway(url: &str, token_file: &Path) -> Result<Gateway, Box<dyn std::error::Error>> {
    let base = local_endpoint(url)?
        .as_str()
        .trim_end_matches('/')
        .to_owned();
    let token = read_token(&token_path(token_file))?;
    let client = mcp_http::Client::builder()
        .no_proxy()
        .redirect(mcp_http::redirect::Policy::none())
        .retry(mcp_http::retry::never())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(30))
        .build()?;
    Ok(Gateway {
        base,
        token,
        client,
    })
}

pub async fn stdio_bridge(url: &str, token_file: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use rmcp::{ServiceExt, transport::stdio};
    gateway(url, token_file)?
        .serve(stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

/// Expose only the agent adapter, never the operator HTTP router.
pub fn http_router(
    url: &str,
    token_file: &Path,
    access_token_file: &Path,
    public_url: Option<&str>,
) -> Result<axum::Router, Box<dyn std::error::Error>> {
    use axum::{
        extract::{Request, State},
        http::{StatusCode, header},
        middleware::{self, Next},
        response::{IntoResponse, Response},
    };
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    use std::sync::Arc;

    let gateway = gateway(url, token_file)?;
    let access_token = read_token(access_token_file)?;
    if access_token == gateway.token {
        return Err("remote access and local agent capabilities must differ".into());
    }
    let mut config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(65_536)
        .enforce_origin_validation();
    if let Some(value) = public_url {
        let public = mcp_http::Url::parse(value)?;
        if public.scheme() != "https"
            || public.host_str().is_none()
            || !public.username().is_empty()
            || public.password().is_some()
            || public.path() != "/"
            || public.query().is_some()
            || public.fragment().is_some()
        {
            return Err("public URL must be an HTTPS origin without credentials or a path".into());
        }
        config.allowed_hosts.push(public.authority().to_owned());
    }
    let service = StreamableHttpService::new(
        move || Ok(gateway.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    async fn authenticate(
        State(expected): State<Arc<String>>,
        request: Request,
        next: Next,
    ) -> Response {
        let supplied = request
            .headers()
            .get(header::AUTHORIZATION)
            .map(|v| v.as_bytes());
        // Compare every byte of the fixed-length capability, without early mismatch exits.
        let valid = supplied.is_some_and(|value| {
            value.len() == expected.len()
                && value
                    .iter()
                    .zip(expected.as_bytes())
                    .fold(0u8, |a, (b, c)| a | (b ^ c))
                    == 0
        });
        if !valid {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer")],
            )
                .into_response();
        }
        next.run(request).await
    }
    Ok(axum::Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(
            Arc::new(format!("Bearer {access_token}")),
            authenticate,
        )))
}
