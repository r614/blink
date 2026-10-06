//! Presentation-only ragdoll from a pawn's `PhysicsAsset` (research/dishonored/ANIMATION.md
//! §8). The original hands a dying pawn to PhysX at `DishonoredNotify_Ragdoll`; here each
//! physics body is a point mass at its bone, linked to its parent body by its
//! `RB_ConstraintSetup` (rest length from the pose at the hand-over, swing cone from
//! `Swing1/Swing2LimitAngle`), falling under gravity onto the floor under the corpse.
//!
//! It runs at a fixed step from the pose sampled exactly at the notify time, so a corpse settles
//! the same way on every run. The simulation's corpse position stays authoritative: the ragdoll
//! only moves bones inside the pawn's frame.

use crate::characters::PartGpu;
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
    floor: f32,
    gravity: f32,
    elapsed: f32,
    acc: f32,
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
    /// the floor height in pawn space, `gravity` uu/s^2 (negative, down).
    pub fn new(asset: &PhysicsAssetData, part: &PartGpu, g0: &[Mat4], g_prev: &[Mat4], floor: f32, gravity: f32) -> Option<Ragdoll> {
        let find = |n: &str| part.bones.iter().position(|b| b.name.eq_ignore_ascii_case(n));
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
            let name = &part.bones[bodies[i].bone].name;
            let c = asset.constraints.iter().find(|c| c.bone1.eq_ignore_ascii_case(name));
            let mut parent = c.and_then(|c| find(&c.bone2)).and_then(|b| body_of(&bodies, b));
            if parent.is_none() {
                let mut k = part.bones[bodies[i].bone].parent;
                while k >= 0 && (k as usize) < part.bones.len() {
                    if let Some(p) = body_of(&bodies, k as usize) {
                        parent = Some(p);
                        break;
                    }
                    let up = part.bones[k as usize].parent;
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
                    bodies[i].swing = c.swing1.max(c.swing2).max(1.0).to_radians();
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
        let owner: Vec<Option<usize>> = (0..part.bones.len())
            .map(|mut k| loop {
                if let Some(b) = body_of(&bodies, k) {
                    break Some(b);
                }
                let p = part.bones[k].parent;
                if p < 0 || p as usize == k || p as usize >= part.bones.len() {
                    break None;
                }
                k = p as usize;
            })
            .collect();
        Some(Ragdoll { bodies, owner, top, g0: g0.to_vec(), floor, gravity, elapsed: 0.0, acc: 0.0 })
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
                        let fixed = Quat::from_axis_angle(axis, lim) * want.normalize() * cur.length();
                        self.bodies[i].pos = self.bodies[p].pos + fixed;
                    }
                }
            }
            // Floor.
            for b in &mut self.bodies {
                let min = self.floor + b.radius;
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

    #[test]
    fn frame_matches_two_axes() {
        let q = frame(Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X);
        assert!((q * Vec3::X - Vec3::Z).length() < 1e-4);
        assert!((q * Vec3::Y + Vec3::X).length() < 1e-4);
    }
}
