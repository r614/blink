//! Turn a [`DishonoredMap`] into Bevy meshes, images, materials and entities.

use crate::coords;
use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat as WgpuFormat};
use dishonored_ir::{BlendHint, DishonoredMap, TextureAsset, TextureFormat};

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
        (TextureFormat::Rg8Snorm, _) => WgpuFormat::Rg8Snorm,
    };
    let data: Vec<u8> = t.mips.concat();
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
    let material_handles: Vec<Handle<StandardMaterial>> = map
        .materials
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let base_color_texture = m.diffuse.map(|t| image_handles[t].clone());
            materials.add(StandardMaterial {
                base_color: if base_color_texture.is_some() {
                    Color::WHITE
                } else {
                    fallback_color(i)
                },
                base_color_texture,
                perceptual_roughness: 0.85,
                metallic: 0.0,
                cull_mode: None,
                double_sided: true,
                alpha_mode: match m.blend {
                    BlendHint::Opaque => AlphaMode::Opaque,
                    BlendHint::Masked => AlphaMode::Mask(0.33),
                    BlendHint::Translucent => AlphaMode::Blend,
                },
                ..default()
            })
        })
        .collect();
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
            m.sections
                .iter()
                .map(|s| {
                    let start = s.first_index as usize;
                    let end = (start + s.num_triangles as usize * 3).min(m.indices.len());
                    let idx = m.indices[start..end].to_vec();
                    let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone())
                        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals.clone())
                        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs.clone())
                        .with_inserted_indices(Indices::U32(idx));
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
            commands.spawn((
                Mesh3d(section_meshes[inst.mesh][si].clone()),
                MeshMaterial3d(mat),
                transform,
                MapGeometry { instance: ii },
            ));
            spawned += 1;
        }
    }
    spawned
}
