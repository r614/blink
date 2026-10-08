//! Dark Vision's presentation (`Twk_Powers.DarkVision.Twk_DarkVision`, EFFECTS.md "Dark
//! Vision"):
//!
//! - The colour grade: while the power is on, `m_PpGlobalParameters.m_StaticUberAdjustement`
//!   overrides the level's ArkPp colour LUT (colour balance shadows (0.5, 0.1, -0.2), midtones /
//!   highlights (0.1, 0, 0), opacity 1, pre-desaturation 1, post-desaturation 0.5, exposure
//!   0.1), faded in over `m_PowerFadeInDuration` (0.1 s) and out over `m_PowerFadeOutDuration`
//!   (0.2 s). It goes through the same LUT pass as the level's grade (`ark_post`).
//! - The souls: revealed pawns (`left_hand::dark_vision_reveals`, within `m_fDistance`) are
//!   drawn again through walls with the soul colour ramp, `m_LowerColor` (131, 69, 33) x
//!   `m_LowerBrightness` 1.301 at the feet to `m_UpperColor` (202, 154, 97) x
//!   `m_UpperBrightness` 0.9827 at the head, brighter at the silhouette. APPROXIMATION: the
//!   soul material `AltScreen_Effects.SAVED.DarkVision_Souls_INST` and the power's screen
//!   material `PPG_DarkVisionFinal_Mat` are not decoded; the ramp colours are the tweak's.

use crate::ark_post::ArkPostSource;
use crate::players::PlayerBody;
use crate::session::SessionView;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, CompareFunction, ShaderType};
use dishonored_ir::PostSettings;
use std::collections::HashMap;

/// `m_PowerFadeInDuration` / `m_PowerFadeOutDuration` of both levels.
pub const FADE_IN: f32 = 0.1;
pub const FADE_OUT: f32 = 0.2;

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

/// Soul ramp colours (linear): (lower, upper), from the tweak's sRGB bytes x brightness.
pub fn soul_colours() -> (Vec4, Vec4) {
    let c = |r: u8, g: u8, b: u8, k: f32| {
        let l = Color::srgb_u8(r, g, b).to_linear();
        Vec4::new(l.red * k, l.green * k, l.blue * k, 1.0)
    };
    (c(131, 69, 33, 1.301), c(202, 154, 97, 0.9827))
}

#[derive(Clone, Copy, Debug, Default, ShaderType)]
pub struct SoulUniform {
    pub lower: Vec4,
    pub upper: Vec4,
    /// (feet y, head y) in Bevy world units, strength, 0.
    pub params: Vec4,
}

/// A revealed pawn's mesh drawn through walls (depth test off, additive).
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct SoulMaterial {
    #[uniform(0)]
    pub soul: SoulUniform,
}

impl Material for SoulMaterial {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/dark_vision.wgsl".into()
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
        // Seen through walls: no depth test, no depth write, both faces.
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.depth_compare = Some(CompareFunction::Always);
            ds.depth_write_enabled = Some(false);
        }
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// A soul copy of one skinned mesh of a pawn.
#[derive(Component)]
pub struct SoulGhost;

/// Live soul copies per pawn root: (copies, material).
#[derive(Resource, Default)]
pub struct Souls {
    ghosts: HashMap<Entity, (Vec<Entity>, Handle<SoulMaterial>)>,
    /// Current grade weight (harness / tests).
    pub weight: f32,
}

pub struct DarkVisionPlugin;

impl Plugin for DarkVisionPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "dark_vision.wgsl");
        app.add_plugins(MaterialPlugin::<SoulMaterial>::default()).init_resource::<Souls>().add_systems(Update, (grade, souls));
    }
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

/// The souls: copies of the revealed pawns' skinned meshes with the soul material.
#[allow(clippy::too_many_arguments)]
fn souls(
    mut commands: Commands,
    view: Option<Res<SessionView>>,
    mut souls: ResMut<Souls>,
    mut mats: ResMut<Assets<SoulMaterial>>,
    roots: Query<(Entity, &PlayerBody, &GlobalTransform, Option<&Children>)>,
    parts: Query<(&Mesh3d, &SkinnedMesh), Without<SoulGhost>>,
    session: Option<Res<crate::session::Session>>,
) {
    let active = view.as_ref().and_then(|v| v.me.as_ref()).is_some_and(|me| me.vitals.dark_vision_remaining > 0.0);
    let revealed: Vec<u32> = if active { view.as_ref().map(|v| v.dark_vision_reveals.clone()).unwrap_or_default() } else { Vec::new() };
    let hh = session.as_ref().map(|s| s.content.sim.gameplay.movement.collision_half_height).unwrap_or(86.0) * crate::coords::UU_TO_BEVY;
    let (lower, upper) = soul_colours();
    let mut keep = Vec::new();
    for (root, body, gt, children) in &roots {
        if !revealed.contains(&body.0) {
            continue;
        }
        keep.push(root);
        let y = gt.translation().y;
        let soul = SoulUniform { lower, upper, params: Vec4::new(y - hh, y + hh, souls.weight.max(0.2), 0.0) };
        if let Some((_, m)) = souls.ghosts.get(&root) {
            if let Some(mut mat) = mats.get_mut(m) {
                mat.soul = soul;
            }
            continue;
        }
        let m = mats.add(SoulMaterial { soul });
        let mut copies = Vec::new();
        for c in children.map(|c| c.to_vec()).unwrap_or_default() {
            if let Ok((mesh, skin)) = parts.get(c) {
                let e = commands.spawn((Mesh3d(mesh.0.clone()), MeshMaterial3d(m.clone()), SkinnedMesh { inverse_bindposes: skin.inverse_bindposes.clone(), joints: skin.joints.clone() }, Transform::default(), bevy::light::NotShadowCaster, bevy::camera::visibility::NoFrustumCulling, SoulGhost, ChildOf(root))).id();
                copies.push(e);
            }
        }
        souls.ghosts.insert(root, (copies, m));
    }
    let gone: Vec<Entity> = souls.ghosts.keys().filter(|r| !keep.contains(r)).copied().collect();
    for r in gone {
        if let Some((copies, m)) = souls.ghosts.remove(&r) {
            for e in copies {
                commands.entity(e).try_despawn();
            }
            mats.remove(&m);
        }
    }
}
