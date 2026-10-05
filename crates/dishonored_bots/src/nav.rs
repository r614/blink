//! Bot navigation: paths on the map's original navigation mesh (`dishonored_world::nav`,
//! decoded from the `L_*_Nav` packages), reached through `WorldQuery::nav()`. Bots still act
//! purely through `PlayerInput`; this only chooses where to steer.
//!
//! The previous waypoint graph over AI points with raycast and "drop" edges was invented
//! and has been removed: the original navmesh has no drop or jump edges, so neither do bots.

pub use dishonored_world::nav::{Follower, NavWorld, PathOpts};

/// Former name of the bot navigation graph, kept for callers.
pub type NavGraph = NavWorld;
