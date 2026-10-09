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

use crate::events::{DamageCause, SimEvent};
use crate::movement::{MoveCtx, eye_position, forward_vector, view_vector};
use crate::state::{Condition, PawnKind, PlayerId, PlayerState, PossessStage, SwarmState, TimeBendState, WorldState, power};
use crate::vitals::spend_mana;
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
        for (i, (un, attrs)) in items.upgrades.iter().enumerate().take(64) {
            if p.inv.upgrades & (1u64 << i) != 0 && host(un) {
                v += attrs.iter().filter(|(n, _)| n == name).map(|(_, x)| *x).sum::<f32>();
            }
        }
    }
    if p.inv.charms_equipped != 0 {
        for slot in 0..charm_slots(cfg) {
            let Some(c) = charm(cfg, slot) else { continue };
            if p.inv.charms_equipped & (1u64 << slot) != 0 && c.attribute == name {
                v += charm_value(p, cfg, slot);
            }
        }
    }
    v
}

/// Whether an equipped bone charm modifies the attribute: the HUD's "small upgrade" gauge bits
/// ask the attribute for a modifier named "Whale Bone Charm <attribute>" (0x00B9F6E0 with
/// `Attribute_HealthMax` / `Attribute_ManaMax`, `FUN_00C7E1C0` on the modifier list).
pub fn charm_modifies(p: &PlayerState, cfg: &GameplayConfig, name: &str) -> bool {
    p.inv.charms_equipped != 0 && (0..charm_slots(cfg)).any(|slot| p.inv.charms_equipped & (1u64 << slot) != 0 && charm(cfg, slot).is_some_and(|c| c.attribute == name))
}

/// Whether an ammo type is unlocked for the wheel (`FUN_00C1C9D0`, asked by the content builder
/// 0x00BD5530 for each ammo type of a weapon): explosive bullets need a non-zero
/// `Attribute_ExplosiveBulletUnlock`, sticky grenades `Attribute_StickyGrenadeUnlock`; every
/// other type is always listed.
pub fn ammo_unlocked(p: &PlayerState, cfg: &GameplayConfig, t: dishonored_ir::items::AmmoType) -> bool {
    use dishonored_ir::items::AmmoType;
    match t {
        AmmoType::BulletExplosive => attribute(p, cfg, "Attribute_ExplosiveBulletUnlock", 0.0) != 0.0,
        AmmoType::StickyGrenade => attribute(p, cfg, "Attribute_StickyGrenadeUnlock", 0.0) != 0.0,
        _ => true,
    }
}

/// Charm slots: `m_Charms` then `m_ScriptedCharms` (at most 64, the inventory's bit width).
pub fn charm_slots(cfg: &GameplayConfig) -> usize {
    (cfg.items.charms.len() + cfg.items.scripted_charms.len()).min(64)
}

/// The charm of a slot.
pub fn charm(cfg: &GameplayConfig, slot: usize) -> Option<&dishonored_ir::items::BoneCharm> {
    let n = cfg.items.charms.len();
    if slot < n { cfg.items.charms.get(slot) } else { cfg.items.scripted_charms.get(slot - n) }
}

/// The slot of a charm effect id (0x00AA8EF0: below `SCRIPTED_CHARM_BASE` an index into
/// `m_Charms`, from it up into `m_ScriptedCharms`).
pub fn charm_slot(cfg: &GameplayConfig, effect: i32) -> Option<usize> {
    let base = dishonored_ir::items::SCRIPTED_CHARM_BASE;
    let n = cfg.items.charms.len();
    let slot = match effect {
        e if e < 0 => return None,
        e if e < base => Some(e as usize).filter(|i| *i < n)?,
        e => Some((e - base) as usize).filter(|j| *j < cfg.items.scripted_charms.len())? + n,
    };
    (slot < 64).then_some(slot)
}

/// The value an owned charm gives: `m_Levels[level].m_fValue` at its current level (the
/// owned entry's level byte, 0x00AB9330).
pub fn charm_value(p: &PlayerState, cfg: &GameplayConfig, slot: usize) -> f32 {
    let Some(c) = charm(cfg, slot) else { return 0.0 };
    let level = if p.inv.charms_level2 & (1u64 << slot) != 0 { 1 } else { 0 };
    c.values.get(level).or(c.values.last()).copied().unwrap_or(0.0)
}

/// Take a bone charm effect (0x00ABBA90): a charm not owned yet is added at level 0 (and
/// equipped while a slot is free, `m_bAutoEquipBoneCharm`); one already owned goes up a level
/// (to the "II" values), never past its last level.
pub fn take_charm(p: &mut PlayerState, cfg: &GameplayConfig, slot: usize) {
    let bit = 1u64 << slot;
    if p.inv.charms_owned & bit != 0 {
        if charm(cfg, slot).is_some_and(|c| c.values.len() > 1) {
            p.inv.charms_level2 |= bit;
        }
        return;
    }
    p.inv.charms_owned |= bit;
    if (p.inv.charms_equipped.count_ones() as u8) < max_charms(p, cfg) {
        p.inv.charms_equipped |= bit;
    }
}

/// Give a workshop upgrade by name (`Twk_Upgrade_<name>`, e.g. `GrenadeBelt`).
pub fn give_upgrade(p: &mut PlayerState, name: &str, cfg: &GameplayConfig) -> bool {
    match cfg.items.upgrades.iter().take(64).position(|(n, _)| n.eq_ignore_ascii_case(name)) {
        Some(i) => {
            p.inv.upgrades |= 1u64 << i;
            true
        }
        None => false,
    }
}

fn any_modifiers(p: &PlayerState) -> bool {
    p.inv.charms_equipped != 0 || p.inv.upgrades != 0 || p.inv.power_levels[power::VITALITY] > 0 || p.inv.power_levels[power::AGILITY] > 0 || p.inv.power_levels[power::BLOOD_THIRSTY] > 0
}

/// `Attribute_HealthMax`: the player's (Corvo 70, with its modifiers) or a guard's (the guard
/// archetype's pawn tweak, 50). Other NPCs take their visual archetype's pawn tweak
/// (`AiConfig::archetype_health_max`: party aristocrats 11, the female aristocrat 12); an
/// original NPC's spawn sets its own tweak's value (the mission host).
pub fn health_max(p: &PlayerState, cfg: &GameplayConfig) -> f32 {
    let base = match p.kind {
        PawnKind::Guard => cfg.ai.combat.guard_health_max,
        PawnKind::Civilian => cfg.ai.archetype_health_max.get(p.archetype as usize).copied().unwrap_or(cfg.vitals.health_max),
        _ => cfg.vitals.health_max,
    };
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
    set_power_level(p, idx, p.inv.power_levels[idx] + 1, cfg);
    true
}

/// `SetPowerLevel` (0x00ACE270) without the heal flag: the current level's modifiers are
/// replaced by the new level's, the stored `HealthMax` is recomputed (whole points) and, when it
/// grew, current health grows by the same amount (Vitality 1 gives +20 health at once; 1 -> 2
/// keeps the same maximum and gives nothing).
pub fn set_power_level(p: &mut PlayerState, idx: usize, level: u8, cfg: &GameplayConfig) {
    let before = health_max(p, cfg).trunc();
    p.inv.power_levels[idx] = level;
    let after = health_max(p, cfg).trunc();
    if after > before {
        p.vitals.health += after - before;
    }
}

/// Equip or unequip an owned bone charm (`m_MaxActivatedBoneCharmCount` slots).
pub fn set_charm(p: &mut PlayerState, i: usize, on: bool, cfg: &GameplayConfig) -> bool {
    if i >= 64 || p.inv.charms_owned & (1u64 << i) == 0 {
        return false;
    }
    if on {
        if p.inv.charms_equipped & (1u64 << i) == 0 && p.inv.charms_equipped.count_ones() as u8 >= max_charms(p, cfg) {
            return false;
        }
        p.inv.charms_equipped |= 1u64 << i;
    } else {
        p.inv.charms_equipped &= !(1u64 << i);
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

/// Rate of time for a pawn. The GameInfo tick (0x009FCBC0) multiplies the channels' rates: the
/// player rate (`WorldInfo.TimeDilation`) is the product of the channels' player dilations and
/// the world rate the product of their world dilations (no channel the powers use is
/// `m_bExclusive`). Per caster here: a caster runs at its own dilation, everything else at the
/// world dilation of every active bend (several casters multiply) and of every other pawn's
/// Blink layer (Blink passes a caster rate of 1).
pub fn pawn_time_scale(state: &WorldState, id: PlayerId, cfg: &GameplayConfig) -> f32 {
    let mut s = 1.0;
    for b in &state.time_bends {
        s *= if b.caster == id { ramp(b, b.self_dilation, cfg) } else { ramp(b, b.world_dilation, cfg) };
    }
    for l in &state.blink_layers {
        if l.wheel || l.caster != id {
            s *= l.current;
        }
    }
    s
}

/// The GameInfo's player dilation (`m_fCurrentPlayerTimeDilation` +0x3ac, which it writes to
/// `WorldInfo.TimeDilation`): the product of the channels' player dilations. Bend Time's channel
/// carries its caster dilation; Blink's passes a caster rate of 1 (0x009FCBC0).
pub fn player_time_dilation(state: &WorldState, cfg: &GameplayConfig) -> f32 {
    let mut s = 1.0;
    for b in &state.time_bends {
        s *= ramp(b, b.self_dilation, cfg);
    }
    // The wheel's channel passes the same rate as caster and world (0x00BE4280).
    for l in state.blink_layers.iter().filter(|l| l.wheel) {
        s *= l.current;
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
    for l in &state.blink_layers {
        s *= l.current;
    }
    s
}

/// The dilation helper (0x009FA0D0) for a caster's Blink layer: a non-zero target is kept
/// within [0.05, caster rate 1]; the layer moves from where it is to the target at
/// `(target - current) / blend` per second (at once when the blend is 0).
pub fn set_blink_layer(state: &mut WorldState, caster: PlayerId, target: f32, blend: f32) {
    set_layer(state, caster, false, target, blend);
}

/// `DisGFxMoviePlayerPowerWheel.m_fBendTimeFactor` (class default 0.05): the world rate while
/// the quick-access wheel is open (0x00BE4280 calls the dilation helper with channel 6 and
/// this factor for the caster, the world and the third rate; 0x00BD4FC0 sets 1 back).
pub const WHEEL_TIME_FACTOR: f32 = 0.05;
/// `m_fTimeTransition`: the blend time of both calls. It has no default and no tweak value
/// (0x00BE4280 passes +0x1BC straight on), so the rate changes at once.
pub const WHEEL_TIME_TRANSITION: f32 = 0.0;

/// The wheel's dilation channel for `caster`: `open` takes the world to
/// [`WHEEL_TIME_FACTOR`], closing sets it back to 1. Only a world with a single human pawn
/// honours it: the original wheel needs the GameInfo (`FUN_00781020`, present on the server /
/// standalone only) and Blink's multiplayer worlds never hand one player the others' clock.
pub fn set_wheel_layer(state: &mut WorldState, caster: PlayerId, open: bool) {
    let single = state.players.iter().filter(|p| p.kind == PawnKind::Human).count() <= 1;
    set_layer(state, caster, true, if open && single { WHEEL_TIME_FACTOR } else { 1.0 }, WHEEL_TIME_TRANSITION);
}

fn set_layer(state: &mut WorldState, caster: PlayerId, wheel: bool, target: f32, blend: f32) {
    let target = if target == 0.0 { 0.0 } else { target.clamp(0.05, 1.0) };
    let i = match state.blink_layers.iter().position(|l| l.caster == caster && l.wheel == wheel) {
        Some(i) => i,
        None => {
            if target == 1.0 {
                return;
            }
            state.blink_layers.push(crate::state::DilationLayer { caster, wheel, current: 1.0, target: 1.0, velocity: 0.0 });
            state.blink_layers.len() - 1
        }
    };
    let l = &mut state.blink_layers[i];
    l.target = target;
    l.velocity = if blend.abs() >= 1e-8 { (target - l.current) / blend } else { f32::MAX };
}

/// The GameInfo tick's layer step (0x009FCBC0, real time): `current += velocity x dt`, landing
/// on the target when it is reached or passed. Layers back at 1 are dropped.
pub fn step_blink_layers(state: &mut WorldState, dt: f32) {
    for l in state.blink_layers.iter_mut() {
        if l.velocity == f32::MAX {
            l.current = l.target;
            continue;
        }
        let old = l.current;
        let new = old + l.velocity * dt;
        l.current = new;
        if (l.target - new) * (l.target - old) <= 0.0 {
            l.current = l.target;
            l.velocity = f32::MAX;
        }
    }
    state.blink_layers.retain(|l| !(l.current == 1.0 && l.target == 1.0));
}

/// Advance bends; end those past their duration. A bend's clock is
/// `m_fBendTimePowerRealTimeSeconds` (0x009E9E80): real time scaled by the world dilation of
/// the channels flagged `m_bAffectsBendTimePower`, which Blink's is and Bend Time's own is not
/// (0x009FCBC0): the caster's Blink slows its own Bend Time clock.
pub fn step_time_bends(state: &mut WorldState, dt: f32, events: &mut Vec<SimEvent>) {
    for b in state.time_bends.iter_mut() {
        let k: f32 = state.blink_layers.iter().filter(|l| !l.wheel && l.caster == b.caster).map(|l| l.current).product();
        b.elapsed += dt * k;
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

/// Field of view the targeting cone and score use (`DishonoredPlayerCamera.m_fCurFOV`, degrees):
/// the default horizontal FOV, the sim's fixed reference view (as for the gun dispersion).
pub const POSSESS_REF_FOV_DEG: f32 = crate::left_hand::DISPERSION_REF_HFOV_DEG;

/// Possession's targeting geometry for `p` (0x00BFB090 / 0x00BF8E20): the eye, the view, the
/// range `m_fPossessRange x (1 + Attribute_SpellStrength)` (at least 1), the cone half-angle
/// `m_fMaxAngleFOVFraction x FOV` and the score's angle normaliser, the FOV (at least 0.1 rad).
struct PossessAim {
    eye: Vec3,
    dir: Vec3,
    range: f32,
    cone: f32,
    fov: f32,
}

fn possess_aim(p: &PlayerState, cfg: &GameplayConfig) -> PossessAim {
    let pc = &cfg.items.possess;
    let level = (p.inv.power_levels[power::POSSESSION] as usize).clamp(1, 2);
    let strength = attribute(p, cfg, "Attribute_SpellStrength", 0.0);
    let fov = POSSESS_REF_FOV_DEG.to_radians();
    PossessAim {
        eye: eye_position(p, &cfg.movement),
        dir: view_vector(p.yaw, p.pitch),
        range: (pc.range[level - 1] * (1.0 + strength)).max(1.0),
        cone: pc.max_angle_fov_fraction * fov,
        fov: fov.max(0.1),
    }
}

impl PossessAim {
    /// The candidate's score (higher wins, 0x00BFCAF0): `m_fAngleWeight x (fov - angle) / fov +
    /// m_fDistanceWeight x (range - d) / range + m_TargetingPriorityWeights[priority]`; None when
    /// it is out of range or outside the cone.
    fn score(&self, to: Vec3, priority: u8, cfg: &GameplayConfig) -> Option<f32> {
        let pc = &cfg.items.possess;
        let d = to.length();
        if d > self.range || d < 1.0 {
            return None;
        }
        let ang = (to.dot(self.dir) / d).clamp(-1.0, 1.0).acos();
        if ang > self.cone {
            return None;
        }
        let prio = pc.priority_weights.get(priority as usize).copied().unwrap_or(0.0);
        Some(pc.angle_weight * (self.fov - ang) / self.fov + pc.distance_weight * (self.range - d) / self.range + prio)
    }
}

/// Pawn id for the next rat a Devouring Swarm releases.
pub fn swarm_rat_id(state: &WorldState) -> PlayerId {
    let mut id = 0x4000_0000 | state.next_entity_id;
    while state.player(id).is_some() {
        id = id.wrapping_add(1);
    }
    id
}

/// Possession target under the crosshair (targeting stage, 0x00BFCEC0 -> 0x00BFCAF0): the
/// candidates within range and inside the cone (0x00BF8E20), visible, with the highest
/// [`PossessAim::score`]. The original tests the host's sample points and keeps the nearest
/// distance and smallest angle; the sim uses the pawn's centre.
pub fn possess_target(state: &WorldState, p: &PlayerState, world: &dyn WorldQuery, cfg: &GameplayConfig) -> Option<PlayerId> {
    possess_target_scored(state, p, world, cfg).map(|b| b.0)
}

fn possess_target_scored(state: &WorldState, p: &PlayerState, world: &dyn WorldQuery, cfg: &GameplayConfig) -> Option<(PlayerId, f32)> {
    let aim = possess_aim(p, cfg);
    let mut best: Option<(PlayerId, f32)> = None;
    for t in &state.players {
        if !can_possess(p, t, cfg) {
            continue;
        }
        let to = t.position - aim.eye;
        let Some(score) = aim.score(to, possessable(t, cfg).targeting_priority, cfg) else { continue };
        let d = to.length();
        if world.raycast(aim.eye, to, d, channel::MELEE).is_some_and(|h| h.distance < d - cfg.movement.collision_radius) {
            continue;
        }
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((t.id, score));
        }
    }
    best
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
                // The possessor's running Bend Time gets `m_fBendTimeExtension` more (0x00BFB6C0 ->
                // 0x00BFA120) unless it is already within `m_fPostEffectCooldownTime` of its end.
                let ext = cfg.items.possess.bend_time_extension[level - 1];
                if let Some(b) = state.time_bends.iter_mut().find(|b| b.caster == p.id) {
                    let post = cfg.items.bend_time.levels[b.level as usize].post_effect_cooldown;
                    if b.elapsed < b.duration - post {
                        b.duration += ext;
                    }
                }
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
    state.swarms.push(SwarmState { id, owner: caster, level, position: pos, spawn_delay: cfg.items.swarm.spawn_delay, age: 0.0, rats: l.rat_count, release_ticks: 0, ending: false, rats_at_end: 0 });
    events.push(SimEvent::SwarmSummoned { id: caster, swarm: id, position: pos });
    // The swarm component's activation (0x00BFAE60) calls the use notification (slot 94) once
    // its destination is valid.
    events.push(SimEvent::PowerUsed { id: caster, power: power::DEVOURING_SWARM as u8, level: p.inv.power_levels[power::DEVOURING_SWARM] });
}

/// The Devouring Swarm actor (`DisRatSwarm` with its `DisRatSwarmSpawner`), MEASURED 2026-10-08:
/// it appears `m_fRatSwarmSpawnDelay` after the cast notify, its spawner releases the level's
/// `m_RatCount` rats as individual crowd agents over 16 ticks, they live `m_fDuration`, then the
/// swarm ends and its rats despawn over about 10 s. Each rat here is a rat pawn
/// ([`crate::rats::spawn_rat`] with the swarm's id): a brain (`dishonored_ai` rat swarm brains,
/// with the Devouring Swarm tweak, sparing the caster) drives it, the attack groups
/// ([`crate::rats::step_rat_attacks`]) bite with the level's damage values. The release spot of
/// each rat around the swarm point is not decoded (the spawner's crowd placement): the sim
/// spreads them on a sunflower spiral of rat-sized spacing.
pub fn step_swarms(state: &mut WorldState, dt_world: f32, cfg: &crate::SimConfig, events: &mut Vec<SimEvent>) {
    let g = &cfg.gameplay;
    let mut swarms = std::mem::take(&mut state.swarms);
    let mut gone: Vec<u32> = Vec::new();
    for s in swarms.iter_mut() {
        if s.spawn_delay > 0.0 {
            s.spawn_delay -= dt_world;
            if s.spawn_delay > 0.0 {
                continue;
            }
            s.release_ticks = crate::state::SWARM_RELEASE_TICKS;
        }
        let l = g.items.swarm.levels[(s.level as usize).min(1)];
        // Release: the rats left spread evenly over the release ticks left.
        if s.rats > 0 && s.release_ticks > 0 {
            let n = s.rats.div_ceil(s.release_ticks as u16);
            let caster_yaw = state.player(s.owner).map_or(0.0, |p| p.yaw);
            for _ in 0..n {
                let k = (l.rat_count - s.rats) as f32;
                let a = k * 2.399_963; // golden angle
                let r = 2.0 * crate::movement::RAT_RADIUS * k.sqrt();
                let pos = s.position + Vec3::new(r * a.cos(), r * a.sin(), crate::movement::RAT_HALF_HEIGHT);
                let id = swarm_rat_id(state);
                state.next_entity_id = state.next_entity_id.wrapping_add(1);
                crate::rats::spawn_rat(state, id, pos, caster_yaw, Some(s.id), cfg, events);
                s.rats -= 1;
            }
            s.release_ticks -= 1;
        }
        s.age += dt_world;
        let sid = s.id;
        let members = |state: &WorldState| state.players.iter().filter(|p| p.kind == PawnKind::Rat && p.swarm == Some(sid)).map(|p| p.id).collect::<Vec<_>>();
        if !s.ending && s.age >= l.duration {
            s.ending = true;
            s.rats = 0;
            s.rats_at_end = members(state).len() as u16;
        }
        if s.ending {
            // Despawn over the tail: keep ceil(n x (1 - f)) of the rats there were at the end.
            let f = ((s.age - l.duration) / crate::state::SWARM_DESPAWN_SECONDS).clamp(0.0, 1.0);
            let keep = (s.rats_at_end as f32 * (1.0 - f)).ceil() as usize;
            let mut ids = members(state);
            if ids.len() > keep {
                ids.sort_unstable();
                let drop: Vec<PlayerId> = ids.split_off(keep);
                state.players.retain(|p| !drop.contains(&p.id));
            }
            if f >= 1.0 {
                gone.push(s.id);
            }
        }
    }
    swarms.retain(|s| !gone.contains(&s.id));
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
