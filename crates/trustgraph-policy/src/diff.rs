//! Who gains or loses access between two versions of a policy.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::explain::duration;
use crate::{ApprovalRule, Policy};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Widens,
    Narrows,
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    pub kind: ChangeKind,
    /// "source salaries", "action post_to_slack", "tests" or "groups".
    pub subject: String,
    /// People (or agent names, for an `agents` list) who gain access.
    pub gained: BTreeSet<String>,
    /// People (or agent names, for an `agents` list) who lose access.
    pub lost: BTreeSet<String>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diff {
    pub changes: Vec<Change>,
}

impl Diff {
    pub fn widens(&self) -> bool {
        self.changes.iter().any(|c| c.kind == ChangeKind::Widens)
    }

    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// Compare on expanded people, so a group edit shows up on every source and
/// action that names the group. Order: sources by name, then actions by name,
/// then neutral changes; within a subject, widening before narrowing.
pub fn diff(old: &Policy, new: &Policy) -> Diff {
    let mut changes = Vec::new();

    let source_names: BTreeSet<_> = old.sources.keys().chain(new.sources.keys()).collect();
    for name in source_names {
        let subject = format!("source {name}");
        let mut subject_changes = Vec::new();
        match (old.sources.get(name), new.sources.get(name)) {
            (None, Some(s)) => {
                let readers = new.people(&s.readers);
                subject_changes.push(added(&subject, readers));
            }
            (Some(s), None) => {
                let readers = old.people(&s.readers);
                subject_changes.push(removed(&subject, readers));
            }
            (Some(o), Some(n)) => {
                set_change(
                    &mut subject_changes,
                    &subject,
                    "readers",
                    &old.people(&o.readers),
                    &new.people(&n.readers),
                );
                set_change(
                    &mut subject_changes,
                    &subject,
                    "agents",
                    &o.agents,
                    &n.agents,
                );
            }
            (None, None) => {}
        }
        push_sorted(&mut changes, subject_changes);
    }

    let action_names: BTreeSet<_> = old.actions.keys().chain(new.actions.keys()).collect();
    for name in action_names {
        let subject = format!("action {name}");
        let mut subject_changes = Vec::new();
        match (old.actions.get(name), new.actions.get(name)) {
            (None, Some(a)) => {
                let audience = new.people(&a.audience);
                subject_changes.push(added(&subject, audience));
            }
            (Some(a), None) => {
                let audience = old.people(&a.audience);
                subject_changes.push(removed(&subject, audience));
            }
            (Some(o), Some(n)) => {
                set_change(
                    &mut subject_changes,
                    &subject,
                    "agents",
                    &o.agents,
                    &n.agents,
                );
                set_change(
                    &mut subject_changes,
                    &subject,
                    "audience",
                    &old.people(&o.audience),
                    &new.people(&n.audience),
                );
                approval_change(
                    &mut subject_changes,
                    &subject,
                    (old, o.approval.as_ref()),
                    (new, n.approval.as_ref()),
                );
            }
            (None, None) => {}
        }
        push_sorted(&mut changes, subject_changes);
    }

    if old.tests != new.tests {
        changes.push(neutral("tests", "tests changed"));
    }
    if old.groups != new.groups && !changes.iter().any(|c| c.kind != ChangeKind::Neutral) {
        changes.push(neutral(
            "groups",
            "group membership changed; no source or action is affected",
        ));
    }
    Diff { changes }
}

fn push_sorted(out: &mut Vec<Change>, mut subject_changes: Vec<Change>) {
    subject_changes.sort_by_key(|c| c.kind);
    out.extend(subject_changes);
}

fn list(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join(", ")
}

fn neutral(subject: &str, summary: &str) -> Change {
    Change {
        kind: ChangeKind::Neutral,
        subject: subject.to_string(),
        gained: BTreeSet::new(),
        lost: BTreeSet::new(),
        summary: summary.to_string(),
    }
}

fn added(subject: &str, people: BTreeSet<String>) -> Change {
    Change {
        kind: ChangeKind::Widens,
        subject: subject.to_string(),
        summary: format!("{subject} added, reaching {}", or_nobody(&people)),
        gained: people,
        lost: BTreeSet::new(),
    }
}

fn removed(subject: &str, people: BTreeSet<String>) -> Change {
    Change {
        kind: ChangeKind::Narrows,
        subject: subject.to_string(),
        summary: format!(
            "{subject} removed, no longer reaching {}",
            or_nobody(&people)
        ),
        gained: BTreeSet::new(),
        lost: people,
    }
}

fn or_nobody(people: &BTreeSet<String>) -> String {
    if people.is_empty() {
        "nobody".to_string()
    } else {
        list(people)
    }
}

/// One Widens change for what was gained and one Narrows change for what was
/// lost, for the given aspect of the subject.
fn set_change(
    out: &mut Vec<Change>,
    subject: &str,
    aspect: &str,
    old: &BTreeSet<String>,
    new: &BTreeSet<String>,
) {
    let gained: BTreeSet<String> = new.difference(old).cloned().collect();
    let lost: BTreeSet<String> = old.difference(new).cloned().collect();
    if !gained.is_empty() {
        out.push(Change {
            kind: ChangeKind::Widens,
            subject: subject.to_string(),
            summary: format!("{aspect} gained {}", list(&gained)),
            gained,
            lost: BTreeSet::new(),
        });
    }
    if !lost.is_empty() {
        out.push(Change {
            kind: ChangeKind::Narrows,
            subject: subject.to_string(),
            summary: format!("{aspect} lost {}", list(&lost)),
            gained: BTreeSet::new(),
            lost,
        });
    }
}

fn describe(policy: &Policy, rule: &ApprovalRule) -> String {
    format!(
        "approval by {}, valid {}",
        or_nobody(&policy.people(&rule.reviewers)),
        duration(rule.valid_for_seconds)
    )
}

fn approval_change(
    out: &mut Vec<Change>,
    subject: &str,
    (old, o): (&Policy, Option<&ApprovalRule>),
    (new, n): (&Policy, Option<&ApprovalRule>),
) {
    let change = |kind, summary: String| Change {
        kind,
        subject: subject.to_string(),
        gained: BTreeSet::new(),
        lost: BTreeSet::new(),
        summary,
    };
    match (o, n) {
        (Some(o), None) => out.push(change(
            ChangeKind::Widens,
            format!("approval removed (was {})", describe(old, o)),
        )),
        (None, Some(n)) => out.push(change(
            ChangeKind::Narrows,
            format!("approval added ({})", describe(new, n)),
        )),
        (Some(o), Some(n)) => {
            let before = old.people(&o.reviewers);
            let after = new.people(&n.reviewers);
            let start = out.len();
            set_change(out, subject, "approval reviewers", &before, &after);
            // Reviewer sets are who may approve; keep that wording explicit.
            for c in &mut out[start..] {
                c.summary = format!(
                    "{} (was {}, now {})",
                    c.summary,
                    list(&before),
                    list(&after)
                );
            }
            if o.valid_for_seconds != n.valid_for_seconds {
                let kind = if n.valid_for_seconds > o.valid_for_seconds {
                    ChangeKind::Widens
                } else {
                    ChangeKind::Narrows
                };
                out.push(change(
                    kind,
                    format!(
                        "approval validity changed from {} to {}",
                        duration(o.valid_for_seconds),
                        duration(n.valid_for_seconds)
                    ),
                ));
            }
        }
        (None, None) => {}
    }
}
