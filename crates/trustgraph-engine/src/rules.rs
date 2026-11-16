//! Pure helpers the engine's handlers share: field bounds, echo clipping,
//! policy validation and the translation of a policy into Graph rules.

use trustgraph_core::digest;
use trustgraph_framework::{ActionRule, ApprovalRequirement, Error, PrincipalId, Readers};
use trustgraph_policy::{Outcome, Policy, Severity, lint, parse, run_tests};

use crate::event::Reply;

/// Longest accepted name or id, in bytes.
pub(crate) const MAX_FIELD: usize = 256;
pub(crate) const MAX_INPUTS: usize = 64;
pub(crate) const MAX_RECIPIENTS: usize = 256;
/// Most characters of caller input one `Rejected` message may echo, in total.
pub(crate) const MAX_ECHO: usize = 64;

pub(crate) fn reject(code: &str, message: impl Into<String>) -> Reply {
    Reply::Rejected {
        code: code.into(),
        message: message.into(),
    }
}

/// Pieces of caller input for one message, cut so that together they hold at
/// most `MAX_ECHO` characters; earlier pieces get the budget first. Every
/// echo of caller input goes through here.
pub(crate) fn echo<const N: usize>(pieces: [&str; N]) -> [String; N] {
    let mut budget = MAX_ECHO;
    pieces.map(|piece| {
        let mut out: String = piece.chars().take(budget).collect();
        budget -= out.chars().count();
        if out.len() < piece.len() {
            out.push('…');
        }
        out
    })
}

/// One piece of caller input, for a message that echoes nothing else.
pub(crate) fn clip(value: &str) -> String {
    let [out] = echo([value]);
    out
}

pub(crate) fn diagnostic(d: &trustgraph_policy::Diagnostic) -> String {
    let [path, message] = echo([&d.path, &d.message]);
    format!("{} at `{path}`: {message}", d.code)
}

pub(crate) fn field(name: &str, value: &str) -> Result<(), Reply> {
    if value.trim().is_empty() || value.len() > MAX_FIELD {
        return Err(reject(
            "invalid_field",
            format!("`{name}` must be 1 to {MAX_FIELD} bytes and not blank"),
        ));
    }
    Ok(())
}

pub(crate) fn sha256(value: &str) -> Result<(), Reply> {
    if digest::Digest(value.to_string()).is_canonical() {
        Ok(())
    } else {
        Err(reject(
            "invalid_digest",
            format!("`{}` is not 64 lowercase hex characters", clip(value)),
        ))
    }
}

pub(crate) fn graph_error(error: Error) -> Reply {
    let code = match error {
        Error::Lineage(_) => "lineage_limit",
        _ => "engine_mismatch",
    };
    reject(code, clip(&error.to_string()))
}

pub(crate) fn principals(names: impl IntoIterator<Item = String>) -> Readers {
    names.into_iter().map(PrincipalId).collect()
}

/// A source's Graph reader set: its people plus the agents that may use it.
/// A source missing from the policy has no readers.
pub(crate) fn graph_readers(policy: &Policy, name: &str) -> Readers {
    match policy.sources.get(name) {
        Some(source) => principals(
            policy
                .people(&source.readers)
                .into_iter()
                .chain(source.agents.iter().cloned()),
        ),
        None => Readers::new(),
    }
}

/// An action's Graph rule. A missing action gets a rule nobody can satisfy.
pub(crate) fn action_rule(policy: &Policy, name: &str) -> ActionRule {
    match policy.actions.get(name) {
        Some(action) => ActionRule {
            actors: principals(action.agents.iter().cloned()),
            approval: action.approval.as_ref().map(|rule| ApprovalRequirement {
                reviewers: principals(policy.people(&rule.reviewers)),
                ttl_seconds: rule.valid_for_seconds,
            }),
        },
        None => ActionRule {
            actors: Readers::new(),
            approval: None,
        },
    }
}

/// Parse, lint and test a policy text; any failure is `invalid_policy`.
/// The size bound is `Event::validate_shape`'s (and `parse` enforces it too).
pub(crate) fn validate_policy(text: &str) -> Result<Policy, Reply> {
    let parsed = parse(text);
    let Some(policy) = parsed.policy else {
        let first = parsed
            .diagnostics
            .iter()
            .find(|d| d.severity == Severity::Error);
        let message = first.map_or("the policy does not parse".to_string(), diagnostic);
        return Err(reject("invalid_policy", message));
    };
    if let Some(d) = lint(&policy)
        .into_iter()
        .find(|d| d.severity == Severity::Error)
    {
        return Err(reject("invalid_policy", diagnostic(&d)));
    }
    if let Some(t) = run_tests(&policy).into_iter().find(|t| !t.pass) {
        return Err(reject(
            "invalid_policy",
            format!(
                "test `{}` expects {} but the policy says {}",
                clip(&t.name),
                t.expect.as_str(),
                t.outcome.verdict.as_str()
            ),
        ));
    }
    Ok(policy)
}

pub(crate) fn decision_reply(id: String, outcome: Outcome) -> Reply {
    Reply::Decision {
        id,
        verdict: outcome.verdict,
        code: outcome.code.to_string(),
        reason: outcome.reason,
        blocking: outcome.blocking,
        expires_at: None,
    }
}
