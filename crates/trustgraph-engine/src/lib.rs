//! The deterministic runtime core behind `trustgraph serve` and the simulator.
//!
//! [`Engine::apply`] is a pure state machine: the caller passes the time and
//! one [`Event`]; the engine returns one [`Reply`]. There is no clock, no
//! randomness and no I/O inside it, so replaying the same events gives the
//! same replies and the same [`Engine::state_digest`].

mod engine;
mod event;
mod journal;
mod rules;
mod runtime;

pub use engine::Engine;
#[cfg(feature = "fault-injection")]
pub use event::Fault;
pub use event::{Event, Reply};
pub use journal::{FileJournal, Journal, JournalError, MemoryJournal, Record};
pub use runtime::{OpenError, Runtime};
