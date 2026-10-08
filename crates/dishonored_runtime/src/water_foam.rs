//! The island sea of `Lighthouse_FX.Materials.water_Foam_MAT` (L_Isl_LowChaos_P /
//! L_Isl_HighChaos_P: `island_water_near.Water_Island_{LC,HC}_{near,far}_Inst`), shaded the way
//! the material's compiled base passes do (`water_foam.wgsl`; the maths is in
//! research/dishonored/RENDERING.md "Island sea").
//!
//! Shader maps (`shader_maps ... water_Foam`): 37072 (no switch), 37073 / 37074 (`Enable_Foam`
//! with `F_Foam_Mask_Chanel` G / R), 37075 / 37076 (`Enable_Foam` + `Enable_Rain`, G / R). The
//! low-chaos instances have foam, the high-chaos ones foam and rain. The material has no diffuse
//! texture: the port drew it as an untextured translucent StandardMaterial in the fallback colour
//! (the pink / pale sea of the Isl starts).
//!
//! The reflection is `C_Realtime_reflection_Texture`, the planar reflection render target of the
//! level's `SceneCaptureReflectActor` (`reflect.rs`); without one (reflections off) the level's
//! reflection cube stands in (U-764, as for the river water, `water.rs`).

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::{MaterialParams, TextureId};

/// The base material this module renders.
pub const WATER_FOAM_MAT: &str = "Lighthouse_FX.Materials.water_Foam_MAT";

/// The textures its base passes sample (kept by name by the importer).
pub const TEXTURES: [&str; 5] = ["W_Waves_normalmap", "W_Waves_normalrain", "F_Foam_Mask", "F_Foam_Distortion", "F_Foam_Texture"];

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct WaterFoamUniform {
    /// rgb: Water_color.rgb x a; w: UE3 radiance to Bevy light units.
    pub water: Vec4,
    /// rgb: Foam_Brightness.rgb x a; w: 1 = Enable_Foam.
    pub foam_color: Vec4,
    /// (W_Waves_Speed, W_Waves_1_tilling, W_Waves_2_tilling, Wavy_UV).
    pub waves: Vec4,
    /// (W_Distorsion_intensity, Wavy_Cube, CubeMap_intensity, Opacity_Depth).
    pub refl: Vec4,
    /// (F_Foam_Distortion_Tilling, F_Foam_Distortion, F_Foam_Tilling, F_Foam_Speed).
    pub foam: Vec4,
    /// (F_Foam_Distortion_Speed, Translucency, 1 = Enable_Rain, 0).
    pub foam2: Vec4,
    /// (Rain_Speed, W_Rain_tilling, Rain_Normal_Intensity, 0).
    pub rain: Vec4,
    /// One-hot `F_Foam_Mask_Chanel` (R without foam).
    pub mask: Vec4,
    /// xyz: `ObjectWorldPositionAndRadius` origin, the instance's world bounds origin (UE3 space);
    /// w: 1 = the planar reflection target is bound.
    pub object: Vec4,
}

fn vector(p: &MaterialParams, name: &str, d: [f32; 4]) -> [f32; 4] {
    p.vectors.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}
fn scalar(p: &MaterialParams, name: &str, d: f32) -> f32 {
    p.scalars.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}

impl WaterFoamUniform {
    /// The uniform of a water_Foam_MAT material (None for other bases); defaults are the base's
    /// (its shader maps' uniform expressions). `object` is the instance's world bounds origin.
    pub fn decode(p: &MaterialParams, object: Vec3, light_unit: f32) -> Option<WaterFoamUniform> {
        if !p.base_material.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(WATER_FOAM_MAT)) {
            return None;
        }
        let on = |n: &str| if p.switch(n) == Some(true) { 1.0 } else { 0.0 };
        let wc = vector(p, "Water_color", [0.3062337, 0.375, 0.28654665, 0.1]);
        let fb = vector(p, "Foam_Brightness", [1.0; 4]);
        let foam = on("Enable_Foam");
        let ch = if foam > 0.0 { p.mask_channel("F_Foam_Mask_Chanel").unwrap_or(0).min(3) as usize } else { 0 };
        let mut mask = Vec4::ZERO;
        mask[ch] = 1.0;
        Some(WaterFoamUniform {
            water: Vec4::new(wc[0] * wc[3], wc[1] * wc[3], wc[2] * wc[3], light_unit),
            foam_color: Vec4::new(fb[0] * fb[3], fb[1] * fb[3], fb[2] * fb[3], foam),
            waves: Vec4::new(scalar(p, "W_Waves_Speed", 0.25), scalar(p, "W_Waves_1_tilling", 3.0), scalar(p, "W_Waves_2_tilling", 2.5), scalar(p, "Wavy_UV", 0.01)),
            refl: Vec4::new(scalar(p, "W_Distorsion_intensity", 1.0), scalar(p, "Wavy_Cube", 0.001), scalar(p, "CubeMap_intensity", 1.0), scalar(p, "Opacity_Depth", 128.0)),
            foam: Vec4::new(scalar(p, "F_Foam_Distortion_Tilling", 1.0), scalar(p, "F_Foam_Distortion", 0.2), scalar(p, "F_Foam_Tilling", 1.0), scalar(p, "F_Foam_Speed", 0.0)),
            foam2: Vec4::new(scalar(p, "F_Foam_Distortion_Speed", 0.0), scalar(p, "Translucency", 0.8), on("Enable_Rain"), 0.0),
            rain: Vec4::new(scalar(p, "Rain_Speed", 1.0), scalar(p, "W_Rain_tilling", 3.0), scalar(p, "Rain_Normal_Intensity", 0.3), 0.0),
            mask,
            object: object.extend(0.0),
        })
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct WaterFoamMaterial {
    #[uniform(0)]
    pub params: WaterFoamUniform,
    #[texture(1)]
    #[sampler(2)]
    pub waves: Option<Handle<Image>>,
    #[texture(3)]
    pub rain: Option<Handle<Image>>,
    #[texture(4)]
    pub mask: Option<Handle<Image>>,
    #[texture(5)]
    pub foam_distortion: Option<Handle<Image>>,
    #[texture(6)]
    pub foam: Option<Handle<Image>>,
    /// The level's reflection cube (`map_render::reflection_cube`), standing in for the planar
    /// reflection.
    #[texture(7, dimension = "cube")]
    #[sampler(8)]
    pub scene: Option<Handle<Image>>,
    /// `C_Realtime_reflection_Texture`: the render target of the level's `SceneCaptureReflectActor`
    /// (`reflect.rs`), read at the screen position; replaces `scene` when bound.
    #[texture(9)]
    #[sampler(10)]
    pub reflect: Option<Handle<Image>>,
}

impl Material for WaterFoamMaterial {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/water_foam.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// A sea section waiting for its material (spawned with a StandardMaterial by
/// `map_render::spawn_map`, swapped on the next update).
#[derive(Component, Clone)]
pub struct PendingWaterFoam(pub WaterFoamMaterial);

impl PendingWaterFoam {
    pub fn new(p: &MaterialParams, textures: &[(String, TextureId)], images: &[Handle<Image>], object: Vec3, scene: Option<Handle<Image>>, reflect: Option<Handle<Image>>) -> Option<PendingWaterFoam> {
        let mut params = WaterFoamUniform::decode(p, object, crate::map_render::light_unit())?;
        params.object.w = if reflect.is_some() { 1.0 } else { 0.0 };
        let tex = |n: &str| textures.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).and_then(|t| images.get(t.1)).cloned();
        let m = WaterFoamMaterial {
            params,
            waves: tex(TEXTURES[0]),
            rain: tex(TEXTURES[1]),
            mask: tex(TEXTURES[2]),
            foam_distortion: tex(TEXTURES[3]),
            foam: tex(TEXTURES[4]),
            scene,
            reflect,
        };
        if m.waves.is_none() || m.mask.is_none() || (params.foam_color.w > 0.0 && (m.foam.is_none() || m.foam_distortion.is_none())) || (params.foam2.z > 0.0 && m.rain.is_none()) {
            warn!("island sea: textures missing (waves {}, rain {}, mask {}, foam {} / {})", m.waves.is_some(), m.rain.is_some(), m.mask.is_some(), m.foam.is_some(), m.foam_distortion.is_some());
        }
        Some(PendingWaterFoam(m))
    }
}

pub struct WaterFoamPlugin;

impl Plugin for WaterFoamPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "water_foam.wgsl");
        app.add_plugins(MaterialPlugin::<WaterFoamMaterial>::default()).add_systems(Update, apply_pending);
    }
}

fn apply_pending(mut commands: Commands, q: Query<(Entity, &PendingWaterFoam)>, mut mats: ResMut<Assets<WaterFoamMaterial>>) {
    for (e, p) in &q {
        let h = mats.add(p.0.clone());
        commands
            .entity(e)
            .remove::<(PendingWaterFoam, MeshMaterial3d<StandardMaterial>, bevy::pbr::Lightmap)>()
            .insert((MeshMaterial3d(h), bevy::light::NotShadowReceiver, bevy::light::NotShadowCaster));
    }
}
