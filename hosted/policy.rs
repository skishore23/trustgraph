//! `POST /api/policy`: the stateless policy playground. Every request carries
//! its own policy text; nothing is stored and nothing outside the request is
//! read. All work runs in a blocking task so a panic becomes a 503, not a hang.
use std::collections::{BTreeMap, BTreeSet};

use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use trustgraph_engine::{Engine, Event, Reply};
use trustgraph_policy::{
    Diagnostic, Policy, Query, STARTER, Severity, decide, diff, explain, lint, parse, run_tests,
};

/// Policy text cap for this endpoint (`policy_too_large`).
pub const MAX_POLICY_BYTES: usize = 65_536;
const MAX_SEED: u64 = 65_535;
const MAX_COUNT: u64 = 8;
const MAX_STEPS: u64 = 120;
const MAX_SOURCES: usize = 64;
const MAX_RECIPIENTS: usize = 256;
const MAX_NAME_BYTES: usize = 256;

const EXAMPLES: [(&str, &str, &str); 7] = [
    (
        "handoff",
        "Hand-off between agents",
        include_str!("../examples/policies/handoff.policy.yaml"),
    ),
    (
        "slack-digest",
        "Slack digest",
        include_str!("../examples/policies/slack-digest.policy.yaml"),
    ),
    (
        "customer-email",
        "Customer email",
        include_str!("../examples/policies/customer-email.policy.yaml"),
    ),
    (
        "pull-request-bot",
        "Pull-request bot",
        include_str!("../examples/policies/pull-request-bot.policy.yaml"),
    ),
    (
        "support-replies",
        "Support replies",
        include_str!("../examples/policies/support-replies.policy.yaml"),
    ),
    (
        "multi-agent-research",
        "Multi-agent research",
        include_str!("../examples/policies/multi-agent-research.policy.yaml"),
    ),
    ("starter", "Starter", STARTER),
];

/// A malformed request: HTTP 400 with a stable code.
struct Rejection {
    code: &'static str,
    message: String,
}

fn reject<T>(code: &'static str, message: impl Into<String>) -> Result<T, Rejection> {
    Err(Rejection {
        code,
        message: message.into(),
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpOnly {
    #[allow(dead_code)]
    op: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyReq {
    #[allow(dead_code)]
    op: String,
    policy: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryReq {
    output_from: Vec<String>,
    agent: String,
    action: String,
    #[serde(default)]
    recipients: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecideReq {
    #[allow(dead_code)]
    op: String,
    policy: String,
    query: QueryReq,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffReq {
    #[allow(dead_code)]
    op: String,
    old: String,
    new: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SimReq {
    #[allow(dead_code)]
    op: String,
    policy: String,
    seed: u64,
    #[serde(default = "default_count")]
    count: u64,
    #[serde(default = "default_steps")]
    steps: u64,
}

fn default_count() -> u64 {
    4
}
fn default_steps() -> u64 {
    80
}

fn parse_req<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, Rejection> {
    serde_json::from_value(value).or_else(|e| reject("bad_request", e.to_string()))
}

fn too_large(text: &str, field: &str) -> Result<(), Rejection> {
    if text.len() > MAX_POLICY_BYTES {
        return reject(
            "policy_too_large",
            format!(
                "`{field}` is {} bytes; the playground limit is {MAX_POLICY_BYTES}.",
                text.len()
            ),
        );
    }
    Ok(())
}

fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().any(|d| d.severity == Severity::Error)
}

/// Parse, then lint when parsing produced a policy.
fn load(text: &str) -> (Option<Policy>, Vec<Diagnostic>) {
    let parsed = parse(text);
    let mut diagnostics = parsed.diagnostics;
    if let Some(policy) = &parsed.policy {
        diagnostics.extend(lint(policy));
    }
    let policy = parsed.policy.filter(|_| !has_errors(&diagnostics));
    (policy, diagnostics)
}

fn summary(policy: &Policy) -> Value {
    let sources: BTreeMap<_, _> = policy
        .sources
        .iter()
        .map(|(name, s)| {
            (
                name,
                json!({
                    "readers": &s.readers,
                    "people": policy.people(&s.readers),
                    "agents": &s.agents,
                }),
            )
        })
        .collect();
    let actions: BTreeMap<_, _> = policy
        .actions
        .iter()
        .map(|(name, a)| {
            let approval = a.approval.as_ref().map(|r| {
                json!({
                    "reviewers": &r.reviewers,
                    "people": policy.people(&r.reviewers),
                    "valid_for_seconds": r.valid_for_seconds,
                })
            });
            (
                name,
                json!({
                    "agents": &a.agents,
                    "audience": &a.audience,
                    "people": policy.people(&a.audience),
                    "approval": approval,
                }),
            )
        })
        .collect();
    json!({
        "agents": policy.agents,
        "sources": sources,
        "actions": actions,
        "groups": policy.groups,
    })
}

fn examples() -> Value {
    let list: Vec<Value> = EXAMPLES
        .iter()
        .map(|(id, title, policy)| {
            let summary = policy
                .lines()
                .take_while(|l| l.starts_with('#'))
                .map(|l| l.trim_start_matches('#').trim())
                .filter(|l| !l.is_empty() && !l.ends_with(".yaml"))
                .collect::<Vec<_>>()
                .join(" ");
            json!({"id": id, "title": title, "summary": summary, "policy": policy})
        })
        .collect();
    json!({"examples": list})
}

fn op_check(req: PolicyReq) -> Value {
    let (policy, diagnostics) = load(&req.policy);
    let ok = !has_errors(&diagnostics);
    match policy {
        Some(policy) => json!({
            "ok": ok,
            "diagnostics": diagnostics,
            "tests": run_tests(&policy),
            "explain": explain(&policy),
            "summary": summary(&policy),
        }),
        None => json!({
            "ok": ok,
            "diagnostics": diagnostics,
            "tests": null,
            "explain": null,
            "summary": null,
        }),
    }
}

fn bounded(list: &[String], max: usize, what: &str) -> Result<BTreeSet<String>, Rejection> {
    if list.len() > max {
        return reject("limit", format!("`{what}` accepts at most {max} entries."));
    }
    if list.iter().any(|n| n.len() > MAX_NAME_BYTES) {
        return reject(
            "limit",
            format!("Names in `{what}` are limited to {MAX_NAME_BYTES} bytes."),
        );
    }
    Ok(list.iter().cloned().collect())
}

fn op_decide(req: DecideReq) -> Result<Value, Rejection> {
    let q = req.query;
    if q.agent.len() > MAX_NAME_BYTES || q.action.len() > MAX_NAME_BYTES {
        return reject(
            "limit",
            format!("Names are limited to {MAX_NAME_BYTES} bytes."),
        );
    }
    let query = Query {
        output_from: bounded(&q.output_from, MAX_SOURCES, "output_from")?,
        agent: q.agent,
        action: q.action,
        recipients: match &q.recipients {
            Some(r) => Some(bounded(r, MAX_RECIPIENTS, "recipients")?),
            None => None,
        },
    };
    let (policy, diagnostics) = load(&req.policy);
    Ok(match policy {
        Some(policy) => json!({
            "ok": true,
            "diagnostics": diagnostics,
            "outcome": decide(&policy, &query),
        }),
        None => json!({"ok": false, "diagnostics": diagnostics, "outcome": null}),
    })
}

fn op_diff(req: DiffReq) -> Value {
    let mut policies = Vec::new();
    for (file, text) in [("old", &req.old), ("new", &req.new)] {
        let (policy, diagnostics) = load(text);
        match policy {
            Some(p) => policies.push(p),
            None => {
                return json!({
                    "ok": false,
                    "diagnostics": diagnostics,
                    "file": file,
                    "widens": false,
                    "changes": [],
                });
            }
        }
    }
    let result = diff(&policies[0], &policies[1]);
    json!({
        "ok": true,
        "diagnostics": [],
        "file": null,
        "widens": result.widens(),
        "changes": result.changes,
    })
}

fn op_simulate(req: SimReq) -> Result<Value, Rejection> {
    if req.seed > MAX_SEED {
        return reject("limit", format!("`seed` must be 0 to {MAX_SEED}."));
    }
    if !(1..=MAX_COUNT).contains(&req.count) {
        return reject("limit", format!("`count` must be 1 to {MAX_COUNT}."));
    }
    if !(1..=MAX_STEPS).contains(&req.steps) {
        return reject("limit", format!("`steps` must be 1 to {MAX_STEPS}."));
    }
    Ok(
        match trustgraph_sim::policy_sim::campaign(
            &req.policy,
            req.seed,
            req.count,
            req.steps as u32,
        ) {
            Ok(c) => {
                let decision = |k: &str| c.decisions.get(k).copied().unwrap_or(0);
                let violations: Vec<Value> = c
                    .violations
                    .iter()
                    .map(|(seed, v)| {
                        json!({"seed": seed, "invariant": v.invariant, "step": v.step, "detail": v.detail})
                    })
                    .collect();
                json!({
                    "ok": c.violations.is_empty(),
                    "diagnostics": [],
                    "first_seed": c.first_seed,
                    "count": c.count,
                    "steps": c.steps,
                    "events": c.events,
                    "decisions": {
                        "allow": decision("allow"),
                        "deny": decision("deny"),
                        "needs_approval": decision("needs_approval"),
                    },
                    "violations": violations,
                    "coverage": c.coverage,
                    "digest": c.digest,
                })
            }
            Err(diagnostics) => json!({"ok": false, "diagnostics": diagnostics}),
        },
    )
}

// ---- `run`: several agent steps through a fresh engine, then one check ----

const MAX_RUN_STEPS: usize = 6;
const MAX_STEP_READS: usize = 8;
const MAX_RUN_NAME_BYTES: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunStep {
    agent: String,
    reads: Vec<String>,
    writes: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunCheck {
    step: usize,
    action: String,
    #[serde(default)]
    recipients: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunReq {
    #[allow(dead_code)]
    op: String,
    policy: String,
    steps: Vec<RunStep>,
    #[serde(default)]
    check: Option<RunCheck>,
}

/// What one step reads: a policy source, or the output of an earlier step.
enum Read {
    Source(String),
    Step(usize),
}

fn run_name(value: &str, what: &str) -> Result<(), Rejection> {
    if value.trim().is_empty() || value.len() > MAX_RUN_NAME_BYTES {
        return reject(
            "limit",
            format!("`{what}` must be 1 to {MAX_RUN_NAME_BYTES} bytes and not blank."),
        );
    }
    Ok(())
}

/// Check the request's shape before any engine work: bounds, names, and that
/// every step reads only sources or earlier steps.
fn plan(req: &RunReq) -> Result<Vec<Vec<Read>>, Rejection> {
    if req.steps.is_empty() || req.steps.len() > MAX_RUN_STEPS {
        return reject(
            "limit",
            format!("`steps` must have 1 to {MAX_RUN_STEPS} entries."),
        );
    }
    let mut plan = Vec::with_capacity(req.steps.len());
    for (index, step) in req.steps.iter().enumerate() {
        run_name(&step.agent, "agent")?;
        run_name(&step.writes, "writes")?;
        if step.reads.is_empty() || step.reads.len() > MAX_STEP_READS {
            return reject(
                "limit",
                format!("Each step reads 1 to {MAX_STEP_READS} inputs."),
            );
        }
        let mut reads = Vec::with_capacity(step.reads.len());
        for read in &step.reads {
            run_name(read, "reads")?;
            if let Some(source) = read.strip_prefix("source:") {
                reads.push(Read::Source(source.to_string()));
            } else if let Some(n) = read.strip_prefix("step:") {
                match n.parse::<usize>() {
                    Ok(n) if n < index => reads.push(Read::Step(n)),
                    _ => {
                        return reject(
                            "bad_request",
                            format!(
                                "Step {index} reads `{read}`; a step may only read earlier steps."
                            ),
                        );
                    }
                }
            } else {
                return reject(
                    "bad_request",
                    "Each read is `source:<name>` or `step:<earlier index>`.",
                );
            }
        }
        plan.push(reads);
    }
    if let Some(check) = &req.check {
        run_name(&check.action, "action")?;
        if check.step >= req.steps.len() {
            return reject("bad_request", "`check.step` must name one of the steps.");
        }
        if let Some(r) = &check.recipients {
            bounded(r, MAX_RECIPIENTS, "recipients")?;
            for name in r {
                run_name(name, "recipients")?;
            }
        }
    }
    Ok(plan)
}

fn synthetic_sha256(parts: &[&str]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The policy text without its `tests:` block, if cutting it leaves exactly
/// the same policy minus its tests. The engine refuses to activate a policy
/// whose tests fail; the playground still lets you try such a draft.
fn without_tests(text: &str, policy: &Policy) -> Option<String> {
    let mut out = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        let top_level = line
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace() && c != '-' && c != '#');
        if top_level {
            skipping = line.starts_with("tests:");
            if skipping {
                continue;
            }
        }
        if !skipping {
            out.push(line);
        }
    }
    let stripped = out.join("\n") + "\n";
    let expected = Policy {
        tests: Vec::new(),
        ..policy.clone()
    };
    (parse(&stripped).policy.as_ref() == Some(&expected)).then_some(stripped)
}

fn readers_of(policy: &Policy, lineage: &BTreeSet<String>) -> BTreeSet<String> {
    let mut sets = lineage.iter().filter_map(|name| {
        policy
            .sources
            .get(name)
            .map(|source| policy.people(&source.readers))
    });
    let first = sets.next().unwrap_or_default();
    sets.fold(first, |acc, s| acc.intersection(&s).cloned().collect())
}

fn rejected(code: &str, message: &str) -> Value {
    json!({"code": code, "message": message})
}

fn op_run(req: RunReq) -> Result<Value, Rejection> {
    let plan = plan(&req)?;
    let (policy, diagnostics) = load(&req.policy);
    let Some(policy) = policy else {
        return Ok(json!({"ok": false, "diagnostics": diagnostics, "steps": [], "decision": null}));
    };
    let text = without_tests(&req.policy, &policy).unwrap_or_else(|| req.policy.clone());
    let mut engine = Engine::new();
    if let Reply::Rejected { code, message } = engine.apply(0, &Event::Genesis { policy: text }) {
        return Ok(json!({
            "ok": false,
            "diagnostics": [{"severity": "error", "path": "", "code": code, "message": message, "suggestion": null}],
            "steps": [],
            "decision": null,
        }));
    }

    let mut done: Vec<(String, BTreeSet<String>)> = Vec::new();
    let mut steps = Vec::new();
    for (index, (step, reads)) in req.steps.iter().zip(&plan).enumerate() {
        let mut inputs = Vec::new();
        let mut lineage = BTreeSet::new();
        let mut refusal = None;
        for read in reads {
            match read {
                Read::Step(n) => {
                    inputs.push(done[*n].0.clone());
                    lineage.extend(done[*n].1.iter().cloned());
                }
                Read::Source(name) => {
                    let sha = synthetic_sha256(&["source", name]);
                    match engine.apply(
                        0,
                        &Event::RegisterSource {
                            schema: None,
                            source: name.clone(),
                            sha256: sha,
                        },
                    ) {
                        Reply::Artifact { id } => {
                            inputs.push(id);
                            lineage.insert(name.clone());
                        }
                        Reply::Rejected { code, message } => {
                            refusal = Some(rejected(&code, &message));
                            break;
                        }
                        _ => {
                            refusal = Some(rejected("engine_mismatch", "unexpected reply"));
                            break;
                        }
                    }
                }
            }
        }
        if refusal.is_none() {
            let sha = synthetic_sha256(&["step", &index.to_string(), &step.writes]);
            let event = Event::RecordOutput {
                contract: None,
                agent: step.agent.clone(),
                step: step.writes.clone(),
                inputs,
                sha256: sha,
            };
            match engine.apply(0, &event) {
                Reply::Artifact { id } => {
                    steps.push(json!({
                        "agent": step.agent,
                        "writes": step.writes,
                        "artifact": {
                            "id": id,
                            "sources": lineage,
                            "readers": readers_of(&policy, &lineage),
                        },
                        "rejected": null,
                    }));
                    done.push((id, lineage));
                    continue;
                }
                Reply::Rejected { code, message } => refusal = Some(rejected(&code, &message)),
                _ => refusal = Some(rejected("engine_mismatch", "unexpected reply")),
            }
        }
        steps.push(json!({
            "agent": step.agent,
            "writes": step.writes,
            "artifact": null,
            "rejected": refusal,
        }));
        return Ok(
            json!({"ok": true, "diagnostics": diagnostics, "steps": steps, "decision": null}),
        );
    }

    let decision = match &req.check {
        None => Value::Null,
        Some(check) => {
            let (artifact, lineage) = &done[check.step];
            let agent = req.steps[check.step].agent.clone();
            let recipients: Option<BTreeSet<String>> = check
                .recipients
                .as_ref()
                .map(|r| r.iter().cloned().collect());
            let event = Event::Check {
                agent: agent.clone(),
                artifact: artifact.clone(),
                action: check.action.clone(),
                recipients: recipients.clone(),
            };
            match engine.apply(0, &event) {
                Reply::Decision {
                    verdict,
                    code,
                    reason,
                    blocking,
                    ..
                } => {
                    // The engine's answer; readers come from the same `decide`
                    // the engine runs, on the same lineage.
                    let readers = decide(
                        &policy,
                        &Query {
                            output_from: lineage.clone(),
                            agent,
                            action: check.action.clone(),
                            recipients,
                        },
                    )
                    .readers;
                    json!({
                        "verdict": verdict,
                        "code": code,
                        "reason": reason,
                        "blocking": blocking,
                        "readers": readers,
                    })
                }
                Reply::Rejected { code, message } => json!({"rejected": rejected(&code, &message)}),
                _ => json!({"rejected": rejected("engine_mismatch", "unexpected reply")}),
            }
        }
    };
    Ok(json!({"ok": true, "diagnostics": diagnostics, "steps": steps, "decision": decision}))
}

fn dispatch(body: Value) -> Result<Value, Rejection> {
    let op = match body.get("op") {
        Some(Value::String(op)) => op.clone(),
        _ => return reject("bad_request", "Send an `op` string."),
    };
    match op.as_str() {
        "examples" => {
            parse_req::<OpOnly>(body)?;
            Ok(examples())
        }
        "check" => {
            let req: PolicyReq = parse_req(body)?;
            too_large(&req.policy, "policy")?;
            Ok(op_check(req))
        }
        "decide" => {
            let req: DecideReq = parse_req(body)?;
            too_large(&req.policy, "policy")?;
            op_decide(req)
        }
        "diff" => {
            let req: DiffReq = parse_req(body)?;
            too_large(&req.old, "old")?;
            too_large(&req.new, "new")?;
            Ok(op_diff(req))
        }
        "simulate" => {
            let req: SimReq = parse_req(body)?;
            too_large(&req.policy, "policy")?;
            op_simulate(req)
        }
        "run" => {
            let req: RunReq = parse_req(body)?;
            too_large(&req.policy, "policy")?;
            op_run(req)
        }
        other => reject(
            "unknown_op",
            format!(
                "Unknown op `{}`. Use examples, check, decide, diff, simulate or run.",
                other.chars().take(40).collect::<String>()
            ),
        ),
    }
}

fn respond(status: StatusCode, value: Value) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
}

pub async fn demo(Json(body): Json<Value>) -> Response {
    match tokio::task::spawn_blocking(move || dispatch(body)).await {
        Ok(Ok(value)) => respond(StatusCode::OK, value),
        Ok(Err(r)) => respond(
            StatusCode::BAD_REQUEST,
            json!({"error": r.message, "code": r.code}),
        ),
        Err(_) => respond(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": "The playground could not complete this request. Please retry.", "code": "unavailable"}),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    const SLACK: &str = include_str!("../examples/policies/slack-digest.policy.yaml");

    async fn raw(content_type: &str, body: String) -> (StatusCode, Value) {
        let response = crate::router()
            .oneshot(
                Request::post("/api/policy")
                    .header("content-type", content_type)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let bytes = to_bytes(response.into_body(), 4_000_000).await.unwrap();
        (status, serde_json::from_slice(&bytes).expect("JSON body"))
    }

    async fn post(body: Value) -> (StatusCode, Value) {
        raw("application/json", body.to_string()).await
    }

    #[tokio::test]
    async fn examples_lists_seven_in_order_and_each_is_clean() {
        let (status, v) = post(json!({"op":"examples"})).await;
        assert_eq!(status, StatusCode::OK);
        let list = v["examples"].as_array().unwrap();
        let ids: Vec<_> = list.iter().map(|e| e["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            [
                "handoff",
                "slack-digest",
                "customer-email",
                "pull-request-bot",
                "support-replies",
                "multi-agent-research",
                "starter"
            ]
        );
        for e in list {
            assert!(!e["title"].as_str().unwrap().is_empty());
            assert!(e["summary"].as_str().unwrap().len() > 20, "{e}");
            let (status, c) = post(json!({"op":"check","policy":e["policy"]})).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(c["ok"], true, "{}: {c}", e["id"]);
            assert_eq!(c["diagnostics"], json!([]), "{}", e["id"]);
            let tests = c["tests"].as_array().unwrap();
            assert!(tests.iter().all(|t| t["pass"] == true), "{}", e["id"]);
        }
        assert!(
            list[0]["summary"]
                .as_str()
                .unwrap()
                .contains("planning notes")
        );
        assert!(list[1]["summary"].as_str().unwrap().contains("digest_bot"));
    }

    #[tokio::test]
    async fn check_returns_tests_explain_and_summary() {
        let (status, v) = post(json!({"op":"check","policy":SLACK})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["ok"], true);
        assert_eq!(v["tests"].as_array().unwrap().len(), 5);
        assert!(v["explain"].as_str().unwrap().contains("handbook"));
        let s = &v["summary"];
        assert_eq!(s["agents"], json!(["digest_bot"]));
        assert_eq!(
            s["sources"]["comp_review"]["readers"],
            json!(["hr", "leadership"])
        );
        assert_eq!(
            s["sources"]["comp_review"]["people"],
            json!(["maya", "nina", "omar"])
        );
        let all_hands = &s["actions"]["post_all_hands"];
        assert_eq!(all_hands["audience"], json!(["everyone"]));
        assert_eq!(all_hands["approval"]["reviewers"], json!(["hr"]));
        assert_eq!(all_hands["approval"]["people"], json!(["nina"]));
        assert_eq!(all_hands["approval"]["valid_for_seconds"], 3600);
        assert!(s["actions"]["post_eng_channel"]["approval"].is_null());
        assert_eq!(s["groups"]["hr"], json!(["nina"]));
    }

    #[tokio::test]
    async fn misspelt_group_is_a_200_with_a_diagnostic() {
        let bad = SLACK.replace("readers: [hr, leadership]", "readers: [hr, leadrship]");
        let (status, v) = post(json!({"op":"check","policy":bad})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["ok"], false);
        let d = v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["severity"] == "error")
            .expect("an error diagnostic");
        assert!(d["path"].as_str().unwrap().contains("comp_review"));
        assert!(!d["code"].as_str().unwrap().is_empty());
        assert!(d["suggestion"].as_str().unwrap().contains("leadership"));
        assert!(v["summary"].is_null() && v["tests"].is_null() && v["explain"].is_null());
    }

    #[tokio::test]
    async fn decide_allows_and_denies_with_the_blocking_source() {
        let q = |from: Value, action: &str| {
            json!({"op":"decide","policy":SLACK,"query":{
                "output_from":from,"agent":"digest_bot","action":action,"recipients":null}})
        };
        let (status, v) = post(q(json!(["eng_updates"]), "post_eng_channel")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["outcome"]["verdict"], "allow");
        let (_, v) = post(q(json!(["comp_review", "eng_updates"]), "post_eng_channel")).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["outcome"]["verdict"], "deny");
        let blocking = v["outcome"]["blocking"].as_array().unwrap();
        assert!(blocking.iter().any(|b| b["source"] == "comp_review"));
        // Explicit recipients are accepted.
        let body = json!({"op":"decide","policy":SLACK,"query":{
            "output_from":["handbook"],"agent":"digest_bot","action":"post_all_hands",
            "recipients":["engineering"]}});
        assert_eq!(post(body).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn decide_on_an_invalid_policy_has_no_outcome() {
        let bad = SLACK.replace("readers: [hr, leadership]", "readers: [hr, leadrship]");
        let (status, v) = post(json!({"op":"decide","policy":bad,"query":{
            "output_from":["handbook"],"agent":"digest_bot","action":"post_eng_channel"}}))
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["ok"], false);
        assert!(v["outcome"].is_null());
        assert!(!v["diagnostics"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn diff_detects_a_widening_and_reports_invalid_files() {
        let wider = SLACK.replace("audience: [engineering]\n", "audience: [engineering, hr]\n");
        let (status, v) = post(json!({"op":"diff","old":SLACK,"new":wider})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["ok"], true);
        assert_eq!(v["widens"], true);
        assert!(v["file"].is_null());
        assert!(
            v["changes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["kind"] == "widens")
        );
        let (_, same) = post(json!({"op":"diff","old":SLACK,"new":SLACK})).await;
        assert_eq!(same["widens"], false);
        let (_, bad) = post(json!({"op":"diff","old":SLACK,"new":"version: 2"})).await;
        assert_eq!(bad["ok"], false);
        assert_eq!(bad["file"], "new");
    }

    #[tokio::test]
    async fn simulate_has_no_violations_and_real_coverage() {
        let (status, v) =
            post(json!({"op":"simulate","policy":SLACK,"seed":3,"count":2,"steps":40})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["ok"], true);
        assert_eq!(v["violations"], json!([]));
        assert_eq!(
            (v["first_seed"].as_u64(), v["count"].as_u64()),
            (Some(3), Some(2))
        );
        assert_eq!(v["steps"], 40);
        assert!(v["events"].as_u64().unwrap() > 0);
        let covered: u64 = v["coverage"]
            .as_object()
            .unwrap()
            .values()
            .map(|n| n.as_u64().unwrap())
            .sum();
        assert!(covered > 0);
        assert!(!v["digest"].as_str().unwrap().is_empty());
        // Defaults apply.
        let (_, d) = post(json!({"op":"simulate","policy":SLACK,"seed":0})).await;
        assert_eq!(
            (d["count"].as_u64(), d["steps"].as_u64()),
            (Some(4), Some(80))
        );
        // A broken policy is a 200 with diagnostics.
        let (status, e) = post(json!({"op":"simulate","policy":"nope: 1","seed":0})).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(e["ok"], false);
        assert!(!e["diagnostics"].as_array().unwrap().is_empty());
    }

    async fn rejected(body: Value, code: &str) {
        let (status, v) = post(body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(v["code"], code, "{v}");
        assert!(!v["error"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn limits_are_rejected_not_clamped() {
        let big = "#".repeat(MAX_POLICY_BYTES + 1);
        rejected(json!({"op":"check","policy":big}), "policy_too_large").await;
        rejected(
            json!({"op":"diff","old":SLACK,"new":big}),
            "policy_too_large",
        )
        .await;
        let ok = "#".repeat(MAX_POLICY_BYTES);
        assert_eq!(
            post(json!({"op":"check","policy":ok})).await.0,
            StatusCode::OK
        );
        for (extra, _) in [
            (json!({"count":9}), ()),
            (json!({"count":0}), ()),
            (json!({"steps":121}), ()),
            (json!({"steps":0}), ()),
            (json!({"seed":65536}), ()),
        ] {
            let mut body = json!({"op":"simulate","policy":SLACK,"seed":0});
            body.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            rejected(body, "limit").await;
        }
        let q = |from: Vec<String>, rec: Option<Vec<String>>| {
            json!({"op":"decide","policy":SLACK,"query":{
                "output_from":from,"agent":"a","action":"b","recipients":rec}})
        };
        rejected(q(vec!["s".into(); 65], None), "limit").await;
        rejected(q(vec![], Some(vec!["r".into(); 257])), "limit").await;
        rejected(q(vec!["x".repeat(257)], None), "limit").await;
        assert_eq!(
            post(q(vec!["s".into(); 64], Some(vec!["r".into(); 256])))
                .await
                .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn malformed_requests_are_json_errors() {
        rejected(json!({"op":"explode"}), "unknown_op").await;
        rejected(json!({"policy":"x"}), "bad_request").await;
        rejected(json!({"op":"check"}), "bad_request").await;
        rejected(json!({"op":"check","policy":"x","extra":1}), "bad_request").await;
        rejected(json!({"op":"examples","extra":1}), "bad_request").await;
        let (s, v) = raw("application/json", "{not json".into()).await;
        assert!(s.is_client_error() && v["error"].is_string());
        let (s, v) = raw("text/plain", "{}".into()).await;
        assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(v["error"].is_string());
        let pad = format!(
            r#"{{"op":"check","policy":"{}"}}"#,
            "x".repeat(crate::MAX_BODY_BYTES)
        );
        let (s, v) = raw("application/json", pad).await;
        assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(v["error"].is_string());
    }

    const HANDOFF: &str = include_str!("../examples/policies/handoff.policy.yaml");

    fn laundering(policy: &str, action: &str) -> Value {
        json!({"op":"run","policy":policy,"steps":[
            {"agent":"researcher","reads":["source:salaries"],"writes":"planning notes"},
            {"agent":"planner","reads":["step:0","source:roadmap"],"writes":"summary"}
        ],"check":{"step":1,"action":action,"recipients":null}})
    }

    #[tokio::test]
    async fn run_follows_salaries_through_notes_written_by_another_agent() {
        let (status, v) = post(laundering(HANDOFF, "post_to_engineering")).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["ok"], true);
        let steps = v["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["artifact"]["sources"], json!(["salaries"]));
        assert_eq!(steps[0]["artifact"]["readers"], json!(["nina"]));
        assert_eq!(
            steps[1]["artifact"]["sources"],
            json!(["roadmap", "salaries"])
        );
        assert_eq!(steps[1]["artifact"]["readers"], json!(["nina"]));
        assert!(steps.iter().all(|s| s["rejected"].is_null()));
        let d = &v["decision"];
        assert_eq!(d["verdict"], "deny", "{d}");
        assert_eq!(d["code"], "audience_outside_readers");
        assert_eq!(d["blocking"][0]["source"], "salaries");
        assert_eq!(d["blocking"][0]["excluded"], json!(["ravi", "sam"]));
        assert_eq!(d["readers"], json!(["nina"]));

        // Same summary, right audience.
        let (_, hr) = post(laundering(HANDOFF, "brief_hr")).await;
        assert_eq!(hr["decision"]["verdict"], "allow", "{hr}");
    }

    #[tokio::test]
    async fn run_refuses_a_step_whose_agent_is_not_cleared_for_what_it_reads() {
        let uncleared = HANDOFF.replace(
            "    readers: [hr]\n    agents: [researcher, planner]",
            "    readers: [hr]\n    agents: [researcher]",
        );
        assert_ne!(uncleared, HANDOFF);
        let (status, v) = post(laundering(&uncleared, "post_to_engineering")).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        let steps = v["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2, "the run stops at the refused step");
        assert!(steps[0]["rejected"].is_null());
        assert!(steps[1]["artifact"].is_null());
        assert_eq!(steps[1]["rejected"]["code"], "agent_cannot_use_source");
        assert!(
            steps[1]["rejected"]["message"]
                .as_str()
                .unwrap()
                .contains("salaries")
        );
        assert!(v["decision"].is_null());
    }

    #[tokio::test]
    async fn run_allows_the_roadmap_alone_and_tries_drafts_with_failing_tests() {
        let body = json!({"op":"run","policy":HANDOFF,"steps":[
            {"agent":"planner","reads":["source:roadmap"],"writes":"summary"}
        ],"check":{"step":0,"action":"post_to_engineering","recipients":["sam"]}});
        let (status, v) = post(body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["decision"]["verdict"], "allow", "{v}");

        // Widening salaries breaks a test; the run still answers for the draft.
        let wider = HANDOFF.replace("    readers: [hr]\n", "    readers: [hr, engineering]\n");
        let (_, w) = post(laundering(&wider, "post_to_engineering")).await;
        assert_eq!(w["ok"], true, "{w}");
        assert_eq!(w["decision"]["verdict"], "allow", "{w}");

        let (_, approval) = post(json!({"op":"run","policy":HANDOFF,"steps":[
            {"agent":"planner","reads":["source:roadmap","source:benchmarks"],"writes":"brief"}
        ],"check":{"step":0,"action":"brief_leadership"}}))
        .await;
        assert_eq!(
            approval["decision"]["verdict"], "needs_approval",
            "{approval}"
        );

        let (_, bad) = post(laundering("version: 2", "post_to_engineering")).await;
        assert_eq!(bad["ok"], false);
        assert!(bad["decision"].is_null());
    }

    #[tokio::test]
    async fn run_limits_are_rejected() {
        let step = json!({"agent":"planner","reads":["source:roadmap"],"writes":"x"});
        let seven = vec![step.clone(); 7];
        rejected(json!({"op":"run","policy":HANDOFF,"steps":seven}), "limit").await;
        rejected(json!({"op":"run","policy":HANDOFF,"steps":[]}), "limit").await;
        let nine: Vec<_> = (0..9).map(|_| "source:roadmap").collect();
        rejected(
            json!({"op":"run","policy":HANDOFF,"steps":[{"agent":"planner","reads":nine,"writes":"x"}]}),
            "limit",
        )
        .await;
        let long = "a".repeat(65);
        rejected(
            json!({"op":"run","policy":HANDOFF,"steps":[{"agent":long,"reads":["source:roadmap"],"writes":"x"}]}),
            "limit",
        )
        .await;
        // A step may only read earlier steps.
        for read in ["step:0", "step:1", "step:x", "roadmap"] {
            rejected(
                json!({"op":"run","policy":HANDOFF,"steps":[{"agent":"planner","reads":[read],"writes":"x"}]}),
                "bad_request",
            )
            .await;
        }
        rejected(
            json!({"op":"run","policy":HANDOFF,"steps":[step],"check":{"step":1,"action":"brief_hr"}}),
            "bad_request",
        )
        .await;
    }
}
