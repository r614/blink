//! Blink, as decoded from `DishonoredActivePowerComponent_Blink` in `Dishonored.exe`
//! (research/dishonored/PLAYER.md, "Blink"). Hold to aim (the target updates every tick),
//! release to travel.
//!
//! Targeting (0x00BF5520, envelope 0x00BF52E0, back-off 0x00BE8F20):
//! 1. Range vector from the view direction: looking level or down it is the view
//!    direction times `m_HorizDistance`; looking up it is the view direction scaled to
//!    reach `m_VertDistance` of height (at most `m_HorizDistance` long) with its height
//!    clamped to `m_VertDistance` (minus the height above ground when
//!    `m_bLimitVerticalDistanceFromGround`). Its direction becomes the aim.
//! 2. Box sweep (`m_TargetTestExtent`) from the camera along it; started inside geometry
//!    -> retry from the pawn.
//! 3. A hit closer than `m_fCloseCollisionDistance`: try the sweep past the obstacle from
//!    offset starts (up by the free headroom of `m_fCloseCollisionOffsetStep` minus the
//!    extent, then down, +Y, -Y by the step); the first clear one is used for a new sweep to
//!    the far end.
//! 4. From the hit (or the far end) step back along the aim, in half the pawn's bounding
//!    sphere radius, until a crawl-sized box (radius x radius x `CrouchHeight`) fits.
//! 5. Raise the point to at least `CollisionHeight` above the floor straight below.
//!
//! Travel (0x00BFC160, step 0x00BECDF0): every `m_fDefaultTimeBetweenBlinkSteps` (at most
//! once per tick) the pawn is moved `m_fDefaultBlinkStepDistance` toward the target
//! without collision (the target was validated). It stops when the target is behind the
//! current view direction, within 25 uu horizontally, or farther from the start than the
//! target was. The pre-Blink velocity and physics are then restored.
//! Release closer than the pawn's bounding sphere radius to the target cancels.
//!
//! Lock-out (CanUse 0x00BE71F0 = mana >= `m_ManaCost` and `m_bActive` clear): starting the
//! power (0x00BFC020) needs CanUse and sets `m_bActive` (0x00BFAAA0); the move's end or a cancel
//! (0x00BFAB80) enters the cooldown modes, whose tick (0x00BED480) counts `m_fCooldownTimer` up
//! past `m_fCooldownTime` and clears `m_bActive` (0x00BE9180). Without the mana the power never
//! starts aiming.
//!
//! World slow-down: the move starts the `BTC_BlinkPower` dilation layer toward 0.01 (clamped to
//! 0.05 by the helper) over `m_fBendTimeDownTime`, caster rate 1 (0x00BFC990); the end or cancel
//! and again the cooldown's end blend it back to 1 over `m_fBendTimeUpTime` from where it is
//! (0x00BE7380).

use crate::events::{SimEvent, SoundCategory};
use crate::left_hand::LeftAction;
use crate::movement::{MoveCtx, eye_position, view_vector};
use crate::state::{BlinkTravel, MoveMode, PlayerState};
use crate::vitals::spend_mana;
use dishonored_ir::gameplay::{BlinkConfig, MovementConfig, VitalsConfig};
use dishonored_world::{PawnBox, WorldQuery, channel};
use glam::Vec3;

/// Blink level of a pawn without the power recorded (Contracts / FFA give level 2, index 1).
pub const BLINK_LEVEL: usize = 1;
/// Horizontal distance at which the target counts as reached (`DAT_01359B8C` = 25.0).
pub const REACHED_DISTANCE: f32 = 25.0;
/// Ground probe length below the target point (20000 uu in 0x00BF5520).
pub const GROUND_PROBE: f32 = 20000.0;
/// World rate the move asks of the dilation layer (the literal 0.01 passed by 0x00BFC990; the
/// helper raises any non-zero target below 0.05 to 0.05).
pub const WORLD_SLOW_TARGET: f32 = 0.01;

fn per_level(v: &[f32], lv: usize) -> f32 {
    v.get(lv).or(v.last()).copied().unwrap_or(0.0)
}

/// CanUse (slot +0x1A4, 0x00BE71F0): the mana check (0x00BF3060, whole points >= `m_ManaCost`)
/// and `m_bActive` clear.
pub fn can_use(p: &PlayerState, cfg: &BlinkConfig) -> bool {
    !p.blink.active && p.vitals.mana >= cfg.mana_cost
}

/// Aiming (`m_BlinkMode` 0 with `m_bActive`).
pub fn aiming(p: &PlayerState) -> bool {
    p.blink.active && !p.blink.cooling && p.mode != MoveMode::Blinking
}

/// The move ended or the aim was cancelled (0x00BFAB80): the cooldown mode starts and the
/// dilation layer blends back to 1 (0x00BE7380).
fn enter_cooldown(p: &mut PlayerState, cfg: &BlinkConfig, actions: &mut Vec<LeftAction>) {
    let lv = level(p, cfg);
    p.blink.cooling = true;
    actions.push(LeftAction::BlinkLayer { target: 1.0, blend: per_level(&cfg.bend_time_up_time, lv) });
}

/// Cancel an aim in progress (the power deselected, 0x00BFAE50 -> 0x00BFAB80 with 1).
pub fn cancel(p: &mut PlayerState, cfg: &BlinkConfig, actions: &mut Vec<LeftAction>) {
    if aiming(p) {
        p.left.blink_target = None;
        enter_cooldown(p, cfg, actions);
    }
}

/// The cooldown modes' tick (0x00BED480): `m_fCooldownTimer` grows by the step; past
/// `m_fCooldownTime` it is clamped and the power deactivates (0x00BE9180, which blends the
/// layer back to 1 again from its current value). A dead pawn deactivates at once (0x00BFC5D0).
pub fn tick_cooldown(p: &mut PlayerState, dt: f32, cfg: &BlinkConfig, actions: &mut Vec<LeftAction>) {
    if !p.blink.active {
        return;
    }
    let lv = level(p, cfg);
    let up = LeftAction::BlinkLayer { target: 1.0, blend: per_level(&cfg.bend_time_up_time, lv) };
    if !p.alive {
        p.blink.active = false;
        p.blink.cooling = false;
        p.left.blink_target = None;
        actions.push(up);
        return;
    }
    if p.blink.cooling {
        p.blink.cooldown_timer += dt;
        let max = per_level(&cfg.cooldown_time, lv);
        if max < p.blink.cooldown_timer {
            p.blink.cooldown_timer = max;
            p.blink.active = false;
            p.blink.cooling = false;
            actions.push(up);
        }
    }
}

/// Index of the pawn's Blink level in the per-level tweak arrays (`m_HorizDistance[level]`...):
/// its `m_Powers` level 1 or 2 (index 0 or 1); a pawn without Blink recorded uses [`BLINK_LEVEL`].
fn level(p: &PlayerState, cfg: &BlinkConfig) -> usize {
    let lv = match p.inv.power_levels[crate::state::power::BLINK] {
        0 => BLINK_LEVEL,
        l => (l as usize).min(2) - 1,
    };
    lv.min(cfg.horiz_distance.len().saturating_sub(1))
}

/// Radius of the pawn's bounding sphere (cylinder bounds: |(r, r, h)|).
pub fn bounds_radius(mv: &MovementConfig) -> f32 {
    Vec3::new(mv.collision_radius, mv.collision_radius, mv.collision_half_height).length()
}

/// Step 1: the range vector (0x00BF52E0). `height_above_ground` is used only with
/// `m_bLimitVerticalDistanceFromGround`.
pub fn range_vector(dir: Vec3, horiz: f32, vert: f32, limit_from_ground: bool, height_above_ground: f32) -> Vec3 {
    if dir.z <= 0.0 {
        return dir.normalize_or_zero() * horiz;
    }
    let ground = if limit_from_ground { height_above_ground.clamp(0.0, vert) } else { 0.0 };
    let d = dir.normalize_or_zero();
    let t = (vert / d.z).min(horiz);
    let mut v = d * t;
    v.z = v.z.min(vert - ground);
    v
}

fn sweep_box(world: &dyn WorldQuery, from: Vec3, delta: Vec3, extent: [f32; 3]) -> Option<(f32, Vec3, bool)> {
    let probe = PawnBox { center: from, half: Vec3::from(extent) };
    world.sweep_box(&probe, delta, channel::PAWN).map(|h| (h.fraction, h.normal, h.start_penetrating))
}

fn box_fits(world: &dyn WorldQuery, center: Vec3, radius: f32, half_height: f32) -> bool {
    let b = PawnBox::new(center, radius, half_height);
    let mut contacts = Vec::new();
    world.box_contacts(&b, channel::PAWN, &mut contacts);
    contacts.iter().all(|c| c.depth < 0.5)
}

/// Compute the Blink destination (capsule centre) for the current view.
pub fn compute_target(p: &PlayerState, world: &dyn WorldQuery, mv: &MovementConfig, cfg: &BlinkConfig) -> Option<Vec3> {
    let lv = level(p, cfg);
    let (horiz, vert) = (cfg.horiz_distance[lv], cfg.vert_distance[lv]);
    let mut start = eye_position(p, mv);
    let view = view_vector(p.yaw, p.pitch);
    let above_ground = if cfg.limit_vertical_from_ground {
        world.raycast(p.position, -Vec3::Z, vert, channel::PAWN).map_or(vert, |h| h.distance)
    } else {
        0.0
    };
    let range = range_vector(view, horiz, vert, cfg.limit_vertical_from_ground, above_ground);
    let max_dist = range.length();
    if max_dist < 1e-3 {
        return None;
    }
    let dir = range / max_dist;
    let ext = cfg.target_test_extent;

    let mut sweep = sweep_box(world, start, range, ext);
    if matches!(sweep, Some((_, _, true))) {
        start = p.position;
        sweep = sweep_box(world, start, dir * max_dist, ext);
    }
    let end = start + dir * max_dist;
    let mut hit = match sweep {
        Some((f, _, false)) => Some(start + dir * max_dist * f),
        Some((_, _, true)) => Some(start),
        None => None,
    };

    // Step 3: close obstacle -> try offset starts past it.
    if let Some(h) = hit {
        if start.distance(h) < cfg.close_collision_distance {
            let step = cfg.close_collision_offset_step;
            let up = Vec3::Z * step;
            let headroom = world.raycast(start + up, -Vec3::Z, step, channel::PAWN).map_or(1.0, |r| r.distance / step);
            let first = Vec3::new(0.0, 0.0, step * headroom - ext[2]);
            let past = h + dir * step;
            let candidates = [first, -up, Vec3::Y * step, -Vec3::Y * step];
            for off in candidates {
                let a = start + off;
                if sweep_box(world, a, past + off - a, ext).is_none() {
                    let to_end = end - a;
                    hit = match sweep_box(world, a, to_end, ext) {
                        Some((f, _, false)) => Some(a + to_end * f),
                        Some((_, _, true)) => Some(a),
                        None => None,
                    };
                    break;
                }
            }
        }
    }
    let point = hit.unwrap_or(end);

    // Step 4: back off until a crawl-sized box fits.
    let cam_dist = eye_position(p, mv).distance(point);
    let step_back = bounds_radius(mv) * 0.5;
    let mut k = 0.0;
    while k < cam_dist {
        if box_fits(world, point - dir * k, mv.collision_radius, mv.crawl_half_height) {
            break;
        }
        k += step_back;
    }
    let mut target = point - dir * k;

    // Step 5: at least CollisionHeight above the floor straight below.
    if let Some(g) = world.raycast(target, -Vec3::Z, GROUND_PROBE, channel::PAWN) {
        let ground_z = target.z - g.distance;
        let lift = mv.collision_half_height - (target.z - ground_z);
        if lift > 0.0 {
            target.z += lift;
        }
    }
    Some(target)
}

/// Per-tick Blink handling from the left hand: `pressed` starts the power (aiming), `held`
/// keeps aiming, `released` moves or cancels.
#[allow(clippy::too_many_arguments)]
pub fn update(
    p: &mut PlayerState,
    pressed: bool,
    held: bool,
    released: bool,
    ctx: &MoveCtx,
    cfg: &BlinkConfig,
    vitals: &VitalsConfig,
    actions: &mut Vec<LeftAction>,
    events: &mut Vec<SimEvent>,
) {
    if p.mode == MoveMode::Blinking {
        let done = travel(p, ctx, cfg);
        // The Blink step's probe grabs a chain it meets (0x00BECDF0); the climb takes over.
        if crate::climb::try_touch_grab(p, ctx, events) {
            enter_cooldown(p, cfg, actions);
            events.push(SimEvent::BlinkEnd { id: p.id });
            return;
        }
        if done {
            // Blink end (0x00BFAB80): restore the velocity and physics from before.
            p.mode = MoveMode::Falling;
            p.velocity = p.blink.origin_velocity;
            enter_cooldown(p, cfg, actions);
            events.push(SimEvent::BlinkEnd { id: p.id });
            // Then the Blink edge finder's ledge, if any, is mantled (0x00BE7420).
            crate::movement::try_blink_mantle(p, ctx, events);
        }
        return;
    }
    // Start (0x00BFC020): only when CanUse; otherwise nothing happens (no aim, nothing to fizzle).
    if pressed && can_use(p, cfg) {
        p.blink.active = true;
        p.blink.cooling = false;
        p.blink.cooldown_timer = 0.0;
    }
    if !aiming(p) {
        p.left.blink_target = None;
        return;
    }
    if held {
        p.left.blink_target = compute_target(p, ctx.world, ctx.cfg, cfg);
    }
    if released || !held {
        let target = p.left.blink_target.take();
        let can = matches!(p.mode, MoveMode::Walking | MoveMode::Falling);
        match target {
            Some(to) if can && to.distance(p.position) > bounds_radius(ctx.cfg) && spend_mana(p, cfg.mana_cost, vitals) => {
                let from = p.position;
                if p.crouched {
                    p.crouched = false;
                }
                let lv = level(p, cfg);
                p.blink = BlinkTravel {
                    target: to,
                    origin: from,
                    origin_velocity: p.velocity,
                    max_distance: from.distance(to),
                    step_timer: 0.0,
                    steps: 0,
                    active: true,
                    cooling: false,
                    cooldown_timer: 0.0,
                };
                actions.push(LeftAction::BlinkLayer { target: WORLD_SLOW_TARGET, blend: per_level(&cfg.bend_time_down_time, lv) });
                p.traversal = crate::state::Traversal { from, to, elapsed: 0.0, duration: 0.0, edge_height: 0.0, anim: None, rate: 0.0, step_ups: p.traversal.step_ups };
                p.mode = MoveMode::Blinking;
                p.velocity = Vec3::ZERO;
                events.push(SimEvent::BlinkStart { id: p.id, from, to });
                // 0x00BFC990 calls the component's use notification (slot 94) as the blink starts.
                events.push(SimEvent::PowerUsed { id: p.id, power: crate::state::power::BLINK as u8, level: p.inv.power_levels[crate::state::power::BLINK] });
                // Port value: no Blink AI noise was found in the original (stealth.md section 5.4).
                events.push(SimEvent::Sound { position: from, radius: 400.0, category: SoundCategory::Blink, context: crate::events::NoiseContext::Anomaly, source: p.id });
            }
            _ => {
                // Cancel (0x00BFAB80 with 1): the cooldown follows a cancel as well.
                enter_cooldown(p, cfg, actions);
                events.push(SimEvent::BlinkFizzle { id: p.id });
            }
        }
    }
}

/// One tick of Blink travel. Returns true when the move is over.
fn travel(p: &mut PlayerState, ctx: &MoveCtx, cfg: &BlinkConfig) -> bool {
    let lv = level(p, cfg);
    let step_time = cfg.time_between_steps.get(lv).copied().unwrap_or(0.01);
    let b = &mut p.blink;
    b.step_timer -= ctx.dt;
    let mut done = false;
    if b.step_timer <= 0.0 {
        b.step_timer += step_time;
        let delta = b.target - p.position;
        let view = view_vector(p.yaw, p.pitch);
        if delta.dot(view) <= 0.0 {
            done = true;
        } else {
            let step = cfg.step_distance;
            let before = p.position;
            p.position = if delta.length_squared() >= step * step { p.position + delta.normalize() * step } else { b.target };
            if p.position.distance(before) < 2.0 {
                done = true;
            }
            b.steps = b.steps.saturating_add(1);
        }
    }
    p.velocity = Vec3::ZERO;
    let flat = (b.target - p.position).truncate().length();
    if flat < REACHED_DISTANCE || p.position.distance(b.origin) > b.max_distance {
        done = true;
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_level_or_down_is_horizontal_distance_along_view() {
        let v = range_vector(Vec3::new(0.6, 0.0, -0.8), 1600.0, 500.0, false, 0.0);
        assert!((v.length() - 1600.0).abs() < 1e-3);
        assert!((v.normalize() - Vec3::new(0.6, 0.0, -0.8)).length() < 1e-5);
    }

    #[test]
    fn range_looking_up_is_limited_by_vertical_distance() {
        let d = Vec3::new(1.0, 0.0, 1.0).normalize();
        let v = range_vector(d, 1600.0, 500.0, false, 0.0);
        assert!((v.z - 500.0).abs() < 1e-3 && (v.x - 500.0).abs() < 1e-3, "{v}");
        // Shallow upward aim: capped by the horizontal distance first.
        let s = Vec3::new(1.0, 0.0, 0.05).normalize();
        assert!((range_vector(s, 1600.0, 500.0, false, 0.0).length() - 1600.0).abs() < 1e-2);
    }
}
