//! The deterministic runtime core behind `trustgraph serve` and the simulator.
//!
//! [`Engine::apply`] is a pure state machine: the caller passes the time and
//! one [`Event`]; the engine returns one [`Reply`]. There is no clock, no
//! randomness and no I/O inside it, so replaying the same events gives the
//! same replies and the same [`Engine::state_digest`].

mod engine;
mod event;

pub use engine::Engine;
pub use event::{Event, Fault, Reply};
