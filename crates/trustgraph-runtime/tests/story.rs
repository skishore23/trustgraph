use serde_json::{Value, json};
use trustgraph_runtime::story::{Command, Scenario, Story, StoryPolicy};

fn execute(story: &mut Story, body: Value) -> Value {
    story
        .execute(serde_json::from_value::<Command>(body).unwrap())
        .unwrap()
}
fn review(story: &mut Story) -> Value {
    let v = story.view();
    execute(
        story,
        json!({"action":"approve","reviewer":"maya","artifact":v["current"]["artifact"]["id"],"expected":v["current"]["assessment"]["context_digest"]}),
    )
}
fn release(story: &mut Story) -> Value {
    let v = story.view();
    execute(
        story,
        json!({"action":"release","artifact":v["current"]["artifact"]["id"],"expected":v["current"]["assessment"]["context_digest"]}),
    )
}
#[test]
fn one_work_item_keeps_exact_artifacts_through_repair_review_release_and_revision() {
    let mut story = Story::new().unwrap();
    let mixed = execute(
        &mut story,
        json!({"action":"generate","approved_inputs":false,"expected_revision":1}),
    );
    assert_eq!(mixed["current"]["assessment"]["code"], "RESTRICTED_SOURCE");
    let blocked = json!({"action":"approve","reviewer":"maya","artifact":mixed["current"]["artifact"]["id"],"expected":mixed["current"]["assessment"]["context_digest"]});
    assert!(
        story
            .execute(serde_json::from_value(blocked).unwrap())
            .is_err()
    );
    let clean = execute(
        &mut story,
        json!({"action":"generate","approved_inputs":true,"expected_revision":1}),
    );
    assert_eq!(clean["work_id"], mixed["work_id"]);
    assert_ne!(
        clean["current"]["artifact"]["id"],
        mixed["current"]["artifact"]["id"]
    );
    assert_eq!(
        clean["current"]["inputs"],
        json!(["directory", "approved-planning", "task-prompt"])
    );
    assert_eq!(clean["current"]["assessment"]["code"], "APPROVAL_REQUIRED");
    let approved = review(&mut story);
    assert_eq!(
        approved["current"]["artifact"]["id"],
        clean["current"]["artifact"]["id"]
    );
    assert_eq!(approved["current"]["assessment"]["code"], "READY");
    let delivered = release(&mut story);
    assert_eq!(
        delivered["current"]["operation"]["bundle"]["intent"]["artifact"],
        clean["current"]["artifact"]["id"]
    );
    assert_eq!(
        delivered["current"]["operation"]["receipt"]["simulated"],
        true
    );
    let again = release(&mut story);
    assert_eq!(
        again["current"]["operation"],
        delivered["current"]["operation"]
    );
    assert_eq!(again["events"], delivered["events"]);
    let revised = execute(
        &mut story,
        json!({"action":"revise","artifact":clean["current"]["artifact"]["id"]}),
    );
    assert_eq!(revised["work_id"], mixed["work_id"]);
    assert_eq!(
        revised["current"]["assessment"]["information_permitted"],
        true
    );
    assert_eq!(revised["current"]["assessment"]["code"], "STALE_APPROVAL");
    assert_eq!(
        revised["current"]["inputs"],
        json!([clean["current"]["artifact"]["id"]])
    );
    assert_eq!(
        revised["versions"][1]["operation"],
        delivered["current"]["operation"]
    );
    assert!(revised["current"]["operation"].is_null());
    let forbidden = json!({"action":"release","artifact":revised["current"]["artifact"]["id"],"expected":revised["current"]["assessment"]["context_digest"]});
    assert!(
        story
            .execute(serde_json::from_value(forbidden).unwrap())
            .is_err()
    );
    assert_eq!(review(&mut story)["current"]["assessment"]["code"], "READY");
    assert_eq!(
        release(&mut story)["current"]["operation"]["state"],
        "PUBLISHED"
    );
}

#[test]
fn simulations_are_identical_on_replay_and_never_mutate_work_or_policy() {
    let story = Story::new().unwrap();
    let before = story.view();
    let policy = StoryPolicy::default();
    let mut case = Scenario::default();
    let first = story.compare(&policy, &case).unwrap();
    assert_eq!(first, story.compare(&policy, &case).unwrap());
    assert_eq!(first["active"]["code"], "RESTRICTED_SOURCE");
    assert_eq!(first["external_writes"], 0);
    case.destination = "hr".into();
    let hr = story.compare(&policy, &case).unwrap();
    assert_eq!(hr["active"]["status"], "needs_approval");
    assert_eq!(hr["active"]["permitted_departments"], json!(["hr"]));
    case.destination = "finance".into();
    let finance = story.compare(&policy, &case).unwrap();
    assert_eq!(finance["active"]["code"], "RESTRICTED_SOURCE");
    assert_eq!(
        finance["active"]["conflicting_sources"],
        json!(["compensation"])
    );
    assert_eq!(story.view(), before);
}

#[test]
fn simulator_and_live_demo_agree_on_policy_changes_and_stale_approvals() {
    let mut story = Story::new().unwrap();
    let case:Scenario=serde_json::from_value(json!({"inputs":["directory","approved-planning"],"destination":"engineering","approval":"valid","complete_lineage":true})).unwrap();
    let mut policy = StoryPolicy::default();
    policy
        .source_readers
        .get_mut("approved-planning")
        .unwrap()
        .remove("engineering");
    let comparison = story.compare(&policy, &case).unwrap();
    assert_eq!(comparison["active"]["code"], "READY");
    assert_eq!(comparison["candidate"]["code"], "RESTRICTED_SOURCE");
    assert_eq!(story.view()["policy_revision"], 1);
    execute(
        &mut story,
        json!({"action":"activate","policy":policy,"expected_revision":1}),
    );
    let work = execute(
        &mut story,
        json!({"action":"generate","approved_inputs":true,"expected_revision":2}),
    );
    assert_eq!(
        work["current"]["assessment"]["code"],
        comparison["candidate"]["code"]
    );
    let changed:Scenario=serde_json::from_value(json!({"inputs":["directory","approved-planning"],"destination":"engineering","approval":"changed","complete_lineage":true})).unwrap();
    let baseline = Story::new().unwrap();
    let result = baseline.compare(&StoryPolicy::default(), &changed).unwrap();
    assert_eq!(result["active"]["code"], "STALE_APPROVAL");
    assert_eq!(result["active"]["information_permitted"], true);
}

#[test]
fn missing_lineage_expiry_and_ineligible_reviewers_cannot_pass_the_gate() {
    let story = Story::new().unwrap();
    for (approval, complete, expected) in [
        ("valid", false, "INCOMPLETE_LINEAGE"),
        ("expired", true, "APPROVAL_EXPIRED"),
    ] {
        let case:Scenario=serde_json::from_value(json!({"inputs":["approved-planning"],"destination":"engineering","approval":approval,"complete_lineage":complete})).unwrap();
        assert_eq!(
            story.compare(&StoryPolicy::default(), &case).unwrap()["active"]["code"],
            expected
        );
    }
    let mut invalid = StoryPolicy::default();
    invalid.approvers.insert("agent".into());
    assert!(story.compare(&invalid, &Scenario::default()).is_err());
    let mut unknown = Scenario::default();
    unknown.inputs.insert("untracked".into());
    assert!(story.compare(&StoryPolicy::default(), &unknown).is_err());
}

#[test]
fn stale_browser_actions_and_stale_policy_revisions_are_rejected() {
    let mut story = Story::new().unwrap();
    let first = execute(
        &mut story,
        json!({"action":"generate","approved_inputs":true,"expected_revision":1}),
    );
    execute(
        &mut story,
        json!({"action":"generate","approved_inputs":true,"expected_revision":1}),
    );
    let stale = json!({"action":"approve","reviewer":"maya","artifact":first["current"]["artifact"]["id"],"expected":first["current"]["assessment"]["context_digest"]});
    assert!(
        story
            .execute(serde_json::from_value(stale).unwrap())
            .is_err()
    );
    let stale = json!({"action":"generate","approved_inputs":true,"expected_revision":0});
    assert!(
        story
            .execute(serde_json::from_value(stale).unwrap())
            .is_err()
    );
}
