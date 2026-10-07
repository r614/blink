//! Bot navigation: paths on the map's original navigation mesh (`dishonored_world::nav`,
//! decoded from the `L_*_Nav` packages), reached through `WorldQuery::nav()`. Bots still act
//! purely through `PlayerInput`; this only chooses where to steer.
//!
//! The previous waypoint graph over AI points with raycast and "drop" edges was invented
//! and has been removed: the original navmesh has no drop or jump edges, so neither do bots.

use dishonored_world::{PawnBox, WorldQuery, channel};
use glam::Vec3;

pub use dishonored_world::nav::{Follower, NavWorld, PathOpts};

/// Former name of the bot navigation graph, kept for callers.
pub type NavGraph = NavWorld;

/// The player's box is wider than the navmesh's agent (34 against 31 uu): corners of a path
/// are pushed this much further out of the turn so the box clears the wall end it bends round.
const CORNER_PUSH: f32 = 30.0;

/// Path following for a player-sized bot: like [`Follower`], but a corner is passed once the
/// next waypoint is in reach of a straight move (a box sweep at knee height is clear), and
/// corners are pushed out of the turn.
#[derive(Debug, Clone, Default)]
pub struct Route {
    pub path: Vec<Vec3>,
    pub goal: Option<Vec3>,
    replan_in: u32,
    /// The last query found no route.
    pub failed: bool,
}

impl Route {
    pub fn force_replan(&mut self) {
        self.replan_in = 0;
    }

    pub fn clear(&mut self) {
        *self = Route::default();
    }

    /// Point to steer toward this tick to reach `goal` from the pawn centre `me`. Without a
    /// navmesh it returns `goal`.
    pub fn steer(&mut self, world: &dyn WorldQuery, me: Vec3, goal: Vec3) -> Vec3 {
        let Some(nav) = world.nav() else {
            self.failed = false;
            return goal;
        };
        self.replan_in = self.replan_in.saturating_sub(1);
        let moved = self.goal.is_none_or(|g| g.truncate().distance(goal.truncate()) > 150.0 || (g.z - goal.z).abs() > 120.0);
        if self.replan_in == 0 || moved {
            self.replan_in = 60;
            self.goal = Some(goal);
            match nav.find_path(me, goal, &PathOpts::default()) {
                Some(p) => {
                    self.path = push_corners(me, &p);
                    self.failed = false;
                }
                None => {
                    self.path.clear();
                    self.failed = true;
                }
            }
        }
        while self.path.len() > 1 && (self.path[0].truncate().distance(me.truncate()) < 24.0 || clear_move(world, me, self.path[1])) {
            self.path.remove(0);
        }
        match self.path.first() {
            Some(&w) => Vec3::new(w.x, w.y, me.z.max(w.z)),
            None => goal,
        }
    }
}

/// Push each inner corner of `path` (started from `from`) out of its turn by [`CORNER_PUSH`].
fn push_corners(from: Vec3, path: &[Vec3]) -> Vec<Vec3> {
    let mut out = path.to_vec();
    for i in 0..path.len().saturating_sub(1) {
        let prev = if i == 0 { from } else { path[i - 1] };
        let d_in = (path[i] - prev).truncate().normalize_or_zero();
        let d_out = (path[i + 1] - path[i]).truncate().normalize_or_zero();
        let away = (d_in - d_out).normalize_or_zero();
        out[i] += away.extend(0.0) * CORNER_PUSH;
    }
    out
}

/// A straight walk from the pawn centre `me` to the floor point `to` is free (box sweep above
/// step height, so kerbs and stairs do not count).
fn clear_move(world: &dyn WorldQuery, me: Vec3, to: Vec3) -> bool {
    let delta = Vec3::new(to.x - me.x, to.y - me.y, 0.0);
    if (to.z - (me.z - 90.0)).abs() > 60.0 {
        return false;
    }
    let b = PawnBox::new(me + Vec3::Z * 25.0, 34.0, 55.0);
    world.sweep_box(&b, delta, channel::PAWN).is_none()
}
