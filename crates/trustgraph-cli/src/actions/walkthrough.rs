//! UI-driven scenarios over the same persistent approval and publication gate.
use super::*;

const DEFAULT_SOURCE: &str = "https://factory.8090.ai/";
const DEFAULT_COMPOSIO: &str = "https://dashboard.composio.dev/";

pub(super) fn metadata(app: &App) -> Value {
    let receipt = app
        .gate
        .state()
        .operations
        .values()
        .filter_map(|op| op.receipt.as_ref())
        .filter(|r| !r.simulated)
        .max_by_key(|r| r.observed_at);
    json!({"source_url":app.config.source_url.as_deref().unwrap_or(DEFAULT_SOURCE),
        "composio_url":app.config.composio_dashboard_url.as_deref().unwrap_or(DEFAULT_COMPOSIO),
        "github_url":format!("https://github.com/{}/issues",app.config.github_repository),
        "verified_receipt":receipt,
        "source_label":"Saved native 8090 draft"})
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Scenario {
    Permission,
    Revocation,
    Revision,
}
impl Scenario {
    fn name(&self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Revocation => "revocation",
            Self::Revision => "revision",
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Start {
    scenario: Scenario,
    key: String,
}

pub(super) async fn start(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<Start>,
) -> WebResult {
    locked(shared, move |app| {
        auth(&headers, app, true)?;
        let uuid = uuid::Uuid::parse_str(&input.key)
            .map_err(|_| invalid("walkthrough key must be a UUID"))?;
        let key = format!("demo-{}-{}", input.scenario.name(), uuid.simple());
        if app.gate.state().requests.contains_key(&key) {
            return view(app, &key);
        }
        // Starting is an explicit local reset of the publication grant, never
        // an external write or a deletion of an earlier issue/receipt.
        if permission_granted(app)? {
            app.gate.set_policy_configuration(
                &"owner".into(),
                app.config.permission_scope(false),
                1800,
            )?;
        }
        let mut proposal: Proposal = serde_json::from_str(include_str!(
            "../../../../examples/8090/live-demo-issue.json"
        ))
        .map_err(storage)?;
        proposal.key = key.clone();
        let artifact = capture(app, proposal.clone())?;
        app.gate.submit_request(
            &"agent".into(),
            Submission {
                key: key.clone(),
                title: proposal.title,
                summary: format!("walkthrough:{}", input.scenario.name()),
                artifact,
                audience: "issue".into(),
            },
            now(),
        )?;
        view(app, &key)
    })
    .await
}

pub(super) async fn evaluate(
    State(shared): State<Shared>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Json(input): Json<Review>,
) -> WebResult {
    locked(shared,move |app| {
        auth(&headers,app,true)?;
        let artifact = current(app,&id,&input)?;
        let permission = permission_granted(app)?;
        let approved = matches!(verify(&app.gate.state().evidence,&intent(&artifact),now()),Decision::Ready{..});
        let reason = if !permission {"permission_revoked"} else if !approved {"review_required"} else {"ready"};
        Ok(json!({"allowed":permission && approved,"reason":reason,
            "publication_allowed":permission,"approved":approved,"context_digest":input.context_digest,
            "provider_called":false,"checked_at":now(),"action":view(app,&id)?}))
    }).await
}

pub(super) async fn edit(
    State(shared): State<Shared>,
    headers: HeaderMap,
    RoutePath(id): RoutePath<String>,
    Json(input): Json<Revision>,
) -> WebResult {
    locked(shared, move |app| {
        auth(&headers, app, true)?;
        let request = app.gate.review_request(&"owner".into(), &id)?.clone();
        if request.revision() != input.revision || request.operation.is_some() {
            return Err(Error::Conflict);
        }
        let previous: Envelope =
            serde_json::from_str(&app.gate.state().evidence.artifacts[request.artifact()].content)
                .map_err(storage)?;
        let mut proposal = previous.proposal;
        proposal.title = input.title;
        proposal.body = input.body;
        let envelope = Envelope::new(&app.config, proposal)?;
        let content = serde_json::to_string(&envelope).map_err(storage)?;
        if content == app.gate.state().evidence.artifacts[request.artifact()].content {
            return Err(invalid(
                "Change the title or description before saving a revision",
            ));
        }
        let key = digest::content(content.as_bytes()).0;
        // Record a human edit linked to the previous received artifact. This
        // records the edit, without inventing a new native-agent generation.
        let artifact = app
            .gate
            .edit(&"owner".into(), request.artifact(), &content, &key)?;
        app.gate
            .revise_request(&"agent".into(), &id, input.revision, &artifact, now())?;
        view(app, &id)
    })
    .await
}
