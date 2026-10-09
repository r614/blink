//! Keyholes, tripwires and the generic Interact event in the mission: the host side of
//! `dishonored_sim::interactables` (research/dishonored/systems/interactables.md).
//!
//! - Before the world steps, a peeking player's input is taken (movement and buttons off, the
//!   entry view set).
//! - After it, the tripwires are touched or damaged by the step's pawns, projectiles and
//!   explosions, and the holds and peeks advance on the inputs the world stepped.
//! - What happened is queued for the Kismet host (`kismet_host::interactables`), and the
//!   client-facing view (`MatchView::peeks`, `MatchView::tripwires`) is filled.

use crate::mission::MissionMatch;
use crate::{LauncherView, TripwireView};
use dishonored_sim::input::PlayerInput;
use dishonored_sim::interactables::{InteractEvent, InteractablesState};
use dishonored_sim::launchers::LaunchersState;
use dishonored_world::WorldQuery;
use dishonored_sim::state::PlayerId;
use dishonored_sim::SimConfig;
use dishonored_sim::events::SimEvent;
use glam::Vec3;

impl MissionMatch {
    fn interactables_key(&self) -> String {
        self.current_map.to_ascii_lowercase()
    }

    /// Whether the door of keyhole `i` of the active map is shut: the Kismet's state for it
    /// (`DisSeqAct_Door`), else the level's (`m_InitialDoorState`).
    fn keyhole_closed(&self, defs: &dishonored_ir::interactables::Interactables, hidden: &[String]) -> Vec<bool> {
        defs.keyholes
            .iter()
            .map(|k| {
                // A door of a Kismet-streamed sublevel that is not in the world is not there.
                if hidden.iter().any(|h| h.eq_ignore_ascii_case(&k.package)) {
                    return false;
                }
                let state = self.kismet.as_ref().and_then(|run| run.door_state(&format!("{}:{}", k.package, k.name)));
                state.unwrap_or(k.initial_state) == 0
            })
            .collect()
    }

    /// The player has the eye at a keyhole.
    pub(crate) fn is_peeking(&self, id: PlayerId) -> bool {
        self.interactables.get(&self.interactables_key()).is_some_and(|s| s.is_peeking(id))
    }

    /// The peeking players' inputs, before the world steps them.
    pub(crate) fn lock_peeking_inputs(&mut self, inputs: &mut [(PlayerId, PlayerInput)]) {
        let key = self.interactables_key();
        let Some(st) = self.interactables.get_mut(&key) else { return };
        for (id, input) in inputs.iter_mut() {
            st.lock_input(*id, input);
        }
    }

    /// Tripwires and keyholes after the world stepped (`events` are the step's).
    pub(crate) fn step_interactables(&mut self, inputs: &[(PlayerId, PlayerInput)], events: &[SimEvent], cfg: &SimConfig, dt: f32) {
        let key = self.interactables_key();
        let Some(defs) = self.setup.interactables.get(&key).cloned() else {
            self.view.peeks.clear();
            self.view.tripwires.clear();
            return;
        };
        if defs.keyholes.is_empty() && defs.tripwires.is_empty() {
            return;
        }
        let hidden = self.hidden_levels();
        let in_world = |package: &str| !hidden.iter().any(|h| h.eq_ignore_ascii_case(package));
        let closed = self.keyhole_closed(&defs, &hidden);
        let mv = &cfg.gameplay.movement;
        let mut st = self.interactables.remove(&key).unwrap_or_else(|| InteractablesState::new(&defs));
        let mut out = st.step_tripwires(
            &defs,
            &self.world.players,
            &self.world.projectiles,
            events,
            |i| defs.tripwires.get(i).is_some_and(|w| in_world(&w.package)),
            |p| {
                let c = dishonored_sim::movement::capsule(p, mv);
                (c.radius, c.half_height)
            },
            |sticky, dist| {
                let kind = if sticky { dishonored_sim::state::ProjectileKind::StickyGrenade } else { dishonored_sim::state::ProjectileKind::Grenade };
                let ex = &dishonored_sim::gadgets::grenade_cfg(&cfg.gameplay, kind).explosion;
                (dishonored_sim::gadgets::explosion_damage(ex, dist, false), ex.radial_damage_type.clone())
            },
            dt,
        );
        let reach = cfg.gameplay.items.inventory.interact_distance;
        out.extend(st.step_peeks(&defs, &self.world.players, inputs, |i| closed.get(i).copied().unwrap_or(false), |p| dishonored_sim::movement::eye_position(p, mv), reach, dt));
        self.view.peeks = st
            .peeks
            .keys()
            .copied()
            .filter_map(|id| {
                let p = self.world.player(id)?;
                let magnification = dishonored_sim::powers::attribute(p, &cfg.gameplay, "Attribute_KeyholePeepingMagnification", 0.0);
                st.view(&defs, id, p, magnification).map(|v| (id, v))
            })
            .collect();
        self.view.tripwires = defs
            .tripwires
            .iter()
            .enumerate()
            .filter(|(_, w)| in_world(&w.package))
            .map(|(i, w)| {
                let (a, b) = wire_ends(&w.triangles);
                TripwireView { actor: format!("{}:{}", w.package, w.name), a: a.to_array(), b: b.to_array(), tripped: st.is_tripped(i) }
            })
            .collect();
        self.interactables.insert(key, st);
        self.interact_events.extend(out);
    }

    /// The Kismet host fires the queued events (called from its step).
    pub(crate) fn take_interact_events(&mut self) -> Vec<InteractEvent> {
        std::mem::take(&mut self.interact_events)
    }

    /// `DisSeqAct_ActivateProjectileLauncher` -> `Fire` (0x00A33F70) on the launcher `name` of
    /// `package` in the active map, with the action's `m_pProjInstigator` (`owner`, 0 for none).
    /// Returns whether a shot left (the launcher exists and was armed).
    pub fn fire_launcher(&mut self, package: &str, name: &str, owner: PlayerId) -> bool {
        let key = self.interactables_key();
        let Some(defs) = self.setup.interactables.get(&key) else { return false };
        let Some(i) = LaunchersState::find(defs, Some(package), name).or_else(|| LaunchersState::find(defs, None, name)) else { return false };
        let st = self.launchers.entry(key).or_insert_with(|| LaunchersState::new(defs));
        st.fire(defs, i, owner, &mut self.world, &mut self.launcher_events)
    }

    /// `SeqAct_Toggle` on a launcher (`OnToggle_Native`, 0x009ED800): input 0 arms, 1 disarms, 2
    /// flips. Returns whether the actor is a launcher of the map.
    pub fn toggle_launcher(&mut self, package: &str, name: &str, input: usize) -> bool {
        let key = self.interactables_key();
        let Some(defs) = self.setup.interactables.get(&key) else { return false };
        let Some(i) = LaunchersState::find(defs, Some(package), name).or_else(|| LaunchersState::find(defs, None, name)) else { return false };
        self.launchers.entry(key).or_insert_with(|| LaunchersState::new(defs)).toggle(defs, i, input);
        true
    }

    /// Whether launcher `name` of the active map is armed (`m_bArmed`).
    pub fn launcher_armed(&self, package: &str, name: &str) -> Option<bool> {
        let key = self.interactables_key();
        let defs = self.setup.interactables.get(&key)?;
        let i = LaunchersState::find(defs, Some(package), name).or_else(|| LaunchersState::find(defs, None, name))?;
        Some(self.launchers.get(&key).and_then(|s| s.launchers.get(i)).map_or(defs.launchers[i].armed, |l| l.armed))
    }

    /// The launchers' shots after the world stepped, and the clients' view of the launchers.
    pub(crate) fn step_launchers(&mut self, world: &dyn WorldQuery, cfg: &SimConfig, events: &mut Vec<SimEvent>) {
        events.append(&mut self.launcher_events);
        let key = self.interactables_key();
        let hidden = self.hidden_levels();
        let Some(defs) = self.setup.interactables.get(&key) else {
            self.view.launchers.clear();
            return;
        };
        if defs.launchers.is_empty() {
            self.view.launchers.clear();
            return;
        }
        let st = self.launchers.entry(key).or_insert_with(|| LaunchersState::new(defs));
        if st.launchers.len() != defs.launchers.len() {
            *st = LaunchersState::new(defs);
        }
        // Projectiles fly at the world's projectile time scale (Bend Time slows them).
        let dt = dishonored_sim::movement::DT * dishonored_sim::powers::world_time_scale(&self.world, &cfg.gameplay, true);
        if dt > 0.0 {
            st.step_shots(&mut self.world, defs, world, &cfg.gameplay, dt, events);
        }
        self.view.launchers = defs
            .launchers
            .iter()
            .zip(&st.launchers)
            .filter(|(l, _)| !hidden.iter().any(|h| h.eq_ignore_ascii_case(&l.package)))
            .map(|(l, s)| LauncherView {
                actor: format!("{}:{}", l.package, l.name),
                position: l.position.to_array(),
                bounds_min: l.bounds_min.to_array(),
                bounds_max: l.bounds_max.to_array(),
                max_distance: l.max_distance,
                armed: s.armed,
                harvested: s.harvested,
                fired: s.fired,
                defused: s.defused,
                prompt: l.prompt.clone(),
                label: l.label.clone(),
            })
            .collect();
    }

    /// The player pressed Use at launcher `i` of the active map: `Interact` (0x00A3B6E0), then
    /// the generic wrapper's (0x00A3DF60) `DisSeqEvent_Interact` and use message.
    pub(crate) fn interact_launcher(&mut self, player: PlayerId, i: usize, cfg: &SimConfig) {
        let key = self.interactables_key();
        let Some(defs) = self.setup.interactables.get(&key) else { return };
        let Some(def) = defs.launchers.get(i) else { return };
        // `m_pItemRequiredToHarvestAmmo`: held in the mission's abstract items.
        let has_item = def.required_item.is_empty() || self.state.items.get(&def.required_item).copied().unwrap_or(0) > 0;
        let st = self.launchers.entry(key).or_insert_with(|| LaunchersState::new(defs));
        let Some(p) = self.world.player_mut(player) else { return };
        if !p.alive {
            return;
        }
        let r = st.interact(defs, i, p, has_item, &cfg.gameplay);
        let mut lines = Vec::new();
        if r.used && !def.use_message.is_empty() {
            lines.push(def.use_message.clone());
        }
        if r.cannot_use_message && !def.cannot_use_message.is_empty() {
            lines.push(def.cannot_use_message.clone());
        }
        if r.used {
            self.interact_events.push(InteractEvent::Launcher { launcher: i, player });
        }
        for l in lines {
            let _ = self.state.apply(&[dishonored_mission::Action::Message(l)], &std::collections::BTreeMap::new());
        }
    }
}

/// The ends of a wire's bodies: the two vertices farthest apart.
pub fn wire_ends(triangles: &[[Vec3; 3]]) -> (Vec3, Vec3) {
    let vs: Vec<Vec3> = triangles.iter().flatten().copied().collect();
    let mut best = (Vec3::ZERO, Vec3::ZERO, -1.0f32);
    for (i, a) in vs.iter().enumerate() {
        for b in &vs[i + 1..] {
            let d = a.distance_squared(*b);
            if d > best.2 {
                best = (*a, *b, d);
            }
        }
    }
    (best.0, best.1)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission::{MissionSetup, MissionMatch};
    use dishonored_ir::interactables::{Interactables, KeyholeDef, TripwireDef};
    use dishonored_sim::input::buttons;

    fn rig(defs: Interactables) -> (MissionMatch, SimConfig) {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/config/gameplay_config.json");
        let cfg = SimConfig::from_json(&std::fs::read_to_string(path).unwrap()).unwrap();
        let setup = MissionSetup { starts: vec![(Vec3::ZERO, 0.0)], start_map: "T".into(), ..Default::default() };
        let mut m = MissionMatch::new(setup, &cfg);
        m.add_player(1, "p", 0, &cfg);
        m.setup.interactables.insert("t".into(), defs);
        (m, cfg)
    }

    fn door() -> KeyholeDef {
        KeyholeDef { name: "D".into(), package: "T".into(), socket: Vec3::new(100.0, 0.0, 60.0), facing: Vec3::X, bounds_min: Vec3::new(95.0, -60.0, -50.0), bounds_max: Vec3::new(105.0, 60.0, 250.0), fov: 75.0, ..Default::default() }
    }

    #[test]
    fn holding_use_at_a_shut_door_peeks_and_takes_the_players_inputs() {
        let (mut m, cfg) = rig(Interactables { keyholes: vec![door()], alt_interact_time: 0.3, ..Default::default() });
        let dt = dishonored_sim::movement::DT;
        let hold = [(1u32, PlayerInput { buttons: buttons::USE, ..Default::default() })];
        for _ in 0..40 {
            m.step_interactables(&hold, &[], &cfg, dt);
        }
        let ev = m.take_interact_events();
        assert!(ev.iter().any(|e| matches!(e, InteractEvent::KeyHole { used: true, player: 1, .. })), "{ev:?}");
        assert_eq!(m.view.peeks.len(), 1);
        let mut input = [(1u32, PlayerInput { forward: 127, buttons: buttons::PRIMARY | buttons::USE, yaw: 90.0, ..Default::default() })];
        m.lock_peeking_inputs(&mut input);
        assert_eq!((input[0].1.forward, input[0].1.buttons, input[0].1.yaw), (0, buttons::USE, 0.0), "movement and buttons taken, the keyhole view handed over");
        // A saved game keeps the peek.
        let st = m.interactables.get("t").unwrap().clone();
        let back: dishonored_sim::interactables::InteractablesState = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert_eq!(back, st);
    }

    fn launcher() -> dishonored_ir::interactables::LauncherDef {
        use dishonored_ir::interactables::{LauncherDef, LauncherProjectile, ShotKind};
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
            projectile: LauncherProjectile { kind: ShotKind::ExplosiveArrow, speed: 2000.0, damage: 10, headshot_mult: 1.0, damage_multiplier: 1.0, gravity_mult: 0.5, explosion: Some(Default::default()), ..Default::default() },
            harvest: vec![(4, 1)],
            prompt: "`GBA_Use` Disarm".into(),
            label: "Projectile Launcher".into(),
            use_message: "Launcher disarmed".into(),
            max_distance: 200.0,
            ..Default::default()
        }
    }

    #[test]
    fn a_launcher_in_view_is_the_interaction_and_its_view_follows_its_state() {
        use crate::mission::Interaction;
        let (mut m, cfg) = rig(Interactables { launchers: vec![launcher()], ..Default::default() });
        let world = dishonored_world::CollisionWorld::from_triangles(Vec::new(), Vec::new());
        // The crosshair is chosen by each step's camera update (`dishonored_sim::crosshair`).
        let focus = |m: &mut crate::mission::MissionMatch, at: Vec3, yaw: f32| {
            let p = m.world.player_mut(1).unwrap();
            p.position = at;
            p.velocity = Vec3::ZERO;
            m.step(&[(1, dishonored_sim::input::PlayerInput { yaw, ..Default::default() })], &world, &cfg);
            m.interaction(1)
        };
        assert_eq!(focus(&mut m, Vec3::new(-120.0, 0.0, 70.0), 0.0), Some(Interaction::Launcher(0)), "the view enters the tripod's bounds within m_fMaxDistance");
        assert_eq!(focus(&mut m, Vec3::new(-400.0, 0.0, 70.0), 0.0), None, "out of reach");
        assert_eq!(focus(&mut m, Vec3::new(-120.0, 0.0, 70.0), 180.0), None, "looking away");
        m.world.player_mut(1).unwrap().position = Vec3::new(-120.0, 0.0, 70.0);
        let mut events = Vec::new();
        m.step_launchers(&world, &cfg, &mut events);
        assert_eq!((m.view.launchers.len(), m.view.launchers[0].armed, m.view.launchers[0].harvested), (1, true, false));
        assert_eq!(m.view.launchers[0].prompt, "`GBA_Use` Disarm");
        // Use: disarmed, the bolt taken, the use message queued for the HUD, the Interact event for the Kismet.
        m.interact_launcher(1, 0, &cfg);
        m.step_launchers(&world, &cfg, &mut events);
        assert_eq!((m.view.launchers[0].armed, m.view.launchers[0].harvested, m.view.launchers[0].defused), (false, true, true));
        assert_eq!(m.take_interact_events(), vec![InteractEvent::Launcher { launcher: 0, player: 1 }]);
        assert!(!m.fire_launcher("T", "DisProjectileLauncher_0", 0), "a disarmed launcher no longer fires");
        // A saved game keeps the launchers (and the shots in flight).
        let st = m.launchers.get("t").unwrap().clone();
        let back: dishonored_sim::launchers::LaunchersState = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert_eq!(back, st);
    }

    #[test]
    fn a_fired_shot_flies_in_the_mission_step_and_the_burst_reaches_the_events() {
        let (mut m, cfg) = rig(Interactables { launchers: vec![launcher()], ..Default::default() });
        let (a, b, c, d) = (Vec3::new(900.0, -2000.0, -2000.0), Vec3::new(900.0, 2000.0, -2000.0), Vec3::new(900.0, 2000.0, 2000.0), Vec3::new(900.0, -2000.0, 2000.0));
        let n = -Vec3::X;
        let tri = |a, b, c| dishonored_world::Tri { a, b, c, n, channels: dishonored_world::channel::ALL, source: 0 };
        let world = dishonored_world::CollisionWorld::from_triangles(vec![tri(a, b, c), tri(a, c, d)], vec![String::new()]);
        m.world.player_mut(1).unwrap().position = Vec3::new(800.0, 700.0, 130.0);
        assert!(m.fire_launcher("T", "DisProjectileLauncher_0", 0));
        let mut events = Vec::new();
        for _ in 0..60 {
            m.step_launchers(&world, &cfg, &mut events);
        }
        assert!(events.iter().any(|e| matches!(e, dishonored_sim::events::SimEvent::Fired { .. })), "the Kismet-fired shot is reported with the first step");
        assert!(events.iter().any(|e| matches!(e, dishonored_sim::events::SimEvent::Explosion { .. })), "it bursts at the wall");
        assert!(m.world.projectiles.is_empty());
        assert!(m.launchers["t"].shots.is_empty());
    }

    #[test]
    fn a_pawn_on_the_wire_trips_it_once_and_the_view_shows_it_snapped() {
        let a = Vec3::new(-100.0, 0.0, 40.0);
        let b = Vec3::new(100.0, 0.0, 40.0);
        let w = Vec3::new(0.0, 3.0, 0.0);
        let wire = TripwireDef { name: "W".into(), package: "T".into(), triangles: vec![[a - w, b - w, b + w], [a - w, b + w, a + w]], ..Default::default() };
        let (mut m, cfg) = rig(Interactables { tripwires: vec![wire], ..Default::default() });
        let dt = dishonored_sim::movement::DT;
        m.world.player_mut(1).unwrap().position = Vec3::new(0.0, 0.0, 100.0);
        m.step_interactables(&[], &[], &cfg, dt);
        m.step_interactables(&[], &[], &cfg, dt);
        let ev = m.take_interact_events();
        assert_eq!(ev, vec![InteractEvent::Tripwire { tripwire: 0, instigator: 1 }], "once");
        assert_eq!(m.view.tripwires.len(), 1);
        assert!(m.view.tripwires[0].tripped);
        let (e0, e1) = wire_ends(&[[a, b, a]]);
        assert!(e0.distance(e1) > 199.0);
    }
}
