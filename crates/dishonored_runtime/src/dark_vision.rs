//! Dark Vision's presentation (`Twk_Powers.DarkVision.Twk_DarkVision`, EFFECTS.md "Dark
//! Vision"):
//!
//! - The colour grade: while the power is on, `m_PpGlobalParameters.m_StaticUberAdjustement`
//!   overrides the level's ArkPp colour LUT (colour balance shadows (0.5, 0.1, -0.2), midtones /
//!   highlights (0.1, 0, 0), opacity 1, pre-desaturation 1, post-desaturation 0.5, exposure
//!   0.1), faded in over `m_PowerFadeInDuration` (0.1 s) and out over `m_PowerFadeOutDuration`
//!   (0.2 s). It goes through the same LUT pass as the level's grade (`ark_post`).
//! - The souls: revealed pawns (`left_hand::dark_vision_reveals`) are drawn again with
//!   `m_pSoulMaterial` = `AltScreen_Effects.SAVED.DarkVision_Souls_INST` (parent
//!   `DarkVision_PMAT`), as its soul pass computes it (RefShaderCache
//!   `TSoulPartMeshPixelShader<FSoulPartMeshPolicy>` 35003, skinned vertex shader 35012):
//!   - where the pawn is in front of the scene: `Full_Color`, alpha
//!     `0.7 x (0.5 + 0.5 (n_view.z + 1)^2)` (brighter towards the silhouette);
//!   - where the scene hides it: `Behind_color + 0.3 x Tex#1.g` at
//!     `uv + frac(-0.05 t) + 0.4 x Tex#0.rg(uv + frac(0.05 t))`, alpha 0.8;
//!   - both x `1 - smoothstep(vNearFarParams.x, .y, view depth)`.
//!   The instance sets `Full_Color` (1, 0.947, 0) and `Behind_color` (0.859, 0.810, 0); the power
//!   sets `vNearFarParams` from `m_fFadeStartAtDistance` (1700) and `m_fDistance` (2000) (LIKELY;
//!   the material's default is (1800, 2000)). The in-front / behind test of the shader (scene
//!   depth at the pixel, 5 uu bias) is done with two depth-test states.

use crate::ark_post::ArkPostSource;
use crate::players::PlayerBody;
use crate::session::SessionView;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, CompareFunction, ShaderType};
use dishonored_ir::PostSettings;
use std::collections::HashMap;
use ue3_package::ObjRef;

/// `m_PowerFadeInDuration` / `m_PowerFadeOutDuration` of both levels.
pub const FADE_IN: f32 = 0.1;
pub const FADE_OUT: f32 = 0.2;
/// `DarkVision_Souls_INST`.
pub const FULL_COLOR: [f32; 3] = [1.0, 0.947_146_5, 0.0];
pub const BEHIND_COLOR: [f32; 3] = [0.858_695_6, 0.810_195_9, 0.0];
/// `m_fFadeStartAtDistance`, `m_fDistance` (both levels), uu.
pub const FADE_START: f32 = 1700.0;
pub const FADE_END: f32 = 2000.0;

/// The grade's overrides (`m_StaticUberAdjustement`, the fields with `m_bOverride*` set).
pub fn grade_overrides() -> PostSettings {
    PostSettings {
        source: "Twk_DarkVision.m_StaticUberAdjustement".into(),
        values: vec![("m_Opacity".into(), 1.0), ("m_PreDesaturation".into(), 1.0), ("m_PostDesaturation".into(), 0.5), ("m_Exposure".into(), 0.1)],
        vectors: vec![
            ("m_CrMgYbShadTones".into(), [0.5, 0.1, -0.2, 0.0]),
            ("m_CrMgYbMidTones".into(), [0.1, 0.0, 0.0, 0.0]),
            ("m_CrMgYbHighTones".into(), [0.1, 0.0, 0.0, 0.0]),
        ],
    }
}

/// The level's post settings with the power's overrides on top.
pub fn merged(level: Option<&PostSettings>, over: &PostSettings) -> PostSettings {
    let mut out = level.cloned().unwrap_or_default();
    for (k, v) in &over.values {
        out.values.retain(|(n, _)| !n.ends_with(k.as_str()));
        out.values.push((k.clone(), *v));
    }
    for (k, v) in &over.vectors {
        out.vectors.retain(|(n, _)| !n.ends_with(k.as_str()));
        out.vectors.push((k.clone(), *v));
    }
    out
}

/// Weight of the power's grade from the pawn's Dark Vision state: ramps up over the fade-in,
/// full for the duration, down over the fade-out.
pub fn weight(d: &dishonored_sim::state::DarkVisionState) -> f32 {
    if d.fade_in > 0.0 {
        (1.0 - d.fade_in / FADE_IN).clamp(0.0, 1.0)
    } else if d.remaining > 0.0 {
        1.0
    } else {
        (d.fade_out / FADE_OUT).clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct SoulUniform {
    /// `Full_Color` (rgb), `Behind_color` (rgb), packed with the pass flag.
    pub full: Vec4,
    pub behind: Vec4,
    /// (fade start, fade end) in Bevy units, behind pass (1) or in front (0), 0.
    pub params: Vec4,
}

impl SoulUniform {
    pub fn new(behind_pass: bool) -> Self {
        let k = crate::coords::UU_TO_BEVY;
        SoulUniform {
            full: Vec3::from(FULL_COLOR).extend(1.0),
            behind: Vec3::from(BEHIND_COLOR).extend(1.0),
            params: Vec4::new(FADE_START * k, FADE_END * k, if behind_pass { 1.0 } else { 0.0 }, 0.0),
        }
    }
}

macro_rules! soul_material {
    ($name:ident, $compare:expr) => {
        #[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
        pub struct $name {
            #[uniform(0)]
            pub soul: SoulUniform,
            /// `DarkVision_PMAT`'s `Tex#0` (distortion) and `Tex#1` (noise).
            #[texture(1)]
            #[sampler(2)]
            pub tex0: Option<Handle<Image>>,
            #[texture(3)]
            pub tex1: Option<Handle<Image>>,
        }

        impl Material for $name {
            fn fragment_shader() -> bevy::shader::ShaderRef {
                "embedded://dishonored_runtime/dark_vision.wgsl".into()
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
                if let Some(ds) = descriptor.depth_stencil.as_mut() {
                    ds.depth_compare = Some($compare);
                    ds.depth_write_enabled = Some(false);
                }
                Ok(())
            }
        }
    };
}

// Reverse Z: in front of the scene = greater or equal depth, hidden = less.
soul_material!(SoulFrontMaterial, CompareFunction::GreaterEqual);
soul_material!(SoulBehindMaterial, CompareFunction::Less);

/// Soul copies of one pawn root: (copies, front material, behind material).
type Ghosts = (Vec<Entity>, Handle<SoulFrontMaterial>, Handle<SoulBehindMaterial>);

/// A soul copy of one skinned mesh of a pawn.
#[derive(Component)]
pub struct SoulGhost;

#[derive(Resource, Default)]
pub struct Souls {
    ghosts: HashMap<Entity, Ghosts>,
    /// `DarkVision_PMAT`'s expression textures (`Tex#0`, `Tex#1`), loaded once.
    textures: Option<(Option<Handle<Image>>, Option<Handle<Image>>)>,
    /// Current grade weight (harness / tests).
    pub weight: f32,
}

pub struct DarkVisionPlugin;

impl Plugin for DarkVisionPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "dark_vision.wgsl");
        app.add_plugins((MaterialPlugin::<SoulFrontMaterial>::default(), MaterialPlugin::<SoulBehindMaterial>::default()))
            .init_resource::<Souls>()
            .add_systems(Update, (grade, souls));
    }
}

/// `DarkVision_PMAT`'s `FMaterial::UniformExpressionTextures` from DishonoredGame.
fn load_textures(images: &mut Assets<Image>) -> (Option<Handle<Image>>, Option<Handle<Image>>) {
    let load = || -> Option<Vec<Handle<Image>>> {
        let content = dishonored_assets::Content::locate().ok()?;
        let pkg = content.package("DishonoredGame").ok()?;
        let mi = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Material" && pkg.obj_path(ObjRef::Export(i)).ends_with("SAVED.DarkVision_PMAT"))?;
        let h = pkg.read_object(mi).ok()?;
        let data = pkg.export_data(mi).ok()?;
        let refs = dishonored_assets::material_chain::uniform_expression_textures(data, h.native_offset)?;
        let mut out = Vec::new();
        for r in refs {
            let idx = match ObjRef::from_index(r) {
                ObjRef::Export(e) => Some(e),
                other => {
                    let name = pkg.obj_name(other);
                    (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Texture2D" && pkg.obj_name(ObjRef::Export(i)).eq_ignore_ascii_case(&name))
                }
            };
            let t = idx.and_then(|i| dishonored_assets::texture::read_texture(&content, &pkg, i, 512).ok());
            match t {
                Some(t) => out.push(images.add(crate::map_render::to_image(&t))),
                None => return Some(out),
            }
        }
        Some(out)
    };
    let v = load().unwrap_or_default();
    if v.len() < 2 {
        warn!("Dark Vision: DarkVision_PMAT expression textures not found ({} of 2)", v.len());
    }
    (v.first().cloned(), v.get(1).cloned())
}

/// The colour grade: blend the power's overrides into the camera's ArkPp source.
fn grade(view: Option<Res<SessionView>>, mut souls: ResMut<Souls>, mut q: Query<&mut ArkPostSource>) {
    let w = view.as_ref().and_then(|v| v.me.as_ref()).map(|me| weight(&me.dark)).unwrap_or(0.0);
    souls.weight = w;
    for mut src in &mut q {
        if src.power_weight != w {
            src.power_weight = w;
        }
        if w > 0.0 && src.power_post.is_none() {
            src.power_post = Some(grade_overrides());
        }
    }
}

/// The souls: copies of the revealed pawns' skinned meshes with the two soul passes.
#[allow(clippy::too_many_arguments)]
fn souls(
    mut commands: Commands,
    view: Option<Res<SessionView>>,
    mut souls: ResMut<Souls>,
    mut images: ResMut<Assets<Image>>,
    mut front: ResMut<Assets<SoulFrontMaterial>>,
    mut behind: ResMut<Assets<SoulBehindMaterial>>,
    roots: Query<(Entity, &PlayerBody, Option<&Children>)>,
    parts: Query<(&Mesh3d, &SkinnedMesh), Without<SoulGhost>>,
) {
    let active = view.as_ref().and_then(|v| v.me.as_ref()).is_some_and(|me| me.vitals.dark_vision_remaining > 0.0);
    let revealed: Vec<u32> = if active { view.as_ref().map(|v| v.dark_vision_reveals.clone()).unwrap_or_default() } else { Vec::new() };
    if !revealed.is_empty() && souls.textures.is_none() {
        souls.textures = Some(load_textures(&mut images));
    }
    let (t0, t1) = souls.textures.clone().unwrap_or((None, None));
    let mut keep = Vec::new();
    for (root, body, children) in &roots {
        if !revealed.contains(&body.0) {
            continue;
        }
        keep.push(root);
        if souls.ghosts.contains_key(&root) {
            continue;
        }
        let f = front.add(SoulFrontMaterial { soul: SoulUniform::new(false), tex0: t0.clone(), tex1: t1.clone() });
        let b = behind.add(SoulBehindMaterial { soul: SoulUniform::new(true), tex0: t0.clone(), tex1: t1.clone() });
        let mut copies = Vec::new();
        for c in children.map(|c| c.to_vec()).unwrap_or_default() {
            if let Ok((mesh, skin)) = parts.get(c) {
                let skinned = || SkinnedMesh { inverse_bindposes: skin.inverse_bindposes.clone(), joints: skin.joints.clone() };
                copies.push(commands.spawn((Mesh3d(mesh.0.clone()), MeshMaterial3d(f.clone()), skinned(), Transform::default(), bevy::light::NotShadowCaster, bevy::camera::visibility::NoFrustumCulling, SoulGhost, ChildOf(root))).id());
                copies.push(commands.spawn((Mesh3d(mesh.0.clone()), MeshMaterial3d(b.clone()), skinned(), Transform::default(), bevy::light::NotShadowCaster, bevy::camera::visibility::NoFrustumCulling, SoulGhost, ChildOf(root))).id());
            }
        }
        souls.ghosts.insert(root, (copies, f, b));
    }
    let gone: Vec<Entity> = souls.ghosts.keys().filter(|r| !keep.contains(r)).copied().collect();
    for r in gone {
        if let Some((copies, f, b)) = souls.ghosts.remove(&r) {
            for e in copies {
                commands.entity(e).try_despawn();
            }
            front.remove(&f);
            behind.remove(&b);
        }
    }
}
