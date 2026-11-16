//! One private bearer token file per agent: `<state>/tokens/<agent>.token`.
//!
//! A token is 32 bytes from the OS random source, written as 64 lowercase hex
//! characters in a 0600 file. A well-formed file is kept across restarts; a
//! malformed one is replaced; a symlink or other non-regular file is an error.
//! Files of agents that are no longer in the active policy are removed.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const SUFFIX: &str = ".token";

/// 32 random bytes as 64 lowercase hex characters.
pub fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn well_formed(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Agent names come from a parsed policy (lowercase letters, digits and
/// `_ . @ -`, starting with a letter or digit), so they are safe file names.
/// Check again anyway: a name that is not is never written.
fn safe_name(agent: &str) -> bool {
    !agent.is_empty()
        && agent.len() <= 64
        && agent
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && agent
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.@-".contains(&b))
}

pub fn dir(state: &Path) -> PathBuf {
    state.join("tokens")
}

pub fn path(state: &Path, agent: &str) -> PathBuf {
    dir(state).join(format!("{agent}{SUFFIX}"))
}

pub(crate) fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)?;
    let metadata = fs::symlink_metadata(dir)?;
    if !metadata.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} must be a directory, not a link",
            dir.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The existing well-formed token, or `None` when the file is missing or
/// malformed. A link or non-regular file is an error.
fn read_existing(path: &Path) -> std::io::Result<Option<String>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !metadata.is_file() {
        return Err(std::io::Error::other(format!(
            "{} must be a regular file",
            path.display()
        )));
    }
    if metadata.len() != 64 {
        return Ok(None);
    }
    let text = fs::read_to_string(path).unwrap_or_default();
    if !well_formed(&text) {
        return Ok(None);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(Some(text))
}

/// Write a new token through a private temp file and an atomic rename.
fn write_new(path: &Path) -> std::io::Result<String> {
    let token = random_token()?;
    let temp = path.with_extension("token.tmp");
    let _ = fs::remove_file(&temp);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    Ok(token)
}

/// Make the token directory match `agents`. Returns `(agent, token)` for
/// every agent that has a usable token, and one message per problem. The
/// caller decides whether a problem is fatal (at start it is).
pub fn sync(state: &Path, agents: &BTreeSet<String>) -> (Vec<(String, String)>, Vec<String>) {
    let mut problems = Vec::new();
    let dir = dir(state);
    if let Err(e) = private_dir(&dir) {
        problems.push(format!("cannot create {}: {e}", dir.display()));
        return (Vec::new(), problems);
    }
    let mut tokens: Vec<(String, String)> = Vec::new();
    for agent in agents {
        if !safe_name(agent) {
            problems.push(format!("agent name `{agent}` is not a safe file name"));
            continue;
        }
        let path = path(state, agent);
        let token = match read_existing(&path) {
            // Two agents must never share a token.
            Ok(Some(t)) if !tokens.iter().any(|(_, other)| *other == t) => Ok(t),
            Ok(_) => write_new(&path),
            Err(e) => Err(e),
        };
        match token {
            Ok(t) => tokens.push((agent.clone(), t)),
            Err(e) => problems.push(format!("token for `{agent}`: {e}")),
        }
    }
    match fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(agent) = name.strip_suffix(SUFFIX) else {
                    continue;
                };
                if !agents.contains(agent)
                    && let Err(e) = fs::remove_file(entry.path())
                {
                    problems.push(format!("cannot remove {name}: {e}"));
                }
            }
        }
        Err(e) => problems.push(format!("cannot list {}: {e}", dir.display())),
    }
    (tokens, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn tokens_are_private_kept_replaced_and_removed() {
        let state = tempfile::tempdir().unwrap();
        let (first, problems) = sync(state.path(), &set(&["a", "b"]));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(first.len(), 2);
        assert_ne!(first[0].1, first[1].1);
        assert!(first.iter().all(|(_, t)| well_formed(t)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(path(state.path(), "a"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let (again, _) = sync(state.path(), &set(&["a", "b"]));
        assert_eq!(again, first, "kept across restarts");

        fs::write(path(state.path(), "a"), "not a token").unwrap();
        let (replaced, _) = sync(state.path(), &set(&["a", "c"]));
        assert_ne!(replaced[0].1, first[0].1, "malformed file is replaced");
        assert!(
            !path(state.path(), "b").exists(),
            "removed agent's file is gone"
        );
        assert!(path(state.path(), "c").exists());

        // Two agents with the same token: the second gets a new one.
        let a = fs::read_to_string(path(state.path(), "a")).unwrap();
        fs::write(path(state.path(), "c"), &a).unwrap();
        let (dedup, _) = sync(state.path(), &set(&["a", "c"]));
        assert_ne!(dedup[0].1, dedup[1].1);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_token_file_is_refused() {
        let state = tempfile::tempdir().unwrap();
        private_dir(&dir(state.path())).unwrap();
        let target = state.path().join("elsewhere");
        fs::write(&target, "x".repeat(64)).unwrap();
        std::os::unix::fs::symlink(&target, path(state.path(), "a")).unwrap();
        let (tokens, problems) = sync(state.path(), &set(&["a"]));
        assert!(tokens.is_empty());
        assert_eq!(problems.len(), 1);
    }

    #[test]
    fn unsafe_names_are_never_written() {
        assert!(safe_name("mailer"));
        assert!(!safe_name("../x"));
        assert!(!safe_name(".hidden"));
        assert!(!safe_name("a/b"));
        assert!(!safe_name(""));
    }
}
