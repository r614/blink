//! Events emitted by a simulation step. Consumed by presentation, networking, scoring,
//! AI perception and the test harness. Never authoritative input to the next step.

use crate::state::PlayerId;
use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum SoundCategory {
    Footstep,
    Sprint,
    Land,
    Blink,
    Crossbow,
    Pistol,
    SwordSwing,
    SwordImpact,
    SwordBlock,
    Windblast,
    BodyFall,
    Mantle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum DamageCause {
    Sword,
    SwordCounter,
    Assassination,
    DropAssassination,
    Pistol,
    PistolHeadshot,
    Crossbow,
    CrossbowHeadshot,
    Windblast,
    Fall,
    World,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SimEvent {
    Jumped { id: PlayerId },
    Landed { id: PlayerId, speed: f32, fall_time: f32 },
    Mantle { id: PlayerId, edge_height: f32 },
    FellOutOfWorld { id: PlayerId },
    BlinkStart { id: PlayerId, from: Vec3, to: Vec3 },
    BlinkEnd { id: PlayerId },
    BlinkFizzle { id: PlayerId },
    SwordSwing { id: PlayerId },
    SwordBlocked { attacker: PlayerId, defender: PlayerId },
    SwordParried { attacker: PlayerId, defender: PlayerId },
    Fired { id: PlayerId, bolt: bool, projectile: u32 },
    Reloaded { id: PlayerId },
    WindblastCast { id: PlayerId },
    DarkVision { id: PlayerId, active: bool },
    Damage { victim: PlayerId, attacker: Option<PlayerId>, amount: f32, cause: DamageCause },
    Death { victim: PlayerId, killer: Option<PlayerId>, cause: DamageCause },
    ProjectileImpact { projectile: u32, point: Vec3, victim: Option<PlayerId> },
    Sound { position: Vec3, radius: f32, category: SoundCategory, source: PlayerId },
}
