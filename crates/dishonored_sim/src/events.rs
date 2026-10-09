//! Events emitted by a simulation step. Consumed by presentation, networking, scoring,
//! AI perception and the test harness. Never authoritative input to the next step.

use crate::state::PlayerId;
use glam::Vec3;
use serde::{Deserialize, Serialize};

pub use dishonored_ir::ai::NoiseContext;

/// What made an AI noise (`SimEvent::Sound`); the AI reacts to its `NoiseContext`.
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
    /// A bullet's impact on the world.
    BulletImpact,
    /// A grenade's explosion.
    Explosion,
    /// A rat swarm biting.
    Rats,
    /// A Kismet `DisSeqAct_AINoise`.
    Scripted,
    /// A breakable reaching a break step (the step's `m_AINoiseLoudness`).
    Breakable,
    /// A physics prop (a movable) striking the world: its contact type's AI noise.
    Thrown,
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
    /// A music-box Overseer's combat tune (`DisDamageType_Musical`).
    Musical,
    /// A thrown prop striking a pawn (`DishonoredMovable.m_Damage`, the breakable's rigid-body
    /// collision 0x00A40CF0).
    ThrownObject,
    /// A weeper's plague cloud (`m_iPlagueDmgPerTick`).
    Plague,
    /// A Tallboy arrow's explosion (`TallboyArrowExplosion_Twk`).
    TallboyExplosion,
    /// A launcher's projectile striking a pawn (`DisProjectile` slot 239: `m_Damage x
    /// m_fDamageMultiplier`).
    TrapShot,
    /// A launcher's explosive arrow bursting (`TrapArrowExplosion_Twk`).
    TrapExplosion,
    /// A wall of light's shock (`DisDamageType_WallOfLight`, an energy type: no damage
    /// reduction).
    WallOfLight,
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
    // --- walls of light (crate::walls) ---
    /// A projectile struck a wall of light that has collision (`wall` indexes the map's walls):
    /// the contact processor (0x00C4F3A0) calls the wall's `TakeDamage` with the projectile as
    /// the damage causer and destroys the projectile (no impact, no stick, no burst). The host
    /// applies the wall's side ([`crate::walls::apply_hits`]).
    WallOfLightHit { wall: u32, point: Vec3, projectile: u32, owner: PlayerId },
    /// `by`'s Windblast front reached a wall of light (0x00BF6AF0 -> 0x00BE9770 -> the wall's
    /// vslot 223): `force` is the blast factor at the wall's bounds centre (0x00BE8570).
    WallOfLightBlast { wall: u32, force: f32, by: PlayerId },
    /// A wall's shock at `point` (presentation): `m_ShockSound`, and with `particles` its
    /// `m_ShockParticleSystemTemplate` (the shock 0x00A3BCE0, a rat 0x00A3E630, a
    /// disintegration 0x00A3E7D0; a pawn's kill 0x00A3B940 plays the sound only).
    WallOfLightShock { wall: u32, point: Vec3, particles: bool },
    /// An AI noise (`DisAINoiseInfo`): heard by listeners other than `source` strictly within
    /// `radius` (0x00C5A760); `context` is its `EAINoiseContext`.
    Sound { position: Vec3, radius: f32, category: SoundCategory, context: NoiseContext, source: PlayerId },
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
    // --- movable props (research/dishonored/systems/movables.md) ---
    /// `id` took a movable prop in his hands: `DisSeqEvent_MovablePickedUp` (0x00A45FD0, from the
    /// grab 0x00C3D030; originator the prop, instigator the pawn). `movable` indexes
    /// `WorldState::movables`.
    MovablePickedUp { id: PlayerId, movable: u32 },
    /// `id` let go of a prop (`thrown`: with the throw impulse): `DisSeqEvent_MovableDropped`
    /// (0x00A46380, from the release 0x00C45340 for both).
    MovableDropped { id: PlayerId, movable: u32, thrown: bool },
    /// A prop struck something at `speed` along the contact normal and that was fast enough to
    /// matter (above its break threshold: the breakable's rigid-body collision, 0x00A40CF0): it
    /// dealt `damage` (its `m_Damage`) to `victim`, the pawn it hit, and, when `breakable` is
    /// set (its index in the map's `WorldActors::breakables`), to itself. `by` is the pawn that
    /// last held or threw it.
    MovableImpact { movable: u32, speed: f32, point: Vec3, by: Option<PlayerId>, victim: Option<PlayerId>, damage: i32, breakable: Option<u32> },
    /// A moving prop touched something (its rigid-body collision notification) at `speed`.
    MovableContact { movable: u32, speed: f32, point: Vec3 },
    // --- powers ---
    /// A power was used (`DishonoredActivePowerComponent` slot 94, 0x00BF32F0, called from the
    /// power's activation: Windblast 0x00BFB950, Blink 0x00BFC990, Dark Vision 0x00BE89E0,
    /// Devouring Swarm 0x00BFAE60): `power` indexes [`crate::state::power`], `level` is its
    /// `m_CurrentLevel` (1 or 2). The Kismet's `DisSeqEvent_PowerUsed` fires from it.
    PowerUsed { id: PlayerId, power: u8, level: u8 },
    /// An NPC was thrown by `by`'s Windblast (the thrown reaction, `DisSeqEvent_Windblasted`
    /// output 1 from 0x00C4DE90).
    Windblasted { victim: PlayerId, by: PlayerId },
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
    /// Rats set upon an NPC (its swarm-melee state starts; `rat` the first attacker).
    RatsAttackStart { victim: PlayerId, rat: PlayerId },
}
