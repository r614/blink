//! Fog and post-processing from the level: Arkane `DisFog` layers and
//! `WorldInfo.m_ArkDefaultPpSettings` (ArkPpConfig), rendered by the decoded original passes in
//! [`crate::ark_post`] (research/LOG.md "DisFog and ArkPp post").
//!
//! With post on, Bevy's tonemapping is off: the ArkPp colour pipeline clamps the scene at 1,
//! applies `2^m_Exposure`, colour balance and the display gamma itself. The original's bloom
//! comes from per-material bloom parts (`B_Enable_Bloom`, `TBloomPartMesh*` shaders), not from
//! a scene threshold, and is not rendered (U-323).

use crate::ark_post::{ArkPost, ArkPostSource};
use crate::map_render::RenderFeatures;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use bevy::render::view::ColorGrading;
use dishonored_ir::{DishonoredMap, FogSettings};

#[derive(Resource, Default, Clone)]
pub struct FogLayers(pub Vec<FogSettings>);

fn tonemapping() -> Tonemapping {
    match std::env::var("DISHONORED_TONEMAP").as_deref() {
        Ok("reinhard") => Tonemapping::Reinhard,
        Ok("aces") => Tonemapping::AcesFitted,
        Ok("agx") => Tonemapping::AgX,
        Ok("tony") => Tonemapping::TonyMcMapface,
        Ok("blender") => Tonemapping::BlenderFilmic,
        Ok("reinhard_luminance") => Tonemapping::ReinhardLuminance,
        _ => Tonemapping::TonyMcMapface,
    }
}

/// Add the level's fog and post to the camera. Returns the fog layers resource.
pub fn setup_camera(map: &DishonoredMap, camera: &mut EntityCommands, features: &RenderFeatures) -> FogLayers {
    let fog_on = features.fog && !map.environment.fog.is_empty();
    let post_on = features.post;
    camera.remove::<(Bloom, bevy::post_process::dof::DepthOfField, bevy::pbr::DistanceFog)>();
    camera.insert(ColorGrading::default());
    if fog_on || post_on {
        let src = ArkPostSource { fog: map.environment.fog.clone(), post: map.environment.post.clone(), fog_on, post_on, display_gamma: 2.2, ..Default::default() };
        let u = src.uniform(0.0);
        camera.insert((src, u));
    } else {
        camera.remove::<(ArkPostSource, ArkPost)>();
    }
    // The ArkPp pass writes display values; without it Bevy's own tonemapper stands in.
    camera.insert(if post_on { Tonemapping::None } else { tonemapping() });
    FogLayers(if fog_on { map.environment.fog.clone() } else { Vec::new() })
}

/// Kept for the systems that schedule it; fog is evaluated per pixel by the ArkPp pass.
pub fn update_fog() {}
