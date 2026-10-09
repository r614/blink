//! Walls of light (`DisWallOfLight`, research/dishonored/systems/walls_of_light.md), decoded from
//! `Dishonored.exe` with the static index.
//!
//! # Power
//!
//! - **Placement** (vslot 132, 0x00A28D80): the wall starts hidden with no collision, its spark
//!   emitters off.
//! - **Consumer callbacks** (the receptacle's plug 0x00C7B490 and unplug 0x00C7B6B0 call every
//!   registered consumer, Kismet notification or not; the wall's `IDisWhaleOilConsumer` slots are
//!   0x00A40230 and 0x00A3EE40): with `m_bEnabled`, a plug powers the wall on when the receptacle's
//!   battery has a charge (else off), an unplug powers it off.
//! - **On** (0x00A3EB10): shown, the activate and ambient sounds, the `m_SparkEffects` emitters
//!   on, `DisSeqEvent_WallOfLight` output 0. **Off** (0x00A3ECA0): hidden, `COLLIDE_NoCollision`,
//!   the spark emitters off, output 1.
//! - **Kismet** (`DisSeqAct_WallofLightControl` 0x00A40270 -> 0x00A3F7F0, by input): 0 TurnOff
//!   (powered off when enabled with a charged receptacle; then not enabled), 1 TurnOn (0x00A3EE60:
//!   a change to enabled powers on when the receptacle has a charge), 2 Toggle (passes the current
//!   `m_bEnabled` to 0x00A3EE60: no change, as decompiled), 3 SwitchPolarity (0x00A29170: flips
//!   `m_bReversed`, output 3 reversed / 2 default).
//!
//! # Tick (vslot 103, 0x00A3EEE0)
//!
//! 1. A running activation timer (`m_ActivationTimer`) that runs out ends the kill effect
//!    (0x00A28F80: the beams off, output 5).
//! 2. With `m_bEnabled`: while time is stopped (Bend Time's world dilation below 1e-8, GameInfo
//!    `m_BendTimeInfo`) the collision goes off; otherwise a wall without collision whose
//!    receptacle's battery has a charge gets `COLLIDE_TouchAll` back.
//! 3. With `COLLIDE_TouchAll`, everything the wall's box overlaps (the octree, then the collision
//!    component's point check with the other's box, flags 0x22086): a live ambient animal (rat)
//!    is shocked (0x00A3E630: a charge at the ambient-animal cost, `TakeDamage(m_Damage)`, the
//!    kill effect); a dead body is disintegrated (0x00A3E7D0); so is a `DishonoredKActor` or
//!    `DishonoredKAsset` that no pawn holds (its `Base` is not a pawn) and that is not a pickup
//!    (`DisPickup_Base`) unless its pickup tweak allows it: a thrown or dropped prop.
//! 4. The cross spark (`m_CrossSparkEffect`, presentation, `dishonored_runtime::walls_of_light`).
//!
//! # Touch (vslot 115, 0x00A3E340)
//!
//! When a live pawn starts touching the powered (charged, enabled, time running) wall: it is
//! "friendly" when its faction is or falls back to one of the wall's factions
//! (`m_FriendlyFactionsOverride` when set, else the tweak's `m_FriendlyFactions`), and, on a
//! reversed wall, also when the player's relationship to it is Enemy. The wall kills when
//! `friendly == m_bReversed` or while its activation timer runs, except that a reversed wall never
//! kills a player-controlled pawn (vslot 125). The kill (0x00A3B940) skips an NPC already in its
//! electrocuted death (`StateNPCMasterDead_Electrocute`, the class at `DAT_0145F1A8`, registration
//! 0x013084A0; the port's burnt NPC is dead at once), takes a charge at the pawn cost (none:
//! nothing happens), plays the shock sound, then an NPC burns to ash (`m_bJib` gibs it instead),
//! any other pawn takes `TakeDamage(m_Damage)` of `DisDamageType_WallOfLight`; the kill effect
//! follows. A reversed wall's kills are the player's.
//!
//! # Kill effect (0x00A3C010)
//!
//! With `m_ActivationTime > 0`: the activation timer is set (`m_ActivationTimeForPawnEffect` for a
//! pawn's kill, else `m_ActivationTime`), then every live pawn in the wall's `Touching` list is
//! killed by 0x00A3B940 whatever its faction (a kill sets the pawn timer unless the tweak jibs,
//! and moves the effect to that pawn), and the `m_Beams` emitters arc to the effect's point
//! (0x00A30CA0). Output 4 fires in any case.
//!
//! # Damage (`TakeDamage_Native`, vslot 232, 0x00A3EA80; vslot 82 forwards to it)
//!
//! With a receptacle whose battery has a charge and `m_bEnabled`: a damage causer that is not a
//! pawn is disintegrated (0x00A3E7D0: a charge at the ambient-animal cost, none: nothing; the
//! shock sound and particles at the hit, the kill effect, then the causer is destroyed, an NPC
//! body burnt). A pawn causer with a `DishonoredDamageType_FastHit` damage type (the sword's) only
//! shocks the wall (0x00A3BCE0). Projectiles reach it through the contact processor (0x00C4F3A0):
//! a hit actor that is a `DisWallOfLight` takes `TakeDamage` with the projectile as causer whatever
//! the damage, and the projectile is destroyed (no impact effects, no stick, no bounce, no burst).
//! The projectile checks meet the wall while it collides: `ShouldTrace` (0x00A21B30) answers the
//! projectile (0x2000000) and melee (0x4000000) checks, none while time is stopped.
//!
//! # Windblast (vslot 223, 0x00A3BEF0)
//!
//! The gather (0x00BF6780) takes the colliding actors in range; when the front passes the wall's
//! bounds centre (0x00BF6AF0, after its sight check) the blast calls the wall's vslot 223 instead
//! of pushing it: the blast factor at the centre (0x00BE8570); at `m_fWindblastMinimumForce` or
//! more, the shock (0x00A3BCE0) at the centre and, with a receptacle, a charge at the
//! ambient-animal cost. The wall is never pushed (it answers 1).
//!
//! # Shock (0x00A3BCE0)
//!
//! The shock sound at the point, the AI noise of the instigator, and with `m_ActivationTime > 0`
//! the activation timer (`m_ActivationTime`; no output 4), the shock particles and the beams.
//!
//! Not ported: the possessed-creature branches of the touch filter, the sword's shock (the sim's
//! sword meets no actors but pawns), the shock's AI noise (`DisNoiseMakerInterface`, loudness 4,
//! context 9), explosions' radius damage on the wall, the activation material
//! (`m_ActivationMat`, none in the shipped tweaks).

use crate::events::{DamageCause, SimEvent};
use crate::interactables::capsule_touches;
use crate::state::{PawnKind, PlayerId, PlayerState, WorldState};
use crate::world_actors::{ChargeCost, WorldActorsState, WorldEvent};
use dishonored_ir::gameplay::GameplayConfig;
use dishonored_ir::world_actors::{WallOfLightDef, WorldActors};
use dishonored_world::geom;
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// `DisSeqEvent_WallOfLight` outputs (`EDisDisSeqEvent_WallOfLightKismetEvent`).
pub mod output {
    pub const ACTIVATED: usize = 0;
    pub const DEACTIVATED: usize = 1;
    pub const POLARITY_DEFAULT: usize = 2;
    pub const POLARITY_REVERSED: usize = 3;
    pub const KILL_EFFECT_ACTIVATED: usize = 4;
    pub const KILL_EFFECT_DEACTIVATED: usize = 5;
}

/// Runtime state of a wall (`m_bEnabled`, `m_bReversed`, shown, `CollisionType`,
/// `m_ActivationTimer`, the pawns it touches).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallState {
    pub enabled: bool,
    pub reversed: bool,
    /// Shown (powered on): the spark emitters run.
    pub powered: bool,
    /// `COLLIDE_TouchAll` (else `COLLIDE_NoCollision`).
    pub collision: bool,
    pub activation: f32,
    /// Pawns overlapping it at the end of the last tick (a touch starts on a new overlap).
    pub touching: Vec<PlayerId>,
    /// Where the beams of the running kill effect or shock end (presentation).
    pub effect_point: Vec3,
}

impl WallState {
    pub fn new(def: &WallOfLightDef) -> Self {
        WallState { enabled: def.enabled, reversed: def.reversed, ..Default::default() }
    }
}

/// A wall that collides this step, as the sim's projectiles and Windblast see it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WallBlocker {
    /// Index into the map's walls.
    pub wall: u32,
    pub triangles: Vec<[Vec3; 3]>,
    pub lo: Vec3,
    pub hi: Vec3,
}

impl WallBlocker {
    /// The bounds centre (the Windblast's target point, slot 80).
    pub fn center(&self) -> Vec3 {
        (self.lo + self.hi) * 0.5
    }
}

/// The walls that collide (the host hands them to the step, [`WorldState::wall_blockers`]).
pub fn blockers(defs: &WorldActors, wa: &WorldActorsState) -> Vec<WallBlocker> {
    defs.walls_of_light
        .iter()
        .zip(&wa.walls)
        .enumerate()
        .filter(|(_, (d, w))| w.collision && !d.triangles.is_empty())
        .map(|(i, (d, _))| WallBlocker { wall: i as u32, triangles: d.triangles.clone(), lo: d.bounds_min, hi: d.bounds_max })
        .collect()
}

/// The colliding walls as a collision layer: they answer the projectile and melee checks
/// (`ShouldTrace` 0x00A21B30), pawns walk into them.
pub fn layer(blockers: &[WallBlocker]) -> dishonored_world::CollisionWorld {
    use dishonored_world::{Tri, channel};
    let tris: Vec<Tri> = blockers
        .iter()
        .flat_map(|b| b.triangles.iter())
        .filter_map(|t| {
            let n = (t[1] - t[0]).cross(t[2] - t[0]);
            (n.length_squared() > 1e-12).then(|| Tri { a: t[0], b: t[1], c: t[2], n: n.normalize(), channels: channel::PROJECTILE | channel::MELEE, source: 0 })
        })
        .collect();
    dishonored_world::CollisionWorld::from_triangles(tris, vec!["DisWallOfLight".into()])
}

/// The first colliding wall the segment `from`..`to` meets: (wall, distance, point).
pub fn segment_hit(blockers: &[WallBlocker], from: Vec3, to: Vec3) -> Option<(u32, f32, Vec3)> {
    let seg = to - from;
    let len = seg.length();
    if len < 1e-6 {
        return None;
    }
    let d = seg / len;
    let mut best: Option<(u32, f32, Vec3)> = None;
    for b in blockers {
        if crate::interactables::ray_box(from, d, b.lo - Vec3::ONE, b.hi + Vec3::ONE, len).is_none() {
            continue;
        }
        for t in &b.triangles {
            if let Some(s) = geom::ray_triangle(from, d, t[0], t[1], t[2]).filter(|s| *s <= len) {
                if best.is_none_or(|x| s < x.1) {
                    best = Some((b.wall, s, from + d * s));
                }
            }
        }
    }
    best
}

/// A projectile's step `from`..`to` struck a colliding wall before anything else: the wall's hit
/// when it is no farther than the world's (the world holds the walls' layer, so a world hit at
/// the wall's distance is the wall) and than `body` (the first pawn's distance).
pub fn projectile_hit(state: &WorldState, from: Vec3, to: Vec3, world: Option<f32>, body: Option<f32>) -> Option<(u32, Vec3)> {
    let (w, d, p) = segment_hit(&state.wall_blockers, from, to)?;
    (world.is_none_or(|x| d <= x + 0.05) && body.is_none_or(|b| d < b)).then_some((w, p))
}

/// How the AI layer answers the faction questions of the touch filter.
pub trait WallSocial {
    /// The pawn's faction is or falls back to one of `factions`.
    fn in_factions(&self, pawn: PlayerId, factions: &[String]) -> bool;
    /// The player's relationship to the pawn is Enemy.
    fn player_enemy(&self, pawn: PlayerId) -> bool;
}

fn ev(wall: usize, output: usize) -> WorldEvent {
    WorldEvent::WallOfLight { wall, output }
}

/// Whether the wall's receptacle has a battery with at least one charge (0x00C65C90 with 1).
pub fn has_charge(defs: &WorldActors, wa: &WorldActorsState, wall: usize) -> bool {
    let Some(r) = defs.walls_of_light.get(wall).and_then(|w| w.receptacle) else { return false };
    let Some(b) = wa.receptacles.get(r).and_then(|r| r.attached) else { return false };
    wa.batteries.get(&b).is_some_and(|b| b.charges >= 1)
}

fn power_on(wa: &mut WorldActorsState, wall: usize, out: &mut Vec<WorldEvent>) {
    wa.walls[wall].powered = true;
    out.push(ev(wall, output::ACTIVATED));
}

fn power_off(wa: &mut WorldActorsState, wall: usize, out: &mut Vec<WorldEvent>) {
    let w = &mut wa.walls[wall];
    w.powered = false;
    w.collision = false;
    w.touching.clear();
    out.push(ev(wall, output::DEACTIVATED));
}

/// The receptacle's consumer callbacks: `plugged` (0x00A40230) or unplugged (0x00A3EE40) for the
/// walls it powers.
pub fn receptacle_changed(defs: &WorldActors, wa: &mut WorldActorsState, receptacle: usize, plugged: bool) -> Vec<WorldEvent> {
    let mut out = Vec::new();
    sync(defs, wa);
    for (i, d) in defs.walls_of_light.iter().enumerate() {
        if d.receptacle != Some(receptacle) || !wa.walls[i].enabled {
            continue;
        }
        if plugged && has_charge(defs, wa, i) {
            power_on(wa, i, &mut out);
        } else {
            power_off(wa, i, &mut out);
        }
    }
    out
}

fn sync(defs: &WorldActors, wa: &mut WorldActorsState) {
    if wa.walls.len() != defs.walls_of_light.len() {
        wa.walls = defs.walls_of_light.iter().map(WallState::new).collect();
    }
}

/// `DisSeqAct_WallofLightControl` input `action` on wall `i` (0x00A3F7F0).
pub fn kismet_action(defs: &WorldActors, wa: &mut WorldActorsState, i: usize, action: usize) -> Vec<WorldEvent> {
    let mut out = Vec::new();
    sync(defs, wa);
    if i >= wa.walls.len() {
        return out;
    }
    let set_enabled = |wa: &mut WorldActorsState, on: bool, out: &mut Vec<WorldEvent>| {
        // 0x00A3EE60: a change powers on / off when the receptacle has a charge.
        if on != wa.walls[i].enabled && has_charge(defs, wa, i) {
            if on {
                power_on(wa, i, out);
            } else {
                power_off(wa, i, out);
            }
        }
        wa.walls[i].enabled = on;
    };
    match action {
        0 => {
            if wa.walls[i].enabled && has_charge(defs, wa, i) {
                power_off(wa, i, &mut out);
            }
            wa.walls[i].enabled = false;
        }
        1 => set_enabled(wa, true, &mut out),
        // Toggle passes the current state (0x00A3F7F0 case 2): no change.
        2 => {
            let cur = wa.walls[i].enabled;
            set_enabled(wa, cur, &mut out);
        }
        3 => {
            let w = &mut wa.walls[i];
            w.reversed = !w.reversed;
            out.push(ev(i, if w.reversed { output::POLARITY_REVERSED } else { output::POLARITY_DEFAULT }));
        }
        _ => {}
    }
    out
}

/// Wall index of an actor (`name`, case-insensitive; `package` when given).
pub fn find(defs: &WorldActors, package: Option<&str>, name: &str) -> Option<usize> {
    defs.walls_of_light.iter().position(|w| w.name.eq_ignore_ascii_case(name) && package.is_none_or(|p| w.package.eq_ignore_ascii_case(p)))
}

/// What a wall acts on in one call: the map's walls and their state, the world, the config, and
/// where the events go.
struct Ctx<'a> {
    defs: &'a WorldActors,
    wa: &'a mut WorldActorsState,
    state: &'a mut WorldState,
    cfg: &'a GameplayConfig,
    events: &'a mut Vec<SimEvent>,
    out: Vec<WorldEvent>,
}

impl Ctx<'_> {
    fn charge(&mut self, wall: usize, cost: ChargeCost) -> bool {
        let Some(r) = self.defs.walls_of_light[wall].receptacle else { return false };
        let (used, evs) = self.wa.use_charge(self.defs, r, cost);
        self.out.extend(evs);
        used
    }

    fn shock_event(&mut self, wall: usize, point: Vec3, particles: bool) {
        self.events.push(SimEvent::WallOfLightShock { wall: wall as u32, point, particles });
    }

    /// The kill (0x00A3B940) of pawn `id` by wall `wall`, credited to `killer`: whether it was
    /// carried out (a charge at the pawn cost was there).
    fn kill_pawn(&mut self, wall: usize, id: PlayerId, killer: Option<PlayerId>) -> bool {
        if self.state.player(id).is_none_or(|p| p.no_body) || !self.charge(wall, ChargeCost::Pawn) {
            return false;
        }
        let defs = self.defs;
        let def = &defs.walls_of_light[wall];
        let Some(p) = self.state.player_mut(id) else { return false };
        let at = p.position;
        if matches!(p.kind, PawnKind::Civilian | PawnKind::Guard) {
            if p.alive {
                burn(p, killer, def.jib, self.events);
            } else {
                // A body: burnt (or gibbed) the same way, nobody's kill.
                p.no_body = true;
                self.events.push(SimEvent::BodyRemoved { body: id, ash: !def.jib });
            }
        } else if p.alive {
            crate::vitals::apply_damage(p, def.damage as f32, killer, DamageCause::WallOfLight, self.events, &self.cfg.vitals);
        }
        self.shock_event(wall, at, false);
        true
    }

    /// The kill effect (0x00A3C010) at `point`: the activation timer, the pawns touching the wall
    /// killed, the beams, output 4. Whether a pawn was killed.
    fn kill_effect(&mut self, wall: usize, point: Vec3, killer: Option<PlayerId>, pawn_effect: bool, touching: &[PlayerId]) -> bool {
        let defs = self.defs;
        let def = &defs.walls_of_light[wall];
        let mut killed = false;
        if def.activation_time > 0.0 {
            self.wa.walls[wall].activation = if pawn_effect { def.activation_time_pawn } else { def.activation_time };
            self.wa.walls[wall].effect_point = point;
            for &id in touching {
                if self.state.player(id).is_some_and(|p| p.alive && p.kind != PawnKind::Rat) && self.kill_pawn(wall, id, killer) {
                    killed = true;
                    if !def.jib {
                        self.wa.walls[wall].activation = def.activation_time_pawn;
                    }
                    if let Some(p) = self.state.player(id) {
                        self.wa.walls[wall].effect_point = p.position;
                    }
                }
            }
        }
        self.out.push(ev(wall, output::KILL_EFFECT_ACTIVATED));
        killed
    }

    /// The shock (0x00A3BCE0) at `point`: the sound, and with `m_ActivationTime` the timer, the
    /// particles and the beams. No output.
    fn shock(&mut self, wall: usize, point: Vec3) {
        let defs = self.defs;
        let def = &defs.walls_of_light[wall];
        let timed = def.activation_time > 0.0;
        if timed {
            self.wa.walls[wall].activation = def.activation_time;
            self.wa.walls[wall].effect_point = point;
        }
        self.shock_event(wall, point, timed);
    }

    /// The disintegration (0x00A3E7D0) of something that is not a live pawn, at `point`: a charge
    /// at the ambient-animal cost (none: nothing, `false`), the shock sound and particles, the
    /// kill effect. The caller destroys the thing.
    fn disintegrate(&mut self, wall: usize, point: Vec3, instigator: Option<PlayerId>, touching: &[PlayerId]) -> bool {
        if !self.charge(wall, ChargeCost::AmbientAnimal) {
            return false;
        }
        self.shock_event(wall, point, true);
        self.kill_effect(wall, point, instigator, false, touching);
        true
    }
}

/// An NPC's death by the wall: burnt to ash, or gibbed with `m_bJib`.
fn burn(p: &mut PlayerState, killer: Option<PlayerId>, jib: bool, events: &mut Vec<SimEvent>) {
    crate::vitals::kill(p, killer, DamageCause::WallOfLight, events);
    p.no_body = true;
    events.push(SimEvent::BodyRemoved { body: p.id, ash: !jib });
}

fn overlaps(def: &WallOfLightDef, p: &PlayerState, cfg: &GameplayConfig) -> bool {
    let c = crate::movement::capsule(p, &cfg.movement);
    let lo = def.bounds_min - Vec3::splat(c.radius) - Vec3::Z * c.half_height;
    let hi = def.bounds_max + Vec3::splat(c.radius) + Vec3::Z * c.half_height;
    c.center.cmpge(lo).all() && c.center.cmple(hi).all() && capsule_touches(&def.triangles, c.center, c.radius, c.half_height)
}

/// Whether a box (`lo`..`hi`) overlaps the wall's collision: it meets one of its triangles or its
/// centre is inside the (convex) mesh. The collision component's point check with the box's
/// extent (0x00A3EEE0, flags 0x22086).
fn box_overlaps(def: &WallOfLightDef, lo: Vec3, hi: Vec3) -> bool {
    if def.triangles.is_empty() || lo.cmpgt(def.bounds_max).any() || hi.cmplt(def.bounds_min).any() {
        return false;
    }
    let (c, half) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
    if def.triangles.iter().any(|t| geom::box_triangle(c, half, t[0], t[1], t[2], 0.0).is_some()) {
        return true;
    }
    let n = (def.triangles.len() * 3) as f32;
    let mid = def.triangles.iter().flatten().fold(Vec3::ZERO, |a, v| a + *v) / n;
    def.triangles.iter().all(|t| {
        let nrm = (t[1] - t[0]).cross(t[2] - t[0]);
        nrm.length_squared() < 1e-12 || (c - t[0]).dot(nrm) * (mid - t[0]).dot(nrm) >= 0.0
    })
}

/// The wall's side of this step's projectile hits and Windblasts (`SimEvent::WallOfLightHit`,
/// `SimEvent::WallOfLightBlast` in `events_in`): the damage (vslot 232) and the Windblast (vslot
/// 223). Call after the step, before [`step`].
pub fn apply_hits(defs: &WorldActors, wa: &mut WorldActorsState, state: &mut WorldState, events_in: &[SimEvent], cfg: &GameplayConfig, events: &mut Vec<SimEvent>) -> Vec<WorldEvent> {
    sync(defs, wa);
    let mut cx = Ctx { defs, wa, state, cfg, events, out: Vec::new() };
    for e in events_in {
        match *e {
            SimEvent::WallOfLightHit { wall, point, owner, .. } => {
                let i = wall as usize;
                if i >= defs.walls_of_light.len() {
                    continue;
                }
                // 0x00A3EA80: a receptacle, a charge, `m_bEnabled`; the projectile is no pawn.
                if !(has_charge(defs, cx.wa, i) && cx.wa.walls[i].enabled) {
                    continue;
                }
                let touching = cx.wa.walls[i].touching.clone();
                cx.disintegrate(i, point, Some(owner).filter(|o| *o != 0), &touching);
            }
            SimEvent::WallOfLightBlast { wall, force, .. } => {
                let i = wall as usize;
                let Some(def) = defs.walls_of_light.get(i) else { continue };
                // 0x00A3BEF0: the shock at the bounds centre, then a charge with a receptacle.
                if force >= def.windblast_min_force {
                    cx.shock(i, (def.bounds_min + def.bounds_max) * 0.5);
                    cx.charge(i, ChargeCost::AmbientAnimal);
                }
            }
            _ => {}
        }
    }
    cx.out
}

/// One tick of every wall of the map. `player` is the pawn whose kills a reversed wall's are and
/// whose relationships the touch filter reads (the local player of the original).
#[allow(clippy::too_many_arguments)]
pub fn step(defs: &WorldActors, wa: &mut WorldActorsState, state: &mut WorldState, social: &dyn WallSocial, player: Option<PlayerId>, cfg: &GameplayConfig, dt: f32, events: &mut Vec<SimEvent>) -> Vec<WorldEvent> {
    sync(defs, wa);
    let stopped = crate::powers::world_time_stopped(state, cfg);
    let mut cx = Ctx { defs, wa, state, cfg, events, out: Vec::new() };
    for (i, def) in defs.walls_of_light.iter().enumerate() {
        // 1. The kill effect's timer.
        if cx.wa.walls[i].activation > 0.0 {
            cx.wa.walls[i].activation -= dt;
            if cx.wa.walls[i].activation <= 0.0 {
                cx.out.push(ev(i, output::KILL_EFFECT_DEACTIVATED));
            }
        }
        if !cx.wa.walls[i].enabled {
            continue;
        }
        // 2. Collision against stopped time and the charge.
        if stopped {
            cx.wa.walls[i].collision = false;
            cx.wa.walls[i].touching.clear();
        } else if !cx.wa.walls[i].collision && has_charge(defs, cx.wa, i) {
            cx.wa.walls[i].collision = true;
        }
        if !cx.wa.walls[i].collision {
            continue;
        }
        // The pawns (live ones and bodies) overlapping it: its `Touching` list.
        let now: Vec<PlayerId> = cx.state.players.iter().filter(|p| !p.no_body && p.kind != PawnKind::Rat && overlaps(def, p, cfg)).map(|p| p.id).collect();
        // 3. What overlaps: rats are shocked, free props disintegrate.
        for k in 0..cx.state.players.len() {
            let p = &cx.state.players[k];
            if !p.alive || p.kind != PawnKind::Rat || !overlaps(def, p, cfg) {
                continue;
            }
            // A rat (0x00A3E630).
            let at = p.position;
            if !cx.charge(i, ChargeCost::AmbientAnimal) {
                continue;
            }
            // A reversed wall's shock is the player's (0x00A3E630).
            let by = if cx.wa.walls[i].reversed { player } else { None };
            crate::vitals::apply_damage(&mut cx.state.players[k], def.damage as f32, by, DamageCause::WallOfLight, cx.events, &cfg.vitals);
            cx.shock_event(i, at, true);
            cx.kill_effect(i, at, None, false, &now);
        }
        for mi in 0..cx.state.movables.len() {
            let m = &cx.state.movables[mi];
            if m.gone || m.holder.is_some() {
                continue;
            }
            let (lo, hi) = crate::movables::world_box(m);
            if !box_overlaps(def, lo, hi) {
                continue;
            }
            let instigator = m.instigator;
            if cx.disintegrate(i, (lo + hi) * 0.5, instigator, &now) {
                let m = &mut cx.state.movables[mi];
                m.gone = true;
                m.phase = crate::state::MovablePhase::Rest;
                m.velocity = Vec3::ZERO;
            }
        }
        // 4. Touch: a pawn that starts overlapping the powered wall.
        let now: Vec<PlayerId> = now.into_iter().filter(|id| cx.state.player(*id).is_some_and(|p| !p.no_body)).collect();
        let started: Vec<PlayerId> = now.iter().copied().filter(|id| !cx.wa.walls[i].touching.contains(id)).collect();
        cx.wa.walls[i].touching = now.clone();
        for id in started {
            if !has_charge(defs, cx.wa, i) {
                continue;
            }
            let Some(p) = cx.state.player(id).filter(|p| !p.no_body) else { continue };
            let reversed = cx.wa.walls[i].reversed;
            let player_controlled = p.kind == PawnKind::Human;
            let friendly = if !p.alive && p.kind != PawnKind::Human {
                // A body (`m_bIsOfficiallyDead`, not the player's pawn): it burns, unless it is
                // carried (`StateNPCMasterDead_BeingCarried`, the class at `DAT_0145F19C`).
                if p.carried_by.is_some() { !reversed } else { reversed }
            } else {
                let mut f = social.in_factions(id, &def.friendly_factions);
                if !f && reversed {
                    f = social.player_enemy(id);
                }
                f
            };
            if !((friendly == reversed || cx.wa.walls[i].activation > 0.0) && (!reversed || !player_controlled)) {
                continue;
            }
            let killer = if reversed { player } else { None };
            let at = p.position;
            if cx.kill_pawn(i, id, killer) {
                cx.kill_effect(i, at, killer, !def.jib, &now);
            }
        }
    }
    cx.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PlayerState;
    use dishonored_ir::world_actors::{BatteryDef, BreakableDef, ReceptacleDef};

    fn cfg() -> GameplayConfig {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
        crate::SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap().gameplay
    }

    /// A box's 12 triangles (the shipped walls' collision is a 20 uu thick box).
    fn box_tris(lo: Vec3, hi: Vec3) -> Vec<[Vec3; 3]> {
        let c = |i: usize| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
        let faces = [[0, 1, 3, 2], [4, 6, 7, 5], [0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3]];
        faces.iter().flat_map(|f| [[c(f[0]), c(f[1]), c(f[2])], [c(f[0]), c(f[2]), c(f[3])]]).collect()
    }

    /// A wall across x = 0 (600 wide, 500 high) powered by a receptacle with a placed battery.
    fn defs(reversed: bool) -> WorldActors {
        let (lo, hi) = (Vec3::new(-10.0, -300.0, 0.0), Vec3::new(10.0, 300.0, 500.0));
        let bat = BreakableDef { name: "Bat".into(), health: 10, battery: Some(BatteryDef { max_charges: 50, initial_charges: 50, pawn_cost: 5, animal_cost: 1, watchtower_cost: 1, ..Default::default() }), ..Default::default() };
        let wall = WallOfLightDef { name: "DisWallOfLight_0".into(), receptacle: Some(0), friendly_factions: vec!["Guards".into()], reversed, enabled: true, damage: 100, activation_time: 0.1, activation_time_pawn: 0.1, triangles: box_tris(lo, hi), bounds_min: lo, bounds_max: hi, windblast_min_force: 0.3, ..Default::default() };
        WorldActors { breakables: vec![bat], receptacles: vec![ReceptacleDef { name: "Rec".into(), battery: Some("Bat".into()), ..Default::default() }], walls_of_light: vec![wall], ..Default::default() }
    }

    struct Soc {
        guards: Vec<PlayerId>,
        enemies: Vec<PlayerId>,
    }

    impl WallSocial for Soc {
        fn in_factions(&self, pawn: PlayerId, f: &[String]) -> bool {
            f.iter().any(|n| n == "Guards") && self.guards.contains(&pawn)
        }
        fn player_enemy(&self, pawn: PlayerId) -> bool {
            self.enemies.contains(&pawn)
        }
    }

    fn pawn(id: PlayerId, x: f32, kind: PawnKind) -> PlayerState {
        let mut p = PlayerState::new(id, Vec3::new(x, 0.0, 100.0), 0.0);
        p.alive = true;
        p.kind = kind;
        p
    }

    /// The receptacle's first tick plugs the placed battery, which powers the wall.
    fn powered(d: &WorldActors, cfg: &GameplayConfig) -> (WorldActorsState, Vec<WorldEvent>) {
        let mut wa = WorldActorsState::new(d);
        let ev = wa.step(d, &[], cfg, 1.0 / 30.0);
        (wa, ev)
    }

    fn tick(d: &WorldActors, wa: &mut WorldActorsState, w: &mut WorldState, soc: &Soc, cfg: &GameplayConfig) -> (Vec<WorldEvent>, Vec<SimEvent>) {
        let mut sim = Vec::new();
        let ev = step(d, wa, w, soc, Some(1), cfg, 1.0 / 30.0, &mut sim);
        (ev, sim)
    }

    #[test]
    fn the_placed_battery_powers_the_wall_on_and_the_tick_gives_it_collision() {
        let (d, c) = (defs(false), cfg());
        let (mut wa, ev) = powered(&d, &c);
        assert_eq!(ev, vec![WorldEvent::WallOfLight { wall: 0, output: output::ACTIVATED }], "the plug callback runs even unnotified");
        assert!(wa.walls[0].powered && !wa.walls[0].collision, "placed without collision (0x00A28D80)");
        assert!(blockers(&d, &wa).is_empty(), "no collision, nothing for the projectiles");
        let mut w = WorldState::default();
        tick(&d, &mut wa, &mut w, &Soc { guards: vec![], enemies: vec![] }, &c);
        assert!(wa.walls[0].collision, "a charged receptacle gives TouchAll back");
        assert_eq!(blockers(&d, &wa).len(), 1);
    }

    #[test]
    fn the_player_walking_in_takes_the_damage_and_a_guard_passes() {
        let (d, c) = (defs(false), cfg());
        let (mut wa, _) = powered(&d, &c);
        let mut w = WorldState::default();
        w.players = vec![pawn(1, -200.0, PawnKind::Human), pawn(2, -200.0, PawnKind::Guard)];
        let soc = Soc { guards: vec![2], enemies: vec![] };
        tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(w.players.iter().all(|p| p.alive), "nobody touches it yet");
        w.players[1].position.x = 0.0;
        let (ev, _) = tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(w.players[1].alive && ev.is_empty(), "the guard's faction is friendly: {ev:?}");
        w.players[0].position.x = 0.0;
        let (ev, sim) = tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(!w.players[0].alive, "100 damage kills the player");
        assert!(sim.iter().any(|e| matches!(e, SimEvent::Damage { victim: 1, cause: DamageCause::WallOfLight, amount, .. } if *amount == 100.0)));
        assert!(!w.players[0].no_body, "the player is not burnt: TakeDamage only");
        assert!(ev.contains(&WorldEvent::WallOfLight { wall: 0, output: output::KILL_EFFECT_ACTIVATED }));
        // The kill effect kills every live pawn in the wall's `Touching` list, the friendly guard
        // standing in it too (0x00A3C010).
        assert!(!w.players[1].alive && w.players[1].no_body, "the guard inside burns with the kill effect");
        assert_eq!(wa.batteries[&0].charges, 40, "two pawn kills at the pawn cost");
        // The timer runs out: output 5.
        let mut ended = false;
        for _ in 0..10 {
            ended |= tick(&d, &mut wa, &mut w, &soc, &c).0.contains(&WorldEvent::WallOfLight { wall: 0, output: output::KILL_EFFECT_DEACTIVATED });
        }
        assert!(ended);
    }

    #[test]
    fn while_the_kill_effect_runs_a_friend_walking_in_burns() {
        let (d, c) = (defs(false), cfg());
        let (mut wa, _) = powered(&d, &c);
        let mut w = WorldState::default();
        w.players = vec![pawn(2, -200.0, PawnKind::Guard)];
        let soc = Soc { guards: vec![2], enemies: vec![] };
        tick(&d, &mut wa, &mut w, &soc, &c);
        w.players[0].position.x = 0.0;
        wa.walls[0].activation = 0.05;
        tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(!w.players[0].alive && w.players[0].no_body, "the guard burns to ash during the effect");
    }

    #[test]
    fn a_reversed_wall_burns_guards_and_the_players_enemies_and_spares_the_player() {
        let (d, c) = (defs(true), cfg());
        let (mut wa, _) = powered(&d, &c);
        let mut w = WorldState::default();
        w.players = vec![pawn(1, 0.0, PawnKind::Human), pawn(2, 0.0, PawnKind::Guard), pawn(3, 0.0, PawnKind::Civilian), pawn(4, -200.0, PawnKind::Civilian)];
        let soc = Soc { guards: vec![2], enemies: vec![3] };
        let (_, sim) = tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(!w.players[1].alive && w.players[1].no_body, "the guard burns");
        assert!(!w.players[2].alive, "the player's enemy burns");
        // The guard's kill effect then kills every pawn touching: the touch filter spares a
        // player-controlled pawn, the kill effect does not (0x00A3C010 has no such test).
        assert!(!w.players[0].alive, "the player inside dies with the kill effect");
        // Once the kill effect is over, a neutral civilian walks through.
        for _ in 0..10 {
            tick(&d, &mut wa, &mut w, &soc, &c);
        }
        w.players[3].position.x = 0.0;
        tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(w.players[3].alive, "a neutral civilian passes");
        assert!(sim.iter().any(|e| matches!(e, SimEvent::Death { victim: 2, killer: Some(1), cause: DamageCause::WallOfLight })), "a reversed wall's kills are the player's");
        assert!(sim.contains(&SimEvent::BodyRemoved { body: 2, ash: true }));
    }

    #[test]
    fn a_reversed_wall_spares_the_player_walking_in() {
        let (d, c) = (defs(true), cfg());
        let (mut wa, _) = powered(&d, &c);
        let mut w = WorldState::default();
        w.players = vec![pawn(1, 0.0, PawnKind::Human)];
        tick(&d, &mut wa, &mut w, &Soc { guards: vec![], enemies: vec![] }, &c);
        assert!(w.players[0].alive, "a reversed wall never kills a player-controlled pawn");
    }

    #[test]
    fn bodies_burn_at_the_pawn_cost_unless_carried_and_stopped_time_switches_the_wall_off() {
        let (d, c) = (defs(false), cfg());
        let (mut wa, _) = powered(&d, &c);
        let mut w = WorldState::default();
        let mut body = pawn(5, 0.0, PawnKind::Guard);
        body.alive = false;
        // A body carried through the wall (`StateNPCMasterDead_BeingCarried`) is spared.
        let mut carried = pawn(6, -200.0, PawnKind::Guard);
        carried.alive = false;
        carried.carried_by = Some(9);
        w.players = vec![body, carried];
        let soc = Soc { guards: vec![5, 6], enemies: vec![] };
        let (ev, sim) = tick(&d, &mut wa, &mut w, &soc, &c);
        // The touch of a dead pawn (0x00A3E340: `m_bIsOfficiallyDead`): it burns, friend or not.
        assert!(w.players[0].no_body && sim.contains(&SimEvent::BodyRemoved { body: 5, ash: true }));
        assert_eq!(wa.batteries[&0].charges, 45, "the pawn cost (0x00A3B940)");
        assert!(ev.contains(&WorldEvent::WallOfLight { wall: 0, output: output::KILL_EFFECT_ACTIVATED }));
        for _ in 0..10 {
            tick(&d, &mut wa, &mut w, &soc, &c);
        }
        w.players[1].position.x = 0.0;
        tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(!w.players[1].no_body, "the carried body is not burnt");
        // Bend Time stopping the world: no collision, nobody dies.
        w.time_bends.push(crate::state::TimeBendState { caster: 9, level: 1, elapsed: 5.0, duration: 100.0, world_dilation: 0.0, self_dilation: 1.0 });
        w.players.push(pawn(1, 0.0, PawnKind::Human));
        tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(!wa.walls[0].collision && w.players[2].alive);
        assert!(blockers(&d, &wa).is_empty(), "stopped time: the projectiles go through");
    }

    #[test]
    fn kismet_turn_off_toggle_and_polarity() {
        let (d, c) = (defs(false), cfg());
        let (mut wa, _) = powered(&d, &c);
        assert_eq!(kismet_action(&d, &mut wa, 0, 2), vec![], "Toggle passes the current state: no change");
        assert_eq!(kismet_action(&d, &mut wa, 0, 0), vec![WorldEvent::WallOfLight { wall: 0, output: output::DEACTIVATED }]);
        assert!(!wa.walls[0].enabled && !wa.walls[0].powered);
        let mut w = WorldState::default();
        w.players = vec![pawn(1, 0.0, PawnKind::Human)];
        tick(&d, &mut wa, &mut w, &Soc { guards: vec![], enemies: vec![] }, &c);
        assert!(w.players[0].alive && !wa.walls[0].collision, "a disabled wall does nothing");
        assert_eq!(kismet_action(&d, &mut wa, 0, 1), vec![WorldEvent::WallOfLight { wall: 0, output: output::ACTIVATED }]);
        assert_eq!(kismet_action(&d, &mut wa, 0, 3), vec![WorldEvent::WallOfLight { wall: 0, output: output::POLARITY_REVERSED }]);
        assert!(wa.walls[0].reversed);
        // Unplugging the battery powers it off (the consumer callback).
        let ev = wa.eject(&d, 0);
        assert_eq!(ev[0], WorldEvent::WallOfLight { wall: 0, output: output::DEACTIVATED });
    }

    /// A powered wall with collision and the step's blockers handed to the world state.
    fn colliding(d: &WorldActors, c: &GameplayConfig) -> (WorldActorsState, WorldState) {
        let (mut wa, _) = powered(d, c);
        let mut w = WorldState::default();
        tick(d, &mut wa, &mut w, &Soc { guards: vec![], enemies: vec![] }, c);
        w.wall_blockers = blockers(d, &wa);
        (wa, w)
    }

    #[test]
    fn a_projectile_crossing_the_wall_is_disintegrated_at_the_animal_cost() {
        let (d, c) = (defs(false), cfg());
        let (mut wa, mut w) = colliding(&d, &c);
        let world = dishonored_world::CollisionWorld::from_triangles(Vec::new(), Vec::new());
        let t = crate::tuning::CombatTuning::from_config(&c);
        let shooter = pawn(1, -400.0, PawnKind::Human);
        // A guard standing in the wall (friendly: it passed) dies with the kill effect.
        w.players = vec![shooter, pawn(2, 0.0, PawnKind::Guard)];
        wa.walls[0].touching = vec![2];
        let bolt = crate::left_hand::make_projectile(7, 1, crate::state::ProjectileKind::Bolt, Vec3::new(-50.0, 0.0, 100.0), Vec3::new(4000.0, 0.0, 0.0), &t);
        w.projectiles.push(bolt);
        let mut sim = Vec::new();
        let none = |_: u32| 1.0;
        crate::left_hand::step_projectiles(&mut w, &world, &c.movement, &t, &c.items.hit_bodies, &c.vitals, 0.0, 1.0 / 30.0, &none, &mut sim);
        assert!(w.projectiles.is_empty(), "the bolt is destroyed");
        let hit = sim.iter().find_map(|e| match e {
            SimEvent::WallOfLightHit { wall: 0, point, projectile: 7, owner: 1 } => Some(*point),
            _ => None,
        });
        let point = hit.expect("the bolt struck the wall");
        assert!((point.x + 10.0).abs() < 0.01, "on the wall's face: {point}");
        assert!(!sim.iter().any(|e| matches!(e, SimEvent::ProjectileImpact { .. })), "no impact: disintegrated, not stuck");
        let mut out = Vec::new();
        let ev = apply_hits(&d, &mut wa, &mut w, &sim, &c, &mut out);
        assert_eq!(wa.batteries[&0].charges, 50 - 1 - 5, "the disintegration's ambient-animal charge, the guard's pawn charge");
        assert!(ev.contains(&WorldEvent::WallOfLight { wall: 0, output: output::KILL_EFFECT_ACTIVATED }));
        assert!(out.contains(&SimEvent::WallOfLightShock { wall: 0, point, particles: true }));
        assert!(!w.players[1].alive, "the kill effect kills the pawn touching the wall");
        assert!(out.iter().any(|e| matches!(e, SimEvent::Death { victim: 2, killer: Some(1), .. })), "credited to the projectile's instigator");
        assert!((wa.walls[0].activation - 0.1).abs() < 1e-6);
    }

    #[test]
    fn a_projectile_flies_through_an_unpowered_wall_and_a_pawn_in_front_takes_the_hit() {
        let (d, c) = (defs(false), cfg());
        let (wa, mut w) = colliding(&d, &c);
        let world = dishonored_world::CollisionWorld::from_triangles(Vec::new(), Vec::new());
        let t = crate::tuning::CombatTuning::from_config(&c);
        // The pawn stands before the wall: the bolt strikes it first.
        w.players = vec![pawn(1, -400.0, PawnKind::Human), pawn(3, -30.0, PawnKind::Guard)];
        w.projectiles.push(crate::left_hand::make_projectile(8, 1, crate::state::ProjectileKind::Bolt, Vec3::new(-100.0, 0.0, 100.0), Vec3::new(3000.0, 0.0, 0.0), &t));
        let mut sim = Vec::new();
        let none = |_: u32| 1.0;
        crate::left_hand::step_projectiles(&mut w, &world, &c.movement, &t, &c.items.hit_bodies, &c.vitals, 0.0, 1.0 / 30.0, &none, &mut sim);
        assert!(sim.iter().any(|e| matches!(e, SimEvent::ProjectileImpact { projectile: 8, victim: Some(3), .. })));
        assert!(!sim.iter().any(|e| matches!(e, SimEvent::WallOfLightHit { .. })));
        // Without collision (no blockers) a bullet crosses the wall's place.
        let _ = wa;
        w.wall_blockers.clear();
        w.players.truncate(1);
        w.projectiles.push(crate::left_hand::make_projectile(9, 1, crate::state::ProjectileKind::Bullet, Vec3::new(-50.0, 0.0, 100.0), Vec3::new(3000.0, 0.0, 0.0), &t));
        let mut sim = Vec::new();
        crate::left_hand::step_projectiles(&mut w, &world, &c.movement, &t, &c.items.hit_bodies, &c.vitals, 0.0, 1.0 / 30.0, &none, &mut sim);
        assert_eq!(w.projectiles.len(), 1, "the bullet flies on");
        assert!(w.projectiles[0].position.x > 10.0);
    }

    #[test]
    fn a_grenade_reaching_the_wall_is_destroyed_without_exploding() {
        let (d, c) = (defs(false), cfg());
        let (_, mut w) = colliding(&d, &c);
        let world = dishonored_world::CollisionWorld::from_triangles(Vec::new(), Vec::new());
        let t = crate::tuning::CombatTuning::from_config(&c);
        w.players = vec![pawn(1, -400.0, PawnKind::Human)];
        w.projectiles.push(crate::left_hand::make_projectile(4, 1, crate::state::ProjectileKind::Grenade, Vec3::new(-30.0, 0.0, 100.0), Vec3::new(1500.0, 0.0, 0.0), &t));
        let mut sim = Vec::new();
        crate::gadgets::step_grenades(&mut w, &world, &c, 1.0 / 30.0, &mut sim);
        assert!(w.projectiles.is_empty());
        assert!(sim.iter().any(|e| matches!(e, SimEvent::WallOfLightHit { wall: 0, projectile: 4, .. })));
        assert!(!sim.iter().any(|e| matches!(e, SimEvent::Explosion { .. })));
    }

    #[test]
    fn a_thrown_prop_in_the_wall_disintegrates_and_a_held_one_does_not() {
        let (d, c) = (defs(false), cfg());
        let (mut wa, mut w) = colliding(&d, &c);
        let prop = |name: &str, half: f32| dishonored_ir::world_actors::MovableDef { name: name.into(), position: Vec3::new(0.0, 0.0, 100.0), bounds_min: Vec3::splat(-half), bounds_max: Vec3::splat(half), ..Default::default() };
        crate::movables::spawn(&mut w, &prop("A", 5.0));
        crate::movables::spawn(&mut w, &prop("B", 5.0));
        w.movables[0].instigator = Some(1);
        w.movables[1].holder = Some(1);
        let soc = Soc { guards: vec![], enemies: vec![] };
        let (ev, sim) = tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(w.movables[0].gone, "the free prop is disintegrated");
        assert!(!w.movables[1].gone, "a prop held by a pawn (its Base) is not");
        assert_eq!(wa.batteries[&0].charges, 49);
        assert!(ev.contains(&WorldEvent::WallOfLight { wall: 0, output: output::KILL_EFFECT_ACTIVATED }));
        assert!(sim.iter().any(|e| matches!(e, SimEvent::WallOfLightShock { wall: 0, particles: true, .. })));
        // A prop well inside the slab (smaller than its thickness) still overlaps it.
        crate::movables::spawn(&mut w, &prop("C", 2.0));
        let (lo, hi) = crate::movables::world_box(&w.movables[2]);
        assert!(box_overlaps(&d.walls_of_light[0], lo, hi));
        assert!(!box_overlaps(&d.walls_of_light[0], lo + Vec3::X * 50.0, hi + Vec3::X * 50.0));
    }

    #[test]
    fn a_windblast_at_the_minimum_force_shocks_the_wall_and_costs_a_charge() {
        let (d, c) = (defs(false), cfg());
        let (mut wa, mut w) = colliding(&d, &c);
        let mut out = Vec::new();
        // Below `m_fWindblastMinimumForce` (0.3): nothing.
        let weak = [SimEvent::WallOfLightBlast { wall: 0, force: 0.29, by: 1 }];
        assert!(apply_hits(&d, &mut wa, &mut w, &weak, &c, &mut out).is_empty());
        assert_eq!(wa.batteries[&0].charges, 50);
        assert!(out.is_empty());
        let strong = [SimEvent::WallOfLightBlast { wall: 0, force: 0.3, by: 1 }];
        let ev = apply_hits(&d, &mut wa, &mut w, &strong, &c, &mut out);
        assert_eq!(wa.batteries[&0].charges, 49, "a charge at the ambient-animal cost");
        assert!(!ev.iter().any(|e| matches!(e, WorldEvent::WallOfLight { output: output::KILL_EFFECT_ACTIVATED, .. })), "the shock fires no output");
        assert!((wa.walls[0].activation - 0.1).abs() < 1e-6, "the activation timer runs: a pawn walking in now dies");
        let centre = Vec3::new(0.0, 0.0, 250.0);
        assert_eq!(out, vec![SimEvent::WallOfLightShock { wall: 0, point: centre, particles: true }]);
        assert_eq!(wa.walls[0].effect_point, centre);
    }

    #[test]
    fn the_windblast_front_reaches_a_colliding_wall_in_its_cone() {
        let (d, c) = (defs(false), cfg());
        let (_, mut w) = colliding(&d, &c);
        let world = dishonored_world::CollisionWorld::from_triangles(Vec::new(), Vec::new());
        let t = crate::tuning::CombatTuning::from_config(&c);
        // The caster faces the wall (+x) from 200 uu.
        let mut caster = pawn(1, -200.0, PawnKind::Human);
        caster.position.z = 250.0 - crate::movement::eye_position(&caster, &c.movement).z + caster.position.z;
        w.players = vec![caster];
        w.windblasts.push(crate::state::WindblastState { caster: 1, warmup: 0.0, elapsed: 0.0, origin: Vec3::ZERO, direction: Vec3::X, targets: Vec::new(), started: false, level: 0, range: 0.0, walls: Vec::new(), projectiles: Vec::new(), suggested: None });
        let mut sim = Vec::new();
        for _ in 0..30 {
            crate::left_hand::step_windblasts(&mut w, &world, &c.movement, &c.vitals, &c.throw, &t, 1.0 / 30.0, &mut sim);
        }
        let force = sim.iter().find_map(|e| match e {
            SimEvent::WallOfLightBlast { wall: 0, force, by: 1 } => Some(*force),
            _ => None,
        });
        let range = t.windblast_distance[0];
        let expect = crate::left_hand::windblast_factor(Vec3::X, Vec3::new(0.0, 0.0, 250.0), crate::movement::eye_position(&w.players[0], &c.movement), range, (t.windblast_angle[0] * 0.5).to_radians().cos());
        assert!(force.is_some_and(|f| (f - expect).abs() < 1e-5 && f > 0.0), "{force:?} vs {expect}");
        assert_eq!(sim.iter().filter(|e| matches!(e, SimEvent::WallOfLightBlast { .. })).count(), 1, "once per blast");
    }
}
