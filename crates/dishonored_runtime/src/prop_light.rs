//! Doors and usable objects lit like the original's light environment (`prop_light.wgsl`).
//!
//! The original lights a placed `DisSkeletalBreakable` with its `DynamicLightEnvironmentComponent`:
//! the static lights and the precomputed light volume around it are composited into one
//! directional light and an upper / lower sky light (`dishonored_assets::light_env`), drawn in the
//! base pass `TBasePassPixelShader<FDirectionalLightLightMapPolicy, SkyLight>` (generic_PMAT on
//! the skinned vertex factory, e.g. 15153 for `Doors.door_rich_01_INST`):
//!
//!   colour = (saturate(N.L) x diffuse + S x pow(saturate(R.L), SP) x (SP + 8) / (8 pi)) x LightColor
//!          + diffuse x (UpperSkyColor x ((1 + N.Z) / 2)^2 + LowerSkyColor x ((1 - N.Z) / 2)^2)
//!
//! (each term 0 below 1e-6; R = reflect(-V, N); S = the specular texture's masked channels). The
//! dominant and toggleable lights stay Bevy lights; Bevy's ambient and environment light, which
//! the original does not have, are removed. Before, the props had Bevy's placeholder ambient and
//! the reflection cube's Fresnel specular (the bluish band on the street door's seam).

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::{LightEnvironment, MaterialParams};

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct PropLightUniform {
    /// xyz: unit direction towards the directional light (Bevy world space).
    pub direction: Vec4,
    /// rgb: the directional light's colour (Bevy light units), w: 1 = light environment on.
    pub color: Vec4,
    pub upper_sky: Vec4,
    pub lower_sky: Vec4,
    /// x: specular power SP, y: 1 = specular texture on, z: 1 = one masked channel (`spec_mask`).
    pub spec: Vec4,
    /// rgb: the specular colour factor; w unused.
    pub spec_color: Vec4,
    /// One-hot mask channel of the specular texture, or (1, 1, 1, 0) for RGB.
    pub spec_mask: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct PropLight {
    #[uniform(100)]
    pub params: PropLightUniform,
    #[texture(101)]
    #[sampler(102)]
    pub specular: Option<Handle<Image>>,
}

impl MaterialExtension for PropLight {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/prop_light.wgsl".into()
    }
}

pub type PropMaterial = ExtendedMaterial<StandardMaterial, PropLight>;

fn scalar(p: &MaterialParams, name: &str) -> Option<f32> {
    p.scalars.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1)
}
fn vector(p: &MaterialParams, name: &str) -> Option<[f32; 4]> {
    p.vectors.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1)
}

/// The specular terms of a material as the light environment's base pass reads them: (spec,
/// spec_color, spec_mask) uniforms and the specular texture.
pub(crate) fn specular_terms(p: Option<&MaterialParams>, textures: &[Handle<Image>]) -> (Vec4, Vec4, Vec4, Option<Handle<Image>>) {
    let specular = p.and_then(|p| p.specular).filter(|_| p.is_some_and(|p| p.switch("S_Enable_Specular") != Some(false))).and_then(|t| textures.get(t)).cloned();
    let sp = p.and_then(|p| scalar(p, "SP_Specular_Power")).unwrap_or(15.0);
    // `S_Enable_Specular_color` multiplies by the colour (rgb x a, the Arkane colour convention).
    let sc = p.filter(|p| p.switch("S_Enable_Specular_color") == Some(true)).and_then(|p| vector(p, "S_Specular_color")).map(|v| Vec4::new(v[0] * v[3], v[1] * v[3], v[2] * v[3], 0.0)).unwrap_or(Vec4::new(1.0, 1.0, 1.0, 0.0));
    let channel = p.and_then(|p| p.mask_channel("S_Specular_Mask_Channel")).filter(|ch| *ch < 4);
    let mask = match channel {
        Some(ch) => {
            let mut m = Vec4::ZERO;
            m[ch as usize] = 1.0;
            m
        }
        None => Vec4::new(1.0, 1.0, 1.0, 0.0),
    };
    (Vec4::new(sp, if specular.is_some() { 1.0 } else { 0.0 }, if channel.is_some() { 1.0 } else { 0.0 }, 0.0), sc, mask, specular)
}

/// The extension for one material of a prop with light environment `env`; `unit` converts UE3
/// light to Bevy units (`map_render::light_unit`).
pub fn prop_light(env: &LightEnvironment, p: Option<&MaterialParams>, textures: &[Handle<Image>], unit: f32) -> PropLight {
    let dir = crate::coords::dir(env.direction).normalize_or_zero();
    let c = |v: [f32; 3]| Vec4::new(v[0] * unit, v[1] * unit, v[2] * unit, 1.0);
    let (spec, spec_color, spec_mask, specular) = specular_terms(p, textures);
    PropLight {
        params: PropLightUniform { direction: dir.extend(0.0), color: c(env.color), upper_sky: c(env.upper_sky), lower_sky: c(env.lower_sky), spec, spec_color, spec_mask },
        specular,
    }
}

pub struct PropLightPlugin;

impl Plugin for PropLightPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "prop_light.wgsl");
        app.add_plugins(MaterialPlugin::<PropMaterial>::default());
    }
}
