//! Opt-in qualification against an actual Hydra server, never an HTTP fixture.
use trustgraph_core::*;
use trustgraph_runtime::{
    hydra::HydraClient, journal::MemoryJournal, projection, publisher::LocalPublisher, *,
};

fn publication_bundle() -> trustgraph_core::bundle::Bundle {
    let mut w = Coordinator::open(MemoryJournal::default(), LocalPublisher::default()).unwrap();
    let artifact = w.generate(&"maya".into(), false, "hydra-contract").unwrap();
    let intent = Intent {
        artifact,
        audience: "company".into(),
        requester: "maya".into(),
    };
    let context = verify(&w.state().evidence, &intent, 10)
        .context()
        .unwrap()
        .digest();
    w.approve(&"maya".into(), &intent, &context, "hydra-contract", 10)
        .unwrap();
    let PublishResult::Operation { operation } =
        w.publish(&intent, &context, "hydra-contract", 11).unwrap()
    else {
        panic!("publication blocked");
    };
    operation.bundle
}
#[test]
fn connected_projection_has_distinct_incidence_records_and_no_unrelated_private_data() {
    let bundle = publication_bundle();
    let graph = projection::build(&bundle, 1_000_000).unwrap();
    assert_eq!(
        graph.nodes.iter().filter(|n| n.kind == "input-use").count(),
        2
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.relation == projection::Relation::Approves)
    );
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.relation == projection::Relation::HasAcl)
    );
    assert!(
        !serde_json::to_string(&graph)
            .unwrap()
            .contains("launch-plan")
    );
    assert!(projection::build(&bundle, u64::MAX).is_err());
    assert_eq!(graph, projection::build(&bundle, 1_000_000).unwrap());
}

#[tokio::test]
#[ignore = "requires an actual Hydra server, token and an explicitly owned numeric ID range"]
async fn real_hydra_projection_roundtrip_is_idempotent_and_complete() {
    let url = std::env::var("TRUSTGRAPH_HYDRA_URL").expect("TRUSTGRAPH_HYDRA_URL");
    let token = std::env::var("TRUSTGRAPH_HYDRA_TOKEN").expect("TRUSTGRAPH_HYDRA_TOKEN");
    let base: u64 = std::env::var("TRUSTGRAPH_HYDRA_BASE_ID")
        .expect("allocate a private ID range")
        .parse()
        .unwrap();
    let graph = std::env::var("TRUSTGRAPH_HYDRA_GRAPH").unwrap_or("default".into());
    let cell = std::env::var("TRUSTGRAPH_HYDRA_CELL").unwrap_or("cell-0".into());
    let expected = projection::build(&publication_bundle(), base).unwrap();
    let mut client = HydraClient::new(&url, &graph, &cell, &token).unwrap();
    client.project(&expected).await.unwrap();
    client.project(&expected).await.unwrap();
    client.validate_projection(&expected, true).await.unwrap();
}
