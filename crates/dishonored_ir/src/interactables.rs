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

/// The interactions of a map.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Interactables {
    pub keyholes: Vec<KeyholeDef>,
    pub tripwires: Vec<TripwireDef>,
    /// `DisTweaks_PlayerInput.m_fAltInteractActivateTime` (0.3): how long Use is held before
    /// the interactable's `AltInteract` runs (the door's peek) instead of `Interact`.
    pub alt_interact_time: f32,
    /// The `DishonoredDamageType` classes whose default `m_bDontTriggerTripWire` is set
    /// (0x00A28BB0 tests it on the damage type of every `TakeDamage` of a tripwire).
    pub no_trip_damage_types: Vec<String>,
}
