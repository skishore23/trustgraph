use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Digest(pub String);

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
