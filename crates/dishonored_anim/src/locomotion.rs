//! `ArkAnimNodeLocomotion` (vtable 0x0109DCB8, TickAnim slot 85 = 0x0091B280; ANIMATION.md
//! section 3): the pawn's velocity smoothed in the plane, the two direction loops around its
//! angle, the two speed modes of the modifier bracketing its (diagonal-corrected) speed, and the
//! `LocoGroup` rate that keeps the authored stride on the ground.
//!
//! The maths is the runtime's (`dishonored_runtime::npc_anim::{smooth_velocity, dir_weights,
//! diagonal_factor, loco_pose}`), decoded from the steps below and copied here unchanged; the
//! runtime keeps its copy until it is wired to this crate:
//! - 0x0090BC00 velocity smoothing (CONFIRMED);
//! - 0x009331D0 direction entries and weights, 0.98 snap;
//! - 0x00910E30 diagonal correction (CONFIRMED);
//! - 0x009105B0 speed-mode bracket, 0.98 snap;
//! - 0x00910970 S and W loops play with rate -1 (the `_Revert` loops are authored reversed);
//! - 0x00916950 sync-group rate, the master being the heaviest loop;
//! - 0x00916770 modifier crossfade over `m_fModifierBlendDuration`.

use dishonored_ir::npc_anim::{LocoConfig, SpeedMode};
use glam::Vec2;

/// Tunables of the node (`NodeProps::Locomotion`: the tree's values over
/// `Engine.Default__ArkAnimNodeLocomotion`, 2000 / 100 / 0.1 / 0.5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocoParams {
    /// `m_fVelocityMaxBlendSpeed` (+0x298): velocity change rate (uu/s per second at full ratio).
    pub rate: f32,
    /// `m_fVelocityMaxDif` (+0x29C): velocity difference at which the full rate applies.
    pub range: f32,
    /// `m_fVelocityMinBlendSpeedRatio` (+0x2A0): minimum fraction of the rate.
    pub min_ratio: f32,
    /// `m_fModifierBlendDuration` (+0x27C): crossfade time between the two modifier slots.
    pub modifier_blend: f32,
}

/// Directional weights for the four move loops (S, W, N, E) from the movement angle relative
/// to facing (0 = forward, +90 = right): loop `k` covers 180 + 90k degrees. Like 0x009331D0,
/// a neighbour within 0.98 takes the whole weight. Returns the weights and whether both
/// neighbours are in use (the diagonal correction needs two).
pub fn dir_weights(angle_deg: f32) -> ([f32; 4], bool) {
    let a = (angle_deg - 180.0).rem_euclid(360.0) / 90.0;
    let k0 = a.floor() as usize % 4;
    let f = a - a.floor();
    let mut w = [0.0; 4];
    if 1.0 - f >= 0.98 {
        w[k0] = 1.0;
        return (w, false);
    }
    if f >= 0.98 {
        w[(k0 + 1) % 4] = 1.0;
        return (w, false);
    }
    w[k0] += 1.0 - f;
    w[(k0 + 1) % 4] += f;
    (w, true)
}

/// Diagonal stride correction (0x00910E30): two direction loops blended at an angle between
/// them cover the chord between their unit directions (table of 8 octant vectors at
/// 0x014527C0), so the speed is divided by the distance from the centre to that chord along
/// the movement direction: `cos(phi) + sin(phi)` for neighbours 90 degrees apart, `phi` the
/// angle from the first neighbour.
pub fn diagonal_factor(angle_deg: f32) -> f32 {
    let (_, both) = dir_weights(angle_deg);
    if !both {
        return 1.0;
    }
    let phi = ((angle_deg - 180.0).rem_euclid(90.0)).to_radians();
    phi.cos() + phi.sin()
}

/// Polar velocity smoothing (0x0090BC00): the node's velocity (`m_SmoothedAngle` +0x200,
/// `m_fSmoothedSpeed` +0x204) moves toward the pawn's (`m_Angle` +0x1F4, `m_fSpeed` +0x1F8) in the
/// plane, by `rate * clamp(dist / range, min, 1) * dt` per tick. Angles in degrees relative to
/// facing.
pub fn smooth_velocity(cur: (f32, f32), target: (f32, f32), dt: f32, p: &LocoParams) -> (f32, f32) {
    let to = |(a, s): (f32, f32)| Vec2::new(a.to_radians().cos() * s, a.to_radians().sin() * s);
    let (c, t) = (to(cur), to(target));
    let d = (t - c).length();
    if d <= 0.0 {
        return cur;
    }
    let ratio = (d / p.range.max(1e-3)).clamp(p.min_ratio, 1.0);
    let k = (p.rate * ratio * dt / d).min(1.0);
    let v = c + (t - c) * k;
    // The node keeps the angle as a rotator (int) and the speed separately.
    let angle = if v.length_squared() > 1e-8 { v.y.atan2(v.x).to_degrees() } else { cur.0 };
    (angle, v.length())
}

/// One loop the node plays for a modifier.
#[derive(Clone, Debug, PartialEq)]
pub struct LocoEntry {
    pub name: String,
    /// Played with rate -1 and its group position reversed (S and W of a moving mode).
    pub reversed: bool,
    pub weight: f32,
    /// The loop's authored ground speed (`m_MoveAnimProps`).
    pub authored: f32,
    /// The mode has a single loop (idle modes).
    pub single: bool,
}

/// The loops of `modifier` for the node's smoothed velocity (angle from facing in degrees,
/// speed), weighted by direction and speed mode, and the speed the bracket and the group rate
/// use (`m_fSmoothedSpeedWithBlend` +0x208: the diagonal-corrected speed).
pub fn loco_entries(cfg: &LocoConfig, modifier: usize, vel: (f32, f32)) -> (Vec<LocoEntry>, f32) {
    let (angle, speed) = vel;
    let (dw, _) = dir_weights(angle);
    // Each speed mode's speed in this direction (idle modes: 0).
    let mut modes: Vec<(&SpeedMode, f32)> = Vec::new();
    for s in 0..cfg.speed_names.len() {
        if let Some(m) = cfg.mode(modifier, s) {
            let sp = if m.move_anims.len() >= 4 { (0..4).map(|k| dw[k] * m.move_anims[k].1).sum() } else { m.move_anims.first().map(|a| a.1).unwrap_or(0.0) };
            if modes.last().is_none_or(|l: &(&SpeedMode, f32)| l.0.name != m.name) {
                modes.push((m, sp));
            }
        }
    }
    if modes.is_empty() {
        return (Vec::new(), speed);
    }
    // Bracketing pair (0x009105B0) on the diagonal-corrected speed (0x00916770 stores
    // `factor * speed` at +0x208 before the search). The factor applies when the speed falls
    // into a moving mode (more than one direction loop).
    let bracket = |speed: f32| {
        let mut lo = 0;
        for k in 0..modes.len() {
            if modes[k].1 <= speed {
                lo = k;
            }
        }
        lo
    };
    // 0x00910E30 takes the bracketing mode of the larger weight, or the other one when that
    // mode's speed is below 0.1 (idle), and corrects only for a mode with several loops.
    let lo0 = bracket(speed);
    let hi0 = (lo0 + 1).min(modes.len() - 1);
    let w0 = if hi0 != lo0 && modes[hi0].1 > modes[lo0].1 { ((speed - modes[lo0].1) / (modes[hi0].1 - modes[lo0].1)).clamp(0.0, 1.0) } else { 0.0 };
    let (dom, other) = if w0 >= 0.5 { (hi0, lo0) } else { (lo0, hi0) };
    let pick = if modes[dom].1.abs() < 0.1 { other } else { dom };
    let moving_mode = modes[pick].0.move_anims.len() > 1;
    let speed = if moving_mode && speed > 0.0 { speed * diagonal_factor(angle) } else { speed };
    let (lo, hi, w_hi) = {
        let lo = bracket(speed);
        let hi = (lo + 1).min(modes.len() - 1);
        let w = if hi != lo && modes[hi].1 > modes[lo].1 { ((speed - modes[lo].1) / (modes[hi].1 - modes[lo].1)).clamp(0.0, 1.0) } else { 0.0 };
        // Snap within 2 % (0.98 threshold in the original).
        let w = if w > 0.98 { 1.0 } else if w < 0.02 { 0.0 } else { w };
        (lo, hi, w)
    };
    let mut out = Vec::new();
    for (k, mw) in [(lo, 1.0 - w_hi), (hi, w_hi)] {
        if mw <= 1e-3 || (k == hi && hi == lo && mw < 1.0) {
            continue;
        }
        let m = modes[k].0;
        if m.move_anims.len() >= 4 && modes[k].1 > 0.0 {
            for d in 0..4 {
                let w = mw * dw[d];
                if w > 1e-3 {
                    // S and W are authored reversed: played with rate -1 (0x00910970).
                    out.push(LocoEntry { name: m.move_anims[d].0.clone(), reversed: d < 2, weight: w, authored: m.move_anims[d].1, single: false });
                }
            }
        } else if let Some(a) = m.move_anims.first() {
            out.push(LocoEntry { name: a.0.clone(), reversed: false, weight: mw, authored: a.1, single: m.move_anims.len() == 1 });
        }
    }
    (out, speed)
}

/// The `LocoGroup` rate scale (0x00916950, `m_fMostRelevantRateScale` +0x21C): `master length *
/// speed / sum(authored speed * length * weight)` times the modifier's `m_fSpeedMultiplier`, or 1
/// when the speed or the sum is below 0.1. `lengths` are the entries' sequence lengths (None:
/// not found); the master is the heaviest entry.
pub fn group_rate(entries: &[LocoEntry], lengths: &[Option<f32>], speed: f32, multiplier: f32) -> f32 {
    let mut master: Option<(usize, f32)> = None;
    let mut sum = 0.0;
    for (i, (e, d)) in entries.iter().zip(lengths).enumerate() {
        let Some(d) = *d else { continue };
        if master.is_none_or(|m| e.weight > entries[m.0].weight) {
            master = Some((i, d));
        }
        sum += e.weight * e.authored * d;
    }
    match master {
        Some((_, d)) if speed.abs() >= 0.1 && sum.abs() >= 0.1 => d * speed * multiplier / sum,
        _ => 1.0,
    }
}
