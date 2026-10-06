//! [interp] Animation the server needs from matinees, baked when the Kismet is loaded (feature
//! `load`): the mission server decodes no animation at run time, so
//! - attachment bones / sockets of skeletal mesh actors animated by their group
//!   ([`BoneTrack`], U-200: Samuel's boat rocks with `SC_boat_Idle`, and Samuel and the player
//!   ride its `Boat_Jnt` sockets), sampled over the matinee position with the same pose the
//!   presentation builds (`runtime::cutscene::props`);
//! - root bone tracks of the sequences AI groups play ([`RootTrack`], U-085 / U-202), for the
//!   root motion of keys with `m_MatineeRootMotionMode` and `SnapToRootBoneLocationWhenFinished`.

use crate::interp::{BoneTrack, Group, RootTrack, SeqInfo, TrackKind};
use crate::model::*;
use dishonored_assets::Content;
use dishonored_assets::edge_anim::BakedClip;
use glam::{Mat4, Quat, Vec3};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use ue3_package::properties::{Property, Value as PV, find};
use ue3_package::{ObjRef, Package};

/// Samples per second of baked tracks.
pub const BAKE_RATE: f32 = 30.0;

/// A skeletal mesh as attachments see it: bind bones, mesh -> actor transform
/// (`RotOrigin` * `Origin`), sockets (name, bone, relative transform).
pub struct Skeleton {
    pub bones: Vec<dishonored_ir::BoneAsset>,
    pub mesh_to_actor: Mat4,
    pub sockets: Vec<(String, usize, Mat4)>,
}

/// The skeletal mesh of a `SkeletalMeshActor` (its `SkeletalMeshComponent.SkeletalMesh`).
pub fn actor_skeleton(content: &Content, pkg: &Arc<Package>, props: &[Property]) -> Option<Skeleton> {
    let ObjRef::Export(c) = ObjRef::from_index(find(props, "SkeletalMeshComponent")?.as_object()?) else { return None };
    let ch = pkg.read_object(c).ok()?;
    let mesh = ObjRef::from_index(find(&ch.properties, "SkeletalMesh")?.as_object()?);
    let (mpkg, me) = match mesh {
        ObjRef::Export(m) => (pkg.clone(), m),
        r @ ObjRef::Import(_) => {
            let path = pkg.obj_path(r);
            let top = path.split('.').next()?.to_string();
            let other = content.package(&top).ok()?;
            let m = (0..other.exports.len()).find(|&i| other.export_class(i) == "SkeletalMesh" && other.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&path))?;
            (other, m)
        }
        ObjRef::Null => return None,
    };
    let raw = dishonored_assets::skeletal_mesh::read_skeletal_mesh(&mpkg, me).ok()?;
    let mesh_to_actor = Mat4::from_mat3(crate::interp::rot_matrix_units(raw.rot_origin)) * Mat4::from_translation(raw.mesh_origin);
    let bones: Vec<dishonored_ir::BoneAsset> = raw.bones.iter().map(|b| dishonored_ir::BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
    let mut sockets = Vec::new();
    let mp = mpkg.read_object(me).ok()?.properties;
    for s in find(&mp, "Sockets").and_then(|p| p.array_objects().ok()).unwrap_or_default() {
        let ObjRef::Export(se) = ObjRef::from_index(s) else { continue };
        let Ok(sh) = mpkg.read_object(se) else { continue };
        let nm = |n: &str| match find(&sh.properties, n).map(|p| &p.value) {
            Some(PV::Name(x)) => x.clone(),
            _ => String::new(),
        };
        let Some(bi) = bones.iter().position(|b| b.name.eq_ignore_ascii_case(&nm("BoneName"))) else { continue };
        let loc = find(&sh.properties, "RelativeLocation").and_then(|p| p.as_vector()).unwrap_or([0.0; 3]);
        let rot = find(&sh.properties, "RelativeRotation").and_then(|p| p.as_rotator()).unwrap_or([0; 3]);
        let rel = Mat4::from_translation(Vec3::from(loc)) * Mat4::from_mat3(crate::interp::rot_matrix_units(rot));
        sockets.push((nm("SocketName"), bi, rel));
    }
    Some(Skeleton { bones, mesh_to_actor, sockets })
}

impl Skeleton {
    /// Actor-space transform of a bone or socket for local bone transforms `locals`.
    pub fn point(&self, locals: &[dishonored_ir::BoneAsset], name: &str) -> Option<Mat4> {
        let g = dishonored_assets::characters::bind_globals(locals);
        if let Some(b) = self.bones.iter().position(|b| b.name.eq_ignore_ascii_case(name)) {
            return Some(self.mesh_to_actor * g[b]);
        }
        let (_, b, rel) = self.sockets.iter().find(|s| s.0.eq_ignore_ascii_case(name))?;
        Some(self.mesh_to_actor * g[*b] * *rel)
    }
}

/// One AnimSet: its track names and sequence exports.
struct SetData {
    pkg: Arc<Package>,
    tracks: Vec<String>,
    /// lowercase sequence name -> export
    seqs: HashMap<String, usize>,
}

/// AnimSets and decoded clips, loaded on demand.
struct Sets<'a> {
    content: &'a Content,
    sets: HashMap<String, Option<Arc<SetData>>>,
    clips: HashMap<(String, String), Option<Arc<BakedClip>>>,
    /// lowercase sequence name -> AnimSets holding it (package, path), over the loaded packages.
    by_seq: Option<HashMap<String, Vec<(String, String)>>>,
}

fn set_key(s: &(String, String)) -> String {
    format!("{}:{}", s.0, s.1).to_ascii_lowercase()
}

impl Sets<'_> {
    fn set(&mut self, s: &(String, String)) -> Option<Arc<SetData>> {
        let key = set_key(s);
        if let Some(d) = self.sets.get(&key) {
            return d.clone();
        }
        let d = (|| {
            let pkg = self.content.package(&s.0).ok()?;
            let e = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "AnimSet" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&s.1))?;
            let h = pkg.read_object(e).ok()?;
            let (n, raw) = find(&h.properties, "TrackBoneNames").and_then(|p| p.array_raw())?;
            let mut rd = ue3_package::reader::Reader::new(raw);
            let tracks: Vec<String> = (0..n).map(|_| ue3_package::FName::read(&mut rd).map(|f| pkg.name(f)).unwrap_or_default()).collect();
            let mut seqs = HashMap::new();
            for o in find(&h.properties, "Sequences").and_then(|p| p.array_objects().ok()).unwrap_or_default() {
                let ObjRef::Export(x) = ObjRef::from_index(o) else { continue };
                let Ok(sh) = pkg.read_object(x) else { continue };
                if let Some(PV::Name(name)) = find(&sh.properties, "SequenceName").map(|p| &p.value) {
                    seqs.insert(name.to_ascii_lowercase(), x);
                }
            }
            Some(Arc::new(SetData { pkg, tracks, seqs }))
        })();
        self.sets.insert(key, d.clone());
        d
    }

    /// The set of `sets` holding `seq`, searched from the last to the first (UE3
    /// `FindAnimSequence`).
    fn holder(&mut self, sets: &[(String, String)], seq: &str) -> Option<((String, String), Arc<SetData>, usize)> {
        let lower = seq.to_ascii_lowercase();
        for s in sets.iter().rev() {
            if let Some(d) = self.set(s) {
                if let Some(e) = d.seqs.get(&lower).copied() {
                    return Some((s.clone(), d, e));
                }
            }
        }
        None
    }

    fn clip(&mut self, set: &(String, String), d: &SetData, export: usize) -> Option<Arc<BakedClip>> {
        let key = (set_key(set), format!("{export}"));
        if let Some(c) = self.clips.get(&key) {
            return c.clone();
        }
        let c = (|| -> anyhow::Result<BakedClip> {
            let h = d.pkg.read_object(export)?;
            let data = d.pkg.export_data(export)?;
            let mut r = ue3_package::reader::Reader::at(data, h.native_offset);
            let _raw = r.i32()?;
            let len = r.count(1)?;
            dishonored_assets::edge_anim::EdgeClip::parse(r.bytes(len)?)?.bake()
        })()
        .ok()
        .map(Arc::new);
        self.clips.insert(key, c.clone());
        c
    }

    /// Every AnimSet of the loaded packages holding a sequence named `seq`.
    fn holders_anywhere(&mut self, pkgs: &[Arc<Package>], seq: &str) -> Vec<(String, String)> {
        if self.by_seq.is_none() {
            let mut m: HashMap<String, Vec<(String, String)>> = HashMap::new();
            for p in pkgs {
                for i in 0..p.exports.len() {
                    if p.export_class(i) != "AnimSet" {
                        continue;
                    }
                    let Ok(h) = p.read_object(i) else { continue };
                    let set = (p.name.clone(), p.obj_path(ObjRef::Export(i)));
                    for o in find(&h.properties, "Sequences").and_then(|x| x.array_objects().ok()).unwrap_or_default() {
                        let n = p.obj_name(ObjRef::from_index(o)).to_ascii_lowercase();
                        let v = m.entry(n).or_default();
                        if !v.contains(&set) {
                            v.push(set.clone());
                        }
                    }
                }
            }
            self.by_seq = Some(m);
        }
        self.by_seq.as_ref().and_then(|m| m.get(&seq.to_ascii_lowercase()).cloned()).unwrap_or_default()
    }
}

/// `base = nlerp(base, over, w)` for one bone (the presentation's `npc_anim::overlay`).
fn overlay(slot: &mut Option<(Quat, Option<Vec3>)>, bind: &dishonored_ir::BoneAsset, q: Quat, t: Option<Vec3>, w: f32) {
    let k = w.min(1.0);
    if k <= 1e-4 {
        return;
    }
    let (fq, ft) = slot.unwrap_or((bind.rotation, None));
    let to = if fq.dot(q) < 0.0 { -q } else { q };
    let nt = match (ft, t) {
        (f, None) => f,
        (a, Some(b)) => Some(a.unwrap_or(bind.translation).lerp(b, k)),
    };
    *slot = Some((fq.lerp(to, k).normalize(), nt));
}

/// Local bone transforms of a skeletal mesh actor posed by its group's anim tracks at `t`.
fn pose(sets: &mut Sets, skel: &Skeleton, g: &Group, t: f32) -> Vec<dishonored_ir::BoneAsset> {
    let mut slots: Vec<Option<(Quat, Option<Vec3>)>> = vec![None; skel.bones.len()];
    let infos: HashMap<String, Option<SeqInfo>> = HashMap::new();
    let infos = std::cell::RefCell::new(infos);
    let gsets = g.anim_sets.clone();
    let seq_info = |n: &str| -> Option<SeqInfo> {
        if let Some(i) = infos.borrow().get(&n.to_ascii_lowercase()) {
            return i.clone();
        }
        None
    };
    // Sequence infos first (the evaluation borrows them).
    for tr in g.tracks.iter().filter(|t| !t.disabled) {
        if let TrackKind::Anim { keys, .. } = &tr.kind {
            for k in keys {
                let key = k.name.to_ascii_lowercase();
                if infos.borrow().contains_key(&key) {
                    continue;
                }
                let info = sets.holder(&gsets, &k.name).and_then(|(_, d, e)| crate::interp::seq_info(&d.pkg, e));
                infos.borrow_mut().insert(key, info);
            }
        }
    }
    for tr in g.tracks.iter().filter(|t| !t.disabled) {
        let Some((s, w)) = tr.anim(t, &seq_info) else { continue };
        if w <= 1e-3 || s.name.is_empty() {
            continue;
        }
        let Some((set, d, e)) = sets.holder(&gsets, &s.name) else { continue };
        let Some(clip) = sets.clip(&set, &d, e) else { continue };
        // Edge joints: the set's tracks in the mesh's bone order.
        let mut tracks = d.tracks.clone();
        tracks.sort_by_key(|n| skel.bones.iter().position(|b| b.name.eq_ignore_ascii_case(n)).unwrap_or(usize::MAX));
        for (j, tn) in tracks.iter().enumerate() {
            let Some(b) = skel.bones.iter().position(|x| x.name.eq_ignore_ascii_case(tn)) else { continue };
            let (q, tr_) = clip.sample(s.position.min(clip.duration), j);
            if let Some(q) = q {
                overlay(&mut slots[b], &skel.bones[b], q, tr_, w);
            }
        }
    }
    skel.bones
        .iter()
        .zip(&slots)
        .map(|(b, s)| {
            let mut b = b.clone();
            if let Some((q, t)) = s {
                b.rotation = *q;
                if let Some(t) = t {
                    b.translation = *t;
                }
            }
            b
        })
        .collect()
}

fn group_has_anims(g: &Group) -> bool {
    g.tracks.iter().any(|t| !t.disabled && matches!(&t.kind, TrackKind::Anim { keys, .. } if !keys.is_empty()))
}

/// Bake every matinee's animated attachment bones and AI root tracks into the database.
/// `actor_src`: Kismet actor index -> (package index into `pkgs`, export).
pub fn bake(content: &Content, pkgs: &[Arc<Package>], db: &mut KismetDb, actor_src: &HashMap<u32, (usize, usize)>) {
    let mut sets = Sets { content, sets: HashMap::new(), clips: HashMap::new(), by_seq: None };
    // Matinee data op -> the SeqAct_Interp ops playing it.
    let mut players: HashMap<OpId, Vec<OpId>> = HashMap::new();
    for o in &db.ops {
        if o.class == "SeqAct_Interp" {
            if let Some(d) = o.var_link("Data").and_then(|l| l.vars.first()) {
                players.entry(*d).or_default().push(o.id);
            }
        }
    }
    let mut skeletons: HashMap<u32, Option<Arc<Skeleton>>> = HashMap::new();
    for di in 0..db.ops.len() {
        let Some(m) = db.ops[di].interp.as_ref().and_then(|i| i.matinee.as_deref()) else { continue };
        if !m.bone_tracks.is_empty() || !m.root_tracks.is_empty() {
            continue;
        }
        let mut bone_tracks: Vec<BoneTrack> = Vec::new();
        // Attachment bones of animated skeletal mesh actor groups.
        for (pi, g) in m.groups.iter().enumerate() {
            if !group_has_anims(g) {
                continue;
            }
            let mut bones: BTreeSet<String> = BTreeSet::new();
            for tr in g.tracks.iter().filter(|t| !t.disabled) {
                if let TrackKind::Attachment { keys, .. } = &tr.kind {
                    bones.extend(keys.iter().filter(|k| !k.detach && !k.bone.is_empty()).map(|k| k.bone.clone()));
                }
            }
            if bones.is_empty() {
                continue;
            }
            let mut actors: Vec<u32> = Vec::new();
            for op in players.get(&(di as OpId)).cloned().unwrap_or_default() {
                for v in db.ops[op as usize].var_link(&g.name).map(|l| l.vars.clone()).unwrap_or_default() {
                    if let Some(Value::Obj(Obj::Actor(a))) = db.ops[v as usize].props.get("ObjValue") {
                        if db.actors[*a as usize].class.starts_with("SkeletalMeshActor") && !actors.contains(a) {
                            actors.push(*a);
                        }
                    }
                }
            }
            for a in actors {
                let skel = skeletons
                    .entry(a)
                    .or_insert_with(|| {
                        let (p, e) = *actor_src.get(&a)?;
                        let props = pkgs[p].read_object(e).ok()?.properties;
                        actor_skeleton(content, &pkgs[p], &props).map(Arc::new)
                    })
                    .clone();
                let Some(skel) = skel else { continue };
                let n = (m.length.max(0.0) * BAKE_RATE).ceil() as usize + 1;
                let mut frames: Vec<Vec<([f32; 3], [f32; 4])>> = vec![Vec::with_capacity(n); bones.len()];
                for i in 0..n {
                    let t = i as f32 / BAKE_RATE;
                    let locals = pose(&mut sets, &skel, &m.groups[pi], t);
                    for (bi, b) in bones.iter().enumerate() {
                        let x = skel.point(&locals, b).unwrap_or(Mat4::IDENTITY);
                        let (_, q, p) = x.to_scale_rotation_translation();
                        frames[bi].push((p.to_array(), q.to_array()));
                    }
                }
                for (b, f) in bones.iter().zip(frames) {
                    bone_tracks.push(BoneTrack { group: g.name.clone(), actor: a, bone: b.clone(), rate: BAKE_RATE, frames: f });
                }
            }
        }
        // Root tracks of the AI groups' sequences: in the group's sets, else every set of the
        // loaded packages holding the name (the pawn's sets pick at run time).
        let mut root_tracks: Vec<RootTrack> = Vec::new();
        for g in m.groups.iter().filter(|g| matches!(g.kind, crate::interp::GroupKind::Ai { .. })) {
            let names: BTreeSet<String> = g.tracks.iter().filter(|t| !t.disabled).flat_map(|t| match &t.kind {
                TrackKind::Anim { keys, .. } => keys.iter().map(|k| k.name.clone()).collect::<Vec<_>>(),
                _ => Vec::new(),
            }).filter(|n| !n.is_empty()).collect();
            for name in names {
                let mut cands: Vec<(String, String)> = g.anim_sets.iter().rev().cloned().collect();
                cands.extend(sets.holders_anywhere(pkgs, &name));
                for set in cands {
                    if root_tracks.iter().any(|r| r.seq.eq_ignore_ascii_case(&name) && set_key(&r.set) == set_key(&set)) {
                        continue;
                    }
                    let Some(d) = sets.set(&set) else { continue };
                    let Some(e) = d.seqs.get(&name.to_ascii_lowercase()).copied() else { continue };
                    let (Some(info), Some(clip)) = (crate::interp::seq_info(&d.pkg, e), sets.clip(&set, &d, e)) else { continue };
                    // Edge joint 0 is the root bone (`root0_jnt`): the first track in every
                    // skeleton's bone order.
                    let n = (info.length.max(0.0) * BAKE_RATE).ceil() as usize + 1;
                    let mut last = Vec3::ZERO;
                    let frames = (0..n)
                        .map(|i| {
                            let t = (i as f32 / BAKE_RATE).min((clip.duration - 1e-4).max(0.0));
                            if let (_, Some(p)) = clip.sample(t, 0) {
                                last = p;
                            }
                            last.to_array()
                        })
                        .collect();
                    root_tracks.push(RootTrack { set, seq: name.clone(), info, rate: BAKE_RATE, frames });
                }
            }
        }
        if let Some(Some(m)) = db.ops[di].interp.as_mut().map(|i| i.matinee.as_deref_mut()) {
            m.bone_tracks = bone_tracks;
            m.root_tracks = root_tracks;
        }
    }
}

