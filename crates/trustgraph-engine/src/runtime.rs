//! Journal-first runtime: every event is made durable before the engine sees
//! it, and the engine is rebuilt by replay on open.

use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

use trustgraph_policy::{Diagnostic, Policy, Severity, lint, parse, run_tests};

use crate::engine::{Engine, ProposalOrigin};
#[cfg(feature = "fault-injection")]
use crate::event::Fault;
use crate::event::{Event, Reply};
use crate::journal::{GENESIS_PREV, Journal, JournalError, Record};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum OpenError {
    #[error("journal error: {0}")]
    Journal(#[from] JournalError),
    #[error("the policy file is invalid ({} diagnostics)", .0.len())]
    InvalidPolicy(Vec<Diagnostic>),
    #[error("replay failed at record {seq}: {reason}")]
    Replay { seq: u64, reason: String },
}

/// Where the active policy came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySource {
    /// The policy file given to `open` (the last `Genesis`).
    File,
    /// An in-service `ActivatePolicy` since the last `Genesis`.
    Activated,
}

impl PolicySource {
    pub fn as_str(self) -> &'static str {
        match self {
            PolicySource::File => "file",
            PolicySource::Activated => "activated",
        }
    }
}

/// What `open` does with a policy file that differs from the active policy
/// of a non-empty journal. On an empty journal the file is always applied.
/// A file that fails validation on a non-empty journal is ignored under
/// `Propose` and is an error under `Activate`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyFile {
    /// Store it as a file proposal for the operator; the active policy stays.
    #[default]
    Propose,
    /// Apply it (a new `Genesis`), as a person at the terminal asked.
    Activate,
}

/// What `open` did with the policy file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOutcome {
    /// The journal was empty: the file is the first policy.
    Genesis,
    /// The file encodes the active policy.
    Unchanged,
    /// The file differs from the active policy but was applied or proposed
    /// before with this digest, so nothing was written. `stored` is who
    /// holds a stored proposal with this digest now, if anyone does.
    AlreadyRecorded {
        digest: String,
        stored: Option<ProposalOrigin>,
    },
    /// The journal is not empty and the file fails validation: it was
    /// ignored, nothing was written, and the active policy stays.
    Invalid { diagnostics: Vec<Diagnostic> },
    /// The file differs: it was stored as a file proposal (`id`).
    Proposed { id: String },
    /// `PolicyFile::Activate`: the file replaced `previous`.
    Activated { previous: Box<Policy> },
}

/// What the journal records that name one agent take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentUsage {
    /// Bytes of journal lines, newline included.
    pub journal_bytes: u64,
    /// Artifacts those records created.
    pub artifacts: u64,
}

#[derive(Debug)]
pub struct Runtime<J: Journal> {
    journal: J,
    engine: Engine,
    last_seq: u64,
    last_hash: String,
    last_at: u64,
    file_outcome: FileOutcome,
    usage: BTreeMap<String, AgentUsage>,
}

/// The agent a record is attributed to: `RecordOutput`, `Check`,
/// `SessionCarry` and an agent's `ProposePolicy` name one. `RegisterSource` names none.
fn record_agent(event: &Event) -> Option<&str> {
    match event {
        Event::RecordOutput { agent, .. }
        | Event::Check { agent, .. }
        | Event::SessionCarry { agent, .. } => Some(agent),
        Event::ProposePolicy { agent, .. } => agent.as_deref(),
        _ => None,
    }
}

/// Charge `record` (which created `created` artifacts) to its agent.
fn charge(usage: &mut BTreeMap<String, AgentUsage>, record: &Record, created: usize) {
    let Some(agent) = record_agent(&record.event) else {
        return;
    };
    let bytes = record.line_len().map_or(0, |n| n as u64 + 1);
    let entry = usage.entry(agent.to_string()).or_default();
    entry.journal_bytes = entry.journal_bytes.saturating_add(bytes);
    entry.artifacts = entry.artifacts.saturating_add(created as u64);
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
    /// Replay `journal`. On an empty journal `policy_yaml` becomes the first
    /// policy, and an invalid one is an error. On a non-empty one it never
    /// takes effect by itself: when it differs from the active policy and
    /// this digest was neither applied nor proposed before, it is stored as
    /// the file proposal (`PolicyFile::Propose`); when it is invalid it is
    /// ignored (`FileOutcome::Invalid`).
    pub fn open(journal: J, policy_yaml: &str, at: u64) -> Result<Self, OpenError> {
        Self::open_with(journal, policy_yaml, at, PolicyFile::Propose)
    }

    /// `open`, choosing what a changed policy file does.
    pub fn open_with(
        journal: J,
        policy_yaml: &str,
        at: u64,
        mode: PolicyFile,
    ) -> Result<Self, OpenError> {
        Self::open_engine(journal, policy_yaml, at, mode, Engine::new)
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
        Self::open_engine(journal, policy_yaml, at, PolicyFile::Propose, || {
            Engine::new().with_fault(fault)
        })
    }

    /// `open`, building every engine it needs with `new_engine`.
    fn open_engine(
        mut journal: J,
        policy_yaml: &str,
        at: u64,
        mode: PolicyFile,
        new_engine: impl Fn() -> Engine,
    ) -> Result<Self, OpenError> {
        let validated = validate(policy_yaml);
        let genesis = |policy_yaml: &str| Event::Genesis {
            policy: policy_yaml.to_string(),
        };
        let records = journal.load()?;
        let empty = records.is_empty();
        // The file must be valid wherever it would be applied: on an empty
        // journal, or when the person at the terminal asked for it.
        // Elsewhere an invalid file is ignored, so whoever can edit the file
        // cannot keep a server with a policy from starting.
        let wanted = match validated {
            Ok(digest) => digest,
            Err(OpenError::InvalidPolicy(diagnostics)) if !empty && mode == PolicyFile::Propose => {
                let (mut runtime, _) = Self::replay(journal, records, new_engine)?;
                runtime.file_outcome = FileOutcome::Invalid { diagnostics };
                return Ok(runtime);
            }
            Err(error) => return Err(error),
        };
        if empty {
            // Nothing may be journaled that the engine would refuse.
            let reply = new_engine().apply(at, &genesis(policy_yaml));
            if let Reply::Rejected { code: _, message } = reply {
                return Err(engine_rejected(message));
            }
        }
        let (mut runtime, file_digests) = Self::replay(journal, records, new_engine)?;
        let at = at.max(runtime.last_at);
        let active = runtime.engine.policy().cloned();
        runtime.file_outcome = match active {
            _ if empty => {
                runtime.apply_file(at, genesis(policy_yaml))?;
                FileOutcome::Genesis
            }
            Some(active) if active.digest() == wanted => FileOutcome::Unchanged,
            Some(active) if mode == PolicyFile::Activate => {
                runtime.apply_file(at, genesis(policy_yaml))?;
                FileOutcome::Activated {
                    previous: Box::new(active),
                }
            }
            _ if file_digests.contains(&wanted) => FileOutcome::AlreadyRecorded {
                stored: runtime.engine.proposal_origin(&wanted).cloned(),
                digest: wanted,
            },
            _ => {
                let event = Event::ProposeFromFile {
                    policy: policy_yaml.to_string(),
                };
                match runtime.apply_file(at, event)? {
                    Reply::Proposed { id, .. } => FileOutcome::Proposed { id },
                    other => {
                        return Err(engine_rejected(format!(
                            "the file proposal was not stored: {other:?}"
                        )));
                    }
                }
            }
        };
        Ok(runtime)
    }

    /// Rebuild the engine from `records`, with the digests the policy file
    /// has had: every accepted Genesis and every file proposal.
    fn replay(
        journal: J,
        records: Vec<Record>,
        new_engine: impl Fn() -> Engine,
    ) -> Result<(Self, BTreeSet<String>), OpenError> {
        let mut engine = new_engine();
        let mut previous_at = 0;
        let mut usage = BTreeMap::new();
        // A file is proposed at most once per digest.
        let mut file_digests: BTreeSet<String> = BTreeSet::new();
        for (index, record) in records.iter().enumerate() {
            let replay = |reason: String| OpenError::Replay {
                seq: record.seq,
                reason,
            };
            let is_genesis = matches!(record.event, Event::Genesis { .. });
            let from_file = matches!(record.event, Event::ProposeFromFile { .. });
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
            let artifacts = engine.artifact_count();
            let reply = engine.apply(record.at, &record.event);
            charge(
                &mut usage,
                record,
                engine.artifact_count().saturating_sub(artifacts),
            );
            match (&reply, is_genesis) {
                (Reply::Rejected { code, message }, true) if index == 0 => {
                    return Err(replay(format!("Genesis was rejected: {code}: {message}")));
                }
                // A later rejected Genesis was rejected the same way live.
                (Reply::Rejected { .. }, _) => {}
                (_, true) => file_digests.extend(engine.policy().map(|p| p.digest())),
                (Reply::Proposed { id, .. }, _) if from_file => {
                    file_digests.insert(id.clone());
                }
                _ => {}
            }
        }
        let (last_seq, last_hash) = records
            .last()
            .map_or((0, GENESIS_PREV.to_string()), |r| (r.seq, r.hash.clone()));
        let runtime = Runtime {
            journal,
            engine,
            last_seq,
            last_hash,
            last_at: previous_at,
            file_outcome: FileOutcome::Genesis,
            usage,
        };
        Ok((runtime, file_digests))
    }

    /// Journal and apply a system event for the policy file; a rejection
    /// means the file cannot be used.
    fn apply_file(&mut self, at: u64, event: Event) -> Result<Reply, OpenError> {
        let reply = self.append_and_apply(at, event)?;
        if let Reply::Rejected { message, .. } = reply {
            return Err(engine_rejected(message));
        }
        Ok(reply)
    }

    /// What `open` did with the policy file.
    pub fn file_outcome(&self) -> &FileOutcome {
        &self.file_outcome
    }

    /// Journal `event`, then apply it. Events the engine rejects on its state
    /// are journaled too: the engine's clock moves even when it refuses an
    /// event, and replay must reach the same state. These are refused without
    /// journaling and change nothing, not even the clock: `Genesis` (written
    /// only by `open`), time going backwards (it would make the journal
    /// unreplayable), `ProposeFromFile` (also only from `open`), an event
    /// that fails `Event::validate_shape`, and an
    /// event whose record would exceed the journal's line cap.
    ///
    /// On `Err` the outcome is UNKNOWN: the record may or may not be durable.
    /// The caller must drop this `Runtime` and call `open` again, which
    /// replays the record if it is durable; the event's reply is learned only
    /// by asking again. A poisoned `FileJournal` makes every later `handle`
    /// fail until the runtime is reopened.
    pub fn handle(&mut self, at: u64, event: Event) -> Result<Reply, JournalError> {
        if matches!(event, Event::Genesis { .. } | Event::ProposeFromFile { .. }) {
            return Ok(Reply::Rejected {
                code: "not_allowed".into(),
                message: "only open writes the policy file's events".into(),
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
        let artifacts = self.engine.artifact_count();
        let reply = self.engine.apply(at, &record.event);
        let created = self.engine.artifact_count().saturating_sub(artifacts);
        charge(&mut self.usage, &record, created);
        Ok(reply)
    }

    /// What the journal records naming each agent take (`RecordOutput`,
    /// `Check`, an agent's `ProposePolicy`), over the whole journal: rebuilt
    /// by replay at open and kept up to date by `handle`. `RegisterSource`
    /// names no agent, so it is in nobody's usage.
    pub fn agent_usage(&self) -> &BTreeMap<String, AgentUsage> {
        &self.usage
    }

    /// `seq` of the last durable record; it moves only when `handle`
    /// journals an event.
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Time of the last durable record; `handle` refuses anything earlier.
    pub fn last_at(&self) -> u64 {
        self.last_at
    }

    /// Whether the active policy is the file's or a later activation.
    pub fn policy_source(&self) -> PolicySource {
        if self.engine.policy_from_genesis() {
            PolicySource::File
        } else {
            PolicySource::Activated
        }
    }

    pub fn journal(&self) -> &J {
        &self.journal
    }

    pub fn into_journal(self) -> J {
        self.journal
    }
}
