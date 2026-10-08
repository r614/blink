// Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.
//! A level's materials rendered with their original shaders (`ue3mat`), built from Blink's map
//! IR: the material builders (`MatBuilder::ue3_material`, `ue3_light_material`,
//! `build_ue3_material`), the per-instance data buffer in `ue3prog`'s layout, the light-map page
//! arrays, the full vertex layout and the dominant lights' passes are dishonored-bevy's
//! `level.rs` / `lightmap.rs`; the glue that feeds them from `DishonoredMap` (its
//! `MaterialParams::ue3` links, `LightmapAtlas` pages, `InstanceLightmap` placements and lights)
//! is Blink's.
//!
//! Every static mesh section whose material has a shader map and whose program builds is drawn
//! per instance with a `Ue3Material` (not merged into `map_render`'s batches: the programs read
//! each instance's light-map placement through its `MeshTag`); the rest keep `map_render`'s path.
//! The programs' WGSL is built on the level's worker thread (`prepare`); the materials are made
//! when the level's entities are added (`Ue3Spawn::apply`).
//!
//! Glue choices (see the report in the commit):
//! - Light-map pages: Blink's atlases are already sRGB-decoded and multiplied by each
//!   component's coefficient scales (`LightmapAtlas`), so they go in as `Rgb9e5Ufloat` pages
//!   with `LightMapScale` = 1. Pages of different sizes are padded to the largest (the instance's
//!   coordinate scale / bias absorb it). The shadow array holds each atlas's dominant
//!   directional plane (layers 0..N) then its dominant point / spot plane (layers N..2N).
//! - The dominant directional light: mode 1 (distance-field shadowed) on components with its
//!   shadow map, else 2 (lit; the attenuation buffer is Bevy's cascades, which the world still
//!   casts into since the IR has no `IrrelevantLights`).
//! - Cube maps go in UE3 face order (the programs sample them with UE-space vectors).
//! - `PF_V8U8` normal maps get an `Rg8Snorm` copy (`map_render::to_image` biases them to unorm).

use crate::map_render::RenderFeatures;
use crate::ue3mat::{self, Ue3Key, Ue3Material, Ue3Params, Ue3Programs};
use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat as WgpuFormat, TextureViewDescriptor, TextureViewDimension};
use bevy::render::storage::ShaderBuffer;
use dishonored_ir::{BlendHint, DishonoredMap, LightKind, MaterialBlend as Blend, TextureFormat, TextureId, UE3_TEX_BLACK};
use std::collections::{HashMap, HashSet};
use ue3_shader::ue3prog::{Policy, DYN_LIGHT_SLOT, DYN_SHADOW_SLOT, INSTANCE_STRIDE};

/// A dominant light's additive pass over a lit surface: its policy, parameters (UE space) and,
/// shadowed, its shadow map's coordinate scale / bias and page layer.
#[derive(Clone)]
pub struct LightPass {
    pub policy: Policy,
    pub params: [Vec4; 4],
    pub shadow: Option<(Vec4, u32)>,
}

/// A mesh section drawn with its original shaders.
#[derive(Clone)]
pub struct Part {
    pub mat: usize,
    pub policy: Policy,
    /// The instance's entry in the instance buffer (`MeshTag`).
    pub tag: u32,
    /// (static mesh, section, TEXCOORD1 from `uv_set1`)
    pub mesh: (usize, usize, bool),
    pub passes: Vec<LightPass>,
}

/// What `prepare` builds on the level's worker thread.
pub struct Prepared {
    pub parts: HashMap<(usize, usize), Part>,
    entries: Vec<[f32; 4]>,
    pages: [Image; 3],
    meshes: Vec<((usize, usize, bool), Mesh)>,
    snorm: Vec<(TextureId, Image)>,
    cubes: Vec<(Vec<TextureId>, Image)>,
    refl: Option<Image>,
}

/// The original-shader parts of a map (None: the path is off, or there is no shader library).
pub fn prepare(map: &DishonoredMap, features: &RenderFeatures) -> Option<Prepared> {
    if !features.materials || features.debug_lighting {
        return None;
    }
    ue3mat::library()?;
    let t0 = std::time::Instant::now();
    let n_pages = if features.lightmaps { map.lightmaps.len() } else { 0 };
    let (pw, ph) = map.lightmaps.iter().take(n_pages).fold((1u32, 1u32), |(w, h), l| (w.max(l.width), h.max(l.height)));
    // an atlas's share of the padded page
    let pad = |a: usize| -> Vec2 {
        let l = &map.lightmaps[a];
        Vec2::new(l.width as f32 / pw as f32, l.height as f32 / ph as f32)
    };
    let sun = map.lights.iter().find(|l| matches!(l.kind, LightKind::Directional) && l.dominant);

    // the candidate sections and the programs they need
    struct Cand {
        ii: usize,
        si: usize,
        mat: usize,
        map: u32,
        policy: Policy,
        lit: bool,
        set1: bool,
        /// the depth-only program it takes (masked: with its clipping); None: translucent
        depth: Option<bool>,
        blend: Blend,
    }
    let mut cands = Vec::new();
    for (ii, inst) in map.instances.iter().enumerate() {
        // hidden instances (shown with --show-hidden) keep the debug material
        if inst.hidden {
            continue;
        }
        let mesh = &map.meshes[inst.mesh];
        let lightmapped = inst.lightmap.is_some_and(|l| l.atlas < n_pages) && mesh.uv1.len() == mesh.positions.len();
        for (si, section) in mesh.sections.iter().enumerate() {
            let Some(mat) = inst.materials.get(si).copied().flatten().or(section.material) else { continue };
            let Some(p) = map.material_params.get(mat) else { continue };
            let Some(ue) = p.ue3.as_ref() else { continue };
            // materials sampling a scene capture's render target keep their own path (reflect.rs)
            if !p.render_targets.is_empty() {
                continue;
            }
            if !features.fx && map.materials[mat].blend == BlendHint::Translucent {
                continue;
            }
            // original shaders: light-mapped surfaces, and unlit / translucent ones
            let policy = if lightmapped { Policy::DirectionalLightMap } else { Policy::NoLightMap };
            let lit = !ue.unlit && matches!(ue.blend, Blend::Opaque | Blend::Masked);
            let set1 = !lightmapped && mesh.uv_set1.len() == mesh.positions.len();
            let depth = match ue.blend {
                Blend::Opaque => Some(false),
                Blend::Masked => Some(true),
                _ => None,
            };
            cands.push(Cand { ii, si, mat, map: ue.map, policy, lit, set1, depth, blend: ue.blend });
        }
    }
    let built = build_programs(cands.iter().map(|c| (c.map, c.policy, c.lit)));
    cands.retain(|c| built.contains(&(c.map, c.policy, c.lit)));
    // the depth-only programs (depth prepass, shadows), across all cores too
    {
        let keys: Vec<(u32, Policy, bool)> = cands.iter().filter_map(|c| Some((c.map, c.policy, c.depth?))).collect::<HashSet<_>>().into_iter().collect();
        crate::map_render::par_map(&keys, |k| ue3mat::depth_wgsl(k.0, k.1, k.2).is_some());
    }

    // the instance buffer: entry 0 the scene, then one per instance drawn here
    let stride = INSTANCE_STRIDE as usize;
    let mut tags: HashMap<usize, u32> = HashMap::new();
    for c in &cands {
        let n = tags.len() as u32 + 1;
        tags.entry(c.ii).or_insert(n);
    }
    let mut entries = vec![[0.0f32; 4]; (tags.len() + 1) * stride];
    let ue_ambient: f32 = std::env::var("DH_UE3_AMBIENT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.05);
    let bits = |v: [u32; 4]| v.map(f32::from_bits);
    for (&ii, &tag) in &tags {
        let inst = &map.instances[ii];
        let mesh = &map.meshes[inst.mesh];
        let o = tag as usize * stride;
        let lm = inst.lightmap.filter(|l| l.atlas < n_pages && mesh.uv1.len() == mesh.positions.len());
        let sun_mode = |df: bool| -> u32 {
            match sun {
                None => 0,
                Some(_) if df => 1,
                Some(_) => 2,
            }
        };
        match lm {
            Some(l) => {
                let k = pad(l.atlas);
                let (scale, bias) = ((l.uv_max - l.uv_min) * k, l.uv_min * k);
                // light-map coordinate scale.xy, bias.xy; the coefficient scales are in the pages
                entries[o] = [scale.x, scale.y, bias.x, bias.y];
                entries[o + 1] = [1.0; 4];
                entries[o + 2] = [1.0; 4];
                let df = l.df_shadow && !map.lightmaps[l.atlas].shadow.is_empty();
                entries[o + 3] = bits([l.atlas as u32, l.atlas as u32, 1, sun_mode(df)]);
                // the dominant light's shadow plane shares the atlas layout
                entries[o + 4] = entries[o];
            }
            None => {
                entries[o + 3] = bits([0, 0, 0, sun_mode(false)]);
                entries[o + 5] = [ue_ambient, ue_ambient, ue_ambient, 1.0];
            }
        }
    }
    // entry 0 as an instance (particles): a soft ambient for lit sprites
    let pa: f32 = std::env::var("DH_UE3_PARTICLE_AMBIENT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.3);
    entries[5] = [pa, pa, pa, 1.0];
    // entry 0: the dominant directional light (UE space, towards the light)
    if let Some(l) = sun {
        let d = l.direction.normalize_or(Vec3::NEG_Z);
        entries[0] = [-d.x, -d.y, -d.z, 0.0];
        let c = Vec3::from(l.color) * l.brightness.max(0.0);
        entries[1] = [c.x, c.y, c.z, 0.0];
    }

    // dynamic lights over the original shaders: each dominant point / spot light's own pass over
    // the lit surfaces it reaches (`DH_NO_DYN_LIGHTS` turns them off)
    let baked_world = n_pages > 0;
    let dyn_lights: Vec<(usize, Policy, Vec3, f32, [Vec4; 4])> = if std::env::var("DH_NO_DYN_LIGHTS").is_ok() {
        Vec::new()
    } else {
        map.lights
            .iter()
            .enumerate()
            .filter(|(_, l)| (l.dominant || !baked_world) && matches!(l.kind, LightKind::Point | LightKind::Spot { .. }) && l.brightness > 0.0 && l.radius > 0.1)
            .map(|(li, l)| {
                let (inner_deg, outer_deg) = match l.kind {
                    LightKind::Spot { inner_cone_deg, outer_cone_deg } => (inner_cone_deg, outer_cone_deg),
                    _ => (0.0, 89.0),
                };
                let c = Vec3::from(l.color) * l.brightness;
                let d = l.direction.normalize_or(Vec3::NEG_Z);
                let outer = outer_deg.clamp(1.0, 89.0).to_radians().cos();
                let inner = inner_deg.clamp(0.0, outer_deg.clamp(1.0, 89.0)).to_radians().cos();
                let params = [
                    l.position.extend(1.0 / l.radius),
                    c.extend(if l.falloff_exponent > 0.0 { l.falloff_exponent } else { 2.0 }),
                    Vec4::new(outer, 1.0 / (inner - outer).max(1e-3), 0.0, 0.0),
                    d.extend(0.0),
                ];
                (li, if matches!(l.kind, LightKind::Spot { .. }) { Policy::SpotLight } else { Policy::PointLight }, l.position, l.radius, params)
            })
            .collect()
    };
    // per instance: the passes of the lights that reach it (policy, parameters, shadow)
    let mut inst_passes: HashMap<usize, Vec<LightPass>> = HashMap::new();
    if !dyn_lights.is_empty() {
        for &ii in tags.keys() {
            let inst = &map.instances[ii];
            let mesh = &map.meshes[inst.mesh];
            let (lo, hi) = (mesh.bounds_min, mesh.bounds_max);
            let center = inst.transform.transform_point3((lo + hi) * 0.5);
            let reach = inst.transform.transform_vector3((hi - lo) * 0.5).length();
            let lm = inst.lightmap.filter(|l| l.atlas < n_pages && mesh.uv1.len() == mesh.positions.len());
            let mut passes = Vec::new();
            for (li, policy, lp, radius, params) in &dyn_lights {
                if center.distance(*lp) > radius + reach {
                    continue;
                }
                // a spot only reaches what is inside its cone
                if *policy == Policy::SpotLight {
                    let l = &map.lights[*li];
                    let to = center - *lp;
                    let d = to.length();
                    if d > reach {
                        let axis = l.direction.normalize_or(Vec3::NEG_Z);
                        let off = to.dot(axis) / d;
                        let outer = match l.kind {
                            LightKind::Spot { outer_cone_deg, .. } => outer_cone_deg,
                            _ => 89.0,
                        };
                        let half = outer.clamp(1.0, 89.0).to_radians() + (reach / d).clamp(-1.0, 1.0).asin();
                        if off.clamp(-1.0, 1.0).acos() > half {
                            continue;
                        }
                    }
                }
                // a dominant light through its shadow map here (none: unshadowed)
                let shadow = lm.filter(|l| l.df_light == Some(*li) && !map.lightmaps[l.atlas].shadow_local.is_empty()).map(|l| {
                    let k = pad(l.atlas);
                    let (scale, bias) = ((l.uv_max - l.uv_min) * k, l.uv_min * k);
                    (Vec4::new(scale.x, scale.y, bias.y, bias.x), (n_pages + l.atlas) as u32)
                });
                let policy = match (shadow.is_some(), *policy) {
                    (true, Policy::SpotLight) => Policy::SpotLightShadowed,
                    (true, _) => Policy::PointLightShadowed,
                    (false, p) => p,
                };
                passes.push(LightPass { policy, params: *params, shadow });
            }
            if !passes.is_empty() {
                inst_passes.insert(ii, passes);
            }
        }
    }
    // the light passes' programs (lit materials only: `ue3_light_material`)
    let light_ok = |mat: usize| map.material_params[mat].ue3.as_ref().is_some_and(|ue| !ue.unlit && matches!(ue.blend, Blend::Opaque | Blend::Masked) && ue.params.len() <= DYN_SHADOW_SLOT);
    let light_built = build_programs(cands.iter().filter(|c| light_ok(c.mat)).flat_map(|c| inst_passes.get(&c.ii).into_iter().flatten().map(move |p| (c.map, p.policy, false))));

    // start composing their pipeline variants (the def lists recorded for each kind of use) on
    // the shader pool, while the level finishes loading
    if ue3mat::shader_cache_enabled() {
        let mut uses: HashSet<((u32, Policy, bool), bool, u8)> = HashSet::new();
        for c in &cands {
            uses.insert(((c.map, c.policy, c.lit), false, ue3mat::blend_code(c.blend)));
            if let Some(masked) = c.depth {
                uses.insert(((c.map, c.policy, masked), true, masked as u8));
            }
            if light_ok(c.mat) {
                for p in inst_passes.get(&c.ii).into_iter().flatten().filter(|p| light_built.contains(&(c.map, p.policy, false))) {
                    // (a light pass: additive, `ue3_light_material`)
                    uses.insert(((c.map, p.policy, false), false, 3));
                }
            }
        }
        ue3mat::prefetch_variants(uses.into_iter().collect());
    }

    let mut parts = HashMap::new();
    for c in &cands {
        let passes = if light_ok(c.mat) {
            inst_passes.get(&c.ii).into_iter().flatten().filter(|p| light_built.contains(&(c.map, p.policy, false))).cloned().collect()
        } else {
            Vec::new()
        };
        let mesh = (map.instances[c.ii].mesh, c.si, c.set1);
        parts.insert((c.ii, c.si), Part { mat: c.mat, policy: c.policy, tag: tags[&c.ii], mesh, passes });
    }

    // their meshes, the V8U8 normal maps and the cube maps they sample
    let mesh_keys: Vec<(usize, usize, bool)> = parts.values().map(|p| p.mesh).collect::<HashSet<_>>().into_iter().collect();
    let meshes = mesh_keys.iter().copied().zip(crate::map_render::par_map(&mesh_keys, |k| full_mesh(map, *k, features))).collect();
    let used: HashSet<usize> = parts.values().map(|p| p.mat).collect();
    let mut snorm_ids = HashSet::new();
    let mut cube_faces = HashSet::new();
    for &m in &used {
        let Some(ue) = map.material_params[m].ue3.as_ref() else { continue };
        for &t in ue.textures.iter().flatten() {
            if t != UE3_TEX_BLACK && map.textures.get(t).is_some_and(|t| t.format == TextureFormat::Rg8Snorm) {
                snorm_ids.insert(t);
            }
        }
        if let Some(Some(f)) = ue.cube_textures.first() {
            cube_faces.insert(f.clone());
        }
    }
    let snorm = snorm_ids.into_iter().map(|t| (t, snorm_image(&map.textures[t]))).collect();
    let cubes = cube_faces.into_iter().filter_map(|f| Some((f.clone(), cube_image(map, &f)?))).collect();
    let refl = cube_image(map, &map.environment.cubemap_faces);
    let pages = page_arrays(map, n_pages, pw, ph);
    info!(
        "original shaders: {} sections of {} instances ({} materials, {} light passes), prepared in {:.2}s",
        parts.len(),
        tags.len(),
        used.len(),
        parts.values().map(|p| p.passes.len()).sum::<usize>(),
        t0.elapsed().as_secs_f32()
    );
    Some(Prepared { parts, entries, pages, meshes, snorm, cubes, refl })
}

/// Build programs' WGSL across all cores (cached in `ue3mat`); the keys that build.
fn build_programs(keys: impl Iterator<Item = (u32, Policy, bool)>) -> HashSet<(u32, Policy, bool)> {
    let keys: Vec<(u32, Policy, bool)> = keys.collect::<HashSet<_>>().into_iter().collect();
    let ok = crate::map_render::par_map(&keys, |k| ue3mat::program_wgsl(k.0, k.1, k.2).is_some());
    keys.into_iter().zip(ok).filter(|(_, ok)| *ok).map(|(k, _)| k).collect()
}

/// A section's mesh with every attribute the original shaders read (UE3 local vertex factory;
/// dishonored-bevy's `make_mesh_full`): texture coordinate 1 (the light-map set, or the mesh's
/// own second set on surfaces without a light map, else UV0), tangents, vertex colours
/// (white without).
fn full_mesh(map: &DishonoredMap, (mi, si, set1): (usize, usize, bool), features: &RenderFeatures) -> Mesh {
    let m = &map.meshes[mi];
    let n = m.positions.len();
    let mut mesh = crate::map_render::build_section_mesh(map, mi, si, features);
    if set1 {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, m.uv_set1.iter().map(|u| u.to_array()).collect::<Vec<[f32; 2]>>());
    } else if m.uv1.len() != n {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, m.uv0.iter().map(|u| u.to_array()).collect::<Vec<[f32; 2]>>());
    }
    if !mesh.contains_attribute(Mesh::ATTRIBUTE_TANGENT) {
        mesh.insert_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[1.0f32, 0.0, 0.0, 1.0]; n]);
    }
    let colors: Vec<[f32; 4]> = if m.colors.len() == n {
        m.colors.iter().map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]).collect()
    } else {
        vec![[1.0; 4]; n]
    };
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh
}

/// A `PF_V8U8` normal map as the signed texture the shaders sample.
fn snorm_image(t: &dishonored_ir::TextureAsset) -> Image {
    let mut img = crate::map_render::to_image(t);
    img.texture_descriptor.format = WgpuFormat::Rg8Snorm;
    img.data = Some(t.mips.concat());
    img
}

/// A cube map from its six faces (+X, -X, +Y, -Y, +Z, -Z), in UE3's face order with its mips.
fn cube_image(map: &DishonoredMap, faces: &[TextureId]) -> Option<Image> {
    if faces.len() != 6 {
        return None;
    }
    let first = map.textures.get(faces[0])?;
    let (size, mips) = (first.width, first.mips.len().max(1));
    if faces.iter().any(|&f| map.textures.get(f).is_none_or(|t| t.width != size || t.height != size || t.mips.len().max(1) != mips)) {
        warn!("cube map {faces:?}: faces don't match");
        return None;
    }
    let mut data = Vec::new();
    for &f in faces {
        for m in 0..mips {
            let (_, _, px) = dishonored_assets::texture::decode_rgba8(&map.textures[f], m)?;
            data.extend(px.iter().flat_map(|p| [p[0], p[1], p[2], 255]));
        }
    }
    let mut img = Image::new_uninit(Extent3d { width: size, height: size, depth_or_array_layers: 6 }, TextureDimension::D2, WgpuFormat::Rgba8UnormSrgb, RenderAssetUsages::RENDER_WORLD);
    img.texture_descriptor.mip_level_count = mips as u32;
    img.data = Some(data);
    img.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() });
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor { mag_filter: ImageFilterMode::Linear, min_filter: ImageFilterMode::Linear, mipmap_filter: ImageFilterMode::Linear, ..default() });
    Some(img)
}

/// The light-map page arrays (dishonored-bevy's `make_page_array`) from Blink's atlases, each
/// padded to the largest: average colour (`LightMapTextures[0]`), basis intensities (`[1]`),
/// and the shadow planes (dominant directional per atlas, then dominant point / spot).
fn page_arrays(map: &DishonoredMap, n: usize, w: u32, h: u32) -> [Image; 3] {
    let texels = (w * h) as usize;
    let pad = |src: &[u8], sw: u32, sh: u32, bpp: usize, fill: u8, out: &mut Vec<u8>| {
        let start = out.len();
        out.resize(start + texels * bpp, fill);
        if src.len() == (sw * sh) as usize * bpp {
            for y in 0..sh as usize {
                let row = &src[y * sw as usize * bpp..(y + 1) * sw as usize * bpp];
                let at = start + y * w as usize * bpp;
                out[at..at + row.len()].copy_from_slice(row);
            }
        }
    };
    let (mut c, mut d, mut s) = (Vec::new(), Vec::new(), Vec::new());
    for l in map.lightmaps.iter().take(n) {
        let bytes = |v: &[u32]| v.iter().flat_map(|t| t.to_le_bytes()).collect::<Vec<u8>>();
        pad(&bytes(&l.texels), l.width, l.height, 4, 0, &mut c);
        // without directional data: unit intensities (as `map_render::lightmap_image`)
        let dir = if l.directional.len() == l.texels.len() { l.directional.clone() } else { vec![dishonored_assets::lightmap::pack_rgb9e5([1.0; 3]); l.texels.len()] };
        pad(&bytes(&dir), l.width, l.height, 4, 0, &mut d);
    }
    for l in map.lightmaps.iter().take(n) {
        pad(&l.shadow, l.width, l.height, 1, 255, &mut s);
    }
    for l in map.lightmaps.iter().take(n) {
        pad(&l.shadow_local, l.width, l.height, 1, 255, &mut s);
    }
    let array = |data: Vec<u8>, layers: usize, format: WgpuFormat, bpp: usize, fill: u8| {
        let (w, h, layers, data) = if layers == 0 { (1, 1, 1, vec![fill; bpp]) } else { (w, h, layers as u32, data) };
        let mut img = Image::new(Extent3d { width: w, height: h, depth_or_array_layers: layers }, TextureDimension::D2, data, format, RenderAssetUsages::RENDER_WORLD);
        img.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::D2Array), ..default() });
        img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::ClampToEdge,
            address_mode_v: ImageAddressMode::ClampToEdge,
            address_mode_w: ImageAddressMode::ClampToEdge,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            ..default()
        });
        img
    };
    [array(c, n, WgpuFormat::Rgb9e5Ufloat, 4, 0), array(d, n, WgpuFormat::Rgb9e5Ufloat, 4, 0), array(s, 2 * n, WgpuFormat::R8Unorm, 1, 255)]
}

/// The prepared parts with their GPU assets added, and the entities they go on, until the
/// materials are made (`apply`, with world access).
pub struct Ue3Spawn {
    parts: HashMap<(usize, usize), Part>,
    meshes: HashMap<(usize, usize, bool), Handle<Mesh>>,
    entries: Vec<[f32; 4]>,
    defs: Vec<Option<dishonored_ir::Ue3Material>>,
    images: Vec<Handle<Image>>,
    snorm: HashMap<TextureId, Handle<Image>>,
    cubes: HashMap<Vec<TextureId>, Handle<Image>>,
    refl: Option<Handle<Image>>,
    pages: [Handle<Image>; 3],
    white: Handle<Image>,
    black: Handle<Image>,
    black_cube: Handle<Image>,
    targets: Vec<(Entity, Part, Handle<StandardMaterial>)>,
}

impl Ue3Spawn {
    pub fn new(p: Prepared, map: &DishonoredMap, image_handles: &[Handle<Image>], images: &mut Assets<Image>, meshes: &mut Assets<Mesh>) -> Self {
        let fill = |images: &mut Assets<Image>, c: [u8; 4]| images.add(Image::new_fill(Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, TextureDimension::D2, &c, WgpuFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD));
        let white = fill(images, [255; 4]);
        let black = fill(images, [0, 0, 0, 255]);
        let black_cube = images.add(black_cube_image());
        let [c, d, s] = p.pages;
        Ue3Spawn {
            parts: p.parts,
            meshes: p.meshes.into_iter().map(|(k, m)| (k, meshes.add(m))).collect(),
            entries: p.entries,
            defs: map.material_params.iter().map(|m| m.ue3.clone()).collect(),
            images: image_handles.to_vec(),
            snorm: p.snorm.into_iter().map(|(t, i)| (t, images.add(i))).collect(),
            cubes: p.cubes.into_iter().map(|(f, i)| (f, images.add(i))).collect(),
            refl: p.refl.map(|i| images.add(i)),
            pages: [images.add(c), images.add(d), images.add(s)],
            white,
            black,
            black_cube,
            targets: Vec::new(),
        }
    }

    /// The part drawn for an instance's section, and its mesh.
    pub fn part(&self, instance: usize, section: usize) -> Option<(Part, Handle<Mesh>)> {
        let p = self.parts.get(&(instance, section))?;
        Some((p.clone(), self.meshes.get(&p.mesh)?.clone()))
    }

    /// An entity drawing `part`; `fallback` is its material's standard approximation, used if
    /// its original-shader material cannot be made.
    pub fn push(&mut self, entity: Entity, part: Part, fallback: Handle<StandardMaterial>) {
        self.targets.push((entity, part, fallback));
    }

    /// Make the materials and the light passes, and put them on the entities.
    pub fn apply(self, world: &mut World) {
        // (an app without `Ue3Plugin`: the standard materials)
        if !world.contains_resource::<Assets<Ue3Material>>() || !world.contains_resource::<Ue3Programs>() || !world.contains_resource::<Assets<ShaderBuffer>>() {
            for (e, _, fallback) in &self.targets {
                if let Ok(mut e) = world.get_entity_mut(*e) {
                    e.insert(MeshMaterial3d(fallback.clone()));
                }
            }
            return;
        }
        let instances = world.resource_mut::<Assets<ShaderBuffer>>().add(ShaderBuffer::from(self.entries));
        let [lm0, lm1, shadow] = self.pages;
        let ue_base = Ue3Material {
            params: Ue3Params::default(),
            t0: self.white.clone(),
            t1: self.white.clone(),
            t2: self.white.clone(),
            t3: self.white.clone(),
            t4: self.white.clone(),
            t5: self.white.clone(),
            t6: self.white.clone(),
            t7: self.white.clone(),
            t8: self.white.clone(),
            t9: self.white.clone(),
            t10: self.white.clone(),
            t11: self.white.clone(),
            cube0: self.black_cube.clone(),
            instances,
            lm0,
            lm1,
            shadow,
            white: self.white.clone(),
            black: self.black.clone(),
            // the level's reflection cube, for every surface
            refl: self.refl.clone().unwrap_or(self.black_cube.clone()),
            black_cube: self.black_cube.clone(),
            key: Ue3Key::new(0, Blend::Opaque, false),
            map: 0,
            mat_id: u32::MAX,
            light_ok: false,
        };
        let mut mb = MatBuilder { defs: &self.defs, images: &self.images, snorm: &self.snorm, cubes: &self.cubes, black: self.black.clone(), ue_base, ue_mats: HashMap::new() };
        let mut made = 0usize;
        let mut fallbacks = 0usize;
        let mut passes = 0usize;
        world.resource_scope(|world, mut materials: Mut<Assets<Ue3Material>>| {
            world.resource_scope(|world, mut programs: Mut<Ue3Programs>| {
                world.resource_scope(|world, mut shaders: Mut<Assets<Shader>>| {
                    for (e, part, fallback) in &self.targets {
                        if world.get_entity(*e).is_err() {
                            continue;
                        }
                        let Some(h) = mb.ue3_material(part.mat as u32, part.policy, &mut materials, &mut programs, &mut shaders) else {
                            world.entity_mut(*e).insert(MeshMaterial3d(fallback.clone()));
                            fallbacks += 1;
                            continue;
                        };
                        world.entity_mut(*e).insert((MeshMaterial3d(h), MeshTag(part.tag)));
                        made += 1;
                        let (Some(mesh), layers) = (world.get::<Mesh3d>(*e).map(|m| m.0.clone()), world.get::<RenderLayers>(*e).cloned()) else { continue };
                        for p in &part.passes {
                            let Some(lm) = mb.ue3_light_material(part.mat as u32, p.policy, p.params, p.shadow, &mut materials, &mut programs, &mut shaders) else { continue };
                            let mut pass = world.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(lm), MeshTag(part.tag), NotShadowCaster, NotShadowReceiver, Transform::IDENTITY, Visibility::Inherited, ChildOf(*e)));
                            if let Some(l) = layers.clone() {
                                pass.insert(l);
                            }
                            passes += 1;
                        }
                    }
                });
            });
        });
        info!("original shaders: {made} surfaces ({fallbacks} fell back to the standard material), {passes} dynamic light passes");
        let s = world.resource::<Ue3Programs>().stats();
        info!("pipelines: {} programs ({} distinct), {} depth/shadow variants ({} requested)", s.programs, s.distinct, s.depth_variants, s.depth_requests);
    }
}

/// The original-shader materials of a level (dishonored-bevy's `MatBuilder`, its original-shader
/// half), over the map's `MaterialParams::ue3` links and texture handles.
struct MatBuilder<'a> {
    defs: &'a [Option<dishonored_ir::Ue3Material>],
    images: &'a [Handle<Image>],
    snorm: &'a HashMap<TextureId, Handle<Image>>,
    cubes: &'a HashMap<Vec<TextureId>, Handle<Image>>,
    black: Handle<Image>,
    /// bindings shared by every original-shader material (textures filled per material)
    ue_base: Ue3Material,
    ue_mats: HashMap<(u32, Policy), Option<Handle<Ue3Material>>>,
}

impl MatBuilder<'_> {
    fn image(&self, id: TextureId) -> Option<Handle<Image>> {
        self.snorm.get(&id).or_else(|| self.images.get(id)).cloned()
    }

    /// The material rendered with its original shaders for a light-map policy, if it has a
    /// compiled program.
    fn ue3_material(&mut self, id: u32, policy: Policy, materials: &mut Assets<Ue3Material>, programs: &mut Ue3Programs, shaders: &mut Assets<Shader>) -> Option<Handle<Ue3Material>> {
        if let Some(m) = self.ue_mats.get(&(id, policy)) {
            return m.clone();
        }
        let r = self.build_ue3_material(id, policy, materials, programs, shaders);
        self.ue_mats.insert((id, policy), r.clone());
        r
    }

    /// A dynamic light's pass over a lit surface: the material's light shaders with the
    /// light's parameters (UE space) in its last uniform slots, added over the surface.
    #[allow(clippy::too_many_arguments)]
    fn ue3_light_material(
        &mut self,
        id: u32,
        policy: Policy,
        light: [Vec4; 4],
        shadow: Option<(Vec4, u32)>,
        materials: &mut Assets<Ue3Material>,
        programs: &mut Ue3Programs,
        shaders: &mut Assets<Shader>,
    ) -> Option<Handle<Ue3Material>> {
        let ue = self.defs.get(id as usize)?.as_ref()?;
        if ue.unlit || !matches!(ue.blend, Blend::Opaque | Blend::Masked) || ue.params.len() > DYN_SHADOW_SLOT {
            return None;
        }
        let base = self.ue3_material(id, policy, materials, programs, shaders)?;
        let mut m = materials.get(&base)?.clone();
        for (k, v) in light.iter().enumerate() {
            m.params.p[DYN_LIGHT_SLOT + k] = *v;
        }
        if let Some((scale_bias, layer)) = shadow {
            m.params.p[DYN_SHADOW_SLOT] = scale_bias;
            m.params.p[DYN_SHADOW_SLOT + 1] = Vec4::new(layer as f32, 0.0, 0.0, 0.0);
        }
        m.key.blend = 3;
        m.key.light_pass = true;
        Some(materials.add(m))
    }

    fn build_ue3_material(&mut self, id: u32, policy: Policy, materials: &mut Assets<Ue3Material>, programs: &mut Ue3Programs, shaders: &mut Assets<Shader>) -> Option<Handle<Ue3Material>> {
        let ue = self.defs.get(id as usize)?.as_ref()?;
        let mut m = self.ue_base.clone();
        let lit = !ue.unlit && matches!(ue.blend, Blend::Opaque | Blend::Masked) && !matches!(policy, Policy::PointLight | Policy::SpotLight | Policy::PointLightShadowed | Policy::SpotLightShadowed);
        let program = programs.get(ue.map, policy, lit, shaders)?;
        m.key = Ue3Key::new(program, ue.blend, ue.two_sided);
        // opaque and masked surfaces take UE3's depth-only shaders in the depth and shadow
        // passes (a dynamic light's pass is never drawn in them)
        if matches!(policy, Policy::NoLightMap | Policy::DirectionalLightMap) {
            let masked = match ue.blend {
                Blend::Opaque => Some(false),
                Blend::Masked => Some(true),
                _ => None,
            };
            if let Some(d) = masked.and_then(|masked| programs.get_depth(ue.map, policy, masked, shaders)) {
                m.key.depth = d;
            }
        }
        m.map = ue.map;
        m.mat_id = id;
        m.light_ok = !ue.unlit && matches!(ue.blend, Blend::Opaque | Blend::Masked) && ue.params.len() <= DYN_SHADOW_SLOT;
        let mut params = Ue3Params::default();
        for (i, v) in ue.params.iter().take(params.p.len()).enumerate() {
            params.p[i] = Vec4::from(*v);
        }
        m.params = params;
        for (i, t) in ue.textures.iter().enumerate() {
            if *t == Some(UE3_TEX_BLACK) {
                m.set_texture(i, self.black.clone());
                continue;
            }
            if let Some(h) = t.and_then(|t| self.image(t)) {
                m.set_texture(i, h);
            }
        }
        if let Some(Some(c)) = ue.cube_textures.first() {
            if let Some(h) = self.cubes.get(c) {
                m.cube0 = h.clone();
            }
        }
        Some(materials.add(m))
    }
}

/// A 1x1 black cube map.
fn black_cube_image() -> Image {
    let mut img = Image::new_fill(Extent3d { width: 1, height: 1, depth_or_array_layers: 6 }, TextureDimension::D2, &[0, 0, 0, 255], WgpuFormat::Rgba8Unorm, RenderAssetUsages::RENDER_WORLD);
    img.texture_view_descriptor = Some(TextureViewDescriptor { dimension: Some(TextureViewDimension::Cube), ..default() });
    img
}
