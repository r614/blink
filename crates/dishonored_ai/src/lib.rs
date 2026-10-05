//! NPC crowd and guard behaviour (Phase II S0, S8), built on Dishonored's own AI model and
//! data. See `research/dishonored/AI.md`.
//!
//! Brains observe the authoritative `WorldState` plus gameplay events and act **only**
//! through `PlayerInput` on ordinary pawns, so NPCs move exactly like players can move.
//! That is what makes behavioural blending possible. Everything is deterministic (seeded
//! per-brain RNG, no wall clock), so server, replays and tests agree.
//!
//! What follows the original:
//! - **Attention** (`DisAIBrainProcessAttention` / `DisTweaks_PawnAttention`): every NPC keeps
//!   an attention value per stimulus source that rises while the source is seen (two vision
//!   cones from `DisTweaks_Vision`, the distance curve `m_DistanceValueTuning`) or heard
//!   (`m_IncreaseFor*Sound`), leaks at `m_fAttentionLeakRate`, and crosses the
//!   `m_AttentionThresholds` levels HeadTrack -> TurnToFace -> Investigate -> Busted with
//!   start/stop hysteresis. Profiles: guard/civilian x unsuspecting/suspecting, plus the
//!   neutral profile used toward guests (players start neutral at the party and become
//!   enemies when seen committing violence).
//! - **Behaviours** mirror the brain's behaviour list: routine (Idle / Patrol / Guard /
//!   Soiree-like groups), Notice (TurnToFace), Investigate then Search with the search
//!   crumbs settings, Combat with the skirmish caps, civilian Panic (flee to the level's flee
//!   points, cower when the enemy is close) with help barks that guards hear. Reactions wait
//!   the global `m_ReactionDelay`.
//! - **Movement** follows the original navigation mesh (`WorldQuery::nav()`).
//!
//! What is inferred (LIKELY/SPECULATIVE in `research/UNKNOWNS.md`): the exact vision rate
//! formula (cone weighting and how the distance curve combines with it), light and stance
//! visibility, patrol route topology, attack patterns and alarm bells.

use dishonored_ir::ai::{AiConfig, AttentionConfig, AttentionIncrease, DAL_BUSTED, DAL_HEAD_TRACK, DAL_INVESTIGATE, DAL_TURN_TO_FACE};
use dishonored_sim::events::{SimEvent, SoundCategory};
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use dishonored_sim::state::{PawnKind, PlayerId, PlayerState, SwordPhase, WorldState};
use dishonored_world::nav::{Follower, NavWorld};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;
use serde::{Deserialize, Serialize};

pub const NPC_ID_BASE: PlayerId = 1000;
/// Stimulus without a known identity (noises, corpses, help barks): attention toward a place.
pub const NOISE: PlayerId = PlayerId::MAX;
/// Largest drop (uu) an NPC will step off deliberately when it walks off the navmesh.
const MAX_SAFE_DROP: f32 = 160.0;
/// Eye height above the pawn centre (the `camera_jnt` bone sits near the top of the
/// 87.5 uu half-height cylinder).
const EYE: f32 = 70.0;
/// Seconds a combat target may stay unseen before the guard falls back to searching
/// (SPECULATIVE: the original hands over to `DisBehaviorSearch` when combat loses track).
const COMBAT_LOST_TIME: f32 = 5.0;
/// A civilian panics when it sees another one flee within this distance (SPECULATIVE
/// range for `TweaksBehavior_Panic_Civilian.m_WitnessAllyPanic`).
const PANIC_CONTAGION_RANGE: f32 = 1000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrainState {
    /// Routine: standing (`DisBehaviorIdle`).
    Idle,
    /// Routine: walking to a random original AI point.
    Wander,
    /// Routine / scripted: walking to `goal` (`DisSeqAct_AIGoToActor`).
    WalkToPoint,
    /// Routine: standing with a group (soiree-like).
    GroupIdle,
    /// Reaction delay before an escalation (`m_ReactionDelay`) or surprise.
    React,
    /// Civilian panic, running to a flee point (`DisBehaviorPanic` / `DisAISubStateFlee`).
    Flee,
    /// Guard patrol (`DisBehaviorPatrol`).
    Patrol,
    /// Going to the attention proxy (`DisAISubStateInvestigate`); civilians search in place.
    Investigate,
    /// Guard standing at a post or patrol point (`DisBehaviorGuard`).
    Alert,
    /// Fighting a hostile (`DisBehaviorCombatEliteGuard`).
    Combat,
    /// TurnToFace: stop and look at the stimulus (`DisBehaviorNotice`).
    Notice,
    /// Wandering around the proxy with the search crumbs (`DisBehaviorSearch`).
    Search,
    /// Panicked civilian with the enemy too close to run (`DisAISubStateCower`).
    Cower,
}

impl BrainState {
    pub fn is_routine(self) -> bool {
        matches!(self, BrainState::Idle | BrainState::Wander | BrainState::WalkToPoint | BrainState::GroupIdle | BrainState::Patrol | BrainState::Alert)
    }
}

/// Attention toward one stimulus source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Attn {
    pub who: PlayerId,
    pub value: f32,
    pub level: u8,
    /// Where the NPC believes the stimulus is (UE3 "attention proxy").
    pub proxy: Vec3,
    /// Raised this tick (no leak).
    #[serde(default)]
    pub fresh: bool,
    /// The reason warrants running (`m_SearchFromNoiseTweaks[..].m_bShouldRun`).
    #[serde(default)]
    pub urgent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Brain {
    pub id: PlayerId,
    pub kind: PawnKind,
    pub state: BrainState,
    pub rng: u64,
    pub timer: f32,
    pub goal: Option<Vec3>,
    pub home: Vec3,
    pub route: Vec<Vec3>,
    pub route_i: usize,
    pub threat_pos: Option<Vec3>,
    pub target: Option<PlayerId>,
    /// Social spot index for GroupIdle.
    pub spot: Option<usize>,
    pub last_pos: Vec3,
    pub stuck: f32,
    /// Mission control: hold position when idle instead of wandering.
    #[serde(default)]
    pub stay: bool,
    /// Mission control: run (not walk) to `goal` in `WalkToPoint`.
    #[serde(default)]
    pub run: bool,
    /// Path following on the original navmesh.
    #[serde(default)]
    pub follow: Follower,
    #[serde(default)]
    pub attn: Vec<Attn>,
    /// Suspicion level (`EDisAISuspicionLevel`): set after an investigation or combat.
    #[serde(default)]
    pub suspecting: bool,
    /// Pawns this NPC treats as enemies (witnessed violence, attacked it, ally's enemy).
    #[serde(default)]
    pub hostile: Vec<PlayerId>,
    /// State to enter when the reaction delay (`React`) ends.
    #[serde(default)]
    pub next: Option<BrainState>,
    #[serde(default)]
    pub corpses_seen: Vec<PlayerId>,
    /// Seconds since the combat target was last seen.
    #[serde(default)]
    pub lost: f32,
    /// Remaining search crumbs.
    #[serde(default)]
    pub crumbs: u32,
    /// Pistol rounds (`m_Ammo` of the guard pawn tweaks: one bullet).
    #[serde(default)]
    pub ammo: u8,
    #[serde(default)]
    pub aim: f32,
    /// Combat role from the skirmish manager: melee attacker (true) or outer ring.
    #[serde(default)]
    pub melee_slot: bool,
    /// Proxy already searched (the stimulus is not investigated again while it fades).
    #[serde(default)]
    pub searched: Option<Vec3>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrowdConfig {
    pub civilians: usize,
    pub guards: usize,
    /// Population centre/radius (uu) used to select points and spawns.
    pub center: Vec3,
    pub radius: f32,
    pub seed: u64,
    /// Visual archetype pools.
    pub civilian_archetypes: Vec<u8>,
    pub guard_archetypes: Vec<u8>,
}

/// Static layout shared by all brains (original Dishonored AI points near the centre).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CrowdLayout {
    pub points: Vec<Vec3>,
    pub social_spots: Vec<Vec3>,
    pub guard_posts: Vec<(Vec3, f32)>,
    /// `DisFleePointActor`s (civilian panic destinations).
    #[serde(default)]
    pub flee_points: Vec<Vec3>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NpcSpawn {
    pub id: PlayerId,
    pub kind: PawnKind,
    pub archetype: u8,
    pub position: Vec3,
    pub yaw: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Crowd {
    pub brains: Vec<Brain>,
    pub layout: CrowdLayout,
    pub spawns: Vec<NpcSpawn>,
    /// Seconds until a dead NPC respawns at its spawn.
    pub respawn: Vec<(PlayerId, f32)>,
    /// AI tuning (set from the match's `SimConfig` when the match starts).
    #[serde(default)]
    pub ai: AiConfig,
    /// Help barks emitted last tick: (position, radius).
    #[serde(default)]
    pub barks: Vec<(Vec3, f32)>,
}

fn rng_next(s: &mut u64) -> f32 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    (*s >> 40) as f32 / (1u64 << 24) as f32
}

fn yaw_to(from: Vec3, to: Vec3) -> f32 {
    let d = to - from;
    d.y.atan2(d.x).to_degrees()
}

/// Piecewise-linear `m_DistanceValueTuning` points (clamped at both ends).
fn curve(points: &[[f32; 2]], x: f32) -> f32 {
    match points {
        [] => 0.0,
        [p] => p[1],
        _ => {
            if x <= points[0][0] {
                return points[0][1];
            }
            for w in points.windows(2) {
                if x <= w[1][0] {
                    let t = (x - w[0][0]) / (w[1][0] - w[0][0]).max(1e-6);
                    return w[0][1] + (w[1][1] - w[0][1]) * t;
                }
            }
            points[points.len() - 1][1]
        }
    }
}

/// What the brains see of each other this tick.
#[derive(Clone, Copy)]
struct Peer {
    id: PlayerId,
    kind: PawnKind,
    state: BrainState,
    target: Option<PlayerId>,
    threat: Option<Vec3>,
}

impl Crowd {
    /// Build a crowd from original placement data.
    /// `points`: (position, yaw) of nav/target points; `guard_spawns`: original guard
    /// spawners; `world` validates positions against collision.
    pub fn populate(cfg: &CrowdConfig, points: &[(Vec3, f32)], guard_spawns: &[(Vec3, f32)], world: &dyn WorldQuery) -> Crowd {
        let mut rng = cfg.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let on_nav = |p: Vec3| world.nav().is_none_or(|n| n.closest_point(p, 200.0).is_some());
        let near: Vec<(Vec3, f32)> = points.iter().copied().filter(|(p, _)| p.distance(cfg.center) <= cfg.radius).filter_map(|(p, y)| snap_floor(world, p).map(|q| (q, y))).filter(|(p, _)| on_nav(*p)).collect();
        let mut layout = CrowdLayout { points: near.iter().map(|p| p.0).collect(), ..Default::default() };
        // Social spots: every third point (deterministic) where groups gather.
        layout.social_spots = layout.points.iter().copied().step_by(3).collect();
        layout.guard_posts = guard_spawns.iter().copied().filter(|(p, _)| p.distance(cfg.center) <= cfg.radius * 1.5).filter_map(|(p, y)| snap_floor(world, p).map(|q| (q, y))).collect();
        let mut brains = Vec::new();
        let mut spawns = Vec::new();
        let mut id = NPC_ID_BASE;
        for i in 0..cfg.civilians {
            if layout.points.is_empty() {
                break;
            }
            let base = layout.points[(rng_next(&mut rng) * layout.points.len() as f32) as usize % layout.points.len()];
            let off = Vec3::new(rng_next(&mut rng) - 0.5, rng_next(&mut rng) - 0.5, 0.0) * 160.0;
            let pos = snap_floor(world, base + off).filter(|p| on_nav(*p)).unwrap_or(base);
            let yaw = rng_next(&mut rng) * 360.0;
            let arch = cfg.civilian_archetypes[i % cfg.civilian_archetypes.len().max(1)];
            spawns.push(NpcSpawn { id, kind: PawnKind::Civilian, archetype: arch, position: pos, yaw });
            brains.push(Brain::new(id, PawnKind::Civilian, pos, cfg.seed ^ (id as u64 * 7919)));
            id += 1;
        }
        for i in 0..cfg.guards {
            let (pos, yaw) = if !layout.guard_posts.is_empty() {
                layout.guard_posts[i % layout.guard_posts.len()]
            } else if !layout.points.is_empty() {
                (layout.points[i % layout.points.len()], 0.0)
            } else {
                break;
            };
            let arch = cfg.guard_archetypes[i % cfg.guard_archetypes.len().max(1)];
            spawns.push(NpcSpawn { id, kind: PawnKind::Guard, archetype: arch, position: pos, yaw });
            let mut b = Brain::new(id, PawnKind::Guard, pos, cfg.seed ^ (id as u64 * 104729));
            // Patrol route: the original nav points nearest the post, visited in a loop
            // (SPECULATIVE topology: the original routes start from a Kismet-assigned point).
            let mut pts: Vec<Vec3> = layout.points.clone();
            pts.sort_by(|a, c| a.distance(pos).total_cmp(&c.distance(pos)));
            b.route = pts.into_iter().take(4).collect();
            b.state = if b.route.len() >= 2 { BrainState::Patrol } else { BrainState::Alert };
            brains.push(b);
            id += 1;
        }
        Crowd { brains, layout, spawns, respawn: Vec::new(), ai: AiConfig::default(), barks: Vec::new() }
    }

    /// Flee destinations (`DisFleePointActor` positions).
    pub fn set_flee_points(&mut self, points: &[Vec3], world: &dyn WorldQuery) {
        self.layout.flee_points = points.iter().filter_map(|p| snap_floor(world, *p).or(Some(*p))).collect();
    }

    /// Produce this tick's inputs for every NPC. `events` are the previous tick's events.
    pub fn think(&mut self, world_state: &WorldState, events: &[SimEvent], world: &dyn WorldQuery, dt: f32) -> Vec<(PlayerId, PlayerInput)> {
        let peers: Vec<Peer> = self.brains.iter().map(|b| Peer { id: b.id, kind: b.kind, state: b.state, target: b.target, threat: b.threat_pos }).collect();
        self.assign_skirmish(world_state);
        let barks = std::mem::take(&mut self.barks);
        let mut new_barks = Vec::new();
        let layout = &self.layout;
        let ai = &self.ai;
        let mut out = Vec::with_capacity(self.brains.len());
        for b in self.brains.iter_mut() {
            let Some(me) = world_state.player(b.id) else { continue };
            if !me.alive {
                continue;
            }
            b.perceive(me, world_state, events, &peers, &barks, world, ai, dt);
            let was = b.state;
            out.push((b.id, b.act(me, world_state, layout, world, ai, dt)));
            if b.kind != PawnKind::Guard && matches!(b.state, BrainState::Flee | BrainState::Cower) && !matches!(was, BrainState::Flee | BrainState::Cower) {
                // `DisAISubStateFlee.bDoBarkForHelp`: call for help at talking loudness.
                new_barks.push((me.position, ai.civilian_talking_loudness));
            }
        }
        self.barks = new_barks;
        out
    }

    /// `DisGlobalCombatManager` skirmish caps: per target, the nearest `skirmish_inner`
    /// guards engage in melee; the next ones (up to `skirmish_total`) hold the outer ring.
    fn assign_skirmish(&mut self, ws: &WorldState) {
        let inner = self.ai.combat.skirmish_inner as usize;
        let mut targets: Vec<PlayerId> = self.brains.iter().filter(|b| b.state == BrainState::Combat).filter_map(|b| b.target).collect();
        targets.sort_unstable();
        targets.dedup();
        for t in targets {
            let Some(tp) = ws.player(t).map(|p| p.position) else { continue };
            let mut idx: Vec<(f32, usize)> = self
                .brains
                .iter()
                .enumerate()
                .filter(|(_, b)| b.state == BrainState::Combat && b.target == Some(t))
                .filter_map(|(i, b)| ws.player(b.id).map(|p| (p.position.distance(tp), i)))
                .collect();
            idx.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            for (k, (_, i)) in idx.into_iter().enumerate() {
                self.brains[i].melee_slot = k < inner;
            }
        }
    }

    /// Respawn bookkeeping: returns NPCs to respawn now.
    pub fn tick_respawns(&mut self, events: &[SimEvent], dt: f32) -> Vec<NpcSpawn> {
        for e in events {
            if let SimEvent::Death { victim, .. } = e {
                if self.brains.iter().any(|b| b.id == *victim) && !self.respawn.iter().any(|(i, _)| i == victim) {
                    self.respawn.push((*victim, 20.0));
                }
            }
        }
        let mut due = Vec::new();
        for (id, t) in self.respawn.iter_mut() {
            *t -= dt;
            if *t <= 0.0 {
                if let Some(s) = self.spawns.iter().find(|s| s.id == *id) {
                    due.push(s.clone());
                }
            }
        }
        self.respawn.retain(|(_, t)| *t > 0.0);
        for s in &due {
            if let Some(b) = self.brains.iter_mut().find(|b| b.id == s.id) {
                *b = Brain { route: b.route.clone(), ..Brain::new(s.id, s.kind, s.position, b.rng) };
                if b.kind == PawnKind::Guard {
                    b.state = if b.route.len() >= 2 { BrainState::Patrol } else { BrainState::Alert };
                }
            }
        }
        due
    }
}

pub fn snap_floor(world: &dyn WorldQuery, p: Vec3) -> Option<Vec3> {
    let h = world.raycast(p + Vec3::Z * 120.0, -Vec3::Z, 400.0, channel::PAWN)?;
    (h.normal.z > 0.7).then(|| h.point + Vec3::Z * 90.0)
}

/// Is the pawn doing something a guard treats as criminal?
fn criminal(p: &PlayerState) -> bool {
    matches!(p.sword.phase, SwordPhase::Startup | SwordPhase::Active | SwordPhase::Assassinating) || p.left.phase == dishonored_sim::state::LeftPhase::Recovering
}

/// AI noise context of a simulation sound (`EAINoiseContext`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Noise {
    Danger,
    Threatening,
    Anomaly,
}

fn noise_context(c: SoundCategory) -> Noise {
    match c {
        SoundCategory::Pistol | SoundCategory::Crossbow | SoundCategory::SwordImpact | SoundCategory::SwordBlock | SoundCategory::Windblast | SoundCategory::BodyFall => Noise::Danger,
        SoundCategory::SwordSwing => Noise::Threatening,
        SoundCategory::Footstep | SoundCategory::Sprint | SoundCategory::Land | SoundCategory::Blink | SoundCategory::Mantle => Noise::Anomaly,
    }
}

impl Brain {
    pub fn new(id: PlayerId, kind: PawnKind, home: Vec3, seed: u64) -> Self {
        Brain {
            id,
            kind,
            state: BrainState::Idle,
            rng: seed | 1,
            timer: 0.0,
            goal: None,
            home,
            route: Vec::new(),
            route_i: 0,
            threat_pos: None,
            target: None,
            spot: None,
            last_pos: home,
            stuck: 0.0,
            stay: false,
            run: false,
            follow: Follower::default(),
            attn: Vec::new(),
            suspecting: false,
            hostile: Vec::new(),
            next: None,
            corpses_seen: Vec::new(),
            lost: 0.0,
            crumbs: 0,
            ammo: if kind == PawnKind::Guard { 1 } else { 0 },
            aim: 0.0,
            melee_slot: true,
            searched: None,
        }
    }

    /// Mission control: treat `target` as an enemy at full attention.
    pub fn alert_to(&mut self, ai: &AiConfig, target: PlayerId, at: Vec3) {
        if !self.hostile.contains(&target) {
            self.hostile.push(target);
        }
        self.bump(ai, target, AttentionIncrease { amount: 0.0, to_level: 5 }, at, true);
        self.suspecting = true;
    }

    /// Mission control: forget every stimulus and enemy.
    pub fn calm(&mut self) {
        self.attn.clear();
        self.hostile.clear();
        self.target = None;
        self.threat_pos = None;
        self.next = None;
        self.follow.clear();
    }

    fn profile<'a>(&self, ai: &'a AiConfig, who: PlayerId) -> &'a AttentionConfig {
        if who != NOISE && !self.hostile.contains(&who) {
            return &ai.neutral;
        }
        match (self.kind == PawnKind::Guard, self.suspecting) {
            (true, false) => &ai.guard_unsuspecting,
            (true, true) => &ai.guard_suspecting,
            (false, false) => &ai.civilian_unsuspecting,
            (false, true) => &ai.civilian_suspecting,
        }
    }

    fn entry(&mut self, who: PlayerId, proxy: Vec3) -> &mut Attn {
        if let Some(i) = self.attn.iter().position(|a| a.who == who) {
            return &mut self.attn[i];
        }
        self.attn.push(Attn { who, value: 0.0, level: 0, proxy, fresh: false, urgent: false });
        self.attn.last_mut().unwrap()
    }

    /// Apply a `DisAttentionIncrease` toward `who`.
    fn bump(&mut self, ai: &AiConfig, who: PlayerId, inc: AttentionIncrease, proxy: Vec3, urgent: bool) {
        let prof = self.profile(ai, who);
        let cap = prof.level_value(inc.to_level).min(prof.max);
        let max = prof.max;
        let e = self.entry(who, proxy);
        let v = if inc.amount > 0.0 { e.value.max((e.value + inc.amount).min(cap)) } else { e.value.max(cap) };
        e.value = v.min(max);
        e.proxy = proxy;
        e.fresh = true;
        e.urgent |= urgent;
    }

    /// A civilian panic trigger (`DisTweaks_AIBehavior_Panic` rules): attention toward the
    /// place goes to Busted, which selects Panic.
    fn panic_from(&mut self, ai: &AiConfig, at: Vec3) {
        if !matches!(self.state, BrainState::Flee | BrainState::Cower) {
            self.threat_pos = Some(at);
        }
        self.bump(ai, NOISE, AttentionIncrease { amount: 0.0, to_level: DAL_BUSTED }, at, true);
    }

    /// Attention per second from seeing `other` (None when not visible). The cone test and
    /// the distance curve are data; how they combine is SPECULATIVE (see AI.md).
    fn sight_rate(me: &PlayerState, other: &PlayerState, world: &dyn WorldQuery, ai: &AiConfig, prof: &AttentionConfig, target_height: f32) -> Option<f32> {
        let v = &ai.vision;
        let eye = me.position + Vec3::Z * EYE;
        let to = other.position + Vec3::Z * target_height - eye;
        let d = to.length();
        if d < 1.0 || d > v.inner_range.max(v.outer_range) {
            return None;
        }
        let fwd = dishonored_sim::movement::forward_vector(me.yaw).truncate().normalize_or_zero();
        let flat = to.truncate();
        let h_ang = fwd.angle_to(flat.normalize_or_zero()).abs().to_degrees();
        let elev = to.z.atan2(flat.length()).to_degrees();
        let in_cone = |fov: f32, up: f32, down: f32, range: f32| {
            let half = fov * 0.5;
            let vlim = if elev >= 0.0 { half * up } else { half * down };
            d <= range && h_ang <= half && elev.abs() <= vlim.max(5.0)
        };
        let inner = in_cone(v.inner_fov_deg, v.inner_up, v.inner_down, v.inner_range);
        let outer = !inner && in_cone(v.outer_fov_deg, v.outer_up, v.outer_down, v.outer_range);
        if !inner && !outer {
            return None;
        }
        if world.raycast(eye, to, d, channel::VISION).is_some_and(|h| h.distance < d - 40.0) {
            return None;
        }
        let closeness = 1.0 - d / v.inner_range;
        let base = if inner { 1.0 - d / v.inner_range } else { 0.5 * (1.0 - d / v.outer_range) };
        let scalar = if other.kind == PawnKind::Human { prof.seeing_scalar_player } else { prof.seeing_scalar_npc.min(1.0) };
        Some(scalar * (base.max(0.0) + curve(&prof.distance_points, closeness)))
    }

    /// Cheap visibility test (cone + line of sight) for events and allies.
    fn sees(me: &PlayerState, other: &PlayerState, world: &dyn WorldQuery, ai: &AiConfig) -> bool {
        Self::sight_rate(me, other, world, ai, &ai.neutral, 40.0).is_some()
    }

    #[allow(clippy::too_many_arguments)]
    fn perceive(&mut self, me: &PlayerState, ws: &WorldState, events: &[SimEvent], peers: &[Peer], barks: &[(Vec3, f32)], world: &dyn WorldQuery, ai: &AiConfig, dt: f32) {
        for a in self.attn.iter_mut() {
            a.fresh = false;
        }
        // Sounds (gameplay sound events, never raw audio) -> attention toward the place.
        for e in events {
            if let SimEvent::Sound { position, radius, category, source } = e {
                if *source == self.id || position.distance(me.position) > *radius {
                    continue;
                }
                let prof = self.profile(ai, NOISE);
                let (inc, urgent) = match noise_context(*category) {
                    Noise::Danger => (prof.danger_sound, true),
                    Noise::Threatening => (prof.threatening_sound, false),
                    Noise::Anomaly => (prof.anomaly_sound, false),
                };
                self.bump(ai, NOISE, inc, *position, urgent);
            }
        }
        // Help barks from fleeing civilians (`EAINoiseContext_Help`). For civilians Help is
        // one of the party's scary noises (`TweaksBehavior_Panic_Civilian.m_HearNoise`:
        // AlarmBell, Help, DeathRattle), so it starts a panic.
        for (p, r) in barks {
            if p.distance(me.position) <= *r && p.distance(me.position) > 1.0 {
                let inc = self.profile(ai, NOISE).help_sound;
                self.bump(ai, NOISE, inc, *p, true);
                if self.kind != PawnKind::Guard {
                    self.panic_from(ai, *p);
                }
            }
        }
        for e in events {
            match e {
                SimEvent::Damage { victim, attacker: Some(a), .. } if *victim == self.id && *a != self.id => {
                    let at = ws.player(*a).map(|p| p.position).unwrap_or(me.position);
                    if !self.hostile.contains(a) {
                        self.hostile.push(*a);
                    }
                    let inc = self.profile(ai, *a).attacked_by;
                    self.bump(ai, *a, inc, at, true);
                }
                SimEvent::Death { victim, killer: Some(k), .. } if *k != self.id => {
                    let (Some(v), Some(kp)) = (ws.player(*victim), ws.player(*k)) else { continue };
                    if Self::sees(me, v, world, ai) || Self::sees(me, kp, world, ai) {
                        // Witnessed murder (`DACRT_SawDeath`): the killer becomes an enemy.
                        if !self.hostile.contains(k) {
                            self.hostile.push(*k);
                        }
                        let inc = self.profile(ai, *k).attacked_enemy;
                        self.bump(ai, *k, inc, kp.position, true);
                        self.threat_pos = Some(kp.position);
                        if !self.corpses_seen.contains(victim) {
                            self.corpses_seen.push(*victim);
                        }
                    } else if v.position.distance(me.position) <= ai.death_rattle_loudness {
                        let inc = self.profile(ai, NOISE).death_rattle;
                        self.bump(ai, NOISE, inc, v.position, true);
                        if self.kind != PawnKind::Guard {
                            self.panic_from(ai, v.position);
                        }
                    }
                }
                _ => {}
            }
        }
        // Vision: humans (neutral or enemy), corpses, allies' alarms.
        for p in ws.players.iter() {
            if p.id == self.id {
                continue;
            }
            if !p.alive {
                if p.kind != PawnKind::Human && !self.corpses_seen.contains(&p.id) && Self::sight_rate(me, p, world, ai, &ai.neutral, 0.0).is_some() {
                    self.corpses_seen.push(p.id);
                    let inc = self.profile(ai, NOISE).discovered_corpse;
                    self.bump(ai, NOISE, inc, p.position, false);
                    self.suspecting = true;
                    if self.kind != PawnKind::Guard {
                        // `m_WitnessCorpse`.
                        self.panic_from(ai, p.position);
                    }
                }
                continue;
            }
            if p.kind == PawnKind::Human {
                let prof = self.profile(ai, p.id).clone();
                let Some(rate) = Self::sight_rate(me, p, world, ai, &prof, 40.0) else { continue };
                if criminal(p) && !self.hostile.contains(&p.id) {
                    // Seen attacking: hostile at once (`m_Response_*`, `DACRT_AttackedEnemy`).
                    self.hostile.push(p.id);
                    let inc = self.profile(ai, p.id).attacked_enemy;
                    self.bump(ai, p.id, inc, p.position, true);
                    self.threat_pos = Some(p.position);
                    continue;
                }
                let touching = p.position.distance(me.position) < 80.0;
                let gain = rate + if touching { prof.touching_rate } else { 0.0 };
                let max = prof.max;
                let e = self.entry(p.id, p.position);
                e.value = (e.value + gain * dt).min(max);
                e.proxy = p.position;
                e.fresh = true;
            }
        }
        // Allies: guards see another guard fighting or a civilian panicking; civilians see
        // another civilian flee.
        for peer in peers {
            if peer.id == self.id {
                continue;
            }
            let Some(pp) = ws.player(peer.id).filter(|p| p.alive) else { continue };
            match (self.kind, peer.kind, peer.state) {
                (PawnKind::Guard, PawnKind::Guard, BrainState::Combat) => {
                    let Some(t) = peer.target else { continue };
                    if self.state == BrainState::Combat || !Self::sees(me, pp, world, ai) {
                        continue;
                    }
                    let tp = ws.player(t).map(|p| p.position).unwrap_or(pp.position);
                    if !self.hostile.contains(&t) {
                        self.hostile.push(t);
                    }
                    let inc = self.profile(ai, t).ally_combat_target;
                    self.bump(ai, t, inc, tp, true);
                }
                (PawnKind::Guard, PawnKind::Civilian, BrainState::Flee | BrainState::Cower) => {
                    if Self::sees(me, pp, world, ai) {
                        let inc = self.profile(ai, NOISE).ally_panic_target;
                        self.bump(ai, NOISE, inc, peer.threat.unwrap_or(pp.position), false);
                    }
                }
                (PawnKind::Civilian, PawnKind::Civilian, BrainState::Flee) => {
                    // `m_WitnessAllyPanic`.
                    if !matches!(self.state, BrainState::Flee | BrainState::Cower) && pp.position.distance(me.position) < PANIC_CONTAGION_RANGE && Self::sees(me, pp, world, ai) {
                        self.panic_from(ai, peer.threat.unwrap_or(pp.position));
                    }
                }
                (PawnKind::Civilian, PawnKind::Guard, BrainState::Combat) => {
                    // `m_WitnessAllyCombat`.
                    if !matches!(self.state, BrainState::Flee | BrainState::Cower) && Self::sees(me, pp, world, ai) {
                        let at = peer.target.and_then(|t| ws.player(t)).map(|p| p.position).unwrap_or(pp.position);
                        self.panic_from(ai, at);
                    }
                }
                _ => {}
            }
        }
        // Leak and levels (start/stop hysteresis).
        let profiles: Vec<AttentionConfig> = self.attn.iter().map(|a| self.profile(ai, a.who).clone()).collect();
        for (a, prof) in self.attn.iter_mut().zip(profiles.iter()) {
            if !a.fresh {
                a.value -= prof.leak_rate * dt;
            }
            let mut lvl = 0u8;
            for (i, s) in prof.start.iter().enumerate() {
                if a.value >= *s {
                    lvl = i as u8 + 1;
                }
            }
            if lvl < a.level && a.level > 0 && a.value >= prof.stop[a.level as usize - 1] {
                lvl = a.level;
            }
            a.level = lvl;
        }
        self.attn.retain(|a| a.value > 0.0 || a.fresh);
        for &id in &self.hostile.clone() {
            if ws.player(id).is_none() {
                self.hostile.retain(|h| *h != id);
            }
        }
    }

    /// Strongest stimulus: (who, level, proxy, urgent). Neutral pawns only ever head-track.
    fn focus(&self) -> Option<(PlayerId, u8, Vec3, bool)> {
        self.attn
            .iter()
            .map(|a| {
                let lvl = if a.who != NOISE && !self.hostile.contains(&a.who) { a.level.min(DAL_HEAD_TRACK) } else { a.level };
                (a.who, lvl, a.proxy, a.urgent, a.value)
            })
            .max_by(|a, b| a.1.cmp(&b.1).then(a.4.total_cmp(&b.4)).then(b.0.cmp(&a.0)))
            .map(|(w, l, p, u, _)| (w, l, p, u))
    }

    fn move_to(&mut self, me: &PlayerState, nav: Option<&NavWorld>, goal: Vec3, run: bool) -> PlayerInput {
        let steer = self.follow.steer(nav, me.position, goal);
        let mut i = PlayerInput { yaw: yaw_to(me.position, steer), forward: 127, left_hand: LeftHand::Empty, ..Default::default() };
        i.buttons |= if run { buttons::SPRINT } else { buttons::WALK };
        i
    }

    fn reached(me: &PlayerState, goal: Vec3, r: f32) -> bool {
        goal.truncate().distance(me.position.truncate()) < r
    }

    /// Enter `to` after the reaction delay.
    fn escalate(&mut self, ai: &AiConfig, to: BrainState) {
        if self.state == to || self.next == Some(to) {
            return;
        }
        let [lo, hi] = ai.reaction_delay;
        self.next = Some(to);
        self.state = BrainState::React;
        self.timer = -(lo + (hi - lo) * rng_next(&mut self.rng));
        self.follow.clear();
    }

    fn back_to_routine(&mut self) {
        if matches!(self.state, BrainState::Investigate | BrainState::Search) {
            self.searched = self.threat_pos;
        }
        self.next = None;
        self.target = None;
        self.timer = 0.0;
        self.goal = None;
        self.follow.clear();
        self.state = if self.kind == PawnKind::Guard {
            if self.route.len() >= 2 { BrainState::Patrol } else { BrainState::Alert }
        } else {
            BrainState::Idle
        };
    }

    /// Behaviour step plus a ledge guard for off-mesh movement.
    #[allow(clippy::too_many_arguments)]
    fn act(&mut self, me: &PlayerState, ws: &WorldState, layout: &CrowdLayout, world: &dyn WorldQuery, ai: &AiConfig, dt: f32) -> PlayerInput {
        self.select(ws, ai);
        let mut i = self.act_inner(me, ws, layout, world, ai, dt);
        let off_mesh = world.nav().is_none() || self.follow.failed;
        if off_mesh && i.forward > 0 && me.mode == dishonored_sim::state::MoveMode::Walking {
            let dir = Vec3::new(i.yaw.to_radians().cos(), i.yaw.to_radians().sin(), 0.0);
            let feet = me.position.z - 87.5;
            let probe = me.position + dir * 70.0;
            let ground = world.raycast(Vec3::new(probe.x, probe.y, feet + 60.0), -Vec3::Z, 60.0 + MAX_SAFE_DROP, channel::PAWN);
            if ground.is_none() {
                i.forward = 0;
                i.buttons &= !(buttons::SPRINT | buttons::WALK);
                self.goal = None;
                if matches!(self.state, BrainState::Wander | BrainState::WalkToPoint) {
                    self.state = if self.spot.is_some() { BrainState::GroupIdle } else { BrainState::Idle };
                    self.timer = 0.0;
                } else if self.state == BrainState::Patrol && !self.route.is_empty() {
                    self.route_i = (self.route_i + 1) % self.route.len();
                }
            }
        }
        i
    }

    /// Choose the behaviour from the attention state (the brain's behaviour priorities:
    /// Combat / Panic > Search / Investigate > Notice > routine).
    fn select(&mut self, ws: &WorldState, ai: &AiConfig) {
        let Some((who, level, proxy, urgent)) = self.focus() else {
            if matches!(self.state, BrainState::Notice) {
                self.back_to_routine();
            }
            return;
        };
        let guard = self.kind == PawnKind::Guard;
        let hostile_alive = who != NOISE && self.hostile.contains(&who) && ws.player(who).is_some_and(|p| p.alive);
        match level {
            DAL_BUSTED if guard && hostile_alive => {
                self.suspecting = true;
                if self.state != BrainState::Combat {
                    self.target = Some(who);
                    self.threat_pos = Some(proxy);
                    if self.next != Some(BrainState::Combat) {
                        self.lost = 0.0;
                        self.escalate(ai, BrainState::Combat);
                    }
                } else if self.target != Some(who) {
                    self.target = Some(who);
                }
            }
            DAL_BUSTED | DAL_INVESTIGATE if !guard && (level == DAL_BUSTED || hostile_alive) => {
                self.suspecting = true;
                if !matches!(self.state, BrainState::Flee | BrainState::Cower) {
                    self.threat_pos = Some(if hostile_alive { ws.player(who).map(|p| p.position).unwrap_or(proxy) } else { self.threat_pos.unwrap_or(proxy) });
                    self.goal = None;
                    self.escalate(ai, BrainState::Flee);
                } else if hostile_alive {
                    self.threat_pos = ws.player(who).map(|p| p.position);
                }
            }
            DAL_BUSTED | DAL_INVESTIGATE => {
                self.suspecting = true;
                let moved = self.threat_pos.is_none_or(|t| t.distance(proxy) > ai.search.proximity);
                match self.state {
                    BrainState::Combat | BrainState::Flee | BrainState::Cower => {}
                    BrainState::Investigate | BrainState::Search if !moved => {}
                    _ if self.searched.is_some_and(|s| s.distance(proxy) <= ai.search.proximity) => {}
                    _ => {
                        self.threat_pos = Some(proxy);
                        self.run = urgent;
                        self.escalate(ai, BrainState::Investigate);
                    }
                }
            }
            DAL_TURN_TO_FACE => {
                if self.state.is_routine() {
                    self.threat_pos = Some(proxy);
                    self.escalate(ai, BrainState::Notice);
                }
            }
            _ => {
                if self.state == BrainState::Notice {
                    self.back_to_routine();
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn act_inner(&mut self, me: &PlayerState, ws: &WorldState, layout: &CrowdLayout, world: &dyn WorldQuery, ai: &AiConfig, dt: f32) -> PlayerInput {
        self.timer += dt;
        let nav = world.nav();
        // Stuck detection.
        if me.position.distance(self.last_pos) < 0.5 && (self.goal.is_some() || !self.follow.path.is_empty()) {
            self.stuck += dt;
        } else {
            self.stuck = 0.0;
        }
        self.last_pos = me.position;
        if self.stuck > 1.5 {
            self.stuck = 0.0;
            self.follow.force_replan();
            if matches!(self.state, BrainState::WalkToPoint | BrainState::Wander) && !self.stay {
                self.goal = None;
                self.state = BrainState::Idle;
                self.timer = 0.0;
            }
        }
        let idle = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        let face = |p: Vec3| PlayerInput { yaw: yaw_to(me.position, p), left_hand: LeftHand::Empty, ..Default::default() };
        // Head tracking toward the strongest stimulus while standing.
        let look = self.attn.iter().filter(|a| a.level >= DAL_HEAD_TRACK).max_by(|a, b| a.value.total_cmp(&b.value)).map(|a| a.proxy);
        match self.state {
            BrainState::React => {
                if self.timer >= 0.0 {
                    let to = self.next.take().unwrap_or(BrainState::Idle);
                    self.state = to;
                    self.timer = 0.0;
                    if to == BrainState::Search {
                        self.crumbs = ai.search.crumbs;
                    }
                }
                self.threat_pos.map(face).unwrap_or(idle)
            }
            BrainState::Idle => {
                if self.stay {
                    return look.map(face).unwrap_or(idle);
                }
                if self.timer > 3.0 + rng_next(&mut self.rng) * 6.0 {
                    self.timer = 0.0;
                    let r = rng_next(&mut self.rng);
                    if self.kind == PawnKind::Guard && self.route.len() >= 2 {
                        self.state = BrainState::Patrol;
                    } else if r < 0.45 && !layout.social_spots.is_empty() {
                        let s = (rng_next(&mut self.rng) * layout.social_spots.len() as f32) as usize % layout.social_spots.len();
                        self.spot = Some(s);
                        let ang = rng_next(&mut self.rng) * std::f32::consts::TAU;
                        self.goal = Some(layout.social_spots[s] + Vec3::new(ang.cos(), ang.sin(), 0.0) * 90.0);
                        self.state = BrainState::WalkToPoint;
                    } else if !layout.points.is_empty() {
                        let k = (rng_next(&mut self.rng) * layout.points.len() as f32) as usize % layout.points.len();
                        self.goal = Some(layout.points[k]);
                        self.state = BrainState::Wander;
                    }
                }
                look.map(face).unwrap_or(idle)
            }
            BrainState::Wander | BrainState::WalkToPoint => match self.goal {
                Some(g) if !Self::reached(me, g, 60.0) && !(self.follow.failed && !self.stay) => {
                    let run = self.run;
                    self.move_to(me, nav, g, run)
                }
                _ => {
                    self.goal = None;
                    self.timer = 0.0;
                    self.follow.clear();
                    self.state = if self.spot.is_some() { BrainState::GroupIdle } else { BrainState::Idle };
                    idle
                }
            },
            BrainState::GroupIdle => {
                // Face the group centre and chat; leave after a while.
                let yaw = self.spot.map(|s| yaw_to(me.position, layout.social_spots[s])).unwrap_or(me.yaw);
                if self.timer > 10.0 + rng_next(&mut self.rng) * 15.0 {
                    self.spot = None;
                    self.state = BrainState::Idle;
                    self.timer = 0.0;
                }
                PlayerInput { yaw: look.map(|p| yaw_to(me.position, p)).unwrap_or(yaw), left_hand: LeftHand::Empty, ..Default::default() }
            }
            BrainState::Patrol => {
                if self.route.is_empty() {
                    self.state = BrainState::Alert;
                    return idle;
                }
                let g = self.route[self.route_i % self.route.len()];
                self.goal = Some(g);
                if Self::reached(me, g, 70.0) || self.follow.failed {
                    // Next patrol point; original points without `m_fGuardDuration` (the
                    // default 0) are passed without stopping.
                    self.route_i = (self.route_i + 1) % self.route.len();
                    self.follow.clear();
                }
                self.move_to(me, nav, g, false)
            }
            BrainState::Alert => {
                // Guard post (`DisBehaviorGuard`): stand, look at stimuli.
                if !Self::reached(me, self.home, 120.0) && !self.follow.failed {
                    let home = self.home;
                    return self.move_to(me, nav, home, false);
                }
                look.map(face).unwrap_or(idle)
            }
            BrainState::Notice => self.threat_pos.or(look).map(face).unwrap_or(idle),
            BrainState::Investigate => {
                let Some(t) = self.threat_pos else {
                    self.back_to_routine();
                    return idle;
                };
                if self.kind != PawnKind::Guard {
                    // Civilians: `DisBehaviorStationarySearch` (look toward it for 2-3 s).
                    let [lo, hi] = ai.search.stationary_duration;
                    if self.timer > lo + (hi - lo) * 0.5 {
                        self.back_to_routine();
                    }
                    return face(t);
                }
                if Self::reached(me, t, ai.search.proximity) || self.follow.failed {
                    // Reached the proxy (or it is unreachable): wait, then search.
                    if self.next.is_none() {
                        let [lo, hi] = ai.search.cancel_investigate_wait;
                        self.next = Some(BrainState::Search);
                        self.state = BrainState::React;
                        self.timer = -(lo + (hi - lo) * rng_next(&mut self.rng));
                        self.follow.clear();
                    }
                    return face(t);
                }
                let run = self.run;
                self.move_to(me, nav, t, run)
            }
            BrainState::Search => {
                // Done when attention toward the stimulus fell below the Investigate stop.
                if self.focus().is_none_or(|f| f.1 < DAL_INVESTIGATE) || self.crumbs == 0 {
                    self.back_to_routine();
                    return idle;
                }
                let center = self.threat_pos.unwrap_or(me.position);
                match self.goal {
                    Some(g) if !Self::reached(me, g, 80.0) && !self.follow.failed => self.move_to(me, nav, g, false),
                    _ => {
                        if self.timer < ai.search.heartbeat {
                            return idle;
                        }
                        // Next crumb: a wander step within the angular range of the facing.
                        self.timer = 0.0;
                        self.crumbs -= 1;
                        let s = &ai.search;
                        let step = s.wander_min + (s.wander_max - s.wander_min) * rng_next(&mut self.rng);
                        let back = (center - me.position).truncate();
                        let base_yaw = if back.length() > s.crumb_spacing { back.y.atan2(back.x).to_degrees() } else { me.yaw };
                        let yaw = (base_yaw + (rng_next(&mut self.rng) - 0.5) * s.wander_angle_deg * 2.0).to_radians();
                        let p = me.position + Vec3::new(yaw.cos(), yaw.sin(), 0.0) * step;
                        self.goal = match nav {
                            Some(n) => n.closest_point(p, 400.0).map(|(_, q)| q + Vec3::Z * 90.0),
                            None => Some(p),
                        };
                        self.follow.clear();
                        idle
                    }
                }
            }
            BrainState::Flee => {
                let from = self.threat_pos.unwrap_or(me.position);
                let enemy_close = self.hostile.iter().filter_map(|h| ws.player(*h)).any(|p| p.alive && p.position.distance(me.position) < ai.panic.cower_distance);
                if enemy_close {
                    self.state = BrainState::Cower;
                    self.timer = 0.0;
                    self.follow.clear();
                    return face(from);
                }
                if self.focus().is_none_or(|f| f.1 < DAL_INVESTIGATE) {
                    self.threat_pos = None;
                    self.back_to_routine();
                    return idle;
                }
                // Free flight straight away from the threat first, then a flee point.
                if self.timer < ai.panic.free_flight_time || self.goal.is_none() {
                    if self.goal.is_none() {
                        self.goal = self.pick_flee_point(me, from, layout, nav, ai);
                    }
                    if self.timer < ai.panic.free_flight_time {
                        let away = (me.position - from).truncate().normalize_or_zero();
                        let away = if away == glam::Vec2::ZERO { glam::Vec2::X } else { away };
                        let mut i = PlayerInput { yaw: away.y.atan2(away.x).to_degrees(), forward: 127, left_hand: LeftHand::Empty, ..Default::default() };
                        i.buttons |= buttons::SPRINT;
                        return i;
                    }
                }
                match self.goal {
                    Some(g) if !Self::reached(me, g, 120.0) && !self.follow.failed => self.move_to(me, nav, g, true),
                    _ => face(from),
                }
            }
            BrainState::Cower => {
                let from = self.threat_pos.unwrap_or(me.position);
                let enemy_close = self.hostile.iter().filter_map(|h| ws.player(*h)).any(|p| p.alive && p.position.distance(me.position) < ai.panic.cower_distance * 1.5);
                if !enemy_close && self.timer > 1.0 {
                    self.state = BrainState::Flee;
                    self.timer = ai.panic.free_flight_time;
                    self.goal = None;
                }
                let mut i = face(from);
                i.buttons |= buttons::CROUCH;
                i
            }
            BrainState::Combat => self.combat(me, ws, world, nav, ai, dt),
        }
    }

    /// `DisAISubStateFlee`: a flee point outside the forbidden cone toward the threat,
    /// nearest first; otherwise a point on the navmesh away from the threat.
    fn pick_flee_point(&mut self, me: &PlayerState, threat: Vec3, layout: &CrowdLayout, nav: Option<&NavWorld>, ai: &AiConfig) -> Option<Vec3> {
        let to_threat = (threat - me.position).truncate();
        let mut best: Option<(f32, Vec3)> = None;
        for fp in &layout.flee_points {
            let dir = (*fp - me.position).truncate();
            if dir.length() < 200.0 {
                continue;
            }
            let forbidden = to_threat.length() < ai.panic.forbidden_radius && dir.angle_to(to_threat).abs().to_degrees() < ai.panic.forbidden_angle_deg;
            if forbidden || fp.distance(threat) < me.position.distance(threat) {
                continue;
            }
            if let Some(n) = nav {
                if n.find_path(me.position, *fp, &Default::default()).is_none() {
                    continue;
                }
            }
            let d = dir.length();
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, *fp));
            }
        }
        if let Some((_, p)) = best {
            return Some(p);
        }
        let away = (me.position - threat).truncate().normalize_or_zero();
        let away = if away == glam::Vec2::ZERO { glam::Vec2::new(1.0, 0.0) } else { away };
        let p = me.position + Vec3::new(away.x, away.y, 0.0) * 1500.0;
        match nav {
            Some(n) => n.closest_point(p, 800.0).map(|(_, q)| q + Vec3::Z * 90.0),
            None => Some(p),
        }
    }

    fn combat(&mut self, me: &PlayerState, ws: &WorldState, world: &dyn WorldQuery, nav: Option<&NavWorld>, ai: &AiConfig, dt: f32) -> PlayerInput {
        let idle = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        let Some(t) = self.target.and_then(|id| ws.player(id)).filter(|p| p.alive).cloned() else {
            // Target gone: search where it was last seen.
            self.target = None;
            self.go_search(ai);
            return idle;
        };
        let visible = Self::sees(me, &t, world, ai) || t.position.distance(me.position) < ai.guard_unsuspecting.distance_to_lose_sight;
        if visible {
            self.lost = 0.0;
            self.threat_pos = Some(t.position);
            let e = self.entry(t.id, t.position);
            e.fresh = true;
        } else {
            self.lost += dt;
            if self.lost > COMBAT_LOST_TIME {
                self.go_search(ai);
                return idle;
            }
        }
        let d = t.position.distance(me.position);
        let reach = ai.combat.reachable_melee_range;
        let mut i = PlayerInput { yaw: yaw_to(me.position, t.position), left_hand: LeftHand::Empty, ..Default::default() };
        if !self.melee_slot {
            // Outer skirmish ring: keep distance; the guard's single bullet is used here
            // (`TwkAttackPattern_RangedElite`, aim time from `StateNPCMasterAim`).
            if self.ammo > 0 && visible && d > 300.0 && d < ai.vision.outer_range {
                i.left_hand = LeftHand::Pistol;
                let aim = t.position + Vec3::Z * 40.0 - (me.position + Vec3::Z * EYE);
                i.pitch = (aim.z / aim.truncate().length().max(1.0)).atan().to_degrees();
                self.aim += dt;
                let ready = me.left.item == LeftHand::Pistol && me.left.phase == dishonored_sim::state::LeftPhase::Idle && me.left.pistol_loaded > 0;
                if self.aim >= ai.combat.aim_time_before_shooting && ready {
                    i.buttons |= buttons::SECONDARY;
                    self.ammo -= 1;
                    self.aim = 0.0;
                }
                return i;
            }
            self.aim = 0.0;
            if d > 500.0 {
                let tp = t.position;
                return self.move_to(me, nav, tp, true);
            }
            if d < 300.0 {
                i.forward = -80;
            }
            return i;
        }
        if d > reach {
            let tp = t.position;
            let mut m = self.move_to(me, nav, tp, true);
            if d < 400.0 {
                m.yaw = yaw_to(me.position, tp);
            }
            return m;
        }
        if ((self.timer * 60.0) as u32) % 45 == 0 {
            i.buttons |= buttons::PRIMARY;
        } else if t.sword.phase == SwordPhase::Startup {
            i.buttons |= buttons::BLOCK;
        }
        i
    }

    fn go_search(&mut self, ai: &AiConfig) {
        self.suspecting = true;
        if let Some(t) = self.target.take() {
            if let Some(a) = self.attn.iter_mut().find(|a| a.who == t) {
                a.value = a.value.min(ai.guard_suspecting.start[2] + 0.05);
            }
        }
        self.state = BrainState::Search;
        self.crumbs = ai.search.crumbs;
        self.goal = None;
        self.timer = 0.0;
        self.follow.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_sim::state::PlayerState;
    use dishonored_world::CollisionWorld;

    fn flat_world() -> CollisionWorld {
        use dishonored_world::Tri;
        let s = 20000.0;
        let a = Vec3::new(-s, -s, 0.0);
        let b = Vec3::new(s, -s, 0.0);
        let c = Vec3::new(s, s, 0.0);
        let d = Vec3::new(-s, s, 0.0);
        let tri = |x: Vec3, y: Vec3, z: Vec3| Tri { a: x, b: y, c: z, n: Vec3::Z, channels: channel::ALL, source: 0 };
        CollisionWorld::from_triangles(vec![tri(a, b, c), tri(a, c, d)], vec!["floor".into()])
    }

    fn pawn(id: PlayerId, kind: PawnKind, pos: Vec3, yaw: f32) -> PlayerState {
        let mut p = PlayerState::new(id, pos, yaw);
        p.kind = kind;
        p
    }

    #[test]
    fn curve_interpolates() {
        let pts = [[0.55, 0.0], [0.875, 0.8], [0.93, 2.0]];
        assert_eq!(curve(&pts, 0.2), 0.0);
        assert!((curve(&pts, 0.875) - 0.8).abs() < 1e-5);
        assert!((curve(&pts, 1.0) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn enemy_in_view_escalates_to_busted_neutral_only_head_tracks() {
        let ai = AiConfig::default();
        let w = flat_world();
        let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let player = pawn(1, PawnKind::Human, Vec3::new(600.0, 0.0, 90.0), 180.0);
        let mut ws = WorldState::default();
        ws.players = vec![guard.clone(), player.clone()];
        let mut b = Brain::new(guard.id, PawnKind::Guard, guard.position, 1);
        // Neutral (a guest): attention rises but never above head tracking.
        for _ in 0..600 {
            b.perceive(&guard, &ws, &[], &[], &[], &w, &ai, 1.0 / 60.0);
        }
        assert!(b.focus().is_some_and(|f| f.1 <= DAL_HEAD_TRACK));
        // Enemy: Busted within a couple of seconds at 6 m in the inner cone.
        b.hostile.push(1);
        let mut t = 0;
        while b.focus().is_none_or(|f| f.1 < DAL_BUSTED) && t < 600 {
            b.perceive(&guard, &ws, &[], &[], &[], &w, &ai, 1.0 / 60.0);
            t += 1;
        }
        assert!(t < 180, "busted after {t} ticks");
        b.select(&ws, &ai);
        assert!(b.state == BrainState::React && b.next == Some(BrainState::Combat));
    }

    #[test]
    fn danger_sound_raises_to_turn_to_face_then_investigate() {
        let ai = AiConfig::default();
        let w = flat_world();
        let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let mut ws = WorldState::default();
        ws.players = vec![guard.clone()];
        let mut b = Brain::new(guard.id, PawnKind::Guard, guard.position, 1);
        let shot = SimEvent::Sound { position: Vec3::new(-500.0, 0.0, 90.0), radius: 1500.0, category: SoundCategory::Pistol, source: 7 };
        b.perceive(&guard, &ws, std::slice::from_ref(&shot), &[], &[], &w, &ai, 1.0 / 60.0);
        assert_eq!(b.focus().map(|f| f.1), Some(DAL_TURN_TO_FACE));
        b.perceive(&guard, &ws, std::slice::from_ref(&shot), &[], &[], &w, &ai, 1.0 / 60.0);
        assert_eq!(b.focus().map(|f| f.1), Some(DAL_INVESTIGATE));
        // Leak: 0.025/s; the Investigate level holds until the 0.3 stop threshold.
        for _ in 0..(60 * 10) {
            b.perceive(&guard, &ws, &[], &[], &[], &w, &ai, 1.0 / 60.0);
        }
        assert_eq!(b.focus().map(|f| f.1), Some(DAL_INVESTIGATE));
    }
}
