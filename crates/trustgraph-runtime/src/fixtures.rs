//! Synthetic Northstar data, never a snapshot of a connected account.
use std::collections::{BTreeMap, BTreeSet};
use trustgraph_core::*;

pub fn readers(names: &[&str]) -> Readers {
    names.iter().map(|n| PrincipalId::from(*n)).collect()
}

pub fn bank() -> Evidence {
    let mut e = northstar();
    e.workspace = "bank".into();
    e.policy.workspace = e.workspace.clone();
    for p in e.principals.values_mut() {
        p.workspace = e.workspace.clone();
    }
    e.artifacts.clear();
    e.sources.clear();
    e.acls.clear();
    e.audiences.clear();
    for (id, content, allowed) in [
        (
            "engineering-requirements",
            "SYNTHETIC BANK REQUIREMENTS: modernize payment reconciliation. Match expected and received minor-unit amounts exactly; reject negative amounts; repeated transaction IDs must not produce duplicate settlements. Prepare a Rust patch and tests for Engineering review.",
            readers(&["maya", "sam", "arun", "agent"]),
        ),
        (
            "approved-specification",
            "APPROVED ENGINEERING SPECIFICATION: use integer minor units; return an explicit result for unmatched records. Test matched, mismatched, duplicate and negative amounts. This specification is authorized for both departments; it is not an export of the confidential investigation.",
            readers(&["maya", "sam", "arun", "agent"]),
        ),
        (
            "risk-investigation",
            "RISK ONLY — FICTIONAL INVESTIGATION: a synthetic fraud pattern exploits timing differences between provisional settlement and duplicate reconciliation retries. The internal investigation contains a non-public detection window. This record and work derived from it must remain in the Risk audience.",
            readers(&["arun", "agent"]),
        ),
        (
            "task-prompt",
            "Prepare a candidate Rust reconciliation patch and tests from exactly the supplied documents. Record all inputs. A human must approve the actual artifact and repository audience; do not push branches or open a PR outside the controlled publisher.",
            readers(&["maya", "sam", "arun", "agent"]),
        ),
    ] {
        let a = Artifact::new(
            id.into(),
            e.workspace.clone(),
            content.into(),
            Origin::Source(id.into()),
        );
        let acl = Acl {
            id: AclId(format!("{id}-acl-1")),
            workspace: e.workspace.clone(),
            revision: 1,
            readers: allowed,
        };
        e.sources.insert(
            id.into(),
            Source {
                id: id.into(),
                workspace: e.workspace.clone(),
                artifact: a.id.clone(),
                acl: acl.id.clone(),
            },
        );
        e.acls.insert(acl.id.clone(), acl);
        e.artifacts.insert(a.id.clone(), a);
    }
    for (id, recipients) in [
        ("engineering-repo", readers(&["maya", "sam"])),
        ("risk-room", readers(&["arun"])),
    ] {
        let a = Audience {
            id: id.into(),
            workspace: e.workspace.clone(),
            destination: format!("sim://bank/{id}"),
            revision: 1,
            recipients,
            resolved: true,
        };
        e.audiences.insert(a.id.clone(), a);
    }
    e
}

pub fn bank_plan(id: &str, restricted: bool, target: &str) -> trustgraph_core::plan::Plan {
    use trustgraph_core::plan::*;
    let mut sources = vec![
        "engineering-requirements",
        "approved-specification",
        "task-prompt",
    ];
    if restricted {
        sources.push("risk-investigation");
    }
    let mut steps: Vec<_> = sources
        .into_iter()
        .map(|source| Step {
            id: format!("read-{source}"),
            operation: Op::Read {
                source: source.into(),
            },
        })
        .collect();
    let inputs = steps.iter().map(|s| s.id.clone()).collect();
    steps.extend([
        Step {
            id: "patch".into(),
            operation: Op::Generate {
                inputs,
                provider: Provider::LocalFixture,
            },
        },
        Step {
            id: "review".into(),
            operation: Op::Approve {
                artifact: "patch".into(),
                audience: target.into(),
            },
        },
        Step {
            id: "publish".into(),
            operation: Op::Publish {
                artifact: "patch".into(),
                audience: target.into(),
                approval: "review".into(),
            },
        },
    ]);
    Plan {
        id: id.into(),
        goal: "Implement and test the reconciliation change; prepare a draft PR".into(),
        steps,
    }
}

pub fn northstar() -> Evidence {
    let workspace = WorkspaceId::from("northstar");
    let company = readers(&["maya", "arun", "sam", "agent"]);
    let mut e = Evidence {
        workspace: workspace.clone(),
        complete: true,
        principals: BTreeMap::new(),
        artifacts: BTreeMap::new(),
        sources: BTreeMap::new(),
        acls: BTreeMap::new(),
        transformations: BTreeMap::new(),
        audiences: BTreeMap::new(),
        policy: Policy {
            workspace: workspace.clone(),
            revision: 1,
            approvers: readers(&["maya", "arun"]),
            max_approval_lifetime: 3600,
            configuration: None,
        },
        approvals: BTreeMap::new(),
        revoked: BTreeSet::new(),
    };
    for name in ["maya", "arun", "sam", "agent"] {
        let p = Principal {
            id: name.into(),
            workspace: workspace.clone(),
            role: if name == "agent" {
                Role::Agent
            } else {
                Role::Human
            },
            enabled: true,
        };
        e.principals.insert(p.id.clone(), p);
    }
    for (name, text, allowed) in [
        (
            "roadmap",
            "Northstar is building a collaborative planning workspace. A launch update will be shared after review.",
            company.clone(),
        ),
        (
            "launch-plan",
            "CONFIDENTIAL: Northstar plans an invitation-only launch on December 8. The unreleased price is $49 per seat.",
            readers(&["maya", "arun", "agent"]),
        ),
        (
            "task-prompt",
            "Prepare a short launch announcement from the supplied documents.",
            company,
        ),
    ] {
        let artifact = Artifact::new(
            name.into(),
            workspace.clone(),
            text.into(),
            Origin::Source(name.into()),
        );
        let acl = Acl {
            id: format!("{name}-acl-1").as_str().into(),
            workspace: workspace.clone(),
            revision: 1,
            readers: allowed,
        };
        e.sources.insert(
            name.into(),
            Source {
                id: name.into(),
                workspace: workspace.clone(),
                artifact: artifact.id.clone(),
                acl: acl.id.clone(),
            },
        );
        e.acls.insert(acl.id.clone(), acl);
        e.artifacts.insert(artifact.id.clone(), artifact);
    }
    for (id, recipients) in [
        ("company", readers(&["maya", "arun", "sam"])),
        ("product", readers(&["maya", "arun"])),
    ] {
        let audience = Audience {
            id: id.into(),
            workspace: workspace.clone(),
            destination: format!("sim://northstar/{id}"),
            revision: 1,
            recipients,
            resolved: true,
        };
        e.audiences.insert(audience.id.clone(), audience);
    }
    e
}

/// Fictional acquisition engagement. Internal use of the benchmark is explicitly
/// authorized in this fixture; it is not a general rule for cross-client use.
pub fn consulting() -> Evidence {
    let workspace = WorkspaceId::from("aster-advisory");
    let all = readers(&["maya", "arun", "sam", "agent"]);
    let mut e = Evidence {
        workspace: workspace.clone(),
        complete: true,
        principals: BTreeMap::new(),
        artifacts: BTreeMap::new(),
        sources: BTreeMap::new(),
        acls: BTreeMap::new(),
        transformations: BTreeMap::new(),
        audiences: BTreeMap::new(),
        policy: Policy {
            workspace: workspace.clone(),
            revision: 1,
            approvers: readers(&["maya", "arun"]),
            max_approval_lifetime: 3600,
            configuration: None,
        },
        approvals: BTreeMap::new(),
        revoked: BTreeSet::new(),
    };
    for name in ["maya", "arun", "sam", "agent"] {
        let id = PrincipalId::from(name);
        e.principals.insert(
            id.clone(),
            Principal {
                id,
                workspace: workspace.clone(),
                role: if name == "agent" {
                    Role::Agent
                } else {
                    Role::Human
                },
                enabled: true,
            },
        );
    }
    for (id, content, allowed) in [
        (
            "market-brief",
            "INDUSTRY BRIEF — Aster's licensed, client-shareable research. Industrial demand is cyclical; compare the target's customer concentration, cash conversion and downside resilience before committing capital. This brief alone does not establish a valuation.",
            all.clone(),
        ),
        (
            "helios-data-room",
            "HELIOS ACQUISITION — Atlas Capital is considering a $240 million enterprise-value offer. The target reports $180 million revenue and $27 million adjusted EBITDA: an implied 8.9x multiple (rounded). Its largest customer accounts for 32% of revenue and renews in six months. Recommendation for partner consideration: defer a binding offer until the renewal and quality-of-earnings review are resolved. These figures and this recommendation are fictional. Authorized for this Atlas engagement and its investment committee.",
            all.clone(),
        ),
        (
            "orion-benchmark",
            "ORION — PRIOR CLIENT CONFIDENTIAL. A previous engagement found an 18% EBITDA shortfall after customer renewal; the private downside model used a 6.5x multiple. Internal engagement analysis is explicitly permitted in this synthetic case. Atlas Capital is not authorized to receive this material or derived recommendations. Do not include these assumptions in an Atlas deliverable.",
            readers(&["maya", "arun", "agent"]),
        ),
        (
            "task-prompt",
            "Prepare an evidence brief for the Atlas acquisition decision. Identify the proposed offer, key diligence risk and conditions for partner review using only the supplied documents. Preserve uncertainty and distinguish recorded facts from judgment. A human partner must decide the recommendation; the agent must not approve a transaction.",
            all,
        ),
    ] {
        let artifact = Artifact::new(
            id.into(),
            workspace.clone(),
            content.into(),
            Origin::Source(id.into()),
        );
        let acl = Acl {
            id: AclId(format!("{id}-acl-1")),
            workspace: workspace.clone(),
            revision: 1,
            readers: allowed,
        };
        e.sources.insert(
            id.into(),
            Source {
                id: id.into(),
                workspace: workspace.clone(),
                artifact: artifact.id.clone(),
                acl: acl.id.clone(),
            },
        );
        e.acls.insert(acl.id.clone(), acl);
        e.artifacts.insert(artifact.id.clone(), artifact);
    }
    for (id, recipients) in [
        ("atlas-committee", readers(&["maya", "arun", "sam"])),
        ("engagement-team", readers(&["maya", "arun"])),
    ] {
        let audience = Audience {
            id: id.into(),
            workspace: workspace.clone(),
            destination: format!("sim://aster-advisory/{id}"),
            revision: 1,
            recipients,
            resolved: true,
        };
        e.audiences.insert(audience.id.clone(), audience);
    }
    e
}

pub(crate) fn render_plan_output(e: &Evidence, inputs: &[ArtifactId]) -> String {
    if e.workspace.0 == "bank" {
        return serde_json::to_string_pretty(&serde_json::json!({"format":"trustgraph-patch-v1","generator":"local_fixture","title":"Reconcile integer payment amounts without duplicate settlement","description":"Synthetic candidate patch for human review. No live coding agent has executed this fixture. Source permissions propagate from every recorded input; tests are proposed, not claimed to have run.","inputs":inputs,"files":[{"path":"src/reconciliation.rs","content":"use std::collections::BTreeSet;\n#[derive(Debug, PartialEq)]\npub enum Outcome { Matched, Mismatch, InvalidAmount, Duplicate }\npub fn reconcile(seen: &mut BTreeSet<String>, id: &str, expected: i64, received: i64) -> Outcome {\n    if expected < 0 || received < 0 { return Outcome::InvalidAmount; }\n    if expected != received { return Outcome::Mismatch; }\n    if !seen.insert(id.to_owned()) { return Outcome::Duplicate; }\n    Outcome::Matched\n}\n#[cfg(test)] mod tests { use super::*; #[test] fn settlement_is_exact_and_once() { let mut s=BTreeSet::new(); assert_eq!(reconcile(&mut s, \"a\",100,100),Outcome::Matched); assert_eq!(reconcile(&mut s, \"a\",100,100),Outcome::Duplicate); assert_eq!(reconcile(&mut s, \"b\",100,99),Outcome::Mismatch); assert_eq!(reconcile(&mut s, \"c\",-1,-1),Outcome::InvalidAmount); }}\n"}]})).expect("fixture JSON");
    }
    let evidence = inputs
        .iter()
        .filter(|id| **id != ArtifactId::from("task-prompt"))
        .map(|id| e.artifacts[id].content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    if e.workspace == WorkspaceId::from("aster-advisory") {
        format!(
            "ATLAS CAPITAL · ACQUISITION DECISION BRIEF\nFor partner review\n\nRECORDED EVIDENCE\n{evidence}\n\nPARTNER DECISION REQUIRED\nConfirm the evidence, assess the downside and decide whether to proceed, renegotiate or pause. Missing sources are not assumed to support this recommendation.\n\nPrepared by the local fixture renderer from the declared inputs. No live model or independent financial assessment was used."
        )
    } else {
        format!("Northstar launch update\n\n{evidence}")
    }
}
