//! Differential oracle for Blink: compares the port against the original game and turns the
//! differences into a scorecard and a ranked work list (research/dishonored/ORACLE.md).
//!
//! This crate holds everything that needs neither the game nor the port's game code:
//! - [`cache`]: original-side answers keyed by (exe SHA-256, subject, probe, query hash);
//! - [`query`]: query sets (collision first) and the import of `probe_launch` captures;
//! - [`scenario`]: the scenario spec (map, setup, fixed-step input, checkpoints, records);
//! - [`trace`]: per-tick behaviour traces;
//! - [`diff`]: agreement, clustering by object, first divergence;
//! - [`report`]: scorecard, work list and evidence dossiers;
//! - [`coverage`]: the static scan of every package against what the port's source handles.
//!
//! The port side (the headless runner calling `dishonored_world`, `dishonored_sim`...) lives in
//! the `oracle` tool so this crate builds in seconds and never depends on Bevy or the runtime.

pub mod cache;
pub mod coverage;
pub mod diff;
pub mod query;
pub mod report;
pub mod scenario;
pub mod trace;

/// The default GOG install (as `dishonored_assets::content`), overridable with `DISHONORED_DIR`.
pub fn install_dir() -> std::path::PathBuf {
    std::env::var_os("DISHONORED_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Program Files\GOG Galaxy\Games\Dishonored"))
}
