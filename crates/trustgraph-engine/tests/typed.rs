use trustgraph_engine::{Event, FileJournal, Journal, OutputContract, Record, Reply, Runtime};
use trustgraph_framework::{ArtifactId, SchemaId};
use trustgraph_policy::Verdict;

const POLICY: &str =
    include_str!("../../trustgraph-policy/tests/fixtures/spec-example.policy.yaml");
fn artifact(reply: Reply) -> String {
    match reply {
        Reply::Artifact { id } => id,
        other => panic!("{other:?}"),
    }
}
fn schema(name: &str) -> SchemaId {
    SchemaId(name.into())
}

#[test]
fn typed_transforms_never_widen_readers_and_replay_identically() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::open(FileJournal::open(dir.path()).unwrap(), POLICY, 0).unwrap();
    let source = artifact(
        rt.handle(
            1,
            Event::RegisterSource {
                source: "salaries".into(),
                sha256: "a".repeat(64),
                schema: Some(schema("SourceCode@1")),
            },
        )
        .unwrap(),
    );
    let event = Event::RecordOutput {
        agent: "planner".into(),
        step: "trusted_summary".into(),
        inputs: vec![source.clone()],
        sha256: "b".repeat(64),
        contract: Some(OutputContract {
            version: "1".into(),
            inputs: vec![schema("SourceCode@1")],
            output: schema("PublicSummary@1"),
        }),
    };
    let output = artifact(rt.handle(2, event.clone()).unwrap());
    assert_eq!(
        rt.engine().graph().artifacts()[&ArtifactId(output.clone())].schema,
        schema("PublicSummary@1")
    );
    assert_eq!(
        rt.engine().graph().readers(&ArtifactId(source)).unwrap(),
        rt.engine()
            .graph()
            .readers(&ArtifactId(output.clone()))
            .unwrap()
    );
    let check = Event::Check {
        agent: "planner".into(),
        artifact: output.clone(),
        action: "post_to_slack".into(),
        recipients: None,
    };
    let denied = rt.handle(3, check.clone()).unwrap();
    assert!(matches!(
        denied,
        Reply::Decision {
            verdict: Verdict::Deny,
            ..
        }
    ));
    let lineage = rt.engine().typed_lineage(&output).unwrap();
    assert!(lineage.to_string().contains("trusted_summary"));
    assert!(lineage.to_string().contains("SourceCode@1"));
    let digest = rt.engine().state_digest();
    drop(rt);
    let mut replay = Runtime::open(FileJournal::open(dir.path()).unwrap(), POLICY, 4).unwrap();
    assert_eq!(replay.engine().state_digest(), digest);
    assert_eq!(replay.engine().typed_lineage(&output), Some(lineage));
    assert_eq!(replay.handle(4, check).unwrap(), denied);
    assert_eq!(artifact(replay.handle(5, event).unwrap()), output);
}

#[test]
fn a_contract_mismatch_cannot_create_an_artifact_or_execution() {
    let mut rt = Runtime::open(trustgraph_engine::MemoryJournal::new(), POLICY, 0).unwrap();
    let source = artifact(
        rt.handle(
            1,
            Event::RegisterSource {
                source: "roadmap".into(),
                sha256: "a".repeat(64),
                schema: Some(schema("SourceCode@1")),
            },
        )
        .unwrap(),
    );
    let reply = rt
        .handle(
            2,
            Event::RecordOutput {
                agent: "planner".into(),
                step: "summary".into(),
                inputs: vec![source],
                sha256: "b".repeat(64),
                contract: Some(OutputContract {
                    version: "1".into(),
                    inputs: vec![schema("SourceCode@2")],
                    output: schema("Summary@1"),
                }),
            },
        )
        .unwrap();
    assert!(matches!(reply, Reply::Rejected { code, .. } if code == "schema_mismatch"));
    assert_eq!(rt.engine().artifact_count(), 1);
    assert!(rt.engine().graph().executions().is_empty());
    assert!(rt.engine().graph().operations().is_empty());
}

#[test]
fn legacy_event_encoding_and_hashes_stay_unchanged() {
    for value in [
        serde_json::json!({"type":"register_source","source":"roadmap","sha256":"a".repeat(64)}),
        serde_json::json!({"type":"record_output","agent":"planner","step":"summarize","inputs":["artifact"],"sha256":"b".repeat(64)}),
    ] {
        let event: Event = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&event).unwrap(), value);
        // Independently encode the old journal hash body, without the new fields.
        let body = serde_json::json!({"domain":"engine-journal-v1","schema":1,"payload":{"prev":"0".repeat(64),"seq":2,"at":1,"event":value}});
        let expected = trustgraph_core::digest::content(&serde_json::to_vec(&body).unwrap()).0;
        let record = Record::seal(2, 1, event, &"0".repeat(64));
        assert_eq!(record.hash, expected);
    }
    let dir = tempfile::tempdir().unwrap();
    let mut rt = Runtime::open(FileJournal::open(dir.path()).unwrap(), POLICY, 0).unwrap();
    let source: Event = serde_json::from_value(
        serde_json::json!({"type":"register_source","source":"roadmap","sha256":"a".repeat(64)}),
    )
    .unwrap();
    let source = artifact(rt.handle(1, source).unwrap());
    drop(rt);
    let mut journal = FileJournal::open(dir.path()).unwrap();
    let records = journal.load().unwrap();
    assert!(
        !serde_json::to_string(&records[1])
            .unwrap()
            .contains("schema")
    );
    drop(journal);
    let replay = Runtime::open(FileJournal::open(dir.path()).unwrap(), POLICY, 2).unwrap();
    assert_eq!(
        replay.engine().graph().artifacts()[&ArtifactId(source)].schema,
        schema("any")
    );
}
