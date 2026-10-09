//! Placed world actors with their own gameplay state: breakables (`DishonoredBreakable` and its
//! subclasses with a `DisTweaks_StaticBreakable`), whale oil batteries (`DisWhaleOilBattery`,
//! `DisTweaks_WhaleOilBattery`) and their receptacles (`DisWhaleOilReceptacle`). Read from the
//! level packages by `dishonored_assets::world_actors`; the rules are in
//! `dishonored_sim::world_actors`. See research/dishonored/systems/world.md sections 4 and 9.

use crate::ai::NoiseContext;
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// One `DishonoredBreakableStep` of `DisTweaks_StaticBreakable.m_Steps`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BreakStepDef {
    /// `m_HealthThreshold`: the step is reached once the health is at most this.
    pub health_threshold: i32,
    /// `m_fStepTimer`: after this step, seconds until the next step breaks by itself (0: none).
    pub step_timer: f32,
    /// The step has an `m_pSoundEvent` (its AI noise goes out with the sound, 0x00a3a5f0).
    pub sound: bool,
    /// `m_AINoiseLoudness` as a distance (`m_LoudnessDistanceArray`), and `m_AINoiseContext`.
    pub noise_radius: f32,
    pub noise_context: NoiseContext,
    /// `m_pExplosion` is set.
    pub explosion: bool,
}

/// `DisTweaks_WhaleOilBattery` (the battery-specific part).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BatteryDef {
    /// `m_NumberOfCharges` (a full battery) and `m_InitialNumberOfCharges`.
    pub max_charges: i32,
    pub initial_charges: i32,
    /// `m_PawnChargeCost`, `m_AmbientAnimalChargeCost`, `m_WatchtowerChargeCost`
    /// (`EWhaleOilChargeCost` 0 / 1 / 2).
    pub pawn_cost: i32,
    pub animal_cost: i32,
    pub watchtower_cost: i32,
    /// `m_bIgnoreShield`.
    pub ignore_shield: bool,
    /// `m_fEjectSpeed`.
    pub eject_speed: f32,
}

/// A placed breakable (or battery): the actor and its breakable tweak.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BreakableDef {
    pub name: String,
    pub package: String,
    pub class: String,
    pub position: Vec3,
    /// The `DisTweaks_StaticBreakable` (or `DisTweaks_Movable` / `DisTweaks_WhaleOilBattery`).
    pub tweak: String,
    /// `m_Health` (-1: never takes damage) and `m_DamageThreshold`.
    pub health: i32,
    pub damage_threshold: i32,
    pub steps: Vec<BreakStepDef>,
    /// `m_pImmuneToDamageTypes` / `m_pInstantBreakDamageTypes` (damage type class names).
    pub immune: Vec<String>,
    pub instant_break: Vec<String>,
    /// `m_WhichStepDropsLoot`.
    pub loot_step: i32,
    /// The battery tweak, for a `DisWhaleOilBattery`.
    pub battery: Option<BatteryDef>,
}

/// A placed movable (`DishonoredMovable`, `DisDLC05_Movable`) with its `DisTweaks_Movable`: the
/// physics prop the player can pick up (`m_bInteractable`), carry, drop and throw
/// (research/dishonored/systems/movables.md). Breaking is the same actor's [`BreakableDef`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MovableDef {
    pub name: String,
    pub package: String,
    pub class: String,
    pub position: Vec3,
    /// `Rotation` as (pitch, yaw, roll) in degrees (`Rotator` units of 360 / 65536).
    pub rotation: Vec3,
    /// The `DisTweaks_Movable` (`m_pMovableTweaks`, set or inherited).
    pub tweak: String,
    /// `m_MovableParams.m_WeightClass` (`EMovableWeightClass`: 0 Tiny, 1 Small, 2 Medium, 3 Large).
    pub weight: u8,
    /// `m_MovableParams.m_Damage` (copied to `DishonoredMovable.m_Damage`, +0x404).
    pub damage: i32,
    /// `m_bInteractable`, `m_bIsKeyItem`, `m_bBlocksPlayer`, `m_bNeverBlocksPlayer`.
    pub interactable: bool,
    pub key_item: bool,
    pub blocks_player: bool,
    pub never_blocks_player: bool,
    /// `DisTweaks_InteractableInterface.m_fMaxDistance` of `m_pInteractableTweaks`.
    pub max_distance: f32,
    /// `m_InteractText` of the same tweak (the HUD prompt: "`GBA_Use` Carry").
    pub prompt: String,
    /// `DisTweaks_StaticBreakable.m_fMinSpeedToCauseDamage` / `m_fMinSpeedToBeDamaged` /
    /// `m_fMinSpeedToBeDamagedOnDrop` (the rigid-body collision of a breakable, 0x00A40CF0).
    pub min_speed_to_cause_damage: f32,
    pub min_speed_to_be_damaged: f32,
    pub min_speed_to_be_damaged_on_drop: f32,
    /// `DisTweaks_KActor.m_pContactTypeOverride` (a `DisContactType_*` class name), if set.
    pub contact_type: String,
    /// The static mesh object path of the actor's `StaticMeshComponent` (presentation).
    pub mesh: String,
    /// The mesh's local bounding box (`FBox` min / max, `StaticMesh.Bounds`): the component's
    /// `GetBoundingBox` (interface slot 5, 0x00A46820) that the hold position reads.
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// Index in [`WorldActors::breakables`] of the same actor (it has a break tweak with a health).
    pub breakable: Option<usize>,
}

/// A placed `DisWhaleOilReceptacle` with its `DisTweaks_WhaleOilReceptacle`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReceptacleDef {
    pub name: String,
    pub package: String,
    pub position: Vec3,
    /// `m_pBattery`: the battery placed in it (actor name), plugged on its first tick.
    pub battery: Option<String>,
    /// `m_bAllowEmptyBatteries`.
    pub allow_empty: bool,
    /// `m_fDetectionRadius`, `m_fDetectionHeight`.
    pub detection_radius: f32,
    pub detection_height: f32,
    /// `m_aAttachedActors`: the devices it powers (actor names).
    pub attached: Vec<String>,
}

/// A placed `DisWallOfLight` with its `DisTweaks_WallOfLight` (research/dishonored/systems/
/// walls_of_light.md).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WallOfLightDef {
    pub name: String,
    pub package: String,
    pub position: Vec3,
    /// The actor's X axis (its `Rotation`): the shock particles are spawned along it.
    pub facing: Vec3,
    /// The receptacle powering it (index into `WorldActors::receptacles`): `m_pAttachedReceptacle`,
    /// else the receptacle listing the wall in `m_aAttachedActors`.
    pub receptacle: Option<usize>,
    /// `m_FriendlyFactionsOverride` when it has entries, else the tweak's `m_FriendlyFactions`.
    pub friendly_factions: Vec<String>,
    /// `m_bReversed`, `m_bEnabled` as placed.
    pub reversed: bool,
    pub enabled: bool,
    /// The tweak's `m_Damage`, `m_bJib`, `m_ActivationTime`, `m_ActivationTimeForPawnEffect`.
    pub damage: i32,
    pub jib: bool,
    pub activation_time: f32,
    pub activation_time_pawn: f32,
    /// The static mesh's collision triangles (its kDOP tree's) in world space.
    pub triangles: Vec<[Vec3; 3]>,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// The tweak's `m_fWindblastMinimumForce`: a Windblast reaching the wall with at least this
    /// blast factor shocks it and costs a charge (0x00A3BEF0).
    pub windblast_min_force: f32,
    /// The mesh component's `HiddenGame` (its own or its archetype's): the mesh is the editor's
    /// stand-in, the wall is seen through its emitters.
    pub mesh_hidden: bool,
    /// Presentation: the placed `Emitter`s of `m_SparkEffects` (on while powered, 0x00A3EB10 /
    /// 0x00A3ECA0) and `m_Beams` (on while the kill effect runs, their beam ends at the effect's
    /// point: 0x00A30CA0, off at its end 0x00A28F80), as `package:actor`.
    pub spark_effects: Vec<String>,
    pub beams: Vec<String>,
    /// The tweak's `m_CrossSparkEffectTemplate` (a `ParticleSystem` path; the wall's tick arcs it
    /// between two random spark emitters every `m_fMinSparkInterval`..`m_fMaxSparkInterval` s
    /// for `m_fSparkDuration` s, active only then with `m_bDisableSystemBetweenSparks`) and
    /// `m_ShockParticleSystemTemplate`.
    pub cross_spark: String,
    pub spark_interval: [f32; 2],
    pub spark_duration: f32,
    pub sparks_only_while_arcing: bool,
    pub shock_particles: String,
}

/// The world actors of one map.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorldActors {
    pub breakables: Vec<BreakableDef>,
    pub receptacles: Vec<ReceptacleDef>,
    /// The placed movables (all of them, whatever their health).
    pub movables: Vec<MovableDef>,
    pub walls_of_light: Vec<WallOfLightDef>,
}
