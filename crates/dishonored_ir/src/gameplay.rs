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
    /// `m_fSlidePercentNotCancelable`: a slide can be cancelled once `SlideTime * this` has
    /// passed (0x00A71450).
    #[serde(default = "d_slide_cancel")]
    pub slide_percent_not_cancelable: f32,
    /// `DishonoredCamera_Lean` (DefaultCamera.ini): the eye swings about a pivot
    /// `lean_height_pct x 2 x CollisionHeight` below it, up to `lean_max_angle` degrees,
    /// driven by a spring (`m_LeanConstants`).
    #[serde(default)]
    pub lean: LeanConfig,
    /// Swimming and breath (`StatePlayerMasterSwim`, `APawn::physSwimming`,
    /// `DisTweaks_PlayerPawn.m_fApnea*`, `Attribute_Drowning*`).
    #[serde(default)]
    pub water: WaterMoveConfig,
    /// `Attribute_JumpZ_PowerJump` before modifiers (0 for Corvo; Agility adds 1300): the
    /// impulse of the power jump at the top of a held jump.
    #[serde(default)]
    pub power_jump_z: f32,
    /// Climbing `DisClimbable` chains (`StatePlayerMasterClimb`, `physLadder` 0x00AA82C0).
    #[serde(default)]
    pub climb: ClimbConfig,
}

/// Climbing constants. Sources: `Twk_Pawn_Corvo_Release` (`m_fClimbingSpeed`, `m_fJumpImpulse`,
/// `m_fClimbingSound{Min,Max}Delay`), `Twk_Pawn_PlayerInput` (`m_fClimbReuseDelay`,
/// `m_bClimbReuseDelayJump`), `DefaultPlayer.ini` (`m_ClimbingAimThreshold`) and the
/// `DisTweaks_Climbable` class default (`m_fCollisionRestorationDelay`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClimbConfig {
    /// `m_fClimbingSpeed`: x 16383.5 is the height offset of the climb target each tick.
    pub speed: f32,
    /// `m_fJumpImpulse`: the velocity along the facing given when leaving the chain.
    pub jump_impulse: f32,
    pub sound_min_delay: f32,
    pub sound_max_delay: f32,
    /// `m_fClimbReuseDelay`: no letting go (and, with `reuse_delay_jump`, no jumping off) this
    /// soon after grabbing.
    pub reuse_delay: f32,
    pub reuse_delay_jump: bool,
    /// `m_ClimbingAimThreshold` (rotator units of camera pitch).
    pub aim_threshold: i32,
    /// `DisTweaks_Climbable.m_fCollisionRestorationDelay`.
    pub collision_restoration_delay: f32,
    /// `DishonoredPlayerPawn.m_bCameraRelativeClimbing` (transient, default false; its setter
    /// was not found, U-440).
    pub camera_relative: bool,
}

impl Default for ClimbConfig {
    fn default() -> Self {
        Self {
            speed: 0.0005,
            jump_impulse: 525.0,
            sound_min_delay: 0.3,
            sound_max_delay: 1.8,
            reuse_delay: 0.5,
            reuse_delay_jump: true,
            aim_threshold: 1000,
            collision_restoration_delay: 0.3,
            camera_relative: false,
        }
    }
}

/// Swimming constants. Sources: `DefaultPlayerState.ini [StatePlayerMasterSwim]`,
/// `Twk_Pawn_Corvo_Release` (`m_fApneaMaxDuration`, `m_fApneaRecoveryDuration`), the attribute
/// tweak (`m_WaterSpeed`, `m_DrowningDamageAmount`, `m_DrowningDamageStep`) and
/// `Default__DishonoredPawn` (`Buoyancy`, `OutofWaterZ`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WaterMoveConfig {
    /// `Attribute_WaterSpeed` (Pawn.WaterSpeed).
    pub water_speed: f32,
    pub buoyancy: f32,
    pub out_of_water_z: f32,
    pub min_swim_accel: f32,
    pub max_swim_accel: f32,
    pub max_speed_no_stroke: f32,
    pub stroke_time: f32,
    pub surface_stroke_delay: f32,
    pub constant_speed_at_surface: bool,
    pub apnea_max: f32,
    pub apnea_recovery: f32,
    pub drowning_damage: f32,
    pub drowning_step: f32,
}

impl Default for WaterMoveConfig {
    fn default() -> Self {
        Self {
            water_speed: 600.0,
            buoyancy: 1.0,
            out_of_water_z: 533.0,
            min_swim_accel: 500.0,
            max_swim_accel: 4000.0,
            max_speed_no_stroke: 350.0,
            stroke_time: 0.3,
            surface_stroke_delay: 2.0,
            constant_speed_at_surface: true,
            apnea_max: 30.0,
            apnea_recovery: 5.0,
            drowning_damage: 1.0,
            drowning_step: 1.0,
        }
    }
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
fn d_slide_cancel() -> f32 {
    0.5
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

/// Blood Thirsty's adrenaline (`Attribute_Adrenaline*` of the player's attribute tweak, the melee
/// tweaks' `m_fAdrenalineMultOnHit` and `m_fAdrenalineAddedParryWin`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdrenalineConfig {
    pub max: f32,
    pub burn_rate: f32,
    pub cooldown: f32,
    pub mult_taking_damage: f32,
    /// `DisTweaks_MeleeAttackPlayer.m_fAdrenalineMultOnHit` (sword hits: damage x this).
    pub mult_on_hit: f32,
    /// The assassination / drop assassination tweaks' `m_fAdrenalineMultOnHit` (victim health x this).
    pub mult_assassinate: f32,
    pub mult_drop_assassinate: f32,
    /// `DisTweaks_MeleeBlockPlayer.m_fAdrenalineAddedParryWin`.
    pub parry_win: f32,
}

impl Default for AdrenalineConfig {
    fn default() -> Self {
        Self { max: 250.0, burn_rate: 1.0, cooldown: 10.0, mult_taking_damage: 0.0, mult_on_hit: 2.0, mult_assassinate: 1.25, mult_drop_assassinate: 1.5, parry_win: 20.0 }
    }
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
    /// Blood Thirsty's adrenaline gauge.
    #[serde(default)]
    pub adrenaline: AdrenalineConfig,
    /// field -> evidence (tweak object path / INI key / class default / UE3 stock).
    pub provenance: BTreeMap<String, String>,
}
