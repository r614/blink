//! Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission. The shape of the launcher's life (fired by the level's sequence, the shot sweeping through the characters in its way and then the world and bursting where it ends, the player disarming it for ammunition) follows game/src/traps.rs `launch` / `fly_darts` / `disarm_focus`; its ballistic `aim` arc, its 0.8 view cone, its 1.5 s fire delay and its per-launcher constants were replaced by the decoded rules below.
//!
//! Projectile launchers (`DisProjectileLauncher`, research/dishonored/systems/interactables.md
//! section 2), decoded from `Dishonored.exe` with the static index.
//!
//! # Firing (`DisSeqAct_ActivateProjectileLauncher`, 0x00B9B1D0 -> `Fire`, 0x00A33F70)
//!
//! - The action's `Activated` walks its "Target" variable links and fires every target that is a
//!   `DisProjectileLauncher`, passing the action's `m_pProjInstigator` (a pawn property; none in
//!   every shipped level) as the projectile's instigator. It is a latent action that finishes
//!   at once: its outputs follow in the same pass.
//! - `Fire` does nothing unless `m_bArmed` (bit 0 of +0x2A8). It plays the tweak's
//!   `m_TrippedAnimName` on the mesh (`SetAnim` slot 97, `PlayAnim` slot 77 with no loop, rate 1),
//!   takes the muzzle from `GetSocketWorldLocationAndRotation` (0x00732530) for
//!   `m_FiringSocketName` (without that socket: a point in front of the mesh from its bounds,
//!   not ported; every shipped launcher has its socket), and, when the tweak has a projectile
//!   (`m_pProjectileTweak`), spawns it at the muzzle with the socket's rotation (0x00C66070 ->
//!   the tweak's spawn slot), instigated by the action's pawn.
//! - **Aim**: with a `m_pTarget`, `FUN_00C3F2A0` runs in mode 2 (its "straight at it" branch,
//!   not the ballistic solver 0x00BEC130 of modes 0 and 1): the direction is the target's
//!   location (an actor that is not a pawn: its collision component's location; a pawn: the
//!   midpoint of two of its body points) minus the muzzle, normalised (0x004019E0, tolerance
//!   1e-8). With no target the direction is the socket's forward axis (`FUN_0040DA70`,
//!   rotator -> vector). The shot is not aimed over gravity: it falls under its own gravity.
//! - **Launch** (`DisProjectile` slot 233, 0x00C540A0, with `Arrow_Explosive`'s 0x00C54E70 on
//!   top): velocity = `m_fProjectileSpeed x direction x DisTweaks_Projectile.m_fSpeedMultiplier`
//!   (+0x98); `m_Damage` = `m_fProjectileDamage` (int), `m_fHeadshotMultiplier` =
//!   `m_fProjectileHeadshotMult`. One shot per `Fire`: the end of the function clears `m_bArmed`.
//!   There is no delay: the shot leaves in the activation's pass, with the animation.
//! - **Flight and hit**: the projectile falls under `GetGravityZ x m_fGravityMultiplier`
//!   (0x00C54F00 predicts it that way). A hit runs the generic contact processor 0x00C4F3A0: the
//!   hit pawn takes `DisProjectile` slot 239 (0x00C4A310: `m_Damage x m_fDamageMultiplier`, times
//!   the headshot factor on the head) through its `TakeDamage`; the explosive arrow's contact
//!   handler (slot 103, 0x00C5A4C0 -> 0x00C550D0) then spawns the explosion tweak's `DisExplosion`
//!   10 uu back along the impact direction, instigated by the arrow's instigator, once.
//! - **Explosion** (`TrapArrowExplosion_twk`): the engine's `DisTweaks_Explosion` rules the
//!   grenades use (`gadgets::explode_with`): `m_RadialDamage` (30 on every difficulty) inside
//!   `m_fFullDamageRadius` (300; players 150), falling to 0 at `m_fDamageRadius` (600), momentum
//!   1500 and 500 up, and an `EAINoiseLoudness4` AI noise (6000 uu).
//!
//! # The player (`Interact`, interface slot 24, 0x00A3B6E0)
//!
//! The launcher's interface slot 9 (0x00633610) is "no alternate action", so Use runs `Interact`
//! at the press (0x00AD5BA0 -> 0x00A3DF60, no hold-to-peek wait):
//!
//! 1. If `m_bArmed`, it is cleared and the tweak's `m_DeactivatedAnimName` plays (0x00A34280).
//! 2. If not `m_bHarvested` and the tweak needs no item (`m_pItemRequiredToHarvestAmmo` none) or
//!    the player holds it (0x00C0B890), each of the 12 `m_HarvestedAmmo` counts is added to the
//!    inventory (`AddAmmo`, 0x00C052E0: what fits under the capacity); any amount added counts
//!    as a harvest (the projectile tweak's pickup sound, a HUD ammo notice 0x00B94FB0).
//! 3. Otherwise the projectile tweak's `m_pCannotPickupSoundEvent` plays, and when the
//!    launcher was not armed its interactable's `m_CannotUseMessage` shows.
//! 4. `m_bHarvested` is set from what step 2 did: **this clears the flag again when step 2 was
//!    skipped or added nothing**, so a launcher that was harvested gives its ammunition again on
//!    the second press after (the decompiled store is `bit1 = local_10`, executed on every path).
//! 5. It returns "used" when it was armed or harvested now; the generic wrapper then fires
//!    `DisSeqEvent_Interact` output 0 (instigator the launcher) and shows the interactable's
//!    `m_UseMessage` ("Launcher disarmed").
//!
//! `AltInteract` (slot 25, 0x00A33E30) only fills the user's interaction-slot table and answers
//! "used"; no level uses it. Not decoded: what makes the prompt appear or vanish (the
//! interactable focus probe), `DisSkeletalBreakable`'s damage path (`m_Health` -1 in the shipped
//! launchers: never destroyed), `SeqAct_Toggle` (`OnToggle_Native`, 0x009ED800 -> slot 238,
//! 0x00A21650: an input on the toggle's first input arms, its second disarms, otherwise it
//! flips; ported as `toggle`).

use crate::events::{DamageCause, SimEvent};
use crate::inventory;
use crate::state::{PlayerId, PlayerState, ProjectileKind, ProjectileState, WorldState};
use dishonored_ir::gameplay::GameplayConfig;
use dishonored_ir::interactables::{Interactables, LauncherDef, ShotKind};
use dishonored_ir::items::AmmoType;
use dishonored_world::{WorldQuery, channel, geom::closest_segment_segment};
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How far back along the impact direction the explosive arrow's explosion spawns
/// (0x00C550D0: `location + direction x 10`, the direction reversed by the caller).
pub const EXPLOSION_BACKOFF: f32 = 10.0;
/// A launched projectile that has not hit anything is dropped after this long (the port's cap,
/// as for the Tallboy's arrow; the projectile's own `LifeSpan` was not read).
pub const SHOT_LIFETIME: f32 = 5.0;
/// Shots of the watch towers share the table: their key is this plus the tower's index
/// (`Interactables::watchtowers`).
pub const TOWER_SHOT_BASE: usize = 1 << 20;

/// The projectile a shot was fired with (a launcher's or a watch tower's).
fn shot_projectile(defs: &Interactables, key: usize) -> Option<&dishonored_ir::interactables::LauncherProjectile> {
    if key >= TOWER_SHOT_BASE {
        defs.watchtowers.get(key - TOWER_SHOT_BASE).map(|w| &w.projectile)
    } else {
        defs.launchers.get(key).map(|l| &l.projectile)
    }
}

/// `m_bArmed` / `m_bHarvested` of one launcher, and whether its animations have played.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherState {
    pub armed: bool,
    pub harvested: bool,
    /// The tripped animation (`m_TrippedAnimName`) was started by a `Fire`.
    pub fired: bool,
    /// The deactivated animation (`m_DeactivatedAnimName`) was started by a disarm.
    pub defused: bool,
}

/// The launchers of the active map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchersState {
    /// One per `Interactables::launchers`.
    pub launchers: Vec<LauncherState>,
    /// The flying shots: projectile id -> launcher index.
    pub shots: BTreeMap<u32, usize>,
}

/// What an `Interact` did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InteractOutcome {
    /// `Interact` returned true (armed or harvested now): the `DisSeqEvent_Interact` fires and
    /// the `m_UseMessage` shows.
    pub used: bool,
    /// The launcher was armed and is not now.
    pub disarmed: bool,
    /// The ammunition added to the inventory.
    pub harvested: Vec<(AmmoType, u16)>,
    /// The projectile tweak's "cannot pickup" sound played.
    pub cannot_pickup_sound: bool,
    /// The interactable's `m_CannotUseMessage` shows (nothing taken and it was not armed).
    pub cannot_use_message: bool,
}

impl LaunchersState {
    pub fn new(defs: &Interactables) -> Self {
        LaunchersState { launchers: defs.launchers.iter().map(|l| LauncherState { armed: l.armed, ..Default::default() }).collect(), shots: BTreeMap::new() }
    }

    /// Keep the state in step with the defs (a map reloaded with another launcher count).
    fn sync(&mut self, defs: &Interactables) {
        if self.launchers.len() != defs.launchers.len() {
            *self = LaunchersState::new(defs);
        }
    }

    /// Launcher index of an actor (`name`; `package` when given), case-insensitive.
    pub fn find(defs: &Interactables, package: Option<&str>, name: &str) -> Option<usize> {
        defs.launchers.iter().position(|l| l.name.eq_ignore_ascii_case(name) && package.is_none_or(|p| l.package.eq_ignore_ascii_case(p)))
    }

    /// `Fire` (0x00A33F70) for launcher `i` with the action's instigator (`owner`, 0 for none).
    /// Returns whether a shot left (the launcher was armed).
    pub fn fire(&mut self, defs: &Interactables, i: usize, owner: PlayerId, state: &mut WorldState, events: &mut Vec<SimEvent>) -> bool {
        self.sync(defs);
        let (Some(def), Some(st)) = (defs.launchers.get(i), self.launchers.get_mut(i)) else { return false };
        if !st.armed {
            return false;
        }
        st.armed = false;
        st.fired = true;
        let pr = &def.projectile;
        // A launcher whose tweak has no projectile only plays its animation.
        if pr.speed <= 0.0 && pr.tweak.is_empty() {
            return true;
        }
        let from = def.muzzle;
        let dir = shot_direction(def);
        let velocity = dir * pr.speed;
        let kind = match pr.kind {
            ShotKind::ExplosiveArrow => ProjectileKind::TrapArrow,
            ShotKind::Bolt => ProjectileKind::TrapBolt,
        };
        let id = state.next_projectile_id;
        state.next_projectile_id = state.next_projectile_id.wrapping_add(1);
        // `m_Damage x m_fDamageMultiplier` (0x00C4A310) is what a hit pawn takes.
        let damage = pr.damage as f32 * pr.damage_multiplier;
        state.projectiles.push(ProjectileState { id, kind, owner, position: from, velocity, age: 0.0, damage, gravity_mult: pr.gravity_mult, armed: false, fuse: 0.0, min_fuse: 0.0, stuck_to: None, stuck: false, stuck_offset: Vec3::ZERO, spawn: from, pierced: None });
        self.shots.insert(id, i);
        events.push(SimEvent::Fired { id: owner, bolt: false, projectile: id, kind, origin: from, velocity });
        true
    }


    /// A watch tower's shot (the update 0x00A3C640 -> 0x00B14E80 from its arrow socket): `velocity`
    /// along the aim with the tower's projectile; it flies, strikes and bursts like a launcher's.
    pub fn fire_tower(&mut self, defs: &Interactables, tower: usize, from: Vec3, velocity: Vec3, state: &mut WorldState, events: &mut Vec<SimEvent>) {
        let Some(def) = defs.watchtowers.get(tower) else { return };
        let pr = &def.projectile;
        let kind = match pr.kind {
            ShotKind::ExplosiveArrow => ProjectileKind::TrapArrow,
            ShotKind::Bolt => ProjectileKind::TrapBolt,
        };
        let id = state.next_projectile_id;
        state.next_projectile_id = state.next_projectile_id.wrapping_add(1);
        let damage = pr.damage as f32 * pr.damage_multiplier;
        state.projectiles.push(ProjectileState { id, kind, owner: 0, position: from, velocity, age: 0.0, damage, gravity_mult: pr.gravity_mult, armed: false, fuse: 0.0, min_fuse: 0.0, stuck_to: None, stuck: false, stuck_offset: Vec3::ZERO, spawn: from, pierced: None });
        self.shots.insert(id, TOWER_SHOT_BASE + tower);
        events.push(SimEvent::Fired { id: 0, bolt: false, projectile: id, kind, origin: from, velocity });
    }
    /// `SeqAct_Toggle` on a launcher (`OnToggle_Native` 0x009ED800 -> slot 238, 0x00A21650):
    /// `input` is the toggle's input (0 turn on, 1 turn off, 2 toggle); `bToggled`-style
    /// actions that set neither leave it.
    pub fn toggle(&mut self, defs: &Interactables, i: usize, input: usize) {
        self.sync(defs);
        if let Some(st) = self.launchers.get_mut(i) {
            match input {
                0 => st.armed = true,
                1 => st.armed = false,
                _ => st.armed = !st.armed,
            }
        }
    }

    /// `Interact` (0x00A3B6E0): see the module docs. `has_item` says whether the player holds
    /// the item the launcher's tweak requires to harvest (true when it requires none).
    pub fn interact(&mut self, defs: &Interactables, i: usize, p: &mut PlayerState, has_item: bool, cfg: &GameplayConfig) -> InteractOutcome {
        self.sync(defs);
        let mut out = InteractOutcome::default();
        let (Some(def), Some(st)) = (defs.launchers.get(i), self.launchers.get_mut(i)) else { return out };
        let was_armed = st.armed;
        if was_armed {
            // 0x00A34280.
            st.armed = false;
            st.defused = true;
            out.disarmed = true;
        }
        let mut took = false;
        if !st.harvested && has_item {
            for &(ty, n) in &def.harvest {
                let Some(t) = AmmoType::ALL.get(ty as usize).copied() else { continue };
                let added = inventory::add_ammo(p, t, n.clamp(0, u16::MAX as i32) as u16, cfg);
                if added > 0 {
                    took = true;
                    out.harvested.push((t, added));
                }
            }
        }
        if !took {
            out.cannot_pickup_sound = true;
            out.cannot_use_message = !was_armed;
        }
        // The store at the end of the function: `m_bHarvested` = what this press harvested.
        st.harvested = took;
        out.used = was_armed || took;
        out
    }

    /// The launcher the view meets within its interactable's reach (`m_fMaxDistance`): the
    /// nearest launcher whose collision bounds the ray enters. `present(i)` says whether the
    /// launcher is in the world (a Kismet-streamed sublevel that is not loaded has none).
    pub fn focused(defs: &Interactables, eye: Vec3, dir: Vec3, present: impl Fn(usize) -> bool) -> Option<(usize, f32)> {
        defs.launchers
            .iter()
            .enumerate()
            .filter(|(i, l)| present(*i) && l.bounds_min.cmplt(l.bounds_max).all())
            .filter_map(|(i, l)| crate::interactables::ray_box(eye, dir, l.bounds_min, l.bounds_max, l.max_distance).map(|t| (i, t)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// Advance the launchers' shots (call after the world stepped; `dt` is the projectile time
    /// step). Each falls, strikes the first pawn or wall on its path, deals the direct hit and, for
    /// an explosive arrow, bursts 10 uu back along its direction.
    pub fn step_shots(&mut self, state: &mut WorldState, defs: &Interactables, world: &dyn WorldQuery, cfg: &GameplayConfig, dt: f32, events: &mut Vec<SimEvent>) {
        if !state.projectiles.iter().any(|p| p.kind.is_launcher_shot()) {
            self.shots.clear();
            return;
        }
        let mv = &cfg.movement;
        let projectiles = std::mem::take(&mut state.projectiles);
        let mut keep = Vec::with_capacity(projectiles.len());
        let mut booms: Vec<(usize, PlayerId, Vec3)> = Vec::new();
        for mut pr in projectiles {
            if !pr.kind.is_launcher_shot() {
                keep.push(pr);
                continue;
            }
            let Some(li) = self.shots.get(&pr.id).copied() else { continue };
            pr.age += dt;
            let old_v = pr.velocity;
            pr.velocity.z += mv.gravity_z * pr.gravity_mult * dt;
            let delta = (old_v + pr.velocity) * 0.5 * dt;
            let len = delta.length();
            let dir = delta.normalize_or_zero();
            let from = pr.position;
            let world_hit = if len > 1e-6 { world.raycast(from, delta, len, channel::PROJECTILE).map(|h| h.distance) } else { None };
            // The first pawn on the path: its capsule against the step (the instigator is skipped,
            // `m_bIgnoreCollisionsWithSource*`).
            let mut body: Option<(usize, f32, Vec3)> = None;
            for (bi, v) in state.players.iter().enumerate() {
                if !v.alive || (pr.owner != 0 && v.id == pr.owner) || v.carried_by.is_some() || crate::powers::is_hidden(v) {
                    continue;
                }
                let hh = crate::movement::half_height(v, mv);
                let a0 = v.position - Vec3::Z * (hh - mv.collision_radius);
                let a1 = v.position + Vec3::Z * (hh - mv.collision_radius);
                let (on_seg, on_axis) = closest_segment_segment(from, from + delta, a0, a1);
                if on_seg.distance(on_axis) <= mv.collision_radius {
                    let along = (on_seg - from).length();
                    if body.is_none_or(|(_, d, _)| along < d) {
                        body = Some((bi, along, on_seg));
                    }
                }
            }
            let kind = shot_projectile(defs, li).map_or(ShotKind::Bolt, |p| p.kind);
            let attacker = Some(pr.owner).filter(|o| *o != 0);
            // A wall of light that collides takes the hit and destroys the shot (0x00C4F3A0).
            if let Some((wall, point)) = crate::walls::projectile_hit(state, from, from + delta, world_hit, body.map(|b| b.1)) {
                events.push(SimEvent::WallOfLightHit { wall, point, projectile: pr.id, owner: pr.owner });
                self.shots.remove(&pr.id);
                continue;
            }
            match (body, world_hit) {
                (Some((bi, along, point)), wh) if wh.is_none_or(|w| along <= w) => {
                    events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: Some(state.players[bi].id) });
                    // 0x00C4F3A0 -> slot 239 (0x00C4A310): the direct hit, an int to `TakeDamage`;
                    // on the head `m_fHeadshotMultiplier` (the launcher's `m_fProjectileHeadshotMult`, 1
                    // in the shipped tweaks) scales it.
                    let head = crate::hit_regions::hit_region(&state.players[bi], mv, &cfg.items.hit_bodies, point, dir) == dishonored_ir::items::HitRegion::Head;
                    let damage = if head { pr.damage * shot_projectile(defs, li).map_or(1.0, |p| p.headshot_mult) } else { pr.damage };
                    crate::vitals::apply_damage(&mut state.players[bi], damage, attacker, DamageCause::TrapShot, events, &cfg.vitals);
                    if kind == ShotKind::ExplosiveArrow {
                        booms.push((li, pr.owner, point - dir * EXPLOSION_BACKOFF));
                    }
                }
                (_, Some(w)) => {
                    let point = from + dir * w;
                    events.push(SimEvent::ProjectileImpact { projectile: pr.id, point, victim: None });
                    if kind == ShotKind::ExplosiveArrow {
                        booms.push((li, pr.owner, point - dir * EXPLOSION_BACKOFF));
                    }
                }
                _ => {
                    pr.position = from + delta;
                    if pr.age < SHOT_LIFETIME && pr.position.z > crate::movement::KILL_Z {
                        keep.push(pr);
                        continue;
                    }
                }
            }
            self.shots.remove(&pr.id);
        }
        state.projectiles = keep;
        let live: Vec<u32> = state.projectiles.iter().map(|p| p.id).collect();
        self.shots.retain(|id, _| live.contains(id));
        for (li, owner, at) in booms {
            let Some(x) = shot_projectile(defs, li).and_then(|p| p.explosion.as_ref()) else { continue };
            crate::gadgets::explode_with(state, owner, at, None, x, DamageCause::TrapExplosion, false, world, cfg, events);
        }
    }
}

/// The low ballistic arc (0x00BEC130): the unit launch direction from `from` to `to` at `speed`
/// under `g` (positive, uu/s^2), and the flight time when the target is in reach. Out of reach
/// the pitch is 45 degrees (no time); points closer than 1e-4 uu aim straight up.
pub fn ballistic(from: Vec3, to: Vec3, speed: f32, g: f32) -> (Vec3, Option<f32>) {
    let d = to - from;
    let h2 = d.x * d.x + d.y * d.y;
    if d.z * d.z + h2 < 1e-8 {
        return (Vec3::Z, None);
    }
    let h = h2.sqrt();
    let mut angle = std::f32::consts::FRAC_PI_4;
    let mut time = None;
    if h >= 1e-8 {
        let v2 = speed * speed;
        let disc = v2 * v2 - (h * h * g + v2 * d.z * 2.0) * g;
        if disc >= 1e-8 {
            angle = ((v2 - disc.sqrt()) / (h * g)).atan();
            time = Some(h / (angle.cos() * speed));
        }
    }
    let flat = Vec3::new(d.x, d.y, 0.0).normalize_or_zero();
    ((flat * angle.cos() + Vec3::Z * angle.sin()).normalize_or_zero(), time)
}

/// The aim of a scripted or AI shot (0x00C3F2A0, the projectile tweak's): `mode` is the action's
/// `EDisComputeHitAccuracy`. Modes 0 and 1 solve the ballistic arc at `speed`
/// (`m_fSpeedMultiplier` x the action's) under `g` (`m_fGravityMultiplier` x -GravityZ); mode 0
/// then leads the target by its velocity over the flight time (flat when `flat_lead`: the floor
/// aim of region 1) and solves again. Any other mode aims straight at the point.
#[allow(clippy::too_many_arguments)]
pub fn aim(mode: u8, from: Vec3, point: Vec3, target_velocity: Vec3, flat_lead: bool, speed: f32, g: f32) -> Vec3 {
    if mode != 0 && mode != 1 {
        return (point - from).normalize_or_zero();
    }
    let (dir, time) = ballistic(from, point, speed, g);
    match (mode, time) {
        (0, Some(t)) => {
            let mut lead = target_velocity * t;
            if flat_lead {
                lead.z = 0.0;
            }
            ballistic(from, point + lead, speed, g).0
        }
        _ => dir,
    }
}

/// A `DisSeqAct_FireProjectile` shot (0x00B8F340): the tweak's projectile spawned at the source
/// and launched along `dir` at `speed x m_fSpeedMultiplier` with the action's damage (slot 233
/// 0x00C540A0, `m_Damage`); it flies and strikes as the player's crossbow round of its class
/// does (`DisProjectile_Arrow`). No instigator. Returns the projectile id.
pub fn fire_scripted(def: &dishonored_ir::interactables::ScriptedProjectileDef, from: Vec3, dir: Vec3, speed: f32, damage: i32, state: &mut WorldState, events: &mut Vec<SimEvent>) -> Option<u32> {
    let kind = match def.class.as_str() {
        "DisTweaks_Arrow_Flare" => ProjectileKind::IncendiaryBolt,
        c if c.starts_with("DisTweaks_Arrow_Sleep") => ProjectileKind::SleepDart,
        "DisTweaks_Arrow" => ProjectileKind::Bolt,
        _ => return None,
    };
    let id = state.next_projectile_id;
    state.next_projectile_id = state.next_projectile_id.wrapping_add(1);
    let velocity = dir * speed * def.speed_mult;
    state.projectiles.push(ProjectileState { id, kind, owner: 0, position: from, velocity, age: 0.0, damage: damage as f32, gravity_mult: def.gravity_mult, armed: false, fuse: 0.0, min_fuse: 0.0, stuck_to: None, stuck: false, stuck_offset: Vec3::ZERO, spawn: from, pierced: None });
    events.push(SimEvent::Fired { id: 0, bolt: true, projectile: id, kind, origin: from, velocity });
    Some(id)
}

/// The direction a launcher's shot leaves in: at the target, else along the muzzle socket.
pub fn shot_direction(def: &LauncherDef) -> Vec3 {
    match def.target {
        // Mode 2 of 0x00C3F2A0: `target - muzzle`, normalised (zero when they coincide).
        Some(t) => (t - def.muzzle).normalize_or_zero(),
        None => def.muzzle_forward,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::interactables::LauncherProjectile;
    use dishonored_ir::items::ExplosionConfig;
    use dishonored_world::CollisionWorld;

    fn cfg() -> GameplayConfig {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
        crate::SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap().gameplay
    }

    /// The Prison Sewer's whiskey launcher: the trap's explosive arrow, one incendiary bolt.
    fn whiskey() -> LauncherDef {
        LauncherDef {
            name: "DisProjectileLauncher_0".into(),
            package: "T".into(),
            position: Vec3::new(0.0, 0.0, 100.0),
            muzzle: Vec3::new(0.0, 0.0, 130.0),
            muzzle_forward: Vec3::X,
            target: Some(Vec3::new(1000.0, 0.0, 130.0)),
            armed: true,
            bounds_min: Vec3::new(-30.0, -30.0, 100.0),
            bounds_max: Vec3::new(30.0, 30.0, 160.0),
            tripped_anim: "TripodTrap_Fire".into(),
            deactivated_anim: "TripodTrap_Defuse".into(),
            projectile: LauncherProjectile {
                kind: ShotKind::ExplosiveArrow,
                tweak: "SecuritySystems_twk.TripodTrap.TrapExplosiveArrow_twk".into(),
                class: "DisTweaks_Arrow_Explosive".into(),
                speed: 2000.0,
                damage: 10,
                headshot_mult: 1.0,
                damage_multiplier: 1.0,
                gravity_mult: 0.5,
                invisible: true,
                explosion: Some(ExplosionConfig { radial_damage: 30.0, damage_radius: 600.0, full_damage_radius: 300.0, player_damage_radius: 600.0, player_full_damage_radius: 150.0, radial_momentum: 1500.0, extra_up_force: 500.0, attached_damage: 0.0, ai_noise_radius: 6000.0, radial_damage_type: "DishonoredDamageType_Explosion".into() }),
            },
            harvest: vec![(4, 1)],
            required_item: String::new(),
            label: "Projectile Launcher".into(),
            prompt: "`GBA_Use` Disarm".into(),
            use_message: "Launcher disarmed".into(),
            cannot_use_message: String::new(),
            max_distance: 200.0,
        }
    }

    fn defs() -> Interactables {
        Interactables { launchers: vec![whiskey()], ..Default::default() }
    }

    fn pawn(id: PlayerId, at: Vec3) -> PlayerState {
        let mut p = PlayerState::new(id, at, 0.0);
        p.alive = true;
        p
    }

    fn empty_world() -> CollisionWorld {
        CollisionWorld::from_triangles(Vec::new(), Vec::new())
    }
    /// A wall of two triangles across x = `x`.
    fn wall(x: f32) -> CollisionWorld {
        let (a, b, c, d) = (Vec3::new(x, -2000.0, -2000.0), Vec3::new(x, 2000.0, -2000.0), Vec3::new(x, 2000.0, 2000.0), Vec3::new(x, -2000.0, 2000.0));
        let n = -Vec3::X;
        CollisionWorld::from_triangles(vec![dishonored_world::Tri { a, b, c, n, channels: channel::ALL, source: 0 }, dishonored_world::Tri { a, b: c, c: d, n, channels: channel::ALL, source: 0 }], vec![String::new()])
    }

    /// Flies `dir x speed` under `g` for `t` seconds.
    fn land(from: Vec3, dir: Vec3, speed: f32, g: f32, t: f32) -> Vec3 {
        from + dir * speed * t - Vec3::Z * 0.5 * g * t * t
    }

    #[test]
    fn the_ballistic_solve_lands_on_the_point_and_leads_a_mover() {
        let (from, to, speed, g) = (Vec3::new(0.0, 0.0, 800.0), Vec3::new(500.0, -300.0, 0.0), 900.0, 1520.0 * 1.3);
        let (dir, t) = ballistic(from, to, speed, g);
        let t = t.expect("in reach");
        assert!((dir.length() - 1.0).abs() < 1e-4);
        assert!(land(from, dir, speed, g, t).distance(to) < 1.0, "the low arc lands on the point: {:?}", land(from, dir, speed, g, t));
        // Out of reach: 45 degrees up, no time.
        let (far, none) = ballistic(Vec3::ZERO, Vec3::new(1.0e6, 0.0, 0.0), 900.0, g);
        assert!(none.is_none() && (far.z - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4);
        // Mode 0 leads a target moving along y by its velocity over the flight time.
        let vel = Vec3::new(0.0, 200.0, 0.0);
        let led = aim(0, from, to, vel, false, speed, g);
        let (_, t2) = ballistic(from, to + vel * t, speed, g);
        assert!(land(from, led, speed, g, t2.unwrap()).distance(to + vel * t) < 1.0);
        // Mode 2 is straight at the point.
        assert!((aim(2, from, to, vel, false, speed, g) - (to - from).normalize()).length() < 1e-5);
    }

    #[test]
    fn a_shot_leaves_at_the_target_at_speed_once_and_disarms_the_launcher() {
        let d = defs();
        let mut s = LaunchersState::new(&d);
        let mut w = WorldState::default();
        let mut ev = Vec::new();
        assert!(s.fire(&d, 0, 0, &mut w, &mut ev));
        let pr = w.projectiles[0];
        assert_eq!(pr.kind, ProjectileKind::TrapArrow);
        assert_eq!(pr.position, Vec3::new(0.0, 0.0, 130.0));
        assert!((pr.velocity - Vec3::new(2000.0, 0.0, 0.0)).length() < 1e-3, "straight at the target at m_fProjectileSpeed x m_fSpeedMultiplier: {:?}", pr.velocity);
        assert_eq!((pr.damage, pr.gravity_mult), (10.0, 0.5));
        assert!(matches!(ev[0], SimEvent::Fired { kind: ProjectileKind::TrapArrow, .. }));
        assert!(!s.launchers[0].armed && s.launchers[0].fired);
        // `Fire` does nothing unless armed: one shot per arming.
        assert!(!s.fire(&d, 0, 0, &mut w, &mut ev));
        assert_eq!(w.projectiles.len(), 1);
    }

    #[test]
    fn without_a_target_the_shot_follows_the_muzzle_socket() {
        let mut d = defs();
        d.launchers[0].target = None;
        d.launchers[0].muzzle_forward = Vec3::new(0.0, 1.0, 0.0);
        let mut s = LaunchersState::new(&d);
        let mut w = WorldState::default();
        s.fire(&d, 0, 0, &mut w, &mut Vec::new());
        assert!((w.projectiles[0].velocity - Vec3::new(0.0, 2000.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn the_speed_multiplier_of_the_projectile_tweak_scales_the_launch() {
        // The extractor folds `DisTweaks_Projectile.m_fSpeedMultiplier` into `speed` (0x00C540A0).
        let mut d = defs();
        d.launchers[0].projectile.speed = 2000.0 * 1.5;
        let mut s = LaunchersState::new(&d);
        let mut w = WorldState::default();
        s.fire(&d, 0, 0, &mut w, &mut Vec::new());
        assert!((w.projectiles[0].velocity.length() - 3000.0).abs() < 1e-2);
    }

    #[test]
    fn the_arrow_bursts_at_the_wall_it_strikes_with_the_trap_explosion_rules() {
        let c = cfg();
        let d = defs();
        let mut s = LaunchersState::new(&d);
        let mut w = WorldState::default();
        // Corvo 400 uu short of the wall, off the arrow's line: the burst is 10 uu before the wall.
        w.players.push(pawn(1, Vec3::new(500.0, 400.0, 130.0)));
        let mut ev = Vec::new();
        s.fire(&d, 0, 0, &mut w, &mut ev);
        let world = wall(900.0);
        let dt = crate::movement::DT;
        let mut ticks = 0;
        while !w.projectiles.is_empty() && ticks < 200 {
            s.step_shots(&mut w, &d, &world, &c, dt, &mut ev);
            ticks += 1;
        }
        assert!(w.projectiles.is_empty() && s.shots.is_empty(), "the arrow is spent at the wall");
        let boom = ev.iter().find_map(|e| if let SimEvent::Explosion { position, owner, .. } = e { Some((*position, *owner)) } else { None }).expect("the explosive arrow bursts");
        assert!((boom.0.x - 890.0).abs() < 8.0 && boom.1 == 0, "10 uu back from the wall at x 900, nobody's: {boom:?}");
        let noise = ev.iter().find_map(|e| if let SimEvent::Sound { radius, source, .. } = e { Some((*radius, *source)) } else { None }).expect("an AI noise");
        assert_eq!(noise, (6000.0, 0), "EAINoiseLoudness4 from nobody");
        // Corvo is 400 uu from the burst (full 150, none from 600): 30 x (1 - 250/450) = 13.33 -> 13.
        let dmg = ev.iter().find_map(|e| if let SimEvent::Damage { victim: 1, amount, cause: DamageCause::TrapExplosion, attacker } = e { Some((*amount, *attacker)) } else { None }).expect("the radial damage");
        let d400 = (w.players[0].position - boom.0).length();
        let expect = (30.0 * (1.0 - (d400 - 150.0) / 450.0)).round();
        assert_eq!(dmg, (expect, None), "player radii 150 / 600, owner none: no attacker");
    }

    #[test]
    fn a_pawn_in_the_way_takes_the_direct_hit_then_the_burst_at_the_hit() {
        let c = cfg();
        let d = defs();
        let mut s = LaunchersState::new(&d);
        let mut w = WorldState::default();
        w.players.push(pawn(7, Vec3::new(600.0, 0.0, 130.0)));
        let mut ev = Vec::new();
        s.fire(&d, 0, 0, &mut w, &mut ev);
        let world = empty_world();
        for _ in 0..60 {
            s.step_shots(&mut w, &d, &world, &c, crate::movement::DT, &mut ev);
        }
        let first = ev.iter().position(|e| matches!(e, SimEvent::Damage { victim: 7, cause: DamageCause::TrapShot, .. })).expect("the direct hit");
        let boom = ev.iter().position(|e| matches!(e, SimEvent::Explosion { .. })).expect("the burst");
        assert!(first < boom, "the contact processor deals the hit before the explosion spawns");
        assert!(matches!(ev[first], SimEvent::Damage { amount, attacker: None, .. } if amount == 10.0), "m_Damage 10 x m_fDamageMultiplier 1");
        assert!(ev.iter().any(|e| matches!(e, SimEvent::Damage { victim: 7, cause: DamageCause::TrapExplosion, .. })) || !w.players[0].alive);
    }

    #[test]
    fn a_head_hit_scales_the_direct_damage_by_the_headshot_multiplier() {
        let c = cfg();
        let mut d = defs();
        d.launchers[0].projectile.kind = ShotKind::Bolt;
        d.launchers[0].projectile.explosion = None;
        d.launchers[0].projectile.headshot_mult = 3.0;
        d.launchers[0].projectile.gravity_mult = 0.0;
        // Aimed at the head: the pawn's centre is 60 uu below the arrow's line.
        d.launchers[0].target = Some(Vec3::new(1000.0, 0.0, 130.0));
        let mut s = LaunchersState::new(&d);
        let mut w = WorldState::default();
        w.players.push(pawn(7, Vec3::new(600.0, 0.0, 130.0 - 75.0)));
        let mut ev = Vec::new();
        s.fire(&d, 0, 0, &mut w, &mut ev);
        let world = empty_world();
        for _ in 0..60 {
            s.step_shots(&mut w, &d, &world, &c, crate::movement::DT, &mut ev);
        }
        let amount = ev.iter().find_map(|e| if let SimEvent::Damage { victim: 7, amount, cause: DamageCause::TrapShot, .. } = e { Some(*amount) } else { None }).expect("hit");
        assert_eq!(amount, 30.0, "10 x m_fHeadshotMultiplier 3 on the head");
    }

    #[test]
    fn the_overseers_bolt_launcher_deals_only_the_direct_hit() {
        let c = cfg();
        let mut d = defs();
        d.launchers[0].projectile.kind = ShotKind::Bolt;
        d.launchers[0].projectile.explosion = None;
        d.launchers[0].projectile.gravity_mult = 1.0;
        let mut s = LaunchersState::new(&d);
        let mut w = WorldState::default();
        w.players.push(pawn(7, Vec3::new(600.0, 0.0, 130.0)));
        let mut ev = Vec::new();
        s.fire(&d, 0, 0, &mut w, &mut ev);
        assert_eq!(w.projectiles[0].kind, ProjectileKind::TrapBolt);
        let world = empty_world();
        for _ in 0..60 {
            s.step_shots(&mut w, &d, &world, &c, crate::movement::DT, &mut ev);
        }
        assert!(ev.iter().any(|e| matches!(e, SimEvent::Damage { victim: 7, amount, cause: DamageCause::TrapShot, .. } if *amount == 10.0)));
        assert!(!ev.iter().any(|e| matches!(e, SimEvent::Explosion { .. })), "a plain arrow does not burst");
        assert!(w.projectiles.is_empty());
    }

    #[test]
    fn gravity_bends_the_shot_by_its_multiplier() {
        let c = cfg();
        let d = defs();
        let mut s = LaunchersState::new(&d);
        let mut w = WorldState::default();
        s.fire(&d, 0, 0, &mut w, &mut Vec::new());
        let world = empty_world();
        let n = 30;
        for _ in 0..n {
            s.step_shots(&mut w, &d, &world, &c, crate::movement::DT, &mut Vec::new());
        }
        let t = n as f32 * crate::movement::DT;
        let drop = 130.0 - w.projectiles[0].position.z;
        let free_fall = -0.5 * c.movement.gravity_z * t * t;
        assert!((drop - 0.5 * free_fall).abs() < 0.05 * free_fall, "half the free-fall drop for m_fGravityMultiplier 0.5: {drop} vs {free_fall}");
    }

    #[test]
    fn interact_disarms_an_armed_launcher_and_gives_the_ammunition() {
        let c = cfg();
        let d = defs();
        let mut s = LaunchersState::new(&d);
        let mut p = pawn(1, Vec3::ZERO);
        let before = inventory::ammo(&p, AmmoType::IncendiaryBolt);
        let r = s.interact(&d, 0, &mut p, true, &c);
        assert!(r.used && r.disarmed);
        assert_eq!(r.harvested, vec![(AmmoType::IncendiaryBolt, 1)], "m_HarvestedAmmo[4] = 1: AddAmmo's incendiary bolt");
        assert_eq!(inventory::ammo(&p, AmmoType::IncendiaryBolt), before + 1);
        assert!(!s.launchers[0].armed && s.launchers[0].harvested && s.launchers[0].defused);
        // A disarmed launcher cannot be fired by the tripwire any more.
        assert!(!s.fire(&d, 0, 0, &mut WorldState::default(), &mut Vec::new()));
    }

    #[test]
    fn a_full_pouch_disarms_but_takes_nothing_and_a_fired_launcher_can_still_be_harvested() {
        let c = cfg();
        let d = defs();
        let mut s = LaunchersState::new(&d);
        let mut p = pawn(1, Vec3::ZERO);
        let cap = inventory::capacity(&p, AmmoType::IncendiaryBolt, &c);
        p.inv.ammo[AmmoType::IncendiaryBolt as usize] = cap;
        let r = s.interact(&d, 0, &mut p, true, &c);
        assert!(r.used && r.disarmed && r.harvested.is_empty(), "armed: disarmed, AddAmmo added 0");
        assert!(r.cannot_pickup_sound && !r.cannot_use_message, "the cannot-use message needs it to have been disarmed already");
        assert!(!s.launchers[0].harvested, "m_bHarvested = nothing harvested, so a later press may");
        p.inv.ammo[AmmoType::IncendiaryBolt as usize] = 0;
        // The tripwire fired it earlier; now it is empty and the player takes its ammunition.
        let mut s2 = LaunchersState::new(&d);
        s2.fire(&d, 0, 0, &mut WorldState::default(), &mut Vec::new());
        let r2 = s2.interact(&d, 0, &mut p, true, &c);
        assert!(r2.used && !r2.disarmed && r2.harvested.len() == 1);
    }

    #[test]
    fn the_harvested_flag_is_stored_from_the_press_so_a_second_press_clears_it() {
        // 0x00A3B6E0 ends with `bit1 = local_10` on every path: press 1 harvests, press 2 takes
        // nothing (and clears the flag), press 3 harvests again.
        let c = cfg();
        let d = defs();
        let mut s = LaunchersState::new(&d);
        let mut p = pawn(1, Vec3::ZERO);
        let r1 = s.interact(&d, 0, &mut p, true, &c);
        assert!(r1.used && s.launchers[0].harvested);
        let r2 = s.interact(&d, 0, &mut p, true, &c);
        assert!(!r2.used && r2.harvested.is_empty() && r2.cannot_pickup_sound && r2.cannot_use_message);
        assert!(!s.launchers[0].harvested);
        let r3 = s.interact(&d, 0, &mut p, true, &c);
        assert!(r3.used && r3.harvested.len() == 1, "the flag was cleared by the failed press");
    }

    #[test]
    fn a_required_item_gates_the_harvest_but_not_the_disarm() {
        let c = cfg();
        let mut d = defs();
        d.launchers[0].required_item = "SomeKey_AbsItm".into();
        let mut s = LaunchersState::new(&d);
        let mut p = pawn(1, Vec3::ZERO);
        let r = s.interact(&d, 0, &mut p, false, &c);
        assert!(r.disarmed && r.used && r.harvested.is_empty());
        assert!(!s.launchers[0].armed);
    }

    #[test]
    fn unknown_ammo_slots_are_skipped_and_toggle_arms_and_disarms() {
        let c = cfg();
        let mut d = defs();
        d.launchers[0].harvest = vec![(11, 5), (2, 2)];
        let mut s = LaunchersState::new(&d);
        let mut p = pawn(1, Vec3::ZERO);
        let r = s.interact(&d, 0, &mut p, true, &c);
        assert_eq!(r.harvested.len(), 1, "type 11 is not an ammo type of the port");
        s.toggle(&d, 0, 0);
        assert!(s.launchers[0].armed);
        s.toggle(&d, 0, 1);
        assert!(!s.launchers[0].armed);
        s.toggle(&d, 0, 2);
        assert!(s.launchers[0].armed);
    }

    #[test]
    fn the_focus_is_the_nearest_launcher_the_view_meets_within_its_reach() {
        let d = defs();
        let eye = Vec3::new(-100.0, 0.0, 130.0);
        assert_eq!(LaunchersState::focused(&d, eye, Vec3::X, |_| true).map(|f| f.0), Some(0));
        assert_eq!(LaunchersState::focused(&d, eye, -Vec3::X, |_| true), None, "looking away");
        assert_eq!(LaunchersState::focused(&d, Vec3::new(-400.0, 0.0, 130.0), Vec3::X, |_| true), None, "beyond m_fMaxDistance 200");
        assert_eq!(LaunchersState::focused(&d, eye, Vec3::X, |_| false), None, "in a sublevel that is not loaded");
    }
}
