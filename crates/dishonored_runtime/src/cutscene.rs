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
//! Pawn and player positions come from the server (it moves them deterministically), so the
//! first-person camera of a player held by a player group follows the matinee by itself.

use crate::characters::{CharacterGpu, CharacterLibrary, Joint, to_bevy};
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
}

fn xform_of(t: &([f32; 3], [f32; 4])) -> glam::Mat4 {
    glam::Mat4::from_rotation_translation(glam::Quat::from_array(t.1), glam::Vec3::from(t.0))
}

fn bound_now(b: &CutsceneBound) -> glam::Mat4 {
    xform_of(b.current.as_ref().unwrap_or(&b.initial))
}

/// Sequence info through a profile (length, soiree markers).
fn seq_fn<'a>(bank: &'a std::cell::RefCell<&mut AnimBank>, profile: usize) -> impl Fn(&str) -> Option<interp::SeqInfo> + 'a {
    move |n: &str| bank.borrow_mut().seq_info(profile, n)
}

/// Anim samples of a group's tracks at `t`: (sequence, position, weight) in track order.
fn group_anims(m: &Matinee, group: &str, t: f32, bank: &mut AnimBank, profile: usize) -> Vec<(String, f32, f32)> {
    let Some(g) = m.group(group) else { return Vec::new() };
    let cell = std::cell::RefCell::new(bank);
    let f = seq_fn(&cell, profile);
    g.tracks.iter().filter(|tr| !tr.disabled).filter_map(|tr| tr.anim(t, &f)).filter(|(s, w)| *w > 1e-3 && !s.name.is_empty()).map(|(s, w)| (s.name, s.position, w)).collect()
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
                        scripted.entry(p).or_default().extend(anims.iter().map(|(n, time, w)| Scripted { name: n.clone(), time: *time, weight: *w, sets: g.anim_sets.clone() }));
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
    mut roots: Query<(Entity, &CutsceneProp, &mut Transform, &Children)>,
    mut joints: Query<(&Joint, &mut Transform), Without<CutsceneProp>>,
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
                    let model = cs.content().and_then(|ct| load_prop(&ct, pkg, name)).map(|cm| {
                        let gpu = crate::characters::upload(&cm, &mut meshes, &mut images, &mut mats, &mut binds, &feat);
                        let sets = m.group(gname).map(|g| g.anim_sets.clone()).unwrap_or_default();
                        PropModel { gpu, profile: (!sets.is_empty()).then(|| bank.sets_profile(&sets)) }
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
                let Some(Some(pm)) = cs.models.get(&key) else { continue };
                let Some((_, _, mut tr, children)) = roots.iter_mut().find(|r| r.1.key == key) else { continue };
                *tr = Transform::from_matrix(coords::transform(bound_now(b)));
                let Some(profile) = pm.profile else { continue };
                let parts = pm.gpu.parts.clone();
                let anims = group_anims(&m, gname, c.position, &mut bank, profile);
                let mut pose: Pose = parts.iter().map(|p| vec![None; p.bones.len()]).collect();
                for (n, time, w) in anims {
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
    for (e, p, _, _) in &roots {
        if !live.contains(&p.key) {
            commands.entity(e).despawn();
            cs.models.remove(&p.key);
        }
    }
}

/// Director camera cuts (camera groups) override the game camera.
pub fn camera(cs: Res<Cutscenes>, view: Res<SessionView>, mut q: Query<&mut Transform, With<GameCamera>>) {
    let list: Vec<CutsceneView> = view.matchv.mission.as_ref().map(|m| m.cutscenes.clone()).unwrap_or_default();
    for c in &list {
        let Some(Some(m)) = cs.data.get(&(c.package.clone(), c.export)) else { continue };
        let Some((cut, _)) = m.director_cut(c.position) else { continue };
        let Some(b) = c.groups.iter().find(|g| g.0.eq_ignore_ascii_case(&cut.group)).and_then(|g| g.1.first()) else { continue };
        let x = bound_now(b);
        let eye = coords::pos(x.w_axis.truncate());
        let fwd = coords::dir(x.x_axis.truncate());
        for mut t in &mut q {
            *t = Transform::from_translation(eye).looking_to(fwd, Vec3::Y);
        }
    }
}

/// The skeletal mesh of a level's `SkeletalMeshActor` (its `SkeletalMeshComponent`).
fn load_prop(content: &dishonored_assets::Content, package: &str, actor: &str) -> Option<dishonored_ir::CharacterModel> {
    use ue3_package::ObjRef;
    use ue3_package::properties::find;
    let pkg = content.package(package).ok()?;
    let e = (0..pkg.exports.len()).find(|&i| pkg.obj_name(ObjRef::Export(i)).eq_ignore_ascii_case(actor) && pkg.obj_path(ObjRef::Export(i)).starts_with("TheWorld.PersistentLevel."))?;
    let props = pkg.read_object(e).ok()?.properties;
    let ObjRef::Export(c) = ObjRef::from_index(find(&props, "SkeletalMeshComponent")?.as_object()?) else { return None };
    let ch = pkg.read_object(c).ok()?;
    let r = ObjRef::from_index(find(&ch.properties, "SkeletalMesh")?.as_object()?);
    let path = pkg.obj_path(r);
    let owner = match r {
        ObjRef::Import(_) => path.split('.').next()?.to_string(),
        _ => package.to_string(),
    };
    dishonored_assets::characters::load_model(content, actor, &[(owner.as_str(), path.as_str())], None, 1024).map_err(|e| warn!("cutscene prop {actor}: {e:#}")).ok()
}
