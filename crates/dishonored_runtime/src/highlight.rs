//! The interactables' highlight drawn for the Kismet (`DisSeqAct_Highlight`, replicated in
//! `MatchView::highlights`; the host side is `dishonored_multiplayer::kismet_host::presentation`).
//!
//! Applying a highlight (0x00A27FF0) needs the HUD's `m_Settings.m_bShowHighlight`
//! (`PSI_HUD_bShowHighlight`, read through 0x00BBF730 / 0x00B87E60) and the actor's interactable
//! tweak's `m_bAllowedToHighlight` and `m_HighLightMaterial` (decoded with the level,
//! `dishonored_ir::HighlightDef`). The original adds a second mesh component over the actor's
//! (usables 0x00A1C920: a `SkeletalMeshComponent` with the actor's mesh, its parent anim component
//! the actor's; pickups and keys 0x00A1D8C0: a `StaticMeshComponent` with the actor's mesh), clears
//! `CastShadow` / `bCastDynamicShadow`, sets `DisTranslucencySortPriority` 5 and puts the highlight
//! material on element 0 (`SetMaterial(0, ..)`). The other elements keep the mesh's own materials
//! and draw on top of themselves, so only the slot-0 sections are copied here.
//!
//! Every retail interactable tweak names `vfx_interactivity.interactivity_INST`, an instance of
//! `Materials_ref.FX_materials.golden_focus_PMAT` (`BLEND_Translucent`, `MLM_Unlit`) without
//! static switches (`N_Enable_NormalMap`, `O_enable_OpacityMask`, `F_Enable_Fresnel_Pulsing` off).
//! Its compiled pixel shader (RefShaderCache-PC-D3D-SM3, shader 36348, `FNoLightMapPolicy`, local
//! and GPU skin vertex factories alike), with V the tangent-space camera vector:
//!
//!   NdotV = saturate(normalize(V).z)
//!   F = (1 - NdotV)^F_Fresnel_Power when 0.999999 - NdotV >= 0, else 0
//!   emissive = F x F_Fresnel_Visibility x F_Fresnel_Color + SelectionColor (0)
//!   opacity = saturate(F x saturate(F_Fresnel_Visibility))
//!
//! The instance sets `F_Fresnel_Power` 4, `F_Fresnel_Visibility` 10, `F_Fresnel_Color`
//! (1, 0.454, 0, 1) (the parent's defaults 5, 10, (16, 10, 3)); the values come from the decoded
//! material. Materials on another base are not drawn (none in the retail data).

use crate::coords;
use crate::level::LevelOf;
use crate::level_props::{LevelProp, PropSection};
use crate::session::SessionView;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::mesh::skinning::SkinnedMesh;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use dishonored_ir::{DishonoredMap, MaterialParams};
use std::collections::HashMap;

/// The base material whose shader [`HighlightMaterial`] ports.
pub const HIGHLIGHT_BASE: &str = "golden_focus_PMAT";

#[derive(Clone, Copy, Debug, Default, PartialEq, ShaderType)]
pub struct HighlightUniform {
    /// `F_Fresnel_Color`.
    pub color: Vec4,
    /// (`F_Fresnel_Power`, `F_Fresnel_Visibility`, UE3 radiance to Bevy light units, 0).
    pub params: Vec4,
    /// `SelectionColor` (0 in game).
    pub selection: Vec4,
}

impl HighlightUniform {
    /// The uniform of a decoded highlight material (None: not a `golden_focus_PMAT` instance).
    pub fn from_params(p: &MaterialParams) -> Option<Self> {
        let base = p.base_material.as_deref()?;
        if !base.rsplit('.').next().is_some_and(|b| b.eq_ignore_ascii_case(HIGHLIGHT_BASE)) {
            return None;
        }
        let scalar = |n: &str, d: f32| p.scalars.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|v| v.1).unwrap_or(d);
        let color = p.vectors.iter().find(|(k, _)| k.eq_ignore_ascii_case("F_Fresnel_Color")).map(|v| Vec4::from_array(v.1)).unwrap_or(Vec4::new(16.0, 10.0, 3.0, 1.0));
        Some(HighlightUniform { color, params: Vec4::new(scalar("F_Fresnel_Power", 5.0), scalar("F_Fresnel_Visibility", 10.0), crate::map_render::light_unit(), 0.0), selection: Vec4::ZERO })
    }

    /// The pixel shader's colour and opacity for a `NdotV` (the WGSL's, for tests).
    pub fn shade(&self, n_dot_v: f32) -> (Vec3, f32) {
        let n = n_dot_v.clamp(0.0, 1.0);
        let f = if 0.999_999 - n >= 0.0 { (1.0 - n).powf(self.params.x) } else { 0.0 };
        let vis = self.params.y;
        (self.color.truncate() * f * vis + self.selection.truncate(), (f * vis.clamp(0.0, 1.0)).clamp(0.0, 1.0))
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct HighlightMaterial {
    #[uniform(0)]
    pub params: HighlightUniform,
}

impl Material for HighlightMaterial {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://dishonored_runtime/highlight.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// A level's highlights, built when it spawns ([`queue`]), consumed by [`update`].
#[derive(Component)]
pub struct PendingHighlights {
    level: String,
    defs: Vec<Def>,
}

/// One highlightable actor: where the level placed it, its material (None: not drawn) and its
/// static mesh copies (slot-0 section, Dishonored world transform).
#[derive(Clone)]
struct Def {
    actor: String,
    level: String,
    placed: Vec3,
    material: Option<HighlightUniform>,
    statics: Vec<(Handle<Mesh>, Mat4)>,
}

#[derive(Resource, Default)]
pub struct Highlights {
    defs: Vec<Def>,
    /// Shown overlays by def index.
    shown: HashMap<usize, Vec<Entity>>,
}

/// A highlight copy of a static mesh: its def and level transform (moved with the actor).
#[derive(Component)]
struct HighlightStatic {
    actor: String,
    placed: Vec3,
    transform: Mat4,
}

pub struct HighlightPlugin;

impl Plugin for HighlightPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "highlight.wgsl");
        app.add_plugins(MaterialPlugin::<HighlightMaterial>::default()).init_resource::<Highlights>().add_systems(Update, (take_pending, update.after(take_pending)));
    }
}

/// Queue a level's highlights (called when its geometry is spawned): the slot-0 section of each
/// static mesh component of the actors.
pub fn queue(level: &str, map: &DishonoredMap, commands: &mut Commands, meshes: &mut Assets<Mesh>, features: &crate::map_render::RenderFeatures) {
    if map.highlights.is_empty() {
        return;
    }
    let defs = map
        .highlights
        .iter()
        .map(|h| {
            let material = h.material.filter(|_| h.allowed).and_then(|m| map.material_params.get(m)).and_then(HighlightUniform::from_params);
            let statics = h
                .instances
                .iter()
                .filter_map(|&i| map.instances.get(i))
                .filter_map(|inst| {
                    let si = map.meshes[inst.mesh].sections.iter().position(|s| s.material_index == 0)?;
                    Some((meshes.add(crate::map_render::build_section_mesh(map, inst.mesh, si, features)), inst.transform))
                })
                .collect();
            Def { actor: h.actor.clone(), level: level.to_string(), placed: Vec3::from_array(h.placed), material, statics }
        })
        .collect();
    commands.spawn(PendingHighlights { level: level.to_string(), defs });
}

fn take_pending(mut commands: Commands, q: Query<(Entity, &PendingHighlights)>, mut hl: ResMut<Highlights>) {
    for (e, p) in &q {
        commands.entity(e).despawn();
        // A level loaded again replaces its old entries.
        let old: Vec<usize> = hl.defs.iter().enumerate().filter(|(_, d)| d.level == p.level).map(|(i, _)| i).collect();
        for i in old.iter().rev() {
            if let Some(es) = hl.shown.remove(i) {
                for e in es {
                    commands.entity(e).try_despawn();
                }
            }
        }
        let Highlights { defs, shown } = &mut *hl;
        let mut keep = Vec::new();
        let mut moved = HashMap::new();
        for (i, d) in std::mem::take(defs).into_iter().enumerate() {
            if d.level != p.level {
                if let Some(s) = shown.remove(&i) {
                    moved.insert(keep.len(), s);
                }
                keep.push(d);
            }
        }
        keep.extend(p.defs.iter().cloned());
        *defs = keep;
        *shown = moved;
    }
}

/// Whether a replicated highlight names a def (same actor key; same-named actors of other
/// sublevels are told apart by the placed location).
fn matches(def_actor: &str, def_placed: Vec3, actor: &str, placed: Vec3) -> bool {
    def_actor.eq_ignore_ascii_case(actor) && def_placed.distance(placed) < 1.0
}

#[allow(clippy::too_many_arguments)]
fn update(
    mut commands: Commands,
    view: Res<SessionView>,
    settings: Option<Res<crate::settings::GameSettings>>,
    mut hl: ResMut<Highlights>,
    mut mats: ResMut<Assets<HighlightMaterial>>,
    props: Query<(Entity, &LevelProp, &Children)>,
    sections: Query<(&Mesh3d, &SkinnedMesh, &PropSection, Option<&bevy::camera::primitives::Aabb>)>,
    mut statics: Query<(&HighlightStatic, &mut Transform)>,
    existing: Query<Entity>,
) {
    let show = crate::hud_settings::options(settings.as_deref()).highlight;
    let wanted: Vec<usize> = if show {
        view.matchv.highlights.iter().filter_map(|v| hl.defs.iter().position(|d| matches(&d.actor, d.placed, &v.actor, Vec3::from_array(v.placed)))).collect()
    } else {
        Vec::new()
    };
    let Highlights { defs, shown } = &mut *hl;
    // Unhighlighted (or gone): remove the copies.
    shown.retain(|i, es| {
        let keep = wanted.contains(i) && es.iter().all(|e| existing.contains(*e));
        if !keep {
            for e in es.iter() {
                commands.entity(*e).try_despawn();
            }
        }
        keep
    });
    for i in wanted {
        if shown.contains_key(&i) {
            continue;
        }
        let d = &defs[i];
        let Some(u) = d.material else { continue };
        let mat = mats.add(HighlightMaterial { params: u });
        let mut es = Vec::new();
        for (mesh, m) in &d.statics {
            es.push(
                commands
                    .spawn((
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(mat.clone()),
                        Transform::from_matrix(coords::transform(*m)),
                        NotShadowCaster,
                        NotShadowReceiver,
                        LevelOf(d.level.clone()),
                        HighlightStatic { actor: d.actor.clone(), placed: d.placed, transform: *m },
                    ))
                    .id(),
            );
        }
        // A usable's skeletal mesh: copies of its slot-0 sections skinned to the same joints.
        for (root, p, children) in &props {
            if !matches(&p.prop.actor, p.prop.actor_to_world.w_axis.truncate(), &d.actor, d.placed) {
                continue;
            }
            for c in children.iter() {
                let Ok((mesh, skin, sec, aabb)) = sections.get(c) else { continue };
                if sec.slot != 0 {
                    continue;
                }
                let mut e = commands.spawn((mesh.clone(), MeshMaterial3d(mat.clone()), skin.clone(), Transform::default(), NotShadowCaster, NotShadowReceiver, ChildOf(root)));
                if let Some(a) = aabb {
                    e.insert((*a, bevy::camera::visibility::NoAutoAabb));
                }
                es.push(e.id());
            }
        }
        // Nothing spawned yet (the props spawn a frame later): try again next frame.
        if !es.is_empty() {
            shown.insert(i, es);
        }
    }
    // Static copies follow their actor when the Kismet moved it.
    if let Some(m) = view.matchv.mission.as_ref() {
        for (s, mut t) in &mut statics {
            let key_actor = s.actor.as_str();
            let delta = crate::level_props::mover_delta(&m.movers, key_actor, s.placed).unwrap_or(glam::Mat4::IDENTITY);
            *t = Transform::from_matrix(coords::transform(delta * s.transform));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interactivity() -> MaterialParams {
        MaterialParams {
            base_material: Some("Materials_ref.FX_materials.golden_focus_PMAT".into()),
            scalars: vec![("F_Fresnel_Power".into(), 4.0), ("F_Fresnel_Visibility".into(), 10.0), ("F_Fresnal_speed".into(), 0.0)],
            vectors: vec![("F_Fresnel_Color".into(), [1.0, 0.4542188, 0.0, 1.0])],
            ..Default::default()
        }
    }

    #[test]
    fn the_highlight_is_a_fresnel_rim() {
        let u = HighlightUniform::from_params(&interactivity()).unwrap();
        assert_eq!((u.params.x, u.params.y), (4.0, 10.0));
        // Facing the camera: nothing (the cmp's 0.999999 cut).
        let (c, a) = u.shade(1.0);
        assert_eq!((c, a), (Vec3::ZERO, 0.0));
        // Grazing: full rim, opacity saturated, colour x visibility.
        let (c, a) = u.shade(0.0);
        assert_eq!(a, 1.0);
        assert!((c - Vec3::new(10.0, 4.542188, 0.0)).length() < 1e-4);
        // Half way: (0.5)^4.
        let (_, a) = u.shade(0.5);
        assert!((a - 0.0625).abs() < 1e-6);
    }

    #[test]
    fn other_bases_are_not_highlights() {
        let p = MaterialParams { base_material: Some("Materials_ref.Other_PMAT".into()), ..Default::default() };
        assert!(HighlightUniform::from_params(&p).is_none());
        // The parent's defaults when the instance overrides nothing.
        let p = MaterialParams { base_material: Some("Materials_ref.FX_materials.golden_focus_PMAT".into()), ..Default::default() };
        let u = HighlightUniform::from_params(&p).unwrap();
        assert_eq!((u.params.x, u.params.y, u.color), (5.0, 10.0, Vec4::new(16.0, 10.0, 3.0, 1.0)));
    }

    #[test]
    fn highlights_match_by_key_and_place() {
        assert!(matches("L_Brothel_Script:DishonoredUsableObject_3", Vec3::new(1.0, 2.0, 3.0), "l_brothel_script:dishonoredusableobject_3", Vec3::new(1.0, 2.0, 3.2)));
        assert!(!matches("L_Brothel_Script:DishonoredUsableObject_3", Vec3::ZERO, "L_Brothel_Script:DishonoredUsableObject_3", Vec3::new(50.0, 0.0, 0.0)));
    }
}
