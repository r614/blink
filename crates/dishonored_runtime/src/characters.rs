//! Skinned Dishonored character models for player bodies.
//!
//! Each part is uploaded once; every player gets a joint hierarchy (flat, children of the
//! body root) driven by decoded Edge animation clips (locomotion chosen from simulation
//! state); `procedural_pose` remains only as a fallback when a clip is unavailable.

use crate::coords;
use crate::map_render::to_image;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use dishonored_assets::characters::{ARCHETYPES, bind_globals, load_character};
use dishonored_ir::CharacterModel;

pub struct PartGpu {
    pub sections: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    pub inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
    /// Bind-pose joint transforms in Bevy space relative to the body root.
    pub bind: Vec<Mat4>,
    pub bones: Vec<dishonored_ir::BoneAsset>,
    pub mesh_to_pawn: glam::Mat4,
}

pub struct CharacterGpu {
    pub name: String,
    pub parts: Vec<PartGpu>,
}

#[derive(Resource, Default)]
pub struct CharacterLibrary {
    pub models: Vec<CharacterGpu>,
}

/// C: Dishonored pawn space (Z-up, uu) -> Bevy (Y-up, scaled).
fn to_bevy(m: glam::Mat4) -> Mat4 {
    coords::transform(m)
}

fn upload(model: &CharacterModel, meshes: &mut Assets<Mesh>, images: &mut Assets<Image>, mats: &mut Assets<StandardMaterial>, binds: &mut Assets<SkinnedMeshInverseBindposes>) -> CharacterGpu {
    let tex: Vec<Handle<Image>> = model.textures.iter().map(|t| images.add(to_image(t))).collect();
    let materials: Vec<Handle<StandardMaterial>> = model
        .materials
        .iter()
        .map(|m| {
            mats.add(StandardMaterial {
                base_color_texture: m.diffuse.map(|t| tex[t].clone()),
                base_color: if m.diffuse.is_some() { Color::WHITE } else { Color::srgb(0.5, 0.45, 0.4) },
                perceptual_roughness: 0.8,
                cull_mode: None,
                double_sided: true,
                ..default()
            })
        })
        .collect();
    let fallback = mats.add(StandardMaterial { base_color: Color::srgb(0.45, 0.4, 0.35), cull_mode: None, ..default() });
    let mut parts = Vec::new();
    for p in &model.parts {
        let m2p = p.mesh_to_pawn;
        let pos: Vec<[f32; 3]> = p.positions.iter().map(|v| coords::pos(m2p.transform_point3(*v)).to_array()).collect();
        let nrm: Vec<[f32; 3]> = p.normals.iter().map(|n| coords::dir(m2p.transform_vector3(*n)).normalize_or_zero().to_array()).collect();
        let uv: Vec<[f32; 2]> = p.uv0.iter().map(|u| u.to_array()).collect();
        let joints: Vec<[u16; 4]> = p.joints.clone();
        let weights: Vec<[f32; 4]> = p.weights.clone();
        let globals = bind_globals(&p.bones);
        let bind: Vec<Mat4> = globals.iter().map(|g| to_bevy(m2p * *g)).collect();
        let inverse: Vec<Mat4> = bind.iter().map(|b| b.inverse()).collect();
        let ib = binds.add(SkinnedMeshInverseBindposes::from(inverse));
        let mut sections = Vec::new();
        for s in &p.sections {
            let start = s.first_index as usize;
            let end = (start + s.num_triangles as usize * 3).min(p.indices.len());
            let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
                .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, nrm.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv.clone())
                .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(joints.clone()))
                .with_inserted_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, weights.clone())
                .with_inserted_indices(Indices::U32(p.indices[start..end].to_vec()));
            let mat = s.material.map(|m| materials[m].clone()).unwrap_or_else(|| fallback.clone());
            sections.push((meshes.add(mesh), mat));
        }
        parts.push(PartGpu { sections, inverse_bindposes: ib, bind, bones: p.bones.clone(), mesh_to_pawn: m2p });
    }
    CharacterGpu { name: model.name.clone(), parts }
}

pub fn load_library(
    mut commands: Commands,
    session: Res<crate::session::Session>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let mut lib = CharacterLibrary::default();
    let hh = session.content.sim.gameplay.movement.collision_half_height;
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    for (name, _, _) in ARCHETYPES {
        match load_character(&content, name, hh, 1024) {
            Ok(m) => lib.models.push(upload(&m, &mut meshes, &mut images, &mut mats, &mut binds)),
            Err(e) => warn!("character {name}: {e:#}"),
        }
    }
    info!("loaded {} character archetypes", lib.models.len());
    commands.insert_resource(lib);
}

#[derive(Component)]
pub struct Joint {
    pub part: usize,
    pub bone: usize,
}

#[derive(Component)]
pub struct CharacterRoot {
    pub archetype: usize,
}

/// Spawn a character under `root` (an existing entity that carries the pawn transform).
pub fn spawn_character(commands: &mut Commands, root: Entity, lib: &CharacterLibrary, archetype: usize) {
    let Some(model) = lib.models.get(archetype % lib.models.len().max(1)) else { return };
    commands.entity(root).insert(CharacterRoot { archetype: archetype % lib.models.len() });
    for (pi, part) in model.parts.iter().enumerate() {
        let joints: Vec<Entity> = part
            .bind
            .iter()
            .enumerate()
            .map(|(bi, b)| commands.spawn((Transform::from_matrix(*b), Joint { part: pi, bone: bi }, ChildOf(root))).id())
            .collect();
        for (mesh, mat) in &part.sections {
            commands.spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: joints.clone() },
                Transform::default(),
                ChildOf(root),
            ));
        }
    }
}

/// Placeholder pose: arms lowered from the bind pose and a speed-driven leg/arm swing.
/// Presentation only; replaced by decoded Edge animation once available.
pub fn procedural_pose(part: &PartGpu, speed: f32, phase: f32, crouched: bool) -> Vec<Mat4> {
    let mut locals: Vec<(glam::Quat, glam::Vec3)> = part.bones.iter().map(|b| (b.rotation, b.translation)).collect();
    let swing = (speed / 400.0).clamp(0.0, 1.5) * 0.6 * phase.sin();
    for (i, b) in part.bones.iter().enumerate() {
        let n = b.name.to_ascii_lowercase();
        let extra = if n.starts_with("upperarm") || n.starts_with("shoulder") || n.contains("arm_l") && n.contains("upper") {
            0.0
        } else if n.contains("thigh") || n.contains("upleg") || n.starts_with("hip_") {
            if n.contains("_l") { swing } else { -swing }
        } else if n.contains("knee") || n.contains("calf") || n.contains("leg_l_jnt") || n.contains("leg_r_jnt") {
            if crouched { 0.9 } else { swing.abs() * 0.8 }
        } else {
            0.0
        };
        if extra != 0.0 {
            locals[i].0 = locals[i].0 * glam::Quat::from_rotation_z(extra);
        }
    }
    let bones: Vec<dishonored_ir::BoneAsset> = part.bones.iter().zip(&locals).map(|(b, (q, t))| dishonored_ir::BoneAsset { rotation: *q, translation: *t, ..b.clone() }).collect();
    bind_globals(&bones).iter().map(|g| to_bevy(part.mesh_to_pawn * *g)).collect()
}

pub fn animate_characters(
    anims: Option<Res<Animations>>,
    mut commands: Commands,
    mut states: Query<&mut AnimState>,
    lib: Option<Res<CharacterLibrary>>,
    view: Res<crate::session::SessionView>,
    time: Res<Time>,
    roots: Query<(Entity, &CharacterRoot, &crate::players::PlayerBody, &Children)>,
    mut joints: Query<(&Joint, &mut Transform)>,
) {
    let Some(lib) = lib else { return };
    for (entity, root, body, children) in &roots {
        let Some(p) = view.others.iter().chain(view.me.iter()).find(|p| p.id == body.0) else { continue };
        let model = &lib.models[root.archetype];
        let speed = p.velocity.truncate().length();
        let phase = time.elapsed_secs() * (speed / 400.0).max(0.2) * 7.0 + p.id as f32;
        let poses: Vec<Vec<Mat4>> = match anims.as_ref() {
            Some(a) => {
                let (clip_name, rate) = choose_clip(p);
                let t = match states.get_mut(entity) {
                    Ok(mut s) => {
                        s.time += time.delta_secs() * rate;
                        s.time
                    }
                    Err(_) => {
                        commands.entity(entity).insert(AnimState::default());
                        0.0
                    }
                };
                match a.lib.clips.get(clip_name) {
                    Some(clip) => model.parts.iter().enumerate().map(|(pi, part)| decoded_pose(part, a, root.archetype, pi, clip, t + p.id as f32 * 0.37)).collect(),
                    None => model.parts.iter().map(|part| procedural_pose(part, speed, phase, p.crouched)).collect(),
                }
            }
            None => model.parts.iter().map(|part| procedural_pose(part, speed, phase, p.crouched)).collect(),
        };
        for c in children.iter() {
            if let Ok((j, mut t)) = joints.get_mut(c) {
                if let Some(m) = poses.get(j.part).and_then(|v| v.get(j.bone)) {
                    *t = Transform::from_matrix(*m);
                }
            }
        }
    }
}

/// Locomotion clip names in `Npc_EliteGuard_empty_as` (decoded Edge animation).
pub const LOCOMOTION: &[&str] = &["Empty_Idle", "Empty_WalkN", "Empty_WalkS_Revert", "Empty_WalkE", "Empty_WalkW_Revert", "Empty_RunN", "Empty_RunS_Revert", "Empty_RunE", "Empty_RunW_Revert"];

#[derive(Resource)]
pub struct Animations {
    pub lib: dishonored_assets::characters::AnimLibrary,
    /// Per model, per part: bone index for each track (None = bone missing).
    pub track_to_bone: Vec<Vec<Vec<Option<usize>>>>,
}

pub fn load_animations(mut commands: Commands, lib: Option<Res<CharacterLibrary>>) {
    let Some(lib) = lib else { return };
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    match dishonored_assets::characters::load_anim_set(&content, "L_Boyle_Ext_Script", "Npc_EliteGuard_empty_as.Npc_EliteGuard_empty_as", LOCOMOTION) {
        Ok(anims) => {
            let track_to_bone = lib
                .models
                .iter()
                .map(|m| m.parts.iter().map(|p| anims.tracks.iter().map(|t| p.bones.iter().position(|b| b.name.eq_ignore_ascii_case(t))).collect()).collect())
                .collect();
            info!("decoded {} Edge clips ({} tracks)", anims.clips.len(), anims.tracks.len());
            commands.insert_resource(Animations { lib: anims, track_to_bone });
        }
        Err(e) => warn!("animations: {e:#}"),
    }
}

/// Pick a locomotion clip from velocity relative to facing (simulation owns motion;
/// animation only presents it).
pub fn choose_clip(p: &dishonored_sim::state::PlayerState) -> (&'static str, f32) {
    let v = p.velocity.truncate();
    let speed = v.length();
    if speed < 20.0 || p.mode != dishonored_sim::state::MoveMode::Walking {
        return ("Empty_Idle", 1.0);
    }
    let fwd = dishonored_sim::movement::forward_vector(p.yaw).truncate();
    let right = dishonored_sim::movement::right_vector(p.yaw).truncate();
    let (f, r) = (v.dot(fwd), v.dot(right));
    let run = speed > 300.0;
    let name = if f.abs() >= r.abs() {
        match (f >= 0.0, run) {
            (true, false) => "Empty_WalkN",
            (true, true) => "Empty_RunN",
            (false, false) => "Empty_WalkS_Revert",
            (false, true) => "Empty_RunS_Revert",
        }
    } else {
        match (r >= 0.0, run) {
            (true, false) => "Empty_WalkE",
            (true, true) => "Empty_RunE",
            (false, false) => "Empty_WalkW_Revert",
            (false, true) => "Empty_RunW_Revert",
        }
    };
    // Playback rate follows ground speed (walk clips authored around 200 uu/s, run ~400).
    let rate = if run { speed / 400.0 } else { speed / 200.0 };
    (name, rate.clamp(0.5, 1.8))
}

#[derive(Component, Default)]
pub struct AnimState {
    pub time: f32,
}

pub fn decoded_pose(part: &PartGpu, anims: &Animations, model: usize, part_i: usize, clip: &dishonored_assets::edge_anim::BakedClip, time: f32) -> Vec<Mat4> {
    let mut bones = part.bones.clone();
    for (t, bone) in anims.track_to_bone[model][part_i].iter().enumerate() {
        let Some(b) = bone else { continue };
        let (r, _tr) = clip.sample(time, t);
        if let Some(q) = r {
            // Rotation-only retargeting: bind translations keep each body's proportions.
            bones[*b].rotation = q;
        }
    }
    bind_globals(&bones).iter().map(|g| to_bevy(part.mesh_to_pawn * *g)).collect()
}
