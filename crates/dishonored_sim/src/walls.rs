//! Walls of light (`DisWallOfLight`, research/dishonored/systems/walls_of_light.md), decoded from
//! `Dishonored.exe` with the static index.
//!
//! # Power
//!
//! - **Placement** (vslot 132, 0x00A28D80): the wall starts hidden with no collision.
//! - **Consumer callbacks** (the receptacle's plug 0x00C7B490 and unplug 0x00C7B6B0 call every
//!   registered consumer, Kismet notification or not; the wall's `IDisWhaleOilConsumer` slots are
//!   0x00A40230 and 0x00A3EE40): with `m_bEnabled`, a plug powers the wall on when the receptacle's
//!   battery has a charge (else off), an unplug powers it off.
//! - **On** (0x00A3EB10): shown, the activate and ambient sounds, `DisSeqEvent_WallOfLight` output
//!   0. **Off** (0x00A3ECA0): hidden, `COLLIDE_NoCollision`, output 1.
//! - **Kismet** (`DisSeqAct_WallofLightControl` 0x00A40270 -> 0x00A3F7F0, by input): 0 TurnOff
//!   (powered off when enabled with a charged receptacle; then not enabled), 1 TurnOn (0x00A3EE60:
//!   a change to enabled powers on when the receptacle has a charge), 2 Toggle (passes the current
//!   `m_bEnabled` to 0x00A3EE60: no change, as decompiled), 3 SwitchPolarity (0x00A29170: flips
//!   `m_bReversed`, output 3 reversed / 2 default).
//!
//! # Tick (vslot 103, 0x00A3EEE0)
//!
//! 1. A running activation timer (`m_ActivationTimer`) that runs out ends the kill effect
//!    (0x00A28F80): output 5.
//! 2. With `m_bEnabled`: while time is stopped (Bend Time's world dilation below 1e-8, GameInfo
//!    `m_BendTimeInfo`) the collision goes off; otherwise a wall without collision whose
//!    receptacle's battery has a charge gets `COLLIDE_TouchAll` back.
//! 3. With `COLLIDE_TouchAll`, everything overlapping the collision box: a live ambient animal
//!    (rat) is shocked (0x00A3E630: a charge at the ambient-animal cost, `TakeDamage(m_Damage)`,
//!    the kill effect); a dead body is disintegrated (0x00A3E7D0: a charge at the ambient-animal
//!    cost, the body burns to ash, the kill effect). The engine's line check from the wall's box to
//!    the victim is not ported (the port's world has no actor identity).
//!
//! # Touch (vslot 115, 0x00A3E340)
//!
//! When a live pawn starts touching the powered (charged, enabled, time running) wall: it is
//! "friendly" when its faction is or falls back to one of the wall's factions
//! (`m_FriendlyFactionsOverride` when set, else the tweak's `m_FriendlyFactions`), and, on a
//! reversed wall, also when the player's relationship to it is Enemy. The wall kills when
//! `friendly == m_bReversed` or while its activation timer runs, except that a reversed wall never
//! kills a player-controlled pawn (vslot 125). The kill (0x00A3B940) takes a charge at the pawn
//! cost (none: nothing happens), then an NPC burns to ash (`m_bJib` gibs it instead), any other
//! pawn takes `TakeDamage(m_Damage)` of `DisDamageType_WallOfLight`; the kill effect follows
//! (0x00A3C010: the activation timer from `m_ActivationTimeForPawnEffect` unless the tweak jibs,
//! else `m_ActivationTime`; output 4). A reversed wall's kills are the player's.
//!
//! Not ported: the special-class exemption of 0x00A3B940 (an exact class match, unresolved), the
//! possessed-creature branches of the touch filter, the windblast rebound (0x00A3BEF0: a charge
//! when the blast's force passes `m_fWindblastMinimumForce`), the projectile and melee blocking
//! while powered (`ShouldTrace` 0x00A21B30, slot 232 0x00A3EA80 disintegrating thrown objects),
//! sparks, beams and sounds (presentation).

use crate::events::{DamageCause, SimEvent};
use crate::interactables::capsule_touches;
use crate::state::{PawnKind, PlayerId, PlayerState, WorldState};
use crate::world_actors::{ChargeCost, WorldActorsState, WorldEvent};
use dishonored_ir::gameplay::GameplayConfig;
use dishonored_ir::world_actors::{WallOfLightDef, WorldActors};
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
    /// Shown (powered on): the beams and the mesh render.
    pub powered: bool,
    /// `COLLIDE_TouchAll` (else `COLLIDE_NoCollision`).
    pub collision: bool,
    pub activation: f32,
    /// Pawns overlapping it at the end of the last tick (a touch starts on a new overlap).
    pub touching: Vec<PlayerId>,
}

impl WallState {
    pub fn new(def: &WallOfLightDef) -> Self {
        WallState { enabled: def.enabled, reversed: def.reversed, ..Default::default() }
    }
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

/// The kill effect (0x00A3C010): the activation timer and output 4.
fn kill_effect(wa: &mut WorldActorsState, def: &WallOfLightDef, wall: usize, pawn_effect: bool, out: &mut Vec<WorldEvent>) {
    wa.walls[wall].activation = if pawn_effect { def.activation_time_pawn } else { def.activation_time };
    out.push(ev(wall, output::KILL_EFFECT_ACTIVATED));
}

/// An NPC's death by the wall: burnt to ash, or gibbed with `m_bJib`.
fn burn(p: &mut PlayerState, killer: Option<PlayerId>, jib: bool, events: &mut Vec<SimEvent>) {
    crate::vitals::kill(p, killer, DamageCause::WallOfLight, events);
    p.no_body = true;
    events.push(SimEvent::BodyRemoved { body: p.id, ash: !jib });
}

fn overlaps(def: &WallOfLightDef, p: &PlayerState, cfg: &GameplayConfig) -> bool {
    let c = crate::movement::capsule(p, &cfg.movement);
    let lo = def.bounds_min - glam::Vec3::splat(c.radius) - glam::Vec3::Z * c.half_height;
    let hi = def.bounds_max + glam::Vec3::splat(c.radius) + glam::Vec3::Z * c.half_height;
    c.center.cmpge(lo).all() && c.center.cmple(hi).all() && capsule_touches(&def.triangles, c.center, c.radius, c.half_height)
}

/// One tick of every wall of the map. `player` is the pawn whose kills a reversed wall's are and
/// whose relationships the touch filter reads (the local player of the original).
#[allow(clippy::too_many_arguments)]
pub fn step(defs: &WorldActors, wa: &mut WorldActorsState, state: &mut WorldState, social: &dyn WallSocial, player: Option<PlayerId>, cfg: &GameplayConfig, dt: f32, events: &mut Vec<SimEvent>) -> Vec<WorldEvent> {
    let mut out = Vec::new();
    sync(defs, wa);
    let stopped = crate::powers::world_time_stopped(state, cfg);
    for (i, def) in defs.walls_of_light.iter().enumerate() {
        // 1. The kill effect's timer.
        if wa.walls[i].activation > 0.0 {
            wa.walls[i].activation -= dt;
            if wa.walls[i].activation <= 0.0 {
                out.push(ev(i, output::KILL_EFFECT_DEACTIVATED));
            }
        }
        if !wa.walls[i].enabled {
            continue;
        }
        // 2. Collision against stopped time and the charge.
        if stopped {
            wa.walls[i].collision = false;
            wa.walls[i].touching.clear();
        } else if !wa.walls[i].collision && has_charge(defs, wa, i) {
            wa.walls[i].collision = true;
        }
        if !wa.walls[i].collision {
            continue;
        }
        // 3. What overlaps: rats are shocked, bodies burn.
        let mut now = Vec::new();
        for k in 0..state.players.len() {
            let p = &state.players[k];
            if p.no_body || !overlaps(def, p, cfg) {
                continue;
            }
            let id = p.id;
            if p.alive && p.kind != PawnKind::Rat {
                now.push(id);
                continue;
            }
            let Some(r) = def.receptacle else { continue };
            let (used, evs) = wa.use_charge(defs, r, ChargeCost::AmbientAnimal);
            out.extend(evs);
            if !used {
                continue;
            }
            let p = &mut state.players[k];
            if p.alive {
                crate::vitals::apply_damage(p, def.damage as f32, None, DamageCause::WallOfLight, events, &cfg.vitals);
            } else {
                p.no_body = true;
                events.push(SimEvent::BodyRemoved { body: id, ash: true });
            }
            kill_effect(wa, def, i, false, &mut out);
        }
        // 4. Touch: a live pawn that starts overlapping the powered wall.
        let started: Vec<PlayerId> = now.iter().copied().filter(|id| !wa.walls[i].touching.contains(id)).collect();
        wa.walls[i].touching = now;
        for id in started {
            if !has_charge(defs, wa, i) {
                continue;
            }
            let Some(p) = state.player(id) else { continue };
            let reversed = wa.walls[i].reversed;
            let player_controlled = p.kind == PawnKind::Human;
            let mut friendly = social.in_factions(id, &def.friendly_factions);
            if !friendly && reversed {
                friendly = social.player_enemy(id);
            }
            if !((friendly == reversed || wa.walls[i].activation > 0.0) && (!reversed || !player_controlled)) {
                continue;
            }
            let Some(r) = def.receptacle else { continue };
            let (used, evs) = wa.use_charge(defs, r, ChargeCost::Pawn);
            out.extend(evs);
            if !used {
                continue;
            }
            let killer = if reversed { player } else { None };
            let Some(p) = state.player_mut(id) else { continue };
            if matches!(p.kind, PawnKind::Civilian | PawnKind::Guard) {
                burn(p, killer, def.jib, events);
            } else {
                crate::vitals::apply_damage(p, def.damage as f32, killer, DamageCause::WallOfLight, events, &cfg.vitals);
            }
            kill_effect(wa, def, i, !def.jib, &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PlayerState;
    use dishonored_ir::world_actors::{BatteryDef, BreakableDef, ReceptacleDef};
    use glam::Vec3;

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
        let wall = WallOfLightDef { name: "DisWallOfLight_0".into(), receptacle: Some(0), friendly_factions: vec!["Guards".into()], reversed, enabled: true, damage: 100, activation_time: 0.1, activation_time_pawn: 0.1, triangles: box_tris(lo, hi), bounds_min: lo, bounds_max: hi, ..Default::default() };
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
        let mut w = WorldState::default();
        tick(&d, &mut wa, &mut w, &Soc { guards: vec![], enemies: vec![] }, &c);
        assert!(wa.walls[0].collision, "a charged receptacle gives TouchAll back");
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
        assert_eq!(wa.batteries[&0].charges, 45, "the pawn cost");
        // While the kill effect runs, whoever starts touching dies, friend or not.
        w.players[1].position.x = -200.0;
        tick(&d, &mut wa, &mut w, &soc, &c);
        w.players[1].position.x = 0.0;
        wa.walls[0].activation = 0.05;
        tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(!w.players[1].alive && w.players[1].no_body, "the guard burns to ash during the effect");
        // The timer runs out: output 5.
        let mut ended = false;
        for _ in 0..10 {
            ended |= tick(&d, &mut wa, &mut w, &soc, &c).0.contains(&WorldEvent::WallOfLight { wall: 0, output: output::KILL_EFFECT_DEACTIVATED });
        }
        assert!(ended);
    }

    #[test]
    fn a_reversed_wall_burns_guards_and_the_players_enemies_and_spares_the_player() {
        let (d, c) = (defs(true), cfg());
        let (mut wa, _) = powered(&d, &c);
        let mut w = WorldState::default();
        w.players = vec![pawn(1, 0.0, PawnKind::Human), pawn(2, 0.0, PawnKind::Guard), pawn(3, 0.0, PawnKind::Civilian), pawn(4, -200.0, PawnKind::Civilian)];
        let soc = Soc { guards: vec![2], enemies: vec![3] };
        let (_, sim) = tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(w.players[0].alive, "a reversed wall never kills a player-controlled pawn");
        assert!(!w.players[1].alive && w.players[1].no_body, "the guard burns");
        assert!(!w.players[2].alive, "the player's enemy burns");
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
    fn bodies_burn_at_the_animal_cost_and_stopped_time_switches_the_wall_off() {
        let (d, c) = (defs(false), cfg());
        let (mut wa, _) = powered(&d, &c);
        let mut w = WorldState::default();
        let mut body = pawn(5, 0.0, PawnKind::Guard);
        body.alive = false;
        w.players = vec![body];
        let soc = Soc { guards: vec![5], enemies: vec![] };
        let (_, sim) = tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(w.players[0].no_body && sim.contains(&SimEvent::BodyRemoved { body: 5, ash: true }));
        assert_eq!(wa.batteries[&0].charges, 49, "the ambient-animal cost");
        // Bend Time stopping the world: no collision, nobody dies.
        w.time_bends.push(crate::state::TimeBendState { caster: 9, level: 1, elapsed: 5.0, duration: 100.0, world_dilation: 0.0, self_dilation: 1.0 });
        w.players.push(pawn(1, 0.0, PawnKind::Human));
        tick(&d, &mut wa, &mut w, &soc, &c);
        assert!(!wa.walls[0].collision && w.players[1].alive);
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
}
