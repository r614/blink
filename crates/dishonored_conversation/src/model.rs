//! Engine-independent form of Dishonored's dialog data.
//!
//! The original data model (research/dishonored/BOYLE_MISSION.md section 4):
//! - `DisDialogOneShot` (a level actor) binds speakers (`m_MeSpeaker`, `m_OtherSpeakers[]`:
//!   nick name, attached spawner, voice data) to a `DisDialogTree_OneShot`.
//! - A tree owns hooks (`DisConv_DialogHook` and `DisConv_Hook_*`, indexed by `m_iHookIndex`
//!   in `m_DialogHooks`), tree-level logic nodes, `DisConv_ConversationRef`s and the
//!   `DisConversation`s.
//! - A conversation owns its nodes (`DisConv_Blurb`, branches, conditions, actions) and starts
//!   at its `Start` output link.
//!
//! Every node keeps its original object path and its output links in the original order.
//! Text is whatever the package holds (English subtitles); nothing here is ever written to
//! the repository.

use serde::{Deserialize, Serialize};

pub type NodeId = u32;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConversationDb {
    /// Packages the data was read from.
    pub packages: Vec<String>,
    pub oneshots: Vec<OneShot>,
    pub trees: Vec<Tree>,
    pub conversations: Vec<Conversation>,
    pub nodes: Vec<Node>,
    /// `DisDialogVoiceData` path -> the trees its blurb lists belong to (`m_VoiceData[].m_pTree`):
    /// a pawn's generic (bark) trees come from its voice (`DisTweaks_NPCPawn.m_GenericDialogData`).
    #[serde(default)]
    pub voice_trees: std::collections::BTreeMap<String, Vec<u32>>,
}

/// `DisDialogOneShot`: a level actor binding speakers to a one-shot tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OneShot {
    pub package: String,
    /// Object name, e.g. `DisDialogOneShot_9`.
    pub name: String,
    /// Full object path inside the package.
    pub path: String,
    pub tree: Option<u32>,
    /// Speaker 0 is `m_MeSpeaker`, 1.. are `m_OtherSpeakers[0..]`.
    pub speakers: Vec<OneShotSpeaker>,
    pub location: [f32; 3],
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OneShotSpeaker {
    /// `m_SpeakerNickName`.
    pub nick: String,
    /// `m_pAttachedActor` (normally a `DishonoredSpawner_N`), object name.
    pub actor: Option<String>,
    /// `m_pVoiceData` object path.
    pub voice: Option<String>,
}

/// `DisDialogTree` / `DisDialogTree_OneShot`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Tree {
    pub package: String,
    pub path: String,
    pub oneshot: Option<u32>,
    /// Hook nodes (`m_DialogHooks`, non-empty entries).
    pub hooks: Vec<NodeId>,
    pub conversations: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Priority {
    Low,
    Medium,
    #[default]
    High,
    NotInterruptable,
}

impl Priority {
    pub fn from_enum(s: &str) -> Self {
        match s {
            "LOW_PRIORITY_CONV" => Priority::Low,
            "MEDIUM_PRIORITY_CONV" => Priority::Medium,
            "NOT_INTERRUPTABLE_CONV" => Priority::NotInterruptable,
            _ => Priority::High,
        }
    }
}

/// Entry of `DisConversation.m_Speakers` (indexed by a node's `m_iSpeaker`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SpeakerSlot {
    /// `ME_SPEAKER`: the one-shot's `m_MeSpeaker` (or the NPC owning a shared tree).
    Me,
    /// `INITIATOR_SPEAKER`: whoever triggered the conversation (another NPC for barks).
    Initiator,
    /// `PLAYER_SPEAKER`.
    Player,
    /// `LINKED_NODE_SPEAKER` -> `DisConv_OneShotSpeaker.m_iOneShotSpeaker` (index into the
    /// one-shot's `m_OtherSpeakers`).
    OneShot(i32),
    /// Anything else (type name).
    Other(String),
}

/// `DisConversation`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Conversation {
    pub path: String,
    /// Object name (`DisConversation_N`).
    pub name: String,
    pub tree: u32,
    /// `m_Label` (e.g. `Hits_On_Player`).
    pub label: String,
    /// `m_Description` (designer note).
    pub description: String,
    pub speakers: Vec<SpeakerSlot>,
    /// Per speaker slot: the linked speaker node's `m_NickName` (e.g. "Samuel The Boatman").
    pub speaker_nicks: Vec<Option<String>>,
    /// Per speaker slot: gating of the linked speaker node.
    pub speaker_gates: Vec<SpeakerGate>,
    /// Output links `Start`, `Interrupted`, `Player Walked Away`.
    pub start: Option<NodeId>,
    pub interrupted: Option<NodeId>,
    pub walked_away: Option<NodeId>,
    /// Defaults from `Default__DisConversation` (cooked data omits values equal to them).
    pub blocking: bool,
    pub blocking_player: bool,
    pub fire_once: bool,
    pub priority: Priority,
    pub player_must_be_near: bool,
    pub player_near_radius: f32,
    pub not_when_player_in_combat: bool,
    pub always_interrupt_for_combat: bool,
}

/// Flags of a conversation's linked speaker node (`DisConv_Speaker`). The bit layout at
/// +0x94 is `m_bNotWhileInCombat` 1, `m_bNotWhileSuspicious` 2, `m_bNotWhileFearful` 4,
/// `m_bOptional` 8 (`FUN_00c9e2b0` lets an optional speaker fail to bind).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpeakerGate {
    pub not_in_combat: bool,
    pub not_fearful: bool,
    pub not_suspicious: bool,
    pub optional: bool,
}

/// One graph node (conversation-level or tree-level).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub path: String,
    pub class: String,
    pub tree: u32,
    /// Owning conversation (None for tree-level nodes such as hooks).
    pub conversation: Option<u32>,
    /// `m_iSpeaker` (index into the conversation's speakers), -1 when absent.
    pub speaker: i32,
    /// `DisConv_Node.m_Label`. Every node that runs with a label records it in the memory of
    /// each bound speaker (`FUN_00ca3a80`), not only lines.
    #[serde(default)]
    pub label: Option<String>,
    /// `m_OutputLinks[i].m_pLink`, in order.
    pub outputs: Vec<Option<NodeId>>,
    pub kind: NodeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoRef {
    /// `m_AkEventName` (`Play_<32 hex>`).
    pub event: String,
    /// `m_pAkBank` name (`Bk_<GUID>`): the per-voice AKPK file `Bk_<GUID>.<LANG>`.
    pub bank: String,
}

/// `DisConv_Blurb` / `DisConv_NonWord`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Line {
    /// `m_iBlurbGUID` as 32 hex digits.
    pub guid: String,
    /// `m_Text` (the subtitle).
    pub text: String,
    pub nonword: bool,
    /// `m_SpokenTo` (speaker index, -1 = nobody).
    pub spoken_to: i32,
    /// `m_bMainSubtitle` (default true; barks set false).
    pub main_subtitle: bool,
    /// `m_fVoiceRadius` (default 3000 uu).
    pub voice_radius: f32,
    /// `m_fBlurbEndDelay` (seconds after the line before the next node).
    pub end_delay: f32,
    /// `m_Label` (checked by `DisConv_SeenDialogLabel`).
    pub label: Option<String>,
    pub look: Option<String>,
    /// VO resolved through the speaker's voice data.
    pub vo: Option<VoRef>,
    /// Length of the decodable VO clip (seconds), when it resolved.
    pub vo_length: Option<f32>,
}

impl Line {
    /// How long the line holds the conversation.
    ///
    /// The original (`DisConv_Blurb` update `FUN_00c9e0f0`) waits until the speaker reports
    /// that no voice is playing any more (`FUN_00c9a160`: the Wwise playing IDs of the
    /// speaker), then counts `m_fBlurbEndDelay` down. There is no reading-time fallback: a
    /// blurb whose speaker has no event for its GUID posts nothing, shows no subtitle
    /// (`FUN_00c9fbf0` only adds a subtitle for a playing ID) and ends on the next tick.
    /// - VO event and clip length known: the clip, plus the end delay.
    /// - No VO event: only the end delay (the original's silent skip).
    /// - VO event whose clip we cannot measure: a reading-time estimate stands in for the
    ///   unknown clip length (APPROXIMATION; the original plays the clip).
    pub fn duration(&self) -> f32 {
        let speech = match (&self.vo, self.vo_length) {
            (_, Some(len)) => len,
            (None, None) => 0.0,
            (Some(_), None) => text_duration(&self.text),
        };
        speech + self.end_delay.max(0.0)
    }

    /// Whether the original shows a subtitle for this line: a playing voice and text.
    pub fn has_subtitle(&self) -> bool {
        self.vo.is_some() && !self.text.trim().is_empty()
    }
}

/// Reading-time stand-in for a clip whose length is unknown: 15 characters per second, at
/// least 1.5 s (not from the original; see [`Line::duration`]).
pub fn text_duration(text: &str) -> f32 {
    (text.trim().chars().count() as f32 / 15.0).max(1.5)
}

/// `DisPlayerChoice_Optional`: shown after the static choices while its story flag equals
/// `!m_bStoryFlagMustBeFalse` (`FUN_00c8b810`); choosing it fires output
/// `static count + its index in the optional list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OptionalChoice {
    pub text: String,
    pub flag: String,
    pub must_be_false: bool,
}

/// What a group branch compares the speaker's pawn against.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GroupKind {
    /// `DisConv_SpeakerInStoryGroup`: `DisSpeakerStoryGroup { m_pStoryGroup, m_bExactMatch }`.
    StoryGroup,
    /// `DisConv_SpeakerHasTweaks`: `DisSpeakerTweaks { m_pTweaks, m_bExactMatch }` (exact =
    /// the pawn's tweaks object itself, else any tweaks it derives from).
    Tweaks,
    /// `DisConv_FactionBranch`: `DisFactionBranch { m_pFaction, m_bExactMatchOnly }`.
    Faction,
    /// `DisConv_SpawnerBranch`: the pawn was spawned by this `DishonoredSpawner`.
    Spawner,
    /// `DisConv_SpeakerSupportsDialogTree`: the speaker can play this tree.
    DialogTree,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroupEntry {
    /// Object name of the group / tweaks / faction / spawner / tree ("" when unset).
    pub name: String,
    pub exact: bool,
}

/// Every node's `Evaluate` (vtable +0x140) returns the index of the output link to follow,
/// or -1 to wait (lines, choices, matinees); the walker `FUN_00ca3c10` follows
/// `m_OutputLinks[index]`. A null link ends the conversation normally, an index past the
/// links ends it as an error (research/dishonored/CONVERSATIONS.md).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NodeKind {
    Line(Line),
    /// `DisConv_PlayerChoice`: `m_Choices_Static[].m_ChoiceText` (output i = choice i), then
    /// the optional choices whose flag test passes. No static choice: output 0 at once.
    Choice {
        options: Vec<String>,
        #[serde(default)]
        optional: Vec<OptionalChoice>,
    },
    /// `DisConv_RandomBranch`: `m_Branches` are percentage weights (default 50/50). A roll
    /// `rand() * 100 / 32767` takes the first output whose cumulative weight exceeds it
    /// (the last output if none does). With `m_bTryKeepUnique` (default true) a repeat of the
    /// previous pick moves to the other output whose weight is closest to it.
    RandomBranch {
        keep_unique: bool,
        #[serde(default)]
        weights: Vec<f32>,
    },
    /// `DisConv_SequentialBranch`: counter c, next = c + 1; past the last output it stays
    /// there, or restarts at 1 with `m_bLooping`; output = next - 1.
    SequentialBranch { looping: bool, global: bool },
    /// `DisConv_RandomSequentialBranch` (extends the sequential branch): the counter walks a
    /// shuffled order of the `branches` outputs; past the end it takes the extra "Depleted"
    /// output, or restarts with `m_bLooping` (reshuffled when the pass commits).
    RandomSequentialBranch {
        global: bool,
        branches: u32,
        #[serde(default)]
        looping: bool,
    },
    /// `DisConv_TimeLimit`: output 1 while `now - last <= seconds` after a pass, else output 0
    /// (which records the time).
    TimeLimit { seconds: f32, global: bool },
    /// `DisConv_CheckStoryFlag`: output 0 = flag false, 1 = flag true.
    CheckFlag { flag: String },
    /// `DisConv_SetStoryFlag` (`m_bValue`, default true).
    SetFlag { flag: String, value: bool },
    /// `DisConv_SeenDialogLabel`: output 1 when the speaker `m_iSpeaker` has heard the label
    /// `m_LabelToCheck` in conversation `m_pConversation` (no conversation set: output 0).
    SeenLabel {
        oneshot: Option<String>,
        #[serde(default)]
        conversation: Option<u32>,
        label: String,
    },
    /// `DisConv_HadConversation`: output 1 when the speaker `m_iSpeaker` took part in
    /// `m_pConversation` before.
    HadConversation { conversation: Option<u32> },
    /// `DisConv_ConversationFired`: output 1 when `m_pConversation` ever started (global).
    ConversationFired { conversation: Option<u32> },
    /// `DisConv_KismetActivateRemoteEvent`: fires the same-named output of the Kismet
    /// `DisSeqEvent_DialogOutputs` bound to the one-shot.
    KismetOutput { name: String },
    /// `DisConv_Soiree`: plays the matinee whose `m_MatineeGUID` equals `guid` and waits for it;
    /// output 0 follows. Output 1 is the chain of preview blurbs played instead when
    /// `m_UsePreviewBlurbsOnly` is set. In a non-blocking conversation it is skipped.
    Soiree {
        guid: String,
        matinee: Option<String>,
        #[serde(default)]
        preview_only: bool,
    },
    /// `DisConv_ConversationRef` -> `DisConversation`.
    ConversationRef { conversation: Option<u32> },
    /// Tree hook: `m_HookEnum` (`DDH_*`), `m_iHookIndex`, Kismet names for
    /// `DisConv_Hook_KismetActivated` (output i = name i).
    Hook { hook: String, index: i32, names: Vec<String>, priority: Option<Priority>, distances: Option<[f32; 3]> },
    /// `DisConv_CheckSpeakerSuspicionLevel`: the output is the speaker's suspicion level.
    CheckSuspicion,
    /// `DisConv_IsPossessed`: 0 = not possessed, 1 = possessed, 2 = possessed by the player's
    /// own possession (three outputs).
    IsPossessed,
    /// `DisConv_SpeakerCombatStatus`: four outputs, see [`crate::runner::ConvEnv::combat_status`].
    CombatStatus,
    /// `DisConv_HasInventoryItem` / `_HasInventoryItemEquipped` / `_HasAbstractItem`:
    /// output 0 = no, 1 = yes. The speaker must be the player.
    HasItem {
        item: String,
        equipped: bool,
        /// `DisConv_HasAbstractItem.m_Quantity` (count must be at least this).
        #[serde(default)]
        quantity: Option<i32>,
    },
    /// `DisConv_IsSpeakerAvailable`: output 0 = unavailable, 1 = available.
    SpeakerAvailable,
    /// `DisConv_SpeakerInStoryGroup` / `_SpeakerHasTweaks` / `_FactionBranch` /
    /// `_SpawnerBranch` / `_SpeakerSupportsDialogTree`: the first matching entry's output,
    /// else output `entries` ("Everyone Else").
    GroupBranch {
        groups: Vec<String>,
        #[serde(default = "default_group_kind")]
        kind: GroupKind,
        #[serde(default)]
        entries: Vec<GroupEntry>,
    },
    /// `DisConv_TryFallbackTree`.
    TryFallbackTree,
    /// Speaker declarations (`DisConv_MeSpeaker`, `_OneShotSpeaker`, `_InitiatorSpeaker`):
    /// gating flags.
    Speaker { oneshot_index: Option<i32>, nick: Option<String>, not_in_combat: bool, not_fearful: bool, not_suspicious: bool, must_have_los: bool },
    /// `DisConv_PlayerLookAtSpeaker` / `_PlayerLookAtActor` / `_PlayerStopLookAt`
    /// (camera constraints; presentation).
    LookAt { speaker: Option<i32> },
    /// Anything else: passes through output 0.
    Other,
}

fn default_group_kind() -> GroupKind {
    GroupKind::StoryGroup
}

impl ConversationDb {
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    /// Conversation display name: `<package>:<one-shot>/<label>`.
    pub fn conversation_name(&self, c: u32) -> String {
        let conv = &self.conversations[c as usize];
        let tree = &self.trees[conv.tree as usize];
        match tree.oneshot.map(|o| &self.oneshots[o as usize]) {
            Some(o) => format!("{}:{}/{}", o.package, o.name, conv.label),
            None => format!("{}:{}/{}", tree.package, tree.path, conv.label),
        }
    }

    /// Find a one-shot by `Package:Name`, `Name` or full path.
    pub fn find_oneshot(&self, reference: &str) -> Result<u32, String> {
        let (pkg, name) = match reference.split_once(':') {
            Some((p, n)) => (Some(p), n),
            None => (None, reference),
        };
        let hits: Vec<u32> = self
            .oneshots
            .iter()
            .enumerate()
            .filter(|(_, o)| (o.name.eq_ignore_ascii_case(name) || o.path.eq_ignore_ascii_case(name)) && pkg.is_none_or(|p| o.package.eq_ignore_ascii_case(p)))
            .map(|(i, _)| i as u32)
            .collect();
        match hits.as_slice() {
            [i] => Ok(*i),
            [] => Err(format!("dialog one-shot '{reference}' not found")),
            _ => Err(format!("dialog one-shot '{reference}' is ambiguous; prefix its package")),
        }
    }

    /// One-shots whose speakers are attached to an actor (`Package:Name` or `Name`).
    pub fn oneshots_for_actor(&self, actor: &str) -> Vec<(u32, usize)> {
        let (pkg, name) = match actor.split_once(':') {
            Some((p, n)) => (Some(p), n),
            None => (None, actor),
        };
        let mut out = Vec::new();
        for (i, o) in self.oneshots.iter().enumerate() {
            if pkg.is_some_and(|p| !o.package.eq_ignore_ascii_case(p)) {
                continue;
            }
            for (k, s) in o.speakers.iter().enumerate() {
                if s.actor.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(name)) {
                    out.push((i as u32, k));
                }
            }
        }
        out
    }

    /// Resolve a conversation reference:
    /// - `Package:DisDialogOneShot_N/<label | DisConversation_M | kismet hook name>`
    /// - `DisDialogOneShot_N/<...>` (unique one-shot name)
    /// - `<label>` alone when it is unique among the loaded conversations
    /// - a full object path ending in `DisConversation_M`.
    pub fn resolve(&self, reference: &str) -> Result<Target, String> {
        if let Some((os, what)) = reference.split_once('/') {
            let o = self.find_oneshot(os)?;
            let Some(t) = self.oneshots[o as usize].tree else { return Err(format!("'{os}' has no dialog tree")) };
            let tree = &self.trees[t as usize];
            // Conversation label or object name.
            let convs: Vec<u32> = tree.conversations.iter().copied().filter(|c| {
                let cv = &self.conversations[*c as usize];
                cv.label.eq_ignore_ascii_case(what) || cv.name.eq_ignore_ascii_case(what)
            }).collect();
            if let [c] = convs.as_slice() {
                return Ok(Target::Conversation { oneshot: Some(o), conversation: *c });
            }
            if convs.len() > 1 {
                return Err(format!("'{reference}': several conversations carry that label"));
            }
            // Kismet hook name (what `DisSeqAct_DialogInputs` addresses).
            for h in &tree.hooks {
                if let NodeKind::Hook { names, .. } = &self.node(*h).kind {
                    if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(what)) {
                        return Ok(Target::Hook { oneshot: o, hook: *h, output: i });
                    }
                }
            }
            // Hook enum (`DDH_PLAYER_USED`...), optionally with an output: `DDH_PLAYER_APPROACHES#1`
            // (distance hooks: 0 close, 1 medium, 2 far).
            let (what_hook, output) = match what.rsplit_once('#') {
                Some((h, n)) if n.parse::<usize>().is_ok() => (h, n.parse::<usize>().unwrap()),
                _ => (what, 0),
            };
            for h in &tree.hooks {
                if let NodeKind::Hook { hook, .. } = &self.node(*h).kind {
                    if hook.eq_ignore_ascii_case(what_hook) {
                        return Ok(Target::Hook { oneshot: o, hook: *h, output });
                    }
                }
            }
            return Err(format!("'{reference}': no conversation, Kismet hook or hook type '{what}' in that one-shot"));
        }
        if let Ok(o) = self.find_oneshot(reference) {
            return self.used_hook(o).map(|(h, out)| Target::Hook { oneshot: o, hook: h, output: out }).ok_or_else(|| format!("'{reference}' has no DDH_PLAYER_USED hook"));
        }
        let hits: Vec<u32> = (0..self.conversations.len() as u32)
            .filter(|c| {
                let cv = &self.conversations[*c as usize];
                cv.label.eq_ignore_ascii_case(reference) || cv.path.eq_ignore_ascii_case(reference) || reference.ends_with(&format!(".{}", cv.path))
            })
            .collect();
        match hits.as_slice() {
            [c] => {
                let tree = &self.trees[self.conversations[*c as usize].tree as usize];
                Ok(Target::Conversation { oneshot: tree.oneshot, conversation: *c })
            }
            [] => Err(format!("conversation '{reference}' not found")),
            _ => Err(format!("conversation '{reference}' is ambiguous ({} matches); use Package:DisDialogOneShot_N/{reference}", hits.len())),
        }
    }

    /// The tree's `DDH_PLAYER_USED` hook (what pressing use on the NPC fires).
    pub fn used_hook(&self, oneshot: u32) -> Option<(NodeId, usize)> {
        let t = self.oneshots[oneshot as usize].tree?;
        self.trees[t as usize].hooks.iter().copied().find(|h| matches!(&self.node(*h).kind, NodeKind::Hook { hook, .. } if hook == "DDH_PLAYER_USED")).map(|h| (h, 0))
    }

    /// Every `DisConv_KismetActivateRemoteEvent` name reachable from a conversation (for
    /// validation and reports).
    pub fn outputs_of(&self, conversation: u32) -> Vec<String> {
        let mut v: Vec<String> = self
            .nodes
            .iter()
            .filter(|n| n.conversation == Some(conversation))
            .filter_map(|n| match &n.kind {
                NodeKind::KismetOutput { name } => Some(name.clone()),
                _ => None,
            })
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Count of line nodes, lines with VO, lines with a resolved clip length.
    pub fn line_stats(&self) -> (usize, usize, usize) {
        let mut s = (0, 0, 0);
        for n in &self.nodes {
            if let NodeKind::Line(l) = &n.kind {
                s.0 += 1;
                if l.vo.is_some() {
                    s.1 += 1;
                }
                if l.vo_length.is_some() {
                    s.2 += 1;
                }
            }
        }
        s
    }
}

/// What a start request points at.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Target {
    Conversation { oneshot: Option<u32>, conversation: u32 },
    /// Enter a tree hook at one of its outputs (Kismet hook names / use / approach).
    Hook { oneshot: u32, hook: NodeId, output: usize },
    /// Enter a hook of a tree that no one-shot owns (an NPC's generic dialog tree: barks).
    TreeHook { tree: u32, hook: NodeId, output: usize },
}
