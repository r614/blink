//! River, canal and pool water of `Materials_ref.water_materials.river_main_PMAT`, shaded the way
//! the material's compiled base pass does (`ark_water.wgsl`; the maths is in
//! research/dishonored/RENDERING.md "Water (river_main_PMAT)").
//!
//! Decoded from RefShaderCache-PC-D3D-SM3.upk, shader map of `boyle_ext_MS.Water_boyle_ext_INST03`
//! (the Boyle river's surface; switches Di_Enable_Distorsion, D_Enable_Opacity_over_Depth,
//! D_Enable_Diffuse_on_top, C_Enable_Realtime_Reflection, D_Enable_Diffuse_Distorsion,
//! F_Enable_Fake_Fog; masks W_Distorsion_Mask_Chanel = G, D_Diffuse_Mask_Chanel = R): base pass
//! 11099 (`FNoLightMapPolicy`) and 11093 (`FDirectionalLightMapTexturePolicy`), and of
//! `Water_boyle_ext_under_01_INST` (the river seen from under water; no realtime reflection, no
//! diffuse on top): 10608, where the reflection is the cubemap.
//!
//! Before, the material drew as Bevy's lit `StandardMaterial` with its `D_diffuse_texture`
//! (`moss_01.moss_02_d`) as the colour: green moss over the whole surface, hidden on Boyle by the
//! river's night lightmap and bright green under the Playground's work lights.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::{MaterialParams, TextureId};

/// The base material this module renders.
pub const RIVER_MAIN_PMAT: &str = "Materials_ref.water_materials.river_main_PMAT";

/// Textures of the base pass, as the importer names them (`MaterialAsset::texture_params`).
pub const TEXTURES: [&str; 3] = ["W_Waves_normalmap", "D_diffuse_mask", "D_diffuse_texture"];

/// Switches of river_main_PMAT whose off state was not decoded (every Boyle water instance has
/// them on). Their decoded on-state terms are used.
pub const UNDECODED_OFF: [&str; 2] = ["D_Enable_Opacity_over_Depth", "Di_Enable_Distorsion"];

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct WaterUniform {
    /// Waves 1: (W_Waves_1_tilling, angle = W1_Waves1_orientation x -0.0175, W1_Waves1_Speed, 0).
    /// Its UV: rotate (tiling x uv - 0.5) by the angle, add
    /// (frac(speed t x -0.13), frac(speed t x 0.075)), add 0.5.
    pub w1: Vec4,
    /// Waves 2: the same with W2_* and the pan (frac(speed t x 0.075), frac(speed t x -0.13)).
    pub w2: Vec4,
    /// (W1_Distorsion1_intensity1, D_diffuse_Distorsion_strength, D_Diffuse_Tilling,
    /// D_Diffuse_Mask_Tilling).
    pub dist: Vec4,
    /// (F_Fog_StartDistance, F_Fog_opacity, F_Fog_Attenuation, 1 = F_Enable_Fake_Fog).
    pub fog: Vec4,
    /// (O_Translucent_Depth, O_Depth_max, O_Depth_min, O_Opacity_Intensity).
    pub opacity: Vec4,
    /// rgb: C_Cubemap_color.rgb x a; w: 1 = D_Enable_Diffuse_on_top.
    pub cube: Vec4,
    /// rgb: F_Fog_color.rgb; w: 1 = C_Enable_Realtime_Reflection (drawn with the cubemap, below).
    pub fog_color: Vec4,
    /// rgb: W_Water_Color.rgb; w: 1 = D_Enable_Diffuse_Distorsion.
    pub water_color: Vec4,
    /// One-hot channel of D_Diffuse_Mask_Chanel.
    pub mask: Vec4,
    /// rgb: AmbientColorAndSkyFactor.rgb in Bevy light units (0 in game, `decals::BASE_PASS_AMBIENT`);
    /// w: UE3 radiance to Bevy light units (`map_render::light_unit`).
    pub ambient: Vec4,
    /// x: 1 = the planar reflection render target (`ArkWater::reflect`) is bound and
    /// C_Enable_Realtime_Reflection is on: the reflection reads it instead of the cube.
    pub planar: Vec4,
}

fn vector(p: &MaterialParams, name: &str, d: [f32; 4]) -> [f32; 4] {
    p.vectors.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}
fn scalar(p: &MaterialParams, name: &str, d: f32) -> f32 {
    p.scalars.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}

impl WaterUniform {
    /// The uniform of a river_main_PMAT material (None for other bases). Defaults are the base
    /// material's (the shader map's parameter defaults).
    pub fn decode(p: &MaterialParams, light_unit: f32) -> Option<WaterUniform> {
        if !p.base_material.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(RIVER_MAIN_PMAT)) {
            return None;
        }
        let on = |n: &str| if p.switch(n) == Some(true) { 1.0 } else { 0.0 };
        let angle = |n: &str| scalar(p, n, 0.0) * -0.0175;
        let cc = vector(p, "C_Cubemap_color", [1.0; 4]);
        let fc = vector(p, "F_Fog_color", [1.0; 4]);
        let wc = vector(p, "W_Water_Color", [0.0, 0.0, 0.0, 1.0]);
        let ch = p.mask_channel("D_Diffuse_Mask_Chanel").unwrap_or(0).min(3) as usize;
        let mut mask = Vec4::ZERO;
        mask[ch] = 1.0;
        let a = Vec3::from(crate::decals::BASE_PASS_AMBIENT) * light_unit;
        Some(WaterUniform {
            w1: Vec4::new(scalar(p, "W_Waves_1_tilling", 0.71), angle("W1_Waves1_orientation"), scalar(p, "W1_Waves1_Speed", 1.0), 0.0),
            w2: Vec4::new(scalar(p, "W_Waves_2_tilling", 0.67), angle("W2_Waves2_orientation"), scalar(p, "W2_Waves2_Speed", 1.0), 0.0),
            dist: Vec4::new(scalar(p, "W1_Distorsion1_intensity1", 1.0), scalar(p, "D_diffuse_Distorsion_strength", 0.01), scalar(p, "D_Diffuse_Tilling", 1.0), scalar(p, "D_Diffuse_Mask_Tilling", 1.0)),
            fog: Vec4::new(scalar(p, "F_Fog_StartDistance", 6000.0).max(1e-3), scalar(p, "F_Fog_opacity", 1.0), scalar(p, "F_Fog_Attenuation", 10.0), on("F_Enable_Fake_Fog")),
            opacity: Vec4::new(scalar(p, "O_Translucent_Depth", 200.0).max(1e-3), scalar(p, "O_Depth_max", 1.0), scalar(p, "O_Depth_min", 0.0), scalar(p, "O_Opacity_Intensity", 0.7)),
            cube: Vec4::new(cc[0] * cc[3], cc[1] * cc[3], cc[2] * cc[3], on("D_Enable_Diffuse_on_top")),
            fog_color: Vec4::new(fc[0], fc[1], fc[2], on("C_Enable_Realtime_Reflection")),
            water_color: Vec4::new(wc[0], wc[1], wc[2], on("D_Enable_Diffuse_Distorsion")),
            mask,
            ambient: a.extend(light_unit),
            planar: Vec4::ZERO,
        })
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct ArkWater {
    #[uniform(100)]
    pub params: WaterUniform,
    /// W_Waves_normalmap.
    #[texture(101)]
    #[sampler(102)]
    pub waves: Option<Handle<Image>>,
    /// D_diffuse_mask.
    #[texture(103)]
    pub diffuse_mask: Option<Handle<Image>>,
    /// D_diffuse_texture.
    #[texture(104)]
    pub diffuse: Option<Handle<Image>>,
    /// The reflection cube: the level's (as the translucent decals' scene reflection,
    /// `map_render::reflection_cube`).
    #[texture(105, dimension = "cube")]
    #[sampler(106)]
    pub scene: Option<Handle<Image>>,
    #[uniform(107)]
    pub lightmap_params: crate::decals::DecalLightmap,
    #[texture(108)]
    #[sampler(109)]
    pub lightmap: Option<Handle<Image>>,
    #[texture(110)]
    pub directional: Option<Handle<Image>>,
    /// `C_Realtime_reflection_Texture`: the render target of the level's
    /// `SceneCaptureReflectActor` (`reflect.rs`), read at the screen position.
    #[texture(111)]
    #[sampler(112)]
    pub reflect: Option<Handle<Image>>,
}

impl MaterialExtension for ArkWater {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/ark_water.wgsl".into()
    }
}

pub type WaterMaterial = ExtendedMaterial<StandardMaterial, ArkWater>;

/// A water instance section waiting for its material (spawned with a StandardMaterial by
/// `map_render::spawn_map`, swapped on the next update).
#[derive(Component, Clone)]
pub struct PendingWater(pub ArkWater);

impl PendingWater {
    /// The water of a section; `lightmap` is the instance's directional lightmap (None: the
    /// dynamic lights only, e.g. a layout map). Missing textures leave their term out.
    pub fn new(p: &MaterialParams, textures: &[(String, TextureId)], images: &[Handle<Image>], scene: Option<Handle<Image>>, lightmap: Option<crate::decals::DecalLightmapRef>, reflect: Option<Handle<Image>>) -> Option<PendingWater> {
        let mut params = WaterUniform::decode(p, crate::map_render::light_unit())?;
        // The planar target replaces the cube only in the permutations that sample it.
        let reflect = reflect.filter(|_| params.fog_color.w > 0.0);
        params.planar.x = if reflect.is_some() { 1.0 } else { 0.0 };
        let off: Vec<&str> = UNDECODED_OFF.iter().copied().filter(|s| p.switch(s) != Some(true)).collect();
        if !off.is_empty() {
            debug!("water: switches off whose terms are not decoded: {}", off.join(", "));
        }
        let tex = |n: &str| textures.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).and_then(|t| images.get(t.1)).cloned();
        let (lightmap_params, lm, dir) = match lightmap {
            Some(l) => (l.uniform, Some(l.color), Some(l.directional)),
            None => (crate::decals::DecalLightmap::default(), None, None),
        };
        Some(PendingWater(ArkWater { params, waves: tex(TEXTURES[0]), diffuse_mask: tex(TEXTURES[1]), diffuse: tex(TEXTURES[2]), scene, lightmap_params, lightmap: lm, directional: dir, reflect }))
    }
}

pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "ark_water.wgsl");
        app.add_plugins(MaterialPlugin::<WaterMaterial>::default()).add_systems(Update, apply_pending_water);
    }
}

fn apply_pending_water(
    mut commands: Commands,
    q: Query<(Entity, &PendingWater, &MeshMaterial3d<StandardMaterial>)>,
    std_mats: Res<Assets<StandardMaterial>>,
    mut mats: ResMut<Assets<WaterMaterial>>,
    mut cache: Local<std::collections::HashMap<(AssetId<StandardMaterial>, Option<AssetId<Image>>, [u32; 4], Option<AssetId<Image>>), Handle<WaterMaterial>>>,
) {
    for (e, p, m) in &q {
        let Some(base) = std_mats.get(&m.0) else { continue };
        let rect = p.0.lightmap_params.rect.to_array().map(f32::to_bits);
        let h = cache
            .entry((m.0.id(), p.0.lightmap.as_ref().map(|h| h.id()), rect, p.0.reflect.as_ref().map(|h| h.id())))
            .or_insert_with(|| {
                // The colour, opacity and normal are the shader's own: the StandardMaterial's
                // textures (the moss as the colour, the waves as a normal map) are dropped.
                let mut base = base.clone();
                base.base_color = Color::WHITE;
                base.base_color_texture = None;
                base.normal_map_texture = None;
                base.emissive = LinearRgba::BLACK;
                base.emissive_texture = None;
                base.specular_tint_texture = None;
                base.reflectance = 0.0;
                base.alpha_mode = AlphaMode::Blend;
                mats.add(WaterMaterial { base, extension: p.0.clone() })
            })
            .clone();
        commands
            .entity(e)
            .remove::<(PendingWater, MeshMaterial3d<StandardMaterial>, bevy::pbr::Lightmap)>()
            .insert((MeshMaterial3d(h), bevy::light::NotShadowReceiver, bevy::light::NotShadowCaster));
    }
}
