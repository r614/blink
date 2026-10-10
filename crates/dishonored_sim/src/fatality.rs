//! The player's finisher kills: the sword's fatality context (`DisItemContext_Fatality` <
//! `_Finisher` < `_MeleeAttackPlayer`), decoded from `Dishonored.exe`; the tweak is
//! `GameplayConfig::fatality` (`dishonored_assets::fatality`).
//!
//! The attack press runs the sword's contexts in slot order: with a full Blood Thirsty gauge the
//! Bash slot first (the controller's attack handler 0x00AAEF40 asks the item for it while the gauge
//! is full; the slot's `Special Fatality` is this context with `m_OwnerContextSlot` 1), then the
//! primary slot's drop assassination, assassination, this fatality and the plain attack.
//!
//! - **Cache** (0x00C14B40): the slow-motion finisher flag rolls `m_fSlomoFinisherRandomChance`;
//!   it is cleared in the Bash slot, within `m_fMinTimeBetweenRandomSlomoFinisher` of the last
//!   one, and unless exactly one enemy engages the player (0x00ABFDF0: the port's
//!   `combat_opponents`). The real-time flag rolls `m_fChanceSlomoFinisherWillBeNormalSpeed`.
//! - **Test** (0x00C10EF0): in the Bash slot or as a slow-motion finisher, the gauge is full, or
//!   the finisher is slow motion and the target is vulnerable (the post-parry window, the port's
//!   stagger) or the attack would kill it (health no more than the rounded damage); in the primary
//!   slot otherwise, the target is vulnerable and stunned (`StateNPCMasterStunned` still stunned,
//!   0x00B73870). The sim's thrown pawn holds its stun for one tick (`m_fStunDuration` 0), so no
//!   target is stunned when an attack starts.
//! - **Generic** (0x00C11130): `m_fGenericFatalityChance` when not slow motion; or no action.
//! - **Speed** (0x00C09FA0): a slow (full) finisher when at most one enemy engages, no Bend Time
//!   runs, and the fast-finisher count is spent or the slow timer ran out after a fast one; else a
//!   fast one.
//! - **Action** (0x00C0E380): the head chop with `m_fBeheadRandomChance`; else by the attacker's
//!   direction from the target (0x00BBA970) its fast action unless slow, falling back to the slow
//!   one (an action counts when it has states: 0x00C07330).
//! - **Execute** (0x00C0DFD0): a fast finisher spends one of the count and marks one seen; a slow
//!   one rolls a new count (`m_NumFinishersBeforeSlow`) and timer (`m_fTimeBeforeSlow`) and stamps
//!   the time (0x00C09ED0). A synced action plays a state's master on the player and its slave on
//!   the victim (the state's index is the slave chain's group); the Bash slot spends the gauge and
//!   the kill gives `m_fAdrenalineMultOnHit` x the victim's health. The generic action plays its
//!   swing, which kills at its damage zone. A slow-motion or Bash-slot finisher is exclusive: its
//!   master's `DisNotify_AdrenalineBendTime_Ranged` windows dilate time (0x00A972D0) unless it plays
//!   in real time; slow motion stamps its time.
//!
//! Dilation, as the quick-access wheel's, only applies in a world with a single human pawn.

use crate::events::{DamageCause, SimEvent, SoundCategory};
use crate::movement::{eye_position, view_vector};
use crate::state::{Condition, FatalityRun, FinisherDilation, PawnKind, PlayerId, PlayerState, SwordPhase, WorldState};
use crate::tuning::CombatTuning;
use dishonored_ir::gameplay::{FatalityConfig, GameplayConfig, fatality_move as fm};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;

/// `SwordState::assassination_move` of a finisher kill (`SwordState::fatality` names it).
pub const FATALITY_MOVE: u8 = 12;

/// The context's slot (`m_OwnerContextSlot`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Primary,
    Bash,
}

fn now(state: &WorldState) -> f32 {
    state.tick as f32 * crate::DT
}

/// 0x00C09ED0: a new fast-finisher count and slow timer, from now.
fn reset_pacing(p: &mut PlayerState, f: &FatalityConfig, now: f32) {
    let [lo, hi] = f.finishers_before_slow;
    p.finisher.fast_left = (p.roll() * (hi - lo) as f32 + lo as f32) as i32;
    let [tl, th] = f.time_before_slow;
    p.finisher.slow_timer = p.roll() * (th - tl) + tl;
    p.finisher.seen_fast = false;
    p.finisher.last_slow = now;
    p.finisher.init = true;
}

/// The context's target: the nearest pawn the attack's sweep meets within reach (the melee
/// context's), an NPC standing in its normal condition within `m_fMaxContextHeightDifference`.
fn target(state: &WorldState, ai: usize, world: &dyn WorldQuery, g: &GameplayConfig, t: &CombatTuning) -> Option<usize> {
    let a = &state.players[ai];
    let mv = &g.movement;
    let eye = eye_position(a, mv);
    let dir = view_vector(a.yaw, a.pitch);
    let range = crate::combat::reach(a, t);
    let mut best: Option<(usize, f32)> = None;
    for (vi, v) in state.players.iter().enumerate() {
        if vi == ai || matches!(v.kind, PawnKind::Human | PawnKind::Rat) || !v.alive || v.condition != Condition::Normal || v.carried_by.is_some() || crate::powers::is_hidden(v) || v.sword.phase == SwordPhase::Assassinating {
            continue;
        }
        if (v.position.z - a.position.z).abs() > g.fatality.max_height_difference {
            continue;
        }
        let (gap, along) = crate::combat::body_distance(eye, dir, range, v, mv);
        if gap > t.attack_sweep_size * 0.5 {
            continue;
        }
        let to = v.position - eye;
        if world.raycast(eye, to, to.length(), channel::MELEE).is_some_and(|h| h.distance < to.length() - mv.collision_radius) {
            continue;
        }
        if best.is_none_or(|(_, d)| along < d) {
            best = Some((vi, along));
        }
    }
    best.map(|b| b.0)
}

/// Evaluate and run the fatality context of `ai`'s attack press in `slot`; true when a finisher
/// started.
pub fn try_start(state: &mut WorldState, ai: usize, slot: Slot, world: &dyn WorldQuery, g: &GameplayConfig, t: &CombatTuning, events: &mut Vec<SimEvent>) -> bool {
    let f = &g.fatality;
    if f.moves.iter().all(|m| m.is_empty()) {
        return false;
    }
    let Some(vi) = target(state, ai, world, g, t) else { return false };
    let now = now(state);
    let bend_running = !state.time_bends.is_empty();
    let v = state.players[vi];
    let a = &mut state.players[ai];
    if !a.finisher.init {
        reset_pacing(a, f, now);
    }
    // 0x00C14B40.
    let mut slomo = a.roll() <= f.slomo_chance;
    if slot == Slot::Bash {
        slomo = false;
    }
    if slomo && a.finisher.last_slomo > 1e-8 && now - a.finisher.last_slomo < f.slomo_min_interval {
        slomo = false;
    }
    if slomo {
        slomo = a.combat_opponents == 1;
    }
    let realtime = a.roll() <= f.slomo_realtime_chance;
    // 0x00C10EF0.
    let dmg = crate::combat::app_ceil(crate::combat::melee_damage(a, &v, false, g, t));
    let would_kill = v.vitals.health <= dmg;
    let vulnerable = v.sword.phase == SwordPhase::Staggered;
    let finisher = if slot == Slot::Bash || slomo { crate::adrenaline::is_full(a, g) || (slomo && (vulnerable || would_kill)) } else { false };
    if !finisher {
        return false;
    }
    // 0x00C11130.
    let mut generic = f.generic_chance > 1e-8 && a.roll() <= f.generic_chance && !slomo;
    // 0x00C09FA0.
    let elapsed = now - a.finisher.last_slow >= a.finisher.slow_timer && a.finisher.seen_fast;
    let slow = a.combat_opponents < 2 && !bend_running && (a.finisher.fast_left <= 0 || elapsed);
    // 0x00C0E380.
    let has = |k: usize| f.moves.get(k).is_some_and(|m| !m.is_empty());
    let mut action = None;
    if a.roll() <= f.behead_chance && has(fm::HEAD_CHOP) {
        action = Some(fm::HEAD_CHOP);
    }
    if action.is_none() {
        let (fast, full) = [(fm::FRONT_FAST, fm::FRONT), (fm::LEFT_FAST, fm::LEFT), (fm::RIGHT_FAST, fm::RIGHT), (fm::BACK_FAST, fm::BACK)][crate::combat::relative_direction(&v, a.position)];
        action = if !slow && has(fast) { Some(fast) } else if has(full) { Some(full) } else { None };
    }
    if action.is_none() {
        generic = true;
    }
    let action = if generic { fm::GENERIC } else { action.unwrap_or(fm::GENERIC) };
    if !has(action) {
        return false;
    }
    // 0x00C0DFD0.
    if slow {
        reset_pacing(a, f, now);
    } else {
        a.finisher.fast_left -= 1;
        a.finisher.seen_fast = true;
    }
    let clips = &f.moves[action];
    let variant = ((a.roll() * clips.len() as f32) as usize).min(clips.len() - 1);
    let clip = &clips[variant];
    let exclusive = slot == Slot::Bash || slomo;
    if slomo {
        a.finisher.last_slomo = now;
    }
    let synced = clip.slave != 0;
    let slave = clip.slave;
    a.sword.phase = SwordPhase::Assassinating;
    a.sword.timer = 0.0;
    a.sword.lock_time = clip.exit;
    a.sword.assassination_target = synced.then_some(v.id);
    a.sword.assassination_move = FATALITY_MOVE;
    a.sword.root_t = 0.0;
    a.sword.root_yaw = a.yaw;
    a.sword.fatality = Some(FatalityRun { action: action as u8, variant: variant as u8, slow_motion: exclusive && !(slomo && realtime), victim: v.id, pending: !synced, clock: 0.0 });
    a.velocity = Vec3::ZERO;
    let attacker = a.id;
    if slot == Slot::Bash {
        crate::adrenaline::spend(a, events);
    }
    // Ahead of the kill's death, so the victim's slave is its death sequence.
    events.push(SimEvent::Fatality { id: attacker, victim: v.id, action: action as u8, variant: variant as u8, slave });
    if synced {
        crate::adrenaline::gain(a, v.vitals.health * f.adrenaline_mult, g, events);
        let cause = if slot == Slot::Bash { DamageCause::Adrenaline } else { DamageCause::Sword };
        let victim = &mut state.players[vi];
        crate::noise::emit(events, &g.ai.noise.sword_body, victim.position, SoundCategory::SwordImpact, attacker, 0.0);
        crate::vitals::kill(victim, Some(attacker), cause, events);
    }
    true
}

/// Set a caster's finisher dilation toward (world, player) over `blend` seconds (the helper
/// 0x009FA0D0: a non-zero target is kept within [0.05, 1]).
fn set_dilation(state: &mut WorldState, caster: PlayerId, world: f32, player: f32, blend: f32) {
    let clamp = |x: f32| if x == 0.0 { 0.0 } else { x.clamp(0.05, 1.0) };
    let (world, player) = (clamp(world), clamp(player));
    let i = match state.finisher_dilation.iter().position(|d| d.caster == caster) {
        Some(i) => i,
        None => {
            if world == 1.0 && player == 1.0 {
                return;
            }
            state.finisher_dilation.push(FinisherDilation { caster, world: [1.0, 1.0, 0.0], player: [1.0, 1.0, 0.0] });
            state.finisher_dilation.len() - 1
        }
    };
    let d = &mut state.finisher_dilation[i];
    for (l, target) in [(&mut d.world, world), (&mut d.player, player)] {
        l[1] = target;
        l[2] = if blend.abs() >= 1e-8 { (target - l[0]) / blend } else { f32::MAX };
    }
}

/// The finishers' tick, after the swords advanced: an ended finisher lets go of its dilation, a
/// running one opens and closes its master's bend-time windows, and a generic swing kills its
/// victim when its damage zone opens.
pub fn step(state: &mut WorldState, g: &GameplayConfig, events: &mut Vec<SimEvent>) {
    let single = state.players.iter().filter(|p| p.kind == PawnKind::Human).count() <= 1;
    for ai in 0..state.players.len() {
        let a = state.players[ai];
        let Some(run) = a.sword.fatality else { continue };
        if a.sword.phase != SwordPhase::Assassinating || a.sword.assassination_move != FATALITY_MOVE || !a.alive {
            state.players[ai].sword.fatality = None;
            set_dilation(state, a.id, 1.0, 1.0, 0.0);
            continue;
        }
        let Some(clip) = g.fatality.moves.get(run.action as usize).and_then(|m| m.get(run.variant as usize)) else { continue };
        let (t0, t1) = (run.clock, a.sword.timer);
        if single {
            for w in clip.bends.iter().filter(|w| !w.adrenaline || run.slow_motion) {
                if t0 < w.start && w.start <= t1 {
                    set_dilation(state, a.id, w.world, w.player, w.blend_in);
                }
                let end = w.start + w.duration;
                if t0 < end && end <= t1 {
                    set_dilation(state, a.id, 1.0, 1.0, w.blend_out);
                }
            }
        }
        if run.pending && clip.hit.is_some_and(|h| t0 < h && h <= t1 || t0 == 0.0 && h <= t1) {
            if let Some(vi) = state.players.iter().position(|p| p.id == run.victim) {
                let victim = &mut state.players[vi];
                if victim.alive {
                    crate::noise::emit(events, &g.ai.noise.sword_body, victim.position, SoundCategory::SwordImpact, a.id, 0.0);
                    crate::vitals::kill(victim, Some(a.id), DamageCause::Sword, events);
                }
            }
            if let Some(r) = state.players[ai].sword.fatality.as_mut() {
                r.pending = false;
            }
        }
        if let Some(r) = state.players[ai].sword.fatality.as_mut() {
            r.clock = t1;
        }
    }
}

/// The finisher dilations' blend (real time, as the GameInfo tick's layers): `current` moves by
/// `velocity x dt` and lands on its target; channels back at 1 are dropped.
pub fn step_dilation(state: &mut WorldState, dt: f32) {
    for d in state.finisher_dilation.iter_mut() {
        for l in [&mut d.world, &mut d.player] {
            if l[2] == f32::MAX {
                l[0] = l[1];
                continue;
            }
            let old = l[0];
            l[0] = old + l[2] * dt;
            if (l[1] - l[0]) * (l[1] - old) <= 0.0 {
                l[0] = l[1];
                l[2] = f32::MAX;
            }
        }
    }
    state.finisher_dilation.retain(|d| !(d.world[0] == 1.0 && d.world[1] == 1.0 && d.player[0] == 1.0 && d.player[1] == 1.0));
}
