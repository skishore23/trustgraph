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

const EXAMPLES: [(&str, &str, &str); 6] = [
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
        other => reject(
            "unknown_op",
            format!(
                "Unknown op `{}`. Use examples, check, decide, diff or simulate.",
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
    async fn examples_lists_six_in_order_and_each_is_clean() {
        let (status, v) = post(json!({"op":"examples"})).await;
        assert_eq!(status, StatusCode::OK);
        let list = v["examples"].as_array().unwrap();
        let ids: Vec<_> = list.iter().map(|e| e["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            [
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
        assert!(list[0]["summary"].as_str().unwrap().contains("digest_bot"));
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
}
