//! Third-person character animation selected the way the original does it (presentation only;
//! research/dishonored/ANIMATION.md):
//! - each pawn tweak binds its own AnimSets; a sequence is found from the last set to the first
//!   (later sets override, e.g. the aristocrat walk);
//! - locomotion follows the tweak's `ArkComponentLocomotionConfig`: a modifier (stance) picks
//!   one speed mode per speed (Idle/Walk/Run), each with four directional loops (S, W, N, E;
//!   S and W are authored reversed and play backwards). Directions blend by angle, speed modes
//!   by ground speed against their authored speeds, and the play rate follows the ground speed;
//! - actions (attacks, parries, hit reactions, deaths, panic, ambient idles) play the chains of
//!   the tweak's `DisAnimDefinitions` class through a full-body slot, upper-body actions through
//!   an upper-body slot filtered from `spine_0_jnt` (`NPC_Human_at`);
//! - matinee anim tracks (soiree gestures, distractions) play over the result.
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
    /// lowercase sequence name -> all notifies of the sequence, sorted by time.
    pub notifies: HashMap<String, Vec<Notify>>,
}

/// A `DishonoredNotify_SeverLimb` (0x00A966D0, the notify's values over its class defaults:
/// blood, no hiding, ragdoll, impulse 300): the pawn's limb at `bone` is cut off (0x00B6CC10,
/// `crate::severed_limbs`).
#[derive(Clone, Debug, PartialEq)]
pub struct SeverNotify {
    pub t: f32,
    pub bone: String,
    /// `m_bUseBlood`: the limb's particle effects play.
    pub blood: bool,
    /// `m_bHideLimb`: the limb only disappears (no severed piece).
    pub hide: bool,
    /// `m_bGoToRagdoll`.
    pub ragdoll: bool,
    /// `m_ImpulseMode`: 0 none, 1 / 2 / 3 the severed bone's X / Y / Z axis.
    pub mode: u8,
    /// `m_fImpulseStrength`.
    pub strength: f32,
}

/// One notify in an animation sequence: time, duration, and type-specific data.
#[derive(Clone, Debug, PartialEq)]
pub struct Notify {
    /// Time within the sequence (seconds).
    pub t: f32,
    /// Duration of the notify window (for trails and other windowed events).
    pub duration: f32,
    /// The notify's kind and type-specific data.
    pub kind: NotifyKind,
}

/// The type and data of a notify.
#[derive(Clone, Debug, PartialEq)]
pub enum NotifyKind {
    /// Physics hand-over point (`DishonoredNotify_Ragdoll`).
    Ragdoll,
    /// Particle effect (`AnimNotify_PlayParticleEffect`): system path, socket or bone, attached.
    PlayParticleEffect { template: String, point: String, attach: bool },
    /// Item drop (`DisNotify_DropItem`).
    DropItem(DropNotify),
    /// Melee trail window (`DisNotify_Trails`).
    Trails,
    /// Limb severance (`DishonoredNotify_SeverLimb`).
    SeverLimb(SeverNotify),
    /// Any other notify class (coverage of unknown types).
    Other { class: String },
}

/// A `DisNotify_DropItem` (0x00A8D400): a dead pawn (health below 1, pawn slot 348 0x00B49480)
/// lets go of its items.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropNotify {
    pub t: f32,
    /// `m_WhichItem` (`DisWhichEquippedItemToDrop`): 0 every equipped item, 1 every attached
    /// item, 2 the item of `m_ByUsage`.
    pub which: u8,
    /// `m_ByUsage` (`EDisEquipUsage`) is not the secondary usage (the off hand).
    pub primary: bool,
    /// `m_DropVelocity`, in the pawn mesh's frame (turned by its LocalToWorld).
    pub velocity: glam::Vec3,
}

pub struct Profile {
    /// Indices into `AnimBank::sets`, in tweak order.
    pub sets: Vec<usize>,
    pub loco: Option<LocoConfig>,
    pub defs: Option<AnimDefs>,
    /// lowercase sequence -> set holding it (last set wins), cached.
    lookup: HashMap<String, Option<usize>>,
    /// The pawn tweak's `PhysicsAsset` (ragdoll), when it decodes.
    pub physics: Option<Arc<PhysicsAssetData>>,
    /// The pawn tweak's `ArkComponentLookatConfig` (look-at grids, blinking).
    pub lookat: Option<Arc<LookatConfig>>,
    /// The pawn tweak's `DisSeveredLimbInfo`.
    pub limbs: Option<Arc<SeveredLimbInfo>>,
    /// Item prefixes of item-prefixed steps: [primary item held, empty hands]
    /// (`AnimProfile::prefix`).
    pub prefix: [String; 2],
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
    /// `LocoBlender` tunables (shared by every pawn using `NPC_Human_at`).
    pub loco_node: LocoNodeParams,
    /// FaceFX actors and per-blurb animations (lip sync).
    pub facefx: crate::facefx::FaceFxBank,
}

/// Packages that cook the Boyle NPC AnimSets, locomotion configs and animation definitions.
const SCRIPT_PACKAGES: &[&str] = &["L_Boyle_Ext_Script", "L_Boyle_Int_Script", "L_BoyleStreet_Ext_Script"];

use dishonored_assets::characters::{ARISTO_SETS, FEMALE_LOCO, FEMALE_SETS, GUARD_LOCO, GUARD_SETS};

/// Notifies a clip window crossed: the original's half-open window `t0 <= t < t1`
/// (`UAnimNodeSequence` 0x00598440 -> 0x00599A50, research/dishonored/systems/tick_order.md
/// section 4); a clip started this frame (`t0` None) fires from its start, a notify at 0 too.
pub fn crossing(notifies: &[Notify], t0: Option<f32>, t1: f32) -> Vec<Notify> {
    let t0 = t0.unwrap_or(0.0);
    notifies.iter().filter(|n| t0 <= n.t && n.t < t1).cloned().collect()
}

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
        // `LocoBlender` tunables: class defaults overridden by the tree's node.
        let f = dishonored_assets::npc_anim::loco_node_floats(&content, &["m_fVelocityMaxBlendSpeed", "m_fVelocityMaxDif", "m_fVelocityMinBlendSpeedRatio", "m_fModifierBlendDuration"]);
        let d = LocoNodeParams::default();
        let loco_node = LocoNodeParams { rate: f[0].unwrap_or(d.rate), range: f[1].unwrap_or(d.range), min_ratio: f[2].unwrap_or(d.min_ratio), modifier_blend: f[3].unwrap_or(d.modifier_blend) };
        AnimBank { content, sets: Vec::new(), set_index: HashMap::new(), clips: HashMap::new(), profiles: Vec::new(), profile_index: HashMap::new(), model_profile: HashMap::new(), bindings: HashMap::new(), masks: HashMap::new(), locator, loco_node, derived: HashMap::new(), facefx: Default::default() }
    }

    pub fn content(&self) -> Arc<Content> {
        self.content.clone()
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
        // Their tweaks' physics assets (`m_pPhysicsAsset`) and look-at config (`DefaultLookatConfig`).
        let physics = if archetype == 3 { ("L_BoyleStreet_Ext_Script", "Npc_Servants.Physics.Skm_servantA_Physics") } else { ("L_Boyle_Ext_Script", "Npc_EliteGuard.Physics.Skm_EliteGuard_Physics") };
        AnimProfile {
            sets: sets.iter().filter_map(|s| self.located(s)).collect(),
            locomotion: self.located(loco),
            defs: defs.to_string(),
            ragdoll_on_death: false,
            physics: Some((physics.0.to_string(), physics.1.to_string())),
            lookat: Some(("DishonoredGame".to_string(), "DisLookatConfig.DefaultLookatConfig".to_string())),
            // The Boyle tweaks' sword (`Twk_Inv_SwordEliteGuard`) and empty hands.
            item_prefix: if archetype == 1 { "Sword_".to_string() } else { String::new() },
            empty_prefix: "Empty_".to_string(),
            // The limb info of the Boyle party tweaks (`Twk_Pawn_Boyle_Party`), the only one the
            // Boyle packages cook.
            severed_limbs: Some(("L_Boyle_Ext_Script".to_string(), "NPC_Common.SeveredLimbsInfo.SLInfo_Elite".to_string())),
        }
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
        let mut notifies: HashMap<String, Vec<Notify>> = HashMap::new();
        for s in find(&h.properties, "Sequences").and_then(|p| p.array_objects().ok()).unwrap_or_default() {
            let ObjRef::Export(x) = ObjRef::from_index(s) else { continue };
            let Ok(sh) = pkg.read_object(x) else { continue };
            if let Some(Value::Name(name)) = find(&sh.properties, "SequenceName").map(|p| &p.value) {
                seqs.insert(name.to_ascii_lowercase(), (x, name.clone()));
                let mut seq_notifies: Vec<Notify> = Vec::new();
                for ev in find(&sh.properties, "Notifies").and_then(|p| p.array_structs(&pkg).ok()).unwrap_or_default() {
                    let Some(ObjRef::Export(ni)) = find(&ev, "Notify").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
                    let t = find(&ev, "Time").and_then(|p| p.as_float()).unwrap_or(0.0);
                    let duration = find(&ev, "Duration").and_then(|p| p.as_float()).unwrap_or(0.0);
                    let class = pkg.export_class(ni);
                    let kind = match class.as_str() {
                        // `DishonoredNotify_Ragdoll`: the death hands the body to physics here.
                        "DishonoredNotify_Ragdoll" => NotifyKind::Ragdoll,
                        // The system plays on the socket (else the bone) of the mesh, attached or
                        // where it was.
                        "AnimNotify_PlayParticleEffect" => {
                            let Ok(nh) = pkg.read_object(ni) else { continue };
                            let nm = |k: &str| match find(&nh.properties, k).map(|p| &p.value) {
                                Some(Value::Name(v)) if v != "None" => v.clone(),
                                _ => String::new(),
                            };
                            let Some(r @ (ObjRef::Export(_) | ObjRef::Import(_))) = find(&nh.properties, "PSTemplate").and_then(|p| p.as_object()).map(ObjRef::from_index) else { continue };
                            let point = if nm("SocketName").is_empty() { nm("BoneName") } else { nm("SocketName") };
                            let attach = matches!(find(&nh.properties, "bAttach").map(|p| &p.value), Some(Value::Bool(true)));
                            NotifyKind::PlayParticleEffect { template: pkg.obj_path(r), point, attach }
                        }
                        // The melee item's swing trail runs over the notify's duration.
                        "DisNotify_Trails" => NotifyKind::Trails,
                        "DisNotify_DropItem" => {
                            let Ok(nh) = pkg.read_object(ni) else { continue };
                            let en = |k: &str| match find(&nh.properties, k).map(|p| &p.value) {
                                Some(Value::Enum(v) | Value::Name(v)) => v.clone(),
                                _ => String::new(),
                            };
                            let which = match en("m_WhichItem").as_str() {
                                "DisWhichEquippedItemToDrop_All_Attached" => 1,
                                "DisWhichEquippedItemToDrop_ByEquipUsage" => 2,
                                _ => 0,
                            };
                            let usage = en("m_ByUsage");
                            let velocity = find(&nh.properties, "m_DropVelocity").and_then(|p| p.as_vector()).map(glam::Vec3::from).unwrap_or(glam::Vec3::ZERO);
                            NotifyKind::DropItem(DropNotify { t, which, primary: usage != "EDisEquipUsage_Secondary", velocity })
                        }
                        "DishonoredNotify_SeverLimb" => {
                            let Ok(nh) = pkg.read_object(ni) else { continue };
                            let flag = |k: &str, default: bool| match find(&nh.properties, k).map(|p| &p.value) {
                                Some(Value::Bool(b)) => *b,
                                _ => default,
                            };
                            let bone = match find(&nh.properties, "m_SeverBoneName").map(|p| &p.value) {
                                Some(Value::Name(b)) if b != "None" => b.clone(),
                                _ => continue,
                            };
                            let mode = match find(&nh.properties, "m_ImpulseMode").map(|p| &p.value) {
                                Some(Value::Enum(e) | Value::Name(e)) if e.ends_with("SeverBoneX") => 1,
                                Some(Value::Enum(e) | Value::Name(e)) if e.ends_with("SeverBoneY") => 2,
                                Some(Value::Enum(e) | Value::Name(e)) if e.ends_with("SeverBoneZ") => 3,
                                _ => 0,
                            };
                            let strength = find(&nh.properties, "m_fImpulseStrength").and_then(|p| p.as_float()).unwrap_or(300.0);
                            NotifyKind::SeverLimb(SeverNotify { t, bone, blood: flag("m_bUseBlood", true), hide: flag("m_bHideLimb", false), ragdoll: flag("m_bGoToRagdoll", true), mode, strength })
                        }
                        _ => NotifyKind::Other { class: class.clone() },
                    };
                    seq_notifies.push(Notify { t, duration, kind });
                }
                seq_notifies.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
                notifies.insert(name.to_ascii_lowercase(), seq_notifies);
            }
        }
        self.sets.push(SetData { pkg: r.0.clone(), tracks, seqs, notifies });
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
        let physics = p.physics.as_ref().and_then(|r| dishonored_assets::npc_anim::load_physics_asset(&self.content, r).map_err(|e| warn!("physics asset {}: {e:#}", r.1)).ok()).filter(|a| a.bodies.len() >= 2).map(Arc::new);
        // Configs cooked outside the searched packages (tallboy, musical overseer) fall back to the
        // default config.
        let default_lookat = ("DishonoredGame".to_string(), "DisLookatConfig.DefaultLookatConfig".to_string());
        let lookat = p.lookat.as_ref().and_then(|r| dishonored_assets::npc_anim::load_lookat_config(&self.content, r).ok()).or_else(|| p.lookat.as_ref().and_then(|_| dishonored_assets::npc_anim::load_lookat_config(&self.content, &default_lookat).ok())).map(Arc::new);
        let prefix = [p.prefix(true).to_string(), p.prefix(false).to_string()];
        let limbs = p.severed_limbs.as_ref().and_then(|r| dishonored_assets::npc_anim::load_severed_limb_info(&self.content, r).map_err(|e| warn!("severed limbs {}: {e:#}", r.1)).ok()).map(Arc::new);
        self.profiles.push(Profile { sets, loco, defs, lookup: HashMap::new(), physics, lookat, prefix, limbs });
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

    /// All notifies of a profile's sequence, cloned.
    pub fn notifies(&mut self, profile: usize, name: &str) -> Vec<Notify> {
        match self.holder(profile, name) {
            Some(s) => self.sets[s].notifies.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// Notifies of a profile's sequence that crossed the time window.
    /// A notify at time `t` crosses if `t0 < t <= t1` (when `t0` is `Some`), or `t <= t1` (when `t0` is `None`).
    pub fn crossed(&mut self, profile: usize, clip: &str, t0: Option<f32>, t1: f32) -> Vec<Notify> {
        let notifies = self.notifies(profile, clip);
        crossing(&notifies, t0, t1)
    }

    /// Whether a `DisNotify_Trails` window of a profile's sequence covers `t`.
    pub fn trailing(&mut self, profile: usize, name: &str, t: f32) -> bool {
        match self.holder(profile, name) {
            Some(s) => {
                if let Some(nots) = self.sets[s].notifies.get(&name.to_ascii_lowercase()) {
                    nots.iter().any(|n| matches!(n.kind, NotifyKind::Trails) && t >= n.t && t < n.t + n.duration)
                } else {
                    false
                }
            }
            None => false,
        }
    }

    /// Time of the sequence's `DishonoredNotify_Ragdoll`, when it has one (first occurrence).
    pub fn ragdoll_time(&mut self, profile: usize, name: &str) -> Option<f32> {
        let set = self.holder(profile, name)?;
        self.sets[set]
            .notifies
            .get(&name.to_ascii_lowercase())?
            .iter()
            .find(|n| matches!(n.kind, NotifyKind::Ragdoll))
            .map(|n| n.t)
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

    /// Upper-body mask: the branch from `spine_0_jnt` (`UpperBodyFilter`, NPC_Human_at).
    fn mask(&mut self, lib: &CharacterLibrary, model: usize) -> Arc<Vec<Vec<f32>>> {
        self.branch_mask(lib, model, "spine_0_jnt", 0)
    }

    /// Face mask: the descendants of `head_jnt` (`FaceFilter`, `m_bExcludeBranchBones`).
    fn face_mask(&mut self, lib: &CharacterLibrary, model: usize) -> Arc<Vec<Vec<f32>>> {
        self.branch_mask(lib, model, "head_jnt", 1 << 40)
    }

    /// Weight 1 for `branch` and its descendants (`AnimNodeBlendPerBone`), cached per model.
    fn branch_mask(&mut self, lib: &CharacterLibrary, model: usize, branch: &str, key: usize) -> Arc<Vec<Vec<f32>>> {
        if let Some(m) = self.masks.get(&(model | key)) {
            return m.clone();
        }
        let m: Vec<Vec<f32>> = lib.models[model]
            .parts
            .iter()
            .map(|p| {
                (0..p.bones.len())
                    .map(|mut i| {
                        // The face filter excludes the branch bone itself.
                        if key != 0 && p.bones[i].name.eq_ignore_ascii_case(branch) {
                            return 0.0;
                        }
                        for _ in 0..64 {
                            if p.bones[i].name.eq_ignore_ascii_case(branch) {
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
        self.masks.insert(model | key, m.clone());
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
    /// Held in a choke (`StateNPCMasterBeingChoked`).
    Choke,
    /// Carried over a shoulder (`CorpseBeingCarried`).
    Carried,
    /// Held while the AI keeps this combat move (`PlayerInput::npc_move`): the Tallboy's bow
    /// aim, the Overseer's combat tune.
    Move(u8),
    /// Held until replaced (the held item's walk upper body).
    Held,
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
    /// Per step, its `m_FaceAnimName` (empty: none).
    pub faces: Vec<String>,
}

impl Chain {
    fn one(name: String, key: &'static str) -> Self {
        Chain { steps: vec![(name, false)], faces: Vec::new(), i: 0, t: 0.0, sustain: Sustain::None, weight: 0.0, blend_in: 0.2, blend_out: 0.1, hold: false, fading: false, key }
    }
}

#[derive(Component, Default)]
pub struct PawnAnim {
    /// The AI combat move last seen in the pawn's input (`PlayerInput::npc_move`).
    pub last_move: u8,
    /// The behaviour's GenericAction request played last (its serial).
    pub action_serial: u32,
    pub phase: f32,
    pub modifier: String,
    pub prev_modifier: Option<(String, f32)>,
    pub full: Option<Chain>,
    pub upper: Option<Chain>,
    pub stance: Option<NpcStance>,
    pub was_alive: bool,
    pub init: bool,
    pub ambient: f32,
    pub rng: u32,
    pub sword: Option<dishonored_sim::state::SwordPhase>,
    pub left: Option<dishonored_sim::state::LeftPhase>,
    /// [interp] Matinee anim track samples on this pawn (set each frame by `cutscene.rs`).
    pub scripted: Vec<Scripted>,
    /// Smoothed locomotion velocity (angle from facing in degrees, speed).
    pub vel: (f32, f32),
    pub vel_init: bool,
    /// Locomotion sync-group master this frame (for footfall notifies).
    pub loco_master: Option<LocoMaster>,
    /// Presentation ragdoll of a body on the ground.
    pub ragdoll: Option<crate::ragdoll::Ragdoll>,
    /// Run the next ragdoll to rest at once (the pawn was first seen down).
    pub settle: bool,
    /// Dead or unconscious last frame.
    pub was_down: bool,
    /// The carrier let go of this body (the ragdoll starts from the carried pose).
    pub dropped: bool,
    /// Moving (start / stop hysteresis between the config's stop and start speeds).
    pub moving: bool,
    /// Seconds to the next blink, and the blink playing (time, rate).
    pub blink_timer: f32,
    pub blink: Option<(f32, f32)>,
    /// Smoothed look-at angles relative to facing (yaw right, pitch up; degrees) and weight.
    pub look: (f32, f32, f32),
    /// Ambient subprocess frequency range in use (min, max seconds).
    pub ambient_range: (f32, f32),
    /// Feet planted (left, right), and those that touched down this frame.
    pub feet: [bool; 2],
    pub feet_down: [bool; 2],
    /// Thrown (flying or in the throw's ragdoll) last frame.
    pub thrown: bool,
    /// The bow's fire button seen held last frame (the Tallboy's shot is its press).
    pub shot: bool,
    /// The upper-body loop the held item asks for (`DishonoredNPCPawn.m_WalkUpperBodyDesire`:
    /// chain, group, step), e.g. the music box's `MusicBoxIdle`.
    pub walk_upper: Option<(NpcAnim, usize, usize)>,
}

impl PawnAnim {
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Item prefix of `eDisItemPrefix` steps: the held item's `m_ItemPrefix` (`Sword_`, `MusicBox_`,
/// `Patrol_`, ...) or the empty hands' (`Empty_`), from the pawn tweak (`Profile::prefix`).
fn prefixed(name: &str, kind: u8, prefix: &str) -> String {
    if kind == 0 { name.to_string() } else { format!("{prefix}{name}") }
}

impl AnimBank {
    /// The item prefix of a profile for a pawn holding its primary item or nothing.
    pub fn item_prefix(&self, profile: usize, armed: bool) -> String {
        self.profiles.get(profile).map(|p| p.prefix[if armed { 0 } else { 1 }].clone()).unwrap_or_else(|| if armed { "Sword_".into() } else { "Empty_".into() })
    }
}

/// Direction of a hit source relative to the victim's facing, degrees (0 front, +90 right).
fn hit_angle(victim: &dishonored_sim::state::PlayerState, from: Vec3) -> f32 {
    let d = (from - victim.position).truncate();
    if d.length_squared() < 1e-3 {
        return 0.0;
    }
    let f = dishonored_sim::movement::forward_vector(victim.yaw).truncate();
    let r = dishonored_sim::movement::right_vector(victim.yaw).truncate();
    d.dot(r).atan2(d.dot(f)).to_degrees()
}

/// `ECardinalDirection` (Forward, Left, Right, Back) of an angle: 90-degree sectors.
pub fn cardinal(angle: f32) -> usize {
    let a = angle.abs();
    if a <= 45.0 {
        0
    } else if a >= 135.0 {
        3
    } else if angle < 0.0 {
        1
    } else {
        2
    }
}

/// `eDis8WayDirection` / `eDisDeathDirection` (Forward, Forward_Left, Forward_Right, Left,
/// Right, Back, Back_Left, Back_Right): 45-degree sectors.
pub fn eight_way(angle: f32) -> usize {
    let a = angle.abs();
    let left = angle < 0.0;
    if a <= 22.5 {
        0
    } else if a <= 67.5 {
        if left { 1 } else { 2 }
    } else if a <= 112.5 {
        if left { 3 } else { 4 }
    } else if a <= 157.5 {
        if left { 6 } else { 7 }
    } else {
        5
    }
}

/// Step of a directional group (`m_bEndChainAfterOneAnimation`): the step index is the
/// direction enum's value. Groups of 4 follow `ECardinalDirection` (the strong hit react state
/// plays its step `m_Direction` and turns by +90 / -90 / 180 degrees for Left / Right / Back,
/// 0x00A78980); groups of 8 or more `eDisDeathDirection` (8 ways, then the `_Up` variants at
/// 8..13). Sector thresholds LIKELY (U-143).
fn directional_step(n: usize, angle: f32) -> usize {
    if n >= 8 {
        eight_way(angle)
    } else if n >= 4 {
        cardinal(angle)
    } else {
        0
    }
}

impl AnimBank {
    /// A chain from the profile's animation definitions. `step`: Some(i) plays one step of an
    /// end-after-one group; None plays the group in order.
    fn chain(&mut self, profile: usize, anim: NpcAnim, group: Option<usize>, angle: f32, armed: bool, rng: f32, key: &'static str) -> Option<Chain> {
        let defs = self.profiles[profile].defs.as_ref()?;
        let ch = defs.chain(anim)?.clone();
        let g = group.unwrap_or(((rng * ch.groups.len() as f32) as usize).min(ch.groups.len() - 1));
        let grp = ch.groups.get(g)?.clone();
        let prefix = self.item_prefix(profile, armed);
        let mut steps: Vec<(String, bool, String)> = Vec::new();
        if grp.end_after_one {
            let k = directional_step(grp.steps.len(), angle);
            let s = grp.steps.get(k).or(grp.steps.first())?;
            let name = s.names.get(((rng * 7919.0) as usize) % s.names.len().max(1)).cloned()?;
            steps.push((prefixed(&name, s.item_prefix, &prefix), s.looped, s.face.clone()));
        } else {
            for s in &grp.steps {
                if s.names.is_empty() {
                    continue;
                }
                let name = &s.names[((rng * 7919.0) as usize) % s.names.len()];
                steps.push((prefixed(name, s.item_prefix, &prefix), s.looped, s.face.clone()));
            }
        }
        steps.retain(|(n, _, _)| n != "None" && self.has(profile, n));
        if steps.is_empty() {
            // A randomly picked group whose sequences this pawn's sets lack (the wolfhound's
            // second `AttackMedium` group names elite-guard sequences): the first group that
            // has them (LIKELY: the chain player's fallback was not traced).
            if group.is_none() {
                for k in 0..ch.groups.len() {
                    if k != g {
                        if let Some(c) = self.chain(profile, anim, Some(k), angle, armed, rng, key) {
                            return Some(c);
                        }
                    }
                }
            }
            return None;
        }
        let blend_in = grp.steps.first().map(|s| s.blend_in).unwrap_or(0.2);
        let faces = steps.iter().map(|s| if self.has(profile, &s.2) { s.2.clone() } else { String::new() }).collect();
        let steps = steps.into_iter().map(|s| (s.0, s.1)).collect();
        Some(Chain { steps, faces, i: 0, t: 0.0, sustain: Sustain::None, weight: 0.0, blend_in, blend_out: grp.blend_out, hold: false, fading: false, key })
    }

    /// Given steps of one group of a chain, in order (each step's first name, item-prefixed),
    /// skipping the `None` and missing ones. `looped` steps keep their loop flag.
    fn chain_steps(&mut self, profile: usize, anim: NpcAnim, group: usize, steps: &[usize], armed: bool, key: &'static str) -> Option<Chain> {
        let defs = self.profiles[profile].defs.as_ref()?;
        let grp = defs.chain(anim)?.groups.get(group)?.clone();
        let prefix = self.item_prefix(profile, armed);
        let mut c = Chain::one(String::new(), key);
        c.steps = steps
            .iter()
            .filter_map(|k| grp.steps.get(*k))
            .filter_map(|s| s.names.first().map(|n| (prefixed(n, s.item_prefix, &prefix), s.looped)))
            .filter(|(n, _)| n != "None")
            .collect();
        c.steps.retain(|(n, _)| self.has(profile, n));
        if c.steps.is_empty() {
            return None;
        }
        c.blend_in = grp.steps.first().map(|s| s.blend_in).unwrap_or(0.2);
        c.blend_out = grp.blend_out;
        Some(c)
    }

    /// The get-up of a thrown pawn at rest (`StateNPCMasterStunned` with
    /// `eStunEnterAnim_RagdollFront`, 0x00A814B0): `Stun` group 1, `Generic_RagdollToFront`, the
    /// stun loop for `m_fStunDuration` (0 here: one tick) and one recovery. The simulation rolled
    /// the recovery and holds the pawn for its length (`movement::step_thrown`): the alternative
    /// whose length best matches the hold left after the first two steps is the one played.
    fn stun_getup(&mut self, profile: usize, armed: bool, hold: f32) -> Option<Chain> {
        let defs = self.profiles[profile].defs.as_ref()?;
        let grp = defs.chain(NpcAnim::Stun)?.groups.get(1)?.clone();
        let prefix = self.item_prefix(profile, armed);
        let mut c = Chain::one(String::new(), "getup");
        c.steps.clear();
        let mut used = 0.0;
        for (k, s) in grp.steps.iter().enumerate() {
            let names: Vec<String> = s.names.iter().map(|n| prefixed(n, s.item_prefix, &prefix)).filter(|n| n != "None" && self.has(profile, n)).collect();
            let Some(first) = names.first().cloned() else { continue };
            let name = if k + 1 == grp.steps.len() && names.len() > 1 {
                let left = hold - used;
                names.iter().min_by(|a, b| (self.duration(profile, a).unwrap_or(0.0) - left).abs().total_cmp(&(self.duration(profile, b).unwrap_or(0.0) - left).abs())).cloned().unwrap_or(first)
            } else {
                first
            };
            if !s.looped {
                used += self.duration(profile, &name).unwrap_or(0.0);
            }
            c.steps.push((name, s.looped));
        }
        if c.steps.is_empty() {
            return None;
        }
        c.blend_in = grp.steps.first().map(|s| s.blend_in).unwrap_or(0.2);
        c.blend_out = grp.blend_out;
        Some(c)
    }

    /// A step of the profile's `Choke` chain whose sequence name contains `suffix` (the slave
    /// sequences `*_In`, `*_Loop`, `*_Win`, `*_Lose`, `*_Cancel`, `*CarryCorpse*`).
    fn choke_step(&mut self, profile: usize, suffix: &str) -> Option<String> {
        let defs = self.profiles[profile].defs.as_ref()?;
        let names: Vec<String> = defs.chain(NpcAnim::Choke)?.groups.iter().flat_map(|g| g.steps.iter()).flat_map(|s| s.names.iter().cloned()).collect();
        names.into_iter().find(|n| n.to_ascii_lowercase().contains(suffix) && self.has(profile, n))
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
    /// The behaviour's GenericAction request: (`eDisNPCAnimations` index, serial).
    pub action: Option<(u8, u32)>,
    pub armed: bool,
    pub talking: bool,
    pub dt: f32,
    /// The current view tick (for age calculations on tick-stamped events).
    pub tick: u64,
    /// Hit / death events this frame: (kind, source position, tick produced).
    pub hits: &'a [(HitKind, Vec3, u64)],
    /// Floor height in pawn space (the model stands on it) and gravity (uu/s^2, negative), for
    /// the ragdoll.
    pub floor: f32,
    /// Floor height (pawn space) under a pawn-space point, from the level collision; the
    /// ragdoll samples it at the hand-over (None: the flat `floor`).
    pub ground: Option<&'a dyn Fn(f32, f32) -> f32>,
    /// The level's collision triangles around the pawn (pawn space), asked for at the ragdoll
    /// hand-over (None: no walls).
    pub walls: Option<&'a dyn Fn() -> Vec<[Vec3; 3]>>,
    pub gravity: f32,
    /// Ground speed of the pawn carrying this body (0 when not carried).
    pub carrier_speed: f32,
    /// What the pawn looks at (world position), when anything (the approximation for spoken
    /// lines; the AI's own look-at is `head`).
    pub look_target: Option<Vec3>,
    /// The AI's look-at aims (`MatchView::npc_heads`), when the head is turned.
    pub head: Option<NpcHead>,
    /// FaceFX bone offsets of the line the pawn speaks (ANIMATION.md §12).
    pub face: &'a [crate::facefx::FaceBone],
}

/// The animation reaction an event asks for (decided in the Bevy-free `present_rules`).
pub use crate::present_rules::HitKind;

/// Locomotion modifier for a pawn (`ArkComponentLocomotionConfig.m_Modifiers`).
fn modifier_for(i: &AnimInput, cfg: &LocoConfig, item: &str) -> String {
    use dishonored_sim::state::PawnKind;
    // An item with a modifier of its own name in the config: the music box (`MusicBox`, idle
    // `Musicbox_Idle`) of the elite guard config's Overseers (LIKELY: the modifier choice of the
    // locomotion component was not traced, U-892).
    let stem = item.trim_end_matches('_');
    if i.armed && i.p.kind != PawnKind::Human && !matches!(stem, "" | "Sword" | "Empty") {
        if let Some(k) = cfg.modifier(stem) {
            return cfg.modifiers[k].name.clone();
        }
    }
    let gun_aim = i.p.left.phase == dishonored_sim::state::LeftPhase::Aiming && matches!(i.p.left.item, dishonored_sim::input::LeftHand::Pistol | dishonored_sim::input::LeftHand::Crossbow);
    let want: &[&str] = match (i.p.kind, i.stance, i.armed) {
        (PawnKind::Human, _, true) if gun_aim => &["Aim", "Sword"],
        (PawnKind::Human, _, true) => &["Sword"],
        (PawnKind::Human, _, false) => &["Empty"],
        (_, Some(NpcStance::Search), true) => &["SearchSword", "Sword"],
        (_, Some(NpcStance::Search), false) => &["SearchEmpty", "Empty"],
        (_, Some(NpcStance::Panic), true) => &["PanicSword", "Sword"],
        (_, Some(NpcStance::Panic), false) => &["PanicEmpty", "Empty"],
        // `eDisNPCBodyStance_PanicRecover` (unarmed) / `Equipped` (armed) of the cower state.
        (_, Some(NpcStance::Cower), false) => &["PanicRecoverEmpty", "Empty"],
        (_, Some(NpcStance::Combat | NpcStance::Notice | NpcStance::Cower), true) => &["Sword"],
        (PawnKind::Guard, _, _) => &["SwordPatrol", "Sword"],
        (_, _, true) => &["Sword"],
        _ => &["Empty"],
    };
    want.iter().find(|w| cfg.modifier(w).is_some()).map(|s| s.to_string()).or_else(|| cfg.modifiers.first().map(|m| m.name.clone())).unwrap_or_default()
}

/// Directional weights for the four move anims (S, W, N, E) from the movement angle relative
/// to facing (0 = forward, +90 = right): anim `k` covers 180 + 90k degrees. Like 0x009331D0,
/// a neighbour within 0.98 takes the whole weight. Returns the weights and whether both
/// neighbours are in use (the diagonal correction needs two).
pub fn dir_weights(angle_deg: f32) -> ([f32; 4], bool) {
    let a = (angle_deg - 180.0).rem_euclid(360.0) / 90.0;
    let k0 = a.floor() as usize % 4;
    let f = a - a.floor();
    let mut w = [0.0; 4];
    if 1.0 - f >= 0.98 {
        w[k0] = 1.0;
        return (w, false);
    }
    if f >= 0.98 {
        w[(k0 + 1) % 4] = 1.0;
        return (w, false);
    }
    w[k0] += 1.0 - f;
    w[(k0 + 1) % 4] += f;
    (w, true)
}

/// Diagonal stride correction (0x00910E30): two direction loops blended at an angle between
/// them cover the chord between their unit directions (table of 8 octant vectors at
/// 0x014527C0), so the speed is divided by the distance from the centre to that chord along
/// the movement direction: `cos(phi) + sin(phi)` for neighbours 90 degrees apart, `phi` the
/// angle from the first neighbour.
pub fn diagonal_factor(angle_deg: f32) -> f32 {
    let (_, both) = dir_weights(angle_deg);
    if !both {
        return 1.0;
    }
    let phi = ((angle_deg - 180.0).rem_euclid(90.0)).to_radians();
    phi.cos() + phi.sin()
}

/// Polar velocity smoothing of `ArkAnimNodeLocomotion` (0x0090BC00): the node's velocity
/// (angle, speed) moves toward the pawn's in the plane, by `rate * clamp(dist / range, min, 1)
/// * dt` per tick. Angles in degrees relative to facing.
pub fn smooth_velocity(cur: (f32, f32), target: (f32, f32), dt: f32, p: &LocoNodeParams) -> (f32, f32) {
    let to = |(a, s): (f32, f32)| Vec2::new(a.to_radians().cos() * s, a.to_radians().sin() * s);
    let (c, t) = (to(cur), to(target));
    let d = (t - c).length();
    if d <= 0.0 {
        return cur;
    }
    let ratio = (d / p.range.max(1e-3)).clamp(p.min_ratio, 1.0);
    let k = (p.rate * ratio * dt / d).min(1.0);
    let v = c + (t - c) * k;
    // The node keeps the angle as a rotator (int) and the speed separately.
    let angle = if v.length_squared() > 1e-8 { v.y.atan2(v.x).to_degrees() } else { cur.0 };
    (angle, v.length())
}

/// Tunables of the `LocoBlender` node (`ArkAnimNodeLocomotion` of `NPC_Human_at`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocoNodeParams {
    /// +0x298: velocity change rate (uu/s per second at full ratio).
    pub rate: f32,
    /// +0x29C: velocity difference at which the full rate applies.
    pub range: f32,
    /// +0x2A0: minimum fraction of the rate.
    pub min_ratio: f32,
    /// +0x27C: crossfade time between the two modifier slots.
    pub modifier_blend: f32,
}

impl Default for LocoNodeParams {
    fn default() -> Self {
        // `Default__ArkAnimNodeLocomotion` (Engine).
        LocoNodeParams { rate: 2000.0, range: 100.0, min_ratio: 0.1, modifier_blend: 0.5 }
    }
}

/// One locomotion sample: sequence, played backwards, weight.
struct LocoSample {
    name: String,
    reversed: bool,
    weight: f32,
    /// The speed mode's `m_fFootOnGroundPct`, and whether the mode moves.
    foot: f32,
    moving: bool,
}

/// Sync-group master of a locomotion evaluation: (sequence, reversed, phase before, after).
pub type LocoMaster = (String, bool, f32, f32);

/// Locomotion pose: speed modes of the modifier blended by speed, directions by angle.
/// `vel` is the node's smoothed (angle from facing in degrees, speed).
#[allow(clippy::too_many_arguments)]
fn loco_pose(bank: &mut AnimBank, lib: &CharacterLibrary, model: usize, profile: usize, modifier: &str, vel: (f32, f32), dt: f32, phase: &mut f32, advance: bool) -> Option<(Pose, Option<LocoMaster>, f32)> {
    let cfg = bank.profiles[profile].loco.clone()?;
    let mi = cfg.modifier(modifier)?;
    let (angle, speed) = vel;
    let (dw, _) = dir_weights(angle);
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
    // Bracketing pair (0x009105B0) on the diagonal-corrected speed (0x00916770 stores
    // `factor * speed` at +0x208 before the search). The factor applies when the speed falls
    // into a moving mode (more than one direction loop).
    let bracket = |speed: f32| {
        let mut lo = 0;
        for k in 0..modes.len() {
            if modes[k].1 <= speed {
                lo = k;
            }
        }
        lo
    };
    // 0x00910E30 takes the bracketing mode of the larger weight, or the other one when that
    // mode's speed is below 0.1 (idle), and corrects only for a mode with several loops.
    let lo0 = bracket(speed);
    let hi0 = (lo0 + 1).min(modes.len() - 1);
    let w0 = if hi0 != lo0 && modes[hi0].1 > modes[lo0].1 { ((speed - modes[lo0].1) / (modes[hi0].1 - modes[lo0].1)).clamp(0.0, 1.0) } else { 0.0 };
    let (dom, other) = if w0 >= 0.5 { (hi0, lo0) } else { (lo0, hi0) };
    let pick = if modes[dom].1.abs() < 0.1 { other } else { dom };
    let moving_mode = modes[pick].0.move_anims.len() > 1;
    let speed = if moving_mode && speed > 0.0 { speed * diagonal_factor(angle) } else { speed };
    let (lo, hi, w_hi) = {
        let lo = bracket(speed);
        let hi = (lo + 1).min(modes.len() - 1);
        let w = if hi != lo && modes[hi].1 > modes[lo].1 { ((speed - modes[lo].1) / (modes[hi].1 - modes[lo].1)).clamp(0.0, 1.0) } else { 0.0 };
        // Snap within 2 % (0.98 threshold in the original).
        let w = if w > 0.98 { 1.0 } else if w < 0.02 { 0.0 } else { w };
        (lo, hi, w)
    };
    let mut samples: Vec<LocoSample> = Vec::new();
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
                    samples.push(LocoSample { name: m.move_anims[d].0.clone(), reversed: d < 2, weight: w, foot: m.foot_on_ground, moving: m.move_anims[0].1.abs() >= 0.1 });
                }
            }
        } else if let Some(a) = m.move_anims.first() {
            samples.push(LocoSample { name: a.0.clone(), reversed: false, weight: mw, foot: m.foot_on_ground, moving: a.1.abs() >= 0.1 });
        }
    }
    // Sync group (`LocoGroup`): the heaviest sequence is the master and fires the notifies;
    // the group rate (0x00916950) is `master length * speed / sum(weight * authored speed *
    // length)` times the modifier's `m_fSpeedMultiplier`, or 1 when either is below 0.1.
    let mut master: Option<(usize, f32)> = None;
    let mut sum = 0.0;
    for (i, s) in samples.iter().enumerate() {
        let Some(d) = bank.duration(profile, &s.name) else { continue };
        if master.is_none_or(|m| s.weight > samples[m.0].weight) {
            master = Some((i, d));
        }
        let authored = modes.iter().flat_map(|m| m.0.move_anims.iter()).find(|a| a.0 == s.name).map(|a| a.1).unwrap_or(0.0);
        sum += s.weight * authored * d;
    }
    let mult = cfg.modifiers.get(mi).map(|m| m.speed_multiplier).unwrap_or(1.0);
    let before = *phase;
    if advance {
        if let Some((_, d)) = master {
            let step = if speed.abs() >= 0.1 && sum.abs() >= 0.1 { dt * speed * mult / sum } else { dt / d.max(0.05) };
            *phase = (*phase + step).rem_euclid(1.0);
        }
    }
    let mut acc = Acc::new(&lib.models[model].parts);
    for s in &samples {
        let Some(d) = bank.duration(profile, &s.name) else { continue };
        if let Some(p) = bank.sample(lib, model, profile, &s.name, *phase * d, s.reversed) {
            acc.add(&p, s.weight);
        }
    }
    let m = master.map(|(i, _)| (samples[i].name.clone(), samples[i].reversed, before, *phase));
    // Half-width of a foot's planted window (0x009114A0): the weighted `m_fFootOnGroundPct`
    // of the moving modes (normalized when their weights sum below 1), halved.
    let (fw, ws) = samples.iter().filter(|s| s.moving).fold((0.0, 0.0), |a, s| (a.0 + s.foot * s.weight, a.1 + s.weight));
    let half = if ws > 0.0 && ws < 1.0 { fw / ws } else { fw } * 0.5;
    Some((acc.finish(), m, half))
}

/// Feet planted at a sync phase (`m_bFootOnGround`, 0x009114A0): foot k is on the ground while
/// the circular distance between the phase and its `m_fOnGroundPct` is within `half`.
pub fn feet_on_ground(feet: &[f32], phase: f32, half: f32) -> [bool; 2] {
    let mut out = [false; 2];
    for (k, f) in feet.iter().take(2).enumerate() {
        let mut d = (f - phase).abs();
        if d > 0.5 {
            d = 1.0 - d;
        }
        out[k] = half > 0.0 && d <= half;
    }
    out
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
    use dishonored_sim::state::{Condition, LeftPhase, PawnKind, SwordPhase};
    if !st.init {
        st.init = true;
        st.rng = i.p.id.wrapping_mul(2654435761) | 1;
        st.was_alive = i.p.alive;
        st.ambient = 2.0 + st.rand() * 4.0;
        st.phase = st.rand();
        st.blink_timer = 0.5 + st.rand() * 5.5;
        if (!i.p.alive || i.p.condition == Condition::Unconscious) && i.p.carried_by.is_none() {
            // Already down when first seen: the end of a death, then the ragdoll at rest.
            if let Some(mut c) = bank.chain(profile, NpcAnim::Death, None, 0.0, i.armed, 0.0, "dead") {
                c.hold = true;
                c.weight = 1.0;
                c.i = c.steps.len() - 1;
                c.t = bank.duration(profile, &c.steps[c.i].0).unwrap_or(0.0);
                st.full = Some(c);
            }
            st.settle = true;
            st.was_down = true;
        }
    }
    let r = st.rand();
    let p = i.p;
    // Combat moves the AI chose (`PlayerInput::npc_move`, replicated in the pawn's last input):
    // the move's `eDisNPCAnimations` chain starts when the move changes. Short attacks play
    // through the swing below; side steps take the front step of their directional group.
    let mv = p.last_input.npc_move;
    // The held item's upper-body loop (`m_WalkUpperBodyDesire`): the music box's equip
    // (`DisWepMusicBox` slot 81, 0x00C291F0) asks for `MusicBoxIdle`.
    if st.walk_upper.is_none() && i.armed && bank.item_prefix(profile, true).eq_ignore_ascii_case("MusicBox_") {
        st.walk_upper = Some((NpcAnim::MusicBoxIdle, 0, 0));
    }
    let dead = st.full.as_ref().is_some_and(|f| f.hold && f.key == "dead");
    if mv != st.last_move {
        let prev = st.last_move;
        st.last_move = mv;
        use dishonored_sim::input::npc_moves as m;
        let able = p.alive && !dead && !p.incapacitated();
        // A held move that ended plays its outro: the Tallboy's aim state leaves through
        // `TallboyAimOut` (sub-state 3). The combat tune's chain carries its own outro step. Its
        // end (`DisItemContext_NPCTune_Combat` slot 103, 0x00C26F70) sets the walk upper body
        // to `PlayMusic` step 1 (`MusicBox_PlayMusic_Loop`).
        if prev == m::TALLBOY_FIRE_BOW && able {
            if let Some(c) = bank.chain(profile, NpcAnim::TallboyAimOut, Some(0), 0.0, true, r, "move") {
                let w = st.full.as_ref().map_or(0.0, |f| f.weight);
                st.full = Some(Chain { weight: w, ..c });
            }
        }
        if prev == m::OVERSEER_TUNE {
            st.walk_upper = Some((NpcAnim::PlayMusic, 0, 1));
        }
        let next = if !able {
            None
        } else {
            match mv {
                m::ATTACK_STEP_BACK => bank.chain(profile, NpcAnim::AttackStepBack, None, 0.0, true, r, "move"),
                m::ATTACK_MEDIUM => bank.chain(profile, NpcAnim::AttackMedium, None, 0.0, true, r, "move"),
                m::ATTACK_LONG => bank.chain(profile, NpcAnim::AttackLong, None, 0.0, true, r, "move"),
                m::JUMP_ATTACK => bank.chain(profile, NpcAnim::JumpAttack, None, 0.0, true, r, "move"),
                m::SIDE_STEP => bank.chain(profile, NpcAnim::SideStep, None, 0.0, true, r, "move"),
                m::BACK_STEP => bank.chain(profile, NpcAnim::JumpBack, None, 0.0, true, r, "move"),
                m::FIRE_PISTOL => bank.chain(profile, NpcAnim::AimShoot, None, 0.0, true, r, "move"),
                // The special classes (systems/ai.md section 12). Each context's
                // `m_MainAttack.m_Animation`: the hound's bite `AttackMedium`, the Tallboy's leg
                // `TallboyStompAttack`, the music box's `Bash`.
                m::WH_BITE => bank.chain(profile, NpcAnim::AttackMedium, None, 0.0, true, r, "move"),
                m::TALLBOY_STOMP => bank.chain(profile, NpcAnim::TallboyStompAttack, None, 0.0, true, r, "move"),
                m::OVERSEER_BASH => bank.chain(profile, NpcAnim::Bash, None, 0.0, true, r, "move"),
                // Minigame attacks (`DisTweaks_Minigame.m_NPCAnimation` = `SyncAttack`, group
                // `m_GroupIndex`, steps by minigame state). The simulation has no sync-attack
                // hold, so the grab never connects: the hound's jump attack (group 1) jumps then
                // steps back (its steps 1-3 are None: the knockdown), its arm attack (group 0)
                // and the weeper's grab (`DisTweaks_WeeperGrab_2`, group 0) try and miss
                // (LIKELY, U-893).
                m::WH_JUMP_ATTACK => bank.chain_steps(profile, NpcAnim::SyncAttack, 1, &[0, 4], true, "move"),
                m::WH_ARM_ATTACK => bank.chain_steps(profile, NpcAnim::SyncAttack, 0, &[0, 1], true, "move"),
                m::WEEPER_GRAB => bank.chain_steps(profile, NpcAnim::SyncAttack, 0, &[0, 1], i.armed, "move"),
                // The weeper's cough (`DisNPCAnim_Weeper_Cough`, `Empty_Idle_Cough`).
                m::WEEPER_COUGH => bank.chain(profile, NpcAnim::WeeperCough, None, 0.0, i.armed, r, "move"),
                // `StateNPCMasterTallboyAim` (0x00A7CEB0): in (sub-state 1), the aim loop (4)
                // held while the AI aims; the shot and the outro below.
                m::TALLBOY_FIRE_BOW => {
                    let mut c = bank.chain(profile, NpcAnim::TallboyAimIn, Some(0), 0.0, true, r, "move");
                    if let (Some(c), Some(l)) = (c.as_mut(), bank.chain(profile, NpcAnim::TallboyAim, Some(0), 0.0, true, r, "move")) {
                        c.steps.extend(l.steps);
                        c.faces.clear();
                    }
                    c.map(|mut c| {
                        c.sustain = Sustain::Move(mv);
                        c
                    })
                }
                // The combat tune (`DisItemContext_NPCTune_Combat` slot 108, 0x00C202B0: chain
                // 0x77 `PlayCombatTune`): in, the loop held while the tune plays, out.
                m::OVERSEER_TUNE => bank.chain(profile, NpcAnim::PlayCombatTune, None, 0.0, true, r, "move").map(|mut c| {
                    c.sustain = Sustain::Move(mv);
                    c
                }),
                _ => None,
            }
        };
        if let Some(c) = next {
            let w = st.full.as_ref().map_or(0.0, |f| f.weight);
            st.full = Some(Chain { weight: w * 0.5, ..c });
        }
    }
    // The Tallboy's shot (sub-state 0, `TallboyAimShoot`) on the bow's fire press, back to the
    // aim loop after it.
    {
        use dishonored_sim::input::{buttons, npc_moves as m};
        let fire = mv == m::TALLBOY_FIRE_BOW && p.last_input.pressed(buttons::SECONDARY);
        if fire && !st.shot && p.alive && !dead {
            let shoot = bank.chain(profile, NpcAnim::TallboyAimShoot, Some(0), 0.0, true, r, "move");
            let aim = bank.chain(profile, NpcAnim::TallboyAim, Some(0), 0.0, true, r, "move");
            if let Some(mut c) = shoot {
                if let Some(l) = aim {
                    c.steps.extend(l.steps);
                }
                c.faces.clear();
                c.sustain = Sustain::Move(mv);
                c.weight = st.full.as_ref().map_or(0.0, |f| f.weight);
                st.full = Some(c);
            }
        }
        st.shot = fire;
    }
    // Event-driven actions.
    for (kind, from, hit_tick) in i.hits {
        let q = hit_angle(p, *from);
        let next = match kind {
            HitKind::Death | HitKind::DeathFast | HitKind::DeathPlague | HitKind::DeathSwarm => {
                // An assassination victim keeps its assassination sequence as its death.
                if let Some(c) = st.full.as_mut().filter(|c| c.key == "assassinated") {
                    c.hold = true;
                    continue;
                }
                // The death chain of the damage (`Generic_Death_Plague`, `<item>SwarmAttack_Out`),
                // else the directional `Death`.
                let a = match kind {
                    HitKind::DeathFast => NpcAnim::DeathFast,
                    HitKind::DeathPlague => NpcAnim::DeathPlague,
                    HitKind::DeathSwarm => NpcAnim::DeathSwarmAttack,
                    _ => NpcAnim::Death,
                };
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
                bank.profiles[profile].defs.as_ref().and_then(|d| d.chain(NpcAnim::HitReactProjectile)).and_then(|c| c.groups.first()).and_then(|g| g.steps.get(k)).and_then(|s| s.names.first().cloned()).map(|n| prefixed(&n, 2, &bank.item_prefix(profile, i.armed))).filter(|n| bank.has(profile, n)).map(|n| Chain::one(n, "hit"))
            }
            HitKind::Swing if p.alive => {
                let g = if p.kind == PawnKind::Human && p.sword.phase == SwordPhase::Assassinating { Some(5) } else { None };
                bank.chain(profile, NpcAnim::AttackShort, g, 0.0, true, r, "swing")
            }
            HitKind::Parried if p.alive => bank.chain(profile, NpcAnim::Parry, Some(0), 0.0, true, r, "parry"),
            HitKind::Assassinated => bank.chain(profile, NpcAnim::Assassination, None, q, i.armed, r, "assassinated"),
            // The drop assassination's and the synced short fatality's slaves (kept as the death).
            HitKind::AssassinatedDrop => bank.chain(profile, NpcAnim::AssassinationDrop, None, q, i.armed, r, "assassinated"),
            HitKind::ShortFatality { backhand, group } => {
                let a = if *backhand { NpcAnim::ShortFatalityBackhand } else { NpcAnim::ShortFatalityForehand };
                bank.chain(profile, a, Some(*group as usize), 0.0, i.armed, r, "assassinated")
            }
            // A finisher's slave, in the attacker's frame (`viewmodel::BodyFrames`).
            HitKind::Fatality { anim, group } => bank.chain(profile, *anim, Some(*group as usize), 0.0, i.armed, r, "assassinated"),
            // A sleep dart's "death" (`m_bSlowDeathOnly`): the death sequence, then the ragdoll.
            HitKind::KnockedOut => bank.chain(profile, NpcAnim::Death, None, q, i.armed, r, "dead").map(|mut c| {
                c.hold = true;
                c
            }),
            // The end of a choke plays the slave sequence linked to the player's Win / Lose /
            // Cancel (`DisNPCAnimDefinitions.Choke`).
            HitKind::ChokeWin | HitKind::ChokeLose | HitKind::ChokeCancel => {
                let suffix = match kind {
                    HitKind::ChokeWin => "_win",
                    HitKind::ChokeLose => "_lose",
                    _ => "_cancel",
                };
                match bank.choke_step(profile, suffix) {
                    Some(n) => {
                        let mut c = Chain::one(n, "choke_end");
                        c.hold = *kind == HitKind::ChokeWin;
                        c.weight = 1.0;
                        st.full = Some(c);
                    }
                    None => {
                        if let Some(c) = st.full.as_mut().filter(|c| c.key == "choke") {
                            c.sustain = Sustain::None;
                        }
                    }
                }
                continue;
            }
            // A carried body let go: the slave of the carrier's low drop
            // (`Corpses_CarryCorpse_DropLow_Slave` with `Empty_CarryCorpse_DropLow_Master`), drawn
            // in the carrier's frame (`viewmodel::BodyFrames`), then the ragdoll at its
            // `DishonoredNotify_Ragdoll` or end; without it, the ragdoll at once.
            HitKind::Dropped => {
                let drop = bank.profiles[profile]
                    .defs
                    .as_ref()
                    .and_then(|d| d.chain(NpcAnim::CorpseBeingCarried))
                    .and_then(|c| c.groups.first())
                    .and_then(|g| {
                        g.steps.iter().filter_map(|s| s.names.first()).find(|n| {
                            let l = n.to_ascii_lowercase();
                            l.contains("droplow") && !l.contains("sneak")
                        })
                    })
                    .cloned();
                match drop.filter(|n| bank.has(profile, n)) {
                    Some(n) => {
                        let mut c = Chain::one(n, "dropped");
                        c.hold = true;
                        c.weight = 1.0;
                        st.full = Some(c);
                        st.ragdoll = None;
                    }
                    None => st.dropped = true,
                }
                continue;
            }
            _ => None,
        };
        if let Some(mut c) = next {
            if st.full.as_ref().is_some_and(|f| f.hold && f.key == "dead") {
                continue;
            }
            // Advance the chain's time by the event's age in simulation ticks.
            let event_age = ((i.tick.saturating_sub(*hit_tick)) as f32 * dishonored_sim::movement::DT).min(bank.duration(profile, &c.steps[c.i].0).unwrap_or(0.0));
            c.t = event_age;
            let w = st.full.as_ref().map_or(0.0, |f| f.weight);
            st.full = Some(Chain { weight: w * 0.5, ..c });
        }
    }
    // Held in a choke: the slave In -> Loop of `Choke`, kept while the condition lasts.
    if p.condition == Condition::BeingChoked && st.full.as_ref().is_none_or(|c| c.key != "choke") {
        let steps: Vec<(String, bool)> = [("_in", false), ("_loop", true)].iter().filter_map(|(s, l)| bank.choke_step(profile, s).map(|n| (n, *l))).collect();
        if !steps.is_empty() {
            let mut c = Chain::one(String::new(), "choke");
            c.steps = steps;
            c.sustain = Sustain::Choke;
            st.full = Some(c);
        }
    }
    // Carried over a shoulder: `CorpseBeingCarried`, looped while carried (the body rides at
    // the carrier's root; `players::sync_players`).
    // The chain's steps are the carry states (In, Idle, Walk, then the drops), not a sequence:
    // the lift plays In, then Idle or Walk loops with the carrier's movement.
    if p.carried_by.is_some() {
        st.ragdoll = None;
        st.dropped = false;
        let steps: Vec<String> = bank.profiles[profile].defs.as_ref().and_then(|d| d.chain(NpcAnim::CorpseBeingCarried)).and_then(|c| c.groups.first()).map(|g| g.steps.iter().filter_map(|s| s.names.first().cloned()).collect()).unwrap_or_default();
        let (mut lift, idle, walk) = (steps.first().cloned(), steps.get(1).cloned(), steps.get(2).cloned());
        // Lifted straight from a won choke: the choke's `Generic_Choke_CarryCorpse_Slave` (with
        // the master's `Sword_Choke_CarryCorpse_Master`) instead of the lift from the ground.
        if st.full.as_ref().is_some_and(|c| c.key == "choke_end" || c.key == "choke") {
            if let Some(n) = bank.choke_step(profile, "carrycorpse") {
                lift = Some(n);
            }
        }
        if st.full.as_ref().is_none_or(|c| c.key != "carried") {
            let mut c = Chain::one(String::new(), "carried");
            c.steps = [lift.clone().map(|n| (n, false)), idle.clone().map(|n| (n, true))].into_iter().flatten().filter(|s| bank.has(profile, &s.0)).collect();
            if !c.steps.is_empty() {
                c.sustain = Sustain::Carried;
                c.hold = true;
                c.weight = 1.0;
                st.full = Some(c);
            }
        }
        if let Some(c) = st.full.as_mut().filter(|c| c.key == "carried" && c.steps[c.i].1) {
            let want = if i.carrier_speed > 20.0 { walk } else { idle };
            if let Some(w) = want.filter(|w| *w != c.steps[c.i].0) {
                c.steps[c.i].0 = w;
                c.t = 0.0;
            }
        }
    }
    // Thrown (`StateNPCMasterThrown`, physics.md 3.7): the `PushToRagdoll` chain's Windblast
    // group, its step by the throw's direction (`movement::throw_direction`: Front, Left, Right,
    // Back), held to its `DishonoredNotify_Ragdoll` where the ragdoll takes over (below). At
    // rest the simulation stands the pawn up and holds it for the get-up
    // (`StateNPCMasterStunned`, `eStunEnterAnim_RagdollFront`): `Stun` group 1.
    let thrown = p.alive && (p.thrown.active || p.thrown.ragdoll);
    if thrown && !st.thrown && !dead {
        let dir = dishonored_sim::movement::throw_direction(p.yaw, p.velocity);
        if let Some(mut c) = bank.chain_steps(profile, NpcAnim::PushToRagdoll, 1, &[dir], i.armed, "thrown") {
            c.hold = true;
            c.weight = st.full.as_ref().map_or(0.0, |f| f.weight);
            st.full = Some(c);
            st.upper = None;
        }
    }
    if !thrown && st.thrown && p.alive && !p.incapacitated() {
        st.ragdoll = None;
        st.full = bank.stun_getup(profile, i.armed, p.action_lock).map(|mut c| {
            // From the body on the ground (the ragdoll's pose is not blended from).
            c.weight = 1.0;
            c
        });
    }
    st.thrown = thrown;
    // State-driven actions (sustained loops).
    let busy = st.full.as_ref().is_some_and(|c| !c.fading);
    if p.alive && !p.incapacitated() && !thrown {
        if !st.was_alive || st.was_down {
            st.full = None;
            st.ragdoll = None;
        }
        // `Parry_AwaitingImpact`: the ready-parry loop held while blocking.
        if p.sword.phase == SwordPhase::Blocking && !busy {
            if let Some(mut c) = bank.chain(profile, NpcAnim::ParryAwaitingImpact, None, 0.0, true, 0.0, "block") {
                c.sustain = Sustain::Block;
                st.full = Some(c);
            }
        }
        if p.sword.phase == SwordPhase::Staggered && st.sword != Some(SwordPhase::Staggered) {
            if let Some(mut c) = bank.chain(profile, NpcAnim::Stun, Some(0), 0.0, true, r, "stun") {
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
            if let Some(c) = a.and_then(|a| bank.chain(profile, a, None, 0.0, true, r, "gun")) {
                st.full = Some(c);
            }
        }
        // A behaviour's GenericAction (`DisAISubStateGenericAction`: the chain its tweak names, full
        // body): the alarm ring, the search's corpse check / lost target, GoHome's last taunt, the
        // explosive surprise.
        if let Some((anim, serial)) = i.action {
            if serial != st.action_serial {
                st.action_serial = serial;
                let a = match anim {
                    5 => Some(NpcAnim::Panic),
                    32 => Some(NpcAnim::Taunt),
                    35 => Some(NpcAnim::TriggerAlarm),
                    36 => Some(NpcAnim::CorpseCheck),
                    37 => Some(NpcAnim::SearchLostTarget),
                    _ => None,
                };
                if let Some(c) = a.and_then(|a| bank.chain(profile, a, None, 0.0, i.armed, r, "action")) {
                    st.full = Some(c);
                }
            }
        }
        let stance = i.stance;
        if stance != st.stance {
            let prev = st.stance;
            // Cowering is a body stance, not a one-shot: `DisAISubStateCower` (0x00B12840) asks
            // for `eDisNPCBodyStance_PanicRecover` (Equipped when armed) and faces the enemy;
            // the stance's locomotion modifier plays `Empty_Idle_Panic_Recover` (see
            // `modifier_for`).
            let a = match stance {
                Some(NpcStance::Panic) if prev != Some(NpcStance::Cower) => bank.chain(profile, NpcAnim::Panic, None, 0.0, i.armed, r, "panic"),
                Some(NpcStance::Notice) => bank.chain(profile, NpcAnim::ReactToNoticing, None, 0.0, i.armed, r, "notice"),
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
    let mut sustain = vec![Sustain::Held];
    if mv != 0 {
        sustain.push(Sustain::Move(mv));
    }
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
    if p.condition == Condition::BeingChoked {
        sustain.push(Sustain::Choke);
    }
    if p.carried_by.is_some() {
        sustain.push(Sustain::Carried);
    }
    let down = !p.alive || p.condition == Condition::Unconscious;
    st.was_down = down;
    // Locomotion modifier and transition sequences (`m_Transitions`).
    let cfg = bank.profiles[profile].loco.clone();
    if let Some(cfg) = cfg.as_ref() {
        let m = modifier_for(i, cfg, &bank.item_prefix(profile, true));
        if m != st.modifier {
            // Transitions start only once the node's speed is below the config's
            // `m_fVelocityToStopMoving` (class default 20; 0x00911230 compares config +0x94).
            if !st.modifier.is_empty() && p.alive && st.full.is_none() && st.vel.1 < cfg.stop_speed {
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
    // Start / stop sequences of the idle speed mode (`m_StartAnimProps` / `m_StopAnimProps`),
    // played like the node's "Specific anim" with `m_fStartBlendDuration` /
    // `m_fStopBlendDuration` (0.1 s). The original's locomotion component requests them from
    // the path (straight distance); here they follow the pawn starting (speed above
    // `m_fVelocityToStartToMove`) and stopping (below `m_fVelocityToStopMoving`), the stop foot
    // from the loop phase (SPECULATIVE, U-280). Root motion is discarded: the simulation moves
    // the pawn.
    if let Some(cfg) = cfg.as_ref() {
        let speed = p.velocity.truncate().length();
        let was = st.moving;
        if !st.moving && speed > cfg.start_speed && p.alive && !p.incapacitated() {
            st.moving = true;
        } else if st.moving && (speed < cfg.stop_speed || !p.alive || p.incapacitated()) {
            st.moving = false;
        }
        if was != st.moving && p.alive && !p.incapacitated() && st.full.is_none() && p.kind != PawnKind::Human {
            let idle = cfg.modifier(&st.modifier).and_then(|mi| cfg.mode(mi, 0)).cloned();
            let name = idle.and_then(|m| {
                if st.moving {
                    m.start_anims.first().cloned()
                } else {
                    let k = if st.phase < 0.5 { 0 } else { 1 };
                    m.stop_anims.get(k).or(m.stop_anims.first()).cloned()
                }
            });
            if let Some(n) = name.filter(|n| bank.has(profile, n)) {
                let mut c = Chain::one(n, if st.moving { "start" } else { "stop" });
                c.blend_in = 0.1;
                c.blend_out = 0.1;
                st.full = Some(c);
            }
        }
    }
    // Ambient idle variations (`DisAISubProcessAmbientAnims`). Only some behaviours run the
    // subprocess, with their own tweak (U-145): patrols and guard posts
    // (`TweaksBehavior_Patrol_Default` / `TweaksBehavior_Guard_Default`: every 30-60 s, upper
    // body, also while walking), stationary searches (class defaults: 2-6 s, full body, only
    // standing). Idle, wander and group-idle behaviours (the relaxed stance) have none.
    let sub: Option<(f32, f32, bool, bool)> = match i.stance {
        Some(NpcStance::Patrol) => Some((30.0, 60.0, false, false)),
        Some(NpcStance::Search) => Some((2.0, 6.0, true, true)),
        _ => None,
    };
    if let Some((lo, hi, full_body, check_movement)) = sub.filter(|_| p.alive && !p.incapacitated() && p.kind != PawnKind::Human && !i.talking) {
        if st.ambient_range != (lo, hi) {
            st.ambient_range = (lo, hi);
            // The first timer is a random fraction of the maximum (0x00B39620).
            st.ambient = st.rand() * hi;
        }
        let still = p.velocity.truncate().length() < 20.0;
        let upper_free = st.upper.as_ref().is_none_or(|c| c.key == "walk_upper");
        let free = if full_body { st.full.is_none() } else { upper_free && st.full.is_none() };
        if free && (still || !check_movement) {
            st.ambient -= i.dt;
            if st.ambient <= 0.0 {
                let patrol = st.modifier.contains("Patrol") || st.modifier.starts_with("Sword");
                let rr = st.rand();
                if let Some(c) = bank.ambient(profile, patrol, rr) {
                    if full_body {
                        st.full = Some(c);
                    } else {
                        st.upper = Some(c);
                    }
                }
                st.ambient = lo + st.rand() * (hi - lo);
            }
        }
    }
    // The held item's walk upper body (`m_WalkUpperBodyDesire`, set by the music box's equip and
    // the combat tune's end) loops on the upper-body slot whenever no other upper-body action
    // plays there (LIKELY: its tree node and its stop were not traced, U-891).
    if let Some((anim, g, s)) = st.walk_upper.filter(|_| p.alive && !down && !thrown && p.carried_by.is_none()) {
        let other = st.upper.as_ref().is_some_and(|c| c.key != "walk_upper" && !c.fading);
        if !other {
            if let Some(mut c) = bank.chain_steps(profile, anim, g, &[s], i.armed, "walk_upper") {
                for x in c.steps.iter_mut() {
                    x.1 = true;
                }
                c.sustain = Sustain::Held;
                let same = st.upper.as_ref().is_some_and(|u| u.key == "walk_upper" && !u.fading && u.steps.first().map(|x| &x.0) == c.steps.first().map(|x| &x.0));
                if !same {
                    st.upper = Some(c);
                }
            }
        }
    }
    // Talking (U-142, resolved): the original has no body gestures for a spoken line. Blurbs
    // (`DisConv_Blurb`) carry no animation, the blocking-dialog behaviour only turns the NPC to
    // face (`DisBehaviorBlockingDialog`: face-to and body-stance requests), no
    // `eDisNPCAnimations` entry is a talk or gesture chain, and the FaceFX actors drive face
    // and head bones only (every Boyle actor's bone poses are on `head_jnt` and below). Body
    // gestures come from soiree matinees, whose AI-group anim tracks (`Gesture`,
    // `ScriptedSequence`, ...) play through `scripted` below.
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
    // The node's polar velocity follows the pawn's (0x0090BC00).
    let target = {
        let v = p.velocity.truncate();
        let moving = p.alive && p.mode == dishonored_sim::state::MoveMode::Walking && !p.incapacitated();
        let fwd = dishonored_sim::movement::forward_vector(p.yaw).truncate();
        let right = dishonored_sim::movement::right_vector(p.yaw).truncate();
        if moving && v.length_squared() > 1e-4 { (v.dot(right).atan2(v.dot(fwd)).to_degrees(), v.length()) } else { (st.vel.0, 0.0) }
    };
    st.vel = if st.vel_init { smooth_velocity(st.vel, target, i.dt, &bank.loco_node) } else { target };
    st.vel_init = true;
    let mut phase = st.phase;
    let (mut pose, master, half) = loco_pose(bank, lib, model, profile, &st.modifier.clone(), st.vel, i.dt, &mut phase, true).unwrap_or_else(|| (parts.iter().map(|p| vec![None; p.bones.len()]).collect(), None, 0.0));
    st.loco_master = master;
    // Feet touching down this frame (footfalls of the NPC locomotion; its loops carry no
    // footfall notifies).
    let feet = bank.profiles[profile].loco.as_ref().map(|c| c.feet.clone()).unwrap_or_default();
    let on = if p.alive && !p.incapacitated() && st.full.as_ref().is_none_or(|c| c.weight < 0.5) { feet_on_ground(&feet, phase, half) } else { [false; 2] };
    st.feet_down = [on[0] && !st.feet[0], on[1] && !st.feet[1]];
    st.feet = on;
    if let Some((pm, w)) = st.prev_modifier.take() {
        let mut ph = phase;
        if let Some((prev, _, _)) = loco_pose(bank, lib, model, profile, &pm, st.vel, i.dt, &mut ph, false) {
            let mut base = prev;
            overlay(&mut base, &pose, 1.0 - w, None, parts);
            pose = base;
        }
        // Modifier slot crossfade (+0x27C of the node, 0x00916770).
        let w = w - i.dt / bank.loco_node.modifier_blend.max(0.01);
        if w > 0.0 {
            st.prev_modifier = Some((pm, w));
        }
    }
    st.phase = phase;
    // A body on the ground hands over to the ragdoll at the death sequence's
    // `DishonoredNotify_Ragdoll` (or its end without one), from the pose at exactly that time.
    // A thrown pawn goes to the ragdoll the same way, at its throw sequence's notify.
    let lying = (down || thrown) && p.carried_by.is_none() && !p.no_body;
    if !lying {
        st.ragdoll = None;
    } else if st.ragdoll.is_none() {
        if let Some(phys) = bank.profiles[profile].physics.clone() {
            let start: Option<(Option<String>, f32, f32, f32)> = match st.full.as_ref() {
                Some(c) => {
                    let name = c.steps[c.i].0.clone();
                    let last = c.i + 1 >= c.steps.len();
                    let dur = bank.duration(profile, &name).unwrap_or(0.0);
                    if st.dropped {
                        Some((Some(name), c.t, 0.0, c.weight))
                    } else if !last || !c.hold {
                        None
                    } else if let Some(rt) = bank.ragdoll_time(profile, &name).filter(|t| *t < dur) {
                        (c.t >= rt).then(|| (Some(name), rt, c.t - rt, c.weight))
                    } else {
                        (c.t >= dur - 2e-3).then(|| (Some(name), (dur - 1e-3).max(0.0), 0.0, c.weight))
                    }
                }
                None => Some((None, 0.0, 0.0, 0.0)),
            };
            if let Some((name, t, behind, w)) = start {
                let at = |bank: &mut AnimBank, t: f32| -> Pose {
                    let mut ps = pose.clone();
                    if let Some(n) = name.as_ref() {
                        if let Some(fp) = bank.sample(lib, model, profile, n, t.max(0.0), false) {
                            overlay(&mut ps, &fp, w.max(0.999), None, parts);
                        }
                    }
                    ps
                };
                let p_now = at(bank, t);
                let p_prev = at(bank, t - crate::ragdoll::STEP);
                let g0 = globals(&parts[0], &p_now[0]);
                let g1 = globals(&parts[0], &p_prev[0]);
                let walls = i.walls.map(|w| w()).unwrap_or_default();
                let bind = globals(&parts[0], &vec![None; parts[0].bones.len()]);
                if let Some(mut rd) = crate::ragdoll::Ragdoll::new(&phys, &parts[0].bones, &bind, &g0, &g1, i.ground.unwrap_or(&|_, _| i.floor), &walls, i.gravity) {
                    if st.settle {
                        rd.settle();
                    } else {
                        rd.advance(behind);
                    }
                    st.ragdoll = Some(rd);
                }
                st.dropped = false;
                st.settle = false;
            }
        }
    } else if let Some(rd) = st.ragdoll.as_mut() {
        rd.advance(i.dt);
    }
    if let Some(rd) = st.ragdoll.as_ref() {
        st.loco_master = None;
        return rd.pose(parts);
    }
    if let Some(c) = st.full.as_ref() {
        let (name, _) = &c.steps[c.i];
        if let Some(mut fp) = bank.sample(lib, model, profile, name, c.t, false) {
            if matches!(c.key, "start" | "stop") {
                // Root motion stays with the simulation.
                for part in fp.iter_mut() {
                    if let Some(Some(k)) = part.first_mut() {
                        k.t = None;
                    }
                }
            }
            overlay(&mut pose, &fp, c.weight, None, parts);
        }
    }
    if let Some(c) = st.upper.as_ref() {
        let (name, _) = &c.steps[c.i];
        if let Some(up) = bank.sample(lib, model, profile, name, c.t, false) {
            let mask = bank.mask(lib, model);
            // The walk upper body gives way to full-body actions (the bash, the combat tune).
            let w = if c.key == "walk_upper" { c.weight * (1.0 - st.full.as_ref().map_or(0.0, |f| f.weight)) } else { c.weight };
            overlay(&mut pose, &up, w, Some(&mask), parts);
        }
    }
    let lookat = bank.profiles[profile].lookat.clone();
    // `BlinkBlender`: the "Blink" sequence (eyelids only) played every
    // `[MinDurationBetweenBlink, MaxDurationBetweenBlink]` seconds at a rate in
    // `[MinBlinkSpeed, MaxBlinkSpeed]` (`ArkComponentLookatConfig` class defaults 0.5-6 s, 1-2).
    if let Some(lc) = lookat.as_ref().filter(|l| l.blink && p.alive && !down) {
        if let Some(dur) = bank.duration(profile, "Blink") {
            match st.blink {
                Some((t, rate)) => {
                    let t = t + i.dt * rate;
                    if t >= dur {
                        st.blink = None;
                        st.blink_timer = lc.blink_interval[0] + st.rand() * (lc.blink_interval[1] - lc.blink_interval[0]);
                    } else {
                        st.blink = Some((t, rate));
                    }
                }
                None => {
                    st.blink_timer -= i.dt;
                    if st.blink_timer <= 0.0 {
                        let rate = lc.blink_rate[0] + st.rand() * (lc.blink_rate[1] - lc.blink_rate[0]);
                        st.blink = Some((0.0, rate));
                    }
                }
            }
            if let Some((t, _)) = st.blink {
                if let Some(bp) = bank.sample(lib, model, profile, "Blink", t, false) {
                    overlay(&mut pose, &bp, 1.0, None, parts);
                }
            }
        }
    }
    // The full-body step's face sequence through `FaceSlot` (`FaceFilter`: below `head_jnt`).
    if let Some(c) = st.full.as_ref() {
        if let Some(face) = c.faces.get(c.i).filter(|f| !f.is_empty()).cloned() {
            if let Some(fp) = bank.sample(lib, model, profile, &face, c.t, false) {
                let mask = bank.face_mask(lib, model);
                overlay(&mut pose, &fp, c.weight, Some(&mask), parts);
            }
        }
    }
    // `LookatBlender`: additive head and torso grids of the look-at mode for the stance and
    // gait, weighted by the target's angles against the mode's ranges.
    let want = match (i.look_target, lookat.as_ref()) {
        (Some(tg), Some(_)) if p.alive && !down => {
            let d = tg - p.position;
            let f = dishonored_sim::movement::forward_vector(p.yaw).truncate();
            let rt = dishonored_sim::movement::right_vector(p.yaw).truncate();
            let h = d.truncate();
            Some((h.dot(rt).atan2(h.dot(f)).to_degrees(), d.z.atan2(h.length().max(1.0)).to_degrees()))
        }
        _ => None,
    };
    {
        // Angles and weight ease toward the target (`m_fLookatSpeed` 0.5 of the locomotion
        // config: about half the way in 0.25 s here, APPROXIMATION U-281).
        let k = 1.0 - (-i.dt * 2.8).exp();
        let (ty, tp, tw) = want.map(|(y, p)| (y, p, 1.0)).unwrap_or((st.look.0, st.look.1, 0.0));
        st.look = (st.look.0 + (ty - st.look.0) * k, st.look.1 + (tp - st.look.1) * k, st.look.2 + (tw - st.look.2) * k);
    }
    // The AI's look-at (`MatchView::npc_heads`): the vision head's cursor split into the torso
    // and head parts as `ArkAnimNodeLookAt` (0x009176F0) poses them, so the visible head turns
    // where the vision cone points. Without it (a line spoken), the eased approximation above
    // gives both grids the same angles (U-281).
    let (aims, look_w) = match i.head.filter(|_| p.alive && !down) {
        Some(h) => ([(h.torso[0].to_degrees(), h.torso[1].to_degrees()), (h.head[0].to_degrees(), h.head[1].to_degrees())], 1.0),
        None => ([(st.look.0, st.look.1); 2], st.look.2),
    };
    if let Some(lc) = lookat.as_ref().filter(|_| look_w > 0.01) {
        let stance = if st.modifier.starts_with("Search") { "Search" } else if st.modifier.starts_with("Sword") || st.modifier == "Aim" { "Sword" } else { "Empty" };
        let gait = if st.vel.1 > 250.0 { "Run" } else if st.vel.1 > 20.0 { "Walk" } else { "Idle" };
        let name = format!("{stance}{gait}");
        if let Some(mode) = lc.modes.iter().find(|m| m.name.eq_ignore_ascii_case(&name)).or_else(|| lc.modes.first()) {
            for (gi, (grid, range)) in [(&mode.torso, mode.torso_range), (&mode.head, mode.head_range)].into_iter().enumerate() {
                let (ax, ay) = aims[gi];
                let x = if ax >= 0.0 { ax / range[0].max(1.0) } else { ax / range[1].min(-1.0) * -1.0 };
                let y = if ay >= 0.0 { ay / range[2].max(1.0) } else { ay / range[3].min(-1.0) * -1.0 };
                let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));
                // Grid order LU, LC, LD, CU, CC, CD, RU, RC, RD: columns left / centre / right,
                // rows up / centre / down; bilinear between the four around (x, y).
                let col = |c: usize| match c {
                    0 => (-x).max(0.0),
                    1 => 1.0 - x.abs(),
                    _ => x.max(0.0),
                };
                let row = |r: usize| match r {
                    0 => y.max(0.0),
                    1 => 1.0 - y.abs(),
                    _ => (-y).max(0.0),
                };
                let mut acc: Option<Pose> = None;
                for c in 0..3 {
                    for r in 0..3 {
                        let w = col(c) * row(r);
                        let n = &grid[c * 3 + r];
                        if w < 1e-3 || n.is_empty() {
                            continue;
                        }
                        if let Some(ap) = bank.sample(lib, model, profile, n, 0.0, false) {
                            accumulate_additive(&mut acc, &ap, w);
                        }
                    }
                }
                if let Some(add) = acc {
                    apply_additive(&mut pose, &add, look_w, parts);
                }
            }
        }
    }
    // [interp] Matinee anim tracks (the `MatineeStackNode` slot channels), in track order.
    for s in st.scripted.clone() {
        let prof = bank.with_sets(profile, &s.sets);
        if let Some(mut sp) = bank.sample(lib, model, prof, &s.name, s.time, false) {
            if s.root_motion {
                for part in sp.iter_mut() {
                    if let Some(Some(k)) = part.first_mut() {
                        k.t = None;
                    }
                }
            }
            overlay(&mut pose, &sp, s.weight, None, parts);
        }
    }
    // FaceFX (§12): the spoken line's bones, added after the AnimTree (UE3 `UpdateFaceFX`,
    // 0x00725EA0, runs on the tree's output; additive blend mode).
    for &(pi, bi, dp, dq) in i.face {
        let Some(bone) = parts.get(pi).and_then(|p| p.bones.get(bi)) else { continue };
        let Some(slot) = pose.get_mut(pi).and_then(|p| p.get_mut(bi)) else { continue };
        let k = slot.unwrap_or(Key { q: bone.rotation, t: None });
        let t = if dp.length_squared() > 1e-10 { Some(k.t.unwrap_or(bone.translation) + dp) } else { k.t };
        *slot = Some(Key { q: (dq * k.q).normalize(), t });
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
    /// The key extracts root motion (`m_MatineeRootMotionMode`): the server moves the pawn by
    /// the root bone's translation, so the mesh keeps the root at its bind translation.
    pub root_motion: bool,
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
        // The matinee's sets extend the pawn's profile; look-at and physics stay the pawn's.
        let (lookat, physics, prefix, limbs) = (self.profiles[profile].lookat.clone(), self.profiles[profile].physics.clone(), self.profiles[profile].prefix.clone(), self.profiles[profile].limbs.clone());
        self.profiles.push(Profile { sets, loco, defs: None, lookup: HashMap::new(), lookat, physics, prefix, limbs });
        let id = self.profiles.len() - 1;
        self.derived.insert(key, id);
        id
    }

    /// [interp] A profile made of AnimSets only (matinee props such as Samuel's boat).
    pub fn sets_profile(&mut self, sets: &[(String, String)]) -> usize {
        self.profile(&AnimProfile { sets: sets.to_vec(), ..Default::default() })
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
                            // Held at the end, not looped (the caller clamps to the duration).
                            let (q, tr) = clip.sample_held(t.min(clip.duration), j);
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

/// Weighted sum of additive deltas (rotations scaled from identity, then combined).
fn accumulate_additive(acc: &mut Option<Pose>, add: &Pose, w: f32) {
    let scaled: Pose = add.iter().map(|part| part.iter().map(|k| k.map(|k| Key { q: Quat::IDENTITY.slerp(k.q, w), t: None })).collect()).collect();
    match acc {
        None => *acc = Some(scaled),
        Some(a) => {
            for (pa, ps) in a.iter_mut().zip(&scaled) {
                for (ka, ks) in pa.iter_mut().zip(ps) {
                    *ka = match (*ka, *ks) {
                        (Some(x), Some(y)) => Some(Key { q: (y.q * x.q).normalize(), t: None }),
                        (x, y) => x.or(y),
                    };
                }
            }
        }
    }
}

/// `AnimNodeAdditiveBlending`: each animated bone's local rotation becomes `delta * base`
/// (deltas cooked as target * inverse(reference)), the delta scaled by `w`.
fn apply_additive(base: &mut Pose, add: &Pose, w: f32, bind: &[PartGpu]) {
    for (pi, (bp, ap)) in base.iter_mut().zip(add).enumerate() {
        for (bi, (b, a)) in bp.iter_mut().zip(ap).enumerate() {
            let Some(a) = a else { continue };
            let cur = b.unwrap_or(Key { q: bind[pi].bones[bi].rotation, t: None });
            let d = Quat::IDENTITY.slerp(a.q, w.clamp(0.0, 1.0));
            *b = Some(Key { q: (d * cur.q).normalize(), t: cur.t });
        }
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
