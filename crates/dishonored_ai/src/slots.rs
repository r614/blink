//! The brain's decision machinery (`DishonoredAIBrain`, research/dishonored/systems/ai.md §1).
//!
//! - **Slots** (`m_ActiveBehaviorStack[19]`, one behaviour per `EAISlot`): the behaviour in
//!   the highest occupied slot is the current one; the others stay alive but dormant and
//!   resume when everything above them has finished (0x00b17100). A behaviour placed in a slot
//!   aborts the slot's occupant and, when its slot is higher than the current one, becomes
//!   current (0x00b11960).
//! - **Stimuli** (`m_StimQueue`, `DisStimRef` = stimulus + delay): a stimulus waits its delay
//!   (the stimulus class's delay getter: 0 for the attention-behaviour stimuli, the global
//!   `m_ReactionDelay` for SearchRequest / Help, `m_MagicReactionDelay` for WitnessMagic) and is
//!   then dispatched (0x00b16eb0): the active behaviours that complete on it end, and every
//!   behaviour class whose evaluate mask contains it is started in its slot.
//! - **The attention poster** (0x00b25e50 / 0x00b25890 / 0x00b24d00 / 0x00b22ec0): the
//!   attention component keeps a behaviour level for its top target and steps it one level at
//!   a time toward the target's attention level, posting `NoticeBegin` / `SearchBegin` /
//!   `CombatBegin` on each step up (levels 2, 3, 4) and the matching `*End` on each step down,
//!   only while the target is an enemy (`ERelationship` 2) and, for the begins, alive. A new
//!   top target first steps the level down to 0 (the ends of the old one).

use super::*;

/// Number of `EAISlot` values (`EAISlot_MAX`).
pub const SLOT_COUNT: usize = 19;

/// `EAISlot` (DishonoredAIBehavior, ascending priority).
pub mod slot {
    pub const IDLE: u8 = 0;
    pub const FULFILL: u8 = 1;
    pub const ACT: u8 = 2;
    pub const NOTICE: u8 = 3;
    pub const PERFORM: u8 = 4;
    pub const SEARCH: u8 = 5;
    pub const DISTRACTION: u8 = 6;
    pub const REACT: u8 = 7;
    pub const ENGAGE: u8 = 8;
    pub const EVADE: u8 = 9;
    pub const RESIGN: u8 = 10;
    pub const COMBAT: u8 = 11;
    pub const ADAPT: u8 = 12;
    pub const ALERT: u8 = 13;
    pub const AWE: u8 = 14;
    pub const ESCAPE: u8 = 15;
    pub const PRESERVE: u8 = 16;
    pub const COMPLY: u8 = 17;
    pub const INHIBITED: u8 = 18;
}

/// The stimuli this port's behaviours use (`EAIStimID`, value in the comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stim {
    /// 71: `DisBehaviorPatrol` evaluates it (a spawner with `patrol_on_startup`, Kismet).
    PatrolRequest,
    /// 63 / 64.
    NoticeBegin,
    NoticeEnd,
    /// 87 / 88.
    SearchBegin,
    SearchEnd,
    /// 17 / 18.
    CombatBegin,
    CombatEnd,
    /// 68: posted by the civilians' panic process (`DisAIBrainProcessPanic`).
    Panicked,
    /// 112: a power seen (0x00b23e50); `flag` is `m_bDisappeared`.
    WitnessMagic,
}

/// A queued stimulus (`DisStimRef`): the stimulus, its remaining delay, and its payload (the
/// attention target or the magic user, where the NPC believes it is, and a flag).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedStim {
    pub stim: Stim,
    pub delay: f32,
    pub who: PlayerId,
    pub at: Vec3,
    #[serde(default)]
    pub flag: bool,
    /// The reason warrants running (`m_SearchFromNoiseTweaks[..].m_bShouldRun`).
    #[serde(default)]
    pub urgent: bool,
}

/// `DisBehaviorSearch` sub-states in tweak order (`m_SubStateTweaks` of
/// `TweaksBehavior_Search_Default`): Investigate, GenericAction, Stand, StareAtUnreachable,
/// TrackTarget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SearchSub {
    #[default]
    Investigate,
    /// The search animation (`SearchLostTarget` / `CorpseCheck`).
    GenericAction,
    /// Facing the proxy for `m_fCancelInvestigateTimer` (0x00af4b10).
    Stand,
    /// Facing an unreachable proxy (0x00af4bd0).
    StareAtUnreachable,
    /// Wandering with the search crumbs.
    TrackTarget,
}

/// State of a `DisBehaviorSearch` (or a civilian's `DisBehaviorStationarySearch`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SearchState {
    pub sub: SearchSub,
    /// The attention target searched for (`NOISE` for a place).
    pub target: PlayerId,
    /// `DisAISubStateInvestigate.m_CachedProxyLoc` and `m_InvestigateMoveDest`.
    pub proxy: Vec3,
    pub dest: Vec3,
    /// `m_bReachedProxy`, `m_bInvestigationFinished`, `m_bWantsStareAtUnreachable`.
    pub reached: bool,
    pub finished: bool,
    pub wants_stare: bool,
    /// `m_PathingFailCount`.
    pub fails: u32,
    /// Sub-state timer (Stand: `m_fCancelInvestigateTimer`, Stare: `m_fStareAtUnreachableTimer`,
    /// GenericAction: the animation, StationarySearch: the duration).
    pub timer: f32,
    /// `m_fSearchAnimCooldownTimer` (-1 at start, 0x00af4380).
    pub anim_cooldown: f32,
    /// A civilian's `DisBehaviorStationarySearch` (looks toward the stimulus in place).
    pub stationary: bool,
    /// The proxy belongs to a corpse (the CorpseCheck animation).
    pub corpse: bool,
}

/// State of a `DisBehaviorMagicResponse` (`m_pMagicUser`, `m_bDisappeared`,
/// `m_bMagicUserIsAnAlly`, `m_bHasBarked`) and its GenericAction animation timer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MagicState {
    pub caster: PlayerId,
    pub disappeared: bool,
    pub ally: bool,
    pub barked: bool,
    pub anim: Option<f32>,
}

/// A behaviour occupying a slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Behavior {
    /// The routine behaviours of slot Idle (`DisBehaviorIdle` / `Patrol` / `Guard`, the scripted
    /// walks): their sub-state is `Brain::routine` (`Brain::state` while current).
    Routine,
    /// `DisBehaviorNotice` (slot Notice, `NoticeBegin`).
    Notice,
    /// `DisBehaviorSearch` / `DisBehaviorStationarySearch` (slot Search, `SearchBegin`).
    Search(SearchState),
    /// `DisBehaviorCombatEliteGuard` (slot Combat, `CombatBegin`); its state is the brain's
    /// fight fields.
    Combat,
    /// `DisBehaviorTriggerAlarm` (slot Alert).
    TriggerAlarm,
    /// `DisBehaviorMagicResponse` (slot Awe, `WitnessMagic`).
    Magic(MagicState),
    /// `DisBehaviorPanic` (slot Escape, `Panicked`): flee, or cower with the enemy close.
    Panic { cower: bool },
}

impl Behavior {
    /// `m_DesignatedSlot` of the behaviour tweak.
    pub fn slot(&self) -> u8 {
        match self {
            Behavior::Routine => slot::IDLE,
            Behavior::Notice => slot::NOTICE,
            Behavior::Search(_) => slot::SEARCH,
            Behavior::Combat => slot::COMBAT,
            Behavior::TriggerAlarm => slot::ALERT,
            Behavior::Magic(_) => slot::AWE,
            Behavior::Panic { .. } => slot::ESCAPE,
        }
    }
}

/// The initial stack: the routine behaviour in slot Idle (`BrainInit` starts `DisBehaviorIdle`).
pub fn routine_stack() -> Vec<Option<Behavior>> {
    let mut v = vec![None; SLOT_COUNT];
    v[0] = Some(Behavior::Routine);
    v
}

/// The slot an externally written `BrainState` stands for, and its behaviour.
fn behavior_for(s: BrainState, crumbs: u32, threat: Option<Vec3>) -> Behavior {
    match s {
        BrainState::Idle | BrainState::Wander | BrainState::WalkToPoint | BrainState::GroupIdle | BrainState::Patrol | BrainState::Alert => Behavior::Routine,
        BrainState::Notice | BrainState::React => Behavior::Notice,
        BrainState::Investigate | BrainState::Search => {
            let p = threat.unwrap_or(Vec3::ZERO);
            let _ = crumbs;
            Behavior::Search(SearchState { sub: if s == BrainState::Search { SearchSub::TrackTarget } else { SearchSub::Investigate }, target: NOISE, proxy: p, dest: p, anim_cooldown: -1.0, ..Default::default() })
        }
        BrainState::Combat => Behavior::Combat,
        BrainState::RingAlarm => Behavior::TriggerAlarm,
        BrainState::Awe => Behavior::Magic(MagicState::default()),
        BrainState::Flee => Behavior::Panic { cower: false },
        BrainState::Cower => Behavior::Panic { cower: true },
    }
}

impl Brain {
    /// The current behaviour's slot: the highest occupied one.
    pub fn current_slot(&self) -> u8 {
        (0..self.slots.len()).rev().find(|&i| self.slots[i].is_some()).unwrap_or(0) as u8
    }

    /// The behaviour in a slot.
    pub fn behavior(&self, s: u8) -> Option<&Behavior> {
        self.slots.get(s as usize).and_then(|b| b.as_ref())
    }

    fn ensure_stack(&mut self) {
        if self.slots.len() != SLOT_COUNT {
            self.slots.resize(SLOT_COUNT, None);
        }
        if self.slots[0].is_none() {
            self.slots[0] = Some(Behavior::Routine);
        }
    }

    /// The observable state of the current behaviour (`Brain::state`).
    pub fn project(&self) -> BrainState {
        match self.behavior(self.current_slot()) {
            None | Some(Behavior::Routine) => {
                if self.state.is_routine() {
                    self.state
                } else {
                    self.routine
                }
            }
            Some(Behavior::Notice) => BrainState::Notice,
            Some(Behavior::Search(s)) => {
                if s.sub == SearchSub::TrackTarget && !s.stationary {
                    BrainState::Search
                } else {
                    BrainState::Investigate
                }
            }
            Some(Behavior::Combat) => BrainState::Combat,
            Some(Behavior::TriggerAlarm) => BrainState::RingAlarm,
            Some(Behavior::Magic(_)) => BrainState::Awe,
            Some(Behavior::Panic { cower }) => {
                if *cower {
                    BrainState::Cower
                } else {
                    BrainState::Flee
                }
            }
        }
    }

    /// Keep the routine's sub-state while another behaviour runs, and restore it (or reset the
    /// per-behaviour scratch) when the current slot changes.
    fn on_current_change(&mut self, prev: u8, prev_state: BrainState) {
        let now = self.current_slot();
        if now == prev {
            return;
        }
        if prev == slot::IDLE && prev_state.is_routine() {
            self.routine = prev_state;
            self.routine_goal = self.goal;
        }
        self.timer = 0.0;
        self.follow.clear();
        if now == slot::IDLE {
            self.state = self.resume_routine();
            self.goal = self.routine_goal;
            self.alarm_enemy = None;
            self.target = None;
        } else {
            self.goal = None;
            self.state = self.project();
        }
    }

    /// Start a behaviour in its slot (0x00b11960): the occupant is aborted; a slot above the
    /// current one makes it current (the current one stays dormant in its slot).
    pub(crate) fn place(&mut self, b: Behavior) {
        self.ensure_stack();
        let prev = self.current_slot();
        let prev_state = self.state;
        let s = b.slot() as usize;
        if let Some(old) = self.slots[s].take() {
            self.on_abort(&old);
        }
        if matches!(b, Behavior::Combat) {
            self.end_fight();
        }
        self.slots[s] = Some(b);
        self.on_current_change(prev, prev_state);
        if s as u8 == self.current_slot() {
            // A restarted current behaviour also starts over.
            self.timer = 0.0;
            self.follow.clear();
            self.state = self.project();
        }
    }

    /// A behaviour finished (complete flag or `*End` stimulus): removed from its slot; the
    /// highest remaining one becomes current (0x00b17100).
    pub(crate) fn complete(&mut self, s: u8) {
        self.ensure_stack();
        if s == slot::IDLE {
            return;
        }
        let prev = self.current_slot();
        let prev_state = self.state;
        if let Some(old) = self.slots[s as usize].take() {
            self.on_abort(&old);
        }
        self.on_current_change(prev, prev_state);
    }

    fn on_abort(&mut self, b: &Behavior) {
        match b {
            Behavior::Combat => self.end_fight(),
            Behavior::Search(s) => {
                if s.target == NOISE || !self.enemies.contains(&s.target) {
                    self.threat_pos.get_or_insert(s.proxy);
                }
            }
            Behavior::TriggerAlarm => {
                self.bell = None;
            }
            _ => {}
        }
    }

    /// An external write of `Brain::state` (mission control, Kismet, the oracle runner): the
    /// behaviour that state stands for becomes current, everything above it is aborted, the
    /// routine takes the state when it is a routine one.
    pub(crate) fn force(&mut self, s: BrainState) {
        self.ensure_stack();
        let b = behavior_for(s, self.crumbs, self.threat_pos);
        let k = b.slot() as usize;
        for i in k + 1..SLOT_COUNT {
            if let Some(old) = self.slots[i].take() {
                self.on_abort(&old);
            }
        }
        if k == 0 {
            self.routine = s;
            self.routine_goal = self.goal;
        } else {
            if matches!(b, Behavior::Combat) && self.slots[k] != Some(Behavior::Combat) {
                self.end_fight();
            }
            self.slots[k] = Some(b);
        }
        self.state = s;
        self.shown = s;
    }

    /// Queue a stimulus.
    pub(crate) fn post(&mut self, stim: Stim, delay: f32, who: PlayerId, at: Vec3, flag: bool, urgent: bool) {
        self.stims.push(QueuedStim { stim, delay, who, at, flag, urgent });
    }

    /// Mission control (`DisSeqAct_AIPatrol`, a spawner's `m_bPatrolOnStartup`): post
    /// `EAIStimID_PatrolRequest`; `DisBehaviorPatrol` adopts a route for it.
    pub fn request_patrol(&mut self) {
        self.post(Stim::PatrolRequest, 0.0, NOISE, self.home, false, false);
    }

    /// The attention poster (see the module docs): step the behaviour level toward the top
    /// target's level, posting the behaviour stimuli. `focus` is the strongest stimulus.
    pub(crate) fn post_attention(&mut self, ws: &WorldState) {
        let top = self.focus();
        let (who, level, at, urgent) = match top {
            Some((w, l, p, u)) => (Some(w), l, p, u),
            None => (None, 0, Vec3::ZERO, false),
        };
        let enemy = who.is_some_and(|w| w == NOISE || self.enemies.contains(&w));
        if who != self.att_target || enemy != self.att_enemy {
            // A new top target (0x00b25e50): the old one's ends first.
            self.step_level(0, NOISE, Vec3::ZERO, false, true);
            self.att_target = who;
            self.att_enemy = enemy;
        }
        let alive = who.is_none_or(|w| w == NOISE || ws.player(w).is_some_and(|p| p.alive));
        self.step_level(level.min(DAL_BUSTED), who.unwrap_or(NOISE), at, urgent, alive);
    }

    fn step_level(&mut self, to: u8, who: PlayerId, at: Vec3, urgent: bool, alive: bool) {
        while self.att_level > to {
            if self.att_enemy {
                let end = match self.att_level {
                    2 => Some(Stim::NoticeEnd),
                    3 => Some(Stim::SearchEnd),
                    4 => Some(Stim::CombatEnd),
                    _ => None,
                };
                if let Some(s) = end {
                    let w = self.att_target.unwrap_or(NOISE);
                    self.post(s, 0.0, w, at, false, false);
                }
            }
            self.att_level -= 1;
        }
        while self.att_level < to {
            self.att_level += 1;
            if self.att_enemy && alive {
                let begin = match self.att_level {
                    2 => Some(Stim::NoticeBegin),
                    3 => Some(Stim::SearchBegin),
                    4 => Some(Stim::CombatBegin),
                    _ => None,
                };
                if let Some(s) = begin {
                    self.post(s, 0.0, who, at, false, urgent);
                }
            }
        }
    }

    /// Release the stimuli whose delay ran out, in queue order, and dispatch them.
    pub(crate) fn drain(&mut self, me: &PlayerState, ws: &WorldState, ctx: &mut Ctx, ai: &AiConfig, dt: f32) {
        self.ensure_stack();
        if self.stims.is_empty() {
            return;
        }
        let mut due = Vec::new();
        let mut keep = Vec::new();
        for mut q in std::mem::take(&mut self.stims) {
            if q.delay <= 0.0 {
                due.push(q);
            } else {
                q.delay -= dt;
                keep.push(q);
            }
        }
        self.stims = keep;
        for q in due {
            self.dispatch(&q, me, ws, ctx, ai);
        }
    }

    /// Dispatch one stimulus: the active behaviours that complete on it end; the behaviour
    /// classes whose evaluate mask contains it start.
    fn dispatch(&mut self, q: &QueuedStim, me: &PlayerState, ws: &WorldState, ctx: &mut Ctx, ai: &AiConfig) {
        let guard = self.kind == PawnKind::Guard;
        match q.stim {
            Stim::NoticeEnd => {
                if matches!(self.behavior(slot::NOTICE), Some(Behavior::Notice)) {
                    self.complete(slot::NOTICE);
                }
            }
            Stim::SearchEnd => {
                if matches!(self.behavior(slot::SEARCH), Some(Behavior::Search(_))) {
                    self.complete(slot::SEARCH);
                }
            }
            Stim::CombatEnd => {
                if matches!(self.behavior(slot::COMBAT), Some(Behavior::Combat)) {
                    self.complete(slot::COMBAT);
                }
            }
            Stim::NoticeBegin => {
                self.threat_pos = Some(q.at);
                self.place(Behavior::Notice);
            }
            Stim::SearchBegin => {
                self.suspecting = true;
                self.threat_pos = Some(q.at);
                self.run = q.urgent;
                let (why, danger) = self.attn.iter().find(|a| a.who == q.who).map(|a| (a.why, a.danger)).unwrap_or((0, false));
                self.cause = why;
                self.cause_danger = danger;
                let corpse = why == reason::FOUND_CORPSE;
                let [lo, hi] = ai.search.stationary_duration;
                let stationary = !guard;
                let timer = if stationary { lo + (hi - lo) * 0.5 } else { 0.0 };
                let cd = match self.behavior(slot::SEARCH) {
                    Some(Behavior::Search(s)) => s.anim_cooldown,
                    _ => -1.0,
                };
                self.crumbs = ai.search.crumbs;
                self.place(Behavior::Search(SearchState { sub: SearchSub::Investigate, target: q.who, proxy: q.at, dest: q.at, anim_cooldown: cd, stationary, corpse, timer, ..Default::default() }));
                if guard {
                    let kind = match why {
                        reason::FOUND_CORPSE => Some(AlarmCause::FoundCorpse),
                        reason::HEARD_DEATH | reason::SAW_DEATH => Some(AlarmCause::WitnessDeath),
                        reason::HEARD_HELP_REQUEST => Some(AlarmCause::Help),
                        _ => None,
                    };
                    if let Some(k) = kind.filter(|_| self.class == NpcClass::EliteGuard) {
                        self.consider_alarm(me, ws, ctx, ai, k, None);
                    }
                }
            }
            Stim::CombatBegin => {
                if guard {
                    self.suspecting = true;
                    self.target = Some(q.who);
                    self.threat_pos = Some(q.at);
                    self.lost = 0.0;
                    self.place(Behavior::Combat);
                    // `EAIStimID_EnemyBusted`: `DisBehaviorTriggerAlarm` may take the sighting (only
                    // in the guard brains: the tallboy, hound, weeper and Overseer lists have none).
                    if self.class == NpcClass::EliteGuard {
                        self.consider_alarm(me, ws, ctx, ai, AlarmCause::Sighting, Some(q.who));
                    }
                }
            }
            Stim::Panicked => {
                if !guard {
                    self.suspecting = true;
                    self.threat_pos = Some(q.at);
                    self.goal = None;
                    if !matches!(self.behavior(slot::ESCAPE), Some(Behavior::Panic { .. })) {
                        self.place(Behavior::Panic { cower: false });
                    }
                }
            }
            Stim::PatrolRequest => {
                // `DisBehaviorPatrol` takes the request when it can adopt a route (0x00c41590).
                let pos = me.position;
                let has = self.patrol.is_some() || self.adopt_route(ctx.routes, ctx.users, ctx.neglect, pos, None) || self.route.len() >= 2;
                if has {
                    self.stay = false;
                    if self.current_slot() == slot::IDLE {
                        self.state = BrainState::Patrol;
                        self.goal = None;
                        self.follow.clear();
                    } else {
                        self.routine = BrainState::Patrol;
                        self.routine_goal = None;
                    }
                }
            }
            Stim::WitnessMagic => {
                // `DisBehaviorMagicResponse` evaluates it when the stimulus names the magic user
                // (0x00ae3d70) and starts with its type, user and `m_bDisappeared` (0x00ae3d90).
                if q.who != NOISE && ws.player(q.who).is_some() {
                    // `m_bMagicUserIsAnAlly`: the relationship toward the user is Ally (0x00afa080).
                    let ally = self.allies.contains(&q.who);
                    self.place(Behavior::Magic(MagicState { caster: q.who, disappeared: q.flag, ally, barked: false, anim: None }));
                    self.threat_pos = Some(q.at);
                }
            }
        }
    }
}

/// What made a guard consider ringing an alarm (`DisTweaks_AIBehavior_TriggerAlarm` chances).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlarmCause {
    Sighting,
    FoundCorpse,
    WitnessDeath,
    Help,
}
