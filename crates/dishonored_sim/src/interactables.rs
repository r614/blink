//! Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission. The hold-to-peek flow (Use held at a shut keyhole door puts the eye to it; Use leaves) follows game/src/keyhole.rs; its hold time (0.4), look cone, eye offset and the player-only wire trip were replaced by the decoded rules below.
//!
//! Door keyholes and tripwires (research/dishonored/systems/interactables.md), decoded from
//! `Dishonored.exe` with the static index.
//!
//! # Keyholes (`StatePlayerMasterHolePeeking`, the door's `AltInteract`)
//!
//! - **Entry.** Use on a focused interactable is a tap or a hold: the `DisUseState_AltInteract`
//!   state (0x00AD57E0) calls the interactable's `AltInteract` (interface slot 25) once the key
//!   has been held for the state's hold time, `Interact` (slot 24, 0x00A3DF60) when it was
//!   released earlier. For a `DisDoor`, slot 25 is 0x00A38580: the user must be the player pawn
//!   (the object-class byte at pawn +0x10A is `$`), the door's keyhole socket
//!   (`m_KeyHoleSocketName`, default `KeyHole`) must exist (0x00A1EF10), and the pawn's view is set
//!   to the socket's rotation, turned 180 degrees when the socket's forward points against the
//!   pawn's own (dot < 0). It then fires `DisSeqEvent_KeyHoleUsed` output 0 (originator the
//!   door, instigator the pawn, exact class). The hold time is
//!   `DisTweaks_PlayerInput.m_fAltInteractActivateTime` (0.3). The door tweak's own
//!   `m_fHolePeekingActivateTime` has the same default; no reader of it was found.
//! - **The state** (vtable 0x010FDB08). Begin 0x00A72D50: the door is the state's parameter;
//!   input masks (all actions off, then the set 0x508D2 on, `Use` included); with a fade
//!   (`m_bFadeIn`) the screen fades to black over 0.2 s and `m_State` = `DPS_FADE_IN`, else
//!   0x00A63400 runs at once. Tick 0x00A75BD0: `DPS_FADE_IN` counts up and at 0.25 s sets the
//!   camera flag, at 0.3 s runs 0x00A63400 and becomes `DPS_IDLE`; `DPS_FADE_OUT` counts up and at
//!   0.3 s the state ends (back to the walking state). 0x00A63400 (the peek starts): black fade
//!   0.2 s, and the field of view becomes `m_fKeyHoleFOV * (1 - Attribute_KeyholePeepingMagnification)`
//!   over 0.2 s (`FUN_00ACA500(2, fov - magnification * fov, 0.2, 0)`).
//!   Leaving (0x00A679D0) restores the HUD and the field of view; `Dis_ExitKeyhole` (0x009EE510,
//!   controller slot 339) requests the exit, and the door's exit method (interface slot 26,
//!   0x00A2E4E0) fires `KeyHoleUsed` output 1 for the pawn.
//!
//! # Tripwires (`DisTripwire`)
//!
//! - **Trip** (0x00A28B40, once): `m_TripwireStatus` `Armed` -> `Tripped`; plays the tweak's
//!   `m_TrippedAnimName`; `FireGeneratedEvents(tripwire, DisSeqEvent_Tripwire, originator the
//!   tripwire, instigator the tripper, exact 0, all outputs)` (0x00A21A30) and a telemetry
//!   record (event 0x2E; no gameplay). The status is saved (0x00A19130).
//! - **Touch** (0x00A30C00): any live actor touching the wire's physics asset trips it unless the
//!   actor is a `DisGameCrowdAgentSkeletalRat` or a `DisRatSwarm` (0x00C70090, 0x00A1A8C0 test
//!   the classes; the two game-info-gated tests ignore a `DisTrigger` / `Trigger_LOS` in the
//!   DLC game infos only). The collision is `COLLIDE_TouchAll` on the skeletal mesh component.
//! - **Damage** (0x00A28BB0, from `TakeDamage_Native`): trips it (the damage causer the
//!   instigator) unless the damage type has `m_bDontTriggerTripWire` (only `DisDamageType_WindBlast`).
//!
//! # Interact
//!
//! `DisSeqEvent_Interact` fires from the Use handler (0x00AD5BA0) through 0x00A3DF60 (output 0)
//! after the interactable's slot 24 returned true, and from 0x00A3E0B0 (output 1, `AltInteract`)
//! after slot 25 did: `FireGeneratedEvents(user, class, originator the user, instigator the
//! interactable, exact 1, [output])`, so it reaches the events registered with the *user*
//! (`SequenceEvent::RegisterEvent`, 0x006E50B0, registers an event only with its `Originator`).
//! `m_pAssociatedTweak` is not a filter: the event's activation (slot 102, 0x00B8E830) stores the
//! interactable's tweak there for the event's variable output.
//!
//! The rules are pure functions of the state and the step's inputs: the same inputs give the same
//! events, in tripwire then player order.

use crate::events::SimEvent;
use crate::input::{PlayerInput, buttons};
use crate::state::{PawnKind, PlayerId, PlayerState, ProjectileState};
use dishonored_ir::interactables::Interactables;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// `StatePlayerMasterHolePeeking` tick constants (0x00A75BD0): the camera flag is set after
/// 0.25 s of the fade, the fade ends (and the exit completes) after 0.3 s.
pub const FADE_FLAG_TIME: f32 = 0.25;
pub const FADE_TIME: f32 = 0.3;
/// A projectile's collision radius against the wire's bodies (uu). The shipped projectile
/// tweaks' collision extents are not read: left at the point-like 1 uu (Unmeasured).
pub const PROJECTILE_RADIUS: f32 = 1.0;
/// The camera fade and field-of-view blend durations of 0x00A63400 / 0x00A72D50.
pub const BLEND_TIME: f32 = 0.2;

/// What the step reports; the Kismet host fires the events (`kismet_host::interactables`).
#[derive(Debug, Clone, PartialEq)]
pub enum InteractEvent {
    /// A tripwire tripped (index into `Interactables::tripwires`), by `instigator`.
    Tripwire { tripwire: usize, instigator: PlayerId },
    /// `DisSeqEvent_KeyHoleUsed` of a door (index into `Interactables::keyholes`): output 0 when
    /// the pawn put its eye to the keyhole, output 1 when it left.
    KeyHole { keyhole: usize, player: PlayerId, used: bool },
    /// `DisSeqEvent_Interact` output 1 (`AltInteract`) of the interactable the user held Use on.
    AltInteract { keyhole: usize, player: PlayerId },
    /// `DisSeqEvent_Interact` output 0 of a launcher the player disarmed or took ammunition from
    /// (index into `Interactables::launchers`; `launchers::LaunchersState::interact` returned
    /// "used").
    Launcher { launcher: usize, player: PlayerId },
}

/// `eDisPeekingState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeekPhase {
    FadeIn,
    Idle,
    FadeOut,
}

/// A pawn at a keyhole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Peek {
    pub keyhole: usize,
    pub phase: PeekPhase,
    /// `m_fFadeTime`: seconds in the current fade phase.
    pub time: f32,
    /// Seconds since the peek started (the camera blend clock).
    pub age: f32,
    /// The view direction through the door, as 0x00A38580 sets it on entry (degrees).
    pub yaw: f32,
    pub pitch: f32,
    /// The entry view has been handed to the pawn.
    pub view_set: bool,
}

/// How long a pawn has held Use on a keyhole door (`DisUseState_AltInteract`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Hold {
    pub keyhole: usize,
    pub time: f32,
}

/// The view a peeking player sees (presentation).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PeekView {
    pub keyhole: usize,
    /// The eye: the keyhole socket in world space.
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// Horizontal field of view in degrees: `m_fKeyHoleFOV * (1 - magnification)`, blended in
    /// from `from_fov` over [`BLEND_TIME`].
    pub fov: f32,
    /// Black over the screen, 0..1 (the fade of the state's begin and end).
    pub fade: f32,
    pub exiting: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InteractablesState {
    /// `m_TripwireStatus == ETS_Tripped`, one per tripwire.
    pub tripped: Vec<bool>,
    pub holds: BTreeMap<PlayerId, Hold>,
    pub peeks: BTreeMap<PlayerId, Peek>,
    /// Players whose Use was down at the end of the last step (the press edge).
    pub use_down: BTreeSet<PlayerId>,
}

/// Distance from `p` to the triangle `a b c` (Ericson, closest point on a triangle).
fn point_triangle_distance(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return ap.length();
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return bp.length();
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (p - (a + ab * v)).length();
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return cp.length();
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (p - (a + ac * w)).length();
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (p - (b + (c - b) * w)).length();
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (p - (a + ab * v + ac * w)).length()
}

/// Whether a vertical capsule (axis from `center - half` to `center + half` on z, `radius`)
/// overlaps the wire's bodies. The axis is sampled at most every half radius: the wire is thin
/// and the capsule wide, so the sampled distance bounds the true one to within a hair.
pub fn capsule_touches(triangles: &[[Vec3; 3]], center: Vec3, radius: f32, half_height: f32) -> bool {
    let steps = ((2.0 * half_height) / (0.5 * radius.max(1.0))).ceil().max(1.0) as usize;
    for t in triangles {
        for s in 0..=steps {
            let p = center + Vec3::Z * (-half_height + 2.0 * half_height * s as f32 / steps as f32);
            if point_triangle_distance(p, t[0], t[1], t[2]) <= radius {
                return true;
            }
        }
    }
    false
}

/// Whether the segment `a`..`b` (a projectile's step) passes within `radius` of the wire.
pub fn segment_touches(triangles: &[[Vec3; 3]], a: Vec3, b: Vec3, radius: f32) -> bool {
    let len = (b - a).length();
    let steps = (len / radius.max(1.0)).ceil().max(1.0) as usize;
    triangles.iter().any(|t| (0..=steps).any(|s| point_triangle_distance(a.lerp(b, s as f32 / steps as f32), t[0], t[1], t[2]) <= radius))
}

/// A ray against an axis-aligned box: the distance along `dir` (unit) to the entry, if within
/// `reach`.
pub fn ray_box(origin: Vec3, dir: Vec3, lo: Vec3, hi: Vec3, reach: f32) -> Option<f32> {
    let mut t0 = 0.0f32;
    let mut t1 = reach;
    for i in 0..3 {
        if dir[i].abs() < 1e-8 {
            if origin[i] < lo[i] || origin[i] > hi[i] {
                return None;
            }
        } else {
            let (a, b) = ((lo[i] - origin[i]) / dir[i], (hi[i] - origin[i]) / dir[i]);
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
            if t0 > t1 {
                return None;
            }
        }
    }
    Some(t0)
}

impl InteractablesState {
    pub fn new(defs: &Interactables) -> Self {
        InteractablesState { tripped: vec![false; defs.tripwires.len()], ..Default::default() }
    }

    /// Whether a tripwire has tripped.
    pub fn is_tripped(&self, i: usize) -> bool {
        self.tripped.get(i).copied().unwrap_or(false)
    }

    /// The pawn is at a keyhole (its movement and buttons are taken, [`Self::lock_input`]).
    pub fn is_peeking(&self, id: PlayerId) -> bool {
        self.peeks.contains_key(&id)
    }

    /// `FireTrip` of 0x00A28B40: once.
    fn trip(&mut self, i: usize, instigator: PlayerId, out: &mut Vec<InteractEvent>) {
        if self.tripped.get(i).copied() == Some(false) {
            self.tripped[i] = true;
            out.push(InteractEvent::Tripwire { tripwire: i, instigator });
        }
    }

    /// The tripwires touched or damaged this step (0x00A30C00 touch, 0x00A28BB0 damage).
    ///
    /// Touch: a live pawn whose cylinder overlaps the wire's bodies, rats excepted; a projectile
    /// whose step crosses them. Damage: an explosion that deals damage at the wire's nearest
    /// point (`damage_at`), of a damage type that does not carry `m_bDontTriggerTripWire`.
    /// `projectiles` are the step's projectiles after the step (their previous point is one
    /// step back along the velocity); `explosions` are the step's [`SimEvent::Explosion`]s with
    /// `damage_at(kind sticky, distance) > 0` deciding the damage.
    pub fn step_tripwires(
        &mut self,
        defs: &Interactables,
        players: &[PlayerState],
        projectiles: &[ProjectileState],
        events: &[SimEvent],
        active: impl Fn(usize) -> bool,
        collision: impl Fn(&PlayerState) -> (f32, f32),
        damage_at: impl Fn(bool, f32) -> (f32, String),
        dt: f32,
    ) -> Vec<InteractEvent> {
        let mut out = Vec::new();
        if self.tripped.len() != defs.tripwires.len() {
            self.tripped = vec![false; defs.tripwires.len()];
        }
        for (i, w) in defs.tripwires.iter().enumerate() {
            if self.tripped[i] || w.triangles.is_empty() || !active(i) {
                continue;
            }
            let touch = players.iter().filter(|p| p.alive && p.kind != PawnKind::Rat && p.carried_by.is_none()).find(|p| {
                let (r, h) = collision(p);
                capsule_touches(&w.triangles, p.position, r, h)
            });
            if let Some(p) = touch {
                self.trip(i, p.id, &mut out);
                continue;
            }
            let hit = projectiles.iter().find(|pr| segment_touches(&w.triangles, pr.position - pr.velocity * dt, pr.position, PROJECTILE_RADIUS));
            if let Some(pr) = hit {
                self.trip(i, pr.owner, &mut out);
                continue;
            }
            for e in events {
                let SimEvent::Explosion { owner, position, sticky } = e else { continue };
                let nearest = w.triangles.iter().map(|t| point_triangle_distance(*position, t[0], t[1], t[2])).fold(f32::INFINITY, f32::min);
                let (damage, kind) = damage_at(*sticky, nearest);
                if damage > 0.0 && !defs.no_trip_damage_types.iter().any(|t| t.eq_ignore_ascii_case(&kind)) {
                    self.trip(i, *owner, &mut out);
                    break;
                }
            }
        }
        out
    }

    /// The keyhole door the player's view meets within `reach` (the door's bounds, the way the
    /// interactable focus traces it), among the closed doors.
    pub fn focused_keyhole(defs: &Interactables, eye: Vec3, dir: Vec3, reach: f32, closed: impl Fn(usize) -> bool) -> Option<usize> {
        defs.keyholes
            .iter()
            .enumerate()
            .filter(|(i, k)| closed(*i) && k.bounds_min.cmplt(k.bounds_max).all())
            .filter_map(|(i, k)| ray_box(eye, dir, k.bounds_min, k.bounds_max, reach).map(|t| (i, t)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.0)
    }

    /// Hold tracking and the peek state machine for one step. `players` are the pawns after the
    /// step; `inputs` this step's inputs (as the world stepped them); `closed(i)` says whether the
    /// door of keyhole `i` is closed; `eye` is the pawn's eye position; `reach` the interact
    /// distance.
    pub fn step_peeks(&mut self, defs: &Interactables, players: &[PlayerState], inputs: &[(PlayerId, PlayerInput)], closed: impl Fn(usize) -> bool, eye: impl Fn(&PlayerState) -> Vec3, reach: f32, dt: f32) -> Vec<InteractEvent> {
        let mut out = Vec::new();
        for (id, inp) in inputs {
            let Some(p) = players.iter().find(|p| p.id == *id) else { continue };
            let held = inp.pressed(buttons::USE);
            let was = self.use_down.contains(id);
            let pressed = held && !was;
            if held {
                self.use_down.insert(*id);
            } else {
                self.use_down.remove(id);
            }
            if let Some(peek) = self.peeks.get_mut(id) {
                peek.age += dt;
                match peek.phase {
                    PeekPhase::FadeIn => {
                        peek.time += dt;
                        if peek.time > FADE_TIME {
                            peek.phase = PeekPhase::Idle;
                            peek.time = 0.0;
                        }
                    }
                    PeekPhase::FadeOut | PeekPhase::Idle => peek.time += dt,
                }
                // `Dis_ExitKeyhole`: Use leaves; a dead pawn or a door that is no longer shut
                // ends it too.
                if peek.phase != PeekPhase::FadeOut && (pressed || !p.alive || !closed(peek.keyhole)) {
                    peek.phase = PeekPhase::FadeOut;
                    peek.time = 0.0;
                    out.push(InteractEvent::KeyHole { keyhole: peek.keyhole, player: *id, used: false });
                }
                if peek.phase == PeekPhase::FadeOut && peek.time > FADE_TIME {
                    self.peeks.remove(id);
                }
                continue;
            }
            // The player pawn only (0x00A38580 checks the pawn's class byte), on foot.
            if p.kind != PawnKind::Human || !p.alive || p.incapacitated() || p.carrying.is_some() {
                self.holds.remove(id);
                continue;
            }
            if !held {
                self.holds.remove(id);
                continue;
            }
            let dir = crate::movement::view_vector(p.yaw, p.pitch);
            let focus = Self::focused_keyhole(defs, eye(p), dir, reach, &closed);
            let Some(k) = focus else {
                self.holds.remove(id);
                continue;
            };
            // A hold begins on the press that met the door and goes on while it stays in view.
            let hold = match self.holds.get_mut(id) {
                Some(h) if h.keyhole == k => {
                    h.time += dt;
                    *h
                }
                _ if pressed => {
                    let h = Hold { keyhole: k, time: dt };
                    self.holds.insert(*id, h);
                    h
                }
                _ => {
                    self.holds.remove(id);
                    continue;
                }
            };
            let def = &defs.keyholes[k];
            if hold.time < defs.alt_interact_time {
                continue;
            }
            self.holds.remove(id);
            // 0x00A38580: the view through the keyhole, turned about when the socket faces
            // against the pawn's own view (dot < 0).
            let mut f = def.facing;
            if f.dot(dir) < 0.0 {
                f = -f;
            }
            let yaw = f.y.atan2(f.x).to_degrees();
            let pitch = f.z.clamp(-1.0, 1.0).asin().to_degrees();
            self.peeks.insert(*id, Peek { keyhole: k, phase: PeekPhase::FadeIn, time: 0.0, age: 0.0, yaw, pitch, view_set: false });
            out.push(InteractEvent::KeyHole { keyhole: k, player: *id, used: true });
            out.push(InteractEvent::AltInteract { keyhole: k, player: *id });
        }
        out
    }

    /// The peeking players' inputs before the world steps them: movement and buttons are taken
    /// (the state switches every action off but Use, which the exit reads), and the first step
    /// hands the pawn the view through the keyhole (0x00A38580).
    pub fn lock_input(&mut self, id: PlayerId, input: &mut PlayerInput) {
        let Some(peek) = self.peeks.get_mut(&id) else { return };
        input.forward = 0;
        input.right = 0;
        input.buttons &= buttons::USE;
        input.action = 0;
        if !peek.view_set {
            peek.view_set = true;
            input.yaw = peek.yaw;
            input.pitch = peek.pitch;
        }
    }

    /// What the peeking player sees. `magnification` is the player's
    /// `Attribute_KeyholePeepingMagnification`.
    pub fn view(&self, defs: &Interactables, id: PlayerId, p: &PlayerState, magnification: f32) -> Option<PeekView> {
        let peek = self.peeks.get(&id)?;
        let def = defs.keyholes.get(peek.keyhole)?;
        let blend = |t: f32| (t / BLEND_TIME).clamp(0.0, 1.0);
        let (fade, exiting) = match peek.phase {
            // To black over the first 0.2 s, held until the view switches at 0.3 s, then clear.
            PeekPhase::FadeIn => (blend(peek.time), false),
            PeekPhase::Idle => (1.0 - blend(peek.time), false),
            PeekPhase::FadeOut => (blend(peek.time), true),
        };
        let fov = def.fov - magnification * def.fov;
        Some(PeekView { keyhole: peek.keyhole, eye: def.socket, yaw: p.yaw, pitch: p.pitch, fov, fade, exiting })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::interactables::{KeyholeDef, TripwireDef};

    /// A thin wire along x at height 50, 6 uu wide (two triangles of a flat strip).
    fn wire() -> TripwireDef {
        let (a, b) = (Vec3::new(-100.0, 0.0, 50.0), Vec3::new(100.0, 0.0, 50.0));
        let w = Vec3::new(0.0, 3.0, 0.0);
        TripwireDef { name: "W".into(), triangles: vec![[a - w, b - w, b + w], [a - w, b + w, a + w]], ..Default::default() }
    }

    fn defs() -> Interactables {
        Interactables { tripwires: vec![wire()], no_trip_damage_types: vec!["DisDamageType_WindBlast".into()], ..Default::default() }
    }

    fn pawn(id: PlayerId, at: Vec3) -> PlayerState {
        let mut p = PlayerState::new(id, at, 0.0);
        p.alive = true;
        p
    }

    fn cyl(_: &PlayerState) -> (f32, f32) {
        (34.0, 90.0)
    }

    fn no_blast(_: bool, _: f32) -> (f32, String) {
        (0.0, String::new())
    }

    #[test]
    fn a_pawn_walking_into_the_wire_trips_it_once_with_the_tripper() {
        let d = defs();
        let mut s = InteractablesState::new(&d);
        let far = pawn(1, Vec3::new(0.0, 300.0, 100.0));
        assert!(s.step_tripwires(&d, &[far], &[], &[], |_| true, cyl, no_blast, 1.0 / 60.0).is_empty());
        let near = pawn(1, Vec3::new(0.0, 20.0, 100.0));
        let ev = s.step_tripwires(&d, &[near.clone()], &[], &[], |_| true, cyl, no_blast, 1.0 / 60.0);
        assert_eq!(ev, vec![InteractEvent::Tripwire { tripwire: 0, instigator: 1 }]);
        assert!(s.is_tripped(0));
        assert!(s.step_tripwires(&d, &[near.clone()], &[], &[], |_| true, cyl, no_blast, 1.0 / 60.0).is_empty(), "0x00A28B40 trips only from Armed");
        let mut hidden = InteractablesState::new(&d);
        assert!(hidden.step_tripwires(&d, &[near], &[], &[], |_| false, cyl, no_blast, 1.0 / 60.0).is_empty(), "a wire in a sublevel that is not in the world is not touched");
    }

    #[test]
    fn a_pawn_above_the_wire_does_not_trip_it_and_rats_never_do() {
        let d = defs();
        let mut s = InteractablesState::new(&d);
        // The capsule's feet are above the wire: a jump over it.
        let over = pawn(1, Vec3::new(0.0, 0.0, 50.0 + 90.0 + 40.0));
        assert!(s.step_tripwires(&d, &[over], &[], &[], |_| true, cyl, no_blast, 1.0 / 60.0).is_empty());
        let mut rat = pawn(2, Vec3::new(0.0, 0.0, 50.0));
        rat.kind = PawnKind::Rat;
        assert!(s.step_tripwires(&d, &[rat], &[], &[], |_| true, cyl, no_blast, 1.0 / 60.0).is_empty(), "0x00C70090 / 0x00A1A8C0 exempt rats");
        let mut dead = pawn(3, Vec3::new(0.0, 0.0, 50.0));
        dead.alive = false;
        assert!(s.step_tripwires(&d, &[dead], &[], &[], |_| true, cyl, no_blast, 1.0 / 60.0).is_empty());
    }

    #[test]
    fn lower_player_id_trips_first_when_two_touch_in_one_step() {
        let d = defs();
        let mut s = InteractablesState::new(&d);
        let a = pawn(5, Vec3::new(0.0, 0.0, 60.0));
        let b = pawn(2, Vec3::new(10.0, 0.0, 60.0));
        let ev = s.step_tripwires(&d, &[a, b], &[], &[], |_| true, cyl, no_blast, 1.0 / 60.0);
        assert_eq!(ev, vec![InteractEvent::Tripwire { tripwire: 0, instigator: 5 }], "players in slot order: the first in the list");
    }

    #[test]
    fn an_explosion_that_deals_damage_trips_it_but_windblast_damage_does_not() {
        let d = defs();
        let blast = [SimEvent::Explosion { owner: 7, position: Vec3::new(0.0, 0.0, 200.0), sticky: false }];
        let mut s = InteractablesState::new(&d);
        let ev = s.step_tripwires(&d, &[], &[], &blast, |_| true, cyl, |_, dist| (if dist < 300.0 { 100.0 } else { 0.0 }, "DisDamageType_Grenade".into()), 1.0 / 60.0);
        assert_eq!(ev, vec![InteractEvent::Tripwire { tripwire: 0, instigator: 7 }]);
        let mut s = InteractablesState::new(&d);
        assert!(s.step_tripwires(&d, &[], &[], &blast, |_| true, cyl, |_, _| (100.0, "DisDamageType_WindBlast".into()), 1.0 / 60.0).is_empty(), "m_bDontTriggerTripWire");
        let mut s = InteractablesState::new(&d);
        assert!(s.step_tripwires(&d, &[], &[], &blast, |_| true, cyl, |_, dist| (if dist < 50.0 { 100.0 } else { 0.0 }, "DisDamageType_Grenade".into()), 1.0 / 60.0).is_empty(), "out of the blast's reach");
    }

    #[test]
    fn a_projectile_crossing_the_wire_trips_it_for_its_owner() {
        let d = defs();
        let mut s = InteractablesState::new(&d);
        let dt = 1.0 / 60.0;
        let pos = Vec3::new(0.0, 1.0, 50.0);
        let pr = ProjectileState { id: 1, kind: crate::state::ProjectileKind::Bolt, owner: 4, position: pos, velocity: Vec3::new(0.0, 600.0, 0.0), age: 0.0, damage: 0.0, gravity_mult: 0.0, armed: false, fuse: 0.0, min_fuse: 0.0, stuck_to: None, stuck: false, stuck_offset: Vec3::ZERO, spawn: pos, pierced: None, homing: None, source_cleared: false, npc_throw: false };
        let ev = s.step_tripwires(&d, &[], &[pr], &[], |_| true, cyl, no_blast, dt);
        assert_eq!(ev, vec![InteractEvent::Tripwire { tripwire: 0, instigator: 4 }]);
    }

    fn door() -> KeyholeDef {
        KeyholeDef {
            name: "DisDoor_0".into(),
            socket: Vec3::new(100.0, 0.0, 60.0),
            facing: Vec3::X,
            bounds_min: Vec3::new(95.0, -50.0, 0.0),
            bounds_max: Vec3::new(105.0, 50.0, 200.0),
            fov: 75.0,
            ..Default::default()
        }
    }

    fn input(use_down: bool) -> (PlayerId, PlayerInput) {
        (1, PlayerInput { buttons: if use_down { buttons::USE } else { 0 }, ..Default::default() })
    }

    fn eye(p: &PlayerState) -> Vec3 {
        p.position + Vec3::Z * 60.0
    }

    #[test]
    fn use_held_for_the_door_hold_time_puts_the_eye_to_the_keyhole() {
        let d = Interactables { keyholes: vec![door()], alt_interact_time: 0.3, ..Default::default() };
        let mut s = InteractablesState::new(&d);
        let dt = 1.0 / 60.0;
        let p = pawn(1, Vec3::new(0.0, 0.0, 0.0));
        let mut events = Vec::new();
        for _ in 0..17 {
            events.extend(s.step_peeks(&d, &[p.clone()], &[input(true)], |_| true, eye, 200.0, dt));
        }
        assert!(events.is_empty(), "17 ticks = 0.283 s < m_fAltInteractActivateTime 0.3");
        assert!(!s.is_peeking(1));
        events.extend(s.step_peeks(&d, &[p.clone()], &[input(true)], |_| true, eye, 200.0, dt));
        assert_eq!(events, vec![InteractEvent::KeyHole { keyhole: 0, player: 1, used: true }, InteractEvent::AltInteract { keyhole: 0, player: 1 }]);
        assert!(s.is_peeking(1));
        // The entry view looks through the door (the pawn already looks along +x).
        let peek = s.peeks[&1].clone();
        assert!(peek.yaw.abs() < 1e-3 && peek.pitch.abs() < 1e-3);
    }

    #[test]
    fn a_tap_or_a_door_out_of_view_never_peeks() {
        let d = Interactables { keyholes: vec![door()], alt_interact_time: 0.3, ..Default::default() };
        let mut s = InteractablesState::new(&d);
        let dt = 1.0 / 60.0;
        let p = pawn(1, Vec3::ZERO);
        for _ in 0..5 {
            s.step_peeks(&d, &[p.clone()], &[input(true)], |_| true, eye, 200.0, dt);
        }
        s.step_peeks(&d, &[p.clone()], &[input(false)], |_| true, eye, 200.0, dt);
        assert!(s.holds.is_empty(), "releasing before the hold time is a tap");
        let mut away = p.clone();
        away.yaw = 180.0;
        for _ in 0..30 {
            assert!(s.step_peeks(&d, &[away.clone()], &[input(true)], |_| true, eye, 200.0, dt).is_empty());
        }
        let mut far = p.clone();
        far.position = Vec3::new(-400.0, 0.0, 0.0);
        for _ in 0..30 {
            assert!(s.step_peeks(&d, &[far.clone()], &[input(true)], |_| true, eye, 200.0, dt).is_empty(), "beyond the interact distance");
        }
        // An open door has no peek.
        for _ in 0..30 {
            assert!(s.step_peeks(&d, &[p.clone()], &[input(true)], |_| false, eye, 200.0, dt).is_empty());
        }
    }

    #[test]
    fn the_view_turns_against_a_pawn_looking_the_other_way_and_the_fov_follows_the_attribute() {
        let d = Interactables { keyholes: vec![door()], alt_interact_time: 0.3, ..Default::default() };
        let mut s = InteractablesState::new(&d);
        let dt = 1.0 / 60.0;
        // On the far side of the door, looking back along -x at it: the socket faces +x, which
        // points against the pawn's view (dot < 0), so the view is turned about (-x).
        let mut p = pawn(1, Vec3::new(200.0, 0.0, 0.0));
        p.yaw = 180.0;
        for _ in 0..20 {
            s.step_peeks(&d, &[p.clone()], &[input(true)], |_| true, eye, 200.0, dt);
        }
        let peek = s.peeks[&1].clone();
        assert!((peek.yaw.abs() - 180.0).abs() < 1e-3, "yaw {}", peek.yaw);
        let v = s.view(&d, 1, &p, 0.0).unwrap();
        assert_eq!(v.fov, 75.0);
        assert_eq!(v.eye, Vec3::new(100.0, 0.0, 60.0));
        // 0x00A63400: fov - magnification * fov.
        assert!((s.view(&d, 1, &p, 0.2).unwrap().fov - 60.0).abs() < 1e-4);
    }

    #[test]
    fn the_peek_takes_the_pawn_and_use_leaves_after_the_fade() {
        let d = Interactables { keyholes: vec![door()], alt_interact_time: 0.3, ..Default::default() };
        let mut s = InteractablesState::new(&d);
        let dt = 1.0 / 60.0;
        let p = pawn(1, Vec3::ZERO);
        for _ in 0..20 {
            s.step_peeks(&d, &[p.clone()], &[input(true)], |_| true, eye, 200.0, dt);
        }
        assert!(s.is_peeking(1));
        // Release, then the movement and buttons are taken.
        s.step_peeks(&d, &[p.clone()], &[input(false)], |_| true, eye, 200.0, dt);
        let mut inp = PlayerInput { forward: 100, right: 5, buttons: buttons::PRIMARY | buttons::USE, yaw: 33.0, ..Default::default() };
        s.lock_input(1, &mut inp);
        assert_eq!((inp.forward, inp.right, inp.buttons), (0, 0, buttons::USE));
        assert_eq!(inp.yaw, 0.0, "the first step hands over the keyhole view");
        let mut inp = PlayerInput { yaw: 33.0, ..Default::default() };
        s.lock_input(1, &mut inp);
        assert_eq!(inp.yaw, 33.0, "afterwards the look is the player's");
        // The fade-in ends at 0.3 s: Idle.
        for _ in 0..20 {
            s.step_peeks(&d, &[p.clone()], &[input(false)], |_| true, eye, 200.0, dt);
        }
        assert_eq!(s.peeks[&1].phase, PeekPhase::Idle);
        // Use leaves: output 1 at the request, the state ends 0.3 s later.
        let ev = s.step_peeks(&d, &[p.clone()], &[input(true)], |_| true, eye, 200.0, dt);
        assert_eq!(ev, vec![InteractEvent::KeyHole { keyhole: 0, player: 1, used: false }]);
        assert!(s.is_peeking(1));
        for _ in 0..30 {
            assert!(s.step_peeks(&d, &[p.clone()], &[input(false)], |_| true, eye, 200.0, dt).is_empty());
        }
        assert!(!s.is_peeking(1));
    }

    #[test]
    fn point_triangle_distance_matches_the_geometry() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X * 10.0, Vec3::Y * 10.0);
        assert!((point_triangle_distance(Vec3::new(2.0, 2.0, 5.0), a, b, c) - 5.0).abs() < 1e-5, "above the face");
        assert!((point_triangle_distance(Vec3::new(-3.0, -4.0, 0.0), a, b, c) - 5.0).abs() < 1e-5, "off a vertex");
        assert!((point_triangle_distance(Vec3::new(5.0, -2.0, 0.0), a, b, c) - 2.0).abs() < 1e-5, "off an edge");
    }
}
