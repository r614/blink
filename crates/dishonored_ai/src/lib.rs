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
//! - **Factions** (`DisTweaks_Faction`, forbidden zones, `SetDisposition`): relationships per
//!   tick (`Social`); **alarm bells** (`DisBehaviorTriggerAlarm`, `DisAlarmBell`); **barks**
//!   (`AiEvent::Bark` at the original's dialog-hook points); the light probe (`light_at`) and
//!   hearing occlusion.
//!
//! What is inferred (LIKELY/SPECULATIVE in `research/UNKNOWNS.md` U-120..U-125): the light
//! probe's scale, hearing through audio cells, the bark attachment to this port's states, and
//! the alarm decision details.

use dishonored_ir::ai::{AiConfig, AiLevel, NpcClass, AlarmBellDef, AttentionConfig, AttentionIncrease, DAL_BUSTED, DAL_HEAD_TRACK, DAL_INVESTIGATE, FactionTable, ForbiddenZone, PatrolRoute, PlayerVisibilityConfig, ProbeLight, Relationship, VisionConfig, reason};
use std::collections::BTreeMap;
use dishonored_sim::events::{NoiseContext, SimEvent};
#[cfg(test)]
use dishonored_sim::events::SoundCategory;
use dishonored_sim::input::{LeftHand, PlayerInput, buttons, npc_moves};
use dishonored_sim::state::{PawnKind, PlayerId, PlayerState, SwordPhase, WorldState};
use dishonored_world::nav::{Follower, NavWorld};
use dishonored_world::{WorldQuery, channel};
use glam::Vec3;
use serde::{Deserialize, Serialize};

pub mod combat;
pub mod head;
pub mod hearing;
pub mod rats;
pub mod slots;
pub mod special;

pub use slots::{Behavior, MagicState, QueuedStim, SearchState, SearchSub, Stim, slot};
use slots::AlarmCause;
pub use special::SpecialCombat;

pub const NPC_ID_BASE: PlayerId = 1000;
/// Stimulus without a known identity (noises, corpses, help barks): attention toward a place.
pub const NOISE: PlayerId = PlayerId::MAX;
/// Largest drop (uu) an NPC will step off deliberately when it walks off the navmesh.
const MAX_SAFE_DROP: f32 = 160.0;
/// Eye height above the pawn centre (the `camera_jnt` bone sits near the top of the
/// 87.5 uu half-height cylinder).
const EYE: f32 = 70.0;
/// Raw light value at the player when the level has no probe lights (fully lit, which the
/// Boyle party is). With lights, [`light_at`] computes it.
pub const LIGHT_LEVEL: f32 = 1.0;
/// Panic triggered by a behaviour rule rather than an attention reason (the original's
/// `DisAIBrainProcessPanic` starts the behaviour directly; here it is modelled as Busted
/// attention toward the place, which no reason gate may block).
const FORCED: u8 = 31;
/// Profile slots of the rolled stop thresholds (`m_AttnStopThresholds_*`).
const P_NEUTRAL: usize = 0;
const P_ENEMY_UNS: usize = 2;
const P_ENEMY_SUS: usize = 3;
/// Distance at which an attack swings rather than travels (the sword trace reaches 210 uu,
/// `DisTweaks_MeleeAttack` of the shared pawn).
const SWORD_REACH: f32 = 200.0;
/// Longest an attack may travel toward its target before it is dropped (s).
const ATTACK_WINDOW: f32 = 1.0;
/// Speed at which the dodge animations cover their distance (uu/s; ground sprint speed).
const DODGE_SPEED: f32 = 600.0;

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
    /// Kept for the saved-state and channel legend layout: the attention behaviours start
    /// without a delay (their stimuli's delay getter returns 0, 0x008fc830), so no brain enters
    /// it; an external write maps to Notice.
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
    /// Going to an alarm bell and ringing it (`DisBehaviorTriggerAlarm`).
    RingAlarm,
    /// Reacting to a power seen (`DisBehaviorMagicResponse`, slot Awe).
    Awe,
}

/// What a crowd reports to its host after a tick.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AiEvent {
    /// The original fires this dialog hook (`eDisDialogHook` name) on the NPC at this
    /// transition (`DisAIBehavior` bark points, `FUN_00af00f0` callers). `initiator` is the
    /// stimulus' pawn when there is one (a player, or an ally for group variants).
    Bark { npc: PlayerId, hook: String, initiator: Option<PlayerId> },
    /// A guard rang an alarm bell (index into `Crowd::alarms`): the alarm is raised.
    AlarmRang { bell: usize, name: String, ringer: PlayerId, enemy: Option<PlayerId> },
    /// `m_fAlarmRingDelayBeforeAINoise` after the ring: the bell's AI noise goes out and its
    /// triggered spawners are asked to spawn ([`alarm_spawn_choice`]), aware of `enemy`.
    AlarmSpawn { bell: usize, enemy: Option<PlayerId>, enemy_pos: Option<Vec3> },
    /// No NPC hunts the alarm's enemy any more: the bell stops (`DisSeqEvent_AlarmEnded`).
    AlarmEnded { bell: usize },
    /// A rat brain's meal ([`rats::RatEvent`]): it started eating a corpse, or ate it (no body
    /// remains) when `devoured`.
    RatEat { rat: PlayerId, corpse: PlayerId, devoured: bool },
    /// An NPC saw a corpse it had not seen before (the corpse reaction, 0x00B40740, which
    /// reports the discovery 0x00A98950: the game counts `NumCorpsesDiscovered` the first time a
    /// given corpse is discovered by anyone).
    CorpseFound { npc: PlayerId, corpse: PlayerId },
}

/// Runtime state of a `DisAlarmBell`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AlarmBell {
    pub def: AlarmBellDef,
    /// `m_bAlarmIsRinging`.
    pub ringing: bool,
    /// Seconds until the AI noise and the spawns (`m_fDelayBeforeAINoise`); <= 0 once done.
    pub noise_timer: f32,
    pub noise_done: bool,
    /// `m_fCooldownTimer` (`m_fTimeInBetweenUses`) after it stops.
    pub cooldown: f32,
    /// The guard on his way to ring it (`m_pNPCOnHisWayToRing`) and the alarm's enemy.
    pub ringer: Option<PlayerId>,
    pub enemy: Option<PlayerId>,
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
    /// Reason of the last increase (`reason::*`) and whether a danger noise was heard.
    #[serde(default)]
    pub why: u8,
    #[serde(default)]
    pub danger: bool,
}

/// A scripted shoot (`OnAIShoot` 0x00b63d00 posts stimulus 0x5c with the target, "shoot
/// forever" = `m_iMaxNumShots` < 1, the shot count and the accuracy override; the brain's
/// `DisBehaviorShoot` takes it). The behaviour stands and turns to the target (Stand
/// sub-state), then runs `DisAISubStateFirePistol` in its Kismet mode (+0xd4 bit 0x40, set up
/// by 0x00b379e0): Relaxed waits `m_TimeToFirstShot` (0 in `AI_BehaviorTweaks_Shoot`), Aiming
/// fires when the weapon is aimed, each shot counts down the shots left unless shooting
/// forever (0x00b46bf0), Recovering / Reloading return to Relaxed. The behaviour ends when the
/// count reaches 0 (slot 106 sets `m_bDone`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShootOrder {
    pub target: PlayerId,
    /// None: shoot forever.
    pub shots_left: Option<u32>,
    /// Relaxed timer (`m_TimeToFirstShot` roll).
    pub wait: f32,
    /// `m_bOverrideAccuracy` / `m_fOverrideAccuracyPercentage` (carried; NPC accuracy is not
    /// modelled).
    pub accuracy: Option<f32>,
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
    /// Kismet `DisSeqAct_AIShoot` order (`DisBehaviorShoot`, Comply slot): stand, face the
    /// target and fire the pistol.
    #[serde(default)]
    pub shoot: Option<ShootOrder>,
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
    /// `m_ActiveBehaviorStack`: one behaviour per `EAISlot`; the highest occupied one runs
    /// ([`slots`]).
    #[serde(default = "slots::routine_stack")]
    pub slots: Vec<Option<Behavior>>,
    /// `m_StimQueue`.
    #[serde(default)]
    pub stims: Vec<QueuedStim>,
    /// The attention component's top target, whether it is an enemy, and the behaviour level
    /// stepped toward its attention level (+0x44 / +0x45).
    #[serde(default)]
    pub att_target: Option<PlayerId>,
    #[serde(default)]
    pub att_enemy: bool,
    #[serde(default)]
    pub att_level: u8,
    /// The routine behaviour's sub-state (and walk goal) while another behaviour runs.
    #[serde(default = "idle_state")]
    pub routine: BrainState,
    #[serde(default)]
    pub routine_goal: Option<Vec3>,
    /// `state` as the brain left it last tick (an external write differs).
    #[serde(default = "idle_state")]
    pub shown: BrainState,
    /// Pawns this NPC treats as allies this tick (`m_bMagicUserIsAnAlly`).
    #[serde(default)]
    pub allies: Vec<PlayerId>,
    /// A magic user seen while Busted on it (or a pending non-Busted witness), the window left
    /// (+0xb0 / +0xb8 of the brain, 3 s, 0x00b23e50), and its attention level last tick.
    #[serde(default)]
    pub magic_pending: Option<(PlayerId, f32)>,
    #[serde(default)]
    pub magic_level: u8,
    /// `DisAISubProcessManageAttacks.m_fAggression` (+0x68).
    #[serde(default)]
    pub aggression: f32,
    /// The pawn's locomotion config (index into `AiConfig::locomotion`; `None`: by kind).
    #[serde(default)]
    pub loco: Option<u16>,
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
    /// The pawn's faction (index into `Crowd::factions`): the spawner's
    /// `m_pFactionTweakOverride`, else the pawn tweak's `m_pFactionTweak`. `None`: only the
    /// individual `hostile` list makes enemies.
    #[serde(default)]
    pub faction: Option<u16>,
    /// The spawner's `m_Squad` (route `m_SupportedSquads`, alarm companions).
    #[serde(default)]
    pub squad: String,
    /// Pawns this NPC treats as enemies this tick (individual, trespass and faction
    /// relationships; recomputed in `perceive`).
    #[serde(default)]
    pub enemies: Vec<PlayerId>,
    /// Reason (`reason::*`) of the stimulus that started the current investigation (barks).
    #[serde(default)]
    pub cause: u8,
    /// The stimulus was a danger noise (`EAINoiseContext_Danger` and above).
    #[serde(default)]
    pub cause_danger: bool,
    /// Bell (index into `Crowd::alarms`) this guard goes to ring, and its wait timer.
    #[serde(default)]
    pub bell: Option<usize>,
    #[serde(default)]
    pub ring_wait: f32,
    /// Enemy the alarm is about.
    #[serde(default)]
    pub alarm_enemy: Option<PlayerId>,
    /// Barks decided this tick (drained into `Crowd::events`).
    #[serde(default)]
    pub barks: Vec<(String, Option<PlayerId>)>,
    /// `DisAISubProcessAmbientBarks.m_fBarkInhibitionTimer` (+0x6c) of the running patrol /
    /// guard behaviour; None while no such behaviour runs.
    #[serde(default)]
    pub ambient_bark_timer: Option<f32>,
    /// Corpses this brain discovered by sight this tick (drained into `Crowd::events` as
    /// [`AiEvent::CorpseFound`]).
    #[serde(default)]
    pub found_corpses: Vec<PlayerId>,
    /// Time left before the first pistol shot is allowed (`m_GunUsageStart`, `m_fGunTimer`).
    #[serde(default)]
    pub gun_timer: f32,
    /// Factions this pawn individually treats as enemies (a reciprocated
    /// `DisSeqAct_SetDisposition` on the pawn).
    #[serde(default)]
    pub hostile_faction: Vec<u16>,
    /// `DisSeqAct_AISetSenses`: no hearing, no vision, no touch.
    #[serde(default)]
    pub deaf: bool,
    #[serde(default)]
    pub blind: bool,
    #[serde(default)]
    pub numb: bool,
    /// Combat move in progress (`npc_moves`; 0 none), its time left, and the dodge direction.
    #[serde(default)]
    pub attack: u8,
    #[serde(default)]
    pub attack_timer: f32,
    #[serde(default)]
    pub dodge_dir: i8,
    /// `DisAISubProcessManageAttacks`: `m_iFlurryAttacksRemaining`, `m_fStaminaRecoveryTimer`,
    /// incoming attacks seen (`m_iIncomingAttacksBeforeBackStep` countdown) and their
    /// forgiveness timer, time a ranged threat has been aimed at this NPC (`m_fSideStepTimer`).
    #[serde(default)]
    pub flurry: i32,
    #[serde(default)]
    pub stamina_timer: f32,
    #[serde(default)]
    pub incoming: i32,
    #[serde(default)]
    pub incoming_needed: i32,
    #[serde(default)]
    pub forgive_timer: f32,
    #[serde(default)]
    pub threat_timer: f32,
    #[serde(default)]
    pub threat_needed: f32,
    /// The target's sword phase last tick (incoming attack edge).
    #[serde(default)]
    pub target_swinging: bool,
    /// `DisAISubStateFirePistol`: in the sub-state, `m_fFireTimer`, `m_fGiveUpTimer`.
    #[serde(default)]
    pub firing: bool,
    #[serde(default)]
    pub fire_timer: f32,
    #[serde(default)]
    pub give_up: f32,
    /// The combat behaviour has started (its timers are rolled once per fight).
    #[serde(default)]
    pub in_fight: bool,
    /// The pawn's NPC class (its brain tweak's Combat-slot behaviour, [`NpcClass`]): which
    /// combat behaviour runs in the Combat slot.
    #[serde(default)]
    pub class: NpcClass,
    /// State of the special combat behaviours ([`special`]).
    #[serde(default)]
    pub special: SpecialCombat,
    /// The look-at head and the vision eye ([`head`]).
    #[serde(default)]
    pub head: head::Head,
}

fn idle_state() -> BrainState {
    BrainState::Idle
}


/// What the brains of a crowd know about pawn relationships this tick.
#[derive(Debug, Clone, Default)]
pub struct Social {
    pub factions: Option<FactionTable>,
    /// Faction of every pawn that has one (players: the player faction).
    pub pawn_faction: BTreeMap<PlayerId, u16>,
    /// (trespassing pawn, owning faction of the forbidden zone it is in).
    pub trespass: Vec<(PlayerId, u16)>,
    /// `DisSeqAct_SetDisposition` overrides: (from faction, recipient, relationship).
    pub overrides: Vec<(u16, Recipient, Relationship)>,
}

/// A `DisSeqAct_SetDisposition` recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recipient {
    Faction(u16),
    Pawn(PlayerId),
}

impl Social {
    /// Relationship of faction `from` toward pawn `to` (0x00c63070): a trespasser in a zone
    /// owned by `from` is an enemy (0x00c5fc20); then the disposition overrides for the pawn,
    /// then for its faction (0x00c5fca0); then the faction lists (0x00c5fd00).
    pub fn faction_relation(&self, from: u16, to: PlayerId) -> Relationship {
        let Some(t) = self.factions.as_ref() else { return Relationship::Neutral };
        if self.trespass.iter().any(|&(p, f)| p == to && t.isa(from, f)) {
            return Relationship::Enemy;
        }
        let theirs = self.pawn_faction.get(&to).copied();
        if let Some(r) = self.overrides.iter().rev().find(|o| t.isa(from, o.0) && o.1 == Recipient::Pawn(to)).map(|o| o.2) {
            return r;
        }
        if let Some(tf) = theirs {
            if let Some(r) = self.overrides.iter().rev().find(|o| t.isa(from, o.0) && matches!(o.1, Recipient::Faction(f) if t.isa(tf, f))).map(|o| o.2) {
                return r;
            }
            return t.relation(from, tf);
        }
        Relationship::Neutral
    }
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
    /// `DishonoredRoute.m_NeglectTimer` per route: seconds without an adopter.
    #[serde(default)]
    pub route_neglect: Vec<f32>,
    /// The level's factions and each pawn's faction (players default to `player_faction`).
    #[serde(default)]
    pub factions: FactionTable,
    #[serde(default)]
    pub pawn_faction: BTreeMap<PlayerId, u16>,
    #[serde(default)]
    pub player_faction: Option<u16>,
    /// Pawn tweak -> faction name (`m_pFactionTweak`), for spawned NPCs.
    #[serde(default)]
    pub tweak_factions: BTreeMap<String, String>,
    #[serde(default)]
    pub zones: Vec<ForbiddenZone>,
    /// `DisSeqAct_SetDisposition` overrides in application order.
    #[serde(default)]
    pub overrides: Vec<(u16, Recipient, Relationship)>,
    #[serde(default)]
    pub alarms: Vec<AlarmBell>,
    /// `DisAlarmGlobalManager.m_fAlarmCooldownTimer` (`m_fAlarmCooldown` after an alarm ends).
    #[serde(default)]
    pub alarm_cooldown: f32,
    /// Alarm AI noises from last tick: (bell position, radius).
    #[serde(default)]
    pub alarm_noise: Vec<(Vec3, f32)>,
    /// Lights the light probe samples (empty: `LIGHT_LEVEL`).
    #[serde(default)]
    pub lights: Vec<ProbeLight>,
    /// Events of the last `think` (barks, alarms).
    #[serde(default)]
    pub events: Vec<AiEvent>,
    /// The level's audio cells (hearing between cells, [`hearing`]).
    #[serde(default)]
    pub audio: dishonored_ir::ai::AudioCells,
    /// Ambient (sky light) brightness added to the light probe.
    #[serde(default)]
    pub sky_light: f32,
    /// The global player light normalization (`Twk_GlobalPlayerVisSettings`), kept while a
    /// level's own range (`AiLevel::player_light_range`) is installed in `ai`.
    #[serde(default)]
    pub global_light_range: Option<[f32; 2]>,
    /// The level's `DisStealthVolume`s (their light range replaces the map's inside them).
    #[serde(default)]
    pub stealth_volumes: Vec<dishonored_ir::ai::StealthVolume>,
    /// The stealth light environment's lights and the levels' light volumes (`AiLevel`).
    #[serde(default)]
    pub stealth_lights: Vec<ProbeLight>,
    #[serde(default)]
    pub light_volumes: Vec<dishonored_ir::ai::LightVolume>,
    /// Per (player, stealth light): the last visibility traces (bit mask over the samples, next
    /// sample, update stamp; 0x004FF270) and the update counter.
    #[serde(skip)]
    pub light_vis: BTreeMap<(PlayerId, usize), (u8, u8, u64)>,
    #[serde(skip)]
    pub light_updates: u64,
    /// The light at each player from the last `think` (raw, normalized).
    #[serde(skip)]
    pub player_lights: BTreeMap<PlayerId, (f32, f32)>,
    /// Brains of the rat pawns (`PawnKind::Rat`) and the swarm each rat was spawned in
    /// (`SimEvent::RatSpawned`).
    #[serde(default)]
    pub rat_brains: Vec<rats::RatSwarm>,
    #[serde(default)]
    pub rat_swarm: BTreeMap<PlayerId, Option<u32>>,
    /// Bend Time casters whose bend stops time (for the witness mode of its end).
    #[serde(default)]
    pub bend_stop: BTreeMap<PlayerId, bool>,
}

/// `DisGlobalManager` alarm cooldown (`DefaultLevel.ini m_fAlarmCooldown`, class default 10).
pub const ALARM_COOLDOWN: f32 = 10.0;

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

/// The raw light normalized between a range's darkest and brightest values (0x00BC9040): 0.5
/// for an empty range, else clamped to [0, 1].
pub fn light01(light: f32, [dark, bright]: [f32; 2]) -> f32 {
    let span = bright - dark;
    if span == 0.0 {
        return 0.5;
    }
    ((light - dark) / span).clamp(0.0, 1.0)
}

/// The player's movement state as the visibility component reads it (0x00AD1590).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VisState {
    /// The swim state: `Some(true)` with the head under water (`m_pActiveWaterVolume` set),
    /// `Some(false)` at the surface (0x00AAB9D0: 2 / 1).
    pub swimming: Option<bool>,
    /// In `StatePlayerMasterSlide`.
    pub sliding: bool,
    /// `Pawn.bIsCrouched`: the engine crouch (the crawl, forced by a low ceiling), not the sneak.
    pub autocrouch: bool,
    /// `m_bSneakModeToggled` (pawn vslot 415).
    pub sneaking: bool,
    /// `m_bSprinting`.
    pub sprinting: bool,
}

/// The visibility component's view of a pawn: the swim state from the sim's swimming mode and
/// head-under flag, the slide, the sneak (the sim's `crouched` is `m_bSneakModeToggled`) and
/// `m_bSprinting`. The sim has no engine crouch (`bIsCrouched`, the crawl), so auto-crouching
/// never applies (stealth.md F7).
pub fn vis_state(p: &PlayerState) -> VisState {
    VisState {
        swimming: (p.mode == dishonored_sim::state::MoveMode::Swimming).then_some(p.water.under),
        sliding: p.slide.active,
        autocrouch: false,
        sneaking: p.crouched,
        sprinting: p.sprinting,
    }
}

/// The player's `Attribute_Visibility` (`DishonoredVisibilityComponent`, 0x00AD1590): the
/// light tuning of the normalized light is the operand of the speed tuning of the speed value,
/// plus the bonus of the movement state.
///
/// The speed value is forced to 0 (`m_bForceZeroSpeedWhileSneaking`) while sneaking or
/// auto-crouching, unless swimming. The bonus is chosen in this order: swimming (under water /
/// at the surface), sliding, auto-crouching, sneaking, standing (speed below 1e-4), walking
/// (not `m_bSprinting`), sprinting. (`m_fSneakToStandBonusDelayTime` would keep the sneaking
/// bonus for a while after standing up; it is 0 in the shipped tweak.)
pub fn player_visibility(pv: &PlayerVisibilityConfig, light01: f32, speed01: f32, s: VisState) -> f32 {
    let zero = (s.sneaking || s.autocrouch) && s.swimming.is_none();
    let speed = if zero && pv.zero_speed_sneaking { 0.0 } else { speed01 };
    let base = pv.speed.apply(speed, pv.light.value(light01));
    let bonus = match s.swimming {
        Some(true) => pv.bonus_swimming,
        Some(false) => pv.bonus_swimming_surface,
        None if s.sliding => pv.bonus_sliding,
        None if s.autocrouch => pv.bonus_autocrouching,
        None if s.sneaking => pv.bonus_sneaking,
        None if speed01 < 1e-4 => pv.bonus_standing,
        None if !s.sprinting => pv.bonus_walking,
        None => pv.bonus_sprinting,
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
    neglect: &'a [f32],
    inhibit: &'a mut Vec<(PlayerId, u8, f32)>,
    alarms: &'a mut [AlarmBell],
    events: &'a mut Vec<AiEvent>,
    /// Global alarm cooldown left.
    cooldown: f32,
}

/// What a brain perceives this tick besides the world state.
pub struct Senses<'a> {
    peers: &'a [Peer],
    barks: &'a [(Vec3, f32)],
    busted: &'a [(PlayerId, PawnKind, Vec3, Vec3)],
    alarm_noise: &'a [(Vec3, f32)],
    social: &'a Social,
    /// Normalized light value (0..1) at each player.
    light: &'a BTreeMap<PlayerId, f32>,
    audio: &'a dishonored_ir::ai::AudioCells,
    /// Powers used this tick: (user, `MagicMode` 0 / 1 / 2 of 0x00b23e50).
    magic: &'a [(PlayerId, u8)],
}

/// The powers of last tick's events as magic witness modes (0x00bf9fb0 callers): Windblast
/// (vslot 90) and Devouring Swarm casts and the end of a slowing Bend Time (0x00bfa200) are 0;
/// a Blink's end (0x00bfab80), possession ending (the inactive stage 0x00bfb320) and the end of
/// a stopping Bend Time are 1; a Blink's start (0x00bfc990), possession taking a host
/// (0x00bfd300) and Bend Time stopping time (0x00bfa180, `m_fTimeScale` < 1e-8) are 2.
fn magic_modes(events: &[SimEvent], ws: &WorldState, stopped: &mut BTreeMap<PlayerId, bool>) -> Vec<(PlayerId, u8)> {
    let mut out = Vec::new();
    for e in events {
        match e {
            SimEvent::WindblastCast { id } => out.push((*id, 0)),
            SimEvent::SwarmSummoned { id, .. } => out.push((*id, 0)),
            SimEvent::BlinkStart { id, .. } => out.push((*id, 2)),
            SimEvent::BlinkEnd { id } => out.push((*id, 1)),
            SimEvent::PossessStart { id, .. } => out.push((*id, 2)),
            SimEvent::PossessEnd { id, .. } => out.push((*id, 1)),
            SimEvent::BendTimeStart { id, .. } => {
                let stop = ws.time_bends.iter().any(|b| b.caster == *id && b.world_dilation < 1e-8);
                stopped.insert(*id, stop);
                if stop {
                    out.push((*id, 2));
                }
            }
            SimEvent::BendTimeEnd { id } => {
                let stop = stopped.remove(id).unwrap_or(false);
                out.push((*id, if stop { 1 } else { 0 }));
            }
            _ => {}
        }
    }
    out
}

/// Raw light value at `p` from the level's lights (the original reads the pawn's
/// `DynamicLightEnvironment`: its static and dynamic spherical-harmonic light, reduced to a
/// brightness and raised to 1/2.2, 0x004e1db0). Deterministic stand-in: the sum over lights of
/// `intensity * (1 - (d/r)^2)^falloff` (spot lights with their cone), counting a shadowing
/// light only when nothing blocks the segment to `p`; then `^(1/2.2)`.
pub fn light_at(lights: &[ProbeLight], p: Vec3, world: &dyn WorldQuery) -> f32 {
    light_at_with(lights, 0.0, p, world)
}

/// [`light_at`] plus an ambient brightness (the level's sky lights, which the light environment
/// adds as a hemisphere term).
pub fn light_at_with(lights: &[ProbeLight], ambient: f32, p: Vec3, world: &dyn WorldQuery) -> f32 {
    let mut sum = ambient;
    for l in lights {
        let v = match l.kind {
            2 => {
                let dir = l.direction.normalize_or(Vec3::NEG_Z);
                if l.casts_shadows && world.raycast(p + Vec3::Z * 20.0, -dir, 50000.0, channel::VISION).is_some() {
                    0.0
                } else {
                    l.intensity
                }
            }
            _ => {
                let d = p.distance(l.position);
                if d >= l.radius || l.radius <= 0.0 {
                    continue;
                }
                let mut a = (1.0 - (d / l.radius).powi(2)).max(0.0).powf(l.falloff_exponent.max(0.0));
                if l.kind == 1 {
                    let c = (p - l.position).normalize_or_zero().dot(l.direction.normalize_or(Vec3::NEG_Z));
                    let (ci, co) = (l.inner_cone.to_radians().cos(), l.outer_cone.to_radians().cos());
                    a *= if ci - co > 1e-4 { ((c - co) / (ci - co)).clamp(0.0, 1.0) } else if c >= co { 1.0 } else { 0.0 };
                }
                if a <= 0.0 {
                    continue;
                }
                if l.casts_shadows {
                    let to = p - l.position;
                    let dd = to.length();
                    if dd > 1.0 && world.raycast(l.position, to / dd, dd - 30.0, channel::VISION).is_some() {
                        continue;
                    }
                }
                l.intensity * a
            }
        };
        sum += v;
    }
    if sum > 1e-4 { sum.powf(1.0 / 2.2) } else { 0.0 }
}

/// Which of a bell's triggered spawners spawn (0x00a36dd0): those at least
/// `m_fSpawnerMinDistFromPlayer` from the enemy, or all when none is; at most
/// `m_MaxSpawnedPawnsPerRing` (negative: no limit). `spawners`: (name, position).
pub fn alarm_spawn_choice(def: &AlarmBellDef, spawners: &[(String, Vec3)], enemy_pos: Option<Vec3>) -> Vec<String> {
    let listed: Vec<&(String, Vec3)> = def.spawners.iter().filter_map(|n| spawners.iter().find(|s| s.0.eq_ignore_ascii_case(n))).collect();
    let far: Vec<&(String, Vec3)> = match enemy_pos {
        Some(e) => listed.iter().copied().filter(|s| s.1.distance(e) > def.spawner_min_dist).collect(),
        None => listed.clone(),
    };
    let pick = if far.is_empty() { listed } else { far };
    let n = if def.max_spawns < 0 { pick.len() } else { (def.max_spawns as usize).min(pick.len()) };
    pick.into_iter().take(n).map(|s| s.0.clone()).collect()
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

    /// A player pawn was replaced (a multiplayer respawn reuses the player id): the original
    /// keeps individual relationships and attention per pawn actor (`m_Response_*` hostility,
    /// the attention entries), and a new pawn starts with none of them. Combat against the old
    /// pawn then ends through the usual Busted test (no entry left: search).
    pub fn forget(&mut self, who: PlayerId) {
        for b in self.brains.iter_mut() {
            b.hostile.retain(|h| *h != who);
            b.enemies.retain(|h| *h != who);
            b.attn.retain(|a| a.who != who);
        }
        self.inhibit.retain(|i| i.0 != who);
    }

    /// Sleep-dart hook (`m_bInstantSleepForUnawareNPCs`): is NPC `npc` unaware of `who`?
    /// Pawns without a brain count as unaware.
    pub fn unaware_of(&self, npc: PlayerId, who: PlayerId) -> bool {
        self.brains.iter().find(|b| b.id == npc).is_none_or(|b| b.unaware_of(who))
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
        self.events.clear();
        // Route neglect timers (`m_NeglectTimer`: grows while nobody walks the route).
        self.route_users.resize(self.routes.len(), 0);
        self.route_neglect.resize(self.routes.len(), 0.0);
        for (t, u) in self.route_neglect.iter_mut().zip(self.route_users.iter()) {
            *t = if *u > 0 { 0.0 } else { *t + dt };
        }
        let alarm_noise = std::mem::take(&mut self.alarm_noise);
        let social = self.social(world_state);
        // The light at each player (the original samples the pawn's light environment),
        // normalized by the range of the stealth volume the player is in, else the map's
        // (`DishonoredVisibilityComponent` tick 0x00AD51E0: 0x00BD0120, 0x00BC9040).
        self.light_updates += 1;
        let humans: Vec<PlayerState> = world_state.players.iter().filter(|p| p.kind == PawnKind::Human && p.alive).copied().collect();
        let light: BTreeMap<PlayerId, f32> = humans.iter().map(|p| (p.id, self.update_player_light(p, world).1)).collect();
        let magic = magic_modes(events, world_state, &mut self.bend_stop);
        let senses = Senses { peers: &peers, barks: &barks, busted: &busted, alarm_noise: &alarm_noise, social: &social, light: &light, audio: &self.audio, magic: &magic };
        let layout = &self.layout;
        let ai = &self.ai;
        let routes = &self.routes;
        let users = &mut self.route_users;
        let neglect = &self.route_neglect;
        let inhibit = &mut self.inhibit;
        let alarms = &mut self.alarms;
        let mut out = Vec::with_capacity(self.brains.len());
        let mut ai_events = Vec::new();
        let cooldown = self.alarm_cooldown;
        for b in self.brains.iter_mut() {
            let Some(me) = world_state.player(b.id) else {
                continue;
            };
            if !me.alive {
                continue;
            }
            let was = b.state;
            b.sync_external();
            b.perceive_with(me, world_state, events, &senses, world, ai, dt);
            let mut ctx = Ctx { layout, routes, users, neglect, inhibit, alarms: &mut *alarms, events: &mut ai_events, cooldown };
            let mut inp = b.act(me, world_state, &mut ctx, world, ai, dt);
            b.step_head(me, world_state, ai, dt);
            if inp.npc_move == 0 {
                inp.npc_move = b.attack_kind();
            }
            out.push((b.id, inp));
            b.transition_barks(was, me, world_state, &peers, ai);
            b.ambient_barks(ai, dt);
            for (hook, initiator) in b.barks.drain(..) {
                ai_events.push(AiEvent::Bark { npc: b.id, hook, initiator });
            }
            for corpse in b.found_corpses.drain(..) {
                ai_events.push(AiEvent::CorpseFound { npc: b.id, corpse });
            }
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
        self.events = ai_events;
        self.tick_alarms(world_state, dt);
        out.extend(self.think_rats(world_state, events, world, dt));
        out
    }

    /// The rat pawns' brains ([`rats::RatSwarm`]). A rat's swarm comes from its
    /// `SimEvent::RatSpawned`; rats of a Devouring Swarm (`WorldState::swarms`) use that power's
    /// swarm tweak and spare its caster, others the level swarm tweak. Possessed rats are driven
    /// by their possessor.
    fn think_rats(&mut self, ws: &WorldState, events: &[SimEvent], world: &dyn WorldQuery, dt: f32) -> Vec<(PlayerId, PlayerInput)> {
        for e in events {
            if let SimEvent::RatSpawned { rat, swarm } = e {
                self.rat_swarm.insert(*rat, *swarm);
                self.rat_brains.retain(|b| b.id != *rat);
            }
        }
        let alive: Vec<&PlayerState> = ws.players.iter().filter(|p| p.kind == PawnKind::Rat && p.alive).collect();
        self.rat_brains.retain(|b| alive.iter().any(|p| p.id == b.id));
        self.rat_swarm.retain(|id, _| ws.player(*id).is_some());
        let mut out = Vec::new();
        for p in alive {
            let possessed = ws.players.iter().any(|q| q.possess.host == Some(p.id) && matches!(q.possess.stage, dishonored_sim::state::PossessStage::PrePossess | dishonored_sim::state::PossessStage::Active));
            if possessed {
                continue;
            }
            let swarm = self.rat_swarm.get(&p.id).copied().flatten().or(p.swarm);
            let power = swarm.and_then(|s| ws.swarms.iter().find(|x| x.id == s));
            // An ended Devouring Swarm: its rats stop (they despawn, `dishonored_sim::powers::step_swarms`).
            if power.is_some_and(|x| x.ending) {
                out.push((p.id, PlayerInput { yaw: p.yaw, left_hand: LeftHand::Empty, ..Default::default() }));
                continue;
            }
            let count = match swarm {
                Some(s) => ws.players.iter().filter(|q| q.kind == PawnKind::Rat && q.alive && self.rat_swarm.get(&q.id).copied().flatten() == Some(s)).count() as u32 + power.map_or(0, |x| x.rats as u32),
                None => 1,
            };
            if !self.rat_brains.iter().any(|b| b.id == p.id) {
                let cfg = if power.is_some() { self.ai.devouring_swarm.clone() } else { self.ai.rats.clone() };
                self.rat_brains.push(rats::RatSwarm::new(p.id, cfg, count, power.map(|x| x.owner), p.id as u64 * 2654435761));
            }
            let Some(b) = self.rat_brains.iter_mut().find(|b| b.id == p.id) else { continue };
            b.rats = count;
            let (input, evs) = b.think(p, ws, world, dt);
            for e in evs {
                match e {
                    rats::RatEvent::EatStart { swarm, corpse } => self.events.push(AiEvent::RatEat { rat: swarm, corpse, devoured: false }),
                    rats::RatEvent::Devoured { swarm, corpse } => self.events.push(AiEvent::RatEat { rat: swarm, corpse, devoured: true }),
                }
            }
            out.push((p.id, input));
        }
        out
    }

    /// Write `PlayerState::ai_unaware` (sleep darts' instant sleep and Shadow Kill level 1 read
    /// it, see [`Brain::unaware_of`]) and `PlayerState::ai_awareness` (the assassination gate, see
    /// [`Brain::awareness`]) for every NPC with a brain.
    pub fn write_awareness(&self, ws: &mut WorldState) {
        for b in &self.brains {
            if let Some(p) = ws.player_mut(b.id) {
                p.ai_unaware = b.unaware_of(NOISE);
                p.ai_awareness = b.awareness();
            }
        }
        // `PlayerState::combat_opponents`: the pawn's skirmish size (the `DisGlobalCombatManager`
        // skirmish of a target holds the brains in combat against it, 0x00C5AF60); Dark Vision
        // reads it (0x00BF84C0). LIKELY: the port's skirmish is the set of Combat brains on that
        // target (`assign_skirmish`).
        for p in ws.players.iter_mut() {
            let n = self.brains.iter().filter(|b| b.state == BrainState::Combat && b.target == Some(p.id)).count();
            p.combat_opponents = n.min(u8::MAX as usize) as u8;
        }
    }

    /// Alarm bells after the brains acted (`DisAlarmBell` tick 0x00a3d0c0): a ringing bell
    /// sends its AI noise and asks its spawners after `m_fAlarmRingDelayBeforeAINoise`
    /// (0x00a36dd0); it stops once no NPC hunts its enemy (0x00a37410), and the global
    /// cooldown starts when no bell rings.
    fn tick_alarms(&mut self, ws: &WorldState, dt: f32) {
        self.alarm_cooldown = (self.alarm_cooldown - dt).max(0.0);
        for i in 0..self.alarms.len() {
            let a = &mut self.alarms[i];
            a.cooldown = (a.cooldown - dt).max(0.0);
            if !a.ringing {
                continue;
            }
            if !a.noise_done {
                a.noise_timer -= dt;
                if a.noise_timer <= 0.0 {
                    a.noise_done = true;
                    self.alarm_noise.push((a.def.position, a.def.loudness));
                    let enemy_pos = a.enemy.and_then(|e| ws.player(e)).map(|p| p.position);
                    self.events.push(AiEvent::AlarmSpawn { bell: i, enemy: a.enemy, enemy_pos });
                }
                continue;
            }
            let hunted = a.enemy.is_some_and(|e| ws.player(e).is_some_and(|p| p.alive) && self.brains.iter().any(|b| b.target == Some(e) || b.attn.iter().any(|x| x.who == e && x.level >= DAL_BUSTED)));
            if !hunted {
                let a = &mut self.alarms[i];
                a.ringing = false;
                a.ringer = None;
                a.cooldown = a.def.time_between_uses;
                self.events.push(AiEvent::AlarmEnded { bell: i });
                if !self.alarms.iter().any(|x| x.ringing) {
                    self.alarm_cooldown = ALARM_COOLDOWN;
                }
            }
        }
    }

    /// Relationship data of this tick: every pawn's faction and the trespassers of enabled
    /// forbidden zones (0x00c953f0: a pawn of a forbidden faction inside the zone trespasses
    /// against each owning faction).
    pub fn social(&self, ws: &WorldState) -> Social {
        let mut pawn_faction = BTreeMap::new();
        for p in &ws.players {
            let f = self.pawn_faction.get(&p.id).copied().or_else(|| self.brains.iter().find(|b| b.id == p.id).and_then(|b| b.faction)).or(if p.kind == PawnKind::Human { self.player_faction } else { None });
            if let Some(f) = f {
                pawn_faction.insert(p.id, f);
            }
        }
        let mut trespass = Vec::new();
        for z in self.zones.iter().filter(|z| z.enabled) {
            let forbidden: Vec<u16> = z.forbidden.iter().filter_map(|n| self.factions.index(n)).collect();
            let owners: Vec<u16> = z.owning.iter().filter_map(|n| self.factions.index(n)).collect();
            for p in ws.players.iter().filter(|p| p.alive) {
                let Some(&f) = pawn_faction.get(&p.id) else { continue };
                if forbidden.iter().any(|&x| self.factions.isa(f, x)) && z.contains(p.position) {
                    for &o in &owners {
                        if !trespass.contains(&(p.id, o)) {
                            trespass.push((p.id, o));
                        }
                    }
                }
            }
        }
        Social { factions: Some(self.factions.clone()), pawn_faction, trespass, overrides: self.overrides.clone() }
    }

    /// The light at a player at `p`: the raw value of the light probe and its normalization by
    /// the stealth volume's range there, else the map's (`DishonoredVisibilityComponent` tick
    /// 0x00AD51E0: `m_fCurrentLightValue` / `_Normalized`; 0x00BD0120, 0x00BC9040).
    ///
    /// The raw value is the player's stealth light environment (`m_pStealthLightingDLE`, updated
    /// in full every tick, `bRequiresNonLatentUpdates`) at the collision cylinder's bounds: the
    /// `m_LightProbe` lights plus the floor level's precomputed light volume
    /// ([`dishonored_ir::ai::stealth_light_value`], stealth.md section 11). Levels without that
    /// data (none loaded) keep the earlier probe over the level's light actors.
    pub fn update_player_light(&mut self, p: &PlayerState, world: &dyn WorldQuery) -> (f32, f32) {
        let pv = &self.ai.player_visibility;
        let origin = p.position;
        let raw = if !self.stealth_lights.is_empty() || !self.light_volumes.is_empty() {
            let [r, hh, sneak] = pv.cylinder;
            let h = if p.crouched { sneak } else { hh };
            let extent = Vec3::new(r, r, h);
            let sphere = extent.length();
            // The floor's level selects the light volume (a line check down from the origin).
            let floor = world.hit_package(origin, -Vec3::Z, 262_144.0);
            let volume = floor.as_deref().and_then(|f| self.light_volumes.iter().find(|v| v.package.eq_ignore_ascii_case(f)));
            // `Default__DishonoredPlayerPawn.StealthLightingDLE.NumVolumeVisibilitySamples` 4:
            // each update traces one sample per light, cycling; the light's visibility is the
            // share of clear traces over the last 4 (reset when the light was not traced the
            // update before).
            const SAMPLES: usize = 4;
            let points = dishonored_ir::ai::visibility_sample_points(SAMPLES);
            if std::env::var_os("DISHONORED_STEALTH_LIGHT_DEBUG").is_some() {
                eprintln!("stealth light at {origin:.0} floor {floor:?} volume {:?} band0 {:.4}", volume.map(|v| &v.package), volume.map_or(0.0, |v| v.band0_at(origin)));
                for l in self.stealth_lights.iter().filter(|l| l.affects(origin, sphere)) {
                    eprintln!("  kind {} at {:.0} intensity {:.4} -> {:.4} shadows {}", l.kind, l.position, l.intensity, l.intensity_at(origin), l.casts_shadows);
                }
            }
            let stamp = self.light_updates;
            let vis = &mut self.light_vis;
            let id = p.id;
            dishonored_ir::ai::stealth_light_value(&self.stealth_lights, volume, origin, sphere, |i, l| {
                let e = vis.entry((id, i)).or_insert((0, 0, stamp));
                if e.2 + 1 != stamp {
                    *e = (0, 0, stamp);
                }
                let k = e.1 as usize % SAMPLES;
                let (a, b) = dishonored_ir::ai::visibility_segment(l, origin, sphere, extent, points[k]);
                let d = b - a;
                let len = d.length();
                let clear = len < 1e-3 || world.raycast(a, d / len, len, channel::ALL).is_none();
                e.0 = if clear { e.0 | 1 << k } else { e.0 & !(1 << k) };
                e.1 = ((k + 1) % SAMPLES) as u8;
                e.2 = stamp;
                e.0.count_ones() as f32 / SAMPLES as f32
            })
        } else if self.lights.is_empty() && self.sky_light <= 0.0 {
            LIGHT_LEVEL
        } else {
            light_at_with(&self.lights, self.sky_light, origin, world)
        };
        let pv = &self.ai.player_visibility;
        let range = dishonored_ir::ai::stealth_light_range(&self.stealth_volumes, origin).unwrap_or([pv.light_darkest, pv.light_brightest]);
        let v = (raw, light01(raw, range));
        self.player_lights.insert(p.id, v);
        v
    }
    /// Install the level's AI data (factions, forbidden zones, alarm bells, probe lights).
    pub fn set_level(&mut self, level: &AiLevel) {
        self.factions = level.factions.clone();
        self.player_faction = Some(self.factions.intern(&level.player_faction));
        self.tweak_factions = level.tweak_factions.clone();
        self.zones = level.zones.clone();
        self.alarms = level.alarms.iter().map(|d| AlarmBell { def: d.clone(), ..Default::default() }).collect();
        self.lights = level.lights.clone();
        self.audio = level.audio.clone();
        self.sky_light = level.sky_light;
        // The map's player light normalization replaces the global one (0x00BC8E60).
        let pv = &mut self.ai.player_visibility;
        let global = *self.global_light_range.get_or_insert([pv.light_darkest, pv.light_brightest]);
        let [dark, bright] = level.player_light_range.unwrap_or(global);
        pv.light_darkest = dark;
        pv.light_brightest = bright;
        self.stealth_volumes = level.stealth_volumes.clone();
        self.stealth_lights = level.stealth_lights.clone();
        self.light_volumes = level.light_volumes.clone();
    }

    /// Faction of a spawned NPC: the spawner's `m_pFactionTweakOverride`, else its pawn
    /// tweak's `m_pFactionTweak` (`DisSpawnerInfo`).
    pub fn spawner_faction(&mut self, override_faction: &str, tweak: &str) -> Option<u16> {
        let name = if !override_faction.is_empty() { Some(override_faction.to_string()) } else { self.tweak_factions.get(tweak).cloned() };
        name.map(|n| self.factions.intern(&n))
    }

    /// Give the crowd's own guards and civilians the factions of the original spawners they
    /// stand in for: a guard takes the faction of the guard spawner at its post; civilians take
    /// the most common civilian spawner faction.
    pub fn assign_crowd_factions(&mut self, spawners: &[dishonored_ir::spawners::Spawner]) {
        let mut civ: BTreeMap<String, usize> = BTreeMap::new();
        // Civilian pawn tweaks only (weepers and other non-guards are not crowd civilians).
        let civilian = |t: &str| {
            let l = t.to_ascii_lowercase();
            ["aristo", "party", "civ", "guest", "servant", "maid", "noble"].iter().any(|k| l.contains(k)) && !l.contains("guard") && !l.contains("overseer") && !l.contains("weep")
        };
        for s in spawners.iter().filter(|s| !s.guard && s.props.on_begin_play && !s.props.spawn_dead && civilian(&s.tweak)) {
            let n = if s.props.faction.is_empty() { self.tweak_factions.get(&s.tweak).cloned().unwrap_or_default() } else { s.props.faction.clone() };
            if !n.is_empty() {
                *civ.entry(n).or_default() += 1;
            }
        }
        let civ_faction = civ.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(n, _)| n.clone()).unwrap_or_else(|| "DisFaction_Defaults.Faction_Civilian_Default".into());
        let civ_idx = self.factions.intern(&civ_faction);
        let guards: Vec<(Vec3, String, String)> = spawners.iter().filter(|s| s.guard).map(|s| (s.position, s.props.faction.clone(), s.tweak.clone())).collect();
        for i in 0..self.brains.len() {
            if self.brains[i].faction.is_some() {
                continue;
            }
            let f = if self.brains[i].kind == PawnKind::Guard {
                let home = self.brains[i].home;
                let near = guards.iter().filter(|g| g.0.truncate().distance(home.truncate()) < 300.0).min_by(|a, b| a.0.distance(home).total_cmp(&b.0.distance(home)));
                match near {
                    Some((_, o, t)) => self.spawner_faction(&o.clone(), &t.clone()),
                    None => Some(self.factions.intern("DisFaction_Defaults.Faction_Guard_Default")),
                }
            } else {
                Some(civ_idx)
            };
            self.brains[i].faction = f;
        }
    }

    /// `DisSeqAct_SetDisposition` on a pawn (`ADishonoredPawn::OnSetDisposition`): its own
    /// relationships toward pawns or factions (only Enemy changes what this port's brains do).
    pub fn set_pawn_disposition(&mut self, pawn: PlayerId, input: u8, rel: Relationship, recipients: &[Recipient]) {
        let Some(b) = self.brains.iter_mut().find(|b| b.id == pawn) else { return };
        if input == 2 {
            b.hostile.clear();
            b.hostile_faction.clear();
            return;
        }
        for &r in recipients {
            match r {
                Recipient::Pawn(p) => {
                    b.hostile.retain(|x| *x != p);
                    if input == 0 && rel == Relationship::Enemy {
                        b.make_hostile(p);
                    }
                }
                Recipient::Faction(f) => {
                    b.hostile_faction.retain(|x| *x != f);
                    if input == 0 && rel == Relationship::Enemy {
                        b.hostile_faction.push(f);
                    }
                }
            }
        }
    }

    /// `DisSeqAct_SetDisposition` (`DisTweaks_Faction::OnSetDisposition`, 0x00c79000): Set
    /// records `faction -> recipient` (and back, with `m_bReciprocate`), Clear removes them,
    /// ClearAll removes every override of the faction.
    pub fn set_disposition(&mut self, faction: &str, input: u8, rel: Relationship, recipients: &[Recipient], reciprocate: bool) {
        let f = self.factions.intern(faction);
        match input {
            0 => {
                for &r in recipients {
                    self.overrides.retain(|o| !(o.0 == f && o.1 == r));
                    self.overrides.push((f, r, rel));
                    if reciprocate {
                        match r {
                            Recipient::Faction(g) => {
                                self.overrides.retain(|o| !(o.0 == g && o.1 == Recipient::Faction(f)));
                                self.overrides.push((g, Recipient::Faction(f), rel));
                            }
                            Recipient::Pawn(p) => {
                                // The pawn's own relationship toward the faction (0x00b04430).
                                if let Some(b) = self.brains.iter_mut().find(|b| b.id == p) {
                                    if rel == Relationship::Enemy {
                                        b.hostile_faction.retain(|x| *x != f);
                                        b.hostile_faction.push(f);
                                    } else {
                                        b.hostile_faction.retain(|x| *x != f);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            1 => {
                for &r in recipients {
                    self.overrides.retain(|o| !(o.0 == f && o.1 == r));
                    if reciprocate {
                        if let Recipient::Faction(g) = r {
                            self.overrides.retain(|o| !(o.0 == g && o.1 == Recipient::Faction(f)));
                        }
                    }
                }
            }
            _ => {
                self.overrides.retain(|o| o.0 != f);
                if reciprocate {
                    for &r in recipients {
                        if let Recipient::Faction(g) = r {
                            self.overrides.retain(|o| !(o.0 == g && o.1 == Recipient::Faction(f)));
                        }
                    }
                }
            }
        }
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
            if b.adopt_route(routes, users, &[], pos, None) {
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
                *b = Brain { route: b.route.clone(), patrol: b.patrol, route_i: b.route_i, faction: b.faction, squad: b.squad.clone(), ..Brain::new(s.id, s.kind, s.position, b.rng) };
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

/// The attention increase a heard noise asks for, by its context: the attention process's
/// noise handler (0x00B40B70, reached from the brain's noise message 0x00B43E10) takes
/// Anomaly and Footfall -> `m_IncreaseForAnomalySound`, Threatening and WolZap ->
/// `m_IncreaseForThreateningSound`, Danger, RatSwarm and WatchTowerAttack ->
/// `m_IncreaseForDangerSound`, DeathRattle -> `m_IncreaseForDeathRattleSound`. Help and the
/// alarm bell go through their own stimuli (0x00C58560 cases 5, 6): `m_IncreaseForHelpSound`,
/// `m_IncreaseForAlarmSound`. The other contexts (ambient white noise, impending explosion,
/// ally search requests and notices, flare combustion) raise no attention there.
/// Returns the increase and whether it is a danger noise (`EAINoiseContext_Danger` and the
/// contexts read like it).
fn noise_increase(prof: &AttentionConfig, c: NoiseContext) -> Option<(AttentionIncrease, bool)> {
    Some(match c {
        NoiseContext::Anomaly | NoiseContext::Footfall => (prof.anomaly_sound, false),
        NoiseContext::Threatening | NoiseContext::WolZap => (prof.threatening_sound, false),
        NoiseContext::Danger | NoiseContext::RatSwarm | NoiseContext::WatchTowerAttack => (prof.danger_sound, true),
        NoiseContext::DeathRattle => (prof.death_rattle, true),
        NoiseContext::Help => (prof.help_sound, true),
        NoiseContext::AlarmBell => (prof.alarm_sound, true),
        _ => return None,
    })
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
            shoot: None,
            run: false,
            follow: Follower::default(),
            attn: Vec::new(),
            suspecting: false,
            hostile: Vec::new(),
            slots: slots::routine_stack(),
            stims: Vec::new(),
            att_target: None,
            att_enemy: false,
            att_level: 0,
            routine: BrainState::Idle,
            routine_goal: None,
            shown: BrainState::Idle,
            allies: Vec::new(),
            magic_pending: None,
            magic_level: 0,
            aggression: 0.0,
            loco: None,
            corpses_seen: Vec::new(),
            lost: 0.0,
            crumbs: 0,
            ammo: if kind == PawnKind::Guard { 1 } else { 0 },
            aim: 0.0,
            melee_slot: true,
            stop_roll: Self::roll_stops(&mut (seed | 1).wrapping_mul(0x2545_F491_4F6C_DD1D)),
            melee_cd: Vec::new(),
            patrol: None,
            patrol_dir: 1,
            guard_timer: 0.0,
            faction: None,
            squad: String::new(),
            enemies: Vec::new(),
            cause: 0,
            cause_danger: false,
            ambient_bark_timer: None,
            bell: None,
            ring_wait: 0.0,
            alarm_enemy: None,
            barks: Vec::new(),
            found_corpses: Vec::new(),
            gun_timer: -1.0,
            hostile_faction: Vec::new(),
            deaf: false,
            blind: false,
            numb: false,
            attack: 0,
            attack_timer: 0.0,
            dodge_dir: 1,
            flurry: 0,
            stamina_timer: -1.0,
            incoming: 0,
            incoming_needed: 0,
            forgive_timer: 0.0,
            threat_timer: 0.0,
            threat_needed: -1.0,
            target_swinging: false,
            firing: false,
            fire_timer: 0.0,
            give_up: 0.0,
            in_fight: false,
            class: if kind == PawnKind::Guard { NpcClass::EliteGuard } else { NpcClass::Civilian },
            special: SpecialCombat::default(),
            head: head::Head::default(),
        }
    }

    /// `m_bAwareOfPlayerUponStartup` (spawner post-spawn 0x00a590e0): the brain gets the
    /// `EAIStimID_MaxOutAttention` stimulus for the player (0x00b23be0), whose handler
    /// (0x00b3f6a0) adds a `DACRT_Psychic` change to `DALFI_MaxAttention` with the proxy at
    /// the player's location.
    pub fn aware_of(&mut self, ai: &AiConfig, target: PlayerId, at: Vec3) {
        self.bump(ai, target, AttentionIncrease { amount: 0.0, to_level: 5 }, at, true, reason::PSYCHIC);
    }

    /// Relationship toward a pawn (0x00c62f20 -> 0x00b043d0): the pawn's individual
    /// relationships first (the `hostile` list: witnessed violence, attacks, mission
    /// dispositions; factions it was set hostile to), then its faction's (`Social`).
    pub fn relation(&self, who: PlayerId, soc: &Social) -> Relationship {
        if who == NOISE || self.hostile.contains(&who) {
            return Relationship::Enemy;
        }
        if let (Some(t), Some(&f)) = (soc.factions.as_ref(), soc.pawn_faction.get(&who)) {
            if self.hostile_faction.iter().any(|&h| t.isa(f, h)) {
                return Relationship::Enemy;
            }
        }
        match self.faction {
            Some(mine) => soc.faction_relation(mine, who),
            None => Relationship::Neutral,
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

    /// Make `id` an individual enemy (and an enemy this tick).
    pub fn make_hostile(&mut self, id: PlayerId) {
        if !self.hostile.contains(&id) {
            self.hostile.push(id);
        }
        if !self.enemies.contains(&id) {
            self.enemies.push(id);
        }
    }

    /// Mission control: treat `target` as an enemy at full attention.
    pub fn alert_to(&mut self, ai: &AiConfig, target: PlayerId, at: Vec3) {
        self.make_hostile(target);
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
        match self.state {
            BrainState::Combat => 0,
            BrainState::Notice | BrainState::Investigate | BrainState::Search | BrainState::React | BrainState::Awe => 1,
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
        self.stims.clear();
        self.magic_pending = None;
        self.follow.clear();
    }

    /// An external write of `state` since the brain's last tick becomes the current behaviour
    /// ([`Brain::force`]).
    pub fn sync_external(&mut self) {
        if self.state != self.shown {
            self.force(self.state);
        }
    }

    /// Profile slot of a target: neutral pawns, or enemies/places while (un)suspecting.
    fn profile_slot(&self, who: PlayerId) -> usize {
        if who != NOISE && !self.enemies.contains(&who) {
            P_NEUTRAL
        } else if self.suspecting {
            P_ENEMY_SUS
        } else {
            P_ENEMY_UNS
        }
    }

    fn profile<'a>(&self, ai: &'a AiConfig, who: PlayerId) -> &'a AttentionConfig {
        // The wolfhound's own profiles (`Twk_Attn_Wolfhound_*`: lower thresholds, faster leak);
        // toward neutrals and allies it uses its unsuspecting one.
        if self.class == NpcClass::Wolfhound {
            let h = &ai.special.wolfhound;
            let p = match self.profile_slot(who) {
                P_ENEMY_SUS => h.suspecting.as_ref(),
                _ => h.unsuspecting.as_ref(),
            };
            if let Some(p) = p {
                return p;
            }
        }
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
        self.attn.push(Attn { who, value: 0.0, level: 0, proxy, fresh: false, urgent: false, sighted: false, los_norm: 0.0, why: 0, danger: false });
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
        e.why = why;
        e.danger = false;
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

    /// The stance modifier of the head's locomotion loop (the runtime's modifier choice:
    /// guards carry the sword, `SwordPatrol` on routine).
    fn head_modifier(&self, _ai: &AiConfig) -> &'static str {
        let guard = self.kind == PawnKind::Guard;
        match self.state {
            BrainState::Combat => if guard { "Sword" } else { "Empty" },
            BrainState::Flee | BrainState::Cower => if guard { "PanicSword" } else { "PanicEmpty" },
            s if s.is_routine() => if guard { "SwordPatrol" } else { "Empty" },
            _ => if guard { "SearchSword" } else { "SearchEmpty" },
        }
    }

    /// The look-at head after this tick's decisions: the attention process's HeadTrack request
    /// (0x00B25890: behaviour level HeadTrack on an enemy focus; head weight 1, torso weight
    /// `m_fHeadTrackTorsoRatio`, speed 0.5) toward the focus's eyes, then the cursor and the
    /// eye ([`head::Head::step`]).
    fn step_head(&mut self, me: &PlayerState, ws: &WorldState, ai: &AiConfig, dt: f32) {
        let focus = if self.att_level == DAL_HEAD_TRACK && self.att_enemy { self.att_target.and_then(|t| ws.player(t)) } else { None };
        let request = focus.map(|t| {
            // `GetActorEyesViewPoint`: `BaseEyeHeight` below the standing cylinder top, the same
            // offset from the crouched top.
            let [eye_h, hh] = ai.player_eye;
            let target = if t.kind == PawnKind::Human && hh > 0.0 {
                let cur = if t.crouched { ai.player_visibility.cylinder[2] } else { hh };
                t.position + Vec3::Z * (cur - (hh - eye_h))
            } else {
                t.position + Vec3::Z * EYE
            };
            head::LookRequest { target, head_weight: 1.0, torso_weight: ai.head_track_torso_ratio, torso_speed_reduction: true, speed: 0.5 }
        });
        let modifier = self.head_modifier(ai);
        let speed = me.velocity.truncate().length();
        self.head.step(ai, me.archetype, modifier, me.position, me.yaw, speed, request, dt);
    }

    /// The look-at node's head and torso aims (radians) for the animation: the same cursor and
    /// weights the vision eye uses ([`head::Head::parts`]).
    pub fn head_parts(&self, me: &PlayerState, ai: &AiConfig) -> ([f32; 2], [f32; 2]) {
        let (h, t) = self.head.parts(ai, self.head_modifier(ai), me.velocity.truncate().length());
        (h.into(), t.into())
    }

    /// The vision eye this tick: the head of the last step ([`head::Head::step`]).
    fn vision_eye(&self, me: &PlayerState, ai: &AiConfig) -> head::Eye {
        self.head.last_eye(ai, me.archetype, &self.head_modifier(ai), me.position, me.yaw)
    }

    /// The vision component's test (0x00c788e0): the target's location inside a cone, then a
    /// line-of-sight trace to one of its test points (top, middle, bottom of the pawn).
    /// Returns the normalized vision distance. The cones are along the head's axis from the
    /// `camera_jnt` eye ([`head`]).
    fn look(veye: &head::Eye, other: &PlayerState, world: &dyn WorldQuery, ai: &AiConfig) -> Option<f32> {
        let (inner, outer) = cones(&ai.vision);
        let (norm, _) = vision_distance(&inner, &outer, veye.local(other.position))?;
        let eye = veye.pos;
        let clear = |p: Vec3| {
            let to = p - eye;
            let d = to.length();
            d < 1.0 || !world.raycast(eye, to / d, d, channel::VISION).is_some_and(|h| h.distance < d - 40.0)
        };
        [60.0, 0.0, -50.0].iter().any(|z| clear(other.position + Vec3::Z * *z)).then_some(norm)
    }

    /// Cone and line-of-sight test (events, allies, corpses).
    fn sees(veye: &head::Eye, other: &PlayerState, world: &dyn WorldQuery, ai: &AiConfig) -> bool {
        Self::look(veye, other, world, ai).is_some()
    }

    /// The target's `Attribute_Visibility`: built from light, speed and stance for players,
    /// the attribute base (0) for NPCs.
    /// The speed value is the horizontal speed over `Attribute_GroundSpeedSprint`, unclamped
    /// (0x00ac33b0); `light01` is the normalized light value at the player.
    fn visibility(ai: &AiConfig, other: &PlayerState, light01: f32) -> f32 {
        if other.kind != PawnKind::Human {
            return 0.0;
        }
        let speed01 = other.velocity.truncate().length() / ai.player_visibility.ground_speed_sprint.max(1.0);
        player_visibility(&ai.player_visibility, light01, speed01, vis_state(other))
    }

    #[cfg(test)]
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
        let social = Social::default();
        let light: BTreeMap<PlayerId, f32> = BTreeMap::new();
        let audio = dishonored_ir::ai::AudioCells::default();
        let senses = Senses { peers, barks, busted: ally_busted, alarm_noise: &[], social: &social, light: &light, audio: &audio, magic: &[] };
        self.perceive_with(me, ws, events, &senses, world, ai, dt);
    }

    #[allow(clippy::too_many_arguments)]
    fn perceive_with(&mut self, me: &PlayerState, ws: &WorldState, events: &[SimEvent], senses: &Senses, world: &dyn WorldQuery, ai: &AiConfig, dt: f32) {
        let (peers, barks, ally_busted) = (senses.peers, senses.barks, senses.busted);
        for a in self.attn.iter_mut() {
            a.fresh = false;
        }
        // Enemies (and allies) this tick: individual, trespass and faction relationships.
        self.enemies = ws.players.iter().filter(|p| p.id != self.id && p.alive && self.relation(p.id, senses.social) == Relationship::Enemy).map(|p| p.id).collect();
        self.allies = ws.players.iter().filter(|p| p.id != self.id && self.relation(p.id, senses.social) == Relationship::Ally).map(|p| p.id).collect();
        let eye = me.position + Vec3::Z * EYE;
        let veye = self.vision_eye(me, ai);
        // Hearing (0x00bb50d0, `hearing::heard`): strictly within the noise radius; between
        // audio cells along the precomputed portal paths; for listeners and noises outside the
        // cells, a trace from the listener to the noise (blocked: not heard).
        let deaf = self.deaf;
        let audio = senses.audio;
        let my_cell = if audio.cells.is_empty() { None } else { hearing::cell_of(audio, me.position) };
        let heard = |pos: Vec3, radius: f32| {
            if deaf {
                return false;
            }
            let nc = if audio.cells.is_empty() { None } else { hearing::cell_of(audio, pos) };
            hearing::heard(audio, me.position, my_cell, pos, nc, radius, || {
                let to = pos - eye;
                let l = to.length();
                l < 1.0 || !world.raycast(eye, to / l, l, channel::VISION).is_some_and(|h| h.distance < l - 40.0)
            })
        };
        // Pain barks: the pawn's damage handler fires `DDH_COMBAT_OUCH_SMALL` (0x00b733f0, hook
        // 60, initiator the damage causer); attacked while on routine, the behaviour also asks
        // `DDH_PATROL_ATTACKED_BY_ENEMY` (0x00afc3e0). Both requests go to the bark manager, which
        // arbitrates (conversation.md R6 / F9). The damage type's hook override (byte +0x84 of
        // its tweak, `FUN_00b72c40`) is not modelled: no damage-type tweak reaches the AI.
        for e in events {
            if let SimEvent::Damage { victim, attacker, .. } = e {
                if *victim == self.id {
                    if self.state.is_routine() {
                        self.barks.push(("DDH_PATROL_ATTACKED_BY_ENEMY".into(), *attacker));
                    }
                    self.barks.push(("DDH_COMBAT_OUCH_SMALL".into(), *attacker));
                }
            }
        }
        // The alarm bell's AI noise (`EAINoiseContext_AlarmBell`): guards investigate
        // (`m_IncreaseForAlarmSound`, reason Alarm); for civilians it is a scary noise.
        for &(p, r) in senses.alarm_noise {
            if heard(p, r) {
                let inc = self.profile(ai, NOISE).alarm_sound;
                self.bump(ai, NOISE, inc, p, true, reason::ALARM);
                if self.kind != PawnKind::Guard {
                    self.panic_from(ai, p);
                }
            }
        }
        // Sounds (gameplay sound events, never raw audio) -> attention toward the place
        // (`DACRT_Heard`).
        for e in events {
            if let SimEvent::Sound { position, radius, context, source, .. } = e {
                if *source == self.id || !heard(*position, *radius) {
                    continue;
                }
                // The attention goes to the noise's instigator (proxy at the noise), with the
                // profile of the relationship toward it: measured on the original
                // (`ai/boyle_hear_shot`, a neutral party guard: player attention 1.0 at the shot,
                // the neutral profile's danger increase to Busted's start, then its 2.0 / s leak,
                // level HeadTrack). A noise without a pawn stays a place entry.
                let who = if *source != NOISE && ws.player(*source).is_some() { *source } else { NOISE };
                let prof = self.profile(ai, who);
                let Some((inc, urgent)) = noise_increase(prof, *context) else { continue };
                // A footfall's stimulus is raised from the floor contact to the walker's
                // height (0x00B40B70 case Footfall: z + target point - floor).
                let at = match (*context, ws.player(*source)) {
                    (NoiseContext::Footfall, Some(src)) => Vec3::new(position.x, position.y, src.position.z),
                    _ => *position,
                };
                self.bump(ai, who, inc, at, urgent, reason::HEARD);
                if matches!(*context, NoiseContext::Danger | NoiseContext::RatSwarm | NoiseContext::WatchTowerAttack) {
                    if let Some(a) = self.attn.iter_mut().find(|a| a.who == who) {
                        a.danger = true;
                    }
                }
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
                    PawnKind::Guard => ws.player(src).is_some_and(|g| Self::sees(&veye, g, world, ai)),
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
                // Being hurt by a player is a reaction response (`m_Response_HurtByPlayerNeutral` /
                // `_HurtByPlayerAlly`, `ResponseCategory_Hostile`): the player becomes an individual
                // enemy. An NPC attacker changes no relationship; factions decide (0x00c62f20).
                SimEvent::Damage { victim, attacker: Some(a), .. } if *victim == self.id && *a != self.id && ws.player(*a).is_some_and(|p| p.kind == PawnKind::Human) => {
                    let at = ws.player(*a).map(|p| p.position).unwrap_or(me.position);
                    self.make_hostile(*a);
                    let inc = self.profile(ai, *a).attacked_by;
                    self.bump(ai, *a, inc, at, true, reason::ATTACKED_BY);
                    // `EAIStimID_AttackedByEnemy` in a fight: `m_fAggressionDeltaPerDamage`
                    // (0x00b3e070; the back-step count is the swing-edge rule in `combat`).
                    if self.in_fight {
                        self.aggression = (self.aggression + ai.combat.dodge.aggression_delta_per_damage).clamp(0.0, 1.0);
                    }
                }
                SimEvent::Death { victim, killer: Some(k), .. } if *k != self.id && ws.player(*k).is_some_and(|p| p.kind == PawnKind::Human) => {
                    let (Some(v), Some(kp)) = (ws.player(*victim), ws.player(*k)) else {
                        continue;
                    };
                    if Self::sees(&veye, v, world, ai) || Self::sees(&veye, kp, world, ai) {
                        // Witnessed murder: the killer becomes an enemy at full attention.
                        self.make_hostile(*k);
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
                    let seen = !self.blind && Self::look(&veye, p, world, ai).is_some_and(|n| seen_rate(prof, prof.seeing_scalar_dead_npc, n, 0.0) > 0.0);
                    if seen {
                        self.corpses_seen.push(p.id);
                        self.found_corpses.push(p.id);
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
            // Players always; NPCs only when they are enemies (`_ToLiveNPC` scalar).
            if p.kind != PawnKind::Human && !self.enemies.contains(&p.id) {
                continue;
            }
            // A pawn seen swinging or firing near the NPC is `m_Response_PlayerThreats`
            // (`ResponseCategory_Harmless`, a bark) in every brain reaction tweak: it changes no
            // relationship. Hostility comes from the Hostile responses (hurt by the player),
            // witnessed deaths, factions and allies' fights.
            let seen = if self.blind { None } else { Self::look(&veye, p, world, ai) };
            let hostile = self.enemies.contains(&p.id);
            let prof = self.profile(ai, p.id).clone();
            let stop = self.stops(ai, p.id);
            let pv = &ai.player_visibility;
            let vis = Self::visibility(ai, p, senses.light.get(&p.id).copied().unwrap_or_else(|| light01(LIGHT_LEVEL, [pv.light_darkest, pv.light_brightest])));
            let near = (p.position - me.position).truncate().length() <= prof.distance_to_lose_sight && (p.position.z - me.position.z).abs() <= prof.distance_to_lose_sight;
            let touching = !self.numb && p.position.distance(me.position) < 80.0;
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
                let scalar = if p.kind == PawnKind::Human { prof.seeing_scalar_player } else { prof.seeing_scalar_npc };
                let rate = seen_rate(&prof, scalar, e.los_norm, vis);
                if rate > 0.0 {
                    e.change(&prof, &stop, rate * dt, 0, reason::SEEN, false);
                    e.why = reason::SEEN;
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
                    if self.state == BrainState::Combat || !Self::sees(&veye, pp, world, ai) {
                        continue;
                    }
                    let tp = ws.player(t).map(|p| p.position).unwrap_or(pp.position);
                    self.make_hostile(t);
                    let inc = self.profile(ai, t).ally_combat_target;
                    self.bump(ai, t, inc, tp, true, reason::SEE_ALLY_COMBAT);
                }
                (PawnKind::Civilian, BrainState::Flee | BrainState::Cower) => {
                    if Self::sees(&veye, pp, world, ai) {
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
        self.perceive_magic(ws, senses.magic, ai, dt);
        // The attention component's behaviour stimuli, then the panic process.
        self.post_attention(ws);
        self.panic_process(ws);
    }

    /// Powers seen (`DishonoredActivePowerComponent` -> 0x00bf9fb0 -> 0x00b23e50 on every
    /// brain): Windblast and Devouring Swarm casts and Bend Time ending a slow-down are mode 0,
    /// a Blink's end, possession ending and Bend Time ending a stop are mode 1, a Blink's start,
    /// possession starting and Bend Time stopping time are mode 2 (`MagicMode`).
    fn perceive_magic(&mut self, ws: &WorldState, magic: &[(PlayerId, u8)], ai: &AiConfig, dt: f32) {
        if let Some((_, t)) = self.magic_pending.as_mut() {
            *t -= dt;
        }
        if self.magic_pending.is_some_and(|(_, t)| t <= 0.0) {
            self.magic_pending = None;
        }
        for &(caster, mode) in magic {
            if caster == self.id {
                continue;
            }
            self.witness_magic(caster, mode, ws, ai);
        }
        // A pending witness whose attention to the user crosses into Busted (0x00b24ec0) or out
        // of it (0x00b23160, the user disappeared) reacts then.
        if let Some((caster, _)) = self.magic_pending {
            let now = self.attn.iter().find(|a| a.who == caster).map_or(0, |a| a.level);
            let at = ws.player(caster).map(|p| p.position).unwrap_or_default();
            if now >= DAL_BUSTED && self.magic_level < DAL_BUSTED {
                self.magic_pending = None;
                self.post_magic(caster, at, false, ai);
            } else if now < DAL_BUSTED && self.magic_level >= DAL_BUSTED {
                self.magic_pending = None;
                self.post_magic(caster, at, true, ai);
            }
            self.magic_level = now;
        }
    }

    /// 0x00b23e50: a brain Busted on the user reacts at once (modes 0 and 1) or remembers it for
    /// `3 s` (mode 2); a brain not Busted remembers it and gets `m_IncreaseForMagic` toward the
    /// user (mode 1).
    fn witness_magic(&mut self, caster: PlayerId, mode: u8, ws: &WorldState, ai: &AiConfig) {
        let level = self.attn.iter().find(|a| a.who == caster).map_or(0, |a| a.level);
        let busted = level >= DAL_BUSTED;
        let at = ws.player(caster).map(|p| p.position).unwrap_or_default();
        self.magic_pending = None;
        match mode {
            0 | 1 if busted => self.post_magic(caster, at, false, ai),
            1 => {
                self.magic_pending = Some((caster, 3.0));
                self.magic_level = level;
                let inc = self.profile(ai, caster).magic;
                self.bump(ai, caster, inc, at, false, reason::MAGIC);
            }
            2 if busted => {
                self.magic_pending = Some((caster, 3.0));
                self.magic_level = level;
            }
            _ => {}
        }
    }

    /// `EAIStimID_WitnessMagic`, delayed by `m_MagicReactionDelay`.
    fn post_magic(&mut self, caster: PlayerId, at: Vec3, disappeared: bool, ai: &AiConfig) {
        let [lo, hi] = ai.magic_reaction_delay;
        let d = lo + (hi - lo) * rng_next(&mut self.rng);
        self.post(Stim::WitnessMagic, d, caster, at, disappeared, false);
    }

    /// The civilians' panic process (`DisAIBrainProcessPanic`): `Panicked` when the strongest
    /// stimulus is Busted, or Investigate on a live enemy (the port's reading of the panic
    /// stimulus table, AI.md "Patrols, combat and panic").
    fn panic_process(&mut self, ws: &WorldState) {
        if self.kind == PawnKind::Guard || matches!(self.behavior(slot::ESCAPE), Some(Behavior::Panic { .. })) || self.stims.iter().any(|q| q.stim == Stim::Panicked) {
            return;
        }
        let Some((who, level, proxy, _)) = self.focus() else { return };
        let hostile_alive = who != NOISE && self.enemies.contains(&who) && ws.player(who).is_some_and(|p| p.alive);
        if level >= DAL_BUSTED || (level >= DAL_INVESTIGATE && hostile_alive) {
            let at = if hostile_alive { ws.player(who).map(|p| p.position).unwrap_or(proxy) } else { self.threat_pos.unwrap_or(proxy) };
            self.post(Stim::Panicked, 0.0, who, at, false, true);
        }
    }

    /// Strongest stimulus: (who, level, proxy, urgent). Neutral pawns only ever head-track.
    fn focus(&self) -> Option<(PlayerId, u8, Vec3, bool)> {
        self.attn
            .iter()
            .map(|a| {
                let lvl = if a.who != NOISE && !self.enemies.contains(&a.who) { a.level.min(DAL_HEAD_TRACK) } else { a.level };
                (a.who, lvl, a.proxy, a.urgent, a.value)
            })
            .max_by(|a, b| a.1.cmp(&b.1).then(a.4.total_cmp(&b.4)).then(b.0.cmp(&a.0)))
            .map(|(w, l, p, u, _)| (w, l, p, u))
    }

    /// Walk (or run) toward `goal` along the navmesh at the brain's locomotion speed.
    fn move_to(&mut self, me: &PlayerState, nav: Option<&NavWorld>, goal: Vec3, run: bool, ai: &AiConfig) -> PlayerInput {
        let steer = self.follow.steer(nav, me.position, goal);
        let mut i = PlayerInput { yaw: yaw_to(me.position, steer), forward: 127, left_hand: LeftHand::Empty, ..Default::default() };
        i.buttons |= if run { buttons::SPRINT } else { buttons::WALK };
        i.npc_speed = self.loco_speed(ai, run);
        i
    }

    /// The pawn's locomotion config: its own, else the elite guard's (the port's NPC
    /// archetypes animate with `EliteGuardLocomotionConfig` unless their tweak names another).
    pub fn loco_config<'a>(&self, ai: &'a AiConfig) -> Option<&'a dishonored_ir::npc_anim::LocoConfig> {
        // The special classes' own configs (`m_pComponentLocomotionConfig` of their pawn tweaks).
        let by_class = match self.class {
            NpcClass::Wolfhound => "WolfHoundLocomotionConfig",
            NpcClass::Tallboy => "TallBoyLocomotionConfig",
            NpcClass::Weeper => "WeepTallLocomotionConfig",
            _ => "EliteGuardLocomotionConfig",
        };
        match self.loco {
            Some(i) => ai.locomotion.get(i as usize),
            None => ai.locomotion.iter().find(|c| c.name.ends_with(by_class)).or_else(|| ai.locomotion.iter().find(|c| c.name.ends_with("EliteGuardLocomotionConfig"))),
        }
    }

    /// Speed of a locomotion request (`DisLocoRequest.m_MaxSpeedIndex`: Walk 1, or Jog 2 when
    /// running) under the stance modifier the current behaviour's body intention selects (as the
    /// presentation picks it, except the routine: EmptyPatrol for a guard's routine, SearchSword / SearchEmpty while
    /// investigating, Sword in combat, PanicEmpty / PanicSword in a panic, Empty otherwise): the
    /// authored speed of the speed mode's forward (N) loop times the modifier's
    /// `m_fSpeedMultiplier`. 0 without a locomotion config. LIKELY (speed = authored loop speed
    /// x modifier, not measured; research/dishonored/systems/ai.md §2, F3).
    pub fn loco_speed(&self, ai: &AiConfig, run: bool) -> f32 {
        let Some(cfg) = self.loco_config(ai) else { return 0.0 };
        // The special classes: their configs' modifiers, and the speed index the transit speed
        // maps to (`m_TransitSpeedToLocomotionSpeed`: hound {0, 1, 3}, tallboy {0, 0, 1}).
        let special: Option<(&[&str], usize)> = match self.class {
            NpcClass::Wolfhound => Some((
                match self.state {
                    BrainState::Combat | BrainState::Notice => &["Sword", "Empty"],
                    BrainState::Investigate | BrainState::Search => &["Search", "Empty"],
                    _ => &["Empty"],
                },
                if run { 3 } else { 1 },
            )),
            NpcClass::Tallboy => Some((&["Empty"], 1)),
            NpcClass::Weeper => Some((if matches!(self.state, BrainState::Investigate | BrainState::Search) { &["SearchEmpty", "Empty"] } else { &["Empty"] }, if run { 2 } else { 1 })),
            NpcClass::MusicalOverseer if self.state == BrainState::Combat => Some((&["MusicBox", "Empty"], if run { 2 } else { 1 })),
            _ => None,
        };
        if let Some((want, speed)) = special {
            let Some(mi) = want.iter().find_map(|w| cfg.modifier(w)) else { return 0.0 };
            let Some(mode) = cfg.mode(mi, speed).or_else(|| cfg.mode(mi, 1)) else { return 0.0 };
            let forward = if mode.move_anims.len() >= 4 { mode.move_anims[2].1 } else { mode.move_anims.first().map_or(0.0, |a| a.1) };
            return forward * cfg.modifiers[mi].speed_multiplier;
        }
        let armed = self.kind == PawnKind::Guard;
        let want: &[&str] = match (self.state, armed) {
            (BrainState::Investigate | BrainState::Search | BrainState::RingAlarm, true) => &["SearchSword", "Sword"],
            (BrainState::Investigate | BrainState::Search | BrainState::RingAlarm, false) => &["SearchEmpty", "Empty"],
            (BrainState::Flee | BrainState::Cower, true) => &["PanicSword", "Sword"],
            (BrainState::Flee | BrainState::Cower, false) => &["PanicEmpty", "Empty"],
            (BrainState::Combat | BrainState::Notice | BrainState::Awe, true) => &["Sword"],
            // A patrolling guard walks sheathed: `EmptyPatrol` (x0.92) on `Empty_Walk_Patrol`
            // (126.4) = 116.3 uu/s, the speed measured on the original (boyle_patrol_speed).
            (_, true) => &["EmptyPatrol", "Empty"],
            _ => &["Empty"],
        };
        let Some(mi) = want.iter().find_map(|w| cfg.modifier(w)) else { return 0.0 };
        let Some(mode) = cfg.mode(mi, if run { 2 } else { 1 }) else { return 0.0 };
        let forward = if mode.move_anims.len() >= 4 { mode.move_anims[2].1 } else { mode.move_anims.first().map_or(0.0, |a| a.1) };
        forward * cfg.modifiers[mi].speed_multiplier
    }

    fn reached(me: &PlayerState, goal: Vec3, r: f32) -> bool {
        goal.truncate().distance(me.position.truncate()) < r
    }

    /// The point a scripted go-to (`stay`, `WalkToPoint`) stops on: the end of the held
    /// navmesh path. An actor destination always asks for an accurate stop (0x00ae4de0), and the
    /// locomotion component's destination is the last point of the found path (0x00949890).
    pub fn accurate_stop(&self) -> Option<Vec3> {
        (self.state == BrainState::WalkToPoint && self.stay).then(|| self.follow.path.last().copied()).flatten()
    }

    /// Arrival of a walk: an accurate stop arrives within 0.1 uu of the path's end (0x00944390,
    /// 0.01 squared); the pawn's last step lands on it (`PlayerState::nav_end`). Port fallbacks:
    /// a pawn held by collision 60 uu from its goal for a second, a go-to without a navmesh
    /// path, and the routine walks arrive within 60 uu.
    fn arrived(&self, me: &PlayerState, goal: Vec3) -> bool {
        match self.accurate_stop() {
            Some(end) => Self::reached(me, end, 0.1) || (self.stuck > 1.0 && Self::reached(me, goal, 60.0)),
            None => Self::reached(me, goal, 60.0),
        }
    }

    /// The routine state the Idle slot resumes with after another behaviour: a scripted walk
    /// (`stay`) continues; otherwise a guard walks its route or returns to its post (the port's
    /// stand-in for `DisBehaviorGoHome`'s tether, which is not ported), a civilian idles.
    pub(crate) fn resume_routine(&mut self) -> BrainState {
        self.guard_timer = 0.0;
        if self.routine == BrainState::WalkToPoint && self.stay && self.routine_goal.is_some() {
            return BrainState::WalkToPoint;
        }
        self.routine_goal = None;
        if self.kind == PawnKind::Guard {
            if self.has_patrol() { BrainState::Patrol } else { BrainState::Alert }
        } else {
            BrainState::Idle
        }
    }

    fn has_patrol(&self) -> bool {
        self.patrol.is_some() || self.route.len() >= 2
    }

    /// Adopt a route (0x00c41590): among the adoptable routes other than `exclude`
    /// (0x00a4bc70: active, squad, capacity, adoption range) the nearest neglected one
    /// (`m_NeglectTimer > m_NeglectThreshold`, 0x00a43860) wins; otherwise the nearest one,
    /// but only for a pawn without a route (or leaving its route). Starts at the point
    /// nearest to `pos`.
    fn adopt_route(&mut self, routes: &[PatrolRoute], users: &mut [i32], neglect: &[f32], pos: Vec3, exclude: Option<usize>) -> bool {
        let mut best: Option<(f32, usize, usize)> = None;
        let mut best_neglected: Option<(f32, usize, usize)> = None;
        for (ri, r) in routes.iter().enumerate() {
            if Some(ri) == exclude || !r.adoptable(users[ri], &self.squad, pos) {
                continue;
            }
            let neglected = neglect.get(ri).is_some_and(|t| *t > r.neglect_threshold);
            for (pi, p) in r.points.iter().enumerate() {
                let d = p.position.distance_squared(pos);
                let slot = if neglected { &mut best_neglected } else { &mut best };
                if slot.is_none_or(|b| d < b.0) {
                    *slot = Some((d, ri, pi));
                }
            }
        }
        let leaving = exclude.is_some() && exclude == self.patrol;
        let pick = best_neglected.or(if self.patrol.is_none() || leaving { best } else { None });
        let Some((_, ri, pi)) = pick else {
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
    fn advance_route(&mut self, routes: &[PatrolRoute], users: &mut [i32], neglect: &[f32], pos: Vec3) {
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
                    if !self.adopt_route(routes, users, neglect, pos, Some(ri)) {
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
        // The brain tick (0x00b17100): the stimuli whose delay ran out are dispatched, then the
        // current behaviour (the highest occupied slot) runs.
        self.drain(me, ws, ctx, ai, dt);
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
                    self.advance_route(ctx.routes, ctx.users, ctx.neglect, me.position);
                } else if self.state == BrainState::Patrol && !self.route.is_empty() {
                    self.route_i = (self.route_i + 1) % self.route.len();
                }
            }
        }
        self.state = self.project();
        self.shown = self.state;
        i
    }

    /// `DisBehaviorTriggerAlarm` (first in the guard behaviour list, slot Alert): on
    /// `EnemyBusted` (with `CombatBegin`, a sighting) or a search caused by a found corpse, a
    /// witnessed death or a help request, a guard goes to ring the nearest ready bell within
    /// `m_fNPCGoToAlarmCloseness` with the tweak's chance, unless the enemy is within
    /// `m_fFightRadius` (0x00af6bd0). The bell keeps one pending ringer, the one nearest to it
    /// (0x00a1e140). Decided once until the brain is back to its routine.
    pub(crate) fn consider_alarm(&mut self, me: &PlayerState, ws: &WorldState, ctx: &mut Ctx, ai: &AiConfig, cause: AlarmCause, enemy: Option<PlayerId>) {
        if self.kind != PawnKind::Guard || self.bell.is_some() || ctx.cooldown > 0.0 || ctx.alarms.is_empty() || self.alarm_enemy.is_some() {
            return;
        }
        let ta = &ai.trigger_alarm;
        let chance = match cause {
            AlarmCause::Sighting => {
                // "Alone": no other guard within the companion range (0x00af5fe0).
                let alone = !ws.players.iter().any(|p| p.id != self.id && p.alive && p.kind == PawnKind::Guard && p.position.distance(me.position) <= ta.companion_range);
                if alone { ta.chance_sighting_alone } else { ta.chance_sighting_in_group }
            }
            AlarmCause::FoundCorpse => ta.chance_found_corpse,
            AlarmCause::WitnessDeath => ta.chance_witness_death,
            AlarmCause::Help => ta.chance_help,
        };
        self.alarm_enemy = Some(enemy.unwrap_or(NOISE));
        if chance <= 0.0 || rng_next(&mut self.rng) >= chance {
            return;
        }
        if let Some(e) = enemy.and_then(|e| ws.player(e)) {
            if e.position.distance(me.position) <= ta.fight_radius {
                return;
            }
        }
        let d2 = |a: &AlarmBell| a.def.position.distance(me.position);
        let pick = ctx.alarms.iter().enumerate().filter(|(_, a)| !a.ringing && a.cooldown <= 0.0 && d2(a) <= a.def.go_closeness).min_by(|a, b| d2(a.1).total_cmp(&d2(b.1)).then(a.0.cmp(&b.0)));
        let Some((bi, bell)) = pick else { return };
        if let Some(r) = bell.ringer.filter(|r| *r != self.id) {
            if ws.player(r).is_some_and(|p| p.alive && p.position.distance(bell.def.position) <= d2(bell)) {
                return;
            }
        }
        ctx.alarms[bi].ringer = Some(self.id);
        ctx.alarms[bi].enemy = enemy;
        self.bell = Some(bi);
        self.ring_wait = 0.0;
        self.place(Behavior::TriggerAlarm);
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
        if self.shoot.is_some() {
            return self.kismet_shoot(me, ws, ai, dt);
        }
        // The search animation's cooldown runs while the Search behaviour exists (0x00ae42c0).
        if let Some(Some(Behavior::Search(s))) = self.slots.get_mut(slot::SEARCH as usize) {
            s.anim_cooldown -= dt;
        }
        let cur = self.current_slot();
        if cur != slot::IDLE {
            self.state = self.project();
            return match cur {
                // `DisBehaviorNotice`: turn to face the stimulus.
                slot::NOTICE => {
                    let p = self.att_target.and_then(|w| self.attn.iter().find(|a| a.who == w)).map(|a| a.proxy).or(self.threat_pos).or(look);
                    p.map(face).unwrap_or(idle)
                }
                slot::SEARCH => self.act_search(me, ws, nav, ai, dt),
                slot::COMBAT => self.combat(me, ws, world, nav, ai, ctx.inhibit, dt),
                slot::ALERT => self.act_alarm(me, ws, ctx, nav, ai, dt),
                slot::AWE => self.act_magic(me, ws, ai, dt),
                slot::ESCAPE => self.act_panic(me, ws, layout, nav, ai),
                _ => idle,
            };
        }
        if !self.state.is_routine() {
            self.state = self.resume_routine();
        }
        match self.state {
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
                Some(g) if !self.arrived(me, g) && !(self.follow.failed && !self.stay) => {
                    let run = self.run;
                    self.move_to(me, nav, g, run, ai)
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
                            self.advance_route(ctx.routes, ctx.users, ctx.neglect, me.position);
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
                        self.advance_route(ctx.routes, ctx.users, ctx.neglect, me.position);
                    }
                    return look.map(face).unwrap_or(idle);
                }
                self.move_to(me, nav, p.position, false, ai)
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
                self.move_to(me, nav, g, false, ai)
            }
            BrainState::Alert => {
                // Guard post (`DisBehaviorGuard`): stand, look at stimuli.
                if !Self::reached(me, self.home, 120.0) && !self.follow.failed {
                    let home = self.home;
                    return self.move_to(me, nav, home, false, ai);
                }
                look.map(face).unwrap_or(idle)
            }
            _ => idle,
        }
    }

    /// `DisBehaviorSearch` (guards) and `DisBehaviorStationarySearch` (civilians).
    fn act_search(&mut self, me: &PlayerState, ws: &WorldState, nav: Option<&NavWorld>, ai: &AiConfig, dt: f32) -> PlayerInput {
        let Some(Some(Behavior::Search(mut s))) = self.slots.get(slot::SEARCH as usize).cloned() else {
            return PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        };
        let out = self.search_step(&mut s, me, ws, nav, ai, dt);
        if let Some(Some(Behavior::Search(slot_s))) = self.slots.get_mut(slot::SEARCH as usize) {
            *slot_s = s;
        }
        self.state = self.project();
        out
    }

    fn search_step(&mut self, s: &mut SearchState, me: &PlayerState, ws: &WorldState, nav: Option<&NavWorld>, ai: &AiConfig, dt: f32) -> PlayerInput {
        let idle = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        let face = |p: Vec3| PlayerInput { yaw: yaw_to(me.position, p), left_hand: LeftHand::Empty, ..Default::default() };
        // Where the NPC believes the target is now (the attention proxy).
        let live = self.attn.iter().find(|a| a.who == s.target).map(|a| a.proxy);
        if s.stationary {
            // Civilians: look toward the stimulus for the behaviour's duration.
            s.timer -= dt;
            if s.timer <= 0.0 {
                self.complete(slot::SEARCH);
                return idle;
            }
            return face(live.unwrap_or(s.proxy));
        }
        let ss = &ai.search;
        let to_track = |b: &mut Brain, s: &mut SearchState| {
            s.sub = SearchSub::TrackTarget;
            b.timer = 0.0;
            b.goal = None;
            b.follow.clear();
        };
        match s.sub {
            SearchSub::Investigate => {
                // A moved proxy is followed (0x00b29f00: new destination, not reached, not facing).
                if let Some(p) = live {
                    if p.distance(s.proxy) >= 1e-4 {
                        s.proxy = p;
                        s.dest = p;
                        s.reached = false;
                        s.finished = false;
                        s.wants_stare = false;
                        self.follow.clear();
                    }
                }
                if !s.finished {
                    // Reached within the proximity threshold (0x00b38530); the facing turn that
                    // follows is immediate here (NPCs turn at once, U-862).
                    if Self::reached(me, s.dest, ss.proximity) {
                        s.reached = true;
                        s.finished = true;
                    } else {
                        let run = self.run;
                        let i = self.move_to(me, nav, s.dest, run, ai);
                        if self.follow.failed {
                            self.pathing_fail(s, me, nav, ai);
                        }
                        if !s.finished {
                            return i;
                        }
                    }
                }
                // The investigation is over (0x00afeb00): stare at an unreachable proxy above,
                // stand when the proxy was not reached, else the search animation (unless it ran
                // within `m_fSearchAnimCooldown`) and then the crumbs.
                self.follow.clear();
                if s.wants_stare {
                    s.sub = SearchSub::StareAtUnreachable;
                    s.timer = ss.stare_max_time;
                } else if !s.reached {
                    s.sub = SearchSub::Stand;
                    let [lo, hi] = ss.cancel_investigate_wait;
                    s.timer = lo + (hi - lo) * rng_next(&mut self.rng);
                } else if s.anim_cooldown > 0.0 {
                    to_track(self, s);
                } else {
                    s.sub = SearchSub::GenericAction;
                    s.timer = if s.corpse { ss.anim_corpse_check } else { ss.anim_lost_target };
                }
                face(s.proxy)
            }
            SearchSub::GenericAction => {
                s.timer -= dt;
                if s.timer <= 0.0 {
                    // Leaving the animation starts the cooldown (0x00af4ab0).
                    s.anim_cooldown = ss.anim_cooldown;
                    to_track(self, s);
                }
                face(s.proxy)
            }
            SearchSub::Stand => {
                s.timer -= dt;
                if s.timer <= 0.0 {
                    to_track(self, s);
                }
                face(s.proxy)
            }
            SearchSub::StareAtUnreachable => {
                // The timer runs only while the target is not seen (0x00af4c40); a target that
                // moved farther than the radius from the proxy ends the stare (0x00af4440).
                let sighted = self.attn.iter().find(|a| a.who == s.target).is_some_and(|a| a.sighted);
                if !sighted {
                    s.timer -= dt;
                }
                let away = s.target != NOISE && ws.player(s.target).is_some_and(|t| t.position.distance(s.proxy) > ss.stare_radius);
                if away || s.timer <= 0.0 {
                    to_track(self, s);
                }
                face(s.proxy)
            }
            SearchSub::TrackTarget => {
                // A proxy that moved away from the searched spot is investigated again (the
                // port's reading of `TopAttnProxyUpdated` in the search).
                if let Some(p) = live {
                    if p.distance(s.proxy) > ss.proximity {
                        s.sub = SearchSub::Investigate;
                        s.proxy = p;
                        s.dest = p;
                        s.reached = false;
                        s.finished = false;
                        s.wants_stare = false;
                        s.fails = 0;
                        self.goal = None;
                        self.follow.clear();
                        return face(p);
                    }
                }
                if self.crumbs == 0 {
                    self.complete(slot::SEARCH);
                    return idle;
                }
                let center = s.proxy;
                match self.goal {
                    Some(g) if !Self::reached(me, g, ss.track_proximity) && !self.follow.failed => self.move_to(me, nav, g, false, ai),
                    _ => {
                        if self.timer < ss.heartbeat {
                            return idle;
                        }
                        // Next crumb: a wander step within the angular range of the facing.
                        self.timer = 0.0;
                        self.crumbs -= 1;
                        let step = ss.wander_min + (ss.wander_max - ss.wander_min) * rng_next(&mut self.rng);
                        let back = (center - me.position).truncate();
                        let base_yaw = if back.length() > ss.crumb_spacing { back.y.atan2(back.x).to_degrees() } else { me.yaw };
                        let yaw = (base_yaw + (rng_next(&mut self.rng) - 0.5) * ss.wander_angle_deg * 2.0).to_radians();
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
        }
    }

    /// `DisAISubStateInvestigate`'s PathingFail handler (0x00b33850): up to
    /// `m_PathingFailRetryCount` retries, each toward the navmesh point nearest the destination
    /// (scaled from the pawn by `m_fPathingFailDistanceScale` after the first) within the
    /// pointer search box (500 uu around, 25 up, 500 down); when none differs from the current
    /// destination the investigation is finished, and a proxy more than 500 uu above the pawn
    /// asks for StareAtUnreachable.
    fn pathing_fail(&mut self, s: &mut SearchState, me: &PlayerState, nav: Option<&NavWorld>, ai: &AiConfig) {
        let ss = &ai.search;
        s.fails += 1;
        let allowed = if ss.pathing_fail_scale == 1.0 { if ss.pathing_fail_retries < 2 { ss.pathing_fail_retries } else { 1 } } else { ss.pathing_fail_retries };
        let [horiz, up, down] = ss.pathing_fail_range;
        while s.fails <= allowed {
            let point = if s.fails > 1 { me.position + (s.dest - me.position) * ss.pathing_fail_scale } else { s.dest };
            if let Some((_, q)) = nav.and_then(|n| n.closest_point_in(point, horiz, up, down)) {
                let q = q + Vec3::Z * 90.0;
                if q.distance(s.dest) >= 1e-4 {
                    s.dest = q;
                    self.follow.clear();
                    return;
                }
            }
            s.fails += 1;
        }
        s.finished = true;
        if s.dest.z - me.position.z > 500.0 {
            s.wants_stare = true;
        }
    }

    /// `DisBehaviorMagicResponse` (slot Awe): face the magic user (unless an ally), bark
    /// `DDH_MAGIC_WITNESSED` and finish (0x00af2800); a user that disappeared gets
    /// `DDH_MAGIC_DISAPPEARED`, and unless it is an ally or in sight the MagicLostTarget
    /// animation plays first (`RotationReached`, 0x00afa1e0).
    fn act_magic(&mut self, me: &PlayerState, ws: &WorldState, ai: &AiConfig, dt: f32) -> PlayerInput {
        let idle = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        let Some(Some(Behavior::Magic(mut m))) = self.slots.get(slot::AWE as usize).cloned() else { return idle };
        let at = ws.player(m.caster).map(|p| p.position).or(self.threat_pos);
        let out = match at {
            Some(p) if !m.ally => PlayerInput { yaw: yaw_to(me.position, p), left_hand: LeftHand::Empty, ..Default::default() },
            _ => idle,
        };
        if !m.barked {
            m.barked = true;
            if !m.disappeared {
                self.barks.push(("DDH_MAGIC_WITNESSED".into(), Some(m.caster)));
                self.complete(slot::AWE);
                return out;
            }
            self.barks.push(("DDH_MAGIC_DISAPPEARED".into(), Some(m.caster)));
            let sighted = self.attn.iter().find(|a| a.who == m.caster).is_some_and(|a| a.sighted);
            if m.ally || sighted {
                self.complete(slot::AWE);
                return out;
            }
            m.anim = Some(ai.anim_magic_lost_target);
        }
        if let Some(t) = m.anim.as_mut() {
            *t -= dt;
            if *t <= 0.0 {
                self.complete(slot::AWE);
                return out;
            }
        }
        if let Some(Some(Behavior::Magic(slot_m))) = self.slots.get_mut(slot::AWE as usize) {
            *slot_m = m;
        }
        out
    }

    /// `DisBehaviorPanic` (civilians): free flight away from the threat, then a flee point;
    /// cower when an enemy is within the cower distance; over once the attention fell below
    /// Investigate.
    fn act_panic(&mut self, me: &PlayerState, ws: &WorldState, layout: &CrowdLayout, nav: Option<&NavWorld>, ai: &AiConfig) -> PlayerInput {
        let idle = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        let face = |p: Vec3| PlayerInput { yaw: yaw_to(me.position, p), left_hand: LeftHand::Empty, ..Default::default() };
        let cower = matches!(self.behavior(slot::ESCAPE), Some(Behavior::Panic { cower: true }));
        let set_cower = |b: &mut Brain, c: bool| {
            if let Some(Some(Behavior::Panic { cower })) = b.slots.get_mut(slot::ESCAPE as usize) {
                *cower = c;
            }
        };
        // The threat follows a live enemy.
        if let Some((who, _, _, _)) = self.focus() {
            if who != NOISE && self.enemies.contains(&who) {
                if let Some(p) = ws.player(who).filter(|p| p.alive) {
                    self.threat_pos = Some(p.position);
                }
            }
        }
        let from = self.threat_pos.unwrap_or(me.position);
        if cower {
            let enemy_close = self.enemies.iter().filter_map(|h| ws.player(*h)).any(|p| p.alive && p.position.distance(me.position) < ai.panic.cower_distance * 1.5);
            if !enemy_close && self.timer > 1.0 {
                set_cower(self, false);
                self.timer = ai.panic.free_flight_time;
                self.goal = None;
            }
            self.state = self.project();
            let mut i = face(from);
            i.buttons |= buttons::CROUCH;
            return i;
        }
        let enemy_close = self.enemies.iter().filter_map(|h| ws.player(*h)).any(|p| p.alive && p.position.distance(me.position) < ai.panic.cower_distance);
        if enemy_close {
            set_cower(self, true);
            self.timer = 0.0;
            self.follow.clear();
            self.state = self.project();
            return face(from);
        }
        if self.focus().is_none_or(|f| f.1 < DAL_INVESTIGATE) {
            self.threat_pos = None;
            self.complete(slot::ESCAPE);
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
                i.npc_speed = self.loco_speed(ai, true);
                return i;
            }
        }
        match self.goal {
            Some(g) if !Self::reached(me, g, 120.0) && !self.follow.failed => self.move_to(me, nav, g, true, ai),
            _ => face(from),
        }
    }

    /// `DisBehaviorTriggerAlarm`: TakePosition to the bell (running), stand
    /// `m_fWaitTimeBeforeRing` within `m_fNPCUseCloseness`, ring (GenericAction). Over when
    /// rung, taken by another guard, with the enemy within the fight radius, or unreachable.
    fn act_alarm(&mut self, me: &PlayerState, ws: &WorldState, ctx: &mut Ctx, nav: Option<&NavWorld>, ai: &AiConfig, dt: f32) -> PlayerInput {
        let idle = PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        let face = |p: Vec3| PlayerInput { yaw: yaw_to(me.position, p), left_hand: LeftHand::Empty, ..Default::default() };
        let Some(bi) = self.bell.filter(|&b| b < ctx.alarms.len()) else {
            self.bell = None;
            self.complete(slot::ALERT);
            return idle;
        };
        let enemy = ctx.alarms[bi].enemy.and_then(|e| ws.player(e)).filter(|p| p.alive).map(|p| (p.id, p.position));
        let taken = ctx.alarms[bi].ringing || ctx.alarms[bi].ringer.is_some_and(|r| r != self.id);
        let close = enemy.is_some_and(|(_, p)| p.distance(me.position) <= ai.trigger_alarm.fight_radius);
        let pos = ctx.alarms[bi].def.position;
        if taken || close || self.follow.failed {
            if ctx.alarms[bi].ringer == Some(self.id) && !ctx.alarms[bi].ringing {
                ctx.alarms[bi].ringer = None;
            }
            self.bell = None;
            self.complete(slot::ALERT);
            return idle;
        }
        if Self::reached(me, pos, ctx.alarms[bi].def.use_closeness.max(60.0)) {
            self.ring_wait += dt;
            if self.ring_wait >= ai.trigger_alarm.wait_before_ring {
                let a = &mut ctx.alarms[bi];
                a.ringing = true;
                a.noise_done = false;
                a.noise_timer = a.def.ring_delay;
                ctx.events.push(AiEvent::AlarmRang { bell: bi, name: a.def.name.clone(), ringer: self.id, enemy: a.enemy });
                self.bell = None;
                self.complete(slot::ALERT);
            }
            return face(pos);
        }
        self.move_to(me, nav, pos, true, ai)
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
            // Target gone: the fight is over; the dormant search (or the routine) resumes.
            self.target = None;
            self.complete(slot::COMBAT);
            return idle;
        };
        // The target is chased while sighted; otherwise the guard goes to the attention proxy
        // (where it proves stale, attention falls to Investigate: see `perceive`).
        let sighted = self.attn.iter().find(|a| a.who == t.id).is_some_and(|a| a.sighted);
        let goal = if sighted { t.position } else { self.attn.iter().find(|a| a.who == t.id).map(|a| a.proxy).unwrap_or(t.position) };
        if sighted {
            self.threat_pos = Some(t.position);
        }
        let veye = self.vision_eye(me, ai);
        let visible = sighted && Self::sees(&veye, &t, world, ai);
        let d = t.position.distance(me.position);
        let dz = t.position.z - me.position.z;
        let reach = ai.combat.reachable_melee_range;
        // The special classes' own combat behaviours (`special`).
        if !matches!(self.class, NpcClass::EliteGuard | NpcClass::Civilian) {
            let tg = special::Target { t: &t, sighted, visible, goal, d, dz };
            if let Some(i) = self.special_combat(me, &tg, ws, world, nav, ai, inhibit, dt) {
                return i;
            }
        }
        let face = yaw_to(me.position, goal);
        let mut i = PlayerInput { yaw: face, left_hand: LeftHand::Empty, ..Default::default() };
        let ctxs = &ai.combat.melee_contexts;
        let dg = &ai.combat.dodge;
        self.melee_cd.resize(ctxs.len(), 0.0);
        for c in self.melee_cd.iter_mut() {
            *c = (*c - dt).max(0.0);
        }
        // `m_fGunTimer` (set from `m_GunUsageStart` when the behaviour starts, 0x00b0e130;
        // `m_GunUsageCooldown` after the pistol sub-state, 0x00b10f60) runs down while fighting.
        if !self.in_fight {
            self.in_fight = true;
            self.gun_timer = combat::roll(ai.combat.gun_usage_start, rng_next(&mut self.rng));
            self.flurry = combat::roll_i(dg.attacks_per_flurry, rng_next(&mut self.rng));
            self.stamina_timer = combat::roll(dg.stamina_recovery, rng_next(&mut self.rng));
            // `m_fAggression`: uniform in [0, 1) when ManageAttacks starts (0x00b3dc10).
            self.aggression = rng_next(&mut self.rng);
        }
        // `m_fAggressionRegenRate` per second, clamped to [0, 1] (0x00b39e50).
        self.aggression = (self.aggression + dg.aggression_regen_rate * dt).clamp(0.0, 1.0);
        self.gun_timer -= dt;
        // Attack pacing (0x00b47880): a flurry of stamina-draining attacks, then a recovery
        // pause before the next flurry.
        if self.flurry < 1 && self.attack == 0 {
            if self.stamina_timer < 0.0 {
                self.flurry = combat::roll_i(dg.attacks_per_flurry, rng_next(&mut self.rng));
                self.stamina_timer = combat::roll(dg.stamina_recovery, rng_next(&mut self.rng));
            } else {
                self.stamina_timer -= dt;
            }
        }
        // Incoming melee attacks (back step after `m_IncomingAttacksBeforeBackStep` within
        // `m_fTimeToForgiveIncomingAttacks`) and ranged threats aimed at this NPC (side step
        // after `m_TimeForThreatToCauseSideStep`).
        let swinging = matches!(t.sword.phase, SwordPhase::Startup);
        let to_me = (me.position - t.position).truncate();
        let aim_dir = glam::Vec2::new(t.yaw.to_radians().cos(), t.yaw.to_radians().sin());
        let off_axis = aim_dir.angle_to(to_me.normalize_or(aim_dir)).abs().to_degrees();
        if swinging && !self.target_swinging && off_axis <= dg.angle_melee_threat && d <= reach + 150.0 {
            if self.incoming_needed <= 0 {
                self.incoming_needed = combat::roll_i(dg.incoming_attacks_before_back_step, rng_next(&mut self.rng));
            }
            self.incoming += 1;
            self.forgive_timer = dg.forgive_incoming_attacks;
        }
        self.target_swinging = swinging;
        self.forgive_timer -= dt;
        if self.forgive_timer <= 0.0 {
            self.incoming = 0;
        }
        let aiming_at_me = t.left.phase == dishonored_sim::state::LeftPhase::Aiming && (t.left.item == LeftHand::Pistol || t.left.item.is_crossbow()) && off_axis <= dg.angle_ranged_threat;
        if aiming_at_me && visible {
            if self.threat_needed < 0.0 {
                self.threat_needed = combat::roll(dg.time_for_threat_side_step, rng_next(&mut self.rng));
            }
            self.threat_timer += dt;
        } else {
            self.threat_timer = 0.0;
            self.threat_needed = -1.0;
        }
        // A move in progress.
        if self.attack != 0 {
            self.attack_timer -= dt;
            let kind = self.attack;
            i.npc_move = kind;
            match kind {
                npc_moves::SIDE_STEP | npc_moves::BACK_STEP => {
                    if self.attack_timer <= 0.0 {
                        self.attack = 0;
                    } else if kind == npc_moves::SIDE_STEP {
                        i.right = 127 * self.dodge_dir;
                    } else {
                        i.forward = -127;
                    }
                    return i;
                }
                _ => {
                    // The attack travels to the target (root motion of the long and jump
                    // attacks), swinging once within the sword's reach.
                    let close = d <= SWORD_REACH;
                    if close && me.sword.phase == SwordPhase::Idle {
                        i.buttons |= buttons::PRIMARY;
                        if kind == npc_moves::ATTACK_STEP_BACK {
                            i.forward = -127;
                        }
                        self.attack = 0;
                        return i;
                    }
                    if self.attack_timer <= 0.0 || !sighted {
                        self.attack = 0;
                    } else if !close && combat::lunges(kind) {
                        i.forward = 127;
                        i.buttons |= buttons::SPRINT;
                        if kind == npc_moves::JUMP_ATTACK && self.attack_timer > ATTACK_WINDOW - dt * 1.5 {
                            i.buttons |= buttons::JUMP;
                        }
                    }
                    return i;
                }
            }
        }
        // Dodges (`DisItemContext_NPCBackStep` / `_NPCSideStep`, with their group inhibition).
        let dodge_ok = |k: u8| !inhibit.iter().any(|x| x.0 == t.id && x.1 == 200 + k);
        if self.incoming_needed > 0 && self.incoming >= self.incoming_needed && dodge_ok(npc_moves::BACK_STEP) {
            self.incoming = 0;
            self.incoming_needed = 0;
            inhibit.push((t.id, 200 + npc_moves::BACK_STEP, dg.back_step_inhibition));
            return self.start_dodge(npc_moves::BACK_STEP, dg.back_step_distance, i);
        }
        if self.threat_needed >= 0.0 && self.threat_timer >= self.threat_needed && dodge_ok(npc_moves::SIDE_STEP) {
            self.threat_timer = 0.0;
            self.threat_needed = -1.0;
            self.dodge_dir = if rng_next(&mut self.rng) < 0.5 { -1 } else { 1 };
            inhibit.push((t.id, 200 + npc_moves::SIDE_STEP, dg.side_step_inhibition));
            return self.start_dodge(npc_moves::SIDE_STEP, dg.side_step_distance, i);
        }
        // The pistol (`DisAISubStateFindShootingPosition` / `DisAISubStateFirePistol`).
        let reachable = nav.is_none_or(|_| !self.follow.failed);
        let still = me.velocity.x.abs() < 1e-4 && me.velocity.y.abs() < 1e-4 && me.velocity.z.abs() < 1e-4;
        let (start, cont) = combat::pistol_willing(&ai.combat.pistol, d, visible, self.gun_timer, still, reachable, !self.melee_slot);
        if self.ammo > 0 && (self.firing && cont || start) {
            if !self.firing {
                // Entering the sub-state (0x00b32c50): give-up and first-shot timers, the
                // `DDH_COMBAT_RANGED_BEGIN` bark.
                self.firing = true;
                self.give_up = combat::roll(ai.combat.pistol.time_before_giving_up, rng_next(&mut self.rng));
                self.fire_timer = combat::roll(ai.combat.pistol.time_to_first_shot, rng_next(&mut self.rng));
                self.barks.push(("DDH_COMBAT_RANGED_BEGIN".into(), Some(t.id)));
            }
            self.give_up -= dt;
            self.fire_timer -= dt;
            i.left_hand = LeftHand::Pistol;
            i.npc_move = npc_moves::FIRE_PISTOL;
            let aim = t.position + Vec3::Z * 40.0 - (me.position + Vec3::Z * EYE);
            i.pitch = (aim.z / aim.truncate().length().max(1.0)).atan().to_degrees();
            self.aim += dt;
            let ready = me.left.item == LeftHand::Pistol && me.left.phase == dishonored_sim::state::LeftPhase::Idle && me.left.pistol_loaded > 0;
            if self.fire_timer <= 0.0 && self.aim >= ai.combat.aim_time_before_shooting && ready {
                i.buttons |= buttons::SECONDARY;
                self.ammo -= 1;
                self.aim = 0.0;
                if rng_next(&mut self.rng) >= ai.combat.pistol.chance_fire_again || self.ammo == 0 {
                    self.stop_firing(ai);
                } else {
                    self.fire_timer = combat::roll(ai.combat.pistol.time_to_first_shot, rng_next(&mut self.rng));
                }
            } else if self.give_up <= 0.0 {
                self.stop_firing(ai);
            }
            return i;
        }
        if self.firing {
            self.stop_firing(ai);
        }
        self.aim = 0.0;
        if !self.melee_slot {
            // Outer skirmish ring: hold distance, stopping to shoot once the gun timer allows.
            if goal.distance(me.position) > ai.combat.pistol.max_range.min(1500.0) {
                return self.move_to(me, nav, goal, true, ai);
            }
            if d < ai.combat.pistol.min_range {
                i.forward = -80;
            }
            return i;
        }
        // Melee: the first context of the pattern that fits (`combat::pick_context`), while
        // the flurry has attacks left.
        if sighted && self.flurry > 0 && me.sword.phase == SwordPhase::Idle {
            if let Some(k) = combat::pick_context(ctxs, &self.melee_cd, inhibit, t.id, d, dz) {
                // The context's cooldown, scaled by `(1 - aggression) x
                // m_fCooldownFactorAtMinAggression + 1` (0x00b3deb0).
                let scale = (1.0 - self.aggression) * dg.cooldown_factor_at_min_aggression + 1.0;
                self.melee_cd[k] = combat::roll(ctxs[k].cooldown, rng_next(&mut self.rng)) * scale;
                if ctxs[k].inhibition > 0.0 {
                    inhibit.push((t.id, k as u8, ctxs[k].inhibition));
                }
                self.flurry -= 1;
                let kind = if ctxs[k].kind == 0 { npc_moves::ATTACK_SHORT } else { ctxs[k].kind };
                // `DisTweaks_MeleeAttackNPCBase.m_AttackBark` (the jump attack's is
                // `DDH_COMBAT_BIG_ATTACK`).
                let bark = if kind == npc_moves::JUMP_ATTACK { "DDH_COMBAT_BIG_ATTACK" } else { "DDH_COMBAT_ATTACK" };
                self.barks.push((bark.into(), Some(t.id)));
                self.attack = kind;
                self.attack_timer = ATTACK_WINDOW;
                i.npc_move = kind;
                if d <= SWORD_REACH {
                    i.buttons |= buttons::PRIMARY;
                    if kind == npc_moves::ATTACK_STEP_BACK {
                        i.forward = -127;
                    }
                    self.attack = 0;
                } else {
                    i.forward = 127;
                    i.buttons |= buttons::SPRINT;
                    if kind == npc_moves::JUMP_ATTACK {
                        i.buttons |= buttons::JUMP;
                    }
                }
                return i;
            }
        }
        if !sighted || d > reach {
            let mut m = self.move_to(me, nav, goal, true, ai);
            if sighted && d < 400.0 {
                m.yaw = face;
            }
            return m;
        }
        if t.sword.phase == SwordPhase::Startup {
            i.buttons |= buttons::BLOCK;
        }
        i
    }

    fn start_dodge(&mut self, kind: u8, distance: f32, mut i: PlayerInput) -> PlayerInput {
        self.attack = kind;
        // Covered at sprint speed (the dodge animations' root motion).
        self.attack_timer = distance / DODGE_SPEED;
        i.npc_move = kind;
        if kind == npc_moves::SIDE_STEP {
            i.right = 127 * self.dodge_dir;
        } else {
            i.forward = -127;
        }
        i
    }

    /// `DisBehaviorShoot` (see `ShootOrder`): standing in place, facing the target, the pistol
    /// out; a shot when the Relaxed wait is over, the weapon has been aimed for
    /// `aim_time_before_shooting` (`StateNPCMasterAim`) and a round is chambered. The order
    /// ends after its last shot or when the target is gone.
    fn kismet_shoot(&mut self, me: &PlayerState, ws: &WorldState, ai: &AiConfig, dt: f32) -> PlayerInput {
        let Some(o) = self.shoot.as_mut() else { return PlayerInput { yaw: me.yaw, ..Default::default() } };
        let Some(t) = ws.player(o.target).filter(|p| p.alive) else {
            self.shoot = None;
            self.aim = 0.0;
            return PlayerInput { yaw: me.yaw, left_hand: LeftHand::Empty, ..Default::default() };
        };
        let mut i = PlayerInput { yaw: yaw_to(me.position, t.position), left_hand: LeftHand::Pistol, npc_move: npc_moves::FIRE_PISTOL, ..Default::default() };
        let aim = t.position + Vec3::Z * 40.0 - (me.position + Vec3::Z * EYE);
        i.pitch = (aim.z / aim.truncate().length().max(1.0)).atan().to_degrees();
        o.wait -= dt;
        self.aim += dt;
        let ready = me.left.item == LeftHand::Pistol && me.left.phase == dishonored_sim::state::LeftPhase::Idle && me.left.pistol_loaded > 0;
        if o.wait <= 0.0 && self.aim >= ai.combat.aim_time_before_shooting && ready {
            i.buttons |= buttons::SECONDARY;
            self.aim = 0.0;
            if let Some(n) = o.shots_left.as_mut() {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    self.shoot = None;
                }
            }
        }
        i
    }

    /// Mission control (`DisSeqAct_AIShoot` Start): shoot at `target`, `max_shots` < 1 meaning
    /// forever. A running order is replaced.
    pub fn order_shoot(&mut self, target: PlayerId, max_shots: i32, accuracy: Option<f32>) {
        self.shoot = Some(ShootOrder { target, shots_left: if max_shots < 1 { None } else { Some(max_shots as u32) }, wait: 0.0, accuracy });
        self.aim = 0.0;
        self.follow.clear();
        self.goal = None;
    }

    /// Leaving the pistol sub-state: `m_fGunTimer` = `m_GunUsageCooldown` (0x00b10f60).
    fn stop_firing(&mut self, ai: &AiConfig) {
        self.firing = false;
        self.aim = 0.0;
        self.gun_timer = combat::roll(ai.combat.gun_usage_cooldown, rng_next(&mut self.rng));
    }

    /// The original's bark points (`FUN_00af00f0` callers) mapped onto this brain's
    /// transitions: Notice begin/end (`DisBehaviorNotice`), search start by cause (vision near
    /// / far by `m_fVisualDetectionRatioNear`, hearing / danger, corpse, witnessed death, from
    /// an ally, alarm), search end (rat / post-corpse / stop hunting, alone or group), search
    /// finding the enemy, combat start (threat, group threat, offscreen) and loss (lost /
    /// group), panic start, begging when cowering, fleeing, going to ring an alarm.
    fn transition_barks(&mut self, was: BrainState, me: &PlayerState, ws: &WorldState, peers: &[Peer], ai: &AiConfig) {
        let now = self.state;
        if now == was {
            return;
        }
        let target_of = |b: &Brain| b.target.or_else(|| b.attn.iter().filter(|a| a.who != NOISE).max_by(|x, y| x.value.total_cmp(&y.value)).map(|a| a.who));
        let tgt = target_of(self);
        // Another guard on the same target (group variants name it as the initiator).
        let ally = |state: BrainState| tgt.and_then(|t| peers.iter().find(|p| p.id != self.id && p.kind == PawnKind::Guard && p.state == state && p.target == Some(t)).map(|p| p.id));
        let mut bark = |h: &str, i: Option<PlayerId>| self.barks.push((h.to_string(), i));
        match (was, now) {
            (_, BrainState::Notice) => bark("DDH_ATTENTION_NOTICE_BEGIN", tgt),
            (BrainState::Notice, s) if s.is_routine() => bark("DDH_ATTENTION_NOTICE_END", tgt),
            (_, BrainState::RingAlarm) => bark("DDH_ALARM_GO_TO_RING_ALARM", self.alarm_enemy.filter(|e| *e != NOISE)),
            // Combat over: the dormant search resumes.
            (BrainState::Combat, BrainState::Investigate | BrainState::Search) => match ally(BrainState::Combat) {
                Some(a) => bark("DDH_COMBAT_LOST_GROUP", Some(a)),
                None => bark("DDH_COMBAT_LOST", None),
            },
            (_, BrainState::Investigate) if self.kind != PawnKind::Guard => {
                bark(if self.cause == reason::ALARM { "DDH_ALARM_HEAR_AND_RESPOND" } else { "DDH_ATTENTION_INVESTIGATE" }, tgt)
            }
            (_, BrainState::Investigate) => {
                let h = match self.cause {
                    reason::SEEN => {
                        let near = self.attn.iter().find(|a| Some(a.who) == tgt).is_some_and(|a| a.los_norm <= ai.visual_detection_ratio_near);
                        if near { "DDH_SEARCH_NEAR_VDETECTION" } else { "DDH_SEARCH_FAR_VDETECTION" }
                    }
                    reason::HEARD if self.cause_danger => "DDH_SEARCH_ADETECTION_DANGER",
                    reason::HEARD => "DDH_SEARCH_ADETECTION",
                    reason::FOUND_CORPSE => "DDH_SEARCH_DETECT_CORPSE",
                    reason::HEARD_DEATH | reason::SAW_DEATH => "DDH_SEARCH_WITNESS_DEATH",
                    reason::HEARD_HELP_REQUEST => "DDH_SEARCH_ADETECTION_FROM_ALLY",
                    reason::SEE_ALLY_COMBAT | reason::SEE_ALLY_PANIC | reason::SEE_ALLY_SEARCH => "DDH_SEARCH_VDETECTION_FROM_ALLY",
                    reason::ALARM => "DDH_ALARM_HEAR_AND_RESPOND",
                    _ => "DDH_SEARCH_ADETECTION",
                };
                bark(h, tgt.filter(|t| *t != NOISE));
            }
            (_, BrainState::Search) => bark("DDH_SEARCH", None),
            (BrainState::Search | BrainState::Investigate, s) if s.is_routine() && self.kind == PawnKind::Guard => {
                let hunting = self.cause == reason::SEEN || self.cause == reason::ATTACKED_BY || self.cause == reason::ATTACKED_ENEMY;
                if hunting {
                    match ally(BrainState::Search) {
                        Some(a) => bark("DDH_SEARCH_STOPHUNTING_GROUP", Some(a)),
                        None => bark("DDH_SEARCH_STOPHUNTING", None),
                    }
                } else if self.cause == reason::FOUND_CORPSE {
                    bark("DDH_SEARCH_GIVEUP_POSTCORPSE", None);
                } else {
                    bark("DDH_SEARCH_MUSTHAVEBEENARAT", None);
                }
            }
            (BrainState::Search | BrainState::Investigate, BrainState::Combat) => bark("DDH_SEARCH_FIND_ENEMY", tgt),
            (_, BrainState::Combat) => {
                // Combat barks sub-process (0x00b41f70): offscreen while the enemy is not
                // seen, else threat, or group threat when an ally already fights it.
                let seen = self.attn.iter().find(|a| Some(a.who) == tgt).is_some_and(|a| a.sighted);
                if !seen {
                    bark("DDH_COMBAT_THREAT_OFFSCREEN", tgt);
                } else if let Some(a) = ally(BrainState::Combat) {
                    bark("DDH_COMBAT_THREAT_GROUP", Some(a));
                } else {
                    bark("DDH_COMBAT_THREAT", tgt);
                }
            }
            (_, BrainState::Flee) if !matches!(was, BrainState::Cower) => bark("DDH_PANIC_START", tgt),
            (BrainState::Cower, BrainState::Flee) => bark("DDH_FLEEING", tgt),
            (_, BrainState::Cower) => bark("DDH_COMBAT_BEGGING", tgt),
            _ => {}
        }
        let _ = (me, ws);
    }

    /// `DisAISubProcessAmbientBarks` of the patrol and guard behaviours (their tweaks own the
    /// sub-process): activation (0x00b47020) sets the inhibition timer to
    /// `m_fBarkInhibitionDuration`; each tick (0x00b3d5e0) counts it down, then asks for
    /// `DDH_PATROL_MISC` every tick: the hook's refire delay (5-12 s per NPC) and the Casual
    /// group spacing pace it. Not ported: the group variant `DDH_PATROL_MISC_GROUP` (a nearby
    /// pawn in AI state 1, `FUN_00b14c00` / `FUN_00b043a0` not decoded), the route-switch bark
    /// and the LOD disable.
    fn ambient_barks(&mut self, ai: &AiConfig, dt: f32) {
        let ambient = self.state == BrainState::Patrol || (self.state == BrainState::Idle && self.kind == PawnKind::Guard);
        if !ambient {
            self.ambient_bark_timer = None;
            return;
        }
        // Activation (also the first tick of a brain that spawns patrolling).
        let timer = self.ambient_bark_timer.get_or_insert(ai.ambient_bark_inhibition);
        if *timer > 0.0 {
            *timer -= dt;
            return;
        }
        self.barks.push(("DDH_PATROL_MISC".into(), None));
    }

    /// How this brain perceives its top target (the initiator of its attention barks), for
    /// `DisConv_Hook_Notice` (0x00cac450, `FUN_00b5bac0` on the owner's brain and the
    /// initiator): the attention entry's change reason 19 (seen) -> 0, 7 (heard) -> 1, or 2 for
    /// a danger noise (context above 3); anything else -> None (the hook cannot start).
    pub fn notice_kind(&self) -> Option<u8> {
        let who = self.target.or_else(|| self.attn.iter().filter(|a| a.who != NOISE).max_by(|x, y| x.value.total_cmp(&y.value)).map(|a| a.who))?;
        let a = self.attn.iter().find(|a| a.who == who)?;
        match a.why {
            reason::SEEN => Some(0),
            reason::HEARD => Some(1 + a.danger as u8),
            _ => None,
        }
    }

    /// The combat behaviour ends: moves, dodges and the pistol sub-state stop.
    fn end_fight(&mut self) {
        self.in_fight = false;
        self.attack = 0;
        self.attack_timer = 0.0;
        self.firing = false;
        self.aim = 0.0;
        self.incoming = 0;
        self.threat_timer = 0.0;
        self.threat_needed = -1.0;
        self.special = SpecialCombat::default();
    }

    /// Combat move in progress (`npc_moves`), for animation.
    pub fn attack_kind(&self) -> combat::AttackKind {
        if self.firing { npc_moves::FIRE_PISTOL } else { self.attack }
    }

    /// Is this NPC "unaware" for a sleep dart (`DishonoredDamageType.m_bInstantSleepForUnawareNPCs`)?
    /// The damage code (0x00b83680) skips `m_fTimeBeforeSleep` when the pawn's virtual
    /// 0x6c0 (0x00b6e340) is false: that tests the brain's `EAIAwareness` (0x00b0c1e0, the
    /// current behaviour's awareness) for `EAIAwareness_InCombat` (5). So every NPC that is not
    /// fighting sleeps at once, whoever shot it (`_who` is kept for the call site). Ringing an
    /// alarm counts as combat here (LIKELY: the TriggerAlarm behaviour's awareness was not read).
    pub fn unaware_of(&self, _who: PlayerId) -> bool {
        // The reaction delay is `EAIAwareness_Surprised` (0x00b0c1e0 returns 2): not combat yet.
        !matches!(self.state, BrainState::Combat | BrainState::RingAlarm)
    }

    /// The pawn's `EAIAwareness` (0x00B0C1E0): `Surprised` during a reaction delay, else the
    /// awareness of the current behaviour. The port's states map to the original behaviours'
    /// (LIKELY: the behaviours' awareness values were not read): fighting and ringing an alarm
    /// `InCombat`, panic `Fearful`, notice / investigate / search `Suspicious`, routines
    /// `Unaware`. `AwareOfPlayer` and `Begging` have no port state.
    pub fn awareness(&self) -> u8 {
        use dishonored_sim::state::awareness as a;
        match self.state {
            // MagicResponse (slot Awe): LIKELY Surprised; the behaviour's awareness getter (vtable
            // +0x188 via 0x00AE4AE0) did not resolve cleanly in the index (U-862 notes).
            BrainState::React | BrainState::Awe => a::SURPRISED,
            BrainState::Combat | BrainState::RingAlarm => a::IN_COMBAT,
            BrainState::Flee | BrainState::Cower => a::FEARFUL,
            BrainState::Notice | BrainState::Investigate | BrainState::Search => a::SUSPICIOUS,
            BrainState::Idle | BrainState::Wander | BrainState::WalkToPoint | BrainState::GroupIdle | BrainState::Patrol | BrainState::Alert => a::UNAWARE,
        }
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
    fn seen_value_matches_the_original_on_boyle_ext() {
        // Engine hook, 2026-10-06 (research/dishonored/HOOK_TRIAGE.md): a party guard's
        // `PlayerAttnInfo` debug values with the player still in front of it. The level's
        // light range (`Pvs_BoyleExterior` 0.2 / 0.5) normalizes the raw light.
        let mut crowd = Crowd::default();
        let level = AiLevel { player_light_range: Some([0.2, 0.5]), ..Default::default() };
        crowd.set_level(&level);
        let pv = &crowd.ai.player_visibility;
        assert_eq!((pv.light_darkest, pv.light_brightest), (0.2, 0.5));
        let prof = AttentionConfig::guard_unsuspecting();
        // (raw light, LOS norm, speed / 600, the original's unclamped value)
        for (light, norm, speed, want) in [(0.3126, 0.145, 0.0, 0.788), (0.1897, 0.164, 0.0, 0.703), (0.1933, 0.214, 0.0, 0.582), (0.1972, 0.358, 0.0, 0.226), (0.191, 0.1402, 0.4228, 0.869)] {
            let vis = player_visibility(pv, light01(light, [pv.light_darkest, pv.light_brightest]), speed, VisState { sprinting: speed > 0.0, ..Default::default() });
            let v = prof.distance_tuning.apply(1.0 - norm, vis);
            assert!((v - want).abs() < 0.006, "light {light} norm {norm}: {v} vs {want}");
        }
        // Another level without a range gets the global one back.
        crowd.set_level(&AiLevel::default());
        assert_eq!(crowd.ai.player_visibility.light_darkest, PlayerVisibilityConfig::default().light_darkest);
    }

    #[test]
    fn player_visibility_light_speed_stance() {
        let pv = PlayerVisibilityConfig::default();
        let range = [pv.light_darkest, pv.light_brightest];
        let l = |raw: f32| light01(raw, range);
        let st = VisState::default();
        // Lit, standing: light 0.1 * 1.0.
        assert!(close(player_visibility(&pv, l(1.0), 0.0, st), 0.1));
        // Lit, full speed: + 0.25.
        assert!(close(player_visibility(&pv, l(1.0), 1.0, VisState { sprinting: true, ..st }), 0.35));
        // Sneaking: the speed counts as zero.
        assert!(close(player_visibility(&pv, l(1.0), 1.0, VisState { sneaking: true, ..st }), 0.1));
        // Darkest light: nothing from light.
        assert!(close(player_visibility(&pv, l(0.05), 0.0, st), 0.0));
        // 0x00AD1590: under water -1 (m_fVisLevelBonus_Swimming) and the speed counts; at the
        // surface the surface bonus; auto-crouching -1 with zero speed; sliding its own bonus.
        assert!(close(player_visibility(&pv, l(1.0), 1.0, VisState { swimming: Some(true), sneaking: true, ..st }), 0.35 - 1.0));
        assert!(close(player_visibility(&pv, l(1.0), 0.0, VisState { swimming: Some(false), ..st }), 0.1 + pv.bonus_swimming_surface));
        assert!(close(player_visibility(&pv, l(1.0), 1.0, VisState { autocrouch: true, ..st }), 0.1 - 1.0));
        assert!(close(player_visibility(&pv, l(1.0), 1.0, VisState { sliding: true, ..st }), 0.35 + pv.bonus_sliding));
        // An empty light range reads 0.5 (0x00BC9040).
        assert_eq!(light01(0.3, [0.4, 0.4]), 0.5);
    }

    /// A stealth volume's range replaces the map's inside it; the highest priority wins and a
    /// head without settings falls through to the map's (0x00BD0120, 0x00C8C280).
    #[test]
    fn stealth_volume_light_range() {
        use dishonored_ir::ai::StealthVolume;
        let cube = |c: f32| vec![vec![[1.0, 0.0, 0.0, c + 100.0], [-1.0, 0.0, 0.0, -(c - 100.0)], [0.0, 1.0, 0.0, 100.0], [0.0, -1.0, 0.0, 100.0], [0.0, 0.0, 1.0, 100.0], [0.0, 0.0, -1.0, 100.0]]];
        let level = AiLevel {
            player_light_range: Some([0.2, 0.5]),
            stealth_volumes: vec![
                StealthVolume { name: "low".into(), priority: 0, light_range: Some([0.0, 0.1]), hulls: cube(0.0) },
                StealthVolume { name: "high".into(), priority: 5, light_range: Some([0.3, 0.9]), hulls: cube(50.0) },
                StealthVolume { name: "empty".into(), priority: 9, light_range: None, hulls: cube(500.0) },
            ],
            ..Default::default()
        };
        assert_eq!(level.light_range_at(Vec3::new(-80.0, 0.0, 0.0)), Some([0.0, 0.1]));
        assert_eq!(level.light_range_at(Vec3::new(20.0, 0.0, 0.0)), Some([0.3, 0.9]));
        assert_eq!(level.light_range_at(Vec3::new(500.0, 0.0, 0.0)), Some([0.2, 0.5]));
        assert_eq!(level.light_range_at(Vec3::new(2000.0, 0.0, 0.0)), Some([0.2, 0.5]));
    }

    #[test]
    fn increase_floors_at_threshold_and_reason_gates_busted() {
        let p = AttentionConfig::guard_unsuspecting();
        let stop = p.stop;
        let mut a = Attn { who: NOISE, value: 0.0, level: 0, proxy: Vec3::ZERO, fresh: false, urgent: false, sighted: false, los_norm: 0.0, why: 0, danger: false };
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
            b.make_hostile(1);
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
        // The attention poster stepped the behaviour level up to Busted: the begin stimuli of
        // every level, with no delay (0x00b25890 / 0x00b24d00, delay getter 0x008fc830).
        let posted: Vec<Stim> = b.stims.iter().map(|q| q.stim).collect();
        assert_eq!(posted, [Stim::NoticeBegin, Stim::SearchBegin, Stim::CombatBegin]);
        assert!(b.stims.iter().all(|q| q.delay == 0.0 && q.who == 1));
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
        let shot = SimEvent::Sound { position: Vec3::new(-500.0, 0.0, 90.0), radius: 1500.0, category: SoundCategory::Pistol, context: NoiseContext::Danger, source: 7 };
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

    /// 0x00B40B70: a Footfall noise is an anomaly noise (`m_IncreaseForAnomalySound`, +0.2 up to
    /// HeadTrack) whose stimulus is lifted from the floor contact to the walker's height; an
    /// ambient white noise raises nothing.
    #[test]
    fn footfall_noise_is_an_anomaly_at_the_walker() {
        let ai = AiConfig::default();
        let w = flat_world();
        let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let walker = pawn(1, PawnKind::Human, Vec3::new(-400.0, 300.0, 90.0), 0.0);
        let mut ws = WorldState::default();
        ws.players = vec![guard.clone(), walker];
        let mut b = Brain::new(guard.id, PawnKind::Guard, guard.position, 1);
        let ambient = SimEvent::Sound { position: Vec3::new(-400.0, 300.0, 2.5), radius: 600.0, category: SoundCategory::Footstep, context: NoiseContext::AmbientWhiteNoise, source: 1 };
        b.perceive(&guard, &ws, std::slice::from_ref(&ambient), &[], &[], &[], &w, &ai, 1.0 / 60.0);
        assert!(b.focus().is_none());
        let step = SimEvent::Sound { position: Vec3::new(-400.0, 300.0, 2.5), radius: 600.0, category: SoundCategory::Footstep, context: NoiseContext::Footfall, source: 1 };
        b.perceive(&guard, &ws, std::slice::from_ref(&step), &[], &[], &[], &w, &ai, 1.0 / 60.0);
        assert_eq!(b.focus().map(|f| f.1), Some(DAL_HEAD_TRACK));
        // The attention goes to the instigator (the walker, id 1).
        let a = b.attn.iter().find(|a| a.who == 1).unwrap();
        let prof = b.profile(&ai, 1).clone();
        assert!((a.value - prof.anomaly_sound.amount.max(prof.start[0])).abs() < 1e-3, "{}", a.value);
        assert_eq!(a.proxy, Vec3::new(-400.0, 300.0, 90.0));
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
            ..Default::default()
        }
    }

    #[test]
    fn routes_adopt_by_distance_and_capacity_and_advance_by_type() {
        let routes = vec![route(2, 3, 0.0), route(1, 3, 5000.0), route(0, 2, 10000.0)];
        let mut users = vec![0; 3];
        let mut a = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 1);
        let mut b = Brain::new(NPC_ID_BASE + 1, PawnKind::Guard, Vec3::ZERO, 2);
        assert!(a.adopt_route(&routes, &mut users, &[], Vec3::new(900.0, 0.0, 90.0), None));
        assert_eq!((a.patrol, a.route_i), (Some(0), 2));
        // Capacity 1: the second guard takes the next nearest route.
        assert!(b.adopt_route(&routes, &mut users, &[], Vec3::new(900.0, 0.0, 90.0), None));
        assert_eq!(b.patrol, Some(1));
        // Circle wraps.
        a.advance_route(&routes, &mut users, &[], Vec3::ZERO);
        assert_eq!(a.route_i, 0);
        // Loop turns back at the end.
        b.route_i = 2;
        b.advance_route(&routes, &mut users, &[], Vec3::ZERO);
        assert_eq!(b.route_i, 1);
        // Linear: at the end another free route is adopted.
        let mut c = Brain::new(NPC_ID_BASE + 2, PawnKind::Guard, Vec3::ZERO, 3);
        assert!(c.adopt_route(&routes, &mut users, &[], Vec3::new(10500.0, 0.0, 90.0), None));
        assert_eq!(c.patrol, Some(2));
        users[0] = 0;
        c.advance_route(&routes, &mut users, &[], Vec3::new(10500.0, 0.0, 90.0));
        c.advance_route(&routes, &mut users, &[], Vec3::new(10500.0, 0.0, 90.0));
        assert_eq!(c.patrol, Some(0));
        assert_eq!(users, vec![1, 1, 0]);
    }

    use dishonored_ir::ai::{Faction, ForbiddenZone};

    /// The Boyle party factions (`Boyle_Factions`, `DisFaction_Defaults`, as read from the install).
    fn boyle_level() -> AiLevel {
        let f = |n: &str, a: &[&str], e: &[&str]| Faction { name: n.into(), allies: a.iter().map(|s| s.to_string()).collect(), enemies: e.iter().map(|s| s.to_string()).collect(), parents: Vec::new() };
        let mut t = FactionTable::default();
        t.factions = vec![
            f("Boyle_Factions.Neutral_Guard", &["DisFaction_Defaults.Faction_Guard_Default", "DisFaction_Defaults.Faction_Civilian_Default", "Boyle_Factions.Neutral_Civilian"], &["DisFaction_Defaults.Faction_Weepers_Default"]),
            f("Boyle_Factions.Neutral_Civilian", &["DisFaction_Defaults.Faction_Guard_Default", "DisFaction_Defaults.Faction_Civilian_Default", "Boyle_Factions.Neutral_Guard"], &["DisFaction_Defaults.Faction_Weepers_Default"]),
            f("Boyle_Factions.Neutral_Guard_Street", &[], &["DisFaction_Defaults.Faction_Weepers_Default", "DisFaction_Defaults.Faction_Corvo_Default"]),
            f("DisFaction_Defaults.Faction_Guard_Default", &[], &["DisFaction_Defaults.Faction_Corvo_Default", "DisFaction_Defaults.Faction_Weepers_Default"]),
            f("DisFaction_Defaults.Faction_Corvo_Default", &[], &["DisFaction_Defaults.Faction_Guard_Default"]),
            f("DisFaction_Defaults.Faction_Civilian_Default", &[], &[]),
            f("DisFaction_Defaults.Faction_Weepers_Default", &[], &["DisFaction_Defaults.Faction_Civilian_Default", "DisFaction_Defaults.Faction_Corvo_Default", "DisFaction_Defaults.Faction_Guard_Default", "Boyle_Factions.Neutral_Guard"]),
        ];
        // Upstairs: a 1000 uu box owned by the party guards, forbidden to the player.
        let bx = vec![[1.0, 0.0, 0.0, 1000.0], [-1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 1000.0], [0.0, -1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 1000.0], [0.0, 0.0, -1.0, 0.0]];
        let zone = ForbiddenZone { name: "DisForbiddenZone_1".into(), enabled: true, owning: vec!["Boyle_Factions.Neutral_Guard".into()], forbidden: vec!["DisFaction_Defaults.Faction_Corvo_Default".into()], hulls: vec![bx] };
        AiLevel { factions: t, player_faction: "DisFaction_Defaults.Faction_Corvo_Default".into(), zones: vec![zone], ..Default::default() }
    }

    #[test]
    fn faction_relations_follow_the_from_factions_lists() {
        let l = boyle_level();
        let t = &l.factions;
        let i = |n: &str| t.index(n).unwrap();
        let (ng, nc, street, guard, corvo, civ, weep) = (i("Boyle_Factions.Neutral_Guard"), i("Boyle_Factions.Neutral_Civilian"), i("Boyle_Factions.Neutral_Guard_Street"), i("DisFaction_Defaults.Faction_Guard_Default"), i("DisFaction_Defaults.Faction_Corvo_Default"), i("DisFaction_Defaults.Faction_Civilian_Default"), i("DisFaction_Defaults.Faction_Weepers_Default"));
        // Disguised at the party: the party guards do not list Corvo, so he is neutral to them.
        assert_eq!(t.relation(ng, corvo), Relationship::Neutral);
        // Street guards and the default (alarm squad / pursuer) guards are his enemies.
        assert_eq!(t.relation(street, corvo), Relationship::Enemy);
        assert_eq!(t.relation(guard, corvo), Relationship::Enemy);
        // Same faction: ally; listed allies: ally; not symmetric (civilians list no one).
        assert_eq!(t.relation(ng, ng), Relationship::Ally);
        assert_eq!(t.relation(ng, nc), Relationship::Ally);
        assert_eq!(t.relation(civ, weep), Relationship::Neutral);
        assert_eq!(t.relation(weep, civ), Relationship::Enemy);
        // Enemies are checked before allies.
        assert_eq!(t.relation(ng, weep), Relationship::Enemy);
    }

    #[test]
    fn trespassers_and_dispositions_override_factions() {
        let mut c = Crowd::default();
        c.set_level(&boyle_level());
        let ng = c.factions.index("Boyle_Factions.Neutral_Guard").unwrap();
        let mut ws = WorldState::default();
        ws.players = vec![pawn(1, PawnKind::Human, Vec3::new(-500.0, 500.0, 90.0), 0.0), pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 0.0)];
        let mut b = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 1);
        b.faction = Some(ng);
        c.brains.push(b);
        let soc = c.social(&ws);
        assert_eq!(c.brains[0].relation(1, &soc), Relationship::Neutral);
        // Upstairs (inside the forbidden zone owned by the party guards): an enemy.
        ws.players[0].position = Vec3::new(500.0, 500.0, 90.0);
        let soc = c.social(&ws);
        assert_eq!(soc.trespass, vec![(1, ng)]);
        assert_eq!(c.brains[0].relation(1, &soc), Relationship::Enemy);
        // A disabled zone does nothing.
        c.zones[0].enabled = false;
        assert_eq!(c.brains[0].relation(1, &c.social(&ws)), Relationship::Neutral);
        // `DisSeqAct_SetDisposition` faction -> player faction (Set), then Clear.
        let corvo = c.player_faction.unwrap();
        c.set_disposition("Boyle_Factions.Neutral_Guard", 0, Relationship::Enemy, &[Recipient::Faction(corvo)], true);
        assert_eq!(c.brains[0].relation(1, &c.social(&ws)), Relationship::Enemy);
        c.set_disposition("Boyle_Factions.Neutral_Guard", 1, Relationship::Enemy, &[Recipient::Faction(corvo)], true);
        assert_eq!(c.brains[0].relation(1, &c.social(&ws)), Relationship::Neutral);
        // Individual relationships come first.
        c.brains[0].make_hostile(1);
        assert_eq!(c.brains[0].relation(1, &c.social(&ws)), Relationship::Enemy);
    }

    #[test]
    fn friendly_fire_between_npcs_changes_no_relationship() {
        let ai = AiConfig::default();
        let w = flat_world();
        let mut c = Crowd { ai: ai.clone(), ..Default::default() };
        c.set_level(&boyle_level());
        let ng = c.factions.index("Boyle_Factions.Neutral_Guard").unwrap();
        let a = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let b = pawn(NPC_ID_BASE + 1, PawnKind::Guard, Vec3::new(100.0, 0.0, 90.0), 180.0);
        let mut ws = WorldState::default();
        ws.players = vec![a.clone(), b.clone()];
        for (id, p) in [(a.id, a.position), (b.id, b.position)] {
            let mut br = Brain::new(id, PawnKind::Guard, p, id as u64);
            br.faction = Some(ng);
            c.brains.push(br);
        }
        let hit = SimEvent::Damage { victim: a.id, attacker: Some(b.id), amount: 10.0, cause: dishonored_sim::events::DamageCause::Sword };
        for _ in 0..30 {
            c.think(&ws, std::slice::from_ref(&hit), &w, 1.0 / 60.0);
        }
        assert!(c.brains[0].enemies.is_empty() && c.brains[0].state != BrainState::Combat);
        // The pain bark fires.
        assert!(c.events.is_empty() || c.events.iter().all(|e| !matches!(e, AiEvent::AlarmRang { .. })));
    }

    #[test]
    fn a_new_pawn_starts_without_the_old_grudges() {
        let ai = AiConfig::default();
        let w = flat_world();
        let mut c = Crowd { ai: ai.clone(), ..Default::default() };
        let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let player = pawn(1, PawnKind::Human, Vec3::new(300.0, 0.0, 90.0), 180.0);
        let mut ws = WorldState::default();
        ws.players = vec![player.clone(), guard.clone()];
        let mut b = Brain::new(guard.id, PawnKind::Guard, guard.position, 1);
        b.alert_to(&ai, player.id, player.position);
        c.brains.push(b);
        for _ in 0..10 {
            c.think(&ws, &[], &w, 1.0 / 60.0);
        }
        assert!(c.brains[0].hostile.contains(&player.id));
        // The player respawns (same id, new pawn).
        c.forget(player.id);
        assert!(!c.brains[0].hostile.contains(&player.id) && !c.brains[0].attn.iter().any(|a| a.who == player.id));
        for _ in 0..10 {
            c.think(&ws, &[], &w, 1.0 / 60.0);
        }
        assert!(!c.brains[0].enemies.contains(&player.id) && c.brains[0].state != BrainState::Combat);
    }

    #[test]
    fn zone_hull_test() {
        let l = boyle_level();
        assert!(l.zones[0].contains(Vec3::new(10.0, 10.0, 10.0)));
        assert!(!l.zones[0].contains(Vec3::new(-10.0, 10.0, 10.0)));
        assert!(!l.zones[0].contains(Vec3::new(10.0, 10.0, 1010.0)));
    }

    #[test]
    fn alarm_spawns_prefer_far_spawners_up_to_the_cap() {
        let def = AlarmBellDef { spawners: vec!["S1".into(), "S2".into(), "S3".into()], spawner_min_dist: 1500.0, max_spawns: 2, ..Default::default() };
        let sp = vec![("S1".to_string(), Vec3::new(100.0, 0.0, 0.0)), ("S2".to_string(), Vec3::new(3000.0, 0.0, 0.0)), ("S3".to_string(), Vec3::new(4000.0, 0.0, 0.0))];
        assert_eq!(alarm_spawn_choice(&def, &sp, Some(Vec3::ZERO)), vec!["S2".to_string(), "S3".to_string()]);
        // None far enough: all (capped).
        let def2 = AlarmBellDef { spawner_min_dist: 3000.0, ..def.clone() };
        assert_eq!(alarm_spawn_choice(&def2, &sp, Some(Vec3::new(2000.0, 0.0, 0.0))), vec!["S1".to_string(), "S2".to_string()]);
    }

    #[test]
    fn guard_rings_the_bell_then_it_sounds_and_spawns() {
        let ai = AiConfig::default();
        let w = flat_world();
        let mut c = Crowd { ai: ai.clone(), ..Default::default() };
        let mut level = boyle_level();
        level.alarms = vec![AlarmBellDef { name: "DisAlarmBell_0".into(), position: Vec3::new(-1500.0, 0.0, 90.0), loudness: 6000.0, go_closeness: 5000.0, use_closeness: 150.0, ring_delay: 2.5, max_spawns: 6, spawners: vec!["S".into()], ..Default::default() }];
        c.set_level(&level);
        let guard_f = c.factions.index("DisFaction_Defaults.Faction_Guard_Default").unwrap();
        let g = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let p = pawn(1, PawnKind::Human, Vec3::new(1200.0, 0.0, 90.0), 180.0);
        let mut ws = WorldState::default();
        ws.players = vec![g.clone(), p.clone()];
        let mut b = Brain::new(g.id, PawnKind::Guard, g.position, 4);
        b.faction = Some(guard_f);
        c.brains.push(b);
        let mut rang = false;
        let mut spawned = false;
        let mut barks = Vec::new();
        for _ in 0..(60 * 20) {
            let inputs = c.think(&ws, &[], &w, 1.0 / 60.0);
            // Move the guard as its input says (sprint speed along its yaw), no physics.
            if let Some((_, i)) = inputs.iter().find(|x| x.0 == g.id) {
                let gp = &mut ws.players[0];
                gp.yaw = i.yaw;
                if i.forward > 0 {
                    gp.position += Vec3::new(i.yaw.to_radians().cos(), i.yaw.to_radians().sin(), 0.0) * 600.0 / 60.0;
                }
            }
            for e in &c.events {
                match e {
                    AiEvent::AlarmRang { ringer, .. } => {
                        assert_eq!(*ringer, g.id);
                        rang = true;
                    }
                    AiEvent::AlarmSpawn { enemy, .. } => {
                        assert!(rang && *enemy == Some(1));
                        spawned = true;
                    }
                    AiEvent::Bark { hook, .. } => barks.push(hook.clone()),
                    _ => {}
                }
            }
            if spawned {
                break;
            }
        }
        assert!(rang && spawned, "rang {rang} spawned {spawned} state {:?}", c.brains[0].state);
        assert!(barks.iter().any(|h| h == "DDH_ALARM_GO_TO_RING_ALARM"), "{barks:?}");
        // The bell's noise reaches the guard next tick as an alarm stimulus (it hunts the
        // enemy already); the bell rings while the enemy is hunted.
        assert!(c.alarms[0].ringing);
    }

    #[test]
    fn hearing_needs_a_clear_line_without_audio_cells() {
        let ai = AiConfig::default();
        use dishonored_world::Tri;
        // Floor plus a wall at x = 250 between the guard and the noise.
        let s = 20000.0;
        let t = |a: Vec3, b: Vec3, c: Vec3, n: Vec3| Tri { a, b, c, n, channels: channel::ALL, source: 0 };
        let (a, b, cc, d) = (Vec3::new(-s, -s, 0.0), Vec3::new(s, -s, 0.0), Vec3::new(s, s, 0.0), Vec3::new(-s, s, 0.0));
        let (w1, w2, w3, w4) = (Vec3::new(250.0, -500.0, -10.0), Vec3::new(250.0, 500.0, -10.0), Vec3::new(250.0, 500.0, 500.0), Vec3::new(250.0, -500.0, 500.0));
        let w = CollisionWorld::from_triangles(vec![t(a, b, cc, Vec3::Z), t(a, cc, d, Vec3::Z), t(w1, w2, w3, -Vec3::X), t(w1, w3, w4, -Vec3::X)], vec!["floor".into()]);
        let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let mut ws = WorldState::default();
        ws.players = vec![guard.clone()];
        let mut b = Brain::new(guard.id, PawnKind::Guard, guard.position, 1);
        let behind = SimEvent::Sound { position: Vec3::new(500.0, 0.0, 90.0), radius: 1500.0, category: SoundCategory::Pistol, context: NoiseContext::Danger, source: 7 };
        b.perceive(&guard, &ws, std::slice::from_ref(&behind), &[], &[], &[], &w, &ai, 1.0 / 60.0);
        assert!(b.focus().is_none());
        let open = SimEvent::Sound { position: Vec3::new(-500.0, 0.0, 90.0), radius: 1500.0, category: SoundCategory::Pistol, context: NoiseContext::Danger, source: 7 };
        b.perceive(&guard, &ws, std::slice::from_ref(&open), &[], &[], &[], &w, &ai, 1.0 / 60.0);
        assert_eq!(b.focus().map(|f| f.1), Some(DAL_INVESTIGATE));
        // Exactly at the radius: not heard (strict).
        let mut c2 = Brain::new(guard.id, PawnKind::Guard, guard.position, 1);
        let edge = SimEvent::Sound { position: Vec3::new(-1500.0, 0.0, 90.0), radius: 1500.0, category: SoundCategory::Pistol, context: NoiseContext::Danger, source: 7 };
        c2.perceive(&guard, &ws, std::slice::from_ref(&edge), &[], &[], &[], &w, &ai, 1.0 / 60.0);
        assert!(c2.focus().is_none());
    }

    #[test]
    fn aware_on_startup_is_psychic_max_attention() {
        let ai = AiConfig::default();
        let mut b = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 1);
        b.make_hostile(1);
        b.aware_of(&ai, 1, Vec3::new(100.0, 0.0, 0.0));
        let a = b.attn.iter().find(|a| a.who == 1).unwrap();
        assert!(close(a.value, 1.0) && a.level == DAL_BUSTED && a.why == reason::PSYCHIC);
        assert_eq!(a.proxy, Vec3::new(100.0, 0.0, 0.0));
    }

    #[test]
    fn speed_value_is_unclamped_over_ground_speed_sprint() {
        let ai = AiConfig::default();
        let mut p = pawn(1, PawnKind::Human, Vec3::ZERO, 0.0);
        p.velocity = Vec3::new(900.0, 0.0, 0.0);
        // 900 / 600 = 1.5 -> 0.25 * 1.5 + light 0.1.
        assert!(close(Brain::visibility(&ai, &p, 1.0), 0.475));
    }

    #[test]
    fn light_probe_attenuates_and_shadows() {
        let w = flat_world();
        let l = ProbeLight { kind: 0, position: Vec3::new(0.0, 0.0, 300.0), intensity: 1.0, radius: 1000.0, falloff_exponent: 2.0, casts_shadows: true, ..Default::default() };
        let near = light_at(std::slice::from_ref(&l), Vec3::new(0.0, 0.0, 90.0), &w);
        let far = light_at(std::slice::from_ref(&l), Vec3::new(2000.0, 0.0, 90.0), &w);
        // d = 210: (1 - 0.0441)^2 = 0.9137 -> ^(1/2.2).
        assert!((near - 0.9137f32.powf(1.0 / 2.2)).abs() < 1e-3, "{near}");
        assert_eq!(far, 0.0);
        // Below the floor: shadowed.
        assert_eq!(light_at(std::slice::from_ref(&l), Vec3::new(0.0, 0.0, -100.0), &w), 0.0);
    }

    #[test]
    fn neglected_routes_win_and_squads_restrict() {
        let mut routes = vec![route(2, 2, 0.0), route(2, 2, 3000.0)];
        routes[0].squads = vec!["Court_Guard".into()];
        let mut users = vec![0, 0];
        let mut a = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 1);
        // Not of the squad: the near route is refused.
        assert!(a.adopt_route(&routes, &mut users, &[], Vec3::ZERO, None));
        assert_eq!(a.patrol, Some(1));
        let mut b = Brain::new(NPC_ID_BASE + 1, PawnKind::Guard, Vec3::ZERO, 2);
        b.squad = "Court_Guard".into();
        routes[0].capacity = 2;
        routes[1].capacity = 2;
        assert!(b.adopt_route(&routes, &mut users, &[], Vec3::ZERO, None));
        assert_eq!(b.patrol, Some(0));
        // A guard with a route only moves to a neglected one.
        let mut users2 = vec![0, 0];
        let mut c = Brain::new(NPC_ID_BASE + 2, PawnKind::Guard, Vec3::ZERO, 3);
        c.squad = "Court_Guard".into();
        assert!(c.adopt_route(&routes, &mut users2, &[0.0, 0.0], Vec3::ZERO, None));
        assert!(!c.adopt_route(&routes, &mut users2, &[0.0, 0.0], Vec3::ZERO, None));
        assert!(c.adopt_route(&routes, &mut users2, &[0.0, 31.0], Vec3::ZERO, None));
        assert_eq!(c.patrol, Some(1));
    }

    #[test]
    fn transitions_bark_the_original_hooks() {
        let ai = AiConfig::default();
        let mut b = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 1);
        let me = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 0.0);
        let ws = WorldState::default();
        b.state = BrainState::Notice;
        b.transition_barks(BrainState::Patrol, &me, &ws, &[], &ai);
        b.state = BrainState::Investigate;
        b.cause = reason::HEARD;
        b.cause_danger = true;
        b.transition_barks(BrainState::React, &me, &ws, &[], &ai);
        b.state = BrainState::Combat;
        b.transition_barks(BrainState::Search, &me, &ws, &[], &ai);
        b.state = BrainState::Search;
        b.transition_barks(BrainState::Combat, &me, &ws, &[], &ai);
        b.cause = reason::FOUND_CORPSE;
        b.state = BrainState::Patrol;
        b.transition_barks(BrainState::Search, &me, &ws, &[], &ai);
        let hooks: Vec<&str> = b.barks.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(hooks, ["DDH_ATTENTION_NOTICE_BEGIN", "DDH_SEARCH_ADETECTION_DANGER", "DDH_SEARCH_FIND_ENEMY", "DDH_COMBAT_LOST", "DDH_SEARCH_GIVEUP_POSTCORPSE"]);
    }

    /// `DisAISubProcessAmbientBarks` (0x00b47020 / 0x00b3d5e0): silent for
    /// `m_fBarkInhibitionDuration` after the patrol starts, then `DDH_PATROL_MISC` every tick
    /// (the bark manager's refire gate spaces them); leaving the behaviour re-arms it.
    #[test]
    fn ambient_barks_after_the_inhibition() {
        let ai = AiConfig::default();
        let mut b = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 1);
        b.state = BrainState::Patrol;
        let mut first = None;
        for i in 0..800 {
            b.ambient_barks(&ai, 1.0 / 60.0);
            if first.is_none() && !b.barks.is_empty() {
                first = Some(i);
            }
        }
        let first = first.expect("a bark") as f32 / 60.0;
        assert!((first - ai.ambient_bark_inhibition).abs() < 0.05, "{first}");
        assert!(b.barks.iter().all(|x| x.0 == "DDH_PATROL_MISC"));
        b.barks.clear();
        b.state = BrainState::Search;
        b.ambient_barks(&ai, 1.0 / 60.0);
        b.state = BrainState::Patrol;
        b.ambient_barks(&ai, 1.0 / 60.0);
        assert!(b.barks.is_empty(), "re-armed");
    }

    /// The slot stack: a higher slot preempts, the lower behaviour stays dormant and resumes
    /// when the higher one finishes (0x00b11960 / 0x00b17100).
    #[test]
    fn higher_slots_preempt_and_lower_ones_resume() {
        let mut b = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 1);
        b.state = BrainState::Patrol;
        b.sync_external();
        b.place(Behavior::Notice);
        assert_eq!(b.project(), BrainState::Notice);
        b.place(Behavior::Search(SearchState { target: NOISE, ..Default::default() }));
        b.place(Behavior::Combat);
        assert_eq!((b.current_slot(), b.project()), (slot::COMBAT, BrainState::Combat));
        // A Notice placed while fighting stays dormant.
        b.place(Behavior::Notice);
        assert_eq!(b.state, BrainState::Combat);
        b.complete(slot::COMBAT);
        assert_eq!(b.state, BrainState::Investigate, "the dormant search resumes");
        b.complete(slot::SEARCH);
        assert_eq!(b.state, BrainState::Notice);
        b.complete(slot::NOTICE);
        assert_eq!(b.state, BrainState::Alert, "a guard without a route returns to its post");
    }

    /// `patrol_on_startup`: `PatrolRequest` makes `DisBehaviorPatrol` adopt a route.
    #[test]
    fn patrol_request_adopts_a_route() {
        let w = flat_world();
        let mut c = Crowd::default();
        c.routes = vec![route(2, 3, 0.0)];
        c.route_users = vec![0];
        let g = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(100.0, 0.0, 90.0), 0.0);
        let mut ws = WorldState::default();
        ws.players = vec![g.clone()];
        let mut b = Brain::new(g.id, PawnKind::Guard, g.position, 1);
        b.stay = true;
        b.request_patrol();
        c.brains.push(b);
        c.think(&ws, &[], &w, 1.0 / 60.0);
        assert_eq!((c.brains[0].patrol, c.brains[0].state), (Some(0), BrainState::Patrol));
    }

    /// PathingFail (0x00b33850): without a navmesh point to retry, the investigation is over;
    /// a proxy more than 500 uu above the pawn asks for StareAtUnreachable.
    #[test]
    fn failed_path_to_a_proxy_above_stares() {
        let ai = AiConfig::default();
        let me = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let mut b = Brain::new(me.id, PawnKind::Guard, me.position, 1);
        let mut s = SearchState { target: NOISE, proxy: Vec3::new(300.0, 0.0, 800.0), dest: Vec3::new(300.0, 0.0, 800.0), ..Default::default() };
        b.pathing_fail(&mut s, &me, None, &ai);
        assert!(s.finished && s.wants_stare && !s.reached);
        let mut s2 = SearchState { target: NOISE, proxy: Vec3::new(300.0, 0.0, 300.0), dest: Vec3::new(300.0, 0.0, 300.0), ..Default::default() };
        b.pathing_fail(&mut s2, &me, None, &ai);
        assert!(s2.finished && !s2.wants_stare);
    }

    /// A power seen while Busted on its user: `WitnessMagic` after `m_MagicReactionDelay`; not
    /// Busted, a mode-1 power raises attention by `m_IncreaseForMagic` (0x00b23e50).
    #[test]
    fn magic_witnesses() {
        let ai = AiConfig::default();
        let mut ws = WorldState::default();
        ws.players = vec![pawn(1, PawnKind::Human, Vec3::new(500.0, 0.0, 90.0), 0.0)];
        let mut b = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 1);
        b.alert_to(&ai, 1, Vec3::new(500.0, 0.0, 90.0));
        b.witness_magic(1, 0, &ws, &ai);
        let q = b.stims.iter().find(|q| q.stim == Stim::WitnessMagic).expect("witness");
        assert!(q.who == 1 && (0.2..=0.3).contains(&q.delay) && !q.flag);
        let mut c = Brain::new(NPC_ID_BASE + 1, PawnKind::Guard, Vec3::ZERO, 2);
        c.witness_magic(1, 1, &ws, &ai);
        assert!(c.stims.is_empty() && c.magic_pending.is_some());
        let a = c.attn.iter().find(|a| a.who == 1).unwrap();
        assert!(a.why == reason::MAGIC && a.level >= DAL_HEAD_TRACK);
    }

    /// The attack cooldown is scaled by `(1 - aggression) x m_fCooldownFactorAtMinAggression + 1`.
    #[test]
    fn low_aggression_doubles_cooldowns() {
        let ai = AiConfig::default();
        let w = flat_world();
        let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
        let mut ws = WorldState::default();
        ws.players = vec![guard.clone(), pawn(1, PawnKind::Human, Vec3::new(150.0, 0.0, 90.0), 180.0)];
        let mut b = Brain::new(guard.id, PawnKind::Guard, guard.position, 1);
        b.alert_to(&ai, 1, Vec3::new(150.0, 0.0, 90.0));
        b.attn[0].sighted = true;
        b.state = BrainState::Combat;
        b.target = Some(1);
        b.in_fight = true;
        b.flurry = 3;
        b.aggression = 0.0;
        b.combat(&guard, &ws, &w, None, &ai, &mut Vec::new(), 1.0 / 60.0);
        let k = b.melee_cd.iter().position(|c| *c > 0.0).unwrap();
        let [lo, hi] = ai.combat.melee_contexts[k].cooldown;
        assert!(b.melee_cd[k] >= lo * 1.99 && b.melee_cd[k] <= hi * 2.0, "{}", b.melee_cd[k]);
    }

    /// Rat pawns of a swarm get brains: twelve rats (the random swarm's attack group) attack a
    /// player in reach after the detection delay; a lone rat is not aggressive. NPC awareness is
    /// written to the pawns.
    #[test]
    fn crowd_drives_rat_pawns_and_writes_awareness() {
        let w = flat_world();
        let mut c = Crowd::default();
        let mut ws = WorldState::default();
        let mut spawned = Vec::new();
        for k in 0..12u32 {
            ws.players.push(pawn(5000 + k, PawnKind::Rat, Vec3::new(0.0, k as f32 * 5.0, 12.0), 0.0));
            spawned.push(SimEvent::RatSpawned { rat: 5000 + k, swarm: Some(9) });
        }
        ws.players.push(pawn(5100, PawnKind::Rat, Vec3::new(-3000.0, 0.0, 12.0), 0.0));
        spawned.push(SimEvent::RatSpawned { rat: 5100, swarm: None });
        ws.players.push(pawn(1, PawnKind::Human, Vec3::new(150.0, 0.0, 90.0), 180.0));
        let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(-2000.0, 0.0, 90.0), 0.0);
        ws.players.push(guard.clone());
        c.brains.push(Brain::new(NPC_ID_BASE, PawnKind::Guard, guard.position, 1));
        let mut inputs = c.think(&ws, &spawned, &w, 0.1);
        for _ in 0..25 {
            inputs = c.think(&ws, &[], &w, 0.1);
        }
        let rat0 = inputs.iter().find(|x| x.0 == 5000).unwrap().1;
        assert!(rat0.buttons & buttons::PRIMARY != 0, "the swarm rat attacks");
        assert!(c.rat_brains.iter().find(|b| b.id == 5000).is_some_and(|b| b.rats == 12));
        let lone = inputs.iter().find(|x| x.0 == 5100).unwrap().1;
        assert_eq!(lone.buttons & buttons::PRIMARY, 0);
        c.write_awareness(&mut ws);
        assert!(ws.player(NPC_ID_BASE).unwrap().ai_unaware);
        c.brains[0].state = BrainState::Combat;
        c.write_awareness(&mut ws);
        assert!(!ws.player(NPC_ID_BASE).unwrap().ai_unaware);
    }

    #[test]
    fn sleep_darts_are_instant_unless_fighting() {
        let mut b = Brain::new(NPC_ID_BASE, PawnKind::Guard, Vec3::ZERO, 1);
        assert!(b.unaware_of(1));
        b.state = BrainState::Search;
        assert!(b.unaware_of(1), "suspicious is not in combat");
        b.state = BrainState::Notice;
        assert!(b.unaware_of(1), "noticing is not in combat yet");
        b.state = BrainState::Combat;
        assert!(!b.unaware_of(1));
    }

    /// A guard in combat picks the attack by distance and exposes it as `npc_move`: the long
    /// attack travels toward the target, the short one swings at once; a back step follows an
    /// incoming attack (`m_IncomingAttacksBeforeBackStep` 1).
    #[test]
    fn combat_moves_follow_item_context_ranges() {
        let ai = AiConfig::default();
        let w = flat_world();
        let setup = |dist: f32| {
            let guard = pawn(NPC_ID_BASE, PawnKind::Guard, Vec3::new(0.0, 0.0, 90.0), 0.0);
            let player = pawn(1, PawnKind::Human, Vec3::new(dist, 0.0, 90.0), 180.0);
            let mut ws = WorldState::default();
            ws.players = vec![guard.clone(), player];
            let mut b = Brain::new(guard.id, PawnKind::Guard, guard.position, 1);
            b.make_hostile(1);
            b.alert_to(&ai, 1, Vec3::new(dist, 0.0, 90.0));
            b.attn[0].sighted = true;
            b.state = BrainState::Combat;
            b.target = Some(1);
            (guard, ws, b)
        };
        let mut inh = Vec::new();
        let (g, ws, mut b) = setup(450.0);
        let i = b.combat(&g, &ws, &w, None, &ai, &mut inh, 1.0 / 60.0);
        assert_eq!(i.npc_move, npc_moves::ATTACK_LONG);
        assert!(i.forward > 0 && i.buttons & buttons::PRIMARY == 0);
        assert_eq!(b.attack, npc_moves::ATTACK_LONG);
        let (g, ws, mut b) = setup(150.0);
        let i = b.combat(&g, &ws, &w, None, &ai, &mut inh, 1.0 / 60.0);
        assert_eq!(i.npc_move, npc_moves::ATTACK_SHORT);
        assert!(i.buttons & buttons::PRIMARY != 0);
        // Incoming attack: the player winds up facing the guard at close range.
        let (g, mut ws, mut b) = setup(150.0);
        b.flurry = 0;
        b.stamina_timer = 10.0;
        b.in_fight = true;
        ws.players[1].sword.phase = SwordPhase::Startup;
        let i = b.combat(&g, &ws, &w, None, &ai, &mut Vec::new(), 1.0 / 60.0);
        assert_eq!(i.npc_move, npc_moves::BACK_STEP);
        assert!(i.forward < 0);
    }
}
