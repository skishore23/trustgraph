//! Ordinary deterministic processing, without an agent or a network service.
use std::collections::BTreeMap;
use trustgraph_framework::*;
fn readers(names: &[&str]) -> Readers {
    names.iter().map(|v| PrincipalId::from(*v)).collect()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut graph = Graph::new();
    let data = graph.register_source(
        "revenue",
        "csv".into(),
        b"amount\n100\n200\n".to_vec(),
        readers(&["analyst", "local-process", "finance"]),
    )?;
    let op = graph.register_operation(OperationDefinition {
        name: "sum_revenue".into(),
        version: "1".into(),
        executor: "local-process".into(),
        inputs: BTreeMap::from([("rows".into(), "csv".into())]),
        outputs: BTreeMap::from([
            ("total".into(), "integer".into()),
            ("audit".into(), "text".into()),
        ]),
    })?;
    let ticket = graph.begin_execution(
        &op,
        BTreeMap::from([("rows".into(), data.clone())]),
        "analyst".into(),
        serde_json::json!({}),
    )?;
    let total: u64 = std::str::from_utf8(&graph.artifacts()[&data].bytes)?
        .lines()
        .skip(1)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .sum();
    let outputs = graph.finish_execution(
        ticket,
        BTreeMap::from([
            (
                "total".into(),
                OutputValue {
                    logical_id: "revenue-total".into(),
                    bytes: total.to_string().into_bytes(),
                },
            ),
            (
                "audit".into(),
                OutputValue {
                    logical_id: "revenue-audit".into(),
                    bytes: b"Processed two records".to_vec(),
                },
            ),
        ]),
        "local sum function returned",
    )?;
    graph.set_action_rule(
        "export",
        ActionRule {
            actors: readers(&["analyst"]),
            approval: None,
        },
    )?;
    for (port, artifact) in outputs {
        let mut intent = ActionIntent {
            actor: "analyst".into(),
            action: "export".into(),
            artifact,
            destination: "finance-bucket".into(),
            recipients: readers(&["finance"]),
            parameters: serde_json::json!({}),
        };
        assert!(matches!(
            graph.check_action(&intent, None, 0),
            Decision::Allow { .. }
        ));
        intent.destination = "public-bucket".into();
        intent.recipients = readers(&["external"]);
        assert!(matches!(
            graph.check_action(&intent, None, 0),
            Decision::Deny { .. }
        ));
        println!("{port}: Finance export allowed; external export denied (no files uploaded)");
    }
    Ok(())
}
