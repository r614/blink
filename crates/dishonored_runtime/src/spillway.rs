//! Falling water sheets of `Vfx_Materials.main_materials.water_spillway_translu_PMAT` (the
//! Prison sewer's outflows, the Pub's spillways, the outro's whisky), shaded the way the
//! material's compiled base passes do (`spillway.wgsl`; the maths is in
//! research/dishonored/RENDERING.md "Water sheets").
//!
//! Shader maps of the base (`shader_maps ... water_spillway`): every instance found
//! (`Prison_FX.Effects.Water_sewer_INST`, `Water_sewer_light_INST`, `Water_sewer_blue_INST`,
//! `Hub_FX.Materials.water_spillway_inst`, `Outro_FX.Materials.Bitch_Wisky_INST`) has
//! `Va_Enable_Rotation_About_Axis`, `Vc_Use_Vertex_Color` and `Va_Enable_Complex_vertex_anim`
//! (map 0x60c2373): base passes 1584 (with the vertex fog) / 1585. The vertex-colour and
//! vertex-animation switches only change the vertex shader (wind); `River` (1595 / 1596) changes
//! the opacity. The material has no diffuse texture, so the port drew it as an untextured
//! translucent StandardMaterial: an opaque-looking pale slab (L_PrsnSewer_P's start).
//!
//! Not drawn: the vertex animation (`Va_*`, as the foliage wind, U-570), the distortion pass
//! (`TDistortionMeshPixelShader`, `Distortion`), the fog of the 1584 / 1595 variants.

use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::{MaterialParams, TextureId};

/// The base material this module renders.
pub const WATER_SPILLWAY_PMAT: &str = "Vfx_Materials.main_materials.water_spillway_translu_PMAT";

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct SpillwayUniform {
    /// rgb: D_DiffuseColorTweak.rgb x a; w: UE3 radiance to Bevy light units.
    pub tweak: Vec4,
    /// rgb: C_Cubemap_color.rgb x a; w: C_Cube_Map_Min_Value.
    pub cube: Vec4,
    /// (SpillWay_Tilling, SpillWay_Distortion, SpillWay_Distortion_Speed, SpillWay_Droplets_Speed).
    pub params: Vec4,
    /// (SpillWay_Droplets_Intensity, SpillWay_Translucency, 1 = River, 1 = TwoSided).
    pub params2: Vec4,
    /// The instance's local-to-world 3x3 in Bevy space (columns; scale included).
    pub l2w_x: Vec4,
    pub l2w_y: Vec4,
    pub l2w_z: Vec4,
}

fn vector(p: &MaterialParams, name: &str, d: [f32; 4]) -> [f32; 4] {
    p.vectors.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}
fn scalar(p: &MaterialParams, name: &str, d: f32) -> f32 {
    p.scalars.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name)).map(|v| v.1).unwrap_or(d)
}

impl SpillwayUniform {
    /// The uniform of a water_spillway_translu_PMAT material (None for other bases); defaults are
    /// the base's (the shader map's uniform expressions). `local_to_world` is the instance's
    /// transform in Bevy space (`coords::transform`).
    pub fn decode(p: &MaterialParams, local_to_world: Mat4, light_unit: f32) -> Option<SpillwayUniform> {
        if !p.base_material.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(WATER_SPILLWAY_PMAT)) {
            return None;
        }
        let tw = vector(p, "D_DiffuseColorTweak", [0.635515, 0.687244, 0.717391, 1.0]);
        let cc = vector(p, "C_Cubemap_color", [1.0, 1.0, 1.0, 5.0]);
        // The scale cancels in the basis change of `coords::transform`; the 3x3 keeps the
        // instance's rotation and scale.
        let m = Mat3::from_mat4(local_to_world);
        Some(SpillwayUniform {
            tweak: Vec4::new(tw[0] * tw[3], tw[1] * tw[3], tw[2] * tw[3], light_unit),
            cube: Vec4::new(cc[0] * cc[3], cc[1] * cc[3], cc[2] * cc[3], scalar(p, "C_Cube_Map_Min_Value", 0.0)),
            params: Vec4::new(scalar(p, "SpillWay_Tilling", 1.0), scalar(p, "SpillWay_Distortion", 0.2), scalar(p, "SpillWay_Distortion_Speed", 1.0), scalar(p, "SpillWay_Droplets_Speed", 1.0)),
            params2: Vec4::new(
                scalar(p, "SpillWay_Droplets_Intensity", 1.0),
                scalar(p, "SpillWay_Translucency", 1.0),
                if p.switch("River") == Some(true) { 1.0 } else { 0.0 },
                if p.two_sided { 1.0 } else { 0.0 },
            ),
            l2w_x: m.x_axis.extend(0.0),
            l2w_y: m.y_axis.extend(0.0),
            l2w_z: m.z_axis.extend(0.0),
        })
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct SpillwayMaterial {
    #[uniform(0)]
    pub params: SpillwayUniform,
    /// `Normal`.
    #[texture(1)]
    #[sampler(2)]
    pub normal: Option<Handle<Image>>,
    /// `SpillWay_Texture`.
    #[texture(3)]
    pub spill: Option<Handle<Image>>,
    /// The base's expression texture `Tex#2` (droplets, blue channel).
    #[texture(4)]
    pub droplets: Option<Handle<Image>>,
    /// The base's expression texture `Tex#3` (mask, blue channel).
    #[texture(5)]
    pub mask: Option<Handle<Image>>,
    /// `C_Cubemap_texture`.
    #[texture(6, dimension = "cube")]
    #[sampler(7)]
    pub cube: Option<Handle<Image>>,
}

impl Material for SpillwayMaterial {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/spillway.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // The shader drops the back faces of single-sided materials (`params2.w`).
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// A water-sheet section waiting for its material (spawned with a StandardMaterial by
/// `map_render::spawn_map`, swapped on the next update).
#[derive(Component, Clone)]
pub struct PendingSpillway(pub SpillwayMaterial);

impl PendingSpillway {
    /// The material of one instance's section (None for other bases). `cube` builds the
    /// material's cubemap from its six face textures.
    pub fn new(p: &MaterialParams, textures: &[(String, TextureId)], images: &[Handle<Image>], local_to_world: Mat4, cube: &mut dyn FnMut(&[TextureId]) -> Option<Handle<Image>>) -> Option<PendingSpillway> {
        let params = SpillwayUniform::decode(p, local_to_world, crate::map_render::light_unit())?;
        let tex = |n: &str| textures.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).and_then(|t| images.get(t.1)).cloned();
        let faces: Vec<TextureId> = (0..6).filter_map(|i| textures.iter().find(|(k, _)| k.eq_ignore_ascii_case(&format!("C_Cubemap_texture.{i}"))).map(|t| t.1)).collect();
        let cube = if faces.len() == 6 { cube(&faces) } else { None };
        let (normal, spill, droplets, mask) = (tex("Normal"), tex("SpillWay_Texture"), tex("Tex2"), tex("Tex3"));
        if normal.is_none() || spill.is_none() || droplets.is_none() || mask.is_none() || cube.is_none() {
            warn!(
                "water sheet: textures missing (normal {}, spillway {}, Tex#2 {}, Tex#3 {}, cube {})",
                normal.is_some(),
                spill.is_some(),
                droplets.is_some(),
                mask.is_some(),
                cube.is_some()
            );
        }
        Some(PendingSpillway(SpillwayMaterial { params, normal, spill, droplets, mask, cube }))
    }
}

pub struct SpillwayPlugin;

impl Plugin for SpillwayPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "spillway.wgsl");
        app.add_plugins(MaterialPlugin::<SpillwayMaterial>::default()).add_systems(Update, apply_pending_spillways);
    }
}

fn apply_pending_spillways(mut commands: Commands, q: Query<(Entity, &PendingSpillway)>, mut mats: ResMut<Assets<SpillwayMaterial>>) {
    for (e, p) in &q {
        let h = mats.add(p.0.clone());
        commands
            .entity(e)
            .remove::<(PendingSpillway, MeshMaterial3d<StandardMaterial>, bevy::pbr::Lightmap)>()
            .insert((MeshMaterial3d(h), bevy::light::NotShadowReceiver, bevy::light::NotShadowCaster));
    }
}
