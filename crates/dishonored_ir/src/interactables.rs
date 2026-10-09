//! Placed world interactions with their own state: door keyholes and tripwires (research/
//! dishonored/systems/interactables.md). Read from the level packages by
//! `dishonored_assets::interactables`; the rules are in `dishonored_sim::interactables`.

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// A `DisDoor` whose mesh has the socket named by `m_KeyHoleSocketName` (default `KeyHole`):
/// the door can be peeked through (`StatePlayerMasterHolePeeking`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct KeyholeDef {
    /// Door actor (`<package>`, `<name>`), the key of its Kismet events.
    pub name: String,
    pub package: String,
    /// The door actor's `Location`.
    pub position: Vec3,
    /// The keyhole socket in world space with the door closed
    /// (`GetSocketWorldLocationAndRotation`, 0x00A1EF10), and its facing: the socket's forward
    /// axis in world space (the original turns it 180 degrees when it faces away from the pawn).
    pub socket: Vec3,
    pub facing: Vec3,
    /// The door's collision bounds with the door closed (world space): what the interactable
    /// focus traces. Equal corners when the door has no physics bodies.
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// `m_fKeyHoleFOV` of the door (degrees).
    pub fov: f32,
    /// `m_InitialDoorState` (0 closed, 1 opened CW, 2 opened CCW).
    pub initial_state: u8,
}

/// A placed `DisTripwire`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TripwireDef {
    pub name: String,
    pub package: String,
    pub position: Vec3,
    /// The tweak's `m_TrippedAnimName` (played when it trips).
    pub tripped_anim: String,
    /// The physics asset's bodies as world-space triangles (the tripwire's `CollisionComponent`
    /// is its skeletal mesh component, `COLLIDE_TouchAll`): what a touching actor must overlap.
    pub triangles: Vec<[Vec3; 3]>,
}

/// How a launcher's projectile acts when it strikes something (its `DisTweaks_Projectile` class).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShotKind {
    /// `DisProjectile_Arrow_Explosive` (`DisTweaks_Arrow_Explosive.m_pExplosionTweak`): explodes at
    /// its first impact (0x00C550D0), after the direct hit's `m_Damage x m_fDamageMultiplier`.
    #[default]
    ExplosiveArrow,
    /// Any other projectile class (the Overseers' Back launcher's `DisProjectile_ThrownObject`):
    /// the direct hit only.
    Bolt,
}

/// The projectile a launcher fires: `DisTweaks_ProjectileLauncher.m_pProjectileTweak` with the
/// launcher's own `m_fProjectileSpeed` / `m_fProjectileDamage` / `m_fProjectileHeadshotMult`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LauncherProjectile {
    pub kind: ShotKind,
    /// The projectile tweak (`Pkg.Obj`) and its class.
    pub tweak: String,
    pub class: String,
    /// `m_fProjectileSpeed x DisTweaks_Projectile.m_fSpeedMultiplier` (0x00C540A0 scales the launch
    /// velocity by the tweak's multiplier).
    pub speed: f32,
    /// `m_fProjectileDamage` (stored as the projectile's `m_Damage`).
    pub damage: i32,
    /// `m_fProjectileHeadshotMult` (the projectile's `m_fHeadshotMultiplier`).
    pub headshot_mult: f32,
    /// `DisTweaks_Projectile.m_fDamageMultiplier` (0x00C4A310: `m_Damage x` it).
    pub damage_multiplier: f32,
    /// `DisTweaks_Projectile.m_fGravityMultiplier`.
    pub gravity_mult: f32,
    /// `DisTweaks_Projectile.m_bInvisible` (the explosive arrow of the trap is not drawn).
    pub invisible: bool,
    /// The tweak's `m_pExplosionTweak` (`DisTweaks_Explosion`), for explosive arrows.
    pub explosion: Option<crate::items::ExplosionConfig>,
}

/// A placed `DisProjectileLauncher` (a `DisSkeletalBreakable` tripod), fired by
/// `DisSeqAct_ActivateProjectileLauncher` (0x00B9B1D0 -> 0x00A33F70) and disarmed / harvested by
/// the player (`Interact`, 0x00A3B6E0).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LauncherDef {
    pub name: String,
    pub package: String,
    pub position: Vec3,
    /// `m_FiringSocketName` of the launcher tweak in world space (the tweak's mesh at its bind pose
    /// under the actor's transform: the pose the socket query reads when the shot is fired is
    /// the idle one), and the socket's forward axis (the direction when there is no target).
    pub muzzle: Vec3,
    pub muzzle_forward: Vec3,
    /// `m_pTarget`'s location (a `TargetPoint` in every shipped level), when set.
    pub target: Option<Vec3>,
    /// `m_bArmed` as placed (class default true).
    pub armed: bool,
    /// The launcher's collision bounds (world space): what the interaction focus traces.
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// `m_TrippedAnimName` / `m_DeactivatedAnimName` of the tweak.
    pub tripped_anim: String,
    pub deactivated_anim: String,
    pub projectile: LauncherProjectile,
    /// `m_HarvestedAmmo` (ammo type index, count) and `m_pItemRequiredToHarvestAmmo` (abstract
    /// item name; empty: none needed).
    pub harvest: Vec<(u8, i32)>,
    pub required_item: String,
    /// The launcher's `DisTweaks_InteractableInterface`: `m_Name`, `m_InteractText` (the prompt,
    /// with its key tag), `m_UseMessage`, and `m_fMaxDistance`; and the projectile tweak's
    /// interactable `m_CannotUseMessage` (shown when there was nothing to disarm or take).
    pub label: String,
    pub prompt: String,
    pub use_message: String,
    pub cannot_use_message: String,
    pub max_distance: f32,
}

/// The interactions of a map.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Interactables {
    pub keyholes: Vec<KeyholeDef>,
    pub tripwires: Vec<TripwireDef>,
    pub launchers: Vec<LauncherDef>,
    /// `DisTweaks_PlayerInput.m_fAltInteractActivateTime` (0.3): how long Use is held before
    /// the interactable's `AltInteract` runs (the door's peek) instead of `Interact`.
    pub alt_interact_time: f32,
    /// The `DishonoredDamageType` classes whose default `m_bDontTriggerTripWire` is set
    /// (0x00A28BB0 tests it on the damage type of every `TakeDamage` of a tripwire).
    pub no_trip_damage_types: Vec<String>,
}
