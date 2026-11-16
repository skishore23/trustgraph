//! Journal-first runtime: every event is made durable before the engine sees
//! it, and the engine is rebuilt by replay on open.

use thiserror::Error;
use trustgraph_policy::{Diagnostic, Severity, lint, parse, run_tests};

use crate::engine::Engine;
use crate::event::{Event, Fault, Reply};
use crate::journal::{GENESIS_PREV, Journal, JournalError, Record};

#[derive(Debug, Error)]
pub enum OpenError {
    #[error("journal error: {0}")]
    Journal(#[from] JournalError),
    #[error("the policy file is invalid ({} diagnostics)", .0.len())]
    InvalidPolicy(Vec<Diagnostic>),
    #[error("replay failed at record {seq}: {reason}")]
    Replay { seq: u64, reason: String },
}

#[derive(Debug)]
pub struct Runtime<J: Journal> {
    journal: J,
    engine: Engine,
    last_seq: u64,
    last_hash: String,
    last_at: u64,
}

/// Parse, lint and run the inline tests; the digest of a valid policy.
fn validate(policy_yaml: &str) -> Result<String, OpenError> {
    let parsed = parse(policy_yaml);
    let mut diagnostics = parsed.diagnostics;
    let Some(policy) = parsed.policy else {
        return Err(OpenError::InvalidPolicy(diagnostics));
    };
    diagnostics.extend(lint(&policy));
    for test in run_tests(&policy).into_iter().filter(|t| !t.pass) {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            path: "tests".into(),
            code: "test_failed",
            message: format!(
                "test `{}` expects {} but the policy says {}",
                test.name,
                test.expect.as_str(),
                test.outcome.verdict.as_str()
            ),
            suggestion: None,
        });
    }
    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Err(OpenError::InvalidPolicy(diagnostics));
    }
    Ok(policy.digest())
}

impl<J: Journal> Runtime<J> {
    pub fn open(journal: J, policy_yaml: &str, at: u64) -> Result<Self, OpenError> {
        Self::open_with_fault(journal, policy_yaml, at, Fault::None)
    }

    /// Like `open`, with a deliberate engine bug for the simulator. The fault
    /// applies to replay and to live handling alike.
    #[doc(hidden)]
    pub fn open_with_fault(
        mut journal: J,
        policy_yaml: &str,
        at: u64,
        fault: Fault,
    ) -> Result<Self, OpenError> {
        let wanted = validate(policy_yaml)?;
        let records = journal.load()?;
        let mut engine = Engine::new().with_fault(fault);
        let mut previous_at = 0;
        for (index, record) in records.iter().enumerate() {
            let replay = |reason: String| OpenError::Replay {
                seq: record.seq,
                reason,
            };
            let is_genesis = matches!(record.event, Event::Genesis { .. });
            if index == 0 && !is_genesis {
                return Err(replay("the first record is not Genesis".into()));
            }
            if record.at < previous_at {
                return Err(replay(format!(
                    "time {} is before the previous record at {previous_at}",
                    record.at
                )));
            }
            previous_at = record.at;
            let reply = engine.apply(record.at, &record.event);
            if let (true, Reply::Rejected { code, message }) = (is_genesis, &reply) {
                return Err(replay(format!("Genesis was rejected: {code}: {message}")));
            }
        }
        let (last_seq, last_hash) = records
            .last()
            .map_or((0, GENESIS_PREV.to_string()), |r| (r.seq, r.hash.clone()));
        let mut runtime = Runtime {
            journal,
            engine,
            last_seq,
            last_hash,
            last_at: previous_at,
        };
        let active = runtime.engine.policy().map(|p| p.digest());
        if active.as_deref() != Some(wanted.as_str()) {
            let at = at.max(runtime.last_at);
            let genesis = Event::Genesis {
                policy: policy_yaml.to_string(),
            };
            let reply = runtime.append_and_apply(at, genesis)?;
            if let Reply::Rejected { code, message } = reply {
                return Err(OpenError::Replay {
                    seq: runtime.last_seq,
                    reason: format!("Genesis was rejected: {code}: {message}"),
                });
            }
        }
        Ok(runtime)
    }

    /// Journal `event`, then apply it. Rejected events are journaled too: the
    /// engine's clock moves even when it refuses an event, and replay must
    /// reach the same state. Two cases are refused without journaling:
    /// time going backwards (it would make the journal unreplayable) and
    /// `Genesis` (written only by `open`).
    pub fn handle(&mut self, at: u64, event: Event) -> Result<Reply, JournalError> {
        if matches!(event, Event::Genesis { .. }) {
            return Ok(Reply::Rejected {
                code: "not_allowed".into(),
                message: "Genesis cannot be sent through handle".into(),
            });
        }
        if at < self.last_at {
            return Ok(Reply::Rejected {
                code: "clock_regressed".into(),
                message: format!("time {at} is before the previous event at {}", self.last_at),
            });
        }
        self.append_and_apply(at, event)
    }

    fn append_and_apply(&mut self, at: u64, event: Event) -> Result<Reply, JournalError> {
        let record = Record::seal(self.last_seq + 1, at, event, &self.last_hash);
        self.journal.append(&record)?;
        self.last_seq = record.seq;
        self.last_hash = record.hash.clone();
        self.last_at = at;
        Ok(self.engine.apply(at, &record.event))
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn into_journal(self) -> J {
        self.journal
    }
}
