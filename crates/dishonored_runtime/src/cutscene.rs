//! [interp] Cutscene presentation: matinees replicated by the mission server
//! (`MissionView::cutscenes`) are evaluated here at their position with the same data and code
//! as the server (`dishonored_kismet::interp`), for what only presents:
//! - level actors the matinee moves (`InterpActor` meshes, kept unmerged by `map_render`);
//! - skeletal mesh props bound to a group (Samuel's boat `SmallBoat`), spawned while the matinee
//!   plays and animated by the group's anim tracks;
//! - anim tracks on pawns (AI groups) through `npc_anim` (`PawnAnim::scripted`), by sequence
//!   name, with Arkane's stretched keys;
//! - AkEvent tracks through the audio engine, on the bound object;
//! - the director group: camera cuts to camera groups, and the fade track (a black overlay).
//!
//! - visibility tracks on skeletal props, toggle tracks on placed emitters, and props /
//!   emitters attached to a pawn's animated bone (U-208);
//! - the player group's full-body animation on the first-person eye, and its look limits
//!   (U-203).
//!
//! Pawn and player positions come from the server (it moves them deterministically), so the
//! first-person camera of a player held by a player group follows the matinee by itself; the
//! group's full-body keys only offset the eye from there.

use crate::characters::{CharacterGpu, Joint, to_bevy};
use crate::coords;
use crate::npc_anim::{AnimBank, Pose, Scripted};
use crate::players::{GameCamera, PlayerBody};
use crate::session::SessionView;
use bevy::mesh::skinning::SkinnedMeshInverseBindposes;
use bevy::prelude::*;
use dishonored_kismet::interp::{self, Matinee, TrackKind};
use dishonored_mission::runtime::{CutsceneBound, CutsceneView};
use std::collections::HashMap;
use std::sync::Arc;

/// A static mesh instance of a level actor a matinee may move (`InterpActor`).
#[derive(Component, Clone, Debug)]
pub struct MovableActor {
    /// Actor object name (`InterpActor_1`).
    pub actor: String,
    /// The instance's translation in Dishonored space (to tell same-named actors of different
    /// sublevels apart).
    pub origin: glam::Vec3,
}

/// Original transform of a moved instance.
#[derive(Component, Clone, Copy)]
pub struct MoverBase(pub Transform);

/// [interp] `map_render` hook: instances of `InterpActor`s stay movable.
pub fn mover_of(source: &str, transform: glam::Mat4) -> Option<MovableActor> {
    let (path, class) = source.rsplit_once(" [")?;
    if !class.trim_end_matches(']').starts_with("InterpActor") {
        return None;
    }
    let parts: Vec<&str> = path.split('.').collect();
    let actor = parts.get(parts.len().checked_sub(2)?)?.to_string();
    Some(MovableActor { actor, origin: transform.w_axis.truncate() })
}

/// A skeletal mesh prop spawned for a matinee group.
#[derive(Component)]
pub struct CutsceneProp {
    pub key: String,
}

#[derive(Component)]
pub struct FadeOverlay;

struct PropModel {
    gpu: CharacterGpu,
    profile: Option<usize>,
    /// The actor's `bHidden` (props shown by a visibility track start hidden).
    hidden: bool,
}

/// [interp] Bones and sockets of pawns that matinee attachments name (a glass in an NPC's
/// `RightHandWpn`, smoke on `Puke_Spit`): requested by the cutscene systems, filled by
/// `characters::animate_characters` from the animated pose (Dishonored world transforms; the
/// previous frame's pose).
#[derive(Resource, Default)]
pub struct PawnPoints {
    /// (pawn, bone or socket name, lowercase).
    pub wanted: Vec<(u32, String)>,
    pub points: HashMap<(u32, String), glam::Mat4>,
}

/// [interp] Bevy world matrix -> Dishonored world matrix (inverse of `coords::transform`).
pub fn from_bevy(m: Mat4) -> glam::Mat4 {
    let swap = Mat4::from_cols(Vec4::X, Vec4::Z, Vec4::Y, Vec4::W);
    let c = Mat4::from_scale(Vec3::splat(coords::UU_TO_BEVY)) * swap;
    let c_inv = swap * Mat4::from_scale(Vec3::splat(1.0 / coords::UU_TO_BEVY));
    c_inv * m * c
}

/// [interp] First-person rig (Engine `Ply_Player.Skm_Player`) for the player group's camera.
struct Rig {
    bones: Vec<dishonored_ir::BoneAsset>,
    mesh_to_pawn: glam::Mat4,
    camera: usize,
}

#[derive(Resource, Default)]
pub struct Cutscenes {
    content: Option<Arc<dishonored_assets::Content>>,
    /// (package, export) -> matinee.
    data: HashMap<(String, u32), Option<Arc<Matinee>>>,
    /// Prop key (package:actor) -> model (None: could not load).
    models: HashMap<String, Option<PropModel>>,
    /// Matinee key -> last position presented (keys crossed since then fire).
    last: HashMap<(String, u32), f32>,
    /// Playing ids started by AkEvent tracks, per (matinee, group, track).
    playing: HashMap<(String, u32, usize, usize), Vec<u32>>,
    next_obj: u64,
    /// Matinee keys seen this frame.
    seen: Vec<(String, u32)>,
    pub fade: f32,
    pub posted: u64,
    /// Where objects attached to a pawn bone were last placed (they stay there when detached).
    placed: HashMap<String, glam::Mat4>,
    /// Emitters started by toggle tracks: "package:actor" -> instance.
    emitters: HashMap<String, Entity>,
    /// Matinee key -> last position the emitters saw (trigger keys crossed).
    emit_last: HashMap<(String, u32), f32>,
    /// [interp] First-person rig (None inside: could not load).
    rig: Option<Option<Arc<Rig>>>,
    /// Offset of the local player's eye from its standing eye while a player group's
    /// full-body animation drives the camera (U-203), Dishonored world units.
    pub eye_offset: Option<glam::Vec3>,
    /// Look limits of the local player's group (`DisInterpTrackAllowPlayerLook`): the yaw
    /// they are centred on (degrees) and (yaw min, yaw max, pitch min, pitch max).
    pub look: Option<(f32, [f32; 4])>,
}

const CUTSCENE_OBJ: u64 = 6 << 32;

impl Cutscenes {
    fn content(&mut self) -> Option<Arc<dishonored_assets::Content>> {
        if self.content.is_none() {
            self.content = dishonored_assets::Content::locate().ok().map(Arc::new);
        }
        self.content.clone()
    }

    fn matinee(&mut self, c: &CutsceneView) -> Option<Arc<Matinee>> {
        let key = (c.package.clone(), c.export);
        if let Some(m) = self.data.get(&key) {
            return m.clone();
        }
        let m = self.content().and_then(|ct| ct.package(&c.package).ok()).and_then(|p| interp::load(&p, c.export as usize)).map(Arc::new);
        if m.is_none() {
            warn!("cutscene: no matinee at {}[{}]", c.package, c.export);
        }
        self.data.insert(key, m.clone());
        m
    }

    /// [interp] Corvo's first-person rig (Engine `Ply_Player.Skm_Player`), loaded once.
    fn rig(&mut self) -> Option<Arc<Rig>> {
        if self.rig.is_none() {
            use ue3_package::ObjRef;
            let r = self.content().and_then(|c| {
                let pkg = c.package("Engine").ok()?;
                let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SkeletalMesh" && pkg.obj_path(ObjRef::Export(i)) == "Ply_Player.Skm_Player")?;
                let m = dishonored_assets::skeletal_mesh::read_skeletal_mesh(&pkg, i).ok()?;
                let bones: Vec<dishonored_ir::BoneAsset> = m.bones.iter().map(|b| dishonored_ir::BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
                let camera = bones.iter().position(|b| b.name.eq_ignore_ascii_case("camera_jnt"))?;
                let mesh_to_pawn = dishonored_assets::util::rotator_matrix(m.rot_origin[0], m.rot_origin[1], m.rot_origin[2]) * glam::Mat4::from_translation(m.mesh_origin);
                Some(Arc::new(Rig { bones, mesh_to_pawn, camera }))
            });
            self.rig = Some(r);
        }
        self.rig.clone().flatten()
    }
}

/// [interp] `camera_jnt` of the first-person rig posed by (sequence, time, weight) samples
/// (each blended over the previous, every bone's rotation and translation: the player sets
/// are authored on this skeleton): pawn-space position and view direction (the viewmodel's
/// convention: the joint's -Z looks forward).
fn rig_camera(bank: &mut AnimBank, rig: &Rig, profile: usize, anims: &[(String, f32, f32)]) -> (glam::Vec3, glam::Vec3) {
    let mut bones = rig.bones.clone();
    for (n, t, w) in anims {
        let Some((set, clip)) = bank.clip(profile, n) else { continue };
        let mut tracks = bank.sets[set].tracks.clone();
        tracks.sort_by_key(|x| rig.bones.iter().position(|b| b.name.eq_ignore_ascii_case(x)).unwrap_or(usize::MAX));
        let k = w.min(1.0);
        for (j, tn) in tracks.iter().enumerate() {
            let Some(b) = rig.bones.iter().position(|x| x.name.eq_ignore_ascii_case(tn)) else { continue };
            let (q, tr) = clip.sample(t.min(clip.duration), j);
            if let Some(q) = q {
                let q = if bones[b].rotation.dot(q) < 0.0 { -q } else { q };
                bones[b].rotation = bones[b].rotation.lerp(q, k).normalize();
            }
            if let Some(tr) = tr {
                bones[b].translation = bones[b].translation.lerp(tr, k);
            }
        }
    }
    let g = rig.mesh_to_pawn * dishonored_assets::characters::bind_globals(&bones)[rig.camera];
    (g.w_axis.truncate(), -g.z_axis.truncate())
}

fn xform_of(t: &([f32; 3], [f32; 4])) -> glam::Mat4 {
    glam::Mat4::from_rotation_translation(glam::Quat::from_array(t.1), glam::Vec3::from(t.0))
}

fn bound_now(b: &CutsceneBound) -> glam::Mat4 {
    xform_of(b.current.as_ref().unwrap_or(&b.initial))
}

/// [interp] Where the object of group `gi` (key `key`) is while an attachment track holds it on
/// a pawn's bone or socket (an NPC's `RightHandWpn`): the pawn's animated point times the key's
/// relative offset and rotation, once the pawn's pose has given the point. A detached object
/// stays where it was last placed. None: not on a pawn bone (the server's pose applies; the
/// server places attachments to pawns at the pawn's origin).
fn pawn_attachment(m: &Matinee, c: &CutsceneView, gi: usize, t: f32, key: &str, points: &mut PawnPoints, placed: &mut HashMap<String, glam::Mat4>) -> Option<glam::Mat4> {
    let (pi, k) = interp::Scene::attachment(m, gi, t)?;
    if k.detach {
        return placed.get(key).copied();
    }
    if k.bone.is_empty() {
        return None;
    }
    let parent = &m.groups[pi];
    let pawn = c.groups.iter().find(|g| g.0.eq_ignore_ascii_case(&parent.name))?.1.first()?.pawn?;
    let want = (pawn, k.bone.to_ascii_lowercase());
    if !points.wanted.contains(&want) {
        points.wanted.push(want.clone());
    }
    let base = *points.points.get(&want)?;
    let rot = if k.use_rotation { glam::Quat::from_mat3(&interp::rot_matrix_units(k.rotation)) } else { glam::Quat::IDENTITY };
    let off = if k.use_offset { glam::Vec3::from(k.offset) } else { glam::Vec3::ZERO };
    let x = base * glam::Mat4::from_rotation_translation(rot, off);
    placed.insert(key.to_string(), x);
    Some(x)
}

/// Sequence info through a profile (length, soiree markers).
fn seq_fn<'a>(bank: &'a std::cell::RefCell<&mut AnimBank>, profile: usize) -> impl Fn(&str) -> Option<interp::SeqInfo> + 'a {
    move |n: &str| bank.borrow_mut().seq_info(profile, n)
}

/// Anim samples of a group's tracks at `t`: (sequence, position, weight, root motion) in track
/// order. Root motion: the key extracts the root bone's translation
/// (`m_MatineeRootMotionMode`), which moves the pawn on the server; the mesh keeps the root at
/// its bind translation.
fn group_anims(m: &Matinee, group: &str, t: f32, bank: &mut AnimBank, profile: usize) -> Vec<(String, f32, f32, bool)> {
    let Some(g) = m.group(group) else { return Vec::new() };
    let cell = std::cell::RefCell::new(bank);
    let f = seq_fn(&cell, profile);
    g.tracks
        .iter()
        .filter(|tr| !tr.disabled)
        .filter_map(|tr| {
            let (s, w) = tr.anim(t, &f)?;
            let rm = match &tr.kind {
                TrackKind::Anim { keys, .. } => keys.get(s.key).is_some_and(|k| k.root_motion != 0),
                _ => false,
            };
            Some((s, w, rm))
        })
        .filter(|(s, w, _)| *w > 1e-3 && !s.name.is_empty())
        .map(|(s, w, rm)| (s.name, s.position, w, rm))
        .collect()
}

/// Matinee data, movers, pawn anims, sounds and fade (before the characters are animated).
#[allow(clippy::too_many_arguments)]
pub fn drive(
    mut cs: ResMut<Cutscenes>,
    view: Res<SessionView>,
    bank: Option<ResMut<AnimBank>>,
    audio: Option<Res<crate::audio::GameAudio>>,
    mut movers: Query<(Entity, &MovableActor, &mut Transform, Option<&MoverBase>)>,
    mut pawns: Query<(&PlayerBody, &mut crate::npc_anim::PawnAnim)>,
    mut fade_q: Query<(Entity, &mut BackgroundColor), With<FadeOverlay>>,
    mut commands: Commands,
    mut points: Option<ResMut<PawnPoints>>,
) {
    let list: Vec<CutsceneView> = view.matchv.mission.as_ref().map(|m| m.cutscenes.clone()).unwrap_or_default();
    let mut bank = bank;
    let mut fade = 0.0f32;
    let mut scripted: HashMap<u32, Vec<Scripted>> = HashMap::new();
    let mut seen = Vec::new();
    for c in &list {
        let Some(m) = cs.matinee(c) else { continue };
        let key = (c.package.clone(), c.export);
        seen.push(key.clone());
        let t = c.position;
        let prev = cs.last.get(&key).copied();
        // Level actors (InterpActor meshes).
        for (_, objs) in &c.groups {
            for b in objs {
                let Some((_, name, class)) = &b.actor else { continue };
                if !class.starts_with("InterpActor") {
                    continue;
                }
                let delta = bound_now(b) * xform_of(&b.initial).inverse();
                let init = glam::Vec3::from(b.initial.0);
                for (e, mv, mut tr, base) in &mut movers {
                    if !mv.actor.eq_ignore_ascii_case(name) || mv.origin.distance(init) > 400.0 {
                        continue;
                    }
                    let base = match base {
                        Some(b) => b.0,
                        None => {
                            commands.entity(e).insert(MoverBase(*tr));
                            *tr
                        }
                    };
                    *tr = Transform::from_matrix(coords::transform(delta) * base.to_matrix());
                }
            }
        }
        // Anim tracks on pawns of AI groups.
        if let Some(bank) = bank.as_mut() {
            for g in &m.groups {
                if !matches!(g.kind, interp::GroupKind::Ai { .. }) {
                    continue;
                }
                let Some((_, objs)) = c.groups.iter().find(|x| x.0.eq_ignore_ascii_case(&g.name)) else { continue };
                let profile = bank.sets_profile(&g.anim_sets);
                let anims = group_anims(&m, &g.name, t, bank, profile);
                for b in objs {
                    if let Some(p) = b.pawn {
                        scripted.entry(p).or_default().extend(anims.iter().map(|(n, time, w, rm)| Scripted { name: n.clone(), time: *time, weight: *w, sets: g.anim_sets.clone(), root_motion: *rm }));
                    }
                }
            }
        }
        // Sounds.
        if let (Some(prev), Some(engine)) = (prev, audio.as_ref().and_then(|a| a.engine.clone())) {
            if t > prev {
                let mut e = engine.lock().unwrap();
                for (gi, g) in m.groups.iter().enumerate() {
                    for (ti, tr) in g.tracks.iter().enumerate().filter(|(_, t)| !t.disabled) {
                        let TrackKind::AkEvent { keys } = &tr.kind else { continue };
                        for (kt, event, stop) in keys.iter().filter(|k| interp::crossed(k.0, prev, t)) {
                            let _ = kt;
                            let pk = (key.0.clone(), key.1, gi, ti);
                            if *stop {
                                for id in cs.playing.remove(&pk).unwrap_or_default() {
                                    e.stop_playing(id, 200.0);
                                }
                                continue;
                            }
                            // On the bound object (actor, pawn), else 2D.
                            let pos = c.groups.iter().find(|x| x.0.eq_ignore_ascii_case(&g.name)).and_then(|x| x.1.first()).and_then(|b| match b.pawn {
                                Some(p) => view.others.iter().chain(view.me.iter()).find(|x| x.id == p).map(|x| x.position),
                                None => Some(bound_now(b).w_axis.truncate()),
                            });
                            let obj = match (pos, matches!(g.kind, interp::GroupKind::Player { .. })) {
                                (Some(p), false) => {
                                    cs.next_obj += 1;
                                    let o = CUTSCENE_OBJ + (cs.next_obj & 0xffff);
                                    e.set_position(o, p.to_array());
                                    o
                                }
                                _ => dishonored_audio::engine::GLOBAL_OBJ,
                            };
                            let id = e.post_event_name(event, obj);
                            if id != 0 {
                                cs.posted += 1;
                                cs.playing.entry(pk).or_default().push(id);
                            }
                        }
                    }
                }
            }
        }
        cs.last.insert(key, t);
        if let Some(f) = m.fade(t) {
            fade = fade.max(f.clamp(0.0, 1.0));
        }
    }
    cs.last.retain(|k, _| seen.contains(k));
    if list.is_empty() {
        cs.placed.clear();
        if let Some(pp) = points.as_mut() {
            pp.wanted.clear();
            pp.points.clear();
        }
    }
    // The player group's full-body animation drives the first-person camera (U-203).
    cs.eye_offset = None;
    cs.look = None;
    if let (Some(bank), Some(me)) = (bank.as_mut(), view.me.as_ref()) {
        for c in list.iter().filter(|c| c.players.contains(&me.id)) {
            let Some(m) = cs.matinee(c) else { continue };
            for g in m.groups.iter().filter(|g| matches!(g.kind, interp::GroupKind::Player { .. })) {
                let Some(b) = c.groups.iter().find(|x| x.0.eq_ignore_ascii_case(&g.name)).and_then(|x| x.1.iter().find(|b| b.pawn == Some(me.id))) else { continue };
                // The pawn's frame: where the matinee holds it (the boat's `Player_Attach`).
                let frame = b.current.as_ref().map(|x| glam::Quat::from_array(x.1)).unwrap_or_else(|| glam::Quat::from_rotation_z(me.yaw.to_radians()));
                let yaw = {
                    let f = frame * glam::Vec3::X;
                    f.y.atan2(f.x).to_degrees()
                };
                let profile = bank.sets_profile(&g.anim_sets);
                let anims: Vec<(String, f32, f32)> = group_anims(&m, &g.name, c.position, bank, profile).into_iter().map(|a| (a.0, a.1, a.2)).collect();
                let mut cam_yaw = 0.0;
                if !anims.is_empty() {
                    if let Some(rig) = cs.rig() {
                        let reference = bank.sets_profile(&[("Startup".to_string(), "Ply_Sword_Idle_as.Ply_Sword_Idle_as".to_string())]);
                        let (cam, fwd) = rig_camera(bank, &rig, profile, &anims);
                        let (cam0, _) = rig_camera(bank, &rig, reference, &[("Sword_Ready_Idle".to_string(), 0.0, 1.0)]);
                        cs.eye_offset = Some(frame * (cam - cam0));
                        cam_yaw = fwd.y.atan2(fwd.x).to_degrees();
                    }
                }
                if let Some(lim) = m.player_look(c.position) {
                    cs.look = Some((yaw + cam_yaw, lim));
                }
            }
        }
    }
    cs.seen = seen;
    for (body, mut st) in &mut pawns {
        st.scripted = scripted.remove(&body.0).unwrap_or_default();
    }
    // Movers of matinees no longer playing go back (they keep their place in the original
    // only while the level actor stays where the matinee left it; the server's actor
    // positions are not replicated outside matinees).
    // Fade overlay.
    cs.fade = fade;
    match fade_q.single_mut() {
        Ok((_, mut bg)) => bg.0 = Color::BLACK.with_alpha(fade),
        Err(_) if fade > 0.0 => {
            commands.spawn((Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), ..default() }, BackgroundColor(Color::BLACK.with_alpha(fade)), GlobalZIndex(90), Pickable::IGNORE, FadeOverlay));
        }
        Err(_) => {}
    }
}

/// Skeletal mesh props bound to matinee groups: spawned while their matinee plays, posed by
/// the group's anim tracks, removed after.
#[allow(clippy::too_many_arguments)]
pub fn props(
    mut cs: ResMut<Cutscenes>,
    view: Res<SessionView>,
    bank: Option<ResMut<AnimBank>>,
    features: Option<Res<crate::map_render::RenderFeatures>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut commands: Commands,
    mut roots: Query<(Entity, &CutsceneProp, &mut Transform, &mut Visibility, &Children)>,
    mut joints: Query<(&Joint, &mut Transform), Without<CutsceneProp>>,
    mut points: ResMut<PawnPoints>,
) {
    let Some(mut bank) = bank else { return };
    let list: Vec<CutsceneView> = view.matchv.mission.as_ref().map(|m| m.cutscenes.clone()).unwrap_or_default();
    let mut live: Vec<String> = Vec::new();
    for c in &list {
        let Some(m) = cs.matinee(c) else { continue };
        for (gname, objs) in &c.groups {
            for b in objs {
                let Some((pkg, name, class)) = &b.actor else { continue };
                if !class.starts_with("SkeletalMeshActor") {
                    continue;
                }
                let key = format!("{pkg}:{name}");
                live.push(key.clone());
                if !cs.models.contains_key(&key) {
                    let feat = features.as_deref().cloned().unwrap_or_else(|| crate::map_render::RenderFeatures::from_flags(&|_| false));
                    let model = cs.content().and_then(|ct| load_prop(&ct, pkg, name)).map(|(cm, hidden)| {
                        let gpu = crate::characters::upload(&cm, &mut meshes, &mut images, &mut mats, &mut binds, &feat);
                        let sets = m.group(gname).map(|g| g.anim_sets.clone()).unwrap_or_default();
                        PropModel { gpu, profile: (!sets.is_empty()).then(|| bank.sets_profile(&sets)), hidden }
                    });
                    if let Some(pm) = &model {
                        let root = commands.spawn((Transform::from_matrix(coords::transform(bound_now(b))), Visibility::default(), CutsceneProp { key: key.clone() })).id();
                        for (pi, part) in pm.gpu.parts.iter().enumerate() {
                            let js: Vec<Entity> = part.bind.iter().enumerate().map(|(bi, bm)| commands.spawn((Transform::from_matrix(*bm), Joint { part: pi, bone: bi }, ChildOf(root))).id()).collect();
                            for (mesh, mat) in &part.sections {
                                commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), bevy::mesh::skinning::SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: js.clone() }, Transform::default(), ChildOf(root)));
                            }
                        }
                        info!("cutscene prop {key} ({} parts)", pm.gpu.parts.len());
                    } else {
                        warn!("cutscene prop {key}: no skeletal mesh");
                    }
                    cs.models.insert(key.clone(), model);
                }
                // Pose.
                let Some(gi) = m.groups.iter().position(|g| g.name.eq_ignore_ascii_case(gname)) else { continue };
                let on_pawn = {
                    let Cutscenes { placed, .. } = &mut *cs;
                    pawn_attachment(&m, c, gi, c.position, &key, &mut points, placed)
                };
                let Some(Some(pm)) = cs.models.get(&key) else { continue };
                let Some((_, _, mut tr, mut vis, children)) = roots.iter_mut().find(|r| r.1.key == key) else { continue };
                *tr = Transform::from_matrix(coords::transform(on_pawn.unwrap_or_else(|| bound_now(b))));
                // Visibility tracks (U-208): props such as the glasses and cigarettes the
                // distraction soirees put in an NPC's hand are hidden in the level and shown by
                // their track while held.
                *vis = if m.visible(gi, c.position, !pm.hidden) { Visibility::Inherited } else { Visibility::Hidden };
                let Some(profile) = pm.profile else { continue };
                let parts = pm.gpu.parts.clone();
                let anims = group_anims(&m, gname, c.position, &mut bank, profile);
                let mut pose: Pose = parts.iter().map(|p| vec![None; p.bones.len()]).collect();
                for (n, time, w, _) in anims {
                    if let Some(sp) = bank.sample_parts(&parts, profile, &n, time) {
                        crate::npc_anim::blend_into(&mut pose, &sp, w, &parts);
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
            }
        }
    }
    for (e, p, _, _, _) in &roots {
        if !live.contains(&p.key) {
            commands.entity(e).despawn();
            cs.models.remove(&p.key);
        }
    }
}

/// Director camera cuts (camera groups) override the game camera. Without a cut, a player group
/// whose full-body animation moves the rig's `camera_jnt` (Samuel's boat: `SC_boat`, seated)
/// moves the local player's first-person eye with it (U-203).
pub fn camera(cs: Res<Cutscenes>, view: Res<SessionView>, mut q: Query<(&mut Transform, &GameCamera)>) {
    let list: Vec<CutsceneView> = view.matchv.mission.as_ref().map(|m| m.cutscenes.clone()).unwrap_or_default();
    let mut cut_done = false;
    for c in &list {
        let Some(Some(m)) = cs.data.get(&(c.package.clone(), c.export)) else { continue };
        let Some((cut, _)) = m.director_cut(c.position) else { continue };
        let Some(b) = c.groups.iter().find(|g| g.0.eq_ignore_ascii_case(&cut.group)).and_then(|g| g.1.first()) else { continue };
        let x = bound_now(b);
        let eye = coords::pos(x.w_axis.truncate());
        let fwd = coords::dir(x.x_axis.truncate());
        for (mut t, _) in &mut q {
            *t = Transform::from_translation(eye).looking_to(fwd, Vec3::Y);
        }
        cut_done = true;
    }
    if let (false, Some(off), Some(me)) = (cut_done, cs.eye_offset, view.me.as_ref()) {
        for (mut t, gc) in &mut q {
            if !gc.third_person && gc.follow.is_none_or(|f| f == me.id) {
                t.translation += coords::pos(off);
            }
        }
    }
}

/// [interp] `DisInterpTrackAllowPlayerLook` (U-203): while the local player's group holds them,
/// their view stays within the key's yaw and pitch limits around the matinee's frame (the
/// pawn's attachment frame turned by the animated camera bone).
pub fn look_limits(cs: Res<Cutscenes>, mut local: ResMut<crate::input_map::LocalInput>) {
    let Some((base, [y0, y1, p0, p1])) = cs.look else { return };
    let rel = (local.yaw - base + 180.0).rem_euclid(360.0) - 180.0;
    let clamped = rel.clamp(y0.min(y1), y0.max(y1));
    if (clamped - rel).abs() > 1e-4 {
        local.yaw = (base + clamped).rem_euclid(360.0);
    }
    local.pitch = local.pitch.clamp(p0.min(p1), p0.max(p1));
}

/// [interp] Emitters bound to matinee groups (U-208): `InterpTrackToggle` keys start (`ETTA_On`),
/// stop (`ETTA_Off`, the particles die out) or flip (`ETTA_Toggle`) the placed emitter, or fire
/// it once (`ETTA_Trigger`). The emitter follows its attachment (a pawn's socket, e.g. the
/// cigarette smoke on `Puke_Spit`), else the server's pose, else its level transform. Emitters
/// the level already plays (`bAutoActivate`) and emitters based on another actor (not imported
/// by the level loader, e.g. `FX_SmallBoat`'s `Emitter_179`) are left alone.
#[allow(clippy::too_many_arguments)]
pub fn emitters(
    mut cs: ResMut<Cutscenes>,
    view: Res<SessionView>,
    mut points: ResMut<PawnPoints>,
    fx: Option<ResMut<crate::effects::Effects>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
    mut q: Query<&mut crate::effects::FxInstance>,
) {
    let Some(mut fx) = fx else { return };
    let list: Vec<CutsceneView> = view.matchv.mission.as_ref().map(|m| m.cutscenes.clone()).unwrap_or_default();
    let mut live: Vec<String> = Vec::new();
    for c in &list {
        let Some(m) = cs.matinee(c) else { continue };
        let mkey = (c.package.clone(), c.export);
        let t = c.position;
        let prev = cs.emit_last.get(&mkey).copied().unwrap_or(t);
        for (gname, objs) in &c.groups {
            let Some(gi) = m.groups.iter().position(|g| g.name.eq_ignore_ascii_case(gname)) else { continue };
            if !m.groups[gi].tracks.iter().any(|tr| !tr.disabled && matches!(tr.kind, TrackKind::Toggle { .. })) {
                continue;
            }
            for b in objs {
                let Some((pkg, name, class)) = &b.actor else { continue };
                if !class.starts_with("Emitter") {
                    continue;
                }
                let key = format!("{pkg}:{name}").to_ascii_lowercase();
                let Some((_, _, level_xf, auto)) = crate::effects::placed_emitter(&fx, &key) else { continue };
                if auto {
                    continue;
                }
                let on_pawn = {
                    let Cutscenes { placed, .. } = &mut *cs;
                    pawn_attachment(&m, c, gi, t, &key, &mut points, placed)
                };
                let xf = on_pawn.or_else(|| b.current.as_ref().map(xform_of)).unwrap_or(level_xf);
                if m.toggled_on(gi, t, false) {
                    live.push(key.clone());
                    match cs.emitters.get(&key).copied() {
                        Some(e) => {
                            if let Ok(mut i) = q.get_mut(e) {
                                i.set_transform(xf);
                            }
                        }
                        None => {
                            if let Some(e) = crate::effects::start_placed_emitter(&mut commands, &mut meshes, &mut fx, &key, xf, false) {
                                cs.emitters.insert(key.clone(), e);
                            }
                        }
                    }
                }
                for _ in 0..m.triggers(gi, prev, t) {
                    crate::effects::start_placed_emitter(&mut commands, &mut meshes, &mut fx, &key, xf, true);
                }
            }
        }
        cs.emit_last.insert(mkey, t);
    }
    let stopped: Vec<String> = cs.emitters.keys().filter(|k| !live.contains(k)).cloned().collect();
    for k in stopped {
        if let Some(e) = cs.emitters.remove(&k) {
            if let Ok(mut i) = q.get_mut(e) {
                i.stop();
            }
        }
    }
    cs.emit_last.retain(|k, _| list.iter().any(|c| c.package == k.0 && c.export == k.1));
}

/// The skeletal mesh of a level's `SkeletalMeshActor` (its `SkeletalMeshComponent`), and the
/// actor's `bHidden`.
fn load_prop(content: &dishonored_assets::Content, package: &str, actor: &str) -> Option<(dishonored_ir::CharacterModel, bool)> {
    use ue3_package::ObjRef;
    use ue3_package::properties::{Value, find};
    let pkg = content.package(package).ok()?;
    let e = (0..pkg.exports.len()).find(|&i| pkg.obj_name(ObjRef::Export(i)).eq_ignore_ascii_case(actor) && pkg.obj_path(ObjRef::Export(i)).starts_with("TheWorld.PersistentLevel."))?;
    let props = pkg.read_object(e).ok()?.properties;
    let hidden = matches!(find(&props, "bHidden").map(|p| &p.value), Some(Value::Bool(true)));
    let ObjRef::Export(c) = ObjRef::from_index(find(&props, "SkeletalMeshComponent")?.as_object()?) else { return None };
    let ch = pkg.read_object(c).ok()?;
    let r = ObjRef::from_index(find(&ch.properties, "SkeletalMesh")?.as_object()?);
    let path = pkg.obj_path(r);
    let owner = match r {
        ObjRef::Import(_) => path.split('.').next()?.to_string(),
        _ => package.to_string(),
    };
    dishonored_assets::characters::load_model(content, actor, &[(owner.as_str(), path.as_str())], None, 1024).map_err(|e| warn!("cutscene prop {actor}: {e:#}")).ok().map(|m| (m, hidden))
}
