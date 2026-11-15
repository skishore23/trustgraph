//! Operator sessions, loopback request checks and private capability files.
//!
//! The operator authority is never a long-lived bearer token. Each server start
//! generates a single-use launch secret in memory and writes it only to a private
//! launch file. Exchanging it (once) yields a random, short-lived session ID in an
//! HttpOnly, SameSite=Strict cookie. Owner routes accept only that cookie.
use axum::http::{HeaderMap, header};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

/// Default operator session lifetime.
pub const SESSION_LIFETIME: Duration = Duration::from_secs(8 * 60 * 60);
const MAX_SESSIONS: usize = 8;

/// Compare secrets without an early exit on the first differing byte.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 64 lowercase hex characters from the OS-backed UUID v4 generator (244 random bits).
pub fn random_secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
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

pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(';'))
        .map(str::trim)
        .find_map(|c| c.strip_prefix(name)?.strip_prefix('='))
}

struct Inner {
    launch: Option<String>,
    launch_file: Option<PathBuf>,
    sessions: HashMap<String, Instant>,
}

/// Single-use launch secret plus short-lived operator sessions.
pub struct OwnerSessions {
    cookie_name: String,
    lifetime: Duration,
    inner: Mutex<Inner>,
}
impl OwnerSessions {
    /// `scope` separates cookies of servers on different ports (cookies ignore ports).
    pub fn new(scope: &str, lifetime: Duration) -> Self {
        let safe: String = scope
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        Self {
            cookie_name: format!("trustgraph_owner_{safe}"),
            lifetime,
            inner: Mutex::new(Inner {
                launch: Some(random_secret()),
                launch_file: None,
                sessions: HashMap::new(),
            }),
        }
    }
    pub fn cookie_name(&self) -> &str {
        &self.cookie_name
    }
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        // The session table holds no cross-field invariants a panic could break.
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
    /// The unused launch secret, if it has not been exchanged yet.
    pub fn launch_secret(&self) -> Option<String> {
        self.inner().launch.clone()
    }
    /// Write a private (0600) launch file; it is deleted after the secret is exchanged.
    pub fn write_launch_file(&self, path: &Path, contents: &str) -> std::io::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(_) => fs::remove_file(path)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let mut file = private_create(path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        self.inner().launch_file = Some(path.to_owned());
        Ok(())
    }
    /// Exchange the launch secret once. Returns the `Set-Cookie` header value.
    pub fn exchange(&self, presented: &str) -> Option<String> {
        let mut inner = self.inner();
        let launch = inner.launch.as_deref()?;
        if !constant_time_eq(launch.as_bytes(), presented.as_bytes()) {
            return None;
        }
        inner.launch = None;
        if let Some(path) = inner.launch_file.take() {
            let _ = fs::remove_file(path);
        }
        let now = Instant::now();
        inner.sessions.retain(|_, expiry| *expiry > now);
        if inner.sessions.len() >= MAX_SESSIONS {
            return None;
        }
        let id = random_secret();
        inner.sessions.insert(id.clone(), now + self.lifetime);
        Some(format!(
            "{}={id}; HttpOnly; SameSite=Strict; Path=/; Max-Age={}",
            self.cookie_name,
            self.lifetime.as_secs()
        ))
    }
    /// True when the request carries a current operator session cookie.
    pub fn valid(&self, headers: &HeaderMap) -> bool {
        let Some(presented) = cookie(headers, &self.cookie_name) else {
            return false;
        };
        let now = Instant::now();
        let inner = self.inner();
        // Compare against every live session so timing does not reveal prefixes.
        inner.sessions.iter().fold(false, |found, (id, expiry)| {
            found | (constant_time_eq(id.as_bytes(), presented.as_bytes()) && *expiry > now)
        })
    }
}
impl Drop for OwnerSessions {
    fn drop(&mut self) {
        if let Some(path) = self.inner().launch_file.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn private_create(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Read or create a private 64-hex capability file. A pre-existing file must be a
/// regular file (not a symlink); its mode is reset to 0600 and then verified.
pub fn capability_file(path: &Path) -> std::io::Result<String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let value = random_secret();
            let mut file = private_create(path)?;
            file.write_all(value.as_bytes())?;
            file.sync_all()?;
            return Ok(value);
        }
        Err(e) => return Err(e),
        Ok(metadata) if !metadata.is_file() => {
            return Err(std::io::Error::other(
                "capability path must be a regular file",
            ));
        }
        Ok(_) => {}
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(path)?;
        let uid = fs::metadata(path.parent().unwrap_or(Path::new(".")))?.uid();
        if !metadata.is_file()
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.uid() != uid
        {
            return Err(std::io::Error::other(
                "capability file must be private to the state directory owner",
            ));
        }
    }
    let value = fs::read_to_string(path)?;
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(std::io::Error::other("invalid local token file"));
    }
    Ok(value)
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
    fn launch_secret_is_single_use_and_sessions_are_cookie_only() {
        let dir = tempfile::tempdir().unwrap();
        let sessions = OwnerSessions::new("8080", SESSION_LIFETIME);
        let launch = sessions.launch_secret().unwrap();
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
        assert!(
            sessions.launch_secret().is_some(),
            "a wrong guess must not burn the secret"
        );
        let set_cookie = sessions.exchange(&launch).unwrap();
        assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"));
        assert!(set_cookie.starts_with("trustgraph_owner_8080="));
        assert!(!file.exists(), "launch file is removed after exchange");
        assert!(
            sessions.exchange(&launch).is_none(),
            "launch secret is single-use"
        );
        let cookie_value = set_cookie.split(';').next().unwrap();
        assert!(!cookie_value.contains(&launch));
        assert!(sessions.valid(&headers(&[(header::COOKIE, cookie_value)])));
        assert!(!sessions.valid(&headers(&[(
            header::AUTHORIZATION,
            &format!("Bearer {launch}")
        )])));
        assert!(!sessions.valid(&headers(&[(
            header::COOKIE,
            &format!("trustgraph_owner_8080={launch}")
        )])));
        let other_port = OwnerSessions::new("8084", SESSION_LIFETIME);
        assert_ne!(other_port.cookie_name(), sessions.cookie_name());
        assert!(!other_port.valid(&headers(&[(header::COOKIE, cookie_value)])));
    }

    #[test]
    fn sessions_expire() {
        let sessions = OwnerSessions::new("1", Duration::from_millis(0));
        let launch = sessions.launch_secret().unwrap();
        let cookie_value = sessions.exchange(&launch).unwrap();
        let cookie_value = cookie_value.split(';').next().unwrap();
        assert!(!sessions.valid(&headers(&[(header::COOKIE, cookie_value)])));
    }

    #[test]
    fn host_and_origin_are_strict() {
        for good in ["127.0.0.1:8080", "localhost:8080"] {
            assert!(loopback_host(&headers(&[(header::HOST, good)]), 8080).is_some());
        }
        for bad in [
            "127.0.0.1:8081",
            "127.0.0.1.evil.test:8080",
            "evil.test:8080",
            "127.0.0.1",
            "127.0.0.1:08080",
            "127.0.0.1:+8080",
            "localhost:8080@evil",
            "",
        ] {
            assert!(
                loopback_host(&headers(&[(header::HOST, bad)]), 8080).is_none(),
                "{bad}"
            );
        }
        assert!(loopback_host(&HeaderMap::new(), 8080).is_none());
        let h = headers(&[(header::ORIGIN, "http://evil.test")]);
        assert!(!same_origin(&h, "127.0.0.1:8080"));
        let h = headers(&[(header::ORIGIN, "http://127.0.0.1:8080")]);
        assert!(same_origin(&h, "127.0.0.1:8080"));
        assert!(same_origin(&HeaderMap::new(), "127.0.0.1:8080"));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn capability_files_are_reset_to_private_and_reject_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent-token");
        let value = capability_file(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(capability_file(&path).unwrap(), value);
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            let link = dir.path().join("linked-token");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(capability_file(&link).is_err());
        }
    }

    #[test]
    fn state_inside_repository_is_refused_without_override() {
        let repo = tempfile::tempdir().unwrap();
        let state = repo.path().join(".trustgraph-live/state");
        fs::create_dir_all(&state).unwrap();
        assert!(state_outside_repositories(&state, &[repo.path()], false).is_err());
        assert!(state_outside_repositories(&state, &[repo.path()], true).is_ok());
        let outside = tempfile::tempdir().unwrap();
        assert!(state_outside_repositories(outside.path(), &[repo.path()], false).is_ok());
    }
}
