//! Hand-off tokens: what one gateway session carries, signed so that only
//! this gateway can make one and nobody can edit one.
//!
//! A token is `tgh1.<hex payload>.<hex HMAC-SHA256>`, keyed by a 32-byte
//! secret kept 0600 in `<state>/gateway.secret`. The payload names the
//! issuing agent, the journal position it was issued at, its expiry and the
//! artifacts it carries. A dropped token cannot be detected: an agent that
//! never passes it on simply does not pass on what it read.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::serve::auth;

/// How long a hand-off token is accepted.
pub const LIFETIME_SECONDS: u64 = 24 * 60 * 60;
/// Longest token accepted.
pub const MAX_TOKEN_BYTES: usize = 32 * 1024;
const PREFIX: &str = "tgh1.";
const SECRET_FILE: &str = "gateway.secret";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    /// The agent whose session issued it.
    pub agent: String,
    /// The journal's last sequence number when it was issued.
    pub seq: u64,
    pub expires_at: u64,
    pub artifacts: Vec<String>,
}

fn hmac(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    let mut padded = [0u8; 64];
    padded[..32].copy_from_slice(key);
    let inner_key: Vec<u8> = padded.iter().map(|b| b ^ 0x36).collect();
    let outer_key: Vec<u8> = padded.iter().map(|b| b ^ 0x5c).collect();
    let inner = Sha256::new()
        .chain_update(&inner_key)
        .chain_update(message)
        .finalize();
    Sha256::new()
        .chain_update(&outer_key)
        .chain_update(inner)
        .finalize()
        .into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| {
            let pair = text.get(i..i + 2)?;
            if !pair
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return None;
            }
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

pub struct Signer {
    key: [u8; 32],
}

impl Signer {
    /// The secret in `<state>/gateway.secret`, created 0600 on first use.
    pub fn open(state: &Path) -> std::io::Result<Self> {
        let path = state.join(SECRET_FILE);
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let text = match options.open(&path) {
            Ok(file) => {
                let metadata = file.metadata()?;
                if !metadata.is_file() {
                    return Err(std::io::Error::other(format!(
                        "{} must be a regular file",
                        path.display()
                    )));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(fs::Permissions::from_mode(0o600))?;
                }
                let mut text = String::new();
                file.take(128).read_to_string(&mut text)?;
                text
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let secret = auth::random_secret();
                let mut file = auth::private_create(&path)?;
                file.write_all(secret.as_bytes())?;
                file.sync_all()?;
                secret
            }
            Err(e) => return Err(e),
        };
        let key: [u8; 32] = unhex(text.trim())
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| {
                std::io::Error::other(format!(
                    "{} is not 64 lowercase hex characters; remove it to make a new one (outstanding hand-off tokens stop working)",
                    path.display()
                ))
            })?;
        Ok(Signer { key })
    }

    #[cfg(test)]
    fn with_key(key: [u8; 32]) -> Self {
        Signer { key }
    }

    pub fn sign(&self, payload: &Payload) -> String {
        let body = hex(&serde_json::to_vec(payload).unwrap_or_default());
        let signed = format!("{PREFIX}{body}");
        let mac = hmac(&self.key, signed.as_bytes());
        format!("{signed}.{}", hex(&mac))
    }

    /// The payload of a token this gateway made and nobody changed; `None`
    /// for anything else. Expiry and position are the caller's to check.
    pub fn verify(&self, token: &str) -> Option<Payload> {
        if token.len() > MAX_TOKEN_BYTES {
            return None;
        }
        let (signed, mac) = token.rsplit_once('.')?;
        let body = signed.strip_prefix(PREFIX)?;
        let mac = unhex(mac)?;
        let expected = hmac(&self.key, signed.as_bytes());
        if !auth::constant_time_eq(&mac, &expected) {
            return None;
        }
        serde_json::from_slice(&unhex(body)?).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231_style_vector() {
        // HMAC-SHA256 with a 32-byte key of 0x0b and "Hi There" (RFC 4231
        // case 1 uses a 20-byte key; padding to 32 zero-extends it the same way).
        let mut key = [0u8; 32];
        key[..20].copy_from_slice(&[0x0b; 20]);
        assert_eq!(
            hex(&hmac(&key, b"Hi There")),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn tokens_verify_only_unchanged() {
        let signer = Signer::with_key([7; 32]);
        let payload = Payload {
            agent: "researcher".into(),
            seq: 4,
            expires_at: 100,
            artifacts: vec!["a".into()],
        };
        let token = signer.sign(&payload);
        assert_eq!(signer.verify(&token), Some(payload.clone()));
        assert!(Signer::with_key([8; 32]).verify(&token).is_none());
        // Flip one character anywhere.
        for i in PREFIX.len()..token.len() {
            let mut bytes = token.clone().into_bytes();
            bytes[i] = if bytes[i] == b'0' { b'1' } else { b'0' };
            let edited = String::from_utf8(bytes).unwrap();
            if edited != token {
                assert!(signer.verify(&edited).is_none(), "{i}");
            }
        }
        assert!(signer.verify("").is_none());
        assert!(signer.verify(&"a".repeat(MAX_TOKEN_BYTES + 1)).is_none());
    }
}
