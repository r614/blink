//! Lens flares: the placed `LensFlareSource` actors (the Lighthouse's beacon, the candle glows, the
//! Outsider dream's hidden one) drawn as the original draws them, on the GPU.
//!
//! What the original does (research/dishonored/systems/render_lighting.md "Lens flares"):
//! `FLensFlareSceneProxy` (Dishonored.exe 0x008eb560) draws its template's *source element* in the
//! template's `SourceDPG` (`SDPG_World`: depth tested, 0x008e5b50) as one camera-facing quad at the
//! source position, with `FLensFlareVertexFactory`'s vertex shader (RefShaderCache shader 1332:
//! `CameraRight`, `CameraUp`, `LocalToWorld`, `ViewProjectionMatrix`) and the flare material's base
//! pass (`LensFlare_PMAT`, additive, unlit):
//!
//! - the element is evaluated at the radial or the source distance (`bUseSourceDistance`: the source
//!   element's is 0, 0x008e1dc0), the `DistMap_` distributions at the camera distance to the source
//!   in uu: quad width = `Size.x x Scaling x AxisScaling.x x DistMap_Scale.x`, likewise the height,
//!   colour = `Color x DistMap_Color`, alpha = `Alpha x DistMap_Alpha`;
//! - the quad is rotated by `Rotation` about the view axis and spans the camera's right and up;
//! - the pixel shader is the material's own: `TBasePassPixelShaderFNoLightMapPolicy...` of the
//!   `FLensFlareVertexFactory` shader map (2643 .. 2652 by the material's static switches),
//!   translated to WGSL by `ue3_shader` (`ue3prog::build_fragment`) and run on the quad. It reads
//!   `TEXCOORD0` (the quad's uv), `TEXCOORD1` (the vertex colour), `TEXCOORD2.y` (the source's
//!   radial distance, 0 at the screen centre, 1 at its edge when `bNormalizeRadialDistance`:
//!   0x008e1dc0 maps the circle to the square, so the distance is `max(|x|, |y|)` of the source's
//!   screen position; LIKELY: the position is normalised device coordinates) and `TEXCOORD5`
//!   (the screen position, for the depth-biased alpha's read of the scene depth).
//!
//! The vertex stage is rebuilt from shader 1332's inputs (the element's constants are in the
//! uniform): no per-frame CPU work, the vertex shader reads the camera. The retail templates have
//! no reflection elements, and their `ScreenPercentageMap` is the constant 0 (its use is in the
//! undecompiled draw 0x008e5b50 / 0x008e63c0, so it is not applied; see the audit doc).

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use dishonored_ir::particles::Dist;
use dishonored_ir::{DishonoredMap, LensFlareSource, UE3_TEX_BLACK};
use std::collections::HashMap;
use std::sync::RwLock;
use ue3_shader::ue3prog::{self, Policy, MAX_PARAMS};

use crate::coords;
use crate::level::LevelOf;
use crate::map_render::{RenderFeatures, to_image};
use crate::ue3mat::{self, Ue3Params};

/// `FLensFlareVertexFactory`'s DistMap samples kept per distribution (the retail tables have 2).
const MAX_SAMPLES: usize = 4;

/// The vertex factory the flare's shaders belong to in the material's shader map.
const VERTEX_FACTORY: &str = "FLensFlareVertexFactory";

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct LensFlareUniform {
    /// (Size.x x Scaling x AxisScaling.x, Size.y x Scaling x AxisScaling.y, Rotation,
    /// `bNormalizeRadialDistance`).
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
    /// The element's constants at the source element's lookup 0 (0x008e1dc0: `bUseSourceDistance`
    /// reads the source distance, which is 0 for the source element).
    pub fn decode(src: &LensFlareSource) -> LensFlareUniform {
        let e = &src.element;
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
            quad: Vec4::new(e.size.x * scaling * axis[0], e.size.y * scaling * axis[1], rotation, e.normalize_radial_distance as u32 as f32),
            base: base_rgb.extend(base_a),
            dm_scale,
            dm_color,
            dm_alpha,
            dm_meta: Vec4::new(ns, nc, na, 0.0),
            dm_time: Vec4::new(ts, tc, ta, 0.0),
        }
    }
}

/// Programs (the generated modules' shaders), indexed by [`LensFlareKey`].
static PROGRAMS: RwLock<Vec<Handle<Shader>>> = RwLock::new(Vec::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LensFlareKey(u32);

impl From<&LensFlareMaterial> for LensFlareKey {
    fn from(m: &LensFlareMaterial) -> Self {
        LensFlareKey(m.program)
    }
}

/// A flare drawn with its material's original pixel shader.
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
#[bind_group_data(LensFlareKey)]
pub struct LensFlareMaterial {
    /// The material's parameter values in the shader map's `param_nodes` order.
    #[uniform(0)]
    pub params: Ue3Params,
    /// `Texture2D_0` (`L_Lensflare_Mask`).
    #[texture(1)]
    #[sampler(2)]
    pub t0: Option<Handle<Image>>,
    /// `Texture2D_1` (`Du_Dust_Texture`).
    #[texture(3)]
    #[sampler(4)]
    pub t1: Option<Handle<Image>>,
    #[uniform(5)]
    pub geom: LensFlareUniform,
    pub program: u32,
}

impl Material for LensFlareMaterial {
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let Some(shader) = PROGRAMS.read().unwrap().get(key.bind_group_data.0 as usize).cloned() else { return Ok(()) };
        descriptor.vertex.shader = shader.clone();
        descriptor.vertex.entry_point = Some("vertex".into());
        descriptor.vertex.buffers = vec![layout.0.get_layout(&[Mesh::ATTRIBUTE_UV_0.at_shader_location(0)])?];
        // The quad faces the camera whatever its winding comes out as.
        descriptor.primitive.cull_mode = None;
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = shader;
            fragment.entry_point = Some("fragment".into());
            // BLEND_Additive: the shader's colour added to the frame.
            let add = |src: BlendFactor, dst: BlendFactor| BlendComponent { src_factor: src, dst_factor: dst, operation: BlendOperation::Add };
            let blend = Some(BlendState { color: add(BlendFactor::One, BlendFactor::One), alpha: add(BlendFactor::Zero, BlendFactor::One) });
            for t in fragment.targets.iter_mut().flatten() {
                t.blend = blend;
            }
        }
        Ok(())
    }
}

/// A flare waiting for its program (spawned without a material, given it on the next update).
#[derive(Component, Clone)]
pub struct PendingLensFlare {
    pub material: LensFlareMaterial,
    /// The flare material's shader map.
    pub map: u32,
}

/// A camera-facing quad: four vertices at the origin; the vertex shader spreads them by uv.
fn quad_mesh() -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 4]);
    m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0f32, 0.0], [0.0, 1.0], [1.0, 0.0], [1.0, 1.0]]);
    m.insert_indices(Indices::U32(vec![0, 1, 2, 2, 1, 3]));
    m
}

/// Spawn the map's lens flares (`--no-lens-flares` leaves them out; so does an install without
/// the shader caches). Returns how many.
pub fn spawn_flares(level: &str, map: &DishonoredMap, features: &RenderFeatures, commands: &mut Commands, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>) -> usize {
    if !features.lens_flares || !features.materials || map.lens_flares.is_empty() || !ue3mat::enabled() {
        return 0;
    }
    let quad = meshes.add(quad_mesh());
    let mut textures: HashMap<usize, Handle<Image>> = HashMap::new();
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
        let Some(ue) = map.material_params.get(mid).and_then(|p| p.ue3.as_ref()) else {
            warn!("lens flare {}: its flare material has no compiled shader map", src.source);
            continue;
        };
        let mut params = Ue3Params::default();
        for (i, v) in ue.params.iter().take(MAX_PARAMS).enumerate() {
            params.p[i] = Vec4::from(*v);
        }
        let mut texture = |k: usize| -> Option<Handle<Image>> {
            let t = (*ue.textures.get(k)?).filter(|t| *t != UE3_TEX_BLACK)?;
            let asset = map.textures.get(t)?;
            Some(textures.entry(t).or_insert_with(|| images.add(to_image(asset))).clone())
        };
        let material = LensFlareMaterial { params, t0: texture(0), t1: texture(1), geom: LensFlareUniform::decode(src), program: u32::MAX };
        let mut e = commands.spawn((
            Mesh3d(quad.clone()),
            Transform::from_translation(coords::pos(src.position)),
            if src.hidden { Visibility::Hidden } else { Visibility::Inherited },
            PendingLensFlare { material, map: ue.map },
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
        app.add_plugins(MaterialPlugin::<LensFlareMaterial>::default()).add_systems(Update, apply_pending_flares);
    }
}

fn apply_pending_flares(
    mut commands: Commands,
    q: Query<(Entity, &PendingLensFlare)>,
    mut mats: ResMut<Assets<LensFlareMaterial>>,
    mut shaders: ResMut<Assets<Shader>>,
    mut programs: Local<HashMap<u32, Option<u32>>>,
) {
    for (e, p) in &q {
        let program = *programs.entry(p.map).or_insert_with(|| build_program(p.map, &mut shaders));
        let mut ent = commands.entity(e);
        ent.remove::<PendingLensFlare>();
        match program {
            Some(program) => {
                let mut material = p.material.clone();
                material.program = program;
                ent.insert(MeshMaterial3d(mats.add(material)));
            }
            None => {
                ent.despawn();
            }
        }
    }
}

/// The module of a flare shader map: the translated pixel shader, the quad's vertex stage.
fn module(map_id: u32) -> Option<String> {
    let lib = ue3mat::library()?;
    let map = lib.maps.get(map_id as usize)?;
    let ps_type = Policy::NoLightMap.shader_types().1;
    let ps = lib.code.get(&map.shader(VERTEX_FACTORY, ps_type)?)?;
    let frag = match ue3prog::build_fragment(map, ps) {
        Ok(f) => f,
        Err(e) => {
            warn!("lens flare shader map {} ({map_id}): {e:#}", map.name);
            return None;
        }
    };
    if !frag.unknown.is_empty() {
        warn!("lens flare shader map {}: unsupported constants {:?}", map.name, frag.unknown);
    }
    let mut m = String::from(ue3prog::PLATFORM_BEVY);
    m.push_str(BINDINGS);
    m.push_str(&frag.wgsl);
    m.push_str(STAGES);
    if std::env::var("DH_UE3_DUMP").is_ok() {
        let dir = dishonored_content::cache_dir().join("debug").join("programs");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("flare_{map_id}.wgsl")), &m);
    }
    Some(m)
}

fn build_program(map_id: u32, shaders: &mut Assets<Shader>) -> Option<u32> {
    let wgsl = module(map_id)?;
    let handle = shaders.add(Shader::from_wgsl(wgsl, format!("ue3/lens_flare_{map_id}.wgsl")));
    let mut programs = PROGRAMS.write().unwrap();
    programs.push(handle);
    Some(programs.len() as u32 - 1)
}

/// The flare material's bindings (`LensFlareMaterial`) and the element's constants.
const BINDINGS: &str = r#"
struct UeMaterial { p: array<vec4<f32>, 64> };
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> ue_mat: UeMaterial;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var ue_t0: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var ue_s0: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var ue_t1: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var ue_s1: sampler;

struct LensFlare {
    quad: vec4<f32>,
    base: vec4<f32>,
    dm_scale: array<vec4<f32>, 4>,
    dm_color: array<vec4<f32>, 4>,
    dm_alpha: array<vec4<f32>, 4>,
    dm_meta: vec4<f32>,
    dm_time: vec4<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var<uniform> lf: LensFlare;
"#;

/// Vertex shader 1332's work and the fragment entry over the translated pixel shader.
const STAGES: &str = r#"
// A cooked distribution's samples, linear between them: index = t x time scale (a time scale of 0
// is the first sample, a constant). Counts of 0 read as 1.
fn lf_lookup(samples: array<vec4<f32>, 4>, count: f32, time_scale: f32, t: f32) -> vec4<f32> {
    var s = samples;
    let n = i32(count);
    if (n <= 0) {
        return vec4<f32>(1.0);
    }
    let x = max(t * time_scale, 0.0);
    let i = min(i32(floor(x)), n - 1);
    let j = min(i + 1, n - 1);
    var a = 0.0;
    if (i != j) {
        a = x - f32(i);
    }
    return mix(s[i], s[j], a);
}

struct FlareIn {
    @builtin(instance_index) instance_index: u32,
    @location(0) uv: vec2<f32>,
};

struct FlareOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) radial: f32,
    @location(3) screen: vec4<f32>,
};

// The quad is centred on the source and spans (uv - 0.5) x size along the camera's right and up,
// turned by the element's rotation: a = cos Right - sin Up, b = cos Up + sin Right.
@vertex
fn vertex(v: FlareIn) -> FlareOut {
    let centre = ue_world_from_local(v.instance_index)[3].xyz;
    // Camera distance to the source in uu: the `DistMap_` distributions' lookup.
    let d = distance(ue_view_position(), centre) * 50.0;
    let scale = lf_lookup(lf.dm_scale, lf.dm_meta.x, lf.dm_time.x, d);
    let half_size = (v.uv - vec2<f32>(0.5)) * vec2<f32>(lf.quad.x * scale.x, lf.quad.y * scale.y) * 0.02;
    let right = view_transformations::direction_view_to_world(vec3<f32>(1.0, 0.0, 0.0));
    let up = view_transformations::direction_view_to_world(vec3<f32>(0.0, 1.0, 0.0));
    let s = sin(lf.quad.z);
    let c = cos(lf.quad.z);
    let a = c * right - s * up;
    let b = c * up + s * right;
    let world = centre + half_size.x * a + half_size.y * b;
    let dm_c = lf_lookup(lf.dm_color, lf.dm_meta.y, lf.dm_time.y, d);
    let dm_a = lf_lookup(lf.dm_alpha, lf.dm_meta.z, lf.dm_time.z, d);
    // The source's radial distance (0x008e1dc0): its screen position's length, or with
    // `bNormalizeRadialDistance` the circle mapped to the square, max(|x|, |y|).
    let cc = ue_clip_from_world() * vec4<f32>(centre, 1.0);
    let ndc = cc.xy / max(abs(cc.w), 1e-4);
    var radial = length(ndc);
    if (lf.quad.w > 0.5) {
        radial = max(abs(ndc.x), abs(ndc.y));
    }
    let clip = ue_clip_from_world() * vec4<f32>(world, 1.0);
    var out: FlareOut;
    out.position = clip;
    out.uv = v.uv;
    out.color = vec4<f32>(lf.base.rgb * dm_c.rgb, lf.base.a * dm_a.x);
    out.radial = radial;
    // (x, y, z, w) of the clip position in uu, as the pixel shader reads the scene depth.
    out.screen = clip * 50.0;
    return out;
}

@fragment
fn fragment(in: FlareOut) -> @location(0) vec4<f32> {
    var tc: array<vec4<f32>, 8>;
    tc[0] = vec4<f32>(in.uv, 0.0, 1.0);
    tc[1] = in.color;
    tc[2] = vec4<f32>(0.0, in.radial, 0.0, 0.0);
    tc[5] = in.screen;
    return ue_ps_call(tc, in.position);
}
"#;
