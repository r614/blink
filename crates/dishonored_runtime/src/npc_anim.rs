//! Third-person character animation selected the way the original does it (presentation only;
//! research/dishonored/ANIMATION.md):
//! - each pawn tweak binds its own AnimSets; a sequence is found from the last set to the first
//!   (later sets override, e.g. the aristocrat walk);
//! - locomotion follows the tweak's `ArkComponentLocomotionConfig`: a modifier (stance) picks
//!   one speed mode per speed (Idle/Walk/Run), each with four directional loops (S, W, N, E;
//!   S and W are authored reversed and play backwards). Directions blend by angle, speed modes
//!   by ground speed against their authored speeds, and the play rate follows the ground speed;
//! - actions (attacks, parries, hit reactions, deaths, panic, ambient idles) play the chains of
//!   the tweak's `DisAnimDefinitions` class through a full-body slot, gestures through an
//!   upper-body slot filtered from `spine_0_jnt` (`NPC_Human_at`).
//!
//! Nothing here feeds back into the simulation.

use crate::characters::{CharacterLibrary, PartGpu};
use bevy::prelude::*;
use dishonored_assets::Content;
use dishonored_assets::edge_anim::BakedClip;
use dishonored_ir::npc_anim::*;
use std::collections::HashMap;
use std::sync::Arc;
use ue3_package::ObjRef;

/// One AnimSet of the install: its track names and sequence exports.
pub struct SetData {
    pub pkg: String,
    pub tracks: Vec<String>,
    /// lowercase sequence name -> (export, original name)
    pub seqs: HashMap<String, (usize, String)>,
}

pub struct Profile {
    /// Indices into `AnimBank::sets`, in tweak order.
    pub sets: Vec<usize>,
    pub loco: Option<LocoConfig>,
    pub defs: Option<AnimDefs>,
    /// lowercase sequence -> set holding it (last set wins), cached.
    lookup: HashMap<String, Option<usize>>,
    /// Talk gestures available (upper body), see `talk_gestures`.
    pub gestures: Vec<String>,
}

#[derive(Resource)]
pub struct AnimBank {
    content: Arc<Content>,
    pub sets: Vec<SetData>,
    set_index: HashMap<String, usize>,
    clips: HashMap<(usize, String), Option<Arc<BakedClip>>>,
    pub profiles: Vec<Profile>,
    profile_index: HashMap<AnimProfile, usize>,
    /// model -> profile (built-in archetypes and published looks).
    pub model_profile: HashMap<usize, usize>,
    /// (model, set) -> per part: Edge joint -> bone.
    bindings: HashMap<(usize, usize), Arc<Vec<Vec<Option<usize>>>>>,
    /// model -> per part: upper-body weight per bone (descendants of `spine_0_jnt`).
    masks: HashMap<usize, Arc<Vec<Vec<f32>>>>,
    /// lowercase object path -> package (AnimSets, locomotion configs) for built-in profiles.
    locator: HashMap<String, String>,
    /// [interp] (profile, extra AnimSets) -> profile with the matinee group's sets appended.
    derived: HashMap<(usize, Vec<(String, String)>), usize>,
}

/// Packages that cook the Boyle NPC AnimSets, locomotion configs and animation definitions.
const SCRIPT_PACKAGES: &[&str] = &["L_Boyle_Ext_Script", "L_Boyle_Int_Script", "L_BoyleStreet_Ext_Script"];

const GUARD_SETS: &[&str] = &[
    "Npc_EliteGuard_Generic_as.Npc_EliteGuard_Generic_as",
    "Npc_EliteGuard_Tracking_as.Npc_EliteGuard_HeadTracking_as",
    "Npc_EliteGuard_Tracking_as.Npc_EliteGuard_TorsoTracking_as",
    "Npc_EliteGuard_empty_as.Npc_EliteGuard_empty_as",
    "Npc_EliteGuard_Sword_as.Npc_EliteGuard_Sword_as",
    "Npc_EliteGuard_Face_as.Npc_EliteGuard_Face_as",
    "Npc_EliteGuard_Gun_as.Npc_EliteGuard_Gun_as",
    "Npc_EliteGuard_Corpses_as.Npc_EliteGuard_Corpses_as",
    "Npc_EliteGuard_Distractions_as.Npc_EliteGuard_Distractions_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_Gestures_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesLeftArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesNoArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesRightArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_Postures_as",
    "Npc_EliteGuard_Blink_as.Npc_EliteGuard_Blink_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesHead_as",
];
/// `Pwn_AristoParty_*` (Twk_Pawn_Boyle_Party): the guard sets without sword and gun, then the
/// tall-civilian and aristocrat empty sets.
const ARISTO_SETS: &[&str] = &[
    "Npc_EliteGuard_Generic_as.Npc_EliteGuard_Generic_as",
    "Npc_EliteGuard_Tracking_as.Npc_EliteGuard_HeadTracking_as",
    "Npc_EliteGuard_Tracking_as.Npc_EliteGuard_TorsoTracking_as",
    "Npc_EliteGuard_empty_as.Npc_EliteGuard_empty_as",
    "Npc_EliteGuard_Face_as.Npc_EliteGuard_Face_as",
    "Npc_CivTall_Empty_as.Npc_CivTall_Empty_as",
    "Npc_EliteGuard_Corpses_as.Npc_EliteGuard_Corpses_as",
    "Npc_EliteGuard_Distractions_as.Npc_EliteGuard_Distractions_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_Gestures_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesLeftArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesNoArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesRightArm_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_Postures_as",
    "Npc_EliteGuard_Blink_as.Npc_EliteGuard_Blink_as",
    "Npc_EliteGuard_Gestures_as.Npc_EliteGuard_GesturesHead_as",
    "Npc_Aristo_Empty_as.Npc_Aristo_Empty_as",
];
/// `Twk_Pawn_Boyle_AristoF*`: the civilian female sets.
const FEMALE_SETS: &[&str] = &[
    "Npc_CivFemale_Generic_as.Npc_CivFemale_Generic_as",
    "Npc_CivFemale_Tracking_as.Npc_CivFemale_HeadTracking_as",
    "Npc_CivFemale_Tracking_as.Npc_CivFemale_TorsoTracking_as",
    "Npc_CivFemale_Empty_as.Npc_CivFemale_Empty_as",
    "Npc_CivFemale_Corpses_as.Npc_CivFemale_Corpses_as",
    "Npc_CivFemale_Distractions_as.Npc_CivFemale_Distraction_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_Gestures_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_GesturesLeftArm_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_GesturesNoArm_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_GesturesRightArm_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_Postures_as",
    "Npc_Female_Blink_as.Npc_Female_Blink_as",
    "Npc_CivFemale_Gestures_as.Npc_CivFemale_GesturesHead_as",
    "Npc_CivFemale_Face_as.Npc_CivFemale_Face_as",
];
const GUARD_LOCO: &str = "DisLocomotionConfig.EliteGuardLocomotionConfig";
const FEMALE_LOCO: &str = "DisLocomotionConfig.FemaleLocomotionConfig";

impl AnimBank {
    pub fn new(content: Arc<Content>) -> Self {
        let mut locator = HashMap::new();
        for p in SCRIPT_PACKAGES {
            let Ok(pkg) = content.package(p) else { continue };
            for i in 0..pkg.exports.len() {
                if matches!(pkg.export_class(i).as_str(), "AnimSet" | "ArkComponentLocomotionConfig") {
                    locator.entry(pkg.obj_path(ObjRef::Export(i)).to_ascii_lowercase()).or_insert_with(|| p.to_string());
                }
            }
        }
        AnimBank { content, sets: Vec::new(), set_index: HashMap::new(), clips: HashMap::new(), profiles: Vec::new(), profile_index: HashMap::new(), model_profile: HashMap::new(), bindings: HashMap::new(), masks: HashMap::new(), locator, derived: HashMap::new() }
    }

    fn located(&self, path: &str) -> Option<(String, String)> {
        self.locator.get(&path.to_ascii_lowercase()).map(|p| (p.clone(), path.to_string()))
    }

    /// The profiles of the built-in archetypes (0 masked aristocrat, 1 elite guard, 2 masked
    /// aristocrat B, 3 masked female aristocrat), from the Boyle pawn tweaks they stand for.
    pub fn builtin(&self, archetype: usize) -> AnimProfile {
        let (sets, loco, defs) = match archetype {
            1 => (GUARD_SETS, GUARD_LOCO, "DisNPCAnimDefinitions_EliteGuard"),
            3 => (FEMALE_SETS, FEMALE_LOCO, "DisNPCAnimDefinitions_Civilian_NoPuke"),
            _ => (ARISTO_SETS, GUARD_LOCO, "DisNPCAnimDefinitions_Civilian_NoPuke"),
        };
        AnimProfile { sets: sets.iter().filter_map(|s| self.located(s)).collect(), locomotion: self.located(loco), defs: defs.to_string(), ragdoll_on_death: false }
    }

    fn set(&mut self, r: &(String, String)) -> Option<usize> {
        let key = format!("{}:{}", r.0, r.1).to_ascii_lowercase();
        if let Some(i) = self.set_index.get(&key) {
            return Some(*i);
        }
        use ue3_package::properties::{Value, find};
        let pkg = self.content.package(&r.0).ok()?;
        let e = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&r.1))?;
        let h = pkg.read_object(e).ok()?;
        let (n, raw) = find(&h.properties, "TrackBoneNames").and_then(|p| p.array_raw())?;
        let mut rd = ue3_package::reader::Reader::new(raw);
        let tracks: Vec<String> = (0..n).map(|_| ue3_package::FName::read(&mut rd).map(|f| pkg.name(f)).unwrap_or_default()).collect();
        let mut seqs = HashMap::new();
        for s in find(&h.properties, "Sequences").and_then(|p| p.array_objects().ok()).unwrap_or_default() {
            let ObjRef::Export(x) = ObjRef::from_index(s) else { continue };
            let Ok(sh) = pkg.read_object(x) else { continue };
            if let Some(Value::Name(name)) = find(&sh.properties, "SequenceName").map(|p| &p.value) {
                seqs.insert(name.to_ascii_lowercase(), (x, name.clone()));
            }
        }
        self.sets.push(SetData { pkg: r.0.clone(), tracks, seqs });
        self.set_index.insert(key, self.sets.len() - 1);
        Some(self.sets.len() - 1)
    }

    /// Load (once) a profile's sets, locomotion config and animation definitions.
    pub fn profile(&mut self, p: &AnimProfile) -> usize {
        if let Some(i) = self.profile_index.get(p) {
            return *i;
        }
        let sets: Vec<usize> = p.sets.iter().filter_map(|s| self.set(s)).collect();
        let loco = p.locomotion.as_ref().and_then(|l| dishonored_assets::npc_anim::load_loco_config(&self.content, l).map_err(|e| warn!("locomotion {}: {e:#}", l.1)).ok());
        let mut pkgs: Vec<String> = p.sets.iter().map(|s| s.0.clone()).collect();
        pkgs.extend(SCRIPT_PACKAGES.iter().map(|s| s.to_string()));
        pkgs.dedup();
        let defs = if p.defs.is_empty() { None } else { dishonored_assets::npc_anim::load_anim_defs(&self.content, &pkgs, &p.defs).map_err(|e| warn!("animation definitions {}: {e:#}", p.defs)).ok() };
        // Talk gestures: the full-arm `Gesture_*` loops of the profile's main gesture set.
        let mut gestures: Vec<String> = Vec::new();
        for &s in &sets {
            let set = &self.sets[s];
            let mut g: Vec<String> = set.seqs.values().map(|v| v.1.clone()).filter(|n| TALK_GESTURES.iter().any(|t| n.eq_ignore_ascii_case(t))).collect();
            g.sort();
            for x in g {
                if !gestures.contains(&x) {
                    gestures.push(x);
                }
            }
        }
        self.profiles.push(Profile { sets, loco, defs, lookup: HashMap::new(), gestures });
        self.profile_index.insert(p.clone(), self.profiles.len() - 1);
        self.profiles.len() - 1
    }

    /// Set holding `name` for a profile (last set wins).
    pub fn holder(&mut self, profile: usize, name: &str) -> Option<usize> {
        let key = name.to_ascii_lowercase();
        let pr = &self.profiles[profile];
        if let Some(h) = pr.lookup.get(&key) {
            return *h;
        }
        let h = pr.sets.iter().rev().copied().find(|s| self.sets[*s].seqs.contains_key(&key));
        self.profiles[profile].lookup.insert(key, h);
        h
    }

    pub fn has(&mut self, profile: usize, name: &str) -> bool {
        self.holder(profile, name).is_some()
    }

    /// Decode (once) a sequence for a profile.
    pub fn clip(&mut self, profile: usize, name: &str) -> Option<(usize, Arc<BakedClip>)> {
        let set = self.holder(profile, name)?;
        let key = (set, name.to_ascii_lowercase());
        if let Some(c) = self.clips.get(&key) {
            return c.clone().map(|c| (set, c));
        }
        let (export, _) = self.sets[set].seqs.get(&key.1).cloned()?;
        let baked = (|| -> anyhow::Result<BakedClip> {
            let pkg = self.content.package(&self.sets[set].pkg)?;
            let h = pkg.read_object(export)?;
            let data = pkg.export_data(export)?;
            let mut r = ue3_package::reader::Reader::at(data, h.native_offset);
            let _raw = r.i32()?;
            let len = r.count(1)?;
            dishonored_assets::edge_anim::EdgeClip::parse(r.bytes(len)?)?.bake()
        })();
        let c = match baked {
            Ok(b) => Some(Arc::new(b)),
            Err(e) => {
                warn!("animation {name}: {e:#}");
                None
            }
        };
        self.clips.insert(key, c.clone());
        c.map(|c| (set, c))
    }

    /// Edge joint -> bone for every part of a model, for one set. Edge joints follow the set's
    /// tracks in the bone order of the skeleton it was compressed against: the profile's
    /// locomotion reference mesh when it is loaded, else the model's own body.
    fn binding(&mut self, lib: &CharacterLibrary, model: usize, profile: usize, set: usize) -> Arc<Vec<Vec<Option<usize>>>> {
        if let Some(b) = self.bindings.get(&(model, set)) {
            return b.clone();
        }
        let m = &lib.models[model];
        let ref_mesh = self.profiles[profile].loco.as_ref().map(|l| l.ref_mesh.clone()).unwrap_or_default();
        let reference = lib.models.iter().find_map(|x| x.parts.first().filter(|p| !ref_mesh.is_empty() && p.source.eq_ignore_ascii_case(&ref_mesh))).or(m.parts.first());
        let mut tracks = self.sets[set].tracks.clone();
        if let Some(r) = reference {
            let idx = |n: &String| r.bones.iter().position(|b| b.name.eq_ignore_ascii_case(n)).unwrap_or(usize::MAX);
            tracks.sort_by_key(idx);
        }
        let b: Vec<Vec<Option<usize>>> = m.parts.iter().map(|p| tracks.iter().map(|t| p.bones.iter().position(|b| b.name.eq_ignore_ascii_case(t))).collect()).collect();
        let b = Arc::new(b);
        self.bindings.insert((model, set), b.clone());
        b
    }

    fn mask(&mut self, lib: &CharacterLibrary, model: usize) -> Arc<Vec<Vec<f32>>> {
        if let Some(m) = self.masks.get(&model) {
            return m.clone();
        }
        let m: Vec<Vec<f32>> = lib.models[model]
            .parts
            .iter()
            .map(|p| {
                (0..p.bones.len())
                    .map(|mut i| {
                        // Inside the branch from `spine_0_jnt` (`UpperBodyFilter`, NPC_Human_at).
                        for _ in 0..64 {
                            if p.bones[i].name.eq_ignore_ascii_case("spine_0_jnt") {
                                return 1.0;
                            }
                            let par = p.bones[i].parent;
                            if par < 0 || par as usize >= p.bones.len() || par as usize == i {
                                return 0.0;
                            }
                            i = par as usize;
                        }
                        0.0
                    })
                    .collect()
            })
            .collect();
        let m = Arc::new(m);
        self.masks.insert(model, m.clone());
        m
    }

    /// Sample a sequence on a model: per part, per bone, the local rotation (None: not
    /// animated). Translations are kept for the root and its children only (the pelvis drop of
    /// deaths, kneels and falls); deeper bones keep their bind translations so each body keeps
    /// its proportions.
    pub fn sample(&mut self, lib: &CharacterLibrary, model: usize, profile: usize, name: &str, t: f32, reversed: bool) -> Option<Pose> {
        let (set, clip) = self.clip(profile, name)?;
        let bind = self.binding(lib, model, profile, set);
        let time = if reversed { (clip.duration - t).max(0.0) } else { t };
        let parts = &lib.models[model].parts;
        Some(
            parts
                .iter()
                .enumerate()
                .map(|(pi, p)| {
                    let mut v: Vec<Option<Key>> = vec![None; p.bones.len()];
                    for (j, bone) in bind[pi].iter().enumerate() {
                        if let Some(b) = bone {
                            let (q, tr) = clip.sample(time, j);
                            if let Some(q) = q {
                                let par = p.bones[*b].parent;
                                let shallow = *b == 0 || par <= 0 || p.bones.get(par as usize).is_some_and(|x| x.parent < 0);
                                v[*b] = Some(Key { q, t: if shallow { tr } else { None } });
                            }
                        }
                    }
                    v
                })
                .collect(),
        )
    }

    pub fn duration(&mut self, profile: usize, name: &str) -> Option<f32> {
        self.clip(profile, name).map(|c| c.1.duration)
    }
}

/// Upper-body `Gesture_*` sequences used while talking (full-arm gestures of the
/// `*_Gestures_as` sets; APPROXIMATION: the original's conversation body gestures come from
/// the dialogue's soiree matinees / FaceFX, not decoded, U-142).
const TALK_GESTURES: &[&str] = &[
    "Gesture_Calm", "Gesture_DualSwip", "Gesture_HandsFront", "Gesture_HandsFront1", "Gesture_HandsFront2", "Gesture_LeftSwip", "Gesture_RightSwip", "Gesture_OpenHand_Think", "Gesture_Point", "Gesture_Precis", "Gesture_Shrug", "Gesture_Show_Front",
    "Gesture_Show_Left", "Gesture_Show_Right", "Gesture_Sweep", "Gesture_Think", "Gesture_Move", "Gesture_Refuse", "Gesture_ChinScratch",
];

/// One animated bone: local rotation and, for root-level bones, translation.
#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub q: Quat,
    pub t: Option<Vec3>,
}

/// Per part, per bone (None = not set by this layer).
pub type Pose = Vec<Vec<Option<Key>>>;

/// Weighted accumulation (hemisphere-aligned nlerp for rotations, linear for translations).
struct Acc(Vec<Vec<(Vec4, f32, Vec3, f32)>>);

impl Acc {
    fn new(parts: &[PartGpu]) -> Self {
        Acc(parts.iter().map(|p| vec![(Vec4::ZERO, 0.0, Vec3::ZERO, 0.0); p.bones.len()]).collect())
    }
    fn add(&mut self, pose: &Pose, w: f32) {
        if w <= 1e-4 {
            return;
        }
        for (pa, pp) in self.0.iter_mut().zip(pose) {
            for (a, k) in pa.iter_mut().zip(pp) {
                if let Some(k) = k {
                    let mut v = Vec4::from(k.q);
                    if a.1 > 0.0 && a.0.dot(v) < 0.0 {
                        v = -v;
                    }
                    a.0 += v * w;
                    a.1 += w;
                    if let Some(t) = k.t {
                        a.2 += t * w;
                        a.3 += w;
                    }
                }
            }
        }
    }
    fn finish(self) -> Pose {
        self.0
            .into_iter()
            .map(|p| p.into_iter().map(|(v, w, t, tw)| if w > 0.0 && v.length_squared() > 1e-12 { Some(Key { q: Quat::from_vec4(v.normalize()), t: (tw > 0.0).then(|| t / tw) }) } else { None }).collect())
            .collect()
    }
}

/// `base = nlerp(base, over, w * mask)` where `over` animates a bone.
fn overlay(base: &mut Pose, over: &Pose, w: f32, mask: Option<&Vec<Vec<f32>>>, bind: &[PartGpu]) {
    if w <= 1e-4 {
        return;
    }
    for (pi, (bp, op)) in base.iter_mut().zip(over).enumerate() {
        for (bi, (b, o)) in bp.iter_mut().zip(op).enumerate() {
            let Some(o) = o else { continue };
            let k = (w * mask.map_or(1.0, |m| m[pi][bi])).min(1.0);
            if k <= 1e-4 {
                continue;
            }
            let bone = &bind[pi].bones[bi];
            let from = b.unwrap_or(Key { q: bone.rotation, t: None });
            let mut to = o.q;
            if from.q.dot(to) < 0.0 {
                to = -to;
            }
            let t = match (from.t, o.t) {
                (_, None) => from.t,
                (a, Some(bt)) => Some(a.unwrap_or(bone.translation).lerp(bt, k)),
            };
            *b = Some(Key { q: from.q.lerp(to, k).normalize(), t });
        }
    }
}

/// What keeps a looping chain step playing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sustain {
    None,
    Block,
    Stagger,
    Cower,
    Crouch,
    Talk,
}

/// A chain of sequences played in a slot (one group of a `DisAnimationChain`).
#[derive(Clone, Debug)]
pub struct Chain {
    pub steps: Vec<(String, bool)>,
    pub i: usize,
    pub t: f32,
    pub sustain: Sustain,
    pub weight: f32,
    pub blend_in: f32,
    pub blend_out: f32,
    /// Hold the last frame forever (deaths: the corpse keeps the final pose).
    pub hold: bool,
    pub fading: bool,
    pub key: &'static str,
}

impl Chain {
    fn one(name: String, key: &'static str) -> Self {
        Chain { steps: vec![(name, false)], i: 0, t: 0.0, sustain: Sustain::None, weight: 0.0, blend_in: 0.2, blend_out: 0.1, hold: false, fading: false, key }
    }
}

#[derive(Component, Default)]
pub struct PawnAnim {
    pub phase: f32,
    pub modifier: String,
    pub prev_modifier: Option<(String, f32)>,
    pub full: Option<Chain>,
    pub upper: Option<Chain>,
    pub stance: Option<NpcStance>,
    pub was_alive: bool,
    pub init: bool,
    pub ambient: f32,
    pub gesture: f32,
    pub rng: u32,
    pub sword: Option<dishonored_sim::state::SwordPhase>,
    pub left: Option<dishonored_sim::state::LeftPhase>,
    /// [interp] Matinee anim track samples on this pawn (set each frame by `cutscene.rs`).
    pub scripted: Vec<Scripted>,
}

impl PawnAnim {
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Item prefix of `eDisItemPrefix` steps: the equipped primary's ("Sword_") or "Empty_".
fn prefixed(name: &str, prefix: u8, armed: bool) -> String {
    if prefix == 0 { name.to_string() } else if armed { format!("Sword_{name}") } else { format!("Empty_{name}") }
}

/// Direction of a hit relative to the victim's facing: 0 front, 1 left, 2 right, 3 back.
fn quadrant(victim: &dishonored_sim::state::PlayerState, from: Vec3) -> usize {
    let d = (from - victim.position).truncate();
    if d.length_squared() < 1e-3 {
        return 0;
    }
    let f = dishonored_sim::movement::forward_vector(victim.yaw).truncate();
    let r = dishonored_sim::movement::right_vector(victim.yaw).truncate();
    let (x, y) = (d.dot(f), d.dot(r));
    if x.abs() >= y.abs() {
        if x >= 0.0 { 0 } else { 3 }
    } else if y < 0.0 {
        1
    } else {
        2
    }
}

/// Step of a directional group (`m_bEndChainAfterOneAnimation` groups list one sequence per
/// direction: 4 entries Front, Left, Right, Back; 8+ entries Front, FrontLeft, FrontRight,
/// Left, Right, Back, BackLeft, BackRight, then the "DownUp" variants). Inferred from the
/// sequence names (U-143).
fn directional_step(n: usize, quad: usize) -> usize {
    if n >= 8 {
        [0, 3, 4, 5][quad]
    } else if n >= 4 {
        quad
    } else {
        0
    }
}

impl AnimBank {
    /// A chain from the profile's animation definitions. `step`: Some(i) plays one step of an
    /// end-after-one group; None plays the group in order.
    fn chain(&mut self, profile: usize, anim: NpcAnim, group: Option<usize>, quad: usize, armed: bool, rng: f32, key: &'static str) -> Option<Chain> {
        let defs = self.profiles[profile].defs.as_ref()?;
        let ch = defs.chain(anim)?.clone();
        let g = group.unwrap_or(((rng * ch.groups.len() as f32) as usize).min(ch.groups.len() - 1));
        let grp = ch.groups.get(g)?.clone();
        let mut steps: Vec<(String, bool)> = Vec::new();
        if grp.end_after_one {
            let k = directional_step(grp.steps.len(), quad);
            let s = grp.steps.get(k).or(grp.steps.first())?;
            let name = s.names.get(((rng * 7919.0) as usize) % s.names.len().max(1)).cloned()?;
            steps.push((prefixed(&name, s.item_prefix, armed), s.looped));
        } else {
            for s in &grp.steps {
                if s.names.is_empty() {
                    continue;
                }
                let name = &s.names[((rng * 7919.0) as usize) % s.names.len()];
                steps.push((prefixed(name, s.item_prefix, armed), s.looped));
            }
        }
        steps.retain(|(n, _)| n != "None" && self.has(profile, n));
        if steps.is_empty() {
            return None;
        }
        let blend_in = grp.steps.first().map(|s| s.blend_in).unwrap_or(0.2);
        Some(Chain { steps, i: 0, t: 0.0, sustain: Sustain::None, weight: 0.0, blend_in, blend_out: grp.blend_out, hold: false, fading: false, key })
    }

    /// `AmbientIdle` (`DisAISubProcessAmbientAnims`, 0x00B39620): the group for the held item,
    /// then a random one of its leading steps that exist in the pawn's sets.
    fn ambient(&mut self, profile: usize, patrol: bool, rng: f32) -> Option<Chain> {
        let defs = self.profiles[profile].defs.as_ref()?;
        let ch = defs.chain(NpcAnim::AmbientIdle)?.clone();
        let grp = ch.groups.get(if patrol { 1 } else { 0 }).or(ch.groups.first())?;
        let avail: Vec<String> = grp.steps.iter().filter_map(|s| s.names.first().cloned()).take_while(|n| self.has(profile, n)).collect();
        if avail.is_empty() {
            return None;
        }
        let n = avail[((rng * avail.len() as f32) as usize).min(avail.len() - 1)].clone();
        let mut c = Chain::one(n, "ambient");
        c.blend_in = grp.steps.first().map(|s| s.blend_in).unwrap_or(0.2);
        c.blend_out = grp.blend_out;
        Some(c)
    }
}

/// Inputs for one pawn's animation this frame.
pub struct AnimInput<'a> {
    pub p: &'a dishonored_sim::state::PlayerState,
    pub stance: Option<NpcStance>,
    pub armed: bool,
    pub talking: bool,
    pub dt: f32,
    /// Hit / death events this frame: (kind, source position).
    pub hits: &'a [(HitKind, Vec3)],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HitKind {
    Death,
    DeathFast,
    Melee,
    Projectile,
    Headshot,
    Windblast,
    Swing,
    Parried,
    Assassinated,
}

/// Locomotion modifier for a pawn (`ArkComponentLocomotionConfig.m_Modifiers`).
fn modifier_for(i: &AnimInput, cfg: &LocoConfig) -> String {
    use dishonored_sim::state::PawnKind;
    let gun_aim = i.p.left.phase == dishonored_sim::state::LeftPhase::Aiming && matches!(i.p.left.item, dishonored_sim::input::LeftHand::Pistol | dishonored_sim::input::LeftHand::Crossbow);
    let want: &[&str] = match (i.p.kind, i.stance, i.armed) {
        (PawnKind::Human, _, true) if gun_aim => &["Aim", "Sword"],
        (PawnKind::Human, _, true) => &["Sword"],
        (PawnKind::Human, _, false) => &["Empty"],
        (_, Some(NpcStance::Search), true) => &["SearchSword", "Sword"],
        (_, Some(NpcStance::Search), false) => &["SearchEmpty", "Empty"],
        (_, Some(NpcStance::Panic), true) => &["PanicSword", "Sword"],
        (_, Some(NpcStance::Panic), false) => &["PanicEmpty", "Empty"],
        (_, Some(NpcStance::Combat | NpcStance::Notice | NpcStance::Cower), true) => &["Sword"],
        (PawnKind::Guard, _, _) => &["SwordPatrol", "Sword"],
        (_, _, true) => &["Sword"],
        _ => &["Empty"],
    };
    want.iter().find(|w| cfg.modifier(w).is_some()).map(|s| s.to_string()).or_else(|| cfg.modifiers.first().map(|m| m.name.clone())).unwrap_or_default()
}

/// Directional weights for the four move anims (S, W, N, E) from the movement angle relative
/// to facing (0 = forward, +90 = right): anim `k` covers 180 + 90k degrees.
fn dir_weights(angle_deg: f32) -> [f32; 4] {
    let a = (angle_deg - 180.0).rem_euclid(360.0) / 90.0;
    let k0 = a.floor() as usize % 4;
    let f = a - a.floor();
    let mut w = [0.0; 4];
    w[k0] += 1.0 - f;
    w[(k0 + 1) % 4] += f;
    w
}

/// Locomotion pose: speed modes of the modifier blended by speed, directions by angle.
fn loco_pose(bank: &mut AnimBank, lib: &CharacterLibrary, model: usize, profile: usize, modifier: &str, i: &AnimInput, phase: &mut f32, advance: bool) -> Option<Pose> {
    let cfg = bank.profiles[profile].loco.clone()?;
    let mi = cfg.modifier(modifier)?;
    let v = i.p.velocity.truncate();
    let mut speed = v.length();
    let moving = i.p.alive && i.p.mode == dishonored_sim::state::MoveMode::Walking;
    if !moving {
        speed = 0.0;
    }
    let fwd = dishonored_sim::movement::forward_vector(i.p.yaw).truncate();
    let right = dishonored_sim::movement::right_vector(i.p.yaw).truncate();
    let angle = v.dot(right).atan2(v.dot(fwd)).to_degrees();
    let dw = dir_weights(angle);
    // Each speed mode's speed in this direction (idle modes: 0).
    let mut modes: Vec<(&SpeedMode, f32)> = Vec::new();
    for s in 0..cfg.speed_names.len() {
        if let Some(m) = cfg.mode(mi, s) {
            let sp = if m.move_anims.len() >= 4 { (0..4).map(|k| dw[k] * m.move_anims[k].1).sum() } else { m.move_anims.first().map(|a| a.1).unwrap_or(0.0) };
            if modes.last().is_none_or(|l: &(&SpeedMode, f32)| l.0.name != m.name) {
                modes.push((m, sp));
            }
        }
    }
    if modes.is_empty() {
        return None;
    }
    // Bracketing pair by speed (`FUN_009105b0`): linear weight between their speeds.
    let (lo, hi, w_hi) = {
        let mut lo = 0;
        for k in 0..modes.len() {
            if modes[k].1 <= speed {
                lo = k;
            }
        }
        let hi = (lo + 1).min(modes.len() - 1);
        let w = if hi != lo && modes[hi].1 > modes[lo].1 { ((speed - modes[lo].1) / (modes[hi].1 - modes[lo].1)).clamp(0.0, 1.0) } else { 0.0 };
        // Snap within 2 % (0.98 threshold in the original).
        let w = if w > 0.98 { 1.0 } else if w < 0.02 { 0.0 } else { w };
        (lo, hi, w)
    };
    let blended_speed = modes[lo].1 * (1.0 - w_hi) + modes[hi].1 * w_hi;
    let mut acc = Acc::new(&lib.models[model].parts);
    let mut master: Option<(f32, f32)> = None; // (weight, duration)
    let mut samples: Vec<(String, bool, f32)> = Vec::new();
    for (k, mw) in [(lo, 1.0 - w_hi), (hi, w_hi)] {
        if mw <= 1e-3 || (k == hi && hi == lo && mw < 1.0) {
            continue;
        }
        let m = modes[k].0;
        if m.move_anims.len() >= 4 && modes[k].1 > 0.0 {
            for d in 0..4 {
                let w = mw * dw[d];
                if w > 1e-3 {
                    // S and W are authored reversed: played with rate -1 (0x00910970).
                    samples.push((m.move_anims[d].0.clone(), d < 2, w));
                }
            }
        } else if let Some(a) = m.move_anims.first() {
            samples.push((a.0.clone(), false, mw));
        }
    }
    for (name, _, w) in &samples {
        if let Some(d) = bank.duration(profile, name) {
            if master.is_none_or(|m| *w > m.0) {
                master = Some((*w, d));
            }
        }
    }
    if advance {
        let rate = if blended_speed > 1.0 && speed > 1.0 { speed / blended_speed } else { 1.0 };
        if let Some((_, d)) = master {
            *phase = (*phase + i.dt * rate / d.max(0.05)).rem_euclid(1.0);
        }
    }
    for (name, rev, w) in &samples {
        let Some(d) = bank.duration(profile, name) else { continue };
        if let Some(p) = bank.sample(lib, model, profile, name, *phase * d, *rev) {
            acc.add(&p, *w);
        }
    }
    Some(acc.finish())
}

/// Advance a chain; returns false when it is finished.
fn step_chain(bank: &mut AnimBank, profile: usize, c: &mut Chain, sustain: &[Sustain], dt: f32) -> bool {
    if c.fading {
        c.weight -= dt / c.blend_out.max(0.05);
        return c.weight > 0.0;
    }
    c.weight = (c.weight + dt / c.blend_in.max(0.05)).min(1.0);
    c.t += dt;
    let (name, looped) = c.steps[c.i].clone();
    let dur = bank.duration(profile, &name).unwrap_or(0.0);
    let sustained = c.sustain != Sustain::None && sustain.contains(&c.sustain);
    if looped {
        if c.t >= dur && dur > 0.0 {
            c.t = c.t.rem_euclid(dur);
        }
        if !sustained {
            c.i += 1;
            c.t = 0.0;
        }
    } else if c.t >= dur {
        if c.i + 1 >= c.steps.len() {
            if c.hold {
                c.t = (dur - 1e-3).max(0.0);
                return true;
            }
            c.fading = true;
            c.t = (dur - 1e-3).max(0.0);
            return true;
        }
        c.i += 1;
        c.t = 0.0;
    }
    if c.i >= c.steps.len() {
        c.i = c.steps.len() - 1;
        c.fading = true;
    }
    true
}

/// Evaluate a pawn: returns per-part local rotations (None = bind).
pub fn animate(bank: &mut AnimBank, lib: &CharacterLibrary, model: usize, profile: usize, st: &mut PawnAnim, i: &AnimInput) -> Pose {
    use dishonored_sim::state::{LeftPhase, PawnKind, SwordPhase};
    if !st.init {
        st.init = true;
        st.rng = i.p.id.wrapping_mul(2654435761) | 1;
        st.was_alive = i.p.alive;
        st.ambient = 2.0 + st.rand() * 4.0;
        st.phase = st.rand();
        if !i.p.alive {
            // Already dead when first seen: the end of a death.
            if let Some(mut c) = bank.chain(profile, NpcAnim::Death, None, 0, i.armed, 0.0, "dead") {
                c.hold = true;
                c.weight = 1.0;
                c.i = c.steps.len() - 1;
                c.t = bank.duration(profile, &c.steps[c.i].0).unwrap_or(0.0);
                st.full = Some(c);
            }
        }
    }
    let r = st.rand();
    let p = i.p;
    // Event-driven actions.
    for (kind, from) in i.hits {
        let q = quadrant(p, *from);
        let next = match kind {
            HitKind::Death | HitKind::DeathFast => {
                // An assassination victim keeps its assassination sequence as its death.
                if let Some(c) = st.full.as_mut().filter(|c| c.key == "assassinated") {
                    c.hold = true;
                    continue;
                }
                let a = if *kind == HitKind::DeathFast { NpcAnim::DeathFast } else { NpcAnim::Death };
                bank.chain(profile, a, None, q, i.armed, r, "dead").or_else(|| bank.chain(profile, NpcAnim::Death, None, q, i.armed, r, "dead")).map(|mut c| {
                    c.hold = true;
                    c
                })
            }
            HitKind::Melee if p.alive => bank.chain(profile, NpcAnim::HitReactStrong, None, q, i.armed, r, "hit"),
            HitKind::Windblast if p.alive => bank.chain(profile, NpcAnim::HitReactWindblast, None, q, i.armed, r, "hit"),
            HitKind::Projectile | HitKind::Headshot if p.alive => {
                // Steps: (none), head, stomach, left leg, right leg, front left, front right.
                let k = if *kind == HitKind::Headshot { 1 } else { 2 + ((r * 5.0) as usize).min(4) };
                bank.profiles[profile].defs.as_ref().and_then(|d| d.chain(NpcAnim::HitReactProjectile)).and_then(|c| c.groups.first()).and_then(|g| g.steps.get(k)).and_then(|s| s.names.first().cloned()).map(|n| prefixed(&n, 2, i.armed)).filter(|n| bank.has(profile, n)).map(|n| Chain::one(n, "hit"))
            }
            HitKind::Swing if p.alive => {
                let g = if p.kind == PawnKind::Human && p.sword.phase == SwordPhase::Assassinating { Some(5) } else { None };
                bank.chain(profile, NpcAnim::AttackShort, g, 0, true, r, "swing")
            }
            HitKind::Parried if p.alive => bank.chain(profile, NpcAnim::Parry, Some(0), 0, true, r, "parry"),
            HitKind::Assassinated => bank.chain(profile, NpcAnim::Assassination, None, q, i.armed, r, "assassinated"),
            _ => None,
        };
        if let Some(c) = next {
            if st.full.as_ref().is_some_and(|f| f.hold && f.key == "dead") {
                continue;
            }
            let w = st.full.as_ref().map_or(0.0, |f| f.weight);
            st.full = Some(Chain { weight: w * 0.5, ..c });
        }
    }
    // State-driven actions (sustained loops).
    let busy = st.full.as_ref().is_some_and(|c| !c.fading);
    if p.alive {
        if !st.was_alive {
            st.full = None;
        }
        // `Parry_AwaitingImpact`: the ready-parry loop held while blocking.
        if p.sword.phase == SwordPhase::Blocking && !busy {
            if let Some(mut c) = bank.chain(profile, NpcAnim::ParryAwaitingImpact, None, 0, true, 0.0, "block") {
                c.sustain = Sustain::Block;
                st.full = Some(c);
            }
        }
        if p.sword.phase == SwordPhase::Staggered && st.sword != Some(SwordPhase::Staggered) {
            if let Some(mut c) = bank.chain(profile, NpcAnim::Stun, Some(0), 0, true, r, "stun") {
                c.sustain = Sustain::Stagger;
                st.full = Some(c);
            }
        }
        if p.left.phase != st.left.unwrap_or(LeftPhase::Idle) && p.kind == PawnKind::Human {
            let gun = matches!(p.left.item, dishonored_sim::input::LeftHand::Pistol | dishonored_sim::input::LeftHand::Crossbow);
            let a = match p.left.phase {
                LeftPhase::Recovering if gun => Some(NpcAnim::AimShoot),
                LeftPhase::Reloading => Some(NpcAnim::AimReload),
                LeftPhase::Equipping if gun && i.armed => Some(NpcAnim::AimIn),
                _ => None,
            };
            if let Some(c) = a.and_then(|a| bank.chain(profile, a, None, 0, true, r, "gun")) {
                st.full = Some(c);
            }
        }
        let stance = i.stance;
        if stance != st.stance {
            let prev = st.stance;
            let a = match stance {
                Some(NpcStance::Panic) if prev != Some(NpcStance::Cower) => bank.chain(profile, NpcAnim::Panic, None, 0, i.armed, r, "panic"),
                Some(NpcStance::Notice) => bank.chain(profile, NpcAnim::ReactToNoticing, None, 0, i.armed, r, "notice"),
                Some(NpcStance::Cower) => bank.chain(profile, NpcAnim::Beg, Some(0), 0, i.armed, r, "cower").map(|mut c| {
                    c.sustain = Sustain::Cower;
                    c
                }),
                _ => None,
            };
            if let Some(c) = a {
                st.full = Some(c);
            }
        }
        // Armed crouch: the cover loop (no crouched locomotion in the NPC sets).
        if p.kind == PawnKind::Human && p.crouched && i.armed && p.velocity.truncate().length() < 20.0 && st.full.is_none() && bank.has(profile, "Sword_Cover_Loop") {
            let mut c = Chain::one("Sword_Cover_Loop".into(), "cover");
            c.steps[0].1 = true;
            c.sustain = Sustain::Crouch;
            st.full = Some(c);
        }
    }
    st.was_alive = p.alive;
    st.stance = i.stance;
    st.sword = Some(p.sword.phase);
    st.left = Some(p.left.phase);
    let mut sustain = Vec::new();
    if p.sword.phase == SwordPhase::Blocking {
        sustain.push(Sustain::Block);
    }
    if p.sword.phase == SwordPhase::Staggered {
        sustain.push(Sustain::Stagger);
    }
    if i.stance == Some(NpcStance::Cower) {
        sustain.push(Sustain::Cower);
    }
    if p.crouched && p.velocity.truncate().length() < 20.0 {
        sustain.push(Sustain::Crouch);
    }
    if i.talking {
        sustain.push(Sustain::Talk);
    }
    // Locomotion modifier and transition sequences (`m_Transitions`).
    let cfg = bank.profiles[profile].loco.clone();
    if let Some(cfg) = cfg.as_ref() {
        let m = modifier_for(i, cfg);
        if m != st.modifier {
            if !st.modifier.is_empty() && p.alive && st.full.is_none() {
                let (a, b) = (cfg.modifier(&st.modifier).map(|x| x as i32), cfg.modifier(&m).map(|x| x as i32));
                if let Some(t) = cfg.transitions.iter().find(|t| Some(t.1) == a && Some(t.2) == b) {
                    if bank.has(profile, &t.0) {
                        st.full = Some(Chain::one(t.0.clone(), "transition"));
                    }
                }
            }
            if !st.modifier.is_empty() {
                st.prev_modifier = Some((std::mem::take(&mut st.modifier), 1.0));
            }
            st.modifier = m;
        }
    }
    // Ambient idle variations while standing.
    let idle = p.alive && p.velocity.truncate().length() < 20.0 && p.kind != PawnKind::Human && matches!(i.stance, None | Some(NpcStance::Relaxed) | Some(NpcStance::Patrol)) && !i.talking;
    if idle && st.full.is_none() {
        st.ambient -= i.dt;
        if st.ambient <= 0.0 {
            let patrol = st.modifier.contains("Patrol") || st.modifier.starts_with("Sword");
            let rr = st.rand();
            if let Some(c) = bank.ambient(profile, patrol, rr) {
                st.full = Some(c);
            }
            // `m_fMinAmbientAnimFrequency` / `m_fMaxAmbientAnimFrequency` class defaults (2, 6).
            st.ambient = 2.0 + st.rand() * 4.0;
        }
    }
    // Talking: upper-body gestures (APPROXIMATION, U-142).
    if i.talking && p.alive && st.scripted.is_empty() /* [interp] the matinee drives the body */ {
        st.gesture -= i.dt;
        if st.upper.as_ref().is_none_or(|c| c.fading) && st.gesture <= 0.0 {
            let g = &bank.profiles[profile].gestures;
            if !g.is_empty() {
                let n = g[((st.rand() * g.len() as f32) as usize).min(g.len() - 1)].clone();
                let mut c = Chain::one(n, "talk");
                c.blend_in = 0.3;
                c.blend_out = 0.3;
                st.upper = Some(c);
            }
            st.gesture = 0.5 + st.rand() * 1.5;
        }
    }
    // Advance chains.
    if let Some(mut c) = st.full.take() {
        if step_chain(bank, profile, &mut c, &sustain, i.dt) {
            st.full = Some(c);
        }
    }
    if let Some(mut c) = st.upper.take() {
        if step_chain(bank, profile, &mut c, &sustain, i.dt) {
            st.upper = Some(c);
        }
    }
    // Compose: locomotion (with modifier crossfade), full-body slot, upper-body slot.
    let parts = &lib.models[model].parts;
    let mut phase = st.phase;
    let mut pose = loco_pose(bank, lib, model, profile, &st.modifier.clone(), i, &mut phase, true).unwrap_or_else(|| parts.iter().map(|p| vec![None; p.bones.len()]).collect());
    if let Some((pm, w)) = st.prev_modifier.take() {
        let mut ph = phase;
        if let Some(prev) = loco_pose(bank, lib, model, profile, &pm, i, &mut ph, false) {
            let mut base = prev;
            overlay(&mut base, &pose, 1.0 - w, None, parts);
            pose = base;
        }
        // `DisAnimNodeBlendByLocoType.m_fBlendTime` 0.2 s.
        let w = w - i.dt / 0.2;
        if w > 0.0 {
            st.prev_modifier = Some((pm, w));
        }
    }
    st.phase = phase;
    if let Some(c) = st.full.as_ref() {
        let (name, _) = &c.steps[c.i];
        if let Some(fp) = bank.sample(lib, model, profile, name, c.t, false) {
            overlay(&mut pose, &fp, c.weight, None, parts);
        }
    }
    if let Some(c) = st.upper.as_ref() {
        let (name, _) = &c.steps[c.i];
        if let Some(up) = bank.sample(lib, model, profile, name, c.t, false) {
            let mask = bank.mask(lib, model);
            overlay(&mut pose, &up, c.weight, Some(&mask), parts);
        }
    }
    // [interp] Matinee anim tracks (the `MatineeStackNode` slot channels), in track order.
    for s in st.scripted.clone() {
        let prof = bank.with_sets(profile, &s.sets);
        if let Some(sp) = bank.sample(lib, model, prof, &s.name, s.time, false) {
            overlay(&mut pose, &sp, s.weight, None, parts);
        }
    }
    pose
}

/// [interp] Blend a layer over a pose (`overlay`, for matinee props).
pub fn blend_into(base: &mut Pose, over: &Pose, w: f32, parts: &[PartGpu]) {
    overlay(base, over, w, None, parts);
}

/// [interp] One matinee anim track's sample on a pawn or prop: sequence, position (seconds),
/// channel weight, and the group's AnimSets (`GroupAnimSets`, searched before the pawn's).
#[derive(Clone, Debug, Default)]
pub struct Scripted {
    pub name: String,
    pub time: f32,
    pub weight: f32,
    pub sets: Vec<(String, String)>,
}

impl AnimBank {
    /// [interp] A profile with extra AnimSets appended (later sets win the name lookup).
    pub fn with_sets(&mut self, profile: usize, extra: &[(String, String)]) -> usize {
        if extra.is_empty() {
            return profile;
        }
        let key = (profile, extra.to_vec());
        if let Some(p) = self.derived.get(&key) {
            return *p;
        }
        let mut sets = self.profiles[profile].sets.clone();
        for s in extra {
            if let Some(i) = self.set(s) {
                sets.push(i);
            }
        }
        let loco = self.profiles[profile].loco.clone();
        self.profiles.push(Profile { sets, loco, defs: None, lookup: HashMap::new(), gestures: Vec::new() });
        let id = self.profiles.len() - 1;
        self.derived.insert(key, id);
        id
    }

    /// [interp] A profile made of AnimSets only (matinee props such as Samuel's boat).
    pub fn sets_profile(&mut self, sets: &[(String, String)]) -> usize {
        self.profile(&AnimProfile { sets: sets.to_vec(), locomotion: None, defs: String::new(), ragdoll_on_death: false })
    }

    /// [interp] Sample a sequence on parts outside the character library (matinee props): the
    /// set's tracks bound to the parts' bones by name, in the first part's bone order.
    pub fn sample_parts(&mut self, parts: &[PartGpu], profile: usize, name: &str, t: f32) -> Option<Pose> {
        let (set, clip) = self.clip(profile, name)?;
        let mut tracks = self.sets[set].tracks.clone();
        if let Some(r) = parts.first() {
            tracks.sort_by_key(|n| r.bones.iter().position(|b| b.name.eq_ignore_ascii_case(n)).unwrap_or(usize::MAX));
        }
        Some(
            parts
                .iter()
                .map(|p| {
                    let mut v: Vec<Option<Key>> = vec![None; p.bones.len()];
                    for (j, tn) in tracks.iter().enumerate() {
                        if let Some(b) = p.bones.iter().position(|b| b.name.eq_ignore_ascii_case(tn)) {
                            let (q, tr) = clip.sample(t.min(clip.duration), j);
                            if let Some(q) = q {
                                v[b] = Some(Key { q, t: tr });
                            }
                        }
                    }
                    v
                })
                .collect(),
        )
    }

    /// [interp] Length and soiree markers of a sequence (stretched matinee keys).
    pub fn seq_info(&mut self, profile: usize, name: &str) -> Option<dishonored_kismet::interp::SeqInfo> {
        let set = self.holder(profile, name)?;
        let (export, _) = self.sets[set].seqs.get(&name.to_ascii_lowercase()).cloned()?;
        let pkg = self.content.package(&self.sets[set].pkg).ok()?;
        dishonored_kismet::interp::seq_info(&pkg, export)
    }
}

/// Pawn-space bone globals from a pose (rotation-only retargeting: bind translations keep each
/// body's proportions).
pub fn globals(part: &PartGpu, pose: &[Option<Key>]) -> Vec<glam::Mat4> {
    let mut bones = part.bones.clone();
    for (b, k) in bones.iter_mut().zip(pose) {
        if let Some(k) = k {
            b.rotation = k.q;
            if let Some(t) = k.t {
                b.translation = t;
            }
        }
    }
    dishonored_assets::characters::bind_globals(&bones).iter().map(|g| part.mesh_to_pawn * *g).collect()
}
