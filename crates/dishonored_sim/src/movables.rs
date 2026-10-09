//! Movable props (`DishonoredMovable`): pick up, carry, drop and throw. Decoded from
//! `Dishonored.exe`; the rules, addresses and open questions are in
//! research/dishonored/systems/movables.md.
//!
//! - **Pick up** (Use on the focused interactable, `DisTweaks_Movable.m_bInteractable`, reach
//!   `DisTweaks_InteractableInterface.m_fMaxDistance`): the player pawn enters
//!   `StatePlayerGrabMovable` (`m_pPending_Movable`), whose enter (0x00A69860) hands the prop to
//!   the empty hands' item (`DishonoredItemEmpty.m_pGrabbed_Movable`, 0x00C3D030); that fires
//!   `DisSeqEvent_MovablePickedUp` (0x00A45FD0; originator the prop, instigator the pawn).
//! - **Carry**: the item's post-update (0x00C4C510 -> 0x00C4C190) puts the prop at
//!   `eye + forward x D - up x H` with the camera's rotation, `D = box.min.x + box.max.x -
//!   m_fMovableHoldDist` and `H = box.min.z + box.max.z + 70 - m_fShowMovableHeight[weight]`
//!   (`DisTweaks_ItemEmpty`, `Twk_EmptyHands`: -100 and 75 / 90 / 90 / 90). For the first
//!   `m_fInitialGrabTime` (0.066 s) it slides there from where it was grabbed.
//! - **Drop** (Use while holding, 0x00AD5BA0 -> pawn slot 381 with 0) and **throw** (Primary
//!   with the empty hands, 0x00AB4240 -> slot 381 with 1): the pawn sets the grab state's
//!   `m_bThrowOnDrop` and leaves it (0x00AAD900); the exit (0x00A698E0) runs the release
//!   0x00C45340: a box trace from the pawn to the prop moves it back if geometry is in the way,
//!   `Instigator` becomes the pawn (0x00A4ADA0), the component releases it to `PHYS_RigidBody`
//!   and fires `DisSeqEvent_MovableDropped` (0x00A46380), and a throw launches it along the view
//!   at `Attribute_ThrowStrength`. Damage, a dialog, a store, a minigame and the prop breaking
//!   drop it too.
//! - **Hits** (the breakable's rigid-body collision 0x00A40CF0): see [`impact`].
//!
//! The rigid body is the bounded model physics.md section 7 asks for: gravity, a box sweep
//! against the world, no bounce (a contact removes the speed along the normal; a floor ends the
//! flight). Nothing here depends on wall-clock time: it is part of the deterministic step.

use crate::events::{DamageCause, SimEvent, SoundCategory};
use crate::movement::{eye_position, half_height, radius, view_vector};
use crate::state::{Holding, MoveMode, MovablePhase, MovableState, PlayerId, PlayerState, WorldState};
use dishonored_ir::gameplay::GameplayConfig;
use dishonored_ir::world_actors::MovableDef;
use dishonored_world::{PawnBox, WorldQuery, channel};
use glam::Vec3;

/// The contact handler ignores a second contact within this time (0x00A40CF0 compares the time
/// since the last one against the constant 0.16666667).
pub const CONTACT_REFIRE: f32 = 1.0 / 6.0;

/// A prop without a mesh box still has a body.
const MIN_HALF: f32 = 2.0;

/// Add a placed movable to the world (once per actor).
pub fn spawn(state: &mut WorldState, def: &MovableDef) {
    if state.movables.iter().any(|m| m.name.eq_ignore_ascii_case(&def.name) && m.package.eq_ignore_ascii_case(&def.package)) {
        return;
    }
    let id = state.next_entity_id;
    state.next_entity_id = state.next_entity_id.wrapping_add(1);
    state.movables.push(MovableState {
        id,
        name: def.name.clone(),
        package: def.package.clone(),
        position: def.position,
        rotation: def.rotation,
        home: def.position,
        home_rotation: def.rotation,
        velocity: Vec3::ZERO,
        phase: MovablePhase::Rest,
        holder: None,
        instigator: None,
        dropped: false,
        grab_from: def.position,
        grab_from_rot: def.rotation,
        grab_time: 0.0,
        grabbing: false,
        since_contact: 1.0,
        released_by: None,
        gone: false,
        weight: def.weight.min(3),
        damage: def.damage,
        interactable: def.interactable,
        max_distance: def.max_distance,
        min_speed_to_cause_damage: def.min_speed_to_cause_damage,
        min_speed_to_be_damaged: def.min_speed_to_be_damaged,
        min_speed_to_be_damaged_on_drop: def.min_speed_to_be_damaged_on_drop,
        contact_type: def.contact_type.clone(),
        bounds_min: def.bounds_min,
        bounds_max: def.bounds_max,
        breakable: def.breakable.map(|b| b as u32),
        mesh: def.mesh.clone(),
        prompt: def.prompt.clone(),
    });
}

/// Remove the props of a level package (a Kismet-streamed sublevel was hidden), letting go of any
/// that are held.
pub fn remove_package(state: &mut WorldState, package: &str) {
    let ids: Vec<u32> = state.movables.iter().filter(|m| m.package.eq_ignore_ascii_case(package)).map(|m| m.id).collect();
    for p in state.players.iter_mut() {
        if p.holding.is_some_and(|h| ids.contains(&h.movable)) {
            p.holding = None;
        }
    }
    state.movables.retain(|m| !m.package.eq_ignore_ascii_case(package));
}

impl MovableState {
    /// Half extents of the mesh box (never below [`MIN_HALF`]).
    pub fn half(&self) -> Vec3 {
        ((self.bounds_max - self.bounds_min) * 0.5).max(Vec3::splat(MIN_HALF))
    }
    /// The box centre relative to the actor's pivot.
    pub fn center_offset(&self) -> Vec3 {
        (self.bounds_max + self.bounds_min) * 0.5
    }
    pub fn center(&self) -> Vec3 {
        self.position + self.center_offset()
    }
    /// The speed an impact must exceed to matter (0x00A40CF0): `m_fMinSpeedToBeDamaged`, or the
    /// on-drop one while `m_bDropped`; `m_fMinSpeedToCauseDamage` when it is the lower of the two
    /// positive ones.
    pub fn impact_threshold(&self) -> f32 {
        let (be, cause) = (self.min_speed_to_be_damaged, self.min_speed_to_cause_damage);
        if be <= cause || cause <= 1e-8 {
            if self.dropped { self.min_speed_to_be_damaged_on_drop } else { be }
        } else {
            cause
        }
    }
}

pub fn find(state: &WorldState, id: u32) -> Option<usize> {
    state.movables.iter().position(|m| m.id == id)
}

/// Where a held prop is: `eye + forward x D - up x H`, the camera's rotation (0x00C4C190).
pub fn hold_target(m: &MovableState, eye: Vec3, yaw: f32, pitch: f32, cfg: &GameplayConfig) -> (Vec3, Vec3) {
    let mv = &cfg.items.movable;
    let fwd = view_vector(yaw, pitch);
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    // The rotation matrix's Z axis (0x00402BC0) for roll 0.
    let up = Vec3::new(-cy * sp, -sy * sp, cp);
    let d = (m.bounds_min.x + m.bounds_max.x) - mv.hold_dist;
    let h = (m.bounds_max.z + m.bounds_min.z + 70.0) - mv.show_height[m.weight.min(3) as usize];
    // The location handed to SetLocation is the actor's pivot.
    (eye + fwd * d - up * h, Vec3::new(pitch, yaw, 0.0))
}

/// The world box of a prop: its mesh's local box turned by the actor's rotation (the component's
/// `Bounds`, an axis-aligned box about the pivot), at the pivot.
pub fn world_box(m: &MovableState) -> (Vec3, Vec3) {
    let (lo, hi) = dishonored_ir::items::rotated_box(m.bounds_min, m.bounds_max, Vec3::ONE, [m.rotation.x, m.rotation.y, m.rotation.z]);
    (m.position + lo, m.position + hi)
}

/// `StatePlayerGrabMovable` entered: the prop goes to the empty hands (0x00C3D030).
pub fn grab(state: &mut WorldState, who: usize, mi: usize, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let pid = state.players[who].id;
    let m = &mut state.movables[mi];
    m.phase = MovablePhase::Held;
    m.holder = Some(pid);
    m.released_by = None;
    m.instigator = Some(pid);
    m.velocity = Vec3::ZERO;
    m.grabbing = true;
    m.grab_time = 0.0;
    m.grab_from = m.position;
    m.grab_from_rot = m.rotation;
    m.dropped = false;
    let (id, weight) = (m.id, m.weight);
    state.players[who].holding = Some(Holding { movable: id, speed_mult: cfg.items.movable.max_pawn_speed[weight as usize] });
    events.push(SimEvent::MovablePickedUp { id: pid, movable: id });
}

/// Let go of the prop held by `who` (the release 0x00C45340): a trace from the pawn moves it back
/// out of geometry, it becomes a free rigid body, and a throw launches it along the view at
/// `Attribute_ThrowStrength`.
pub fn release(state: &mut WorldState, who: usize, thrown: bool, world: &dyn WorldQuery, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let p = state.players[who];
    let Some(h) = p.holding else { return };
    state.players[who].holding = None;
    let Some(mi) = find(state, h.movable) else { return };
    let eye = eye_position(&p, &cfg.movement);
    let strength = crate::powers::attribute(&p, cfg, "Attribute_ThrowStrength", cfg.items.inventory.throw_strength);
    let m = &mut state.movables[mi];
    let off = m.center_offset();
    let c = m.center();
    let to = c - eye;
    let d = to.length();
    if d > 1e-3 {
        let dir = to / d;
        if let Some(hit) = world.raycast(eye, dir, d, channel::NON_PAWN) {
            if hit.distance < d - 1.0 {
                let back = m.half().max_element().min(hit.distance);
                m.position = eye + dir * (hit.distance - back).max(0.0) - off;
            }
        }
    }
    m.phase = MovablePhase::Fly;
    m.holder = None;
    m.released_by = Some(p.id);
    m.instigator = Some(p.id);
    // `m_bDropped` (+0x384 bit 1): its setter was not found. By its name and the tweaks (a glass
    // bottle breaks above 1000, 4000 "on drop"; a throw leaves at `ThrowStrength` 2000) it is set
    // by a drop and not by a throw.
    m.dropped = !thrown;
    m.grabbing = false;
    m.velocity = if thrown { view_vector(p.yaw, p.pitch) * strength } else { Vec3::ZERO };
    m.since_contact = 1.0;
    events.push(SimEvent::MovableDropped { id: p.id, movable: m.id, thrown });
}

/// The holder's side each tick, before it moves: Use drops, Primary throws, and what takes the
/// hands away (death, a knock-out, a choke, possession, a body on the shoulder) drops.
///
/// Returns whether the hands were busy with the prop this tick (the press that let go of it does
/// nothing else: no sword swing, no second pick-up).
pub fn step_holder(state: &mut WorldState, who: usize, input: &crate::input::PlayerInput, world: &dyn WorldQuery, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) -> bool {
    let p = state.players[who];
    if p.holding.is_none() {
        return false;
    }
    let busy = !p.alive || p.incapacitated() || crate::powers::is_hidden(&p) || p.carrying.is_some() || p.choke.phase != crate::state::ChokePhase::None || p.condition != crate::state::Condition::Normal;
    if busy {
        release(state, who, false, world, cfg, events);
        true
    } else if p.just_pressed(input, crate::input::buttons::USE) {
        release(state, who, false, world, cfg, events);
        true
    } else if p.just_pressed(input, crate::input::buttons::PRIMARY) {
        release(state, who, true, world, cfg, events);
        true
    } else {
        false
    }
}

/// Whether `p` can take a prop in its hands now (the grab state's entry conditions are those of
/// lifting a body: on its feet, no action running, nothing in the other hand's way).
pub fn can_grab(p: &PlayerState) -> bool {
    p.alive && p.holding.is_none() && p.carrying.is_none() && p.mode == MoveMode::Walking && p.action_lock <= 0.0 && p.sword.phase == crate::state::SwordPhase::Idle
}

/// The props' tick, after every pawn moved: held props follow their holder, free ones fall and
/// strike. Pawns that took damage let go first.
pub fn step(state: &mut WorldState, world: &dyn WorldQuery, cfg: &GameplayConfig, dt: f32, events: &mut Vec<SimEvent>) {
    if state.movables.is_empty() {
        return;
    }
    // Damage takes the hands away (0x00AB59F0 calls slot 381 with 0).
    let hurt: Vec<PlayerId> = events.iter().filter_map(|e| if let SimEvent::Damage { victim, .. } = e { Some(*victim) } else { None }).collect();
    for pid in hurt {
        if let Some(who) = state.players.iter().position(|p| p.id == pid && p.holding.is_some()) {
            release(state, who, false, world, cfg, events);
        }
    }
    for mi in 0..state.movables.len() {
        state.movables[mi].since_contact += dt;
        match state.movables[mi].phase {
            MovablePhase::Held => step_held(state, mi, cfg, dt),
            MovablePhase::Fly => step_fly(state, mi, world, cfg, dt, events),
            MovablePhase::Rest => {}
        }
        // A released prop collides with the pawn that held it again once it is
        // `m_fRadiusChannelReset` away (0x00A465E0).
        let m = &state.movables[mi];
        if let Some(h) = m.released_by {
            let far = state.player(h).is_none_or(|p| p.position.distance(m.center()) > cfg.items.movable.radius_channel_reset);
            if far {
                state.movables[mi].released_by = None;
            }
        }
    }
}

fn step_held(state: &mut WorldState, mi: usize, cfg: &GameplayConfig, dt: f32) {
    let Some(holder) = state.movables[mi].holder.and_then(|h| state.player(h).copied()) else {
        // The holder is gone: the prop falls where it is.
        let m = &mut state.movables[mi];
        m.phase = MovablePhase::Fly;
        m.holder = None;
        m.dropped = true;
        return;
    };
    let eye = eye_position(&holder, &cfg.movement);
    let t_grab = cfg.items.movable.initial_grab_time;
    let m = &mut state.movables[mi];
    // `DishonoredItemEmpty` tick (0x00C4C440): the grab timer runs to `m_fInitialGrabTime`.
    if m.grabbing {
        m.grab_time += dt;
        if m.grab_time >= t_grab {
            m.grab_time = t_grab;
            m.grabbing = false;
        }
    }
    let (target, rot) = hold_target(m, eye, holder.yaw, holder.pitch, cfg);
    if m.grabbing {
        let f = if t_grab > 0.0 { (m.grab_time / t_grab).clamp(0.0, 1.0) } else { 1.0 };
        m.position = m.grab_from + (target - m.grab_from) * f;
        let d = |a: f32, b: f32| {
            let mut x = (b - a) % 360.0;
            if x > 180.0 {
                x -= 360.0;
            } else if x < -180.0 {
                x += 360.0;
            }
            a + x * f
        };
        m.rotation = Vec3::new(d(m.grab_from_rot.x, rot.x), d(m.grab_from_rot.y, rot.y), d(m.grab_from_rot.z, rot.z));
    } else {
        m.position = target;
        m.rotation = rot;
    }
    m.velocity = Vec3::ZERO;
}

fn step_fly(state: &mut WorldState, mi: usize, world: &dyn WorldQuery, cfg: &GameplayConfig, dt: f32, events: &mut Vec<SimEvent>) {
    let g = cfg.movement.gravity_z;
    let (half, off) = {
        let m = &state.movables[mi];
        (m.half(), m.center_offset())
    };
    let mut v = state.movables[mi].velocity;
    v.z += g * dt;
    let mut c0 = state.movables[mi].center();
    // A prop let go inside geometry (held against a table edge) is pushed out first, as PhysX
    // depenetrates a body.
    for _ in 0..3 {
        let mut cs = Vec::new();
        world.box_contacts(&PawnBox { center: c0, half }, channel::NON_PAWN, &mut cs);
        let Some(k) = cs.iter().filter(|k| k.depth > 0.05).max_by(|a, b| a.depth.total_cmp(&b.depth)) else { break };
        c0 += k.normal * (k.depth + 0.01);
    }
    let delta = v * dt;
    let boxx = PawnBox { center: c0, half };
    let mut c = c0 + delta;
    let mut contact: Option<(Vec3, Vec3, f32)> = None; // point, normal, speed along the normal
    let mut rest = false;
    if let Some(h) = world.sweep_box(&boxx, delta, channel::NON_PAWN) {
        if h.start_penetrating && h.fraction <= 0.0 {
            // Inside geometry (the mesh's own instance): stop where it is.
            c = c0;
            v = Vec3::ZERO;
            rest = true;
        } else {
            c = c0 + delta * h.fraction.clamp(0.0, 1.0);
            let n = h.normal.normalize_or_zero();
            let into = -v.dot(n);
            if into > 0.0 {
                contact = Some((h.point, n, into));
                v += n * into;
            }
            if n.z > 0.5 {
                rest = true;
            }
        }
    }
    // Pawns in the way.
    let mut victim: Option<usize> = None;
    let (rxy, by) = {
        let m = &state.movables[mi];
        (half.x.max(half.y), m.instigator)
    };
    for (pi, p) in state.players.iter().enumerate() {
        if !p.alive || p.carried_by.is_some() || state.movables[mi].released_by == Some(p.id) || p.condition == crate::state::Condition::Unconscious {
            continue;
        }
        let dh = (c.truncate() - p.position.truncate()).length();
        if dh < radius(p, &cfg.movement) + rxy && (c.z - p.position.z).abs() < half_height(p, &cfg.movement) + half.z {
            victim = Some(pi);
            break;
        }
    }
    // Out of the world: it stays where it fell.
    if c.z < crate::movement::KILL_Z {
        rest = true;
        v = Vec3::ZERO;
    }
    let mut hit_point = contact.map(|c| c.0).unwrap_or(c);
    let mut speed = contact.map_or(0.0, |c| c.2);
    if let Some(pi) = victim {
        let rel = (v - state.players[pi].velocity).length();
        speed = speed.max(rel);
        hit_point = c;
        // The prop stops on the pawn and drops.
        v.x = 0.0;
        v.y = 0.0;
    }
    {
        let m = &mut state.movables[mi];
        m.position = c - off;
        m.velocity = if rest { Vec3::ZERO } else { v };
        if rest {
            m.phase = MovablePhase::Rest;
            m.released_by = None;
        }
    }
    if contact.is_some() || victim.is_some() {
        impact(state, mi, speed, hit_point, victim, by, cfg, events);
    }
}

/// A contact at `speed` along the normal. Two things come of it:
/// - the contact system's AI noise (the prop's contact type against the world): from
///   `DishonoredKActor.m_ContactPhysicsInfo`, above `m_fMinImpactSpeed` and at most every
///   `m_fRefireDelay` (class defaults 50 and 0.3; LIKELY, the notification's reader is not
///   decoded), made by the pawn that last held it;
/// - the breakable's collision (0x00A40CF0): nothing at or below [`MovableState::impact_threshold`]
///   (or with a negative `m_Damage`, or within 1/6 s of the last one); above it the prop deals
///   `m_Damage` to the pawn it hit when it is faster than `m_fMinSpeedToCauseDamage` and is not
///   that pawn's own (LIKELY: the amount is taken from the call 0x00A40CF0 makes on the struck
///   pawn; its height test is not decoded), and above `m_fMinSpeedToBeDamaged` it deals the same
///   to itself and wears (`m_Damage -= amount`). `m_bDropped` ends at the first impact above
///   `m_fMinSpeedToCauseDamage`.
#[allow(clippy::too_many_arguments)]
fn impact(state: &mut WorldState, mi: usize, speed: f32, point: Vec3, victim: Option<usize>, by: Option<PlayerId>, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let mc = &cfg.items.movable;
    let noise_ok = speed >= mc.min_impact_speed && state.movables[mi].since_contact >= mc.impact_refire;
    if noise_ok {
        let n = cfg.ai.noise.prop_contact(&state.movables[mi].contact_type);
        crate::noise::emit(events, &n, point, SoundCategory::Thrown, by.unwrap_or(0), speed);
    }
    let m = &mut state.movables[mi];
    let first_contact = m.since_contact >= CONTACT_REFIRE;
    if noise_ok {
        m.since_contact = 0.0;
    }
    if m.damage < 0 || !first_contact {
        return;
    }
    let threshold = m.impact_threshold();
    if threshold <= 0.0 || speed <= threshold {
        return;
    }
    let fast = speed > m.min_speed_to_cause_damage;
    let dmg = m.damage;
    let victim_id = victim.map(|v| state.players[v].id);
    let m = &mut state.movables[mi];
    let hurts_victim = fast && dmg > 0 && victim_id.is_some() && victim_id != m.released_by;
    let hurts_self = speed > m.min_speed_to_be_damaged && dmg > 0;
    let breakable = if hurts_self { m.breakable } else { None };
    if hurts_self {
        m.damage -= dmg;
    }
    if fast {
        m.dropped = false;
    }
    let id = m.id;
    if hurts_victim {
        if let Some(vi) = victim {
            crate::vitals::apply_damage(&mut state.players[vi], dmg as f32, by, DamageCause::ThrownObject, events, &cfg.vitals);
        }
    }
    if hurts_victim || hurts_self {
        events.push(SimEvent::MovableImpact { movable: id, speed, point, by, victim: victim_id.filter(|_| hurts_victim), damage: dmg, breakable });
    }
}

/// The breakable of a prop reached its last step (the mission layer saw `WorldEvent::Broken`):
/// the prop is gone, and whoever holds it lets go.
pub fn on_broken(state: &mut WorldState, breakable: u32) {
    for mi in 0..state.movables.len() {
        if state.movables[mi].breakable != Some(breakable) {
            continue;
        }
        let id = state.movables[mi].id;
        state.movables[mi].gone = true;
        state.movables[mi].phase = MovablePhase::Rest;
        state.movables[mi].holder = None;
        state.movables[mi].velocity = Vec3::ZERO;
        for p in state.players.iter_mut() {
            if p.holding.is_some_and(|h| h.movable == id) {
                p.holding = None;
            }
        }
    }
}

/// The prop a pawn holds, if any.
pub fn held_by(state: &WorldState, pid: PlayerId) -> Option<&MovableState> {
    let h = state.player(pid)?.holding?;
    state.movables.iter().find(|m| m.id == h.movable)
}
