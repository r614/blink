//! Gameplay constants, normalized from Arkane tweak objects, class defaults and INIs.
//!
//! Every field records where it came from in [`GameplayConfig::provenance`] so tests and
//! docs can cite evidence. Units: Unreal units (uu), seconds, degrees.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MovementConfig {
    /// `DefaultGame.ini [Engine.WorldInfo] DefaultGravityZ`.
    pub gravity_z: f32,
    pub run_speed: f32,
    pub sprint_speed: f32,
    pub walk_speed: f32,
    pub crouch_speed: f32,
    pub accel_rate: f32,
    /// Stock UE3 `PhysicsVolume.GroundFriction` unless overridden.
    pub ground_friction: f32,
    pub jump_z: f32,
    pub air_control: f32,
    /// UE3 `Pawn.MaxFallSpeed` semantics: max landing speed without damage (NOT a velocity clamp).
    pub max_fall_speed: f32,
    /// UE3 `PhysicsVolume.TerminalVelocity` (stock 4000, LIKELY).
    #[serde(default = "default_terminal_velocity")]
    pub terminal_velocity: f32,
    pub max_step_height: f32,
    /// `CylinderComponent` radius / half-height (standing).
    pub collision_radius: f32,
    pub collision_half_height: f32,
    pub crouch_half_height: f32,
    pub base_eye_height: f32,
    pub strafe_mult_run: f32,
    pub strafe_mult_sprint: f32,
    pub strafe_mult_sneak: f32,
    pub back_mult_run: f32,
    pub back_mult_sprint: f32,
    pub back_mult_sneak: f32,
    pub sword_unsheathed_speed_factor: f32,
    /// Floor normals with Z below this are not walkable (UE3 `WalkableFloorZ`).
    pub walkable_floor_z: f32,
    pub max_speed_before_falling_damage: f32,
    pub max_speed_before_falling_death: f32,
    pub mantle: MantleConfig,
}

fn default_terminal_velocity() -> f32 {
    4000.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MantleConfig {
    pub min_edge_height: f32,
    pub low_max_edge_height: f32,
    pub medium_max_edge_height: f32,
    pub max_edge_height: f32,
    pub edge_search_dist: f32,
    pub max_slope_angle_edge_top: f32,
    pub forward_move_amount: f32,
    pub max_fall_speed_for_mantle: f32,
    pub fall_speed_for_ledge_grab: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VitalsConfig {
    pub health_max: f32,
    pub health_regen_rate: f32,
    pub health_regen_amount: f32,
    pub health_regen_limit: f32,
    pub health_regen_initial_delay: f32,
    pub mana_max: f32,
    pub mana_regen_amount: f32,
    /// Mana regenerates only up to this many points (Dishonored's partial regen).
    pub mana_regen_additive_portion: f32,
    pub mana_regen_step_time: f32,
    pub mana_regen_initial_delay: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlinkConfig {
    /// Per power level (index 0 = level 1).
    pub horiz_distance: Vec<f32>,
    pub vert_distance: Vec<f32>,
    pub step_distance: f32,
    pub target_test_extent: [f32; 3],
    pub close_collision_distance: f32,
    pub close_collision_offset_step: f32,
    pub mana_cost: f32,
    pub ledge_check_distance_scale: f32,
    pub max_angle_big_step: f32,
    pub max_angle_small_step: f32,
    pub step_length_big: f32,
    pub step_length_small: f32,
    pub impulse_radius: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PowerConfig {
    pub mana_cost: f32,
    /// Free-form resolved fields (range, duration, force...) keyed by tweak name.
    pub values: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WeaponConfig {
    pub values: BTreeMap<String, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GameplayConfig {
    pub movement: MovementConfig,
    pub vitals: VitalsConfig,
    pub blink: BlinkConfig,
    pub windblast: PowerConfig,
    pub dark_vision: PowerConfig,
    pub sword: WeaponConfig,
    pub pistol: WeaponConfig,
    pub crossbow: WeaponConfig,
    pub melee: WeaponConfig,
    /// NPC perception and behaviour.
    #[serde(default)]
    pub ai: crate::ai::AiConfig,
    /// field -> evidence (tweak object path / INI key / class default / UE3 stock).
    pub provenance: BTreeMap<String, String>,
}
