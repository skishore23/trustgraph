use crate::*;
use std::process::Command;
use trustgraph_runtime::publisher::{Acknowledgment, Outcome, Payload, Publisher};

/// Per-call bound for one `gh api` subprocess.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Bound for a whole delivery (pre-checks plus every write).
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(150);
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

fn api_until(
    method: &str,
    path: &str,
    body: Option<Value>,
    deadline: std::time::Instant,
) -> Result<Value> {
    let remaining = deadline
        .saturating_duration_since(std::time::Instant::now())
        .min(CALL_TIMEOUT);
    if remaining.is_zero() {
        return Err(err("GitHub request deadline exceeded"));
    }
    let mut c = Command::new("gh");
    c.args(["api", "--method", method, path]);
    let input = match body {
        Some(body) => {
            c.args(["--input", "-"]);
            Some(serde_json::to_vec(&body)?)
        }
        None => None,
    };
    let (code, stdout) = runner::bounded(&mut c, input, remaining, MAX_RESPONSE)
        .map_err(|_| err("GitHub API request timed out or exceeded its output bound"))?;
    if code != 0 {
        return Err(err(
            "GitHub API request failed; no credential or response body was exposed",
        ));
    }
    Ok(serde_json::from_slice(&stdout)?)
}

/// Agent-written text must not notify people or close issues on GitHub.
/// A zero-width joiner after `@` breaks mentions and team pings; `#` refs keep
/// working as plain links but cannot be combined with closing keywords because
/// the summary is placed inside a fenced block in the body.
pub(crate) fn neutralize_mentions(text: &str) -> String {
    text.replace('@', "@\u{200d}")
}
fn fenced(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}text\n{text}\n{fence}")
}
pub(crate) fn pull_request_text(summary: &str) -> (String, String) {
    let summary = neutralize_mentions(summary);
    (
        summary.chars().take(120).collect(),
        format!("Agent summary (unverified text):\n\n{}", fenced(&summary)),
    )
}

pub struct GitHub {
    repo: String,
    directory: PathBuf,
}
impl GitHub {
    pub fn new(repo: &str, directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(&directory)?;
        Ok(Self {
            repo: repo.into(),
            directory,
        })
    }
    pub fn check_readers(&self) -> Result<()> {
        self.check_readers_until(std::time::Instant::now() + DELIVERY_TIMEOUT)
    }
    fn check_readers_until(&self, deadline: std::time::Instant) -> Result<()> {
        let repo = api_until("GET", &format!("repos/{}", self.repo), None, deadline)?;
        if repo["private"] != true
            || repo["owner"]["login"] != self.repo.split('/').next().unwrap_or("")
        {
            return Err(err(
                "publication is limited to the configured private personal repository",
            ));
        }
        let users = api_until(
            "GET",
            &format!("repos/{}/collaborators?per_page=100", self.repo),
            None,
            deadline,
        )?;
        let owner = self.repo.split('/').next().unwrap_or("");
        if users
            .as_array()
            .is_none_or(|users| users.is_empty() || users.iter().any(|p| p["login"] != owner))
        {
            return Err(err(
                "repository readers changed; refresh the configured audience before publication",
            ));
        }
        Ok(())
    }
    /// Read-only checks of the audience and base, made before any reservation.
    pub fn precheck(&self, base_commit: &str) -> Result<()> {
        let deadline = std::time::Instant::now() + DELIVERY_TIMEOUT;
        self.check_readers_until(deadline)?;
        self.check_base(base_commit, deadline)
    }
    fn check_base(&self, base_commit: &str, deadline: std::time::Instant) -> Result<()> {
        let current = api_until(
            "GET",
            &format!("repos/{}/git/ref/heads/main", self.repo),
            None,
            deadline,
        )?;
        if current["object"]["sha"] != base_commit {
            return Err(err(
                "repository base changed; regenerate and review against the current base",
            ));
        }
        Ok(())
    }
    fn key_path(&self, prefix: &str, key: &str) -> PathBuf {
        self.directory.join(format!(
            "{prefix}-{}.json",
            digest::content(key.as_bytes()).0
        ))
    }
    /// Marker created exclusively before the first GitHub write. Its presence
    /// means a write may have happened, so delivery is never attempted again.
    fn attempt_path(&self, key: &str) -> PathBuf {
        self.key_path("attempt", key)
    }
    /// The reserved payload, staged while the workspace lock is held.
    fn staged_path(&self, key: &str) -> PathBuf {
        self.key_path("staged", key)
    }
    fn ack_path(&self, key: &str) -> PathBuf {
        self.key_path("ack", key)
    }
    fn checked_change(&self, payload: &Payload) -> Result<ChangeSet> {
        let change: ChangeSet = serde_json::from_str(&payload.content)?;
        if payload.hash != digest::content(payload.content.as_bytes())
            || payload.destination != format!("github://{}/pulls", self.repo)
            || change.repository != self.repo
            || change.files.is_empty()
            || change
                .files
                .iter()
                .any(|f| !["src/lib.rs", "README.md"].contains(&f.path.as_str()))
            || (change.policy.require_tests && !change.checks.passed)
            || change.checks.patch_digest
                != patch_digest(&change.repository, &change.base_commit, &change.files)
        {
            return Err(err("publication payload does not match its checked patch"));
        }
        Ok(change)
    }
    /// True when a reserved payload exists, no write has started and no receipt exists.
    pub fn deliverable(&self, key: &str) -> bool {
        self.staged_path(key).exists()
            && !self.attempt_path(key).exists()
            && !self.ack_path(key).exists()
    }
    /// Permanently prevent delivery of an operation whose writes never started.
    /// Fails if a delivery attempt (or receipt) already exists, because GitHub
    /// may then hold the change and a failure cannot be confirmed.
    pub fn cancel_undelivered(&self, key: &str) -> Result<()> {
        use std::io::Write;
        if self.ack_path(key).exists() {
            return Err(err("publication receipt exists; it was delivered"));
        }
        let mut marker = private_file(&self.attempt_path(key)).map_err(|_| {
            err("a GitHub write may have started; failure cannot be confirmed locally")
        })?;
        marker.write_all(br#"{"cancelled_before_delivery":true}"#)?;
        marker.sync_all()?;
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
    /// Perform the GitHub writes for a reserved operation, outside the workspace lock.
    /// Never resends: once the first write may have started, delivery is refused.
    pub fn deliver(&self, key: &str) -> Result<Acknowledgment> {
        if let Some(ack) = self.read_ack(key)? {
            return Ok(ack);
        }
        if self.attempt_path(key).exists() {
            return Err(err(
                "publication was already attempted; its outcome is uncertain and it is never resent",
            ));
        }
        let payload: Payload = serde_json::from_slice(&fs::read(self.staged_path(key))?)?;
        if payload.operation != key {
            return Err(err("reserved payload does not match its operation"));
        }
        let change = self.checked_change(&payload)?;
        let deadline = std::time::Instant::now() + DELIVERY_TIMEOUT;
        self.check_readers_until(deadline)?;
        self.check_base(&change.base_commit, deadline)?;
        {
            use std::io::Write;
            // Exclusive creation: of two concurrent deliveries only one proceeds.
            let mut marker = private_file(&self.attempt_path(key)).map_err(|_| {
                err("publication was already attempted; its outcome is uncertain and it is never resent")
            })?;
            marker.write_all(&serde_json::to_vec(&payload)?)?;
            marker.sync_all()?;
            File::open(&self.directory)?.sync_all()?;
        }
        let commit = api_until(
            "GET",
            &format!("repos/{}/git/commits/{}", self.repo, change.base_commit),
            None,
            deadline,
        )?;
        let mut tree = Vec::new();
        for file in &change.files {
            let blob = api_until(
                "POST",
                &format!("repos/{}/git/blobs", self.repo),
                Some(json!({"content":file.content,"encoding":"utf-8"})),
                deadline,
            )?;
            tree.push(json!({"path":file.path,"mode":"100644","type":"blob","sha":blob["sha"]}));
        }
        let tree = api_until(
            "POST",
            &format!("repos/{}/git/trees", self.repo),
            Some(json!({"base_tree":commit["tree"]["sha"],"tree":tree})),
            deadline,
        )?;
        let commit = api_until(
            "POST",
            &format!("repos/{}/git/commits", self.repo),
            Some(
                json!({"message":format!("TrustGraph reviewed change\n\nOperation: {}\nArtifact: {}",payload.operation,payload.hash.0),"tree":tree["sha"],"parents":[change.base_commit]}),
            ),
            deadline,
        )?;
        let branch = format!("trustgraph/{}", payload.operation);
        api_until(
            "POST",
            &format!("repos/{}/git/refs", self.repo),
            Some(json!({"ref":format!("refs/heads/{branch}"),"sha":commit["sha"]})),
            deadline,
        )?;
        let (title, summary) = pull_request_text(&change.summary);
        let pr = api_until(
            "POST",
            &format!("repos/{}/pulls", self.repo),
            Some(
                json!({"title":title,"body":format!("{summary}\n\nPublished through TrustGraph after exact-context approval.\n\nArtifact: `{}`\nChecks: `{}`",payload.hash.0,change.checks.command),"head":branch,"base":"main","draft":true}),
            ),
            deadline,
        )?;
        let ack = Acknowledgment {
            provider_id: pr["html_url"]
                .as_str()
                .ok_or_else(|| err("GitHub PR receipt missing"))?
                .into(),
            payload,
        };
        atomic(&self.ack_path(key), &ack)?;
        Ok(ack)
    }
    fn read_ack(&self, key: &str) -> Result<Option<Acknowledgment>> {
        let path = self.ack_path(key);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
    }
}
impl Publisher for GitHub {
    fn simulated(&self) -> bool {
        false
    }
    /// Reserve only: validate the payload offline and stage it durably. The
    /// network writes happen in [`GitHub::deliver`] after the workspace lock is
    /// released; the engine records the outcome as unknown until the receipt is
    /// reconciled. A staged or attempted operation is never staged again.
    fn send(
        &mut self,
        payload: &Payload,
    ) -> std::result::Result<(Outcome, Option<Acknowledgment>), trustgraph_runtime::Error> {
        let key = &payload.operation;
        if self.attempt_path(key).exists()
            || self.staged_path(key).exists()
            || self.ack_path(key).exists()
        {
            return Ok((Outcome::Unknown, None));
        }
        if self.checked_change(payload).is_err() {
            return Ok((Outcome::Rejected, None));
        }
        match atomic(&self.staged_path(key), payload) {
            Ok(()) => Ok((Outcome::Unknown, None)),
            Err(_) if self.staged_path(key).exists() => Ok((Outcome::Unknown, None)),
            Err(_) => Ok((Outcome::Rejected, None)),
        }
    }
    fn lookup(
        &mut self,
        key: &str,
    ) -> std::result::Result<Option<Acknowledgment>, trustgraph_runtime::Error> {
        // A recorded receipt is immutable. Missing receipts are never a reason to resend.
        self.read_ack(key)
            .map_err(|_| trustgraph_runtime::Error::RecoveryRequired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agent_summary_cannot_mention_people_or_escape_its_fence() {
        let (title, body) = pull_request_text("Fix for @octocat and @org/team ``` closes #1");
        assert!(title.contains("@\u{200d}octocat"));
        assert!(!title.contains("@octocat"));
        assert!(!body.contains("@octocat") && !body.contains("@org/team"));
        assert!(body.contains("````text\n"));
        assert!(body.trim_end().ends_with("````"));
    }
    #[test]
    fn staging_never_resends_and_requires_a_checked_payload() {
        let dir = tempfile::tempdir().unwrap();
        let mut github = GitHub::new("owner/repo", dir.path().into()).unwrap();
        let bad = Payload {
            operation: "op-1".into(),
            destination: "github://owner/repo/pulls".into(),
            content: "not a change".into(),
            hash: digest::content(b"not a change"),
        };
        assert_eq!(github.send(&bad).unwrap().0, Outcome::Rejected);
        assert!(!github.deliverable("op-1"));
        let files = vec![ChangedFile {
            path: "src/lib.rs".into(),
            content: "pub fn f() {}".into(),
        }];
        let change = ChangeSet {
            repository: "owner/repo".into(),
            base_commit: "base".into(),
            summary: "s".into(),
            checks: CheckReceipt {
                patch_digest: patch_digest("owner/repo", "base", &files),
                passed: true,
                command: "rustc --test".into(),
                output: String::new(),
            },
            files,
            policy: WorkPolicy::default(),
        };
        let content = serde_json::to_string(&change).unwrap();
        let good = Payload {
            operation: "op-2".into(),
            destination: "github://owner/repo/pulls".into(),
            hash: digest::content(content.as_bytes()),
            content,
        };
        assert_eq!(github.send(&good).unwrap().0, Outcome::Unknown);
        assert!(github.deliverable("op-2"));
        assert_eq!(github.send(&good).unwrap().0, Outcome::Unknown);
        // A started attempt is never delivered again and has no receipt.
        fs::write(github.attempt_path("op-2"), b"{}").unwrap();
        assert!(!github.deliverable("op-2"));
        assert!(github.deliver("op-2").is_err());
        assert!(github.lookup("op-2").unwrap().is_none());
        assert!(
            github.cancel_undelivered("op-2").is_err(),
            "a started attempt cannot be confirmed failed"
        );
        let mut later = good.clone();
        later.operation = "op-3".into();
        assert_eq!(github.send(&later).unwrap().0, Outcome::Unknown);
        github.cancel_undelivered("op-3").unwrap();
        assert!(
            !github.deliverable("op-3"),
            "cancelled payload is never delivered"
        );
        assert!(github.deliver("op-3").is_err());
    }
}
