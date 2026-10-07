//! Headless scenario runner on the port's simulation (`dishonored_sim::step_world` against
//! the map's collision world), fixed step, no presentation. Systems the runner does not drive
//! yet are reported as such, never skipped silently.

use anyhow::{Result, bail};
use dishonored_oracle::scenario::{Button, Record, Scenario, System};
use dishonored_oracle::trace::{Trace, TraceEvent};
use dishonored_sim::input::{PlayerInput, buttons};
use dishonored_sim::state::WorldState;
use dishonored_sim::{SimConfig, spawn_player, step_world};
use dishonored_world::{CollisionWorld, WorldQuery, channel};
use glam::Vec3;

/// Systems this runner can play.
pub fn supports(s: System) -> bool {
    matches!(s, System::Movement)
}

fn button_bits(b: &[Button]) -> u16 {
    b.iter()
        .map(|b| match b {
            Button::Jump => buttons::JUMP,
            Button::Crouch => buttons::CROUCH,
            Button::Sprint => buttons::SPRINT,
            Button::Walk => buttons::WALK,
            Button::Primary => buttons::PRIMARY,
            Button::Secondary => buttons::SECONDARY,
            Button::Block => buttons::BLOCK,
            Button::LeanLeft => buttons::LEAN_LEFT,
            Button::LeanRight => buttons::LEAN_RIGHT,
            Button::Use => buttons::USE,
            Button::Reload => buttons::RELOAD,
        })
        .fold(0, |a, b| a | b)
}

/// Play a scenario on the port and record its trace.
pub fn run(s: &Scenario, world: &CollisionWorld, cfg: &SimConfig) -> Result<Trace> {
    if !supports(s.system) {
        bail!("the headless runner does not drive {:?} scenarios yet", s.system);
    }
    if s.setup.kismet || !s.setup.npcs.is_empty() || !s.setup.powers.is_empty() || !s.setup.items.is_empty() {
        bail!("{}: the headless runner does not set up Kismet, NPCs, powers or items yet", s.name);
    }
    let Some(ps) = &s.setup.player else { bail!("{}: no player setup", s.name) };
    let mv = &cfg.gameplay.movement;
    let mut pos = Vec3::from(ps.position);
    if ps.snap_to_floor {
        if let Some(h) = world.raycast(pos + Vec3::Z * 100.0, -Vec3::Z, 2000.0, channel::PAWN) {
            pos = h.point + Vec3::Z * (mv.collision_half_height + 1.0);
        }
    }
    let mut state = WorldState::default();
    let id = 1;
    spawn_player(&mut state, id, pos, ps.yaw, cfg);
    let dt_hz = s.tick_hz;
    let want_vitals = s.record.contains(&Record::Vitals);
    let mut chans: Vec<(&str, &str)> = vec![("pawn.x", "uu"), ("pawn.y", "uu"), ("pawn.z", "uu"), ("pawn.vx", "uu/s"), ("pawn.vy", "uu/s"), ("pawn.vz", "uu/s"), ("pawn.speed2d", "uu/s"), ("pawn.crouched", "bool")];
    if want_vitals {
        chans.extend([("pawn.health", "pt"), ("pawn.mana", "pt")]);
    }
    let mut trace = Trace::new(&s.name, "port", dt_hz, &chans);
    let timeline = s.input_timeline();
    let sample = |state: &WorldState| -> Vec<f64> {
        let p = state.player(id).unwrap();
        let mut v = vec![p.position.x as f64, p.position.y as f64, p.position.z as f64, p.velocity.x as f64, p.velocity.y as f64, p.velocity.z as f64, p.velocity.truncate().length() as f64, p.crouched as u8 as f64];
        if want_vitals {
            v.extend([p.vitals.health as f64, p.vitals.mana as f64]);
        }
        v
    };
    trace.push(0, sample(&state));
    for (t, (fw, rt, yaw, pitch, held)) in timeline.iter().enumerate() {
        let input = PlayerInput { tick: state.tick, forward: *fw, right: *rt, yaw: *yaw, pitch: *pitch, buttons: button_bits(held), ..Default::default() };
        // `keep_alive` is not applied on the port side (the sim has no minimum scripted health
        // yet): a death shows up in the trace as it happens.
        let out = step_world(&mut state, &[(id, input)], world, cfg);
        let tick = t as u32 + 1;
        trace.push(tick, sample(&state));
        if s.record.contains(&Record::SimEvents) {
            for e in out.events {
                let name = format!("{e:?}");
                let name = name.split([' ', '{', '(']).next().unwrap_or("").to_string();
                trace.events.push(TraceEvent { tick, kind: "Sim".into(), object: "player".into(), name, detail: String::new() });
            }
        }
    }
    Ok(trace)
}
