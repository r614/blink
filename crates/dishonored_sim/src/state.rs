//! Authoritative simulation state. Contains only gameplay-relevant data (no visuals).

use crate::input::{LeftHand, PlayerInput};
use glam::Vec3;
use serde::{Deserialize, Serialize};

pub type PlayerId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MoveMode {
    #[default]
    Walking,
    Falling,
    /// Climbing onto a ledge: interpolates from `from` to `to` over `duration` seconds.
    Mantling,
    /// Blink travel in progress.
    Blinking,
    Dead,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Traversal {
    pub from: Vec3,
    pub to: Vec3,
    pub elapsed: f32,
    pub duration: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SwordPhase {
    #[default]
    Idle,
    /// Wind-up before the hit window.
    Startup,
    /// Hit window: the swing trace is live.
    Active,
    Recovery,
    Blocking,
    /// Knocked off balance after being parried / block broken.
    Staggered,
    /// Locked in an assassination (attacker or victim).
    Assassinating,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct SwordState {
    pub phase: SwordPhase,
    pub timer: f32,
    /// Seconds since block started (perfect-block / parry window).
    pub block_time: f32,
    /// Already applied damage during this swing.
    pub hit_done: bool,
    pub assassination_target: Option<PlayerId>,
    /// `m_fForceBlockTimeForPerfectParry`: block held (and parries suppressed) after a
    /// tap parry.
    pub force_block: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LeftPhase {
    #[default]
    Idle,
    /// Holding the button: Blink aiming, ranged aiming.
    Aiming,
    /// Equipping / switching the left-hand item.
    Equipping,
    /// Fire/cast recovery.
    Recovering,
    Reloading,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct LeftHandState {
    pub item: LeftHand,
    pub phase: LeftPhase,
    pub timer: f32,
    pub pistol_loaded: u8,
    pub pistol_reserve: u16,
    pub bolts_loaded: u8,
    pub bolts_reserve: u16,
    /// Current Blink destination while aiming (None = invalid target).
    pub blink_target: Option<Vec3>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Vitals {
    /// Whole points, as the original keeps them (`Pawn.Health` and the mana are ints).
    pub health: f32,
    pub mana: f32,
    /// Countdown to the next mana step (`ManaRegenInitialDelay` after a spend, then
    /// `ManaRegenStepTime`).
    pub mana_regen_timer: f32,
    /// Mana points still to regenerate (at most `ManaRegenAdditivePortion`).
    pub mana_regen_pool: f32,
    /// Mana level regeneration will reach (`mana + pool`, for the HUD).
    pub mana_regen_cap: f32,
    /// `m_HealthRegenTickCountdown`.
    pub health_regen_timer: f32,
    pub dark_vision_remaining: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PawnKind {
    #[default]
    Human,
    /// AI-driven pawn (crowd civilian or guard). Mechanically identical to players.
    Civilian,
    Guard,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    pub id: PlayerId,
    pub kind: PawnKind,
    /// Visual archetype index (presentation only; players and NPCs share the pool).
    pub archetype: u8,
    /// Collision cylinder centre (UE `Location`), Dishonored world space.
    pub position: Vec3,
    pub velocity: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub mode: MoveMode,
    pub crouched: bool,
    pub floor_normal: Vec3,
    pub traversal: Traversal,
    pub vitals: Vitals,
    pub sword: SwordState,
    pub left: LeftHandState,
    pub alive: bool,
    pub respawn_timer: f32,
    /// Previous tick's buttons, for edge detection.
    pub prev_buttons: u16,
    pub last_input: PlayerInput,
    /// Seconds spent falling, and peak downward speed (fall damage).
    pub fall_time: f32,
    pub lean: f32,
    pub lean_velocity: f32,
    /// Ground-speed modifier of the previous tick (`DishonoredPawn.m_fLastSpeedModifier`):
    /// drops are blended down at `speed_blend_down_speed`, rises are instant.
    pub speed_mod: f32,
    /// Locomotion flags the original keeps on the pawn (`m_bSprinting`, `m_bStrafing`,
    /// `m_bMovingBackwards`, `Pawn.bIsWalking`).
    pub sprinting: bool,
    pub strafing: bool,
    pub moving_backwards: bool,
    pub walking: bool,
    /// Blink travel in progress (`DishonoredActivePowerComponent_Blink` move mode).
    pub blink: BlinkTravel,
    /// Sprint-slide (`StatePlayerMasterSlide`).
    pub slide: SlideState,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct SlideState {
    pub active: bool,
    pub elapsed: f32,
    pub start_velocity: Vec3,
    pub target_velocity: Vec3,
    /// Speed fell below the target: velocity is no longer driven by the ease.
    pub stop_interpolating: bool,
}

/// State of a Blink move: the original steps toward the target with collision instead of
/// interpolating, and restores the pre-Blink velocity at the end.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct BlinkTravel {
    pub target: Vec3,
    pub origin: Vec3,
    pub origin_velocity: Vec3,
    /// `m_MaxDistanceFromOrigLoc`.
    pub max_distance: f32,
    /// `m_fTimeToNextBlink`.
    pub step_timer: f32,
    pub steps: u16,
}

impl PlayerState {
    pub fn new(id: PlayerId, position: Vec3, yaw: f32) -> Self {
        Self {
            id,
            kind: PawnKind::Human,
            archetype: 0,
            position,
            velocity: Vec3::ZERO,
            yaw,
            pitch: 0.0,
            mode: MoveMode::Falling,
            crouched: false,
            floor_normal: Vec3::Z,
            traversal: Traversal::default(),
            vitals: Vitals::default(),
            sword: SwordState::default(),
            left: LeftHandState::default(),
            alive: true,
            respawn_timer: 0.0,
            prev_buttons: 0,
            last_input: PlayerInput::default(),
            fall_time: 0.0,
            lean: 0.0,
            lean_velocity: 0.0,
            speed_mod: 1.0,
            sprinting: false,
            strafing: false,
            moving_backwards: false,
            walking: false,
            blink: BlinkTravel::default(),
            slide: SlideState::default(),
        }
    }

    pub fn just_pressed(&self, input: &PlayerInput, b: u16) -> bool {
        input.buttons & b != 0 && self.prev_buttons & b == 0
    }
    pub fn just_released(&self, input: &PlayerInput, b: u16) -> bool {
        input.buttons & b == 0 && self.prev_buttons & b != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProjectileKind {
    Bullet,
    Bolt,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ProjectileState {
    pub id: u32,
    pub kind: ProjectileKind,
    pub owner: PlayerId,
    pub position: Vec3,
    pub velocity: Vec3,
    pub age: f32,
    pub damage: f32,
    pub gravity_mult: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorldState {
    pub tick: u64,
    pub players: Vec<PlayerState>,
    pub projectiles: Vec<ProjectileState>,
    pub next_projectile_id: u32,
    /// Windblasts in flight (`DishonoredActivePowerComponent_WindBlast` warmup / blast).
    #[serde(default)]
    pub windblasts: Vec<WindblastState>,
}

/// A Windblast: `m_fWarmupTime` after the cast the origin and direction are taken from
/// the caster's view and the blast front expands at `m_Speed`; targets gathered at that
/// moment are hit when the front reaches them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindblastState {
    pub caster: PlayerId,
    /// Seconds until the blast starts (warmup), then seconds since it started.
    pub warmup: f32,
    pub elapsed: f32,
    pub origin: Vec3,
    pub direction: Vec3,
    /// `m_ActorsToBlast` still to be reached.
    pub targets: Vec<PlayerId>,
    pub started: bool,
}

impl WorldState {
    pub fn player(&self, id: PlayerId) -> Option<&PlayerState> {
        self.players.iter().find(|p| p.id == id)
    }
    pub fn player_mut(&mut self, id: PlayerId) -> Option<&mut PlayerState> {
        self.players.iter_mut().find(|p| p.id == id)
    }
}
