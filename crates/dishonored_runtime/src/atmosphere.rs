//! Fog and post-processing from the level: Arkane `DisFog` layers -> Bevy `DistanceFog`,
//! `WorldInfo.m_ArkDefaultPpSettings` (ArkPpConfig) -> tonemapping, colour grading, bloom, DOF.
//!
//! APPROXIMATIONS (see research/LOG.md "Atmosphere and post"):
//! - DisFog is a stack of height-bounded linear fog layers; Bevy has one distance fog per
//!   camera, so the layer containing the camera with the nearest `NearPlane` is used each frame
//!   (falling back to the tallest layer) and the height bound only selects the layer.
//! - ArkPp HDR `m_Exposure` is read as a tonemapping key against 0.18 middle grey (EV offset
//!   log2(key / 0.18)); gamma/contrast adjustments are not mapped (see setup_camera);
//!   colour balance (Cyan-Red, Magenta-Green, Yellow-Blue) midtones -> white-balance
//!   temperature/tint. Bloom tint and film grain have no Bevy equivalent and are dropped.

use crate::coords;
use crate::map_render::RenderFeatures;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::post_process::bloom::Bloom;
use bevy::post_process::dof::{DepthOfField, DepthOfFieldMode};
use bevy::prelude::*;
use bevy::render::view::{ColorGrading, ColorGradingGlobal};
use dishonored_ir::{DishonoredMap, FogSettings};

#[derive(Resource, Default, Clone)]
pub struct FogLayers(pub Vec<FogSettings>);

fn post_value(map: &DishonoredMap, key: &str) -> Option<f32> {
    map.environment.post.as_ref()?.values.iter().find(|(k, _)| k.ends_with(key)).map(|v| v.1)
}
fn post_vector(map: &DishonoredMap, key: &str) -> Option<[f32; 4]> {
    map.environment.post.as_ref()?.vectors.iter().find(|(k, _)| k.ends_with(key)).map(|v| v.1)
}

/// Extra exposure (EV) on top of the ArkPp key, chosen by eye on Boyle Ext until original
/// captures exist (U-011); override with `DISHONORED_EV_BIAS`.
pub const CALIBRATION_EV: f32 = 0.0;

fn calibration_ev() -> f32 {
    std::env::var("DISHONORED_EV_BIAS").ok().and_then(|s| s.parse().ok()).unwrap_or(CALIBRATION_EV)
}

fn tonemapping() -> Tonemapping {
    match std::env::var("DISHONORED_TONEMAP").as_deref() {
        Ok("none") => Tonemapping::None,
        Ok("reinhard") => Tonemapping::Reinhard,
        Ok("aces") => Tonemapping::AcesFitted,
        Ok("agx") => Tonemapping::AgX,
        Ok("tony") => Tonemapping::TonyMcMapface,
        Ok("blender") => Tonemapping::BlenderFilmic,
        Ok("reinhard_luminance") => Tonemapping::ReinhardLuminance,
        _ => Tonemapping::TonyMcMapface,
    }
}

/// Add fog and post components to the camera. Returns the fog layers resource.
pub fn setup_camera(map: &DishonoredMap, camera: &mut EntityCommands, features: &RenderFeatures) -> FogLayers {
    if features.post {
        let key = post_value(map, "m_Exposure").unwrap_or(0.18).max(0.01);
        let mid = post_vector(map, "m_CrMgYbMidTones").unwrap_or([0.0; 4]);
        // m_GammaAdjustment (0.95) and m_GimpContrast (0.05) are NOT mapped: Bevy's per-section
        // gamma/contrast with those values crushed the night shadows to black
        // (harness_out/cal/*), so they wait for reference captures.
        camera.insert((
            tonemapping(),
            ColorGrading {
                global: ColorGradingGlobal {
                    exposure: (key / 0.18).log2() + calibration_ev(),
                    temperature: -0.1 * mid[2],
                    tint: -0.05 * mid[1],
                    ..default()
                },
                ..default()
            },
            Bloom { intensity: 0.4 * post_value(map, "m_Scale").unwrap_or(0.375), ..Bloom::NATURAL },
        ));
        // ArkPp DOF is far-only (`m_FocusDistance` 5500 uu, `m_FarBlurAmount` 0.2); Bevy's
        // DepthOfField also blurs everything nearer than the focal plane (first-person props,
        // walls at arm's length), which looks far worse than no DOF, so it is opt-in.
        if std::env::args().any(|a| a == "--dof") {
            if let Some(focus) = post_value(map, "m_FocusDistance") {
                let amount = post_value(map, "m_FarBlurAmount").unwrap_or(0.0).max(0.01);
                camera.insert(DepthOfField {
                    mode: DepthOfFieldMode::Gaussian,
                    focal_distance: focus * coords::UU_TO_BEVY,
                    aperture_f_stops: 4.0 / amount,
                    max_circle_of_confusion_diameter: 64.0 * amount,
                    ..default()
                });
            }
        }
    }
    if features.fog && !map.environment.fog.is_empty() {
        camera.insert(DistanceFog { color: Color::NONE, falloff: FogFalloff::Linear { start: 1.0, end: 2.0 }, ..default() });
        return FogLayers(map.environment.fog.clone());
    }
    FogLayers::default()
}

/// Pick the DisFog layer for the camera height every frame.
pub fn update_fog(layers: Res<FogLayers>, mut q: Query<(&GlobalTransform, &mut DistanceFog), With<Camera3d>>) {
    if layers.0.is_empty() {
        return;
    }
    for (tf, mut fog) in &mut q {
        let z = tf.translation().y / coords::UU_TO_BEVY;
        let inside = layers.0.iter().filter(|l| z >= l.origin && z <= l.origin + l.height).min_by(|a, b| a.near.total_cmp(&b.near));
        let l = inside.or_else(|| layers.0.iter().max_by(|a, b| a.height.total_cmp(&b.height))).unwrap();
        fog.color = Color::linear_rgba(l.color[0], l.color[1], l.color[2], l.opacity.clamp(0.0, 1.0));
        fog.falloff = FogFalloff::Linear { start: l.near * coords::UU_TO_BEVY, end: l.far.max(l.near + 1.0) * coords::UU_TO_BEVY };
    }
}
