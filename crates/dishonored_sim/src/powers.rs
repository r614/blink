//! Powers beyond Blink/Windblast/Dark Vision, passive powers, bone charms and runes, as decoded
//! from `Dishonored.exe` (research/dishonored/PLAYER.md "Powers"):
//!
//! - cost (`DishonoredActivePowerComponent`, 0x00BF33xx): a power can be used when mana is at
//!   least `m_ManaCost`; using it spends the cost (0x00AA93E0).
//! - Possession (`DishonoredActivePowerComponent_Possess`, stages 0x00BFD6D0): targeting, then
//!   the possessor travels into the host (`m_IntroDuration`), then possession runs for
//!   `possessable.m_Levels[level].m_fPossessDuration * (1 + Attribute_SpellStrength)` (0x00BFB6C0);
//!   it ends when the timer runs out, the host dies, or the power is used again (0x00BFB520 sets
//!   the end flag); afterwards `m_fCancelCooldown` blocks a new cast (0x00BED880). A host needs
//!   `m_RequiredPossessPowerLevel` and must not be in a refused awareness state
//!   (`m_AwarenessSettings`: an NPC being choked cannot be possessed).
//! - Bend Time (`DishonoredActivePowerComponent_BendTime`, start 0x00BF5190 / 0x00BF8810, tick
//!   0x00BFBE60, end 0x00BFA200): world and caster dilation of the level, duration
//!   `m_TimeDuration * (1 + SpellStrength)` measured in real time; the dilation returns to 1
//!   over `m_fTimeDilationCooldownTime` before the end; projectiles wake up
//!   `m_fProjectileEarlyWakeupTime` before the end; using the power again ends it early
//!   (0x00BF8950 moves the start so only the cooldown remains).
//! - passive powers (`DishonoredPowersComponent.m_Powers`, DefaultPlayer.ini) and bone charms
//!   (`DisTweaks_WhaleBoneCharmList`) are attribute modifiers.

use crate::events::{DamageCause, SimEvent, SoundCategory};
use crate::movement::{MoveCtx, eye_position, forward_vector, view_vector};
use crate::state::{Condition, PawnKind, PlayerId, PlayerState, PossessStage, SwarmState, TimeBendState, WorldState, power};
use crate::vitals::{apply_damage, spend_mana};
use dishonored_ir::gameplay::{GameplayConfig, MovementConfig, VitalsConfig};
use dishonored_ir::items::ModifierType;
use dishonored_world::{Capsule, WorldQuery, channel};
use glam::Vec3;
use std::borrow::Cow;

/// Value of an attribute for `p`: `base` with the current level's modifiers of each owned
/// passive power applied in order (`AddVal` adds, `SetVal` replaces, `AddBasePercent` adds
/// `base * v / 100`), then the value of every equipped bone charm on that attribute added.
pub fn attribute(p: &PlayerState, cfg: &GameplayConfig, name: &str, base: f32) -> f32 {
    attribute_for(p, cfg, name, base, |_| true)
}

/// [`attribute`] counting only the workshop upgrades `host` accepts (by upgrade name): an
/// upgrade modifies the attributes of its `m_pHostItemType` only (e.g. `Pistol_Accuracy1` ->
/// `DishonoredWepPistol`, `Crossbow_Accuracy` -> the crossbow).
pub fn attribute_for(p: &PlayerState, cfg: &GameplayConfig, name: &str, base: f32, host: impl Fn(&str) -> bool) -> f32 {
    let items = &cfg.items;
    let mut v = base;
    for pp in &items.passive_powers {
        let Some(idx) = power::NAMES.iter().position(|n| *n == pp.name) else { continue };
        let lvl = p.inv.power_levels[idx] as usize;
        let Some((_, mods)) = pp.levels.get(lvl) else { continue };
        if lvl == 0 {
            continue;
        }
        for m in mods.iter().filter(|m| m.attribute == name) {
            match m.kind {
                ModifierType::AddVal => v += m.value,
                ModifierType::SetVal => v = m.value,
                ModifierType::AddBasePercent => v += base * m.value / 100.0,
                ModifierType::Other => {}
            }
        }
    }
    if p.inv.upgrades != 0 {
        for (i, (un, attrs)) in items.upgrades.iter().enumerate().take(32) {
            if p.inv.upgrades & (1 << i) != 0 && host(un) {
                v += attrs.iter().filter(|(n, _)| n == name).map(|(_, x)| *x).sum::<f32>();
            }
        }
    }
    if p.inv.charms_equipped != 0 {
        for (i, c) in items.charms.iter().enumerate().take(32) {
            if p.inv.charms_equipped & (1 << i) != 0 && c.attribute == name {
                v += c.values.first().copied().unwrap_or(0.0);
            }
        }
    }
    v
}

/// Give a workshop upgrade by name (`Twk_Upgrade_<name>`, e.g. `GrenadeBelt`).
pub fn give_upgrade(p: &mut PlayerState, name: &str, cfg: &GameplayConfig) -> bool {
    match cfg.items.upgrades.iter().take(32).position(|(n, _)| n.eq_ignore_ascii_case(name)) {
        Some(i) => {
            p.inv.upgrades |= 1 << i;
            true
        }
        None => false,
    }
}

fn any_modifiers(p: &PlayerState) -> bool {
    p.inv.charms_equipped != 0 || p.inv.upgrades != 0 || p.inv.power_levels[power::VITALITY] > 0 || p.inv.power_levels[power::AGILITY] > 0 || p.inv.power_levels[power::BLOOD_THIRSTY] > 0
}

/// `Attribute_HealthMax`: the player's (Corvo 70, with its modifiers) or a guard's (the guard
/// archetype's pawn tweak, 50). Civilians keep the player's base (their archetypes vary: 10-11
/// for aristocrats, not modelled per pawn).
pub fn health_max(p: &PlayerState, cfg: &GameplayConfig) -> f32 {
    let base = if p.kind == PawnKind::Guard { cfg.ai.combat.guard_health_max } else { cfg.vitals.health_max };
    attribute(p, cfg, "Attribute_HealthMax", base)
}

pub fn mana_max(p: &PlayerState, cfg: &GameplayConfig) -> f32 {
    attribute(p, cfg, "Attribute_ManaMax", cfg.vitals.mana_max)
}

pub fn max_charms(p: &PlayerState, cfg: &GameplayConfig) -> u8 {
    attribute(p, cfg, "Attribute_MaxActivatedBoneCharmCountBonus", 0.0) as u8 + cfg.items.inventory.max_equipped_charms
}

/// Vitals with the player's attribute modifiers (Vitality, charms).
pub fn effective_vitals<'a>(p: &PlayerState, cfg: &'a GameplayConfig) -> Cow<'a, VitalsConfig> {
    if !any_modifiers(p) {
        return Cow::Borrowed(&cfg.vitals);
    }
    let b = &cfg.vitals;
    Cow::Owned(VitalsConfig {
        health_max: attribute(p, cfg, "Attribute_HealthMax", b.health_max),
        health_regen_rate: attribute(p, cfg, "Attribute_HealthRegenRate", b.health_regen_rate),
        health_regen_amount: attribute(p, cfg, "Attribute_HealthRegenAmount", b.health_regen_amount),
        health_regen_limit: attribute(p, cfg, "Attribute_HealthRegenLimit", b.health_regen_limit),
        health_regen_initial_delay: attribute(p, cfg, "Attribute_HealthRegenInitialDelay", b.health_regen_initial_delay),
        mana_max: attribute(p, cfg, "Attribute_ManaMax", b.mana_max),
        mana_regen_amount: attribute(p, cfg, "Attribute_ManaRegenAmount", b.mana_regen_amount),
        mana_regen_additive_portion: attribute(p, cfg, "Attribute_ManaRegenAdditivePortion", b.mana_regen_additive_portion),
        mana_regen_step_time: attribute(p, cfg, "Attribute_ManaRegenStepTime", b.mana_regen_step_time),
        mana_regen_initial_delay: attribute(p, cfg, "Attribute_ManaRegenInitialDelay", b.mana_regen_initial_delay),
    })
}

/// Movement attributes for this tick: modifiers, carrying a body
/// (`Attribute_GroundSpeedCarryingCorpse` replaces `Attribute_GroundSpeed`, 0x00B56880;
/// `Attribute_JumpZCarryingCorpse`), a possessed host (`Modifier_PossessGroundSpeed` sets the
/// ground speed to `m_fGroundSpeedWhilePossessed`, 0x00A61770).
pub fn effective_movement<'a>(p: &PlayerState, cfg: &'a GameplayConfig) -> Cow<'a, MovementConfig> {
    let carrying = p.carrying.is_some();
    let possessed = p.condition == Condition::Possessed;
    let rat = p.kind == PawnKind::Rat;
    if !carrying && !possessed && !rat && p.rat_slow <= 0.0 && !any_modifiers(p) {
        return Cow::Borrowed(&cfg.movement);
    }
    let b = &cfg.movement;
    let mut m = b.clone();
    let a = |n: &str, base: f32| attribute(p, cfg, n, base);
    m.run_speed = a("Attribute_GroundSpeed", b.run_speed);
    m.sprint_speed = a("Attribute_GroundSpeedSprint", b.sprint_speed);
    m.crouch_speed = a("Attribute_GroundSpeedCrouch", b.crouch_speed);
    m.strafe_mult_sprint = a("Attribute_GroundStrafeMultiplierSprint", b.strafe_mult_sprint);
    m.back_mult_sprint = a("Attribute_GroundBackwardMultiplierSprint", b.back_mult_sprint);
    m.sword_unsheathed_speed_factor = a("Attribute_SwordUnsheathedSpeedFactor", b.sword_unsheathed_speed_factor);
    m.max_speed_before_falling_damage = a("Attribute_MaxSpeedBeforeFallingDamage", b.max_speed_before_falling_damage);
    m.max_speed_before_falling_death = a("Attribute_MaxSpeedBeforeFallingDeath", b.max_speed_before_falling_death);
    // Agility's power jump and swim speed.
    m.power_jump_z = a("Attribute_JumpZ_PowerJump", b.power_jump_z);
    m.water.water_speed = a("Attribute_WaterSpeed", b.water.water_speed);
    m.mantle.anim_rate = a("Attribute_MantleAnimRate", b.mantle.anim_rate);
    // A rat bite's slowdown: `Modifier_RatAttackSpeedModifier` adds -m_fPawnSlowdownPercent % of the
    // base to Attribute_GroundSpeed and Attribute_GroundSpeedSprint (0x00C891E0).
    if p.rat_slow > 0.0 && p.rat_slow_percent > 0.0 {
        let k = 1.0 - p.rat_slow_percent / 100.0;
        m.run_speed -= b.run_speed * (1.0 - k);
        m.sprint_speed -= b.sprint_speed * (1.0 - k);
    }
    if carrying {
        let inv = &cfg.items.inventory;
        m.run_speed = a("Attribute_GroundSpeedCarryingCorpse", inv.carry_ground_speed);
        m.jump_z = a("Attribute_JumpZCarryingCorpse", inv.carry_jump_z);
    }
    if rat {
        // Rat crowd agents move at `MaxSpeed` (`RatSwarm.CrowdAgent_SmallRat`: 350), any direction,
        // with no sword factor and no jump.
        crate::rats::rat_movement(&mut m);
    }
    if possessed {
        m.run_speed = if rat { cfg.items.possess.rat.ground_speed } else { cfg.items.possess.npc.ground_speed };
        // `m_bCanSprintWhilePossessed` is false for every possessable: sprint keeps the base speed.
        m.sprint_speed = m.run_speed;
        if rat {
            m.crouch_speed = m.run_speed;
            m.walk_speed = m.run_speed;
        }
    }
    Cow::Owned(m)
}

// ---------------------------------------------------------------------------------------
// Runes

/// Rune cost of the next level of power `idx` (None when maxed).
pub fn next_level_cost(p: &PlayerState, idx: usize, cfg: &GameplayConfig) -> Option<i32> {
    let lvl = p.inv.power_levels[idx] as usize;
    let name = power::NAMES[idx];
    if let Some((_, c)) = cfg.items.active_rune_costs.iter().find(|(n, _)| n == name) {
        return c.get(lvl).copied();
    }
    let pp = cfg.items.passive_powers.iter().find(|pp| pp.name == name)?;
    pp.levels.get(lvl + 1).map(|l| l.0)
}

/// Buy the next level of a power with runes (`DishonoredPowersComponent`, Outsider's shrine).
pub fn buy_power(p: &mut PlayerState, idx: usize, cfg: &GameplayConfig) -> bool {
    let Some(cost) = next_level_cost(p, idx, cfg) else { return false };
    if cost < 0 || (p.inv.runes as i32) < cost {
        return false;
    }
    p.inv.runes -= cost as u16;
    p.inv.power_levels[idx] += 1;
    true
}

/// Equip or unequip an owned bone charm (`m_MaxActivatedBoneCharmCount` slots).
pub fn set_charm(p: &mut PlayerState, i: usize, on: bool, cfg: &GameplayConfig) -> bool {
    if i >= 32 || p.inv.charms_owned & (1 << i) == 0 {
        return false;
    }
    if on {
        if p.inv.charms_equipped & (1 << i) == 0 && p.inv.charms_equipped.count_ones() as u8 >= max_charms(p, cfg) {
            return false;
        }
        p.inv.charms_equipped |= 1 << i;
    } else {
        p.inv.charms_equipped &= !(1 << i);
    }
    true
}

// ---------------------------------------------------------------------------------------
// Bend Time

/// Notify times of the cast animations (`Ply_Powers_as`): `DisNotify_BendTime` at 0.140 s,
/// `DisNotify_DevouringSwarm` at 0.165 s, `DisNotify_Possession` at 0.172 s of the Out anim;
/// the casts end (`AnimStateExit`) at 1.295 / 1.679 / 1.117 s.
pub const BEND_TIME_NOTIFY: f32 = 0.140;
pub const BEND_TIME_EXIT: f32 = 1.295;
pub const SWARM_NOTIFY: f32 = 0.165;
pub const SWARM_EXIT: f32 = 1.679;
pub const POSSESS_NOTIFY: f32 = 0.172;
pub const POSSESS_EXIT: f32 = 1.117;

/// Start Bend Time for `caster` (0x00BF5190): dilations and duration of its level.
pub fn start_bend_time(state: &mut WorldState, caster: PlayerId, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let Some(p) = state.player(caster) else { return };
    let level = p.inv.power_levels[power::BEND_TIME].clamp(1, 2) - 1;
    let l = cfg.items.bend_time.levels[level as usize];
    let strength = attribute(p, cfg, "Attribute_SpellStrength", 0.0);
    state.time_bends.retain(|b| b.caster != caster);
    state.time_bends.push(TimeBendState {
        caster,
        level,
        elapsed: 0.0,
        duration: l.duration + strength * l.duration,
        world_dilation: l.world_dilation,
        self_dilation: l.player_dilation,
    });
    events.push(SimEvent::BendTimeStart { id: caster, level });
}

/// Using Bend Time while it runs ends it: the start moves so that only the dilation cooldown
/// remains (0x00BF8950).
pub fn end_bend_time_early(state: &mut WorldState, caster: PlayerId, cfg: &GameplayConfig) -> bool {
    let Some(b) = state.time_bends.iter_mut().find(|b| b.caster == caster) else { return false };
    let cool = cfg.items.bend_time.levels[b.level as usize].dilation_cooldown;
    b.elapsed = b.elapsed.max(b.duration - cool);
    true
}

/// Dilation `target` blended in over the warmup and out over the cooldown (the blend helper
/// 0x009FA0D0 is called with the cooldown time; linear blending is LIKELY).
fn ramp(b: &TimeBendState, target: f32, cfg: &GameplayConfig) -> f32 {
    let l = cfg.items.bend_time.levels[b.level as usize];
    let e = b.elapsed;
    if e < l.dilation_warmup && l.dilation_warmup > 0.0 {
        1.0 + (target - 1.0) * (e / l.dilation_warmup)
    } else if e > b.duration - l.dilation_cooldown && l.dilation_cooldown > 0.0 {
        let k = ((e - (b.duration - l.dilation_cooldown)) / l.dilation_cooldown).clamp(0.0, 1.0);
        target + (1.0 - target) * k
    } else {
        target
    }
}

/// Rate of time for a pawn: the caster runs at its own dilation, everything else at the world
/// dilation of every active bend (several casters multiply).
pub fn pawn_time_scale(state: &WorldState, id: PlayerId, cfg: &GameplayConfig) -> f32 {
    let mut s = 1.0;
    for b in &state.time_bends {
        s *= if b.caster == id { ramp(b, b.self_dilation, cfg) } else { ramp(b, b.world_dilation, cfg) };
    }
    s
}

/// Rate of time for projectiles, traps and swarms: the world dilation, except that projectiles
/// wake up `m_fProjectileEarlyWakeupTime` before a bend ends.
pub fn world_time_scale(state: &WorldState, cfg: &GameplayConfig, projectile: bool) -> f32 {
    let mut s = 1.0;
    for b in &state.time_bends {
        let l = cfg.items.bend_time.levels[b.level as usize];
        if projectile && b.elapsed >= b.duration - l.projectile_early_wakeup {
            continue;
        }
        s *= ramp(b, b.world_dilation, cfg);
    }
    s
}

/// Advance bends in real time; end those past their duration.
pub fn step_time_bends(state: &mut WorldState, dt: f32, events: &mut Vec<SimEvent>) {
    for b in state.time_bends.iter_mut() {
        b.elapsed += dt;
    }
    let ended: Vec<PlayerId> = state.time_bends.iter().filter(|b| b.elapsed > b.duration).map(|b| b.caster).collect();
    state.time_bends.retain(|b| b.elapsed <= b.duration);
    for id in ended {
        events.push(SimEvent::BendTimeEnd { id });
    }
}

// ---------------------------------------------------------------------------------------
// Possession

/// Whether pawn `t` can be possessed by `p` now (`DisPossessableInterface` checks).
pub fn can_possess(p: &PlayerState, t: &PlayerState, cfg: &GameplayConfig) -> bool {
    // Rats use `Twk_Possessable_Rat` (Possession level 1, 20 / 30 s), people `Twk_Possessable_BaseNPC`.
    let c = possessable(t, cfg);
    let level = p.inv.power_levels[power::POSSESSION];
    if t.kind == PawnKind::Human || !t.alive || t.id == p.id || c.disallow || level < c.required_level {
        return false;
    }
    // `m_AwarenessSettings[EAIAwareness]`: being choked (7) is refused; an unconscious pawn
    // cannot be possessed (the NPC must be awake to be taken over).
    let awareness = match t.condition {
        Condition::BeingChoked => 7,
        Condition::Unconscious | Condition::Possessed => return false,
        Condition::Normal => 0,
    };
    if !c.awareness[awareness] || t.carried_by.is_some() {
        return false;
    }
    c.duration[(level as usize).clamp(1, 2) - 1] > 0.0
}

/// The possessable tweak of a host.
pub fn possessable<'a>(t: &PlayerState, cfg: &'a GameplayConfig) -> &'a dishonored_ir::items::PossessableConfig {
    if t.kind == PawnKind::Rat { &cfg.items.possess.rat } else { &cfg.items.possess.npc }
}

/// What a Possession cast takes over: a pawn, or one rat of a Devouring Swarm (the swarm's rats
/// are crowd agents in the original; the sim spawns that rat as a pawn when it is possessed).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PossessTarget {
    Pawn(PlayerId),
    SwarmRat(u32),
}

/// Possession target including swarm rats (same cone and range as [`possess_target`]).
pub fn possess_target_any(state: &WorldState, p: &PlayerState, world: &dyn WorldQuery, cfg: &GameplayConfig) -> Option<PossessTarget> {
    if let Some(id) = possess_target(state, p, world, cfg) {
        return Some(PossessTarget::Pawn(id));
    }
    let level = p.inv.power_levels[power::POSSESSION] as usize;
    if level < cfg.items.possess.rat.required_level as usize {
        return None;
    }
    let range = cfg.items.possess.range[level.clamp(1, 2) - 1];
    let eye = eye_position(p, &cfg.movement);
    let dir = view_vector(p.yaw, p.pitch);
    let max_angle = 0.3 * 90.0_f32;
    let mut best: Option<(u32, f32)> = None;
    for s in &state.swarms {
        if s.spawn_delay > 0.0 || s.rats == 0 {
            continue;
        }
        let to = s.position - eye;
        let d = to.length();
        if d > range || d < 1.0 {
            continue;
        }
        let ang = (to.dot(dir) / d).clamp(-1.0, 1.0).acos().to_degrees();
        if ang > max_angle || world.raycast(eye, to, d, channel::MELEE).is_some_and(|h| h.distance < d - 20.0) {
            continue;
        }
        let score = d / range + 10.0 * ang / max_angle;
        if best.is_none_or(|(_, b)| score < b) {
            best = Some((s.id, score));
        }
    }
    best.map(|b| PossessTarget::SwarmRat(b.0))
}

/// Pawn id given to a rat taken out of a swarm.
pub fn swarm_rat_id(state: &WorldState) -> PlayerId {
    let mut id = 0x4000_0000 | state.next_entity_id;
    while state.player(id).is_some() {
        id = id.wrapping_add(1);
    }
    id
}

/// Possession target under the crosshair (targeting stage, `FUN_00bfcec0`): within the level's
/// `m_fPossessRange`, inside `m_fMaxAngleFOVFraction` of a 90-degree field of view, visible;
/// scored by `m_fDistanceWeight * d/range + m_fAngleWeight * angle/max` (LIKELY weighting).
pub fn possess_target(state: &WorldState, p: &PlayerState, world: &dyn WorldQuery, cfg: &GameplayConfig) -> Option<PlayerId> {
    let pc = &cfg.items.possess;
    let level = (p.inv.power_levels[power::POSSESSION] as usize).clamp(1, 2);
    let range = pc.range[level - 1];
    let eye = eye_position(p, &cfg.movement);
    let dir = view_vector(p.yaw, p.pitch);
    let max_angle = 0.3 * 90.0_f32;
    let mut best: Option<(PlayerId, f32)> = None;
    for t in &state.players {
        if !can_possess(p, t, cfg) {
            continue;
        }
        let to = t.position - eye;
        let d = to.length();
        if d > range || d < 1.0 {
            continue;
        }
        let ang = (to.dot(dir) / d).clamp(-1.0, 1.0).acos().to_degrees();
        if ang > max_angle {
            continue;
        }
        if world.raycast(eye, to, d, channel::MELEE).is_some_and(|h| h.distance < d - cfg.movement.collision_radius) {
            continue;
        }
        let score = d / range + 10.0 * ang / max_angle;
        if best.is_none_or(|(_, s)| score < s) {
            best = Some((t.id, score));
        }
    }
    best.map(|b| b.0)
}

/// Begin possessing `host`: the possessor travels into it over `m_IntroDuration`.
pub fn begin_possession(state: &mut WorldState, who: PlayerId, host: PlayerId, cfg: &GameplayConfig) {
    let Some(p) = state.player_mut(who) else { return };
    p.possess.stage = PossessStage::PrePossess;
    p.possess.host = Some(host);
    p.possess.timer = cfg.items.possess.intro_duration;
}

/// Per-tick possession logic for the possessor.
pub fn step_possession(state: &mut WorldState, who: usize, ctx: &MoveCtx, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let dt = ctx.dt;
    let p = state.players[who];
    match p.possess.stage {
        PossessStage::Cooldown => {
            let c = &mut state.players[who].possess;
            c.cooldown -= dt;
            if c.cooldown < 0.0 {
                c.stage = PossessStage::Inactive;
                c.cooldown = 0.0;
            }
        }
        PossessStage::PrePossess => {
            let Some(host) = p.possess.host.and_then(|h| state.player(h)).copied() else {
                end_possession(state, who, false, ctx, cfg, events);
                return;
            };
            if !host.alive || !can_possess(&p, &host, cfg) {
                end_possession(state, who, false, ctx, cfg, events);
                return;
            }
            let pp = &mut state.players[who];
            pp.possess.timer -= dt;
            // Travel toward the host (the original moves the possessor in `m_NumSteps` steps).
            let k = if pp.possess.timer <= 0.0 { 1.0 } else { (dt / (pp.possess.timer + dt)).clamp(0.0, 1.0) };
            pp.position = pp.position.lerp(host.position, k);
            pp.velocity = Vec3::ZERO;
            if pp.possess.timer <= 0.0 {
                let level = (p.inv.power_levels[power::POSSESSION] as usize).clamp(1, 2);
                let strength = attribute(&p, cfg, "Attribute_SpellStrength", 0.0);
                let base = possessable(&host, cfg).duration[level - 1];
                pp.possess.stage = PossessStage::Active;
                pp.possess.timer = (strength + 1.0) * base;
                pp.position = host.position;
                let h = state.player_mut(host.id).unwrap();
                h.condition = Condition::Possessed;
                h.condition_by = Some(p.id);
                events.push(SimEvent::PossessStart { id: p.id, host: host.id });
            }
        }
        PossessStage::Active => {
            let host = p.possess.host.and_then(|h| state.player(h)).copied();
            let alive = host.is_some_and(|h| h.alive && h.condition == Condition::Possessed);
            // The timer pauses while the controller is in cinematic mode (0x00BFB390).
            state.players[who].possess.timer -= dt;
            if !alive || state.players[who].possess.timer <= 0.0 {
                end_possession(state, who, true, ctx, cfg, events);
                return;
            }
            let h = host.unwrap();
            let pp = &mut state.players[who];
            pp.position = h.position;
            pp.velocity = Vec3::ZERO;
        }
        _ => {}
    }
}

/// End possession (0x00BF9330): the possessor reappears out of the host (`DUET_UpFromFeet` for
/// people: the possessor stands where the host stands and the host is pushed out of the way;
/// here the possessor steps out just behind the host when that spot is free, else it takes the
/// first free spot around it, else the host's spot), then `m_fCancelCooldown`.
pub fn end_possession(state: &mut WorldState, who: usize, had_host: bool, ctx: &MoveCtx, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let pid = state.players[who].id;
    let host_id = state.players[who].possess.host;
    if let Some(hid) = host_id {
        let host = state.player(hid).copied();
        if let Some(h) = host {
            if had_host || state.players[who].possess.stage == PossessStage::Active {
                let mv = &cfg.movement;
                let r = mv.collision_radius;
                let fwd = forward_vector(h.yaw);
                let right = Vec3::new(-fwd.y, fwd.x, 0.0);
                let mut spot = h.position;
                for off in [-fwd, -right, right, fwd] {
                    let c = h.position + off * (2.0 * r + 5.0);
                    let cap = Capsule { center: c, radius: r, half_height: mv.collision_half_height };
                    let mut contacts = Vec::new();
                    ctx.world.capsule_contacts(&cap, channel::PAWN, &mut contacts);
                    if !contacts.iter().any(|k| k.depth > 0.5) && ctx.world.raycast(h.position, c - h.position, (c - h.position).length(), channel::PAWN).is_none() {
                        spot = c;
                        break;
                    }
                }
                let p = &mut state.players[who];
                p.position = spot;
                p.yaw = h.yaw;
                p.velocity = Vec3::ZERO;
                p.mode = crate::state::MoveMode::Falling;
                events.push(SimEvent::PossessEnd { id: pid, host: hid });
            }
            if let Some(hp) = state.player_mut(hid) {
                if hp.condition == Condition::Possessed {
                    hp.condition = Condition::Normal;
                    hp.condition_by = None;
                }
            }
        }
    }
    let p = &mut state.players[who];
    p.possess.stage = PossessStage::Cooldown;
    p.possess.host = None;
    p.possess.timer = 0.0;
    p.possess.cooldown = cfg.items.possess.cancel_cooldown;
}

/// A possessor is hidden inside its host: not hittable, not visible (hook for perception).
pub fn is_hidden(p: &PlayerState) -> bool {
    matches!(p.possess.stage, PossessStage::PrePossess | PossessStage::Active)
}

// ---------------------------------------------------------------------------------------
// Devouring Swarm

/// Rat speed: `MaxSpeed` of `RatSwarm.CrowdAgent_SmallRat` (the archetype of
/// `RatSwarm.RatCrowdAgentList`).
pub const RAT_SPEED: f32 = crate::rats::RAT_SPEED;

/// Summon point: along the view up to `m_fMaxDistance`, dropped onto the floor.
pub fn summon_swarm(state: &mut WorldState, caster: PlayerId, world: &dyn WorldQuery, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let Some(p) = state.player(caster).copied() else { return };
    let level = p.inv.power_levels[power::DEVOURING_SWARM].clamp(1, 2) - 1;
    let l = cfg.items.swarm.levels[level as usize];
    let eye = eye_position(&p, &cfg.movement);
    let dir = view_vector(p.yaw, p.pitch);
    let mut pos = match world.raycast(eye, dir, l.max_distance, channel::PAWN) {
        Some(h) => h.point + h.normal * 10.0,
        None => eye + dir * l.max_distance,
    };
    if let Some(g) = world.raycast(pos, Vec3::NEG_Z, 5000.0, channel::PAWN) {
        pos = g.point + Vec3::Z * 10.0;
    }
    let id = state.next_entity_id;
    state.next_entity_id = state.next_entity_id.wrapping_add(1);
    state.swarms.push(SwarmState { id, owner: caster, level, position: pos, spawn_delay: cfg.items.swarm.spawn_delay, age: 0.0, rats: l.rat_count, target: None, attack_time: 0.0, bite_timer: 0.0, eating: None, eat_time: 0.0 });
    events.push(SimEvent::SwarmSummoned { id: caster, swarm: id, position: pos });
}

/// Devouring Swarm as one attack group, in the order of the swarm's Attack state (the state
/// machine 0x00C65380 takes the first of Hidden, Flee, Attack, Move, Idle whose condition holds,
/// then runs its update): the condition (0x00C87EA0) keeps or drops the group's target and turns
/// a group whose target died into an eating one; detection picks a target when there is none; the
/// update (0x00C89EE0) chases while no rat is in reach, else bites a living target (0x00C891E0:
/// after `m_fInitialDelayBeforeDealingDamage`, none on the player, every `m_fDamageTimeInterval`
/// `clamp(rats x m_fDamagePerBite, m_fMinDamage, m_fMaxDamage)`) or eats a body (0x00C6D760).
pub fn step_swarms(state: &mut WorldState, dt_world: f32, cfg: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let mv = &cfg.movement;
    let vitals = &cfg.vitals;
    let mut swarms = std::mem::take(&mut state.swarms);
    for s in swarms.iter_mut() {
        if s.spawn_delay > 0.0 {
            s.spawn_delay -= dt_world;
            continue;
        }
        let l = cfg.items.swarm.levels[s.level as usize];
        s.age += dt_world;
        let targetable = |p: &PlayerState| p.id != s.owner && p.kind != PawnKind::Rat && !p.no_body && p.carried_by.is_none() && !is_hidden(p);
        // The Attack state's condition (0x00C87EA0) keeps the group's target while it is valid:
        // a living target is dropped beyond `m_fPawnEscapeRadius`; a target that became a corpse
        // is eaten when the group has `m_EatRequiredRatCount` rats, else dropped.
        if let Some(tid) = s.target {
            let keep = state.player(tid).is_some_and(|p| targetable(p) && (p.is_body() || p.position.distance(s.position) <= l.escape_radius));
            if !keep {
                s.target = None;
                s.eating = None;
            } else if s.eating != Some(tid) && state.player(tid).is_some_and(|p| p.is_body()) {
                if s.rats >= l.eat_required_rats {
                    // A group that was attacking (biting in reach) starts its eating clock at
                    // `-m_fEatStartupDelayAfterKill` (0x00C89EE0).
                    s.eat_time = crate::rats::eat_clock_start(s.attack_time > 0.0, l.eat_delay_after_kill);
                    s.eating = Some(tid);
                } else {
                    s.target = None;
                }
            }
        }
        // Detection: a swarm without a target takes the nearest pawn but the summoner within
        // `m_fPawnDetectionRadius` (bodies are eaten, the living bitten).
        if s.target.is_none() {
            let mut best: Option<(PlayerId, f32)> = None;
            for p in &state.players {
                if !targetable(p) {
                    continue;
                }
                let d = p.position.distance(s.position);
                if d <= l.detection_radius && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((p.id, d));
                }
            }
            s.attack_time = 0.0;
            s.bite_timer = 0.0;
            s.eating = None;
            s.eat_time = 0.0;
            if let Some((tid, _)) = best {
                s.target = Some(tid);
                if state.player(tid).is_some_and(|p| p.is_body()) && s.rats >= l.eat_required_rats {
                    s.eating = Some(tid);
                    s.eat_time = crate::rats::eat_clock_start(false, l.eat_delay_after_kill);
                }
            }
        }
        let Some(tid) = s.target else { continue };
        let tp = *state.player(tid).unwrap();
        let d = tp.position.distance(s.position);
        let tpos = tp.position - Vec3::Z * crate::movement::half_height(&tp, mv);
        // Rats bite within `m_fAttackDistance` (120, `Twk_Rat`); a player standing within 200,
        // sneaking within 60 (`m_fStand/CrouchedRatSwarmAttackDistance`).
        let reach = if tp.is_body() { crate::rats::RAT_ATTACK_DISTANCE } else { crate::rats::attack_distance(&tp) };
        if d > reach {
            // No rat in reach: the group chases and its damage interval counter restarts.
            let step = (tpos - s.position).normalize_or_zero() * RAT_SPEED * dt_world;
            s.position += step;
            s.bite_timer = 0.0;
            continue;
        }
        if s.eating == Some(tid) {
            // Eating (0x00C6D760): the clock runs with the required rats in reach; a dead body
            // takes the startup and one interval per limb plus a last one.
            let total = crate::rats::eat_duration(!tp.alive, crate::rats::CORPSE_LIMBS, l.eat_startup, l.eat_per_limb);
            s.eat_time += dt_world;
            if s.eat_time >= total {
                if let Some(p) = state.player_mut(tid) {
                    if p.alive {
                        crate::vitals::kill(p, Some(s.owner), DamageCause::Rats, events);
                    }
                    p.no_body = true;
                }
                events.push(SimEvent::BodyRemoved { body: tid, ash: false });
                s.eating = None;
                s.target = None;
                s.eat_time = 0.0;
            }
            continue;
        }
        let t = state.player_mut(tid).unwrap();
        // Attack group (0x00C891E0): after the initial delay (none on the player), every interval
        // (accumulated) one bite of clamp(rats x per-bite, min, max) as an int, plus the slowdown.
        s.attack_time += dt_world;
        if s.attack_time <= l.initial_damage_delay && t.kind != PawnKind::Human {
            continue;
        }
        s.bite_timer += dt_world;
        let tweak = crate::rats::RatAttackTweak { damage_per_bite: l.damage_per_bite, min_damage: l.min_damage, max_damage: l.max_damage, interval: l.damage_interval, ..Default::default() };
        while s.bite_timer > l.damage_interval && t.alive {
            s.bite_timer -= l.damage_interval;
            let dmg = crate::rats::group_damage(s.rats as usize, &tweak);
            t.rat_slow = tweak.slowdown_duration;
            t.rat_slow_percent = tweak.slowdown_percent;
            apply_damage(t, dmg, Some(s.owner), DamageCause::Rats, events, vitals);
            events.push(SimEvent::Sound { position: s.position, radius: 800.0, category: SoundCategory::BodyFall, source: s.owner });
        }
    }
    swarms.retain(|s| s.spawn_delay > 0.0 || s.age < cfg.items.swarm.levels[s.level as usize].duration);
    state.swarms = swarms;
}

// ---------------------------------------------------------------------------------------
// Shadow Kill

/// Whether a kill by `killer` turns the corpse to ash (0x00A87B60): Shadow Kill level 2 always;
/// level 1 only for stealth kills (the original tests the victim's awareness; the sim uses the
/// stealth kinds: assassinations, kills of sleepers).
pub fn shadow_kill_applies(killer: &PlayerState, victim: &PlayerState, cause: DamageCause, victim_was_unconscious: bool) -> bool {
    if victim.kind == PawnKind::Human {
        return false;
    }
    // 0x00A87B60: level 2 always; level 1 when the victim's awareness is below 1 (unaware, the AI's
    // `ai_unaware` hook) or it was out (sleeping / knocked out); never for rat bites.
    if matches!(cause, DamageCause::Rats | DamageCause::RatBite) {
        return false;
    }
    match killer.inv.power_levels[power::SHADOW_KILL] {
        0 => false,
        1 => victim_was_unconscious || victim.ai_unaware,
        _ => true,
    }
}

/// Mana check and spend for a power (`m_ManaCost`).
pub fn try_spend(p: &mut PlayerState, cost: f32, vitals: &VitalsConfig) -> bool {
    spend_mana(p, cost, vitals)
}
