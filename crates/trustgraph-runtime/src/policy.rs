//! A deliberately closed policy language. Mandatory gates cannot be disabled.
use crate::{journal::Journal, publisher::Publisher, *};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use trustgraph_core::{
    digest::{self, Digest},
    plan, *,
};

pub const RULES: [&str; 3] = [
    "derived_artifact_inherits_input_restrictions",
    "approval_binds_exact_patch_and_destination",
    "missing_lineage_blocks",
];
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySpec {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub groups: BTreeMap<String, Readers>,
    pub sources: BTreeMap<SourceId, SourceRule>,
    pub destinations: BTreeMap<AudienceId, DestinationRule>,
    pub review: ReviewRule,
    pub rules: BTreeSet<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRule {
    pub readers: BTreeSet<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationRule {
    pub audience: BTreeSet<String>,
    pub requires_approval: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRule {
    pub approvers: Readers,
    pub max_age_seconds: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyVersion {
    pub yaml: String,
    pub spec: PolicySpec,
    pub digest: Digest,
    pub submitted_by: PrincipalId,
    pub created_at: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySnapshot {
    pub version: String,
    pub policy: Policy,
    pub acls: BTreeMap<SourceId, Acl>,
    pub audiences: BTreeMap<AudienceId, Audience>,
}
pub fn parse(yaml: &str) -> Result<PolicySpec, Error> {
    if yaml.len() > 32_768 {
        return Err(Error::Invalid("policy exceeds 32768 bytes".into()));
    }
    serde_yaml::from_str(yaml).map_err(|_| {
        Error::Invalid(
            "invalid policy YAML or unsupported field; schema_version 1 is required".into(),
        )
    })
}
fn invalid(message: &str) -> Error {
    Error::Invalid(message.into())
}
pub fn compile(
    e: &Evidence,
    spec: &PolicySpec,
    source_authority: &BTreeMap<SourceId, Readers>,
    destination_authority: &BTreeMap<AudienceId, Audience>,
) -> Result<PolicySnapshot, Error> {
    crate::engine::validate_key(&spec.id)?;
    if spec.schema_version != 1
        || spec.name.is_empty()
        || spec.name.len() > 120
        || spec.groups.is_empty()
        || spec.groups.len() > 16
        || spec.review.approvers.is_empty()
        || !(60..=86400).contains(&spec.review.max_age_seconds)
    {
        return Err(invalid(
            "invalid version, name, groups, reviewers or approval lifetime (60..86400 seconds)",
        ));
    }
    if spec.rules != RULES.into_iter().map(str::to_owned).collect() {
        return Err(invalid(
            "all three supported rules are mandatory; unknown rules are rejected",
        ));
    }
    if spec.sources.keys().ne(source_authority.keys())
        || spec.destinations.keys().ne(destination_authority.keys())
    {
        return Err(invalid(
            "policy must explicitly cover every authoritative source and destination",
        ));
    }
    for (group, members) in &spec.groups {
        crate::engine::validate_key(group)?;
        if members.is_empty()
            || members.iter().any(|id| {
                !e.principals
                    .get(id)
                    .is_some_and(|p| p.id == *id && p.enabled && p.workspace == e.workspace)
            })
        {
            return Err(invalid("groups must contain known enabled principals"));
        }
    }
    if spec.review.approvers.iter().any(|id| {
        !e.principals
            .get(id)
            .is_some_and(|p| p.enabled && p.role == Role::Human && p.workspace == e.workspace)
    }) {
        return Err(invalid("only enabled humans may approve"));
    }
    let expand = |groups: &BTreeSet<String>| -> Result<Readers, Error> {
        if groups.is_empty() {
            return Err(invalid("source/destination group sets cannot be empty"));
        }
        let mut members = Readers::new();
        for group in groups {
            members.extend(
                spec.groups
                    .get(group)
                    .ok_or_else(|| invalid("unknown group reference"))?
                    .iter()
                    .cloned(),
            );
        }
        Ok(members)
    };
    let revision = e.policy.revision.checked_add(1).ok_or(Error::Conflict)?;
    let configuration = digest::structured("policy-package", spec);
    let mut acls = BTreeMap::new();
    for (id, rule) in &spec.sources {
        let readers = expand(&rule.readers)?;
        if !readers.contains(&PrincipalId::from(administrator(e))) {
            return Err(invalid(
                "the demo policy administrator must retain access needed to inspect policy candidates",
            ));
        }
        if !readers.is_subset(&source_authority[id]) {
            return Err(invalid(
                "policy cannot grant source access beyond its authoritative ceiling",
            ));
        }
        let source = e.sources.get(id).ok_or(Error::NotFound)?;
        let previous = e.acls.get(&source.acl).ok_or(Error::NotFound)?;
        acls.insert(
            id.clone(),
            Acl {
                id: AclId(format!(
                    "policy-{}-{}-{}",
                    &configuration.0[..16],
                    revision,
                    id.0
                )),
                workspace: e.workspace.clone(),
                revision: previous.revision.checked_add(1).ok_or(Error::Conflict)?,
                readers,
            },
        );
    }
    let mut audiences = BTreeMap::new();
    for (id, rule) in &spec.destinations {
        if !rule.requires_approval {
            return Err(invalid("human approval cannot be disabled"));
        }
        let recipients: Readers = expand(&rule.audience)?
            .into_iter()
            .filter(|id| e.principals[id].role == Role::Human)
            .collect();
        if recipients.is_empty() || recipients != destination_authority[id].recipients {
            return Err(invalid(
                "destination audience must match authoritative membership; policy cannot hide actual recipients",
            ));
        }
        let mut a = destination_authority[id].clone();
        a.recipients = recipients;
        a.revision = e
            .audiences
            .get(id)
            .ok_or(Error::NotFound)?
            .revision
            .checked_add(1)
            .ok_or(Error::Conflict)?;
        a.resolved = true;
        audiences.insert(id.clone(), a);
    }
    Ok(PolicySnapshot {
        version: spec.id.clone(),
        policy: Policy {
            workspace: e.workspace.clone(),
            revision,
            approvers: spec.review.approvers.clone(),
            max_approval_lifetime: spec.review.max_age_seconds,
            configuration: Some(configuration),
        },
        acls,
        audiences,
    })
}
fn apply(e: &mut Evidence, snapshot: &PolicySnapshot) -> Result<(), Error> {
    if snapshot.policy.workspace != e.workspace || snapshot.policy.revision != e.policy.revision + 1
    {
        return Err(Error::Conflict);
    }
    for (id, acl) in &snapshot.acls {
        let s = e.sources.get_mut(id).ok_or(Error::NotFound)?;
        s.acl = acl.id.clone();
        e.acls.insert(acl.id.clone(), acl.clone());
    }
    e.audiences = snapshot.audiences.clone();
    e.policy = snapshot.policy.clone();
    Ok(())
}
impl<J: Journal, P: Publisher> Coordinator<J, P> {
    pub fn propose_policy(
        &mut self,
        actor: &PrincipalId,
        yaml: &str,
        now: u64,
    ) -> Result<PolicyVersion, Error> {
        self.actor(actor)?;
        if !can_inspect(&self.state.evidence, actor) {
            return Err(Error::Unauthorized);
        }
        let spec = parse(yaml)?;
        compile(
            &self.state.evidence,
            &spec,
            &self.state.source_authority,
            &self.state.destination_authority,
        )?;
        let digest = digest::structured("policy-package", &spec);
        if let Some(old) = self.state.policy_versions.get(&spec.id) {
            return if old.spec == spec {
                Ok(old.clone())
            } else {
                Err(Error::Conflict)
            };
        }
        let version = PolicyVersion {
            yaml: serde_yaml::to_string(&spec)
                .map_err(|_| invalid("cannot canonicalize policy"))?,
            spec,
            digest,
            submitted_by: actor.clone(),
            created_at: now,
        };
        self.commit(Event::PolicyProposed {
            version: Box::new(version.clone()),
        })?;
        Ok(version)
    }
    pub fn activate_policy(
        &mut self,
        actor: &PrincipalId,
        id: &str,
        expected_revision: u64,
    ) -> Result<(), Error> {
        if self.actor(actor)?.role != Role::Human
            || actor.0 != administrator(&self.state.evidence)
            || !can_inspect(&self.state.evidence, actor)
        {
            return Err(Error::Unauthorized);
        }
        if self.state.evidence.policy.revision != expected_revision {
            return Err(Error::Conflict);
        }
        let version = self.state.policy_versions.get(id).ok_or(Error::NotFound)?;
        let snapshot = compile(
            &self.state.evidence,
            &version.spec,
            &self.state.source_authority,
            &self.state.destination_authority,
        )?;
        self.commit(Event::PolicyActivated {
            snapshot: Box::new(snapshot),
            activated_by: actor.clone(),
        })
    }
}
pub fn administrator(e: &Evidence) -> &'static str {
    if e.workspace.0 == "bank" {
        "arun"
    } else {
        "maya"
    }
}
pub fn can_inspect(e: &Evidence, actor: &PrincipalId) -> bool {
    e.sources
        .values()
        .all(|s| lineage::can_read(e, actor, &s.artifact))
}
pub(crate) fn reduce(state: &mut State, event: &Event) -> Result<(), Error> {
    match event {
        Event::PolicyProposed { version } => {
            if version.digest != digest::structured("policy-package", &version.spec)
                || state.policy_versions.contains_key(&version.spec.id)
                || serde_yaml::to_string(&version.spec).map_err(|_| Error::Conflict)?
                    != version.yaml
                || !can_inspect(&state.evidence, &version.submitted_by)
            {
                return Err(Error::Conflict);
            }
            compile(
                &state.evidence,
                &version.spec,
                &state.source_authority,
                &state.destination_authority,
            )?;
            state
                .policy_versions
                .insert(version.spec.id.clone(), *version.clone());
        }
        Event::PolicyActivated {
            snapshot,
            activated_by,
        } => {
            if activated_by.0 != administrator(&state.evidence)
                || !can_inspect(&state.evidence, activated_by)
                || !state
                    .evidence
                    .principals
                    .get(activated_by)
                    .is_some_and(|p| p.enabled && p.role == Role::Human)
            {
                return Err(Error::Unauthorized);
            }
            let version = state
                .policy_versions
                .get(&snapshot.version)
                .ok_or(Error::Conflict)?;
            let expected = compile(
                &state.evidence,
                &version.spec,
                &state.source_authority,
                &state.destination_authority,
            )?;
            if expected != **snapshot {
                return Err(Error::Conflict);
            }
            apply(&mut state.evidence, snapshot)?;
        }
        _ => return Err(Error::Conflict),
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    pub id: String,
    pub expected: String,
    pub observed: String,
    pub passed: bool,
    pub details: serde_json::Value,
}
#[derive(Debug, Clone, Serialize)]
pub struct PolicyTestReport {
    pub passed: bool,
    pub scope: String,
    pub cases: Vec<CaseResult>,
}
pub fn test_policy(
    e: &Evidence,
    spec: &PolicySpec,
    sources: &BTreeMap<SourceId, Readers>,
    destinations: &BTreeMap<AudienceId, Audience>,
) -> Result<PolicyTestReport, Error> {
    if e.workspace.0 != "bank" {
        return Err(invalid("the five-case suite is scoped to the bank fixture"));
    }
    let snapshot = compile(e, spec, sources, destinations)?;
    let mut candidate = e.clone();
    apply(&mut candidate, &snapshot)?;
    let safe = fixtures::bank_plan("policy-safe", false, "engineering-repo");
    let unsafe_plan = fixtures::bank_plan("policy-private", true, "engineering-repo");
    let agent = PrincipalId::from("agent");
    let allowed = plan::analyze(&candidate, &safe, &agent);
    let blocked = plan::analyze(&candidate, &unsafe_plan, &agent);
    let repaired = plan::repairs(&candidate, &unsafe_plan, &agent)
        .into_iter()
        .any(|r| r.kind == plan::RepairKind::FreshContext && r.report.ready_to_generate);
    let mut stale = false;
    let mut missing = false;
    let mut stale_details = serde_json::json!(null);
    let mut missing_details = serde_json::json!(null);
    if allowed.ready_to_generate {
        let mut w = Coordinator::open_with_evidence(
            crate::journal::MemoryJournal::default(),
            crate::publisher::LocalPublisher::default(),
            candidate.clone(),
        )?;
        w.propose_plan(&agent, safe.clone(), 10)?;
        let run = w.prepare_plan(
            &agent,
            &safe.id,
            &allowed.plan_digest,
            &allowed.snapshot_digest,
            "suite",
            10,
        )?;
        let artifact = w.state().requests[&run.request].artifact().clone();
        let signer = candidate
            .policy
            .approvers
            .iter()
            .find(|s| {
                lineage::can_read(
                    &candidate,
                    s,
                    &candidate.sources[&SourceId::from("engineering-requirements")].artifact,
                )
            })
            .cloned();
        if let Some(signer) = signer {
            let intent = Intent {
                artifact: artifact.clone(),
                audience: "engineering-repo".into(),
                requester: signer.clone(),
            };
            if let Some(context) = verify(&w.state().evidence, &intent, 10).context() {
                let digest = context.digest();
                w.approve(&signer, &intent, &digest, "suite", 10)?;
            }
            let edited = w.edit(
                &agent,
                &artifact,
                "Changed patch after approval",
                "suite-edit",
            )?;
            let changed = Intent {
                artifact: edited,
                audience: "engineering-repo".into(),
                requester: signer.clone(),
            };
            let moved = Intent {
                artifact: artifact.clone(),
                audience: "risk-room".into(),
                requester: signer,
            };
            let changed_decision = verify(&w.state().evidence, &changed, 11);
            let moved_decision = verify(&w.state().evidence, &moved, 11);
            stale = changed_decision.has(Code::StaleApproval)
                && moved_decision.has(Code::StaleApproval);
            stale_details = serde_json::json!({"changed_patch":changed_decision,"changed_destination":moved_decision});
        }
        let mut unknown = w.state().evidence.clone();
        let mut output = unknown.artifacts[&artifact].clone();
        output.origin = Origin::Transformation("missing-manifest".into());
        unknown.artifacts.insert(artifact.clone(), output);
        let decision = verify(
            &unknown,
            &Intent {
                artifact,
                audience: "engineering-repo".into(),
                requester: agent.clone(),
            },
            11,
        );
        missing = decision.has(Code::IncompleteLineage);
        missing_details = serde_json::json!(decision);
    }
    let cases = [
        (
            "allowed",
            "permission pass; human approval required",
            allowed.ready_to_generate,
        ),
        (
            "restricted-source",
            "blocked by Risk-only source",
            blocked
                .conflict
                .as_ref()
                .is_some_and(|c| c.code == plan::PlanCode::AudienceConflict),
        ),
        (
            "fresh-context-repair",
            "recompiled using authorized sources",
            repaired,
        ),
        (
            "stale-approval",
            "changed patch AND destination blocked",
            stale,
        ),
        (
            "unknown-provenance",
            "missing transformation blocked",
            missing,
        ),
    ]
    .into_iter()
    .map(|(id, expected, passed)| CaseResult {
        id: id.into(),
        expected: expected.into(),
        observed: if passed {
            expected.into()
        } else {
            "expected condition was not observed under this policy".into()
        },
        passed,
        details: match id {
            "allowed" => serde_json::json!(allowed),
            "restricted-source" => serde_json::json!(blocked),
            "fresh-context-repair" => {
                serde_json::json!(plan::repairs(&candidate, &unsafe_plan, &agent))
            }
            "stale-approval" => stale_details.clone(),
            _ => missing_details.clone(),
        },
    })
    .collect::<Vec<_>>();
    Ok(PolicyTestReport{passed:cases.iter().all(|c|c.passed),scope:"Deterministic bank fixture; local generation; no GitHub or model calls. Stale checks exercise the actual verifier; no claim of financial/code correctness.".into(),cases})
}
