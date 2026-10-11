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
    /// Slide impacts (0x00A79BF0 from the slide tick 0x00A7B220): a line trace of
    /// `m_fSlideDamageTestDistance` along the velocity; the first pawn hit takes the
    /// `m_SlideDamageInfo` damage of its class with momentum `velocity x m_fSlideImpactDamageVelRatio`.
    #[serde(default = "d_slide_test")]
    pub slide_damage_test_distance: f32,
    #[serde(default = "d_slide_vel_ratio")]
    pub slide_impact_damage_vel_ratio: f32,
    /// `m_bSlideDamageIgnoresSleepers` (Corvo: true).
    #[serde(default = "d_true_slide")]
    pub slide_damage_ignores_sleepers: bool,
    /// `m_SlideDamageInfo`: `DishonoredNPCPawn` 0, `DisGameCrowdAgentSkeletalRat` 0, `Actor` 5.
    #[serde(default)]
    pub slide_damage_npc: f32,
    #[serde(default)]
    pub slide_damage_rat: f32,
    #[serde(default = "d_slide_other")]
    pub slide_damage_other: f32,
    /// `DishonoredCamera_Lean` (DefaultCamera.ini): the eye swings about a pivot
    /// `lean_height_pct x 2 x CollisionHeight` below it, up to `lean_max_angle` degrees,
    /// driven by a spring (`m_LeanConstants`).
    #[serde(default)]
    pub lean: LeanConfig,
    /// The player camera's resting position (`CameraCache.POV.Location`) as a function of the
    /// view pitch, measured on the original ([`CameraRig`]).
    #[serde(default)]
    pub camera: CameraRig,
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

/// The player camera's position at rest (no lean, no bob): the eye sits on an arc about a neck
/// pivot, `POV = pivot + R (sin(phi0 - pitch), cos(phi0 - pitch))` in the (view-forward, up)
/// plane, the pivot measured from the top of the pawn's collision cylinder. Measured 2026-10-09
/// on Boyle Int (`CameraCache.POV.Location` of the standing pawn at 15 pitches from -85 to 85
/// degrees, fixed 60 Hz: least squares fit, residual below 0.1 uu; the crouched pawn's camera
/// keeps the same arc from its lower cylinder top within 1 uu). The arc comes from the
/// `DishonoredPlayerCamera` influences (the look-pitch pose of the arms rig's camera joint),
/// not from a tweak value: U-980.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CameraRig {
    /// The arc's centre: forward of the pawn's location, and above the top of its cylinder.
    pub pivot_forward: f32,
    pub pivot_up: f32,
    /// The arc's radius and its angle from vertical at pitch 0 (degrees, towards forward).
    pub radius: f32,
    pub phi0: f32,
}

impl Default for CameraRig {
    fn default() -> Self {
        // The fit's centre, 46.582 above the middle of the 87.5 cylinder: -40.918 from its top.
        Self { pivot_forward: -0.6152, pivot_up: -40.918, radius: 31.892, phi0: 10.462 }
    }
}

impl CameraRig {
    /// (forward, up from the cylinder top) of the camera at a pitch in degrees.
    pub fn offset(&self, pitch_deg: f32) -> (f32, f32) {
        let a = (self.phi0 - pitch_deg).to_radians();
        (self.pivot_forward + self.radius * a.sin(), self.pivot_up + self.radius * a.cos())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LeanConfig {
    pub height_pct: f32,
    pub max_angle: f32,
    pub max_angle_crouched: f32,
    pub springiness: f32,
    pub damping: f32,
    /// `m_fMaxLeanSoftenAngle` / `m_fMaxLeanSoftenSpeed`: extra angle allowed while the lean
    /// pushes at the limit (0x00AC5540).
    #[serde(default = "d_soften_angle")]
    pub soften_angle: f32,
    #[serde(default = "d_soften_speed")]
    pub soften_speed: f32,
    /// `m_fCameraTiltPercent`: camera roll as a fraction of the lean angle.
    #[serde(default = "d_tilt")]
    pub camera_tilt_percent: f32,
    /// `DishonoredCameraInfluence.m_fFixedTimeStep` (class default 0.016667): the lean camera
    /// advances in whole steps of this length (`m_bUseFixedTimeStep`), from an accumulator
    /// (`m_fAccumulatedDelta`) fed by the frame time; the view interpolates by its fraction
    /// (0x00ACAAB0).
    #[serde(default = "d_fixed_step")]
    pub fixed_time_step: f32,
    /// `StatePlayerMasterLeaning.m_fLeanSpeed` (class default 1000): the head point's velocity
    /// while a lean key is held (0x00A76590).
    #[serde(default = "d_lean_speed")]
    pub lean_speed: f32,
    /// `StatePlayerMasterLeaning.m_fLeanReleaseTime` (class default 0.2): how long the state
    /// outlives the key.
    #[serde(default = "d_release_time")]
    pub release_time: f32,
}

fn d_fixed_step() -> f32 {
    0.016667
}
fn d_lean_speed() -> f32 {
    1000.0
}
fn d_release_time() -> f32 {
    0.2
}
fn d_soften_angle() -> f32 {
    5.0
}
fn d_soften_speed() -> f32 {
    5.0
}
fn d_tilt() -> f32 {
    0.5
}

impl Default for LeanConfig {
    fn default() -> Self {
        Self { height_pct: 2.0, max_angle: 15.0, max_angle_crouched: 15.0, springiness: 80.0, damping: 12.0, soften_angle: 5.0, soften_speed: 5.0, camera_tilt_percent: 0.5, fixed_time_step: d_fixed_step(), lean_speed: d_lean_speed(), release_time: d_release_time() }
    }
}

fn d_slide_test() -> f32 {
    75.0
}
fn d_slide_vel_ratio() -> f32 {
    0.15
}
fn d_true_slide() -> bool {
    true
}
fn d_slide_other() -> f32 {
    5.0
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
    /// `m_bLowMantleUseStepUp`: an edge no higher than `low_max_edge_height` (and not needing
    /// a crouch) is a step-up, not a mantle (0x00AD78D0 -> 0x00AD35E0).
    #[serde(default)]
    pub low_use_step_up: bool,
    /// The mantle sequences' root motion, low / medium / high (`Ply_Player_Mantle_at`); the
    /// mantle state moves the pawn by it (`RMM_Translate`).
    #[serde(default)]
    pub anims: Vec<MantleAnim>,
    /// `Attribute_MantleAnimRate` (`m_MantleAnimRate`, Normal 1): the play rate of the mantle
    /// sequences (`m_bScaleByMantleAnimRate`), raised by Agility.
    #[serde(default = "d_mantle_rate")]
    pub anim_rate: f32,
}

fn d_mantle_rate() -> f32 {
    1.0
}

/// Root motion of one player mantle sequence (`DishonoredAnimNodeSeqMantle`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MantleAnim {
    pub sequence: String,
    /// `m_fMantleTravelDistanceZ`: the sequence's own vertical travel. The node scales the
    /// root motion's Z by mantle height / this (0x00A8F9B0); X and Y are unscaled.
    pub travel_z: f32,
    pub sample_rate: f32,
    /// Length in seconds.
    pub duration: f32,
    /// Root bone translation per frame in pawn space, relative to frame 0: (forward, up).
    pub root: Vec<[f32; 2]>,
}

impl MantleAnim {
    /// Root offset (forward, up) at `t` seconds, linear between frames, held after the end.
    pub fn root_at(&self, t: f32) -> [f32; 2] {
        let Some(last) = self.root.last() else { return [0.0, 0.0] };
        let f = (t.max(0.0) * self.sample_rate).min((self.root.len() - 1) as f32);
        let i = f.floor() as usize;
        let j = (i + 1).min(self.root.len() - 1);
        if i >= self.root.len() - 1 {
            return *last;
        }
        let a = f - i as f32;
        let (p, q) = (self.root[i], self.root[j]);
        [p[0] + (q[0] - p[0]) * a, p[1] + (q[1] - p[1]) * a]
    }
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
    /// `m_Levels[i].m_fBendTimeDownTime`: blend of the Blink time-dilation layer toward its
    /// world rate when the move starts (0x00BFC990).
    #[serde(default = "d_blink_down")]
    pub bend_time_down_time: Vec<f32>,
    /// `m_Levels[i].m_fBendTimeUpTime`: blend of the layer back to 1 at the end of the move and
    /// again when the cooldown ends (0x00BE7380).
    #[serde(default = "d_blink_up")]
    pub bend_time_up_time: Vec<f32>,
    /// `m_Levels[i].m_fCooldownTime`: seconds after the move (or cancel) before Blink can be
    /// started again (0x00BED480).
    #[serde(default = "d_blink_cooldown")]
    pub cooldown_time: Vec<f32>,
}

fn d_blink_step_time() -> Vec<f32> {
    vec![0.01, 0.01]
}
// Serde defaults for configs cached before these fields were loaded: the `Twk_Blink` values
// (both levels), so an old cache behaves as a fresh load.
fn d_blink_down() -> Vec<f32> {
    vec![0.9, 0.9]
}
fn d_blink_up() -> Vec<f32> {
    vec![2.5, 2.5]
}
fn d_blink_cooldown() -> Vec<f32> {
    vec![1.0, 1.0]
}

/// A thrown NPC (`StateNPCMasterThrown`, research/dishonored/systems/physics.md):
/// - `effective_mass`: `Twk_Component_HitReactionThrow_NPC.m_ComponentConfig.m_fEffectiveMassForThrowing`
///   (0x00BEB6E0: the throw velocity is `momentum / effective_mass`, 2 without the tweak);
/// - the reaction thresholds of the same config (data only: their consumer is not decoded);
/// - `min_momentum_*`: the state's class defaults `m_fMinMomentum2DMagnitude` / `m_fMinMomentumZ`
///   and their `...WhenStunned` pair (0x00BE8710 on entry 0x00A86B90);
/// - `fluid_friction`: `Default__PhysicsVolume.FluidFriction` (the flight is `PHYS_Flying`, whose
///   `CalcVelocity` takes half of it, fluid);
/// - `windblast_ragdoll_notify`: the `DishonoredNotify_Ragdoll` time (sequence seconds over
///   `RateScale`) of `Npc_EliteGuard_Generic_as` `Generic_WindBlast_Front` / `_Left` / `_Right` /
///   `_Back`, indexed by `m_ThrowDirection`: the notify ragdolls the pawn (0x00B6FBB0, event 5),
///   which ends the flight (the throw state's callback 0x00A7CBA0).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ThrowConfig {
    pub effective_mass: f32,
    pub strong_hit_react_threshold: f32,
    pub anim_ragdoll_threshold: f32,
    pub full_ragdoll_threshold: f32,
    pub ragdoll_move_end_threshold: f32,
    pub min_momentum_2d: f32,
    pub min_momentum_z: f32,
    pub min_momentum_2d_stunned: f32,
    pub min_momentum_z_stunned: f32,
    pub fluid_friction: f32,
    pub windblast_ragdoll_notify: [f32; 4],
    /// `DisTweaks_NPCPawn_Body.m_fMaxRagdollSpeedToRecover`: the throw state's rest test
    /// (0x00A98DD0) gets the pawn up once each horizontal Velocity component is below it (times
    /// `CustomTimeDilation`).
    pub max_ragdoll_speed_to_recover: f32,
    /// The get-up after the rest (`StateNPCMasterStunned`, `eStunEnterAnim_RagdollFront`: the
    /// elite guard's `Stun` chain group 1): the `RagdollToFront` sequence's play length, then
    /// (after the loop step, left at once with `m_fStunDuration` 0) one of the recovery sequences.
    pub getup_anim: f32,
    pub getup_recover: Vec<f32>,
}

// Serde defaults for configs cached before the field was loaded: the loaded values.
impl Default for ThrowConfig {
    fn default() -> Self {
        Self {
            effective_mass: 2.0,
            strong_hit_react_threshold: 500.0,
            anim_ragdoll_threshold: 1000.0,
            full_ragdoll_threshold: 3800.0,
            ragdoll_move_end_threshold: 10.0,
            min_momentum_2d: 800.0,
            min_momentum_z: 0.0,
            min_momentum_2d_stunned: 3000.0,
            min_momentum_z_stunned: 1500.0,
            fluid_friction: 0.3,
            windblast_ragdoll_notify: [0.455, 0.39, 0.4483, 0.4179],
            max_ragdoll_speed_to_recover: 5.0,
            getup_anim: 2.3333,
            getup_recover: vec![1.8667, 1.5],
        }
    }
}

/// A bend-time window of a finisher's master clip (`DisNotify_BendTime_Ranged`; with `adrenaline`,
/// the `DisNotify_AdrenalineBendTime_Ranged` only an exclusive, not real-time finisher honours):
/// from `start` for `duration` seconds of the clip the world runs at `world` and the player at
/// `player`, blended in and out over the transition times (the dilation helper 0x009FA0D0,
/// channel 4).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BendWindow {
    pub start: f32,
    pub duration: f32,
    pub world: f32,
    pub player: f32,
    pub blend_in: f32,
    pub blend_out: f32,
    pub adrenaline: bool,
}

/// One animation state of a finisher action (`DisItemAction.m_AnimStates`): the player state, its
/// master sequence (the pawn tweak's state, item prefix applied), the length of the action (the
/// master's `DishonoredNotify_AnimStateExit`, else its length), the victim's slave chain
/// (`m_SlaveAction.m_Animation`, an `eDisNPCAnimations` index; 0 for an unsynced action) whose
/// group is the state's index in the action, and the master's bend-time windows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FatalityClip {
    pub state: String,
    pub master: String,
    pub exit: f32,
    pub slave: u8,
    /// The state plays on the upper body (`DisTweaks_PlayerPawn_UpperBodyAnimation`).
    #[serde(default)]
    pub upper_body: bool,
    /// An unsynced swing's hit: when its `DishonoredNotify_AttackZone`'s damage zone opens (the
    /// zone's end less `m_fDamageZoneMaxTime`, 0x00A8E760).
    #[serde(default)]
    pub hit: Option<f32>,
    pub bends: Vec<BendWindow>,
}

/// The actions of a fatality move set, by [`FatalityConfig::moves`] index.
pub mod fatality_move {
    pub const FRONT: usize = 0;
    pub const LEFT: usize = 1;
    pub const RIGHT: usize = 2;
    pub const BACK: usize = 3;
    pub const FRONT_FAST: usize = 4;
    pub const LEFT_FAST: usize = 5;
    pub const RIGHT_FAST: usize = 6;
    pub const BACK_FAST: usize = 7;
    pub const HEAD_CHOP: usize = 8;
    pub const KNOCKDOWN: usize = 9;
    pub const KNOCKDOWN_FAST: usize = 10;
    /// `m_Generic`: an unsynced kill swing.
    pub const GENERIC: usize = 11;
    pub const COUNT: usize = 12;
    /// The `DisFatalityMoveSet` fields, in this order.
    pub const FIELDS: [&str; COUNT] = ["m_Front", "m_Left", "m_Right", "m_Back", "m_Front_Fast", "m_Left_Fast", "m_Right_Fast", "m_Back_Fast", "m_HeadChop", "m_Knockdown", "m_Knockdown_Fast", "m_Generic"];
}

/// The player sword's `DisTweaks_Fatality` (the finisher kills; `dishonored_sim::fatality`): move
/// set 0 (humans) and the `DisTweaks_Finisher` slow-finisher pacing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FatalityConfig {
    pub behead_chance: f32,
    pub slomo_chance: f32,
    pub slomo_min_interval: f32,
    pub slomo_realtime_chance: f32,
    pub generic_chance: f32,
    /// `m_NumFinishersBeforeSlow_Min/Max`, `m_fTimeBeforeSlow_Min/Max`.
    pub finishers_before_slow: [i32; 2],
    pub time_before_slow: [f32; 2],
    /// `m_fAdrenalineMultOnHit`: adrenaline gained x the victim's health.
    pub adrenaline_mult: f32,
    /// `m_fMaxContextHeightDifference`: the target's height difference limit.
    pub max_height_difference: f32,
    /// Per [`fatality_move`] action, its states (empty: the action has none).
    pub moves: Vec<Vec<FatalityClip>>,
}

// Serde default for configs cached before the field was loaded: the loaded chances and pacing,
// no actions (no fatality plays until the config is decoded again).
impl Default for FatalityConfig {
    fn default() -> Self {
        Self {
            behead_chance: 0.25,
            slomo_chance: 0.75,
            slomo_min_interval: 30.0,
            slomo_realtime_chance: 0.9,
            generic_chance: 0.0,
            finishers_before_slow: [4, 8],
            time_before_slow: [90.0, 120.0],
            adrenaline_mult: 1.0,
            max_height_difference: 50.0,
            moves: vec![Vec::new(); fatality_move::COUNT],
        }
    }
}

/// The root motion of a player master sequence (its state's `m_RootMotionMode` `RMM_Translate`
/// and `m_RootMotionRotationMode` `RMRM_RotateActor`: the root bone's travel moves the pawn and its
/// turn turns it): the root bone's offset from its first frame in the pawn's frame (forward,
/// right) and its yaw from the first frame (degrees, positive to the right), `rate` samples per
/// second.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RootMotion {
    pub rate: f32,
    pub keys: Vec<[f32; 2]>,
    #[serde(default)]
    pub yaw: Vec<f32>,
}

impl RootMotion {
    /// Sample index and blend of clip time `t` over `n` samples (held past the ends).
    fn span(&self, t: f32, n: usize) -> Option<(usize, usize, f32)> {
        let last = n.checked_sub(1)?;
        let x = (t * self.rate).clamp(0.0, last as f32);
        let i = (x.floor() as usize).min(last);
        Some((i, (i + 1).min(last), x - i as f32))
    }

    /// The offset at clip time `t` (linear between samples).
    pub fn at(&self, t: f32) -> [f32; 2] {
        let Some((i, j, f)) = self.span(t, self.keys.len()) else { return [0.0; 2] };
        let (a, b) = (self.keys[i], self.keys[j]);
        [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]
    }

    /// The yaw at clip time `t` (linear between samples).
    pub fn yaw_at(&self, t: f32) -> f32 {
        let Some((i, j, f)) = self.span(t, self.yaw.len()) else { return 0.0 };
        self.yaw[i] + (self.yaw[j] - self.yaw[i]) * f
    }
}

/// The masters of the player's paired moves, by `SwordState::assassination_move`:
/// `Ply_Sword_Assassination_as`'s front, left, right and back assassinations
/// (`DisTweaks_Assassinate` `m_Front/m_Left/m_Right/m_Back` -> `Assassinate_<dir>`), the drop set's
/// (`Assassinate_Drop_<dir>`), then the synced short fatalities of `Ply_Sword_Fatality_as`
/// (`ShortFatality_Forehand{A,B}` / `Backhand{A,B}`).
pub const ASSASSINATION_MASTERS: [&str; 12] = [
    "Sword_Ready_Assassination_Front_Master",
    "Sword_Ready_Assassination_Left_Master",
    "Sword_Ready_Assassination_Right_Master",
    "Sword_Ready_Assassination_Back_Master",
    "Sword_Ready_Assassination_DropFront_Master",
    "Sword_Ready_Assassination_DropLeft_Master",
    "Sword_Ready_Assassination_DropRight_Master",
    "Sword_Ready_Assassination_DropBack_Master",
    "Sword_Ready_Fatality_SmallFront_Right_A_Master",
    "Sword_Ready_Fatality_SmallFront_Right_B_Master",
    "Sword_Ready_Fatality_SmallFront_Left_A_Master",
    "Sword_Ready_Fatality_SmallFront_Left_B_Master",
];

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
    /// The NPC throw (`StateNPCMasterThrown`, `Twk_Component_HitReactionThrow_NPC`).
    #[serde(default)]
    pub throw: ThrowConfig,
    /// The player sword's finisher kills (`DisTweaks_Fatality`).
    #[serde(default)]
    pub fatality: FatalityConfig,
    /// The paired moves' masters' root motion, by sequence name.
    #[serde(default)]
    pub root_motion: BTreeMap<String, RootMotion>,
    /// The sequences the simulation plays (`dishonored_sim::anim`), by [`sequence_key`]: their
    /// lengths, rate scales and notifies, so the simulation steps them and fires their notifies
    /// with or without the install. Its sorted key order is the index `SimEvent::AnimNotify`
    /// names.
    #[serde(default)]
    pub sequences: BTreeMap<String, SequenceInfo>,
    /// The NPC animation the simulation's rules time NPC actions with (the elite guard's
    /// `DisNPCAnimDefinitions` chains and sets).
    #[serde(default)]
    pub npc_anim: NpcAnimConfig,
    /// field -> evidence (tweak object path / INI key / class default / UE3 stock).
    pub provenance: BTreeMap<String, String>,
}

/// The key of a sequence in [`GameplayConfig::sequences`]: its `AnimSet`'s object name and the
/// sequence name, as UE3 paths the `AnimSequence` (`Ply_Sword_Attack_as.Sword_Ready_AttackRight_A_Small`).
pub fn sequence_key(set: &str, sequence: &str) -> String {
    format!("{set}.{sequence}")
}

/// One `AnimNotifyEvent` of a sequence (`UAnimSequence.Notifies`, cooked in time order): the
/// notify object's class, `Time` and `Duration` (sequence seconds; a notify with a duration is
/// one of Arkane's windows, `0x00A8F340` / `0x00A9BC90`), and the notify object's properties over
/// its class defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NotifyInfo {
    pub class: String,
    pub time: f32,
    #[serde(default)]
    pub duration: f32,
    /// Floats, ints, bytes and bools (0 / 1), by property name (`m_fDamageZoneMaxTime`, ...).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub payload: BTreeMap<String, f32>,
    /// Names, enums and object paths, by property name (`m_SwingDirection`, `AkEvent`, ...).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, String>,
}

/// One `AnimSequence` (`SequenceLength`, `RateScale`, `Notifies`). A sequence advances by
/// `RateScale x Rate x dt` of sequence time per tick (`AnimNodeSequence::TickAnim` 0x0059D6A0);
/// notify times are sequence times.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SequenceInfo {
    pub duration: f32,
    pub rate_scale: f32,
    pub notifies: Vec<NotifyInfo>,
    /// The key of the sequence's root motion in [`GameplayConfig::root_motion`] when that table
    /// has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_motion: Option<String>,
}

impl SequenceInfo {
    /// The first notify of `class`.
    pub fn notify(&self, class: &str) -> Option<&NotifyInfo> {
        self.notifies.iter().find(|n| n.class == class)
    }

    /// Play time (seconds at rate 1) of the first notify of `class`: `Time / RateScale`.
    pub fn notify_time(&self, class: &str) -> Option<f32> {
        self.notify(class).map(|n| n.time / self.rate_scale_or_one())
    }

    /// Play length at rate 1: `SequenceLength / RateScale`.
    pub fn play_length(&self) -> f32 {
        self.duration / self.rate_scale_or_one()
    }

    /// `RateScale`, 1 where the sequence has none (0 never advances).
    pub fn rate_scale_or_one(&self) -> f32 {
        if self.rate_scale > 0.0 { self.rate_scale } else { 1.0 }
    }

    /// Play time at which the state the sequence plays ends: its `DishonoredNotify_AnimStateExit`,
    /// else its length.
    pub fn exit_time(&self) -> f32 {
        self.notify_time("DishonoredNotify_AnimStateExit").unwrap_or_else(|| self.play_length())
    }
}

/// The NPC animation profile the simulation's rules use: a `DisAnimDefinitions` class's chains
/// (`m_Animations`), the `AnimSet` object names its sequences are looked up in (last set first,
/// `FindAnimSequence`), and the pawn's primary-item and empty-hands prefixes (`m_ItemPrefix`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NpcAnimConfig {
    pub defs: crate::npc_anim::AnimDefs,
    pub sets: Vec<String>,
    pub item_prefix: String,
    pub empty_prefix: String,
}

impl GameplayConfig {
    /// A sequence of a set (see [`sequence_key`]).
    pub fn sequence(&self, set: &str, sequence: &str) -> Option<&SequenceInfo> {
        self.sequences.get(&sequence_key(set, sequence))
    }
}
