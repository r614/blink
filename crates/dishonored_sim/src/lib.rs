//! Engine-independent, fixed-step Dishonored gameplay simulation.
//!
//! ```text
//! PlayerInput -> step_world -> WorldState (+ events) -> snapshot
//! ```
//! The same code runs on the dedicated server, in client prediction, in replays and in
//! the test harness. Nothing here depends on Bevy.

pub mod adrenaline;
pub mod blink;
pub mod climb;
pub mod combat;
pub mod events;
pub mod gadgets;
pub mod input;
pub mod inventory;
pub mod left_hand;
pub mod movement;
pub mod powers;
pub mod rats;
pub mod state;
pub mod swim;
pub mod takedown;
pub mod tuning;
pub mod vitals;
pub mod volumes;

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
    let g = &cfg.gameplay;
    let mv = &g.movement;
    let mut spawns = Vec::new();
    let mut actions: Vec<(usize, left_hand::LeftAction)> = Vec::new();

    // Inputs (late packets repeat the last input without its one-shot action).
    let mut ins: Vec<PlayerInput> = state
        .players
        .iter()
        .map(|p| inputs.iter().find(|(id, _)| *id == p.id).map(|(_, i)| *i).unwrap_or(PlayerInput { tick: state.tick, action: 0, ..p.last_input }))
        .collect();
    // A possessed host is driven by its possessor (movement, look, jump/sneak/sprint/walk).
    for i in 0..state.players.len() {
        let p = &state.players[i];
        if p.possess.stage != state::PossessStage::Active {
            continue;
        }
        let Some(h) = p.possess.host.and_then(|h| state.players.iter().position(|q| q.id == h)) else { continue };
        let pi = ins[i];
        let keep = input::buttons::JUMP | input::buttons::CROUCH | input::buttons::SPRINT | input::buttons::WALK;
        ins[h] = PlayerInput { tick: pi.tick, forward: pi.forward, right: pi.right, yaw: pi.yaw, pitch: pi.pitch, buttons: pi.buttons & keep, left_hand: state.players[h].left.item, action: 0, npc_move: 0 };
    }
    // Bend Time: each pawn's rate this tick; the player's armour (`Attribute_DamageReduction`,
    // the Armor upgrade, applied by the player's TakeDamage 0x00AB0A30).
    for i in 0..state.players.len() {
        let id = state.players[i].id;
        state.players[i].time_scale = powers::pawn_time_scale(state, id, g);
        let p = &state.players[i];
        let reduction = if p.kind == state::PawnKind::Human { powers::attribute(p, g, "Attribute_DamageReduction", 0.0).trunc() } else { 0.0 };
        let free_chance = powers::attribute(p, g, "Attribute_ChanceUseNoManaForSpell", 0.0);
        state.players[i].damage_reduction = reduction;
        state.players[i].free_spell_chance = free_chance;
    }

    for i in 0..state.players.len() {
        let input = ins[i];
        let scale = state.players[i].time_scale;
        if scale <= 0.0 {
            // Stopped in time: nothing happens to this pawn (input waits).
            continue;
        }
        let dt = DT * scale;
        let p0 = state.players[i];
        let mvc = powers::effective_movement(&p0, g);
        let ctx = MoveCtx { world, cfg: &mvc, dt };
        if !p0.alive || p0.incapacitated() {
            let p = &mut state.players[i];
            if p.alive && p.condition == state::Condition::BeingChoked {
                p.velocity = Vec3::ZERO;
            } else if p.carried_by.is_none() {
                takedown::step_body(p, world, g, dt);
            }
            if !p.alive {
                p.yaw = input.yaw;
                p.pitch = input.pitch;
            }
            takedown::step_afflictions(state, i, dt, g, &mut out.events);
            let p = &mut state.players[i];
            p.prev_buttons = input.buttons;
            p.last_input = input;
            continue;
        }
        if powers::is_hidden(&p0) {
            // Inside a host: only the left hand (to leave) and the vitals run.
            let p = &mut state.players[i];
            p.yaw = input.yaw;
            p.pitch = input.pitch;
            let mut local = Vec::new();
            let mut acts = Vec::new();
            left_hand::update(p, &input, &ctx, g, &cfg.tuning, &mut local, &mut acts, false, &mut out.events);
            actions.extend(acts.into_iter().map(|a| (i, a)));
            vitals::tick(p, &powers::effective_vitals(&p0, g), dt);
            p.prev_buttons = input.buttons;
            p.last_input = input;
            continue;
        }
        if p0.kind == state::PawnKind::Rat {
            // Rat pawns only move (their attacks resolve after every pawn moved).
            let p = &mut state.players[i];
            movement::step_movement(p, &input, false, p.action_lock <= 0.0, &ctx, &mut out.events);
            p.rat_slow = (p.rat_slow - dt).max(0.0);
            p.prev_buttons = input.buttons;
            p.last_input = input;
            continue;
        }

        // Takedowns, bodies, pickups (Use), elixirs: aimed with this tick's view.
        state.players[i].yaw = input.yaw;
        state.players[i].pitch = input.pitch.clamp(-89.0, 89.0);
        let choking = takedown::step_choke(state, i, &input, dt, g, &mut out.events);
        takedown::step_carry(state, i, &input, dt, world, g, &mut out.events);
        let p = state.players[i];
        let mut used = false;
        if !choking && p.carrying.is_none() && p.just_pressed(&input, input::buttons::USE) && p.mode == MoveMode::Walking && p.action_lock <= 0.0 && p.sword.phase == state::SwordPhase::Idle {
            used = true;
            if let Some(vi) = takedown::choke_target(state, &p, world, g, &cfg.tuning) {
                takedown::start_choke(state, i, vi, g, &mut out.events);
            } else if !inventory::try_use_pickup(state, i, world, g, &mut out.events) {
                if let Some(b) = takedown::body_in_view(state, &p, world, g) {
                    takedown::lift(state, p.id, b, &mut out.events);
                } else {
                    used = false;
                }
            }
        }
        // Use on a climbable chain (its interactable, 0x00A23130), on foot or in the air.
        if !used && !choking && p.just_pressed(&input, input::buttons::USE) && p.action_lock <= 0.0 {
            climb::try_use_grab(&mut state.players[i], world, &mvc, g.items.inventory.interact_distance, inventory::INTERACT_COS, &mut out.events);
        }
        match input.action {
            input::actions::DRINK_HEALTH => {
                inventory::drink(&mut state.players[i], dishonored_ir::items::ElixirType::Health, g, &mut out.events);
            }
            input::actions::DRINK_MANA => {
                inventory::drink(&mut state.players[i], dishonored_ir::items::ElixirType::Mana, g, &mut out.events);
            }
            _ => {}
        }
        let bend_active = state.time_bends.iter().any(|b| b.caster == p.id);
        let p = &mut state.players[i];
        let choke_lock = p.choke.phase != state::ChokePhase::None;
        let locked = combat::sword_locks_movement(p) || p.mode == MoveMode::Blinking || choke_lock || p.action_lock > 0.0 || p.carry_anim > 0.0;
        let sword_out = p.carrying.is_none() && !p.sword_sheathed;
        movement::step_movement(p, &input, sword_out, !locked, &ctx, &mut out.events);
        // No sword while choking, carrying or in an action.
        let sword_input = if choke_lock || p.carrying.is_some() || p.action_lock > 0.0 {
            PlayerInput { buttons: input.buttons & !(input::buttons::PRIMARY | input::buttons::BLOCK), ..input }
        } else {
            input
        };
        combat::update_sword(p, &sword_input, &cfg.tuning, dt, &mut out.events);
        let mut local = Vec::new();
        let mut acts = Vec::new();
        left_hand::update(p, &input, &ctx, g, &cfg.tuning, &mut local, &mut acts, bend_active, &mut out.events);
        actions.extend(acts.into_iter().map(|a| (i, a)));
        spawns.extend(local.into_iter().map(|(k, pos, vel)| (p.id, k, pos, vel)));
        let breath = powers::attribute(&p0, g, "Attribute_BreathCapacityIncrease", 0.0);
        let air_max = swim::air_capacity(&mvc.water, breath);
        let drowning = p0.water.under && air_max > 0.0 && p0.water.air_used >= air_max - 1e-6;
        vitals::tick_with(p, &powers::effective_vitals(&p0, g), dt, !drowning);
        left_hand::tick_dark_vision(p, dt, &mut out.events);
        adrenaline::tick(p, g, dt);
        swim::step_breath(p, world, &mvc, breath, 0.0, dt, &mut out.events);
        p.rat_slow = (p.rat_slow - dt).max(0.0);
        p.action_lock = (p.action_lock - dt).max(0.0);
        p.prev_buttons = input.buttons;
        p.last_input = input;
        takedown::step_afflictions(state, i, dt, g, &mut out.events);
    }
    // Rat attack groups.
    let rat_dt = DT * powers::world_time_scale(state, g, false);
    if rat_dt > 0.0 && state.players.iter().any(|p| p.kind == state::PawnKind::Rat) {
        rats::step_rat_attacks(state, &ins, rat_dt, &g.vitals, &rats::RatAttackTweak::default(), &mut out.events);
    }

    // World effects asked for by left hands.
    for (i, a) in actions {
        let id = state.players[i].id;
        match a {
            left_hand::LeftAction::BendTime => powers::start_bend_time(state, id, g, &mut out.events),
            left_hand::LeftAction::EndBendTime => {
                powers::end_bend_time_early(state, id, g);
            }
            left_hand::LeftAction::Swarm => powers::summon_swarm(state, id, world, g, &mut out.events),
            left_hand::LeftAction::PlaceTrap => gadgets::place_trap(state, id, world, g, &mut out.events),
            left_hand::LeftAction::PossessRelease => {
                let p = state.players[i];
                if let Some(target) = powers::possess_target_any(state, &p, world, g) {
                    if vitals::spend_mana(&mut state.players[i], g.items.possess.mana_cost, &g.vitals) {
                        let host = match target {
                            powers::PossessTarget::Pawn(h) => h,
                            powers::PossessTarget::SwarmRat(sid) => {
                                // One rat leaves the swarm as a pawn to be possessed.
                                let rid = powers::swarm_rat_id(state);
                                let (pos, yaw) = match state.swarms.iter_mut().find(|s| s.id == sid) {
                                    Some(s) => {
                                        s.rats = s.rats.saturating_sub(1);
                                        (s.position + Vec3::Z * movement::RAT_HALF_HEIGHT, p.yaw)
                                    }
                                    None => (p.position, p.yaw),
                                };
                                rats::spawn_rat(state, rid, pos, yaw, Some(sid), cfg, &mut out.events);
                                rid
                            }
                        };
                        powers::begin_possession(state, id, host, g);
                    }
                }
            }
            left_hand::LeftAction::PossessEnd => {
                let ctx = MoveCtx { world, cfg: mv, dt: DT };
                let had = state.players[i].possess.stage == state::PossessStage::Active;
                powers::end_possession(state, i, had, &ctx, g, &mut out.events);
            }
        }
    }
    for i in 0..state.players.len() {
        if matches!(state.players[i].possess.stage, state::PossessStage::PrePossess | state::PossessStage::Active | state::PossessStage::Cooldown) {
            let dt = DT * state.players[i].time_scale;
            let ctx = MoveCtx { world, cfg: mv, dt };
            powers::step_possession(state, i, &ctx, g, &mut out.events);
        }
    }

    for (owner, kind, pos, vel) in spawns {
        let id = state.next_projectile_id;
        state.next_projectile_id = state.next_projectile_id.wrapping_add(1);
        state.projectiles.push(left_hand::make_projectile(id, owner, kind, pos, vel, &cfg.tuning));
        out.events.push(SimEvent::Fired { id: owner, bolt: kind.is_bolt(), projectile: id });
    }

    // Health before the sword resolves (assassinations give adrenaline by the victim's health).
    let pre_health: Vec<(PlayerId, f32)> = state.players.iter().map(|p| (p.id, p.vitals.health)).collect();
    combat::resolve_sword_hits(state, world, mv, &cfg.gameplay.vitals, &cfg.tuning, &mut out.events);
    movement::slide_impacts(state, mv, &cfg.gameplay.vitals, &mut out.events);

    for e in &out.events {
        if let SimEvent::WindblastCast { id } = e {
            state.windblasts.push(state::WindblastState {
                caster: *id,
                warmup: left_hand::WINDBLAST_WARMUP,
                elapsed: 0.0,
                origin: Vec3::ZERO,
                direction: Vec3::X,
                targets: Vec::new(),
                started: false,
            });
        }
    }
    let world_dt = DT * powers::world_time_scale(state, g, false);
    let proj_dt = DT * powers::world_time_scale(state, g, true);
    if world_dt > 0.0 {
        left_hand::step_windblasts(state, world, mv, &cfg.gameplay.vitals, &cfg.tuning, world_dt, &mut out.events);
    }
    if proj_dt > 0.0 {
        // `m_fTimeBeforeSleep` less the owner's `Attribute_SleepDelayReduction` (Instant Sleep upgrade, LIKELY).
        let delays: Vec<(PlayerId, f32)> = state.players.iter().map(|p| (p.id, (takedown::SLEEP_DELAY - powers::attribute(p, g, "Attribute_SleepDelayReduction", 0.0)).max(0.0))).collect();
        let sleep_delay = |id: u32| delays.iter().find(|d| d.0 == id).map_or(takedown::SLEEP_DELAY, |d| d.1);
        left_hand::step_projectiles(state, world, mv, &cfg.tuning, &cfg.gameplay.vitals, mv.gravity_z, proj_dt, &sleep_delay, &mut out.events);
        gadgets::step_grenades(state, world, g, proj_dt, &mut out.events);
    }
    if world_dt > 0.0 {
        gadgets::step_traps(state, world, g, world_dt, &mut out.events);
        powers::step_swarms(state, world_dt, g, &mut out.events);
    }
    powers::step_time_bends(state, DT, &mut out.events);
    inventory::update_carried(state, mv);

    volumes::step_pain(state, world, cfg, &mut out.events);

    // Environmental damage from this tick's movement events.
    let env: Vec<SimEvent> = out.events.clone();
    for e in env {
        match e {
            SimEvent::FellOutOfWorld { id } => {
                if let Some(p) = state.player_mut(id) {
                    vitals::kill(p, None, DamageCause::World, &mut out.events);
                }
            }
            SimEvent::Landed { id, speed, .. } => {
                // `ADishonoredPawn::TakeFallingDamage` (0x00B4DCC0, DishonoredPawn vtable +0x528):
                // damage = (int)(HealthMax * clamp((v - dmg) / (death - dmg), 0, 1)) for
                // an impact speed v above Attribute_MaxSpeedBeforeFallingDamage.
                // The thresholds are attributes (Agility raises them, 0x00B56880).
                let Some(p0) = state.player(id).copied() else { continue };
                let pm = powers::effective_movement(&p0, g);
                let dmg = fall_damage(speed, &pm, powers::health_max(&p0, g));
                if dmg > 0.0 {
                    if let Some(p) = state.player_mut(id) {
                        vitals::apply_damage(p, dmg, None, DamageCause::Fall, &mut out.events, &cfg.gameplay.vitals);
                    }
                }
            }
            SimEvent::Damage { victim, attacker, amount, cause } => {
                let ad = &g.adrenaline;
                if let Some(a) = attacker.filter(|a| *a != victim) {
                    let gain = match cause {
                        DamageCause::Sword | DamageCause::SwordCounter => amount * ad.mult_on_hit,
                        _ => 0.0,
                    };
                    if gain > 0.0 {
                        if let Some(p) = state.player_mut(a) {
                            adrenaline::gain(p, gain, g, &mut out.events);
                        }
                    }
                }
                // Damage taken (`m_bCanGiveAdrenaline`, every base damage type).
                if ad.mult_taking_damage > 0.0 {
                    if let Some(p) = state.player_mut(victim) {
                        let m = powers::attribute(p, g, "Attribute_AdrenalineMultWhenTakingDamage", ad.mult_taking_damage);
                        adrenaline::gain(p, amount * m, g, &mut out.events);
                    }
                }
            }
            SimEvent::SwordParried { defender, .. } => {
                if let Some(p) = state.player_mut(defender) {
                    adrenaline::gain(p, g.adrenaline.parry_win, g, &mut out.events);
                }
            }
            SimEvent::Death { victim, killer: Some(k), cause } => {
                let (Some(kp), Some(vp)) = (state.player(k).copied(), state.player(victim).copied()) else { continue };
                // Adrenaline from assassinations (victim health x the tweak's multiplier) and rat kills.
                let health = pre_health.iter().find(|h| h.0 == victim).map_or(0.0, |h| h.1);
                let gain = match cause {
                    DamageCause::Assassination => health * g.adrenaline.mult_assassinate,
                    DamageCause::DropAssassination => health * g.adrenaline.mult_drop_assassinate,
                    _ if vp.kind == state::PawnKind::Rat => powers::attribute(&kp, g, "Attribute_AdrenalineOnRatKill", 0.0),
                    _ => 0.0,
                };
                if gain > 0.0 && k != victim {
                    if let Some(p) = state.player_mut(k) {
                        adrenaline::gain(p, gain, g, &mut out.events);
                    }
                }
                // Shadow Kill (0x00A87B60): the corpse burns to ash.
                if powers::shadow_kill_applies(&kp, &vp, cause, vp.condition == state::Condition::Unconscious) {
                    if let Some(v) = state.player_mut(victim) {
                        v.no_body = true;
                    }
                    out.events.push(SimEvent::BodyRemoved { body: victim, ash: true });
                }
                // Falling Star charm (0x00ABBD10): mana and health on a drop assassination.
                if cause == DamageCause::DropAssassination {
                    let mana = powers::attribute(&kp, g, "Attribute_DropAssassinationManaBonus", 0.0).trunc();
                    let health = powers::attribute(&kp, g, "Attribute_DropAssassinationHealthBonus", 0.0).trunc();
                    if mana > 0.0 || health > 0.0 {
                        let hmax = powers::health_max(&kp, g);
                        let mmax = powers::mana_max(&kp, g);
                        if let Some(a) = state.player_mut(k) {
                            a.vitals.mana = (a.vitals.mana + mana).min(mmax);
                            a.vitals.health = (a.vitals.health + health).min(hmax);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    state.tick += 1;
    out
}

/// Grant a player the items and power levels a mission or test hands out.
pub fn grant(state: &mut WorldState, id: PlayerId, f: impl FnOnce(&mut PlayerState)) {
    if let Some(p) = state.player_mut(id) {
        f(p);
    }
}

/// Falling damage for a landing at `speed` (positive, downward), whole points.
pub fn fall_damage(speed: f32, mv: &dishonored_ir::gameplay::MovementConfig, health_max: f32) -> f32 {
    let (lo, hi) = {
        let (a, b) = (mv.max_speed_before_falling_damage, mv.max_speed_before_falling_death);
        if b < a { (b, a) } else { (a, b) }
    };
    if speed <= lo {
        return 0.0;
    }
    let f = ((speed - lo) / (hi - lo)).clamp(0.0, 1.0);
    (health_max * f).trunc()
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
        mix(p.condition as u64);
        mix(p.choke.phase as u64);
        mix(p.carrying.map_or(0, |c| c as u64 + 1));
        mix(p.possess.stage as u64);
        mix(p.time_scale.to_bits() as u64);
        mix(p.no_body as u64);
        for a in p.inv.ammo {
            mix(a as u64);
        }
        mix(p.inv.elixirs[0] as u64 | (p.inv.elixirs[1] as u64) << 8);
        mix(p.inv.coins as u64);
        mix(p.kind as u64);
        for f in [p.water.air_used, p.water.stroke, p.adrenaline.value, p.dark.remaining, p.rat_slow, p.jump.held] {
            mix(f.to_bits() as u64);
        }
        mix(p.helmet as u64 | (p.jump.powered as u64) << 1 | (p.ai_unaware as u64) << 2);
        mix(p.rng as u64);
        // Climbing (only once a pawn has touched a chain, so earlier hashes are unchanged).
        if p.climb != state::ClimbState::default() {
            let c = &p.climb;
            mix(c.on.map_or(u64::MAX, |i| i as u64) ^ (c.down as u64) << 32 ^ (c.letting_go as u64) << 33 ^ (c.jumped as u64) << 34);
            for f in [c.time, c.since_sound, c.no_touch] {
                mix(f.to_bits() as u64);
            }
        }
    }
    for pk in &state.pickups {
        mix(pk.id as u64 | (pk.active as u64) << 32);
    }
    for t in &state.traps {
        mix(t.id as u64 | (t.triggered as u64) << 32 | (t.detonated as u64) << 33);
    }
    for s in &state.swarms {
        mix(s.id as u64);
        mix(s.position.x.to_bits() as u64);
        mix(s.position.y.to_bits() as u64);
    }
    for b in &state.time_bends {
        mix(b.caster as u64);
        mix(b.elapsed.to_bits() as u64);
    }
    for wb in &state.windblasts {
        mix(wb.caster as u64);
        mix(wb.elapsed.to_bits() as u64);
        mix(wb.warmup.to_bits() as u64);
        mix(wb.targets.len() as u64);
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
