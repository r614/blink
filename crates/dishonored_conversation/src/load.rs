//! Reading dialog graphs from cooked level packages (feature `load`).
//!
//! Defaults: cooked packages omit properties whose value equals the class default object, so
//! missing values take the `Default__<Class>` values read from `DishonoredGame.upk`
//! (e.g. `DisConversation.m_bIsBlocking = true`, `m_Priority = HIGH_PRIORITY_CONV`,
//! `m_fPlayerNearRadius = 500`; `DisConv_Blurb.m_bMainSubtitle = true`,
//! `m_fVoiceRadius = 3000`, `m_SpokenTo = -1`; `DisConv_SetStoryFlag.m_bValue = true`;
//! `DisConv_TimeLimit.m_fTimeLimitInSeconds = 30`, `m_bApplyGlobally = true`;
//! `DisConv_RandomBranch.m_bTryKeepUnique = true`; `DisConv_Hook_Distance` 200/500/1000).

use crate::model::*;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use ue3_package::properties::{Property, Value, find};
use ue3_package::{FName, ObjRef, Package};

/// Story-flag GUID -> name, from every `DisStoryFlagSet` in `Startup.upk`.
#[derive(Debug, Clone, Default)]
pub struct StoryFlags {
    pub by_guid: HashMap<[u32; 4], (String, String)>,
}

impl StoryFlags {
    pub fn load(cooked: &Path) -> Result<Self> {
        let pkg = Package::open(cooked.join("Startup.upk"))?;
        let mut by_guid = HashMap::new();
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != "DisStoryFlagSet" {
                continue;
            }
            let set = pkg.obj_name(ObjRef::Export(i));
            let Ok(h) = pkg.read_object(i) else { continue };
            let Some(p) = find(&h.properties, "m_StoryFlags") else { continue };
            for f in p.array_structs(&pkg).unwrap_or_default() {
                let name = match find(&f, "m_Name").map(|p| &p.value) {
                    Some(Value::Name(n)) => n.clone(),
                    _ => continue,
                };
                if let Some(Value::Guid(g)) = find(&f, "m_GUID").map(|p| &p.value) {
                    by_guid.insert(*g, (set.clone(), name));
                }
            }
        }
        Ok(Self { by_guid })
    }

    pub fn name(&self, g: &[u32; 4]) -> String {
        self.by_guid.get(g).map(|x| x.1.clone()).unwrap_or_else(|| format!("SF_{}", guid_hex(g)))
    }
}

pub fn guid_hex(g: &[u32; 4]) -> String {
    format!("{:08X}{:08X}{:08X}{:08X}", g[0], g[1], g[2], g[3])
}

/// Length of a line's VO clip, if it resolves to decodable media (see [`crate::vo`]).
pub trait VoLengths {
    fn length(&mut self, vo: &VoRef) -> Option<f32>;
}

/// No VO lookups (lines are timed from their text).
pub struct NoVo;
impl VoLengths for NoVo {
    fn length(&mut self, _: &VoRef) -> Option<f32> {
        None
    }
}

fn prop<'a>(props: &'a [Property], n: &str) -> Option<&'a Value> {
    find(props, n).map(|p| &p.value)
}
fn pbool(props: &[Property], n: &str, d: bool) -> bool {
    match prop(props, n) {
        Some(Value::Bool(b)) => *b,
        _ => d,
    }
}
fn pfloat(props: &[Property], n: &str, d: f32) -> f32 {
    find(props, n).and_then(|p| p.as_float()).unwrap_or(d)
}
fn pint(props: &[Property], n: &str, d: i32) -> i32 {
    match prop(props, n) {
        Some(Value::Int(i)) => *i,
        _ => d,
    }
}
fn pstr(props: &[Property], n: &str) -> Option<String> {
    match prop(props, n) {
        Some(Value::Str(s)) => Some(s.clone()),
        Some(Value::Name(s)) if s != "None" => Some(s.clone()),
        Some(Value::Enum(s)) => Some(s.clone()),
        _ => None,
    }
}
fn pobj(props: &[Property], n: &str) -> Option<usize> {
    match prop(props, n) {
        Some(Value::Object(i)) => match ObjRef::from_index(*i) {
            ObjRef::Export(e) => Some(e),
            _ => None,
        },
        _ => None,
    }
}
fn pobj_name(pkg: &Package, props: &[Property], n: &str) -> Option<String> {
    match prop(props, n) {
        Some(Value::Object(i)) if *i != 0 => Some(pkg.obj_name(ObjRef::from_index(*i))),
        _ => None,
    }
}
fn pguid(props: &[Property], n: &str) -> Option<[u32; 4]> {
    match prop(props, n) {
        Some(Value::Guid(g)) => Some(*g),
        _ => None,
    }
}

/// `m_OutputLinks[i].m_pLink` export indices.
fn output_links(pkg: &Package, props: &[Property]) -> Vec<Option<usize>> {
    let Some(p) = find(props, "m_OutputLinks") else { return Vec::new() };
    p.array_structs(pkg).unwrap_or_default().iter().map(|e| pobj(e, "m_pLink")).collect()
}

/// Array of FNames.
fn name_array(pkg: &Package, props: &[Property], n: &str) -> Vec<String> {
    let Some((count, body)) = find(props, n).and_then(|p| p.array_raw()) else { return Vec::new() };
    if body.len() != count * 8 {
        return Vec::new();
    }
    body.chunks(8)
        .map(|c| {
            let index = i32::from_le_bytes(c[..4].try_into().unwrap());
            let number = i32::from_le_bytes(c[4..].try_into().unwrap());
            if index >= 0 && (index as usize) < pkg.names.len() { pkg.name(FName { index, number }) } else { String::new() }
        })
        .collect()
}

/// Array of floats (`DisConv_RandomBranch.m_Branches`).
fn float_array(props: &[Property], n: &str) -> Option<Vec<f32>> {
    let (count, body) = find(props, n).and_then(|p| p.array_raw())?;
    (body.len() == count * 4).then(|| body.chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect())
}

fn object_array(props: &[Property], n: &str) -> Vec<Option<usize>> {
    find(props, n)
        .and_then(|p| p.array_objects().ok())
        .unwrap_or_default()
        .into_iter()
        .map(|i| match ObjRef::from_index(i) {
            ObjRef::Export(e) => Some(e),
            _ => None,
        })
        .collect()
}

/// Incremental loader: add packages one by one, then [`Loader::finish`].
pub struct Loader {
    pub cooked: PathBuf,
    pub flags: StoryFlags,
    pub db: ConversationDb,
    /// Subtitle language (`DISHONORED_LANGUAGE`, default `INT`). The GOG install cooks INT
    /// only; other languages read `DisConv_Blurb.m_PerLanguageData`.
    pub language: String,
}

impl Loader {
    pub fn new(cooked: impl Into<PathBuf>) -> Result<Self> {
        let cooked = cooked.into();
        let flags = StoryFlags::load(&cooked).context("story flags (Startup.upk)")?;
        let language = std::env::var("DISHONORED_LANGUAGE").unwrap_or_else(|_| "INT".into());
        let mut db = ConversationDb::default();
        db.settings = load_settings(&cooked).context("Default__DisTweaks_ConvSettings (DishonoredGame.upk)")?;
        Ok(Self { cooked, flags, db, language })
    }

    /// Default install location (`DISHONORED_DIR` or the GOG path).
    pub fn locate() -> Result<Self> {
        let root = std::env::var_os("DISHONORED_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Program Files\GOG Galaxy\Games\Dishonored"));
        Self::new(root.join("DishonoredGame").join("CookedPCConsole"))
    }

    pub fn finish(self) -> ConversationDb {
        self.db
    }

    /// Read every dialog graph of one cooked package.
    pub fn add_package(&mut self, name: &str, vo: &mut dyn VoLengths) -> Result<()> {
        if self.db.packages.iter().any(|p| p.eq_ignore_ascii_case(name)) {
            return Ok(());
        }
        let pkg = Package::open(self.cooked.join(format!("{name}.upk")))?;
        let pname = pkg.name.clone();
        self.db.packages.push(pname.clone());
        // Classes of interest.
        let mut oneshots = Vec::new();
        let mut trees = Vec::new();
        let mut convs = Vec::new();
        let mut nodes = Vec::new();
        let mut voices = Vec::new();
        let mut interps = Vec::new();
        for i in 0..pkg.exports.len() {
            if pkg.exports[i].object_flags & ue3_package::properties::RF_CLASS_DEFAULT_OBJECT != 0 {
                continue;
            }
            let c = pkg.export_class(i);
            match c.as_str() {
                "DisDialogOneShot" => oneshots.push(i),
                "DisDialogTree" | "DisDialogTree_OneShot" => trees.push(i),
                "DisConversation" => convs.push(i),
                "DisDialogVoiceData" | "DisDialogVoiceData_OneShot" => voices.push(i),
                "SeqAct_Interp" => interps.push(i),
                c if c.starts_with("DisConv_") && c != "DisConv_Comment" => nodes.push(i),
                _ => {}
            }
        }
        let read = |i: usize| pkg.read_object(i).map(|h| h.properties).unwrap_or_default();
        // Voice tables: voice data path -> GUID -> VO, and a package-wide fallback.
        let mut voice_tables: HashMap<usize, HashMap<[u32; 4], VoRef>> = HashMap::new();
        let mut any_voice: HashMap<[u32; 4], VoRef> = HashMap::new();
        for &i in &voices {
            let props = read(i);
            let bank = pobj_name(&pkg, &props, "m_pAkBank").unwrap_or_default();
            let mut t = HashMap::new();
            for tree in find(&props, "m_VoiceData").and_then(|p| p.array_structs(&pkg).ok()).unwrap_or_default() {
                for b in find(&tree, "m_Blurbs").and_then(|p| p.array_structs(&pkg).ok()).unwrap_or_default() {
                    let (Some(g), Some(ev)) = (pguid(&b, "m_GUID"), pstr(&b, "m_AkEventName")) else { continue };
                    if ev.is_empty() || bank.is_empty() {
                        continue;
                    }
                    let v = VoRef { event: ev, bank: bank.clone() };
                    any_voice.entry(g).or_insert_with(|| v.clone());
                    t.insert(g, v);
                }
            }
            voice_tables.insert(i, t);
        }
        // Matinees by GUID (soiree links are recorded, not played).
        let mut matinees: HashMap<[u32; 4], String> = HashMap::new();
        for &i in &interps {
            let props = read(i);
            if let Some(g) = pguid(&props, "m_MatineeGUID") {
                matinees.insert(g, pkg.obj_path(ObjRef::Export(i)));
            }
        }
        // Id assignment.
        let tree_base = self.db.trees.len() as u32;
        let conv_base = self.db.conversations.len() as u32;
        let node_base = self.db.nodes.len() as u32;
        let os_base = self.db.oneshots.len() as u32;
        let tree_id: HashMap<usize, u32> = trees.iter().enumerate().map(|(k, &e)| (e, tree_base + k as u32)).collect();
        // Voice data -> its trees (generic bark trees of the pawns using that voice): its
        // `m_pDialogTree` and the trees of its `m_VoiceData` blurb lists, matched by path (the
        // reference may be an import of the tree's own package).
        let tree_by_path: HashMap<String, u32> = trees.iter().map(|&e| (pkg.obj_path(ObjRef::Export(e)).to_ascii_lowercase(), tree_id[&e])).collect();
        for &i in &voices {
            let props = read(i);
            let mut refs: Vec<i32> = find(&props, "m_pDialogTree").and_then(|p| p.as_object()).into_iter().collect();
            for t in find(&props, "m_VoiceData").and_then(|p| p.array_structs(&pkg).ok()).unwrap_or_default() {
                refs.extend(find(&t, "m_pTree").and_then(|p| p.as_object()));
            }
            let mut ids: Vec<u32> = Vec::new();
            for r in refs {
                let r = ObjRef::from_index(r);
                if r == ObjRef::Null {
                    continue;
                }
                if let Some(id) = tree_by_path.get(&pkg.obj_path(r).to_ascii_lowercase()) {
                    if !ids.contains(id) {
                        ids.push(*id);
                    }
                }
            }
            if !ids.is_empty() {
                self.db.voice_trees.insert(pkg.obj_path(ObjRef::Export(i)), ids);
            }
        }
        let conv_id: HashMap<usize, u32> = convs.iter().enumerate().map(|(k, &e)| (e, conv_base + k as u32)).collect();
        let node_id: HashMap<usize, u32> = nodes.iter().enumerate().map(|(k, &e)| (e, node_base + k as u32)).collect();
        let os_id: HashMap<usize, u32> = oneshots.iter().enumerate().map(|(k, &e)| (e, os_base + k as u32)).collect();
        // A link may point at a conversation (ConversationRef) or a node.
        let link = |e: Option<usize>| e.and_then(|e| node_id.get(&e).copied());

        // One-shots.
        let mut os_speaker_voice: HashMap<u32, Vec<Option<usize>>> = HashMap::new();
        for &i in &oneshots {
            let props = read(i);
            let speaker = |s: &[Property]| OneShotSpeaker {
                nick: pstr(s, "m_SpeakerNickName").unwrap_or_default(),
                actor: pobj_name(&pkg, s, "m_pAttachedActor"),
                voice: pobj(s, "m_pVoiceData").map(|v| pkg.obj_path(ObjRef::Export(v))),
            };
            let mut speakers = Vec::new();
            let mut vids = Vec::new();
            if let Some(Value::Struct(_, s)) = prop(&props, "m_MeSpeaker") {
                speakers.push(speaker(s));
                vids.push(pobj(s, "m_pVoiceData"));
            } else {
                speakers.push(OneShotSpeaker::default());
                vids.push(None);
            }
            for s in find(&props, "m_OtherSpeakers").and_then(|p| p.array_structs(&pkg).ok()).unwrap_or_default() {
                speakers.push(speaker(&s));
                vids.push(pobj(&s, "m_pVoiceData"));
            }
            let id = os_id[&i];
            os_speaker_voice.insert(id, vids);
            self.db.oneshots.push(OneShot {
                package: pname.clone(),
                name: pkg.obj_name(ObjRef::Export(i)),
                path: pkg.obj_path(ObjRef::Export(i)),
                tree: pobj(&props, "m_pDialogTree").and_then(|t| tree_id.get(&t).copied()),
                speakers,
                location: find(&props, "Location").and_then(|p| p.as_vector()).unwrap_or_default(),
            });
        }
        // Trees.
        let mut tree_oneshot: HashMap<u32, u32> = HashMap::new();
        for &i in &trees {
            let props = read(i);
            let id = tree_id[&i];
            let oneshot = pobj(&props, "m_pBoundToOneShot").and_then(|o| os_id.get(&o).copied()).or_else(|| {
                // Fallback: the one-shot whose m_pDialogTree is this tree.
                self.db.oneshots.iter().position(|o| o.tree == Some(id)).map(|p| p as u32)
            });
            if let Some(o) = oneshot {
                tree_oneshot.insert(id, o);
            }
            self.db.trees.push(Tree {
                package: pname.clone(),
                path: pkg.obj_path(ObjRef::Export(i)),
                oneshot,
                hooks: object_array(&props, "m_DialogHooks").into_iter().flatten().filter_map(|e| node_id.get(&e).copied()).collect(),
                conversations: object_array(&props, "m_Conversations").into_iter().flatten().filter_map(|e| conv_id.get(&e).copied()).collect(),
                // `m_pFallbackTree` (+0x148), an export of this package or a tree of a package
                // loaded before (matched by path).
                fallback: find(&props, "m_pFallbackTree").and_then(|p| p.as_object()).map(ObjRef::from_index).filter(|r| *r != ObjRef::Null).and_then(|r| {
                    let path = pkg.obj_path(r).to_ascii_lowercase();
                    tree_by_path.get(&path).copied().or_else(|| self.db.trees.iter().position(|t| t.path.eq_ignore_ascii_case(&path)).map(|p| p as u32))
                }),
            });
        }
        // Owner of an export: walk outers up to a conversation or tree.
        let owner_of = |mut e: usize| -> (Option<u32>, Option<u32>) {
            let mut conv = None;
            for _ in 0..16 {
                let o = pkg.exports[e].outer;
                let ObjRef::Export(oe) = ObjRef::from_index(o) else { break };
                if conv.is_none() {
                    if let Some(c) = conv_id.get(&oe) {
                        conv = Some(*c);
                    }
                }
                if let Some(t) = tree_id.get(&oe) {
                    return (conv, Some(*t));
                }
                e = oe;
            }
            (conv, None)
        };
        // Conversations.
        let mut conv_speakers: HashMap<u32, Vec<SpeakerSlot>> = HashMap::new();
        for &i in &convs {
            let props = read(i);
            let id = conv_id[&i];
            let (_, tree) = owner_of(i);
            let tree = pobj(&props, "m_pOwner").and_then(|t| tree_id.get(&t).copied()).or(tree).unwrap_or(tree_base);
            let mut speakers = Vec::new();
            let mut speaker_nicks = Vec::new();
            let mut speaker_gates = Vec::new();
            for s in find(&props, "m_Speakers").and_then(|p| p.array_structs(&pkg).ok()).unwrap_or_default() {
                let t = pstr(&s, "m_Type").unwrap_or_default();
                let linked = pobj(&s, "m_pLinkedNode").map(read);
                speakers.push(match t.as_str() {
                    "ME_SPEAKER" => SpeakerSlot::Me,
                    "INITIATOR_SPEAKER" => SpeakerSlot::Initiator,
                    "PLAYER_SPEAKER" => SpeakerSlot::Player,
                    "LINKED_NODE_SPEAKER" => SpeakerSlot::OneShot(linked.as_ref().map(|n| pint(n, "m_iOneShotSpeaker", -1)).unwrap_or(-1)),
                    other => SpeakerSlot::Other(other.to_string()),
                });
                speaker_nicks.push(linked.as_ref().and_then(|n| pstr(n, "m_NickName")).filter(|s| !s.is_empty()));
                speaker_gates.push(
                    linked
                        .as_ref()
                        .map(|n| SpeakerGate {
                            not_in_combat: pbool(n, "m_bNotWhileInCombat", false),
                            not_fearful: pbool(n, "m_bNotWhileFearful", false),
                            not_suspicious: pbool(n, "m_bNotWhileSuspicious", false),
                            optional: pbool(n, "m_bOptional", false),
                        })
                        .unwrap_or_default(),
                );
            }
            if speakers.is_empty() {
                speakers = vec![SpeakerSlot::Me, SpeakerSlot::Initiator, SpeakerSlot::Player];
                speaker_nicks = vec![None; 3];
                speaker_gates = vec![SpeakerGate::default(); 3];
            }
            conv_speakers.insert(id, speakers.clone());
            let outs = output_links(&pkg, &props);
            self.db.conversations.push(Conversation {
                path: pkg.obj_path(ObjRef::Export(i)),
                name: pkg.obj_name(ObjRef::Export(i)),
                tree,
                label: pstr(&props, "m_Label").unwrap_or_default(),
                description: pstr(&props, "m_Description").map(|s| s.trim().to_string()).unwrap_or_default(),
                speakers,
                speaker_nicks,
                speaker_gates,
                start: link(outs.first().copied().flatten()),
                interrupted: link(outs.get(1).copied().flatten()),
                walked_away: link(outs.get(2).copied().flatten()),
                blocking: pbool(&props, "m_bIsBlocking", true),
                blocking_player: pbool(&props, "m_bIsBlocking_Player", false),
                fire_once: pbool(&props, "m_bFireConvOnlyOnce", false),
                priority: Priority::from_enum(&pstr(&props, "m_Priority").unwrap_or_default()),
                player_must_be_near: pbool(&props, "m_bPlayerMustBeNear", false),
                player_near_radius: pfloat(&props, "m_fPlayerNearRadius", 500.0),
                not_when_player_in_combat: pbool(&props, "m_bNotWhenPlayerIsInCombat", false),
                always_interrupt_for_combat: pbool(&props, "m_bAlwaysInterruptForCombat", false),
            });
        }
        // Nodes.
        for &i in &nodes {
            let props = read(i);
            let class = pkg.export_class(i);
            let (conv, tree) = owner_of(i);
            let tree = tree.unwrap_or(tree_base);
            let outs: Vec<Option<NodeId>> = output_links(&pkg, &props).into_iter().map(link).collect();
            let speaker = pint(&props, "m_iSpeaker", if class == "DisConv_Blurb" || class == "DisConv_NonWord" { 0 } else { -1 });
            let flag = |props: &[Property]| pguid(props, "m_StoryFlag").map(|g| self.flags.name(&g)).unwrap_or_else(|| "not_set".into());
            let kind = match class.as_str() {
                "DisConv_Blurb" | "DisConv_NonWord" => {
                    let g = pguid(&props, "m_iBlurbGUID").unwrap_or_default();
                    // The speaker's own voice data first (one-shots), else any voice table.
                    let slot = conv.and_then(|c| conv_speakers.get(&c)).and_then(|s| s.get(speaker.max(0) as usize)).cloned();
                    let os = tree_oneshot.get(&tree).copied();
                    let voice_idx = match (slot, os) {
                        (Some(SpeakerSlot::Me), Some(o)) => os_speaker_voice.get(&o).and_then(|v| v.first().copied().flatten()),
                        (Some(SpeakerSlot::OneShot(k)), Some(o)) if k >= 0 => os_speaker_voice.get(&o).and_then(|v| v.get(k as usize + 1).copied().flatten()),
                        _ => None,
                    };
                    let voref = voice_idx.and_then(|v| voice_tables.get(&v)).and_then(|t| t.get(&g)).or_else(|| any_voice.get(&g)).cloned();
                    let vo_length = voref.as_ref().and_then(|v| vo.length(v));
                    NodeKind::Line(Line {
                        guid: guid_hex(&g),
                        // `FUN_00c92ce0`: language INT reads `m_Text`; any other language reads
                        // the single `m_PerLanguageData` entry its cooked packages carry.
                        text: if self.language.eq_ignore_ascii_case("INT") {
                            pstr(&props, "m_Text").unwrap_or_default()
                        } else {
                            match find(&props, "m_PerLanguageData").and_then(|p| p.array_structs(&pkg).ok()).as_deref() {
                                Some([one]) => pstr(one, "m_LocalizedText").unwrap_or_default(),
                                _ => pstr(&props, "m_Text").unwrap_or_default(),
                            }
                        },
                        nonword: class == "DisConv_NonWord",
                        spoken_to: pint(&props, "m_SpokenTo", -1),
                        main_subtitle: pbool(&props, "m_bMainSubtitle", class != "DisConv_NonWord"),
                        voice_radius: pfloat(&props, "m_fVoiceRadius", 3000.0),
                        end_delay: pfloat(&props, "m_fBlurbEndDelay", 0.0),
                        label: pstr(&props, "m_Label"),
                        look: pstr(&props, "m_LookType"),
                        vo: voref,
                        vo_length,
                    })
                }
                "DisConv_PlayerChoice" => NodeKind::Choice {
                    // Cooked data omits the class default (one static choice); its text is not
                    // read here, a neutral label stands in.
                    options: match find(&props, "m_Choices_Static").and_then(|p| p.array_structs(&pkg).ok()) {
                        Some(v) => v.iter().map(|c| pstr(c, "m_ChoiceText").unwrap_or_default()).collect(),
                        None => vec!["Continue".into()],
                    },
                    optional: find(&props, "m_Choices_Optional")
                        .and_then(|p| p.array_structs(&pkg).ok())
                        .unwrap_or_default()
                        .iter()
                        .map(|c| OptionalChoice { text: pstr(c, "m_ChoiceText").unwrap_or_default(), flag: flag(c), must_be_false: pbool(c, "m_bStoryFlagMustBeFalse", false) })
                        .collect(),
                },
                "DisConv_RandomBranch" => NodeKind::RandomBranch { keep_unique: pbool(&props, "m_bTryKeepUnique", true), weights: float_array(&props, "m_Branches").unwrap_or_else(|| vec![50.0, 50.0]) },
                "DisConv_SequentialBranch" => NodeKind::SequentialBranch { looping: pbool(&props, "m_bLooping", false), global: pbool(&props, "m_bApplyGlobally", false) },
                "DisConv_RandomSequentialBranch" => NodeKind::RandomSequentialBranch {
                    global: pbool(&props, "m_bApplyGlobally", true),
                    branches: find(&props, "m_Branches").and_then(|p| p.array_raw()).map(|(n, _)| n as u32).unwrap_or(outs.len().saturating_sub(1) as u32),
                    looping: pbool(&props, "m_bLooping", false),
                },
                "DisConv_TimeLimit" => NodeKind::TimeLimit { seconds: pfloat(&props, "m_fTimeLimitInSeconds", 30.0), global: pbool(&props, "m_bApplyGlobally", true) },
                "DisConv_CheckStoryFlag" => NodeKind::CheckFlag { flag: flag(&props) },
                "DisConv_SetStoryFlag" => NodeKind::SetFlag { flag: flag(&props), value: pbool(&props, "m_bValue", true) },
                "DisConv_SeenDialogLabel" => NodeKind::SeenLabel {
                    oneshot: pobj(&props, "m_pDialogOneShot").map(|o| pkg.obj_name(ObjRef::Export(o))),
                    conversation: pobj(&props, "m_pConversation").and_then(|c| conv_id.get(&c).copied()),
                    label: pstr(&props, "m_LabelToCheck").unwrap_or_default(),
                },
                "DisConv_HadConversation" => NodeKind::HadConversation { conversation: pobj(&props, "m_pConversation").and_then(|c| conv_id.get(&c).copied()) },
                "DisConv_ConversationFired" => NodeKind::ConversationFired { conversation: pobj(&props, "m_pConversation").and_then(|c| conv_id.get(&c).copied()) },
                "DisConv_SpeakerCombatStatus" => NodeKind::CombatStatus,
                "DisConv_KismetActivateRemoteEvent" => NodeKind::KismetOutput { name: pstr(&props, "m_EventName").unwrap_or_default() },
                "DisConv_Soiree" => {
                    let g = pguid(&props, "m_SoireeGUID").unwrap_or_default();
                    NodeKind::Soiree { guid: guid_hex(&g), matinee: matinees.get(&g).cloned(), preview_only: pbool(&props, "m_UsePreviewBlurbsOnly", false) }
                }
                "DisConv_ConversationRef" => {
                    // The referenced conversation is output 0's target.
                    let raw = output_links(&pkg, &props);
                    NodeKind::ConversationRef { conversation: raw.first().copied().flatten().and_then(|e| conv_id.get(&e).copied()) }
                }
                c if c == "DisConv_DialogHook" || c.starts_with("DisConv_Hook_") => NodeKind::Hook {
                    hook: pstr(&props, "m_HookEnum").unwrap_or_else(|| "DDH_INVALID".into()),
                    index: pint(&props, "m_iHookIndex", -1),
                    names: name_array(&pkg, &props, "m_KismetDialogHooks"),
                    priority: pstr(&props, "m_Priority").map(|p| Priority::from_enum(&p)),
                    distances: (c == "DisConv_Hook_Distance").then(|| [pfloat(&props, "m_fCloseDistance", 200.0), pfloat(&props, "m_fMediumDistance", 500.0), pfloat(&props, "m_fFarDistance", 1000.0)]),
                    // Class defaults (`Default__DisConv_Hook_*`): loiter 600 uu / 5-7 s, look-at
                    // 300 uu, suspicion distance 500 uu.
                    class: match HookClass::from_class(c) {
                        HookClass::SuspicionDist { .. } => HookClass::SuspicionDist { radius: pfloat(&props, "m_fRadius", 500.0) },
                        HookClass::PlayerLoiter { .. } => HookClass::PlayerLoiter { radius: pfloat(&props, "m_fLoiterRadius", 600.0), min: pfloat(&props, "m_fLoiterTime_Min", 5.0), max: pfloat(&props, "m_fLoiterTime_Max", 7.0) },
                        HookClass::PlayerLookAt { .. } => HookClass::PlayerLookAt {
                            radius: pfloat(&props, "m_fLookAtRadius", 300.0),
                            // `m_LookAtTargets[]` entries (0xc bytes: actor, angle, time).
                            targets: find(&props, "m_LookAtTargets")
                                .and_then(|p| p.array_structs(&pkg).ok())
                                .unwrap_or_default()
                                .iter()
                                .map(|t| {
                                    let actor = t.iter().find_map(|p| if let Value::Object(o) = p.value { (o != 0).then(|| pkg.obj_name(ObjRef::from_index(o))) } else { None }).unwrap_or_default();
                                    let floats: Vec<f32> = t.iter().filter_map(|p| p.as_float()).collect();
                                    (actor, floats.first().copied().unwrap_or(0.0), floats.get(1).copied().unwrap_or(0.0))
                                })
                                .collect(),
                        },
                        k => k,
                    },
                },
                "DisConv_CheckSpeakerSuspicionLevel" => NodeKind::CheckSuspicion,
                "DisConv_IsPossessed" => NodeKind::IsPossessed,
                "DisConv_HasInventoryItem" | "DisConv_HasInventoryItemEquipped" | "DisConv_HasAbstractItem" => NodeKind::HasItem {
                    item: pobj_name(&pkg, &props, "m_pItemClass").or_else(|| pobj_name(&pkg, &props, "m_pItem")).unwrap_or_default(),
                    equipped: class == "DisConv_HasInventoryItemEquipped",
                    quantity: (class == "DisConv_HasAbstractItem").then(|| pint(&props, "m_Quantity", 0)),
                },
                "DisConv_IsSpeakerAvailable" => NodeKind::SpeakerAvailable,
                "DisConv_CompareDarknessLevel" => NodeKind::CompareDarknessLevel { level: pint(&props, "m_LevelToCompare", 0) },
                "DisConv_SpeakerInStoryGroup" | "DisConv_SpeakerHasTweaks" | "DisConv_FactionBranch" | "DisConv_SpawnerBranch" | "DisConv_SpeakerSupportsDialogTree" => {
                    let kind = match class.as_str() {
                        "DisConv_SpeakerInStoryGroup" => GroupKind::StoryGroup,
                        "DisConv_SpeakerHasTweaks" => GroupKind::Tweaks,
                        "DisConv_FactionBranch" => GroupKind::Faction,
                        "DisConv_SpawnerBranch" => GroupKind::Spawner,
                        _ => GroupKind::DialogTree,
                    };
                    let name_of = |o: i32| if o != 0 { pkg.obj_name(ObjRef::from_index(o)) } else { String::new() };
                    // Spawner and dialog-tree branches list objects; the others list structs
                    // { object, exact-match flag }.
                    let entries: Vec<GroupEntry> = if matches!(kind, GroupKind::Spawner | GroupKind::DialogTree) {
                        find(&props, "m_Branches").and_then(|p| p.array_objects().ok()).unwrap_or_default().into_iter().map(|o| GroupEntry { name: name_of(o), exact: true }).collect()
                    } else {
                        find(&props, "m_Branches")
                            .and_then(|p| p.array_structs(&pkg).ok())
                            .unwrap_or_default()
                            .iter()
                            .map(|b| GroupEntry {
                                name: b.iter().find_map(|p| if let Value::Object(o) = p.value { Some(name_of(o)) } else { None }).unwrap_or_default(),
                                exact: b.iter().any(|p| matches!(p.value, Value::Bool(true)) && p.name.starts_with("m_bExact")),
                            })
                            .collect()
                    };
                    // The node has one output per entry plus "Everyone Else".
                    NodeKind::GroupBranch { groups: entries.iter().map(|e| e.name.clone()).collect(), kind, entries }
                }
                "DisConv_TryFallbackTree" => NodeKind::TryFallbackTree,
                "DisConv_MeSpeaker" | "DisConv_OneShotSpeaker" | "DisConv_InitiatorSpeaker" => NodeKind::Speaker {
                    oneshot_index: (class == "DisConv_OneShotSpeaker").then(|| pint(&props, "m_iOneShotSpeaker", -1)),
                    nick: pstr(&props, "m_NickName"),
                    not_in_combat: pbool(&props, "m_bNotWhileInCombat", false),
                    not_fearful: pbool(&props, "m_bNotWhileFearful", false),
                    not_suspicious: pbool(&props, "m_bNotWhileSuspicious", false),
                    must_have_los: pbool(&props, "m_bMustHaveLOS", false),
                },
                "DisConv_PlayerLookAtSpeaker" => NodeKind::LookAt { speaker: Some(pint(&props, "m_iSpeakerToLookAt", 0)) },
                "DisConv_PlayerLookAtActor" | "DisConv_PlayerStopLookAt" => NodeKind::LookAt { speaker: None },
                _ => NodeKind::Other,
            };
            // A ConversationRef's output 0 points at a conversation, not a node.
            let outputs = if matches!(kind, NodeKind::ConversationRef { .. }) { outs.iter().enumerate().map(|(k, o)| if k == 0 { None } else { *o }).collect() } else { outs };
            self.db.nodes.push(Node { path: pkg.obj_path(ObjRef::Export(i)), class, tree, conversation: conv, speaker, label: pstr(&props, "m_Label"), outputs, kind });
        }
        Ok(())
    }
}

/// `DisTweaks_ConvSettings` from its class default object (the only tweak data of the class:
/// no `Twk_ConvSettings` object is shipped). Values the cooked object omits keep the class
/// defaults of [`crate::hooks::ConvSettings`].
pub fn load_settings(cooked: &Path) -> Result<crate::hooks::ConvSettings> {
    use crate::hooks::{ConvSettings, HookOverride};
    let pkg = Package::open(cooked.join("DishonoredGame.upk"))?;
    let i = (0..pkg.exports.len()).find(|&i| pkg.obj_name(ObjRef::Export(i)) == "Default__DisTweaks_ConvSettings").context("no Default__DisTweaks_ConvSettings")?;
    let props = pkg.read_object(i)?.properties;
    let mut s = ConvSettings::default();
    let loud = |props: &[Property], n: &str, d: u8| pstr(props, n).and_then(|e| e.strip_prefix("EAINoiseLoudness").and_then(|x| x.parse().ok())).unwrap_or(d);
    s.battle_loudness = loud(&props, "m_BattleLoudness", s.battle_loudness);
    s.search_loudness = loud(&props, "m_SearchLoudness", s.search_loudness);
    s.notice_loudness = loud(&props, "m_NoticeLoudness", s.notice_loudness);
    let group = |props: &[Property]| pstr(props, "m_FireGroup").and_then(|e| e.strip_prefix("eDisDialogHookFireGroup_").map(str::to_string)).and_then(|n| ["Casual", "CasualStateChanges", "Attention", "Reaction", "Exertions", "Agitated", "CombatCries", "Combat", "Threat", "WitnessedMagic", "SoundAlarm", "CombatImportant", "GameplayImportant"].iter().position(|g| *g == n)).map(|g| g as u8);
    if let Some(list) = find(&props, "m_DialogHookOverrides").and_then(|p| p.array_structs(&pkg).ok()) {
        s.overrides = list
            .iter()
            .filter_map(|o| {
                let hook = pstr(o, "m_DialogHookToOverride").and_then(|h| crate::hooks::hook_index(&h))?;
                Some(HookOverride { hook, min: pfloat(o, "m_fRefire_DelayMin", 1.0), max: pfloat(o, "m_fRefire_DelayMax", 1.0), global: pfloat(o, "m_fRefire_Delay_Global", 0.0), group: group(o).unwrap_or(0) })
            })
            .collect();
    }
    s.max_at_once = [pint(&props, "m_MaxDialogHooksAtOnce", s.max_at_once[0]), pint(&props, "m_MaxDialogHooksAtOnce_Quiet", s.max_at_once[1])];
    s.max_distance = pfloat(&props, "m_fMaxDialogDistance", s.max_distance);
    s.loud_distance = pfloat(&props, "m_fDialogDistanceConsideredLoud", s.loud_distance);
    s.weight_rand_range = pfloat(&props, "m_fHookWeightRandRange", s.weight_rand_range);
    if let Some(list) = find(&props, "m_FireGroupConfig").and_then(|p| p.array_structs(&pkg).ok()) {
        for c in &list {
            let Some(g) = group(c) else { continue };
            let d = s.fire_groups[g as usize];
            s.fire_groups[g as usize] = crate::hooks::FireGroupConfig {
                max_per_group: [pint(c, "m_MaxPlaying_PerGroup", d.max_per_group[0]), pint(c, "m_MaxPlaying_PerGroup_Quiet", d.max_per_group[1])],
                max_per_initiator: [pint(c, "m_MaxPlaying_PerInitiator", d.max_per_initiator[0]), pint(c, "m_MaxPlaying_PerInitiator_Quiet", d.max_per_initiator[1])],
                ignore_global_cap: pbool(c, "m_bIgnoreGlobalCap", d.ignore_global_cap),
                min_time_between: pfloat(c, "m_fMinTimeBetweenFiring", d.min_time_between),
            };
        }
    }
    s.non_dominant_volume_scale = pfloat(&props, "m_fNonDominantVolumeScale", s.non_dominant_volume_scale);
    s.dominant_max_distance_percent = pfloat(&props, "m_fDominantMaxDistance_Percent", s.dominant_max_distance_percent);
    s.duck_blend_speed = pfloat(&props, "m_fAudioDuckBlendSpeed", s.duck_blend_speed);
    Ok(s)
}

/// Load the packages named (cooked package names without extension).
pub fn load_packages(cooked: &Path, packages: &[String], vo: &mut dyn VoLengths) -> Result<ConversationDb> {
    let mut l = Loader::new(cooked)?;
    for p in packages {
        l.add_package(p, vo).with_context(|| format!("dialog data of {p}"))?;
    }
    Ok(l.finish())
}

/// Per-class counts (research/reporting).
pub fn class_histogram(db: &ConversationDb) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for n in &db.nodes {
        *m.entry(n.class.clone()).or_default() += 1;
    }
    m
}
