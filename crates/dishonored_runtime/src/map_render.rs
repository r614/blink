//! Turn a [`DishonoredMap`] into Bevy meshes, images, materials and entities.

use crate::coords;
use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, TextureDimension, TextureFormat as WgpuFormat};
use dishonored_ir::{BlendHint, DishonoredMap, LightmapAtlas, MaterialAsset, MaterialBlend, MaterialParams, TextureAsset, TextureFormat};

#[derive(Component)]
pub struct MapGeometry {
    pub instance: usize,
}

/// Per-feature switches for the visual-fidelity work, so every feature can be compared
/// on/off with otherwise identical frames (`harness fidelity`).
#[derive(Resource, Clone, Debug)]
pub struct RenderFeatures {
    /// Baked lightmaps (LightMap2D) on UV1.
    pub lightmaps: bool,
    /// Full material slots (normal, specular, emissive, opacity, cubemap) instead of diffuse only.
    pub materials: bool,
    /// Translucent/additive FX cards (fog sheets, smoke, light shafts).
    pub fx: bool,
    /// Lights from the `_Light` packages (dominant + movable).
    pub dynamic_lights: bool,
    /// Distance/height fog.
    pub fog: bool,
    /// Post stack from the level's post-process settings.
    pub post: bool,
    /// Skydome and distant backdrop cards.
    pub sky: bool,
    /// Debug: white albedo so only lighting is visible (`--debug-lighting`).
    pub debug_lighting: bool,
    /// View-dependent specular from the directional lightmap (the original base pass's
    /// lightmap specular term).
    pub lightmap_specular: bool,
    /// The dominant directional light's distance-field shadow maps on lightmapped static
    /// geometry instead of Bevy's cascades (`--no-df-shadows` turns them off).
    pub df_shadows: bool,
}

impl RenderFeatures {
    pub fn from_flags(has: &dyn Fn(&str) -> bool) -> Self {
        let base = has("--baseline");
        let on = |f: &str| !base && !has(f);
        Self {
            lightmaps: on("--no-lightmaps"),
            materials: on("--no-materials"),
            fx: on("--no-fx"),
            dynamic_lights: on("--no-dynamic-lights"),
            fog: on("--no-fog"),
            post: on("--no-post"),
            sky: on("--no-sky"),
            debug_lighting: has("--debug-lighting"),
            // Full strength: the base pass's `SpecularOverrideParameter` is (0, 0, 0, 1) in the
            // running game (measured, U-320). `--no-lightmap-specular` turns it off.
            lightmap_specular: on("--no-lightmap-specular"),
            df_shadows: on("--no-df-shadows"),
        }
    }
}

pub fn to_image(t: &TextureAsset) -> Image {
    let format = match (t.format, t.srgb) {
        (TextureFormat::Bc1, true) => WgpuFormat::Bc1RgbaUnormSrgb,
        (TextureFormat::Bc1, false) => WgpuFormat::Bc1RgbaUnorm,
        (TextureFormat::Bc2, true) => WgpuFormat::Bc2RgbaUnormSrgb,
        (TextureFormat::Bc2, false) => WgpuFormat::Bc2RgbaUnorm,
        (TextureFormat::Bc3, true) => WgpuFormat::Bc3RgbaUnormSrgb,
        (TextureFormat::Bc3, false) => WgpuFormat::Bc3RgbaUnorm,
        (TextureFormat::Bc5, _) => WgpuFormat::Bc5RgUnorm,
        (TextureFormat::Bgra8, true) => WgpuFormat::Bgra8UnormSrgb,
        (TextureFormat::Bgra8, false) => WgpuFormat::Bgra8Unorm,
        (TextureFormat::R8, _) => WgpuFormat::R8Unorm,
        // Converted to unorm below so Bevy recognises a two-channel normal map.
        (TextureFormat::Rg8Snorm, _) => WgpuFormat::Rg8Unorm,
    };
    let mut data: Vec<u8> = t.mips.concat();
    if t.format == TextureFormat::Rg8Snorm {
        for b in data.iter_mut() {
            *b = (*b as i8 as i16 + 128) as u8;
        }
    }
    // `Image::new` debug-asserts that the data is exactly mip 0, so the chain is set afterwards
    // (debug builds panicked here on every mipmapped texture).
    let mut img = Image::new_uninit(
        Extent3d {
            width: t.width,
            height: t.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.data = Some(data);
    img.texture_descriptor.mip_level_count = t.mips.len() as u32;
    img.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::Repeat,
        address_mode_v: bevy::image::ImageAddressMode::Repeat,
        mag_filter: bevy::image::ImageFilterMode::Linear,
        min_filter: bevy::image::ImageFilterMode::Linear,
        mipmap_filter: bevy::image::ImageFilterMode::Linear,
        anisotropy_clamp: 8,
        ..Default::default()
    });
    img
}

/// Multiplier that maps UE3 linear radiance 1.0 to display 1.0 under Bevy's default
/// camera exposure (EV100 9.7). Lightmaps and light intensities are expressed in this unit.
pub fn light_unit() -> f32 {
    1.0 / bevy::camera::Exposure::default().exposure()
}

/// Which plane of a lightmap atlas to upload.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LightmapPlane {
    /// Scaled average colour (the directional shader's `color`).
    Color,
    /// Scaled basis intensities (the directional shader's `dir`).
    Directional,
    /// Irradiance for the unperturbed normal, for Bevy's own per-entity lightmaps.
    Flat,
}

pub fn lightmap_image(l: &LightmapAtlas, plane: LightmapPlane) -> Image {
    use dishonored_assets::lightmap::{directional_irradiance, pack_rgb9e5, unpack_rgb9e5};
    let texels: Vec<u32> = match plane {
        LightmapPlane::Color => l.texels.clone(),
        LightmapPlane::Directional if l.directional.len() == l.texels.len() => l.directional.clone(),
        // Without directional data the colour plane is used with unit intensities.
        LightmapPlane::Directional => vec![pack_rgb9e5([1.0; 3]); l.texels.len()],
        LightmapPlane::Flat if l.directional.len() == l.texels.len() => l
            .texels
            .iter()
            .zip(&l.directional)
            .map(|(&c, &d)| if c == 0 { 0 } else { pack_rgb9e5(directional_irradiance(unpack_rgb9e5(c), unpack_rgb9e5(d), [0.0, 0.0, 1.0])) })
            .collect(),
        LightmapPlane::Flat => l.texels.clone(),
    };
    let data: Vec<u8> = texels.iter().flat_map(|t| t.to_le_bytes()).collect();
    let mut img = Image::new(
        Extent3d { width: l.width, height: l.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        WgpuFormat::Rgb9e5Ufloat,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::ClampToEdge,
        address_mode_v: bevy::image::ImageAddressMode::ClampToEdge,
        mag_filter: bevy::image::ImageFilterMode::Linear,
        min_filter: bevy::image::ImageFilterMode::Linear,
        ..Default::default()
    });
    img
}

/// The level's reflection cubemap (`WorldInfo.mSceneReflection`) as a GPU-filtered
/// environment light for specular reflections (`C_` cubemap slots). Diffuse is left to the
/// lightmaps (`affects_lightmapped_mesh_diffuse: false`).
///
/// APPROXIMATION: UE3 Z-up faces are reassigned to Bevy's Y-up face slots
/// (+X, -X, +Z(UE) -> +Y, -Z(UE) -> -Y, +Y(UE) -> +Z, -Y(UE) -> -Z) without per-face
/// rotation, and reflections apply to every material through Bevy's Fresnel term instead of
/// Arkane's per-material `C_Cubemap_Mask`/`C_Cubemap_color`.
pub fn environment_light(map: &DishonoredMap, images: &mut Assets<Image>) -> Option<bevy::light::GeneratedEnvironmentMapLight> {
    Some(bevy::light::GeneratedEnvironmentMapLight {
        environment_map: images.add(reflection_cube(map)?),
        intensity: light_unit(),
        rotation: Quat::IDENTITY,
        affects_lightmapped_mesh_diffuse: false,
    })
}

/// The level's reflection cubemap as a cube image (same face assignment as [`environment_light`]);
/// also the `SceneReflectionTexture` of the translucent decals (`decals.rs`).
pub fn reflection_cube(map: &DishonoredMap) -> Option<Image> {
    cube_from_faces(map, &map.environment.cubemap_faces)
}

/// A `TextureCube`'s six face textures (+X, -X, +Y, -Y, +Z, -Z in UE3 space) as a cube image,
/// with the face assignment of [`environment_light`].
pub fn cube_from_faces(map: &DishonoredMap, faces: &[dishonored_ir::TextureId]) -> Option<Image> {
    if faces.len() != 6 {
        return None;
    }
    let order = [0usize, 1, 4, 5, 2, 3];
    let mut size = 0;
    let mut data = Vec::new();
    for &f in &order {
        let (w, h, px) = dishonored_assets::texture::decode_rgba8(&map.textures[faces[f]], 0)?;
        if size == 0 {
            size = w;
        }
        if w != size || h != size {
            return None;
        }
        data.extend(px.iter().flat_map(|p| [p[0], p[1], p[2], 255]));
    }
    let mut img = Image::new(
        Extent3d { width: size, height: size, depth_or_array_layers: 6 },
        TextureDimension::D2,
        data,
        WgpuFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_view_descriptor = Some(bevy::render::render_resource::TextureViewDescriptor {
        dimension: Some(bevy::render::render_resource::TextureViewDimension::Cube),
        ..default()
    });
    Some(img)
}


fn scalar(p: &MaterialParams, name: &str) -> Option<f32> {
    p.scalars.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1)
}
fn vector(p: &MaterialParams, name: &str) -> Option<[f32; 4]> {
    p.vectors.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1)
}

/// Map a Dishonored material (Arkane slot convention) onto Bevy's StandardMaterial.
///
/// Approximations (documented in research/LOG.md, "Full materials"):
/// - UE3 Phong `SP_Specular_Power` n stored as perceptual roughness (2 / (n + 2))^(1/4), read back by
///   the patched light passes (`lighting::ue3_light`); Bevy GGX only for its environment light.
/// - `S_` map -> specular tint texture, `S_Specular_color` rgb -> tint, reflectance 0.5: S = 2 x reflectance.
/// - Colour vectors use Arkane's rgb * a intensity convention for emissive/specular, and for
///   `D_Diffuse_color` as decoded from the base-pass shaders (`diffuse_color_terms`).
/// - Detail normals (`DN_`) and per-slot tiling other than `D_Diffuse_Tilling` are ignored
///   (Bevy has a single UV transform per material).
pub fn build_material(i: usize, m: &MaterialAsset, p: &MaterialParams, images: &[Handle<Image>], f: &RenderFeatures) -> StandardMaterial {
    let tex = |t: Option<usize>| t.map(|t| images[t].clone());
    let base_color_texture = tex(m.diffuse).filter(|_| !f.debug_lighting);
    let mut mat = StandardMaterial {
        base_color: if base_color_texture.is_some() || f.debug_lighting { Color::WHITE } else { fallback_color(i) },
        base_color_texture,
        perceptual_roughness: 0.85,
        metallic: 0.0,
        reflectance: 0.3,
        cull_mode: None,
        double_sided: true,
        alpha_mode: match m.blend {
            BlendHint::Opaque => AlphaMode::Opaque,
            BlendHint::Masked => AlphaMode::Mask(0.33),
            BlendHint::Translucent => AlphaMode::Blend,
        },
        lightmap_exposure: light_unit(),
        ..default()
    };
    if !f.materials {
        return mat;
    }
    mat.alpha_mode = match p.blend {
        MaterialBlend::Opaque => AlphaMode::Opaque,
        // `O_Enable_full_opacity` compiles the masked base without its opacity mask (no texkill:
        // vertexAnim_PMAT base passes 1815 / 1831 of the Boyle balloons): nothing is clipped.
        MaterialBlend::Masked if p.switch("O_Enable_full_opacity") == Some(true) => AlphaMode::Opaque,
        MaterialBlend::Masked => AlphaMode::Mask(p.opacity_clip.clamp(0.01, 0.99)),
        MaterialBlend::Translucent => AlphaMode::Blend,
        MaterialBlend::Additive => AlphaMode::Add,
        MaterialBlend::Modulate => AlphaMode::Multiply,
    };
    // UE3 culls back faces unless the material is TwoSided. Mirroring Z-up/LH into
    // Y-up/RH turns UE3's clockwise front faces into Bevy's counter-clockwise ones.
    if !p.two_sided {
        mat.cull_mode = Some(bevy::render::render_resource::Face::Back);
        mat.double_sided = false;
    }
    mat.unlit = p.unlit;
    if let Some(c) = vector(p, "D_Diffuse_color").or(vector(p, "D_DiffuseColorTweak")) {
        let (k, a) = diffuse_color_terms(p, c[3]);
        if !f.debug_lighting {
            mat.base_color = Color::linear_rgba(c[0] * k, c[1] * k, c[2] * k, a);
        }
    }
    if let Some(t) = scalar(p, "D_Diffuse_Tilling").filter(|t| *t > 0.0 && (*t - 1.0).abs() > 1e-3) {
        mat.uv_transform = bevy::math::Affine2::from_scale(Vec2::splat(t));
    }
    if let Some(n) = m.normal.filter(|_| !p.unlit) {
        mat.normal_map_texture = Some(images[n].clone());
        // UE3 normal maps are DirectX-style (green = -Y in Bevy's convention).
        mat.flip_normal_map_y = true;
    }
    if let Some(s) = p.specular {
        // The colour multiplies the specular map only when `S_Enable_Specular_color` is on.
        let c = vector(p, "S_Specular_color").filter(|_| p.switch("S_Enable_Specular_color") != Some(false)).unwrap_or([1.0, 1.0, 1.0, 2.0]);
        mat.specular_tint_texture = Some(images[s].clone());
        mat.specular_tint = Color::linear_rgb(c[0], c[1], c[2]);
        // The UE3 specular colour S = texture x tint is 2 x Bevy's reflectance (the patched light
        // passes, lighting.rs `ue3_light`, and the lightmap specular read it back that way).
        mat.reflectance = 0.5;
        let n = scalar(p, "SP_Specular_Power").unwrap_or(20.0).max(1.0);
        mat.perceptual_roughness = (2.0 / (n + 2.0)).sqrt().sqrt().clamp(0.089, 1.0);
    } else {
        // No specular term without `S_Enable_Specular`.
        mat.reflectance = 0.0;
    }
    if p.sky && f.sky {
        // Sky dome, moon and sun: unlit, unfogged (UE3 skies sit beyond the fog's reach),
        // visible from inside. APPROXIMATION of Skies_PMAT (gradient + two scrolling cloud
        // layers + horizon glow): texture * (D_Diffuse_color + mean(gradient top, bottom)).
        mat.unlit = true;
        mat.fog_enabled = false;
        mat.cull_mode = None;
        mat.double_sided = true;
        mat.normal_map_texture = None;
        if let Some(c) = vector(p, "Moon_Color").or(vector(p, "Color")) {
            // Moon_Color.rgb x a; Sun_PMAT ignores Color.a and scales by SunIntensity (sky_disc.rs).
            let k = if vector(p, "Moon_Color").is_some() { c[3] } else { scalar(p, "SunIntensity").unwrap_or(2.0) };
            mat.base_color = Color::linear_rgb(c[0] * k, c[1] * k, c[2] * k);
            mat.alpha_mode = AlphaMode::Add;
        } else {
            let d = vector(p, "D_Diffuse_color").unwrap_or([0.0; 4]);
            let t = vector(p, "G_Gradiant_Top").unwrap_or([0.0; 4]);
            let b = vector(p, "G_Gradiant_Bottom").unwrap_or([0.0; 4]);
            let c = [0, 1, 2].map(|i| d[i] + 0.5 * (t[i] + b[i]));
            mat.base_color = Color::linear_rgb(c[0], c[1], c[2]);
            mat.alpha_mode = AlphaMode::Opaque;
        }
        return mat;
    }
    if let Some(e) = p.emissive {
        let c = vector(p, "E_Emissive_Color").unwrap_or([1.0; 4]);
        mat.emissive_texture = Some(images[e].clone());
        // Bevy does not apply camera exposure to emissive (emissive_exposure_weight 0), so
        // UE3 emissive radiance maps to display values directly.
        mat.emissive = LinearRgba::rgb(c[0] * c[3], c[1] * c[3], c[2] * c[3]);
    }
    mat
}

/// How the alpha `w` of `D_Diffuse_color` enters the diffuse colour: (rgb multiplier, alpha).
///
/// The compiled base-pass pixel shaders of every base material that exposes `D_Diffuse_color`
/// (`generic_PMAT`, `generic_masked_PMAT`, `generic_masked_SingleSided_PMAT`,
/// `generic_translucent_PMAT`, `generic_translucent_2sided_PMAT`, `vertexAnim_PMAT`,
/// `vertexAnim_2Sided_PMAT`, `spline_PMAT`, `bugs_PMAT`, `water_slide_PMAT`, `DefaultMesh_PMAT`,
/// `DefaultMesh_2_Sided_PMAT`, `Default_Simple_Translucent_PMAT`, `Skies_PMAT`) compute
/// `D_Diffuse_color.rgb x D_Diffuse_color.w` (`mul r.xyz, cN, cN.w`): the alpha is an intensity.
/// Only `FogSheet_PMAT` reads the whole vector (`mov r0, cN`); it and undecoded unlit translucent
/// bases keep the alpha as opacity.
pub fn diffuse_color_terms(p: &MaterialParams, w: f32) -> (f32, f32) {
    let base = p.base_material.as_deref().unwrap_or("");
    let decoded = ["generic_", "vertexAnim_", "spline_PMAT", "bugs_PMAT", "water_slide_PMAT", "DefaultMesh_", "Default_Simple_Translucent_PMAT"]
        .iter()
        .any(|b| base.rsplit('.').next().unwrap_or(base).starts_with(b));
    if decoded {
        (w.max(0.0), 1.0)
    } else if p.unlit && p.blend != MaterialBlend::Opaque {
        (1.0, w.clamp(0.0, 1.0))
    } else {
        (1.0, 1.0)
    }
}

/// Deterministic pastel colour for untextured materials so surfaces stay readable.
fn fallback_color(seed: usize) -> Color {
    let h = (seed.wrapping_mul(2654435761) % 360) as f32;
    Color::hsl(h, 0.15, 0.55)
}

pub fn spawn_map(
    map: &DishonoredMap,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    baked: &mut Assets<BakedMaterial>,
    show_hidden: bool,
    features: &RenderFeatures,
) -> usize {
    let show_fx = features.fx;
    let image_handles: Vec<Handle<Image>> = map.textures.iter().map(|t| images.add(to_image(t))).collect();
    let default_params = MaterialParams::default();
    let material_handles: Vec<Handle<StandardMaterial>> = map
        .materials
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let p = map.material_params.get(i).unwrap_or(&default_params);
            materials.add(build_material(i, m, p, &image_handles, features))
        })
        .collect();
    let (lightmap_handles, directional_handles): (Vec<Handle<Image>>, Vec<Handle<Image>>) = if features.lightmaps {
        map.lightmaps
            .iter()
            .map(|l| (images.add(lightmap_image(l, LightmapPlane::Color)), images.add(lightmap_image(l, LightmapPlane::Directional))))
            .unzip()
    } else {
        (Vec::new(), Vec::new())
    };
    let shadow_handles: Vec<Handle<Image>> = if features.lightmaps { map.lightmaps.iter().map(|l| images.add(shadow_image(&l.shadow, l))).collect() } else { Vec::new() };
    let shadow_local_handles: Vec<Handle<Image>> = if features.lightmaps { map.lightmaps.iter().map(|l| images.add(shadow_image(&l.shadow_local, l))).collect() } else { Vec::new() };
    // Flat-normal irradiance pages for geometry drawn with Bevy's own per-entity lightmaps
    // (translucent and unmerged instances), built on first use.
    let mut flat_handles: Vec<Option<Handle<Image>>> = vec![None; map.lightmaps.len()];
    let default_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.6, 0.6, 0.6),
        cull_mode: None,
        double_sided: true,
        perceptual_roughness: 0.9,
        ..default()
    });
    let hidden_material = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.2, 0.8, 0.25),
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        double_sided: true,
        unlit: true,
        ..default()
    });

    // One Bevy mesh per (static mesh, section), built only for geometry drawn per instance
    // (merged batches carry their own vertices).
    let mut section_cache: std::collections::HashMap<(usize, usize), Handle<Mesh>> = std::collections::HashMap::new();
    let mut section_mesh = |mi: usize, si: usize, meshes: &mut Assets<Mesh>| -> Handle<Mesh> {
        section_cache
            .entry((mi, si))
            .or_insert_with(|| {
                let m = &map.meshes[mi];
            let positions: Vec<[f32; 3]> = m.positions.iter().map(|p| coords::pos(*p).to_array()).collect();
            let normals: Vec<[f32; 3]> = m
                .normals
                .iter()
                .map(|n| coords::dir(*n).normalize_or_zero().to_array())
                .collect();
            let uvs: Vec<[f32; 2]> = m.uv0.iter().map(|u| u.to_array()).collect();
            let uv1: Vec<[f32; 2]> = m.uv1.iter().map(|u| u.to_array()).collect();
                let s = &m.sections[si];
                    let start = s.first_index as usize;
                    let end = (start + s.num_triangles as usize * 3).min(m.indices.len());
                    let idx = m.indices[start..end].to_vec();
                    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone())
                        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals.clone())
                        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs.clone())
                        .with_inserted_indices(Indices::U32(idx));
                    if features.materials {
                        // MikkTSpace in Bevy space (equivalent to the packed UE3 basis up to
                        // the handedness flip of the coordinate change).
                        let _ = mesh.generate_tangents();
                    }
                    if m.uv1.len() == m.positions.len() {
                        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, uv1.clone());
                    }
                meshes.add(mesh)
            })
            .clone()
    };

    let merge = std::env::var_os("DISHONORED_NO_MERGE").is_none();
    let mut batches: std::collections::HashMap<BatchKey, Batch> = std::collections::HashMap::new();
    // Lightmap rects of the instances batched alone for their diffuse variation mask.
    let mut d2_rects: std::collections::HashMap<u32, (Vec2, Vec2)> = std::collections::HashMap::new();
    // Bloom parts by material: (emissive texture, bloom colour, world-space triangles).
    let mut bloom_parts: std::collections::BTreeMap<usize, (usize, Vec3, Vec<f32>)> = std::collections::BTreeMap::new();
    let mut spawned = 0;
    // The level's reflection cube for the translucent decals, built on first use.
    let mut decal_scene: Option<Option<Handle<Image>>> = None;
    // Light-beam sections with their vertex colours, by (mesh, section).
    let mut volume_light_meshes: std::collections::HashMap<(usize, usize), Handle<Mesh>> = std::collections::HashMap::new();
    // Material cubemaps by their face textures.
    let mut cube_cache: std::collections::HashMap<Vec<dishonored_ir::TextureId>, Option<Handle<Image>>> = std::collections::HashMap::new();
    for (ii, inst) in map.instances.iter().enumerate() {
        if inst.hidden && !show_hidden {
            continue;
        }
        let mesh = &map.meshes[inst.mesh];
        let transform = Transform::from_matrix(coords::transform(inst.transform));
        for (si, section) in mesh.sections.iter().enumerate() {
            let mat_id = inst.materials.get(si).copied().flatten().or(section.material);
            if !show_fx && mat_id.is_some_and(|m| map.materials[m].blend == BlendHint::Translucent) {
                continue;
            }
            let is_sky = features.sky && mat_id.and_then(|m| map.material_params.get(m)).is_some_and(|p| p.sky);
            if is_sky && std::env::var_os("DISHONORED_SKY_DEBUG").is_some() {
                let c = inst.transform.w_axis;
                let r = mesh.positions.iter().map(|p| inst.transform.transform_point3(glam::Vec3::from(*p)).distance(c.truncate())).fold(0.0f32, f32::max);
                info!("sky instance {} section {si}: centre ({:.0},{:.0},{:.0}) radius {:.0} uu", mesh.source, c.x, c.y, c.z, r);
            }
            let translucent = mat_id.is_some_and(|m| map.materials[m].blend == BlendHint::Translucent);
            // Bloom parts (bloom.rs): the section again, in world space, for its material's
            // bloom colour.
            if !inst.hidden && features.materials && features.post {
                let bloom = mat_id.and_then(|m| Some((m, map.material_params.get(m)?))).and_then(|(m, p)| Some((m, p.emissive?, crate::bloom::bloom_color(p)?)));
                if let Some((m, tex, colour)) = bloom {
                    let entry = bloom_parts.entry(m).or_insert_with(|| (tex, colour, Vec::new()));
                    append_world_triangles(&mut entry.2, mesh, si, coords::transform(inst.transform));
                }
            }
            // Static opaque/masked geometry is merged into world-space batches (one draw per
            // material, lightmap page and cell instead of one per instance section); sorted
            // translucent, sky and debug geometry stays per instance.
            // [interp] Matinee-driven actors (InterpActor) stay separate so cutscenes can move them.
            let mover = crate::cutscene::mover_of(&inst.source, inst.transform);
            if merge && !inst.hidden && !is_sky && !translucent && mover.is_none() {
                let has_uv1 = mesh.uv1.len() == mesh.positions.len();
                let lm = inst.lightmap.filter(|_| features.lightmaps && has_uv1);
                let t = inst.transform.w_axis;
                let key = BatchKey {
                    material: mat_id.map(|m| m as u32).unwrap_or(u32::MAX),
                    atlas: lm.map(|l| l.atlas as u32),
                    cell: ((t.x / BATCH_CELL).floor() as i32, (t.y / BATCH_CELL).floor() as i32),
                    uv1: has_uv1,
                    df: lm.is_some_and(|l| l.df_shadow && features.df_shadows && !map.lightmaps[l.atlas].shadow.is_empty()),
                    df_light: lm
                        .and_then(|l| l.df_light.filter(|_| features.df_shadows && !map.lightmaps[l.atlas].shadow_local.is_empty()))
                        .map(|i| i as u32),
                    d2_inst: lm
                        .filter(|_| features.materials)
                        .and_then(|l| {
                            let m = mat_id?;
                            d2_variation(&map.materials[m], map.material_params.get(m)?).filter(|d| d.lightmap_uv).map(|_| {
                                d2_rects.insert(ii as u32, (l.uv_min, l.uv_max));
                                ii as u32
                            })
                        }),
                };
                let b = batches.entry(key).or_default();
                b.append(mesh, si, coords::transform(inst.transform), lm.map(|l| (l.uv_min, l.uv_max)));
                continue;
            }
            let mat = if inst.hidden {
                hidden_material.clone()
            } else {
                inst.materials
                    .get(si)
                    .copied()
                    .flatten()
                    .or(section.material)
                    .map(|m| material_handles[m].clone())
                    .unwrap_or_else(|| default_material.clone())
            };
            let mut e = commands.spawn((
                Mesh3d(section_mesh(inst.mesh, si, meshes)),
                MeshMaterial3d(mat),
                transform,
                MapGeometry { instance: ii },
            ));
            if is_sky || translucent {
                e.insert(bevy::light::NotShadowCaster);
            }
            // Skies_PMAT domes: the material's own shader (sky.rs) replaces the approximation.
            if is_sky && features.materials && !inst.hidden {
                let sky = mat_id.and_then(|m| Some((map.material_params.get(m)?, map.materials.get(m)?)));
                if let Some(sm) = sky.and_then(|(p, ma)| crate::sky::SkyMaterial::new(p, &ma.texture_params, &image_handles)) {
                    e.insert(crate::sky::PendingSky(sm));
                    // The dome's `Storm` texture is read at texture coordinate 1, which is not
                    // the lightmap set on unlit domes (U-580).
                    let m = &map.meshes[inst.mesh];
                    if m.uv_set1.len() == m.positions.len() {
                        let h = section_mesh(inst.mesh, si, meshes);
                        if let Some(mut sky_mesh) = meshes.get(&h).cloned() {
                            sky_mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, m.uv_set1.iter().map(|u| u.to_array()).collect::<Vec<[f32; 2]>>());
                            e.insert(Mesh3d(meshes.add(sky_mesh)));
                        }
                    }
                } else if let Some(dm) = sky.and_then(|(p, ma)| crate::sky_disc::SkyDiscMaterial::new(p, &ma.texture_params, &image_handles)) {
                    // Moon_PMAT / Sun_PMAT: their own shaders (sky_disc.rs).
                    e.insert(crate::sky_disc::PendingSkyDisc(dm));
                }
            }
            // Light beams (Volume_Light_PMAT): their own shading (volume_light.rs), with the
            // mesh's vertex colours for the `Use_Vertex_Color` permutations.
            let volume_light = (translucent && features.materials && !inst.hidden)
                .then(|| mat_id.and_then(|m| crate::volume_light::VolumeLightMaterial::new(map.material_params.get(m)?, &map.materials[m].texture_params, &image_handles)))
                .flatten();
            if let Some(vl) = volume_light {
                let h = section_mesh(inst.mesh, si, meshes);
                let coloured = volume_light_meshes
                    .entry((inst.mesh, si))
                    .or_insert_with(|| meshes.get(&h).cloned().map(|m| meshes.add(crate::volume_light::with_vertex_colors(m, mesh))).unwrap_or(h))
                    .clone();
                e.insert((Mesh3d(coloured), crate::volume_light::PendingVolumeLight(vl)));
            } else if let Some(sp) = (translucent && features.materials && !inst.hidden)
                .then(|| {
                    // Water sheets (water_spillway_translu_PMAT): their own shading (spillway.rs),
                    // with the material's cubemap and the instance's LocalToWorld.
                    let m = mat_id?;
                    let mut cube = |f: &[dishonored_ir::TextureId]| cube_cache.entry(f.to_vec()).or_insert_with(|| cube_from_faces(map, f).map(|i| images.add(i))).clone();
                    crate::spillway::PendingSpillway::new(map.material_params.get(m)?, &map.materials[m].texture_params, &image_handles, coords::transform(inst.transform), &mut cube)
                })
                .flatten()
            {
                e.insert(sp);
            } else if translucent && features.materials && !inst.hidden {
                // Translucent decals (generic_translucent_PMAT): the material's own shading (decals.rs).
                let scene = decal_scene.get_or_insert_with(|| reflection_cube(map).map(|i| images.add(i))).clone();
                // The component's directional lightmap (the base pass of a lightmapped
                // translucent static mesh is `FDirectionalLightMapTexturePolicy`, decals.rs).
                let lightmap = inst.lightmap.filter(|_| features.lightmaps && !features.debug_lighting && mesh.uv1.len() == mesh.positions.len()).map(|l| {
                    let df = l.df_shadow && features.df_shadows && !map.lightmaps[l.atlas].shadow.is_empty();
                    crate::decals::DecalLightmapRef {
                        color: lightmap_handles[l.atlas].clone(),
                        directional: directional_handles[l.atlas].clone(),
                        shadow: df.then(|| shadow_handles[l.atlas].clone()),
                        uniform: crate::decals::DecalLightmap {
                            params: Vec4::new(light_unit(), 1.0, if df { 1.0 } else { 0.0 }, 0.0),
                            rect: Vec4::new(l.uv_min.x, l.uv_min.y, l.uv_max.x - l.uv_min.x, l.uv_max.y - l.uv_min.y),
                            df: Vec4::new(DF_SHADOW.0, DF_SHADOW.1, DF_SHADOW.2, 0.0),
                        },
                    }
                });
                // Water (river_main_PMAT): its own shading (water.rs).
                if let Some(w) = mat_id.and_then(|m| crate::water::PendingWater::new(map.material_params.get(m)?, &map.materials[m].texture_params, &image_handles, scene.clone(), lightmap.clone())) {
                    e.insert(w);
                } else if let Some(d) = mat_id.and_then(|m| crate::decals::PendingDecal::new(map.material_params.get(m)?, &map.materials[m].texture_params, &image_handles, scene, lightmap)) {
                    e.insert(d);
                }
            }
            if let Some(m) = mover {
                e.insert(m); // [interp]
            }
            if let Some(lm) = inst.lightmap.filter(|_| !is_sky && features.lightmaps && mesh.uv1.len() == mesh.positions.len()) {
                e.insert(bevy::pbr::Lightmap {
                    image: flat_handles[lm.atlas].get_or_insert_with(|| images.add(lightmap_image(&map.lightmaps[lm.atlas], LightmapPlane::Flat))).clone(),
                    uv_rect: Rect::from_corners(lm.uv_min, lm.uv_max),
                    bicubic_sampling: false,
                });
            }
            spawned += 1;
        }
    }
    if !bloom_parts.is_empty() {
        info!("bloom parts: {} materials, {} triangles", bloom_parts.len(), bloom_parts.values().map(|b| b.2.len() / (3 * crate::bloom::VERTEX_FLOATS)).sum::<usize>());
        let batches = bloom_parts
            .into_values()
            .map(|(tex, color, verts)| crate::bloom::BloomBatch { verts: std::sync::Arc::new(verts), texture: image_handles[tex].clone(), color })
            .collect();
        commands.spawn((crate::bloom::BloomParts { batches }, Transform::IDENTITY, Visibility::default(), MapGeometry { instance: usize::MAX }));
    }
    let n_batches = batches.len();
    // Lightmap debugging (`DISHONORED_LIGHTMAP_DEBUG`): `nospec` drops the lightmap specular,
    // `flat` ignores normal maps in the directional reconstruction.
    let lm_debug = match std::env::var("DISHONORED_LIGHTMAP_DEBUG").as_deref() {
        Ok("nospec") => 1.0,
        Ok("flat") => 2.0,
        Ok("lobe") => 3.0,
        Ok("view") => 4.0,
        Ok("normal") => 5.0,
        Ok("diffuse") => 6.0,
        Ok("dir") => 7.0,
        Ok("shadow") => 8.0,
        Ok("shadow_local") => 9.0,
        _ => 0.0,
    };
    let mut baked_cache: std::collections::HashMap<(u32, u32, bool, Option<u32>, Option<u32>), Handle<BakedMaterial>> = std::collections::HashMap::new();
    // Build batch meshes (MikkTSpace tangents dominate) across all cores.
    let built: Vec<(BatchKey, Mesh)> = {
        let jobs: Vec<(BatchKey, Batch)> = batches.into_iter().filter(|(_, b)| !b.indices.is_empty()).collect();
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        let chunk = jobs.len().div_ceil(threads).max(1);
        let tangents = features.materials;
        let mut jobs = jobs;
        let mut parts: Vec<Vec<(BatchKey, Batch)>> = Vec::new();
        while !jobs.is_empty() {
            let rest = jobs.split_off(chunk.min(jobs.len()));
            parts.push(std::mem::replace(&mut jobs, rest));
        }
        std::thread::scope(|s| {
            let handles: Vec<_> = parts
                .into_iter()
                .map(|part| {
                    s.spawn(move || {
                        part.into_iter()
                            .map(|(key, b)| {
                                let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                                    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, b.positions)
                                    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, b.normals)
                                    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, b.uv0)
                                    .with_inserted_indices(Indices::U32(b.indices));
                                if key.uv1 {
                                    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, b.uv1);
                                }
                                if tangents {
                                    let _ = mesh.generate_tangents();
                                }
                                (key, mesh)
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        })
    };
    for (key, mesh) in built {
        let mat = if key.material == u32::MAX { default_material.clone() } else { material_handles[key.material as usize].clone() };
        let mut e = commands.spawn((Mesh3d(meshes.add(mesh)), Transform::IDENTITY, MapGeometry { instance: usize::MAX }));
        match key.atlas {
            // Lightmap regions are already folded into UV1: one shared material per
            // (material, atlas page).
            Some(atlas) => {
                let h = baked_cache.entry((key.material, atlas, key.df, key.df_light, key.d2_inst)).or_insert_with(|| {
                    let base = materials.get(&mat).cloned().unwrap_or_default();
                    // Diffuse variation (generic_PMAT D2_): texture, mask and their parameters.
                    let d2 = (key.material != u32::MAX && features.materials && !features.debug_lighting)
                        .then(|| d2_variation(&map.materials[key.material as usize], map.material_params.get(key.material as usize)?))
                        .flatten();
                    let rect = key.d2_inst.and_then(|i| d2_rects.get(&i)).map(|(a, b)| Vec4::new(a.x, a.y, 1.0 / (b.x - a.x).max(1e-9), 1.0 / (b.y - a.y).max(1e-9))).unwrap_or(Vec4::new(0.0, 0.0, 1.0, 1.0));
                    let exposure = base.lightmap_exposure;
                    // Lightmap specular: colour = reflectance * 2 (see lightmap_ext.wgsl),
                    // lobe exponent SP_Specular_Power + 1.
                    let p = map.material_params.get(key.material as usize).filter(|p| features.materials && features.lightmap_specular && p.specular.is_some());
                    let spec_scale = p.map(|_| 2.0 / (2.0 * base.reflectance).max(1e-3)).unwrap_or(0.0);
                    let power = p.and_then(|p| scalar(p, "SP_Specular_Power")).unwrap_or(15.0).max(0.0) + 1.0;
                    baked.add(BakedMaterial {
                        base,
                        extension: BakedLightmap {
                            params: Vec4::new(exposure, if lm_debug == 1.0 { 0.0 } else { spec_scale }, power, lm_debug),
                            lightmap: lightmap_handles[atlas as usize].clone(),
                            directional: directional_handles[atlas as usize].clone(),
                            shadow: shadow_handles[atlas as usize].clone(),
                            shadow_params: Vec4::new(if key.df { 1.0 } else { 0.0 }, DF_SHADOW.0, DF_SHADOW.1, DF_SHADOW.2),
                            shadow_local: shadow_local_handles[atlas as usize].clone(),
                            local_light: key
                                .df_light
                                .and_then(|i| map.lights.get(i as usize))
                                .map(|l| coords::pos(l.position).extend(DF_SHADOW_LOCAL_SCALE))
                                .unwrap_or(Vec4::ZERO),
                            d2_params: d2.as_ref().map(|d| Vec4::new(1.0, d.power, d.tiling, if d.lightmap_uv { 1.0 } else { 0.0 })).unwrap_or(Vec4::ZERO),
                            d2_rect: rect,
                            d2_channel: d2.as_ref().map(|d| Vec4::from_array(std::array::from_fn(|k| if k == d.channel as usize { 1.0 } else { 0.0 }))).unwrap_or(Vec4::ZERO),
                            d2_diffuse: d2.as_ref().map(|d| image_handles[d.diffuse].clone()),
                            d2_mask: d2.as_ref().map(|d| image_handles[d.mask].clone()),
                            v2_params: d2
                                .as_ref()
                                .map(|d| {
                                    let (n_on, n_t) = d.n2.map(|n| (1.0, n.2)).unwrap_or((0.0, 1.0));
                                    // The specular variation feeds the lightmap specular only when it is drawn.
                                    let (s_on, s_t) = d.s2.filter(|_| spec_scale > 0.0).map(|s| (1.0, s.1)).unwrap_or((0.0, 1.0));
                                    Vec4::new(n_on, n_t, s_on, s_t)
                                })
                                .unwrap_or(Vec4::ZERO),
                            s2_channel: d2
                                .as_ref()
                                .and_then(|d| d.s2)
                                .and_then(|s| s.2)
                                .filter(|c| *c < 4)
                                .map(|c| Vec4::from_array(std::array::from_fn(|k| if k == c as usize || k == 3 { 1.0 } else { 0.0 })))
                                .unwrap_or(Vec4::ZERO),
                            v2_normal: d2.as_ref().and_then(|d| d.n2).map(|n| image_handles[n.0].clone()),
                            n2: d2.as_ref().and_then(|d| d.n2).map(|n| image_handles[n.1].clone()),
                            s2: d2.as_ref().and_then(|d| d.s2).map(|s| image_handles[s.0].clone()),
                        },
                    })
                });
                e.insert(MeshMaterial3d(h.clone()));
            }
            None => {
                e.insert(MeshMaterial3d(mat));
            }
        }
        spawned += 1;
    }
    if merge {
        info!("static geometry merged into {n_batches} batches");
    }
    spawned
}

/// Batch cell size (Dishonored units): coarse enough to cut draw calls, fine enough to keep
/// frustum and shadow-cascade culling useful.
const BATCH_CELL: f32 = 4096.0;

#[derive(Hash, PartialEq, Eq, Clone, Copy)]
struct BatchKey {
    material: u32,
    atlas: Option<u32>,
    /// Distance-field shadowed by the dominant directional light.
    df: bool,
    /// The dominant point / spot light (index in `DishonoredMap::lights`) whose distance field
    /// shadows the batch (U-403).
    df_light: Option<u32>,
    cell: (i32, i32),
    uv1: bool,
    /// The instance, for materials whose diffuse variation mask is read through the
    /// component's own lightmap UVs (`D2Variation::lightmap_uv`).
    d2_inst: Option<u32>,
}

/// generic_PMAT's diffuse variation (`D2_Enable_Diffuse_Variation`), decoded from the base pass
/// shaders (e.g. 15243 with the mask on UV0, 28053 with `D2_Use_UV_coordinate_Index_1`):
/// `diffuse = lerp(D_Diffuse_color.rgb x a x D, D2(UV0 x D2_Diffuse_variation_Tilling),
/// D2_Diffuse_variation_mask(uv).channel x D2_Variation_Mask_Power)`, not saturated; the mask's
/// channel is the static mask `D2_Diffuse_Variation_mask_channel`. The same mask value lerps the
/// normal map to `N2_NormalMap_variation_texture` (UV0 x `N2_NormalMap_variation_Tilling`,
/// normalised after the lerp) with `N2_Enable_NormalMap_Variation`, and the specular texture to
/// `S2_specular_variation_texture` (UV0 x `S2_Specular_variation_Tilling`, the static mask
/// `S2_Specular_variation_Mask_Channel`) with `S2_Enable_Specular_Variation` (28116, the gravel
/// `ext_boyle_ground_01.gravel_02_garden_inst`). Variants with `D2_Enable_Multiply_for_variation`,
/// `D2_Enable_Diffuse_variation_color`, `S2_Enable_Specular_variation_color` or
/// `D2_Use_UV_coordinate_Index_2` are not decoded (U-583).
struct D2Variation {
    diffuse: usize,
    mask: usize,
    power: f32,
    tiling: f32,
    /// Mask on texture coordinate 1, the meshes' lightmap channel (`LightMapCoordinateIndex` 1).
    lightmap_uv: bool,
    channel: u8,
    /// The base normal map and the normal variation (texture, tiling).
    n2: Option<(usize, usize, f32)>,
    /// The specular variation (texture, tiling, single mask channel or RGB).
    s2: Option<(usize, f32, Option<u8>)>,
}

fn d2_variation(m: &MaterialAsset, p: &MaterialParams) -> Option<D2Variation> {
    if p.switch("D2_Enable_Diffuse_Variation") != Some(true)
        || p.switch("D2_Enable_Multiply_for_variation") == Some(true)
        || p.switch("D2_Enable_Diffuse_variation_color") == Some(true)
        || p.switch("D2_Use_UV_coordinate_Index_2") == Some(true)
    {
        return None;
    }
    let tex = |n: &str| m.texture_params.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|t| t.1);
    Some(D2Variation {
        diffuse: tex("D2_diffuse_variation_texture")?,
        mask: tex("D2_Diffuse_variation_mask")?,
        power: scalar(p, "D2_Variation_Mask_Power").unwrap_or(1.0),
        tiling: scalar(p, "D2_Diffuse_variation_Tilling").unwrap_or(1.0),
        lightmap_uv: p.switch("D2_Use_UV_coordinate_Index_1") == Some(true),
        channel: p.mask_channel("D2_Diffuse_Variation_mask_channel").unwrap_or(0),
        n2: (p.switch("N2_Enable_NormalMap_Variation") == Some(true) && p.switch("N_Enable_NormalMap") != Some(false))
            .then(|| Some((m.normal.or(tex("N_NormalMap_texture"))?, tex("N2_NormalMap_variation_texture")?, scalar(p, "N2_NormalMap_variation_Tilling").unwrap_or(1.0))))
            .flatten(),
        s2: (p.switch("S2_Enable_Specular_Variation") == Some(true) && p.switch("S2_Enable_Specular_variation_color") != Some(true) && p.specular.is_some())
            .then(|| Some((tex("S2_specular_variation_texture")?, scalar(p, "S2_Specular_variation_Tilling").unwrap_or(1.0), p.mask_channel("S2_Specular_variation_Mask_Channel"))))
            .flatten(),
    })
}

#[derive(Default)]
struct Batch {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uv0: Vec<[f32; 2]>,
    uv1: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl Batch {
    /// Append one section of a static mesh transformed to Bevy world space (`m`), with its
    /// lightmap region (if any) baked into UV1.
    fn append(&mut self, mesh: &dishonored_ir::StaticMeshAsset, section: usize, m: Mat4, lightmap: Option<(Vec2, Vec2)>) {
        let s = &mesh.sections[section];
        let start = s.first_index as usize;
        let end = (start + s.num_triangles as usize * 3).min(mesh.indices.len());
        let normal_m = Mat3::from_mat4(m).inverse().transpose();
        let flip = m.determinant() < 0.0;
        let mut remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut tri = [0u32; 3];
        for (k, &vi) in mesh.indices[start..end].iter().enumerate() {
            let out = *remap.entry(vi).or_insert_with(|| {
                let v = vi as usize;
                let p = m.transform_point3(coords::pos(mesh.positions[v]));
                let n = mesh.normals.get(v).map(|n| (normal_m * coords::dir(*n)).normalize_or_zero()).unwrap_or(Vec3::Y);
                self.positions.push(p.to_array());
                self.normals.push(n.to_array());
                self.uv0.push(mesh.uv0.get(v).map(|u| u.to_array()).unwrap_or([0.0, 0.0]));
                let u1 = mesh.uv1.get(v).copied().unwrap_or(Vec2::ZERO);
                let u1 = match lightmap {
                    Some((lo, hi)) => lo + u1 * (hi - lo),
                    None => u1,
                };
                self.uv1.push(u1.to_array());
                (self.positions.len() - 1) as u32
            });
            tri[k % 3] = out;
            if k % 3 == 2 {
                if flip {
                    self.indices.extend_from_slice(&[tri[0], tri[2], tri[1]]);
                } else {
                    self.indices.extend_from_slice(&tri);
                }
            }
        }
    }
}

/// Append one section of a static mesh as world-space (Bevy) triangles: position, UV0 per vertex
/// (`bloom::VERTEX_FLOATS`).
fn append_world_triangles(out: &mut Vec<f32>, mesh: &dishonored_ir::StaticMeshAsset, section: usize, m: Mat4) {
    let s = &mesh.sections[section];
    let start = s.first_index as usize;
    let end = (start + s.num_triangles as usize * 3).min(mesh.indices.len());
    for &vi in &mesh.indices[start..end] {
        let v = vi as usize;
        let Some(p) = mesh.positions.get(v) else { continue };
        let p = m.transform_point3(coords::pos(*p));
        let uv = mesh.uv0.get(v).copied().unwrap_or(Vec2::ZERO);
        out.extend_from_slice(&[p.x, p.y, p.z, uv.x, uv.y]);
    }
}

/// Baked lightmap as a material extension (see `lightmap_ext.wgsl`): one bind group per
/// material instead of Bevy's per-entity lightmap slots, which cost ~4 ms/frame of
/// `prepare_mesh_bind_groups` on Boyle and prevented batching.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct BakedLightmap {
    /// x = exposure (UE3 lightmap scale to Bevy units).
    #[uniform(100)]
    pub params: Vec4,
    #[texture(101)]
    #[sampler(102)]
    pub lightmap: Handle<Image>,
    /// Basis intensities of the directional lightmap (same layout as `lightmap`).
    #[texture(103)]
    pub directional: Handle<Image>,
    /// Dominant directional light distance field (same layout; `LightmapAtlas::shadow`).
    #[texture(104)]
    pub shadow: Handle<Image>,
    /// (on, bias, scale, exponent) of the distance-field shadow, see `DF_SHADOW`.
    #[uniform(105)]
    pub shadow_params: Vec4,
    /// The batch's dominant point / spot light distance field (`LightmapAtlas::shadow_local`).
    #[texture(106)]
    pub shadow_local: Handle<Image>,
    /// That light's Bevy world position; w = its distance-field scale (0: the batch has none).
    #[uniform(107)]
    pub local_light: Vec4,
    /// Diffuse variation (`D2Variation`): (on, mask power, tiling, mask on the lightmap UV).
    #[uniform(108)]
    pub d2_params: Vec4,
    /// The component's lightmap rect: (uv_min, 1 / size), mapping atlas UV back to its UV1.
    #[uniform(109)]
    pub d2_rect: Vec4,
    /// One-hot mask channel.
    #[uniform(110)]
    pub d2_channel: Vec4,
    #[texture(111)]
    #[sampler(112)]
    pub d2_diffuse: Option<Handle<Image>>,
    #[texture(113)]
    pub d2_mask: Option<Handle<Image>>,
    /// Normal and specular variations: (N2 on, N2 tiling, S2 on, S2 tiling).
    #[uniform(114)]
    pub v2_params: Vec4,
    /// One-hot S2 mask channel; w = 1 when a single channel is used (else RGB).
    #[uniform(115)]
    pub s2_channel: Vec4,
    /// The base normal map (lerped with N2 in tangent space).
    #[texture(116)]
    pub v2_normal: Option<Handle<Image>>,
    #[texture(117)]
    pub n2: Option<Handle<Image>>,
    #[texture(118)]
    pub s2: Option<Handle<Image>>,
}

/// `DistanceFieldParameters` of the original's distance-field light shaders
/// (`saturate((d + bias) * scale)^exponent`), measured in the running game for Boyle Ext's moon
/// (engine hook, research/dishonored/INSTRUMENTATION.md; U-401, U-460): (-0.36667, 3.75, 2.0).
/// The bias is always -0.5 + 0.5 / scale: a ramp of width 1 / scale about the field's 0.5 edge
/// (Lightmass stores d = 0.5 + distance / (2 x `MaxTransitionDistanceWorldSpace` 50 uu)).
pub const DF_SHADOW: (f32, f32, f32) = (-0.5 + 0.5 / 3.75, 3.75, 2.0);
/// Scale measured for the bluish (dominant) spot lights' distance-field shadows; warm spot lights
/// drew with 100 (which light gets which is U-461). Same bias rule, exponent 2.
pub const DF_SHADOW_LOCAL_SCALE: f32 = 3.125;

/// The shadow plane of an atlas as a linear R8 image (white when it has none).
pub fn shadow_image(plane: &[u8], l: &LightmapAtlas) -> Image {
    let (w, h, data) = if plane.len() == (l.width * l.height) as usize { (l.width, l.height, plane.to_vec()) } else { (1, 1, vec![255]) };
    let mut img = Image::new(Extent3d { width: w, height: h, depth_or_array_layers: 1 }, TextureDimension::D2, data, WgpuFormat::R8Unorm, RenderAssetUsages::RENDER_WORLD);
    img.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::ClampToEdge,
        address_mode_v: bevy::image::ImageAddressMode::ClampToEdge,
        mag_filter: bevy::image::ImageFilterMode::Linear,
        min_filter: bevy::image::ImageFilterMode::Linear,
        ..Default::default()
    });
    img
}

impl bevy::pbr::MaterialExtension for BakedLightmap {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/lightmap_ext.wgsl".into()
    }
}

pub type BakedMaterial = bevy::pbr::ExtendedMaterial<StandardMaterial, BakedLightmap>;
