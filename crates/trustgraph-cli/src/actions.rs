//! Exact-action approval for native 8090 proposals. No generation or inferred source lineage.
mod connect;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path as RoutePath, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use trustgraph_core::{digest, *};
use trustgraph_runtime::{
    Coordinator, Error, Submission,
    journal::DiskJournal,
    publisher::{Acknowledgment, Outcome, Payload, Publisher},
};

const TOOL: &str = "GITHUB_CREATE_AN_ISSUE";
const COMPOSIO: &str = "https://backend.composio.dev";
type Result<T> = std::result::Result<T, Error>;
type Gate = Coordinator<DiskJournal, IssuePublisher>;
type Shared = Arc<Mutex<App>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Preview,
    Live,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    #[default]
    ProjectApi,
    ConnectMcp,
}
impl Transport {
    fn is_project(&self) -> bool {
        *self == Self::ProjectApi
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub directory: PathBuf,
    pub github_repository: String,
    pub mode: Mode,
    #[serde(default)]
    pub connected_account_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub tool_version: Option<String>,
    #[serde(default)]
    pub composio_transport: Transport,
    #[serde(default)]
    pub tool_schema_hash: Option<String>,
    #[serde(default)]
    pub agent_public_url: Option<String>,
}
impl Config {
    fn validate(&self) -> Result<()> {
        let parts: Vec<_> = self.github_repository.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|p| {
                p.is_empty()
                    || *p == "."
                    || *p == ".."
                    || !p
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            })
        {
            return Err(invalid("configure one explicit owner/repository"));
        }
        if self.mode == Mode::Live {
            if self
                .connected_account_id
                .as_deref()
                .is_none_or(str::is_empty)
            {
                return Err(invalid("live mode needs a connected account"));
            }
            let version = self.tool_version.as_deref().unwrap_or("");
            if self.composio_transport == Transport::ConnectMcp {
                let hash = self.tool_schema_hash.as_deref().unwrap_or("");
                if self.tool_version.is_some()
                    || self.user_id.is_some()
                    || hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                {
                    return Err(invalid(
                        "Connect MCP needs a pinned input schema SHA-256; project user/version fields must be absent",
                    ));
                }
            } else if version.len() != 11
                || version.as_bytes()[8] != b'_'
                || !version
                    .bytes()
                    .enumerate()
                    .all(|(i, c)| i == 8 || c.is_ascii_digit())
            {
                return Err(invalid(
                    "live mode needs a connected account and pinned YYYYMMDD_NN tool version",
                ));
            }
        }
        if let Some(origin) = &self.agent_public_url {
            public_origin(origin)?;
        }
        Ok(())
    }
    fn scope(&self) -> digest::Digest {
        let mut policy = json!({
            "mode":self.mode,"repository":self.github_repository,"tool":TOOL,
            "account":self.connected_account_id,"user_id":self.user_id,"version":self.tool_version,
            "boundary":"submitted action only; native agent context is not captured",
        });
        // Preserve the existing preview/project policy encoding.
        if self.composio_transport == Transport::ConnectMcp {
            policy["transport"] = json!(self.composio_transport);
            policy["input_schema_sha256"] = json!(self.tool_schema_hash);
        }
        digest::structured("exact-issue-action-policy", &policy)
    }
    fn destination(&self) -> String {
        format!(
            "composio://github/{}/{}/issues",
            self.connected_account_id.as_deref().unwrap_or("preview"),
            self.github_repository
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkOrder {
    pub project_id: String,
    pub id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub key: String,
    pub title: String,
    pub body: String,
    pub work_order: WorkOrder,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    mode: Mode,
    tool: String,
    repository: String,
    connected_account_id: Option<String>,
    user_id: Option<String>,
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Transport::is_project")]
    transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_schema_sha256: Option<String>,
    proposal: Proposal,
}
impl Envelope {
    fn new(config: &Config, proposal: Proposal) -> Result<Self> {
        if proposal.key.is_empty()
            || proposal.key.len() > 64
            || !proposal
                .key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            || proposal.title.trim().is_empty()
            || proposal.title.len() > 160
            || proposal.body.len() > 32_000
            || proposal.work_order.project_id.is_empty()
            || proposal.work_order.project_id.len() > 96
            || proposal.work_order.id.is_empty()
            || proposal.work_order.id.len() > 96
        {
            return Err(invalid(
                "use a key of 1..64 letters/digits/_/-, a title of 1..160 bytes, body up to 32000 bytes and work-order identifiers",
            ));
        }
        Ok(Self {
            mode: config.mode.clone(),
            tool: TOOL.into(),
            repository: config.github_repository.clone(),
            connected_account_id: config.connected_account_id.clone(),
            user_id: config.user_id.clone(),
            version: config.tool_version.clone(),
            transport: config.composio_transport.clone(),
            input_schema_sha256: config.tool_schema_hash.clone(),
            proposal,
        })
    }
}

struct IssuePublisher {
    config: Config,
    api_key: Option<String>,
    endpoint: String,
    receipts: PathBuf,
}
impl IssuePublisher {
    fn new(config: Config) -> Result<Self> {
        let api_key = if config.mode == Mode::Live {
            let key = std::env::var("COMPOSIO_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    let path = config.directory.join("composio-key");
                    let metadata = fs::symlink_metadata(&path).ok()?;
                    if !metadata.is_file() || metadata.len() > 512 {
                        return None;
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if metadata.permissions().mode() & 0o077 != 0 {
                            return None;
                        }
                    }
                    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
                })
                .filter(|v| !v.trim().is_empty() && v.bytes().all(|b| (33..=126).contains(&b)));
            Some(key.ok_or_else(|| invalid("live mode needs COMPOSIO_API_KEY or a private composio-key file in the state directory"))?)
        } else {
            None
        };
        let receipts = config.directory.join("receipts");
        private_directory(&receipts)?;
        Ok(Self {
            endpoint: if config.composio_transport == Transport::ConnectMcp {
                connect::ENDPOINT.into()
            } else {
                COMPOSIO.into()
            },
            config,
            api_key,
            receipts,
        })
    }
    fn receipt_path(&self, key: &str) -> PathBuf {
        self.receipts.join(format!(
            "{}.json",
            digest::structured("issue-operation", &key).0
        ))
    }
    fn save_receipt(&self, ack: &Acknowledgment) -> Result<()> {
        let path = self.receipt_path(&ack.payload.operation);
        let temporary = path.with_extension("tmp");
        let mut file = private_options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(storage)?;
        file.write_all(&serde_json::to_vec(ack).map_err(storage)?)
            .and_then(|_| file.sync_all())
            .map_err(storage)?;
        fs::rename(temporary, path).map_err(storage)?;
        File::open(&self.receipts)
            .and_then(|f| f.sync_all())
            .map_err(storage)
    }
    fn execute(&self, envelope: &Envelope) -> Result<Option<String>> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(25))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(storage)?;
        let (owner, repo) = envelope
            .repository
            .split_once('/')
            .ok_or_else(|| invalid("invalid repository"))?;
        let response = client.post(format!("{}/api/v3.1/tools/execute/{TOOL}",self.endpoint))
            .header("x-api-key",self.api_key.as_deref().ok_or_else(|| invalid("Composio key unavailable"))?)
            .json(&json!({"connected_account_id":envelope.connected_account_id,"user_id":envelope.user_id,
                "version":envelope.version,"arguments":{"owner":owner,"repo":repo,
                    "title":envelope.proposal.title,"body":envelope.proposal.body}})).send();
        let Ok(response) = response else {
            return Ok(None);
        };
        if !response.status().is_success() {
            return Ok(None);
        }
        let mut bytes = vec![];
        if response.take(1_048_577).read_to_end(&mut bytes).is_err() || bytes.len() > 1_048_576 {
            return Ok(None);
        }
        let Ok(reply) = serde_json::from_slice::<Value>(&bytes) else {
            return Ok(None);
        };
        Ok(issue_url(&reply, envelope))
    }
}

fn issue_url(reply: &Value, envelope: &Envelope) -> Option<String> {
    if reply["successful"] != true {
        return None;
    }
    if reply["data"]["title"].as_str() != Some(envelope.proposal.title.as_str())
        || reply["data"]["body"].as_str().unwrap_or("") != envelope.proposal.body
    {
        return None;
    }
    let number = reply["data"]["number"].as_u64().filter(|n| *n > 0)?;
    let url = format!("https://github.com/{}/issues/{number}", envelope.repository);
    if reply["data"]["html_url"].as_str() != Some(url.as_str()) {
        return None;
    }
    Some(url)
}
impl Publisher for IssuePublisher {
    fn simulated(&self) -> bool {
        self.config.mode == Mode::Preview
    }
    fn send(&mut self, payload: &Payload) -> Result<(Outcome, Option<Acknowledgment>)> {
        if payload.hash != digest::content(payload.content.as_bytes())
            || payload.destination != self.config.destination()
        {
            return Err(invalid(
                "approved action does not match the configured destination",
            ));
        }
        if let Some(existing) = self.lookup(&payload.operation)? {
            return if existing.payload == *payload {
                Ok((Outcome::Accepted, Some(existing)))
            } else {
                Err(Error::Conflict)
            };
        }
        let envelope: Envelope = serde_json::from_str(&payload.content).map_err(storage)?;
        let expected = Envelope::new(&self.config, envelope.proposal.clone())?;
        if serde_json::to_value(&envelope).map_err(storage)?
            != serde_json::to_value(expected).map_err(storage)?
        {
            return Err(invalid(
                "action tool, account, version or repository changed",
            ));
        }
        let provider_id = if self.config.mode == Mode::Preview {
            format!("preview:{}", payload.operation)
        } else {
            let result = if self.config.composio_transport == Transport::ConnectMcp {
                match connect::execute(self, &envelope) {
                    Ok(result) => result,
                    Err(()) => return Ok((Outcome::Rejected, None)),
                }
            } else {
                self.execute(&envelope)?
            };
            let Some(url) = result else {
                return Ok((Outcome::Unknown, None));
            };
            url
        };
        let ack = Acknowledgment {
            provider_id,
            payload: payload.clone(),
        };
        self.save_receipt(&ack)?;
        Ok((Outcome::Accepted, Some(ack)))
    }
    fn lookup(&mut self, operation: &str) -> Result<Option<Acknowledgment>> {
        let path = self.receipt_path(operation);
        if !path.exists() {
            return Ok(None);
        }
        let ack: Acknowledgment =
            serde_json::from_slice(&fs::read(path).map_err(storage)?).map_err(storage)?;
        if ack.payload.operation != operation
            || ack.payload.hash != digest::content(ack.payload.content.as_bytes())
        {
            return Err(invalid("issue receipt integrity mismatch"));
        }
        Ok(Some(ack))
    }
}

struct App {
    config: Config,
    gate: Gate,
    owner: String,
    agent: String,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn invalid(value: &str) -> Error {
    Error::Invalid(value.into())
}
fn storage(value: impl std::fmt::Display) -> Error {
    Error::Storage(value.to_string())
}
fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(storage)?;
    }
    Ok(())
}
fn capability(path: &Path) -> Result<String> {
    if !path.exists() {
        let value = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut file = private_options()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(storage)?;
        file.write_all(value.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(storage)?;
    }
    crate::mcp::read_token(path).map_err(storage)
}
fn public_origin(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value).map_err(storage)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("agent_public_url must be an HTTPS origin"));
    }
    Ok(url)
}
fn genesis(config: &Config) -> Evidence {
    let workspace: WorkspaceId = "8090-issue-actions".into();
    let readers: Readers = ["owner".into(), "agent".into()].into_iter().collect();
    let principals = [("owner", Role::Human), ("agent", Role::Agent)]
        .into_iter()
        .map(|(name, role)| {
            let p = Principal {
                id: name.into(),
                workspace: workspace.clone(),
                role,
                enabled: true,
            };
            (p.id.clone(), p)
        })
        .collect();
    let audience = Audience {
        id: "issue".into(),
        workspace: workspace.clone(),
        destination: config.destination(),
        revision: 1,
        recipients: readers,
        resolved: true,
    };
    Evidence {
        workspace: workspace.clone(),
        complete: true,
        principals,
        artifacts: BTreeMap::new(),
        sources: BTreeMap::new(),
        acls: BTreeMap::new(),
        transformations: BTreeMap::new(),
        audiences: [(audience.id.clone(), audience)].into_iter().collect(),
        policy: Policy {
            workspace,
            revision: 1,
            approvers: ["owner".into()].into_iter().collect(),
            max_approval_lifetime: 1800,
            configuration: Some(config.scope()),
        },
        approvals: BTreeMap::new(),
        revoked: BTreeSet::new(),
    }
}
fn open(config: Config) -> Result<Shared> {
    config.validate()?;
    private_directory(&config.directory)?;
    let owner = capability(&config.directory.join("owner-token"))?;
    let agent = capability(&config.directory.join("agent-token"))?;
    if owner == agent {
        return Err(invalid("owner and agent capabilities must differ"));
    }
    let gate = Coordinator::open_with_evidence(
        DiskJournal::open(&config.directory.join("journal"))?,
        IssuePublisher::new(config.clone())?,
        genesis(&config),
    )?;
    if gate.state().evidence.policy.configuration != Some(config.scope())
        || gate.state().evidence.workspace.0 != "8090-issue-actions"
    {
        return Err(invalid(
            "saved state belongs to another action configuration; use a separate state directory",
        ));
    }
    Ok(Arc::new(Mutex::new(App {
        config,
        gate,
        owner,
        agent,
    })))
}
fn intent(artifact: &ArtifactId) -> Intent {
    Intent {
        artifact: artifact.clone(),
        audience: "issue".into(),
        requester: "owner".into(),
    }
}
fn capture(app: &mut App, proposal: Proposal) -> Result<ArtifactId> {
    let envelope = Envelope::new(&app.config, proposal)?;
    let content = serde_json::to_string(&envelope).map_err(storage)?;
    let id = ArtifactId(format!("action-{}", digest::content(content.as_bytes()).0));
    // This source is the received action envelope, not a claim about the agent's prior inputs.
    app.gate.import_source(
        &"owner".into(),
        id,
        content,
        ["owner".into(), "agent".into()].into_iter().collect(),
    )
}
fn view(app: &App, key: &str) -> Result<Value> {
    let request = app.gate.review_request(&"owner".into(), key)?;
    let decision = verify(
        &app.gate.state().evidence,
        &intent(request.artifact()),
        now(),
    );
    let mut unsigned = app.gate.state().evidence.clone();
    unsigned.approvals.clear();
    let context = verify(&unsigned, &intent(request.artifact()), now())
        .context()
        .map(Context::digest);
    let action: Value =
        serde_json::from_str(&app.gate.state().evidence.artifacts[request.artifact()].content)
            .map_err(storage)?;
    let operation = request
        .operation
        .as_ref()
        .and_then(|id| app.gate.state().operations.get(id));
    Ok(
        json!({"id":key,"revision":request.revision(),"action":action,"context_digest":context,"decision":decision,
        "operation":operation,"approved":matches!(decision,Decision::Ready{..}),"mode":app.config.mode,
        "evidence_scope":"Exact submitted action only. Native-agent context and source lineage are not captured."}),
    )
}

struct WebError(Error);
impl From<Error> for WebError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}
impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            Error::Unauthorized => StatusCode::UNAUTHORIZED,
            Error::Conflict => StatusCode::CONFLICT,
            Error::NotFound => StatusCode::NOT_FOUND,
            _ => StatusCode::BAD_REQUEST,
        };
        (status, Json(json!({"error":self.0.to_string()}))).into_response()
    }
}
type WebResult = std::result::Result<Json<Value>, WebError>;
fn auth(headers: &HeaderMap, app: &App, owner: bool) -> Result<()> {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let local = host.split_once(':').is_some_and(|(name, port)| {
        matches!(name, "localhost" | "127.0.0.1") && port.parse::<u16>().is_ok()
    });
    let remote = !owner
        && app
            .config
            .agent_public_url
            .as_ref()
            .and_then(|url| public_origin(url).ok())
            .is_some_and(|url| url.authority() == host);
    if !local && !remote {
        return Err(Error::Unauthorized);
    }
    if let Some(origin) = headers.get(header::ORIGIN)
        && (!owner || origin.to_str().ok() != Some(format!("http://{host}").as_str()))
    {
        return Err(Error::Unauthorized);
    }
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(';')
                .map(str::trim)
                .find_map(|s| s.strip_prefix("trustgraph_actions="))
        });
    if (owner && (bearer == Some(app.owner.as_str()) || cookie == Some(app.owner.as_str())))
        || (!owner && bearer == Some(app.agent.as_str()))
    {
        Ok(())
    } else {
        Err(Error::Unauthorized)
    }
}
async fn locked<F>(shared: Shared, operation: F) -> WebResult
where
    F: FnOnce(&mut App) -> Result<Value> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let mut app = shared
            .lock()
            .map_err(|_| invalid("action workspace requires restart"))?;
        operation(&mut app).map(Json)
    })
    .await
    .map_err(|_| {
        WebError(invalid(
            "action operation interrupted; inspect status before retrying",
        ))
    })?
    .map_err(WebError)
}
async fn list(State(shared): State<Shared>, headers: HeaderMap) -> WebResult {
    locked(shared, move |app| {
        auth(&headers, app, false)?;
        list_value(app)
    })
    .await
}
fn list_value(app: &App) -> Result<Value> {
    let items = app
        .gate
        .visible_requests(&"agent".into())
        .iter()
        .map(|r| view(app, &r.id))
        .collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"mode":app.config.mode,"repository":app.config.github_repository,"tool":TOOL,"items":items}),
    )
}
async fn owner_list(State(shared): State<Shared>, headers: HeaderMap) -> WebResult {
    locked(shared, move |app| {
        auth(&headers, app, true)?;
        list_value(app)
    })
    .await
}
async fn one(
    State(shared): State<Shared>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
) -> WebResult {
    locked(shared, move |app| {
        auth(&headers, app, false)?;
        view(app, &id)
    })
    .await
}
async fn propose(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<Proposal>,
) -> WebResult {
    locked(shared, move |app| {
        auth(&headers, app, false)?;
        let artifact = capture(app, input.clone())?;
        app.gate.submit_request(
            &"agent".into(),
            Submission {
                key: input.key.clone(),
                title: input.title,
                summary: "8090 proposal for exact GitHub issue approval".into(),
                artifact,
                audience: "issue".into(),
            },
            now(),
        )?;
        view(app, &input.key)
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revision {
    revision: u64,
    title: String,
    body: String,
}
async fn revise(
    State(shared): State<Shared>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Json(input): Json<Revision>,
) -> WebResult {
    locked(shared, move |app| {
        auth(&headers, app, false)?;
        let request = app.gate.review_request(&"agent".into(), &id)?;
        let previous: Envelope =
            serde_json::from_str(&app.gate.state().evidence.artifacts[request.artifact()].content)
                .map_err(storage)?;
        let mut proposal = previous.proposal;
        proposal.title = input.title;
        proposal.body = input.body;
        let artifact = capture(app, proposal)?;
        app.gate
            .revise_request(&"agent".into(), &id, input.revision, &artifact, now())?;
        view(app, &id)
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    revision: u64,
    context_digest: digest::Digest,
}
fn current(app: &App, id: &str, review: &Review) -> Result<ArtifactId> {
    let request = app.gate.review_request(&"owner".into(), id)?;
    if request.revision() != review.revision
        || view(app, id)?["context_digest"] != json!(review.context_digest)
    {
        return Err(Error::Conflict);
    }
    Ok(request.artifact().clone())
}
async fn approve(
    State(shared): State<Shared>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Json(input): Json<Review>,
) -> WebResult {
    locked(shared, move |app| {
        auth(&headers, app, true)?;
        let artifact = current(app, &id, &input)?;
        let key = format!(
            "review-{}",
            digest::structured(
                "issue-review",
                &(&id, input.revision, &input.context_digest)
            )
            .0
        );
        app.gate.approve(
            &"owner".into(),
            &intent(&artifact),
            &input.context_digest,
            &key,
            now(),
        )?;
        view(app, &id)
    })
    .await
}
async fn execute(
    State(shared): State<Shared>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Json(input): Json<Review>,
) -> WebResult {
    locked(shared, move |app| {
        auth(&headers, app, true)?;
        let artifact = current(app, &id, &input)?;
        let released = app
            .gate
            .review_request(&"owner".into(), &id)?
            .operation
            .is_some();
        if !released
            && !matches!(
                verify(&app.gate.state().evidence, &intent(&artifact), now()),
                Decision::Ready { .. }
            )
        {
            return Err(Error::Unauthorized);
        }
        app.gate.release_request(
            &"owner".into(),
            &id,
            input.revision,
            &input.context_digest,
            now(),
        )?;
        view(app, &id)
    })
    .await
}
async fn session(
    State(shared): State<Shared>,
    headers: HeaderMap,
) -> std::result::Result<Response, WebError> {
    let app = shared
        .lock()
        .map_err(|_| WebError(invalid("workspace unavailable")))?;
    auth(&headers, &app, true)?;
    Ok((
        [
            (
                header::SET_COOKIE,
                format!(
                    "trustgraph_actions={}; HttpOnly; SameSite=Strict; Path=/",
                    app.owner
                ),
            ),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
        Json(json!({"authenticated":true})),
    )
        .into_response())
}
fn routers(shared: Shared) -> (Router, Router) {
    let owner = Router::new().route("/",get(|| async {
        ([(header::CONTENT_SECURITY_POLICY,"default-src 'self'; script-src 'self'; style-src 'self'; frame-ancestors 'none'; base-uri 'none'"),
          (header::CACHE_CONTROL,"no-store")],Html(include_str!("../../../web/actions.html")))
    })).route("/actions.js",get(|| async { ([(header::CONTENT_TYPE,"text/javascript")],include_str!("../../../web/actions.js")) }))
        .route("/actions.css",get(|| async { ([(header::CONTENT_TYPE,"text/css")],include_str!("../../../web/actions.css")) }))
        .route("/api/session",post(session)).route("/api/actions",get(owner_list))
        .route("/api/actions/{id}/approve",post(approve)).route("/api/actions/{id}/execute",post(execute))
        .layer(DefaultBodyLimit::max(65_536)).with_state(shared.clone());
    let agent = Router::new()
        .route("/api/actions", get(list).post(propose))
        .route("/api/actions/{id}", get(one))
        .route("/api/actions/{id}/revise", post(revise))
        .layer(DefaultBodyLimit::max(65_536))
        .with_state(shared);
    (owner, agent)
}
/// Check the configured consumer credential, pinned account and schema without a write.
pub fn check(path: &Path) -> Result<()> {
    let config: Config =
        serde_json::from_slice(&fs::read(path).map_err(storage)?).map_err(storage)?;
    config.validate()?;
    if config.mode != Mode::Live || config.composio_transport != Transport::ConnectMcp {
        return Err(invalid(
            "--check requires a live Connect MCP config; use the Python preflight for a project key",
        ));
    }
    let publisher = IssuePublisher::new(config)?;
    connect::check(&publisher).map_err(|()| {
        invalid("Connect account or input schema check failed; no issue was created")
    })?;
    println!(
        "Connect credential, active GitHub account and pinned input schema verified. No issue was created."
    );
    Ok(())
}

pub async fn serve(
    path: &Path,
    port: u16,
    agent_port: u16,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let config: Config = serde_json::from_slice(&fs::read(path)?)?;
    let directory = config.directory.clone();
    let mode = config.mode.clone();
    let shared = tokio::task::spawn_blocking(move || open(config)).await??;
    let (owner, agent) = routers(shared.clone());
    let owner_listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let agent_listener = tokio::net::TcpListener::bind(("127.0.0.1", agent_port)).await?;
    let operator_file = directory.join("operator.html");
    let owner_token = shared
        .lock()
        .map_err(|_| "workspace unavailable")?
        .owner
        .clone();
    private_options().write(true).create(true).truncate(true).open(&operator_file)?.write_all(format!(
        "<!doctype html><meta http-equiv=refresh content=\"0;url=http://127.0.0.1:{port}/#{}\">",owner_token).as_bytes())?;
    println!(
        "TrustGraph issue actions ({mode:?}): http://127.0.0.1:{port}\nOperator sign-in: {}\nAgent API: http://127.0.0.1:{agent_port}/api/actions\nAgent token file: {}",
        operator_file.display(),
        directory.join("agent-token").display()
    );
    tokio::try_join!(
        axum::serve(owner_listener, owner),
        axum::serve(agent_listener, agent)
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    const OWNER: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const AGENT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn configured(directory: &Path, endpoint: &str) -> Shared {
        configured_transport(directory, endpoint, None)
    }
    fn configured_transport(directory: &Path, endpoint: &str, schema: Option<&str>) -> Shared {
        let mut config = Config {
            directory: directory.into(),
            github_repository: "example/demo".into(),
            mode: Mode::Live,
            connected_account_id: Some("ca_test".into()),
            user_id: Some("demo-user".into()),
            tool_version: Some("20261101_00".into()),
            composio_transport: Transport::ProjectApi,
            tool_schema_hash: None,
            agent_public_url: None,
        };
        if let Some(hash) = schema {
            config.composio_transport = Transport::ConnectMcp;
            config.tool_schema_hash = Some(hash.into());
            config.tool_version = None;
            config.user_id = None;
        }
        config.validate().unwrap();
        let receipts = directory.join("receipts");
        private_directory(&receipts).unwrap();
        let publisher = IssuePublisher {
            config: config.clone(),
            api_key: Some("test-key".into()),
            endpoint: endpoint.into(),
            receipts,
        };
        let gate = Coordinator::open_with_evidence(
            DiskJournal::open(&directory.join("journal")).unwrap(),
            publisher,
            genesis(&config),
        )
        .unwrap();
        Arc::new(Mutex::new(App {
            config,
            gate,
            owner: OWNER.into(),
            agent: AGENT.into(),
        }))
    }
    fn proposal() -> Value {
        json!({"key":"wo-2-issue","title":"Fix zero-value authorization","body":"Fictional test body", "work_order":{"project_id":"project-1","id":"WO-2"}})
    }
    async fn request(
        router: &Router,
        token: &str,
        method: &str,
        path: &str,
        value: Value,
    ) -> (StatusCode, Value) {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::HOST, "127.0.0.1:8085")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(value.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
    fn binding(item: &Value) -> Value {
        json!({"revision":item["revision"],"context_digest":item["context_digest"]})
    }
    async fn provider(
        success: bool,
    ) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
        let calls = Arc::new(Mutex::new(vec![]));
        let record = calls.clone();
        let app = Router::new().route("/api/v3.1/tools/execute/GITHUB_CREATE_AN_ISSUE",post(move |headers: HeaderMap,Json(body):Json<Value>| {
            let record = record.clone(); async move {
                assert_eq!(headers["x-api-key"],"test-key"); record.lock().unwrap().push(body.clone());
                Json(if success { json!({"successful":true,"data":{"number":42,"html_url":"https://github.com/example/demo/issues/42","title":body["arguments"]["title"],"body":body["arguments"]["body"]}}) }
                else { json!({"successful":true,"data":{}}) })
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, calls, handle)
    }

    async fn connect_provider(
        schema: Value,
        active: bool,
        ambiguous: bool,
        sse: bool,
    ) -> (String, Arc<Mutex<Vec<Value>>>, tokio::task::JoinHandle<()>) {
        let calls = Arc::new(Mutex::new(vec![]));
        let record = calls.clone();
        let app = Router::new().route("/mcp", post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let record = record.clone(); let schema = schema.clone();
            async move {
                assert_eq!(headers["x-consumer-api-key"], "test-key");
                assert!(!headers.contains_key("x-api-key"));
                assert_eq!(headers["mcp-protocol-version"], "2025-03-26");
                if body["method"] != "initialize" {
                    assert_eq!(headers["mcp-session-id"], "transport-session");
                }
                if body["method"] == "notifications/initialized" {
                    return StatusCode::ACCEPTED.into_response();
                }
                let result = if body["method"] == "initialize" {
                    json!({"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"mock","version":"1"}})
                } else {
                    let args = &body["params"]["arguments"];
                    let data = match body["params"]["name"].as_str().unwrap() {
                        "COMPOSIO_SEARCH_TOOLS" => json!({"session":{"id":"workflow-session"},"toolkit_connection_statuses":[{
                            "toolkit":"github","has_active_connection":active,
                            "accounts":[{"id":"ca_test","status":if active {"ACTIVE"} else {"EXPIRED"}}]}]}),
                        "COMPOSIO_GET_TOOL_SCHEMAS" => {
                            assert_eq!(args["session_id"], "workflow-session");
                            assert_eq!(args["tool_slugs"], json!([TOOL]));
                            json!({"tool_schemas":{TOOL:{"input_schema":schema}}})
                        },
                        "COMPOSIO_MULTI_EXECUTE_TOOL" => {
                            record.lock().unwrap().push(args.clone());
                            assert_eq!(args["session_id"], "workflow-session");
                            assert_eq!(args["tools"].as_array().unwrap().len(),1);
                            assert_eq!(args["tools"][0]["account"],"ca_test");
                            assert_eq!(args["tools"][0]["tool_slug"],TOOL);
                            let data = if ambiguous { json!({}) } else {
                                json!({"number":42,"html_url":"https://github.com/example/demo/issues/42",
                                    "title":args["tools"][0]["arguments"]["title"],"body":args["tools"][0]["arguments"]["body"]})
                            };
                            json!({"results":[{"tool_slug":TOOL,"response":{"successful":true,"data":data}}]})
                        },
                        other => panic!("unexpected MCP tool {other}"),
                    };
                    json!({"content":[{"type":"text","text":json!({"successful":true,"data":data}).to_string()}]})
                };
                let reply = json!({"jsonrpc":"2.0","id":body["id"],"result":result});
                let mut response = if sse {
                    ([(header::CONTENT_TYPE,"text/event-stream")],format!(": keepalive\n\ndata: {reply}\n\n")).into_response()
                } else { Json(reply).into_response() };
                response.headers_mut().insert("Mcp-Session-Id", "transport-session".parse().unwrap());
                response
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, calls, handle)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn connect_mcp_pins_schema_account_and_exact_arguments_and_never_resends() {
        let schema = json!({"type":"object","required":["owner","repo","title"],
            "properties":{"owner":{"type":"string"},"repo":{"type":"string"},"title":{"type":"string"},"body":{"type":"string"}}});
        let hash = digest::content(&serde_json::to_vec(&schema).unwrap()).0;
        // Success over JSON and SSE; preflight failures cannot invoke the writer;
        // an ambiguous writer response stays unknown across retries and restart.
        for (active, matches, ambiguous, sse, state, writes) in [
            (true, true, false, false, "PUBLISHED", 1),
            (true, true, false, true, "PUBLISHED", 1),
            (true, false, false, false, "FAILED_CONFIRMED", 0),
            (false, true, false, false, "FAILED_CONFIRMED", 0),
            (true, true, true, true, "OUTCOME_UNKNOWN", 1),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let (url, calls, server) =
                connect_provider(schema.clone(), active, ambiguous, sse).await;
            let pin = if matches {
                hash.clone()
            } else {
                "0".repeat(64)
            };
            let shared = configured_transport(directory.path(), &url, Some(&pin));
            let (owner, agent) = routers(shared.clone());
            let first = request(&agent, AGENT, "POST", "/api/actions", proposal())
                .await
                .1;
            assert_eq!(first["action"]["transport"], "connect_mcp");
            assert_eq!(first["action"]["input_schema_sha256"], pin);
            assert!(calls.lock().unwrap().is_empty());
            assert_eq!(
                request(
                    &owner,
                    OWNER,
                    "POST",
                    "/api/actions/wo-2-issue/execute",
                    binding(&first)
                )
                .await
                .0,
                StatusCode::UNAUTHORIZED
            );
            request(
                &owner,
                OWNER,
                "POST",
                "/api/actions/wo-2-issue/approve",
                binding(&first),
            )
            .await;
            for _ in 0..2 {
                let result = request(
                    &owner,
                    OWNER,
                    "POST",
                    "/api/actions/wo-2-issue/execute",
                    binding(&first),
                )
                .await;
                assert_eq!(result.1["operation"]["state"], state, "{result:?}");
            }
            assert_eq!(calls.lock().unwrap().len(), writes);
            if writes == 1 {
                assert_eq!(
                    calls.lock().unwrap()[0]["tools"][0]["arguments"],
                    json!({"owner":"example","repo":"demo","title":proposal()["title"],"body":proposal()["body"]})
                );
            }
            drop(owner);
            drop(agent);
            drop(shared);
            let (owner, _) = routers(configured_transport(directory.path(), &url, Some(&pin)));
            let result = request(
                &owner,
                OWNER,
                "POST",
                "/api/actions/wo-2-issue/execute",
                binding(&first),
            )
            .await;
            assert_eq!(result.1["operation"]["state"], state);
            assert_eq!(calls.lock().unwrap().len(), writes);
            server.abort();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_proposal_requires_exact_owner_approval_and_executes_once_across_restart() {
        let directory = tempfile::tempdir().unwrap();
        let (url, calls, server) = provider(true).await;
        let shared = configured(directory.path(), &url);
        let (owner, agent) = routers(shared.clone());
        let (status, first) = request(&agent, AGENT, "POST", "/api/actions", proposal()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["approved"], false);
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(
            request(&agent, AGENT, "POST", "/api/actions", proposal())
                .await
                .1["revision"],
            1
        );
        let mut changed = proposal();
        changed["body"] = json!("Different body with the same key");
        assert_eq!(
            request(&agent, AGENT, "POST", "/api/actions", changed)
                .await
                .0,
            StatusCode::CONFLICT
        );
        let mut routed = proposal();
        routed["repository"] = json!("other/target");
        assert_eq!(
            request(&agent, AGENT, "POST", "/api/actions", routed)
                .await
                .0,
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(
            request(&agent, OWNER, "GET", "/api/actions", Value::Null)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &owner,
                AGENT,
                "POST",
                "/api/actions/wo-2-issue/approve",
                binding(&first)
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &agent,
                AGENT,
                "POST",
                "/api/actions/wo-2-issue/execute",
                binding(&first)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            request(
                &owner,
                OWNER,
                "POST",
                "/api/actions/wo-2-issue/execute",
                binding(&first)
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            request(
                &owner,
                OWNER,
                "POST",
                "/api/actions/wo-2-issue/approve",
                binding(&first)
            )
            .await
            .0,
            StatusCode::OK
        );
        let (status, second) = request(
            &agent,
            AGENT,
            "POST",
            "/api/actions/wo-2-issue/revise",
            json!({"revision":1,"title":"Revised title","body":"Revised exact body"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(second["revision"], 2);
        assert_eq!(second["approved"], false);
        for action in ["approve", "execute"] {
            assert_eq!(
                request(
                    &owner,
                    OWNER,
                    "POST",
                    &format!("/api/actions/wo-2-issue/{action}"),
                    binding(&first)
                )
                .await
                .0,
                StatusCode::CONFLICT
            );
        }
        assert!(calls.lock().unwrap().is_empty());
        request(
            &owner,
            OWNER,
            "POST",
            "/api/actions/wo-2-issue/approve",
            binding(&second),
        )
        .await;
        let (status, result) = request(
            &owner,
            OWNER,
            "POST",
            "/api/actions/wo-2-issue/execute",
            binding(&second),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["operation"]["state"], "PUBLISHED");
        assert_eq!(result["operation"]["receipt"]["simulated"], false);
        assert_eq!(
            result["operation"]["receipt"]["provider_id"],
            "https://github.com/example/demo/issues/42"
        );
        request(
            &owner,
            OWNER,
            "POST",
            "/api/actions/wo-2-issue/execute",
            binding(&second),
        )
        .await;
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                json!({"connected_account_id":"ca_test","user_id":"demo-user","version":"20261101_00",
            "arguments":{"owner":"example","repo":"demo","title":"Revised title","body":"Revised exact body"}})
            ]
        );
        drop(owner);
        drop(agent);
        drop(shared);
        let reopened = configured(directory.path(), &url);
        let (owner, _) = routers(reopened);
        assert_eq!(
            request(
                &owner,
                OWNER,
                "POST",
                "/api/actions/wo-2-issue/execute",
                binding(&second)
            )
            .await
            .1["operation"]["state"],
            "PUBLISHED"
        );
        assert_eq!(calls.lock().unwrap().len(), 1);
        server.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ambiguous_provider_result_is_never_resent_even_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let (url, calls, server) = provider(false).await;
        let shared = configured(directory.path(), &url);
        let (owner, agent) = routers(shared.clone());
        let first = request(&agent, AGENT, "POST", "/api/actions", proposal())
            .await
            .1;
        request(
            &owner,
            OWNER,
            "POST",
            "/api/actions/wo-2-issue/approve",
            binding(&first),
        )
        .await;
        for _ in 0..2 {
            let result = request(
                &owner,
                OWNER,
                "POST",
                "/api/actions/wo-2-issue/execute",
                binding(&first),
            )
            .await;
            assert_eq!(result.1["operation"]["state"], "OUTCOME_UNKNOWN");
        }
        assert_eq!(calls.lock().unwrap().len(), 1);
        drop(owner);
        drop(agent);
        drop(shared);
        let (owner, _) = routers(configured(directory.path(), &url));
        assert_eq!(
            request(
                &owner,
                OWNER,
                "POST",
                "/api/actions/wo-2-issue/execute",
                binding(&first)
            )
            .await
            .1["operation"]["state"],
            "OUTCOME_UNKNOWN"
        );
        assert_eq!(calls.lock().unwrap().len(), 1);
        server.abort();
    }

    #[tokio::test]
    async fn host_origin_and_expired_approval_cannot_authorize_an_action() {
        let directory = tempfile::tempdir().unwrap();
        let shared = configured(directory.path(), "http://127.0.0.1:1");
        let (owner, agent) = routers(shared.clone());
        let item = request(&agent, AGENT, "POST", "/api/actions", proposal())
            .await
            .1;
        for (router, token, host, origin) in [
            (&agent, AGENT, "attacker.example", None),
            (
                &agent,
                AGENT,
                "127.0.0.1:8085",
                Some("https://attacker.example"),
            ),
            (
                &owner,
                OWNER,
                "127.0.0.1:8084",
                Some("https://attacker.example"),
            ),
        ] {
            let mut builder = Request::builder()
                .uri("/api/actions")
                .header(header::HOST, host)
                .header(header::AUTHORIZATION, format!("Bearer {token}"));
            if let Some(origin) = origin {
                builder = builder.header(header::ORIGIN, origin);
            }
            let reply = router
                .clone()
                .oneshot(builder.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(reply.status(), StatusCode::UNAUTHORIZED);
        }
        {
            let mut app = shared.lock().unwrap();
            let artifact = app
                .gate
                .review_request(&"owner".into(), "wo-2-issue")
                .unwrap()
                .artifact()
                .clone();
            app.gate
                .approve(
                    &"owner".into(),
                    &intent(&artifact),
                    &serde_json::from_value(item["context_digest"].clone()).unwrap(),
                    "expired",
                    now() - 1801,
                )
                .unwrap();
        }
        assert_eq!(
            request(
                &owner,
                OWNER,
                "POST",
                "/api/actions/wo-2-issue/execute",
                binding(&item)
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn changing_repository_mode_or_version_changes_the_approval_scope() {
        let directory = tempfile::tempdir().unwrap();
        let config = Config {
            directory: directory.path().into(),
            github_repository: "example/demo".into(),
            mode: Mode::Preview,
            connected_account_id: None,
            user_id: None,
            tool_version: None,
            composio_transport: Transport::ProjectApi,
            tool_schema_hash: None,
            agent_public_url: None,
        };
        let original = config.scope();
        for changed in [
            Config {
                github_repository: "other/repo".into(),
                ..config.clone()
            },
            Config {
                mode: Mode::Live,
                ..config.clone()
            },
            Config {
                tool_version: Some("20261101_00".into()),
                ..config.clone()
            },
            Config {
                connected_account_id: Some("ca_other".into()),
                ..config.clone()
            },
        ] {
            assert_ne!(changed.scope(), original);
        }
        let opened = open(config.clone()).unwrap();
        drop(opened);
        assert!(
            open(Config {
                github_repository: "other/repo".into(),
                ..config
            })
            .is_err()
        );
    }
}
