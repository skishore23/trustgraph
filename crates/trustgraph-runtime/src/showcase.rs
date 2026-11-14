//! An isolated, resettable bank walkthrough. All decisions use the shared
//! verifier. Fixture generation records real manifests; no model or publisher
//! is called. Hypotheses modify a clone, never the coordinator or workspace.
use crate::{Coordinator, Error, fixtures, journal::MemoryJournal, publisher::LocalPublisher};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use trustgraph_core::{digest::Digest, verifier::publication_binding, *};

type DemoWorld = Coordinator<MemoryJournal, LocalPublisher>;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    SimulateMixed,
    FreshGeneration,
    ApprovePatch { expected: Digest },
    ChangePatch,
    ChangeDestination,
    SimulatePatch,
    ApproveBaseline { expected: BTreeMap<String, Digest> },
    MarkImpact { source: SourceId },
    EvaluateImpact,
    ClearImpact,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Baseline,
    Marked,
    Evaluated,
}

pub struct Showcase {
    world: DemoWorld,
    mixed_simulated: bool,
    fresh: Option<ArtifactId>,
    patch: ArtifactId,
    patch_audience: AudienceId,
    patch_simulated: bool,
    phase: Phase,
    changed_source: Option<SourceId>,
}

const RELEASES: [(&str, &str); 4] = [
    ("release-bundle", "engineering-repo"),
    ("risk-note", "risk-room"),
    ("compatibility-patch", "engineering-repo"),
    ("documentation-fix", "engineering-repo"),
];
const GRAPH: [(&str, &str, usize, &str); 9] = [
    ("case-analysis", "Case analysis", 1, "regression"),
    ("api-spec", "API specification", 1, "compatibility"),
    (
        "regression-fixtures",
        "Regression fixtures",
        2,
        "regression",
    ),
    (
        "compatibility-patch",
        "Compatibility patch",
        2,
        "compatibility",
    ),
    ("regression-report", "Regression report", 3, "regression"),
    (
        "compatibility-report",
        "Compatibility report",
        3,
        "compatibility",
    ),
    ("release-bundle", "Release bundle", 4, "release"),
    ("documentation-fix", "Documentation fix", 2, "independent"),
    ("risk-note", "Risk analysis note", 2, "risk"),
];

fn intent(artifact: &str, audience: &str) -> Intent {
    Intent {
        artifact: artifact.into(),
        audience: audience.into(),
        requester: "agent".into(),
    }
}

fn fixture_source(e: &mut Evidence, id: &str, content: &str) {
    let artifact = Artifact::new(
        id.into(),
        e.workspace.clone(),
        content.into(),
        Origin::Source(id.into()),
    );
    let acl = Acl {
        id: AclId(format!("{id}-acl-1")),
        workspace: e.workspace.clone(),
        revision: 1,
        readers: fixtures::readers(&["maya", "arun", "sam", "agent"]),
    };
    e.sources.insert(
        id.into(),
        Source {
            id: id.into(),
            workspace: e.workspace.clone(),
            artifact: artifact.id.clone(),
            acl: acl.id.clone(),
        },
    );
    e.acls.insert(acl.id.clone(), acl);
    e.artifacts.insert(artifact.id.clone(), artifact);
}

impl Showcase {
    pub fn new() -> Result<Self, Error> {
        let mut evidence = fixtures::bank();
        fixture_source(
            &mut evidence,
            "case-analysis",
            "FICTIONAL SHARED CASE ANALYSIS: duplicate payment retries can create reconciliation exceptions. This initial revision is authorized for Engineering and Risk; the simulator can hypothesize a later Risk-only access revision.",
        );
        fixture_source(
            &mut evidence,
            "api-spec",
            "FICTIONAL API SPECIFICATION: retain the reconciliation response contract and error codes; document duplicate transaction handling. Authorized for Engineering and Risk.",
        );
        let mut world = Coordinator::open_with_evidence(
            MemoryJournal::default(),
            LocalPublisher::default(),
            evidence,
        )?;
        for (id, inputs, content) in [
            (
                "mixed-test",
                vec!["engineering-requirements", "risk-investigation"],
                "// Synthetic generated test, with both Engineering and Risk context.\n#[test]\nfn duplicate_payment_is_reconciled_once() {\n    let ids = [\"txn-42\", \"txn-42\"];\n    assert_eq!(ids.into_iter().collect::<std::collections::BTreeSet<_>>().len(), 1);\n}\n",
            ),
            (
                "patch-v1",
                vec!["engineering-requirements", "approved-specification"],
                "// Candidate patch v1 — synthetic fixture, not executed.\npub fn is_duplicate(id: &str, seen: &mut std::collections::BTreeSet<String>) -> bool {\n    !seen.insert(id.to_owned())\n}\n",
            ),
            (
                "regression-fixtures",
                vec!["case-analysis"],
                "FICTIONAL FIXTURES: matched payment, duplicate retry, delayed settlement.",
            ),
            (
                "compatibility-patch",
                vec!["api-spec"],
                "FICTIONAL PATCH: preserve the API response contract for duplicate reconciliation.",
            ),
            (
                "regression-report",
                vec!["regression-fixtures"],
                "SYNTHETIC REPORT FIXTURE: records a regression review candidate. No tests were executed; this artifact is not a test attestation.",
            ),
            (
                "compatibility-report",
                vec!["compatibility-patch"],
                "SYNTHETIC REPORT FIXTURE: records compatibility review notes. No tests were executed; this artifact is not a test attestation.",
            ),
            (
                "release-bundle",
                vec!["regression-report", "compatibility-report"],
                "FICTIONAL RELEASE BUNDLE: proposed reconciliation release assembled from the two recorded report inputs. Nothing has been pushed.",
            ),
            (
                "documentation-fix",
                vec!["api-spec"],
                "FICTIONAL DOCUMENTATION FIX: describe the existing duplicate transaction error contract.",
            ),
            (
                "risk-note",
                vec!["case-analysis"],
                "FICTIONAL RISK NOTE: review duplicate retries within the Risk audience.",
            ),
        ] {
            generate(&mut world, id, &inputs, content)?;
        }
        Ok(Self {
            world,
            mixed_simulated: false,
            fresh: None,
            patch: "patch-v1".into(),
            patch_audience: "engineering-repo".into(),
            patch_simulated: false,
            phase: Phase::Baseline,
            changed_source: None,
        })
    }

    /// UI identities are authenticated by the host API. This check also protects
    /// direct callers and keeps restricted chapter-one metadata out of Maya's view.
    fn check_actor(&self, actor: &PrincipalId) -> Result<(), Error> {
        self.world.actor(actor).map(|_| ())
    }

    pub fn execute(
        &mut self,
        actor: &PrincipalId,
        command: Command,
        now: u64,
    ) -> Result<Value, Error> {
        self.check_actor(actor)?;
        match command {
            Command::SimulateMixed => {
                if !lineage::can_read(&self.world.state.evidence, actor, &"mixed-test".into()) {
                    return Err(Error::Unauthorized);
                }
                self.mixed_simulated = true;
            }
            Command::FreshGeneration => {
                if self.fresh.is_none() {
                    self.fresh = Some(generate(
                        &mut self.world,
                        "fresh-test",
                        &["engineering-requirements"],
                        "// Fresh isolated fixture: only Engineering requirements and the captured task instruction.\n#[test]\nfn repeated_transaction_ids_are_deduplicated() {\n    let ids = [\"payment-17\", \"payment-17\"];\n    assert_eq!(ids.into_iter().collect::<std::collections::BTreeSet<_>>().len(), 1);\n}\n",
                    )?);
                }
            }
            Command::ApprovePatch { expected } => {
                let target = intent(&self.patch.0, &self.patch_audience.0);
                let key = format!("demo-patch-{}", self.world.state.evidence.approvals.len());
                self.world.approve(actor, &target, &expected, &key, now)?;
                self.patch_simulated = true;
            }
            Command::ChangePatch => {
                let version = self.world.state.evidence.artifacts[&self.patch].version + 1;
                let content = format!(
                    "// Candidate patch v{version} — synthetic fixture, not executed.\npub fn is_duplicate(id: &str, seen: &mut std::collections::BTreeSet<String>) -> bool {{\n    let already_seen = !seen.insert(id.to_owned());\n    already_seen\n}}\n"
                );
                self.patch =
                    self.world
                        .edit(actor, &self.patch, &content, &format!("demo-v{version}"))?;
                self.patch_simulated = false;
            }
            Command::ChangeDestination => {
                self.patch_audience = if self.patch_audience.0 == "engineering-repo" {
                    "risk-room"
                } else {
                    "engineering-repo"
                }
                .into();
                self.patch_simulated = false;
            }
            Command::SimulatePatch => self.patch_simulated = true,
            Command::ApproveBaseline { expected } => {
                if self.phase != Phase::Baseline {
                    return Err(Error::Conflict);
                }
                self.world.reviewer(actor)?;
                if expected.len() != RELEASES.len() {
                    return Err(Error::Conflict);
                }
                // Preflight every context before recording any human decision.
                for (id, audience) in RELEASES {
                    let target = intent(id, audience);
                    let mut clean = self.world.state.evidence.clone();
                    clean.approvals.clear();
                    let mut reviewer_target = target;
                    reviewer_target.requester = actor.clone();
                    let context = verify(&clean, &reviewer_target, now)
                        .context()
                        .cloned()
                        .ok_or(Error::Unauthorized)?;
                    if expected.get(id) != Some(&context.digest()) {
                        return Err(Error::Conflict);
                    }
                }
                let batch = self.world.state.evidence.approvals.len();
                for (id, audience) in RELEASES {
                    self.world.approve(
                        actor,
                        &intent(id, audience),
                        &expected[id],
                        &format!("demo-{id}-{batch}"),
                        now,
                    )?;
                }
            }
            Command::MarkImpact { source } => {
                if !["case-analysis", "api-spec"].contains(&source.0.as_str()) {
                    return Err(Error::Invalid("choose a demo source".into()));
                }
                self.changed_source = Some(source);
                self.phase = Phase::Marked;
            }
            Command::EvaluateImpact => {
                if self.phase != Phase::Marked {
                    return Err(Error::Conflict);
                }
                self.phase = Phase::Evaluated;
            }
            Command::ClearImpact => {
                self.changed_source = None;
                self.phase = Phase::Baseline;
            }
        }
        self.view(actor, now)
    }

    pub fn view(&self, actor: &PrincipalId, now: u64) -> Result<Value, Error> {
        self.check_actor(actor)?;
        let e = &self.world.state.evidence;
        let can_review =
            e.policy.approvers.contains(actor) && e.principals[actor].role == Role::Human;
        let mixed_visible = lineage::can_read(e, actor, &"mixed-test".into());
        let simple = if mixed_visible {
            json!({"visible":true,"simulated":self.mixed_simulated,"mixed":artifact_view(e,"mixed-test"),"assessment":if self.mixed_simulated {Some(assessment(e,&intent("mixed-test","engineering-repo"),now))}else{None},"path":["Confidential Risk investigation","Atlas generation","Duplicate-payment test","Engineering repository"]})
        } else {
            json!({"visible":false,"reason":"The mixed-context test is restricted. Preview as Ravi to inspect this chapter; Maya cannot inspect Risk-derived work."})
        };
        let fresh = self.fresh.as_ref().map(|id| json!({"artifact":artifact_view(e,&id.0),"assessment":assessment(e,&intent(&id.0,"engineering-repo"),now)}));
        let patch_target = intent(&self.patch.0, &self.patch_audience.0);
        let binding = publication_binding(e, &patch_target).map_err(|_| Error::Conflict)?;
        let patch_history: Vec<_> = e
            .approvals
            .values()
            .filter(|a| e.artifacts[&a.context.artifact].logical_id == "patch-v1")
            .collect();
        let overlay = self.overlay();
        let affected = self
            .changed_source
            .as_ref()
            .map(|s| descendants(e, &e.sources[s].artifact))
            .unwrap_or_default();
        let evaluated = self.phase == Phase::Evaluated;
        let graph_evidence = if evaluated { &overlay } else { e };
        let mut nodes = Vec::new();
        for (id, title, level, lane) in GRAPH {
            let a = &e.artifacts[&ArtifactId::from(id)];
            let is_source = matches!(a.origin, Origin::Source(_));
            let target = intent(
                id,
                if id == "risk-note" {
                    "risk-room"
                } else {
                    "engineering-repo"
                },
            );
            let recheck = affected.contains(&a.id) && self.phase == Phase::Marked;
            let check = if is_source {
                None
            } else {
                Some(assessment(graph_evidence, &target, now))
            };
            nodes.push(json!({"id":id,"title":title,"level":level,"lane":lane,"kind":if is_source {"source"}else{"artifact"},"affected":affected.contains(&a.id),"needs_recheck":recheck,"assessment":check,"artifact":artifact_view(e,id),"readers":lineage::derived_readers(graph_evidence,&a.id).ok(),"publication_candidate":RELEASES.iter().any(|(root,_)|*root==id)}));
        }
        let selected: BTreeSet<_> = GRAPH.iter().map(|(id, ..)| ArtifactId::from(*id)).collect();
        let edges:Vec<_> = e.transformations.values().filter(|t| selected.contains(&t.output)).flat_map(|t| t.manifest.inputs.iter().filter(|i| selected.contains(&i.artifact)).map(move |i|json!({"from":i.artifact,"to":t.output,"kind":"used_as_input","transformation":t.id,"occurrence":i.occurrence}))).collect();
        let baseline_contexts: BTreeMap<_, _> = RELEASES
            .iter()
            .map(|(id, audience)| {
                Ok((
                    id.to_string(),
                    publication_binding(e, &intent(id, audience))
                        .map_err(|_| Error::Conflict)?
                        .digest(),
                ))
            })
            .collect::<Result<_, Error>>()?;
        let candidates:Vec<_> = RELEASES.iter().map(|(id,audience)|json!({"artifact":id,"audience":audience,"needs_recheck":affected.contains(&ArtifactId::from(*id))&&self.phase==Phase::Marked,"assessment":assessment(graph_evidence,&intent(id,audience),now)})).collect();
        let approval_edges:Vec<_> = e.approvals.values().filter(|a| selected.contains(&a.context.artifact)).map(|a|json!({"from":a.context.artifact,"to":a.id,"kind":"approved_context","context_digest":a.context.digest(),"signer":a.signer})).collect();
        let target_edges: Vec<_> = RELEASES
            .iter()
            .map(|(id, audience)| json!({"from":id,"to":audience,"kind":"targets_repository"}))
            .collect();
        let complex_history: Vec<_> = e
            .approvals
            .values()
            .filter(|a| selected.contains(&a.context.artifact))
            .collect();
        Ok(json!({
            "sandbox":true,"policy_revision":e.policy.revision,"policy_scope":"Fixed fictional demo policy; separate from the persistent workspace", "actor":actor,"can_review":can_review,"publisher_calls":self.world.publisher.sends,
            "simple":simple,"fresh":fresh,
            "medium":{"artifact":artifact_view(e,&self.patch.0),"audience":e.audiences[&self.patch_audience],"expected":binding.digest(),"simulated":self.patch_simulated,"assessment":if self.patch_simulated{Some(assessment(e,&patch_target,now))}else{None},"approvals":patch_history,"reviewer_authorized":e.policy.approvers.contains(&PrincipalId::from("maya"))&&e.principals[&PrincipalId::from("maya")].enabled},
            "complex":{"phase":match self.phase{Phase::Baseline=>"baseline",Phase::Marked=>"marked",Phase::Evaluated=>"evaluated"},"changed_source":self.changed_source,"affected":affected,"nodes":nodes,"edges":edges,"approval_edges":approval_edges,"target_edges":target_edges,"candidates":candidates,"expected":baseline_contexts,"approvals":complex_history,"active_source_readers":self.changed_source.as_ref().map(|s| &e.acls[&e.sources[s].acl].readers),"hypothetical_readers":self.changed_source.as_ref().map(|_|fixtures::readers(&["arun","agent"]))}
        }))
    }

    fn overlay(&self) -> Evidence {
        let mut e = self.world.state.evidence.clone();
        if let Some(source) = &self.changed_source {
            let previous = &e.acls[&e.sources[source].acl];
            let acl = Acl {
                id: AclId(format!(
                    "{}-hypothetical-{}",
                    source.0,
                    previous.revision + 1
                )),
                workspace: e.workspace.clone(),
                revision: previous.revision + 1,
                readers: fixtures::readers(&["arun", "agent"]),
            };
            e.sources.get_mut(source).expect("validated source").acl = acl.id.clone();
            e.acls.insert(acl.id.clone(), acl);
        }
        e
    }
}

fn generate(
    world: &mut DemoWorld,
    id: &str,
    inputs: &[&str],
    content: &str,
) -> Result<ArtifactId, Error> {
    let mut captured: Vec<ArtifactId> = inputs.iter().map(|id| (*id).into()).collect();
    captured.push("task-prompt".into());
    world.create_derived(
        &"agent".into(),
        id.into(),
        captured,
        content.into(),
        None,
        "bank-story-local-fixture-v1",
    )
}

/// Forward traversal only. Sharing a downstream bundle never makes another
/// input branch a descendant of this source.
fn descendants(e: &Evidence, root: &ArtifactId) -> BTreeSet<ArtifactId> {
    let mut found = BTreeSet::from([root.clone()]);
    loop {
        let before = found.len();
        for t in e.transformations.values() {
            if t.manifest
                .inputs
                .iter()
                .any(|i| found.contains(&i.artifact))
            {
                found.insert(t.output.clone());
            }
        }
        if before == found.len() {
            return found;
        }
    }
}

fn artifact_view(e: &Evidence, id: &str) -> Value {
    let a = &e.artifacts[&ArtifactId::from(id)];
    let transformation = match &a.origin {
        Origin::Transformation(t) => e.transformations.get(t),
        _ => None,
    };
    json!({"id":a.id,"version":a.version,"hash":a.hash,"content":a.content,"transformation":transformation})
}

fn assessment(e: &Evidence, target: &Intent, now: u64) -> Value {
    let decision = verify(e, target, now);
    let mut without_approval = e.clone();
    without_approval.approvals.clear();
    let permitted = matches!(
        verify(&without_approval, target, now),
        Decision::PendingApproval { .. }
    );
    let binding = publication_binding(e, target).ok();
    let relevant: Vec<_> = e
        .approvals
        .values()
        .filter(|a| {
            e.artifacts
                .get(&a.context.artifact)
                .is_some_and(|old| old.logical_id == e.artifacts[&target.artifact].logical_id)
        })
        .collect();
    let matching = relevant
        .iter()
        .any(|a| binding.as_ref() == Some(&a.context));
    let changed: BTreeSet<_> = relevant
        .iter()
        .filter(|_| !matching)
        .flat_map(|a| {
            let Some(b) = &binding else {
                return vec!["lineage"];
            };
            let old = &a.context;
            let mut changes = Vec::new();
            if old.artifact != b.artifact || old.artifact_hash != b.artifact_hash {
                changes.push("patch version / content");
            }
            if old.audience != b.audience {
                changes.push("destination / audience");
            }
            if old.acl_hash != b.acl_hash {
                changes.push("source access revision");
            }
            if old.lineage_hash != b.lineage_hash {
                changes.push("recorded lineage");
            }
            if old.policy_hash != b.policy_hash {
                changes.push("policy revision");
            }
            changes
        })
        .collect();
    let status = if !permitted {
        "blocked"
    } else if matches!(decision, Decision::Ready { .. }) {
        "ready"
    } else if !relevant.is_empty() {
        "needs_approval_renewal"
    } else {
        "needs_approval"
    };
    json!({"status":status,"information_permitted":permitted,"approval_matches":matching,"approval_valid":matches!(decision,Decision::Ready{..}),"approval_count":relevant.len(),"changed":changed,"decision":decision,"context_digest":binding.map(|b|b.digest())})
}
