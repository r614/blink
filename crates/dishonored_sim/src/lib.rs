//! Engine-independent, fixed-step Dishonored gameplay simulation.
//!
//! ```text
//! PlayerInput -> step_world -> WorldState (+ events) -> snapshot
//! ```
//! The same code runs on the dedicated server, in client prediction, in replays and in
//! the test harness. Nothing here depends on Bevy.

pub mod blink;
pub mod combat;
pub mod events;
pub mod input;
pub mod left_hand;
pub mod movement;
pub mod state;
pub mod tuning;
pub mod vitals;

use dishonored_ir::gameplay::GameplayConfig;
use dishonored_world::WorldQuery;
use events::{DamageCause, SimEvent};
use glam::Vec3;
use input::PlayerInput;
use movement::{DT, MoveCtx};
use state::{MoveMode, PlayerId, PlayerState, WorldState};
use tuning::CombatTuning;

/// Everything constant during a match.
#[derive(Debug, Clone)]
pub struct SimConfig {
    pub gameplay: GameplayConfig,
    pub tuning: CombatTuning,
}

impl SimConfig {
    pub fn new(gameplay: GameplayConfig) -> Self {
        let tuning = CombatTuning::from_config(&gameplay);
        Self { gameplay, tuning }
    }

    /// Load the config JSON extracted from the install by `gameplay_config`.
    pub fn from_json(text: &str) -> Result<Self, serde_json_error::Error> {
        serde_json_error::from_str(text).map(Self::new)
    }
}

/// Minimal JSON shim so the sim crate does not depend on serde_json at runtime APIs.
pub mod serde_json_error {
    pub use serde_json::Error;
    pub fn from_str<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, Error> {
        serde_json::from_str(s)
    }
}

#[derive(Debug, Default, Clone)]
pub struct StepOutput {
    pub events: Vec<SimEvent>,
}

/// Create or reset a player at a spawn point.
pub fn spawn_player(state: &mut WorldState, id: PlayerId, position: Vec3, yaw: f32, cfg: &SimConfig) {
    let mut p = PlayerState::new(id, position, yaw);
    vitals::reset(&mut p, &cfg.gameplay.vitals);
    left_hand::reset_loadout(&mut p, &cfg.tuning);
    p.last_input.yaw = yaw;
    if let Some(existing) = state.player_mut(id) {
        let prev = existing.prev_buttons;
        *existing = p;
        existing.prev_buttons = prev;
    } else {
        state.players.push(p);
        state.players.sort_by_key(|p| p.id);
    }
}

pub fn remove_player(state: &mut WorldState, id: PlayerId) {
    state.players.retain(|p| p.id != id);
}

/// Advance the world by one fixed tick. `inputs` may omit players; missing players repeat
/// their last input (with buttons released except held movement), as a server does when
/// a packet is late.
pub fn step_world(
    state: &mut WorldState,
    inputs: &[(PlayerId, PlayerInput)],
    world: &dyn WorldQuery,
    cfg: &SimConfig,
) -> StepOutput {
    let mut out = StepOutput::default();
    let mv = &cfg.gameplay.movement;
    let ctx = MoveCtx { world, cfg: mv, dt: DT };
    let mut spawns = Vec::new();

    for p in state.players.iter_mut() {
        let input = inputs
            .iter()
            .find(|(id, _)| *id == p.id)
            .map(|(_, i)| *i)
            .unwrap_or(PlayerInput { tick: state.tick, ..p.last_input });
        if p.alive {
            let locked = combat::sword_locks_movement(p) || p.mode == MoveMode::Blinking;
            let sword_out = true;
            movement::step_movement(p, &input, sword_out, !locked, &ctx, &mut out.events);
            combat::update_sword(p, &input, &cfg.tuning, DT, &mut out.events);
            let mut local = Vec::new();
            left_hand::update(p, &input, &ctx, &cfg.gameplay, &cfg.tuning, &mut local, &mut out.events);
            spawns.extend(local.into_iter().map(|(k, pos, vel)| (p.id, k, pos, vel)));
            vitals::tick(p, &cfg.gameplay.vitals, DT);
        } else {
            p.yaw = input.yaw;
            p.pitch = input.pitch;
        }
        p.prev_buttons = input.buttons;
        p.last_input = input;
    }

    for (owner, kind, pos, vel) in spawns {
        let id = state.next_projectile_id;
        state.next_projectile_id = state.next_projectile_id.wrapping_add(1);
        state.projectiles.push(left_hand::make_projectile(id, owner, kind, pos, vel, &cfg.tuning));
        out.events.push(SimEvent::Fired { id: owner, bolt: kind == state::ProjectileKind::Bolt, projectile: id });
    }

    combat::resolve_sword_hits(state, world, mv, &cfg.gameplay.vitals, &cfg.tuning, &mut out.events);

    let casters: Vec<u32> = out
        .events
        .iter()
        .filter_map(|e| match e {
            SimEvent::WindblastCast { id } => Some(*id),
            _ => None,
        })
        .collect();
    if !casters.is_empty() {
        left_hand::resolve_windblasts(state, world, mv, &cfg.tuning, &casters, &mut out.events);
    }

    left_hand::step_projectiles(state, world, mv, &cfg.tuning, mv.gravity_z, DT, &mut out.events);

    // Environmental damage from this tick's movement events.
    let env: Vec<SimEvent> = out.events.clone();
    for e in env {
        match e {
            SimEvent::FellOutOfWorld { id } => {
                if let Some(p) = state.player_mut(id) {
                    vitals::kill(p, None, DamageCause::World, &mut out.events);
                }
            }
            SimEvent::Landed { id, speed, .. } if speed > mv.max_speed_before_falling_damage => {
                if let Some(p) = state.player_mut(id) {
                    if speed >= mv.max_speed_before_falling_death {
                        vitals::kill(p, None, DamageCause::Fall, &mut out.events);
                    } else {
                        let f = (speed - mv.max_speed_before_falling_damage)
                            / (mv.max_speed_before_falling_death - mv.max_speed_before_falling_damage);
                        let dmg = f * cfg.gameplay.vitals.health_max;
                        vitals::apply_damage(p, dmg, None, DamageCause::Fall, &mut out.events);
                    }
                }
            }
            _ => {}
        }
    }

    state.tick += 1;
    out
}

/// Order-stable hash of the authoritative state (desync detection, replay verification).
pub fn state_hash(state: &WorldState) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |x: u64| {
        h ^= x;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix(state.tick);
    for p in &state.players {
        mix(p.id as u64);
        for f in [
            p.position.x, p.position.y, p.position.z, p.velocity.x, p.velocity.y, p.velocity.z,
            p.vitals.health, p.vitals.mana, p.yaw, p.pitch, p.sword.timer, p.left.timer,
        ] {
            mix(f.to_bits() as u64);
        }
        mix(p.mode as u64);
        mix(p.sword.phase as u64);
        mix(p.left.phase as u64);
        mix(p.alive as u64);
        mix(p.crouched as u64);
    }
    for pr in &state.projectiles {
        mix(pr.id as u64);
        mix(pr.position.x.to_bits() as u64);
        mix(pr.position.y.to_bits() as u64);
        mix(pr.position.z.to_bits() as u64);
    }
    h
}

/// Spawn (or respawn) an AI pawn. Mechanically identical to a player.
pub fn spawn_npc(state: &mut WorldState, id: PlayerId, kind: state::PawnKind, archetype: u8, position: Vec3, yaw: f32, cfg: &SimConfig) {
    spawn_player(state, id, position, yaw, cfg);
    if let Some(p) = state.player_mut(id) {
        p.kind = kind;
        p.archetype = archetype;
    }
}
