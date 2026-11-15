use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Digest(pub String);
impl Digest {
    /// Whether this is a canonical v1 digest: 64 lowercase hexadecimal ASCII
    /// characters. Validate untrusted digests with this before using them.
    pub fn is_canonical(&self) -> bool {
        self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }
    /// A short ASCII prefix for identifiers; never panics on arbitrary input.
    pub fn prefix(&self, len: usize) -> &str {
        let end = self
            .0
            .char_indices()
            .map(|(i, c)| i + c.len_utf8())
            .take_while(|end| *end <= len)
            .last()
            .unwrap_or(0);
        &self.0[..end]
    }
}

/// SHA-256 of exact bytes. No newline or whitespace normalization.
pub fn content(bytes: &[u8]) -> Digest {
    Digest(format!("{:x}", Sha256::digest(bytes)))
}

/// v1 canonical encoding: sorted object keys, compact JSON, ordered arrays,
/// explicit null fields, integer timestamps. Domain models contain no floats.
pub fn structured(domain: &str, payload: &impl Serialize) -> Digest {
    let value = serde_json::json!({"domain": domain, "schema": 1, "payload": payload});
    content(&serde_json::to_vec(&value).expect("domain data is JSON serializable"))
}
