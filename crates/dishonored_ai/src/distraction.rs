//! NPC distractions (`DisNPCDistractor` / `DisNPCDistractionComponent`, the brain's
//! `DisAISubProcessDistractions` and `DisBehaviorInteract`), decoded from `Dishonored.exe`
//! (research/dishonored/systems/kismet.md 3.1, world.md section 9):
//!
//! - **Who is near** (`DisDistractionGlobalManager`, 0x00b52170 from the game info tick, per
//!   brain 0x00b52050): every brain whose distractions sub-process is active (the sub-process is
//!   owned by the patrol behaviour tweaks, `TweaksBehavior_Patrol_*`, so: the patrol behaviour is
//!   current) and whose pawn is `m_bAwareOfDistraction` (the class default, true) is put in each
//!   distractor's `m_NearbyBrains` (5 slots) when within `m_fDistractionRange` of it, and taken
//!   out otherwise (0x00b80f30). A brain entering `m_NearbyBrains` is also put in
//!   `m_BrainsToCheckOnTick` (0x00b6ec60 / 0x00b6ec00). A brain whose sub-process stops
//!   (0x00b2b040) is taken out of every distractor.
//! - **The component tick** (slot 79, 0x00b84e90): `m_fCooldownTimer -= dt`, then the selection
//!   (0x00b842f0): the distractor is ready when enabled, the cooldown is over and no pawn is
//!   linked (going to it or animating). Without a pulse (`m_fDistractionPulseTime < 0`) the
//!   candidates are the brains that came in range since the last tick; with one, the pulse timer
//!   counts down and on the tick it runs out the candidates are all the nearby brains (and the
//!   timer reloads). The check list is emptied every tick. Each candidate in slot order gets the
//!   chance roll (0x00b83bb0) while the distractor stays ready.
//! - **The roll** (0x00b83bb0): the pawn has no distraction linked, its own countdown
//!   (`m_fDistractionCooldownTime_Countdown`) is over, its tweak passes `m_DistractablePawnTweaks`
//!   (if any) and `m_NonDistractablePawnTweaks` (0x00b74b00, the pawn tweak's lineage), its current
//!   patrol route is one of `m_AllowedRoutes` (if any, 0x00b74b60), and it has chances
//!   (0x00b6eca0: the pawn's locomotion modifier index 0 / 1 / 4 with suspicion level 0 ->
//!   `m_SuspicionLvl1_Chances`; 5 / 6 / 10 -> `m_SuspicionLvl234_Chances`; others none). The chances
//!   are normalized by `max(look + animate, 1)`; with `r` uniform in [0, 1): look when `look > 0`
//!   and `look >= r`, else animate when `animate > 0` and `look + animate >= r`; then the line of
//!   sight (0x00b81010, when `m_bRequireLOS`: the distractor inside the vision cones and a clear
//!   trace from the eye) and the message to the brain: look (0x1c) or animate (0x1b).
//! - **Look** (`DisAISubProcessDistractions`, 0x00b2b150 / 0x00b28550): taken when the
//!   sub-process has no look target and the distractor is within 90 degrees of the pawn's facing;
//!   `DisSeqEvent_Distracted` output 2 fires (0x00b7da20), the pawn's countdown is set to its
//!   tweak's `m_fDistractionCooldownTime`, the look lasts `m_DistractionTime` (none -> forever)
//!   and ends early when the distractor leaves the 90 degrees (0x00b2b080) or the sub-process
//!   stops.
//! - **Animate** (`DisBehaviorInteract`, slot Distraction): evaluated when nobody goes to the
//!   distractor and the animate chance is positive (0x00ae3ca0); the pawn walks to the
//!   distractor's location and rotation, linked as the go-to pawn (0x00b80e80). On arrival
//!   (follow-up 0x00b84d60) output 0 fires and the pawn is the animating pawn: a soiree
//!   distraction plays its matinee (0x00b84750), an animated one switches its weapon away then
//!   plays `Distraction_<type>` (0x00b84670 / 0x00b84c20). The end fires output 1: the soiree's end
//!   (0x00b844f0), or the distracted state's exit (0x00b84810, which also sets the pawn's
//!   countdown). Every output resets the distractor's cooldown (0x00b79b80).
//!
//! Port limits (stated in world.md): the soiree's duration is computed from its matinee's
//! loop keys and the distractor's loop counts ([`soiree_duration`]) rather than taken from the
//! Kismet matinee the host plays; the animated variant's In / Out animation lengths are not
//! loaded (the loop alone, `m_DistractionTime`); the weapon switch takes one tick;
//! `m_bLowerAIAlertness` is not applied; a distraction is given up when a higher behaviour
//! becomes current (on the way: no event; animating: output 1).

use super::*;
use dishonored_ir::ai::{DistractionChances, DistractorDef};

/// `DISTRACTION_MAX_BRAINS`.
pub const MAX_BRAINS: usize = 5;

/// `EDisAnimDistractionState`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnimState {
    #[default]
    GoingTo,
    WeaponSwitching,
    Animating,
}

/// Runtime state of a placed distractor's component.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Distractor {
    pub def: DistractorDef,
    /// `m_bDistractionEnabled` (Kismet `SeqAct_Toggle`, 0x00a4c350).
    pub enabled: bool,
    /// `m_fCooldownTimer`, `m_fPulseTimer`.
    pub cooldown_timer: f32,
    pub pulse_timer: f32,
    /// `m_NearbyBrains`, `m_BrainsToCheckOnTick` (brain ids).
    pub nearby: [Option<PlayerId>; MAX_BRAINS],
    pub check: [Option<PlayerId>; MAX_BRAINS],
    /// `m_pLinkedGoToPawn`, `m_pLinkedAnimPawn`.
    pub linked_goto: Option<PlayerId>,
    pub linked_anim: Option<PlayerId>,
    /// `m_AnimDistractionState`.
    pub anim_state: AnimState,
}

/// The `DisBehaviorInteract` started for a distraction (slot Distraction).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DistractionState {
    /// Index into `Crowd::distractors`.
    pub distractor: usize,
    pub phase: AnimState,
    /// Animating: time left (None: until interrupted).
    pub left: Option<f32>,
    /// Going: arrived.
    pub reached: bool,
}

/// The `DisSeqEvent_Distracted` outputs.
pub mod output {
    pub const START: u8 = 0;
    pub const END: u8 = 1;
    pub const LOOKED_AT: u8 = 2;
}

impl Distractor {
    pub fn new(def: DistractorDef, seed: u64) -> Self {
        // Registration (slot 78, 0x00b6eb10): the pulse starts at a random phase.
        let mut s = seed | 1;
        let pulse_timer = def.pulse * rng_next(&mut s);
        Distractor { enabled: def.enabled, def, pulse_timer, ..Default::default() }
    }

    /// 0x00b6ebb0: ready for a distraction (`ignore_cooldown` for the Kismet start).
    pub fn ready(&self, ignore_cooldown: bool) -> bool {
        self.enabled && (ignore_cooldown || self.cooldown_timer <= 0.0) && self.linked_goto.is_none() && self.linked_anim.is_none()
    }

    fn slot_of(list: &[Option<PlayerId>; MAX_BRAINS], id: PlayerId) -> Option<usize> {
        list.iter().position(|x| *x == Some(id))
    }

    /// 0x00b6ec60: into `m_NearbyBrains` (and, when new, `m_BrainsToCheckOnTick`).
    fn add(&mut self, id: PlayerId) {
        if Self::slot_of(&self.nearby, id).is_some() {
            return;
        }
        let Some(i) = self.nearby.iter().position(|x| x.is_none()) else { return };
        self.nearby[i] = Some(id);
        if Self::slot_of(&self.check, id).is_none() {
            if let Some(j) = self.check.iter().position(|x| x.is_none()) {
                self.check[j] = Some(id);
            }
        }
    }

    /// 0x00b80f30: out of both lists; returns true when the brain was the linked go-to pawn
    /// (its go-to is cancelled).
    fn remove(&mut self, id: PlayerId) -> bool {
        let Some(i) = Self::slot_of(&self.nearby, id) else { return false };
        self.nearby[i] = None;
        if let Some(j) = Self::slot_of(&self.check, id) {
            self.check[j] = None;
        }
        self.linked_goto == Some(id)
    }

    /// The chances the pawn has (0x00b6eca0): by its locomotion modifier index and suspicion.
    pub fn chances(&self, modifier: Option<usize>, suspicion: u8) -> Option<DistractionChances> {
        match modifier? {
            0 | 1 | 4 if suspicion == 0 => Some(self.def.chances_lvl1),
            5 | 6 | 10 => Some(self.def.chances_lvl234),
            _ => None,
        }
    }
}

/// The roll of 0x00b83bb0 on normalized chances: Some(true) look, Some(false) animate.
pub fn roll(c: DistractionChances, r: f32) -> Option<bool> {
    let sum = (c.look + c.animate).max(1.0);
    let (look, anim) = (c.look * (1.0 / sum), c.animate * (1.0 / sum));
    if look > 0.0 && look >= r {
        Some(true)
    } else if anim > 0.0 && anim + look >= r {
        Some(false)
    } else {
        None
    }
}

/// How long the soiree plays: its matinee (the distraction's own `SeqAct_Interp` plays
/// `m_pDistractionMatineeData` with the pawn bound) runs to `InterpLength`; a soiree loop key
/// plays its section `m_LoopCount` times (0x00900480) and holds it until broken when the count
/// is 0, a pause key holds until broken. On start (0x00b79cf0) every `m_lDistractionSoireeLoops`
/// entry with `0 < min <= max` sets the count of its pin to `rand % (max - min + 1) + min`
/// (FUN_00618b40). None: held until interrupted.
pub fn soiree_duration(def: &DistractorDef, rng: &mut u64) -> Option<f32> {
    let mut counts: Vec<(String, i32)> = Vec::new();
    for (pin, lo, hi) in &def.soiree_loops {
        if hi >= lo && *lo > 0 && *hi > 0 {
            let r = (rng_next(rng) * 32768.0) as i32;
            counts.push((pin.clone(), r % (hi - lo + 1) + lo));
        }
    }
    let mut total = def.soiree_length;
    for k in &def.soiree_keys {
        if k.start >= def.soiree_length {
            continue;
        }
        if k.pause {
            return None;
        }
        let n = counts.iter().find(|c| c.0.eq_ignore_ascii_case(&k.pin)).map_or(k.loops, |c| c.1);
        if n <= 0 || k.length <= 0.0 {
            if n <= 0 && k.length > 0.0 {
                return None;
            }
            continue;
        }
        total += (n - 1) as f32 * k.length;
    }
    Some(total)
}

/// `m_DistractionTime` sampled (`RawDistributionFloat::GetValue`, 0x00434ef0, time 0).
fn sample_time(def: &DistractorDef, r: f32) -> f32 {
    if def.time.table.is_empty() { 0.0 } else { def.time.eval1(0.0, r) }
}

/// Whether a point is within `deg` degrees of the pawn's facing (0x00bbfa60).
fn within(me: &PlayerState, p: Vec3, deg: f32) -> bool {
    let d = p - me.position;
    if d.length() < 1e-4 {
        return true;
    }
    let (s, c) = me.yaw.to_radians().sin_cos();
    Vec3::new(c, s, 0.0).dot(d.normalize()) >= deg.to_radians().cos()
}

impl Brain {
    /// The distractions sub-process is active: the patrol behaviour (its owner) is current.
    pub fn distraction_listening(&self) -> bool {
        self.current_slot() == slot::IDLE && self.state == BrainState::Patrol
    }

    /// The pawn tweak filters (0x00b74b00): in `m_DistractablePawnTweaks` when that is set, and
    /// not in `m_NonDistractablePawnTweaks`, by the pawn tweak's lineage.
    fn tweak_passes(&self, def: &DistractorDef) -> bool {
        let isa = |t: &String| self.lineage.iter().any(|l| l.eq_ignore_ascii_case(t));
        (def.distractable.is_empty() || def.distractable.iter().any(isa)) && !def.non_distractable.iter().any(isa)
    }

    /// The route filter (0x00b74b60): with `m_AllowedRoutes`, a patrolling brain's route must be
    /// one of them.
    fn route_passes(&self, def: &DistractorDef, routes: &[PatrolRoute]) -> bool {
        if def.routes.is_empty() || self.state != BrainState::Patrol {
            return true;
        }
        match self.patrol.and_then(|r| routes.get(r)) {
            Some(r) => def.routes.iter().any(|n| n.eq_ignore_ascii_case(&r.name)),
            None => false,
        }
    }

    /// The locomotion modifier index the current behaviour selects (`m_pCpntLocomotion`
    /// +0x1a0, the index into the config's `m_Modifiers`); see [`Brain::loco_speed`].
    pub fn loco_modifier(&self, ai: &AiConfig) -> Option<usize> {
        let cfg = self.loco_config(ai)?;
        let armed = self.kind == PawnKind::Guard;
        let want: &[&str] = match (self.state, armed) {
            (BrainState::Investigate | BrainState::Search | BrainState::RingAlarm, true) => &["SearchSword", "Sword"],
            (BrainState::Investigate | BrainState::Search | BrainState::RingAlarm, false) => &["SearchEmpty", "Empty"],
            (BrainState::Flee | BrainState::Cower, true) => &["PanicSword", "Sword"],
            (BrainState::Flee | BrainState::Cower, false) => &["PanicEmpty", "Empty"],
            (BrainState::Combat | BrainState::Notice | BrainState::Awe, true) => &["Sword"],
            (_, true) => &["EmptyPatrol", "Empty"],
            _ => &["Empty"],
        };
        want.iter().find_map(|w| cfg.modifier(w))
    }

    /// The line of sight to the distractor (0x00b81010).
    fn sees_point(&self, me: &PlayerState, p: Vec3, world: &dyn WorldQuery, ai: &AiConfig) -> bool {
        let (inner, outer) = cones(&ai.vision);
        let veye = self.vision_eye(me, ai);
        if vision_distance(&inner, &outer, veye.local(p)).is_none() {
            return false;
        }
        let eye = veye.pos;
        let to = p - eye;
        let d = to.length();
        d < 1.0 || world.raycast(eye, to / d, d, channel::VISION).is_none()
    }

    /// The distraction behaviour (`DisBehaviorInteract`, slot Distraction).
    pub(crate) fn act_distraction(&mut self, me: &PlayerState, nav: Option<&NavWorld>, ctx: &mut Ctx, ai: &AiConfig, dt: f32) -> PlayerInput {
        let idle = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        let Some(Some(Behavior::Distraction(mut s))) = self.slots.get(slot::DISTRACTION as usize).cloned() else { return idle };
        let Some(d) = ctx.distractors.get_mut(s.distractor) else {
            self.complete(slot::DISTRACTION);
            return idle;
        };
        let out = match s.phase {
            AnimState::GoingTo => {
                let dest = d.def.position;
                let end = self.follow.path.last().copied();
                let arrived = match end {
                    Some(e) => Self::reached(me, e, 0.1) || (self.stuck > 1.0 && Self::reached(me, dest, 60.0)),
                    None => Self::reached(me, dest, 60.0) || self.follow.failed,
                };
                if !arrived {
                    self.goal = Some(dest);
                    return self.move_to(me, nav, dest, false, ai);
                }
                self.follow.clear();
                self.goal = None;
                // The follow-up (0x00b84d60) and the go-to's unlink (0x00b80e80).
                d.linked_goto = None;
                self.distraction = None;
                d.linked_anim = Some(self.id);
                d.cooldown_timer = d.def.cooldown;
                ctx.events.push(AiEvent::Distracted { distractor: s.distractor, pawn: self.id, output: output::START });
                if d.def.soiree {
                    d.anim_state = AnimState::Animating;
                    s.phase = AnimState::Animating;
                    s.left = soiree_duration(&d.def, &mut self.rng);
                } else {
                    d.anim_state = AnimState::WeaponSwitching;
                    s.phase = AnimState::WeaponSwitching;
                }
                PlayerInput { yaw: d.def.yaw, left_hand: LeftHand::Empty, ..Default::default() }
            }
            AnimState::WeaponSwitching => {
                d.anim_state = AnimState::Animating;
                s.phase = AnimState::Animating;
                s.left = Some(sample_time(&d.def, rng_next(&mut self.rng)));
                PlayerInput { yaw: d.def.yaw, left_hand: LeftHand::Empty, ..Default::default() }
            }
            AnimState::Animating => {
                if let Some(t) = s.left.as_mut() {
                    *t -= dt;
                    if *t <= 0.0 {
                        let soiree = d.def.soiree;
                        self.end_distraction(s.distractor, ctx, !soiree);
                        self.complete(slot::DISTRACTION);
                        return PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
                    }
                }
                PlayerInput { yaw: d.def.yaw, left_hand: LeftHand::Empty, ..Default::default() }
            }
        };
        if let Some(Some(Behavior::Distraction(slot_s))) = self.slots.get_mut(slot::DISTRACTION as usize) {
            *slot_s = s;
        }
        out
    }

    /// The distraction ends (output 1): the soiree's end (0x00b844f0), or the distracted
    /// state's exit (0x00b84810, which also sets the pawn's countdown).
    fn end_distraction(&mut self, distractor: usize, ctx: &mut Ctx, state_exit: bool) {
        let Some(d) = ctx.distractors.get_mut(distractor) else { return };
        if d.linked_anim != Some(self.id) {
            return;
        }
        if state_exit {
            self.distraction_countdown = self.distraction_cooldown_time;
        }
        d.cooldown_timer = d.def.cooldown;
        d.linked_anim = None;
        ctx.events.push(AiEvent::Distracted { distractor, pawn: self.id, output: output::END });
    }

    /// A distraction that is no longer the current behaviour is given up: on the way the go-to is
    /// unlinked (0x00b80e80); animating, the distracted state exits (0x00b84810, output 1).
    pub(crate) fn drop_distraction(&mut self, ctx: &mut Ctx) {
        let Some(Some(Behavior::Distraction(s))) = self.slots.get(slot::DISTRACTION as usize).cloned() else { return };
        if self.current_slot() == slot::DISTRACTION {
            return;
        }
        if s.phase == AnimState::GoingTo {
            if let Some(d) = ctx.distractors.get_mut(s.distractor) {
                if d.linked_goto == Some(self.id) {
                    d.linked_goto = None;
                }
            }
            self.distraction = None;
        } else {
            self.end_distraction(s.distractor, ctx, true);
        }
        self.slots[slot::DISTRACTION as usize] = None;
    }
}

impl Crowd {
    /// Install the level's distractors.
    pub fn set_distractors(&mut self, defs: &[DistractorDef]) {
        self.distractors = defs.iter().enumerate().map(|(i, d)| Distractor::new(d.clone(), 0xD15_7AC7 ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))).collect();
    }

    /// Index of a distractor by actor name (and package when given).
    pub fn distractor(&self, package: Option<&str>, name: &str) -> Option<usize> {
        self.distractors.iter().position(|d| d.def.name.eq_ignore_ascii_case(name) && package.is_none_or(|p| d.def.package.eq_ignore_ascii_case(p)))
    }

    /// `SeqAct_Toggle` on a `DisNPCDistractor` (0x00a4c350 -> 0x00b83b60): input 0 on, 1 off,
    /// else toggle; switching off unlinks the go-to pawn.
    pub fn toggle_distractor(&mut self, i: usize, input: u8) {
        let Some(d) = self.distractors.get_mut(i) else { return };
        let on = match input {
            0 => true,
            1 => false,
            _ => !d.enabled,
        };
        d.enabled = on;
        if !on {
            if let Some(p) = d.linked_goto.take() {
                if let Some(b) = self.brains.iter_mut().find(|b| b.id == p) {
                    b.distraction = None;
                    if matches!(b.behavior(slot::DISTRACTION), Some(Behavior::Distraction(s)) if s.phase == AnimState::GoingTo) {
                        b.complete(slot::DISTRACTION);
                        b.shown = b.state;
                    }
                }
            }
        }
    }

    /// `DisSeqAct_AIStartDistraction` (`OnAIStartDistraction`, 0x00b63ed0): the distractor is
    /// ready (its cooldown ignored) and the pawn has a positive animate chance: the animate
    /// message (0x00b60800). Returns whether the pawn took it.
    pub fn start_distraction(&mut self, pawn: PlayerId, i: usize) -> bool {
        let ai = self.ai.clone();
        let Some(d) = self.distractors.get(i) else { return false };
        if !d.ready(true) {
            return false;
        }
        let Some(bi) = self.brains.iter().position(|b| b.id == pawn) else { return false };
        let c = d.chances(self.brains[bi].loco_modifier(&ai), self.brains[bi].suspicion_level());
        if !c.is_some_and(|c| c.animate > 0.0) {
            return false;
        }
        self.animate_distraction(bi, i)
    }

    /// The animate message taken by `DisBehaviorInteract` (evaluate 0x00ae3ca0: nobody goes to the
    /// distractor; start 0x00ae9480: walk to its location and rotation, linked as the go-to pawn).
    fn animate_distraction(&mut self, bi: usize, i: usize) -> bool {
        let d = &mut self.distractors[i];
        if d.linked_goto.is_some() {
            return false;
        }
        let b = &mut self.brains[bi];
        // A pawn linked elsewhere is unlinked there first (0x00b80e80).
        if let Some(old) = b.distraction.take() {
            if let Some(o) = self.distractors.get_mut(old) {
                if o.linked_goto == Some(b.id) {
                    o.linked_goto = None;
                }
            }
        }
        // The interact behaviour puts the patrol (the distractions sub-process' owner) aside:
        // the sub-process stops and takes the brain out of every distractor (0x00b2b040 ->
        // 0x00b5dcd0) before the go-to links it.
        let id = b.id;
        for o in self.distractors.iter_mut() {
            o.remove(id);
        }
        let b = &mut self.brains[bi];
        let d = &mut self.distractors[i];
        d.linked_goto = Some(b.id);
        d.anim_state = AnimState::GoingTo;
        b.distraction = Some(i);
        b.follow.clear();
        b.place(Behavior::Distraction(DistractionState { distractor: i, ..Default::default() }));
        // The brain's own decision, not an external write of its state.
        b.shown = b.state;
        true
    }

    /// The distraction manager and component ticks (see the module docs); runs before the
    /// brains act. Returns the `DisSeqEvent_Distracted` activations.
    pub(crate) fn tick_distractions(&mut self, ws: &WorldState, world: &dyn WorldQuery, dt: f32) -> Vec<AiEvent> {
        let mut out = Vec::new();
        // The pawns' countdowns (NPC pawn tick 0x00b6da40).
        for b in self.brains.iter_mut() {
            b.distraction_countdown -= dt;
        }
        if self.distractors.is_empty() {
            return out;
        }
        // Links whose pawn no longer runs the distraction (killed, removed, or its behaviour
        // replaced by a scripted one): the go-to is dropped; an animating pawn's distracted
        // state exits (0x00b84810: output 1, the pawn's countdown).
        for (i, d) in self.distractors.iter_mut().enumerate() {
            let runs = |id: PlayerId, going: bool| {
                self.brains.iter().any(|b| b.id == id && ws.player(id).is_some_and(|p| p.alive) && matches!(b.behavior(slot::DISTRACTION), Some(Behavior::Distraction(s)) if s.distractor == i && (s.phase == AnimState::GoingTo) == going))
            };
            if d.linked_goto.is_some_and(|id| !runs(id, true)) {
                d.linked_goto = None;
            }
            if let Some(id) = d.linked_anim.filter(|id| !runs(*id, false)) {
                d.linked_anim = None;
                d.cooldown_timer = d.def.cooldown;
                if let Some(b) = self.brains.iter_mut().find(|b| b.id == id) {
                    b.distraction_countdown = b.distraction_cooldown_time;
                }
                out.push(AiEvent::Distracted { distractor: i, pawn: id, output: output::END });
            }
        }
        // The manager's range update (0x00b52170 / 0x00b52050) and the sub-process stop.
        let mut cancel = Vec::new();
        for b in &self.brains {
            let Some(me) = ws.player(b.id).filter(|p| p.alive) else {
                for d in self.distractors.iter_mut() {
                    if d.remove(b.id) {
                        cancel.push(b.id);
                    }
                }
                continue;
            };
            let listening = b.distraction_listening();
            for d in self.distractors.iter_mut() {
                if listening && me.position.distance(d.def.position) <= d.def.range {
                    d.add(b.id);
                } else if d.remove(b.id) {
                    cancel.push(b.id);
                }
            }
        }
        for id in cancel {
            for d in self.distractors.iter_mut().filter(|d| d.linked_goto == Some(id)) {
                d.linked_goto = None;
            }
            if let Some(b) = self.brains.iter_mut().find(|b| b.id == id) {
                b.distraction = None;
                if matches!(b.behavior(slot::DISTRACTION), Some(Behavior::Distraction(s)) if s.phase == AnimState::GoingTo) {
                    b.complete(slot::DISTRACTION);
                    b.shown = b.state;
                }
            }
        }
        // The brains' look-at sub-process (0x00b2b080 tick; 0x00b2b040 stop).
        for b in self.brains.iter_mut() {
            let Some((i, t)) = b.look_distraction else { continue };
            let me = ws.player(b.id);
            let keep = b.distraction_listening() && t - dt > 0.0 && me.zip(self.distractors.get(i)).is_some_and(|(me, d)| within(me, d.def.position, 90.0));
            b.look_distraction = keep.then_some((i, t - dt));
        }
        // The component ticks.
        let ai = self.ai.clone();
        for i in 0..self.distractors.len() {
            let d = &mut self.distractors[i];
            d.cooldown_timer -= dt;
            let ready = d.ready(false);
            let pulse = d.def.pulse >= 0.0 && {
                d.pulse_timer -= dt;
                d.pulse_timer <= 0.0
            };
            let mut cands = [None; MAX_BRAINS];
            let mut go = false;
            if ready {
                go = true;
                if pulse {
                    cands = d.nearby;
                    d.pulse_timer = d.def.pulse;
                } else {
                    cands = d.check;
                }
            }
            d.check = [None; MAX_BRAINS];
            if !go {
                continue;
            }
            for id in cands.into_iter().flatten() {
                if !self.distractors[i].ready(false) {
                    break;
                }
                let Some(bi) = self.brains.iter().position(|b| b.id == id) else { continue };
                let Some(me) = ws.player(id) else { continue };
                // 0x00b4b3c0: `m_bAwareOfDistraction` (true for every pawn the port has).
                if let Some(ev) = self.roll_distraction(bi, i, me, world, &ai) {
                    out.push(ev);
                }
            }
        }
        out
    }

    /// 0x00b83bb0 for one candidate; the look message is taken at once (output 2).
    fn roll_distraction(&mut self, bi: usize, i: usize, me: &PlayerState, world: &dyn WorldQuery, ai: &AiConfig) -> Option<AiEvent> {
        let b = &self.brains[bi];
        let d = &self.distractors[i];
        if b.distraction.is_some() || b.distraction_countdown > 0.0 || !b.tweak_passes(&d.def) || !b.route_passes(&d.def, &self.routes) {
            return None;
        }
        let c = d.chances(b.loco_modifier(ai), b.suspicion_level())?;
        let r = rng_next(&mut self.brains[bi].rng);
        let look = roll(c, r)?;
        let b = &self.brains[bi];
        if d.def.require_los && !self.brains[bi].sees_point(me, d.def.position, world, ai) {
            return None;
        }
        if look {
            // `DisAISubProcessDistractions` (0x00b2b150): no look target yet and within 90 degrees.
            if b.look_distraction.is_some() || !within(me, d.def.position, 90.0) {
                return None;
            }
            let t = sample_time(&d.def, rng_next(&mut self.brains[bi].rng));
            let b = &mut self.brains[bi];
            b.look_distraction = Some((i, if t <= 0.0 { f32::MAX } else { t }));
            b.distraction_countdown = b.distraction_cooldown_time;
            let d = &mut self.distractors[i];
            d.cooldown_timer = d.def.cooldown;
            return Some(AiEvent::Distracted { distractor: i, pawn: me.id, output: output::LOOKED_AT });
        }
        let c = self.distractors[i].chances(self.brains[bi].loco_modifier(ai), self.brains[bi].suspicion_level());
        if c.is_some_and(|c| c.animate > 0.0) {
            self.animate_distraction(bi, i);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roll_normalizes_and_prefers_the_look() {
        let c = DistractionChances { look: 0.2, animate: 0.8 };
        assert_eq!(roll(c, 0.1), Some(true));
        assert_eq!(roll(c, 0.2), Some(true));
        assert_eq!(roll(c, 0.5), Some(false));
        assert_eq!(roll(c, 0.99), Some(false));
        // Below 1 in total the chances are not scaled up: a miss is possible.
        let c = DistractionChances { look: 0.0, animate: 0.5 };
        assert_eq!(roll(c, 0.4), Some(false));
        assert_eq!(roll(c, 0.6), None);
        // Above 1 they are scaled down to 1.
        let c = DistractionChances { look: 1.0, animate: 1.0 };
        assert_eq!(roll(c, 0.5), Some(true));
        assert_eq!(roll(c, 0.75), Some(false));
        assert_eq!(roll(DistractionChances::default(), 0.0), None);
    }

    #[test]
    fn chances_by_modifier_and_suspicion() {
        let d = Distractor::new(DistractorDef { chances_lvl1: DistractionChances { look: 0.2, animate: 0.8 }, chances_lvl234: DistractionChances { look: 0.6, animate: 0.4 }, ..Default::default() }, 1);
        assert_eq!(d.chances(Some(4), 0).map(|c| c.look), Some(0.2), "EmptyPatrol, unsuspecting");
        assert_eq!(d.chances(Some(4), 1), None, "EmptyPatrol, suspecting");
        assert_eq!(d.chances(Some(10), 1).map(|c| c.look), Some(0.6), "SwordPatrol");
        assert_eq!(d.chances(Some(2), 0), None, "PanicEmpty");
        assert_eq!(d.chances(None, 0), None, "no locomotion");
    }

    #[test]
    fn soiree_duration_with_loop_counts() {
        use dishonored_ir::ai::SoireeKeyDef;
        let mut d = DistractorDef { soiree_length: 5.0, soiree_keys: vec![SoireeKeyDef { pin: "Loop".into(), start: 1.0, length: 2.0, loops: 0, pause: false }], ..Default::default() };
        let mut rng = 7;
        assert_eq!(soiree_duration(&d, &mut rng), None, "a loop without a count holds");
        d.soiree_loops = vec![("Loop".into(), 1, 1)];
        assert_eq!(soiree_duration(&d, &mut rng), Some(5.0), "played once");
        d.soiree_loops = vec![("Loop".into(), 3, 3)];
        assert_eq!(soiree_duration(&d, &mut rng), Some(9.0), "played three times");
        d.soiree_loops = vec![("Other".into(), 2, 2)];
        assert_eq!(soiree_duration(&d, &mut rng), None);
        d.soiree_keys[0].loops = 2;
        assert_eq!(soiree_duration(&d, &mut rng), Some(7.0), "the key's own count");
        d.soiree_keys.push(SoireeKeyDef { pin: "Hold".into(), start: 4.0, pause: true, ..Default::default() });
        assert_eq!(soiree_duration(&d, &mut rng), None, "a pause holds");
    }

    #[test]
    fn nearby_and_check_lists() {
        let mut d = Distractor::new(DistractorDef::default(), 1);
        d.add(7);
        d.add(7);
        assert_eq!(d.nearby[0], Some(7));
        assert_eq!(d.check[0], Some(7));
        assert!(d.nearby[1..].iter().all(|x| x.is_none()));
        for id in 8..14 {
            d.add(id);
        }
        assert_eq!(d.nearby.iter().flatten().count(), MAX_BRAINS, "five slots");
        d.linked_goto = Some(8);
        assert!(d.remove(8), "the linked go-to pawn's go-to is cancelled");
        assert!(!d.remove(99));
    }
}
