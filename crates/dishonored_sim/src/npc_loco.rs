//! The NPC locomotion component's start, stop and turn sequences (`ArkComponentLocomotion`,
//! research/dishonored/ANIMATION.md section 3, UNKNOWNS U-280): requested from the path the
//! brain's locomotion request follows ([`crate::pawn_states::Request::path`]), not from speed
//! thresholds, and played in the tree's locomotion node's "Specific anim" child.
//!
//! - **The component** (+0x218 state, setter 0x009459A0; the per-tick 0x0094BEC0): the turn
//!   machine with its three triggers (0x0094B9D0; static 0x0094B4F0, moving 0x0094B780), the start
//!   rule (0x00946F60) and the stop rule (0x00947300), in that order, with the speed and yaw each
//!   state asks of the path following. States 0 none, 1-5 turn (pending, requested, blending in,
//!   fully in, blending out), 6-9 start (requested, blending in, fully in, blending out), 10-14
//!   stop (window, requested, blending in, fully in, blending out).
//! - **The node's specific anim** (`ArkAnimNodeLocomotion` +0x1CC `m_SpecificAnimProps`, the
//!   flags +0x238): a request plays from the next animation update at the modifier's and the
//!   node's `m_fSpeedMultiplier` (0x00917440, 0x009118B0), blends in over its blend time from its
//!   play time (`max(0.01, t x Rate / Blend)`) and out over the last blend time before its
//!   effective end (the length, times `m_fCutIfMoving` for a turn into a walk), and ends below a
//!   weight of 1e-4 (0x0090BDB0); "fully in" is the move loops' weight at 0 (0x00909B40).
//! - **Movement**: while a sequence is fully in (and in states 4, 8, 13, 14) the pawn is moved by
//!   the sequence's root translation, as the chains' (`crate::pawn_states::locomotion`:
//!   displacement over `dt` times the modifier's `m_fSpeedMultiplier`); otherwise the path
//!   following moves it at the state's speed and yaw ([`Drive`]).
//!
//! The component runs at the start of the pawn's next tick here, before its physics: the
//! original runs it after the pose barrier and its velocity is integrated on the next tick, so
//! the order of decision and integration is the same (tick_order.md section 5).
//!
//! Not decoded, kept as the port had it (LIKELY): the static turn without a moving path (its
//! target is a facing request read through an object Ghidra does not show, 0x004049F0: the
//! brain's facing stays instant), the corner turn (0x0094B880: the component's own face-to
//! request +0x25C and `FUN_009C5AF0`), the path following's rotation step (0x00941330:
//! `m_fRotationMaxSpeed`, U-862) and braking toward the stop window (0x00948050), the avoidance
//! push, the face-to priorities (0x0093E750: assumed free), the AI LOD flag (+0x1AC bit 0:
//! every NPC is at full LOD here), the move loops' re-seat at a blend-out (0x00916950, the
//! `fmod` operand) and who applies a turn's root yaw (the barrier stores it in +0x330 but applies
//! it only with `RMM_Translate`: the turn here rotates the pawn by it while fully in).

use crate::SimConfig;
use crate::anim::SeqId;
use crate::pawn_states::{self, LocoPath, root_mode};
use crate::state::{PlayerId, WorldState};
use dishonored_ir::npc_anim::{LocoConfig, SpeedMode};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// The component's states (+0x218).
pub mod state {
    pub const NONE: u8 = 0;
    pub const TURN_PENDING: u8 = 1;
    pub const TURN_REQUESTED: u8 = 2;
    pub const TURN_BLEND_IN: u8 = 3;
    pub const TURN_IN: u8 = 4;
    pub const TURN_BLEND_OUT: u8 = 5;
    pub const START_REQUESTED: u8 = 6;
    pub const START_BLEND_IN: u8 = 7;
    pub const START_IN: u8 = 8;
    pub const START_BLEND_OUT: u8 = 9;
    pub const STOP_WINDOW: u8 = 10;
    pub const STOP_REQUESTED: u8 = 11;
    pub const STOP_BLEND_IN: u8 = 12;
    pub const STOP_IN: u8 = 13;
    pub const STOP_BLEND_OUT: u8 = 14;
}

/// The specific anim's `m_Type` (+0x1E8).
pub mod kind {
    pub const TURN: u8 = 0;
    pub const START: u8 = 1;
    pub const STOP: u8 = 2;
}

/// The node's "Specific anim" child: the sequence, its type, `m_bSpecificAnimRequested` /
/// `m_bPlayingSpecificAnim`, its time (sequence seconds) and the last advance's start, its rate,
/// blend time, the fraction of its length played (`m_bUseCutBeforeEnd`: `m_fCutIfMoving`, else
/// 1) and its weight (`m_fModeWeights[1]`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Specific {
    pub seq: SeqId,
    pub kind: u8,
    pub requested: bool,
    pub playing: bool,
    pub time: f32,
    pub prev: f32,
    pub rate: f32,
    pub blend: f32,
    pub cut: f32,
    pub weight: f32,
}

impl Specific {
    /// 0x00909B40: playing with the move loops' weight at 0.
    pub fn fully_in(&self) -> bool {
        self.playing && (1.0 - self.weight).abs() < 1e-8
    }

    fn active(&self) -> bool {
        self.requested || self.playing
    }
}

/// The turn's bookkeeping (+0x21C..+0x24C): where it was asked, the yaws from and to, the speed
/// of its states 1-3 (+0x230), the speed mode (an index into the config's modes) and its prop,
/// the modifier, the immediate and to-walk flags, the blend (fast / slow), the previous squared
/// distance and time to phase, and the root rotation scale (+0x374 of the mesh).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub at: Vec3,
    pub from: f32,
    pub to: f32,
    pub speed: f32,
    pub mode: u16,
    pub prop: u8,
    pub modifier: u8,
    pub to_walk: bool,
    pub fast: bool,
    pub prev_d2: f32,
    pub prev_ttp: f32,
    pub scale: f32,
}

/// An NPC's locomotion component and its node's specific anim (`PawnStates::loco`), with the
/// node's smoothed velocity (angle from facing, speed) and the move loops' sync position
/// (`m_fMovePrevProgress` / `m_fMoveCurProgress` +0x22C / +0x230), rate scale
/// (`m_fMostRelevantRateScale` +0x21C) and master length.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct NpcLoco {
    pub state: u8,
    pub specific: Option<Specific>,
    pub turn: Turn,
    /// +0x250 bit 0.
    pub stop_allowed: bool,
    pub vel: (f32, f32),
    pub phase_prev: f32,
    pub phase: f32,
    pub group_rate: f32,
    pub master_len: f32,
}

impl NpcLoco {
    fn fully_in(&self) -> bool {
        self.specific.is_some_and(|s| s.fully_in())
    }

    fn playing(&self) -> bool {
        self.specific.is_some_and(|s| s.playing)
    }

    /// The component moves the pawn by the sequence's root motion (0x0094BEC0: states 4, 8, 14 or
    /// fully in; 13 is fully in).
    pub fn anim_driven(&self) -> bool {
        matches!(self.state, state::TURN_IN | state::START_IN | state::STOP_IN | state::STOP_BLEND_OUT) || self.fully_in()
    }

    /// The end functions (0x00945A10 turn, 0x00945AE0 start, 0x00945B70 stop) and the cancel
    /// (0x00909B70): state 0, the request dropped; a turn's rotation scale back to 1.
    fn end(&mut self) {
        self.state = state::NONE;
        self.specific = None;
        self.turn.scale = 1.0;
    }
}

/// What the component asks of the pawn's movement this tick: moved by the root motion (the
/// input moves nothing), the yaw (None: the brain's, toward the path point) and the speed
/// (uu/s, the modifier's multiplier included; None: the brain's).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Drive {
    pub anim: bool,
    pub yaw: Option<f32>,
    pub speed: Option<f32>,
}

/// The pawn's component and specific anim (for the presentation: the sequence the tree's
/// locomotion node plays in its "Specific anim" child).
pub fn component(state: &WorldState, id: PlayerId) -> Option<&NpcLoco> {
    state.pawn_states.get(&id).map(|s| &s.loco)
}

fn wrap(a: f32) -> f32 {
    let a = (a + 180.0).rem_euclid(360.0) - 180.0;
    if a <= -180.0 { a + 360.0 } else { a }
}

fn yaw_of(v: Vec2) -> f32 {
    v.y.atan2(v.x).to_degrees()
}

/// A speed mode's forward speed (its N loop's `m_fAnimSpeed`; an idle mode's 0).
fn forward_speed(m: &SpeedMode) -> f32 {
    if m.move_anims.len() >= 4 { m.move_anims[2].1 } else { m.move_anims.first().map_or(0.0, |a| a.1) }
}

fn mode_index(lc: &LocoConfig, modifier: usize, speed: usize) -> Option<usize> {
    let i = *lc.table.get(modifier * lc.speed_names.len() + speed)?;
    (i >= 0).then_some(i as usize)
}

fn seq_of(cfg: &SimConfig, name: &str) -> Option<SeqId> {
    cfg.anim.find(&cfg.gameplay.npc_anim.sets, name)
}

fn length(cfg: &SimConfig, seq: SeqId) -> f32 {
    cfg.anim.info(seq).map_or(0.0, |i| i.duration)
}

/// The pawn's locomotion config and stance from its request.
fn config<'a>(cfg: &'a SimConfig, loco: Option<(u16, u8)>) -> Option<(&'a LocoConfig, usize)> {
    let (c, m) = loco?;
    let lc = cfg.gameplay.ai.locomotion.get(c as usize)?;
    ((m as usize) < lc.modifiers.len()).then_some((lc, m as usize))
}

/// One tick of the component for its pawn.
struct Tick<'a> {
    cfg: &'a SimConfig,
    lc: &'a LocoConfig,
    modifier: usize,
    mult: f32,
    path: Option<LocoPath>,
    /// +0x104 & 0x200: a path, a request and a speed index (0x00943130).
    moving: bool,
    pos: Vec2,
    yaw: f32,
    /// +0x110: |Velocity| over the modifier's multiplier (the look-at factor is 1, U-862).
    speed: f32,
    dt: f32,
}

impl Tick<'_> {
    fn idle(&self) -> Option<&SpeedMode> {
        self.lc.mode(self.modifier, 0)
    }

    /// The segment's yaw (+0x98: `P[k-1]` to `P[k]`; from the pawn when the segment is a point).
    fn segment_yaw(&self) -> f32 {
        let Some(p) = self.path else { return self.yaw };
        let d = p.to.truncate() - p.from.truncate();
        if d.length_squared() > 1e-4 { yaw_of(d) } else { yaw_of(p.to.truncate() - self.pos) }
    }

    /// The horizontal distance to the destination (+0x1B0; the path's points lie on the navmesh,
    /// the pawn's location a capsule half-height above: compared in the plane, LIKELY).
    fn dest_dist(&self) -> f32 {
        self.path.map_or(0.0, |p| (p.dest.truncate() - self.pos).length())
    }

    fn point_dist(&self) -> f32 {
        self.path.map_or(0.0, |p| (p.to.truncate() - self.pos).length())
    }

    /// Bit 7 (0x00944390): an accurate request arrives within 0.1 uu; another within
    /// `m_fUnaccuracyRadius` once its speed is below `m_fVelocityToStopMoving` (with stop props:
    /// after the stop; without, LIKELY the same test).
    fn arrived(&self) -> bool {
        let Some(p) = self.path else { return true };
        let d = self.dest_dist();
        if p.accurate { d < 0.1 } else { d < self.lc.unaccuracy_radius && self.speed < self.lc.stop_speed }
    }

    /// The node's request rate (0x009118B0): the modifier's and the node's `m_fSpeedMultiplier`.
    fn rate(&self) -> f32 {
        self.mult * self.cfg.gameplay.npc_anim.loco_node.speed_multiplier
    }
}

/// The component's tick for pawn `i` before its physics: the state machine, the triggers and the
/// requests; returns what it asks of the pawn's movement. A component switched off (a full-body
/// chain plays), a pawn without a locomotion request or one down drops its sequence.
pub fn pre_move(state: &mut WorldState, i: usize, cfg: &SimConfig) -> Drive {
    let p = state.players[i];
    let req = pawn_states::request(state, p.id);
    let Some(s) = state.pawn_states.get_mut(&p.id) else { return Drive::default() };
    let l = &mut s.loco;
    let conf = config(cfg, req.loco).filter(|_| !s.loco_off && p.alive && !p.incapacitated());
    let Some((lc, modifier)) = conf else {
        if l.state != state::NONE || l.specific.is_some() {
            l.end();
        }
        state.players[i].loco_arrived = false;
        state.players[i].loco_stopping = false;
        return Drive::default();
    };
    let mult = lc.modifiers[modifier].speed_multiplier.max(1e-3);
    let t = Tick {
        cfg,
        lc,
        modifier,
        mult,
        path: req.path,
        moving: req.path.is_some() && req.loco_speed > 0.0,
        pos: p.position.truncate(),
        yaw: p.yaw,
        speed: p.velocity.truncate().length() / mult,
        dt: crate::DT * p.time_scale,
    };
    turn_machine(l, &t, &mut state.players[i].velocity);
    start_rule(l, &t);
    stop_rule(l, &t, &mut state.players[i].velocity);
    // The arrival of a request that stops with a sequence comes only after the stop (states 0
    // and 5, 0x00944390); the port's brain reads it (`dishonored_ai`'s walk arrival).
    let stops = t.idle().is_some_and(|m| !m.stop_props.is_empty() && m.stop_props.iter().all(|p| seq_of(cfg, &p.name).is_some()));
    state.players[i].loco_arrived = stops && t.path.is_some_and(|p| !p.accurate) && matches!(l.state, state::NONE | state::TURN_BLEND_OUT) && t.arrived();
    state.players[i].loco_stopping = (state::STOP_WINDOW..=state::STOP_BLEND_OUT).contains(&l.state);
    if l.state == state::TURN_BLEND_IN && s.rm_mode != root_mode::ACCEL {
        // State 2 -> 3 (0x0094B9D0): the mesh in (`RMM_Accel`, `RMRM_RotateActor`); the component
        // never restores them (the chains' stop does, slot 470).
        s.rm_mode = root_mode::ACCEL;
        s.rm_rotation = 1;
    }
    let l = &s.loco;
    let walk = lc.mode(modifier, 1).map_or(0.0, forward_speed);
    let anim = l.anim_driven();
    // The desired yaw and the target speed per state (0x00947950, 0x00948050).
    let yaw = match l.state {
        _ if anim => Some(p.yaw),
        state::TURN_PENDING..=state::TURN_BLEND_IN => Some(l.turn.from),
        state::TURN_BLEND_OUT => Some(l.turn.to),
        state::STOP_WINDOW..=state::STOP_BLEND_OUT => Some(p.yaw),
        _ => None,
    };
    let speed = match l.state {
        _ if anim => None,
        state::TURN_PENDING..=state::TURN_BLEND_IN => Some(l.turn.speed * mult),
        state::START_REQUESTED..=state::START_IN | state::STOP_IN => Some(0.0),
        state::STOP_WINDOW..=state::STOP_BLEND_IN => Some(walk * mult),
        _ => None,
    };
    Drive { anim, yaw, speed }
}

/// The node's specific-anim request setters (turn 0x009101D0, start 0x00910340, stop 0x00910470).
fn request(l: &mut NpcLoco, t: &Tick, seq: SeqId, kind: u8, blend: f32, cut: f32) {
    l.specific = Some(Specific { seq, kind, requested: true, playing: false, time: 0.0, prev: 0.0, rate: t.rate(), blend, cut, weight: 0.0 });
}

/// The time to the turn prop's phase (0x00910060): `m_fMostRelevantRateScale x ((m_fMoveAnimIn -
/// master phase) mod 1) x length`.
fn time_to_phase(l: &NpcLoco, move_in: f32) -> f32 {
    l.group_rate * (move_in - l.phase).rem_euclid(1.0) * l.master_len
}

/// `RequestTurn` (0x0094AF40) from yaw `from` to `to` at speed `v` (unscaled), asked at `at`.
fn request_turn(l: &mut NpcLoco, t: &Tick, from: f32, to: f32, v: f32, at: Vec2) {
    let lc = t.lc;
    // The speed modes bracketing `v` (0x009343A0): (speed index, forward speed).
    let speeds: Vec<(usize, f32)> = (0..lc.speed_names.len()).filter_map(|k| lc.mode(t.modifier, k).map(|m| (k, forward_speed(m)))).collect();
    if speeds.is_empty() {
        return;
    }
    let lo = speeds.iter().rposition(|s| s.1 <= v).unwrap_or(0);
    let hi = (lo + 1).min(speeds.len() - 1);
    let (vlo, vhi) = (speeds[lo].1, speeds[hi].1);
    let w_lo = if vhi > vlo { ((vhi - v) / (vhi - vlo)).clamp(0.0, 1.0) } else { 1.0 };
    let lo_idle = speeds[lo].0 == 0;
    let pick = if w_lo >= 0.5 && !(lo_idle && v >= 0.1) { lo } else { hi };
    let Some(mi) = mode_index(lc, t.modifier, speeds[pick].0) else { return };
    let mode = &lc.modes[mi];
    let props = &mode.turn_props;
    let count = props.len();
    if count == 0 {
        return;
    }
    // The prop (0x0094AE10): the first pair whose nominal angle (180, 135, 90, 45) is within the
    // half-width of the turn, left even and right odd.
    let delta = wrap(to - from);
    let half = if count == 2 { 45.0 } else { 22.5 };
    let Some(pair) = (0..count.min(8) / 2).find(|&k| ((180.0 - 45.0 * k as f32) - delta.abs()).abs() < half) else { return };
    let mut idx = 2 * pair + (delta > 0.0) as usize;
    let mut to_walk = t.moving && !t.arrived();
    if count > 8 {
        let cand = idx + 8;
        to_walk = to_walk && cand < count && t.point_dist() > props[cand].straight_dist.max(0.1);
        if to_walk {
            idx = cand;
        }
    }
    let Some(seq) = props.get(idx).and_then(|pr| seq_of(t.cfg, &pr.name)) else { return };
    let idle = speeds[pick].0 == 0;
    let immediate = lo_idle && w_lo >= 0.5;
    let fast = !(to_walk && idle && count <= 8);
    let nominal = (4 - ((idx % 8) >> 1)) as f32 * 45.0;
    l.turn = Turn {
        at: at.extend(0.0),
        from,
        to,
        speed: if idle { 0.0 } else { forward_speed(mode) },
        mode: mi as u16,
        prop: idx as u8,
        modifier: t.modifier as u8,
        to_walk,
        fast,
        prev_d2: 0.0,
        prev_ttp: time_to_phase(l, props[idx].move_anim_in),
        scale: delta.abs() / nominal,
    };
    if immediate {
        request_turn_anim(l, t, seq);
        l.state = state::TURN_REQUESTED;
    } else {
        l.state = state::TURN_PENDING;
    }
}

fn request_turn_anim(l: &mut NpcLoco, t: &Tick, seq: SeqId) {
    let node = &t.cfg.gameplay.npc_anim.loco_node;
    let prop = &t.lc.modes[l.turn.mode as usize].turn_props[l.turn.prop as usize];
    let blend = if l.turn.fast { node.turn_fast_blend } else { node.turn_slow_blend };
    let cut = if l.turn.to_walk && prop.cut_if_moving > 0.0 { prop.cut_if_moving } else { 1.0 };
    request(l, t, seq, kind::TURN, blend, cut);
}

/// The turn machine (0x0094B9D0): its triggers in state 0 (0x0093E7A0: no other sequence), then
/// the turn's states.
fn turn_machine(l: &mut NpcLoco, t: &Tick, velocity: &mut Vec3) {
    if l.state == state::NONE && !l.specific.is_some_and(|s| s.active()) {
        static_turn(l, t);
        if l.state == state::NONE {
            moving_turn(l, t);
        }
    }
    match l.state {
        state::TURN_PENDING => {
            if !t.moving || t.modifier != l.turn.modifier as usize {
                // The modifier changed (the re-request is not ported) or the request dropped
                // before the turn was asked of the node (LIKELY).
                l.end();
                return;
            }
            let prop = &t.lc.modes[l.turn.mode as usize].turn_props[l.turn.prop as usize];
            let d2 = (t.pos - l.turn.at.truncate()).length_squared();
            let near = d2 >= l.turn.prev_d2 || d2 <= prop.straight_dist * prop.straight_dist || (d2 - l.turn.prev_d2).abs() < 1.0;
            let ttp = time_to_phase(l, prop.move_anim_in);
            let phase = ttp > l.turn.prev_ttp || ttp <= t.dt;
            l.turn.prev_d2 = d2;
            l.turn.prev_ttp = ttp;
            if near && phase {
                if let Some(seq) = seq_of(t.cfg, &prop.name) {
                    l.turn.fast = true;
                    request_turn_anim(l, t, seq);
                    l.state = state::TURN_REQUESTED;
                }
            }
        }
        state::TURN_REQUESTED => {
            if l.playing() {
                l.state = state::TURN_BLEND_IN;
            } else if !l.specific.is_some_and(|s| s.active()) {
                l.end();
            }
        }
        state::TURN_BLEND_IN => {
            if l.fully_in() || !l.playing() {
                l.state = state::TURN_IN;
            }
        }
        state::TURN_IN => {
            if !l.fully_in() {
                l.state = state::TURN_BLEND_OUT;
                // A turn into a walk leaves at `m_fVelocityToStartToMove` (+0x114).
                if l.turn.to_walk && t.speed < t.lc.stop_speed {
                    let v = crate::movement::forward_vector(t.yaw) * t.lc.start_speed * t.mult;
                    *velocity = Vec3::new(v.x, v.y, velocity.z);
                }
            }
        }
        state::TURN_BLEND_OUT => {
            if !l.playing() {
                l.end();
            }
        }
        _ => {}
    }
}

/// The static turn (0x0094B4F0) on a moving path: below `m_fMaxSpeedForStaticTurn`, more than
/// `m_fMaxDistToPathEndToTurn` from the destination, the path `m_fMaxDistToFirstPathTurn` ahead
/// within 90 degrees of the yaw toward the path point, which is more than 15 degrees off.
fn static_turn(l: &mut NpcLoco, t: &Tick) {
    let Some(p) = t.path.filter(|_| t.moving && !t.arrived()) else { return };
    if t.speed >= t.lc.max_speed_static_turn || t.dest_dist() <= t.lc.max_dist_to_path_end_to_turn {
        return;
    }
    let to = p.to.truncate() - t.pos;
    if to.length_squared() < 1e-4 {
        return;
    }
    let target = yaw_of(to);
    // The path's direction `m_fMaxDistToFirstPathTurn` ahead (0x009428B0).
    let ahead = match p.next {
        Some(n) if to.length() < t.lc.max_dist_to_first_path_turn && (n - p.to).truncate().length_squared() > 1e-4 => yaw_of((n - p.to).truncate()),
        _ => target,
    };
    if wrap(ahead - target).abs() > 90.0 || wrap(target - t.yaw).abs() <= 15.0 {
        return;
    }
    request_turn(l, t, t.yaw, target, 0.0, t.pos);
}

/// The moving turn (0x0094B780): at `m_fMaxSpeedForStaticTurn` or more, the segment more than 15
/// degrees off the pawn's yaw, the path's start more than 100 uu from its destination (the
/// path's first point is the segment's start here, LIKELY).
fn moving_turn(l: &mut NpcLoco, t: &Tick) {
    let Some(p) = t.path.filter(|_| t.moving) else { return };
    if t.speed < t.lc.max_speed_static_turn || (p.from - p.dest).truncate().length() <= 100.0 {
        return;
    }
    let seg = t.segment_yaw();
    if wrap(seg - t.yaw).abs() <= 15.0 {
        return;
    }
    request_turn(l, t, t.yaw, seg, t.speed, t.pos);
}

/// The start rule (0x00946F60) in state 0 and the start's states.
fn start_rule(l: &mut NpcLoco, t: &Tick) {
    if l.state == state::NONE && !l.specific.is_some_and(|s| s.active()) && t.moving && t.speed < t.lc.stop_speed && !t.arrived() {
        let Some(p) = t.path else { return };
        let Some(prop) = t.idle().and_then(|m| m.start_props.first()) else { return };
        let seg = t.segment_yaw();
        let to = p.to.truncate() - t.pos;
        let dir = Vec2::from_angle(seg.to_radians());
        if wrap(t.yaw - seg).abs() < 22.5 && to.length_squared() > prop.straight_dist * prop.straight_dist && to.dot(dir) > 0.0 {
            if let Some(seq) = seq_of(t.cfg, &prop.name) {
                request(l, t, seq, kind::START, t.cfg.gameplay.npc_anim.loco_node.start_blend, 1.0);
                l.state = state::START_REQUESTED;
            }
        }
        return;
    }
    match l.state {
        state::START_REQUESTED..=state::START_BLEND_OUT if !t.moving => l.end(),
        state::START_REQUESTED => {
            if l.playing() {
                l.state = state::START_BLEND_IN;
            } else if !l.specific.is_some_and(|s| s.active()) {
                l.end();
            }
        }
        state::START_BLEND_IN => {
            if l.fully_in() || !l.playing() {
                l.state = state::START_IN;
            }
        }
        state::START_IN => {
            if !l.fully_in() {
                l.state = state::START_BLEND_OUT;
            }
        }
        state::START_BLEND_OUT => {
            if !l.playing() {
                l.end();
            }
        }
        _ => {}
    }
}

/// Whether the sync position crossed `x` this tick (wrap-aware).
fn crossed(prev: f32, cur: f32, x: f32) -> bool {
    if prev <= cur { prev <= x && x <= cur } else { x >= prev || x <= cur }
}

/// The stop rule (0x00947300): the window within `m_fUnaccuracyRadius` of a non-accurate
/// request's destination, the stop asked when the move loops' sync position crosses a stop
/// prop's `m_fMoveAnimIn` unless the next foot ends nearer the destination, then the stop's states.
fn stop_rule(l: &mut NpcLoco, t: &Tick, velocity: &mut Vec3) {
    // A profile whose stop sequences the simulation's table does not hold has none (U-978).
    let stops = t.idle().map(|m| &m.stop_props[..]).filter(|s| s.iter().all(|p| seq_of(t.cfg, &p.name).is_some())).unwrap_or(&[]);
    match l.state {
        state::NONE | state::TURN_BLEND_OUT | state::STOP_WINDOW => {
            let accurate = t.path.is_none_or(|p| p.accurate);
            l.stop_allowed = t.moving && !stops.is_empty() && !accurate && !t.arrived() && wrap(t.yaw - t.segment_yaw()).abs() < 22.5;
        }
        state::STOP_REQUESTED..=state::STOP_BLEND_OUT => l.stop_allowed = true,
        _ => {}
    }
    let r2 = t.lc.unaccuracy_radius * t.lc.unaccuracy_radius;
    let d = t.dest_dist();
    if l.state == state::NONE && l.stop_allowed && d * d < r2 {
        l.state = state::STOP_WINDOW;
    }
    match l.state {
        state::STOP_WINDOW => {
            if !l.stop_allowed || d * d > r2 || l.specific.is_some_and(|s| s.active()) {
                l.state = state::NONE;
                return;
            }
            let Some(k) = stops.iter().position(|s| crossed(l.phase_prev, l.phase, s.move_anim_in)) else { return };
            // The next foot's travel: the walk mode's forward loop over the stop props.
            let walk = t.lc.mode(t.modifier, 1);
            let n = walk.and_then(|m| if m.move_anims.len() >= 4 { m.move_anims.get(2) } else { m.move_anims.first() });
            let len = n.and_then(|a| seq_of(t.cfg, &a.0)).map_or(0.0, |q| length(t.cfg, q));
            let s = len * n.map_or(0.0, |a| a.1) / stops.len() as f32;
            let r = d - stops[k].straight_dist;
            if (r - s).abs() <= r.abs() {
                return;
            }
            if let Some(seq) = seq_of(t.cfg, &stops[k].name) {
                request(l, t, seq, kind::STOP, t.cfg.gameplay.npc_anim.loco_node.stop_blend, 1.0);
                l.state = state::STOP_REQUESTED;
            }
        }
        state::STOP_REQUESTED => {
            if l.playing() {
                l.state = state::STOP_BLEND_IN;
            } else if !l.specific.is_some_and(|s| s.active()) {
                l.end();
            }
        }
        state::STOP_BLEND_IN => {
            if l.fully_in() {
                l.state = state::STOP_IN;
            } else if !l.playing() {
                l.end();
            }
        }
        state::STOP_IN => {
            if !l.fully_in() {
                l.state = state::STOP_BLEND_OUT;
            }
        }
        state::STOP_BLEND_OUT => {
            if !l.playing() {
                l.end();
                // +0x114 = 0: the pawn stands.
                *velocity = Vec3::new(0.0, 0.0, velocity.z);
            }
        }
        _ => {}
    }
}

/// The tree's locomotion node in this tick's animation update for every NPC: the move loops'
/// sync position (the node's smoothed velocity, 0x0090BC00; the loops and the group rate,
/// 0x009331D0 / 0x00916950) and the specific anim's play, blend and end (0x00917440,
/// 0x0090BDB0). A request is taken by this update and advances in it (the node ticks before
/// its children, LIKELY).
pub fn advance(state: &mut WorldState, cfg: &SimConfig) {
    let node = cfg.gameplay.npc_anim.loco_node;
    let params = dishonored_anim::locomotion::LocoParams { rate: node.velocity_rate, range: node.velocity_range, min_ratio: node.velocity_min_ratio, modifier_blend: node.modifier_blend };
    for i in 0..state.players.len() {
        let p = state.players[i];
        if p.time_scale <= 0.0 {
            continue;
        }
        let req = pawn_states::request(state, p.id);
        let Some(s) = state.pawn_states.get_mut(&p.id) else { continue };
        let l = &mut s.loco;
        if !p.alive || p.incapacitated() {
            if l.state != state::NONE || l.specific.is_some() {
                l.end();
            }
            continue;
        }
        let dt = crate::DT * p.time_scale;
        if let Some((lc, mi)) = config(cfg, req.loco) {
            let fwd = crate::movement::forward_vector(p.yaw).truncate();
            let right = crate::movement::right_vector(p.yaw).truncate();
            let v = p.velocity.truncate();
            let target = (v.dot(right).atan2(v.dot(fwd)).to_degrees(), v.length());
            l.vel = dishonored_anim::locomotion::smooth_velocity(l.vel, target, dt, &params);
            let (entries, speed) = dishonored_anim::locomotion::loco_entries(lc, mi, l.vel);
            let lengths: Vec<Option<f32>> = entries.iter().map(|e| seq_of(cfg, &e.name).map(|q| length(cfg, q)).filter(|d| *d > 0.0)).collect();
            let mult = lc.modifiers[mi].speed_multiplier * node.speed_multiplier;
            l.group_rate = dishonored_anim::locomotion::group_rate(&entries, &lengths, speed, mult);
            let master = entries.iter().zip(&lengths).filter_map(|(e, d)| d.map(|d| (e.weight, d))).fold(None, |m: Option<(f32, f32)>, x| if m.is_none_or(|m| x.0 > m.0) { Some(x) } else { m });
            l.phase_prev = l.phase;
            if let Some((_, len)) = master {
                l.master_len = len;
                l.phase = (l.phase + dt * l.group_rate / len).rem_euclid(1.0);
            }
        }
        let Some(sp) = l.specific.as_mut() else { continue };
        let len = length(cfg, sp.seq);
        if sp.requested {
            sp.requested = false;
            sp.playing = true;
            sp.time = 0.0;
        }
        if !sp.playing {
            continue;
        }
        sp.prev = sp.time;
        sp.time = (sp.time + dt * sp.rate).min(len);
        let end = len * sp.cut;
        let left = end - sp.time;
        let w = if sp.blend <= 0.0 {
            1.0
        } else if left < sp.blend {
            left / sp.blend
        } else {
            (sp.time / sp.blend).clamp(0.01, 1.0)
        };
        if w < 1e-4 {
            sp.playing = false;
            sp.weight = 0.0;
        } else {
            sp.weight = w;
        }
    }
}

/// The component's movement after the pose barrier: while it is root-motion driven, the
/// specific anim's root translation this tick (times its weight) over `dt` times the modifier's
/// `m_fSpeedMultiplier` is the velocity the next tick's walking physics integrates, as the
/// chains' (`crate::pawn_states::locomotion`); a turn fully in rotates the pawn by its root yaw
/// times the rotation scale (LIKELY, see the module).
pub fn locomotion(state: &mut WorldState, cfg: &SimConfig) {
    for i in 0..state.players.len() {
        let p = state.players[i];
        if p.time_scale <= 0.0 || !p.alive || p.incapacitated() {
            continue;
        }
        let req = pawn_states::request(state, p.id);
        let Some(s) = state.pawn_states.get(&p.id) else { continue };
        let l = s.loco;
        if s.loco_off || s.rm_mode == root_mode::IGNORE || !l.anim_driven() {
            continue;
        }
        let Some(sp) = l.specific.filter(|x| x.playing && x.time > x.prev) else { continue };
        let Some(rm) = cfg.anim.info(sp.seq).and_then(|n| n.root_motion.as_ref()).and_then(|k| cfg.gameplay.root_motion.get(k)) else { continue };
        let (a, b) = (rm.at(sp.prev), rm.at(sp.time));
        let (f, r) = ((b[0] - a[0]) * sp.weight, (b[1] - a[1]) * sp.weight);
        // The translation in the root's frame at the last sample (the sequence's keys are in its
        // first frame's): the pawn's yaw less the root's turn so far.
        let frame = p.yaw - rm.yaw_at(sp.prev);
        if l.state == state::TURN_IN {
            let dy = (rm.yaw_at(sp.time) - rm.yaw_at(sp.prev)) * sp.weight * l.turn.scale;
            state.players[i].yaw = wrap(p.yaw + dy);
        }
        let falling = !matches!(p.mode, crate::state::MoveMode::Walking) || p.thrown.active || p.thrown.ragdoll;
        if falling {
            continue;
        }
        let mult = config(cfg, req.loco).map_or(1.0, |(lc, mi)| lc.modifiers[mi].speed_multiplier);
        let w = crate::movement::forward_vector(frame) * f + crate::movement::right_vector(frame) * r;
        let d = Vec3::new(w.x, w.y, 0.0);
        let dt = crate::DT * p.time_scale;
        let n = p.floor_normal;
        let v = if n.z < 0.99 && n.z.abs() >= 1e-4 && d.length_squared() > 1e-8 {
            Vec3::new(d.x, d.y, -(d.x * n.x + d.y * n.y) / n.z).normalize_or_zero() * (d.length() / dt * mult)
        } else {
            d / dt * mult
        };
        state.players[i].root_velocity = Some(v);
    }
}

/// The specific anim the tree's locomotion node is asked to play this update (its sequence name
/// and blend time; None once it blends out, which the evaluator fades over its blend).
pub fn tree_request<'a>(state: &WorldState, id: PlayerId, cfg: &'a SimConfig) -> Option<(&'a str, f32)> {
    let sp = state.pawn_states.get(&id)?.loco.specific?;
    if !sp.playing {
        return None;
    }
    let end = cfg.anim.info(sp.seq).map_or(0.0, |n| n.duration) * sp.cut;
    if end - sp.time < sp.blend {
        return None;
    }
    cfg.anim.name(sp.seq).map(|n| (n, sp.blend))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_wraps() {
        assert!(crossed(0.4, 0.5, 0.488));
        assert!(!crossed(0.5, 0.6, 0.488));
        assert!(crossed(0.95, 0.02, 0.0));
        assert!(!crossed(0.1, 0.2, 0.0));
    }

    #[test]
    fn yaw_wrap() {
        assert_eq!(wrap(190.0), -170.0);
        assert_eq!(wrap(-180.0), 180.0);
        assert_eq!(wrap(45.0), 45.0);
    }
}
