//! Third-person character animation drawn from the simulation's state
//! (research/dishonored/ANIMATION.md; research/ARCHITECTURE.md, step 7: the runtime is a skinner
//! and renderer, it picks no clip):
//! - the simulation steps every NPC's AnimTree (`dishonored_sim::anim`: `NPC_Human_at`, then
//!   `NPC_Dead_at` when it dies) from its states' requests, its physics, velocity and stance; the
//!   renderer poses that [`TreeState`] with `dishonored_anim` over each part of the pawn's model
//!   ([`pose_tree`]: the tree's shape built for the part's skeleton, the pawn tweak's AnimSets
//!   bound to it as the evaluator's clips, [`PartClips`]);
//! - each pawn tweak binds its own AnimSets; a sequence is found from the last set to the first
//!   (later sets override, e.g. the aristocrat walk), so the sequences the simulation names (the
//!   one NPC profile it plays, U-978) resolve in the pawn's own sets;
//! - the presentation's own inputs on a copy of the state (`TreeState::present`): the AI's
//!   look-at aims (`MatchView::npc_heads`) and the matinee anim tracks (the `MatineeStackNode`
//!   channels, [`Scripted`]); the FaceFX bones of a spoken line go on the tree's output;
//! - a pawn without a tree (a human player's body: the simulation steps trees for NPCs only)
//!   stands in `NPC_Human_at` as cooked (its locomotion idle), like an NPC without a request;
//! - a body on the ground hands over to the presentation ragdoll at its sequence's
//!   `DishonoredNotify_Ragdoll` (the simulation's event), at the end of a sequence without one, or
//!   at once when nothing plays.
//!
//! Nothing here feeds back into the simulation, and nothing advances an animation clock.

use crate::characters::{CharacterLibrary, PartGpu};
use bevy::prelude::*;
use dishonored_anim::tree::{LookInput, StackChannel, TreeShape, TreeState};
use dishonored_anim::{BakedClip, ClipRef, Clips, SetFlags};
use dishonored_assets::Content;
use dishonored_ir::anim_tree::AnimTreeAsset;
use dishonored_ir::npc_anim::*;
use std::collections::HashMap;
use std::sync::Arc;
use ue3_package::ObjRef;

/// One AnimSet of the install: its track names, sequence exports and translation flags.
pub struct SetData {
    pub pkg: String,
    pub tracks: Vec<String>,
    /// lowercase sequence name -> (export, original name)
    pub seqs: HashMap<String, (usize, String)>,
    /// `bAnimRotationOnly`, `UseTranslationBoneNames`, `ForceMeshTranslationBoneNames`.
    pub flags: SetFlags,
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
    /// The pawn tweak's `ArkComponentLookatConfig` (look-at grids).
    pub lookat: Option<Arc<LookatConfig>>,
    /// The pawn tweak's `DisSeveredLimbInfo`.
    pub limbs: Option<Arc<SeveredLimbInfo>>,
    /// Item prefixes of item-prefixed steps: [primary item held, empty hands]
    /// (`AnimProfile::prefix`).
    pub prefix: [String; 2],
}

/// A set's linkup to one part of a model: per Edge joint the bone it drives and whether its
/// translation is used (`dishonored_anim::bind_tracks`).
type PartLinkup = (Vec<Option<usize>>, Vec<bool>);

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
    /// (model, set) -> per part: the set's linkup.
    bindings: HashMap<(usize, usize), Arc<Vec<PartLinkup>>>,
    /// lowercase object path -> package (AnimSets, locomotion configs) for built-in profiles.
    locator: HashMap<String, String>,
    /// [interp] (profile, extra AnimSets) -> profile with the matinee group's sets appended.
    derived: HashMap<(usize, Vec<(String, String)>), usize>,
    /// `Npc_Common_at.NPC_Human_at` and `NPC_Dead_at` (Startup), the trees the simulation steps.
    trees: [Option<AnimTreeAsset>; 2],
    /// (model, part, dead) -> the tree's shape over the part's skeleton.
    shapes: HashMap<(usize, usize, bool), Option<Arc<TreeShape>>>,
    /// profile -> sequence name as the trees name it -> its set and clip (AnimBank::prepare).
    resolved: HashMap<usize, HashMap<String, Option<(usize, Arc<BakedClip>)>>>,
    /// FaceFX actors and per-blurb animations (lip sync).
    pub facefx: crate::facefx::FaceFxBank,
}

/// Packages that cook the Boyle NPC AnimSets, locomotion configs and animation definitions.
const SCRIPT_PACKAGES: &[&str] = &["L_Boyle_Ext_Script", "L_Boyle_Int_Script", "L_BoyleStreet_Ext_Script"];

use dishonored_assets::characters::{ARISTO_SETS, FEMALE_LOCO, FEMALE_SETS, GUARD_LOCO, GUARD_SETS};

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
        // The trees of `DisTweaks_Pawn.m_pAnimTreeTemplate` / `m_pAnimTreeWhenDeadTemplate`
        // (every pawn tweak's, ANIMATION.md section 1), as the simulation loads them.
        let tree = |path: &str| dishonored_assets::anim_tree::load_anim_tree(&content, "Startup", path).map_err(|e| warn!("anim tree {path}: {e:#}")).ok();
        let trees = [tree("Npc_Common_at.NPC_Human_at"), tree("Npc_Common_at.NPC_Dead_at")];
        AnimBank { content, sets: Vec::new(), set_index: HashMap::new(), clips: HashMap::new(), profiles: Vec::new(), profile_index: HashMap::new(), model_profile: HashMap::new(), bindings: HashMap::new(), locator, derived: HashMap::new(), trees, shapes: HashMap::new(), resolved: HashMap::new(), facefx: Default::default() }
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
        let names = |n: &str| -> Vec<String> {
            let Some((count, raw)) = find(&h.properties, n).and_then(|p| p.array_raw()) else { return Vec::new() };
            let mut rd = ue3_package::reader::Reader::new(raw);
            (0..count.min(4096)).filter_map(|_| ue3_package::FName::read(&mut rd).ok().map(|f| pkg.name(f))).collect()
        };
        let tracks = names("TrackBoneNames");
        if tracks.is_empty() {
            return None;
        }
        // The set's translation flags (`FAnimSetMeshLinkup::BuildLinkup`; none of the Boyle NPC
        // sets cooks one, `dishonored_anim::clip`).
        let flags = SetFlags { rotation_only: matches!(find(&h.properties, "bAnimRotationOnly").map(|p| &p.value), Some(Value::Bool(true))), use_translation: names("UseTranslationBoneNames"), force_mesh_translation: names("ForceMeshTranslationBoneNames") };
        let mut seqs = HashMap::new();
        for s in find(&h.properties, "Sequences").and_then(|p| p.array_objects().ok()).unwrap_or_default() {
            let ObjRef::Export(x) = ObjRef::from_index(s) else { continue };
            let Ok(sh) = pkg.read_object(x) else { continue };
            if let Some(Value::Name(name)) = find(&sh.properties, "SequenceName").map(|p| &p.value) {
                seqs.insert(name.to_ascii_lowercase(), (x, name.clone()));
            }
        }
        self.sets.push(SetData { pkg: r.0.clone(), tracks, seqs, flags });
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

    /// A set's linkup to every part of a model. Edge joints follow the set's tracks in the bone
    /// order of the skeleton it was compressed against: the profile's locomotion reference mesh
    /// when it is loaded, else the model's own body (`dishonored_anim::bind_tracks` with the set's
    /// translation flags against each part's skeleton).
    fn binding(&mut self, lib: &CharacterLibrary, model: usize, profile: usize, set: usize) -> Arc<Vec<PartLinkup>> {
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
        let b: Vec<PartLinkup> = m.parts.iter().map(|p| dishonored_anim::bind_tracks(&tracks, &self.sets[set].flags, &p.bones)).collect();
        let b = Arc::new(b);
        self.bindings.insert((model, set), b.clone());
        b
    }

    pub fn duration(&mut self, profile: usize, name: &str) -> Option<f32> {
        self.clip(profile, name).map(|c| c.1.duration)
    }

    /// The simulation's tree (`NPC_Dead_at` when `dead`) shaped over one part of a model: the
    /// graph and tick order of the cooked tree with the part's per-bone masks and reference pose
    /// (the mesh rotation is the root motion's, which the renderer does not read).
    pub fn tree_shape(&mut self, lib: &CharacterLibrary, model: usize, part: usize, dead: bool) -> Option<Arc<TreeShape>> {
        if let Some(s) = self.shapes.get(&(model, part, dead)) {
            return s.clone();
        }
        let s = match (&self.trees[dead as usize], lib.models.get(model).and_then(|m| m.parts.get(part))) {
            (Some(t), Some(p)) => Some(Arc::new(TreeShape::new(t, &p.bones, Quat::IDENTITY))),
            _ => None,
        };
        self.shapes.insert((model, part, dead), s.clone());
        s
    }

    /// Decode the named sequences of a profile (once per name) and bind their sets to a model:
    /// what [`AnimBank::clips`] resolves.
    pub fn prepare(&mut self, lib: &CharacterLibrary, model: usize, profile: usize, names: &[&str]) {
        for n in names {
            let known = self.resolved.get(&profile).and_then(|m| m.get(*n)).cloned();
            let found = match known {
                Some(f) => f,
                None => {
                    let f = self.clip(profile, n);
                    self.resolved.entry(profile).or_default().insert(n.to_string(), f.clone());
                    f
                }
            };
            if let Some((set, _)) = found {
                if !self.bindings.contains_key(&(model, set)) {
                    self.binding(lib, model, profile, set);
                }
            }
        }
    }

    /// A profile's sets bound to one part of a model as the evaluator's clips (the sequences
    /// [`AnimBank::prepare`] decoded).
    pub fn clips(&self, model: usize, part: usize, profile: usize) -> PartClips<'_> {
        PartClips { bank: self, model, part, profile }
    }
}

/// `dishonored_anim::Clips` over the bank's decoded sets: a sequence is found from the profile's
/// last set to its first (`FindAnimSequence`), the set's linkup to the part's skeleton.
pub struct PartClips<'a> {
    bank: &'a AnimBank,
    model: usize,
    part: usize,
    profile: usize,
}

impl Clips for PartClips<'_> {
    fn find(&self, name: &str) -> Option<ClipRef<'_>> {
        if name.is_empty() || name.eq_ignore_ascii_case("None") {
            return None;
        }
        let b = self.bank;
        let (set, clip) = b.resolved.get(&self.profile)?.get(name)?.as_ref()?;
        let (bones, translate) = b.bindings.get(&(self.model, *set))?.get(self.part)?;
        Some(ClipRef { clip, bones, translate })
    }
}

/// The pose of a pawn's tree state (the simulation's, `NPC_Dead_at`'s when `dead`) on a model:
/// per part, every bone's local transform. The look-at aims and the matinee's channels are set on
/// a copy (`TreeState::present`); the channels' sequences are found in the matinee group's sets
/// (`stack_sets`, searched before the pawn's). `None` without the install's trees.
#[allow(clippy::too_many_arguments)]
pub fn pose_tree(bank: &mut AnimBank, lib: &CharacterLibrary, model: usize, profile: usize, state: &TreeState, dead: bool, look: Option<&LookInput>, stacks: &[StackChannel], stack_sets: &[(String, String)]) -> Option<Vec<dishonored_anim::Pose>> {
    let shape = bank.tree_shape(lib, model, 0, dead)?;
    if !shape.fits(state) {
        return None;
    }
    let mut st = state.clone();
    st.present(&shape, look, stacks);
    let profile = bank.with_sets(profile, stack_sets);
    bank.prepare(lib, model, profile, &st.sequence_names());
    let parts = lib.models[model].parts.len();
    let shapes: Vec<Option<Arc<TreeShape>>> = (0..parts).map(|pi| bank.tree_shape(lib, model, pi, dead)).collect();
    Some(shapes.iter().enumerate().map(|(pi, s)| s.as_ref().map_or_else(|| dishonored_anim::pose::bind_pose(&lib.models[model].parts[pi].bones), |s| st.pose(s, &bank.clips(model, pi, profile)))).collect())
}

/// The look-at mode (`ArkComponentLookatConfig.m_LookAtModes`) for the locomotion's modifier and
/// smoothed speed: stance `Search` / `Sword` / `Empty` and gait `Idle` / `Walk` / `Run` (the
/// mode names; above 250 uu/s runs, above 20 walks), else the config's first.
pub fn lookat_mode<'a>(cfg: &'a LookatConfig, modifier: &str, speed: f32) -> Option<&'a LookatMode> {
    let stance = if modifier.starts_with("Search") { "Search" } else if modifier.starts_with("Sword") || modifier == "Aim" { "Sword" } else { "Empty" };
    let gait = if speed > 250.0 { "Run" } else if speed > 20.0 { "Walk" } else { "Idle" };
    let name = format!("{stance}{gait}");
    cfg.modes.iter().find(|m| m.name.eq_ignore_ascii_case(&name)).or_else(|| cfg.modes.first())
}

/// The matinee's anim tracks on a pawn as the `MatineeStackNode` channels (0-5, in track order).
pub fn matinee_channels(scripted: &[Scripted]) -> Vec<StackChannel> {
    scripted.iter().take(6).enumerate().map(|(k, s)| StackChannel { stack: "MatineeStackNode".into(), channel: k, sequence: s.name.clone(), time: s.time, weight: s.weight }).collect()
}

/// Pawn-space bone globals of a part from its local pose.
pub fn local_globals(part: &PartGpu, pose: &[(Quat, Vec3)]) -> Vec<glam::Mat4> {
    let mut bones = part.bones.clone();
    for (b, (q, t)) in bones.iter_mut().zip(pose) {
        b.rotation = *q;
        b.translation = *t;
    }
    dishonored_assets::characters::bind_globals(&bones).iter().map(|g| part.mesh_to_pawn * *g).collect()
}

/// A drawn pawn's presentation state: the matinee's tracks on it, its presentation ragdoll and
/// the tree outputs the renderer reacts to. It holds no animation clock: the simulation steps the
/// pawn's animation (`dishonored_sim::anim`); the ragdoll steps by the simulation's ticks.
#[derive(Component, Default)]
pub struct PawnAnim {
    /// [interp] Matinee anim track samples on this pawn (set each frame by `cutscene.rs`).
    pub scripted: Vec<Scripted>,
    /// Presentation ragdoll of a body on the ground.
    pub ragdoll: Option<crate::ragdoll::Ragdoll>,
    /// Run the next ragdoll to rest at once (the pawn was first seen down).
    pub settle: bool,
    /// A hand-over the simulation's notifies asked for (`DishonoredNotify_Ragdoll`, a sever with
    /// `m_bGoToRagdoll`), not started yet.
    pub to_ragdoll: bool,
    /// The locomotion's planted feet last drawn (left, right), for the touch-downs.
    pub feet: [bool; 2],
    /// The simulation tick of the last frame drawn (the ragdoll steps by the ticks since).
    pub tick: u64,
    /// Part 0's pawn-space globals of the last two ticks drawn (tick, globals): the ragdoll's
    /// step before a hand-over.
    pub history: [Option<(u64, Vec<glam::Mat4>)>; 2],
    pub init: bool,
}

impl PawnAnim {
    /// Keep the globals drawn at `tick` (the latest of a tick replaces the earlier frames').
    pub fn remember(&mut self, tick: u64, globals: &[glam::Mat4]) {
        if self.history[1].as_ref().is_some_and(|h| h.0 != tick) {
            self.history[0] = self.history[1].take();
        }
        self.history[1] = Some((tick, globals.to_vec()));
    }

    /// The globals drawn at `tick`, if a frame drew that tick.
    pub fn at(&self, tick: u64) -> Option<&[glam::Mat4]> {
        self.history.iter().flatten().find(|h| h.0 == tick).map(|h| h.1.as_slice())
    }
}

/// One animated bone of a matinee prop or level prop: local rotation and, where the clip drives
/// it, translation.
#[derive(Clone, Copy, Debug)]
pub struct Key {
    pub q: Quat,
    pub t: Option<Vec3>,
}

/// A prop's pose: per part, per bone (None = not set by this layer).
pub type Pose = Vec<Vec<Option<Key>>>;

/// `base = nlerp(base, over, w)` where `over` animates a bone.
fn overlay(base: &mut Pose, over: &Pose, w: f32, bind: &[PartGpu]) {
    if w <= 1e-4 {
        return;
    }
    for (pi, (bp, op)) in base.iter_mut().zip(over).enumerate() {
        for (bi, (b, o)) in bp.iter_mut().zip(op).enumerate() {
            let Some(o) = o else { continue };
            let k = w.min(1.0);
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

/// [interp] Blend a layer over a prop's pose.
pub fn blend_into(base: &mut Pose, over: &Pose, w: f32, parts: &[PartGpu]) {
    overlay(base, over, w, parts);
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

/// Pawn-space bone globals of a prop's part from its pose (bones the pose leaves keep their bind
/// transform).
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
