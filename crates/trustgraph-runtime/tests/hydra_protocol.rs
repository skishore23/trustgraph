use std::io::{Read, Write};
use trustgraph_runtime::hydra::*;

/// A deliberately small HTTP fixture: the real HTTP client and parser run.
fn server(
    responses: Vec<serde_json::Value>,
) -> (String, std::thread::JoinHandle<Vec<serde_json::Value>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        let mut requests = vec![];
        for response in responses {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = vec![];
            let mut chunk = [0; 4096];
            let header_end = loop {
                let read = socket.read(&mut chunk).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer fixture-token")
            );
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .map(str::to_owned)
                })
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            while bytes.len() - header_end < length {
                let read = socket.read(&mut chunk).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&chunk[..read]);
            }
            requests.push(serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap());
            let body = response.to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
        requests
    });
    (address, handle)
}
fn page(row: u64, cursor: Option<u64>) -> serde_json::Value {
    serde_json::json!({"query_id":"q-1", "columns":["id"], "rows":[[{"type":"vertex_id","value":row}]], "next_cursor":cursor, "bookmark":"bookmark-1", "read_epoch":17})
}

#[tokio::test]
async fn remote_bundle_load_requires_one_well_formed_root_and_valid_body() {
    use trustgraph_core::digest::Digest;
    for rows in [
        serde_json::json!([]),
        serde_json::json!([[{"type":"vertex_id","value":1},{"type":"string","value":"root"},{"type":"string","value":"bundle"},{"type":"string","value":"not-json"},{"type":"string","value":"hash"},{"type":"string","value":"expected"}]]),
    ] {
        let (url, handle) = server(vec![
            serde_json::json!({"query_id":"q-1","columns":["id","logical","kind","body","digest","bundle"],"rows":rows,"next_cursor":null,"bookmark":"read-1"}),
        ]);
        let mut client = HydraClient::new(&url, "default", "cell-0", "fixture-token")
            .unwrap()
            .with_namespace("trustgraph-test")
            .unwrap();
        assert!(
            client
                .load_bundle(&Digest("expected".into()), 1000000)
                .await
                .is_err()
        );
        let requests = handle.join().unwrap();
        assert_eq!(requests[0]["parameters"]["kind"], "bundle");
    }
}

#[tokio::test]
async fn reconstructed_hydra_bundle_requires_every_record_relation_and_completion() {
    use trustgraph_core::*;
    use trustgraph_runtime::{
        Coordinator, PublishResult, journal::MemoryJournal, projection, publisher::LocalPublisher,
    };
    let mut w = Coordinator::open(MemoryJournal::default(), LocalPublisher::default()).unwrap();
    let artifact = w.generate(&"maya".into(), false, "remote-load").unwrap();
    let intent = Intent {
        artifact,
        audience: "company".into(),
        requester: "maya".into(),
    };
    let context = verify(&w.state().evidence, &intent, 10)
        .context()
        .unwrap()
        .digest();
    w.approve(&"maya".into(), &intent, &context, "remote-load", 10)
        .unwrap();
    let PublishResult::Operation { operation } =
        w.publish(&intent, &context, "remote-load", 11).unwrap()
    else {
        panic!("expected operation")
    };
    let bundle = operation.bundle;
    let projection = projection::build(&bundle, 1000000).unwrap();
    let node_page = |node: &projection::Node| serde_json::json!({"query_id":"q-1","columns":["id","logical","kind","body","digest","bundle"],"rows":[[Cell::VertexId(node.id),Cell::String(node.logical.clone()),Cell::String(node.kind.clone()),Cell::String(node.body.clone()),Cell::String(node.digest.clone()),Cell::String(node.bundle.clone())]],"next_cursor":null,"bookmark":"b-1"});
    for complete in [true, false] {
        let root = projection
            .nodes
            .iter()
            .find(|n| n.kind == "bundle")
            .unwrap();
        let mut pages = vec![node_page(root)];
        pages.extend(projection.nodes.iter().map(node_page));
        let relations: std::collections::BTreeSet<_> =
            projection.edges.iter().map(|e| e.relation).collect();
        for relation in relations {
            let rows: Vec<_> = projection
                .edges
                .iter()
                .filter(|e| e.relation == relation)
                .map(|e| vec![Cell::VertexId(e.src), Cell::VertexId(e.dst)])
                .collect();
            pages.push(serde_json::json!({"query_id":"q-1","columns":["src","dst"],"rows":rows,"next_cursor":null,"bookmark":"b-1"}));
        }
        let mut marker = node_page(&projection.marker);
        if !complete {
            marker["rows"] = serde_json::json!([]);
        }
        pages.push(marker);
        let (url, handle) = server(pages);
        let mut client = HydraClient::new(&url, "default", "cell-0", "fixture-token").unwrap();
        let result = client.load_bundle(&bundle.digest, 1000000).await;
        if complete {
            assert_eq!(result.unwrap(), bundle);
        } else {
            assert!(result.is_err());
        }
        let requests = handle.join().unwrap();
        assert_eq!(requests[0]["parameters"]["bundle"], bundle.digest.0);
        assert_eq!(
            requests.last().unwrap()["parameters"]["id"],
            projection.marker.id
        );
    }
}

#[tokio::test]
async fn consuming_cursor_keeps_full_request_and_accepts_constant_handle() {
    let (url, handle) = server(vec![page(1, Some(8)), page(2, Some(8)), page(3, None)]);
    let mut client = HydraClient::new(&url, "default", "cell-0", "fixture-token").unwrap();
    let rows = client
        .query(
            "MATCH (n:Record) WHERE n.logical = $name RETURN n.id AS id",
            serde_json::json!({"name":"a'quoted"}),
            1,
        )
        .await
        .unwrap();
    assert_eq!(
        rows.rows,
        vec![
            vec![Cell::VertexId(1)],
            vec![Cell::VertexId(2)],
            vec![Cell::VertexId(3)]
        ]
    );
    let requests = handle.join().unwrap();
    for request in &requests {
        assert_eq!(request["cell_id"], "cell-0");
        assert_eq!(request["page_size"], 1);
        assert_eq!(request["parameters"]["name"], "a'quoted");
    }
    assert_eq!(requests[1]["cursor"], 8);
    assert_eq!(requests[2]["cursor"], 8);
    assert_eq!(requests[1]["query_id"], "q-1");
}
#[tokio::test]
async fn null_optional_row_is_not_an_identity_and_malformed_envelopes_fail() {
    let (url, handle) = server(vec![
        serde_json::json!({"query_id":"q-1","columns":["id"],"rows":[[{"type":"null"}]],"next_cursor":null,"bookmark":null}),
    ]);
    let mut client = HydraClient::new(&url, "default", "cell-0", "fixture-token").unwrap();
    assert_eq!(
        client
            .query("query", serde_json::json!({}), 64)
            .await
            .unwrap()
            .rows,
        vec![vec![Cell::Null]]
    );
    handle.join().unwrap();
    for response in [
        serde_json::json!({"error":{"code":"internal"}}),
        serde_json::json!({"rows":[]}),
        serde_json::json!({"query_id":"q","columns":["id"],"rows":[[]],"next_cursor":null}),
    ] {
        let (url, handle) = server(vec![response]);
        let mut client = HydraClient::new(&url, "default", "cell-0", "fixture-token").unwrap();
        assert!(
            client
                .query("query", serde_json::json!({}), 64)
                .await
                .is_err()
        );
        handle.join().unwrap();
    }
}

#[tokio::test]
async fn write_bookmark_is_propagated_and_a_later_missing_ack_is_rejected() {
    let mutation = serde_json::json!({"query_id":"w","columns":[],"rows":[],"next_cursor":null,"bookmark":"write-position","read_epoch":null});
    let missing = serde_json::json!({"query_id":"w2","columns":[],"rows":[],"next_cursor":null,"bookmark":null,"read_epoch":null});
    let (url, handle) = server(vec![mutation, missing]);
    let mut client = HydraClient::new(&url, "default", "cell-0", "fixture-token").unwrap();
    client.mutate("write", serde_json::json!({})).await.unwrap();
    assert!(
        client
            .mutate("write again", serde_json::json!({}))
            .await
            .is_err()
    );
    let requests = handle.join().unwrap();
    assert_eq!(requests[1]["bookmark"], "write-position");
    assert!(requests[1].get("bookmarks").is_none());
}

#[tokio::test]
async fn failed_consuming_page_discards_all_rows_without_retry() {
    let (url, handle) = server(vec![
        page(1, Some(8)),
        serde_json::json!({"error":{"code":"internal"}}),
    ]);
    let mut client = HydraClient::new(&url, "default", "cell-0", "fixture-token").unwrap();
    assert!(
        client
            .query("query", serde_json::json!({}), 1)
            .await
            .is_err()
    );
    assert_eq!(handle.join().unwrap().len(), 2);
}
