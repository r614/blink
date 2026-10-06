//! [interp] Matinees played on the mission server (the gameplay side of cutscenes).
//!
//! When a `SeqAct_Interp` starts, its groups are bound like `InterpGroupInst::InitGroupInst`:
//! each group to the objects of the `SeqAct_Interp` variable link of the same name (a
//! `DisSeqVar_SpawnerPawn` gives the spawner's pawn), the player group to the human players.
//! Every step the scene is evaluated at the matinee position (`dishonored_kismet::interp`):
//! move tracks and attachments place level actors and pawns, locomotion keys walk AI pawns to
//! their marks, face-to keys turn them, visibility keys hide actors. Pawns of AI groups stop
//! their own behaviour while the matinee holds them (`LockRequests`), the players of a player
//! group lose movement control (`StatePlayerMasterSoiree`; looking around stays). Everything
//! is deterministic: positions come from the matinee data and the Kismet clock.

use super::{H, KismetRun};
use dishonored_ai::BrainState;
use dishonored_kismet::interp::{Bound, GroupKind, Matinee, Scene, SceneWorld, Xform};
use dishonored_kismet::runtime::Ctx;
use dishonored_kismet::{Kismet, Obj, OpId};
use dishonored_mission::runtime::{CutsceneBound, CutsceneView};
use dishonored_sim::state::PlayerId;
use glam::Vec3;

/// A matinee being played.
#[derive(Debug, Clone)]
pub struct Playing {
    pub op: OpId,
    pub scene: Scene,
    pub prev: f32,
    /// AI pawns held and their `stay` flag before.
    pub held: Vec<(PlayerId, bool)>,
    /// Players under matinee control.
    pub players: Vec<PlayerId>,
    /// Last pose applied per bound object (for the view).
    pub poses: Vec<(Bound, Xform)>,
    /// Conversation instance whose soiree started this matinee (it continues at the end).
    pub conv: Option<u32>,
    /// Blurbs started by dialog keys: (GUID, Kismet time they end).
    pub speaking: Vec<(String, f64)>,
}

fn matinee<'a>(k: &Kismet, db: &'a dishonored_kismet::KismetDb, op: OpId) -> Option<&'a Matinee> {
    let d = k.link_vars(op, "Data").into_iter().next()?;
    db.ops[d as usize].interp.as_ref().and_then(|i| i.matinee.as_deref())
}

struct World<'a, 'b> {
    h: &'a H<'b>,
}

impl SceneWorld for World<'_, '_> {
    fn xform(&self, b: Bound) -> Option<Xform> {
        match b {
            Bound::Actor(a) => {
                let actor = self.h.db.actors.get(a as usize)?;
                let mut x = Xform::new(actor.location, actor.rotation);
                if let Some(p) = self.h.st.actor_pos.get(&a) {
                    x.pos = *p;
                }
                if let Some(r) = self.h.st.actor_rot.get(&a) {
                    x.rot = glam::Mat3::from_quat(*r);
                }
                Some(x)
            }
            Bound::Pawn(p) => {
                let pl = self.h.m.world.player(p)?;
                Some(Xform { pos: pl.position, rot: dishonored_kismet::interp::rot_matrix_deg(0.0, pl.yaw, 0.0) })
            }
        }
    }
    fn bone(&self, actor: u32, bone: &str) -> Option<Xform> {
        let a = self.h.db.actors.get(actor as usize)?;
        let (_, p, q) = a.bones.iter().find(|b| b.0.eq_ignore_ascii_case(bone))?;
        Some(Xform { pos: Vec3::from(*p), rot: glam::Mat3::from_quat(glam::Quat::from_array(*q)) })
    }
}

/// `SeqAct_Interp` started: bind the groups and hold their pawns.
pub(super) fn started(h: &mut H, cx: &mut Ctx) {
    let op = cx.op;
    let kdb = cx.k.db.clone();
    let Some(m) = matinee(cx.k, &kdb, op) else { return };
    h.st.cutscenes.retain(|c| c.op != op);
    let humans = h.players();
    let mut bound: Vec<Vec<Bound>> = Vec::new();
    for g in &m.groups {
        let mut v = Vec::new();
        if matches!(g.kind, GroupKind::Player { .. }) {
            v.extend(humans.iter().map(|p| Bound::Pawn(*p)));
        } else if cx.k.db.op(op).var_link(&g.name).is_some() {
            for o in cx.objects(&g.name) {
                let b = match &o {
                    Obj::Pawn(p) => Some(Bound::Pawn(*p)),
                    Obj::Actor(a) => Some(h.pawn_of(&o).map(Bound::Pawn).unwrap_or(Bound::Actor(*a))),
                    _ => None,
                };
                if let Some(b) = b {
                    if !v.contains(&b) {
                        v.push(b);
                    }
                }
            }
        }
        bound.push(v);
    }
    let scene = Scene::new(m, bound, &World { h });
    let mut held = Vec::new();
    for p in scene.controlled_pawns(m) {
        if humans.contains(&p) {
            continue;
        }
        if let Some(b) = h.m.crowd.brains.iter_mut().find(|b| b.id == p) {
            held.push((p, b.stay));
            b.stay = true;
            b.goal = None;
            b.state = BrainState::Idle;
            b.follow = Default::default();
        }
    }
    let players: Vec<PlayerId> = if m.has_player_group() { humans } else { Vec::new() };
    let groups: Vec<String> = m.groups.iter().zip(&scene.bound).filter(|(_, b)| !b.is_empty()).map(|(g, b)| format!("{}={}", g.name, b.len())).collect();
    h.note(format!("matinee {} bound: {}", h.db.describe(op), groups.join(", ")));
    let conv = h.st.soiree_conv.remove(&op);
    h.st.cutscenes.push(Playing { op, scene, prev: 0.0, held, players, poses: Vec::new(), conv, speaking: Vec::new() });
}

/// A conversation's soiree asked for a matinee (`play_matinee_guid`): it continues when the
/// matinee finishes, or at once when there is none.
pub(super) fn soiree(h: &mut H, instance: u32, op: Option<OpId>) {
    match op {
        Some(op) => {
            h.st.soiree_conv.insert(op, instance);
        }
        None => {
            if let Some(r) = h.m.state.conv.as_mut() {
                r.matinee_done(instance);
            }
        }
    }
}

/// `SeqAct_Interp` finished: release its pawns (they stay where the matinee left them).
pub(super) fn finished(h: &mut H, op: OpId) {
    let Some(i) = h.st.cutscenes.iter().position(|c| c.op == op) else { return };
    let c = h.st.cutscenes.remove(i);
    if let (Some(inst), Some(r)) = (c.conv, h.m.state.conv.as_mut()) {
        r.matinee_done(inst);
    }
    for (p, stay) in c.held {
        let pos = h.m.world.player(p).map(|x| x.position);
        if let Some(b) = h.m.crowd.brains.iter_mut().find(|b| b.id == p) {
            b.stay = stay;
            if let Some(pos) = pos {
                b.home = pos;
            }
        }
    }
}

/// Apply the playing matinees to the world (after the Kismet tick).
pub(super) fn apply(k: &mut Kismet, h: &mut H) {
    let mut list = std::mem::take(&mut h.st.cutscenes);
    let now = k.time;
    for c in &mut list {
        let Some(pos) = k.playing_matinees().iter().find(|x| x.0 == c.op).map(|x| x.1) else { continue };
        let kdb = k.db.clone();
        let Some(m) = matinee(k, &kdb, c.op) else { continue };
        // Dialog keys: the last key crossed this step starts its blurb (main GUID; the
        // original picks at random among the main and alternative blurbs when there are some).
        for tr in m.groups.iter().flat_map(|g| g.tracks.iter()).filter(|t| !t.disabled) {
            if let dishonored_kismet::interp::TrackKind::Dialog { keys } = &tr.kind {
                if let Some(key) = keys.iter().rev().find(|key| key.start <= pos && key.start > c.prev) {
                    let dur = h.m.state.conv.as_mut().and_then(|r| r.speak_blurb(c.conv, &key.blurb, pos - key.start));
                    match dur {
                        Some(d) => {
                            c.speaking.push((key.blurb.clone(), now + (d - (pos - key.start)).max(0.0) as f64));
                            h.note(format!("matinee {} dialog key {:.2} s: blurb {} ({d:.2} s)", h.db.describe(c.op), key.start, key.blurb));
                        }
                        None => h.note(format!("matinee {} dialog key {:.2} s: blurb {} not found", h.db.describe(c.op), key.start, key.blurb)),
                    }
                }
            }
        }
        c.speaking.retain(|s| s.1 > now);
        if let Some(ps) = k.interp_play.get_mut(&c.op) {
            ps.speaking = c.speaking.iter().map(|s| s.0.clone()).collect();
        }
        let out = c.scene.update(m, c.prev, pos, &World { h });
        c.prev = pos;
        for (b, x) in &out.poses {
            match *b {
                Bound::Actor(a) => {
                    h.st.actor_pos.insert(a, x.pos);
                    h.st.actor_rot.insert(a, x.quat());
                }
                Bound::Pawn(p) => {
                    if let Some(pl) = h.m.world.player_mut(p) {
                        pl.position = x.pos;
                        pl.velocity = Vec3::ZERO;
                        // The players keep their own view direction.
                        if !c.players.contains(&p) {
                            pl.yaw = x.yaw_deg();
                        }
                    }
                    if let Some(br) = h.m.crowd.brains.iter_mut().find(|br| br.id == p) {
                        br.home = x.pos;
                        br.goal = None;
                    }
                }
            }
        }
        c.poses = out.poses.clone();
        for (p, target, speed, _stop) in out.gotos {
            let goal = World { h }.xform(target).map(|x| x.pos);
            if let (Some(g), Some(b)) = (goal, h.m.crowd.brains.iter_mut().find(|b| b.id == p)) {
                b.goal = Some(g);
                b.run = speed.to_ascii_lowercase().contains("run");
                b.spot = None;
                b.state = BrainState::WalkToPoint;
                b.follow = Default::default();
            }
        }
        for (p, target) in out.faces {
            let (Some(me), Some(t)) = (h.m.world.player(p).map(|x| x.position), World { h }.xform(target).map(|x| x.pos)) else { continue };
            let d = t - me;
            if d.truncate().length() > 1.0 {
                if let Some(pl) = h.m.world.player_mut(p) {
                    pl.yaw = d.y.atan2(d.x).to_degrees();
                }
            }
        }
        for (b, shown) in out.visibility {
            if let Bound::Actor(a) = b {
                if shown {
                    h.st.hidden.remove(&a);
                } else {
                    h.st.hidden.insert(a);
                }
            }
        }
    }
    h.st.cutscenes = list;
}

/// Players under matinee control this step.
pub fn locked(run: &KismetRun, p: PlayerId) -> bool {
    run.st.cutscenes.iter().any(|c| c.players.contains(&p))
}

/// Replicated state for the clients' presentation.
pub fn view(run: &KismetRun) -> Vec<CutsceneView> {
    let k = &run.k;
    let mut out = Vec::new();
    for c in &run.st.cutscenes {
        let kdb = k.db.clone();
        let Some(m) = matinee(k, &kdb, c.op) else { continue };
        let Some((_, pos, len)) = k.playing_matinees().into_iter().find(|x| x.0 == c.op) else { continue };
        let mut groups = Vec::new();
        for (gi, g) in m.groups.iter().enumerate() {
            let objs = c.scene.bound.get(gi).cloned().unwrap_or_default();
            if objs.is_empty() {
                continue;
            }
            let v = objs
                .iter()
                .enumerate()
                .map(|(oi, b)| {
                    let init = c.scene.initial.get(gi).and_then(|v| v.get(oi).copied().flatten()).unwrap_or_default();
                    let cur = c.poses.iter().find(|p| p.0 == *b).map(|p| (p.1.pos.to_array(), p.1.quat().to_array()));
                    match b {
                        Bound::Pawn(p) => CutsceneBound { pawn: Some(*p), actor: None, initial: (init.pos.to_array(), init.quat().to_array()), current: cur },
                        Bound::Actor(a) => {
                            let ac = &k.db.actors[*a as usize];
                            CutsceneBound { pawn: None, actor: Some((ac.package.clone(), ac.name.clone(), ac.class.clone())), initial: (init.pos.to_array(), init.quat().to_array()), current: cur }
                        }
                    }
                })
                .collect();
            groups.push((g.name.clone(), v));
        }
        out.push(CutsceneView { package: m.package.clone(), export: m.export, position: pos, length: len, groups, players: c.players.clone() });
    }
    out
}
