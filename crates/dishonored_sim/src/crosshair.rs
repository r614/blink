//! The crosshair target: the one interactable the player's Use and the HUD's prompt act on
//! (research/dishonored/systems/crosshair.md).
//!
//! The original keeps it in `DishonoredPlayerController.m_pCrosshairActor` (+0x69c), with
//! `m_CrosshairStatus` (+0x63e, `eCrossHairStatus`) and `m_bCanInteractWithCrosshairActor`
//! (+0x610 bit 13). One writer, `0x00AA6280` (called from `0x00AB7B80`), runs once per camera
//! update, i.e. every world tick, from `DishonoredPlayerCamera.UpdateCamera` (0x00AD80A0 ->
//! 0x00ABA860); the Use handler (0x00AD5BA0), the Use / Holster / AltInteract states (0x00AD5A00,
//! 0x00AD57E0, 0x00AD5970) and the HUD (0x00BA5840) read the stored value, so a Use acts on what
//! the previous update chose, not on this tick's view.
//!
//! The choice (0x00AB70F0), from `ViewTarget.POV` (the camera's location, `movement::camera_pov`)
//! along the view for `m_fUsableObjectDist` (`DisTweaks_PlayerInput`):
//!
//! 1. **Narrow sweep.** One box sweep of half extent `m_fUsableObjectExtentNarrow` (trace flags
//!    0x01022097, the crosshair channel) reports its nearest hit. If that is an actor with a
//!    `DisInteractableInterface` whose `CanInteract` holds and the hit lies within the
//!    interactable's `m_fMaxDistance` (`DisTweaks_InteractableInterface`) of the eye, it is the
//!    target.
//! 2. **Broad sweep.** Else the box sweep of half extent `m_fUsableObjectExtentBroad` lists every
//!    hit along the view, nearest first. Each actor with the interface and `CanInteract` is
//!    tried in that order: the distance of stage 1's hit location (zero when nothing was hit) to
//!    the eye must be under its `m_fMaxDistance`; the point aimed at is the actor's location, or
//!    when that is above the eye the top of its bounding box (where the view segment enters the
//!    box, else the top corner nearest the view line); a point line check from the eye to it must
//!    reach it or hit that actor first. The first candidate whose hit component is not a
//!    `SkeletalMeshComponent` wins at once; skeletal ones (pawns, doors, usables) are kept in
//!    reserve, the first of them winning when the list ends or a second one is met.
//!
//! The pawn's `slot 365` (0x00AB30F0) gates the whole update (no focus when dead, in an action
//! or hole-peeking state). The weapon's aim trace (0x00AA5FF0: a ranged item aimed at a pawn
//! sets the actor to that pawn with the interact flag clear, `CHS_OVER_ENEMY`) is not ported:
//! it never selects a Use target.
//!
//! The port's candidates are the sim's own actors (movables, pickups, bodies, pawns) plus the
//! host's ([`HostTarget`]: a mission's usables, doors, talkers, Kismet usables, launchers). Their
//! collision stand-ins are boxes (a mesh's world bounds, a pawn's physics-asset bodies, a point
//! for a host target the host knows only by its centre): the original traces their collision
//! meshes, so the edges of a hit differ by what the mesh leaves out of its box (U-981).

use crate::hit_regions;
use crate::movement::{camera_pov, half_height, view_vector};
use crate::state::{MovablePhase, PawnKind, PlayerId, PlayerState, WorldState};
use crate::{SimConfig, inventory, movables, takedown};
use dishonored_world::{PawnBox, WorldQuery, channel};
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// What the crosshair is over, by the sim's own ids (or the host's key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetRef {
    /// A physics prop, `MovableState::id`.
    Prop(u32),
    /// A pickup, `PickupState::id`.
    Pickup(u32),
    /// A body on the ground (dead or unconscious).
    Body(PlayerId),
    /// A living pawn (choke).
    Pawn(PlayerId),
    /// A target the host owns (`WorldState::host_targets`, by its key).
    Host(u32),
}

/// `eCrossHairStatus` (`DishonoredPlayerController.m_CrosshairStatus`, +0x63e).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum CrosshairStatus {
    #[default]
    Neutral = 0,
    Enemy = 1,
    Pickup = 2,
    CannotUse = 3,
    Usable = 4,
    UsableButBusy = 5,
    Steal = 6,
    Loot = 7,
    MovableObject = 8,
    MovableCorpse = 9,
    DoorLockedBreakable = 10,
    DoorLockedUnbreakable = 11,
    DoorLevelTransition = 12,
    PickupNote = 13,
}

/// `m_pCrosshairActor` with its interact flag and status.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Crosshair {
    pub target: TargetRef,
    /// `m_bCanInteractWithCrosshairActor`: the Use acts on the target.
    pub usable: bool,
    pub status: CrosshairStatus,
}

/// An interactable the host knows, handed to the sim each step ([`WorldState::host_targets`]).
#[derive(Debug, Clone, PartialEq)]
pub struct HostTarget {
    /// The host's handle for it (an index into its own list); the sim returns it in
    /// [`TargetRef::Host`].
    pub key: u32,
    /// The actor's location (the aim point of the broad sweep's line check).
    pub origin: Vec3,
    /// The world box of its collision (`lo == hi` for a point: the host knows only its centre).
    pub lo: Vec3,
    pub hi: Vec3,
    /// `DisTweaks_InteractableInterface.m_fMaxDistance`.
    pub max_distance: f32,
    /// The hit component is a `SkeletalMeshComponent` (a door, a usable, an NPC).
    pub skeletal: bool,
    /// `CanInteract`.
    pub usable: bool,
    pub status: CrosshairStatus,
    /// The pawn this target is (a talkable NPC): its physics-asset bodies are the collision,
    /// and the sim does not offer the pawn a second time.
    pub pawn: Option<PlayerId>,
    /// The placed actor (level package, actor name) whose collision the world holds: when the
    /// host knows no box (`lo == hi`) the box its triangles span stands for it
    /// ([`WorldQuery::actor_bounds`]).
    pub actor: Option<(String, String)>,
    /// How far the host has moved that actor from where the level placed it (what the world's
    /// box is shifted by).
    pub shift: Vec3,
    /// A turned mesh's box (pivot, rotation, local box) that stands for `lo..hi`, its world box.
    pub oriented: Option<(Vec3, glam::Quat, Vec3, Vec3)>,
}

/// The collision stand-in of a candidate.
#[derive(Debug, Clone)]
enum Shape {
    Aabb(Vec3, Vec3),
    /// A turned mesh's box: its center, rotation, local box and the world box around it.
    Obb { pos: Vec3, rot: glam::Quat, lo: Vec3, hi: Vec3, world: (Vec3, Vec3) },
    /// A living pawn: its physics-asset bodies fitted to it (`hit_regions`).
    Rig(PlayerId),
}

#[derive(Debug, Clone)]
struct Candidate {
    target: TargetRef,
    shape: Shape,
    origin: Vec3,
    max_distance: f32,
    skeletal: bool,
    /// Has the interface and `CanInteract` holds: can be chosen. Others only block.
    selectable: bool,
    status: CrosshairStatus,
}

/// The world box of a mesh of local box `lo..hi` (relative to its location) at `pos`.
fn at(pos: Vec3, lo: Vec3, hi: Vec3) -> Shape {
    Shape::Aabb(pos + lo, pos + hi)
}

/// The sim-owned candidates.
fn candidates(state: &WorldState, p: &PlayerState, world: &dyn WorldQuery, cfg: &SimConfig) -> Vec<Candidate> {
    let g = &cfg.gameplay;
    let mv = &g.movement;
    let mut out = Vec::new();
    // Physics props (`DishonoredMovable`): a static mesh; interactable when its tweak says so.
    for m in &state.movables {
        if m.gone || m.phase == MovablePhase::Held {
            continue;
        }
        let (lo, hi) = movables::world_box(m);
        out.push(Candidate {
            target: TargetRef::Prop(m.id),
            shape: Shape::Aabb(lo, hi),
            origin: m.position,
            max_distance: m.max_distance,
            skeletal: false,
            selectable: m.interactable && m.damage >= 0 && p.holding.is_none() && p.carrying.is_none(),
            status: CrosshairStatus::MovableObject,
        });
    }
    // Pickups: a static mesh; carried ones belong to their carrier (pickpocketing).
    for pk in &state.pickups {
        if !pk.active || pk.carried_by.is_some() {
            continue;
        }
        let pos = inventory::pickup_position(state, pk, mv);
        out.push(Candidate {
            target: TargetRef::Pickup(pk.id),
            shape: if pk.local_max.cmpgt(pk.local_min).any() && pk.rotation != Vec3::ZERO {
                Shape::Obb { pos, rot: dishonored_ir::items::ue3_rotation(pk.rotation), lo: pk.local_min, hi: pk.local_max, world: (pos + pk.bounds_min, pos + pk.bounds_max) }
            } else {
                at(pos, pk.bounds_min, pk.bounds_max)
            },
            origin: pos,
            max_distance: g.items.inventory.interact_distance,
            skeletal: false,
            selectable: true,
            status: CrosshairStatus::Pickup,
        });
    }
    // Pawns: bodies on the ground (lift), living ones (choke).
    let host_pawns: Vec<PlayerId> = state.host_targets.iter().filter_map(|h| h.pawn).collect();
    for v in &state.players {
        if v.id == p.id || v.kind == PawnKind::Rat || v.carried_by.is_some() || v.no_body && !v.alive {
            continue;
        }
        if host_pawns.contains(&v.id) {
            continue; // offered by the host (talk), with the pawn's collision
        }
        if v.is_body() {
            let hh = half_height(v, mv);
            let r = mv.collision_radius;
            // A body lies on the ground along the way it fell (its yaw): a box as long as the
            // cylinder is high and as wide and high as it is round (the pose's real collision is
            // the physics asset's bodies in the ragdoll's pose, U-981).
            let c = v.position - Vec3::Z * (hh - r);
            let (lo, hi) = dishonored_ir::items::rotated_box(Vec3::new(-hh, -r, -r), Vec3::new(hh, r, r), Vec3::ONE, [0.0, v.yaw, 0.0]);
            out.push(Candidate {
                target: TargetRef::Body(v.id),
                shape: Shape::Aabb(c + lo, c + hi),
                origin: v.position,
                max_distance: g.items.inventory.interact_distance,
                skeletal: true,
                selectable: p.holding.is_none() && p.carrying.is_none(),
                status: CrosshairStatus::MovableCorpse,
            });
        } else if v.alive {
            out.push(Candidate {
                target: TargetRef::Pawn(v.id),
                shape: Shape::Rig(v.id),
                origin: v.position,
                max_distance: g.items.inventory.interact_distance,
                skeletal: true,
                selectable: takedown::choke_eligible(p, v, &cfg.tuning, mv),
                status: CrosshairStatus::Usable,
            });
        }
    }
    // The host's.
    for h in &state.host_targets {
        let (lo, hi) = match &h.actor {
            Some((package, actor)) if h.lo == h.hi => world.actor_bounds(package, actor).map_or((h.lo, h.hi), |(l, u)| (l + h.shift, u + h.shift)),
            _ => (h.lo, h.hi),
        };
        out.push(Candidate {
            target: TargetRef::Host(h.key),
            shape: match (h.pawn, h.oriented) {
                (Some(id), _) => Shape::Rig(id),
                (None, Some((pos, rot, l, u))) => Shape::Obb { pos, rot, lo: l, hi: u, world: (lo, hi) },
                (None, None) => Shape::Aabb(lo, hi),
            },
            origin: h.origin,
            max_distance: h.max_distance,
            skeletal: h.skeletal,
            selectable: h.usable,
            status: h.status,
        });
    }
    out
}

/// Where a swept box of half extent `e` along `eye + dir t` (`0 <= t <= len`) first meets the
/// candidate: the distance along the line at contact.
fn hit_t(c: &Candidate, state: &WorldState, cfg: &SimConfig, eye: Vec3, dir: Vec3, len: f32, e: f32) -> Option<f32> {
    match &c.shape {
        Shape::Aabb(lo, hi) => crate::interactables::ray_box(eye, dir, *lo - Vec3::splat(e), *hi + Vec3::splat(e), len),
        Shape::Obb { pos, rot, lo, hi, .. } => {
            // In the box's frame; the swept cube's reach along each of its axes.
            let inv = rot.inverse();
            let reach = Vec3::new((*rot * Vec3::X).abs().element_sum(), (*rot * Vec3::Y).abs().element_sum(), (*rot * Vec3::Z).abs().element_sum()) * e;
            crate::interactables::ray_box(inv * (eye - *pos), inv * dir, *lo - reach, *hi + reach, len)
        }
        Shape::Rig(id) => {
            let v = state.player(*id)?;
            hit_regions::ray_pawn(v, &cfg.gameplay.movement, &cfg.gameplay.items.hit_bodies, eye, dir, len, e)
        }
    }
}

/// The world box of a candidate (the aim point's box, `ebx + 0x208` / `GetComponentsBoundingBox`).
fn bounds(c: &Candidate, state: &WorldState, cfg: &SimConfig) -> (Vec3, Vec3) {
    match &c.shape {
        Shape::Aabb(lo, hi) => (*lo, *hi),
        Shape::Obb { world, .. } => *world,
        Shape::Rig(id) => match state.player(*id) {
            Some(v) => {
                let mv = &cfg.gameplay.movement;
                let hh = half_height(v, mv);
                (v.position - Vec3::new(mv.collision_radius, mv.collision_radius, hh), v.position + Vec3::new(mv.collision_radius, mv.collision_radius, hh))
            }
            None => (c.origin, c.origin),
        },
    }
}

/// How far (uu) beyond a candidate's box a world surface still counts as its own collision: its
/// stand-in box is the mesh's bounds, the world holds the mesh's triangles.
const OWN_SURFACE: f32 = 2.0;

/// The nearest hit of a box sweep of half extent `e` along the line, over the static world and
/// every candidate: (distance along the line, candidate index or None for the world).
fn nearest(state: &WorldState, cfg: &SimConfig, world: &dyn WorldQuery, cands: &[Candidate], eye: Vec3, dir: Vec3, len: f32, e: f32) -> Option<(f32, Option<usize>)> {
    let mut best: Option<(f32, Option<usize>)> = None;
    let w = if e > 0.0 {
        world.sweep_box(&PawnBox { center: eye, half: Vec3::splat(e) }, dir * len, channel::CROSSHAIR).map(|h| h.fraction * len)
    } else {
        world.raycast(eye, dir, len, channel::CROSSHAIR).map(|h| h.distance)
    };
    if let Some(t) = w {
        // The static world holds the collision of the props, pickups and usables as the map
        // placed them (U-650): a surface met inside a candidate's box is that candidate's own.
        let at = eye + dir * t;
        let pad = Vec3::splat(e + OWN_SURFACE);
        let owner = cands.iter().position(|c| match &c.shape {
            Shape::Aabb(lo, hi) => at.cmpge(*lo - pad).all() && at.cmple(*hi + pad).all(),
            Shape::Obb { pos, rot, lo, hi, .. } => {
                let l = rot.inverse() * (at - *pos);
                l.cmpge(*lo - pad).all() && l.cmple(*hi + pad).all()
            }
            Shape::Rig(_) => false,
        });
        best = Some((t, owner));
    }
    for (i, c) in cands.iter().enumerate() {
        if let Some(t) = hit_t(c, state, cfg, eye, dir, len, e) {
            if best.is_none_or(|b| t < b.0) {
                best = Some((t, Some(i)));
            }
        }
    }
    best
}

/// The aim point of the broad sweep for a candidate (0x00AB7517 - 0x00AB79DA): its location, or
/// when that is above the eye the top of its box.
fn aim_point(c: &Candidate, state: &WorldState, cfg: &SimConfig, eye: Vec3, dir: Vec3, len: f32) -> Vec3 {
    if c.origin.z <= eye.z {
        return c.origin;
    }
    let (lo, hi) = bounds(c, state, cfg);
    // The segment's slab test against the box (0x00BBB810): where it enters.
    if let Some(t) = crate::interactables::ray_box(eye, dir, lo, hi, len) {
        let p = eye + dir * t;
        return Vec3::new(p.x, p.y, hi.z);
    }
    // Else the top corner nearest the view line.
    let corners = [Vec3::new(lo.x, lo.y, hi.z), Vec3::new(hi.x, lo.y, hi.z), Vec3::new(hi.x, hi.y, hi.z), Vec3::new(lo.x, hi.y, hi.z)];
    let mut best = (f32::MAX, corners[0]);
    for c in corners {
        let d = (c - eye).cross(dir).length_squared();
        if d < best.0 {
            best = (d, c);
        }
    }
    best.1
}

/// Whether the focus update runs for this pawn (`slot 365`, 0x00AB30F0, is non-zero).
fn updates(p: &PlayerState) -> bool {
    p.alive && !p.incapacitated() && p.kind != PawnKind::Rat
}

/// The crosshair target of `p` for its current view (`m_pCrosshairActor` after one camera
/// update).
pub fn target(state: &WorldState, p: &PlayerState, world: &dyn WorldQuery, cfg: &SimConfig) -> Option<Crosshair> {
    if !updates(p) {
        return None;
    }
    let inv = &cfg.gameplay.items.inventory;
    let (len, narrow, broad) = (inv.usable_object_dist, inv.usable_object_extent_narrow, inv.usable_object_extent_broad);
    if len <= 0.0 {
        return None;
    }
    let eye = camera_pov(p, &cfg.gameplay.movement);
    let dir = view_vector(p.yaw, p.pitch.clamp(-89.0, 89.0));
    let cands = candidates(state, p, world, cfg);
    let done = |i: usize| Some(Crosshair { target: cands[i].target, usable: true, status: cands[i].status });

    // 1. The narrow sweep's nearest hit.
    let first = nearest(state, cfg, world, &cands, eye, dir, len, narrow);
    // Its location: the eye along the line to the contact, zero without a hit.
    let loc = first.map_or(Vec3::ZERO, |(t, _)| eye + dir * t);
    if let Some((_, Some(i))) = first {
        let c = &cands[i];
        if c.selectable && loc.distance_squared(eye) < c.max_distance * c.max_distance {
            return done(i);
        }
    }

    // 2. The broad sweep's hits, nearest first.
    let mut hits: Vec<(f32, usize)> = cands.iter().enumerate().filter(|(_, c)| c.selectable).filter_map(|(i, c)| hit_t(c, state, cfg, eye, dir, len, broad).map(|t| (t, i))).collect();
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut reserve: Option<usize> = None;
    for (_, i) in hits {
        let c = &cands[i];
        if loc.distance_squared(eye) >= c.max_distance * c.max_distance {
            continue;
        }
        let aim = aim_point(c, state, cfg, eye, dir, len);
        let to = aim - eye;
        let d = to.length();
        if d > 1e-4 {
            let u = to / d;
            // A point line check to it must reach it, or meet this actor first.
            let first = nearest(state, cfg, world, &cands, eye, u, d, 0.0);
            if let Some((_, hit)) = first {
                if hit != Some(i) {
                    continue;
                }
            }
        }
        match (reserve, c.skeletal) {
            (None, false) => return done(i),
            (None, true) => reserve = Some(i),
            (Some(_), false) => return done(i),
            (Some(r), true) => return done(r),
        }
    }
    reserve.and_then(done)
}

/// The yaw and pitch (degrees) that put the camera's view line through `point` for pawn `p`
/// standing where it is (the camera's place depends on the pitch: a few passes settle it). For
/// scripted agents that aim at an interactable.
pub fn aim_at(p: &PlayerState, mv: &dishonored_ir::gameplay::MovementConfig, point: Vec3) -> (f32, f32) {
    let mut q = *p;
    for _ in 0..4 {
        let d = point - camera_pov(&q, mv);
        q.yaw = d.y.atan2(d.x).to_degrees();
        q.pitch = (d.z / d.truncate().length().max(1.0)).atan().to_degrees().clamp(-89.0, 89.0);
    }
    (q.yaw, q.pitch)
}

/// The pawn a choke press would take: the living pawn the crosshair is on (offered by the sim
/// or by the host, a talkable NPC), when it can be choked from where `p` stands. The choke is
/// the Block button's (`GBA_Block` sets `m_bBlockButton` and `m_bChokeButton`), not Use's.
pub fn choke_target(state: &WorldState, p: &PlayerState, cfg: &SimConfig) -> Option<PlayerId> {
    let id = match p.crosshair?.target {
        TargetRef::Pawn(v) => v,
        TargetRef::Host(k) => state.host_targets.iter().find(|h| h.key == k)?.pawn?,
        _ => return None,
    };
    let v = state.player(id)?;
    takedown::choke_eligible(p, v, &cfg.tuning, &cfg.gameplay.movement).then_some(id)
}

/// The sim's own Use on a focused target: lift, grab, take. Returns whether the Use was spent
/// (the original's Use handler returns after the focused interactable's `Interact`, whatever
/// it did). Host targets are the host's; a living pawn takes no Use (its choke is Block's).
pub fn use_target(state: &mut WorldState, who: usize, c: Crosshair, world: &dyn WorldQuery, cfg: &SimConfig, events: &mut Vec<crate::events::SimEvent>) -> bool {
    let g = &cfg.gameplay;
    let pid = state.players[who].id;
    match c.target {
        TargetRef::Pawn(_) => false,
        TargetRef::Body(b) => {
            takedown::lift(state, pid, b, g, events);
            true
        }
        TargetRef::Prop(id) => {
            if let Some(mi) = movables::find(state, id) {
                movables::grab(state, who, mi, g, events);
            }
            true
        }
        TargetRef::Pickup(id) => {
            if let Some(pi) = state.pickups.iter().position(|x| x.id == id) {
                inventory::use_pickup(state, who, pi, world, g, events);
            }
            true
        }
        TargetRef::Host(_) => false,
    }
}
