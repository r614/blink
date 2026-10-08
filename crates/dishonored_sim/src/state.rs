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
    /// `PHYS_Swimming`: the pawn's centre is in a (deep) water volume (`APawn::physSwimming`).
    Swimming,
    /// `StatePlayerMasterClimb` / `PHYS_Ladder`: hanging on a `DisClimbable` chain
    /// (`ADishonoredPlayerPawn::physLadder` 0x00AA82C0), see [`ClimbState`].
    Climbing,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Traversal {
    pub from: Vec3,
    pub to: Vec3,
    pub elapsed: f32,
    pub duration: f32,
    /// Mantle: the edge's height above the feet (`DishonoredAnimNodeBlendByMantleType` picks
    /// low / medium / high from it against `m_LowMantleMaxEdgeHeight` /
    /// `m_MediumMantleMaxEdgeHeight`).
    #[serde(default)]
    pub edge_height: f32,
    /// Mantle: the sequence moving the pawn (index into `MantleConfig::anims`: low, medium,
    /// high) and its play rate; `None` interpolates `from` -> `to` linearly.
    #[serde(default)]
    pub anim: Option<u8>,
    #[serde(default)]
    pub rate: f32,
    /// Step-ups so far (wrapping; kept across traversals): the first-person view plays the
    /// step-up's head (`StepUpMantle`) on each.
    #[serde(default)]
    pub step_ups: u8,
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
    /// Swings started so far (wrapping) and the clip of the current one: 0-3 cycle
    /// `Sword_Ready_Attack{Right_A,Left_A,Right_B,Left_B}_Small`, 4 is
    /// `Sword_Sneak_Attack_Small` (crouched). The cycle order is the port's (U-601).
    #[serde(default)]
    pub swings: u8,
    #[serde(default)]
    pub swing_clip: u8,
    pub assassination_target: Option<PlayerId>,
    /// `m_fForceBlockTimeForPerfectParry`: block held (and parries suppressed) after a
    /// tap parry.
    pub force_block: f32,
    /// Parries won so far (wrapping): the view plays `Blocking Parry` on each.
    #[serde(default)]
    pub parries: u8,
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
    /// `DishonoredWeapon_Ranged.m_fDispersion` (+0x12C) of the held pistol or crossbow: the
    /// radius of the shot's random offset, in pixels of a 720-line screen (U-247).
    #[serde(default)]
    pub dispersion: f32,
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
    /// A rat (`DisGameCrowdAgentSkeletalRat`): small, no sword or left hand, bites; possessable
    /// with Possession level 1 (`Twk_Possessable_Rat`). Driven by a brain (`dishonored_ai`) or
    /// by its possessor. See [`crate::rats`].
    Rat,
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
    /// `m_fCurMaxLeanSoften` (+0xA0): extra lean angle (degrees) grown while leaning at the limit.
    #[serde(default)]
    pub lean_soften: f32,
    /// The lean head point's upward velocity (`lean_velocity` is its sideways one), uu/s.
    #[serde(default)]
    pub lean_velocity_z: f32,
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
    /// Ammo, elixirs, coins, runes, bone charms, power levels (`DishonoredInventory`,
    /// `DishonoredPowersComponent`).
    #[serde(default)]
    pub inv: Inventory,
    /// Choke / knock-out / possession state of this pawn as a victim or host.
    #[serde(default)]
    pub condition: Condition,
    /// Who caused `condition` (choker, possessor, sleeper).
    #[serde(default)]
    pub condition_by: Option<PlayerId>,
    /// Seconds until a sleep dart puts this pawn to sleep (`m_fTimeBeforeSleep`), 0 = none.
    #[serde(default)]
    pub sleep_timer: f32,
    /// Seconds this pawn has been burning (incendiary bolt), 0 = not burning.
    #[serde(default)]
    pub burning: f32,
    /// The attacker side of a choke (`StatePlayerMasterChoke`).
    #[serde(default)]
    pub choke: ChokeState,
    /// Body this pawn carries (`StatePlayerCarryCorpseIdle`), and the carrier of this body.
    #[serde(default)]
    pub carrying: Option<PlayerId>,
    #[serde(default)]
    pub carried_by: Option<PlayerId>,
    /// Lift / drop animation in progress (seconds left) while carrying.
    #[serde(default)]
    pub carry_anim: f32,
    /// Possession (`DishonoredActivePowerComponent_Possess`).
    #[serde(default)]
    pub possess: PossessState,
    /// Rate this pawn's time ran at this tick (Bend Time), 1 = normal, 0 = stopped.
    #[serde(default = "one")]
    pub time_scale: f32,
    /// Corpse turned to ash (Shadow Kill) or eaten: no body remains.
    #[serde(default)]
    pub no_body: bool,
    /// Action animation lock (power cast, gadget throw, drink): seconds left.
    #[serde(default)]
    pub action_lock: f32,
    /// `StatePlayerMasterJump`: held time and power jump (Agility).
    #[serde(default)]
    pub jump: JumpState,
    /// Swimming strokes, air and drowning.
    #[serde(default)]
    pub water: WaterState,
    /// Pain-causing physics volumes the pawn touched last step (bit per volume index; entry
    /// pain fires on a new bit).
    #[serde(default)]
    pub pain_volumes: u64,
    /// Climbing a chain.
    #[serde(default)]
    pub climb: ClimbState,
    /// Blood Thirsty's adrenaline gauge (`DishonoredPlayerPawn` +0x91C / +0x920).
    #[serde(default)]
    pub adrenaline: AdrenalineState,
    /// Dark Vision timers (`DisActivePowerComponent_DarkVision`).
    #[serde(default)]
    pub dark: DarkVisionState,
    /// Seconds left of a rat bite slowdown (`m_fPawnSlowdownDuration`, `Modifier_RatAttackSpeedModifier`)
    /// and its percent.
    #[serde(default)]
    pub rat_slow: f32,
    #[serde(default)]
    pub rat_slow_percent: f32,
    /// Hook written by the AI each tick for NPC pawns: true while this NPC is unaware of any
    /// threat (no attention). Read by sleep darts (`m_bInstantSleepForUnawareNPCs`) and Shadow Kill
    /// level 1 (0x00A87B60). False for players and when no AI sets it.
    #[serde(default)]
    pub ai_unaware: bool,
    /// Hook written by the mission host each tick for NPC pawns: the locomotion component follows
    /// a navmesh path, so the pawn walks on the navmesh (`movement::nav_walk_move`). False for
    /// players and when no host sets it.
    #[serde(default)]
    pub nav_walk: bool,
    /// Hook written by the mission host with `nav_walk`: the end of the path of an accurate
    /// stop (a scripted go-to). The navmesh move does not pass it: the step that would reach it
    /// lands on it and the pawn stops (arrival within 0.1 uu, 0x00944390). Host-only, never
    /// sent or saved.
    #[serde(skip)]
    pub nav_end: Option<Vec3>,
    /// Helmet of an NPC (`m_PotentialNPCArmor`, `DisTweaks_Armor` protecting the head), until hits
    /// knock it off. The armour damage filter (0x00B78610) zeroes hits of `m_DamageTypesToAbsorb`
    /// types on the protected region and side, but the shipped helmets (elite, city, thug) absorb
    /// nothing: they leave that list to `Twk_Inv_HelmetBase`, which no cooked package contains.
    /// Set by the mission layer from the spawner's armour roll.
    #[serde(default)]
    pub helmet: bool,
    /// Rats attacking this pawn (attack group timers of 0x00C891E0).
    #[serde(default)]
    pub rat_attack: RatAttackState,
    /// `Attribute_DamageReduction` of a player this tick (Armor upgrade): subtracted from damage of
    /// types with `m_bApplyDamageReduction`, never below 1 (0x00AB0A30).
    #[serde(default)]
    pub damage_reduction: f32,
    /// `Attribute_ChanceUseNoManaForSpell` this tick: a power's mana spend is skipped when a roll is
    /// below it (0x00AA93E0).
    #[serde(default)]
    pub free_spell_chance: f32,
    /// This pawn's deterministic random stream (xorshift32; the original uses the C runtime `rand()`).
    #[serde(default)]
    pub rng: u32,
    /// The sword is in its sheath (no weapon drawn): the ground speed takes
    /// `Attribute_SwordSheathedSpeedFactor` instead of the unsheathed one (measured run / sprint
    /// 400 / 600 uu/s sheathed, 360 / 540 drawn, INSTRUMENTATION.md). The game's players start
    /// with it drawn (nothing sheathes it yet); scenario setups set it.
    #[serde(default)]
    pub sword_sheathed: bool,
    /// Phase (0..1) of the player's locomotion loop, whose `DishonoredNotify_Footfall`s make
    /// the footfall contacts (and their AI noise, `noise::step_footfalls`).
    #[serde(default)]
    pub footfall_phase: f32,
    /// `StatePlayerMasterFalling.m_bIsStealthFall`: the pawn was sneaking when its fall state
    /// began (0x00A759F0); the landing is then `JumpLand_Stealth`.
    #[serde(default)]
    pub stealth_fall: bool,
    /// `StatePlayerMasterFalling.m_bCameFromJump`: the fall state was entered from the jump.
    #[serde(default)]
    pub fall_from_jump: bool,
}

/// The attack group on a pawn: seconds under attack (`m_fInitialDelayBeforeDealingDamage`) and
/// the damage interval accumulator.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct RatAttackState {
    pub time: f32,
    pub acc: f32,
}

fn one() -> f32 {
    1.0
}

/// `StatePlayerMasterJump` (0x00A75EE0 enter, 0x00A76110 tick for jump style 0).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct JumpState {
    /// In the jump state (rising from a jump; the state hands over to falling at the apex).
    pub active: bool,
    /// `m_fHeldButtonTime`: seconds the jump button has been held since the jump.
    pub held: f32,
    /// `m_JumpState == eDisJumpState_PowerJump`.
    pub powered: bool,
}

/// Climbing (`DishonoredPlayerPawn.m_pClimbable`, `m_bClimbingUp`, `m_bLettingGoOfClimbable`,
/// `m_fClimbingSoundLastTime`; `StatePlayerMasterClimb.m_fStartTime`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ClimbState {
    /// `m_pClimbable`: index into `WorldQuery::climbables()`. Kept through a jump off the chain
    /// until the jump ends, which keeps the chain from being grabbed again meanwhile.
    pub on: Option<u16>,
    /// Seconds since the climb began (`TimeSeconds - m_fStartTime`).
    pub time: f32,
    /// `!m_bClimbingUp` (the class default is true: looking up).
    pub down: bool,
    /// Seconds since the last climbing sound (`TimeSeconds - m_fClimbingSoundLastTime`).
    pub since_sound: f32,
    /// `m_bLettingGoOfClimbable`: let go with Use; touching a chain does not grab until the fall
    /// ends (`StatePlayerMasterFalling` exit 0x00A63110 clears it).
    pub letting_go: bool,
    /// `StatePlayerMasterJump.m_bJumpingFromChain` (+0x54 bit 0): the jump started on a chain.
    pub jumped: bool,
    /// `DisClimbable.m_fNoCollisionDuration` of the chain this pawn left: it cannot be touched
    /// or used again until this runs out.
    pub no_touch: f32,
}

/// Swimming and breath (`DishonoredPlayerPawn.m_fAirCapacity`, `m_fDrowningDamageCountdown`,
/// `StatePlayerMasterSwim.m_fCurStrokeTime`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct WaterState {
    /// Air used (seconds below `m_fMaxAirCapacity`); 0 = full lungs.
    pub air_used: f32,
    pub drown_timer: f32,
    /// `m_bUnderWater`: the eye is inside a water volume.
    pub under: bool,
    /// Seconds of the current swim stroke left (`m_fCurStrokeTime`).
    pub stroke: f32,
    /// Countdown to the next stroke (`m_fSurfaceSwimDelayBetweenStrokes`).
    pub stroke_delay: f32,
}

/// Blood Thirsty: `+0x91C` adrenaline, `+0x920` seconds since the last gain, `m_bNeedAdrenalineEffect`
/// (the gauge is full).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct AdrenalineState {
    pub value: f32,
    pub idle: f32,
    pub full: bool,
}

/// `DisActivePowerComponent_DarkVision`: `m_fFadeInTimeRemaining`, `m_fTimeRemaining`,
/// `m_fFadeOutTimeRemaining`, `m_fCooldownTimer`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct DarkVisionState {
    pub fade_in: f32,
    pub remaining: f32,
    pub fade_out: f32,
    pub cooldown: f32,
    /// Level used (1 or 2) for the reveal distance.
    pub level: u8,
}

/// Power and charm progression indices (`DishonoredPowersComponent`): level 0 = not owned.
pub mod power {
    pub const BLINK: usize = 0;
    pub const WINDBLAST: usize = 1;
    pub const DARK_VISION: usize = 2;
    pub const POSSESSION: usize = 3;
    pub const BEND_TIME: usize = 4;
    pub const DEVOURING_SWARM: usize = 5;
    pub const VITALITY: usize = 6;
    pub const BLOOD_THIRSTY: usize = 7;
    pub const SHADOW_KILL: usize = 8;
    pub const AGILITY: usize = 9;
    pub const COUNT: usize = 10;
    /// Names as the power tweaks / `m_Powers` call them.
    pub const NAMES: [&str; COUNT] = ["Blink", "WindBlast", "DarkVision", "Possess", "BendTime", "DevouringSwarm", "Vitality", "BloodThirsty", "ShadowKill", "Celerity"];
}

/// The player's inventory (`DishonoredInventory`): ammo counts by `eDisAmmoType` (bullets and
/// bolts stay in [`LeftHandState`]'s reserves), `m_ElixirCounts`, the coin and rune abstract
/// items, owned/equipped bone charms (bit per `m_Charms` index) and power levels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub ammo: [u16; 8],
    pub elixirs: [u8; 2],
    pub coins: u32,
    pub runes: u16,
    pub charms_owned: u32,
    pub charms_equipped: u32,
    pub power_levels: [u8; power::COUNT],
    /// Owned workshop upgrades (bit per `ItemsConfig::upgrades` index).
    #[serde(default)]
    pub upgrades: u32,
}

impl Default for Inventory {
    fn default() -> Self {
        // Players start with Blink, Windblast and Dark Vision at the levels the multiplayer
        // loadout has always used (level 2 Blink/Windblast, Dark Vision 2).
        let mut power_levels = [0; power::COUNT];
        power_levels[power::BLINK] = 2;
        power_levels[power::WINDBLAST] = 2;
        power_levels[power::DARK_VISION] = 2;
        Self { ammo: [0; 8], elixirs: [0; 2], coins: 0, runes: 0, charms_owned: 0, charms_equipped: 0, power_levels, upgrades: 0 }
    }
}

/// What has been done to this pawn (`StateNPCMasterBeingChoked`, unconscious NPCs,
/// `DisPossessablePawn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Condition {
    #[default]
    Normal,
    /// Held in a choke (cannot act).
    BeingChoked,
    /// Knocked out (choke win, sleep dart): non-lethal, stays down; can be carried.
    Unconscious,
    /// Controlled by a possessor.
    Possessed,
}

/// `DisItemContext_Choke.eChokeState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ChokePhase {
    #[default]
    None,
    In,
    Loop,
    Win,
    Lose,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct ChokeState {
    pub phase: ChokePhase,
    pub target: Option<PlayerId>,
    /// `m_fTimer` (counts down from `m_fRequiredTime`).
    pub timer: f32,
    pub required: f32,
    /// Seconds into the current phase.
    pub phase_time: f32,
    /// `m_fHoldTimeForAutoCarry`.
    pub hold: f32,
}

/// `EDisPossessPowerStage`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PossessStage {
    #[default]
    Inactive,
    Targeting,
    PrePossess,
    Active,
    Cooldown,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct PossessState {
    pub stage: PossessStage,
    pub host: Option<PlayerId>,
    /// `m_fPossessionTimer` (seconds of possession left) / pre-possess and cooldown timers.
    pub timer: f32,
    pub cooldown: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct SlideState {
    pub active: bool,
    pub elapsed: f32,
    pub start_velocity: Vec3,
    pub target_velocity: Vec3,
    /// Speed fell below the target: velocity is no longer driven by the ease.
    pub stop_interpolating: bool,
    /// This slide has hit a pawn (the slide tick stops testing; LIKELY: the state flag at +0x70).
    #[serde(default)]
    pub hit: bool,
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
            // A pawn without input this tick repeats its last one (`step_world`): it starts
            // with the spawn view, so a passive NPC keeps its placed yaw.
            last_input: PlayerInput { yaw, ..PlayerInput::default() },
            fall_time: 0.0,
            lean: 0.0,
            lean_velocity: 0.0,
            lean_soften: 0.0,
            lean_velocity_z: 0.0,
            speed_mod: 1.0,
            sprinting: false,
            strafing: false,
            moving_backwards: false,
            walking: false,
            blink: BlinkTravel::default(),
            slide: SlideState::default(),
            inv: Inventory::default(),
            condition: Condition::Normal,
            condition_by: None,
            sleep_timer: 0.0,
            burning: 0.0,
            choke: ChokeState::default(),
            carrying: None,
            carried_by: None,
            carry_anim: 0.0,
            possess: PossessState::default(),
            time_scale: 1.0,
            no_body: false,
            action_lock: 0.0,
            jump: JumpState::default(),
            water: WaterState::default(),
            pain_volumes: 0,
            climb: ClimbState::default(),
            adrenaline: AdrenalineState::default(),
            dark: DarkVisionState::default(),
            rat_slow: 0.0,
            rat_slow_percent: 0.0,
            ai_unaware: false,
            nav_walk: false,
            nav_end: None,
            helmet: false,
            rat_attack: RatAttackState::default(),
            damage_reduction: 0.0,
            free_spell_chance: 0.0,
            rng: id.wrapping_mul(0x9E37_79B9) | 1,
            sword_sheathed: false,
            footfall_phase: 0.0,
            stealth_fall: false,
            fall_from_jump: false,
        }
    }

    /// Cannot act: knocked out, held in a choke, carried, or possessed (the possessor drives
    /// it). AI brains should not think for such pawns (hook for `dishonored_ai`).
    pub fn incapacitated(&self) -> bool {
        matches!(self.condition, Condition::BeingChoked | Condition::Unconscious) || self.carried_by.is_some()
    }

    /// A body on the ground: dead or unconscious, not carried (can be picked up).
    pub fn is_body(&self) -> bool {
        (!self.alive || self.condition == Condition::Unconscious) && self.carried_by.is_none() && !self.no_body
    }

    /// Next roll in [0, 1) of this pawn's random stream (the original's `rand() / 32768`).
    pub fn roll(&mut self) -> f32 {
        let mut x = if self.rng == 0 { 0x2545_F491 } else { self.rng };
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
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
    /// `Twk_Proj_Arrow_Sleep`: puts NPCs to sleep (`DisDamageType_Arrow_Sleep`).
    SleepDart,
    /// `Twk_Proj_Arrow_Flare`: sets the target on fire (`DisDamageType_Arrow_Flare`).
    IncendiaryBolt,
    /// `DisProjectile_Grenade`: bounces, explodes after impact (`DisGrenadeComponent`).
    Grenade,
    /// `DisProjectile_StickyGrenade`: sticks to what it hits.
    StickyGrenade,
}

impl ProjectileKind {
    pub fn is_bolt(self) -> bool {
        matches!(self, Self::Bolt | Self::SleepDart | Self::IncendiaryBolt)
    }
    pub fn is_grenade(self) -> bool {
        matches!(self, Self::Grenade | Self::StickyGrenade)
    }
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
    /// Grenades: armed by the first impact (`DisGrenadeComponent` +0x64 bit 1), seconds to
    /// detonation (+0xB0) and the floor it cannot go below (+0xB4).
    #[serde(default)]
    pub armed: bool,
    #[serde(default)]
    pub fuse: f32,
    #[serde(default)]
    pub min_fuse: f32,
    /// Sticky grenade: pawn it is stuck to (and the offset from its centre), or stuck in the world.
    #[serde(default)]
    pub stuck_to: Option<PlayerId>,
    #[serde(default)]
    pub stuck: bool,
    #[serde(default)]
    pub stuck_offset: Vec3,
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
    /// Pickups in the world (placed by the level or dropped).
    #[serde(default)]
    pub pickups: Vec<PickupState>,
    /// Abstract items held (`DishonoredInventory.m_AbstractItem`): keys, notes, mission items.
    #[serde(default)]
    pub abstract_items: Vec<HeldItem>,
    /// Placed Spring Razors (`DisGadget_SpringRazorPlaced`).
    #[serde(default)]
    pub traps: Vec<TrapState>,
    /// Active Bend Time casts (`DishonoredActivePowerComponent_BendTime`).
    #[serde(default)]
    pub time_bends: Vec<TimeBendState>,
    /// Devouring Swarms (`DisRatSwarm`).
    #[serde(default)]
    pub swarms: Vec<SwarmState>,
    #[serde(default)]
    pub next_entity_id: u32,
}

/// The world parts added with inventory, gadgets and powers, replicated as one block in
/// snapshots (additive to the original protocol fields).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WorldExtras {
    pub pickups: Vec<PickupState>,
    pub abstract_items: Vec<HeldItem>,
    pub traps: Vec<TrapState>,
    pub time_bends: Vec<TimeBendState>,
    pub swarms: Vec<SwarmState>,
    pub next_entity_id: u32,
}

impl WorldState {
    pub fn extras(&self) -> WorldExtras {
        WorldExtras {
            pickups: self.pickups.clone(),
            abstract_items: self.abstract_items.clone(),
            traps: self.traps.clone(),
            time_bends: self.time_bends.clone(),
            swarms: self.swarms.clone(),
            next_entity_id: self.next_entity_id,
        }
    }
    pub fn set_extras(&mut self, e: WorldExtras) {
        self.pickups = e.pickups;
        self.abstract_items = e.abstract_items;
        self.traps = e.traps;
        self.time_bends = e.time_bends;
        self.swarms = e.swarms;
        self.next_entity_id = e.next_entity_id;
    }
}

/// A pickup actor (`DisPickup_Base`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PickupState {
    pub id: u32,
    /// Level actor name (missions refer to it) and the level package it is in.
    pub name: String,
    #[serde(default)]
    pub package: String,
    pub position: Vec3,
    pub kind: dishonored_ir::items::PickupKind,
    /// Taken (destroyed) or still in the world.
    pub active: bool,
    /// Stat pickups: ammo left per type, rolled once from `m_AmmoRanges` (`FUN_00c3d4e0`).
    pub ammo_left: [u16; 8],
    /// Carried by this NPC (`m_pStealablePickup`): follows it, can be pickpocketed.
    pub carried_by: Option<PlayerId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeldItem {
    pub owner: PlayerId,
    pub item: String,
    pub quantity: i32,
}

/// `DisGadget_SpringRazorPlaced`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrapState {
    pub id: u32,
    pub owner: PlayerId,
    pub position: Vec3,
    /// Up direction of the surface it is on.
    pub normal: Vec3,
    /// `m_fArmingTimer`, `m_fDetonationTimer`, `m_fDamageActorTimer`.
    pub arming: f32,
    pub detonation: f32,
    pub damage_timer: f32,
    /// `m_bTriggered`, `m_bDetonated`, `m_bEmittedDamage`.
    pub triggered: bool,
    pub detonated: bool,
    pub emitted: bool,
    /// Radius scale (`Attribute_SpringRazorRadiusBonus` of the owner when above 1, 0x00C2D360).
    #[serde(default = "one")]
    pub radius_scale: f32,
}

/// `DishonoredActivePowerComponent_BendTime` while active.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeBendState {
    pub caster: PlayerId,
    /// 0-based level.
    pub level: u8,
    /// Real (undilated) seconds since start, and `m_fMaxDuration`.
    pub elapsed: f32,
    pub duration: f32,
    pub world_dilation: f32,
    pub self_dilation: f32,
}

/// A Devouring Swarm.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SwarmState {
    pub id: u32,
    pub owner: PlayerId,
    pub level: u8,
    pub position: Vec3,
    /// Seconds until the rats appear (`m_fRatSwarmSpawnDelay`), then seconds alive.
    pub spawn_delay: f32,
    pub age: f32,
    pub rats: u16,
    pub target: Option<PlayerId>,
    /// Seconds on the current target (before `m_fInitialDelayBeforeDealingDamage`), and to the next bite.
    pub attack_time: f32,
    pub bite_timer: f32,
    /// Corpse being eaten and seconds into eating it.
    pub eating: Option<PlayerId>,
    pub eat_time: f32,
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
