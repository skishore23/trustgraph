//! Deterministic governance. No clocks, network clients or persistence live here.
pub mod bundle;
pub mod digest;
pub mod domain;
pub mod lineage;
pub mod plan;
pub mod verifier;
pub use domain::*;
pub use verifier::verify;
