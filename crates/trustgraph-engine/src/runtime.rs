//! Journal-first runtime: every event is made durable before the engine sees
//! it, and the engine is rebuilt by replay on open.

use thiserror::Error;
use trustgraph_policy::{Diagnostic, Severity, lint, parse, run_tests};

use crate::engine::Engine;
#[cfg(feature = "fault-injection")]
use crate::event::Fault;
use crate::event::{Event, Reply};
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

fn engine_rejected(message: String) -> OpenError {
    OpenError::InvalidPolicy(vec![Diagnostic {
        severity: Severity::Error,
        path: String::new(),
        code: "engine_rejected",
        message,
        suggestion: None,
    }])
}

impl<J: Journal> Runtime<J> {
    pub fn open(journal: J, policy_yaml: &str, at: u64) -> Result<Self, OpenError> {
        Self::open_engine(journal, policy_yaml, at, Engine::new)
    }

    /// Like `open`, with a deliberate engine bug for the simulator. The fault
    /// applies to replay and to live handling alike.
    #[cfg(feature = "fault-injection")]
    #[doc(hidden)]
    pub fn open_with_fault(
        journal: J,
        policy_yaml: &str,
        at: u64,
        fault: Fault,
    ) -> Result<Self, OpenError> {
        Self::open_engine(journal, policy_yaml, at, || Engine::new().with_fault(fault))
    }

    /// `open`, building every engine it needs with `new_engine`.
    fn open_engine(
        mut journal: J,
        policy_yaml: &str,
        at: u64,
        new_engine: impl Fn() -> Engine,
    ) -> Result<Self, OpenError> {
        let wanted = validate(policy_yaml)?;
        let genesis = |policy_yaml: &str| Event::Genesis {
            policy: policy_yaml.to_string(),
        };
        let records = journal.load()?;
        if records.is_empty() {
            // Nothing may be journaled that the engine would refuse.
            let reply = new_engine().apply(at, &genesis(policy_yaml));
            if let Reply::Rejected { code: _, message } = reply {
                return Err(engine_rejected(message));
            }
        }
        let mut engine = new_engine();
        let mut previous_at = 0;
        // Digest of the policy in the last Genesis the engine accepted.
        let mut file_digest: Option<String> = None;
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
            match (&reply, is_genesis) {
                (Reply::Rejected { code, message }, true) if index == 0 => {
                    return Err(replay(format!("Genesis was rejected: {code}: {message}")));
                }
                // A later rejected Genesis was rejected the same way live.
                (Reply::Rejected { .. }, _) => {}
                (_, true) => file_digest = engine.policy().map(|p| p.digest()),
                _ => {}
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
        // The file wins only when it changed since the last start; an in-band
        // activation since then is kept.
        if file_digest.as_deref() != Some(wanted.as_str()) {
            let at = at.max(runtime.last_at);
            let reply = runtime.append_and_apply(at, genesis(policy_yaml))?;
            if let Reply::Rejected { message, .. } = reply {
                return Err(engine_rejected(message));
            }
        }
        Ok(runtime)
    }

    /// Journal `event`, then apply it. Events the engine rejects on its state
    /// are journaled too: the engine's clock moves even when it refuses an
    /// event, and replay must reach the same state. These are refused without
    /// journaling and change nothing, not even the clock: `Genesis` (written
    /// only by `open`), time going backwards (it would make the journal
    /// unreplayable), an event that fails `Event::validate_shape`, and an
    /// event whose record would exceed the journal's line cap.
    ///
    /// On `Err` the outcome is UNKNOWN: the record may or may not be durable.
    /// The caller must drop this `Runtime` and call `open` again, which
    /// replays the record if it is durable; the event's reply is learned only
    /// by asking again. A poisoned `FileJournal` makes every later `handle`
    /// fail until the runtime is reopened.
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
        if let Err(rejected) = event.validate_shape() {
            return Ok(rejected);
        }
        self.append_and_apply(at, event)
    }

    fn append_and_apply(&mut self, at: u64, event: Event) -> Result<Reply, JournalError> {
        let record = Record::seal(self.last_seq + 1, at, event, &self.last_hash);
        if let Err(len) = record.line_len() {
            return Ok(Reply::Rejected {
                code: "record_too_large".into(),
                message: format!(
                    "the event encodes to {len} bytes; the journal limit is {}",
                    Record::MAX_LINE
                ),
            });
        }
        self.journal.append(&record)?;
        self.last_seq = record.seq;
        self.last_hash = record.hash.clone();
        self.last_at = at;
        Ok(self.engine.apply(at, &record.event))
    }

    /// `seq` of the last durable record; it moves only when `handle`
    /// journals an event.
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn into_journal(self) -> J {
        self.journal
    }
}
