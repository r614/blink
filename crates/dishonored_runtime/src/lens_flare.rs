//! Lens flares: the placed `LensFlareSource` actors (the Lighthouse's beacon, Bridge Part 1's two
//! candle glows, the Outsider dream's hidden one) drawn as the original draws them, on the GPU.
//!
//! What the original does (research/dishonored/systems/render_lighting.md "Lens flares"):
//! `FLensFlareSceneProxy` (Dishonored.exe 0x008eb560) draws its template's *source element* in the
//! template's `SourceDPG` (`SDPG_World`: depth tested, 0x008e5b50) as one camera-facing quad at the
//! source position, with `FLensFlareVertexFactory`'s vertex shader (shader 1332) and the flare
//! material's base pass (`LensFlare_PMAT`, additive, unlit):
//!
//! - the element is evaluated at the radial or the source distance (`bUseSourceDistance`: the source
//!   element's is 0, 0x008e1dc0), the `DistMap_` distributions at the camera distance to the source
//!   in uu: quad width = `Size.x x Scaling x AxisScaling.x x DistMap_Scale.x`, likewise the height,
//!   colour = `Color x DistMap_Color`, alpha = `Alpha x DistMap_Alpha`;
//! - the quad is rotated by `Rotation` about the view axis and spans the camera's right and up;
//! - the pixel shaders (2643 .. 2652) shade `uv` (see `lens_flare.wgsl`).
//!
//! No per-frame CPU work: the vertex shader reads the camera.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::particles::Dist;
use dishonored_ir::{DishonoredMap, LensFlareSource, MaterialParams};

use crate::coords;
use crate::level::LevelOf;
use crate::map_render::{RenderFeatures, light_unit, to_image};

/// `FLensFlareVertexFactory`'s DistMap samples kept per distribution (the retail tables have 2).
const MAX_SAMPLES: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct LensFlareUniform {
    /// (L_LensFlare_Power, L_Radial_Distance_Factor, L_Maximum_Opacity, L_Minimum_Opacity).
    pub terms: Vec4,
    /// (L_LensFlare_Glowing_Speed, Lg_LensFlare_Glowing_Range, Lg_LensFlare_Glowing_Range_Visibility,
    /// permutation bits: 1 glow, 2 mask, 4 dust, 8 depth-biased alpha).
    pub glow: Vec4,
    /// (Du_Dust_intensity, Du_Dust_tilling, Du_Dust_Speed, Db_Depth_Biased_Alpha_Intensity).
    pub dust: Vec4,
    /// `L_LensFlare_Color`; rgb x w multiplies the flare.
    pub color: Vec4,
    /// (Size.x x Scaling x AxisScaling.x, Size.y x Scaling x AxisScaling.y, Rotation, UE3 radiance to
    /// Bevy light units).
    pub quad: Vec4,
    /// (Color x source colour, Alpha) at the source distance.
    pub base: Vec4,
    /// `DistMap_Scale` / `DistMap_Color` / `DistMap_Alpha` samples (xyz / xyz / x).
    pub dm_scale: [Vec4; MAX_SAMPLES],
    pub dm_color: [Vec4; MAX_SAMPLES],
    pub dm_alpha: [Vec4; MAX_SAMPLES],
    /// Sample counts and time scales (per uu) of the three.
    pub dm_meta: Vec4,
    pub dm_time: Vec4,
}

fn scalar(p: &MaterialParams, name: &str, d: f32) -> f32 {
    p.scalars.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}

/// A distribution's samples (`dims` components each) and its time scale, for the shader's
/// piecewise-linear lookup (`Dist::eval`: index = (t - start) x scale, linear between samples).
fn pack(d: &Dist, dims: usize, what: &str, source: &str) -> ([Vec4; MAX_SAMPLES], f32, f32) {
    let mut out = [Vec4::ZERO; MAX_SAMPLES];
    let chunk = d.dims.max(1) as usize * if d.ranged { 2 } else { 1 };
    let n = if d.time_scale == 0.0 { 1 } else { d.table.len() / chunk.max(1) };
    if n > MAX_SAMPLES {
        warn!("lens flare {source}: {what} has {n} samples, the first {MAX_SAMPLES} are used");
    }
    if d.start_time != 0.0 {
        warn!("lens flare {source}: {what} starts at {} (read as 0)", d.start_time);
    }
    for (k, o) in out.iter_mut().enumerate().take(n.min(MAX_SAMPLES)) {
        for c in 0..dims.min(4) {
            o[c] = d.table.get(k * chunk + c.min(d.dims.max(1) as usize - 1)).copied().unwrap_or(0.0);
        }
    }
    (out, n.min(MAX_SAMPLES) as f32, d.time_scale)
}

impl LensFlareUniform {
    /// The uniform of a source with its flare material's parameters (the `LensFlare_PMAT` base's
    /// parameter defaults fill what the instance leaves out: Power 5, Radial_Distance_Factor 1,
    /// Maximum_Opacity 1, Minimum_Opacity 0, Glowing_Speed 0.5, Range 0.1, Range_Visibility 0.5,
    /// Du_Dust_intensity 1, Du_Dust_tilling 1, Du_Dust_Speed -0.05, Db intensity 0.1, colour white).
    pub fn decode(src: &LensFlareSource, p: &MaterialParams, light_unit: f32) -> LensFlareUniform {
        let e = &src.element;
        let on = |n: &str| p.switch(n) == Some(true);
        let flags = (on("Lg_Enable_Glowing_LensFlare") as u32) | ((on("L_Enable_LensFlare_Mask") as u32) << 1) | ((on("Du_Enable_Dust_panner") as u32) << 2) | ((on("Db_Enable_Depth_Biased_Alpha") as u32) << 3);
        let lf = p.vectors.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case("L_LensFlare_Color")).map(|v| v.1).unwrap_or([1.0; 4]);
        // The base distributions at the source element's lookup 0 (0x008e1dc0: `bUseSourceDistance`
        // reads the source distance, which is 0 for the source element).
        let scaling = e.scaling.eval1(0.0, 0.0);
        let axis = e.axis_scaling.eval(0.0, 0.0);
        let rotation = e.rotation.eval1(0.0, 0.0);
        let col = e.color.eval(0.0, 0.0);
        let alpha = e.alpha.eval1(0.0, 0.0);
        let (mut base_rgb, mut base_a) = (Vec3::new(col[0], col[1], col[2]), alpha);
        if e.modulate_color_by_source {
            base_rgb *= Vec3::new(src.source_color[0], src.source_color[1], src.source_color[2]);
            base_a *= src.source_color[3];
        }
        let (dm_scale, ns, ts) = pack(&e.dist_scale, 3, "DistMap_Scale", &src.source);
        let (dm_color, nc, tc) = pack(&e.dist_color, 3, "DistMap_Color", &src.source);
        let (dm_alpha, na, ta) = pack(&e.dist_alpha, 1, "DistMap_Alpha", &src.source);
        LensFlareUniform {
            terms: Vec4::new(scalar(p, "L_LensFlare_Power", 5.0), scalar(p, "L_Radial_Distance_Factor", 1.0), scalar(p, "L_Maximum_Opacity", 1.0), scalar(p, "L_Minimum_Opacity", 0.0)),
            glow: Vec4::new(scalar(p, "L_LensFlare_Glowing_Speed", 0.5), scalar(p, "Lg_LensFlare_Glowing_Range", 0.1), scalar(p, "Lg_LensFlare_Glowing_Range_Visibility", 0.5), flags as f32),
            dust: Vec4::new(scalar(p, "Du_Dust_intensity", 1.0), scalar(p, "Du_Dust_tilling", 1.0), scalar(p, "Du_Dust_Speed", -0.05), scalar(p, "Db_Depth_Biased_Alpha_Intensity", 0.1)),
            color: Vec4::from_array(lf),
            quad: Vec4::new(e.size.x * scaling * axis[0], e.size.y * scaling * axis[1], rotation, light_unit),
            base: base_rgb.extend(base_a),
            dm_scale,
            dm_color,
            dm_alpha,
            dm_meta: Vec4::new(ns, nc, na, 0.0),
            dm_time: Vec4::new(ts, tc, ta, 0.0),
        }
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct LensFlareMaterial {
    #[uniform(0)]
    pub params: LensFlareUniform,
    /// `L_Lensflare_Mask` (`L_Enable_LensFlare_Mask`, channel R).
    #[texture(1)]
    #[sampler(2)]
    pub mask: Option<Handle<Image>>,
    /// `Du_Dust_Texture` (`Du_Enable_Dust_panner`, channel R).
    #[texture(3)]
    pub dust: Option<Handle<Image>>,
}

impl Material for LensFlareMaterial {
    fn vertex_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/lens_flare.wgsl".into()
    }
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/lens_flare.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // The quad faces the camera whatever its winding comes out as.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// A flare waiting for its material (spawned without one, given it on the next update).
#[derive(Component, Clone)]
pub struct PendingLensFlare(pub LensFlareMaterial);

/// A camera-facing quad: four vertices at the origin; the vertex shader spreads them.
fn quad_mesh() -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 4]);
    m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0f32, 1.0, 0.0]; 4]);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0]]);
    m.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0f32; 4]; 4]);
    m.insert_indices(Indices::U32(vec![0, 1, 2, 2, 1, 3]));
    m
}

/// Spawn the map's lens flares (`--no-lens-flares` leaves them out). Returns how many.
pub fn spawn_flares(level: &str, map: &DishonoredMap, features: &RenderFeatures, commands: &mut Commands, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>) -> usize {
    if !features.lens_flares || !features.materials || map.lens_flares.is_empty() {
        return 0;
    }
    let quad = meshes.add(quad_mesh());
    let mut n = 0;
    for src in &map.lens_flares {
        let e = &src.element;
        if !e.enabled {
            continue;
        }
        if !e.use_source_distance {
            warn!("lens flare {}: the source element reads the radial distance (bUseSourceDistance off), which is not decoded; evaluated at 0", src.source);
        }
        if src.reflections > 0 {
            warn!("lens flare {}: {} reflection elements are not drawn", src.source, src.reflections);
        }
        // `LFMaterialIndex` at the lookup, out of range -> the first material (0x008e1dc0).
        let idx = e.material_index.eval1(0.0, 0.0);
        let mid = if idx >= 0.0 && (idx as usize) < e.materials.len() { e.materials[idx as usize] } else { e.materials.first().copied().unwrap_or(usize::MAX) };
        let (Some(params), Some(asset)) = (map.material_params.get(mid), map.materials.get(mid)) else {
            warn!("lens flare {}: no flare material", src.source);
            continue;
        };
        let mut texture = |name: &str| asset.texture_params.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).and_then(|t| map.textures.get(t.1)).map(|t| images.add(to_image(t)));
        let material = LensFlareMaterial { params: LensFlareUniform::decode(src, params, light_unit()), mask: texture("L_Lensflare_Mask"), dust: texture("Du_Dust_Texture") };
        let flags = material.params.glow.w as u32;
        if (flags & 2 != 0 && material.mask.is_none()) || (flags & 4 != 0 && material.dust.is_none()) {
            warn!("lens flare {}: a texture of its permutation did not resolve (mask {}, dust {})", src.source, material.mask.is_some(), material.dust.is_some());
        }
        let mut e = commands.spawn((
            Mesh3d(quad.clone()),
            Transform::from_translation(coords::pos(src.position)),
            if src.hidden { Visibility::Hidden } else { Visibility::Inherited },
            PendingLensFlare(material),
            bevy::light::NotShadowCaster,
            bevy::light::NotShadowReceiver,
            LevelOf(level.to_string()),
        ));
        match src.bounds {
            // The flare is culled with the template's fixed box about the source.
            Some([lo, hi]) => {
                let (a, b) = (coords::pos(lo), coords::pos(hi));
                e.insert(bevy::camera::primitives::Aabb::from_min_max(a.min(b), a.max(b)));
            }
            None => {
                e.insert(bevy::camera::visibility::NoFrustumCulling);
            }
        }
        if let Some(tag) = crate::level::streamed_tag(&map.streamed_levels, crate::level::source_package(&src.source)) {
            e.insert(tag);
        }
        n += 1;
    }
    n
}

pub struct LensFlarePlugin;

impl Plugin for LensFlarePlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "lens_flare.wgsl");
        app.add_plugins(MaterialPlugin::<LensFlareMaterial>::default()).add_systems(Update, apply_pending_flares);
    }
}

fn apply_pending_flares(mut commands: Commands, q: Query<(Entity, &PendingLensFlare)>, mut mats: ResMut<Assets<LensFlareMaterial>>) {
    for (e, p) in &q {
        let h = mats.add(p.0.clone());
        commands.entity(e).remove::<PendingLensFlare>().insert(MeshMaterial3d(h));
    }
}
