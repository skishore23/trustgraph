//! Bounded, fictional demonstrations evaluated by the actual generic Rust SDK.
use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use trustgraph_framework::*;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    Documents,
    Export,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    Finance,
    Sales,
    External,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mutation {
    None,
    SourcePermissions,
    UnrelatedPermissions,
    ArtifactRevision,
    SchemaMismatch,
    ExecutorAccess,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoRequest {
    scenario: Scenario,
    audience: Audience,
    approve: bool,
    mutation: Mutation,
}
fn readers(names: &[&str]) -> Readers {
    names.iter().map(|n| PrincipalId::from(*n)).collect()
}
fn status(decision: &Decision) -> &'static str {
    match decision {
        Decision::Allow { .. } => "allow",
        Decision::NeedsApproval { .. } => "needs_approval",
        Decision::Deny { .. } => "deny",
    }
}
fn reason(decision: &Decision) -> String {
    match decision {
    Decision::Allow{..} => "Current source permissions and action policy permit this exact intent.".into(),
    Decision::NeedsApproval{..} => "Source access is permitted. This action separately requires review of the exact artifact and destination.".into(),
    Decision::Deny{reason} => reason.clone(),
}
}
fn snapshot(graph: &Graph) -> (Value, Value, Value) {
    let mut nodes = vec![];
    let mut edges = vec![];
    for artifact in graph.artifacts().values() {
        let allowed: Vec<_> = graph
            .readers(&artifact.id)
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.0)
            .collect();
        nodes.push(json!({"id":artifact.id.0,"label":artifact.logical_id,"kind":match artifact.origin { Origin::Source=>"source",_=>"artifact" },"schema":artifact.schema.0,"readers":allowed,"hash":artifact.content_hash,"preview":String::from_utf8_lossy(&artifact.bytes)}));
    }
    for execution in graph.executions().values() {
        let operation = &graph.operations()[&execution.operation];
        nodes.push(json!({"id":execution.id.0,"label":format!("{}@{}",operation.name,operation.version),"kind":"operation","schema":"typed operation","readers":[],"inputs":operation.inputs,"outputs":operation.outputs}));
        for (port, artifact) in &execution.inputs {
            edges.push(json!({"from":artifact.0,"to":execution.id.0,"label":port}));
        }
        for (port, artifact) in &execution.outputs {
            edges.push(json!({"from":execution.id.0,"to":artifact.0,"label":port}));
        }
    }
    (
        json!(nodes),
        json!(edges),
        json!({"artifacts":graph.artifacts(),"executions":graph.executions(),"operations":graph.operations()}),
    )
}
fn invalid(graph: &Graph, error: Error, steps: Vec<String>) -> Value {
    let (nodes, edges, records) = snapshot(graph);
    json!({"status":"invalid","reason":error.to_string(),"phase":"before_execution","nodes":nodes,"edges":edges,"records":records,"steps":steps,"affected":[],"external_writes":0,"before_status":"not_run"})
}
fn run(
    graph: &mut Graph,
    op: &OperationId,
    inputs: Inputs,
    outputs: &[(&str, &str, &str)],
) -> Result<BTreeMap<String, ArtifactId>, Error> {
    let ticket = graph.begin_execution(op, inputs, "operator".into(), json!({}))?;
    graph.finish_execution(
        ticket,
        outputs
            .iter()
            .map(|(port, id, text)| {
                (
                    port.to_string(),
                    OutputValue {
                        logical_id: id.to_string(),
                        bytes: text.as_bytes().to_vec(),
                    },
                )
            })
            .collect(),
        "fictional deterministic demo operation completed",
    )
}
fn evaluate(request: DemoRequest) -> Result<Value, Error> {
    let mut graph = Graph::new();
    let mut steps =
        vec!["Register exact source bytes, nominal schema identities and reader sets.".into()];
    let broad = readers(&["operator", "worker", "reviewer", "finance", "sales"]);
    let narrow = if matches!(request.mutation, Mutation::ExecutorAccess) {
        readers(&["operator", "reviewer", "finance"])
    } else {
        readers(&["operator", "worker", "reviewer", "finance"])
    };
    let export = matches!(request.scenario, Scenario::Export);
    let required_schema = if export { "csv" } else { "text" };
    let actual_schema = if matches!(request.mutation, Mutation::SchemaMismatch) {
        "image"
    } else {
        required_schema
    };
    let primary = graph.register_source(
        if export {
            "Revenue rows"
        } else {
            "Internal price sheet"
        },
        actual_schema.into(),
        if export {
            b"amount\n100\n200".to_vec()
        } else {
            b"Internal pricing: $300".to_vec()
        },
        narrow,
    )?;
    let shared = graph.register_source(
        if export {
            "Export specification"
        } else {
            "Customer request"
        },
        "text".into(),
        b"Prepare internal report".to_vec(),
        broad.clone(),
    )?;
    let unrelated = graph.register_source(
        "Unrelated handbook",
        "text".into(),
        b"Office opening hours".to_vec(),
        broad,
    )?;
    let op = graph.register_operation(OperationDefinition {
        name: if export {
            "aggregate_revenue"
        } else {
            "create_quote"
        }
        .into(),
        version: "1".into(),
        executor: "worker".into(),
        inputs: BTreeMap::from([
            ("data".into(), required_schema.into()),
            ("request".into(), "text".into()),
        ]),
        outputs: if export {
            BTreeMap::from([
                ("total".into(), "integer".into()),
                ("audit".into(), "text".into()),
            ])
        } else {
            BTreeMap::from([("quote".into(), "text".into())])
        },
    })?;
    steps.push(
        "Before execution, check exact input ports, schema IDs, actor access and worker access."
            .into(),
    );
    let result = run(
        &mut graph,
        &op,
        BTreeMap::from([
            ("data".into(), primary.clone()),
            ("request".into(), shared.clone()),
        ]),
        if export {
            &[
                ("total", "Revenue total", "300"),
                ("audit", "Audit note", "Two rows processed"),
            ]
        } else {
            &[("quote", "Quote", "Internal quote: $300")]
        },
    );
    let first = match result {
        Ok(v) => v,
        Err(e) => return Ok(invalid(&graph, e, steps)),
    };
    steps.push(
        "Record exact output artifact(s); every output inherits every input's restrictions.".into(),
    );
    let mut final_artifact = if export {
        first["total"].clone()
    } else {
        first["quote"].clone()
    };
    let mut final_op = None;
    if !export {
        let op = graph.register_operation(OperationDefinition {
            name: "draft_email".into(),
            version: "1".into(),
            executor: "worker".into(),
            inputs: BTreeMap::from([("quote".into(), "text".into())]),
            outputs: BTreeMap::from([("email".into(), "text".into())]),
        })?;
        final_artifact = run(
            &mut graph,
            &op,
            BTreeMap::from([("quote".into(), final_artifact)]),
            &[(
                "email",
                "Email draft",
                "Please review our internal quote: $300",
            )],
        )?["email"]
            .clone();
        final_op = Some(op);
        steps.push("The quote's artifact ID becomes the next step's input. That shared reference connects the graph.".into());
    }
    let action = if export { "export" } else { "send_email" };
    graph.set_action_rule(
        action,
        ActionRule {
            actors: readers(&["operator"]),
            approval: if export {
                None
            } else {
                Some(ApprovalRequirement {
                    reviewers: readers(&["reviewer"]),
                    ttl_seconds: 600,
                })
            },
        },
    )?;
    let recipient = match request.audience {
        Audience::Finance => "finance",
        Audience::Sales => "sales",
        Audience::External => "external",
    };
    let mut intent = ActionIntent {
        actor: "operator".into(),
        action: action.into(),
        artifact: final_artifact,
        destination: format!("{recipient}-destination"),
        recipients: readers(&[recipient]),
        parameters: json!({}),
    };

    let approval = if request.approve && !export {
        match graph.approve(&intent, "reviewer".into(), 100) {
            Ok(a) => {
                steps.push(
                    "Reviewer approved the exact artifact, destination and dependency revisions."
                        .into(),
                );
                Some(a)
            }
            Err(e) => {
                steps.push(format!("Approval could not bypass access: {e}"));
                None
            }
        }
    } else {
        None
    };
    let before = graph.check_action(&intent, approval.as_ref(), 100);
    let mut affected = vec![];
    match request.mutation {
        Mutation::SourcePermissions => {
            graph.update_source_permissions(
                &primary,
                readers(&["operator", "worker", "reviewer", "finance", "auditor"]),
            )?;
            affected = graph.impact(&primary)?.into_iter().map(|id| id.0).collect();
            steps.push("Changed the relevant source ACL revision (Finance still permitted). Old approval is stale, not an access grant.".into());
        }
        Mutation::UnrelatedPermissions => {
            graph.update_source_permissions(&unrelated, readers(&["operator"]))?;
            affected = graph
                .impact(&unrelated)?
                .into_iter()
                .map(|id| id.0)
                .collect();
            steps.push("Changed only the unrelated handbook. This artifact's decision context is unchanged.".into());
        }
        Mutation::ArtifactRevision => {
            let newer = if let Some(op) = final_op {
                run(
                    &mut graph,
                    &op,
                    BTreeMap::from([("quote".into(), first["quote"].clone())]),
                    &[("email", "Email draft v2", "Revised internal quote email")],
                )?["email"]
                    .clone()
            } else {
                run(
                    &mut graph,
                    &op,
                    BTreeMap::from([("data".into(), primary), ("request".into(), shared)]),
                    &[
                        ("total", "Revenue total v2", "300"),
                        ("audit", "Audit note v2", "Second computation"),
                    ],
                )?["total"]
                    .clone()
            };
            intent.artifact = newer;
            steps.push("Created a new output revision. An old approval cannot transfer to its new identity.".into());
        }
        _ => {}
    }
    let decision = graph.check_action(&intent, approval.as_ref(), 101);
    steps.push("Recheck the exact intent against current lineage before any external effect. This demo performs no effect.".into());
    let (nodes, edges, records) = snapshot(&graph);
    Ok(
        json!({"scenario":if export{"export"}else{"documents"},"status":status(&decision),"reason":reason(&decision),"phase":"action_check","nodes":nodes,"edges":edges,"records":records,"steps":steps,"affected":affected,"before_status":status(&before),"intent":intent,"decision":decision,"external_writes":0}),
    )
}
pub async fn demo(Json(request): Json<DemoRequest>) -> Response {
    let (status, value) = match evaluate(request) {
        Ok(v) => (StatusCode::OK, v),
        Err(e) => (StatusCode::BAD_REQUEST, json!({"error":e.to_string()})),
    };
    (status, [(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(scenario: &str, audience: &str, approve: bool, mutation: &str) -> Value {
        evaluate(serde_json::from_value(json!({"scenario":scenario,"audience":audience,"approve":approve,"mutation":mutation})).unwrap()).unwrap()
    }
    #[test]
    fn document_checks_restrictions_approval_and_selective_changes() {
        assert_eq!(
            check("documents", "finance", false, "none")["status"],
            "needs_approval"
        );
        assert_eq!(
            check("documents", "finance", true, "none")["status"],
            "allow"
        );
        assert_eq!(check("documents", "sales", true, "none")["status"], "deny");
        assert_eq!(
            check("documents", "finance", true, "source_permissions")["status"],
            "deny"
        );
        assert_eq!(
            check("documents", "finance", true, "unrelated_permissions")["status"],
            "allow"
        );
        assert_eq!(
            check("documents", "finance", true, "artifact_revision")["status"],
            "deny"
        );
        for mutation in ["schema_mismatch", "executor_access"] {
            let result = check("documents", "finance", true, mutation);
            assert_eq!(result["status"], "invalid");
            assert_eq!(
                result["records"]["executions"].as_object().unwrap().len(),
                0
            );
        }
    }
    #[test]
    fn export_has_two_outputs_without_mandatory_approval() {
        let result = check("export", "finance", false, "none");
        assert_eq!(result["status"], "allow");
        assert_eq!(
            result["records"]["executions"]
                .as_object()
                .unwrap()
                .values()
                .next()
                .unwrap()["outputs"]
                .as_object()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(check("export", "external", false, "none")["status"], "deny");
        assert_eq!(
            check("export", "finance", false, "source_permissions")["status"],
            "allow"
        );
    }
    #[test]
    fn replay_is_deterministic_and_has_no_external_writes() {
        for scenario in ["documents", "export"] {
            for mutation in [
                "none",
                "source_permissions",
                "unrelated_permissions",
                "artifact_revision",
                "schema_mismatch",
                "executor_access",
            ] {
                let a = check(scenario, "finance", true, mutation);
                let b = check(scenario, "finance", true, mutation);
                assert_eq!(a, b);
                assert_eq!(a["external_writes"], 0);
            }
        }
    }
    #[tokio::test]
    async fn framework_endpoint_is_wired_and_rejects_unknown_payloads() {
        use axum::{body::Body, http::Request};
        use tower::ServiceExt;
        let response=crate::router().oneshot(Request::post("/api/framework").header("content-type","application/json").body(Body::from(json!({"scenario":"documents","audience":"finance","approve":true,"mutation":"none"}).to_string())).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let response = crate::router()
            .oneshot(
                Request::post("/api/framework")
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_client_error());
    }
}
