use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use trustgraph_core::digest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Policy {
    pub version: u32,
    pub groups: BTreeMap<String, BTreeSet<String>>,
    pub agents: BTreeSet<String>,
    pub sources: BTreeMap<String, Source>,
    pub actions: BTreeMap<String, Action>,
    pub tests: Vec<Case>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Source {
    pub readers: BTreeSet<String>,
    pub agents: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Action {
    pub agents: BTreeSet<String>,
    pub audience: BTreeSet<String>,
    pub approval: Option<ApprovalRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApprovalRule {
    pub reviewers: BTreeSet<String>,
    pub valid_for_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Case {
    pub name: String,
    pub output_from: BTreeSet<String>,
    pub agent: String,
    pub action: String,
    pub recipients: Option<BTreeSet<String>>,
    pub expect: Verdict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Allow,
    NeedsApproval,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub path: String,
    pub code: &'static str,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Parsed {
    /// `Some` only when `diagnostics` holds no `Severity::Error`.
    pub policy: Option<Policy>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::NeedsApproval => "needs_approval",
            Verdict::Deny => "deny",
        }
    }
}

impl Policy {
    /// Expand group names to the people in them. A name that is itself a
    /// person stands for that person; names that are neither are dropped.
    pub fn people(&self, names: &BTreeSet<String>) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for name in names {
            if let Some(members) = self.groups.get(name) {
                out.extend(members.iter().cloned());
            } else if self.groups.values().any(|m| m.contains(name)) {
                out.insert(name.clone());
            }
        }
        out
    }

    /// SHA-256 of the canonical JSON form; independent of YAML key order.
    pub fn digest(&self) -> String {
        digest::structured("policy-v1", self).0
    }

    /// Canonical YAML that `parse` reads back to an equal policy.
    pub fn to_yaml(&self) -> String {
        let sources: BTreeMap<_, _> = self
            .sources
            .iter()
            .map(|(k, s)| {
                (
                    k,
                    serde_json::json!({"readers": s.readers, "agents": s.agents}),
                )
            })
            .collect();
        let actions: BTreeMap<_, _> = self
            .actions
            .iter()
            .map(|(k, a)| {
                let mut v = serde_json::json!({"agents": a.agents, "audience": a.audience});
                if let Some(r) = &a.approval {
                    v["approval"] = serde_json::json!({
                        "reviewers": r.reviewers,
                        "valid_for": format!("{}s", r.valid_for_seconds),
                    });
                }
                (k, v)
            })
            .collect();
        let tests: Vec<_> = self
            .tests
            .iter()
            .map(|c| {
                let mut v = serde_json::json!({
                    "name": c.name,
                    "output_from": c.output_from,
                    "agent": c.agent,
                    "action": c.action,
                    "expect": c.expect.as_str(),
                });
                if let Some(r) = &c.recipients {
                    v["recipients"] = serde_json::json!(r);
                }
                v
            })
            .collect();
        let doc = serde_json::json!({
            "version": self.version,
            "groups": self.groups,
            "agents": self.agents,
            "sources": sources,
            "actions": actions,
            "tests": tests,
        });
        serde_yaml::to_string(&doc).expect("policy is YAML serializable")
    }
}
