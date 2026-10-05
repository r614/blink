//! UE3 Kismet as Dishonored uses it.
//!
//! - [`model`]: a typed, complete model of the Kismet of a set of maps plus the always-loaded
//!   global game sequence (`DishonoredGameFull_P`), read from the install by [`load`] (feature
//!   `load`).
//! - [`runtime`]: a deterministic, tick-driven interpreter reproducing the engine's
//!   activation semantics (decoded from `Dishonored.exe`, see research/dishonored/KISMET.md):
//!   per-sequence op stacks, input/output impulses with queued activations and delays,
//!   latent ops, event trigger rules, remote events across levels, attached events,
//!   variables, and the engine's flow-control actions.
//! - [`host`]: everything the game must do for Kismet (story flags, objectives, NPCs,
//!   travel, conversations, sounds...) behind a trait, implemented by the mission runtime.

pub mod host;
pub mod model;
pub mod runtime;

#[cfg(feature = "load")]
pub mod load;

pub use host::{Host, HostResult, NullHost};
pub use model::*;
pub use runtime::{Coverage, Kismet};
