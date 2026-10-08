//! Breakables, whale oil batteries and receptacles (research/dishonored/systems/world.md
//! sections 4 and 9, physics.md 3.5, kismet.md 3.4 / 3.5), decoded from `Dishonored.exe`:
//!
//! - **Breakable damage** (`DishonoredBreakable` vslot 235, 0x00a26a90): ignored while the actor
//!   is pending destruction or when the damage type is in `m_pImmuneToDamageTypes`; when
//!   `m_CurHealth >= 0` and the damage is at least `m_DamageThreshold`, the health drops by the
//!   damage (to 0 for an `m_pInstantBreakDamageTypes` type), floored at 0, and the steps are
//!   scanned from the last down to `m_NextStep`: the first whose `m_HealthThreshold` is at least
//!   the health becomes the wanted step (`m_PendingBreakage`). Any positive damage cuts a
//!   running step timer to 0.01 s. `m_Health` -1 (the class default) never breaks.
//! - **Breakable tick** (vslot 103, 0x00a3b0d0, then the step 0x00a3a5f0): a wanted step at or
//!   after `m_NextStep` is applied: the step's hook (vslot 237: a battery's damaged / destroyed
//!   notification), the `DisSeqEvent_BreakableBroken` output `Num(m_Steps) - step - 1` (originator
//!   the breakable, no instigator; the last step is output 0 "Broken 100%"), the step's AI noise
//!   when it has a sound (its `m_AINoiseLoudness` / `m_AINoiseContext`, 0x00b14e80 -> 0x00c5a760),
//!   then `m_NextStep = step + 1` and the step timer from the step's `m_fStepTimer`. A running
//!   step timer that runs out sets the health to the next step's threshold and makes it the
//!   wanted step (applied next tick).
//! - **Battery charges** (`DisWhaleOilBattery::SetCharges` 0x00c81d70): going from empty to
//!   charged restores `m_CurHealth` to the tweak's `m_Health`; an empty battery has health -1
//!   (cannot be damaged); with `notify` the `DisSeqEvent_WhaleOilBattery` fires output 1
//!   (AllChargesDepleted) when empty, else 0 (ChargeUsed); an empty battery that is not
//!   recharging is ejected from its receptacle (0x00c7d190).
//! - **Use of a charge** (0x00c85170, called by the powered devices): the receptacle's battery
//!   with `charges >= 0` loses the cost of the victim class (`EWhaleOilChargeCost`, 0x00c731d0:
//!   pawn / ambient animal / watchtower, any other 1), notified.
//! - **Plug** (0x00c7b490): a receptacle holding another battery refuses; the attach test
//!   (0x00c65820) refuses an empty battery in a receptacle that does not allow empty ones, and a
//!   charged one in a receptacle that does, unless it is already plugged there; a plugged battery
//!   is recharging when the receptacle allows empty batteries. With `notify` the
//!   `DisSeqEvent_WhaleOilReceptacle` output 0 (OnPlug) fires, instigator the battery. A
//!   receptacle placed with a battery (`m_pBattery`) plugs it on its first tick, not notified
//!   (0x00c822b0).
//! - **Unplug** (0x00c7b6b0): output 1 (OnUnplug), instigator the battery; the battery leaves
//!   the receptacle.

use crate::events::{SimEvent, SoundCategory};
use crate::gadgets::{explosion_damage, grenade_cfg};
use crate::state::{PlayerId, ProjectileKind};
use dishonored_ir::gameplay::GameplayConfig;
use dishonored_ir::world_actors::{BreakableDef, WorldActors};
use serde::{Deserialize, Serialize};

/// `EWhaleOilChargeCost`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChargeCost {
    Pawn,
    AmbientAnimal,
    Watchtower,
}

/// Runtime state of a breakable (`m_CurHealth`, `m_NextStep`, `m_fStepTimer`,
/// `m_PendingBreakage.m_iWantedStep`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BreakableState {
    pub cur_health: i32,
    pub next_step: usize,
    pub step_timer: f32,
    pub wanted: Option<usize>,
    /// The last damage's instigator (+0x15c), the maker of the steps' AI noises.
    pub instigator: Option<PlayerId>,
}

/// Runtime state of a battery (`m_CurrentCharges`, `m_pPlug`, `m_bRecharging`, `m_bDamaged`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatteryState {
    pub charges: i32,
    /// Index of the receptacle it is plugged in.
    pub plug: Option<usize>,
    pub recharging: bool,
    pub damaged: bool,
}

/// Runtime state of a receptacle (`m_pAttachedBattery`, `m_pPreviousBattery`,
/// `m_bBatteryAttached`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReceptacleState {
    pub attached: Option<usize>,
    pub previous: Option<usize>,
    pub placed_attached: bool,
}

/// What the world actors report (the host fires the Kismet events).
#[derive(Debug, Clone, PartialEq)]
pub enum WorldEvent {
    /// `DisSeqEvent_BreakableBroken` output of a breakable (index into `breakables`).
    Broken { breakable: usize, output: usize },
    /// `DisSeqEvent_WhaleOilBattery` output (0 ChargeUsed, 1 AllChargesDepleted, 2 Damaged,
    /// 3 Destroyed) of a battery (index into `breakables`).
    Battery { battery: usize, output: usize },
    /// `DisSeqEvent_WhaleOilReceptacle` output (0 OnPlug, 1 OnUnplug), instigator the battery.
    Receptacle { receptacle: usize, battery: usize, output: usize },
    /// An AI noise (a breaking step's).
    Noise(SimEvent),
}

/// The world actors of the active map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldActorsState {
    /// One per `WorldActors::breakables` (batteries included).
    pub breakables: Vec<BreakableState>,
    /// Batteries by breakable index.
    pub batteries: std::collections::BTreeMap<usize, BatteryState>,
    /// One per `WorldActors::receptacles`.
    pub receptacles: Vec<ReceptacleState>,
}

impl BreakableState {
    pub fn new(def: &BreakableDef) -> Self {
        BreakableState { cur_health: def.health, ..Default::default() }
    }

    /// 0x00a26a90 (see the module docs).
    pub fn take_damage(&mut self, def: &BreakableDef, damage: i32, damage_type: &str, instigator: Option<PlayerId>) {
        let is = |l: &[String]| !damage_type.is_empty() && l.iter().any(|t| t.eq_ignore_ascii_case(damage_type));
        if is(&def.immune) {
            return;
        }
        if instigator.is_some() {
            self.instigator = instigator;
        }
        if self.cur_health >= 0 && damage >= def.damage_threshold {
            self.cur_health = if is(&def.instant_break) { 0 } else { self.cur_health - damage }.max(0);
            if let Some(i) = (self.next_step..def.steps.len()).rev().find(|&i| self.cur_health <= def.steps[i].health_threshold) {
                self.wanted = Some(i);
            }
        }
        if damage > 0 && self.step_timer > 0.0 {
            self.step_timer = 0.01;
        }
    }

    /// The breakable's tick (0x00a3b0d0): the wanted step is applied (0x00a3a5f0), then the step
    /// timer runs. Returns the step applied this tick.
    pub fn tick(&mut self, def: &BreakableDef, dt: f32) -> Option<usize> {
        let mut applied = None;
        if let Some(w) = self.wanted.filter(|&w| w >= self.next_step && w < def.steps.len()) {
            applied = Some(w);
            self.next_step = w + 1;
            self.step_timer = def.steps[w].step_timer;
        }
        if self.step_timer > 0.0 {
            self.step_timer -= dt;
            if self.step_timer <= 0.0 && self.next_step < def.steps.len() {
                self.cur_health = def.steps[self.next_step].health_threshold;
                self.wanted = Some(self.next_step);
                self.step_timer = 0.0;
            }
        }
        applied
    }

    /// `DisSeqEvent_BreakableBroken` output of a step (0x00a3a5f0: `Num(m_Steps) - step - 1`).
    pub fn output(def: &BreakableDef, step: usize) -> usize {
        def.steps.len().saturating_sub(step + 1)
    }
}

impl WorldActorsState {
    /// Fresh state for a map: health from the tweaks, batteries with `m_InitialNumberOfCharges`
    /// (vslot 131, 0x00c850c0: `SetCharges(initial, 0)`).
    pub fn new(defs: &WorldActors) -> Self {
        let mut s = WorldActorsState { breakables: defs.breakables.iter().map(BreakableState::new).collect(), batteries: Default::default(), receptacles: vec![ReceptacleState::default(); defs.receptacles.len()] };
        for (i, d) in defs.breakables.iter().enumerate() {
            if let Some(b) = &d.battery {
                s.batteries.insert(i, BatteryState { charges: 0, ..Default::default() });
                s.set_charges(defs, i, b.initial_charges, false);
            }
        }
        s
    }

    /// Breakable index of an actor (`name`, case-insensitive; `package` when given).
    pub fn find(defs: &WorldActors, package: Option<&str>, name: &str) -> Option<usize> {
        defs.breakables.iter().position(|b| b.name.eq_ignore_ascii_case(name) && package.is_none_or(|p| b.package.eq_ignore_ascii_case(p)))
    }

    /// `SetCharges(n, notify)` (0x00c81d70). Returns the events (battery notification, and the
    /// unplug of an emptied battery).
    pub fn set_charges(&mut self, defs: &WorldActors, battery: usize, n: i32, notify: bool) -> Vec<WorldEvent> {
        let mut out = Vec::new();
        let Some(def) = defs.breakables.get(battery) else { return out };
        let Some(b) = self.batteries.get_mut(&battery) else { return out };
        let brk = &mut self.breakables[battery];
        if b.charges < 1 && n > 0 {
            brk.cur_health = def.health;
        }
        b.charges = n;
        if n < 1 {
            brk.cur_health = -1;
        }
        if notify {
            out.push(WorldEvent::Battery { battery, output: if n < 1 { 1 } else { 0 } });
        }
        if b.charges < 1 && !b.recharging {
            out.extend(self.eject(battery));
        }
        out
    }

    /// The battery leaves its receptacle (0x00c7d190 -> 0x00c7b6b0). The physical ejection's
    /// shield test (0x00c7b1a0, a trace for a usable object in the way) is not modelled: the
    /// battery always leaves.
    pub fn eject(&mut self, battery: usize) -> Vec<WorldEvent> {
        let Some(b) = self.batteries.get_mut(&battery) else { return Vec::new() };
        let Some(r) = b.plug.take() else { return Vec::new() };
        b.recharging = false;
        if let Some(rs) = self.receptacles.get_mut(r) {
            if rs.attached == Some(battery) {
                rs.attached = None;
            }
        }
        vec![WorldEvent::Receptacle { receptacle: r, battery, output: 1 }]
    }

    /// Plug a battery in a receptacle (0x00c7b490). Returns whether it is plugged, and the events.
    pub fn plug(&mut self, defs: &WorldActors, receptacle: usize, battery: usize, notify: bool) -> (bool, Vec<WorldEvent>) {
        let mut out = Vec::new();
        let (Some(rdef), Some(rs)) = (defs.receptacles.get(receptacle), self.receptacles.get(receptacle)) else { return (false, out) };
        if rs.attached.is_some_and(|a| a != battery) {
            return (false, out);
        }
        let Some(b) = self.batteries.get(&battery) else { return (false, out) };
        // The attach test (0x00c65820): an empty battery needs a receptacle allowing empty ones,
        // a charged one a receptacle that does not, unless it is plugged there already.
        let allow = rdef.allow_empty;
        let needs_plugged = if !allow { b.charges < 1 } else { b.charges > 0 };
        if needs_plugged && b.plug != Some(receptacle) {
            self.receptacles[receptacle].attached = None;
            return (false, out);
        }
        // A battery plugged elsewhere leaves that receptacle first (it is moved here).
        if let Some(old) = b.plug.filter(|&o| o != receptacle) {
            if let Some(o) = self.receptacles.get_mut(old) {
                if o.attached == Some(battery) {
                    o.attached = None;
                }
            }
        }
        let b = self.batteries.get_mut(&battery).unwrap();
        b.plug = Some(receptacle);
        b.recharging = allow;
        let rs = &mut self.receptacles[receptacle];
        rs.attached = Some(battery);
        if notify {
            out.push(WorldEvent::Receptacle { receptacle, battery, output: 0 });
        }
        rs.previous = rs.attached;
        (true, out)
    }

    /// A powered device uses a charge of its receptacle's battery (0x00c85170). Returns whether
    /// a charge was used, and the events.
    pub fn use_charge(&mut self, defs: &WorldActors, receptacle: usize, cost: ChargeCost) -> (bool, Vec<WorldEvent>) {
        let Some(battery) = self.receptacles.get(receptacle).and_then(|r| r.attached) else { return (false, Vec::new()) };
        let Some(bdef) = defs.breakables.get(battery).and_then(|d| d.battery.as_ref()) else { return (false, Vec::new()) };
        let c = match cost {
            ChargeCost::Pawn => bdef.pawn_cost,
            ChargeCost::AmbientAnimal => bdef.animal_cost,
            ChargeCost::Watchtower => bdef.watchtower_cost,
        };
        let charges = self.batteries.get(&battery).map_or(-1, |b| b.charges);
        if charges < 0 {
            return (false, Vec::new());
        }
        (true, self.set_charges(defs, battery, charges - c, true))
    }

    /// Whether a receptacle powers its devices (a battery is attached).
    pub fn powered(&self, receptacle: usize) -> bool {
        self.receptacles.get(receptacle).is_some_and(|r| r.attached.is_some())
    }

    /// One tick: receptacles plug their placed battery (0x00c822b0), explosions damage the
    /// breakables in reach, and every breakable ticks (batteries excepted: their damage path,
    /// with the plugged battery's shield test and the explosion chain, is not ported).
    ///
    /// Explosion damage per breakable (0x00A02E90 with the non-player radii, 0x009FEAA0): the
    /// blast's `m_RadialDamage` falling from `m_fFullDamageRadius` to 0 at `m_fDamageRadius`,
    /// rounded, of the explosion's `m_pRadialDamageType`, at the distance to the breakable's
    /// location. The blast's line check to the actor (0x00A02B60) is not applied: the port's
    /// collision has no actor identity to ignore the victim's own geometry.
    pub fn step(&mut self, defs: &WorldActors, events: &[SimEvent], cfg: &GameplayConfig, dt: f32) -> Vec<WorldEvent> {
        let mut out = Vec::new();
        if self.breakables.len() != defs.breakables.len() || self.receptacles.len() != defs.receptacles.len() {
            *self = WorldActorsState::new(defs);
        }
        for r in 0..defs.receptacles.len() {
            if self.receptacles[r].placed_attached {
                continue;
            }
            let Some(name) = defs.receptacles[r].battery.clone() else { continue };
            let Some(b) = Self::find(defs, Some(&defs.receptacles[r].package), &name).or_else(|| Self::find(defs, None, &name)) else { continue };
            let (ok, ev) = self.plug(defs, r, b, false);
            self.receptacles[r].placed_attached = ok;
            out.extend(ev);
        }
        for e in events {
            let SimEvent::Explosion { owner, position, sticky } = e else { continue };
            let kind = if *sticky { ProjectileKind::StickyGrenade } else { ProjectileKind::Grenade };
            let ex = &grenade_cfg(cfg, kind).explosion;
            for (i, d) in defs.breakables.iter().enumerate() {
                if d.battery.is_some() {
                    continue;
                }
                let dmg = explosion_damage(ex, d.position.distance(*position), false).round() as i32;
                if dmg > 0 {
                    self.breakables[i].take_damage(d, dmg, &ex.radial_damage_type, Some(*owner));
                }
            }
        }
        for (i, d) in defs.breakables.iter().enumerate() {
            if d.battery.is_some() {
                continue;
            }
            let Some(step) = self.breakables[i].tick(d, dt) else { continue };
            out.push(WorldEvent::Broken { breakable: i, output: BreakableState::output(d, step) });
            let s = &d.steps[step];
            if s.sound && s.noise_radius > 0.0 {
                out.push(WorldEvent::Noise(SimEvent::Sound { position: d.position, radius: s.noise_radius, category: SoundCategory::Breakable, context: s.noise_context, source: self.breakables[i].instigator.unwrap_or(0) }));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::world_actors::{BatteryDef, BreakStepDef, ReceptacleDef};

    fn plank() -> BreakableDef {
        // movable_twk.wood_plank_04_twk: health 30, threshold 1, steps 20 / 0.
        BreakableDef { name: "Plank".into(), health: 30, damage_threshold: 1, steps: vec![BreakStepDef { health_threshold: 20, ..Default::default() }, BreakStepDef { health_threshold: 0, ..Default::default() }], ..Default::default() }
    }

    #[test]
    fn damage_below_the_threshold_is_ignored_and_steps_follow_the_health() {
        let mut d = plank();
        d.damage_threshold = 5;
        let mut s = BreakableState::new(&d);
        s.take_damage(&d, 4, "", None);
        assert_eq!(s.cur_health, 30);
        s.take_damage(&d, 10, "", None);
        assert_eq!((s.cur_health, s.wanted), (20, Some(0)));
        assert_eq!(s.tick(&d, 0.1), Some(0));
        assert_eq!(BreakableState::output(&d, 0), 1, "first of two steps is output 1 (66%)");
        assert_eq!(s.tick(&d, 0.1), None, "a step is applied once");
        s.take_damage(&d, 50, "", None);
        assert_eq!((s.cur_health, s.wanted), (0, Some(1)));
        assert_eq!(s.tick(&d, 0.1), Some(1));
        assert_eq!(BreakableState::output(&d, 1), 0, "the last step is output 0 (100%)");
    }

    #[test]
    fn a_big_hit_jumps_to_the_last_step_reached() {
        let d = plank();
        let mut s = BreakableState::new(&d);
        s.take_damage(&d, 30, "", None);
        assert_eq!(s.tick(&d, 0.1), Some(1), "health 0: the last step (scanned from the top) only");
        assert_eq!(s.next_step, 2);
    }

    #[test]
    fn immune_types_unbreakable_health_and_instant_break() {
        let mut d = plank();
        d.immune = vec!["DisDamageType_Grenade".into()];
        d.instant_break = vec!["DisDamageType_Sword".into()];
        let mut s = BreakableState::new(&d);
        s.take_damage(&d, 100, "DisDamageType_Grenade", None);
        assert_eq!(s.cur_health, 30);
        s.take_damage(&d, 1, "DisDamageType_Sword", None);
        assert_eq!((s.cur_health, s.wanted), (0, Some(1)));
        let mut u = plank();
        u.health = -1;
        let mut us = BreakableState::new(&u);
        us.take_damage(&u, 1000, "", None);
        assert_eq!((us.cur_health, us.wanted), (-1, None));
    }

    #[test]
    fn step_timer_breaks_the_next_step_and_damage_hurries_it() {
        let mut d = plank();
        d.steps[0].step_timer = 2.0;
        let mut s = BreakableState::new(&d);
        s.take_damage(&d, 10, "", None);
        assert_eq!(s.tick(&d, 0.5), Some(0));
        assert!((s.step_timer - 1.5).abs() < 1e-6);
        s.take_damage(&d, 1, "", None);
        assert_eq!(s.step_timer, 0.01, "positive damage cuts the timer");
        assert_eq!(s.tick(&d, 0.5), None);
        assert_eq!((s.cur_health, s.wanted), (0, Some(1)));
        assert_eq!(s.tick(&d, 0.5), Some(1));
    }

    fn battery_defs(allow_empty: bool) -> WorldActors {
        let bat = BreakableDef { name: "Bat".into(), health: 10, battery: Some(BatteryDef { max_charges: 50, initial_charges: 50, pawn_cost: 5, animal_cost: 1, watchtower_cost: 1, ..Default::default() }), ..Default::default() };
        WorldActors { breakables: vec![bat], receptacles: vec![ReceptacleDef { name: "Rec".into(), battery: Some("Bat".into()), allow_empty, ..Default::default() }] }
    }

    #[test]
    fn placed_battery_plugs_silently_and_charges_drain_to_ejection() {
        let defs = battery_defs(false);
        let mut s = WorldActorsState::new(&defs);
        assert_eq!(s.breakables[0].cur_health, 10, "a charged battery has its tweak health");
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
        let cfg = crate::SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap().gameplay;
        let ev = s.step(&defs, &[], &cfg, 1.0 / 60.0);
        assert!(ev.is_empty(), "the first-tick plug is not notified: {ev:?}");
        assert!(s.powered(0));
        let mut used = 0;
        loop {
            let (ok, ev) = s.use_charge(&defs, 0, ChargeCost::Pawn);
            if !ok {
                break;
            }
            used += 1;
            if s.batteries[&0].charges < 1 {
                assert_eq!(ev, vec![WorldEvent::Battery { battery: 0, output: 1 }, WorldEvent::Receptacle { receptacle: 0, battery: 0, output: 1 }]);
            } else {
                assert_eq!(ev, vec![WorldEvent::Battery { battery: 0, output: 0 }]);
            }
        }
        assert_eq!(used, 10, "50 charges at 5 per pawn");
        assert!(!s.powered(0));
        assert_eq!(s.breakables[0].cur_health, -1, "an empty battery cannot be damaged");
        // Refill (`DisSeqAct_RefillWhaleOilBattery`, not notified) and plug back (Kismet, notified).
        let ev = s.set_charges(&defs, 0, (50.0f32 * 0.01 * 50.0) as i32, false);
        assert!(ev.is_empty());
        assert_eq!(s.breakables[0].cur_health, 10);
        let (ok, ev) = s.plug(&defs, 0, 0, true);
        assert!(ok);
        assert_eq!(ev, vec![WorldEvent::Receptacle { receptacle: 0, battery: 0, output: 0 }]);
    }

    #[test]
    fn recharging_receptacle_takes_empty_batteries_and_keeps_them() {
        let mut defs = battery_defs(true);
        defs.breakables[0].battery.as_mut().unwrap().initial_charges = 0;
        let mut s = WorldActorsState::new(&defs);
        let (ok, _) = s.plug(&defs, 0, 0, true);
        assert!(ok, "an empty battery goes in a receptacle that allows empty ones");
        assert!(s.batteries[&0].recharging);
        let ev = s.set_charges(&defs, 0, 0, true);
        assert_eq!(ev, vec![WorldEvent::Battery { battery: 0, output: 1 }], "a recharging battery is not ejected");
        let mut full = battery_defs(true);
        full.receptacles[0].battery = None;
        let mut f = WorldActorsState::new(&full);
        assert!(!f.plug(&full, 0, 0, true).0, "a charged battery is refused by a recharger");
    }
}
