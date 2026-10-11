//! The NPC pawn states and the item contexts the AI requests them through (research/
//! ARCHITECTURE.md, migration step 6).
//!
//! - **Requests** ([`Request`]): what an NPC's brain asks of its pawn in a tick: the item context
//!   it runs (`DisItemContext_NPCAttack*`, the dodges, the pistol, the special classes', [`context`]),
//!   a behaviour's GenericAction chain, and the locomotion's speed. The host hands them to the step
//!   in [`WorldState::requests`] (`dishonored_ai::Crowd::requests`), as it hands the crosshair's
//!   targets; they are not part of the state.

use crate::state::{PlayerId, WorldState};
use serde::{Deserialize, Serialize};

/// The item contexts an NPC's brain runs (`DisItemContext_NPC*`), by the codes of the attack
/// patterns (`dishonored_ir::ai::MeleeContextConfig::kind`, `SpecialAttack::kind`). 0: none.
pub type ContextKind = u8;

/// [`ContextKind`] values.
pub mod context {
    pub const NONE: u8 = 0;
    /// `DisItemContext_NPCAttackShort` (`DisNPCAnim_AttackShort`).
    pub const ATTACK_SHORT: u8 = 1;
    /// `DisItemContext_NPCAttackStepBack`.
    pub const ATTACK_STEP_BACK: u8 = 2;
    /// `DisItemContext_NPCAttackMedium`.
    pub const ATTACK_MEDIUM: u8 = 3;
    /// `DisItemContext_NPCAttackLong` (`DisNPCAnim_AttackLong`).
    pub const ATTACK_LONG: u8 = 4;
    /// `DisItemContext_NPCJumpAttack` (`DisNPCAnim_JumpAttack`).
    pub const JUMP_ATTACK: u8 = 5;
    /// `DisItemContext_NPCSideStep` (dodge).
    pub const SIDE_STEP: u8 = 6;
    /// `DisItemContext_NPCBackStep` (dodge).
    pub const BACK_STEP: u8 = 7;
    /// `DisAISubStateFirePistol` aiming / firing (`DisItemContext_NPCFireGun`).
    pub const FIRE_PISTOL: u8 = 8;
    /// Rat swarm attacking (rat pawns).
    pub const RAT_ATTACK: u8 = 9;
    /// Rat swarm eating a corpse.
    pub const RAT_EAT: u8 = 10;
    /// The special NPC classes' contexts (research/dishonored/systems/ai.md F6).
    pub use dishonored_ir::ai::special_moves::*;
}

/// What an NPC's brain asks of its pawn this tick (the original's AI asks the item contexts, the
/// pawn's state machine and the locomotion component; the port's brain produces this beside the
/// pawn's [`crate::input::PlayerInput`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Request {
    /// The item context the brain runs ([`context`]): held while it runs.
    pub context: ContextKind,
    /// A behaviour's GenericAction chain (`eDisNPCAnimations`) and the serial of the request
    /// (`dishonored_ai::AnimRequest`): a new serial starts it.
    pub play_anim: Option<(u8, u32)>,
    /// The speed the brain asks of the locomotion (uu/s): the authored speed of the loop its
    /// stance modifier and speed index select, times the modifier's `m_fSpeedMultiplier`
    /// (`ArkComponentLocomotionConfig`); 0 leaves the pawn's own speeds.
    pub loco_speed: f32,
}

/// The request of pawn `id` this step (none: the default).
pub fn request(state: &WorldState, id: PlayerId) -> Request {
    state.requests.iter().find(|(p, _)| *p == id).map(|(_, r)| *r).unwrap_or_default()
}
