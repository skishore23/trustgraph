//! The deterministic runtime core behind `trustgraph serve` and the simulator.
//!
//! [`Engine::apply`] is a pure state machine: the caller passes the time and
//! one [`Event`]; the engine returns one [`Reply`]. There is no clock, no
//! randomness and no I/O inside it, so replaying the same events gives the
//! same replies and the same [`Engine::state_digest`].
//!
//! [`Runtime`] journals each event before the engine applies it and rebuilds
//! the engine by replay on open. Agent input that fails
//! [`Event::validate_shape`] is refused before the journal and changes
//! nothing.
//!
//! Policy activation: EVERY activation, even of an identical policy or of an
//! unrelated edit, voids all approvals and forgets every pending decision.
//! Agents must ask again and reviewers must approve again.
//!
//! Growth: the journal grows without bound by design; there is no compaction
//! yet. Artifacts, Graph operations and lineage also grow with every agent
//! call (remembered pending decisions are capped at 64 per agent and 1,024 in
//! all; stored proposals at 32 for the operator, 32 for all agents with 8
//! per agent, and one, the latest, from the policy file). A server that
//! exposes this runtime to agents must add per-agent quotas, as
//! `trustgraph serve` does; [`Runtime::agent_usage`] gives what each agent's
//! journal records take, rebuilt at open.
//!
//! Errors: when [`Runtime::handle`] or [`Journal::append`] returns `Err`, the
//! outcome is unknown. Drop the `Runtime` and [`Runtime::open`] it again.
//!
//! Journal compatibility: until a journal format version is introduced, a
//! journal is only guaranteed to replay on the engine version
//! that wrote it.

mod engine;
mod event;
mod journal;
mod rules;
mod runtime;

/// True when this build has the `fault-injection` feature, i.e. deliberate
/// engine bugs are compiled in. Production builds must have it false.
pub const FAULT_INJECTION: bool = cfg!(feature = "fault-injection");

pub use engine::{Engine, LiveApproval, PendingDecision, ProposalOrigin, StoredProposal};
#[cfg(feature = "fault-injection")]
pub use event::Fault;
pub use event::{Event, Reply};
pub use journal::{FileJournal, Journal, JournalError, MemoryJournal, Record};
pub use runtime::{AgentUsage, FileOutcome, OpenError, PolicyFile, PolicySource, Runtime};
