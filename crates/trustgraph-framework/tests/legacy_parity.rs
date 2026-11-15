//! Both API generations must implement the same conservative reader algebra.
use std::collections::{BTreeMap, BTreeSet};
use trustgraph_core as legacy;
use trustgraph_framework as sdk;

fn names(mask: u8) -> Vec<&'static str> {
    let mut result = vec!["host"];
    for (bit, name) in ["alice", "bob", "reviewer"].into_iter().enumerate() {
        if mask & (1 << bit) != 0 {
            result.push(name);
        }
    }
    result
}
fn legacy_source(e: &mut legacy::Evidence, name: &str, readers: &[&str]) {
    let source = legacy::Source {
        id: name.into(),
        workspace: e.workspace.clone(),
        artifact: name.into(),
        acl: name.into(),
    };
    e.acls.insert(
        source.acl.clone(),
        legacy::Acl {
            id: source.acl.clone(),
            workspace: e.workspace.clone(),
            revision: 1,
            readers: readers
                .iter()
                .map(|r| legacy::PrincipalId::from(*r))
                .collect(),
        },
    );
    e.artifacts.insert(
        name.into(),
        legacy::Artifact::new(
            name.into(),
            e.workspace.clone(),
            name.into(),
            legacy::Origin::Source(name.into()),
        ),
    );
    e.sources.insert(source.id.clone(), source);
}
fn legacy_derive(e: &mut legacy::Evidence, name: &str, inputs: &[&str]) {
    let manifest = legacy::Manifest {
        complete: true,
        provider: "host".into(),
        template_revision: "1".into(),
        inputs: inputs
            .iter()
            .enumerate()
            .map(|(order, input)| legacy::InputUse {
                occurrence: format!("{name}-{order}"),
                artifact: (*input).into(),
                role: legacy::InputRole::Document,
                order,
            })
            .collect(),
    };
    let transformation = legacy::Transformation {
        id: name.into(),
        workspace: e.workspace.clone(),
        output: name.into(),
        manifest_hash: legacy::digest::structured("context-manifest", &manifest),
        manifest,
    };
    e.transformations
        .insert(transformation.id.clone(), transformation);
    e.artifacts.insert(
        name.into(),
        legacy::Artifact::new(
            name.into(),
            e.workspace.clone(),
            name.into(),
            legacy::Origin::Transformation(name.into()),
        ),
    );
}
fn legacy_fixture(left: &[&str], right: &[&str]) -> legacy::Evidence {
    let workspace = legacy::WorkspaceId::from("parity");
    let mut e = legacy::Evidence {
        workspace: workspace.clone(),
        complete: true,
        principals: BTreeMap::new(),
        artifacts: BTreeMap::new(),
        sources: BTreeMap::new(),
        acls: BTreeMap::new(),
        transformations: BTreeMap::new(),
        audiences: BTreeMap::new(),
        approvals: BTreeMap::new(),
        revoked: BTreeSet::new(),
        policy: legacy::Policy {
            workspace: workspace.clone(),
            revision: 1,
            approvers: BTreeSet::new(),
            max_approval_lifetime: 10,
            configuration: None,
        },
    };
    for name in ["host", "alice", "bob", "reviewer"] {
        e.principals.insert(
            name.into(),
            legacy::Principal {
                id: name.into(),
                workspace: workspace.clone(),
                role: legacy::Role::Human,
                enabled: true,
            },
        );
    }
    legacy_source(&mut e, "left", left);
    legacy_source(&mut e, "right", right);
    legacy_derive(&mut e, "merged", &["left", "right"]);
    legacy_derive(&mut e, "final", &["merged", "left"]);
    e
}
fn sdk_derive(
    g: &mut sdk::Graph,
    left: &sdk::ArtifactId,
    right: &sdk::ArtifactId,
    name: &str,
) -> sdk::ArtifactId {
    let operation = g
        .register_operation(sdk::OperationDefinition {
            name: "merge".into(),
            version: "1".into(),
            executor: "host".into(),
            inputs: BTreeMap::from([
                ("left".into(), "text".into()),
                ("right".into(), "text".into()),
            ]),
            outputs: BTreeMap::from([("output".into(), "text".into())]),
        })
        .unwrap();
    let ticket = g
        .begin_execution(
            &operation,
            BTreeMap::from([
                ("left".into(), left.clone()),
                ("right".into(), right.clone()),
            ]),
            "host".into(),
            serde_json::Value::Null,
        )
        .unwrap();
    g.finish_execution(
        ticket,
        BTreeMap::from([(
            "output".into(),
            sdk::OutputValue {
                logical_id: name.into(),
                bytes: name.as_bytes().to_vec(),
            },
        )]),
        "host observed",
    )
    .unwrap()["output"]
        .clone()
}

#[test]
fn legacy_and_framework_agree_on_all_reader_pairs_and_transitive_updates() {
    for left in 0..8 {
        for right in 0..8 {
            let mut legacy = legacy_fixture(&names(left), &names(right));
            let mut graph = sdk::Graph::new();
            let l = graph
                .register_source(
                    "left",
                    "text".into(),
                    b"left".to_vec(),
                    names(left)
                        .into_iter()
                        .map(sdk::PrincipalId::from)
                        .collect(),
                )
                .unwrap();
            let r = graph
                .register_source(
                    "right",
                    "text".into(),
                    b"right".to_vec(),
                    names(right)
                        .into_iter()
                        .map(sdk::PrincipalId::from)
                        .collect(),
                )
                .unwrap();
            let merged = sdk_derive(&mut graph, &l, &r, "merged");
            let final_artifact = sdk_derive(&mut graph, &merged, &l, "final");
            let expected: BTreeSet<_> = legacy::lineage::derived_readers(&legacy, &"final".into())
                .unwrap()
                .into_iter()
                .map(|p| p.0)
                .collect();
            let actual: BTreeSet<_> = graph
                .readers(&final_artifact)
                .unwrap()
                .into_iter()
                .map(|p| p.0)
                .collect();
            assert_eq!(actual, expected);
            legacy.acls.get_mut(&"right".into()).unwrap().readers = BTreeSet::from(["host".into()]);
            legacy.acls.get_mut(&"right".into()).unwrap().revision += 1;
            graph
                .update_source_permissions(&r, BTreeSet::from(["host".into()]))
                .unwrap();
            let expected: BTreeSet<_> = legacy::lineage::derived_readers(&legacy, &"final".into())
                .unwrap()
                .into_iter()
                .map(|p| p.0)
                .collect();
            let actual: BTreeSet<_> = graph
                .readers(&final_artifact)
                .unwrap()
                .into_iter()
                .map(|p| p.0)
                .collect();
            assert_eq!(actual, expected);
        }
    }
}
