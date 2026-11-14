use reqwest::{Client, StatusCode};
use serde_json::{Value, json};

#[tokio::test]
async fn consulting_simulation_enforces_evidence_scope_and_prepares_reviewable_memos() {
    let temp = tempfile::tempdir().unwrap();
    let app = trustgraph_cli::api::consulting_router(temp.path()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let c = Client::new();
    let get = |path: &str| c.get(format!("{base}/api{path}")).bearer_auth("demo-maya");
    let post = |path: &str, body: Value| {
        c.post(format!("{base}/api{path}"))
            .bearer_auth("demo-maya")
            .json(&body)
    };
    let all = json!({"sources":["market-brief","helios-data-room","orion-benchmark"],"audience":"atlas-committee"});
    let before: Value = get("/state").send().await.unwrap().json().await.unwrap();
    let blocked: Value = post("/scenario/simulate", all.clone())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(blocked["report"]["ready_to_generate"], false);
    assert_eq!(blocked["report"]["conflict"]["recipient"], "sam");
    assert_eq!(blocked["report"]["conflict"]["source"], "orion-benchmark");
    assert_eq!(blocked["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(
        get("/state")
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        before
    );
    let blocked_prepare = json!({"selection":all,"plan_digest":blocked["report"]["plan_digest"],"snapshot_digest":blocked["report"]["snapshot_digest"]});
    assert_eq!(
        post("/scenario/prepare", blocked_prepare)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(
        get("/plans")
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["plans"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for sources in [json!([]), json!(["market-brief", "market-brief"])] {
        assert_eq!(
            post(
                "/scenario/simulate",
                json!({"sources":sources,"audience":"atlas-committee"})
            )
            .send()
            .await
            .unwrap()
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let sam: Value = c
        .get(format!("{base}/api/scenario"))
        .bearer_auth("demo-sam")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!sam.to_string().contains("orion"));
    let mut unavailable = vec![];
    for source in ["orion-benchmark", "does-not-exist"] {
        let response = c
            .post(format!("{base}/api/scenario/simulate"))
            .bearer_auth("demo-sam")
            .json(&json!({"sources":[source],"audience":"atlas-committee"}))
            .send()
            .await
            .unwrap();
        unavailable.push((response.status(), response.json::<Value>().await.unwrap()));
    }
    assert_eq!(
        unavailable[0], unavailable[1],
        "hidden and unknown sources must be indistinguishable"
    );
    assert_eq!(
        c.post(format!("{base}/api/scenario/simulate"))
            .bearer_auth("demo-sam")
            .json(&json!({"sources":["orion-benchmark"],"audience":"atlas-committee"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let internal: Value = post("/scenario/simulate",json!({"sources":["market-brief","helios-data-room","orion-benchmark"],"audience":"engagement-team"})).send().await.unwrap().json().await.unwrap();
    assert_eq!(internal["report"]["ready_to_generate"], true);
    let selection =
        json!({"sources":["market-brief","helios-data-room"],"audience":"atlas-committee"});
    let safe: Value = post("/scenario/simulate", selection.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(safe["report"]["ready_to_generate"], true);
    let stale = json!({"selection":selection,"plan_digest":blocked["report"]["plan_digest"],"snapshot_digest":safe["report"]["snapshot_digest"]});
    assert_eq!(
        post("/scenario/prepare", stale)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert!(
        get("/plans")
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["plans"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let prepare = json!({"selection":selection,"plan_digest":safe["report"]["plan_digest"],"snapshot_digest":safe["report"]["snapshot_digest"]});
    assert_eq!(
        c.post(format!("{base}/api/scenario/prepare"))
            .bearer_auth("demo-sam")
            .json(&prepare)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let run: Value = post("/scenario/prepare", prepare.clone())
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        post("/scenario/prepare", prepare)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        run
    );
    let request = run["run"]["request"].as_str().unwrap();
    let prepared: Value = post("/scenario/simulate", selection.clone())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(prepared["prepared_review"]["request"], request);
    assert_eq!(prepared["prepared_review"]["status"], "awaiting_review");
    let review: Value = get(&format!("/requests/{request}"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(review["status"], "awaiting_review");
    assert!(
        review["artifact"]["content"]
            .as_str()
            .unwrap()
            .contains("$240 million")
    );
    assert!(
        !review["artifact"]["content"]
            .as_str()
            .unwrap()
            .contains("ORION")
    );
    let source_ids: Vec<_> = review["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["artifact"]["id"].as_str().unwrap())
        .collect();
    assert!(source_ids.contains(&"task-prompt"));
    assert!(source_ids.contains(&"helios-data-room"));
    assert!(!source_ids.contains(&"orion-benchmark"));
    let released: Value = post(
        &format!("/requests/{request}/release"),
        json!({"revision":1,"context_digest":review["context_digest"]}),
    )
    .send()
    .await
    .unwrap()
    .error_for_status()
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(released["status"], "released");
    assert_eq!(
        get("/state")
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()["operations"][0]["context"]["audience"]["destination"],
        "sim://aster-advisory/atlas-committee"
    );
    let internal_selection = json!({"sources":["market-brief","helios-data-room","orion-benchmark"],"audience":"engagement-team"});
    let internal_run: Value = post("/scenario/prepare",json!({"selection":internal_selection,"plan_digest":internal["report"]["plan_digest"],"snapshot_digest":internal["report"]["snapshot_digest"]})).send().await.unwrap().json().await.unwrap();
    let returned = internal_run["run"]["request"].as_str().unwrap();
    post(&format!("/requests/{returned}/changes"),json!({"revision":1,"body":"Revisit the evidence used for the client recommendation.","key":"return-internal"})).send().await.unwrap().error_for_status().unwrap();
    let returned_preview: Value = post("/scenario/simulate", internal_selection)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        returned_preview["prepared_review"]["status"],
        "changes_requested"
    );
    server.abort();
}
