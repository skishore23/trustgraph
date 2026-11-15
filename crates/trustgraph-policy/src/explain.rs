//! Plain-English reading of a policy, one line per source and action.

use std::collections::BTreeSet;
use std::fmt::Write;

use crate::Policy;

/// Plain text, sources first (by name) then actions (by name), one line each.
pub fn explain(policy: &Policy) -> String {
    let mut out = String::new();
    for (name, source) in &policy.sources {
        let agents = if source.agents.is_empty() {
            "Usable by no agent.".to_string()
        } else if source.agents.len() == 1 {
            format!("Usable by agent {}.", join(&source.agents))
        } else {
            format!("Usable by agents {}.", join(&source.agents))
        };
        let _ = writeln!(
            out,
            "{name}: output can reach {}. {agents}",
            named(policy, &source.readers)
        );
    }
    for (name, action) in &policy.actions {
        let who = if action.agents.is_empty() {
            "no agent".to_string()
        } else {
            join(&action.agents)
        };
        let approval = action.approval.as_ref().map_or(String::new(), |rule| {
            format!(
                " after approval by {}, valid {}",
                named(policy, &rule.reviewers),
                duration(rule.valid_for_seconds)
            )
        });
        let _ = writeln!(
            out,
            "{name}: {who} may send to {}{approval}.",
            named(policy, &action.audience)
        );
    }
    out
}

fn join(names: &BTreeSet<String>) -> String {
    names.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// Names, with each group followed by its sorted members in parentheses.
fn named(policy: &Policy, names: &BTreeSet<String>) -> String {
    if names.is_empty() {
        return "nobody".to_string();
    }
    names
        .iter()
        .map(|n| match policy.groups.get(n) {
            Some(members) => format!("{n} ({})", join(members)),
            None => n.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Largest exact unit: 1800 -> 30m, 3600 -> 1h, 86400 -> 1d, 90 -> 90s.
pub(crate) fn duration(seconds: u64) -> String {
    for (unit, size) in [("d", 86_400), ("h", 3_600), ("m", 60)] {
        if seconds >= size && seconds.is_multiple_of(size) {
            return format!("{}{unit}", seconds / size);
        }
    }
    format!("{seconds}s")
}
