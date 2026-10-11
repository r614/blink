//! Differential oracle for Blink: compares the port against the original game and turns the
//! differences into a scorecard and a ranked work list (research/dishonored/ORACLE.md).
//!
//! This crate holds everything that needs neither the game nor the port's game code:
//! - [`cache`]: original-side answers keyed by (exe SHA-256, subject, probe, query hash);
//! - [`query`]: query sets (collision first) and the import of `probe_launch` captures;
//! - [`scenario`]: the scenario spec (map, setup, fixed-step input, checkpoints, records);
//! - [`trace`]: per-tick behaviour traces;
//! - [`anim`]: per-tick AnimTree node states and bones of a pawn (capture sidecar and port),
//!   the animation residuals and metrics;
//! - [`diff`]: agreement, clustering by object, first divergence;
//! - [`report`]: scorecard, work list and evidence dossiers;
//! - [`coverage`]: the static scan of every package against what the port's source handles.
//! - [`structure`]: field-by-field structural diff of live objects (the primary proof);
//! - [`sampling`]: primitive-directed queries, trace-profile expansion, densification;
//! - [`residual`]: cause classification of residuals (the definition of done);
//! - [`index`]: the static index of the original (phase 3): the exe's natives, vtables,
//!   cross-references, trace call sites and decompiled C, and the decompiled UnrealScript.
//!
//! The port side (the headless runner calling `dishonored_world`, `dishonored_sim`...) lives in
//! the `oracle` tool so this crate builds in seconds and never depends on Bevy or the runtime.

pub mod anim;
pub mod cache;
pub mod coverage;
pub mod diff;
pub mod event_map;
pub mod index;
pub mod query;
pub mod report;
pub mod residual;
pub mod sampling;
pub mod scenario;
pub mod structure;
pub mod trace;

/// The default GOG install (as `dishonored_assets::content`), overridable with `DISHONORED_DIR`.
pub fn install_dir() -> std::path::PathBuf {
    std::env::var_os("DISHONORED_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Program Files\GOG Galaxy\Games\Dishonored"))
}
