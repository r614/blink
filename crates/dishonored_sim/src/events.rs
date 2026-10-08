//! Events emitted by a simulation step. Consumed by presentation, networking, scoring,
//! AI perception and the test harness. Never authoritative input to the next step.

use crate::state::PlayerId;
use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum SoundCategory {
    Footstep,
    Sprint,
    Land,
    Blink,
    Crossbow,
    Pistol,
    SwordSwing,
    SwordImpact,
    SwordBlock,
    Windblast,
    BodyFall,
    Mantle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum DamageCause {
    Sword,
    SwordCounter,
    Assassination,
    DropAssassination,
    Pistol,
    PistolHeadshot,
    Crossbow,
    CrossbowHeadshot,
    Windblast,
    Fall,
    World,
    Grenade,
    StickyGrenade,
    SpringRazor,
    /// Incendiary bolt fire (`DisDamageType_Arrow_Flare`, `m_bPutToFire`).
    Fire,
    /// Devouring Swarm rats.
    Rats,
    /// Possession ended inside geometry (`DisDamageType_UnpossessSquash`).
    UnpossessSquash,
    /// Out of air under water (`DmgType_Drowning` path of 0x00ABB1E0).
    Drowning,
    /// A rat's bite (`DishonoredDamageType_Bite`).
    RatBite,
    /// Blood Thirsty's adrenaline kill (`DisDamageType_ImpactHit_Adrenaline`).
    Adrenaline,
    /// Hit by a sliding pawn (`DisDamageType_Slide`, `m_SlideDamageInfo`).
    Slide,
}

/// How a choke ended (`DisItemContext_Choke.eChokeState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum ChokeOutcome {
    /// Knocked out.
    Win,
    /// The choke button was released: the victim breaks free.
    Lose,
    /// The victim left the being-choked state (hurt, killed, moved away).
    Cancel,
}

/// Why a pawn became unconscious.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum KnockoutCause {
    Choke,
    SleepDart,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SimEvent {
    Jumped { id: PlayerId },
    Landed { id: PlayerId, speed: f32, fall_time: f32 },
    Mantle { id: PlayerId, edge_height: f32 },
    FellOutOfWorld { id: PlayerId },
    BlinkStart { id: PlayerId, from: Vec3, to: Vec3 },
    BlinkEnd { id: PlayerId },
    BlinkFizzle { id: PlayerId },
    SwordSwing { id: PlayerId },
    SwordBlocked { attacker: PlayerId, defender: PlayerId },
    SwordParried { attacker: PlayerId, defender: PlayerId },
    /// A projectile left: its kind, spawn point and velocity (presentation starts its trail from
    /// here, as a bolt can hit within its launch tick and never be in a snapshot).
    Fired { id: PlayerId, bolt: bool, projectile: u32, kind: crate::state::ProjectileKind, origin: Vec3, velocity: Vec3 },
    Reloaded { id: PlayerId },
    WindblastCast { id: PlayerId },
    DarkVision { id: PlayerId, active: bool },
    Damage { victim: PlayerId, attacker: Option<PlayerId>, amount: f32, cause: DamageCause },
    Death { victim: PlayerId, killer: Option<PlayerId>, cause: DamageCause },
    ProjectileImpact { projectile: u32, point: Vec3, victim: Option<PlayerId> },
    Sound { position: Vec3, radius: f32, category: SoundCategory, source: PlayerId },
    // --- inventory ---
    /// A pickup was taken (`DisSeqEvent_PickupPickedUp`); `name` is the level actor name.
    PickedUp { id: PlayerId, pickup: u32 },
    /// A pickup was stolen from the NPC carrying it.
    Pickpocketed { id: PlayerId, victim: PlayerId, pickup: u32 },
    ElixirUsed { id: PlayerId, mana: bool },
    // --- takedowns (hooks for the AI: the victim is held / unconscious / free again) ---
    ChokeStart { attacker: PlayerId, victim: PlayerId },
    ChokeEnd { attacker: PlayerId, victim: PlayerId, outcome: ChokeOutcome },
    /// Non-lethal: the victim stays down (`DisSeqEvent_KnockedOut`, `DisSeqEvent_PutToSleep`).
    KnockedOut { victim: PlayerId, by: Option<PlayerId>, cause: KnockoutCause },
    BodyLifted { carrier: PlayerId, body: PlayerId },
    BodyDropped { carrier: PlayerId, body: PlayerId },
    // --- powers ---
    PossessStart { id: PlayerId, host: PlayerId },
    PossessEnd { id: PlayerId, host: PlayerId },
    BendTimeStart { id: PlayerId, level: u8 },
    BendTimeEnd { id: PlayerId },
    SwarmSummoned { id: PlayerId, swarm: u32, position: Vec3 },
    /// Corpse eaten by a swarm or turned to ash (Shadow Kill): no body remains.
    BodyRemoved { body: PlayerId, ash: bool },
    // --- gadgets ---
    TrapPlaced { id: PlayerId, trap: u32, position: Vec3 },
    TrapTriggered { trap: u32 },
    Explosion { owner: PlayerId, position: Vec3, sticky: bool },
    Ignited { victim: PlayerId, by: PlayerId },
    // --- movement ---
    /// Entered (`entered`) or left a deep water volume (physics Swimming).
    Swim { id: PlayerId, entered: bool },
    /// Agility's power jump at the top of a held jump (0x00A71370).
    PowerJump { id: PlayerId },
    /// Grabbed (`started`) or left a climbable chain: the chain's `DisSeqEvent_Climb` outputs
    /// Start / Finish (0x00BCFAC0 from the climb state's enter and exit).
    Climb { id: PlayerId, climbable: u16, started: bool },
    /// A climbing step sound (`DisTweaks_Climbable.m_pClimbingSound`, its RTPC set to `volume`).
    ClimbSound { id: PlayerId, climbable: u16, volume: f32 },
    // --- Blood Thirsty ---
    /// The adrenaline gauge filled (`m_bNeedAdrenalineEffect`).
    AdrenalineFull { id: PlayerId },
    /// An adrenaline attack was released (the next sword attack with a full gauge).
    AdrenalineAttack { id: PlayerId },
    // --- rats (hooks for `dishonored_ai`) ---
    /// A rat pawn was spawned (by a swarm, a spawner or a mission).
    RatSpawned { rat: PlayerId, swarm: Option<u32> },
    /// A rat bit a pawn.
    RatBite { rat: PlayerId, victim: PlayerId },
}
