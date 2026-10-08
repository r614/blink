//! Translucent decals of `generic_translucent_PMAT` (ground puddles, street splotches, graffiti,
//! confetti), shaded the way the material's compiled shaders do (`ark_translucent.wgsl`; the
//! maths is in research/dishonored/RENDERING.md "Translucent decals").
//!
//! Boyle's ground decals are ordinary static meshes laid a few units over the ground (there are
//! no `DecalActor`s on Boyle) with a translucent material: `ext_boyle_puddles_decals_01` with
//! `towerEmpress_ground.puddles_decals_01_inst`. Drawn as Bevy's lit `StandardMaterial`, they took
//! Bevy's environment specular (Fresnel-bright at the grazing angles of the ground) and the
//! placeholder ambient, and showed as pale patches. The original's base pass for a lightmapped
//! static mesh (`TBasePassPixelShader<FDirectionalLightMapTexturePolicy>`, 9347 for the puddles)
//! lights the diffuse texture plus the scene reflection masked by the cubemap mask's channel with
//! the component's directional lightmap, exactly like the opaque base pass's diffuse term; its
//! lightmap specular is multiplied by `SpecularOverrideParameter.xyz` (0 in game), so there is
//! none. The dominant / toggleable lights add their light passes' `N.L` (and the dominant
//! directional light its distance-field shadow). The result is blended over the ground by the
//! opacity mask's channel clamped to `O_Minimum_Opacity` .. `O_Maximum_Opacity`.
//!
//! An earlier reading of the shader maps took the skinned vertex factory's shader list for the
//! static one (a mesh shader map names its vertex factory after its shaders), concluded that
//! translucent static meshes had no lightmap and drew the decals with the dynamic lights only:
//! nearly black puddles darkened every gravel path (U-561, U-584).

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::{MaterialParams, TextureId};

/// The base material this module renders.
pub const GENERIC_TRANSLUCENT_PMAT: &str = "Materials_ref.main_materials.generic_translucent_PMAT";

/// `AmbientColorAndSkyFactor.rgb` of the translucent base pass: measured in the running game on
/// Boyle Ext as (0, 0, 0, 1) in every base pass drawn (`probe_launch measure psconst
/// AmbientColorAndSkyFactor`, U-562), so the decals are lit by their lightmap and the light
/// passes only.
pub const BASE_PASS_AMBIENT: [f32; 3] = [0.0, 0.0, 0.0];

/// Switches of generic_translucent_PMAT whose terms are not decoded (no Boyle shader map has
/// them; U-563).
pub const UNDECODED: [&str; 2] = ["F_Enable_Fresnel_Mask", "C_Enable_Custom_cubemap"];

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct DecalUniform {
    pub ambient: Vec4,
    pub cube: Vec4,
    pub mask: Vec4,
    pub fresnel: Vec4,
}

fn vector(p: &MaterialParams, name: &str, d: [f32; 4]) -> [f32; 4] {
    p.vectors.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}
fn scalar(p: &MaterialParams, name: &str, d: f32) -> f32 {
    p.scalars.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}

impl DecalUniform {
    /// The uniform of a translucent generic_translucent_PMAT material (None for other bases).
    /// `light_unit` converts UE3 radiance to Bevy light units (`map_render::light_unit`).
    pub fn decode(p: &MaterialParams, light_unit: f32) -> Option<DecalUniform> {
        if p.blend != dishonored_ir::MaterialBlend::Translucent || !p.base_material.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(GENERIC_TRANSLUCENT_PMAT)) {
            return None;
        }
        let on = |n: &str| p.switch(n) == Some(true);
        let premul = |c: [f32; 4]| Vec3::new(c[0] * c[3], c[1] * c[3], c[2] * c[3]);
        let cube_colour = if on("C_Enable_Cubemap_Color") { premul(vector(p, "C_Cubemap_color", [1.0; 4])) } else { Vec3::ONE };
        // Component mask defaults of the base (R) when the instance does not override it.
        let ch = p.mask_channel("C_Cubemap_Mask_Channel").unwrap_or(0).min(3) as usize;
        let mut mask = Vec4::ZERO;
        mask[ch] = 1.0;
        // Every Boyle shader map has F_Invert_Fresnel_result: f = (1 - N.V)^exponent.
        let fresnel = if on("F_Enable_Fresnel") && p.switch("F_Invert_Fresnel_result") != Some(false) {
            premul(vector(p, "F_Fresnel_Color", [1.0, 0.0, 0.100_950_24, 1.0])).extend(scalar(p, "F_Fresnel_falloff_exponent", 4.0).max(1e-3))
        } else {
            Vec4::ZERO
        };
        let a = Vec3::from(BASE_PASS_AMBIENT) * light_unit;
        Some(DecalUniform {
            ambient: a.extend(if on("C_Enable_Cubemap") { 1.0 } else { 0.0 }),
            cube: cube_colour.extend(if on("C_Enable_CubemapMask") { 1.0 } else { 0.0 }),
            mask,
            fresnel,
        })
    }

    /// The diffuse colour factor: `D_Diffuse_color.rgb x a` with `D_Enable_Diffuse_color`, else 1.
    pub fn diffuse_color(p: &MaterialParams) -> Vec3 {
        if p.switch("D_Enable_Diffuse_color") == Some(true) {
            let c = vector(p, "D_Diffuse_color", [1.0; 4]);
            Vec3::new(c[0] * c[3], c[1] * c[3], c[2] * c[3])
        } else {
            Vec3::ONE
        }
    }
}

/// The component's directional lightmap for the decal base pass (`lightmap_ext.wgsl`'s planes).
#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct DecalLightmap {
    /// x: exposure (UE3 lightmap unit to Bevy), y: 1 = lightmapped, z: 1 = the dominant
    /// directional light is shadowed by the component's distance field, w: unused.
    pub params: Vec4,
    /// The component's region of the atlas: (uv_min, uv_max - uv_min); UV1 is mesh-local.
    pub rect: Vec4,
    /// Distance-field shadow (bias, scale, exponent, 0), see `map_render::DF_SHADOW`.
    pub df: Vec4,
}

/// One instance's lightmap: atlas planes and its rect.
#[derive(Clone, Debug)]
pub struct DecalLightmapRef {
    pub color: Handle<Image>,
    pub directional: Handle<Image>,
    pub shadow: Option<Handle<Image>>,
    pub uniform: DecalLightmap,
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct ArkTranslucent {
    #[uniform(100)]
    pub params: DecalUniform,
    #[texture(101)]
    #[sampler(102)]
    pub mask: Option<Handle<Image>>,
    #[texture(103, dimension = "cube")]
    #[sampler(104)]
    pub scene: Option<Handle<Image>>,
    #[uniform(105)]
    pub lightmap_params: DecalLightmap,
    #[texture(106)]
    #[sampler(107)]
    pub lightmap: Option<Handle<Image>>,
    #[texture(108)]
    pub directional: Option<Handle<Image>>,
    #[texture(109)]
    pub shadow: Option<Handle<Image>>,
}

impl MaterialExtension for ArkTranslucent {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/ark_translucent.wgsl".into()
    }
}

pub type DecalMaterial = ExtendedMaterial<StandardMaterial, ArkTranslucent>;

/// A decal instance section waiting for its material (spawned with a StandardMaterial by
/// `map_render::spawn_map`, swapped on the next update).
#[derive(Component, Clone)]
pub struct PendingDecal {
    pub ext: ArkTranslucent,
    pub diffuse_color: Vec3,
}

impl PendingDecal {
    /// The decal of a translucent section; `lightmap` is the instance's (None: lit by the
    /// dynamic lights only, e.g. with `--no-lightmaps`).
    pub fn new(p: &MaterialParams, textures: &[(String, TextureId)], images: &[Handle<Image>], scene: Option<Handle<Image>>, lightmap: Option<DecalLightmapRef>) -> Option<PendingDecal> {
        let params = DecalUniform::decode(p, crate::map_render::light_unit())?;
        let missing: Vec<&str> = UNDECODED.iter().copied().filter(|s| p.switch(s) == Some(true)).collect();
        if !missing.is_empty() {
            debug!("translucent decal: switches not decoded (U-563): {}", missing.join(", "));
        }
        let mask = textures.iter().find(|(k, _)| k.eq_ignore_ascii_case("C_Cubemap_Mask")).and_then(|t| images.get(t.1)).cloned();
        let mut params = params;
        if mask.is_none() {
            params.cube.w = 0.0;
        }
        if scene.is_none() {
            params.ambient.w = 0.0;
        }
        let (lightmap_params, lm, dir, shadow) = match lightmap {
            Some(l) => (l.uniform, Some(l.color), Some(l.directional), l.shadow),
            None => (DecalLightmap::default(), None, None, None),
        };
        Some(PendingDecal { ext: ArkTranslucent { params, mask, scene, lightmap_params, lightmap: lm, directional: dir, shadow }, diffuse_color: DecalUniform::diffuse_color(p) })
    }
}

pub struct DecalPlugin;

impl Plugin for DecalPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "ark_translucent.wgsl");
        app.add_plugins(MaterialPlugin::<DecalMaterial>::default()).add_systems(Update, apply_pending_decals);
    }
}

fn apply_pending_decals(
    mut commands: Commands,
    q: Query<(Entity, &PendingDecal, &MeshMaterial3d<StandardMaterial>)>,
    std_mats: Res<Assets<StandardMaterial>>,
    mut mats: ResMut<Assets<DecalMaterial>>,
    mut cache: Local<std::collections::HashMap<(AssetId<StandardMaterial>, Option<AssetId<Image>>, [u32; 4]), Handle<DecalMaterial>>>,
) {
    for (e, p, m) in &q {
        let Some(base) = std_mats.get(&m.0) else { continue };
        // One material per lightmap region (the section meshes are shared, UV1 is mesh-local).
        let rect = p.ext.lightmap_params.rect.to_array().map(f32::to_bits);
        let h = cache
            .entry((m.0.id(), p.ext.lightmap.as_ref().map(|h| h.id()), rect))
            .or_insert_with(|| {
                let mut base = base.clone();
                let a = base.base_color.alpha();
                let c = p.diffuse_color;
                base.base_color = Color::linear_rgba(c.x, c.y, c.z, a);
                base.reflectance = 0.0;
                base.specular_tint_texture = None;
                mats.add(DecalMaterial { base, extension: p.ext.clone() })
            })
            .clone();
        // The lightmap is the material's own (directional, `ark_translucent.wgsl`), not Bevy's;
        // the dominant light's shadow is the distance field, not Bevy's shadow maps.
        commands
            .entity(e)
            .remove::<(PendingDecal, MeshMaterial3d<StandardMaterial>, bevy::pbr::Lightmap)>()
            .insert((MeshMaterial3d(h), bevy::light::NotShadowReceiver));
    }
}
