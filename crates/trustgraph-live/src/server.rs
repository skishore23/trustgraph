use crate::*;
use axum::{
    Json, Router,
    extract::{Path as RoutePath, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};

#[derive(Debug)]
struct WebError(StatusCode, String);
impl<E: std::fmt::Display> From<E> for WebError {
    fn from(e: E) -> Self {
        Self(StatusCode::BAD_REQUEST, e.to_string())
    }
}
impl WebError {
    fn bad(message: String) -> Self {
        Self(StatusCode::BAD_REQUEST, message)
    }
    fn unauthorized(message: &str) -> Self {
        Self(StatusCode::UNAUTHORIZED, message.into())
    }
}
impl IntoResponse for WebError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
type WebResult = std::result::Result<Json<Value>, WebError>;
fn lock(s: &Shared) -> Result<std::sync::MutexGuard<'_, Workspace>> {
    s.lock().map_err(|_| err("workspace requires restart"))
}
/// Loopback Host for this server's port plus a same-origin browser Origin.
fn local_request<'a>(h: &'a HeaderMap, w: &Workspace) -> std::result::Result<&'a str, WebError> {
    let host = auth::loopback_host(h, w.port)
        .ok_or_else(|| WebError(StatusCode::FORBIDDEN, "loopback host required".into()))?;
    if !auth::same_origin(h, host) {
        return Err(WebError(
            StatusCode::FORBIDDEN,
            "same-origin request required".into(),
        ));
    }
    Ok(host)
}
/// Owner routes accept only a current operator session cookie. Agent routes
/// accept the agent bearer token or an operator session. The owner launch
/// secret is never accepted as a bearer credential for any route.
fn authorized(h: &HeaderMap, w: &Workspace, owner: bool) -> std::result::Result<(), WebError> {
    local_request(h, w)?;
    if w.owner.valid(h) {
        return Ok(());
    }
    if !owner
        && auth::bearer(h)
            .is_some_and(|t| auth::constant_time_eq(t.as_bytes(), w.agent_token.as_bytes()))
    {
        return Ok(());
    }
    Err(WebError::unauthorized(if owner {
        "operator session required"
    } else {
        "workspace authentication required"
    }))
}
/// After a restart no worker owns an in-progress job. Leaving it active would
/// block new work forever, so it becomes `interrupted` (never resumed).
fn recover_jobs(saved: &mut Saved) {
    for job in &mut saved.jobs {
        if ["queued", "running", "revising"].contains(&job.status.as_str()) {
            job.error = Some(if job.status == "revising" {
                "Application restarted during a revision. Submit the work again; this item cannot be approved or published.".into()
            } else {
                "Application restarted. This run was not retried.".into()
            });
            job.status = "interrupted".into();
        }
    }
}

/// Startup options for the live workspace server.
#[derive(Debug, Clone)]
pub struct Options {
    /// Loopback port the server is bound to; requests must name it in Host.
    pub port: u16,
    /// Permit, with a warning, a state directory inside the target or context repository.
    pub allow_state_in_repository: bool,
}
/// A ready router plus the private, single-use operator launch file.
pub struct Started {
    pub router: Router,
    pub launch_file: PathBuf,
}

pub async fn router(config: Config, options: Options) -> Result<Started> {
    if !config.github_repository.contains('/')
        || config
            .github_repository
            .bytes()
            .any(|b| !(b.is_ascii_alphanumeric() || b"-_./".contains(&b)))
        || config.context_files.is_empty()
        || config.context_files.len() > 8
    {
        return Err(err("invalid repository setup"));
    }
    fs::create_dir_all(&config.directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config.directory, fs::Permissions::from_mode(0o700))?;
    }
    auth::state_outside_repositories(
        &config.directory,
        &[&config.target_repository, &config.context_repository],
        options.allow_state_in_repository,
    )?;
    // Earlier versions kept a reusable owner token on disk. Remove it: operator
    // authority is now a per-start, single-use launch secret held in memory.
    let legacy = config.directory.join("owner-token");
    if fs::symlink_metadata(&legacy).is_ok() {
        fs::remove_file(&legacy)?;
    }
    let agent_token = token(&config.directory.join("agent-token"))?;
    let saved_path = config.directory.join("work.json");
    let mut saved: Saved = if saved_path.exists() {
        serde_json::from_slice(&fs::read(saved_path)?)?
    } else {
        Saved {
            policy: if config.departments {
                department_policy()
            } else {
                WorkPolicy::default()
            },
            ..Saved::default()
        }
    };
    saved.policy.validate_live()?;
    if config.departments != saved.policy.departments.is_some() {
        return Err(err(
            "use a separate state directory for the department workspace",
        ));
    }
    recover_jobs(&mut saved);
    let gate = Coordinator::open_with_evidence(
        DiskJournal::open(&config.directory.join("journal"))?,
        github::GitHub::new(
            &config.github_repository,
            config.directory.join("publisher"),
        )?,
        genesis_for(&config.github_repository, config.departments),
    )?;
    if gate.state().evidence.workspace.0
        != if config.departments {
            "bank-departments"
        } else {
            "repository-work"
        }
        || gate.state().evidence.audiences[&AudienceId::from("repository")].destination
            != format!("github://{}/pulls", config.github_repository)
        || gate.state().evidence.policy.configuration
            != Some(digest::structured("work-policy", &saved.policy))
    {
        return Err(err(
            "saved workspace and configuration differ; use the matching configuration",
        ));
    }
    let hydra = hydra::Hydra::local()?;
    hydra.check().await?;
    let mut w = Workspace {
        config,
        gate,
        saved,
        hydra,
        owner: auth::OwnerSessions::new(&options.port.to_string(), auth::SESSION_LIFETIME),
        agent_token,
        port: options.port,
        simulations: Default::default(),
    };
    let base = git_head(&w.config.target_repository)?;
    let context_base = git_head(&w.config.context_repository)?;
    let mut material = Vec::new();
    for p in &w.config.context_files {
        material.push((
            format!("TrustGraph/{p}@{context_base}"),
            git_file(&w.config.context_repository, &context_base, p)?,
        ));
    }
    for p in ["src/lib.rs", "README.md", "Cargo.toml"] {
        material.push((
            format!("{}/{p}@{base}", w.config.github_repository),
            git_file(&w.config.target_repository, &base, p)?,
        ));
    }
    let mut sources = Vec::new();
    for (path, content) in material {
        let a = w.import(&path, content)?;
        let graph_id = w.hydra.put("source", &a, &[]).await?;
        let knowledge_status = w
            .hydra
            .knowledge_status(&a.id.0)
            .await
            .unwrap_or_else(|_| "status unavailable".into());
        let mut record = SourceRecord {
            id: a.id,
            restricted: w.config.departments && path.contains("/risk-investigation.md@"),
            path,
            hash: a.hash,
            graph_id,
            knowledge_status,
            permissions_graph: None,
        };
        if w.config.departments {
            apply_source_policy(&mut w.gate, &record, &w.saved.policy)?;
            record.permissions_graph = Some(
                w.hydra
                    .put(
                        "source-permission",
                        &source_permission(&w.gate, &record.id)?,
                        std::slice::from_ref(&record.graph_id),
                    )
                    .await?,
            );
        }
        sources.push(record);
    }
    w.hydra.put("policy", &w.saved.policy, &[]).await?;
    w.saved.sources = sources;
    w.save()?;
    let launch_file = w.config.directory.join("operator.html");
    let secret = w
        .owner
        .launch_secret()
        .ok_or_else(|| err("operator launch secret unavailable"))?;
    w.owner.write_launch_file(
        &launch_file,
        &format!(
            "<!doctype html><meta name=referrer content=no-referrer><meta http-equiv=refresh content=\"0;url=http://127.0.0.1:{}/#/connect/{secret}\">",
            w.port
        ),
    )?;
    Ok(Started {
        router: routes(Arc::new(Mutex::new(w))),
        launch_file,
    })
}
fn routes(shared: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/agent", get(index))
        .route("/policy", get(index))
        .route("/simulator", get(index))
        .route("/agent/{id}", get(index))
        .route("/api/session", post(session))
        .route(
            "/workspace.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript")],
                    include_str!("../../../web/workspace.js"),
                )
            }),
        )
        .route(
            "/workspace.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css")],
                    include_str!("../../../web/workspace.css"),
                )
            }),
        )
        .route("/api/work", get(view).post(create))
        .route("/api/work/{id}/approve", post(approve))
        .route("/api/work/{id}/publish", post(publish))
        .route("/api/work/{id}/repair", post(repair))
        .route("/api/work/{id}/revise", post(revise))
        .route("/api/policy", post(policy))
        .route("/api/simulate", post(simulate))
        .route("/api/graph", get(graph))
        .route("/api/work/{id}/gate", post(publication_gate))
        .route("/api/connections", get(connections))
        .with_state(shared)
}
async fn index(State(s): State<Shared>, h: HeaderMap) -> Response {
    let Ok(w) = lock(&s) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if local_request(&h, &w).is_err() {
        return StatusCode::FORBIDDEN.into_response();
    }
    ([(header::CACHE_CONTROL,"no-store"),(header::CONTENT_SECURITY_POLICY,"default-src 'self'; script-src 'self'; style-src 'self'; frame-ancestors 'none'; base-uri 'none'"),(header::REFERRER_POLICY,"no-referrer")],Html(include_str!("../../../web/workspace.html"))).into_response()
}
/// Exchange the single-use launch secret (sent once as a bearer by the launch
/// page) for a short-lived HttpOnly, SameSite=Strict session cookie.
async fn session(State(s): State<Shared>, h: HeaderMap) -> Response {
    let w = match lock(&s) {
        Ok(w) => w,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    if let Err(e) = local_request(&h, &w) {
        return e.into_response();
    }
    if w.owner.valid(&h) {
        return (
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({"authenticated":true})),
        )
            .into_response();
    }
    let Some(cookie) = auth::bearer(&h).and_then(|secret| w.owner.exchange(secret)) else {
        return WebError::unauthorized(
            "operator launch link is invalid or already used; restart TrustGraph for a new one",
        )
        .into_response();
    };
    (
        [
            (header::SET_COOKIE, cookie),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
        Json(json!({"authenticated":true})),
    )
        .into_response()
}
fn job_view(w: &Workspace, j: &Job) -> Value {
    let decision = j
        .artifact
        .as_ref()
        .map(|id| verify(&w.gate.state().evidence, &w.intent(id, "owner"), now()));
    let mut violations = j
        .change
        .as_ref()
        .map(|c| w.saved.policy.violations(&c.files, Some(&c.checks)))
        .unwrap_or_default();
    if j.change
        .as_ref()
        .is_some_and(|c| c.policy != w.saved.policy)
    {
        violations.push("Policy changed; run this work again under the active policy".into());
    }
    let information_permitted = j.artifact.as_ref().is_some_and(|id| {
        let mut e = w.gate.state().evidence.clone();
        e.approvals.clear();
        matches!(
            verify(&e, &w.intent(id, "owner"), now()),
            Decision::PendingApproval { .. }
        )
    });
    let context = decision
        .as_ref()
        .and_then(Decision::context)
        .map(Context::digest)
        .or_else(|| {
            j.artifact.as_ref().and_then(|id| {
                let mut evidence = w.gate.state().evidence.clone();
                evidence.approvals.clear();
                verify(&evidence, &w.intent(id, "owner"), now())
                    .context()
                    .map(Context::digest)
            })
        });
    let operation = j
        .operation
        .as_ref()
        .and_then(|id| w.gate.state().operations.get(id));
    json!({"id":j.id,"goal":j.goal,"status":j.status,"created_at":j.created_at,"artifact":j.artifact,"change":j.change,"base_files":j.base_files,"error":j.error,"provider":j.provider,"graph_id":j.graph_id,"decision":decision,"context_digest":context,"violations":violations,"information_permitted":information_permitted,"operation":operation,"events":j.events,"plan":j.plan,"graph_approval_recorded":j.authorization_graph.is_some()&&j.approval_graph.is_some(),"include_risk":j.include_risk,"repair_of":j.repair_of})
}
async fn view(State(s): State<Shared>, h: HeaderMap) -> WebResult {
    let w = lock(&s)?;
    authorized(&h, &w, false)?;
    Ok(Json(
        json!({"departments":w.config.departments,"repository":w.config.github_repository,"policy":w.saved.policy,"policy_revision":w.gate.state().evidence.policy.revision,"sources":w.saved.sources,"jobs":w.saved.jobs.iter().rev().map(|j|job_view(&w,j)).collect::<Vec<_>>(),"invariants":["Source permissions follow derived work","Approval binds the exact change and repository","Uncaptured input prevents publication"]}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewWork {
    goal: String,
    #[serde(default)]
    include_risk: bool,
    #[serde(default)]
    repair_of: Option<String>,
}
async fn create(State(s): State<Shared>, h: HeaderMap, Json(body): Json<NewWork>) -> WebResult {
    if body.goal.trim().is_empty() || body.goal.len() > 4000 {
        return Err(WebError::bad("task needs 1..4000 bytes".into()));
    }
    let id = format!("job-{}", Uuid::new_v4().simple());
    {
        let mut w = lock(&s)?;
        authorized(&h, &w, false)?;
        if w.config.departments {
            let owner = authorized(&h, &w, true).is_ok();
            let predefined = include_str!("../../../examples/bankcase/work-order.txt").trim();
            if !owner
                && body.goal != predefined
                && !w.saved.admitted_work_orders.contains(&body.goal)
            {
                return Err(WebError::bad("Agent work orders must match an operator-admitted task. Use examples/bankcase/work-order.txt, or submit the new task as the operator in Workspace first. An agent cannot declare new input text public.".into()));
            }
            if owner {
                w.saved.admitted_work_orders.insert(body.goal.clone());
            }
        }
        if w.saved
            .jobs
            .iter()
            .any(|j| ["queued", "running", "revising"].contains(&j.status.as_str()))
        {
            return Err(WebError::bad("one agent run is already active".into()));
        }
        w.saved.jobs.push(Job {
            id: id.clone(),
            goal: body.goal,
            status: "queued".into(),
            created_at: now(),
            artifact: None,
            change: None,
            error: None,
            provider: Value::Null,
            graph_id: None,
            graph_edges: BTreeMap::new(),
            base_files: vec![],
            input_ids: vec![],
            operation: None,
            events: vec![
                json!({"at":now(),"stage":"accepted","message":"Work order accepted through HTTP"}),
            ],
            plan: Value::Null,
            approval_graph: None,
            authorization_graph: None,
            include_risk: body.include_risk,
            repair_of: body.repair_of,
        });
        w.save()?;
    }
    let run_id = id.clone();
    tokio::spawn(supervise(s, run_id));
    Ok(Json(json!({"id":id})))
}
/// Run one job and record any failure, including a panic in the worker task.
/// A poisoned workspace lock is never written through: the server then reports
/// that it requires a restart, and restart marks the job interrupted.
async fn supervise(s: Shared, id: String) {
    let worker_state = s.clone();
    let worker_id = id.clone();
    let outcome = tokio::spawn(async move { work(&worker_state, &worker_id).await }).await;
    let error = match outcome {
        Ok(Ok(())) => return,
        Ok(Err(e)) => e.to_string(),
        Err(_) => "The run stopped unexpectedly. It was not retried; submit it again.".into(),
    };
    fail_job(&s, &id, error);
}
fn fail_job(s: &Shared, id: &str, error: String) {
    let Ok(mut w) = lock(s) else {
        return;
    };
    if let Some(j) = w
        .saved
        .jobs
        .iter_mut()
        .find(|j| j.id == id && ["queued", "running"].contains(&j.status.as_str()))
    {
        j.status = "blocked".into();
        j.error = Some(error);
    }
    let _ = w.save();
}
async fn work(s: &Shared, id: &str) -> Result<()> {
    let (hydra, sources, config, policy, goal, base) = {
        let mut w = lock(s)?;
        let j = w
            .saved
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| err("run missing"))?;
        j.status = "running".into();
        let goal = j.goal.clone();
        let include_risk = j.include_risk;
        w.save()?;
        (
            w.hydra.clone(),
            w.saved
                .sources
                .iter()
                .filter(|source| include_risk || !source.restricted)
                .cloned()
                .collect::<Vec<_>>(),
            w.config.clone(),
            w.saved.policy.clone(),
            goal,
            git_head(&w.config.target_repository)?,
        )
    };
    activity(
        s,
        id,
        "hydra_inputs",
        "Loading exact source bytes from HydraDB",
    )?;
    let policy_graph = hydra.put("policy", &policy, &[]).await?;
    let manifest: Vec<_> = sources.iter().map(|source| json!({"id":source.id,"path":source.path,"hash":source.hash,"graph_id":source.graph_id})).collect();
    let plan = json!({"goal":goal,"input_manifest":manifest,"policy_fingerprint":digest::structured("work-policy",&policy),"repository":config.github_repository,"base_commit":base,"steps":["load registered inputs","generate changed files","run isolated Rust checks","require exact human approval","check publication gate"]});
    let source_parents: Vec<_> = sources
        .iter()
        .flat_map(|source| {
            std::iter::once(source.graph_id.clone()).chain(source.permissions_graph.clone())
        })
        .collect();
    let plan_graph = hydra.put("plan", &plan, &source_parents).await?;
    {
        let mut w = lock(s)?;
        if let Some(j) = w.saved.jobs.iter_mut().find(|j| j.id == id) {
            j.plan = plan.clone();
        }
        w.save()?;
    }
    activity(
        s,
        id,
        "manifest",
        "Plan and input manifest recorded in HydraDB",
    )?;
    let mut inputs = Vec::new();
    let mut material = Vec::new();
    let mut base_files = Vec::new();
    for source in &sources {
        if let Some(permission) = &source.permissions_graph {
            let actual: Value = serde_json::from_str(&hydra.get(permission).await?)?;
            let expected = { source_permission(&lock(s)?.gate, &source.id)? };
            if actual != expected {
                return Err(err(
                    "HydraDB source permission no longer matches the current authority",
                ));
            }
        }
        let a: Artifact = serde_json::from_str(&hydra.get(&source.graph_id).await?)?;
        if a.id != source.id
            || a.hash != source.hash
            || a.hash != digest::content(a.content.as_bytes())
        {
            return Err(err(
                "HydraDB source does not match its imported fingerprint",
            ));
        }
        if source
            .path
            .starts_with(&format!("{}/", config.github_repository))
        {
            if !source.path.ends_with(&format!("@{base}")) {
                return Err(err(
                    "target base changed since import; restart to refresh source snapshots",
                ));
            }
            let path = source
                .path
                .strip_prefix(&format!("{}/", config.github_repository))
                .unwrap_or("")
                .split('@')
                .next()
                .unwrap_or("");
            base_files.push(ChangedFile {
                path: path.into(),
                content: a.content.clone(),
            });
        }
        inputs.push(a.id.clone());
        material.push(json!({"path":source.path,"sha256":a.hash,"content":a.content}));
    }
    let prompt = format!(
        "You are the coding agent for a controlled TrustGraph work order. Produce only the requested JSON object. All repository source inputs and instructions are supplied here. Do not use tools, other files, personal memory, web search or external context. Do not publish or claim tests executed. Return full contents of changed files only, never unchanged files. Allowed paths: {}. Target repository: {}. Base: {}. Work order: {}\n\nBusiness policy:\n{}\n\nCaptured repository inputs:\n{}",
        policy.allowed_paths.join(", "),
        config.github_repository,
        base,
        goal,
        serde_json::to_string(&policy)?,
        serde_json::to_string(&material)?
    );
    let prompt = format!(
        "{prompt}\n\nRecorded execution plan:\n{}",
        serde_json::to_string(&plan)?
    );
    let (goal_artifact, prompt_artifact) = {
        let mut w = lock(s)?;
        let goal_artifact = w.import(&format!("work-order/{id}"), goal.clone())?;
        inputs.push(goal_artifact.id.clone());
        let prompt_id = ArtifactId(format!("prompt-{id}"));
        w.gate.record_broker_output(
            &"agent".into(),
            prompt_id.clone(),
            inputs.clone(),
            prompt.clone(),
            "captured-prompt-assembly",
        )?;
        (
            goal_artifact,
            w.gate.state().evidence.artifacts[&prompt_id].clone(),
        )
    };
    let goal_graph = hydra.put("source", &goal_artifact, &[]).await?;
    let mut prompt_parents = source_parents.clone();
    prompt_parents.extend([goal_graph.clone(), plan_graph.clone(), policy_graph.clone()]);
    let prompt_graph = hydra
        .put("captured-prompt", &prompt_artifact, &prompt_parents)
        .await?;
    inputs.push(prompt_artifact.id.clone());
    let dir = config.directory.join("runs").join(id);
    fs::create_dir_all(&dir)?;
    activity(
        s,
        id,
        "agent",
        "Codex started with the captured inputs; external tools disabled",
    )?;
    let clone_dir = dir.clone();
    let (output, provider) =
        tokio::task::spawn_blocking(move || runner::generate(&prompt, &clone_dir)).await??;
    if output.summary.trim().is_empty() || output.summary.len() > 2000 {
        return Err(err("agent summary is missing or oversized"));
    }
    let failures = policy.violations(&output.files, None);
    if !failures.is_empty() {
        return Err(err(&failures.join("; ")));
    }
    let raw = {
        let mut w = lock(s)?;
        let aid = ArtifactId(format!("agent-{id}"));
        w.gate.record_broker_output(
            &"agent".into(),
            aid.clone(),
            inputs.clone(),
            serde_json::to_string(&output)?,
            "codex-cli-captured-v1",
        )?;
        w.gate.state().evidence.artifacts[&aid].clone()
    };
    let mut parents = source_parents.clone();
    parents.push(prompt_graph.clone());
    parents.push(plan_graph.clone());
    parents.push(policy_graph.clone());
    let manifest = {
        let w = lock(s)?;
        match &raw.origin {
            Origin::Transformation(t) => w.gate.state().evidence.transformations[t].clone(),
            _ => return Err(err("generated manifest missing")),
        }
    };
    let raw_graph = hydra
        .put(
            "generation",
            &json!({"artifact":raw,"manifest":manifest,"provider":provider}),
            &parents,
        )
        .await?;
    let files = output.files.clone();
    let base_for_check = base_files.clone();
    let patch = patch_digest(&config.github_repository, &base, &files);
    let check_dir = dir.clone();
    let check_hash = patch.clone();
    activity(
        s,
        id,
        "checks",
        "Running Rust checks against the exact proposed patch in isolation",
    )?;
    let checks = tokio::task::spawn_blocking(move || {
        runner::test(&files, &base_for_check, &check_dir, &check_hash)
    })
    .await??;
    let change = ChangeSet {
        repository: config.github_repository,
        base_commit: base,
        summary: output.summary,
        files: output.files,
        checks,
        policy: policy.clone(),
    };
    let (checked, artifact) = {
        let mut w = lock(s)?;
        let checked = ArtifactId(format!("checks-{id}"));
        w.gate.record_broker_output(
            &"agent".into(),
            checked.clone(),
            vec![raw.id.clone()],
            serde_json::to_string(&change.checks)?,
            "isolated-rust-test-v1",
        )?;
        let artifact = ArtifactId(format!("change-{id}"));
        w.gate.record_broker_output(
            &"agent".into(),
            artifact.clone(),
            vec![raw.id, checked.clone()],
            serde_json::to_string(&change)?,
            "checked-change-assembly-v1",
        )?;
        (
            w.gate.state().evidence.artifacts[&checked].clone(),
            w.gate.state().evidence.artifacts[&artifact].clone(),
        )
    };
    let checked_graph = hydra
        .put("checks", &checked, std::slice::from_ref(&raw_graph))
        .await?;
    let final_parents = vec![raw_graph.clone(), checked_graph.clone()];
    let graph_id = hydra.put("change", &artifact, &final_parents).await?;
    let mut graph_edges: BTreeMap<String, Vec<String>> = sources
        .iter()
        .map(|s| (s.graph_id.clone(), vec![]))
        .collect();
    for source in &sources {
        if let Some(permission) = &source.permissions_graph {
            graph_edges.insert(permission.clone(), vec![source.graph_id.clone()]);
        }
    }
    graph_edges.insert(goal_graph, vec![]);
    graph_edges.insert(prompt_graph.clone(), prompt_parents);
    graph_edges.insert(policy_graph.clone(), vec![]);
    graph_edges.insert(plan_graph, source_parents);
    graph_edges.insert(raw_graph.clone(), parents);
    graph_edges.insert(checked_graph.clone(), vec![raw_graph]);
    graph_edges.insert(graph_id.clone(), final_parents);
    for (id, parents) in &graph_edges {
        hydra.validate_edges(id, parents).await?;
    }
    activity(
        s,
        id,
        "review",
        "Generated patch, check receipt and lineage verified in HydraDB",
    )?;
    let mut w = lock(s)?;
    let failures = w
        .saved
        .policy
        .violations(&change.files, Some(&change.checks));
    let changed_policy = w.saved.policy != policy;
    let flow_allowed = matches!(
        verify(
            &w.gate.state().evidence,
            &w.intent(&artifact.id, "owner"),
            now()
        ),
        Decision::PendingApproval { .. } | Decision::Ready { .. }
    );
    let j = w
        .saved
        .jobs
        .iter_mut()
        .find(|j| j.id == id)
        .ok_or_else(|| err("run missing"))?;
    j.status = if failures.is_empty() && !changed_policy && flow_allowed {
        "awaiting_review"
    } else {
        "blocked"
    }
    .into();
    j.error = if changed_policy {
        Some("Policy changed during generation. Run again under the active policy.".into())
    } else if !flow_allowed {
        Some("Risk investigation -> captured generation -> proposed patch -> Engineering repository. Engineering is not permitted to read the Risk-only input. Human approval cannot override source access.".into())
    } else if failures.is_empty() {
        None
    } else {
        Some(failures.join("; "))
    };
    j.artifact = Some(artifact.id);
    j.change = Some(change);
    j.provider = provider;
    j.graph_id = Some(graph_id);
    j.graph_edges = graph_edges;
    j.input_ids = inputs;
    j.base_files = base_files;
    w.save()?;
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Review {
    context_digest: Digest,
}
async fn repair(
    State(s): State<Shared>,
    h: HeaderMap,
    RoutePath(id): RoutePath<String>,
) -> WebResult {
    let goal = {
        let w = lock(&s)?;
        authorized(&h, &w, false)?;
        let old = w
            .saved
            .jobs
            .iter()
            .find(|j| j.id == id)
            .ok_or_else(|| err("work missing"))?;
        if !old.include_risk {
            return Err(WebError::bad(
                "this run already used approved inputs only".into(),
            ));
        }
        old.goal.clone()
    };
    create(
        State(s),
        h,
        Json(NewWork {
            goal,
            include_risk: false,
            repair_of: Some(id),
        }),
    )
    .await
}
async fn revise(
    State(s): State<Shared>,
    h: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Json(body): Json<Review>,
) -> WebResult {
    let (config, mut change, base, old, parent, hydra) = {
        let mut w = lock(&s)?;
        authorized(&h, &w, true)?;
        let artifact = preflight(&w, &id)?;
        let decision = verify(
            &w.gate.state().evidence,
            &w.intent(&artifact, "owner"),
            now(),
        );
        if !matches!(decision, Decision::Ready { .. })
            || decision.context().map(Context::digest) != Some(body.context_digest)
        {
            return Err(WebError::bad(
                "approve the exact current patch before testing a revision".into(),
            ));
        }
        let j = w
            .saved
            .jobs
            .iter()
            .find(|j| j.id == id)
            .ok_or_else(|| err("work missing"))?;
        if j.operation.is_some() {
            return Err(WebError::bad(
                "published or uncertain work cannot be revised in place".into(),
            ));
        }
        let values = (
            w.config.clone(),
            j.change.clone().ok_or_else(|| err("change missing"))?,
            j.base_files.clone(),
            artifact,
            j.graph_id.clone().ok_or_else(|| err("graph missing"))?,
            w.hydra.clone(),
        );
        let j = w.saved.jobs.iter_mut().find(|j| j.id == id).expect("job");
        j.status = "revising".into();
        w.save()?;
        values
    };
    let result: Result<()> = async {
        let file = change.files.iter_mut().find(|file| file.path == "src/lib.rs").ok_or_else(|| err("Rust file missing"))?;
        file.content.push_str("\n// Reconciliation patch revised after the previous review.\n");
        let hash = patch_digest(&change.repository, &change.base_commit, &change.files);
        let files = change.files.clone();let dir = config.directory.join("runs").join(&id).join(format!("revision-{}",Uuid::new_v4()));
        change.checks = tokio::task::spawn_blocking(move || runner::test(&files, &base, &dir, &hash)).await??;
        if !change.checks.passed { return Err(err("revised patch tests failed")); }
        let artifact = {
            let mut w = lock(&s)?;
            if change.policy != w.saved.policy { return Err(err("policy changed during revision")); }
            let updated = w.gate.edit(&"owner".into(), &old, &serde_json::to_string(&change)?, &format!("revision-{}",Uuid::new_v4().simple()))?;
            w.gate.state().evidence.artifacts[&updated].clone()
        };
        let check_graph = hydra.put("checks", &change.checks, std::slice::from_ref(&parent)).await?;
        let parents = vec![parent.clone(),check_graph.clone()];
        let graph = hydra.put("change", &artifact, &parents).await?;
        let mut w = lock(&s)?;
        if change.policy != w.saved.policy { return Err(err("policy changed during revision persistence")); }
        let j = w.saved.jobs.iter_mut().find(|j|j.id == id).ok_or_else(|| err("work missing"))?;
        if j.artifact.as_ref() != Some(&old) || j.status != "revising" { return Err(err("current patch changed during revision")); }
        j.graph_edges.insert(check_graph, vec![parent]);j.graph_edges.insert(graph.clone(), parents);
        j.artifact = Some(artifact.id);j.graph_id = Some(graph);j.change = Some(change);
        j.approval_graph = None;j.authorization_graph = None;j.status = "awaiting_review".into();j.error = None;
        j.events.push(json!({"at":now(),"stage":"revision","message":"Actual patch revised and Rust tests rerun; previous approval no longer matches"}));w.save()?;
        Ok(())
    }.await;
    if let Err(error) = result {
        if let Ok(mut w) = lock(&s) {
            if let Some(j) = w.saved.jobs.iter_mut().find(|j| j.id == id) {
                j.status = "revision_failed".into();
                j.error = Some(error.to_string());
            }
            let _ = w.save();
        }
        return Err(WebError::bad(error.to_string()));
    }
    Ok(Json(json!({"revised":true,"requires_new_approval":true})))
}
/// Fingerprint of every local value [`graph_check`] validates against HydraDB.
/// Comparing it under the lock right before approval or reservation closes the
/// window between the asynchronous evidence check and the state change.
fn evidence_fingerprint(w: &Workspace, id: &str) -> Result<Digest> {
    let j = w
        .saved
        .jobs
        .iter()
        .find(|j| j.id == id)
        .ok_or_else(|| err("work item missing"))?;
    let permissions = w
        .saved
        .sources
        .iter()
        .filter_map(|source| {
            source
                .permissions_graph
                .as_ref()
                .map(|graph| Ok((graph.clone(), source_permission(&w.gate, &source.id)?)))
        })
        .collect::<Result<Vec<_>>>()?;
    let artifact = j
        .artifact
        .as_ref()
        .and_then(|a| w.gate.state().evidence.artifacts.get(a));
    Ok(digest::structured(
        "publication-evidence",
        &(
            artifact,
            &j.graph_id,
            &j.graph_edges,
            &j.approval_graph,
            &j.authorization_graph,
            &j.status,
            &w.gate.state().evidence.policy,
            &w.saved.policy,
            permissions,
        ),
    ))
}
async fn graph_check(s: &Shared, id: &str) -> Result<Digest> {
    let fingerprint = evidence_fingerprint(&*lock(s)?, id)?;
    graph_check_values(s, id).await?;
    if evidence_fingerprint(&*lock(s)?, id)? != fingerprint {
        return Err(err(
            "workspace evidence changed during the HydraDB check; retry",
        ));
    }
    Ok(fingerprint)
}
async fn graph_check_values(s: &Shared, id: &str) -> Result<()> {
    let (hydra, graph, expected, edges) = {
        let w = lock(s)?;
        let j = w
            .saved
            .jobs
            .iter()
            .find(|j| j.id == id)
            .ok_or_else(|| err("work item missing"))?;
        let a = j
            .artifact
            .as_ref()
            .ok_or_else(|| err("agent has not produced a checked change"))?;
        (
            w.hydra.clone(),
            j.graph_id
                .clone()
                .ok_or_else(|| err("graph evidence missing"))?,
            w.gate.state().evidence.artifacts[a].clone(),
            j.graph_edges.clone(),
        )
    };
    let permissions = {
        let w = lock(s)?;
        w.saved
            .sources
            .iter()
            .filter_map(|source| {
                source
                    .permissions_graph
                    .as_ref()
                    .map(|graph| (graph.clone(), source_permission(&w.gate, &source.id)))
            })
            .collect::<Vec<_>>()
    };
    for (graph, expected) in permissions {
        let expected = expected?;
        let actual: Value = serde_json::from_str(&hydra.get(&graph).await?)?;
        if actual != expected {
            return Err(err(
                "live source permissions do not match the current authority",
            ));
        }
    }
    for (id, parents) in &edges {
        hydra.validate_edges(id, parents).await?;
    }
    let approval = {
        let w = lock(s)?;
        w.saved
            .jobs
            .iter()
            .find(|j| j.id == id)
            .and_then(|j| j.approval_graph.clone())
    };
    if let Some(approval) = approval {
        let _: Approval = serde_json::from_str(&hydra.get(&approval).await?)?;
    }
    let authorization = {
        let w = lock(s)?;
        w.saved
            .jobs
            .iter()
            .find(|j| j.id == id)
            .and_then(|j| j.authorization_graph.clone())
    };
    if let Some(authorization) = authorization {
        let bundle = hydra.verify_authorization(&authorization).await?;
        if bundle.intent.artifact != expected.id {
            return Err(err(
                "authorization graph does not describe the reviewed artifact",
            ));
        }
    }
    let actual: Artifact = serde_json::from_str(&hydra.get(&graph).await?)?;
    if actual != expected {
        return Err(err("live HydraDB evidence no longer matches the review"));
    }
    Ok(())
}
fn preflight(w: &Workspace, id: &str) -> Result<ArtifactId> {
    let j = w
        .saved
        .jobs
        .iter()
        .find(|j| j.id == id)
        .ok_or_else(|| err("work item missing"))?;
    if ["revising", "revision_failed"].contains(&j.status.as_str()) {
        return Err(err(
            "a patch revision must finish before review or publication",
        ));
    }
    if j.status == "interrupted" {
        return Err(err(
            "this work was interrupted by a restart; submit it again",
        ));
    }
    let change = j
        .change
        .as_ref()
        .ok_or_else(|| err("checked change missing"))?;
    if change.policy != w.saved.policy {
        return Err(err(
            "policy changed; run this task again under the active policy",
        ));
    }
    let failures = w
        .saved
        .policy
        .violations(&change.files, Some(&change.checks));
    if !failures.is_empty() {
        return Err(err(&failures.join("; ")));
    }
    if change.base_commit != git_head(&w.config.target_repository)? {
        return Err(err(
            "local repository base changed; regenerate before review",
        ));
    }
    j.artifact.clone().ok_or_else(|| err("artifact missing"))
}
fn approval_key(
    evidence: &Evidence,
    intent: &Intent,
    id: &str,
    context: &Digest,
    timestamp: u64,
) -> Result<String> {
    match verify(evidence, intent, timestamp) {
        Decision::Ready { approval, .. } => Ok(approval
            .0
            .strip_prefix("approval-")
            .ok_or_else(|| err("invalid approval key"))?
            .to_owned()),
        _ => Ok(format!(
            "{id}-{}-{timestamp}",
            context
                .0
                .get(..12)
                .ok_or_else(|| err("invalid context fingerprint"))?
        )),
    }
}
async fn approve(
    State(s): State<Shared>,
    h: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Json(body): Json<Review>,
) -> WebResult {
    {
        let w = lock(&s)?;
        authorized(&h, &w, true)?;
        preflight(&w, &id)?;
    }
    let checked = graph_check(&s, &id).await?;
    let (hydra, approval, policy, parent) = {
        let mut w = lock(&s)?;
        authorized(&h, &w, true)?;
        let artifact = preflight(&w, &id)?;
        if evidence_fingerprint(&w, &id)? != checked {
            return Err(WebError::bad(
                "workspace evidence changed after the HydraDB check; review again".into(),
            ));
        }
        let intent = w.intent(&artifact, "owner");
        let timestamp = now();
        let key = approval_key(
            &w.gate.state().evidence,
            &intent,
            &id,
            &body.context_digest,
            timestamp,
        )?;
        let aid = w.gate.approve(
            &"owner".into(),
            &intent,
            &body.context_digest,
            &key,
            timestamp,
        )?;
        let j = w
            .saved
            .jobs
            .iter()
            .find(|j| j.id == id)
            .ok_or_else(|| err("work missing"))?;
        (
            w.hydra.clone(),
            w.gate.state().evidence.approvals[&aid].clone(),
            w.gate.state().evidence.policy.clone(),
            j.graph_id.clone().ok_or_else(|| err("graph missing"))?,
        )
    };
    let pg = hydra.put("engine-policy", &policy, &[]).await?;
    let ag = hydra.put("approval", &approval, &[parent, pg]).await?;
    let bundle = {
        let w = lock(&s)?;
        let artifact = preflight(&w, &id)?;
        trustgraph_core::bundle::VerifiedBundle::check(
            &w.gate.state().evidence,
            &w.intent(&artifact, "owner"),
            now(),
        )
        .map_err(|_| err("approval no longer authorizes this context"))?
        .export()
        .clone()
    };
    let authorization = hydra
        .store_authorization(&bundle, std::slice::from_ref(&ag))
        .await?;
    let mut w = lock(&s)?;
    if w.gate.state().evidence.policy != policy {
        return Err(WebError::bad("policy changed during approval".into()));
    }
    let j = w
        .saved
        .jobs
        .iter_mut()
        .find(|j| j.id == id)
        .ok_or_else(|| err("work missing"))?;
    j.status = "approved".into();
    j.approval_graph = Some(ag);
    j.authorization_graph = Some(authorization);
    j.events.push(json!({"at":now(),"stage":"approval","message":"Exact operator approval recorded and read back from HydraDB"}));
    w.save()?;
    Ok(Json(json!({"approved":true})))
}
fn record_operation(
    w: &mut Workspace,
    id: &str,
    key: &str,
    state: trustgraph_runtime::OperationState,
) -> Result<()> {
    let j = w
        .saved
        .jobs
        .iter_mut()
        .find(|j| j.id == id)
        .ok_or_else(|| err("run missing"))?;
    j.operation = Some(key.into());
    j.status = match state {
        trustgraph_runtime::OperationState::Published => "published",
        trustgraph_runtime::OperationState::OutcomeUnknown
        | trustgraph_runtime::OperationState::Reserved
        | trustgraph_runtime::OperationState::Dispatching => "outcome_unknown",
        _ => "publication_stopped",
    }
    .into();
    w.save()
}
async fn publish(
    State(s): State<Shared>,
    h: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Json(body): Json<Review>,
) -> WebResult {
    {
        let w = lock(&s)?;
        authorized(&h, &w, true)?;
        let artifact = preflight(&w, &id)?;
        if let Decision::Blocked { violations } = verify(
            &w.gate.state().evidence,
            &w.intent(&artifact, "owner"),
            now(),
        ) {
            let reason = match violations[0].code {
                trustgraph_core::Code::RestrictedSource => {
                    "Engineering cannot receive work derived from a Risk-only source. Approval cannot grant source access."
                }
                trustgraph_core::Code::StaleApproval => {
                    "Approval no longer matches the current patch, policy or audience. Review the current change again."
                }
                trustgraph_core::Code::ApprovalExpired => {
                    "Approval has expired. Review the current change again."
                }
                _ => "Current policy or lineage blocks publication.",
            };
            return Err(WebError::bad(format!(
                "Policy blocks this action: {:?}. {reason}",
                violations[0].code
            )));
        }
    }
    {
        let w = lock(&s)?;
        if w.saved
            .jobs
            .iter()
            .find(|j| j.id == id)
            .filter(|j| j.approval_graph.is_some() && j.authorization_graph.is_some())
            .is_none()
        {
            return Err(WebError::bad(
                "approval must be recorded in HydraDB before publication".into(),
            ));
        }
    }
    let checked = graph_check(&s, &id).await?;
    let key = format!(
        "{id}-{}",
        body.context_digest
            .0
            .get(..12)
            .ok_or_else(|| err("invalid context fingerprint"))?
    );
    let (repository, publisher_directory, base_commit) = {
        let w = lock(&s)?;
        let base = w
            .saved
            .jobs
            .iter()
            .find(|j| j.id == id)
            .and_then(|j| j.change.as_ref())
            .map(|c| c.base_commit.clone())
            .ok_or_else(|| err("checked change missing"))?;
        (
            w.config.github_repository.clone(),
            w.config.directory.join("publisher"),
            base,
        )
    };
    // Read-only GitHub audience and base checks, outside the workspace lock and
    // before anything is reserved, so a definite refusal leaves no operation.
    let github = Arc::new(github::GitHub::new(&repository, publisher_directory)?);
    let precheck = github.clone();
    tokio::task::spawn_blocking(move || precheck.precheck(&base_commit)).await??;
    // Reserve under the lock: re-run every local check, require the evidence the
    // HydraDB check saw, then let the engine verify and stage the exact payload.
    let reserve_state = s.clone();
    let reserve_id = id.clone();
    let reserve_key = key.clone();
    let reserved =
        tokio::task::spawn_blocking(move || -> std::result::Result<(Value, bool), WebError> {
            let mut w = lock(&reserve_state)?;
            authorized(&h, &w, true)?;
            let artifact = preflight(&w, &reserve_id)?;
            if evidence_fingerprint(&w, &reserve_id)? != checked {
                return Err(WebError::bad(
                    "workspace evidence changed after the HydraDB check; review again".into(),
                ));
            }
            let intent = w.intent(&artifact, "owner");
            let result = w
                .gate
                .publish(&intent, &body.context_digest, &reserve_key, now())?;
            let mut deliver = false;
            if let trustgraph_runtime::PublishResult::Operation { operation } = &result {
                deliver = operation.state == trustgraph_runtime::OperationState::OutcomeUnknown;
                record_operation(&mut w, &reserve_id, &reserve_key, operation.state)?;
            }
            Ok((serde_json::to_value(result)?, deliver))
        })
        .await?;
    let (mut result, deliver) = reserved?;
    if deliver {
        // Network writes run without the workspace lock. The publisher refuses
        // a second delivery once a write may have started.
        let delivery = github.clone();
        let delivery_key = key.clone();
        let delivered = tokio::task::spawn_blocking(move || {
            if delivery.deliverable(&delivery_key) {
                delivery.deliver(&delivery_key).map(|_| ())
            } else {
                Ok(())
            }
        })
        .await?;
        let record_state = s.clone();
        let record_id = id.clone();
        let record_key = key.clone();
        result = tokio::task::spawn_blocking(move || -> Result<Value> {
            let mut w = lock(&record_state)?;
            let operation = w.gate.reconcile(&"owner".into(), &record_key, now())?;
            record_operation(&mut w, &record_id, &record_key, operation.state)?;
            Ok(serde_json::to_value(
                trustgraph_runtime::PublishResult::Operation {
                    operation: Box::new(operation),
                },
            )?)
        })
        .await??;
        if let Err(error) = delivered {
            result["delivery_error"] = json!(error.to_string());
        }
    }
    let work_id = id.clone();
    let (hydra, parents) = {
        let w = lock(&s)?;
        let job = w
            .saved
            .jobs
            .iter()
            .find(|j| j.id == work_id)
            .ok_or_else(|| err("work missing"))?;
        (
            w.hydra.clone(),
            [
                job.graph_id.clone(),
                job.approval_graph.clone(),
                job.authorization_graph.clone(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        )
    };
    let receipt = hydra
        .put(
            "publication-receipt",
            &json!({"work":work_id,"result":result,"observed_at":now()}),
            &parents,
        )
        .await;
    let mut response = result;
    match receipt {
        Ok(graph) => {
            response["graph_id"] = json!(graph);
            activity(
                &s,
                &work_id,
                "publication",
                "GitHub action observed; exact publication receipt recorded in HydraDB",
            )?;
        }
        Err(error) => {
            response["evidence_warning"] = json!(format!(
                "GitHub outcome is journaled; graph receipt synchronization needs attention: {error}"
            ));
        }
    }
    Ok(Json(response))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyInput {
    policy: WorkPolicy,
    activate: bool,
    expected_revision: u64,
}
async fn sync_source_permissions(s: &Shared) -> Result<()> {
    let (hydra, entries) = {
        let w = lock(s)?;
        (
            w.hydra.clone(),
            w.saved
                .sources
                .iter()
                .filter(|_| w.config.departments)
                .map(|source| {
                    Ok((
                        source.id.clone(),
                        source.graph_id.clone(),
                        source_permission(&w.gate, &source.id)?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?,
        )
    };
    for (id, parent, expected) in entries {
        let graph = hydra.put("source-permission", &expected, &[parent]).await?;
        let mut w = lock(s)?;
        if source_permission(&w.gate, &id)? != expected {
            return Err(err("source policy changed during graph synchronization"));
        }
        if let Some(source) = w.saved.sources.iter_mut().find(|source| source.id == id) {
            source.permissions_graph = Some(graph);
        }
        w.save()?;
    }
    Ok(())
}
async fn policy(State(s): State<Shared>, h: HeaderMap, Json(body): Json<PolicyInput>) -> WebResult {
    let hydra = {
        let w = lock(&s)?;
        authorized(&h, &w, true)?;
        body.policy.validate()?;
        if body.activate {
            body.policy.validate_live()?;
        }
        if w.config.departments && body.policy.departments.is_none() {
            return Err(WebError::bad(
                "department policy must explicitly specify investigation readers".into(),
            ));
        }
        w.hydra.clone()
    };
    if body.activate {
        hydra.put("policy", &body.policy, &[]).await?;
    }
    let cases = {
        let mut w = lock(&s)?;
        authorized(&h, &w, true)?;
        if body.expected_revision != w.gate.state().evidence.policy.revision {
            return Err(WebError::bad("policy changed; reload before saving".into()));
        }
        let cases: Vec<_> = w.saved.jobs.iter().filter_map(|j|j.change.as_ref().map(|c|json!({"work":j.id,"violations":body.policy.violations(&c.files,Some(&c.checks))}))).collect();
        if body.activate {
            w.gate.set_policy_configuration(
                &"owner".into(),
                digest::structured("work-policy", &body.policy),
                body.policy.approval_minutes * 60,
            )?;
            if w.config.departments {
                for source in w.saved.sources.clone() {
                    apply_source_policy(&mut w.gate, &source, &body.policy)?;
                }
            }
            w.saved.policy = body.policy;
            w.save()?;
        }
        cases
    };
    if body.activate {
        sync_source_permissions(&s).await?;
    }
    Ok(Json(json!({"activated":body.activate,"cases":cases})))
}

async fn connections(State(s): State<Shared>, h: HeaderMap) -> WebResult {
    let (hydra, repo, config) = {
        let w = lock(&s)?;
        authorized(&h, &w, true)?;
        (
            w.hydra.clone(),
            w.config.github_repository.clone(),
            w.config.clone(),
        )
    };
    let graph = hydra.check().await.is_ok();
    let checks = tokio::task::spawn_blocking(move || {
        let codex = runner::bounded(
            std::process::Command::new("codex").args(["login", "status"]),
            None,
            Duration::from_secs(15),
            65_536,
        )
        .is_ok_and(|(code, _)| code == 0);
        let github = github::GitHub::new(
            &config.github_repository,
            config.directory.join("publisher"),
        )
        .is_ok_and(|p| p.check_readers().is_ok());
        (codex, github)
    })
    .await?;
    Ok(Json(
        json!({"hydra":{"connected":graph,"database":hydra.database},"agent":{"connected":checks.0,"name":"Codex"},"github":{"connected":checks.1,"repository":repo},"agent_token_file":lock(&s)?.config.directory.join("agent-token"),"agent_api":"POST /api/work with a goal; Bearer agent token; no agent approval or publication authority"}),
    ))
}

fn activity(s: &Shared, id: &str, stage: &str, message: &str) -> Result<()> {
    let mut w = lock(s)?;
    let j = w
        .saved
        .jobs
        .iter_mut()
        .find(|j| j.id == id)
        .ok_or_else(|| err("work item missing"))?;
    j.events
        .push(json!({"at":now(),"stage":stage,"message":message}));
    w.save()
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ApprovalAssumption {
    Missing,
    Valid,
    Stale,
    PreviousPolicy,
    Expired,
    Destination,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Simulation {
    paths: Vec<String>,
    tests_passed: bool,
    lineage_complete: bool,
    approval: ApprovalAssumption,
    #[serde(default)]
    approval_age_minutes: u64,
    #[serde(default)]
    include_risk: bool,
    #[serde(default = "engineering_audience")]
    audience: String,
    #[serde(skip)]
    restricted_inputs: Vec<ArtifactId>,
    #[serde(default)]
    policy: Option<WorkPolicy>,
}
fn engineering_audience() -> String {
    "engineering".into()
}
fn evaluate_simulation(
    e: &Evidence,
    policy: &WorkPolicy,
    inputs: Vec<ArtifactId>,
    case: &Simulation,
) -> Result<Value> {
    use trustgraph_runtime::{journal::MemoryJournal, publisher::LocalPublisher};
    let timestamp = now();
    let mut evidence = e.clone();
    evidence.approvals.clear();
    evidence.revoked.clear();
    if policy.departments.is_some() {
        for id in &case.restricted_inputs {
            let source = evidence
                .sources
                .get(&SourceId(id.0.clone()))
                .ok_or_else(|| err("restricted input missing"))?;
            let acl = evidence
                .acls
                .get_mut(&source.acl)
                .ok_or_else(|| err("source ACL missing"))?;
            let readers = source_readers(policy, true);
            if acl.readers != readers {
                acl.readers = readers;
                acl.revision += 1;
            }
        }
    }
    evidence.policy.configuration = Some(digest::structured("work-policy", policy));
    evidence.policy.max_approval_lifetime = policy.approval_minutes * 60;
    let mut w = Coordinator::open_with_evidence(
        MemoryJournal::default(),
        LocalPublisher::default(),
        evidence,
    )?;
    let id = ArtifactId(format!("simulation-{}", Uuid::new_v4().simple()));
    w.record_broker_output(
        &"agent".into(),
        id.clone(),
        inputs,
        json!({"paths":case.paths,"assumed_tests":case.tests_passed}).to_string(),
        "hypothetical-policy-simulator",
    )?;
    let mut intent = Intent {
        artifact: id.clone(),
        audience: if case.audience == "risk" {
            "risk-room".into()
        } else {
            "repository".into()
        },
        requester: "owner".into(),
    };
    if !matches!(case.approval, ApprovalAssumption::Missing) {
        let age_minutes = if matches!(case.approval, ApprovalAssumption::Expired)
            && case.approval_age_minutes == 0
        {
            1441
        } else {
            case.approval_age_minutes
        };
        let reviewed_at = timestamp.saturating_sub(age_minutes * 60);
        if let Some(context) = verify(&w.state().evidence, &intent, reviewed_at)
            .context()
            .map(Context::digest)
        {
            w.approve(
                &"owner".into(),
                &intent,
                &context,
                "assumed-review",
                reviewed_at,
            )?;
        }
    }
    if matches!(case.approval, ApprovalAssumption::Destination) {
        intent.audience = if case.audience == "risk" {
            "repository".into()
        } else {
            "risk-room".into()
        };
    }
    if matches!(case.approval, ApprovalAssumption::Stale) {
        intent.artifact = w.edit(
            &"agent".into(),
            &id,
            "changed hypothetical patch",
            &format!("simulation-{}", Uuid::new_v4().simple()),
        )?;
    }
    let mut evidence = w.state().evidence.clone();
    if matches!(case.approval, ApprovalAssumption::PreviousPolicy) {
        evidence.policy.revision += 1;
    }
    if !case.lineage_complete
        && let Origin::Transformation(t) = &evidence.artifacts[&intent.artifact].origin
    {
        let t = evidence
            .transformations
            .get_mut(t)
            .ok_or_else(|| err("simulation manifest missing"))?;
        t.manifest.complete = false;
        t.manifest_hash = digest::structured("context-manifest", &t.manifest);
    }
    let decision = verify(&evidence, &intent, timestamp);
    let mut permitted = evidence.clone();
    permitted.approvals.clear();
    let information_permitted = matches!(
        verify(&permitted, &intent, timestamp),
        Decision::PendingApproval { .. }
    );
    let files: Vec<_> = case
        .paths
        .iter()
        .map(|p| ChangedFile {
            path: p.clone(),
            content: "hypothetical change".into(),
        })
        .collect();
    let check = CheckReceipt {
        patch_digest: digest::content(b"hypothesis"),
        passed: case.tests_passed,
        command: "assumed test outcome; not executed".into(),
        output: String::new(),
    };
    let violations = policy.violations(&files, Some(&check));
    let status = if !violations.is_empty() || matches!(decision, Decision::Blocked { .. }) {
        "blocked"
    } else if matches!(decision, Decision::Ready { .. }) {
        "allowed"
    } else {
        "needs_approval"
    };
    let counterexample = if !violations.is_empty() {
        Some(
            json!({"rule":"business policy","explanation":violations[0],"path":[case.paths.first().map(String::as_str).unwrap_or("No proposed files"), policy.name.as_str(), "Publication gate"]}),
        )
    } else if let Decision::Blocked { violations } = &decision {
        let explanation = match violations[0].code {
            trustgraph_core::Code::RestrictedSource => {
                "Risk investigation -> agent generation -> proposed patch -> Engineering repository: Engineering cannot read the Risk-only input. Approval cannot grant source access."
            }
            trustgraph_core::Code::IncompleteLineage => {
                "An input manifest is incomplete. The artifact cannot be authorized."
            }
            trustgraph_core::Code::ApprovalExpired => {
                "The approval expired before the proposed action. Obtain a new approval."
            }
            trustgraph_core::Code::StaleApproval
                if matches!(case.approval, ApprovalAssumption::PreviousPolicy) =>
            {
                "Policy changed after approval. The previous approval does not authorize the new policy context."
            }
            trustgraph_core::Code::StaleApproval
                if matches!(case.approval, ApprovalAssumption::Destination) =>
            {
                "The audience changed after approval. The saved approval does not authorize this destination."
            }
            _ => "The saved approval covers a different artifact version.",
        };
        Some(
            json!({"rule":violations[0].code,"explanation":explanation,"path":["Recorded inputs", "Generated artifact", "Exact approval", "Publication gate"]}),
        )
    } else if matches!(decision, Decision::PendingApproval { .. }) {
        Some(
            json!({"rule":"APPROVAL_REQUIRED","explanation":"The proposed change is permitted but still requires exact operator approval.","path":["Proposed change", "Operator approval", "Publication gate"]}),
        )
    } else {
        None
    };
    Ok(
        json!({"hypothetical":true,"action_effects":0,"status":status,"information_permitted":information_permitted,"policy_passed":violations.is_empty(),"approval_matched":matches!(decision,Decision::Ready{..}),"violations":violations,"counterexample":counterexample,"decision":decision,"assumptions":case}),
    )
}
fn compare_simulation(
    evidence: &Evidence,
    active: &WorkPolicy,
    candidate: &WorkPolicy,
    inputs: Vec<ArtifactId>,
    case: &Simulation,
) -> Result<Value> {
    active.validate()?;
    candidate.validate()?;
    let mut result = evaluate_simulation(evidence, candidate, inputs.clone(), case)?;
    let baseline = evaluate_simulation(evidence, active, inputs, case)?;
    result["baseline"] = baseline;
    result["active_policy"] = json!(active);
    result["candidate_policy"] = json!(candidate);
    result["policy_changed"] = json!(active != candidate);
    result["active_policy_fingerprint"] = json!(digest::structured("work-policy", active));
    result["candidate_policy_fingerprint"] = json!(digest::structured("work-policy", candidate));
    result["active_policy_revision"] = json!(evidence.policy.revision);
    result["policy_activated"] = json!(false);
    result["code_executed"] = json!(false);
    Ok(result)
}
const SIMULATIONS_PER_WINDOW: usize = 20;
const SIMULATION_WINDOW: Duration = Duration::from_secs(60);
fn admit_simulation(
    recent: &mut std::collections::VecDeque<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    while recent
        .front()
        .is_some_and(|t| now.duration_since(*t) >= SIMULATION_WINDOW)
    {
        recent.pop_front();
    }
    if recent.len() >= SIMULATIONS_PER_WINDOW {
        return false;
    }
    recent.push_back(now);
    true
}
async fn simulate(
    State(s): State<Shared>,
    h: HeaderMap,
    Json(mut case): Json<Simulation>,
) -> WebResult {
    if case.paths.len() > 16
        || case.paths.iter().any(|p| p.len() > 160)
        || case.approval_age_minutes > 10080
        || !["engineering", "risk"].contains(&case.audience.as_str())
    {
        return Err(WebError::bad("bounded file list required".into()));
    }
    let (hydra, sources, evidence, active, impacts) = {
        let mut w = lock(&s)?;
        authorized(&h, &w, false)?;
        // Every simulation writes candidate-policy evidence to HydraDB; bound it.
        if !admit_simulation(&mut w.simulations, std::time::Instant::now()) {
            return Err(WebError(
                StatusCode::TOO_MANY_REQUESTS,
                format!("at most {SIMULATIONS_PER_WINDOW} simulations per minute; retry shortly"),
            ));
        }
        let candidate = case.policy.as_ref().unwrap_or(&w.saved.policy);
        candidate.validate()?;
        let impacts: Vec<_> = w
            .saved
            .jobs
            .iter()
            .rev()
            .filter_map(|j| {
                j.change.as_ref().map(|c| json!({
            "work":j.id,"goal":j.goal,"violations":candidate.violations(&c.files,Some(&c.checks)),
            "requires_fresh_run":c.policy != *candidate
        }))
            })
            .take(10)
            .collect();
        (
            w.hydra.clone(),
            w.saved
                .sources
                .iter()
                .filter(|source| case.include_risk || !source.restricted)
                .cloned()
                .collect::<Vec<_>>(),
            w.gate.state().evidence.clone(),
            w.saved.policy.clone(),
            impacts,
        )
    };
    for source in &sources {
        let a: Artifact = serde_json::from_str(&hydra.get(&source.graph_id).await?)?;
        if a.id != source.id || a.hash != source.hash {
            return Err(WebError::bad(
                "live source no longer matches captured evidence".into(),
            ));
        }
    }
    case.restricted_inputs = sources
        .iter()
        .filter(|source| source.restricted)
        .map(|source| source.id.clone())
        .collect();
    let candidate = case.policy.as_ref().unwrap_or(&active);
    let mut result = compare_simulation(
        &evidence,
        &active,
        candidate,
        sources.iter().map(|s| s.id.clone()).collect(),
        &case,
    )?;
    result["impact"] = json!(impacts);
    let candidate_graph = hydra.put("policy-candidate", candidate, &[]).await?;
    let mut parents: Vec<_> = sources.iter().map(|s| s.graph_id.clone()).collect();
    parents.push(candidate_graph.clone());
    let graph = hydra.put("simulation", &result, &parents).await?;
    result["graph_id"] = json!(graph);
    result["candidate_policy_graph_id"] = json!(candidate_graph);
    let current_revision = lock(&s)?.gate.state().evidence.policy.revision;
    result["active_policy_changed_during_check"] =
        json!(current_revision != evidence.policy.revision);
    Ok(Json(result))
}

async fn graph(State(s): State<Shared>, h: HeaderMap) -> WebResult {
    let (hydra, source_ids) = {
        let w = lock(&s)?;
        authorized(&h, &w, false)?;
        (
            w.hydra.clone(),
            w.saved
                .sources
                .iter()
                .map(|s| s.id.0.clone())
                .collect::<Vec<_>>(),
        )
    };
    let records = hydra
        .query(
            "MATCH (n:TrustGraphRecord) RETURN n.kind AS kind,count(n) AS records",
            json!({}),
        )
        .await?;
    let links=hydra.query("MATCH (:TrustGraphRecord)-[r:USED_AS_INPUT]->(:TrustGraphRecord) RETURN count(r) AS links",json!({})).await?;
    let knowledge_result = hydra.knowledge_progress(&source_ids).await;
    let knowledge_error = knowledge_result.as_ref().err().map(ToString::to_string);
    let knowledge = knowledge_result.unwrap_or_default();
    Ok(Json(
        json!({"database":hydra.database,"collection":"trustgraph-governance","records":records,"links":links,"connected":true,"knowledge":knowledge,"knowledge_error":knowledge_error}),
    ))
}
async fn publication_gate(
    State(s): State<Shared>,
    h: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Json(body): Json<Review>,
) -> WebResult {
    publish(State(s), h, RoutePath(id), Json(body)).await
}

#[cfg(test)]
mod simulator_tests {
    use super::*;
    #[test]
    fn bank_policies_check_source_flow_repair_audiences_and_exact_approval() {
        use trustgraph_runtime::{journal::MemoryJournal, publisher::LocalPublisher};
        let mut gate = Coordinator::open_with_evidence(
            MemoryJournal::default(),
            LocalPublisher::default(),
            genesis_for("owner/repo", true),
        )
        .unwrap();
        let all: Readers = BTreeSet::from(["owner".into(), "engineering".into(), "agent".into()]);
        let public = gate
            .import_source(
                &"owner".into(),
                "engineering-source".into(),
                "Engineering requirements".into(),
                all.clone(),
            )
            .unwrap();
        let risk = gate
            .import_source(
                &"owner".into(),
                "risk-source".into(),
                "Risk investigation".into(),
                all,
            )
            .unwrap();
        gate.change_acl(
            &"owner".into(),
            &"risk-source".into(),
            BTreeSet::from(["owner".into(), "agent".into()]),
            "risk-only",
        )
        .unwrap();
        let policy = department_policy();
        let mut case = Simulation {
            paths: vec!["src/lib.rs".into()],
            tests_passed: true,
            lineage_complete: true,
            approval: ApprovalAssumption::Valid,
            approval_age_minutes: 0,
            include_risk: true,
            audience: "engineering".into(),
            restricted_inputs: vec![risk.clone()],
            policy: Some(policy.clone()),
        };
        let risky = evaluate_simulation(
            &gate.state().evidence,
            &policy,
            vec![public.clone(), risk.clone()],
            &case,
        )
        .unwrap();
        assert_eq!(risky["counterexample"]["rule"], "RESTRICTED_SOURCE");
        assert_eq!(risky["information_permitted"], false);
        case.audience = "risk".into();
        assert_eq!(
            evaluate_simulation(
                &gate.state().evidence,
                &policy,
                vec![public.clone(), risk.clone()],
                &case
            )
            .unwrap()["status"],
            "allowed"
        );
        case.audience = "engineering".into();
        case.include_risk = false;
        assert_eq!(
            evaluate_simulation(&gate.state().evidence, &policy, vec![public.clone()], &case)
                .unwrap()["status"],
            "allowed"
        );
        case.approval = ApprovalAssumption::Destination;
        assert_eq!(
            evaluate_simulation(&gate.state().evidence, &policy, vec![public.clone()], &case)
                .unwrap()["counterexample"]["rule"],
            "STALE_APPROVAL"
        );
        case.approval = ApprovalAssumption::Valid;
        let mut granted = policy.clone();
        granted
            .departments
            .as_mut()
            .unwrap()
            .investigation_readers
            .push("engineering".into());
        assert_eq!(
            compare_simulation(
                &gate.state().evidence,
                &policy,
                &granted,
                vec![public.clone(), risk.clone()],
                &case
            )
            .unwrap()["baseline"]["status"],
            "blocked"
        );
        assert_eq!(
            compare_simulation(
                &gate.state().evidence,
                &policy,
                &granted,
                vec![public.clone(), risk.clone()],
                &case
            )
            .unwrap()["status"],
            "allowed"
        );
        // The fully assembled model prompt is also derived work. It must retain Risk restrictions.
        let prompt = gate
            .record_broker_output(
                &"agent".into(),
                "captured-prompt".into(),
                vec![public.clone(), risk],
                "actual supplied prompt".into(),
                "broker",
            )
            .unwrap();
        let patch = gate
            .record_broker_output(
                &"agent".into(),
                "risk-patch".into(),
                vec![prompt],
                "does not quote the source".into(),
                "model",
            )
            .unwrap();
        let intent = Intent {
            artifact: patch,
            audience: "repository".into(),
            requester: "owner".into(),
        };
        assert!(matches!(
            verify(&gate.state().evidence, &intent, 1),
            Decision::Blocked { .. }
        ));
        assert!(
            gate.approve(
                &"owner".into(),
                &intent,
                &digest::content(b"not permission"),
                "cannot-override",
                1
            )
            .is_err()
        );
        assert_eq!(gate.publisher.sends, 0);
    }
    #[test]
    fn expired_review_can_be_renewed_and_retries_reuse_current_approval() {
        let mut w = Coordinator::open_with_evidence(
            trustgraph_runtime::journal::MemoryJournal::default(),
            trustgraph_runtime::publisher::LocalPublisher::default(),
            genesis("owner/repo"),
        )
        .unwrap();
        let source = w
            .import_source(
                &"owner".into(),
                "source".into(),
                "input".into(),
                BTreeSet::from(["owner".into(), "agent".into()]),
            )
            .unwrap();
        let patch = w
            .record_broker_output(
                &"agent".into(),
                "patch".into(),
                vec![source.clone()],
                "patch bytes".into(),
                "test",
            )
            .unwrap();
        let intent = Intent {
            artifact: patch,
            audience: "repository".into(),
            requester: "owner".into(),
        };
        let context = verify(&w.state().evidence, &intent, 100)
            .context()
            .unwrap()
            .digest();
        let old = w
            .approve(&"owner".into(), &intent, &context, "old", 100)
            .unwrap();
        let later = 100 + w.state().evidence.policy.max_approval_lifetime;
        assert!(matches!(
            verify(&w.state().evidence, &intent, later),
            Decision::Blocked { .. }
        ));
        let key = approval_key(&w.state().evidence, &intent, "job-renew", &context, later).unwrap();
        let renewed = w
            .approve(&"owner".into(), &intent, &context, &key, later)
            .unwrap();
        assert_ne!(old, renewed);
        assert_eq!(
            approval_key(
                &w.state().evidence,
                &intent,
                "job-renew",
                &context,
                later + 1
            )
            .unwrap(),
            key
        );
        assert!(matches!(
            verify(&w.state().evidence, &intent, later + 1),
            Decision::Ready { .. }
        ));
        assert!(
            w.approve(&"agent".into(), &intent, &context, "forged", later + 1)
                .is_err()
        );
        assert_eq!(w.publisher.sends, 0);
    }
    #[test]
    fn candidate_comparison_is_bounded_supports_custom_paths_and_never_mutates_live_state() {
        let mut w = Coordinator::open_with_evidence(
            trustgraph_runtime::journal::MemoryJournal::default(),
            trustgraph_runtime::publisher::LocalPublisher::default(),
            genesis("owner/repo"),
        )
        .unwrap();
        let source = w
            .import_source(
                &"owner".into(),
                "source".into(),
                "input".into(),
                BTreeSet::from(["owner".into(), "agent".into()]),
            )
            .unwrap();
        let active = WorkPolicy::default();
        let mut candidate = active.clone();
        candidate.allowed_paths = vec!["src/lib.rs".into(), "README.md".into()];
        candidate.max_changed_files = 2;
        let mut case = Simulation {
            paths: vec!["src/lib.rs".into(), "README.md".into()],
            tests_passed: true,
            lineage_complete: true,
            approval: ApprovalAssumption::Valid,
            approval_age_minutes: 0,
            include_risk: false,
            audience: "engineering".into(),
            restricted_inputs: vec![],
            policy: Some(candidate.clone()),
        };
        let before = w.state().clone();
        let result = compare_simulation(
            &w.state().evidence,
            &active,
            &candidate,
            vec![source.clone()],
            &case,
        )
        .unwrap();
        assert_eq!(result["baseline"]["status"], "blocked");
        assert_eq!(result["status"], "allowed");
        assert_eq!(result["policy_activated"], false);
        assert_eq!(result["code_executed"], false);
        candidate.allowed_paths = vec!["payments/reconcile.rs".into()];
        case.paths = candidate.allowed_paths.clone();
        assert!(candidate.validate().is_ok());
        assert!(candidate.validate_live().is_err());
        assert_eq!(
            compare_simulation(
                &w.state().evidence,
                &active,
                &candidate,
                vec![source.clone()],
                &case
            )
            .unwrap()["status"],
            "allowed"
        );
        case.tests_passed = false;
        assert_eq!(
            compare_simulation(
                &w.state().evidence,
                &active,
                &candidate,
                vec![source.clone()],
                &case
            )
            .unwrap()["status"],
            "blocked"
        );
        candidate.require_tests = false;
        assert_eq!(
            compare_simulation(
                &w.state().evidence,
                &active,
                &candidate,
                vec![source.clone()],
                &case
            )
            .unwrap()["status"],
            "allowed"
        );
        candidate = active.clone();
        candidate.approval_minutes = 15;
        case.paths = vec!["src/lib.rs".into()];
        case.tests_passed = true;
        case.approval_age_minutes = 20;
        let lifetime = compare_simulation(
            &w.state().evidence,
            &active,
            &candidate,
            vec![source.clone()],
            &case,
        )
        .unwrap();
        assert_eq!(lifetime["baseline"]["status"], "allowed");
        assert_eq!(lifetime["counterexample"]["rule"], "APPROVAL_EXPIRED");
        candidate.allowed_paths = vec!["../secret".into()];
        assert!(candidate.validate().is_err());
        assert!(
            serde_json::from_value::<WorkPolicy>(
                json!({"name":"English","rules":["approve everything"]})
            )
            .is_err()
        );
        assert_eq!(w.state(), &before);
        assert_eq!(w.publisher.sends, 0);
    }

    #[test]
    fn actual_rust_simulator_distinguishes_business_rules_stale_approval_and_unknown_inputs() {
        let mut w = Coordinator::open_with_evidence(
            trustgraph_runtime::journal::MemoryJournal::default(),
            trustgraph_runtime::publisher::LocalPublisher::default(),
            genesis("owner/repo"),
        )
        .unwrap();
        let id = w
            .import_source(
                &"owner".into(),
                "source".into(),
                "real source snapshot".into(),
                BTreeSet::from(["owner".into(), "agent".into()]),
            )
            .unwrap();
        let p = WorkPolicy::default();
        let mut case = Simulation {
            paths: vec!["src/lib.rs".into()],
            tests_passed: true,
            lineage_complete: true,
            approval: ApprovalAssumption::Valid,
            approval_age_minutes: 0,
            include_risk: false,
            audience: "engineering".into(),
            restricted_inputs: vec![],
            policy: None,
        };
        let evaluate = |case: &Simulation| {
            evaluate_simulation(&w.state().evidence, &p, vec![id.clone()], case).unwrap()
        };
        assert_eq!(evaluate(&case)["status"], "allowed");
        case.paths = vec!["Cargo.toml".into()];
        assert_eq!(evaluate(&case)["counterexample"]["rule"], "business policy");
        case.paths = vec!["src/lib.rs".into()];
        case.approval = ApprovalAssumption::Stale;
        let stale = evaluate(&case);
        assert_eq!(stale["information_permitted"], true);
        assert_eq!(stale["counterexample"]["rule"], "STALE_APPROVAL");
        case.approval = ApprovalAssumption::Valid;
        case.lineage_complete = false;
        assert_eq!(
            evaluate(&case)["counterexample"]["rule"],
            "INCOMPLETE_LINEAGE"
        );
        case.lineage_complete = true;
        case.approval = ApprovalAssumption::Missing;
        assert_eq!(evaluate(&case)["status"], "needs_approval");
        case.approval = ApprovalAssumption::PreviousPolicy;
        assert_eq!(evaluate(&case)["counterexample"]["rule"], "STALE_APPROVAL");
        case.approval = ApprovalAssumption::Expired;
        assert_eq!(
            evaluate(&case)["counterexample"]["rule"],
            "APPROVAL_EXPIRED"
        );
        case.approval = ApprovalAssumption::Valid;
        case.tests_passed = false;
        assert_eq!(evaluate(&case)["status"], "blocked");
        assert_eq!(w.publisher.sends, 0);
    }
}

/// Drives the real live router over loopback HTTP without HydraDB, Codex or
/// GitHub: every request here is decided before any evidence or network I/O.
#[cfg(test)]
mod router_tests {
    use super::*;
    use reqwest::{Method, header as h};

    const AGENT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    struct Server {
        base: String,
        host: String,
        launch: String,
        client: reqwest::Client,
        shared: Shared,
        _dir: tempfile::TempDir,
    }
    async fn server() -> Server {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let config = Config {
            directory: dir.path().into(),
            context_repository: dir.path().into(),
            context_files: vec!["README.md".into()],
            target_repository: dir.path().into(),
            github_repository: "owner/repo".into(),
            departments: false,
        };
        let gate = Coordinator::open_with_evidence(
            DiskJournal::open(&dir.path().join("journal")).unwrap(),
            github::GitHub::new("owner/repo", dir.path().join("publisher")).unwrap(),
            genesis_for("owner/repo", false),
        )
        .unwrap();
        let owner = auth::OwnerSessions::new(&port.to_string(), auth::SESSION_LIFETIME);
        let launch = owner.launch_secret().unwrap();
        let shared = Arc::new(Mutex::new(Workspace {
            config,
            gate,
            saved: Saved {
                policy: WorkPolicy::default(),
                ..Saved::default()
            },
            hydra: hydra::Hydra::unreachable(),
            owner,
            agent_token: AGENT.into(),
            port,
            simulations: Default::default(),
        }));
        let app = routes(shared.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Server {
            base: format!("http://127.0.0.1:{port}"),
            host: format!("127.0.0.1:{port}"),
            launch,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            shared,
            _dir: dir,
        }
    }
    impl Server {
        async fn call(
            &self,
            method: Method,
            path: &str,
            headers: &[(h::HeaderName, String)],
            body: Option<Value>,
        ) -> (u16, Option<String>, Value) {
            let mut request = self.client.request(method, format!("{}{path}", self.base));
            for (k, v) in headers {
                request = request.header(k.clone(), v);
            }
            if let Some(body) = body {
                request = request.json(&body);
            }
            let response = request.send().await.unwrap();
            let status = response.status().as_u16();
            let cookie = response
                .headers()
                .get(h::SET_COOKIE)
                .map(|v| v.to_str().unwrap().to_owned());
            let value = response.json().await.unwrap_or(Value::Null);
            (status, cookie, value)
        }
    }
    fn bearer(token: &str) -> Vec<(h::HeaderName, String)> {
        vec![(h::AUTHORIZATION, format!("Bearer {token}"))]
    }
    fn owner_routes() -> Vec<(Method, &'static str, Option<Value>)> {
        let review = json!({"context_digest":"0123456789abcdef0123"});
        vec![
            (
                Method::POST,
                "/api/work/job-1/approve",
                Some(review.clone()),
            ),
            (
                Method::POST,
                "/api/work/job-1/publish",
                Some(review.clone()),
            ),
            (Method::POST, "/api/work/job-1/gate", Some(review.clone())),
            (Method::POST, "/api/work/job-1/revise", Some(review)),
            (
                Method::POST,
                "/api/policy",
                Some(json!({"policy":WorkPolicy::default(),"activate":true,"expected_revision":1})),
            ),
            (Method::GET, "/api/connections", None),
        ]
    }

    #[tokio::test]
    async fn agent_and_launch_bearers_cannot_use_owner_routes() {
        let server = server().await;
        for (method, path, body) in owner_routes() {
            for token in [AGENT, server.launch.as_str()] {
                let (status, cookie, _) = server
                    .call(method.clone(), path, &bearer(token), body.clone())
                    .await;
                assert_eq!(status, 401, "{method} {path}");
                assert!(cookie.is_none());
            }
            let (status, _, _) = server.call(method.clone(), path, &[], body.clone()).await;
            assert_eq!(status, 401, "{method} {path} without credentials");
        }
        // The agent bearer still reaches agent routes.
        let (status, _, _) = server
            .call(Method::GET, "/api/work", &bearer(AGENT), None)
            .await;
        assert_eq!(status, 200);
        // Probing owner routes did not consume the launch secret.
        assert!(
            lock(&server.shared)
                .unwrap()
                .owner
                .launch_secret()
                .is_some()
        );
    }

    #[tokio::test]
    async fn launch_secret_is_exchanged_once_for_a_cookie_only_session() {
        let server = server().await;
        let (status, cookie, _) = server
            .call(Method::POST, "/api/session", &bearer(AGENT), None)
            .await;
        assert_eq!(
            (status, cookie),
            (401, None),
            "agent token is not a launch secret"
        );
        let (status, cookie, _) = server
            .call(Method::POST, "/api/session", &bearer(&server.launch), None)
            .await;
        assert_eq!(status, 200);
        let cookie = cookie.unwrap();
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Max-Age="));
        let port = server.host.rsplit(':').next().unwrap();
        assert!(cookie.starts_with(&format!("trustgraph_owner_{port}=")));
        assert!(!cookie.contains(&server.launch));
        let session = vec![(h::COOKIE, cookie.split(';').next().unwrap().to_owned())];
        let (status, again, _) = server
            .call(Method::POST, "/api/session", &bearer(&server.launch), None)
            .await;
        assert_eq!((status, again), (401, None), "launch secret is single-use");
        assert!(
            lock(&server.shared)
                .unwrap()
                .owner
                .launch_secret()
                .is_none()
        );
        // The session cookie authorizes owner and agent routes.
        let (status, _, _) = server.call(Method::GET, "/api/work", &session, None).await;
        assert_eq!(status, 200);
        let (status, _, value) = server
            .call(
                Method::POST,
                "/api/policy",
                &session,
                Some(
                    json!({"policy":WorkPolicy::default(),"activate":false,"expected_revision":1}),
                ),
            )
            .await;
        assert_eq!(status, 200, "{value}");
        let (status, _, _) = server
            .call(
                Method::POST,
                "/api/work/job-1/approve",
                &session,
                Some(json!({"context_digest":"0123456789abcdef0123"})),
            )
            .await;
        assert_eq!(
            status, 400,
            "authenticated, then refused because the work is missing"
        );
        // A forged or replayed launch value in the cookie is rejected.
        let forged = vec![(
            h::COOKIE,
            format!("trustgraph_owner_{port}={}", server.launch),
        )];
        let (status, _, _) = server
            .call(Method::GET, "/api/connections", &forged, None)
            .await;
        assert_eq!(status, 401);
    }

    #[tokio::test]
    async fn host_and_origin_checks_reject_other_sites_and_ports() {
        let server = server().await;
        let port: u16 = server.host.rsplit(':').next().unwrap().parse().unwrap();
        for (host, origin) in [
            (format!("127.0.0.1:{}", port.wrapping_add(1)), None),
            (format!("evil.test:{port}"), None),
            (format!("127.0.0.1.evil.test:{port}"), None),
            ("127.0.0.1".to_owned(), None),
            (server.host.clone(), Some("http://evil.test".to_owned())),
            (
                server.host.clone(),
                Some(format!("https://{}", server.host)),
            ),
        ] {
            let mut headers = bearer(AGENT);
            headers.push((h::HOST, host.clone()));
            if let Some(origin) = &origin {
                headers.push((h::ORIGIN, origin.clone()));
            }
            let (status, _, _) = server.call(Method::GET, "/api/work", &headers, None).await;
            assert_eq!(status, 403, "{host} {origin:?}");
            let (status, cookie, _) = server
                .call(
                    Method::POST,
                    "/api/session",
                    &{
                        let mut h = headers.clone();
                        h[0] = (h::AUTHORIZATION, format!("Bearer {}", server.launch));
                        h
                    },
                    None,
                )
                .await;
            assert_eq!((status, cookie), (403, None));
            let (status, _, _) = server.call(Method::GET, "/", &headers, None).await;
            assert_eq!(status, 403);
        }
        let mut same = bearer(AGENT);
        same.push((h::ORIGIN, format!("http://{}", server.host)));
        let (status, _, _) = server.call(Method::GET, "/api/work", &same, None).await;
        assert_eq!(status, 200);
        let mut localhost = bearer(AGENT);
        localhost.push((h::HOST, format!("localhost:{port}")));
        let (status, _, _) = server
            .call(Method::GET, "/api/work", &localhost, None)
            .await;
        assert_eq!(status, 200);
    }

    #[test]
    fn restart_interrupts_queued_running_and_revising_work() {
        let job = |id: &str, status: &str| Job {
            id: id.into(),
            goal: "goal".into(),
            status: status.into(),
            created_at: 0,
            artifact: None,
            change: None,
            error: None,
            provider: Value::Null,
            graph_id: None,
            graph_edges: BTreeMap::new(),
            base_files: vec![],
            input_ids: vec![],
            operation: None,
            events: vec![],
            plan: Value::Null,
            approval_graph: None,
            authorization_graph: None,
            include_risk: false,
            repair_of: None,
        };
        let mut saved = Saved {
            jobs: vec![
                job("a", "queued"),
                job("b", "running"),
                job("c", "revising"),
                job("d", "awaiting_review"),
                job("e", "published"),
            ],
            ..Saved::default()
        };
        recover_jobs(&mut saved);
        let states: Vec<_> = saved.jobs.iter().map(|j| j.status.as_str()).collect();
        assert_eq!(
            states,
            [
                "interrupted",
                "interrupted",
                "interrupted",
                "awaiting_review",
                "published"
            ]
        );
        assert!(
            !saved
                .jobs
                .iter()
                .any(|j| ["queued", "running", "revising"].contains(&j.status.as_str())),
            "a new run is no longer blocked"
        );
        assert!(saved.jobs[2].error.as_deref().unwrap().contains("revision"));
    }

    #[tokio::test]
    async fn interrupted_revision_cannot_be_reviewed_and_a_worker_panic_is_recorded() {
        let server = server().await;
        {
            let mut w = lock(&server.shared).unwrap();
            let mut saved = Saved::default();
            for (id, status) in [("job-r", "revising"), ("job-p", "running")] {
                saved.jobs.push(Job {
                    id: id.into(),
                    goal: "goal".into(),
                    status: status.into(),
                    created_at: 0,
                    artifact: None,
                    change: None,
                    error: None,
                    provider: Value::Null,
                    graph_id: None,
                    graph_edges: BTreeMap::new(),
                    base_files: vec![],
                    input_ids: vec![],
                    operation: None,
                    events: vec![],
                    plan: Value::Null,
                    approval_graph: None,
                    authorization_graph: None,
                    include_risk: false,
                    repair_of: None,
                });
            }
            recover_jobs(&mut saved);
            saved.jobs[1].status = "running".into();
            w.saved.jobs = saved.jobs;
            assert!(
                preflight(&w, "job-r")
                    .unwrap_err()
                    .to_string()
                    .contains("interrupted")
            );
        }
        fail_job(&server.shared, "job-p", "worker stopped".into());
        let w = lock(&server.shared).unwrap();
        assert_eq!(w.saved.jobs[1].status, "blocked");
        assert_eq!(
            w.saved.jobs[0].status, "interrupted",
            "only active jobs are failed"
        );
    }

    #[test]
    fn simulations_are_rate_limited_per_window() {
        let mut recent = std::collections::VecDeque::new();
        let start = std::time::Instant::now();
        for _ in 0..SIMULATIONS_PER_WINDOW {
            assert!(admit_simulation(&mut recent, start));
        }
        assert!(!admit_simulation(&mut recent, start));
        assert!(admit_simulation(&mut recent, start + SIMULATION_WINDOW));
    }
}
