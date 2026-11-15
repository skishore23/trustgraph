//! No agent, database, API keys or external effects required.
use std::collections::{BTreeMap, BTreeSet};
use trustgraph_framework::*;

fn readers(names: &[&str]) -> Readers {
    names.iter().map(|name| PrincipalId::from(*name)).collect()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::new();
    let broad = readers(&["writer", "model", "reviewer", "finance", "sales"]);
    let restricted = readers(&["writer", "model", "reviewer", "finance"]);
    let request = graph.register_source(
        "request",
        "text".into(),
        b"Prepare quote".to_vec(),
        broad.clone(),
    )?;
    let price = graph.register_source(
        "prices",
        "text".into(),
        b"Internal price sheet".to_vec(),
        restricted.clone(),
    )?;
    let unrelated = graph.register_source(
        "public-guide",
        "text".into(),
        b"Guide".to_vec(),
        broad.clone(),
    )?;
    let quote_op = graph.register_operation(OperationDefinition {
        name: "create_quote".into(),
        version: "1".into(),
        executor: "model".into(),
        inputs: BTreeMap::from([
            ("request".into(), "text".into()),
            ("prices".into(), "text".into()),
        ]),
        outputs: BTreeMap::from([("quote".into(), "text".into())]),
    })?;
    let ticket = graph.begin_execution(
        &quote_op,
        BTreeMap::from([
            ("request".into(), request),
            ("prices".into(), price.clone()),
        ]),
        "writer".into(),
        serde_json::json!({}),
    )?;
    // The application runs its function here, after the input access check.
    let outputs = graph.finish_execution(
        ticket,
        BTreeMap::from([(
            "quote".into(),
            OutputValue {
                logical_id: "quote".into(),
                bytes: b"Quote derived from internal prices".to_vec(),
            },
        )]),
        "local function completed",
    )?;
    let quote = outputs["quote"].clone();
    let email_op = graph.register_operation(OperationDefinition {
        name: "draft_email".into(),
        version: "1".into(),
        executor: "model".into(),
        inputs: BTreeMap::from([("quote".into(), "text".into())]),
        outputs: BTreeMap::from([("email".into(), "text".into())]),
    })?;
    let ticket = graph.begin_execution(
        &email_op,
        BTreeMap::from([("quote".into(), quote.clone())]),
        "writer".into(),
        serde_json::json!({}),
    )?;
    let outputs = graph.finish_execution(
        ticket,
        BTreeMap::from([(
            "email".into(),
            OutputValue {
                logical_id: "email".into(),
                bytes: b"Please review this quote".to_vec(),
            },
        )]),
        "local email formatter completed",
    )?;
    let email = outputs["email"].clone();
    graph.set_action_rule(
        "send_email",
        ActionRule {
            actors: readers(&["writer"]),
            approval: Some(ApprovalRequirement {
                reviewers: readers(&["reviewer"]),
                ttl_seconds: 600,
            }),
        },
    )?;
    let mut intent = ActionIntent {
        actor: "writer".into(),
        action: "send_email".into(),
        artifact: email.clone(),
        destination: "sales-mailbox".into(),
        recipients: readers(&["sales"]),
        parameters: serde_json::json!({}),
    };
    assert!(matches!(
        graph.check_action(&intent, None, 100),
        Decision::Deny { .. }
    ));
    println!("Sales send: denied by inherited price-sheet restriction");
    intent.destination = "finance-mailbox".into();
    intent.recipients = readers(&["finance"]);
    assert!(matches!(
        graph.check_action(&intent, None, 100),
        Decision::NeedsApproval { .. }
    ));
    let approval = graph.approve(&intent, "reviewer".into(), 100)?;
    assert!(matches!(
        graph.check_action(&intent, Some(&approval), 101),
        Decision::Allow { .. }
    ));
    println!("Finance send: allowed after exact-context approval (no email sent)");
    graph.update_source_permissions(&unrelated, BTreeSet::new())?;
    assert!(matches!(
        graph.check_action(&intent, Some(&approval), 102),
        Decision::Allow { .. }
    ));
    graph.update_source_permissions(&price, readers(&["writer", "model", "reviewer"]))?;
    assert!(matches!(
        graph.check_action(&intent, Some(&approval), 103),
        Decision::Deny { .. }
    ));
    let impact = graph.impact(&price)?;
    assert!(impact.contains(&quote) && impact.contains(&email) && !impact.contains(&unrelated));
    println!(
        "Price-sheet access changed: dependent send blocked; unrelated artifact excluded from impact"
    );
    Ok(())
}
