//! The static lighting a dynamic object's light environment reads: the level's static light
//! components and its precomputed light volumes (`dishonored_assets::light_env` decodes them,
//! the runtime evaluates them for characters at the original's update rate).

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// A light as `FDynamicLightEnvironmentState` reads it (`dishonored_assets::light_env::EnvLight`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvLightData {
    /// 0 point, 1 spot, 2 directional, 3 sky.
    pub kind: u8,
    pub inner_deg: f32,
    pub outer_deg: f32,
    pub position: Vec3,
    /// Spot / directional axis (direction the light shines).
    pub direction: Vec3,
    /// `Brightness x FLinearColor(LightColor)`.
    pub intensity: [f32; 3],
    /// Radius (directional lights: `TraceDistance`).
    pub radius: f32,
    pub falloff_exponent: f32,
    /// `LightingChannelContainer` bits.
    pub channels: u32,
    /// `CastShadows && CastStaticShadows`: the visibility is traced.
    pub traced: bool,
    /// `bOnlyAffectSameAndSpecifiedLevels`: the level packages the light affects.
    pub only_levels: Option<Vec<String>>,
}

/// One `FVolumeLightingSample` of a precomputed light volume.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VolumeSampleData {
    pub position: Vec3,
    pub radius: f32,
    /// Indirect direction (theta, phi) and environment direction (theta, phi), one byte each.
    pub directions: [u8; 4],
    /// RGBE `FColor`s in memory order (B, G, R, exponent).
    pub indirect: [u8; 4],
    pub environment: [u8; 4],
    pub ambient: [u8; 4],
}

/// A level package's precomputed light volume.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LightVolumeData {
    /// The level package (`L_Boyle_Ext_Env`).
    pub level: String,
    pub samples: Vec<VolumeSampleData>,
}

/// Everything a light environment needs of a map: the static lights of every level and their
/// light volumes (the persistent level's first).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LightEnvWorld {
    pub lights: Vec<EnvLightData>,
    pub volumes: Vec<LightVolumeData>,
}
