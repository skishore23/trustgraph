use crate::*;
use std::{
    io::Write,
    process::{Command, Stdio},
};
use trustgraph_runtime::publisher::{Acknowledgment, Outcome, Payload, Publisher};

pub fn api(method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let mut c = Command::new("gh");
    c.args(["api", "--method", method, path]);
    if body.is_some() {
        c.args(["--input", "-"]).stdin(Stdio::piped());
    }
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn()?;
    if let Some(body) = body {
        child
            .stdin
            .take()
            .ok_or_else(|| err("GitHub stdin unavailable"))?
            .write_all(&serde_json::to_vec(&body)?)?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(err(
            "GitHub API request failed; no credential or response body was exposed",
        ));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
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
        let repo = api("GET", &format!("repos/{}", self.repo), None)?;
        if repo["private"] != true
            || repo["owner"]["login"] != self.repo.split('/').next().unwrap_or("")
        {
            return Err(err(
                "publication is limited to the configured private personal repository",
            ));
        }
        let users = api(
            "GET",
            &format!("repos/{}/collaborators?per_page=100", self.repo),
            None,
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
    fn attempt_path(&self, key: &str) -> PathBuf {
        self.directory.join(format!(
            "attempt-{}.json",
            digest::content(key.as_bytes()).0
        ))
    }
    fn ack_path(&self, key: &str) -> PathBuf {
        self.directory
            .join(format!("ack-{}.json", digest::content(key.as_bytes()).0))
    }
    fn send_checked(&self, payload: &Payload) -> Result<Acknowledgment> {
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
        self.check_readers()?;
        let current = api(
            "GET",
            &format!("repos/{}/git/ref/heads/main", self.repo),
            None,
        )?;
        if current["object"]["sha"] != change.base_commit {
            return Err(err(
                "repository base changed; regenerate and review against the current base",
            ));
        }
        atomic(&self.attempt_path(&payload.operation), payload)?;
        let commit = api(
            "GET",
            &format!("repos/{}/git/commits/{}", self.repo, change.base_commit),
            None,
        )?;
        let mut tree = Vec::new();
        for file in &change.files {
            let blob = api(
                "POST",
                &format!("repos/{}/git/blobs", self.repo),
                Some(json!({"content":file.content,"encoding":"utf-8"})),
            )?;
            tree.push(json!({"path":file.path,"mode":"100644","type":"blob","sha":blob["sha"]}));
        }
        let tree = api(
            "POST",
            &format!("repos/{}/git/trees", self.repo),
            Some(json!({"base_tree":commit["tree"]["sha"],"tree":tree})),
        )?;
        let commit = api(
            "POST",
            &format!("repos/{}/git/commits", self.repo),
            Some(
                json!({"message":format!("TrustGraph reviewed change\n\nOperation: {}\nArtifact: {}",payload.operation,payload.hash.0),"tree":tree["sha"],"parents":[change.base_commit]}),
            ),
        )?;
        let branch = format!("trustgraph/{}", payload.operation);
        api(
            "POST",
            &format!("repos/{}/git/refs", self.repo),
            Some(json!({"ref":format!("refs/heads/{branch}"),"sha":commit["sha"]})),
        )?;
        let pr = api(
            "POST",
            &format!("repos/{}/pulls", self.repo),
            Some(
                json!({"title":change.summary.chars().take(120).collect::<String>(),"body":format!("{}\n\nPublished through TrustGraph after exact-context approval.\n\nArtifact: `{}`\nChecks: `{}`",change.summary,payload.hash.0,change.checks.command),"head":branch,"base":"main","draft":true}),
            ),
        )?;
        let ack = Acknowledgment {
            provider_id: pr["html_url"]
                .as_str()
                .ok_or_else(|| err("GitHub PR receipt missing"))?
                .into(),
            payload: payload.clone(),
        };
        atomic(&self.ack_path(&payload.operation), &ack)?;
        Ok(ack)
    }
}
impl Publisher for GitHub {
    fn simulated(&self) -> bool {
        false
    }
    fn send(
        &mut self,
        payload: &Payload,
    ) -> std::result::Result<(Outcome, Option<Acknowledgment>), trustgraph_runtime::Error> {
        if self.attempt_path(&payload.operation).exists() {
            return Ok((Outcome::Unknown, None));
        }
        match self.send_checked(payload) {
            Ok(ack) => Ok((Outcome::Accepted, Some(ack))),
            Err(_) => Ok((
                if self.attempt_path(&payload.operation).exists() {
                    Outcome::Unknown
                } else {
                    Outcome::Rejected
                },
                None,
            )),
        }
    }
    fn lookup(
        &mut self,
        key: &str,
    ) -> std::result::Result<Option<Acknowledgment>, trustgraph_runtime::Error> {
        // A recorded receipt is immutable. Missing receipts are never a reason to resend.
        let path = self.ack_path(key);
        if path.exists() {
            return serde_json::from_slice(
                &fs::read(path).map_err(|_| trustgraph_runtime::Error::RecoveryRequired)?,
            )
            .map(Some)
            .map_err(|_| trustgraph_runtime::Error::RecoveryRequired);
        }
        Ok(None)
    }
}
