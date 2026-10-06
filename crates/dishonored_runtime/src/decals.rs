//! Translucent decals of `generic_translucent_PMAT` (ground puddles, street splotches, graffiti,
//! confetti), shaded the way the material's compiled shaders do (`ark_translucent.wgsl`; the
//! maths is in research/dishonored/RENDERING.md "Translucent decals").
//!
//! Boyle's ground decals are ordinary static meshes laid a few units over the ground (there are
//! no `DecalActor`s on Boyle) with a translucent material: `ext_boyle_puddles_decals_01` with
//! `towerEmpress_ground.puddles_decals_01_inst`. Drawn as Bevy's lit `StandardMaterial`, they took
//! Bevy's environment specular (Fresnel-bright at the grazing angles of the ground) and the
//! placeholder ambient, and showed as pale patches. The original's base pass has no lightmap and
//! no specular for them: the diffuse texture plus the scene reflection masked by the cubemap
//! mask's channel, lit by the base pass ambient and the light passes' `N.L`, blended over the
//! ground by the opacity mask's channel clamped to `O_Minimum_Opacity` .. `O_Maximum_Opacity`.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::{MaterialParams, TextureId};

/// The base material this module renders.
pub const GENERIC_TRANSLUCENT_PMAT: &str = "Materials_ref.main_materials.generic_translucent_PMAT";

/// `AmbientColorAndSkyFactor.rgb` of the translucent base pass (`TBasePassPixelShader
/// FNoLightMapPolicyNoSkyLight`): measured in the running game on Boyle Ext as (0, 0, 0, 1) in
/// every base pass drawn (`probe_launch measure psconst AmbientColorAndSkyFactor`, U-562), so the
/// decals are lit by the light passes only.
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
    pub fn new(p: &MaterialParams, textures: &[(String, TextureId)], images: &[Handle<Image>], scene: Option<Handle<Image>>) -> Option<PendingDecal> {
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
        Some(PendingDecal { ext: ArkTranslucent { params, mask, scene }, diffuse_color: DecalUniform::diffuse_color(p) })
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
    mut cache: Local<std::collections::HashMap<AssetId<StandardMaterial>, Handle<DecalMaterial>>>,
) {
    for (e, p, m) in &q {
        let Some(base) = std_mats.get(&m.0) else { continue };
        let h = cache
            .entry(m.0.id())
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
        // No lightmap and no shadows for translucency (FNoLightMapPolicy, FNoStaticShadowingPolicy).
        commands
            .entity(e)
            .remove::<(PendingDecal, MeshMaterial3d<StandardMaterial>, bevy::pbr::Lightmap)>()
            .insert((MeshMaterial3d(h), bevy::light::NotShadowReceiver));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(switches: &[(&str, bool)], masks: &[(&str, u8)], vectors: &[(&str, [f32; 4])]) -> MaterialParams {
        MaterialParams {
            blend: dishonored_ir::MaterialBlend::Translucent,
            base_material: Some(GENERIC_TRANSLUCENT_PMAT.into()),
            static_switches: switches.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            mask_channels: masks.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            vectors: vectors.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            ..Default::default()
        }
    }

    /// `towerEmpress_ground.puddles_decals_01_inst` (Boyle's ground puddles): scene reflection
    /// masked by the B channel of `_s`, no diffuse colour, no Fresnel.
    #[test]
    fn decodes_the_boyle_puddles() {
        let p = params(
            &[("N_Enable_NormalMap", true), ("C_Enable_Cubemap", true), ("C_Enable_CubemapMask", true), ("D_Enable_Diffuse_color", false), ("F_Invert_Fresnel_result", true), ("S_Enable_Specular", false)],
            &[("C_Cubemap_Mask_Channel", 2), ("O_Opacity_Mask_Channel", 1)],
            &[("D_Diffuse_color", [1.0, 1.0, 1.0, 1.0])],
        );
        let u = DecalUniform::decode(&p, 1.0).unwrap();
        assert_eq!(u.mask, Vec4::new(0.0, 0.0, 1.0, 0.0));
        assert_eq!(u.ambient.w, 1.0);
        assert_eq!(u.cube, Vec4::new(1.0, 1.0, 1.0, 1.0));
        assert_eq!(u.fresnel, Vec4::ZERO);
        assert_eq!(DecalUniform::diffuse_color(&p), Vec3::ONE);
    }

    /// Colour switches premultiply rgb by a; the Fresnel term carries its exponent.
    #[test]
    fn colour_switches_and_fresnel() {
        let p = params(
            &[("D_Enable_Diffuse_color", true), ("C_Enable_Cubemap", true), ("C_Enable_Cubemap_Color", true), ("F_Enable_Fresnel", true), ("F_Invert_Fresnel_result", true)],
            &[],
            &[("D_Diffuse_color", [0.5, 0.25, 1.0, 2.0]), ("C_Cubemap_color", [1.0, 1.0, 1.0, 2.0]), ("F_Fresnel_Color", [1.0, 0.5, 0.0, 0.5])],
        );
        let u = DecalUniform::decode(&p, 1.0).unwrap();
        assert_eq!(DecalUniform::diffuse_color(&p), Vec3::new(1.0, 0.5, 2.0));
        assert_eq!(u.cube.truncate(), Vec3::splat(2.0));
        assert_eq!(u.cube.w, 0.0, "no cubemap mask");
        assert_eq!(u.fresnel, Vec4::new(0.5, 0.25, 0.0, 4.0));
        // Other bases and opaque materials are not decals.
        assert!(DecalUniform::decode(&MaterialParams::default(), 1.0).is_none());
        let mut q = p.clone();
        q.blend = dishonored_ir::MaterialBlend::Masked;
        assert!(DecalUniform::decode(&q, 1.0).is_none());
    }
}
