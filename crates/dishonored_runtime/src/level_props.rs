//! Doors and usable objects of the level (`DisDoor`, `DishonoredUsableObject` and subclasses),
//! drawn with their skeletal meshes (`dishonored_assets::usable_props`).
//!
//! - Placement: the actor x component transform, times the delta the Kismet moved the actor by
//!   (`MissionView::movers`, matinees and attachments).
//! - Doors follow their `EDisDoorState`: the level's `m_InitialDoorState`, or the state the
//!   mission's Kismet set (`DisSeqAct_Door`, replicated in `MissionView::doors`). A change plays
//!   the stage's transition animation (`m_DoorOpenCW` .. `m_DoorCloseCCW` of the door tweak); an
//!   open door rests on the last frame of its open animation, a closed one in the reference pose.
//! - In `--inspect` there is no mission: every prop stays where and as the level placed it.
//!
//! Level actors moved by the Kismet outside matinees (`InterpActor` static meshes) follow
//! `MissionView::movers` too ([`follow_movers`]).

use crate::characters::{CharacterGpu, Joint, to_bevy};
use crate::coords;
use crate::level::LevelOf;
use crate::npc_anim::{AnimBank, Pose};
use crate::session::SessionView;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use dishonored_ir::{DoorAnims, SkeletalProp};
use dishonored_mission::runtime::MoverView;
use std::collections::HashMap;
use std::sync::Arc;

/// Props of a freshly imported level, spawned by [`spawn_props`].
#[derive(Component)]
pub struct PendingProps {
    level: String,
    props: Vec<SkeletalProp>,
}

/// Queue a level's props (called when its geometry is spawned).
pub fn queue(level: &str, map: &dishonored_ir::DishonoredMap, commands: &mut Commands) {
    if !map.skeletal_props.is_empty() {
        commands.spawn(PendingProps { level: level.to_string(), props: map.skeletal_props.clone() });
    }
}

/// One spawned prop (the root of its joints and skinned meshes).
#[derive(Component)]
pub struct LevelProp {
    pub prop: SkeletalProp,
    model: (String, String),
    /// Door state shown, and the transition playing (animation, seconds since it started).
    shown: u8,
    playing: Option<(String, f32)>,
    /// What the joints were last posed for (animation, time), to skip unchanged frames.
    posed: Option<(String, f32)>,
}

#[derive(Resource, Default)]
pub struct LevelProps {
    content: Option<Arc<dishonored_assets::Content>>,
    models: HashMap<(String, String), Option<CharacterGpu>>,
    profiles: HashMap<Vec<(String, String)>, usize>,
}

/// The transition animation a door plays from state `from` to `to` (`EDisDoorState`): the open
/// stage of the new direction, or the close stage of the direction it was open in. Empty when
/// the tweak names none.
pub fn transition_anim(door: &DoorAnims, from: u8, to: u8) -> &str {
    match (from, to) {
        (_, 1) => &door.open_cw,
        (_, 2) => &door.open_ccw,
        (2, 0) => &door.close_ccw,
        _ => &door.close_cw,
    }
}

/// The animation whose last frame holds a door at rest in `state` (None: the reference pose,
/// closed).
pub fn rest_anim(door: &DoorAnims, state: u8) -> Option<&str> {
    match state {
        1 => Some(door.open_cw.as_str()),
        2 => Some(door.open_ccw.as_str()),
        _ => None,
    }
    .filter(|s| !s.is_empty())
}

/// The Kismet's delta for a level actor ("package:actor" placed at `placed`), if it moved it.
pub fn mover_delta(movers: &[MoverView], actor: &str, placed: glam::Vec3) -> Option<glam::Mat4> {
    movers
        .iter()
        .find(|m| m.actor.eq_ignore_ascii_case(actor) && glam::Vec3::from(m.placed).distance(placed) < 1.0)
        .map(|m| glam::Mat4::from_rotation_translation(glam::Quat::from_array(m.rotation), glam::Vec3::from(m.translation)))
}

/// Load and spawn queued props (models cached per skeletal mesh).
#[allow(clippy::too_many_arguments)]
pub fn spawn_props(
    mut commands: Commands,
    mut lp: ResMut<LevelProps>,
    pending: Query<(Entity, &PendingProps)>,
    features: Option<Res<crate::map_render::RenderFeatures>>,
    settings: Option<Res<crate::LevelSettings>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut prop_mats: ResMut<Assets<crate::prop_light::PropMaterial>>,
) {
    for (e, p) in &pending {
        commands.entity(e).despawn();
        if lp.content.is_none() {
            lp.content = dishonored_assets::Content::locate().ok().map(Arc::new);
        }
        let Some(content) = lp.content.clone() else { return };
        let feat = features.as_deref().cloned().unwrap_or_else(|| crate::map_render::RenderFeatures::from_flags(&|_| false));
        let max_tex = settings.as_ref().map(|s| s.0.max_tex).unwrap_or(1024);
        let mut spawned = 0;
        for prop in &p.props {
            let key = prop.mesh.clone();
            let gpu = lp
                .models
                .entry(key.clone())
                .or_insert_with(|| {
                    let name = key.1.rsplit('.').next().unwrap_or(&key.1).to_string();
                    match dishonored_assets::characters::load_model(&content, &name, &[(key.0.as_str(), key.1.as_str())], None, max_tex) {
                        Ok(m) => Some(crate::characters::upload(&m, &mut meshes, &mut images, &mut mats, &mut binds, &feat)),
                        Err(err) => {
                            warn!("level prop {}: {err:#}", prop.actor);
                            None
                        }
                    }
                })
                .clone();
            let Some(gpu) = gpu else { continue };
            let vis = if prop.hidden { Visibility::Hidden } else { Visibility::Inherited };
            let root = commands
                .spawn((
                    Transform::from_matrix(coords::transform(prop.transform)),
                    vis,
                    LevelOf(p.level.clone()),
                    LevelProp { prop: prop.clone(), model: key.clone(), shown: prop.door.as_ref().map(|d| d.initial).unwrap_or(0), playing: None, posed: None },
                ))
                .id();
            for (pi, part) in gpu.parts.iter().enumerate() {
                let js: Vec<Entity> = part.bind.iter().enumerate().map(|(bi, bm)| commands.spawn((Transform::from_matrix(*bm), Joint { part: pi, bone: bi }, ChildOf(root))).id()).collect();
                for (si, (mesh, mat)) in part.sections.iter().enumerate() {
                    let skin = SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: js.clone() };
                    // The original's light environment (prop_light.rs) when it was computed.
                    let env = prop.light_env.filter(|_| feat.materials && feat.dynamic_lights && !feat.debug_lighting);
                    match (env, mats.get(mat)) {
                        (Some(env), Some(base)) => {
                            let p = part.section_materials.get(si).copied().flatten().and_then(|m| gpu.material_params.get(m));
                            let ext = crate::prop_light::prop_light(&env, p, &gpu.textures, crate::map_render::light_unit());
                            let h = prop_mats.add(crate::prop_light::PropMaterial { base: base.clone(), extension: ext });
                            commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(h), skin, Transform::default(), ChildOf(root)));
                        }
                        _ => {
                            commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), skin, Transform::default(), ChildOf(root)));
                        }
                    }
                }
            }
            spawned += 1;
        }
        info!("{}: {spawned} of {} doors and usable objects spawned ({} skeletal meshes)", p.level, p.props.len(), lp.models.len());
    }
}

/// Place the props where the Kismet has their actors, and pose doors for their state.
pub fn pose_props(
    time: Res<Time>,
    mut lp: ResMut<LevelProps>,
    view: Option<Res<SessionView>>,
    bank: Option<ResMut<AnimBank>>,
    mut props: Query<(&mut LevelProp, &mut Transform, &Children)>,
    mut joints: Query<(&Joint, &mut Transform), Without<LevelProp>>,
) {
    let mission = view.as_ref().and_then(|v| v.matchv.mission.as_ref());
    let mut bank = bank;
    let dt = time.delta_secs();
    for (mut p, mut tr, children) in &mut props {
        // Placement.
        let placed = p.prop.actor_to_world.w_axis.truncate();
        let delta = mission.and_then(|m| mover_delta(&m.movers, &p.prop.actor, placed)).unwrap_or(glam::Mat4::IDENTITY);
        let want = Transform::from_matrix(coords::transform(delta * p.prop.transform));
        if *tr != want {
            *tr = want;
        }
        // Door state.
        let Some(door) = p.prop.door.clone() else { continue };
        let state = mission.and_then(|m| m.doors.iter().find(|d| d.0.eq_ignore_ascii_case(&p.prop.actor)).map(|d| d.1)).unwrap_or(door.initial);
        if state != p.shown {
            let anim = transition_anim(&door, p.shown, state).to_string();
            p.playing = (!anim.is_empty()).then_some((anim, 0.0));
            p.shown = state;
        }
        let Some(bank) = bank.as_mut() else { continue };
        let Some(Some(gpu)) = lp.models.get(&p.model).cloned() else { continue };
        let sets = p.prop.anim_sets.clone();
        let profile = *lp.profiles.entry(sets.clone()).or_insert_with(|| bank.sets_profile(&sets));
        // The transition plays to its end, then the door rests on its stage's pose.
        let mut sample: Option<(String, f32)> = None;
        if let Some((anim, t)) = p.playing.clone() {
            let len = bank.clip(profile, &anim).map(|c| c.1.duration).unwrap_or(0.0);
            let t = t + dt;
            if t < len {
                p.playing = Some((anim.clone(), t));
                sample = Some((anim, t));
            } else {
                p.playing = None;
            }
        }
        if sample.is_none() {
            sample = rest_anim(&door, state).map(|a| (a.to_string(), f32::MAX));
        }
        if p.posed == sample {
            continue;
        }
        let parts = gpu.parts.clone();
        let mut pose: Pose = parts.iter().map(|q| vec![None; q.bones.len()]).collect();
        if let Some((anim, t)) = &sample {
            let t = match bank.clip(profile, anim) {
                Some((_, c)) => t.min(c.duration),
                None => 0.0,
            };
            match bank.sample_parts(&parts, profile, anim, t) {
                Some(sp) => crate::npc_anim::blend_into(&mut pose, &sp, 1.0, &parts),
                None => warn!("door {}: no animation {anim}", p.prop.actor),
            }
        }
        let globals: Vec<Vec<glam::Mat4>> = parts.iter().zip(&pose).map(|(part, pp)| crate::npc_anim::globals(part, pp)).collect();
        for ch in children.iter() {
            if let Ok((j, mut t)) = joints.get_mut(ch) {
                if let Some(g) = globals.get(j.part).and_then(|v| v.get(j.bone)) {
                    *t = Transform::from_matrix(to_bevy(*g));
                }
            }
        }
        p.posed = sample;
    }
}

/// [interp] Level actors (`InterpActor` meshes) the Kismet moved outside a playing matinee
/// (where a matinee left them, attachments, teleports) are drawn where the server has them;
/// during a matinee `cutscene::drive` places them.
pub fn follow_movers(
    view: Res<SessionView>,
    mut commands: Commands,
    mut movers: Query<(Entity, &crate::cutscene::MovableActor, &mut Transform, Option<&crate::cutscene::MoverBase>)>,
) {
    let Some(m) = view.matchv.mission.as_ref() else { return };
    let in_matinee = |name: &str| m.cutscenes.iter().any(|c| c.groups.iter().any(|g| g.1.iter().any(|b| b.actor.as_ref().is_some_and(|a| a.1.eq_ignore_ascii_case(name)))));
    for (e, mv, mut tr, base) in &mut movers {
        if in_matinee(&mv.actor) {
            continue;
        }
        // Mover entries name "package:actor"; the instance knows its actor name and origin.
        let delta = m.movers.iter().find(|x| x.actor.rsplit(':').next().is_some_and(|n| n.eq_ignore_ascii_case(&mv.actor)) && glam::Vec3::from(x.placed).distance(mv.origin) < 400.0);
        match (delta, base) {
            (Some(x), _) => {
                let base = match base {
                    Some(b) => b.0,
                    None => {
                        commands.entity(e).insert(crate::cutscene::MoverBase(*tr));
                        *tr
                    }
                };
                let d = glam::Mat4::from_rotation_translation(glam::Quat::from_array(x.rotation), glam::Vec3::from(x.translation));
                let want = Transform::from_matrix(coords::transform(d) * base.to_matrix());
                if *tr != want {
                    *tr = want;
                }
            }
            (None, Some(b)) => {
                if *tr != b.0 {
                    *tr = b.0;
                }
            }
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn door() -> DoorAnims {
        // DBDoors_big_rich_ext_metal_twk: the stages name the opposite direction's animation.
        DoorAnims { initial: 0, open_cw: "Player_Open_Ccw".into(), open_ccw: "Player_Open_Cw".into(), close_cw: "Player_Close_Ccw".into(), close_ccw: "Player_Close_Cw".into() }
    }

    #[test]
    fn doors_play_the_stage_of_their_new_state() {
        let d = door();
        assert_eq!(transition_anim(&d, 0, 1), "Player_Open_Ccw");
        assert_eq!(transition_anim(&d, 0, 2), "Player_Open_Cw");
        assert_eq!(transition_anim(&d, 1, 0), "Player_Close_Ccw");
        assert_eq!(transition_anim(&d, 2, 0), "Player_Close_Cw");
        // Open doors rest on their open stage, closed ones in the reference pose.
        assert_eq!(rest_anim(&d, 1), Some("Player_Open_Ccw"));
        assert_eq!(rest_anim(&d, 0), None);
        assert_eq!(rest_anim(&DoorAnims::default(), 2), None);
    }

    #[test]
    fn movers_match_actor_and_placement() {
        let mv = vec![MoverView { actor: "L_A:InterpActor_1".into(), placed: [10.0, 0.0, 0.0], translation: [0.0, 0.0, 50.0], rotation: [0.0, 0.0, 0.0, 1.0] }];
        let d = mover_delta(&mv, "l_a:interpactor_1", glam::Vec3::new(10.0, 0.0, 0.0)).unwrap();
        assert_eq!(d.w_axis.truncate(), glam::Vec3::new(0.0, 0.0, 50.0));
        // Same name in another sublevel (elsewhere): not moved.
        assert!(mover_delta(&mv, "L_A:InterpActor_1", glam::Vec3::new(5000.0, 0.0, 0.0)).is_none());
    }
}
