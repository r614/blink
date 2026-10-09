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
use crate::TripwireView;
use dishonored_sim::input::PlayerInput;
use dishonored_sim::interactables::{InteractEvent, InteractablesState};
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
