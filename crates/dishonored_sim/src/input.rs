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
    pub const USE: u16 = 1 << 9;
    pub const RELOAD: u16 = 1 << 10;
    /// Pick option 1..4 of an open dialog choice (`DisSeqAct_DialogScriptedChoice`, conversations).
    pub const CHOICE_1: u16 = 1 << 11;
    pub const CHOICE_2: u16 = 1 << 12;
    pub const CHOICE_3: u16 = 1 << 13;
    pub const CHOICE_4: u16 = 1 << 14;
    pub const CHOICES: [u16; 4] = [CHOICE_1, CHOICE_2, CHOICE_3, CHOICE_4];
}

/// Left-hand selection (Dishonored binds powers and gadgets to the left hand).
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
}

impl LeftHand {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Blink,
            1 => Self::Windblast,
            2 => Self::DarkVision,
            3 => Self::Pistol,
            4 => Self::Crossbow,
            _ => Self::Empty,
        }
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
}

impl PlayerInput {
    pub fn pressed(&self, b: u16) -> bool {
        self.buttons & b != 0
    }
    pub fn axes(&self) -> (f32, f32) {
        (self.forward as f32 / 127.0, self.right as f32 / 127.0)
    }
}
