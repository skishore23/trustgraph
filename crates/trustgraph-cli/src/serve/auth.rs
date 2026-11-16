//! Operator sessions and loopback request checks for `trustgraph serve`.
//!
//! The operator authority is never a long-lived bearer token and never a
//! cookie (cookies ignore ports, so any other server on 127.0.0.1 would
//! receive one). Each server holds a single-use launch secret in memory and
//! writes it only to a private launch file. Exchanging it yields a random
//! session id in the JSON body; the page keeps it in `sessionStorage` (scoped
//! to this origin, port included) and sends it as `X-TrustGraph-Operator`.
//! Only one session is live at a time: a new exchange ends the previous one.
//! Every exchange makes a fresh launch secret for the next sign-in.
use axum::http::{HeaderMap, header};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

/// Default operator session lifetime.
pub const SESSION_LIFETIME: Duration = Duration::from_secs(8 * 60 * 60);
/// The request header that carries the operator session id.
pub const OPERATOR_HEADER: &str = "x-trustgraph-operator";

/// Compare secrets without an early exit on the first differing byte.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 32 bytes from the OS random source as 64 lowercase hex characters.
///
/// Panics only if the OS random source is unavailable, where no secret could
/// be made at all.
pub fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS random source unavailable");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Return the Host header only when it is exactly `127.0.0.1:<port>` or `localhost:<port>`.
pub fn loopback_host(headers: &HeaderMap, port: u16) -> Option<&str> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let (name, actual) = host.split_once(':')?;
    (matches!(name, "127.0.0.1" | "localhost")
        && !actual.starts_with('+')
        && actual.parse::<u16>().ok() == Some(port)
        && actual == port.to_string())
    .then_some(host)
}

/// A browser Origin, when present, must be the same loopback origin as Host.
pub fn same_origin(headers: &HeaderMap, host: &str) -> bool {
    headers
        .get(header::ORIGIN)
        .is_none_or(|origin| origin.to_str().ok() == Some(format!("http://{host}").as_str()))
}

pub fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// What an operator request's session header amounts to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    /// The current, unexpired session.
    Valid,
    /// The session a newer sign-in replaced.
    Replaced,
    /// No header, a wrong one, or an expired session.
    Invalid,
}

struct Inner {
    launch: String,
    launch_file: Option<PathBuf>,
    /// `(id, expiry)` of the one live session.
    session: Option<(String, Instant)>,
    /// The id the last sign-in replaced, to tell its page why it is out.
    replaced: Option<String>,
}

/// Single-use launch secret plus the one operator session.
pub struct OwnerSessions {
    lifetime: Duration,
    inner: Mutex<Inner>,
}

impl OwnerSessions {
    pub fn new(lifetime: Duration) -> Self {
        Self {
            lifetime,
            inner: Mutex::new(Inner {
                launch: random_secret(),
                launch_file: None,
                session: None,
                replaced: None,
            }),
        }
    }

    pub fn lifetime(&self) -> Duration {
        self.lifetime
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        // The session table holds no cross-field invariants a panic could break.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The current, unused launch secret.
    pub fn launch_secret(&self) -> String {
        self.inner().launch.clone()
    }

    /// Write the private (0600) launch file atomically: a new temp file,
    /// then a rename over the old one.
    pub fn write_launch_file(&self, path: &Path, contents: &str) -> std::io::Result<()> {
        let temp = path.with_extension("html.tmp");
        match fs::symlink_metadata(&temp) {
            Ok(_) => fs::remove_file(&temp)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut file = private_create(&temp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        self.inner().launch_file = Some(path.to_owned());
        Ok(())
    }

    /// Exchange the launch secret. On success the secret is dead, a new one
    /// replaces it (the caller writes the new launch file), the previous
    /// session ends, and the new session id is returned.
    pub fn exchange(&self, presented: &str) -> Option<String> {
        let mut inner = self.inner();
        if !constant_time_eq(inner.launch.as_bytes(), presented.as_bytes()) {
            return None;
        }
        inner.launch = random_secret();
        let id = random_secret();
        let previous = inner
            .session
            .replace((id.clone(), Instant::now() + self.lifetime));
        inner.replaced = previous.map(|(old, _)| old);
        Some(id)
    }

    /// The operator session named by the request's `X-TrustGraph-Operator`.
    pub fn check(&self, headers: &HeaderMap) -> Session {
        let Some(presented) = headers.get(OPERATOR_HEADER).and_then(|v| v.to_str().ok()) else {
            return Session::Invalid;
        };
        let inner = self.inner();
        let now = Instant::now();
        let current = inner.session.as_ref().is_some_and(|(id, expiry)| {
            constant_time_eq(id.as_bytes(), presented.as_bytes()) & (*expiry > now)
        });
        let replaced = inner
            .replaced
            .as_ref()
            .is_some_and(|id| constant_time_eq(id.as_bytes(), presented.as_bytes()));
        if current {
            Session::Valid
        } else if replaced {
            Session::Replaced
        } else {
            Session::Invalid
        }
    }
}

impl Drop for OwnerSessions {
    fn drop(&mut self) {
        if let Some(path) = self.inner().launch_file.take() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Create a new 0600 file; never through a link.
pub(crate) fn private_create(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// Refuse a state directory inside a repository that agents or tools read, unless allowed.
pub fn state_outside_repositories(
    state: &Path,
    repositories: &[&Path],
    allow: bool,
) -> std::io::Result<()> {
    let state = fs::canonicalize(state)?;
    for repository in repositories {
        let Ok(repository) = fs::canonicalize(repository) else {
            continue;
        };
        if state.starts_with(&repository) {
            let message = format!(
                "state directory {} is inside repository {}; agents with workspace access could read local capabilities. Move it outside, or pass --allow-state-in-repository",
                state.display(),
                repository.display()
            );
            if allow {
                eprintln!("warning: {message}");
            } else {
                return Err(std::io::Error::other(message));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(k.clone(), HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn launch_secret_is_single_use_and_sessions_are_header_only() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = OwnerSessions::new(SESSION_LIFETIME);
        let launch = sessions.launch_secret();
        let file = dir.path().join("operator.html");
        sessions.write_launch_file(&file, &launch).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(sessions.exchange(&"0".repeat(64)).is_none());
        assert_eq!(
            sessions.launch_secret(),
            launch,
            "a wrong guess must not burn the secret"
        );
        let id = sessions.exchange(&launch).unwrap();
        assert_ne!(id, launch);
        assert_ne!(sessions.launch_secret(), launch, "a new secret replaces it");
        assert!(
            sessions.exchange(&launch).is_none(),
            "the old secret is dead"
        );
        let with = |name: &str, value: &str| {
            headers(&[(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value,
            )])
        };
        assert_eq!(sessions.check(&with(OPERATOR_HEADER, &id)), Session::Valid);
        assert_eq!(sessions.check(&HeaderMap::new()), Session::Invalid);
        assert_eq!(
            sessions.check(&with("cookie", &format!("trustgraph_owner={id}"))),
            Session::Invalid,
            "a cookie is never a session"
        );
        assert_eq!(
            sessions.check(&with("authorization", &format!("Bearer {id}"))),
            Session::Invalid
        );
        assert_eq!(
            sessions.check(&with(OPERATOR_HEADER, &launch)),
            Session::Invalid
        );

        // A second sign-in ends the first session.
        let second = sessions.exchange(&sessions.launch_secret()).unwrap();
        assert_eq!(
            sessions.check(&with(OPERATOR_HEADER, &id)),
            Session::Replaced
        );
        assert_eq!(
            sessions.check(&with(OPERATOR_HEADER, &second)),
            Session::Valid
        );
    }

    #[test]
    fn sessions_expire() {
        let sessions = OwnerSessions::new(Duration::from_millis(0));
        let id = sessions.exchange(&sessions.launch_secret()).unwrap();
        let h = headers(&[(
            header::HeaderName::from_static(OPERATOR_HEADER),
            id.as_str(),
        )]);
        assert_eq!(sessions.check(&h), Session::Invalid);
    }

    #[test]
    fn host_and_origin_are_strict() {
        for good in ["127.0.0.1:8787", "localhost:8787"] {
            assert!(loopback_host(&headers(&[(header::HOST, good)]), 8787).is_some());
        }
        for bad in [
            "127.0.0.1:8788",
            "127.0.0.1.evil.test:8787",
            "evil.test:8787",
            "127.0.0.1",
            "127.0.0.1:08787",
            "127.0.0.1:+8787",
            "localhost:8787@evil",
            "",
        ] {
            assert!(
                loopback_host(&headers(&[(header::HOST, bad)]), 8787).is_none(),
                "{bad}"
            );
        }
        assert!(loopback_host(&HeaderMap::new(), 8787).is_none());
        let h = headers(&[(header::ORIGIN, "http://evil.test")]);
        assert!(!same_origin(&h, "127.0.0.1:8787"));
        let h = headers(&[(header::ORIGIN, "http://127.0.0.1:8787")]);
        assert!(same_origin(&h, "127.0.0.1:8787"));
        assert!(same_origin(&HeaderMap::new(), "127.0.0.1:8787"));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn state_inside_repository_is_refused_without_override() {
        let repo = tempfile::tempdir().unwrap();
        let state = repo.path().join(".trustgraph/state");
        fs::create_dir_all(&state).unwrap();
        assert!(state_outside_repositories(&state, &[repo.path()], false).is_err());
        assert!(state_outside_repositories(&state, &[repo.path()], true).is_ok());
        let outside = tempfile::tempdir().unwrap();
        assert!(state_outside_repositories(outside.path(), &[repo.path()], false).is_ok());
    }
}
