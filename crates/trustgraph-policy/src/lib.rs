//! Policy-as-code: the `trustgraph.policy.yaml` file format.
//!
//! Parsing never panics and reports every problem it finds in one pass, each
//! with a path into the file, a stable code and, where possible, a suggestion.

mod model;
mod parse;

pub use model::{
    Action, ApprovalRule, Case, Diagnostic, Parsed, Policy, Severity, Source, Verdict,
};
pub use parse::{MAX_BYTES, parse};
