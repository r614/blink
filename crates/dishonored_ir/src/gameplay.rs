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
    /// `Attribute_GroundSpeedSlowWalk` (analog stick barely deflected).
    #[serde(default = "d_slow_walk")]
    pub slow_walk_speed: f32,
    /// `Attribute_SwordSheathedSpeedFactor`.
    #[serde(default = "d_one")]
    pub sword_sheathed_speed_factor: f32,
    /// `DisTweaks_Pawn.m_fSpeedBlendDownSpeed`: how fast the speed modifier falls when it
    /// drops (rises are instant), per second.
    #[serde(default = "d_blend_down")]
    pub speed_blend_down_speed: f32,
    /// `DisTweaks_Pawn.m_fCrouchHeight` (Pawn.CrouchHeight: the crawl cylinder, used by
    /// Blink's destination fit test; sneaking uses `crouch_half_height`).
    #[serde(default = "d_crouch_crawl")]
    pub crawl_half_height: f32,
    /// `Twk_Pawn_DefaultPlayer.Twk_Pawn_PlayerInput` thresholds.
    #[serde(default)]
    pub input: PlayerInputConfig,
    /// `DisTweaks_PlayerPawn.m_fSlideTime`: length of the sprint slide.
    #[serde(default = "d_slide_time")]
    pub slide_time: f32,
    /// `m_fSlideVelocityDeactivateAngle`: the slide ends when the velocity turns this far
    /// from its starting direction.
    #[serde(default = "d_slide_angle")]
    pub slide_deactivate_angle: f32,
    /// `DishonoredCamera_Lean` (DefaultCamera.ini): the eye swings about a pivot
    /// `lean_height_pct x 2 x CollisionHeight` below it, up to `lean_max_angle` degrees,
    /// driven by a spring (`m_LeanConstants`).
    #[serde(default)]
    pub lean: LeanConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LeanConfig {
    pub height_pct: f32,
    pub max_angle: f32,
    pub max_angle_crouched: f32,
    pub springiness: f32,
    pub damping: f32,
}

impl Default for LeanConfig {
    fn default() -> Self {
        Self { height_pct: 2.0, max_angle: 15.0, max_angle_crouched: 15.0, springiness: 80.0, damping: 12.0 }
    }
}

fn d_slide_time() -> f32 {
    1.1
}
fn d_slide_angle() -> f32 {
    25.0
}

fn default_terminal_velocity() -> f32 {
    4000.0
}
fn d_slow_walk() -> f32 {
    100.0
}
fn d_one() -> f32 {
    1.0
}
fn d_blend_down() -> f32 {
    8.0
}
fn d_crouch_crawl() -> f32 {
    33.0
}

/// Movement input thresholds of `DisTweaks_PlayerInput` (stick magnitudes and degrees).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayerInputConfig {
    pub slow_walk_threshold: f32,
    pub walk_threshold: f32,
    pub stop_sprint_threshold: f32,
    pub strafe_angle_forward: f32,
    pub strafe_angle_back: f32,
    pub backwards_angle: f32,
    pub min_speed_to_slide: f32,
}

impl Default for PlayerInputConfig {
    /// Values of `Twk_Pawn_DefaultPlayer.Twk_Pawn_PlayerInput` over the class defaults.
    fn default() -> Self {
        Self {
            slow_walk_threshold: 0.45,
            walk_threshold: 0.95,
            stop_sprint_threshold: 0.8,
            strafe_angle_forward: 10.0,
            strafe_angle_back: 50.0,
            backwards_angle: 40.0,
            min_speed_to_slide: 400.0,
        }
    }
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
    /// `m_LineCheckStepSize`: vertical spacing of the forward edge-search traces.
    #[serde(default = "d_line_step")]
    pub line_check_step_size: f32,
    /// `m_MaxVerticalAngleForEdgeFace`: the wall under the edge may lean at most this far
    /// from vertical (degrees).
    #[serde(default = "d_vert_face")]
    pub max_vertical_angle_edge_face: f32,
    /// `m_MaxHorizontalAngleForEdgeFace`: angle between the wall normal and the facing.
    #[serde(default = "d_horiz_face")]
    pub max_horizontal_angle_edge_face: f32,
    /// `m_fLedgeGrabMantleMinEdgeHeight`: minimum edge height while falling fast.
    #[serde(default = "d_ledge_grab_min")]
    pub ledge_grab_min_edge_height: f32,
}

fn d_line_step() -> f32 {
    12.0
}
fn d_vert_face() -> f32 {
    20.0
}
fn d_horiz_face() -> f32 {
    45.0
}
fn d_ledge_grab_min() -> f32 {
    155.0
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
    /// `m_Levels[i].m_fDefaultTimeBetweenBlinkSteps` (travel steps every this many seconds,
    /// at most one per tick).
    #[serde(default = "d_blink_step_time")]
    pub time_between_steps: Vec<f32>,
    /// `DisTweaks_Blink.m_bLimitVerticalDistanceFromGround`.
    #[serde(default)]
    pub limit_vertical_from_ground: bool,
}

fn d_blink_step_time() -> Vec<f32> {
    vec![0.01, 0.01]
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
    /// Inventory, takedowns, powers and gadgets.
    #[serde(default)]
    pub items: crate::items::ItemsConfig,
    /// field -> evidence (tweak object path / INI key / class default / UE3 stock).
    pub provenance: BTreeMap<String, String>,
}
