//! Light beams of `Vfx_Materials.main_materials.Volume_Light_PMAT`: the window shafts (Pub,
//! Art Dealer, Prison sewer), lamp cones and the outro's god rays, shaded the way the material's
//! compiled base passes do (`volume_light.wgsl`; the maths is in research/dishonored/RENDERING.md
//! "Light beams").
//!
//! Shader maps of the base (RefShaderCache-PC-D3D-SM3.upk, `shader_maps ... Volume_Light_PMAT`),
//! FLocalVertexFactory base passes (`FNoLightMapPolicy`, vertex shader 30 / 3): 6627 (no static
//! switch), 6629 (`Disable_CameraFade`), 6631 (`Use_Vertex_Color`), 6633 (`Use_Vertex_Color` +
//! `Use_rays`; the `Rays_Channel` mask does not change the code, it reads `.r`). The particle
//! vertex factory's 6626 / 6628 / 6630 / 6632 compute the same.
//!
//! Before, the beams drew as Bevy's unlit additive `StandardMaterial` in the untextured fallback
//! colour at full strength: solid pale cones over the Pub's bar, the Art Dealer's hall and the
//! whole view at L_Out_P's start. The original fades them by the facing (|N.V|^Fresnel_exponent),
//! the UV sphere mask or the mesh's vertex colour (white at the window, black at the beam's end),
//! the scene depth and the camera distance (over 2000 x `Distance` uu past `StartOffset`).

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::{MaterialParams, TextureId};

/// The base material this module renders.
pub const VOLUME_LIGHT_PMAT: &str = "Vfx_Materials.main_materials.Volume_Light_PMAT";

/// The rays texture of `Use_rays` (`Texture2D_0` of 6633).
pub const RAYS_TEXTURE: &str = "LightRays_Mask_R";

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct VolumeLightUniform {
    /// rgb: lerp(Color, ColorAlert, Alert) x LightIntensity (Color's alpha is unused);
    /// w: UE3 radiance to Bevy light units (`map_render::light_unit`).
    pub color: Vec4,
    /// `MeshEmitterVertexColor`: (1, 1, 1, 1) on static meshes, the particle colour on mesh
    /// emitters. Its alpha multiplies the opacity twice.
    pub mesh_color: Vec4,
    /// (Fresnel_exponent, SphereMask_Power, depth-biased alpha scale k, StartOffset uu).
    pub terms: Vec4,
    /// (camera fade rate 0.0005 / Distance, or -1 with `Disable_CameraFade`; 1 = Use_Vertex_Color;
    /// 1 = Use_rays; Ray_Speed).
    pub flags: Vec4,
    /// x: 1 = TwoSided (else back faces are dropped: UE3 culls them; front faces are
    /// counter-clockwise after the Z-up to Y-up mirror, as for the StandardMaterials).
    pub sides: Vec4,
}

fn vector(p: &MaterialParams, name: &str, d: [f32; 4]) -> [f32; 4] {
    p.vectors.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}
fn scalar(p: &MaterialParams, name: &str, d: f32) -> f32 {
    p.scalars.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}

/// Whether a material is drawn by this module.
pub fn is_volume_light(p: &MaterialParams) -> bool {
    p.base_material.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(VOLUME_LIGHT_PMAT))
}

impl VolumeLightUniform {
    /// The uniform of a Volume_Light_PMAT material (None for other bases). Defaults are the base
    /// material's parameter defaults (the shader map's uniform expressions: Color (1, 0.9406,
    /// 0.7445), ColorAlert (2, 0, 0.05), Alert 0, LightIntensity 2, SphereMask_Power 2,
    /// Fresnel_exponent 5, Db_Depth_Biased_Alpha_Intensity 0.05, StartOffset 128, Distance 1.5,
    /// Ray_Speed 1).
    pub fn decode(p: &MaterialParams, light_unit: f32) -> Option<VolumeLightUniform> {
        if !is_volume_light(p) {
            return None;
        }
        let c = vector(p, "Color", [1.0, 0.940601, 0.74453, 1.0]);
        let ca = vector(p, "ColorAlert", [2.0, 0.0, 0.05, 1.0]);
        let alert = scalar(p, "Alert", 0.0);
        let k = scalar(p, "LightIntensity", 2.0);
        let colour = Vec3::from_array([0, 1, 2].map(|i| (c[i] + (ca[i] - c[i]) * alert) * k));
        // Depth-biased alpha: 1 / ((1 - intensity) x 50), 1000 when that product is below 0.001.
        let b = (1.0 - scalar(p, "Db_Depth_Biased_Alpha_Intensity", 0.05)) * 50.0;
        let db = if b - 0.001 >= 0.0 { 1.0 / b } else { 1000.0 };
        let on = |n: &str| p.switch(n) == Some(true);
        let fade = if on("Disable_CameraFade") { -1.0 } else { 0.0005 / scalar(p, "Distance", 1.5).max(1e-6) };
        Some(VolumeLightUniform {
            color: colour.extend(light_unit),
            mesh_color: Vec4::ONE,
            terms: Vec4::new(scalar(p, "Fresnel_exponent", 5.0), scalar(p, "SphereMask_Power", 2.0), db, scalar(p, "StartOffset", 128.0)),
            flags: Vec4::new(fade, if on("Use_Vertex_Color") { 1.0 } else { 0.0 }, if on("Use_rays") { 1.0 } else { 0.0 }, scalar(p, "Ray_Speed", 1.0)),
            sides: Vec4::new(if p.two_sided { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0),
        })
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct VolumeLightMaterial {
    #[uniform(0)]
    pub params: VolumeLightUniform,
    /// `LightRays_Mask_R` (only sampled with Use_rays).
    #[texture(1)]
    #[sampler(2)]
    pub rays: Option<Handle<Image>>,
}

impl Material for VolumeLightMaterial {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/volume_light.wgsl".into()
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
        // No culling here: the shader drops the back faces of single-sided materials
        // (`VolumeLightUniform::sides`), so one pipeline serves both.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

impl VolumeLightMaterial {
    /// The material of a Volume_Light_PMAT section (None for other bases).
    pub fn new(p: &MaterialParams, textures: &[(String, TextureId)], images: &[Handle<Image>]) -> Option<VolumeLightMaterial> {
        let params = VolumeLightUniform::decode(p, crate::map_render::light_unit())?;
        let rays = textures.iter().find(|(k, _)| k.eq_ignore_ascii_case(RAYS_TEXTURE)).and_then(|t| images.get(t.1)).cloned();
        if params.flags.z > 0.0 && rays.is_none() {
            warn!("volume light: Use_rays without its {RAYS_TEXTURE} texture; the rays term reads 0");
        }
        Some(VolumeLightMaterial { params, rays })
    }
}

/// A beam section waiting for its material (spawned with a StandardMaterial by
/// `map_render::spawn_map`, swapped on the next update).
#[derive(Component, Clone)]
pub struct PendingVolumeLight(pub VolumeLightMaterial);

/// A static mesh section with its vertex colours (linear byte / 255, as the vertex factory's
/// `D3DCOLOR` stream reads them) for the `Use_Vertex_Color` permutations; white without colours.
pub fn with_vertex_colors(mut mesh: Mesh, m: &dishonored_ir::StaticMeshAsset) -> Mesh {
    let n = m.positions.len();
    let colours: Vec<[f32; 4]> = if m.colors.len() == n { m.colors.iter().map(|c| c.map(|b| b as f32 / 255.0)).collect() } else { vec![[1.0; 4]; n] };
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colours);
    mesh
}

/// A whole static mesh (every section) in Bevy space for a mesh emitter: positions, normals,
/// UV0 and vertex colours, UE3's winding kept as `map_render`'s section meshes keep it (its
/// clockwise front faces are counter-clockwise after the mirror), so the shader's front-face
/// test matches.
pub fn beam_mesh(m: &dishonored_ir::StaticMeshAsset) -> Mesh {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};
    let n = m.positions.len();
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, m.positions.iter().map(|p| crate::coords::pos(*p).to_array()).collect::<Vec<_>>());
    let normals: Vec<[f32; 3]> = if m.normals.len() == n { m.normals.iter().map(|v| crate::coords::dir(*v).normalize_or_zero().to_array()).collect() } else { vec![[0.0, 1.0, 0.0]; n] };
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    let uv: Vec<[f32; 2]> = if m.uv0.len() == n { m.uv0.iter().map(|u| u.to_array()).collect() } else { vec![[0.0; 2]; n] };
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
    mesh.insert_indices(Indices::U32(m.indices.clone()));
    if m.normals.len() != n {
        mesh.compute_smooth_normals();
    }
    with_vertex_colors(mesh, m)
}

pub struct VolumeLightPlugin;

impl Plugin for VolumeLightPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "volume_light.wgsl");
        app.add_plugins(MaterialPlugin::<VolumeLightMaterial>::default()).add_systems(Update, apply_pending_volume_lights);
    }
}

fn apply_pending_volume_lights(mut commands: Commands, q: Query<(Entity, &PendingVolumeLight)>, mut mats: ResMut<Assets<VolumeLightMaterial>>) {
    for (e, p) in &q {
        let h = mats.add(p.0.clone());
        commands
            .entity(e)
            .remove::<(PendingVolumeLight, MeshMaterial3d<StandardMaterial>, bevy::pbr::Lightmap)>()
            .insert((MeshMaterial3d(h), bevy::light::NotShadowReceiver, bevy::light::NotShadowCaster));
    }
}
