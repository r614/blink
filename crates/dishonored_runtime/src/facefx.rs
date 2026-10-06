//! FaceFX lip sync and facial animation for speaking NPCs (research/dishonored/ANIMATION.md §12).
//!
//! A conversation line names its blurb GUID; the speaker's voice data cooks a `FaceFXAnimSet`
//! with one animation per blurb GUID (`DisDialogVoiceData.m_pFaceFXAnimSet`), authored from the
//! line's WAV. The speaker's head mesh names its `FaceFXAsset` (the actor: face graph and bone
//! poses). While the line plays, the animation's curves (time = time since the line started,
//! i.e. the VO's position) drive the face graph, whose bone pose nodes move the face bones and
//! the head.

use dishonored_assets::Content;
use dishonored_assets::facefx::{Actor, Anim};
use glam::{Quat, Vec3};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use ue3_package::ObjRef;

/// An actor bound to one model: per actor bone, (part, bone) pairs it moves.
pub struct Binding {
    pub actor: Arc<Actor>,
    pub bones: Vec<Vec<(usize, usize)>>,
}

#[derive(Default)]
pub struct FaceFxBank {
    /// Blurb GUID (uppercase) -> animation.
    anims: HashMap<String, Arc<Anim>>,
    /// Packages whose anim sets are indexed (lowercase).
    indexed: HashSet<String>,
    /// model -> its FaceFX actor (None: no part names one).
    models: HashMap<usize, Option<Arc<Binding>>>,
    /// Lines seen speaking: line id -> local time the line started.
    started: HashMap<u32, f64>,
}

/// One bone's FaceFX offset this frame: part, bone, translation delta (bone-local, uu) and
/// rotation delta (applied after the animated rotation).
pub type FaceBone = (usize, usize, Vec3, Quat);

impl FaceFxBank {
    /// Index the FaceFX anim sets of `packages` (once each).
    pub fn index(&mut self, content: &Content, packages: &[String]) {
        for p in packages {
            if !self.indexed.insert(p.to_ascii_lowercase()) {
                continue;
            }
            let Ok(pkg) = content.package(p) else { continue };
            for s in dishonored_assets::facefx::package_anim_sets(&pkg) {
                match s {
                    Ok(s) => {
                        for a in s.anims {
                            self.anims.entry(a.name.to_ascii_uppercase()).or_insert_with(|| Arc::new(a));
                        }
                    }
                    Err(e) => bevy::log::warn!("FaceFX anim set in {p}: {e:#}"),
                }
            }
        }
    }

    pub fn anim(&self, blurb: &str) -> Option<Arc<Anim>> {
        self.anims.get(&blurb.to_ascii_uppercase()).cloned()
    }

    /// The FaceFX actor of a model: the first part whose skeletal mesh names a `FaceFXAsset`
    /// (the head). `packages` are searched for the mesh export (the look's own package first).
    pub fn binding(&mut self, content: &Content, model: usize, parts: &[crate::characters::PartGpu], packages: &[String]) -> Option<Arc<Binding>> {
        if let Some(b) = self.models.get(&model) {
            return b.clone();
        }
        let mut actor = None;
        'parts: for part in parts {
            for p in packages {
                let Ok(pkg) = content.package(p) else { continue };
                let Some(e) = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SkeletalMesh" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&part.source)) else { continue };
                match dishonored_assets::facefx::mesh_actor(&pkg, e) {
                    Ok(Some(a)) => {
                        actor = Some(a);
                        break 'parts;
                    }
                    Ok(None) => continue 'parts,
                    Err(e) => {
                        bevy::log::warn!("FaceFX actor of {}: {e:#}", part.source);
                        continue 'parts;
                    }
                }
            }
        }
        let b = actor.map(|a| {
            let bones = a
                .bones
                .iter()
                .map(|b| {
                    parts
                        .iter()
                        .enumerate()
                        .filter_map(|(pi, p)| p.bones.iter().position(|x| x.name.eq_ignore_ascii_case(&b.name)).map(|bi| (pi, bi)))
                        .collect()
                })
                .collect();
            Arc::new(Binding { actor: Arc::new(a), bones })
        });
        self.models.insert(model, b.clone());
        b
    }

    /// Local time a line started, first seen `elapsed` seconds into it at local time `now`.
    pub fn line_start(&mut self, line: u32, now: f64, elapsed: f32) -> f64 {
        *self.started.entry(line).or_insert(now - elapsed.max(0.0) as f64)
    }

    /// Forget lines no longer speaking.
    pub fn retain_lines(&mut self, live: &HashSet<u32>) {
        self.started.retain(|k, _| live.contains(k));
    }
}

/// The bone offsets of `anim` at time `t` on a bound actor, for the additive FaceFX blend
/// (`SkeletalMeshComponent.FaceFXBlendMode` = `FXBM_Additive`, the engine default; Dishonored
/// does not override it). 0x00725EA0: position += FaceFX position - mesh reference position;
/// rotation = (FaceFX rotation x mesh reference rotation^-1) x animated rotation.
pub fn face_bones(b: &Binding, anim: &Anim, t: f32, parts: &[crate::characters::PartGpu]) -> Vec<FaceBone> {
    let tracks = anim.tracks(&b.actor, t);
    if tracks.iter().all(|v| *v == 0.0) {
        return Vec::new();
    }
    let values = b.actor.evaluate(&tracks);
    let mut out = Vec::new();
    for (bi, pos, rot) in b.actor.bone_poses(&values) {
        let q = Quat::from_xyzw(rot[0], rot[1], rot[2], rot[3]).normalize();
        for &(part, bone) in &b.bones[bi] {
            let Some(bind) = parts.get(part).and_then(|p| p.bones.get(bone)) else { continue };
            let dq = (q * bind.rotation.inverse()).normalize();
            out.push((part, bone, Vec3::from(pos) - bind.translation, dq));
        }
    }
    out
}
