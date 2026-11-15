//! Fixed Composio Connect MCP route. No generic agent-controlled MCP execution.
use super::*;
use std::io::{BufRead, BufReader};

pub(super) const ENDPOINT: &str = "https://connect.composio.dev/mcp";
const LIMIT: u64 = 1_048_576;

struct Client<'a> {
    http: reqwest::blocking::Client,
    endpoint: &'a str,
    key: &'a str,
    protocol: String,
    session: Option<String>,
    sequence: u64,
}
impl<'a> Client<'a> {
    fn open(publisher: &'a IssuePublisher) -> std::result::Result<Self, ()> {
        let mut client = Self {
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(40))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| ())?,
            endpoint: &publisher.endpoint,
            key: publisher.api_key.as_deref().ok_or(())?,
            protocol: "2025-03-26".into(),
            session: None,
            sequence: 0,
        };
        let init = client.rpc(
            "initialize",
            json!({"protocolVersion":"2025-03-26",
            "capabilities":{},"clientInfo":{"name":"trustgraph-issue-gateway","version":"1"}}),
        )?;
        let protocol = init["protocolVersion"].as_str().ok_or(())?;
        if protocol != "2025-03-26" {
            return Err(());
        }
        client.protocol = protocol.into();
        let response =
            client.post(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
        if !response.status().is_success() {
            return Err(());
        }
        Ok(client)
    }
    fn post(&self, body: &Value) -> std::result::Result<reqwest::blocking::Response, ()> {
        let mut request = self
            .http
            .post(self.endpoint)
            .header("x-consumer-api-key", self.key)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", &self.protocol)
            .json(body);
        if let Some(session) = &self.session {
            request = request.header("Mcp-Session-Id", session);
        }
        request.send().map_err(|_| ())
    }
    fn rpc(&mut self, method: &str, params: Value) -> std::result::Result<Value, ()> {
        self.sequence += 1;
        let id = self.sequence;
        let response =
            self.post(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        if !response.status().is_success() {
            return Err(());
        }
        if let Some(session) = response.headers().get("Mcp-Session-Id") {
            self.session = Some(session.to_str().map_err(|_| ())?.into());
        }
        let sse = response
            .headers()
            .get("Content-Type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        let reply = if sse {
            let mut reader = BufReader::new(response.take(LIMIT + 1));
            let mut consumed = 0;
            let mut event = String::new();
            loop {
                let mut line = String::new();
                let n = reader.read_line(&mut line).map_err(|_| ())?;
                consumed += n;
                if n == 0 || consumed as u64 > LIMIT {
                    return Err(());
                }
                if line.trim().is_empty() {
                    if !event.is_empty() {
                        let value: Value =
                            serde_json::from_str(event.trim_end()).map_err(|_| ())?;
                        if value["id"] == id {
                            break value;
                        }
                        event.clear();
                    }
                } else if let Some(data) = line.strip_prefix("data:") {
                    event.push_str(
                        data.strip_prefix(' ')
                            .unwrap_or(data)
                            .trim_end_matches(['\r', '\n']),
                    );
                    event.push('\n');
                }
            }
        } else {
            let mut bytes = Vec::new();
            response
                .take(LIMIT + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| ())?;
            if bytes.len() as u64 > LIMIT {
                return Err(());
            }
            serde_json::from_slice::<Value>(&bytes).map_err(|_| ())?
        };
        if reply["jsonrpc"] != "2.0" || reply["id"] != id || reply.get("error").is_some() {
            return Err(());
        }
        reply.get("result").cloned().ok_or(())
    }
    fn call(&mut self, name: &str, arguments: Value) -> std::result::Result<Value, ()> {
        let result = self.rpc("tools/call", json!({"name":name,"arguments":arguments}))?;
        if result["isError"] == true {
            return Err(());
        }
        let value = if let Some(value) = result.get("structuredContent") {
            value.clone()
        } else {
            result["content"]
                .as_array()
                .ok_or(())?
                .iter()
                .filter(|block| block["type"] == "text")
                .find_map(|block| serde_json::from_str::<Value>(block["text"].as_str()?).ok())
                .ok_or(())?
        };
        if value["successful"] != true {
            return Err(());
        }
        Ok(value)
    }
}

// Err means no mutation was attempted. Once the mutation request is sent, any
// unverified response is Unknown; the runtime will never implicitly resend it.
fn prepare(publisher: &IssuePublisher) -> std::result::Result<(Client<'_>, String), ()> {
    let config = &publisher.config;
    let mut client = Client::open(publisher)?;
    let search = client.call(
        "COMPOSIO_SEARCH_TOOLS",
        json!({
            "queries":[{"use_case":"Create a GitHub issue after human approval",
                "known_fields":format!("repository: {}", config.github_repository)}],
            "session":{"generate_id":true}
        }),
    )?;
    let account = config.connected_account_id.as_deref().ok_or(())?;
    let active = search["data"]["toolkit_connection_statuses"]
        .as_array()
        .ok_or(())?
        .iter()
        .any(|toolkit| {
            toolkit["toolkit"] == "github"
                && toolkit["has_active_connection"] == true
                && toolkit["accounts"].as_array().is_some_and(|accounts| {
                    accounts
                        .iter()
                        .any(|a| a["id"] == account && a["status"] == "ACTIVE")
                })
        });
    if !active {
        return Err(());
    }
    let session = search["data"]["session"]["id"].as_str().ok_or(())?;
    let schemas = client.call(
        "COMPOSIO_GET_TOOL_SCHEMAS",
        json!({
            "tool_slugs":[TOOL],"include":["input_schema"],"session_id":session
        }),
    )?;
    let schema = &schemas["data"]["tool_schemas"][TOOL]["input_schema"];
    if !schema.is_object()
        || config.tool_schema_hash.as_deref()
            != Some(
                digest::content(&serde_json::to_vec(schema).map_err(|_| ())?)
                    .0
                    .as_str(),
            )
    {
        return Err(());
    }
    Ok((client, session.into()))
}

pub(super) fn check(publisher: &IssuePublisher) -> std::result::Result<(), ()> {
    prepare(publisher).map(|_| ())
}

pub(super) fn execute(
    publisher: &IssuePublisher,
    envelope: &Envelope,
) -> std::result::Result<Option<String>, ()> {
    let (mut client, session) = prepare(publisher)?;
    let account = envelope.connected_account_id.as_deref().ok_or(())?;
    let (owner, repo) = envelope.repository.split_once('/').ok_or(())?;
    let reply = client.call("COMPOSIO_MULTI_EXECUTE_TOOL", json!({
        "tools":[{"tool_slug":TOOL,"account":account,"arguments":{
            "owner":owner,"repo":repo,"title":envelope.proposal.title,"body":envelope.proposal.body}}],
        "session_id":session,"sync_response_to_workbench":false,"current_step":"CREATE_APPROVED_ISSUE"
    }));
    let Ok(reply) = reply else {
        return Ok(None);
    };
    let Some(results) = reply["data"]["results"].as_array() else {
        return Ok(None);
    };
    if results.len() != 1 || results[0]["tool_slug"] != TOOL {
        return Ok(None);
    }
    Ok(issue_url(&results[0]["response"], envelope))
}
