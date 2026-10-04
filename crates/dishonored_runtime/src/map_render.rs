//! Turn a [`DishonoredMap`] into Bevy meshes, images, materials and entities.

use crate::coords;
use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat as WgpuFormat};
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
    let mut img = Image::new(
        Extent3d {
            width: t.width,
            height: t.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
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

pub fn lightmap_image(l: &LightmapAtlas) -> Image {
    let data: Vec<u8> = l.texels.iter().flat_map(|t| t.to_le_bytes()).collect();
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
    let faces = &map.environment.cubemap_faces;
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
    Some(bevy::light::GeneratedEnvironmentMapLight {
        environment_map: images.add(img),
        intensity: light_unit(),
        rotation: Quat::IDENTITY,
        affects_lightmapped_mesh_diffuse: false,
    })
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
/// - UE3 Phong `SP_Specular_Power` n -> GGX via alpha = sqrt(2 / (n + 2)).
/// - `S_` map -> specular tint texture, `S_Specular_color` rgb * a -> tint * reflectance.
/// - Colour vectors use Arkane's rgb * a intensity convention for emissive/specular; the
///   alpha of `D_Diffuse_color` is ignored (its meaning is unknown).
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
        let a = if p.unlit && p.blend != MaterialBlend::Opaque { c[3].clamp(0.0, 1.0) } else { 1.0 };
        if !f.debug_lighting {
            mat.base_color = Color::linear_rgba(c[0], c[1], c[2], a);
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
        let c = vector(p, "S_Specular_color").unwrap_or([1.0; 4]);
        mat.specular_tint_texture = Some(images[s].clone());
        mat.specular_tint = Color::linear_rgb(c[0], c[1], c[2]);
        mat.reflectance = (0.5 * c[3]).clamp(0.0, 1.0);
        let n = scalar(p, "SP_Specular_Power").unwrap_or(20.0).max(1.0);
        mat.perceptual_roughness = (2.0 / (n + 2.0)).sqrt().sqrt().clamp(0.089, 1.0);
    } else {
        mat.reflectance = 0.1;
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
    let lightmap_handles: Vec<Handle<Image>> = if features.lightmaps {
        map.lightmaps.iter().map(|l| images.add(lightmap_image(l))).collect()
    } else {
        Vec::new()
    };
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

    // One Bevy mesh per (static mesh, section).
    let section_meshes: Vec<Vec<Handle<Mesh>>> = map
        .meshes
        .iter()
        .map(|m| {
            let positions: Vec<[f32; 3]> = m.positions.iter().map(|p| coords::pos(*p).to_array()).collect();
            let normals: Vec<[f32; 3]> = m
                .normals
                .iter()
                .map(|n| coords::dir(*n).normalize_or_zero().to_array())
                .collect();
            let uvs: Vec<[f32; 2]> = m.uv0.iter().map(|u| u.to_array()).collect();
            let uv1: Vec<[f32; 2]> = m.uv1.iter().map(|u| u.to_array()).collect();
            m.sections
                .iter()
                .map(|s| {
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
                .collect()
        })
        .collect();

    let mut spawned = 0;
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
                Mesh3d(section_meshes[inst.mesh][si].clone()),
                MeshMaterial3d(mat),
                transform,
                MapGeometry { instance: ii },
            ));
            if let Some(lm) = inst.lightmap.filter(|_| features.lightmaps && mesh.uv1.len() == mesh.positions.len()) {
                e.insert(bevy::pbr::Lightmap {
                    image: lightmap_handles[lm.atlas].clone(),
                    uv_rect: Rect::from_corners(lm.uv_min, lm.uv_max),
                    bicubic_sampling: false,
                });
            }
            spawned += 1;
        }
    }
    spawned
}
