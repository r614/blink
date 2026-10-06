//! Presentation-only ragdoll from a pawn's `PhysicsAsset` (research/dishonored/ANIMATION.md
//! §8). The original hands a dying pawn to PhysX at `DishonoredNotify_Ragdoll`; here each
//! physics body is a point mass at its bone, linked to its parent body by its
//! `RB_ConstraintSetup` (rest length from the pose at the hand-over, swing cone from
//! `Swing1/Swing2LimitAngle`), falling under gravity onto the level floor around the corpse
//! (a height grid sampled from the collision at the hand-over: stairs, slopes and ledges) and
//! kept out of the level's walls (the steep collision triangles around the corpse, copied into
//! pawn space at the hand-over; each body is a sphere of its physics radius, U-282).
//!
//! It runs at a fixed step from the pose sampled exactly at the notify time, so a corpse settles
//! the same way on every run. The simulation's corpse position stays authoritative: the ragdoll
//! only moves bones inside the pawn's frame.

use crate::characters::PartGpu;
use dishonored_ir::BoneAsset;
use crate::npc_anim::{Key, Pose};
use dishonored_ir::npc_anim::PhysicsAssetData;
use glam::{Mat4, Quat, Vec3};

/// Fixed step (seconds) and constraint passes per step.
pub const STEP: f32 = 1.0 / 60.0;
const ITERATIONS: usize = 8;
/// Velocity kept per step (air drag of the point masses).
const DAMPING: f32 = 0.985;
/// Horizontal velocity kept on floor contact.
const FRICTION: f32 = 0.6;
/// A ragdoll stops after this long (PhysX puts resting bodies to sleep).
const MAX_TIME: f32 = 4.0;

struct Body {
    bone: usize,
    parent: Option<usize>,
    /// Child body used to orient this one, and a second one (for a frame) when it has several.
    child: Option<usize>,
    child2: Option<usize>,
    pos: Vec3,
    prev: Vec3,
    radius: f32,
    rest: f32,
    swing: f32,
    /// Start pose: global (pawn space) transform of the bone.
    g0: Mat4,
}

pub struct Ragdoll {
    bodies: Vec<Body>,
    /// Per bone of part 0: nearest body at or above it (None: above the top body).
    owner: Vec<Option<usize>>,
    top: usize,
    /// Start globals of part 0 (pawn space).
    g0: Vec<Mat4>,
    floor: FloorGrid,
    walls: Vec<WallTri>,
    gravity: f32,
    elapsed: f32,
    acc: f32,
}

/// Floor heights (pawn space) on a square grid around the pawn's origin, sampled once.
struct FloorGrid {
    h: Vec<f32>,
}

const GRID_N: usize = 17;
const GRID_CELL: f32 = 12.0;

impl FloorGrid {
    fn sample(floor_at: &dyn Fn(f32, f32) -> f32) -> FloorGrid {
        let half = (GRID_N - 1) as f32 * 0.5 * GRID_CELL;
        let mut h = Vec::with_capacity(GRID_N * GRID_N);
        for j in 0..GRID_N {
            for i in 0..GRID_N {
                h.push(floor_at(i as f32 * GRID_CELL - half, j as f32 * GRID_CELL - half));
            }
        }
        FloorGrid { h }
    }

    /// Bilinear height at (x, y), clamped to the grid's edge.
    fn at(&self, x: f32, y: f32) -> f32 {
        let half = (GRID_N - 1) as f32 * 0.5 * GRID_CELL;
        let fx = ((x + half) / GRID_CELL).clamp(0.0, (GRID_N - 1) as f32);
        let fy = ((y + half) / GRID_CELL).clamp(0.0, (GRID_N - 1) as f32);
        let (i, j) = ((fx as usize).min(GRID_N - 2), (fy as usize).min(GRID_N - 2));
        let (tx, ty) = (fx - i as f32, fy - j as f32);
        let g = |a: usize, b: usize| self.h[b * GRID_N + a];
        // a + (b - a) t: exact where the corners agree (a flat floor stays exactly flat).
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let top = lerp(g(i, j), g(i + 1, j), tx);
        let bot = lerp(g(i, j + 1), g(i + 1, j + 1), tx);
        lerp(top, bot, ty)
    }
}

/// Collision triangles steeper than this (|normal.z|) are walls; flatter ones are floor (the
/// grid) or ceiling.
pub const WALL_MAX_NZ: f32 = 0.7;
/// Horizontal reach (uu, from the pawn's origin) of the walls copied at the hand-over: the floor
/// grid's half width plus the largest body radius.
pub const WALL_REACH: f32 = (GRID_N - 1) as f32 * 0.5 * GRID_CELL + 32.0;

/// One wall triangle in pawn space (`n`: unit face normal, the side bodies stay on).
#[derive(Clone, Copy, Debug)]
struct WallTri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    n: Vec3,
}

impl WallTri {
    fn new(t: [Vec3; 3]) -> Option<WallTri> {
        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
        (n != Vec3::ZERO && n.z.abs() < WALL_MAX_NZ).then_some(WallTri { a: t[0], b: t[1], c: t[2], n })
    }

    /// Closest point of the triangle to `p` (Ericson, Real-Time Collision Detection 5.1.5).
    fn closest(&self, p: Vec3) -> Vec3 {
        let (a, b, c) = (self.a, self.b, self.c);
        let (ab, ac, ap) = (b - a, c - a, p - a);
        let (d1, d2) = (ab.dot(ap), ac.dot(ap));
        if d1 <= 0.0 && d2 <= 0.0 {
            return a;
        }
        let bp = p - b;
        let (d3, d4) = (ab.dot(bp), ac.dot(bp));
        if d3 >= 0.0 && d4 <= d3 {
            return b;
        }
        let vc = d1 * d4 - d3 * d2;
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            return a + ab * (d1 / (d1 - d3));
        }
        let cp = p - c;
        let (d5, d6) = (ab.dot(cp), ac.dot(cp));
        if d6 >= 0.0 && d5 <= d6 {
            return c;
        }
        let vb = d5 * d2 - d1 * d6;
        if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            return a + ac * (d2 / (d2 - d6));
        }
        let va = d3 * d6 - d5 * d4;
        if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
        }
        let denom = 1.0 / (va + vb + vc);
        a + ab * (vb * denom) + ac * (vc * denom)
    }

    /// Push-out of a sphere at `p` (direction, depth) when it overlaps the triangle, away from
    /// the face on the side the centre is on (the level's collision is queried two-sided, like
    /// the line checks).
    fn push(&self, p: Vec3, radius: f32) -> Option<(Vec3, f32)> {
        let sd = (p - self.a).dot(self.n);
        if sd.abs() >= radius {
            return None;
        }
        let d = p - self.closest(p);
        let dist = d.length();
        if dist >= radius {
            return None;
        }
        if dist < 1e-4 {
            return Some((if sd < 0.0 { -self.n } else { self.n }, radius - dist));
        }
        Some((d / dist, radius - dist))
    }
}

fn arc(a: Vec3, b: Vec3) -> Quat {
    let (a, b) = (a.normalize_or_zero(), b.normalize_or_zero());
    if a == Vec3::ZERO || b == Vec3::ZERO {
        return Quat::IDENTITY;
    }
    Quat::from_rotation_arc(a, b)
}

/// Rotation taking the frame (a1, a2) to (b1, b2): primary axes match exactly, the secondary
/// as close as possible.
fn frame(a1: Vec3, a2: Vec3, b1: Vec3, b2: Vec3) -> Quat {
    let q1 = arc(a1, b1);
    let a2r = q1 * a2;
    let n = b1.normalize_or_zero();
    let pa = (a2r - n * a2r.dot(n)).normalize_or_zero();
    let pb = (b2 - n * b2.dot(n)).normalize_or_zero();
    if pa == Vec3::ZERO || pb == Vec3::ZERO {
        return q1;
    }
    let ang = pa.cross(pb).dot(n).atan2(pa.dot(pb));
    Quat::from_axis_angle(n, ang) * q1
}

impl Ragdoll {
    /// Build from part 0's bones and the start / previous-step globals (pawn space). `floor` is
    /// the floor height under a pawn-space point, `walls` the level's collision triangles around
    /// the pawn (pawn space; only the steep ones are kept), `gravity` uu/s^2 (negative, down).
    pub fn new(asset: &PhysicsAssetData, part: &[BoneAsset], g0: &[Mat4], g_prev: &[Mat4], floor_at: &dyn Fn(f32, f32) -> f32, walls: &[[Vec3; 3]], gravity: f32) -> Option<Ragdoll> {
        let find = |n: &str| part.iter().position(|b| b.name.eq_ignore_ascii_case(n));
        let mut bodies: Vec<Body> = Vec::new();
        for b in &asset.bodies {
            let Some(bone) = find(&b.bone) else { continue };
            let pos = g0[bone].w_axis.truncate();
            let prev = g_prev.get(bone).map(|g| g.w_axis.truncate()).unwrap_or(pos);
            bodies.push(Body { bone, parent: None, child: None, child2: None, pos, prev, radius: b.radius.max(2.0), rest: 0.0, swing: std::f32::consts::PI, g0: g0[bone] });
        }
        if bodies.len() < 2 {
            return None;
        }
        let body_of = |bodies: &[Body], bone: usize| bodies.iter().position(|b| b.bone == bone);
        // Parents: the constraint's second bone, else the nearest body up the skeleton.
        for i in 0..bodies.len() {
            let name = &part[bodies[i].bone].name;
            let c = asset.constraints.iter().find(|c| c.bone1.eq_ignore_ascii_case(name));
            let mut parent = c.and_then(|c| find(&c.bone2)).and_then(|b| body_of(&bodies, b));
            if parent.is_none() {
                let mut k = part[bodies[i].bone].parent;
                while k >= 0 && (k as usize) < part.len() {
                    if let Some(p) = body_of(&bodies, k as usize) {
                        parent = Some(p);
                        break;
                    }
                    let up = part[k as usize].parent;
                    if up == k {
                        break;
                    }
                    k = up;
                }
            }
            if parent == Some(i) {
                parent = None;
            }
            bodies[i].parent = parent;
            if let (Some(p), Some(c)) = (parent, c) {
                bodies[i].rest = (bodies[i].pos - bodies[p].pos).length();
                if c.swing_limited {
                    bodies[i].swing = c.swing1.max(c.swing2).max(5.0).to_radians();
                }
            } else if let Some(p) = parent {
                bodies[i].rest = (bodies[i].pos - bodies[p].pos).length();
            }
        }
        // Children: the farthest-reaching child body orients a body; a second one fixes twist.
        for i in 0..bodies.len() {
            let kids: Vec<usize> = (0..bodies.len()).filter(|&k| bodies[k].parent == Some(i)).collect();
            let mut by_len = kids.clone();
            by_len.sort_by(|a, b| bodies[*b].rest.total_cmp(&bodies[*a].rest).then(a.cmp(b)));
            bodies[i].child = by_len.first().copied();
            bodies[i].child2 = by_len.get(1).copied();
        }
        let top = (0..bodies.len()).find(|&i| bodies[i].parent.is_none())?;
        let owner: Vec<Option<usize>> = (0..part.len())
            .map(|mut k| loop {
                if let Some(b) = body_of(&bodies, k) {
                    break Some(b);
                }
                let p = part[k].parent;
                if p < 0 || p as usize == k || p as usize >= part.len() {
                    break None;
                }
                k = p as usize;
            })
            .collect();
        let walls = walls.iter().filter_map(|t| WallTri::new(*t)).collect();
        Some(Ragdoll { bodies, owner, top, g0: g0.to_vec(), floor: FloorGrid::sample(floor_at), walls, gravity, elapsed: 0.0, acc: 0.0 })
    }

    /// Parents before children.
    fn order(&self) -> Vec<usize> {
        let depth = |mut i: usize| {
            let mut d = 0;
            while let Some(p) = self.bodies[i].parent {
                d += 1;
                i = p;
                if d > 64 {
                    break;
                }
            }
            d
        };
        let mut o: Vec<usize> = (0..self.bodies.len()).collect();
        o.sort_by_key(|&i| (depth(i), i));
        o
    }

    fn step(&mut self) {
        let g = Vec3::Z * self.gravity * STEP * STEP;
        for b in &mut self.bodies {
            let v = (b.pos - b.prev) * DAMPING;
            b.prev = b.pos;
            b.pos += v + g;
        }
        let order = self.order();
        for _ in 0..ITERATIONS {
            // Joint lengths (equal mass split; a parent fixed by the floor still moves).
            for &i in &order {
                let Some(p) = self.bodies[i].parent else { continue };
                let d = self.bodies[i].pos - self.bodies[p].pos;
                let len = d.length();
                if len < 1e-4 {
                    continue;
                }
                let corr = d * ((len - self.bodies[i].rest) / len) * 0.5;
                self.bodies[i].pos -= corr;
                self.bodies[p].pos += corr;
            }
            // Swing cones: the child segment stays within the limit of its start direction,
            // carried by the parent segment's turn.
            for &i in &order {
                let Some(p) = self.bodies[i].parent else { continue };
                let Some(gp) = self.bodies[p].parent else { continue };
                let rest_dir = (self.bodies[i].g0.w_axis - self.bodies[p].g0.w_axis).truncate();
                let par0 = (self.bodies[p].g0.w_axis - self.bodies[gp].g0.w_axis).truncate();
                let par = self.bodies[p].pos - self.bodies[gp].pos;
                let want = arc(par0, par) * rest_dir;
                let cur = self.bodies[i].pos - self.bodies[p].pos;
                let ang = want.angle_between(cur);
                let lim = self.bodies[i].swing;
                if ang.is_finite() && ang > lim {
                    let axis = want.cross(cur).normalize_or_zero();
                    if axis != Vec3::ZERO {
                        // Split the correction between child and parent so the joint pushes no
                        // net momentum into the body.
                        let fixed = Quat::from_axis_angle(axis, lim) * want.normalize() * cur.length();
                        let corr = (fixed - cur) * 0.5;
                        self.bodies[i].pos += corr;
                        self.bodies[p].pos -= corr;
                    }
                }
            }
            // Walls: out of every overlapping triangle, losing the velocity into it.
            for b in &mut self.bodies {
                for w in &self.walls {
                    if let Some((dir, depth)) = w.push(b.pos, b.radius) {
                        b.pos += dir * depth;
                        let vn = (b.pos - b.prev).dot(dir);
                        if vn < 0.0 {
                            b.prev += dir * vn;
                        }
                    }
                }
            }
            // Floor.
            for b in &mut self.bodies {
                let min = self.floor.at(b.pos.x, b.pos.y) + b.radius;
                if b.pos.z < min {
                    b.pos.z = min;
                    if b.prev.z < min {
                        b.prev.z = min;
                    }
                    let h = (b.pos - b.prev).truncate() * (1.0 - FRICTION);
                    b.prev.x += h.x;
                    b.prev.y += h.y;
                }
            }
        }
    }

    /// Advance by `dt` real seconds (fixed steps; the result does not depend on frame times).
    pub fn advance(&mut self, dt: f32) {
        if self.elapsed >= MAX_TIME {
            return;
        }
        self.acc += dt;
        while self.acc >= STEP && self.elapsed < MAX_TIME {
            self.acc -= STEP;
            self.elapsed += STEP;
            self.step();
        }
    }

    /// Run to rest at once (a corpse first seen after it fell).
    pub fn settle(&mut self) {
        while self.elapsed < MAX_TIME {
            self.elapsed += STEP;
            self.step();
        }
    }

    /// Current pawn-space globals of part 0's bones.
    fn globals(&self) -> Vec<Mat4> {
        let n = self.bodies.len();
        let mut rot: Vec<Option<Quat>> = vec![None; n];
        for &i in &self.order() {
            let b = &self.bodies[i];
            let parent_q = b.parent.and_then(|p| rot[p]).unwrap_or(Quat::IDENTITY);
            let q = match (b.child, b.child2) {
                (Some(c), Some(c2)) if b.parent.is_none() => {
                    let a1 = (self.bodies[c].g0.w_axis - b.g0.w_axis).truncate();
                    let a2 = (self.bodies[c2].g0.w_axis - b.g0.w_axis).truncate();
                    frame(a1, a2, self.bodies[c].pos - b.pos, self.bodies[c2].pos - b.pos)
                }
                (Some(c), _) => {
                    let s0 = parent_q * (self.bodies[c].g0.w_axis - b.g0.w_axis).truncate();
                    arc(s0, self.bodies[c].pos - b.pos) * parent_q
                }
                _ => parent_q,
            };
            rot[i] = Some(q);
        }
        let top_q = rot[self.top].unwrap_or(Quat::IDENTITY);
        let body_g = |i: usize| -> Mat4 {
            let b = &self.bodies[i];
            let (_, r0, _) = b.g0.to_scale_rotation_translation();
            Mat4::from_rotation_translation(rot[i].unwrap_or(top_q) * r0, b.pos)
        };
        (0..self.g0.len())
            .map(|k| {
                let o = self.owner[k].unwrap_or(self.top);
                let gb = body_g(o);
                gb * self.bodies[o].g0.inverse() * self.g0[k]
            })
            .collect()
    }

    /// The ragdoll as a pose: local rotations for every bone of every part (bones matched by
    /// name with part 0), translations for the root level, like sampled sequences.
    pub fn pose(&self, parts: &[PartGpu]) -> Pose {
        let p0 = &parts[0];
        let g = self.globals();
        let inv = p0.mesh_to_pawn.inverse();
        let mesh: Vec<Mat4> = g.iter().map(|m| inv * *m).collect();
        let local: Vec<Key> = (0..p0.bones.len())
            .map(|k| {
                let par = p0.bones[k].parent;
                let l = if par >= 0 && (par as usize) < mesh.len() && par as usize != k { mesh[par as usize].inverse() * mesh[k] } else { mesh[k] };
                let (_, q, t) = l.to_scale_rotation_translation();
                let shallow = k == 0 || par <= 0 || p0.bones.get(par as usize).is_some_and(|x| x.parent < 0);
                Key { q: q.normalize(), t: shallow.then_some(t) }
            })
            .collect();
        parts
            .iter()
            .map(|part| part.bones.iter().map(|b| p0.bones.iter().position(|x| x.name.eq_ignore_ascii_case(&b.name)).map(|i| local[i])).collect())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The elite guard's ragdoll, dropped from its bind pose, settles on the floor near where it
    /// started (skipped without the install).
    #[test]
    fn elite_guard_settles_on_the_floor() {
        let Ok(c) = dishonored_assets::Content::locate() else { return };
        let hh = 97.0;
        let m = dishonored_assets::characters::load_character(&c, "elite_guard", hh, 16).unwrap();
        let part = &m.parts[0];
        let g0: Vec<Mat4> = dishonored_assets::characters::bind_globals(&part.bones).iter().map(|g| part.mesh_to_pawn * *g).collect();
        let a = dishonored_assets::npc_anim::load_physics_asset(&c, &("L_Boyle_Ext_Script".into(), "Npc_EliteGuard.Physics.Skm_EliteGuard_Physics".into())).unwrap();
        let mut r = Ragdoll::new(&a, &part.bones, &g0, &g0, &|_, _| -hh, &[], -980.0).unwrap();
        r.settle();
        for b in &r.bodies {
            assert!(b.pos.is_finite(), "{}", part.bones[b.bone].name);
            assert!(b.pos.truncate().length() < 250.0, "{} drifted to {}", part.bones[b.bone].name, b.pos);
            assert!(b.pos.z >= -hh - 1.0 && b.pos.z < -hh + 200.0, "{} at {}", part.bones[b.bone].name, b.pos);
        }
        // Joint lengths held.
        for b in &r.bodies {
            if let Some(p) = b.parent {
                let d = (b.pos - r.bodies[p].pos).length();
                assert!((d - b.rest).abs() < 0.1 * b.rest.max(1.0) + 1.0, "{}: {d} vs {}", part.bones[b.bone].name, b.rest);
            }
        }
        let g = r.globals();
        for (k, m) in g.iter().enumerate() {
            assert!(m.is_finite() && m.w_axis.truncate().length() < 300.0, "{} at {}", part.bones[k].name, m.w_axis);
        }
    }


    #[test]
    fn floor_grid_follows_a_step() {
        // A 30 uu step at x = 0: bilinear between samples, clamped beyond the grid.
        let g = FloorGrid::sample(&|x, _| if x < 0.0 { -97.0 } else { -67.0 });
        assert_eq!(g.at(-50.0, 0.0), -97.0);
        assert_eq!(g.at(50.0, 10.0), -67.0);
        assert_eq!(g.at(500.0, -500.0), -67.0);
        let mid = g.at(-6.0, 0.0);
        assert!(mid > -97.0 && mid < -67.0, "{mid}");
    }

    /// The elite guard dropped over a 30 uu step lies on both levels (skipped without the install).
    #[test]
    fn elite_guard_settles_on_a_step() {
        let Ok(c) = dishonored_assets::Content::locate() else { return };
        let hh = 97.0;
        let m = dishonored_assets::characters::load_character(&c, "elite_guard", hh, 16).unwrap();
        let part = &m.parts[0];
        let g0: Vec<Mat4> = dishonored_assets::characters::bind_globals(&part.bones).iter().map(|g| part.mesh_to_pawn * *g).collect();
        let a = dishonored_assets::npc_anim::load_physics_asset(&c, &("L_Boyle_Ext_Script".into(), "Npc_EliteGuard.Physics.Skm_EliteGuard_Physics".into())).unwrap();
        let floor = |x: f32, _y: f32| if x < 0.0 { -hh } else { -hh + 30.0 };
        let mut r = Ragdoll::new(&a, &part.bones, &g0, &g0, &floor, &[], -980.0).unwrap();
        r.settle();
        for b in &r.bodies {
            let f = r.floor.at(b.pos.x, b.pos.y);
            assert!(b.pos.z >= f + b.radius - 1.0, "{} at {} under the floor {f}", part.bones[b.bone].name, b.pos);
        }
    }
    /// A three-body chain (root, spine, head) as a physics asset and its bind globals.
    fn chain() -> (PhysicsAssetData, Vec<BoneAsset>, Vec<Mat4>) {
        let bone = |name: &str, parent: i32, t: Vec3| BoneAsset { name: name.into(), parent, rotation: Quat::IDENTITY, translation: t };
        let part = vec![bone("root", -1, Vec3::ZERO), bone("spine", 0, Vec3::new(0.0, 0.0, 30.0)), bone("head", 1, Vec3::new(0.0, 0.0, 30.0))];
        let g0 = vec![Mat4::from_translation(Vec3::ZERO), Mat4::from_translation(Vec3::new(0.0, 0.0, 30.0)), Mat4::from_translation(Vec3::new(0.0, 0.0, 60.0))];
        let body = |b: &str| dishonored_ir::npc_anim::PhysBody { bone: b.into(), radius: 8.0, ..Default::default() };
        let joint = |a: &str, b: &str| dishonored_ir::npc_anim::PhysConstraint { bone1: a.into(), bone2: b.into(), swing_limited: true, swing1: 45.0, swing2: 45.0, ..Default::default() };
        let asset = PhysicsAssetData { name: "chain".into(), bodies: vec![body("root"), body("spine"), body("head")], constraints: vec![joint("spine", "root"), joint("head", "spine")] };
        (asset, part, g0)
    }

    /// A wall 20 uu in front of the pawn (+X), facing it, as two triangles.
    fn wall_at_x(x: f32) -> Vec<[Vec3; 3]> {
        let (lo, hi) = (Vec3::new(x, -200.0, -100.0), Vec3::new(x, 200.0, 300.0));
        vec![
            [lo, Vec3::new(x, lo.y, hi.z), hi],
            [lo, hi, Vec3::new(x, hi.y, lo.z)],
        ]
    }

    /// The chain, toppling towards a wall, stops at it (every body at least its radius in front
    /// of the plane); without the wall it falls through that plane. Two runs settle bit-identically.
    #[test]
    fn chain_falls_against_a_wall() {
        let (asset, part, g0) = chain();
        // Moving +X at 600 uu/s at the hand-over (previous step 10 uu behind).
        let g_prev: Vec<Mat4> = g0.iter().map(|g| Mat4::from_translation(Vec3::new(-10.0, 0.0, 0.0)) * *g).collect();
        let floor = |_: f32, _: f32| -8.0;
        let walls = wall_at_x(20.0);
        let run = |walls: &[[Vec3; 3]]| {
            let mut r = Ragdoll::new(&asset, &part, &g0, &g_prev, &floor, walls, -980.0).unwrap();
            r.settle();
            r.bodies.iter().map(|b| b.pos).collect::<Vec<Vec3>>()
        };
        let free = run(&[]);
        assert!(free.iter().any(|p| p.x > 20.0), "without the wall the chain should cross x = 20: {free:?}");
        let a = run(&walls);
        for p in &a {
            assert!(p.is_finite());
            assert!(p.x <= 20.0 - 8.0 + 0.5, "body through the wall: {p}");
            assert!(p.z >= -8.0 + 8.0 - 0.5, "body under the floor: {p}");
        }
        let b = run(&walls);
        assert_eq!(a.iter().map(|p| p.to_array().map(f32::to_bits)).collect::<Vec<_>>(), b.iter().map(|p| p.to_array().map(f32::to_bits)).collect::<Vec<_>>());
    }

    #[test]
    fn walls_keep_only_steep_triangles_and_push_out() {
        // A floor triangle is not a wall.
        assert!(WallTri::new([Vec3::ZERO, Vec3::X, Vec3::Y]).is_none());
        let w = WallTri::new(wall_at_x(20.0)[0]).unwrap();
        // Facing the pawn (-X).
        assert!((w.n.x + 1.0).abs() < 1e-6);
        // A sphere straddling the face is pushed to touch it from the front.
        let p = Vec3::new(20.0 + w.n.x * 3.0, 0.0, 200.0);
        let (dir, depth) = w.push(p, 8.0).unwrap();
        assert!((dir - w.n).length() < 1e-5 && (depth - 5.0).abs() < 1e-4, "{dir} {depth}");
        // From behind: pushed back out behind (two-sided).
        let (dir, depth) = w.push(Vec3::new(20.0 - w.n.x * 3.0, 0.0, 200.0), 8.0).unwrap();
        assert!((dir + w.n).length() < 1e-5 && (depth - 5.0).abs() < 1e-4, "{dir} {depth}");
        // Beyond the radius on either side: nothing.
        assert!(w.push(Vec3::new(20.0 + w.n.x * 9.0, 0.0, 200.0), 8.0).is_none());
        assert!(w.push(Vec3::new(20.0 - w.n.x * 9.0, 0.0, 200.0), 8.0).is_none());
        // Past the triangle's edge: no push.
        assert!(w.push(Vec3::new(20.0 + w.n.x * 3.0, 500.0, 200.0), 8.0).is_none());
    }

    #[test]
    fn frame_matches_two_axes() {
        let q = frame(Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X);
        assert!((q * Vec3::X - Vec3::Z).length() < 1e-4);
        assert!((q * Vec3::Y + Vec3::X).length() < 1e-4);
    }
}
