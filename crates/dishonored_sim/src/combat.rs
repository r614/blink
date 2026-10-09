//! Sword combat (M9): swing phases, block, parry, back and drop assassinations
//! (research/dishonored/systems/melee.md).
//!
//! Hit geometry (range 210, sweep width 60, assassinate range 275) is CONFIRMED tweak data. A hit
//! deals `appCeil(m_fAttackDamageMultiplier x (Attribute_MeleeDamage + Attribute_MeleeDamageBonus))`
//! (0x00C131C0 -> slot 132: 0x00C07C80, NPC 0x00C33DD0); the health ratios 0.2 / 0.7 only pick the
//! contact effect. The player's swing phases come from its attack clips' notifies (the hit
//! lands in the damage zone at the end of the attack zone, U-590); stagger and recoil are
//! SPECULATIVE (see `CombatTuning`).
//! Simulation owns timing; animation only presents it.

use crate::events::{DamageCause, SimEvent, SoundCategory};
use crate::input::{PlayerInput, buttons};
use crate::movement::{eye_position, forward_vector, half_height, view_vector};
use crate::state::{Condition, MoveMode, PawnKind, PlayerId, PlayerState, SwordPhase, WorldState};
use crate::tuning::CombatTuning;
use crate::vitals::{apply_damage, kill};
use dishonored_ir::ai::DIFFICULTY_NORMAL;
use dishonored_ir::gameplay::{GameplayConfig, MovementConfig};
use dishonored_world::{WorldQuery, channel, geom::closest_segment_segment};
use glam::Vec3;

/// Advance the attacker-side state machine and collect swing requests.
pub fn update_sword(p: &mut PlayerState, input: &PlayerInput, t: &CombatTuning, dt: f32, events: &mut Vec<SimEvent>) {
    let (swing_startup, swing_active, swing_recovery) = swing_phases(p, t);
    let s = &mut p.sword;
    s.timer += dt;
    match s.phase {
        SwordPhase::Idle => {
            if !p.alive || matches!(p.mode, MoveMode::Mantling | MoveMode::Blinking) {
                return;
            }
            if input.pressed(buttons::BLOCK) {
                s.phase = SwordPhase::Blocking;
                s.timer = 0.0;
                s.block_time = 0.0;
            } else if input.buttons & buttons::PRIMARY != 0 && p.prev_buttons & buttons::PRIMARY == 0 {
                s.phase = SwordPhase::Startup;
                s.timer = 0.0;
                s.hit_done = false;
                s.hit_count = 0;
                s.offscreen_to = None;
                // The item context the brain swings with (players: none).
                s.npc_move = input.npc_move;
                s.swing_clip = if p.crouched { 4 } else { s.swings % 4 };
                s.swings = s.swings.wrapping_add(1);
                events.push(SimEvent::SwordSwing { id: p.id });
            }
        }
        SwordPhase::Blocking => {
            s.block_time += dt;
            s.force_block -= dt;
            if !input.pressed(buttons::BLOCK) && s.force_block <= 0.0 {
                s.phase = SwordPhase::Idle;
                s.timer = 0.0;
                s.force_block = 0.0;
            }
        }
        SwordPhase::Startup if s.timer >= swing_startup => {
            s.phase = SwordPhase::Active;
            s.timer = 0.0;
        }
        SwordPhase::Active if s.timer >= swing_active => {
            s.phase = SwordPhase::Recovery;
            s.timer = 0.0;
        }
        SwordPhase::Recovery if s.timer >= swing_recovery => {
            s.phase = SwordPhase::Idle;
            s.timer = 0.0;
        }
        SwordPhase::Staggered if s.timer >= t.stagger_time => {
            s.phase = SwordPhase::Idle;
            s.timer = 0.0;
        }
        SwordPhase::Assassinating if s.timer >= s.lock_time => {
            s.phase = SwordPhase::Idle;
            s.timer = 0.0;
            s.assassination_target = None;
        }
        _ => {}
    }
}

/// Phase lengths of the current swing. The player's come from its attack clip: the hit is
/// only possible in the damage zone at the end of the clip's attack zone (U-590, see
/// [`crate::tuning::SwingTiming`]). AI pawns keep the uniform port values (U-600).
pub fn swing_phases(p: &PlayerState, t: &CombatTuning) -> (f32, f32, f32) {
    match p.kind {
        PawnKind::Human => t.player_swings[(p.sword.swing_clip as usize).min(4)].phases(),
        _ => (t.swing_startup, t.swing_active, t.swing_recovery),
    }
}

pub fn sword_locks_movement(p: &PlayerState) -> bool {
    matches!(p.sword.phase, SwordPhase::Staggered | SwordPhase::Assassinating)
}

/// Melee reach (0x00C0A060 with 0x00AB2050): `m_fMaxContextRange` scaled by the forward-speed
/// factor `1 + clamp((v . view - m_fMinSpeedRayScale) * m_fRaySpeedScale, 0, m_fRaySpeedScale_Max)`
/// blended by `m_fRayScalePercent`: `range * ((factor - 1) * percent + 1)`. Sprinting forward
/// at 540 uu/s reaches 210 x 1.35 = 283.5 uu.
pub fn reach(a: &PlayerState, t: &CombatTuning) -> f32 {
    context_reach(a, t, t.attack_range, t.reach_scale_percent)
}

/// [`reach`] of another player context (assassinate 275 x 1.25, drop assassinate 250 x 2.0:
/// each context's own `m_fMaxContextRange` and `m_fRayScalePercent`).
pub fn context_reach(a: &PlayerState, t: &CombatTuning, range: f32, percent: f32) -> f32 {
    let along = a.velocity.dot(view_vector(a.yaw, a.pitch));
    if t.reach_speed_scale <= 0.0 || along <= t.reach_min_speed || percent <= 1e-4 {
        return range;
    }
    let factor = 1.0 + ((along - t.reach_min_speed) * t.reach_speed_scale).clamp(0.0, t.reach_speed_scale_max);
    range * ((factor - 1.0) * percent + 1.0)
}

/// Distance from the attacker's swing segment to the victim's body axis.
fn body_distance(eye: Vec3, dir: Vec3, range: f32, v: &PlayerState, mv: &MovementConfig) -> (f32, f32) {
    let hh = half_height(v, mv);
    let a0 = v.position - Vec3::Z * (hh - mv.collision_radius);
    let a1 = v.position + Vec3::Z * (hh - mv.collision_radius);
    let (on_swing, on_body) = closest_segment_segment(eye, eye + dir * range, a0, a1);
    (on_swing.distance(on_body) - mv.collision_radius, (on_swing - eye).length())
}

/// `UDisItemContext_MeleeBlockPlayer` blocked-attack handler (0x00C22B60): the hit is
/// parried when the block started less than `m_fTapForPerfectParryWindow` ago or the
/// block animation's `DisNotify_BlockParryZone` is active (t = 0.001..0.431 s of
/// Sword_Block, U-021), unless the post-tap forced block is still running.
pub fn parries(s: &crate::state::SwordState, t: &CombatTuning) -> bool {
    if s.force_block > 1e-8 {
        return false;
    }
    let tap = s.block_time < t.parry_tap_window;
    let zone = s.block_time >= 0.001 && s.block_time <= 0.001 + t.parry_window;
    tap || zone
}

fn angle_deg(a: Vec3, b: Vec3) -> f32 {
    a.normalize_or_zero().dot(b.normalize_or_zero()).clamp(-1.0, 1.0).acos().to_degrees()
}

/// UE3 `appCeil` as the hit handler (0x00C131C0) does it: x87 round-to-nearest-even of
/// `-0.5 - 2d`, arithmetic shift right once, negated.
pub fn app_ceil(d: f32) -> f32 {
    let r = (-0.5 - 2.0 * d).round_ties_even() as i32;
    -(r >> 1) as f32
}

/// The damage function of the attack context (slot 132): the player's 0x00C07C80
/// `m_fAttackDamageMultiplier x (Attribute_MeleeDamage of the sword + the wielder's
/// Attribute_MeleeDamageBonus)`; NPCs (0x00C33DD0) the same with the guards' sword and the move's
/// context multiplier, less `m_fOffscreenMeleeDamageReduction` (at least 1) on a player the swing
/// started offscreen for and that still does not see the attacker. Not yet rounded.
pub fn melee_damage(a: &PlayerState, victim: &PlayerState, offscreen: bool, g: &GameplayConfig, t: &CombatTuning) -> f32 {
    let inv = &g.items.inventory;
    if a.kind == PawnKind::Human {
        let base = crate::powers::attribute(a, g, "Attribute_MeleeDamage", inv.sword_melee_damage) + crate::powers::attribute(a, g, "Attribute_MeleeDamageBonus", inv.melee_damage_bonus);
        return t.attack_damage_multiplier * base;
    }
    let c = &g.ai.combat;
    // A special class's attack (hound bite, Tallboy stomp, Overseer bash): its own context and
    // weapon (`ceil(m_fAttackDamageMultiplier x Attribute_MeleeDamage)`, Normal).
    if let Some(s) = g.ai.special.attack(a.sword.npc_move) {
        return s.damage;
    }
    let mult = match a.sword.npc_move {
        0 => 1.0,
        // A pattern without the move (older configs): the elite sword's value for its class.
        k => c.melee_contexts.iter().find(|x| x.kind == k).map(|x| x.attack_damage_multiplier()).unwrap_or_else(|| {
            dishonored_ir::ai::elite_sword_multiplier(if k == crate::input::npc_moves::JUMP_ATTACK { "DisItemContext_NPCJumpAttack" } else { "" })
        }),
    };
    let mut d = mult * (c.npc_sword_damage[DIFFICULTY_NORMAL] + c.npc_melee_damage_bonus);
    if offscreen && d > 0.0 && victim.kind == PawnKind::Human {
        d *= 1.0 - inv.offscreen_melee_reduction[DIFFICULTY_NORMAL];
        if d <= 1.0 {
            d = 1.0;
        }
    }
    d
}

/// `m_bRenderedByPlayer` together with the view-plane test of 0x00BBEF40: whether player `v`
/// sees pawn `a` (in front of its view, inside the default 75-degree 16:9 view, line of sight
/// clear). The original reads the renderer's flag; the sim takes the view frustum and a melee
/// trace (Unverified stand-in).
pub fn seen_by(v: &PlayerState, a: &PlayerState, world: &dyn WorldQuery, mv: &MovementConfig) -> bool {
    let eye = eye_position(v, mv);
    let f = view_vector(v.yaw, v.pitch);
    let r = forward_vector(v.yaw + 90.0);
    let u = r.cross(f).normalize_or_zero();
    let d = a.position - eye;
    let x = d.dot(f);
    if x <= 1.0 {
        return false;
    }
    let tan_h = (crate::left_hand::DISPERSION_REF_HFOV_DEG.to_radians() * 0.5).tan();
    let tan_v = tan_h / crate::left_hand::DISPERSION_REF_ASPECT;
    let hh = half_height(a, mv);
    if (d.dot(r).abs() - mv.collision_radius) / x > tan_h || (d.dot(u).abs() - hh) / x > tan_v {
        return false;
    }
    let len = d.length();
    !world.raycast(eye, d, len, channel::MELEE).is_some_and(|h| h.distance < len - mv.collision_radius)
}

/// 0x00BBA970: the direction an attacker at `from` is in, in the victim's frame (the dominant
/// axis of the flattened local offset): 0 front, 1 left, 2 right, 3 back.
pub fn relative_direction(victim: &PlayerState, from: Vec3) -> usize {
    let d = (from - victim.position).truncate();
    let fwd = forward_vector(victim.yaw).truncate();
    let right = forward_vector(victim.yaw + 90.0).truncate();
    let (x, y) = (d.dot(fwd), d.dot(right));
    if x.abs() <= y.abs() {
        if y > 0.0 { 2 } else { 1 }
    } else if x >= 0.0 {
        0
    } else {
        3
    }
}

/// `FUN_00B79430 == 2`: the pawn moves at its running gait (the AI runs with Sprint).
fn running(v: &PlayerState) -> bool {
    v.last_input.buttons & buttons::SPRINT != 0 && v.velocity.truncate().length() > 1.0
}

/// Resolve sword hits for this tick. Runs after every player's state machine advanced.
pub fn resolve_sword_hits(state: &mut WorldState, world: &dyn WorldQuery, g: &GameplayConfig, t: &CombatTuning, events: &mut Vec<SimEvent>) {
    let mv = &g.movement;
    let vitals = &g.vitals;
    let noise = &g.ai.noise;
    let n = state.players.len();
    // Deterministic order: players are kept sorted by id.
    for ai in 0..n {
        let attacker = state.players[ai];
        if !attacker.alive || attacker.incapacitated() || attacker.carrying.is_some() {
            continue;
        }
        let wants_attack = attacker.sword.phase == SwordPhase::Startup && attacker.sword.timer == 0.0;
        let active = attacker.sword.phase == SwordPhase::Active;

        // Assassinations are player item contexts (`DisItemContext_Assassinate` <
        // `_Finisher` < `_MeleeAttackPlayer`; `DisItemContext_DropAssassinate`): NPC swings
        // are their own `DisItemContext_NPCAttack*` moves and never assassinate.
        if attacker.kind == PawnKind::Human && attacker.sword.phase != SwordPhase::Assassinating {
            // The drop context is evaluated every tick of the fall (0x00C14960 caches it per
            // tick); the attack fires it while the button is down.
            let held = attacker.last_input.buttons & buttons::PRIMARY != 0;
            let swinging = matches!(attacker.sword.phase, SwordPhase::Idle | SwordPhase::Startup | SwordPhase::Active | SwordPhase::Recovery);
            let found = if attacker.mode == MoveMode::Falling && (wants_attack || held && swinging) {
                find_drop_assassination(&attacker, state, world, mv, t).map(|v| (v, true))
            } else if wants_attack && attacker.mode == MoveMode::Walking {
                find_assassination(&attacker, state, world, mv, t).map(|v| (v, false))
            } else {
                None
            };
            if let Some((vi, drop)) = found {
                let vid = state.players[vi].id;
                let dir = relative_direction(&state.players[vi], attacker.position);
                let cause = if drop { DamageCause::DropAssassination } else { DamageCause::Assassination };
                {
                    let a = &mut state.players[ai];
                    a.sword.phase = SwordPhase::Assassinating;
                    a.sword.timer = 0.0;
                    a.sword.lock_time = if drop { t.drop_assassination_times[dir] } else { t.assassination_times[dir] };
                    a.sword.assassination_target = Some(vid);
                    a.sword.assassination_move = dir as u8 + if drop { 4 } else { 0 };
                    a.velocity = Vec3::ZERO;
                }
                let v = &mut state.players[vi];
                kill(v, Some(attacker.id), cause, events);
                continue;
            }
        }
        // NPC swings (0x00C24020, at the context's start): `m_bEffectivelyOffscreen` = the target is
        // a player that does not see the attacker. The sim's target is the player the NPC swings at.
        if wants_attack && attacker.kind != PawnKind::Human {
            let fwd = forward_vector(attacker.yaw);
            let target = state
                .players
                .iter()
                .filter(|p| p.kind == PawnKind::Human && p.alive && p.id != attacker.id)
                .filter(|p| (p.position - attacker.position).truncate().normalize_or_zero().dot(fwd.truncate()) > 0.0)
                .min_by(|x, y| x.position.distance_squared(attacker.position).total_cmp(&y.position.distance_squared(attacker.position)))
                .copied();
            state.players[ai].sword.offscreen_to = target.filter(|p| !seen_by(p, &attacker, world, mv)).map(|p| p.id);
        }
        if !active {
            continue;
        }
        // Every pawn the sweep meets is hit once per swing (`m_AlreadyHitActors`, the tick loop
        // 0x00C13640 calls the hit handler for each), nearest first.
        let eye = eye_position(&attacker, mv);
        let dir = view_vector(attacker.yaw, attacker.pitch);
        let range = reach(&attacker, t);
        let mut hits: Vec<(usize, f32)> = Vec::new();
        for vi in 0..n {
            let v = &state.players[vi];
            if vi == ai || !v.alive || v.carried_by.is_some() || crate::powers::is_hidden(v) || attacker.sword.already_hit(v.id) {
                continue;
            }
            let (gap, along) = body_distance(eye, dir, range, v, mv);
            if gap > t.attack_sweep_size * 0.5 {
                continue;
            }
            // Melee trace channel must be clear between the eye and the target.
            let to = v.position - eye;
            if world.raycast(eye, to, to.length(), channel::MELEE).is_some_and(|h| h.distance < to.length() - mv.collision_radius) {
                continue;
            }
            hits.push((vi, along));
        }
        hits.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (vi, _) in hits {
            // A block or parry ends the swing.
            let vid = state.players[vi].id;
            if state.players[ai].sword.phase != SwordPhase::Active || !state.players[ai].sword.record_hit(vid) {
                break;
            }
            state.players[ai].sword.hit_done = true;
            let attacker = state.players[ai];
            let victim = state.players[vi];
            let to_attacker = attacker.position - victim.position;
            let facing = angle_deg(forward_vector(victim.yaw), to_attacker) <= t.block_half_angle;
            // `m_MainAttack.m_bUnblockable` (the bash, the stomp, the hound's jump attack).
            let unblockable = attacker.kind != PawnKind::Human && g.ai.special.attack(attacker.sword.npc_move).is_some_and(|s| s.unblockable);
            if victim.sword.phase == SwordPhase::Blocking && facing && !unblockable {
                if parries(&victim.sword, t) {
                    let a = &mut state.players[ai];
                    a.sword.phase = SwordPhase::Staggered;
                    a.sword.timer = 0.0;
                    if victim.sword.block_time < t.parry_tap_window {
                        state.players[vi].sword.force_block = t.force_block_after_tap;
                    }
                    state.players[vi].sword.parries = state.players[vi].sword.parries.wrapping_add(1);
                    events.push(SimEvent::SwordParried { attacker: attacker.id, defender: victim.id });
                } else {
                    let a = &mut state.players[ai];
                    a.sword.phase = SwordPhase::Recovery;
                    a.sword.timer = -t.blocked_recoil;
                    events.push(SimEvent::SwordBlocked { attacker: attacker.id, defender: victim.id });
                }
                // The blade meets the weapon: the `Sword` -> `Weapon` contact's AI noise (a swing that
                // hits nothing makes none: there is no contact).
                crate::noise::emit(events, &noise.sword_weapon, victim.position, SoundCategory::SwordBlock, attacker.id, 0.0);
                continue;
            }
            // Blood Thirsty: with a full gauge the attack on an NPC is a finisher (0x00C10EF0).
            if attacker.adrenaline.full && victim.kind != PawnKind::Human {
                crate::adrenaline::spend(&mut state.players[ai], events);
                let v = &mut state.players[vi];
                crate::noise::emit(events, &noise.sword_body, v.position, SoundCategory::SwordImpact, attacker.id, 0.0);
                kill(v, Some(attacker.id), DamageCause::Adrenaline, events);
                continue;
            }
            // The offscreen reduction holds when the swing started offscreen for this player and
            // the attacker is still not seen (0x00C33DD0 re-tests 0x00BBEF40).
            let offscreen = attacker.sword.offscreen_to == Some(victim.id) && !seen_by(&victim, &attacker, world, mv);
            let dmg = app_ceil(melee_damage(&attacker, &victim, offscreen, g, t));
            let v = &mut state.players[vi];
            crate::noise::emit(events, &noise.sword_body, v.position, SoundCategory::SwordImpact, attacker.id, 0.0);
            apply_damage(v, dmg, Some(attacker.id), DamageCause::Sword, events, vitals);
            // Interrupt the victim's own swing.
            if v.alive && matches!(v.sword.phase, SwordPhase::Startup) {
                v.sword.phase = SwordPhase::Idle;
                v.sword.timer = 0.0;
            }
        }
    }
}

/// Whether the attacker's view takes in `v` and nothing blocks the melee trace (the sim's
/// stand-in for the context's target, `DisItemContext.m_PawnInfo_Melee`, whose selection is not
/// decoded: within 45 degrees of the view).
fn in_view(a: &PlayerState, v: &PlayerState, world: &dyn WorldQuery, mv: &MovementConfig) -> bool {
    let eye = eye_position(a, mv);
    let dir = view_vector(a.yaw, a.pitch);
    if angle_deg(dir, v.position - a.position) > 45.0 {
        return false;
    }
    let to = v.position - eye;
    !world.raycast(eye, to, to.length(), channel::MELEE).is_some_and(|h| h.distance < to.length() - mv.collision_radius)
}

/// `DisItemContext_Assassinate` target test (0x00C0D8F0) and range: an NPC within the context
/// reach (`m_fMaxContextRange` 275, `m_fRayScalePercent` 1.25, any direction:
/// `m_fAllowedAngleFromTargetDir` 360, no height limit) whose awareness the
/// `m_AssassinateOnAwareness` table allows (0x00C09060) and that is not running
/// (`m_bCanAssassinateRunners` off, 0x00C090D0). Another player (the fork's PvP rule): from
/// behind, not blocking.
/// The HUD's interaction probe `m_bCanAssassinate` (0x00BA50F0): a walking, free player whose
/// usable crosshair is on the pawn the assassinate context selects (LIKELY: the context's target
/// compared with the crosshair actor, the equality helper was not followed to its end).
pub fn can_assassinate(p: &PlayerState, state: &WorldState, world: &dyn WorldQuery, mv: &MovementConfig, t: &CombatTuning) -> bool {
    use crate::crosshair::TargetRef;
    if p.kind != PawnKind::Human || !p.alive || p.incapacitated() || p.carrying.is_some() || p.sword.phase == SwordPhase::Assassinating || p.mode != crate::state::MoveMode::Walking {
        return false;
    }
    let Some(TargetRef::Pawn(v)) = p.crosshair.filter(|c| c.usable).map(|c| c.target) else { return false };
    find_assassination(p, state, world, mv, t).is_some_and(|i| state.players[i].id == v)
}

/// `m_bCanDropAssassinate` (0x00BA5420): falling with a drop-assassination target below.
pub fn can_drop_assassinate(p: &PlayerState, state: &WorldState, world: &dyn WorldQuery, mv: &MovementConfig, t: &CombatTuning) -> bool {
    p.kind == PawnKind::Human && p.alive && p.mode == crate::state::MoveMode::Falling && find_drop_assassination(p, state, world, mv, t).is_some()
}

fn find_assassination(a: &PlayerState, state: &WorldState, world: &dyn WorldQuery, mv: &MovementConfig, t: &CombatTuning) -> Option<usize> {
    let range = context_reach(a, t, t.assassinate_range, t.assassinate_ray_scale);
    let mut best: Option<(usize, f32)> = None;
    for (vi, v) in state.players.iter().enumerate() {
        if v.id == a.id || !v.alive || v.condition != Condition::Normal || v.carried_by.is_some() || crate::powers::is_hidden(v) || v.sword.phase == SwordPhase::Assassinating {
            continue;
        }
        let to_v = v.position - a.position;
        let d = to_v.length();
        if d > range {
            continue;
        }
        let ok = match v.kind {
            PawnKind::Human => angle_deg(-forward_vector(v.yaw), -to_v) <= t.back_half_angle && v.sword.phase != SwordPhase::Blocking,
            PawnKind::Rat => false,
            _ => t.assassinate_on_awareness.get(v.ai_awareness as usize).copied().unwrap_or(false) && (t.assassinate_runners || !running(v)),
        };
        if !ok || !in_view(a, v, world, mv) {
            continue;
        }
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((vi, d));
        }
    }
    best.map(|(i, _)| i)
}

/// `DisItemContext_DropAssassinate` (0x00C09A50 search, 0x00C09900 test). The fall is predicted
/// over `m_fHitWindowInSeconds` from the current velocity, the vertical speed at least
/// `m_fMinDropDownVel` downward, with gravity (0x00C09810). The target is the context's own
/// (in view within its reach, 250 x ray scale 2.0) or else the first pawn a pawn-box sweep
/// along the predicted fall meets (then straight down). It is taken when the player falls
/// (`Velocity.Z < m_fMaxAllowedDropJumpVel`), the target is below by more than
/// `m_fMinDropDistToTarget` and no more than `m_fMaxDropDistToTarget` (else TooHighToStart: not
/// yet), and a pawn trace from the player reaches it. No awareness gate (the drop context never
/// calls 0x00C0D8F0).
fn find_drop_assassination(a: &PlayerState, state: &WorldState, world: &dyn WorldQuery, mv: &MovementConfig, t: &CombatTuning) -> Option<usize> {
    let r = &t.drop_rule;
    if a.velocity.z >= r.max_allowed_drop_jump_vel {
        return None;
    }
    let tw = r.hit_window;
    let vz = a.velocity.z.min(-r.min_drop_down_vel);
    let pred = Vec3::new(a.velocity.x * tw, a.velocity.y * tw, vz * tw + mv.gravity_z * tw * tw * 0.5);
    if -pred.z <= r.min_drop_dist {
        return None;
    }
    let eligible = |v: &PlayerState| v.id != a.id && v.alive && v.condition == Condition::Normal && v.carried_by.is_none() && !crate::powers::is_hidden(v) && v.kind != PawnKind::Rat;
    // The context's target.
    let range = context_reach(a, t, t.drop_assassinate_range, t.drop_assassinate_ray_scale);
    let ctx = state
        .players
        .iter()
        .enumerate()
        .filter(|(_, v)| eligible(v) && v.position.distance(a.position) <= range && in_view(a, v, world, mv))
        .min_by(|x, y| x.1.position.distance_squared(a.position).total_cmp(&y.1.position.distance_squared(a.position)))
        .map(|(i, _)| i);
    if let Some(i) = ctx.filter(|&i| drop_ready(a, &state.players[i], world, mv, r)) {
        return Some(i);
    }
    // The sweeps along the predicted fall, then straight down.
    for path in [pred, Vec3::new(0.0, 0.0, pred.z)] {
        let len = path.length();
        let wall = world.raycast(a.position, path, len, channel::PAWN).map_or(len, |h| h.distance);
        let mut first: Option<(usize, f32)> = None;
        for (i, v) in state.players.iter().enumerate() {
            if v.id == a.id || !v.alive || v.carried_by.is_some() {
                continue;
            }
            let hh = half_height(v, mv);
            let (on_path, on_body) = closest_segment_segment(a.position, a.position + path, v.position - Vec3::Z * hh, v.position + Vec3::Z * hh);
            if on_path.distance(on_body) > 2.0 * mv.collision_radius {
                continue;
            }
            let s = (on_path - a.position).length();
            if s <= wall && first.is_none_or(|(_, fs)| s < fs) {
                first = Some((i, s));
            }
        }
        if let Some((i, _)) = first {
            return (eligible(&state.players[i]) && drop_ready(a, &state.players[i], world, mv, r)).then_some(i);
        }
    }
    None
}

/// 0x00C09900 for a target: DoNow when the player's base is above the target's location by more
/// than `m_fMinDropDistToTarget` and at most `m_fMaxDropDistToTarget`, with a clear pawn trace.
fn drop_ready(a: &PlayerState, v: &PlayerState, world: &dyn WorldQuery, mv: &MovementConfig, r: &crate::tuning::DropRule) -> bool {
    let h = (a.position.z - half_height(a, mv)) - v.position.z;
    if h <= r.min_drop_dist || h > r.max_drop_dist {
        return false;
    }
    let to = v.position - a.position;
    let len = to.length();
    !world.raycast(a.position, to, len, channel::PAWN).is_some_and(|hit| hit.distance < len - mv.collision_radius)
}

pub fn id_of(state: &WorldState, i: usize) -> PlayerId {
    state.players[i].id
}
