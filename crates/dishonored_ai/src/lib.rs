//! NPC crowd and guard behaviour (Phase II S0, S8), built on Dishonored's own AI model and
//! data. See `research/dishonored/AI.md`.
//!
//! Brains observe the authoritative `WorldState` plus gameplay events and act **only**
//! through `PlayerInput` on ordinary pawns, so NPCs move exactly like players can move.
//! That is what makes behavioural blending possible. Everything is deterministic (seeded
//! per-brain RNG, no wall clock), so server, replays and tests agree.
//!
//! What follows the original (decompiled from the executable, see AI.md for addresses):
//! - **Attention** (`DisAIBrainProcessAttention` / `DisAttentionInfo_Complex` /
//!   `DisTweaks_PawnAttention`): every NPC keeps an attention value per stimulus source. Seen
//!   targets add `scalar * clamp(tuning(1 - d/R) + visibility)` per second, where `d/R` is the
//!   distance over the range of the first `DisTweaks_Vision` cone (inner, then outer) that
//!   contains the target; increases add their amount then floor the value at their
//!   threshold; the value leaks only on ticks without any increase; levels rise on the
//!   `m_AttentionThresholds` starts and fall on the stops (rolled once per brain with
//!   `m_fStoppingVariance`); only reasons with `m_bCanIncreaseToBusted` reach Busted.
//!   Profiles: guard/civilian x unsuspecting/suspecting, plus the neutral profile used
//!   toward guests (players start neutral at the party and become enemies when seen
//!   committing violence).
//! - **Behaviours** mirror the brain's behaviour list: routine (Idle / Patrol / Guard /
//!   Soiree-like groups), Notice (TurnToFace), Investigate then Search with the search
//!   crumbs settings, Combat with the skirmish caps, civilian Panic (flee to the level's flee
//!   points, cower when the enemy is close) with help barks that guards hear. Reactions wait
//!   the global `m_ReactionDelay`.
//! - **Movement** follows the original navigation mesh (`WorldQuery::nav()`).
//!
//!   Combat lasts while attention toward the target stays Busted: unseen, it leaks, and an
//!   NPC that reaches a stale proxy drops it to Investigate (then Search). Melee attacks
//!   follow the elite attack pattern's context cooldowns with the combat manager's group
//!   inhibition. Guards patrol the level's `DishonoredRoute`s (adopted by distance and
//!   capacity, waiting `m_fGuardDuration` at nav points). A civilian entering Panic (or a
//!   guard entering Combat) is an `AllyBusted` group stimulus for civilians within
//!   `m_fGroupDistance`.
//!
//! What is inferred (LIKELY/SPECULATIVE in `research/UNKNOWNS.md`): the light level (the
//! simulation has no light probe: `LIGHT_LEVEL`), the speed normalisation of the player's
//! visibility, squad restrictions of routes, and alarm bells.

use dishonored_ir::ai::{AiConfig, AttentionConfig, AttentionIncrease, DAL_BUSTED, DAL_HEAD_TRACK, DAL_INVESTIGATE, DAL_TURN_TO_FACE, PatrolRoute, PlayerVisibilityConfig, VisionConfig, reason};
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
/// Raw light value at the player (SPECULATIVE: no light probe in the simulation; fully lit,
/// which the Boyle party is).
pub const LIGHT_LEVEL: f32 = 1.0;
/// Speed giving the speed value 1 in the player's visibility (SPECULATIVE normalisation of
/// `DishonoredVisibilityComponent.m_fCurrentSpeedValue`: the sprint speed).
const VIS_SPEED_REF: f32 = 600.0;
/// Panic triggered by a behaviour rule rather than an attention reason (the original's
/// `DisAIBrainProcessPanic` starts the behaviour directly; here it is modelled as Busted
/// attention toward the place, which no reason gate may block).
const FORCED: u8 = 31;
/// Profile slots of the rolled stop thresholds (`m_AttnStopThresholds_*`).
const P_NEUTRAL: usize = 0;
const P_ENEMY_UNS: usize = 2;
const P_ENEMY_SUS: usize = 3;

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
    /// `m_bIsSighted`: seen with line of sight, kept while a hostile target stays within
    /// `m_fDistanceToLoseSight`.
    #[serde(default)]
    pub sighted: bool,
    /// `m_fLastLOSDistance`: normalized distance in the cone that saw the target.
    #[serde(default)]
    pub los_norm: f32,
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
    /// Stop-threshold rolls in [-1, 1] per profile slot and level, scaled by
    /// `m_fStoppingVariance` (`DisAIBrainProcessAttention` 0x00b30270).
    #[serde(default)]
    pub stop_roll: [[f32; 4]; 4],
    /// Remaining cooldown of each melee context (`AiConfig::combat.melee_contexts`).
    #[serde(default)]
    pub melee_cd: Vec<f32>,
    /// Adopted `DishonoredRoute` (index into `Crowd::routes`), point index and direction.
    #[serde(default)]
    pub patrol: Option<usize>,
    #[serde(default)]
    pub patrol_dir: i8,
    /// Time left standing at a guard point (`m_fGuardTimer`); negative = forever.
    #[serde(default)]
    pub guard_timer: f32,
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
    /// `EAIStimID_AllyBusted` group stimuli from last tick: (source, kind, position, threat).
    #[serde(default)]
    pub ally_busted: Vec<(PlayerId, PawnKind, Vec3, Vec3)>,
    /// Group melee inhibition (`DisGlobalCombatManager` attack history): (target, context,
    /// seconds left).
    #[serde(default)]
    pub inhibit: Vec<(PlayerId, u8, f32)>,
    /// The level's patrol routes (`DishonoredRoute`) and how many NPCs adopted each.
    #[serde(default)]
    pub routes: Vec<PatrolRoute>,
    #[serde(default)]
    pub route_users: Vec<i32>,
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

/// One vision cone of `DisTweaks_Vision`, prepared like the vision component does when the
/// tweak changes (0x00c67410 / 0x00c66830): half angle, its cosines and the elliptical
/// vertical limits (`m_fVision_*VerticalFOVScale_Up/Down` scale the half angle).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cone {
    pub range: f32,
    half: f32,
    cos_half: f32,
    /// cos(2 * half)
    cos_full: f32,
    sin_half: f32,
    /// (angle, sin(angle), cos(2 * angle)) of the vertical half angle up / down.
    up: [f32; 3],
    down: [f32; 3],
    /// The same for the rear cone of wide (> 180 deg) fields of view.
    rear: f32,
    rear_up: [f32; 3],
    rear_down: [f32; 3],
}

impl Cone {
    pub fn new(fov_deg: f32, range: f32, up: f32, down: f32) -> Cone {
        let half = fov_deg.to_radians() * 0.5;
        let v = |a: f32| [a, a.sin(), 1.0 - 2.0 * a.sin() * a.sin()];
        let rear = std::f32::consts::PI - half;
        Cone {
            range,
            half,
            cos_half: half.cos(),
            cos_full: 2.0 * half.cos() * half.cos() - 1.0,
            sin_half: half.sin(),
            up: v(up * half),
            down: v(down * half),
            rear,
            rear_up: v(rear * up),
            rear_down: v(rear * down),
        }
    }

    /// Is the eye-space unit direction (x forward, y lateral, z up) inside the cone?
    /// (0x00bec910.) The off-axis angle limit is interpolated between the horizontal and
    /// the vertical half angle with cos(2 phi), phi the elliptical angle around the axis.
    pub fn contains(&self, dir: Vec3) -> bool {
        let lim = |cos2_v: f32, sin_v: f32| {
            let phi = (self.sin_half * dir.z).atan2(sin_v * dir.y);
            let c = (2.0 * phi).cos();
            (cos2_v + c * (self.cos_full - cos2_v) + self.cos_full) * 0.5
        };
        if self.half > std::f32::consts::FRAC_PI_2 {
            // Wide cone: the whole front, and the rear minus the excluded back cone.
            if dir.x > 0.0 {
                return true;
            }
            let back = -dir.x;
            if self.rear > 0.0 {
                let [a, s, c2] = if dir.z < 0.0 { self.rear_down } else { self.rear_up };
                if a > 0.0 && -self.cos_half <= back && lim(c2, s) <= back * back * 2.0 - 1.0 {
                    return false;
                }
            }
            return true;
        }
        if dir.x < 0.0 || self.half <= 0.0 {
            return false;
        }
        let [a, s, c2] = if dir.z < 0.0 { self.down } else { self.up };
        if a <= 0.0 || dir.x < self.cos_half {
            return false;
        }
        dir.x * dir.x * 2.0 - 1.0 >= lim(c2, s)
    }
}

/// The two cones of a vision tweak.
pub fn cones(v: &VisionConfig) -> (Cone, Cone) {
    (Cone::new(v.inner_fov_deg, v.inner_range, v.inner_up, v.inner_down), Cone::new(v.outer_fov_deg, v.outer_range, v.outer_up, v.outer_down))
}

/// Normalized vision distance of an eye-space offset (0x00bf1580): the inner cone is tested
/// first, then the outer one; the result is the distance over the range of the cone that
/// contains the target, and whether it is the inner cone. `None` outside both.
pub fn vision_distance(inner: &Cone, outer: &Cone, local: Vec3) -> Option<(f32, bool)> {
    let d2 = local.length_squared();
    if d2 < 1e-4 {
        return (inner.range > 0.0 || outer.range > 0.0).then_some((0.0, inner.range > 0.0));
    }
    let d = d2.sqrt();
    let dir = local / d;
    if d2 <= inner.range * inner.range && inner.contains(dir) {
        return Some((d / inner.range, true));
    }
    if d2 <= outer.range * outer.range && outer.contains(dir) {
        return Some((d / outer.range, false));
    }
    None
}

/// Attention per second while a target is sighted (`DisAttentionInfo_Complex`, 0x00b4fa50):
/// `m_DistanceValueTuning(1 - norm)` with the target's `Attribute_Visibility` as operand,
/// clamped to [0, 1] when `m_bClampTargetVisibility`, times the seeing scalar. (The target's
/// `Attribute_Visibility[Not]MovingMultiplier` would scale the value by `1 + m`; both are 0
/// unless a power or bone charm modifies them.)
pub fn seen_rate(prof: &AttentionConfig, scalar: f32, norm: f32, visibility: f32) -> f32 {
    let v = prof.distance_tuning.apply(1.0 - norm, visibility);
    let v = if prof.clamp_target_visibility { v.clamp(0.0, 1.0) } else { v };
    scalar * v
}

/// The player's `Attribute_Visibility` (`DishonoredVisibilityComponent`, 0x00ad1590): the
/// light tuning of the normalized light is the operand of the speed tuning of the speed value
/// (zero while sneaking), plus the stance bonus.
pub fn player_visibility(pv: &PlayerVisibilityConfig, light: f32, speed01: f32, crouched: bool, sprinting: bool) -> f32 {
    let span = (pv.light_brightest - pv.light_darkest).max(1e-6);
    let light01 = ((light - pv.light_darkest) / span).clamp(0.0, 1.0);
    let speed = if crouched && pv.zero_speed_sneaking { 0.0 } else { speed01 };
    let base = pv.speed.apply(speed, pv.light.value(light01));
    let bonus = if crouched {
        pv.bonus_sneaking
    } else if speed01 < 1e-4 {
        pv.bonus_standing
    } else if sprinting {
        pv.bonus_sprinting
    } else {
        pv.bonus_walking
    };
    base + bonus
}

/// Level after a change (0x00b4c600): rising values climb past the start thresholds,
/// falling values drop below the (rolled) stop thresholds; the level never moves the other
/// way.
pub fn level_after(level: u8, value: f32, rising: bool, start: &[f32; 4], stop: &[f32; 4]) -> u8 {
    let mut l = level;
    if rising {
        for (i, s) in start.iter().enumerate() {
            if value >= *s && l < i as u8 + 1 {
                l = i as u8 + 1;
            }
        }
    } else {
        for i in (0..4).rev() {
            if value <= stop[i] && l > i as u8 {
                l = i as u8;
            }
        }
    }
    l
}

impl Attn {
    /// Apply one attention change (the pending-change pass 0x00b60ca0 and the level update
    /// 0x00b5b680): add `amount`, floor at the start threshold of `floor_level`, clamp to the
    /// maximum, then update the level; Busted needs a reason with `m_bCanIncreaseToBusted`
    /// (unless already Busted) and is refused for an unsighted target whose proxy turned out
    /// stale (`stale_proxy`, 0x00b4fce0). A refused Busted caps the value at its start.
    fn change(&mut self, prof: &AttentionConfig, stop: &[f32; 4], amount: f32, floor_level: u8, why: u8, stale_proxy: bool) {
        let before = self.value.clamp(0.0, prof.max);
        let mut v = (self.value + amount).max(0.0);
        if floor_level > 0 {
            v = v.max(prof.level_value(floor_level));
        }
        self.value = v.min(prof.max);
        let mut lvl = level_after(self.level, self.value, before <= self.value, &prof.start, stop);
        if lvl == DAL_BUSTED {
            let refused = (self.level < DAL_BUSTED && why != FORCED && !prof.reason_can_bust(why)) || stale_proxy;
            if refused {
                self.value = self.value.min(prof.start[3]);
                lvl = DAL_INVESTIGATE;
            }
        }
        self.level = lvl;
    }
}

/// Crowd state a brain uses while acting.
struct Ctx<'a> {
    layout: &'a CrowdLayout,
    routes: &'a [PatrolRoute],
    users: &'a mut Vec<i32>,
    inhibit: &'a mut Vec<(PlayerId, u8, f32)>,
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
        let near: Vec<(Vec3, f32)> =
            points.iter().copied().filter(|(p, _)| p.distance(cfg.center) <= cfg.radius).filter_map(|(p, y)| snap_floor(world, p).map(|q| (q, y))).filter(|(p, _)| on_nav(*p)).collect();
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
            b.state = if b.has_patrol() { BrainState::Patrol } else { BrainState::Alert };
            brains.push(b);
            id += 1;
        }
        Crowd { brains, layout, spawns, ..Default::default() }
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
        let busted = std::mem::take(&mut self.ally_busted);
        let mut new_barks = Vec::new();
        let mut new_busted = Vec::new();
        for i in self.inhibit.iter_mut() {
            i.2 -= dt;
        }
        self.inhibit.retain(|i| i.2 > 0.0);
        let layout = &self.layout;
        let ai = &self.ai;
        let routes = &self.routes;
        let users = &mut self.route_users;
        let inhibit = &mut self.inhibit;
        let mut out = Vec::with_capacity(self.brains.len());
        for b in self.brains.iter_mut() {
            let Some(me) = world_state.player(b.id) else {
                continue;
            };
            if !me.alive {
                continue;
            }
            b.perceive(me, world_state, events, &peers, &barks, &busted, world, ai, dt);
            let was = b.state;
            let mut ctx = Ctx { layout, routes, users, inhibit };
            out.push((b.id, b.act(me, world_state, &mut ctx, world, ai, dt)));
            let panicked = |s: BrainState| matches!(s, BrainState::Flee | BrainState::Cower);
            if b.kind != PawnKind::Guard && panicked(b.state) && !panicked(was) {
                // `DisAISubStateFlee.bDoBarkForHelp`: call for help at talking loudness.
                new_barks.push((me.position, ai.civilian_talking_loudness));
                new_busted.push((b.id, b.kind, me.position, b.threat_pos.unwrap_or(me.position)));
            }
            if b.kind == PawnKind::Guard && b.state == BrainState::Combat && was != BrainState::Combat {
                let at = b.target.and_then(|t| world_state.player(t)).map(|p| p.position).unwrap_or(me.position);
                new_busted.push((b.id, b.kind, me.position, at));
            }
        }
        self.barks = new_barks;
        self.ally_busted = new_busted;
        out
    }

    /// The level's `DishonoredRoute`s: every guard adopts one (`DisPatrolManager`, 0x00c41590:
    /// the route nearest to the pawn among those with free `m_Capacity`, starting at the
    /// nearest point). Without routes guards keep their fallback loop of nav points.
    pub fn set_routes(&mut self, routes: &[PatrolRoute], world: &dyn WorldQuery) {
        self.routes = routes
            .iter()
            .filter(|r| !r.points.is_empty())
            .map(|r| {
                let mut r = r.clone();
                for p in r.points.iter_mut() {
                    p.position = snap_floor(world, p.position).unwrap_or(p.position);
                }
                r
            })
            .collect();
        self.route_users = vec![0; self.routes.len()];
        let routes = &self.routes;
        let users = &mut self.route_users;
        for b in self.brains.iter_mut().filter(|b| b.kind == PawnKind::Guard) {
            b.patrol = None;
            let pos = b.home;
            if b.adopt_route(routes, users, pos, None) {
                b.state = BrainState::Patrol;
            }
        }
    }

    /// `DisGlobalCombatManager` skirmish caps: per target, the nearest `skirmish_inner`
    /// guards engage in melee; the next ones (up to `skirmish_total`) hold the outer ring.
    fn assign_skirmish(&mut self, ws: &WorldState) {
        let inner = self.ai.combat.skirmish_inner as usize;
        let mut targets: Vec<PlayerId> = self.brains.iter().filter(|b| b.state == BrainState::Combat).filter_map(|b| b.target).collect();
        targets.sort_unstable();
        targets.dedup();
        for t in targets {
            let Some(tp) = ws.player(t).map(|p| p.position) else {
                continue;
            };
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
                // The route stays adopted (its point index restarts where it was).
                *b = Brain { route: b.route.clone(), patrol: b.patrol, route_i: b.route_i, ..Brain::new(s.id, s.kind, s.position, b.rng) };
                if b.kind == PawnKind::Guard {
                    b.state = if b.has_patrol() { BrainState::Patrol } else { BrainState::Alert };
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
            stop_roll: Self::roll_stops(&mut (seed | 1).wrapping_mul(0x2545_F491_4F6C_DD1D)),
            melee_cd: Vec::new(),
            patrol: None,
            patrol_dir: 1,
            guard_timer: 0.0,
        }
    }

    /// The per-brain stop-threshold rolls (uniform in [-1, 1]).
    fn roll_stops(rng: &mut u64) -> [[f32; 4]; 4] {
        let mut r = [[0.0; 4]; 4];
        for p in r.iter_mut() {
            for v in p.iter_mut() {
                *v = rng_next(rng) * 2.0 - 1.0;
            }
        }
        r
    }

    /// Mission control: treat `target` as an enemy at full attention.
    pub fn alert_to(&mut self, ai: &AiConfig, target: PlayerId, at: Vec3) {
        if !self.hostile.contains(&target) {
            self.hostile.push(target);
        }
        self.bump(ai, target, AttentionIncrease { amount: 0.0, to_level: 5 }, at, true, reason::ATTACKED_BY);
        self.suspecting = true;
    }

    /// `EDisAISuspicionLevel` (DisGlobalEnums): 0 `DAISL_Unsuspecting`, 1 `DAISL_Suspecting`.
    /// It selects the attention profile and is the output index of the conversation
    /// suspicion check.
    pub fn suspicion_level(&self) -> u8 {
        self.suspecting as u8
    }

    /// `eDisConvCombatStatus` (`DisConv_SpeakerCombatStatus` output, `m_bNotWhile*` speaker
    /// rules): 0 InCombat, 1 Suspicious, 2 Fearful, 3 Other. LIKELY mapping from the
    /// behaviour (the native accessor was not traced): Combat; Notice / Investigate / Search
    /// (or reacting toward one); Panic (Flee / Cower); anything else.
    pub fn conv_combat_status(&self) -> u8 {
        let s = if self.state == BrainState::React { self.next.unwrap_or(BrainState::Idle) } else { self.state };
        match s {
            BrainState::Combat => 0,
            BrainState::Notice | BrainState::Investigate | BrainState::Search => 1,
            BrainState::Flee | BrainState::Cower => 2,
            _ => 3,
        }
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

    /// Profile slot of a target: neutral pawns, or enemies/places while (un)suspecting.
    fn profile_slot(&self, who: PlayerId) -> usize {
        if who != NOISE && !self.hostile.contains(&who) {
            P_NEUTRAL
        } else if self.suspecting {
            P_ENEMY_SUS
        } else {
            P_ENEMY_UNS
        }
    }

    fn profile<'a>(&self, ai: &'a AiConfig, who: PlayerId) -> &'a AttentionConfig {
        match (self.profile_slot(who), self.kind == PawnKind::Guard) {
            (P_NEUTRAL, _) => &ai.neutral,
            (P_ENEMY_UNS, true) => &ai.guard_unsuspecting,
            (_, true) => &ai.guard_suspecting,
            (P_ENEMY_UNS, false) => &ai.civilian_unsuspecting,
            (_, false) => &ai.civilian_suspecting,
        }
    }

    /// The brain's stop thresholds for a target (`m_AttnStopThresholds_*`): the profile's
    /// stops moved by the brain's roll times `m_fStoppingVariance`.
    fn stops(&self, ai: &AiConfig, who: PlayerId) -> [f32; 4] {
        let prof = self.profile(ai, who);
        let roll = self.stop_roll[self.profile_slot(who)];
        std::array::from_fn(|i| prof.stop[i] + roll[i] * prof.stopping_variance[i])
    }

    fn entry(&mut self, who: PlayerId, proxy: Vec3) -> &mut Attn {
        if let Some(i) = self.attn.iter().position(|a| a.who == who) {
            return &mut self.attn[i];
        }
        self.attn.push(Attn { who, value: 0.0, level: 0, proxy, fresh: false, urgent: false, sighted: false, los_norm: 0.0 });
        self.attn.last_mut().unwrap()
    }

    /// Apply a `DisAttentionIncrease` toward `who` for a reason (`reason::*`).
    fn bump(&mut self, ai: &AiConfig, who: PlayerId, inc: AttentionIncrease, proxy: Vec3, urgent: bool, why: u8) {
        if inc.amount == 0.0 && inc.to_level == 0 {
            return;
        }
        let prof = self.profile(ai, who).clone();
        let stop = self.stops(ai, who);
        let e = self.entry(who, proxy);
        e.change(&prof, &stop, inc.amount, inc.to_level, why, false);
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
        self.bump(ai, NOISE, AttentionIncrease { amount: 0.0, to_level: DAL_BUSTED }, at, true, FORCED);
    }

    /// Eye-space offset of a point seen by `me` (yaw only; x forward, y lateral, z up).
    fn eye_local(me: &PlayerState, p: Vec3) -> Vec3 {
        let d = p - (me.position + Vec3::Z * EYE);
        let (s, c) = me.yaw.to_radians().sin_cos();
        Vec3::new(d.x * c + d.y * s, -d.x * s + d.y * c, d.z)
    }

    /// The vision component's test (0x00c788e0): the target's location inside a cone, then a
    /// line-of-sight trace to one of its test points (top, middle, bottom of the pawn).
    /// Returns the normalized vision distance.
    fn look(me: &PlayerState, other: &PlayerState, world: &dyn WorldQuery, ai: &AiConfig) -> Option<f32> {
        let (inner, outer) = cones(&ai.vision);
        let (norm, _) = vision_distance(&inner, &outer, Self::eye_local(me, other.position))?;
        let eye = me.position + Vec3::Z * EYE;
        let clear = |p: Vec3| {
            let to = p - eye;
            let d = to.length();
            d < 1.0 || !world.raycast(eye, to / d, d, channel::VISION).is_some_and(|h| h.distance < d - 40.0)
        };
        [60.0, 0.0, -50.0].iter().any(|z| clear(other.position + Vec3::Z * *z)).then_some(norm)
    }

    /// Cone and line-of-sight test (events, allies, corpses).
    fn sees(me: &PlayerState, other: &PlayerState, world: &dyn WorldQuery, ai: &AiConfig) -> bool {
        Self::look(me, other, world, ai).is_some()
    }

    /// The target's `Attribute_Visibility`: built from light, speed and stance for players,
    /// the attribute base (0) for NPCs.
    fn visibility(ai: &AiConfig, other: &PlayerState) -> f32 {
        if other.kind != PawnKind::Human {
            return 0.0;
        }
        let speed01 = (other.velocity.truncate().length() / VIS_SPEED_REF).clamp(0.0, 1.0);
        let sprinting = other.last_input.buttons & buttons::SPRINT != 0;
        player_visibility(&ai.player_visibility, LIGHT_LEVEL, speed01, other.crouched, sprinting)
    }

    #[allow(clippy::too_many_arguments)]
    fn perceive(
        &mut self,
        me: &PlayerState,
        ws: &WorldState,
        events: &[SimEvent],
        peers: &[Peer],
        barks: &[(Vec3, f32)],
        ally_busted: &[(PlayerId, PawnKind, Vec3, Vec3)],
        world: &dyn WorldQuery,
        ai: &AiConfig,
        dt: f32,
    ) {
        for a in self.attn.iter_mut() {
            a.fresh = false;
        }
        // Sounds (gameplay sound events, never raw audio) -> attention toward the place
        // (`DACRT_Heard`).
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
                self.bump(ai, NOISE, inc, *position, urgent, reason::HEARD);
            }
        }
        // Help barks from fleeing civilians (`EAIStimID_Help`, `DACRT_HeardHelpRequest`). For
        // civilians Help is one of the party's scary noises (`m_HearNoise`: AlarmBell, Help,
        // DeathRattle), so it starts a panic.
        for (p, r) in barks {
            if p.distance(me.position) <= *r && p.distance(me.position) > 1.0 {
                let inc = self.profile(ai, NOISE).help_sound;
                self.bump(ai, NOISE, inc, *p, true, reason::HEARD_HELP_REQUEST);
                if self.kind != PawnKind::Guard {
                    self.panic_from(ai, *p);
                }
            }
        }
        // Group stimuli (`EAIStimID_AllyBusted`, `m_fGroupDistance`): the panic process
        // (0x00b61430) starts a panic on `m_WitnessAllyPanic` when the ally's threat is not an
        // enemy of it, and on `m_WitnessAllyCombat` (seeing the fight) when it is.
        if self.kind != PawnKind::Guard && !matches!(self.state, BrainState::Flee | BrainState::Cower) {
            for &(src, kind, pos, threat) in ally_busted {
                if src == self.id || pos.distance(me.position) > ai.group_distance {
                    continue;
                }
                let witnessed = match kind {
                    PawnKind::Guard => ws.player(src).is_some_and(|g| Self::sees(me, g, world, ai)),
                    _ => true,
                };
                if witnessed {
                    self.panic_from(ai, threat);
                    break;
                }
            }
        }
        for e in events {
            match e {
                // Only players become enemies this way: NPCs of one faction do not turn on each other
                // over friendly fire (factions are not modelled yet, see AI.md).
                SimEvent::Damage { victim, attacker: Some(a), .. } if *victim == self.id && *a != self.id && ws.player(*a).is_some_and(|p| p.kind == PawnKind::Human) => {
                    let at = ws.player(*a).map(|p| p.position).unwrap_or(me.position);
                    if !self.hostile.contains(a) {
                        self.hostile.push(*a);
                    }
                    let inc = self.profile(ai, *a).attacked_by;
                    self.bump(ai, *a, inc, at, true, reason::ATTACKED_BY);
                }
                SimEvent::Death { victim, killer: Some(k), .. } if *k != self.id && ws.player(*k).is_some_and(|p| p.kind == PawnKind::Human) => {
                    let (Some(v), Some(kp)) = (ws.player(*victim), ws.player(*k)) else {
                        continue;
                    };
                    if Self::sees(me, v, world, ai) || Self::sees(me, kp, world, ai) {
                        // Witnessed murder: the killer becomes an enemy at full attention.
                        if !self.hostile.contains(k) {
                            self.hostile.push(*k);
                        }
                        let inc = self.profile(ai, *k).attacked_enemy;
                        self.bump(ai, *k, inc, kp.position, true, reason::ATTACKED_ENEMY);
                        self.threat_pos = Some(kp.position);
                        if !self.corpses_seen.contains(victim) {
                            self.corpses_seen.push(*victim);
                        }
                    } else if v.position.distance(me.position) <= ai.death_rattle_loudness {
                        let inc = self.profile(ai, NOISE).death_rattle;
                        self.bump(ai, NOISE, inc, v.position, true, reason::HEARD_DEATH);
                        if self.kind != PawnKind::Guard {
                            self.panic_from(ai, v.position);
                        }
                    }
                }
                _ => {}
            }
        }
        // Vision: humans (neutral or enemy) and corpses.
        for p in ws.players.iter() {
            if p.id == self.id {
                continue;
            }
            if !p.alive {
                // A corpse is discovered once its seen value is non-zero (dead-NPC scalar).
                if p.kind != PawnKind::Human && !self.corpses_seen.contains(&p.id) {
                    let prof = self.profile(ai, NOISE);
                    let seen = Self::look(me, p, world, ai).is_some_and(|n| seen_rate(prof, prof.seeing_scalar_dead_npc, n, 0.0) > 0.0);
                    if seen {
                        self.corpses_seen.push(p.id);
                        let inc = prof.discovered_corpse;
                        self.bump(ai, NOISE, inc, p.position, false, reason::FOUND_CORPSE);
                        self.suspecting = true;
                        if self.kind != PawnKind::Guard {
                            // `m_WitnessCorpse`.
                            self.panic_from(ai, p.position);
                        }
                    }
                }
                continue;
            }
            if p.kind != PawnKind::Human {
                continue;
            }
            let seen = Self::look(me, p, world, ai);
            if seen.is_some() && criminal(p) && !self.hostile.contains(&p.id) {
                // Seen attacking: hostile at once (`m_Response_*`, `DACRT_AttackedEnemy`).
                self.hostile.push(p.id);
                let inc = self.profile(ai, p.id).attacked_enemy;
                self.bump(ai, p.id, inc, p.position, true, reason::ATTACKED_ENEMY);
                self.threat_pos = Some(p.position);
                continue;
            }
            let hostile = self.hostile.contains(&p.id);
            let prof = self.profile(ai, p.id).clone();
            let stop = self.stops(ai, p.id);
            let vis = Self::visibility(ai, p);
            let near = (p.position - me.position).truncate().length() <= prof.distance_to_lose_sight && (p.position.z - me.position.z).abs() <= prof.distance_to_lose_sight;
            let touching = p.position.distance(me.position) < 80.0;
            if seen.is_none() && !touching && !self.attn.iter().any(|a| a.who == p.id) {
                continue;
            }
            let e = self.entry(p.id, p.position);
            match seen {
                Some(n) => {
                    e.sighted = true;
                    e.los_norm = n;
                }
                // `m_fDistanceToLoseSight` (enemies only): sight is kept without line of
                // sight while the target stays this close (0x00b4f970).
                None if e.sighted && hostile && near => {}
                None => e.sighted = false,
            }
            if e.sighted {
                let rate = seen_rate(&prof, prof.seeing_scalar_player, e.los_norm, vis);
                if rate > 0.0 {
                    e.change(&prof, &stop, rate * dt, 0, reason::SEEN, false);
                    e.proxy = p.position;
                    e.fresh = true;
                }
            }
            if touching {
                e.change(&prof, &stop, prof.touching_rate * dt, 0, reason::TOUCHED, false);
                e.proxy = p.position;
                e.fresh = true;
            }
        }
        // Allies: guards see another guard fighting or a civilian panicking.
        for peer in peers {
            if peer.id == self.id || self.kind != PawnKind::Guard {
                continue;
            }
            let Some(pp) = ws.player(peer.id).filter(|p| p.alive) else {
                continue;
            };
            match (peer.kind, peer.state) {
                (PawnKind::Guard, BrainState::Combat) => {
                    let Some(t) = peer.target else { continue };
                    if self.state == BrainState::Combat || !Self::sees(me, pp, world, ai) {
                        continue;
                    }
                    let tp = ws.player(t).map(|p| p.position).unwrap_or(pp.position);
                    if !self.hostile.contains(&t) {
                        self.hostile.push(t);
                    }
                    let inc = self.profile(ai, t).ally_combat_target;
                    self.bump(ai, t, inc, tp, true, reason::SEE_ALLY_COMBAT);
                }
                (PawnKind::Civilian, BrainState::Flee | BrainState::Cower) => {
                    if Self::sees(me, pp, world, ai) {
                        let inc = self.profile(ai, NOISE).ally_panic_target;
                        self.bump(ai, NOISE, inc, peer.threat.unwrap_or(pp.position), false, reason::SEE_ALLY_PANIC);
                    }
                }
                _ => {}
            }
        }
        // Leak (0x00b61040): only entries without any change this tick lose
        // `m_fAttentionLeakRate` per second; levels fall on the rolled stops. An enemy entry
        // whose proxy proved stale (the NPC stands at it, the target is elsewhere and unseen)
        // cannot stay Busted (0x00b4fce0).
        let slots: Vec<(AttentionConfig, [f32; 4])> = self.attn.iter().map(|a| (self.profile(ai, a.who).clone(), self.stops(ai, a.who))).collect();
        for (a, (prof, stop)) in self.attn.iter_mut().zip(slots.iter()) {
            let stale = !a.sighted && a.who != NOISE && {
                let r = prof.min_range_unsighted_busted_proxy;
                let target_far = ws.player(a.who).is_none_or(|t| t.position.distance(a.proxy) > r);
                me.position.distance(a.proxy) <= r && target_far
            };
            if !a.fresh {
                a.change(prof, stop, -prof.leak_rate * dt, 0, reason::DRAIN, stale);
            } else if stale && a.level == DAL_BUSTED {
                a.change(prof, stop, 0.0, 0, reason::DRAIN, true);
            }
        }
        self.attn.retain(|a| a.value > 0.0 || a.fresh || a.sighted);
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
        self.guard_timer = 0.0;
        self.state = if self.kind == PawnKind::Guard { if self.has_patrol() { BrainState::Patrol } else { BrainState::Alert } } else { BrainState::Idle };
    }

    fn has_patrol(&self) -> bool {
        self.patrol.is_some() || self.route.len() >= 2
    }

    /// Adopt the nearest route with free capacity (0x00c41590), other than `exclude`; start
    /// at its point nearest to `pos`.
    fn adopt_route(&mut self, routes: &[PatrolRoute], users: &mut [i32], pos: Vec3, exclude: Option<usize>) -> bool {
        let mut best: Option<(f32, usize, usize)> = None;
        for (ri, r) in routes.iter().enumerate() {
            if Some(ri) == exclude || (r.capacity >= 0 && users[ri] >= r.capacity) {
                continue;
            }
            for (pi, p) in r.points.iter().enumerate() {
                let d = p.position.distance_squared(pos);
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, ri, pi));
                }
            }
        }
        let Some((_, ri, pi)) = best else {
            return false;
        };
        if let Some(old) = self.patrol {
            users[old] -= 1;
        }
        users[ri] += 1;
        self.patrol = Some(ri);
        self.route_i = pi;
        self.patrol_dir = 1;
        self.guard_timer = 0.0;
        true
    }

    /// Next point of the adopted route (`Route.ResolveRouteIndex`): Circle wraps, Loop turns
    /// back at the ends, Linear ends the route (another one is adopted).
    fn advance_route(&mut self, routes: &[PatrolRoute], users: &mut [i32], pos: Vec3) {
        let Some(ri) = self.patrol else { return };
        let n = routes[ri].points.len();
        let next = self.route_i as i64 + self.patrol_dir as i64;
        match routes[ri].kind {
            2 => self.route_i = next.rem_euclid(n as i64) as usize,
            1 => {
                if next < 0 || next >= n as i64 {
                    self.patrol_dir = -self.patrol_dir;
                }
                self.route_i = (self.route_i as i64 + self.patrol_dir as i64).clamp(0, n as i64 - 1) as usize;
            }
            _ => {
                if next >= n as i64 || next < 0 {
                    if !self.adopt_route(routes, users, pos, Some(ri)) {
                        self.route_i = 0;
                    }
                } else {
                    self.route_i = next as usize;
                }
            }
        }
        self.guard_timer = 0.0;
    }

    /// Behaviour step plus a ledge guard for off-mesh movement.
    #[allow(clippy::too_many_arguments)]
    fn act(&mut self, me: &PlayerState, ws: &WorldState, ctx: &mut Ctx, world: &dyn WorldQuery, ai: &AiConfig, dt: f32) -> PlayerInput {
        self.select(ws, ai);
        let mut i = self.act_inner(me, ws, ctx, world, ai, dt);
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
                } else if self.state == BrainState::Patrol && self.patrol.is_some() {
                    self.advance_route(ctx.routes, ctx.users, me.position);
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
        // Combat lasts while the target is Busted (`EAIStimID_CombatToSearch` otherwise):
        // unseen, its attention leaks below the Busted stop or the proxy proves stale.
        if self.state == BrainState::Combat
            && let Some(t) = self.target
            && self.attn.iter().find(|a| a.who == t).is_none_or(|a| a.level < DAL_BUSTED)
        {
            self.go_search(ai);
            return;
        }
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
    fn act_inner(&mut self, me: &PlayerState, ws: &WorldState, ctx: &mut Ctx, world: &dyn WorldQuery, ai: &AiConfig, dt: f32) -> PlayerInput {
        let layout = ctx.layout;
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
                    if self.kind == PawnKind::Guard && self.has_patrol() {
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
            BrainState::Patrol if self.patrol.is_some() => {
                // `DisBehaviorPatrol`: walk the adopted route; at a nav point with
                // `m_fGuardDuration` stand that long (forever with `m_bGuardForever`, also for
                // a single-point route), otherwise walk past it.
                let ri = self.patrol.unwrap_or(0).min(ctx.routes.len().saturating_sub(1));
                let Some(route) = ctx.routes.get(ri) else {
                    self.patrol = None;
                    return idle;
                };
                let p = route.points[self.route_i.min(route.points.len() - 1)].clone();
                let single = route.points.len() == 1;
                if self.guard_timer != 0.0 {
                    if self.guard_timer > 0.0 {
                        self.guard_timer = (self.guard_timer - dt).max(0.0);
                        if self.guard_timer == 0.0 {
                            self.advance_route(ctx.routes, ctx.users, me.position);
                        }
                    }
                    return look.map(face).unwrap_or(idle);
                }
                self.goal = Some(p.position);
                if Self::reached(me, p.position, 70.0) || self.follow.failed {
                    self.follow.clear();
                    if p.guard_forever || single {
                        self.guard_timer = -1.0;
                    } else if p.guard_duration > 0.0 {
                        self.guard_timer = p.guard_duration;
                    } else {
                        self.advance_route(ctx.routes, ctx.users, me.position);
                    }
                    return look.map(face).unwrap_or(idle);
                }
                self.move_to(me, nav, p.position, false)
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
            BrainState::Combat => self.combat(me, ws, world, nav, ai, ctx.inhibit, dt),
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

    #[allow(clippy::too_many_arguments)]
    fn combat(&mut self, me: &PlayerState, ws: &WorldState, world: &dyn WorldQuery, nav: Option<&NavWorld>, ai: &AiConfig, inhibit: &mut Vec<(PlayerId, u8, f32)>, dt: f32) -> PlayerInput {
        let idle = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        let Some(t) = self.target.and_then(|id| ws.player(id)).filter(|p| p.alive).cloned() else {
            // Target gone: search where it was last seen.
            self.target = None;
            self.go_search(ai);
            return idle;
        };
        // The target is chased while sighted; otherwise the guard goes to the attention proxy
        // (where it proves stale, attention falls to Investigate: see `perceive`).
        let sighted = self.attn.iter().find(|a| a.who == t.id).is_some_and(|a| a.sighted);
        let goal = if sighted { t.position } else { self.attn.iter().find(|a| a.who == t.id).map(|a| a.proxy).unwrap_or(t.position) };
        if sighted {
            self.threat_pos = Some(t.position);
        }
        let visible = sighted && Self::sees(me, &t, world, ai);
        let d = t.position.distance(me.position);
        let reach = ai.combat.reachable_melee_range;
        let mut i = PlayerInput { yaw: yaw_to(me.position, goal), left_hand: LeftHand::Empty, ..Default::default() };
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
            if goal.distance(me.position) > 500.0 {
                return self.move_to(me, nav, goal, true);
            }
            if d < 300.0 {
                i.forward = -80;
            }
            return i;
        }
        if !sighted || d > reach {
            let mut m = self.move_to(me, nav, goal, true);
            if sighted && d < 400.0 {
                m.yaw = yaw_to(me.position, goal);
            }
            return m;
        }
        // Melee: the first context of the attack pattern that is off its own cooldown
        // (`m_ContextCooldown`) and not inhibited on this target by another NPC's recent use
        // of the same attack class (`DisGlobalCombatManager.m_fInhibitionTime*`).
        let ctxs = &ai.combat.melee_contexts;
        self.melee_cd.resize(ctxs.len(), 0.0);
        for c in self.melee_cd.iter_mut() {
            *c = (*c - dt).max(0.0);
        }
        if me.sword.phase == SwordPhase::Idle {
            let ready = (0..ctxs.len()).find(|&k| self.melee_cd[k] <= 0.0 && !inhibit.iter().any(|x| x.0 == t.id && x.1 as usize == k));
            if let Some(k) = ready {
                let [lo, hi] = ctxs[k].cooldown;
                self.melee_cd[k] = lo + (hi - lo) * rng_next(&mut self.rng);
                if ctxs[k].inhibition > 0.0 {
                    inhibit.push((t.id, k as u8, ctxs[k].inhibition));
                }
                i.buttons |= buttons::PRIMARY;
                return i;
            }
        }
        if t.sword.phase == SwordPhase::Startup {
            i.buttons |= buttons::BLOCK;
        }
        i
    }

    fn go_search(&mut self, ai: &AiConfig) {
        self.suspecting = true;
        self.target = None;
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
    use dishonored_ir::ai::{RoutePoint, VisTuning};
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

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Eye-space direction at `yaw` degrees off axis horizontally and `pitch` degrees up.
    fn dir(yaw: f32, pitch: f32) -> Vec3 {
        let (y, p) = (yaw.to_radians(), pitch.to_radians());
        Vec3::new(y.cos() * p.cos(), y.sin() * p.cos(), p.sin())
    }

    #[test]
    fn distance_tuning_curve_and_formula() {
        let t = VisTuning::guard_distance();
        assert_eq!(t.value(0.2), 0.0);
        assert!(close(t.value(0.55), 0.0));
        assert!(close(t.value(0.7125), 0.4));
        assert!(close(t.value(0.875), 0.8));
        assert!(close(t.value(1.0), 2.0));
        // `DVCTPAO_Add`: the operand is added after the curve.
        assert!(close(t.apply(0.875, 0.1), 0.9));
        // Formula mode (`(x^e + b) * s`) and the multiply op.
        let f = VisTuning { use_points: false, multiply: true, ..VisTuning::guard_distance() };
        assert!(close(f.apply(0.5, 2.0), 0.5f32.powf(4.0) * 3.0 * 2.0));
    }

    #[test]
    fn inner_cone_is_elliptical() {
        let v = AiConfig::default().vision;
        let (inner, outer) = cones(&v);
        // 45 deg inner cone: half angle 22.5 deg horizontally.
        assert!(inner.contains(dir(0.0, 0.0)));
        assert!(inner.contains(dir(20.0, 0.0)));
        assert!(!inner.contains(dir(25.0, 0.0)));
        // Vertical half angles: 0.25 * 22.5 = 5.6 deg up, 0.7 * 22.5 = 15.75 deg down.
        assert!(inner.contains(dir(0.0, 5.0)));
        assert!(!inner.contains(dir(0.0, 7.0)));
        assert!(inner.contains(dir(0.0, -15.0)));
        assert!(!inner.contains(dir(0.0, -17.0)));
        // 160 deg outer cone: 80 deg to the side, but only 8 deg up.
        assert!(outer.contains(dir(75.0, 0.0)));
        assert!(!outer.contains(dir(85.0, 0.0)));
        assert!(!outer.contains(dir(0.0, 10.0)));
        assert!(!outer.contains(dir(180.0, 0.0)));
    }

    #[test]
    fn vision_distance_prefers_the_inner_cone() {
        let (inner, outer) = cones(&AiConfig::default().vision);
        let (n, i) = vision_distance(&inner, &outer, Vec3::new(1000.0, 0.0, 0.0)).unwrap();
        assert!(i && close(n, 1000.0 / 3500.0));
        let (n, i) = vision_distance(&inner, &outer, dir(60.0, 0.0) * 1000.0).unwrap();
        assert!(!i && close(n, 1000.0 / 1700.0));
        assert!(vision_distance(&inner, &outer, dir(60.0, 0.0) * 2000.0).is_none());
        assert!(vision_distance(&inner, &outer, Vec3::new(-500.0, 0.0, 0.0)).is_none());
        assert!(vision_distance(&inner, &outer, Vec3::new(3600.0, 0.0, 0.0)).is_none());
    }

    #[test]
    fn seen_rate_at_distances() {
        let p = AttentionConfig::guard_unsuspecting();
        // 600 uu straight ahead: x = 1 - 600/3500 = 0.8286 -> 0.6857 per second.
        let r = seen_rate(&p, 1.0, 600.0 / 3500.0, 0.0);
        assert!((r - 0.685_71).abs() < 1e-3, "{r}");
        // Beyond 45 % of the cone range the curve gives nothing: only visibility counts.
        assert_eq!(seen_rate(&p, 1.0, 2000.0 / 3500.0, 0.0), 0.0);
        assert!(close(seen_rate(&p, 1.0, 2000.0 / 3500.0, 0.1), 0.1));
        // Close: clamped to 1 before the scalar (Hard difficulty scalar 1.3).
        assert!(close(seen_rate(&p, 1.3, 0.05, 0.0), 1.3));
        // Hidden (visibility -1, e.g. auto-crouched in cover): nothing at 800 uu, but seen
        // at 200 uu where the curve gives 2.
        assert_eq!(seen_rate(&p, 1.0, 800.0 / 3500.0, -1.0), 0.0);
        assert!(close(seen_rate(&p, 1.0, 200.0 / 3500.0, -1.0), 1.0));
    }

    #[test]
    fn player_visibility_light_speed_stance() {
        let pv = PlayerVisibilityConfig::default();
        // Lit, standing: light 0.1 * 1.0.
        assert!(close(player_visibility(&pv, 1.0, 0.0, false, false), 0.1));
        // Lit, full speed: + 0.25.
        assert!(close(player_visibility(&pv, 1.0, 1.0, false, true), 0.35));
        // Sneaking: the speed counts as zero.
        assert!(close(player_visibility(&pv, 1.0, 1.0, true, false), 0.1));
        // Darkest light: nothing from light.
        assert!(close(player_visibility(&pv, 0.05, 0.0, false, false), 0.0));
    }

    #[test]
    fn increase_floors_at_threshold_and_reason_gates_busted() {
        let p = AttentionConfig::guard_unsuspecting();
        let stop = p.stop;
        let mut a = Attn { who: NOISE, value: 0.0, level: 0, proxy: Vec3::ZERO, fresh: false, urgent: false, sighted: false, los_norm: 0.0 };
        // Danger sound: +0.4, then at least the Investigate start (0.6).
        a.change(&p, &stop, 0.4, DAL_INVESTIGATE, reason::HEARD, false);
        assert!(close(a.value, 0.6) && a.level == DAL_INVESTIGATE);
        // Another one: 1.0, but hearing cannot reach Busted: capped at the Busted start.
        a.change(&p, &stop, 0.4, DAL_INVESTIGATE, reason::HEARD, false);
        assert!(close(a.value, 0.85) && a.level == DAL_INVESTIGATE);
        // An increase with no amount raises straight to its threshold (AttackedBy -> max).
        a.change(&p, &stop, 0.0, 5, reason::ATTACKED_BY, false);
        assert!(close(a.value, 1.0) && a.level == DAL_BUSTED);
        // Falling: the level holds until the stop threshold.
        a.change(&p, &stop, -0.19, 0, reason::DRAIN, false);
        assert_eq!(a.level, DAL_BUSTED);
        a.change(&p, &stop, -0.02, 0, reason::DRAIN, false);
        assert_eq!(a.level, DAL_INVESTIGATE);
        // A stale proxy cannot keep Busted.
        let mut b = Attn { value: 0.95, level: DAL_BUSTED, ..a.clone() };
        b.change(&p, &stop, 0.0, 0, reason::DRAIN, true);
        assert!(b.level == DAL_INVESTIGATE && close(b.value, 0.85));
    }

    #[test]
    fn stop_thresholds_roll_within_variance() {
        let ai = AiConfig::default();
        for seed in 1..50u64 {
            let mut b = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, seed);
            b.hostile.push(1);
            let s = b.stops(&ai, 1);
            for i in 0..4 {
                assert!((s[i] - ai.guard_unsuspecting.stop[i]).abs() <= 0.025 + 1e-6);
            }
            // The neutral profile has no variance.
            assert_eq!(b.stops(&ai, 2), ai.neutral.stop);
        }
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
            b.perceive(&guard, &ws, &[], &[], &[], &[], &w, &ai, 1.0 / 60.0);
        }
        assert!(b.focus().is_some_and(|f| f.1 <= DAL_HEAD_TRACK));
        // Enemy: (0.6857 + 0.1 visibility, clamped) per second -> Busted (0.85) in about
        // 1.1 s from 0, faster from the neutral attention already built.
        b.hostile.push(1);
        b.attn.clear();
        let mut t = 0;
        while b.focus().is_none_or(|f| f.1 < DAL_BUSTED) && t < 600 {
            b.perceive(&guard, &ws, &[], &[], &[], &[], &w, &ai, 1.0 / 60.0);
            t += 1;
        }
        assert!((60..75).contains(&t), "busted after {t} ticks");
        b.select(&ws, &ai);
        assert!(b.state == BrainState::React && b.next == Some(BrainState::Combat));
    }

    #[test]
    fn far_player_is_seen_only_through_visibility() {
        let ai = AiConfig::default();
        let w = flat_world();
        let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let mut player = pawn(1, PawnKind::Human, Vec3::new(2500.0, 0.0, 90.0), 180.0);
        let mut ws = WorldState::default();
        ws.players = vec![guard.clone(), player.clone()];
        let mut b = Brain::new(guard.id, PawnKind::Guard, guard.position, 1);
        b.hostile.push(1);
        for _ in 0..60 {
            b.perceive(&guard, &ws, &[], &[], &[], &[], &w, &ai, 1.0 / 60.0);
        }
        // Lit and standing: 0.1 per second.
        let v = b.attn.iter().find(|a| a.who == 1).map(|a| a.value).unwrap();
        assert!((v - 0.1).abs() < 0.01, "{v}");
        // Crouched (sneaking) and still: same light term, no speed; 2500 uu is outside the
        // outer cone's range, so nothing from the curve either.
        player.crouched = true;
        ws.players[1] = player;
        for _ in 0..60 {
            b.perceive(&guard, &ws, &[], &[], &[], &[], &w, &ai, 1.0 / 60.0);
        }
        let v2 = b.attn.iter().find(|a| a.who == 1).map(|a| a.value).unwrap();
        assert!((v2 - 0.2).abs() < 0.01, "{v2}");
    }

    #[test]
    fn danger_sound_raises_to_investigate_and_holds_until_stop() {
        let ai = AiConfig::default();
        let w = flat_world();
        let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let mut ws = WorldState::default();
        ws.players = vec![guard.clone()];
        let mut b = Brain::new(guard.id, PawnKind::Guard, guard.position, 1);
        let shot = SimEvent::Sound { position: Vec3::new(-500.0, 0.0, 90.0), radius: 1500.0, category: SoundCategory::Pistol, source: 7 };
        b.perceive(&guard, &ws, std::slice::from_ref(&shot), &[], &[], &[], &w, &ai, 1.0 / 60.0);
        assert_eq!(b.focus().map(|f| f.1), Some(DAL_INVESTIGATE));
        // Leak 0.025/s from 0.6: after 10 s 0.35, above the Investigate stop (0.3 +- 0.025).
        for _ in 0..(60 * 10) {
            b.perceive(&guard, &ws, &[], &[], &[], &[], &w, &ai, 1.0 / 60.0);
        }
        assert_eq!(b.focus().map(|f| f.1), Some(DAL_INVESTIGATE));
        // After 14 s more (0.0) it is gone.
        for _ in 0..(60 * 14) {
            b.perceive(&guard, &ws, &[], &[], &[], &[], &w, &ai, 1.0 / 60.0);
        }
        assert!(b.focus().is_none());
    }

    #[test]
    fn unseen_enemy_leaks_out_of_combat() {
        let ai = AiConfig::default();
        let w = flat_world();
        let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        // Behind the guard, far: never seen.
        let player = pawn(1, PawnKind::Human, Vec3::new(-2000.0, 0.0, 90.0), 0.0);
        let mut ws = WorldState::default();
        ws.players = vec![guard.clone(), player.clone()];
        let mut b = Brain::new(guard.id, PawnKind::Guard, guard.position, 3);
        b.alert_to(&ai, 1, player.position);
        assert_eq!(b.focus().map(|f| f.1), Some(DAL_BUSTED));
        // Suspecting leak 0.015/s from 1.0 to the Busted stop 0.8 +- 0.025: 11.7 to 15 s.
        let mut t = 0;
        while b.focus().is_some_and(|f| f.1 == DAL_BUSTED) && t < 60 * 30 {
            b.perceive(&guard, &ws, &[], &[], &[], &[], &w, &ai, 1.0 / 60.0);
            t += 1;
        }
        assert!((60 * 11..60 * 15).contains(&t), "left Busted after {t} ticks");
    }

    #[test]
    fn ally_busted_group_stimulus_panics_civilians_in_range() {
        let ai = AiConfig::default();
        let w = flat_world();
        let civ = pawn(NPC_ID_BASE, PawnKind::Civilian, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let mut ws = WorldState::default();
        ws.players = vec![civ.clone()];
        let mut b = Brain::new(civ.id, PawnKind::Civilian, civ.position, 1);
        // An ally panicking 1200 uu away (beyond m_fGroupDistance 1000): nothing.
        let far = [(NPC_ID_BASE + 1, PawnKind::Civilian, Vec3::new(1200.0, 0.0, 90.0), Vec3::new(1500.0, 0.0, 90.0))];
        b.perceive(&civ, &ws, &[], &[], &[], &far, &w, &ai, 1.0 / 60.0);
        assert!(b.focus().is_none());
        // 900 uu away, behind it (no line of sight needed): panic toward the ally's threat.
        let near = [(NPC_ID_BASE + 1, PawnKind::Civilian, Vec3::new(-900.0, 0.0, 90.0), Vec3::new(-1200.0, 0.0, 90.0))];
        b.perceive(&civ, &ws, &[], &[], &[], &near, &w, &ai, 1.0 / 60.0);
        assert_eq!(b.focus().map(|f| f.1), Some(DAL_BUSTED));
        assert_eq!(b.threat_pos, Some(Vec3::new(-1200.0, 0.0, 90.0)));
    }

    fn route(kind: u8, n: usize, x0: f32) -> PatrolRoute {
        PatrolRoute {
            name: String::new(),
            kind,
            capacity: 1,
            points: (0..n).map(|i| RoutePoint { position: Vec3::new(x0 + i as f32 * 500.0, 0.0, 90.0), guard_duration: 0.0, guard_forever: false }).collect(),
        }
    }

    #[test]
    fn routes_adopt_by_distance_and_capacity_and_advance_by_type() {
        let routes = vec![route(2, 3, 0.0), route(1, 3, 5000.0), route(0, 2, 10000.0)];
        let mut users = vec![0; 3];
        let mut a = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 1);
        let mut b = Brain::new(NPC_ID_BASE + 1, PawnKind::Guard, Vec3::ZERO, 2);
        assert!(a.adopt_route(&routes, &mut users, Vec3::new(900.0, 0.0, 90.0), None));
        assert_eq!((a.patrol, a.route_i), (Some(0), 2));
        // Capacity 1: the second guard takes the next nearest route.
        assert!(b.adopt_route(&routes, &mut users, Vec3::new(900.0, 0.0, 90.0), None));
        assert_eq!(b.patrol, Some(1));
        // Circle wraps.
        a.advance_route(&routes, &mut users, Vec3::ZERO);
        assert_eq!(a.route_i, 0);
        // Loop turns back at the end.
        b.route_i = 2;
        b.advance_route(&routes, &mut users, Vec3::ZERO);
        assert_eq!(b.route_i, 1);
        // Linear: at the end another free route is adopted.
        let mut c = Brain::new(NPC_ID_BASE + 2, PawnKind::Guard, Vec3::ZERO, 3);
        assert!(c.adopt_route(&routes, &mut users, Vec3::new(10500.0, 0.0, 90.0), None));
        assert_eq!(c.patrol, Some(2));
        users[0] = 0;
        c.advance_route(&routes, &mut users, Vec3::new(10500.0, 0.0, 90.0));
        c.advance_route(&routes, &mut users, Vec3::new(10500.0, 0.0, 90.0));
        assert_eq!(c.patrol, Some(0));
        assert_eq!(users, vec![1, 1, 0]);
    }
}
