//! Weapons a dead pawn lets go of.
//!
//! A death animation's `DisNotify_DropItem` (0x00A8D400; only for a pawn whose health is below 1,
//! pawn slot 348 0x00B49480) queues a drop on the pawn's inventory (`m_bNeedToDropAnItem`,
//! 0x00BFEC20) of every equipped item (`m_WhichItem` 0), every attached one (1) or the item of
//! one usage (2), at `m_DropVelocity` turned by the pawn mesh's LocalToWorld. Every NPC death,
//! fatality and assassination slave of the AnimSets carries one. The limp death
//! (`StateNPCMasterDead_Limp`, straight to the ragdoll) drops every attached item when it starts
//! (`m_bNeedToDropItems`, set by its enter 0x00A9FC40 and run by its update 0x00A9CFC0) at
//! `m_ThrowMomentum` x 0.2 (the port's ragdoll has no throw momentum: zero).
//!
//! The inventory's tick (0x00C19A20 -> 0x00C19080) calls the item's Drop (0x00C0FB30): the item
//! tweak's `m_pPickupToDrop` is spawned with `m_fPickupToDrop_Chance` (0x00C0BC00; class default
//! 1.0, which no weapon overrides) at the item mesh's transform turned -90 degrees in yaw (where
//! the pickup's `_world` static mesh covers the held mesh), moving at the item's velocity plus the
//! drop velocity, without spin, and handed to rigid-body physics. The item's velocity (0x00C05910)
//! comes from its first melee extent's cache, which records only while a melee context of the
//! item runs (0x00C061F0) and is reset when none does: a dying pawn's item has none, so the drop
//! velocity alone moves the pickup.
//!
//! The pickup's physics is PhysX in the original. Here, presentation only like the ragdoll, the
//! pickup is a rigid set of four point masses (the ends of its mesh box's longest axis and the
//! sides of its second, each a sphere of half the thinnest extent) held at their distances,
//! falling under gravity onto the level's floor and kept out of its walls (the ragdoll's floor
//! grid and wall triangles, around the drop point). Resting on an edge is unstable for that
//! body, so a blade settles flat as its hull does. It runs at the ragdoll's fixed step.

use crate::ragdoll::{DAMPING, FRICTION, FloorGrid, ITERATIONS, MAX_TIME, SLEEP_SPEED, SLEEP_TIME, STEP, WallTri, frame};
use bevy::prelude::*;
use std::collections::HashSet;

/// A weapon let go of this frame: the pawn, the weapon (`WeaponLibrary` index), the pickup mesh's
/// world transform (Dishonored space) and its velocity (uu/s).
pub struct DropRequest {
    pub pawn: u32,
    pub weapon: usize,
    pub frame: Mat4,
    pub velocity: Vec3,
}

#[derive(Resource, Default)]
pub struct DroppedWeapons {
    pub pending: Vec<DropRequest>,
    /// (pawn, weapon) let go of: no longer drawn in the hand until the pawn lives again.
    pub dropped: HashSet<(u32, usize)>,
    /// (pawn, weapon) drawn in the hand last frame: a corpse first seen without it has nothing to
    /// drop.
    pub held: HashSet<(u32, usize)>,
    live: Vec<Dropped>,
}

/// Most dropped weapons kept (the oldest goes first).
const MAX_DROPPED: usize = 64;

/// The item mesh -> pickup mesh turn of the Drop (0x00C0FB30: the rotator (0, -16384, 0) applied
/// before the item mesh's LocalToWorld).
pub fn pickup_turn() -> Mat4 {
    dishonored_assets::util::rotator_matrix(0, -16384, 0)
}

/// The four point masses of a mesh box (mesh space): the ends of its longest axis and the sides of
/// its second, at the box centre; and their radius, half the thinnest extent.
pub fn body_points(lo: Vec3, hi: Vec3) -> ([Vec3; 4], f32) {
    let e = hi - lo;
    let c = (lo + hi) * 0.5;
    let mut ax = [0usize, 1, 2];
    ax.sort_by(|a, b| e[*b].total_cmp(&e[*a]).then(a.cmp(b)));
    let a = Vec3::AXES[ax[0]] * (e[ax[0]] * 0.5);
    let b = Vec3::AXES[ax[1]] * (e[ax[1]] * 0.5);
    ([c + a, c - a, c + b, c - b], (e[ax[2]] * 0.5).max(0.5))
}

#[derive(Component)]
pub struct DroppedJoint;

/// A dropped pickup in flight or at rest. Points are relative to `origin` (the drop point), the
/// frame of its floor grid and walls.
pub struct Dropped {
    pub pawn: u32,
    pub origin: Vec3,
    pub pts: [Vec3; 4],
    pub prev: [Vec3; 4],
    rest: [[f32; 4]; 4],
    p0: [Vec3; 4],
    m0: Mat4,
    radius: f32,
    floor: FloorGrid,
    walls: Vec<WallTri>,
    gravity: f32,
    pub elapsed: f32,
    acc: f32,
    /// Seconds every point has moved slower than PhysX's sleep speed.
    still: f32,
    entities: Vec<Entity>,
}

impl Dropped {
    /// A pickup whose mesh is at `m0` (world) with its mesh box `lo..hi`, moving at `velocity`.
    /// `floor_at` is the floor height under an origin-relative (x, y), `walls` the level's
    /// collision triangles around it (origin-relative; the steep ones are kept).
    #[allow(clippy::too_many_arguments)]
    pub fn new(pawn: u32, m0: Mat4, lo: Vec3, hi: Vec3, velocity: Vec3, floor_at: &dyn Fn(f32, f32) -> f32, walls: &[[Vec3; 3]], gravity: f32) -> Dropped {
        let origin = m0.w_axis.truncate();
        let (local, radius) = body_points(lo, hi);
        let p0 = local.map(|p| m0.transform_point3(p) - origin);
        let mut rest = [[0.0; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                rest[i][j] = (p0[j] - p0[i]).length();
            }
        }
        let back = velocity * STEP;
        Dropped { pawn, origin, pts: p0, prev: p0.map(|p| p - back), rest, p0, m0, radius, floor: FloorGrid::sample(floor_at), walls: walls.iter().filter_map(|t| WallTri::new(*t)).collect(), gravity, elapsed: 0.0, acc: 0.0, still: 0.0, entities: Vec::new() }
    }

    pub fn step(&mut self) {
        let g = Vec3::Z * self.gravity * STEP * STEP;
        for i in 0..4 {
            let v = (self.pts[i] - self.prev[i]) * DAMPING;
            self.prev[i] = self.pts[i];
            self.pts[i] += v + g;
        }
        for _ in 0..ITERATIONS {
            // Rigid: every pair at its start distance (equal masses).
            for i in 0..4 {
                for j in i + 1..4 {
                    let d = self.pts[j] - self.pts[i];
                    let len = d.length();
                    if len < 1e-4 {
                        continue;
                    }
                    let corr = d * ((len - self.rest[i][j]) / len) * 0.5;
                    self.pts[i] += corr;
                    self.pts[j] -= corr;
                }
            }
            for i in 0..4 {
                for w in &self.walls {
                    if let Some((dir, depth)) = w.push(self.pts[i], self.radius) {
                        self.pts[i] += dir * depth;
                        let vn = (self.pts[i] - self.prev[i]).dot(dir);
                        if vn < 0.0 {
                            self.prev[i] += dir * vn;
                        }
                    }
                }
                let min = self.floor.at(self.pts[i].x, self.pts[i].y) + self.radius;
                if self.pts[i].z < min {
                    self.pts[i].z = min;
                    if self.prev[i].z < min {
                        self.prev[i].z = min;
                    }
                    let h = (self.pts[i] - self.prev[i]).truncate() * (1.0 - FRICTION);
                    self.prev[i].x += h.x;
                    self.prev[i].y += h.y;
                }
            }
        }
    }

    /// Advance by `dt` real seconds (fixed steps); a pickup sleeps as the ragdoll does (PhysX's
    /// sleep speed held for its sleep time, at the latest after the ragdoll's time).
    pub fn advance(&mut self, dt: f32) {
        self.acc += dt;
        while self.acc >= STEP && self.elapsed < MAX_TIME {
            self.acc -= STEP;
            self.elapsed += STEP;
            self.step();
            let fastest = (0..4).map(|i| (self.pts[i] - self.prev[i]).length() / STEP).fold(0.0f32, f32::max);
            self.still = if fastest < SLEEP_SPEED { self.still + STEP } else { 0.0 };
            if self.still >= SLEEP_TIME {
                self.elapsed = self.elapsed.max(MAX_TIME);
            }
        }
    }

    /// The pickup mesh's world transform now: the start transform carried by the points' turn.
    pub fn transform(&self) -> Mat4 {
        let mean = |p: &[Vec3; 4]| (p[0] + p[1] + p[2] + p[3]) * 0.25;
        let (c0, c) = (mean(&self.p0), mean(&self.pts));
        let q = frame(self.p0[0] - self.p0[1], self.p0[2] - self.p0[3], self.pts[0] - self.pts[1], self.pts[2] - self.pts[3]);
        Mat4::from_translation(self.origin + c) * Mat4::from_quat(q) * Mat4::from_translation(-(self.origin + c0)) * self.m0
    }
}

/// Spawn the pickups let go of this frame, run them, and drop those whose pawn lives again or is
/// gone.
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    drops: Option<ResMut<DroppedWeapons>>,
    weapons: Option<Res<crate::characters::WeaponLibrary>>,
    session: Res<crate::session::Session>,
    view: Res<crate::session::SessionView>,
    time: Res<Time>,
    mut joints: Query<&mut Transform, With<DroppedJoint>>,
) {
    let (Some(mut drops), Some(weapons)) = (drops, weapons) else { return };
    let mv = &session.content.sim.gameplay.movement;
    let world = &session.content.collision;
    for r in std::mem::take(&mut drops.pending) {
        let Some(pk) = weapons.weapons.get(r.weapon).and_then(|w| w.pickup.as_ref()) else { continue };
        let Some(part) = pk.gpu.parts.first() else { continue };
        let origin = r.frame.w_axis.truncate();
        let floor_at = |x: f32, y: f32| -> f32 {
            use dishonored_world::WorldQuery;
            let p = origin + Vec3::new(x, y, 0.0);
            world.raycast(p + Vec3::Z * 60.0, -Vec3::Z, 600.0, dishonored_world::channel::NON_PAWN).map(|h| h.point.z - origin.z).unwrap_or(-540.0)
        };
        let reach = crate::ragdoll::WALL_REACH;
        let q = dishonored_world::geom::Aabb { min: origin - Vec3::new(reach, reach, 600.0), max: origin + Vec3::new(reach, reach, 200.0) };
        let mut ids = Vec::new();
        world.bvh.query_aabb(&q, |i| ids.push(i));
        ids.sort_unstable();
        let walls: Vec<[Vec3; 3]> = ids
            .into_iter()
            .map(|i| &world.tris[i as usize])
            .filter(|t| t.channels & dishonored_world::channel::NON_PAWN != 0)
            .map(|t| [t.a - origin, t.b - origin, t.c - origin])
            .collect();
        let mut d = Dropped::new(r.pawn, r.frame, pk.lo, pk.hi, r.velocity, &floor_at, &walls, mv.gravity_z);
        let joint = commands.spawn((Transform::from_matrix(crate::characters::to_bevy(r.frame)), DroppedJoint)).id();
        d.entities.push(joint);
        for (mesh, mat) in &part.sections {
            let e = commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), bevy::mesh::skinning::SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: vec![joint] }, Transform::default(), bevy::camera::visibility::NoFrustumCulling)).id();
            d.entities.push(e);
        }
        drops.live.push(d);
    }
    // A pawn living again (a respawn) or gone takes its weapons back; the oldest go past the cap.
    let dead = |id: u32| view.others.iter().chain(view.me.iter()).any(|p| p.id == id && !p.alive);
    let mut keep: Vec<bool> = drops.live.iter().map(|d| dead(d.pawn)).collect();
    let mut excess = keep.iter().filter(|k| **k).count().saturating_sub(MAX_DROPPED);
    for k in keep.iter_mut() {
        if excess == 0 {
            break;
        }
        if *k {
            *k = false;
            excess -= 1;
        }
    }
    let mut i = 0;
    drops.live.retain(|d| {
        let k = keep[i];
        i += 1;
        if !k {
            for e in &d.entities {
                commands.entity(*e).despawn();
            }
        }
        k
    });
    let dt = time.delta_secs().min(0.1);
    for d in drops.live.iter_mut() {
        if d.elapsed < MAX_TIME {
            d.advance(dt);
            if let Some(mut t) = d.entities.first().and_then(|e| joints.get_mut(*e).ok()) {
                *t = Transform::from_matrix(crate::characters::to_bevy(d.transform()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blade dropped edge-down onto a flat floor settles lying flat on it.
    #[test]
    fn a_dropped_blade_settles_flat() {
        let (lo, hi) = (Vec3::new(-2.0, -40.0, -0.5), Vec3::new(2.0, 40.0, 0.5));
        // Long axis Y, width X, thin Z; stood on its edge (width vertical), 60 uu up, turned.
        let m0 = Mat4::from_translation(Vec3::new(0.0, 0.0, 60.0)) * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2 - 0.05) * Mat4::from_rotation_z(0.3);
        let mut d = Dropped::new(1, m0, lo, hi, Vec3::new(30.0, 0.0, 0.0), &|_, _| -60.0, &[], -1050.0);
        while d.elapsed < MAX_TIME {
            d.advance(STEP);
        }
        let m = d.transform();
        // The thin axis points up (flat), and the blade lies on the floor (z = -60 + radius).
        let thin = m.transform_vector3(Vec3::Z).normalize();
        assert!(thin.z.abs() > 0.95, "thin axis {thin}");
        let z = m.w_axis.z - d.origin.z;
        assert!((z - (-60.0 + 0.5)).abs() < 1.0, "rest height {z}");
    }

    /// The pickup keeps its shape: the transform carries the mesh box rigidly.
    #[test]
    fn the_transform_is_rigid() {
        let (lo, hi) = (Vec3::new(-1.0, -30.0, -3.0), Vec3::new(1.0, 30.0, 3.0));
        let m0 = Mat4::from_translation(Vec3::new(5.0, 7.0, 20.0)) * Mat4::from_rotation_x(0.7);
        let mut d = Dropped::new(1, m0, lo, hi, Vec3::new(100.0, -50.0, 200.0), &|_, _| -20.0, &[], -1050.0);
        for _ in 0..90 {
            d.advance(STEP);
        }
        let m = d.transform();
        let (pts, _) = body_points(lo, hi);
        for (k, p) in pts.iter().enumerate() {
            let w = m.transform_point3(*p) - d.origin;
            assert!((w - d.pts[k]).length() < 0.5, "point {k}: {w} vs {}", d.pts[k]);
        }
    }
}
