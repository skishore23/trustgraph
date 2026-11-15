//! Policy-as-code: the `trustgraph.policy.yaml` file format.
//!
//! Parsing never panics and reports every problem it finds in one pass, each
//! with a path into the file, a stable code and, where possible, a suggestion.

pub mod decide;
pub mod diff;
pub mod explain;
pub mod lint;
mod model;
mod parse;

pub use decide::{Blocking, Outcome, Query, TestResult, decide, run_tests};
pub use diff::{Change, ChangeKind, Diff, diff};
pub use explain::explain;
pub use lint::lint;
pub use model::{
    Action, ApprovalRule, Case, Diagnostic, Parsed, Policy, Severity, Source, Verdict,
};
pub use parse::{MAX_BYTES, parse};
