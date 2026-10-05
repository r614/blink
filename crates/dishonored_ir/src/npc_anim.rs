//! NPC animation data of the original (engine-agnostic): which AnimSets a pawn tweak binds,
//! its Arkane locomotion config (`ArkComponentLocomotionConfig`) and its animation definitions
//! class (`DisAnimDefinitions` subclass: one chain of sequences per `eDisNPCAnimations`).
//! See research/dishonored/ANIMATION.md.

use serde::{Deserialize, Serialize};

/// What a pawn tweak says about animation (`DisTweaks_Pawn`): resolved by the host, carried to
/// clients in `MatchView::anim_profiles`. References are (package, object path) of a package
/// that cooks each object.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AnimProfile {
    /// `m_AnimSets` in tweak order. A sequence name is looked up from the last set to the first
    /// (UE3 `USkeletalMeshComponent::FindAnimSequence`), so later sets override earlier ones
    /// (e.g. `Npc_Aristo_Empty_as` replaces the guard's `Empty_WalkN` for aristocrats).
    pub sets: Vec<(String, String)>,
    /// `m_pComponentLocomotionConfig`.
    pub locomotion: Option<(String, String)>,
    /// `m_pAnimDefClass` (class name, e.g. `DisNPCAnimDefinitions_EliteGuard`).
    pub defs: String,
    /// `m_bRagdollOnDeath`.
    pub ragdoll_on_death: bool,
}

/// `ArkComponentLocomotionConfig.m_Modifiers[i]`: a locomotion stance ("Empty", "Sword",
/// "SwordPatrol", "PanicEmpty", ...), its acceleration and speed multiplier.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LocoModifier {
    pub name: String,
    pub acceleration: f32,
    pub deceleration: f32,
    pub speed_multiplier: f32,
}

/// One `m_SpeedModePropDictionary` entry: the loop played at one speed of one modifier.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SpeedMode {
    pub name: String,
    pub rotation_max_speed: f32,
    /// `m_MoveAnimProps`: (sequence, authored ground speed uu/s). Moving modes list the four
    /// directions in the order S, W, N, E; idle modes one loop with speed 0.
    pub move_anims: Vec<(String, f32)>,
    /// `m_StartAnimProps` / `m_StopAnimProps` / `m_TurnAnimProps` sequence names.
    pub start_anims: Vec<String>,
    pub stop_anims: Vec<String>,
    pub turn_anims: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LocoConfig {
    pub name: String,
    pub modifiers: Vec<LocoModifier>,
    /// `m_SpeedNames` (Idle, Walk, Run, Sprint).
    pub speed_names: Vec<String>,
    pub modes: Vec<SpeedMode>,
    /// `m_SpeedModes`: `[modifier * speed_names.len() + speed]` -> index into `modes` (-1: none).
    pub table: Vec<i32>,
    /// `m_Transitions`: (sequence, from modifier, to modifier).
    pub transitions: Vec<(String, i32, i32)>,
    /// `m_SkeletalRefMeshName`: the skeleton the config's sequences were authored on.
    #[serde(default)]
    pub ref_mesh: String,
}

impl LocoConfig {
    pub fn modifier(&self, name: &str) -> Option<usize> {
        self.modifiers.iter().position(|m| m.name.eq_ignore_ascii_case(name))
    }
    /// The speed mode of `modifier` at speed index `speed` (0 idle, 1 walk, 2 run, 3 sprint).
    pub fn mode(&self, modifier: usize, speed: usize) -> Option<&SpeedMode> {
        let n = self.speed_names.len();
        let i = *self.table.get(modifier * n + speed)?;
        if i < 0 { None } else { self.modes.get(i as usize) }
    }
}

/// `DisAnimation`: one step of a group; `names` are alternatives (one picked at random).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AnimStep {
    pub names: Vec<String>,
    pub looped: bool,
    pub optional: bool,
    pub blend_in: f32,
    pub rate_range: f32,
    /// `m_FirstItemPrefix` (`eDisItemPrefix`: 0 none, 1 both, 2 primary, 3 secondary, ...):
    /// the sequence name is prefixed with the equipped item's prefix ("Sword_", "Empty_").
    pub item_prefix: u8,
}

/// `DisAnimationGroup`: steps played in order (intro, loop, outro).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AnimGroup {
    pub steps: Vec<AnimStep>,
    pub blend_out: f32,
    pub end_after_one: bool,
}

/// `DisAnimationChain`: alternative groups (one picked) for one `eDisNPCAnimations`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AnimChain {
    pub groups: Vec<AnimGroup>,
    pub ragdoll_if_pushed: bool,
}

/// A `DisAnimDefinitions` class's `m_Animations`, indexed by `eDisNPCAnimations`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AnimDefs {
    pub class: String,
    pub chains: Vec<AnimChain>,
}

impl AnimDefs {
    pub fn chain(&self, anim: NpcAnim) -> Option<&AnimChain> {
        self.chains.get(anim as usize).filter(|c| !c.groups.is_empty())
    }
}

/// The `eDisNPCAnimations` entries the presentation uses (values are the enum's indices).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NpcAnim {
    Beg = 1,
    Panic = 5,
    Stun = 6,
    HitReactStrong = 8,
    HitReactWindblast = 9,
    HitReactSmallAdditive = 10,
    HitReactProjectile = 11,
    Death = 13,
    DeathNoDirection = 14,
    DeathFast = 16,
    EquipToCombat = 20,
    EquipToRelaxedPatrol = 21,
    UnequipFromCombat = 23,
    PanicRecover = 29,
    Taunt = 32,
    Surprised = 34,
    TriggerAlarm = 35,
    CorpseCheck = 36,
    SearchLostTarget = 37,
    ReactToNoticing = 38,
    AttackLong = 39,
    AttackMedium = 40,
    AttackShort = 41,
    ParryAwaitingImpact = 61,
    Parry = 62,
    ParryVersus = 63,
    AmbientIdle = 66,
    AmbientAnxiousGradual = 68,
    CorpseBeingCarried = 71,
    Assassination = 76,
    AssassinationFast = 78,
    Fatality = 88,
    AimShoot = 96,
    AimReload = 97,
    AimIn = 98,
}

/// Names of `eDisNPCAnimations` (DishonoredGame `DisAnimDefinitions`), for tools and docs.
pub const NPC_ANIM_NAMES: &[&str] = &[
    "INVALID", "Beg", "BegGive", "BegIntimidate", "BegIntimidateGive", "Panic", "Stun", "PushToRagdoll", "HitReactStrong", "HitReactWindblast",
    "HitReactSmall_Additive", "HitReactProjectile", "ThrownKnockDown", "Death", "DeathNoDirection", "DeathCrouchNoDirection", "DeathFast", "DeathSwarmAttack", "DeathWallOfLight", "DeathPlague",
    "EquipToCombat", "EquipToRelaxedPatrol", "EquipToSearch", "UnequipFromCombat", "UnequipFromRelaxedPatrol", "UnequipFromSearch", "PostPossess", "Distraction", "CheckPockets", "PanicRecover",
    "Falling", "MagicLostTarget", "Taunt", "RatCrushAttempt", "Surprised", "TriggerAlarm", "CorpseCheck", "SearchLostTarget", "ReactToNoticing", "AttackLong",
    "AttackMedium", "AttackShort", "AttackStepBack", "BackLeftAttack", "BackRightAttack", "LeftAttack", "RightAttack", "SwarmAttack", "Bash", "PushBack",
    "JumpAttack", "RiposteLeft", "RiposteLeftSidestep", "RiposteRight", "RiposteRightSidestep", "Ambush", "SyncAttack", "NPCStomp", "NPCFatality", "NPCUnder",
    "NPCUnder_Ranged", "Parry_AwaitingImpact", "Parry", "ParryVersus", "Dodge", "SideStep", "AmbientIdle", "AmbientIdle_Weeper", "AmbientAnxiousGradual", "AmbientAnxiousSudden",
    "AmbientWeepVariation", "CorpseBeingCarried", "Weep", "Weeper_Stare", "Weeper_Callback", "Weeper_Cough", "Assassination", "AssassinationDrop", "AssassinationFast", "AssassinationDramatic_Back",
    "AssassinationDramatic_PendletonA", "AssassinationDramatic_PendletonB", "AssassinationDramatic_Campbell", "AssassinationDramatic_Martin", "AssassinationDramatic_Havelock", "AssassinationDramatic_LadyBoyle", "AssassinationDramatic_LordRegent", "AssassinationDramatic_Daud", "Fatality", "FatalityFast",
    "FatalityKnockDown", "FatalityHeadChop", "FatalityHeadChopFast", "ShortFatality_Forehand", "ShortFatality_Backhand", "Choke", "AimShoot", "AimReload", "AimIn", "AimOut",
    "Throw", "LieInWait", "Immolate", "TallboyAim", "TallboyAimShoot", "TallboyAimIn", "TallboyAimOut", "TallboyStompAttack", "TallboyFoldStilts", "TallboyUnFoldStilts",
    "WhiskeyFire", "ThrowWhiskey", "JumpBack", "FeelSick", "AttractSpell", "AttractSpellCoupDeGrace", "RushPrepare", "JumpBackToss", "PlayMusic", "PlayCombatTune",
    "Whistle", "MusicBoxIdle", "Sniffing", "DLC06_AssassinSalute", "DLC06_AssassinFail", "DLC06_AssassinIdle", "DLC06_SmokeBombed", "DLC06_SawIdle", "DLC06_SawFire", "DLC07_Pulled",
    "DLC07_SummonTentacle", "DLC07_TentacleSpawn", "DLC07_TentacleGrabWindup", "DLC07_TentacleLoseGrab", "DLC07_TentacleDespawn", "DLC07_WitchScream", "DLC07_GravehoundSpawn", "DLC07_Coughing",
];

/// Presentation stance of an AI pawn, published by the host (`MatchView::npc_poses`) so
/// clients can pick the locomotion modifier and one-shots the original's AI would request.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[repr(u8)]
pub enum NpcStance {
    /// Routine (idle, wander, group idle): "Empty" for civilians, "SwordPatrol" for guards.
    #[default]
    Relaxed = 0,
    /// Patrolling or standing a post (`DisBehaviorPatrol` / `DisBehaviorGuard`).
    Patrol = 1,
    /// Investigate / search: the "Search*" modifiers.
    Search = 2,
    /// Combat: "Sword" (weapon drawn).
    Combat = 3,
    /// Civilian panic run: "PanicEmpty".
    Panic = 4,
    /// Cowering in place (`DisAISubStateCower`).
    Cower = 5,
    /// Noticing / reaction delay (`DisBehaviorNotice`, `React`).
    Notice = 6,
}
