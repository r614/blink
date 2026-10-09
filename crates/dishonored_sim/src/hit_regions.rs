//! Hit regions of a projectile hit on a pawn (research/dishonored/systems/ranged.md F4).
//!
//! The original's projectile trace (`TRACE_ComplexCollision`) meets the pawn's physics-asset
//! bodies; the hit bone is looked up in the pawn's `m_HitRegions_Map` (0x00B66950 /
//! 0x00B66A50 / 0x00B66900) for an `eDisHitRegion`. The sim's pawns are capsules: once the
//! shot meets the capsule, the region is that of the first body of the rig
//! ([`HitBodyConfig`], the elite guard's physics asset in its bind pose, fitted to the
//! pawn's collision height and turned to its yaw) the shot's line crosses; a line that
//! crosses none takes the body nearest the capsule hit. Without bodies (a config that
//! predates them) the old capsule-height rule stands (head above 0.6 of the half height).

use crate::movement::half_height;
use crate::state::PlayerState;
use dishonored_ir::gameplay::MovementConfig;
use dishonored_ir::items::{HitBody, HitBodyConfig, HitRegion};
use dishonored_world::geom::closest_segment_segment;
use glam::Vec3;

/// The region a shot travelling along `dir` hits on `v`, entering its capsule at `point`.
pub fn hit_region(v: &PlayerState, mv: &MovementConfig, hb: &HitBodyConfig, point: Vec3, dir: Vec3) -> HitRegion {
    let hh = half_height(v, mv);
    if hb.bodies.is_empty() || hb.half_height <= 0.0 {
        return if point.z > v.position.z + hh * 0.6 { HitRegion::Head } else { HitRegion::Torso };
    }
    // Rig frame: x forward, y right, z up from the pawn's location; heights scaled from the
    // rig's to the pawn's (a crouched pawn shortens it).
    let s = hh / hb.half_height;
    let (sy, cy) = v.yaw.to_radians().sin_cos();
    let fwd = Vec3::new(cy, sy, 0.0);
    let right = Vec3::new(-sy, cy, 0.0);
    let local = |w: Vec3| Vec3::new(w.dot(fwd), w.dot(right), w.z / s);
    let p = local(point - v.position);
    let d = local(dir).normalize_or_zero();
    if d == Vec3::ZERO {
        return nearest(hb, p);
    }
    // The line through the pawn: from well before the capsule hit to well beyond it.
    let reach = 4.0 * (hb.half_height + mv.collision_radius);
    let origin = p - d * reach;
    let mut best: Option<(f32, HitRegion)> = None;
    for b in &hb.bodies {
        if let Some(t) = ray_body(b, origin, d, 2.0 * reach) {
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, b.region));
            }
        }
    }
    best.map(|b| b.1).unwrap_or_else(|| nearest(hb, p))
}

/// Where a line along `dir` (unit) from `origin` first meets a pawn's physics-asset bodies, grown by
/// `inflate` (the half extent of a swept box, taken as a sphere): the distance along the line
/// within `len`. The rig is fitted to the pawn as in [`hit_region`]; a pawn without bodies
/// falls back to its cylinder.
pub fn ray_pawn(v: &PlayerState, mv: &MovementConfig, hb: &HitBodyConfig, origin: Vec3, dir: Vec3, len: f32, inflate: f32) -> Option<f32> {
    let hh = half_height(v, mv);
    if hb.bodies.is_empty() || hb.half_height <= 0.0 {
        let r = mv.collision_radius + inflate;
        return crate::interactables::ray_box(origin, dir, v.position - Vec3::new(r, r, hh + inflate), v.position + Vec3::new(r, r, hh + inflate), len);
    }
    let s = hh / hb.half_height;
    let (sy, cy) = v.yaw.to_radians().sin_cos();
    let fwd = Vec3::new(cy, sy, 0.0);
    let right = Vec3::new(-sy, cy, 0.0);
    let local = |w: Vec3| Vec3::new(w.dot(fwd), w.dot(right), w.z / s);
    let o = local(origin - v.position);
    let d = local(dir);
    let dl = d.length();
    if dl < 1e-6 {
        return None;
    }
    let d = d / dl;
    let ll = len * dl;
    let mut best: Option<f32> = None;
    for b in &hb.bodies {
        let mut g = b.clone();
        // The swept box's reach grows each body: a capsule's radius, a box's half extents.
        if g.capsule {
            g.half[0] += inflate;
        } else {
            for h in g.half.iter_mut() {
                *h += inflate;
            }
        }
        if let Some(t) = ray_body(&g, o, d, ll) {
            if best.is_none_or(|bt| t < bt) {
                best = Some(t);
            }
        }
    }
    best.map(|t| t / dl)
}

fn axes(b: &HitBody) -> [Vec3; 3] {
    [Vec3::from(b.axes[0]), Vec3::from(b.axes[1]), Vec3::from(b.axes[2])]
}

/// Distance along the ray at which it enters the body, if it does within `len`.
fn ray_body(b: &HitBody, o: Vec3, d: Vec3, len: f32) -> Option<f32> {
    let c = Vec3::from(b.center);
    let ax = axes(b);
    if b.capsule {
        let r = b.half[0];
        let a0 = c - ax[2] * b.half[2];
        let a1 = c + ax[2] * b.half[2];
        let (on_ray, on_axis) = closest_segment_segment(o, o + d * len, a0, a1);
        let dist = on_ray.distance(on_axis);
        if dist > r {
            return None;
        }
        let t = (on_ray - o).dot(d) - (r * r - dist * dist).max(0.0).sqrt();
        return Some(t.max(0.0));
    }
    // Box: slabs along its three axes.
    let (mut t0, mut t1) = (0.0f32, len);
    let rel = o - c;
    for i in 0..3 {
        let e = ax[i].dot(d);
        let f = ax[i].dot(rel);
        let h = b.half[i];
        if e.abs() < 1e-6 {
            if f.abs() > h {
                return None;
            }
            continue;
        }
        let (mut a, mut z) = ((-h - f) / e, (h - f) / e);
        if a > z {
            std::mem::swap(&mut a, &mut z);
        }
        t0 = t0.max(a);
        t1 = t1.min(z);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// Distance from `p` to the body's surface (negative inside).
fn body_distance(b: &HitBody, p: Vec3) -> f32 {
    let c = Vec3::from(b.center);
    let ax = axes(b);
    let rel = p - c;
    if b.capsule {
        let h = ax[2].dot(rel).clamp(-b.half[2], b.half[2]);
        return (rel - ax[2] * h).length() - b.half[0];
    }
    let q = Vec3::new(ax[0].dot(rel).abs() - b.half[0], ax[1].dot(rel).abs() - b.half[1], ax[2].dot(rel).abs() - b.half[2]);
    q.max(Vec3::ZERO).length() + q.max_element().min(0.0)
}

fn nearest(hb: &HitBodyConfig, p: Vec3) -> HitRegion {
    hb.bodies.iter().map(|b| (body_distance(b, p), b.region)).min_by(|a, b| a.0.total_cmp(&b.0)).map(|b| b.1).unwrap_or(HitRegion::Torso)
}

/// Head or torso (the crossbow's stealth regions, 0x00C503D0).
pub fn is_core(r: HitRegion) -> bool {
    matches!(r, HitRegion::Head | HitRegion::Torso)
}
