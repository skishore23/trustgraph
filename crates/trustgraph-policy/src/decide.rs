//! Pure, deterministic answer to "may this agent send an output derived from
//! these sources through this action?". The ordering and codes come from the
//! shared kernel in `trustgraph_core::decision`; this module only computes the
//! facts and words the reason.

use std::collections::BTreeSet;

use serde::Serialize;
use trustgraph_core::decision::{Facts, Gate, gate};
use trustgraph_core::permissions::{can_flow, intersect};

use crate::model::{Policy, Verdict};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Query {
    pub output_from: BTreeSet<String>,
    pub agent: String,
    pub action: String,
    /// People or groups; `None` means the action's whole audience.
    pub recipients: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Blocking {
    pub source: String,
    /// Reader names as written in the policy.
    pub readers: BTreeSet<String>,
    /// Effective recipients (people) this source does not allow.
    pub excluded: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Outcome {
    pub verdict: Verdict,
    pub code: &'static str,
    pub reason: String,
    pub blocking: Vec<Blocking>,
    /// People who may read the output.
    pub readers: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TestResult {
    pub name: String,
    pub expect: Verdict,
    pub outcome: Outcome,
    pub pass: bool,
}

pub fn decide(policy: &Policy, query: &Query) -> Outcome {
    let action = policy.actions.get(&query.action);
    let action_known = action.is_some();
    let agent_known = policy.agents.contains(&query.agent);
    let sources_known = query
        .output_from
        .iter()
        .all(|s| policy.sources.contains_key(s));
    let has_sources = !query.output_from.is_empty();
    let resolved = action_known && agent_known && sources_known;

    let agent_listed = action.is_some_and(|a| a.agents.contains(&query.agent));
    let agent_may_use_inputs = resolved
        && has_sources
        && query
            .output_from
            .iter()
            .all(|s| policy.sources[s].agents.contains(&query.agent));

    // Who may read the output: the intersection over its sources.
    let mut readers: BTreeSet<String> = BTreeSet::new();
    if resolved {
        for (i, name) in query.output_from.iter().enumerate() {
            let people = policy.people(&policy.sources[name].readers);
            readers = if i == 0 {
                people
            } else {
                intersect(&readers, &people)
            };
        }
    }

    // Effective recipients and whether they sit inside the action's audience.
    let mut recipients: BTreeSet<String> = BTreeSet::new();
    let mut recipients_in_audience = false;
    let mut recipient_names: BTreeSet<String> = BTreeSet::new();
    if let Some(action) = action {
        let audience = policy.people(&action.audience);
        match &query.recipients {
            None => {
                recipient_names = action.audience.clone();
                recipients = audience;
                recipients_in_audience = true;
            }
            Some(explicit) => {
                recipient_names = explicit.clone();
                recipients = policy.people(explicit);
                // Names that resolve to nobody must never pass vacuously.
                let all_resolve = explicit
                    .iter()
                    .all(|n| !policy.people(&BTreeSet::from([n.clone()])).is_empty());
                recipients_in_audience =
                    !explicit.is_empty() && all_resolve && can_flow(&audience, &recipients);
            }
        }
    }
    let audience_in_readers = resolved && can_flow(&readers, &recipients);

    let facts = Facts {
        action_known,
        agent_known,
        sources_known,
        has_sources,
        agent_listed,
        agent_may_use_inputs,
        recipients_in_audience,
        audience_in_readers,
        approval_required: action.is_some_and(|a| a.approval.is_some()),
        approval_live: false,
    };
    let g = gate(facts);
    let verdict = match g {
        Gate::Allowed => Verdict::Allow,
        Gate::NeedsApproval => Verdict::NeedsApproval,
        _ => Verdict::Deny,
    };

    let mut blocking = Vec::new();
    if g == Gate::AudienceOutsideReaders {
        for name in &query.output_from {
            let source = &policy.sources[name];
            let allowed = policy.people(&source.readers);
            let excluded: BTreeSet<String> = recipients.difference(&allowed).cloned().collect();
            if !excluded.is_empty() {
                blocking.push(Blocking {
                    source: name.clone(),
                    readers: source.readers.clone(),
                    excluded,
                });
            }
        }
    }

    let reason = reason_for(policy, query, g, &blocking, &recipient_names);
    Outcome {
        verdict,
        code: g.code(),
        reason,
        blocking,
        readers: if resolved { readers } else { BTreeSet::new() },
    }
}

fn first_missing<'a>(
    names: impl Iterator<Item = &'a String>,
    known: impl Fn(&str) -> bool,
) -> String {
    names
        .into_iter()
        .find(|n| !known(n))
        .cloned()
        .unwrap_or_default()
}

fn reason_for(
    policy: &Policy,
    query: &Query,
    g: Gate,
    blocking: &[Blocking],
    recipient_names: &BTreeSet<String>,
) -> String {
    match g {
        Gate::UnknownAction => format!("denied: unknown action `{}`", query.action),
        Gate::UnknownAgent => format!("denied: unknown agent `{}`", query.agent),
        Gate::UnknownSource => {
            let missing =
                first_missing(query.output_from.iter(), |n| policy.sources.contains_key(n));
            format!("denied: unknown source `{missing}`")
        }
        Gate::EmptyOutput => "denied: the output is not derived from any source".to_string(),
        Gate::AgentNotListed => format!(
            "denied: agent `{}` may not use action `{}`",
            query.agent, query.action
        ),
        Gate::AgentCannotUseSource => {
            let source = query
                .output_from
                .iter()
                .find(|s| !policy.sources[*s].agents.contains(&query.agent))
                .cloned()
                .unwrap_or_default();
            format!(
                "denied: agent `{}` may not use source `{source}`",
                query.agent
            )
        }
        Gate::RecipientsOutsideAudience => format!(
            "denied: recipients are not all in the audience of action `{}`",
            query.action
        ),
        Gate::AudienceOutsideReaders => {
            let sentences: Vec<String> = blocking
                .iter()
                .map(|b| {
                    let readers = b.readers.iter().cloned().collect::<Vec<_>>().join(", ");
                    let mut parts = Vec::new();
                    for name in recipient_names {
                        let shown: Vec<String> = match policy.groups.get(name) {
                            Some(members) => members.intersection(&b.excluded).cloned().collect(),
                            None if b.excluded.contains(name) => {
                                parts.push(name.clone());
                                continue;
                            }
                            None => continue,
                        };
                        if !shown.is_empty() {
                            parts.push(format!("{name} ({})", shown.join(", ")));
                        }
                    }
                    format!(
                        "`{}` is readable by {readers} only; {} is not included",
                        b.source,
                        parts.join(", ")
                    )
                })
                .collect();
            format!("denied: {}", sentences.join(" "))
        }
        Gate::NeedsApproval => format!(
            "needs approval: action `{}` requires a reviewer to approve it",
            query.action
        ),
        Gate::Allowed => "allowed".to_string(),
    }
}

/// Run every inline test case in the policy.
pub fn run_tests(policy: &Policy) -> Vec<TestResult> {
    policy
        .tests
        .iter()
        .map(|case| {
            let outcome = decide(
                policy,
                &Query {
                    output_from: case.output_from.clone(),
                    agent: case.agent.clone(),
                    action: case.action.clone(),
                    recipients: case.recipients.clone(),
                },
            );
            TestResult {
                name: case.name.clone(),
                expect: case.expect,
                pass: outcome.verdict == case.expect,
                outcome,
            }
        })
        .collect()
}
