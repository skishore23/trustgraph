//! One continuous, deterministic governance story. All material is fictional.
//! The ordinary policy compiler, coordinator and verifier make every decision.
//! This session has only an in-memory simulated publisher, never credentials.
use crate::{
    Coordinator, Error,
    journal::MemoryJournal,
    policy::{self, DestinationRule, PolicySpec, ReviewRule, SourceRule},
    publisher::LocalPublisher,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use trustgraph_core::{digest, lineage, verifier::publication_binding, *};

type World = Coordinator<MemoryJournal, LocalPublisher>;
const CLOCK: u64 = 1_800_000_000;
const DEPARTMENTS: [&str; 3] = ["hr", "finance", "engineering"];
const PEOPLE: [(&str, &str, &str); 4] = [
    ("nina", "Nina", "HR"),
    ("leo", "Leo", "Finance"),
    ("sam", "Sam", "Engineering"),
    ("maya", "Maya", "Governance"),
];
const SOURCES: [(&str, &str, &str, &str); 4] = [
    (
        "directory",
        "Organization directory",
        "Company",
        "Departments, roles and headcounts. No compensation data.",
    ),
    (
        "compensation",
        "Employee compensation",
        "HR",
        "Individual salaries and planned salary adjustments. HR only.",
    ),
    (
        "budget",
        "Department budgets",
        "Finance",
        "Department budgets, shared with HR for planning.",
    ),
    (
        "approved-planning",
        "Shared planning data",
        "HR",
        "Headcounts and budget bands HR shares with Finance and Engineering. No individual salaries.",
    ),
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoryPolicy {
    pub name: String,
    pub source_readers: BTreeMap<String, BTreeSet<String>>,
    pub approvers: BTreeSet<String>,
    pub approval_minutes: u64,
}
impl Default for StoryPolicy {
    fn default() -> Self {
        Self {
            name: "Department boundaries".into(),
            source_readers: [
                ("directory", vec!["hr", "finance", "engineering"]),
                ("compensation", vec!["hr"]),
                ("budget", vec!["hr", "finance"]),
                ("approved-planning", vec!["hr", "finance", "engineering"]),
            ]
            .into_iter()
            .map(|(id, groups)| (id.into(), groups.into_iter().map(str::to_owned).collect()))
            .collect(),
            approvers: ["maya", "nina", "leo", "sam"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            approval_minutes: 30,
        }
    }
}
impl StoryPolicy {
    fn spec(&self) -> Result<PolicySpec, Error> {
        if self.name.trim().is_empty()
            || self.name.len() > 80
            || !(1..=1440).contains(&self.approval_minutes)
            || self
                .source_readers
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != SOURCES.iter().map(|s| s.0).collect()
            || self.source_readers.values().any(|groups| {
                groups.is_empty() || groups.iter().any(|g| !DEPARTMENTS.contains(&g.as_str()))
            })
            || self.approvers.is_empty()
            || self
                .approvers
                .iter()
                .any(|id| !PEOPLE.iter().any(|p| p.0 == id))
        {
            return Err(Error::Invalid("Use the four known sources, known departments, human reviewers and a 1–1440 minute approval lifetime.".into()));
        }
        let mut groups: BTreeMap<String, Readers> = [
            ("hr", "nina"),
            ("finance", "leo"),
            ("engineering", "sam"),
            ("governance", "maya"),
            ("generation", "agent"),
        ]
        .into_iter()
        .map(|(g, p)| (g.into(), [p.into()].into_iter().collect()))
        .collect();
        groups.insert(
            "company".into(),
            ["nina", "leo", "sam", "maya", "agent"]
                .into_iter()
                .map(PrincipalId::from)
                .collect(),
        );
        let mut sources: BTreeMap<SourceId, SourceRule> = self
            .source_readers
            .iter()
            .map(|(id, readers)| {
                let mut readers = readers.clone();
                readers.extend(["governance".into(), "generation".into()]);
                (SourceId(id.clone()), SourceRule { readers })
            })
            .collect();
        sources.insert(
            "task-prompt".into(),
            SourceRule {
                readers: ["company".into()].into_iter().collect(),
            },
        );
        Ok(PolicySpec {
            schema_version: 1,
            id: format!(
                "story-policy-{}",
                &digest::structured("story-policy", self).0[..16]
            ),
            name: self.name.clone(),
            groups,
            sources,
            destinations: DEPARTMENTS
                .into_iter()
                .map(|id| {
                    (
                        AudienceId(id.into()),
                        DestinationRule {
                            audience: [id.to_owned()].into_iter().collect(),
                            requires_approval: true,
                        },
                    )
                })
                .collect(),
            review: ReviewRule {
                approvers: self.approvers.iter().cloned().map(PrincipalId).collect(),
                max_age_seconds: self.approval_minutes * 60,
            },
            rules: policy::RULES.into_iter().map(str::to_owned).collect(),
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub inputs: BTreeSet<String>,
    pub destination: String,
    pub approval: ApprovalCondition,
    #[serde(default)]
    pub approval_age_minutes: u64,
    pub complete_lineage: bool,
}
impl Default for Scenario {
    fn default() -> Self {
        Self {
            inputs: ["directory", "compensation", "budget"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            destination: "engineering".into(),
            approval: ApprovalCondition::Missing,
            approval_age_minutes: 0,
            complete_lineage: true,
        }
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalCondition {
    Missing,
    Valid,
    Changed,
    Expired,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Activate {
        policy: StoryPolicy,
        expected_revision: u64,
    },
    Generate {
        approved_inputs: bool,
        expected_revision: u64,
    },
    Approve {
        reviewer: String,
        artifact: ArtifactId,
        expected: digest::Digest,
    },
    Release {
        artifact: ArtifactId,
        expected: digest::Digest,
    },
    Revise {
        artifact: ArtifactId,
    },
}
#[derive(Clone, Serialize)]
struct WorkVersion {
    version: usize,
    artifact: ArtifactId,
    inputs: Vec<ArtifactId>,
    fresh_context: bool,
    event: String,
    operation: Option<String>,
}

pub struct Story {
    world: World,
    active: StoryPolicy,
    versions: Vec<WorkVersion>,
    events: Vec<String>,
    policy_history: Vec<Value>,
}
fn genesis() -> Evidence {
    let workspace: WorkspaceId = "northstar-company-story".into();
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
            approvers: PEOPLE.iter().map(|p| PrincipalId::from(p.0)).collect(),
            max_approval_lifetime: 1800,
            configuration: None,
        },
        approvals: BTreeMap::new(),
        revoked: BTreeSet::new(),
    };
    for id in PEOPLE.iter().map(|p| p.0).chain(["agent"]) {
        let role = if id == "agent" {
            Role::Agent
        } else {
            Role::Human
        };
        let id: PrincipalId = id.into();
        e.principals.insert(
            id.clone(),
            Principal {
                id,
                workspace: workspace.clone(),
                role,
                enabled: true,
            },
        );
    }
    let all: Readers = e.principals.keys().cloned().collect();
    for (id, _, _, content) in SOURCES.into_iter().chain([("task-prompt", "Work order", "Company", "Build a headcount planning dashboard for Engineering. Capture all inputs; route release through exact human approval.")]) {
        let artifact = Artifact::new(id.into(), workspace.clone(), content.into(), Origin::Source(id.into()));
        let acl = Acl { id: AclId(format!("initial-{id}")), workspace: workspace.clone(), revision: 1, readers: all.clone() };
        e.sources.insert(id.into(), Source { id: id.into(), workspace: workspace.clone(), artifact: artifact.id.clone(), acl: acl.id.clone() });
        e.acls.insert(acl.id.clone(), acl);
        e.artifacts.insert(artifact.id.clone(), artifact);
    }
    for (id, person) in [("hr", "nina"), ("finance", "leo"), ("engineering", "sam")] {
        e.audiences.insert(
            id.into(),
            Audience {
                id: id.into(),
                workspace: workspace.clone(),
                destination: format!("sim://northstar/{id}"),
                revision: 1,
                recipients: [person.into()].into_iter().collect(),
                resolved: true,
            },
        );
    }
    e
}

fn apply(world: &mut World, policy: &StoryPolicy) -> Result<(), Error> {
    let spec = policy.spec()?;
    let version = world.propose_policy(
        &"maya".into(),
        &serde_json::to_string(&spec).map_err(|e| Error::Invalid(e.to_string()))?,
        CLOCK,
    )?;
    let revision = world.state().evidence.policy.revision;
    world.activate_policy(&"maya".into(), &version.spec.id, revision)
}
fn intent(id: &ArtifactId, destination: &str) -> Intent {
    Intent {
        artifact: id.clone(),
        audience: destination.into(),
        requester: "agent".into(),
    }
}
fn content(approved: bool) -> String {
    format!(
        "// Headcount planning dashboard\n// {}\nexport const planning = [\n  {{ department: 'Engineering', headcount: 42, budgetBand: 'On plan' }},\n  {{ department: 'Finance', headcount: 12, budgetBand: 'On plan' }},\n];\n{}",
        if approved {
            "Inputs: organization directory + shared planning data"
        } else {
            "Inputs: organization directory + private compensation + department budgets"
        },
        if approved {
            ""
        } else {
            "\n// Individual compensation from the captured HR input\nexport const compensation = [{ employee: 'Employee 104', salary: 145000 }];\n"
        }
    )
}
fn input_ids(approved: bool) -> Vec<ArtifactId> {
    if approved {
        vec![
            "directory".into(),
            "approved-planning".into(),
            "task-prompt".into(),
        ]
    } else {
        vec![
            "directory".into(),
            "compensation".into(),
            "budget".into(),
            "task-prompt".into(),
        ]
    }
}
fn source_ids(e: &Evidence, id: &ArtifactId, sources: &mut BTreeSet<SourceId>) {
    if let Some(a) = e.artifacts.get(id) {
        match &a.origin {
            Origin::Source(s) => {
                sources.insert(s.clone());
            }
            Origin::Transformation(t) => {
                if let Some(t) = e.transformations.get(t) {
                    for input in &t.manifest.inputs {
                        source_ids(e, &input.artifact, sources);
                    }
                }
            }
        }
    }
}
fn assessment(e: &Evidence, id: &ArtifactId, destination: &str, now: u64) -> Value {
    let intent = intent(id, destination);
    let decision = verify(e, &intent, now);
    let mut without_approval = e.clone();
    without_approval.approvals.clear();
    let information_permitted = matches!(
        verify(&without_approval, &intent, now),
        Decision::PendingApproval { .. }
    );
    let (status, code) = match &decision {
        Decision::Ready { .. } => ("ready", "READY".to_owned()),
        Decision::PendingApproval { .. } => ("needs_approval", "APPROVAL_REQUIRED".to_owned()),
        Decision::Blocked { violations } => (
            "blocked",
            serde_json::to_value(violations[0].code)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned(),
        ),
    };
    let mut ancestors = BTreeSet::new();
    source_ids(e, id, &mut ancestors);
    let conflicts: Vec<_> = ancestors
        .iter()
        .filter(|s| {
            let source = &e.sources[*s];
            !e.audiences[&intent.audience]
                .recipients
                .is_subset(&e.acls[&source.acl].readers)
        })
        .map(|s| s.0.clone())
        .collect();
    let permitted_departments: Vec<_> = lineage::derived_readers(e, id)
        .map(|readers| {
            DEPARTMENTS
                .into_iter()
                .filter(|d| {
                    e.audiences[&AudienceId::from(*d)]
                        .recipients
                        .is_subset(&readers)
                })
                .collect()
        })
        .unwrap_or_default();
    let context = publication_binding(e, &intent).ok().map(|c| c.digest());
    let explanation = match code.as_str() {
        "READY" => "The selected department can access this work, and this version has approval.",
        "APPROVAL_REQUIRED" => {
            "The selected department can access this work. A human must approve this version before it can be published."
        }
        "RESTRICTED_SOURCE" => {
            "The selected department cannot access some data used to create this version. Human approval cannot grant that access."
        }
        "STALE_APPROVAL" => {
            "The saved approval covers a different version or policy context. Review the current version again."
        }
        "APPROVAL_EXPIRED" => {
            "The approval is older than the policy allows. Obtain a new approval for this exact version."
        }
        "INCOMPLETE_LINEAGE" => {
            "Some inputs were not recorded. Publishing is blocked until the input record is complete."
        }
        "SIGNER_UNAUTHORIZED" => {
            "The reviewer is not eligible under the current policy and source permissions."
        }
        _ => "The Rust verifier blocked this release. Inspect the decision evidence.",
    };
    json!({"status":status,"code":code,"explanation":explanation,"information_permitted":information_permitted,
        "approval_valid":matches!(decision,Decision::Ready{..}),"conflicting_sources":conflicts,
        "permitted_departments":permitted_departments,"context_digest":context,"decision":decision})
}
impl Story {
    pub fn new() -> Result<Self, Error> {
        let active = StoryPolicy::default();
        let mut world = Coordinator::open_with_evidence(
            MemoryJournal::default(),
            LocalPublisher::default(),
            genesis(),
        )?;
        apply(&mut world, &active)?;
        Ok(Self {
            world,
            active: active.clone(),
            versions: vec![],
            events: vec![
                "Department boundaries policy activated. No work has been generated.".into(),
            ],
            policy_history: vec![json!({"revision":1,"policy":active})],
        })
    }
    pub fn view(&self) -> Value {
        let e = &self.world.state().evidence;
        let current = self.versions.last().map(|v| {
            let a = &e.artifacts[&v.artifact];
            let reviewers: Vec<_> = PEOPLE.iter().filter(|p| e.policy.approvers.contains(&PrincipalId::from(p.0)) && lineage::can_read(e, &PrincipalId::from(p.0), &v.artifact)).map(|p| p.0).collect();
            json!({"version":v.version,"artifact":a,"inputs":v.inputs,"fresh_context":v.fresh_context,
                "assessment":assessment(e,&v.artifact,"engineering",CLOCK),"eligible_reviewers":reviewers,
                "operation":v.operation.as_ref().and_then(|key|self.world.state().operations.get(key))})
        });
        let versions: Vec<_> = self
            .versions
            .iter()
            .map(|v| {
                json!({"version":v.version,"artifact":v.artifact,
            "hash":e.artifacts[&v.artifact].hash,"fresh_context":v.fresh_context,"event":v.event,
            "assessment":assessment(e,&v.artifact,"engineering",CLOCK),
            "operation":v.operation.as_ref().and_then(|key|self.world.state().operations.get(key))})
            })
            .collect();
        json!({"company":"Northstar","work_id":"TG-104","policy":self.active,"policy_revision":e.policy.revision-1,
            "sources":SOURCES.iter().map(|(id,title,owner,description)|json!({"id":id,"title":title,"owner":owner,"description":description})).collect::<Vec<_>>(),
            "people":PEOPLE.iter().map(|(id,name,department)|json!({"id":id,"name":name,"department":department})).collect::<Vec<_>>(),
            "current":current,"versions":versions,"events":self.events,"policy_history":self.policy_history,
            "policy_spec":self.active.spec().ok(),"simulated":true,"external_writes":0,"clock":CLOCK})
    }
    pub fn compare(&self, candidate: &StoryPolicy, case: &Scenario) -> Result<Value, Error> {
        candidate.spec()?;
        if case.inputs.is_empty()
            || case
                .inputs
                .iter()
                .any(|s| !SOURCES.iter().any(|source| source.0 == s))
            || !DEPARTMENTS.contains(&case.destination.as_str())
            || case.approval_age_minutes > 10080
        {
            return Err(Error::Invalid(
                "Choose known sources, a department and an approval age of at most seven days."
                    .into(),
            ));
        }
        let evaluate = |policy: &StoryPolicy| -> Result<Value, Error> {
            // Rebuild a canonical isolated snapshot: no UUIDs, wall clock or live mutations.
            let mut world = Coordinator::open_with_evidence(
                MemoryJournal::default(),
                LocalPublisher::default(),
                genesis(),
            )?;
            apply(&mut world, policy)?;
            let inputs = case
                .inputs
                .iter()
                .map(|s| ArtifactId(s.clone()))
                .chain(["task-prompt".into()])
                .collect();
            let mut id = world.record_broker_output(
                &"agent".into(),
                "simulation-output".into(),
                inputs,
                "Deterministic hypothetical dashboard".into(),
                "story-simulation-v1",
            )?;
            if !matches!(case.approval, ApprovalCondition::Missing) {
                let age = if matches!(case.approval, ApprovalCondition::Expired) {
                    policy.approval_minutes + 1
                } else {
                    case.approval_age_minutes
                };
                let at = CLOCK.saturating_sub(age * 60);
                let eligible = policy.approvers.iter().find(|person| {
                    lineage::can_read(
                        &world.state().evidence,
                        &PrincipalId((*person).clone()),
                        &id,
                    )
                });
                if let Some(reviewer) = eligible {
                    let intent = intent(&id, &case.destination);
                    let context = verify(&world.state().evidence, &intent, at)
                        .context()
                        .map(Context::digest);
                    if let Some(context) = context {
                        world.approve(
                            &PrincipalId(reviewer.clone()),
                            &intent,
                            &context,
                            "simulation-review",
                            at,
                        )?;
                    }
                }
            }
            if matches!(case.approval, ApprovalCondition::Changed) {
                id = world.edit(
                    &"agent".into(),
                    &id,
                    "Changed hypothetical dashboard",
                    "simulation-edit",
                )?;
            }
            let mut evidence = world.state().evidence.clone();
            if !case.complete_lineage
                && let Origin::Transformation(t) = &evidence.artifacts[&id].origin
            {
                let t = evidence
                    .transformations
                    .get_mut(t)
                    .expect("recorded transformation");
                t.manifest.complete = false;
                t.manifest_hash = digest::structured("context-manifest", &t.manifest);
            }
            Ok(assessment(&evidence, &id, &case.destination, CLOCK))
        };
        let active = evaluate(&self.active)?;
        let proposed = evaluate(candidate)?;
        let fingerprint = digest::structured(
            "story-comparison",
            &(&self.active, candidate, case, CLOCK, &active, &proposed),
        );
        Ok(
            json!({"active":active,"candidate":proposed,"candidate_policy":candidate,"scenario":case,
            "fingerprint":fingerprint,"evaluation_time":CLOCK,"policy_revision":self.world.state().evidence.policy.revision-1,
            "policy_activated":false,"external_writes":0,"generation":"hypothetical","deterministic":true}),
        )
    }
    pub fn execute(&mut self, command: Command) -> Result<Value, Error> {
        match command {
            Command::Activate {
                policy,
                expected_revision,
            } => {
                self.check_revision(expected_revision)?;
                if self.active != policy {
                    apply(&mut self.world, &policy)?;
                    self.active = policy;
                    self.policy_history.push(json!({"revision":self.world.state().evidence.policy.revision-1,"policy":self.active}));
                    self.events.push("Candidate policy activated. All release decisions now use this policy revision.".into());
                }
            }
            Command::Generate {
                approved_inputs,
                expected_revision,
            } => {
                self.check_revision(expected_revision)?;
                let version = self.versions.len() + 1;
                let inputs = input_ids(approved_inputs);
                let id = self.world.record_broker_output(
                    &"agent".into(),
                    ArtifactId(format!("tg104-v{version}")),
                    inputs.clone(),
                    content(approved_inputs),
                    "story-recorded-fixture-v1",
                )?;
                self.versions.push(WorkVersion {
                    version,
                    artifact: id,
                    inputs,
                    fresh_context: true,
                    event: if approved_inputs {
                        "Fresh generation · shared planning data"
                    } else {
                        "Initial generation · private HR and Finance inputs"
                    }
                    .into(),
                    operation: None,
                });
                self.events.push(format!(
                    "TG-104 version {version} generated with a captured input manifest. {}",
                    if approved_inputs {
                        "The private compensation data and earlier draft were excluded."
                    } else {
                        "HR compensation restrictions follow this output."
                    }
                ));
            }
            Command::Approve {
                reviewer,
                artifact,
                expected,
            } => {
                self.check_artifact(&artifact)?;
                let version = self.versions.len();
                self.world.approve(
                    &PrincipalId(reviewer.clone()),
                    &intent(&artifact, "engineering"),
                    &expected,
                    &format!(
                        "tg104-v{version}-{}",
                        &expected.0[..16.min(expected.0.len())]
                    ),
                    CLOCK,
                )?;
                self.events.push(format!("{reviewer} approved the exact TG-104 version {version}, destination and policy context."));
            }
            Command::Release { artifact, expected } => {
                self.check_artifact(&artifact)?;
                let key = format!(
                    "tg104-release-v{}-{}",
                    self.versions.len(),
                    &expected.0[..16.min(expected.0.len())]
                );
                match self.world.publish(
                    &intent(&artifact, "engineering"),
                    &expected,
                    &key,
                    CLOCK,
                )? {
                    crate::PublishResult::Operation { operation }
                        if operation.state == crate::OperationState::Published =>
                    {
                        let v = self.versions.last_mut().expect("current work version");
                        if v.operation.is_none() {
                            self.events.push(format!(
                                "Version {} release recorded for the Engineering destination.",
                                v.version
                            ));
                        }
                        v.operation = Some(key);
                    }
                    _ => return Err(Error::Invalid(
                        "Current permissions and an exact approval are required before release."
                            .into(),
                    )),
                }
            }
            Command::Revise { artifact } => {
                self.check_artifact(&artifact)?;
                if !matches!(
                    verify(
                        &self.world.state().evidence,
                        &intent(&artifact, "engineering"),
                        CLOCK
                    ),
                    Decision::Ready { .. }
                ) {
                    return Err(Error::Invalid(
                        "Approve the current permitted version before demonstrating a revision."
                            .into(),
                    ));
                }
                let version = self.versions.len() + 1;
                let revised_content = format!(
                    "{}\nexport const includeContractors = true;\n",
                    self.world.state().evidence.artifacts[&artifact].content
                );
                let id = self.world.edit(
                    &"agent".into(),
                    &artifact,
                    &revised_content,
                    &format!("tg104-v{version}"),
                )?;
                self.versions.push(WorkVersion {
                    version,
                    artifact: id,
                    inputs: vec![artifact],
                    fresh_context: false,
                    event: "Edited after approval · contractor toggle added".into(),
                    operation: None,
                });
                self.events.push(format!("Version {version} adds a contractor toggle. The previous exact approval does not cover these new bytes."));
            }
        }
        Ok(self.view())
    }
    fn check_revision(&self, expected: u64) -> Result<(), Error> {
        if self.world.state().evidence.policy.revision - 1 != expected {
            Err(Error::Conflict)
        } else {
            Ok(())
        }
    }
    fn check_artifact(&self, id: &ArtifactId) -> Result<(), Error> {
        if self.versions.last().is_some_and(|v| v.artifact == *id) {
            Ok(())
        } else {
            Err(Error::Conflict)
        }
    }
}
