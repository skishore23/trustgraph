//! Advisory checks on a parsed policy. Everything is a warning except a
//! reviewer list that can never read what it is asked to approve.

use std::collections::BTreeSet;

use trustgraph_core::permissions::can_flow;

use crate::model::{Diagnostic, Policy, Severity};

fn diag(severity: Severity, path: String, code: &'static str, message: String) -> Diagnostic {
    Diagnostic {
        severity,
        path,
        code,
        message,
        suggestion: None,
    }
}

pub fn lint(policy: &Policy) -> Vec<Diagnostic> {
    let mut out = Vec::new();

    if policy.tests.is_empty() {
        out.push(diag(
            Severity::Warning,
            "tests".into(),
            "no_tests",
            "the policy has no tests; add cases that pin down what must be denied".into(),
        ));
    }

    for (name, source) in &policy.sources {
        if source.agents.is_empty() {
            out.push(diag(
                Severity::Warning,
                format!("sources.{name}"),
                "source_unusable",
                format!("no agent may use source `{name}`"),
            ));
        }
    }

    for (name, action) in &policy.actions {
        let audience = policy.people(&action.audience);
        // Sources that could flow to this action on their own.
        let capable: Vec<BTreeSet<String>> = policy
            .sources
            .values()
            .filter(|s| s.agents.intersection(&action.agents).next().is_some())
            .map(|s| policy.people(&s.readers))
            .filter(|readers| can_flow(readers, &audience))
            .collect();
        if capable.is_empty() {
            out.push(diag(
                Severity::Warning,
                format!("actions.{name}"),
                "action_unsatisfiable",
                format!("no single source can ever flow to the audience of action `{name}`"),
            ));
        } else if let Some(rule) = &action.approval {
            let reviewers = policy.people(&rule.reviewers);
            let any_reads = capable
                .iter()
                .any(|readers| !readers.is_disjoint(&reviewers));
            if !any_reads {
                out.push(diag(
                    Severity::Error,
                    format!("actions.{name}.approval.reviewers"),
                    "reviewers_cannot_read",
                    format!(
                        "no reviewer of action `{name}` can read any source that could flow to it, so it can never be approved"
                    ),
                ));
            }
        }
    }

    let mut used: BTreeSet<&String> = BTreeSet::new();
    for s in policy.sources.values() {
        used.extend(&s.readers);
    }
    for a in policy.actions.values() {
        used.extend(&a.audience);
        if let Some(r) = &a.approval {
            used.extend(&r.reviewers);
        }
    }
    for c in &policy.tests {
        used.extend(c.recipients.iter().flatten());
    }
    for group in policy.groups.keys() {
        if !used.contains(group) {
            out.push(diag(
                Severity::Warning,
                format!("groups.{group}"),
                "unused_group",
                format!("group `{group}` is never referenced"),
            ));
        }
    }

    for agent in &policy.agents {
        let used = policy.sources.values().any(|s| s.agents.contains(agent))
            || policy.actions.values().any(|a| a.agents.contains(agent));
        if !used {
            out.push(diag(
                Severity::Warning,
                format!("agents.{agent}"),
                "unused_agent",
                format!("agent `{agent}` is not allowed on any source or action"),
            ));
        }
    }

    out
}
