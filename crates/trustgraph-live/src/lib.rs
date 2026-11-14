mod github;
mod hydra;
mod runner;
mod server;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub use server::router;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use trustgraph_core::{
    digest::{self, Digest},
    *,
};
use trustgraph_runtime::{Coordinator, journal::DiskJournal};
use uuid::Uuid;
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
fn err(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    message.to_owned().into()
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub directory: PathBuf,
    pub context_repository: PathBuf,
    pub context_files: Vec<String>,
    pub target_repository: PathBuf,
    pub github_repository: String,
    #[serde(default)]
    pub departments: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkPolicy {
    pub name: String,
    pub allowed_paths: Vec<String>,
    pub max_changed_files: usize,
    pub require_tests: bool,
    pub approval_minutes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub departments: Option<DepartmentPolicy>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepartmentPolicy {
    pub investigation_readers: Vec<String>,
}
impl Default for WorkPolicy {
    fn default() -> Self {
        Self {
            name: "Reviewed Rust changes".into(),
            allowed_paths: vec!["src/lib.rs".into()],
            max_changed_files: 1,
            require_tests: true,
            approval_minutes: 30,
            departments: None,
        }
    }
}
impl WorkPolicy {
    fn validate(&self) -> Result<()> {
        if let Some(policy) = &self.departments
            && (!policy.investigation_readers.iter().any(|r| r == "risk")
                || policy
                    .investigation_readers
                    .iter()
                    .any(|r| !["risk", "engineering"].contains(&r.as_str()))
                || policy
                    .investigation_readers
                    .iter()
                    .collect::<BTreeSet<_>>()
                    .len()
                    != policy.investigation_readers.len())
        {
            return Err(err(
                "investigation readers must include its Risk owner; supported audiences are risk and engineering",
            ));
        }
        if self.name.trim().is_empty()
            || self.name.len() > 100
            || !(1..=10).contains(&self.max_changed_files)
            || !(1..=1440).contains(&self.approval_minutes)
            || self.allowed_paths.is_empty()
            || self.allowed_paths.len() > 16
            || self.allowed_paths.iter().any(|p| !safe_path(p))
            || self.allowed_paths.iter().collect::<BTreeSet<_>>().len() != self.allowed_paths.len()
        {
            return Err(err(
                "policy needs explicit relative paths, 1..10 files and 1..1440 approval minutes",
            ));
        }
        Ok(())
    }
    fn validate_live(&self) -> Result<()> {
        self.validate()?;
        if self
            .allowed_paths
            .iter()
            .any(|p| !["src/lib.rs", "README.md"].contains(&p.as_str()))
        {
            return Err(err(
                "the live workbench supports src/lib.rs and README.md; other relative paths can be tested in the simulator",
            ));
        }
        Ok(())
    }
    fn violations(&self, files: &[ChangedFile], checks: Option<&CheckReceipt>) -> Vec<String> {
        let mut v = Vec::new();
        if files.is_empty() || files.len() > self.max_changed_files {
            v.push(format!(
                "policy allows 1..{} changed files",
                self.max_changed_files
            ));
        }
        let mut seen = BTreeSet::new();
        for f in files {
            if !safe_path(&f.path)
                || !self.allowed_paths.contains(&f.path)
                || !seen.insert(&f.path)
                || f.content.len() > 65_536
            {
                v.push(format!("path is outside policy or duplicated: {}", f.path));
            }
        }
        if self.require_tests && checks.is_some_and(|c| !c.passed) {
            v.push("required Rust tests did not pass".into());
        }
        v
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangedFile {
    pub path: String,
    pub content: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentOutput {
    pub summary: String,
    pub files: Vec<ChangedFile>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReceipt {
    pub patch_digest: Digest,
    pub passed: bool,
    pub command: String,
    pub output: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSet {
    pub repository: String,
    pub base_commit: String,
    pub summary: String,
    pub files: Vec<ChangedFile>,
    pub checks: CheckReceipt,
    pub policy: WorkPolicy,
}
fn patch_digest(repo: &str, base: &str, files: &[ChangedFile]) -> Digest {
    digest::structured("checked-patch", &(repo, base, files))
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SourceRecord {
    id: ArtifactId,
    path: String,
    hash: Digest,
    graph_id: String,
    #[serde(default)]
    knowledge_status: String,
    #[serde(default)]
    restricted: bool,
    #[serde(default)]
    permissions_graph: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Job {
    id: String,
    goal: String,
    status: String,
    created_at: u64,
    artifact: Option<ArtifactId>,
    change: Option<ChangeSet>,
    error: Option<String>,
    provider: Value,
    graph_id: Option<String>,
    #[serde(default)]
    graph_edges: BTreeMap<String, Vec<String>>,
    base_files: Vec<ChangedFile>,
    input_ids: Vec<ArtifactId>,
    operation: Option<String>,
    #[serde(default)]
    events: Vec<Value>,
    #[serde(default)]
    plan: Value,
    #[serde(default)]
    approval_graph: Option<String>,
    #[serde(default)]
    authorization_graph: Option<String>,
    #[serde(default)]
    include_risk: bool,
    #[serde(default)]
    repair_of: Option<String>,
}
#[derive(Serialize, Deserialize, Default)]
struct Saved {
    #[serde(default)]
    admitted_work_orders: BTreeSet<String>,
    jobs: Vec<Job>,
    policy: WorkPolicy,
    sources: Vec<SourceRecord>,
}
type Gate = Coordinator<DiskJournal, github::GitHub>;
struct Workspace {
    config: Config,
    gate: Gate,
    saved: Saved,
    hydra: hydra::Hydra,
    owner_token: String,
    agent_token: String,
}
type Shared = Arc<Mutex<Workspace>>;
fn private_file(path: &Path) -> Result<File> {
    let mut o = OpenOptions::new();
    o.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    Ok(o.open(path)?)
}
fn atomic(path: &Path, body: &impl Serialize) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| err("invalid persistence path"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".write-{}", Uuid::new_v4()));
    let mut f = private_file(&temp)?;
    f.write_all(&serde_json::to_vec(body)?)?;
    f.sync_all()?;
    fs::rename(temp, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
fn token(path: &Path) -> Result<String> {
    if path.exists() {
        let t = fs::read_to_string(path)?;
        if t.len() != 64 {
            return Err(err("invalid local token file"));
        }
        return Ok(t);
    }
    use std::io::Write;
    let t = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let mut f = private_file(path)?;
    f.write_all(t.as_bytes())?;
    f.sync_all()?;
    Ok(t)
}
fn safe_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() < 160
        && !p.starts_with('/')
        && !p
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == ".." || s.starts_with('.'))
        && p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_/ .".contains(&b))
}
fn git_head(path: &Path) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if !out.status.success() {
        return Err(err("target repository has no committed base"));
    }
    Ok(String::from_utf8(out.stdout)?.trim().into())
}
fn git_file(path: &Path, base: &str, name: &str) -> Result<String> {
    if !safe_path(name) {
        return Err(err("source path is not a supported relative file"));
    }
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["show", &format!("{base}:{name}")])
        .output()?;
    if !out.status.success() || out.stdout.len() > 65_536 {
        return Err(err("committed source is unavailable or too large"));
    }
    Ok(String::from_utf8(out.stdout)?)
}
fn genesis(repo: &str) -> Evidence {
    let w: WorkspaceId = "repository-work".into();
    Evidence {
        workspace: w.clone(),
        complete: true,
        principals: [("owner", Role::Human), ("agent", Role::Agent)]
            .into_iter()
            .map(|(id, role)| {
                (
                    id.into(),
                    Principal {
                        id: id.into(),
                        workspace: w.clone(),
                        role,
                        enabled: true,
                    },
                )
            })
            .collect(),
        artifacts: BTreeMap::new(),
        sources: BTreeMap::new(),
        acls: BTreeMap::new(),
        transformations: BTreeMap::new(),
        audiences: BTreeMap::from([(
            "repository".into(),
            Audience {
                id: "repository".into(),
                workspace: w.clone(),
                destination: format!("github://{repo}/pulls"),
                revision: 1,
                recipients: BTreeSet::from(["owner".into()]),
                resolved: true,
            },
        )]),
        policy: Policy {
            workspace: w,
            revision: 1,
            approvers: BTreeSet::from(["owner".into()]),
            max_approval_lifetime: 1800,
            configuration: Some(digest::structured("work-policy", &WorkPolicy::default())),
        },
        approvals: BTreeMap::new(),
        revoked: BTreeSet::new(),
    }
}
fn department_policy() -> WorkPolicy {
    WorkPolicy {
        name: "Engineering and Risk sharing".into(),
        departments: Some(DepartmentPolicy {
            investigation_readers: vec!["risk".into()],
        }),
        ..WorkPolicy::default()
    }
}
fn genesis_for(repo: &str, departments: bool) -> Evidence {
    let mut evidence = genesis(repo);
    if departments {
        let workspace: WorkspaceId = "bank-departments".into();
        evidence.workspace = workspace.clone();
        evidence.policy.workspace = workspace.clone();
        for principal in evidence.principals.values_mut() {
            principal.workspace = workspace.clone();
        }
        for audience in evidence.audiences.values_mut() {
            audience.workspace = workspace.clone();
        }
        evidence.principals.insert(
            "engineering".into(),
            Principal {
                id: "engineering".into(),
                workspace: workspace.clone(),
                role: Role::Human,
                enabled: true,
            },
        );
        evidence
            .audiences
            .get_mut(&AudienceId::from("repository"))
            .expect("repository")
            .recipients = BTreeSet::from(["engineering".into()]);
        evidence.audiences.insert(
            "risk-room".into(),
            Audience {
                id: "risk-room".into(),
                workspace,
                destination: "risk://internal-review".into(),
                revision: 1,
                recipients: BTreeSet::from(["owner".into()]),
                resolved: true,
            },
        );
        evidence.policy.configuration =
            Some(digest::structured("work-policy", &department_policy()));
    }
    evidence
}
fn source_readers(policy: &WorkPolicy, restricted: bool) -> Readers {
    let mut readers = BTreeSet::from(["owner".into(), "agent".into()]);
    if policy
        .departments
        .as_ref()
        .is_some_and(|p| !restricted || p.investigation_readers.iter().any(|r| r == "engineering"))
    {
        readers.insert("engineering".into());
    }
    readers
}
fn apply_source_policy(gate: &mut Gate, source: &SourceRecord, policy: &WorkPolicy) -> Result<()> {
    let current = gate
        .state()
        .evidence
        .sources
        .get(&SourceId(source.id.0.clone()))
        .ok_or_else(|| err("source missing"))?;
    let readers = source_readers(policy, source.restricted);
    if gate.state().evidence.acls[&current.acl].readers == readers {
        return Ok(());
    }
    let source_id = current.id.clone();
    let key = digest::structured(
        "source-permission",
        &(
            source_id.clone(),
            policy,
            gate.state().evidence.policy.revision,
        ),
    );
    gate.change_acl(&"owner".into(), &source_id, readers, &key.0)?;
    Ok(())
}
fn source_permission(gate: &Gate, id: &ArtifactId) -> Result<Value> {
    let source = gate
        .state()
        .evidence
        .sources
        .get(&SourceId(id.0.clone()))
        .ok_or_else(|| err("source missing"))?;
    Ok(json!({"source":source,"acl":gate.state().evidence.acls[&source.acl]}))
}
impl Workspace {
    fn save(&self) -> Result<()> {
        atomic(&self.config.directory.join("work.json"), &self.saved)
    }
    fn intent(&self, id: &ArtifactId, actor: &str) -> Intent {
        Intent {
            artifact: id.clone(),
            audience: "repository".into(),
            requester: actor.into(),
        }
    }
    fn import(&mut self, path: &str, content: String) -> Result<Artifact> {
        let id = ArtifactId(format!(
            "source-{}",
            digest::structured("imported-source", &(path, &content)).0
        ));
        self.gate.import_source(
            &"owner".into(),
            id.clone(),
            content,
            if self.config.departments {
                BTreeSet::from(["owner".into(), "engineering".into(), "agent".into()])
            } else {
                BTreeSet::from(["owner".into(), "agent".into()])
            },
        )?;
        Ok(self.gate.state().evidence.artifacts[&id].clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trustgraph_runtime::{
        journal::MemoryJournal,
        publisher::{Acknowledgment, Outcome, Payload, Publisher},
    };
    #[derive(Default)]
    struct ControlledPublisher {
        calls: usize,
    }
    impl Publisher for ControlledPublisher {
        fn simulated(&self) -> bool {
            false
        }
        fn send(
            &mut self,
            p: &Payload,
        ) -> std::result::Result<(Outcome, Option<Acknowledgment>), trustgraph_runtime::Error>
        {
            self.calls += 1;
            Ok((
                Outcome::Accepted,
                Some(Acknowledgment {
                    provider_id: "provider-receipt".into(),
                    payload: p.clone(),
                }),
            ))
        }
        fn lookup(
            &mut self,
            _: &str,
        ) -> std::result::Result<Option<Acknowledgment>, trustgraph_runtime::Error> {
            Ok(None)
        }
    }
    #[test]
    fn business_policy_tests_paths_counts_and_required_checks() {
        let policy = WorkPolicy::default();
        let files = vec![ChangedFile {
            path: "src/lib.rs".into(),
            content: "pub fn amount() {}".into(),
        }];
        let receipt = CheckReceipt {
            patch_digest: patch_digest("owner/repo", "base", &files),
            passed: false,
            command: "rustc --test".into(),
            output: "failed".into(),
        };
        assert!(
            policy
                .violations(&files, Some(&receipt))
                .iter()
                .any(|s| s.contains("tests"))
        );
        let mut optional = policy.clone();
        optional.require_tests = false;
        assert!(optional.violations(&files, Some(&receipt)).is_empty());
        optional.allowed_paths = vec!["README.md".into()];
        assert!(!optional.violations(&files, Some(&receipt)).is_empty());
        assert_ne!(
            patch_digest("owner/repo", "base", &files),
            patch_digest("owner/other", "base", &files)
        );
        assert_ne!(
            patch_digest("owner/repo", "base", &files),
            patch_digest("owner/repo", "changed-base", &files)
        );
        for path in ["../auth.json", "/etc/passwd", "src/../../private", ".env"] {
            assert!(!safe_path(path));
        }
    }
    #[test]
    fn trusted_import_and_capture_replay_and_policy_change_stops_effects() {
        let mut w = Coordinator::open_with_evidence(
            MemoryJournal::default(),
            ControlledPublisher::default(),
            genesis("owner/repo"),
        )
        .unwrap();
        let readers = BTreeSet::from(["owner".into(), "agent".into()]);
        assert!(
            w.import_source(
                &"agent".into(),
                "src".into(),
                "source bytes".into(),
                readers.clone()
            )
            .is_err()
        );
        let source = w
            .import_source(
                &"owner".into(),
                "src".into(),
                "source bytes".into(),
                readers.clone(),
            )
            .unwrap();
        assert!(
            w.import_source(
                &"owner".into(),
                "src".into(),
                "different bytes".into(),
                readers
            )
            .is_err()
        );
        let artifact = w
            .record_broker_output(
                &"agent".into(),
                "patch".into(),
                vec![source],
                "changed file bytes".into(),
                "captured-test",
            )
            .unwrap();
        let intent = Intent {
            artifact,
            audience: "repository".into(),
            requester: "owner".into(),
        };
        let digest = verify(&w.state().evidence, &intent, 1)
            .context()
            .unwrap()
            .digest();
        assert!(
            w.approve(&"agent".into(), &intent, &digest, "fake", 1)
                .is_err()
        );
        w.approve(&"owner".into(), &intent, &digest, "review", 1)
            .unwrap();
        w.set_policy_configuration(
            &"owner".into(),
            digest::content(b"new business conditions"),
            900,
        )
        .unwrap();
        assert!(matches!(
            verify(&w.state().evidence, &intent, 2),
            Decision::Blocked { .. }
        ));
        let result = w.publish(&intent, &digest, "no-effect", 2).unwrap();
        assert!(matches!(
            result,
            trustgraph_runtime::PublishResult::Blocked { .. }
        ));
        assert_eq!(w.publisher.calls, 0);
        let restored = Coordinator::open_with_evidence(
            w.journal.clone(),
            ControlledPublisher::default(),
            genesis("owner/repo"),
        )
        .unwrap();
        assert_eq!(restored.state().evidence, w.state().evidence);
    }
    #[test]
    fn non_simulated_provider_receipts_are_labeled_correctly() {
        let mut w = Coordinator::open_with_evidence(
            MemoryJournal::default(),
            ControlledPublisher::default(),
            genesis("owner/repo"),
        )
        .unwrap();
        let source = w
            .import_source(
                &"owner".into(),
                "src".into(),
                "source bytes".into(),
                BTreeSet::from(["owner".into(), "agent".into()]),
            )
            .unwrap();
        let artifact = w
            .record_broker_output(
                &"agent".into(),
                "patch".into(),
                vec![source],
                "exact bytes".into(),
                "captured-test",
            )
            .unwrap();
        let intent = Intent {
            artifact,
            audience: "repository".into(),
            requester: "owner".into(),
        };
        let digest = verify(&w.state().evidence, &intent, 1)
            .context()
            .unwrap()
            .digest();
        w.approve(&"owner".into(), &intent, &digest, "review", 1)
            .unwrap();
        let trustgraph_runtime::PublishResult::Operation { operation } =
            w.publish(&intent, &digest, "effect", 2).unwrap()
        else {
            panic!("publish stopped")
        };
        assert!(!operation.receipt.unwrap().simulated);
    }
    #[test]
    #[cfg(target_os = "macos")]
    fn rust_tests_run_without_home_file_or_network_access() {
        let dir = tempfile::tempdir().unwrap();
        let private = PathBuf::from(std::env::var("HOME").unwrap()).join(".hydradb/config.json");
        let content = format!(
            "#[test] fn isolation() {{assert!(std::fs::read_to_string({:?}).is_err());assert!(std::net::TcpStream::connect(\"127.0.0.1:8080\").is_err());}}",
            private.to_str().unwrap()
        );
        let files = vec![ChangedFile {
            path: "src/lib.rs".into(),
            content,
        }];
        let hash = patch_digest("owner/repo", "base", &files);
        let receipt = runner::test(&files, &[], dir.path(), &hash).unwrap();
        assert!(receipt.passed, "{}", receipt.output);
        assert_eq!(receipt.patch_digest, hash);
    }
}
