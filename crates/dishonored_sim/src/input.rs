//! Player input for one simulation tick. Compact and network-friendly.

use serde::{Deserialize, Serialize};

pub mod buttons {
    pub const JUMP: u16 = 1 << 0;
    /// Toggle sneak (crouch) on press.
    pub const CROUCH: u16 = 1 << 1;
    pub const SPRINT: u16 = 1 << 2;
    pub const WALK: u16 = 1 << 3;
    /// Right hand: sword attack.
    pub const PRIMARY: u16 = 1 << 4;
    /// Left hand: selected power or ranged weapon (hold to aim/charge, release to use).
    pub const SECONDARY: u16 = 1 << 5;
    pub const BLOCK: u16 = 1 << 6;
    pub const LEAN_LEFT: u16 = 1 << 7;
    pub const LEAN_RIGHT: u16 = 1 << 8;
    /// Use / interact (pick up, talk, choke: `m_bUseButton` / `m_bChokeButton`, carry, drop).
    pub const USE: u16 = 1 << 9;
    pub const RELOAD: u16 = 1 << 10;
    /// Pick option 1..4 of an open dialog choice (`DisSeqAct_DialogScriptedChoice`, conversations).
    pub const CHOICE_1: u16 = 1 << 11;
    pub const CHOICE_2: u16 = 1 << 12;
    pub const CHOICE_3: u16 = 1 << 13;
    pub const CHOICE_4: u16 = 1 << 14;
    pub const CHOICES: [u16; 4] = [CHOICE_1, CHOICE_2, CHOICE_3, CHOICE_4];
    /// The quick-access wheel is open for this player (`DisGFxMoviePlayerPowerWheel` mode 1,
    /// 0x00BE4280). In a single-human world it slows the world to `m_fBendTimeFactor`
    /// ([`crate::powers::WHEEL_TIME_FACTOR`]); with other human players it is ignored.
    pub const WHEEL: u16 = 1 << 15;
}

/// One-shot item actions (`Dis_ConsumeElixir`, the quick-use keys), carried in
/// [`PlayerInput::action`] on the tick they are pressed.
pub mod actions {
    pub const NONE: u8 = 0;
    pub const DRINK_HEALTH: u8 = 1;
    pub const DRINK_MANA: u8 = 2;
    /// The player opened the journal (`Dis_ToggleJournal` 0x009EE550 -> 0x00AA3290 ->
    /// 0x00BD7940): the movie's start (0x00BD11E0) fires `DisSeqEvent_JournalViewed`.
    pub const OPEN_JOURNAL: u8 = 3;
}

/// Left-hand selection (Dishonored binds powers and gadgets to the left hand; the
/// quick-selection wheel, `eDisUISelectionType` / `EDisPowerWheelItem`, picks one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
#[repr(u8)]
pub enum LeftHand {
    #[default]
    Blink = 0,
    Windblast = 1,
    DarkVision = 2,
    Pistol = 3,
    Crossbow = 4,
    Empty = 5,
    Possession = 6,
    BendTime = 7,
    DevouringSwarm = 8,
    /// The crossbow loaded with sleep darts / incendiary bolts (`DPWI_SleepDart` / `DPWI_FlareBolt`).
    SleepDart = 9,
    IncendiaryBolt = 10,
    Grenade = 11,
    StickyGrenade = 12,
    SpringRazor = 13,
}

impl LeftHand {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Blink,
            1 => Self::Windblast,
            2 => Self::DarkVision,
            3 => Self::Pistol,
            4 => Self::Crossbow,
            6 => Self::Possession,
            7 => Self::BendTime,
            8 => Self::DevouringSwarm,
            9 => Self::SleepDart,
            10 => Self::IncendiaryBolt,
            11 => Self::Grenade,
            12 => Self::StickyGrenade,
            13 => Self::SpringRazor,
            _ => Self::Empty,
        }
    }
    /// The crossbow, whatever bolt it is loaded with.
    pub fn is_crossbow(self) -> bool {
        matches!(self, Self::Crossbow | Self::SleepDart | Self::IncendiaryBolt)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct PlayerInput {
    /// Tick this input is intended for.
    pub tick: u64,
    /// -127..127 each.
    pub forward: i8,
    pub right: i8,
    /// View yaw/pitch in degrees (Dishonored convention: yaw about +Z, 0 = +X).
    pub yaw: f32,
    pub pitch: f32,
    pub buttons: u16,
    pub left_hand: LeftHand,
    /// [`actions`] pressed this tick.
    #[serde(default)]
    pub action: u8,
    /// NPC combat move the brain is performing this tick ([`npc_moves`]): which item context
    /// (`DisItemContext_NPCAttack*`, dodges, pistol) drives the input, so presentation can pick
    /// the matching animation. Always 0 for players.
    #[serde(default)]
    pub npc_move: u8,
    /// NPC locomotion speed (uu/s) the brain requests: the authored speed of the locomotion
    /// loop its stance modifier and speed index select, times the modifier's speed multiplier
    /// (`ArkComponentLocomotionConfig`). 0: the player speeds (and always for players).
    #[serde(default)]
    pub npc_speed: f32,
}

/// NPC combat moves carried in [`PlayerInput::npc_move`] (`dishonored_ai::AttackKind`).
pub mod npc_moves {
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

impl PlayerInput {
    pub fn pressed(&self, b: u16) -> bool {
        self.buttons & b != 0
    }
    pub fn axes(&self) -> (f32, f32) {
        (self.forward as f32 / 127.0, self.right as f32 / 127.0)
    }
}
