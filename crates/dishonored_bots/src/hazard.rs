//! Ground the test bots keep off: drops they cannot see the bottom of, deep (swimmable) water
//! and pain-causing physics volumes. The original level lets a player walk off a roof or a pier
//! into the sea and swim out past the sea's water volume (then fall to `KillZ`); bots must not.
//! These are probes of the bot's own, read through `WorldQuery` like everything else a client
//! knows (the map is loaded on every client).

use dishonored_world::{WorldQuery, channel};
use glam::Vec3;

/// Pawn centre above its feet (cylinder half height).
pub const CENTRE: f32 = 90.0;
/// The largest step down a bot takes without a navmesh link (stairs and kerbs are smaller).
pub const MAX_DROP: f32 = 260.0;

/// Deep water (a swimmable `DishonoredWaterVolume`) at `p`.
pub fn deep_water(world: &dyn WorldQuery, p: Vec3) -> bool {
    world.water_at(p).is_some_and(|w| !w.shallow)
}

/// A pain-causing physics volume touches a pawn standing with its centre at `p`.
pub fn pain_at(world: &dyn WorldQuery, p: Vec3) -> bool {
    world.physics_volumes().iter().any(|v| v.pain.as_ref().is_some_and(|pain| pain.damage_per_sec > 0.0) && v.touches(p, 34.0, CENTRE))
}

/// The floor under a pawn centre `p`, within `depth` below its feet.
pub fn floor_below(world: &dyn WorldQuery, p: Vec3, depth: f32) -> Option<f32> {
    world.raycast(p, -Vec3::Z, CENTRE + depth, channel::PAWN).map(|h| p.z - h.distance)
}

/// Whether a pawn at `from` (centre) may move `dist` along the horizontal `dir`: a wall in the
/// way is harmless (the move is blocked); otherwise there must be floor within [`MAX_DROP`]
/// below the point ahead, not in deep water nor in a pain volume.
pub fn safe_ahead(world: &dyn WorldQuery, from: Vec3, dir: Vec3, dist: f32) -> bool {
    let dir = Vec3::new(dir.x, dir.y, 0.0).normalize_or_zero();
    if dir == Vec3::ZERO {
        return true;
    }
    if world.raycast(from, dir, dist, channel::PAWN).is_some() {
        return true;
    }
    let ahead = from + dir * dist;
    let Some(z) = floor_below(world, ahead, MAX_DROP) else { return false };
    let stand = Vec3::new(ahead.x, ahead.y, z + CENTRE);
    !deep_water(world, stand) && !pain_at(world, stand)
}
