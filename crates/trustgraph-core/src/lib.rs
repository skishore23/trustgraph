//! Deterministic governance. No clocks, network clients or persistence live here.
pub mod bundle;
pub mod decision;
pub mod digest;
pub mod domain;
pub mod lineage;
pub mod permissions;
pub mod plan;
pub mod verifier;
pub use domain::*;
pub use verifier::verify;
