//! Ask one question of a policy: may this agent send an output derived from
//! these sources through this action?
//!
//! Run with: `cargo run -p trustgraph-policy --example decide`
use trustgraph_policy::{Query, STARTER, Verdict, decide, parse};

fn main() {
    // In your system this is the text of your own `trustgraph.policy.yaml`.
    let parsed = parse(STARTER);
    let policy = parsed.policy.expect("the starter policy is valid");

    let ask = |sources: &[&str]| Query {
        output_from: sources.iter().map(|s| s.to_string()).collect(),
        agent: "planner".into(),
        action: "post_to_slack".into(),
        recipients: None, // the action's whole audience
    };

    // The agent read the salary file and the roadmap, then wants to post.
    let outcome = decide(&policy, &ask(&["salaries", "roadmap"]));
    assert_eq!(outcome.verdict, Verdict::Deny);
    println!("salaries + roadmap -> {}", outcome.reason);

    // The same agent, having read only the roadmap.
    let outcome = decide(&policy, &ask(&["roadmap"]));
    assert_eq!(outcome.verdict, Verdict::Allow);
    println!("roadmap only       -> {}", outcome.reason);
}
