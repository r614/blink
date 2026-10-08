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
    /// `DisTweaks_Pawn.m_pPhysicsAsset` (the ragdoll's bodies and joints).
    #[serde(default)]
    pub physics: Option<(String, String)>,
    /// `DisTweaks_NPCPawn.m_pComponentLookatConfig` (look-at modes, blinking).
    #[serde(default)]
    pub lookat: Option<(String, String)>,
    /// `m_ItemPrefix` of the pawn's primary item (`m_ContentInventoryLoadout[0]`): the prefix of
    /// the chain steps with an item prefix while the item is held (`Sword_` elite guards,
    /// `MusicBox_` music-box Overseers, `Patrol_` wolfhounds, `Empty_` Tallboys). Empty: none.
    #[serde(default)]
    pub item_prefix: String,
    /// `m_ItemPrefix` of the pawn's empty hands (`m_pEmptyHandsItem`): `Empty_` for every
    /// Boyle pawn. Empty: none.
    #[serde(default)]
    pub empty_prefix: String,
}

impl AnimProfile {
    /// The prefix of item-prefixed chain steps (`m_FirstItemPrefix`) for a pawn holding its
    /// primary item (`armed`) or nothing. A pawn without a primary item (weepers) holds its empty
    /// hands; `Sword_` / `Empty_` when the tweak was not resolved (both prefixes empty).
    pub fn prefix(&self, armed: bool) -> &str {
        match (armed, self.item_prefix.is_empty(), self.empty_prefix.is_empty()) {
            (true, false, _) => self.item_prefix.as_str(),
            (true, true, true) => "Sword_",
            (_, _, false) => self.empty_prefix.as_str(),
            (false, _, true) => "Empty_",
        }
    }
}

/// An NPC's look-at aim as the AI's head computes it (`ArkAnimNodeLookAt` 0x009176F0 split of
/// the look-at cursor between the torso and head grids; `dishonored_ai::head`), published so the
/// visible head turns where the vision cone points. Radians: yaw to the pawn's right, pitch up.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct NpcHead {
    pub id: u32,
    pub head: [f32; 2],
    pub torso: [f32; 2],
}

/// One `ArkComponentLookatConfig.m_LookAtModes` entry: the additive head / torso grids for a
/// stance and gait ("EmptyIdle", "SwordWalk", "SearchIdle", ...) and their angle ranges
/// (degrees: right, left, up, down; left and down negative).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LookatMode {
    pub name: String,
    pub head_range: [f32; 4],
    pub torso_range: [f32; 4],
    pub head_horizontal_pct: f32,
    pub head_vertical_pct: f32,
    /// `m_bDoClampRange` with `m_ClampRightRange`, `m_ClampLeftRange`, `m_ClampUpRange`,
    /// `m_ClampDownRange` (degrees, left and down negative).
    #[serde(default)]
    pub clamp: Option<[f32; 4]>,
    /// `Start_Torso` / `End_Head`: where along the reach the torso starts taking the aim
    /// (0x009176F0).
    #[serde(default)]
    pub start_torso: f32,
    #[serde(default)]
    pub end_head: f32,
    /// 3x3 grids in the order LU, LC, LD, CU, CC, CD, RU, RC, RD (empty name: none).
    pub head: Vec<String>,
    pub torso: Vec<String>,
}

/// `ArkComponentLookatConfig`: look-at modes and the blink timing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct LookatConfig {
    pub modes: Vec<LookatMode>,
    pub blink: bool,
    /// `MinDurationBetweenBlink` / `MaxDurationBetweenBlink` (s), `MinBlinkSpeed` /
    /// `MaxBlinkSpeed` (play rate).
    pub blink_interval: [f32; 2],
    pub blink_rate: [f32; 2],
    /// `m_fTorsoInfluenceMaxMoveSpeed` (class default 200): the torso weight of a request
    /// flagged so falls linearly to 0 at this pawn speed (0x00953050).
    #[serde(default)]
    pub torso_influence_max_move_speed: f32,
}

/// One rigid body of a `PhysicsAsset` (`RB_BodySetup`): the bone it drives and its collision
/// shapes (`AggGeom`), reduced to what a presentation ragdoll needs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PhysBody {
    pub bone: String,
    /// Largest sphere / capsule radius of the body (uu).
    pub radius: f32,
    /// Capsule (`KSphylElem`) centre in bone space and its axis length, when the body has one.
    pub center: [f32; 3],
    pub length: f32,
    /// `bFixed`.
    pub fixed: bool,
}

/// One joint of a `PhysicsAsset` (`RB_ConstraintSetup`): `ConstraintBone1` is the child body's
/// bone, `ConstraintBone2` the parent's; limits in degrees.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PhysConstraint {
    pub bone1: String,
    pub bone2: String,
    pub swing_limited: bool,
    pub twist_limited: bool,
    pub swing1: f32,
    pub swing2: f32,
    pub twist: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PhysicsAssetData {
    pub name: String,
    pub bodies: Vec<PhysBody>,
    pub constraints: Vec<PhysConstraint>,
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
    /// `m_fFootOnGroundPct`: fraction of the cycle a foot stays planted (0 for idle modes).
    #[serde(default)]
    pub foot_on_ground: f32,
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
    /// `m_fVelocityToStopMoving` / `m_fVelocityToStartToMove` (uu/s; class defaults 20 / 60).
    #[serde(default = "default_stop_speed")]
    pub stop_speed: f32,
    #[serde(default = "default_start_speed")]
    pub start_speed: f32,
    /// `m_Feet[].m_fOnGroundPct`: sync phase at which each foot is planted (class default left
    /// 0.44, right 0.94).
    #[serde(default)]
    pub feet: Vec<f32>,
}

fn default_stop_speed() -> f32 {
    20.0
}

fn default_start_speed() -> f32 {
    60.0
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
    /// `m_FaceAnimName`: sequence the `FaceSlot` plays with this step (full name, empty: none).
    #[serde(default)]
    pub face: String,
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
    /// Thrown by a push or a Windblast (group 1: `Generic_WindBlast_{Front,Left,Right,Back}`).
    PushToRagdoll = 7,
    HitReactStrong = 8,
    HitReactWindblast = 9,
    HitReactSmallAdditive = 10,
    HitReactProjectile = 11,
    Death = 13,
    DeathNoDirection = 14,
    DeathFast = 16,
    DeathSwarmAttack = 17,
    DeathPlague = 19,
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
    AttackStepBack = 42,
    Bash = 48,
    JumpAttack = 50,
    /// Minigame attacks (`DisTweaks_Minigame.m_NPCAnimation`): the wolfhound's arm and jump
    /// attacks, the weeper's arm grab; the group is the context's `m_GroupIndex`, the step the
    /// minigame state.
    SyncAttack = 56,
    Dodge = 64,
    SideStep = 65,
    ParryAwaitingImpact = 61,
    Parry = 62,
    ParryVersus = 63,
    AmbientIdle = 66,
    AmbientAnxiousGradual = 68,
    AmbientWeepVariation = 70,
    CorpseBeingCarried = 71,
    Weep = 72,
    WeeperCough = 75,
    Assassination = 76,
    AssassinationFast = 78,
    Fatality = 88,
    Choke = 95,
    AimShoot = 96,
    AimReload = 97,
    AimIn = 98,
    /// `StateNPCMasterTallboyAim` sub-states (0x00A7CEB0): 4 aim loop, 0 shoot, 1 / 2 in, 3 out,
    /// 5 the stomp.
    TallboyAim = 103,
    TallboyAimShoot = 104,
    TallboyAimIn = 105,
    TallboyAimOut = 106,
    TallboyStompAttack = 107,
    JumpBack = 112,
    PlayMusic = 118,
    PlayCombatTune = 119,
    /// The music box held: `DisWepMusicBox` slots 81 / 96 (0x00C291F0, 0x00C29220) set the
    /// pawn's `m_WalkUpperBodyDesire` to it.
    MusicBoxIdle = 121,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `NpcAnim` value is the index of its `eDisNPCAnimations` name.
    #[test]
    fn npc_anim_values_match_the_enum_names() {
        use NpcAnim as A;
        let named = [
            (A::Beg, "Beg"), (A::Panic, "Panic"), (A::Stun, "Stun"), (A::PushToRagdoll, "PushToRagdoll"), (A::HitReactStrong, "HitReactStrong"),
            (A::HitReactWindblast, "HitReactWindblast"), (A::HitReactProjectile, "HitReactProjectile"), (A::Death, "Death"), (A::DeathFast, "DeathFast"),
            (A::DeathSwarmAttack, "DeathSwarmAttack"), (A::DeathPlague, "DeathPlague"), (A::AttackShort, "AttackShort"), (A::Bash, "Bash"),
            (A::JumpAttack, "JumpAttack"), (A::SyncAttack, "SyncAttack"), (A::SideStep, "SideStep"), (A::Parry, "Parry"), (A::AmbientIdle, "AmbientIdle"),
            (A::AmbientWeepVariation, "AmbientWeepVariation"), (A::CorpseBeingCarried, "CorpseBeingCarried"), (A::Weep, "Weep"), (A::WeeperCough, "Weeper_Cough"),
            (A::Assassination, "Assassination"), (A::Choke, "Choke"), (A::AimShoot, "AimShoot"), (A::TallboyAim, "TallboyAim"), (A::TallboyAimShoot, "TallboyAimShoot"),
            (A::TallboyAimIn, "TallboyAimIn"), (A::TallboyAimOut, "TallboyAimOut"), (A::TallboyStompAttack, "TallboyStompAttack"), (A::JumpBack, "JumpBack"),
            (A::PlayMusic, "PlayMusic"), (A::PlayCombatTune, "PlayCombatTune"), (A::MusicBoxIdle, "MusicBoxIdle"),
        ];
        for (a, n) in named {
            assert_eq!(NPC_ANIM_NAMES[a as usize], n);
        }
    }

    #[test]
    fn item_prefix_falls_back_to_sword_and_empty() {
        let mut p = AnimProfile::default();
        assert_eq!(p.prefix(true), "Sword_");
        assert_eq!(p.prefix(false), "Empty_");
        // A weeper: no primary item, its empty hands.
        p.empty_prefix = "Empty_".into();
        assert_eq!(p.prefix(true), "Empty_");
        p.item_prefix = "MusicBox_".into();
        assert_eq!(p.prefix(true), "MusicBox_");
        assert_eq!(p.prefix(false), "Empty_");
    }
}
