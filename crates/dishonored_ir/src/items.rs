//! Inventory, pickups, takedowns, powers and gadgets: the tweak values the original reads,
//! normalized (see research/dishonored/INVENTORY.md for the code that consumes them).
//!
//! Defaults are the values of the GOG install (Corvo, Normal difficulty) so a config
//! serialized before these fields existed still runs the original rules; the loader in
//! `dishonored_assets::gameplay` reads every field from the install and records its source.

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// `eDisAmmoType` (DisGlobalEnums), base-game entries only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum AmmoType {
    Bullet = 0,
    BulletExplosive = 1,
    Bolt = 2,
    SleepDart = 3,
    IncendiaryBolt = 4,
    SpringRazor = 5,
    Grenade = 6,
    StickyGrenade = 7,
}

impl AmmoType {
    pub const ALL: [AmmoType; 8] = [
        AmmoType::Bullet,
        AmmoType::BulletExplosive,
        AmmoType::Bolt,
        AmmoType::SleepDart,
        AmmoType::IncendiaryBolt,
        AmmoType::SpringRazor,
        AmmoType::Grenade,
        AmmoType::StickyGrenade,
    ];
    pub fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }
    /// The `eDisAmmoType_*` name used in tweaks (`m_AmmoType`, `m_AmmoTypeOnRecover`).
    pub fn from_name(n: &str) -> Option<Self> {
        Some(match n.trim_start_matches("eDisAmmoType_") {
            "Bullet" => Self::Bullet,
            "Bullet_Explosive" => Self::BulletExplosive,
            "Arrow" => Self::Bolt,
            "Arrow_Sleep" => Self::SleepDart,
            "Arrow_Flare" => Self::IncendiaryBolt,
            "SpringRazor" => Self::SpringRazor,
            "Grenade" => Self::Grenade,
            "StickyGrenade" => Self::StickyGrenade,
            _ => return None,
        })
    }
}

/// `EElixirType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[repr(u8)]
pub enum ElixirType {
    Health = 0,
    Mana = 1,
}

/// Player inventory rules (`DisTweaks_PlayerPawn` / `_Attributes` of `Twk_Pawn_Corvo_Release`,
/// `DishonoredInventory` defaults, `DisTweaks_PlayerInput`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InventoryConfig {
    /// `Attribute_*Capacity` per [`AmmoType`] (the attribute update 0x00B56880 copies them
    /// into `DishonoredInventory.m_AmmoInfo[].m_AmmoCapacity`).
    pub ammo_capacity: [u16; 8],
    /// `m_nHealthElixirValue` / `m_nManaElixirValue` (points per elixir).
    pub health_elixir_value: f32,
    pub mana_elixir_value: f32,
    /// `m_nMaxHealthElixir` / `m_nMaxManaElixir`.
    pub max_health_elixirs: u8,
    pub max_mana_elixirs: u8,
    /// `DisTweaks_InteractableInterface.m_fMaxDistance` (class default; pickups do not override it).
    pub interact_distance: f32,
    /// `Attribute_ChokeTime`, added to the choke animation length (0x00A7AA00).
    pub choke_time: f32,
    /// Length of the choke animation the timer adds (`FUN_00c4de20` of anim state 0x5f): the
    /// `Sword_Choke_Loop_Master` sequence of `Ply_Sword_Choke_as` (LIKELY mapping, U-101).
    pub choke_anim_time: f32,
    /// `DisTweaks_PlayerInput.m_fChokeHoldToAutoCarry` / `m_fAssassinateHoldToAutoCorpseCarry`.
    pub choke_hold_to_auto_carry: f32,
    pub assassinate_hold_to_auto_carry: f32,
    /// `Attribute_GroundSpeedCarryingCorpse` (replaces `Attribute_GroundSpeed` while carrying).
    pub carry_ground_speed: f32,
    /// `Attribute_JumpZCarryingCorpse`.
    pub carry_jump_z: f32,
    /// `Attribute_ThrowStrength` (speed a dropped/thrown body or object leaves the hand at).
    pub throw_strength: f32,
    /// `Attribute_RangedDamage` of the player's crossbow and pistol (`DisTweaks_InventoryItem_Attributes`
    /// through their fallback chains): the projectile's base damage (0x00C257A0).
    pub crossbow_ranged_damage: f32,
    pub pistol_ranged_damage: f32,
    /// `m_MaxActivatedBoneCharmCount`.
    pub max_equipped_charms: u8,
    /// `DishonoredPlayerPawn` `m_fMinSpeedRayScale`, `m_fRaySpeedScale`, `m_fRaySpeedScale_Max`
    /// (DefaultPlayer.ini): melee reach grows with forward speed (0x00AB2050).
    #[serde(default = "d_ray")]
    pub melee_ray_speed: [f32; 3],
    /// `Attribute_MeleeDamage` of Corvo's sword (`Twk_Inv_SwordCorvo` attributes, Normal) and
    /// Corvo's own `Attribute_MeleeDamageBonus` (`Twk_Pawn_Corvo`): a sword hit deals
    /// `round(m_fAttackDamageMultiplier x (MeleeDamage + MeleeDamageBonus))` (0x00C07C80).
    #[serde(default = "d_sword_damage")]
    pub sword_melee_damage: f32,
    #[serde(default)]
    pub melee_damage_bonus: f32,
    /// `DishonoredPlayerPawn.m_fOffscreenMeleeDamageReduction[difficulty]` (DefaultPlayer.ini):
    /// an NPC melee hit from offscreen loses this fraction (0x00C33DD0).
    #[serde(default = "d_offscreen")]
    pub offscreen_melee_reduction: [f32; 4],
}

fn d_ray() -> [f32; 3] {
    [400.0, 0.005, 1.5]
}
fn d_sword_damage() -> f32 {
    10.0
}
fn d_offscreen() -> [f32; 4] {
    [0.5; 4]
}

impl Default for InventoryConfig {
    fn default() -> Self {
        Self {
            ammo_capacity: [10, 10, 10, 10, 10, 5, 5, 5],
            health_elixir_value: 40.0,
            mana_elixir_value: 50.0,
            max_health_elixirs: 10,
            max_mana_elixirs: 10,
            interact_distance: 200.0,
            choke_time: 1.5,
            choke_anim_time: 5.033,
            choke_hold_to_auto_carry: 0.2,
            assassinate_hold_to_auto_carry: 0.2,
            carry_ground_speed: 350.0,
            carry_jump_z: 800.0,
            throw_strength: 2000.0,
            crossbow_ranged_damage: 20.0,
            pistol_ranged_damage: 10.0,
            max_equipped_charms: 3,
            melee_ray_speed: d_ray(),
            sword_melee_damage: d_sword_damage(),
            melee_damage_bonus: 0.0,
            offscreen_melee_reduction: d_offscreen(),
        }
    }
}

/// `DisTweaks_Possessable` (one per possessable kind).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PossessableConfig {
    /// `m_Levels[i].m_fPossessDuration` per Possession level (0 = cannot at that level).
    pub duration: [f32; 2],
    /// `m_RequiredPossessPowerLevel` (1-based).
    pub required_level: u8,
    /// `m_fGroundSpeedWhilePossessed` (set through `Modifier_PossessGroundSpeed`).
    pub ground_speed: f32,
    /// `m_AwarenessSettings[EAIAwareness].m_bCanBePossessed`.
    pub awareness: [bool; 8],
    pub disallow: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PossessConfig {
    pub mana_cost: f32,
    pub rune_costs: [i32; 2],
    /// `m_Levels[i].m_fPossessRange`.
    pub range: [f32; 2],
    /// `m_Levels[i].m_fBendTimeExtension`.
    pub bend_time_extension: [f32; 2],
    /// `m_fCancelCooldown`: cooldown after a cancelled or ended possession.
    pub cancel_cooldown: f32,
    pub magical_appearance_delay: f32,
    /// `m_IntroDuration` (camera travel into the host).
    pub intro_duration: f32,
    pub warn_before_exit: f32,
    pub exit_fade: f32,
    /// `Twk_Possessable_BaseNPC` and `Twk_Possessable_Rat`.
    pub npc: PossessableConfig,
    pub rat: PossessableConfig,
}

impl Default for PossessConfig {
    fn default() -> Self {
        Self {
            mana_cost: 60.0,
            rune_costs: [3, 5],
            range: [1000.0, 2000.0],
            bend_time_extension: [0.0, 4.0],
            cancel_cooldown: 1.0,
            magical_appearance_delay: 0.8,
            intro_duration: 1.1,
            warn_before_exit: 4.0,
            exit_fade: 0.8,
            npc: PossessableConfig { duration: [0.0, 15.0], required_level: 2, ground_speed: 300.0, awareness: [true, true, true, true, true, true, true, false], disallow: false },
            rat: PossessableConfig { duration: [20.0, 30.0], required_level: 1, ground_speed: 300.0, awareness: [true; 8], disallow: false },
        }
    }
}

/// `PowerAttributes_BendTime`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BendTimeLevel {
    pub world_dilation: f32,
    pub player_dilation: f32,
    pub duration: f32,
    pub dilation_warmup: f32,
    pub dilation_cooldown: f32,
    pub projectile_early_wakeup: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BendTimeConfig {
    pub mana_cost: f32,
    pub rune_costs: [i32; 2],
    pub levels: [BendTimeLevel; 2],
}

impl Default for BendTimeConfig {
    fn default() -> Self {
        let l = |w, p, d| BendTimeLevel { world_dilation: w, player_dilation: p, duration: d, dilation_warmup: 0.15, dilation_cooldown: 0.25, projectile_early_wakeup: 0.25 };
        Self { mana_cost: 60.0, rune_costs: [2, 8], levels: [l(0.15, 0.85, 12.0), l(0.0, 0.9, 8.0)] }
    }
}

/// `PowerAttributes_DevouringSwarm` + its `DisTweaks_RatSwarm`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SwarmLevel {
    pub max_distance: f32,
    pub duration: f32,
    pub rat_count: u16,
    pub detection_radius: f32,
    pub initial_damage_delay: f32,
    pub damage_interval: f32,
    pub damage_per_bite: f32,
    pub min_damage: f32,
    pub max_damage: f32,
    pub eat_startup: f32,
    pub eat_per_limb: f32,
    pub eat_delay_after_kill: f32,
    /// `m_fPawnEscapeRadius`: an attack group drops a target farther than this (750).
    #[serde(default = "swarm_escape_radius")]
    pub escape_radius: f32,
    /// `m_EatRequiredRatCount`: rats in reach needed for the eating clock to run (5).
    #[serde(default = "swarm_eat_required")]
    pub eat_required_rats: u16,
}

fn swarm_escape_radius() -> f32 {
    750.0
}
fn swarm_eat_required() -> u16 {
    5
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SwarmConfig {
    pub mana_cost: f32,
    pub rune_costs: [i32; 2],
    /// `m_fRatSwarmSpawnDelay`.
    pub spawn_delay: f32,
    pub levels: [SwarmLevel; 2],
}

impl Default for SwarmConfig {
    fn default() -> Self {
        let l = |n| SwarmLevel {
            max_distance: 1500.0,
            duration: 30.0,
            rat_count: n,
            detection_radius: 700.0,
            initial_damage_delay: 2.0,
            damage_interval: 0.5,
            damage_per_bite: 0.4,
            min_damage: 2.0,
            max_damage: 5.0,
            eat_startup: 1.5,
            eat_per_limb: 1.5,
            eat_delay_after_kill: 0.5,
            escape_radius: 750.0,
            eat_required_rats: 5,
        };
        Self { mana_cost: 40.0, rune_costs: [3, 4], spawn_delay: 0.65, levels: [l(30), l(60)] }
    }
}

/// `eDisAttributeModifierType` of `DishonoredPowerModifierInfo` (DefaultPlayer.ini).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModifierType {
    AddVal,
    SetVal,
    AddBasePercent,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AttributeModifier {
    pub attribute: String,
    pub kind: ModifierType,
    pub value: f32,
}

/// One `DishonoredPowersComponent.m_Powers` entry (passive powers: Vitality, BloodThirsty,
/// ShadowKill, Celerity = Agility). Level 0 is "not bought".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PassivePower {
    pub name: String,
    /// Per level: (rune cost, modifiers applied while that level is current).
    pub levels: Vec<(i32, Vec<AttributeModifier>)>,
}

/// `DisTweaks_WhaleBoneCharmList.m_Charms` entry: an attribute raised by the charm's value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoneCharm {
    pub name: String,
    pub attribute: String,
    pub values: Vec<f32>,
}

/// `DisTweaks_SpringRazorPlaced` + `[DisGadget_SpringRazorPlaced]` (DefaultGadget.ini).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpringRazorConfig {
    pub arming_time: f32,
    pub detection_radius: f32,
    pub detection_height: f32,
    pub detonation_radius: f32,
    pub detonation_height: f32,
    pub trigger_reaction_time: f32,
    pub damage_reaction_time: f32,
    pub damage_delay: f32,
    pub max_damage_distance: f32,
    /// `m_NonNPCDamage[Normal]`: damage to non-NPC pawns (players).
    pub non_npc_damage: f32,
    /// `m_ImmuneNPCDamage`: damage to NPCs immune to the razor's kill.
    pub immune_npc_damage: f32,
    pub impulse: f32,
    pub sensor_offset: f32,
}

impl Default for SpringRazorConfig {
    fn default() -> Self {
        Self {
            arming_time: 0.5,
            detection_radius: 250.0,
            detection_height: 250.0,
            detonation_radius: 250.0,
            detonation_height: 250.0,
            trigger_reaction_time: 0.0,
            damage_reaction_time: 0.75,
            damage_delay: 0.35,
            max_damage_distance: 900.0,
            non_npc_damage: 5.0,
            immune_npc_damage: 50.0,
            impulse: 800.0,
            sensor_offset: 10.0,
        }
    }
}

/// `DisTweaks_Explosion` (`m_RadialDamage` at Normal difficulty).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExplosionConfig {
    pub radial_damage: f32,
    pub damage_radius: f32,
    pub full_damage_radius: f32,
    pub player_damage_radius: f32,
    pub player_full_damage_radius: f32,
    pub radial_momentum: f32,
    pub extra_up_force: f32,
    pub attached_damage: f32,
}

impl Default for ExplosionConfig {
    fn default() -> Self {
        Self { radial_damage: 50.0, damage_radius: 600.0, full_damage_radius: 300.0, player_damage_radius: 600.0, player_full_damage_radius: 150.0, radial_momentum: 1500.0, extra_up_force: 500.0, attached_damage: 75.0 }
    }
}

/// Grenade projectile + `DisTweaks_GrenadeComponent` + the player's `DisTweaks_ThrowGrenade`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GrenadeConfig {
    pub throw_speed: f32,
    /// Degrees the throw is raised above the aim.
    pub throw_angle: f32,
    pub gravity_mult: f32,
    /// `m_fDetonationDelay` (time fuse once armed; 3.0 in the cooked tweaks).
    pub fuse: f32,
    /// `m_fMinDetonationDelayAfterHit`.
    pub min_delay_after_hit: f32,
    pub bounciness: f32,
    pub floor_bounciness: f32,
    pub sticky: bool,
    pub explosion: ExplosionConfig,
}

impl Default for GrenadeConfig {
    fn default() -> Self {
        Self { throw_speed: 2500.0, throw_angle: 5.0, gravity_mult: 1.0, fuse: 3.0, min_delay_after_hit: 0.5, bounciness: 0.3, floor_bounciness: 0.65, sticky: false, explosion: ExplosionConfig::default() }
    }
}

/// A crossbow ammo variant (`Twk_Proj_Arrow*`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct BoltConfig {
    pub damage_mult: f32,
    pub speed_mult: f32,
    pub gravity_mult: f32,
    pub limb_damage_cap: f32,
    pub kill_on_headshot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GadgetConfig {
    pub springrazor: SpringRazorConfig,
    pub grenade: GrenadeConfig,
    pub sticky_grenade: GrenadeConfig,
    pub bolt: BoltConfig,
    pub sleep_dart: BoltConfig,
    pub incendiary_bolt: BoltConfig,
    /// Seconds an incendiary bolt sets a pawn on fire for (`m_fNPCImmolationTime`).
    pub immolation_time: f32,
}

impl Default for GadgetConfig {
    fn default() -> Self {
        Self {
            springrazor: SpringRazorConfig::default(),
            grenade: GrenadeConfig::default(),
            sticky_grenade: GrenadeConfig { sticky: true, ..GrenadeConfig::default() },
            bolt: BoltConfig { damage_mult: 1.0, speed_mult: 1.0, gravity_mult: 1.3, limb_damage_cap: 15.0, kill_on_headshot: true },
            sleep_dart: BoltConfig { damage_mult: 1.25, speed_mult: 1.0, gravity_mult: 0.4, limb_damage_cap: 25.0, kill_on_headshot: false },
            incendiary_bolt: BoltConfig { damage_mult: 2.5, speed_mult: 0.2, gravity_mult: 0.5, limb_damage_cap: 50.0, kill_on_headshot: false },
            immolation_time: 0.5,
        }
    }
}

/// `DisTweaks_NPCPawn` Shadow Kill timings (`m_fShadowKillDelay`, `m_fShadowKillFadeToBlackDuration`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShadowKillConfig {
    pub delay: f32,
    pub fade_duration: f32,
    pub rune_costs: [i32; 2],
}

impl Default for ShadowKillConfig {
    fn default() -> Self {
        Self { delay: 0.0, fade_duration: 5.0, rune_costs: [2, 4] }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ItemsConfig {
    pub inventory: InventoryConfig,
    pub possess: PossessConfig,
    pub bend_time: BendTimeConfig,
    pub swarm: SwarmConfig,
    pub shadow_kill: ShadowKillConfig,
    pub gadgets: GadgetConfig,
    pub passive_powers: Vec<PassivePower>,
    pub charms: Vec<BoneCharm>,
    /// Rune costs of the active powers per level (`m_RuneCosts`): (power, [level 1, level 2]).
    pub active_rune_costs: Vec<(String, [i32; 2])>,
    /// `DisTweaks_Upgrade` objects of `Twk_Upgrades` (Piero's workshop): name and
    /// `m_Attributes` (attribute, value added).
    #[serde(default)]
    pub upgrades: Vec<(String, Vec<(String, f32)>)>,
}

impl AmmoType {
    /// The `Attribute_*Capacity` that sets this ammo's capacity.
    pub fn capacity_attribute(self) -> &'static str {
        match self {
            AmmoType::Bullet => "Attribute_BulletCapacity",
            AmmoType::BulletExplosive => "Attribute_ExplosiveBulletCapacity",
            AmmoType::Bolt => "Attribute_ArrowCapacity",
            AmmoType::SleepDart => "Attribute_SleepArrowCapacity",
            AmmoType::IncendiaryBolt => "Attribute_FlareArrowCapacity",
            AmmoType::SpringRazor => "Attribute_SpringRazorCapacity",
            AmmoType::Grenade => "Attribute_GrenadeCapacity",
            AmmoType::StickyGrenade => "Attribute_StickyGrenadeCapacity",
        }
    }
}

/// What a placed pickup gives (the actor class and its tweak).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PickupKind {
    /// `DisAbstractItemPickup(Note|AudioLog)`: `m_pAbstractItem` x `m_Quantity` (coins, runes,
    /// keys, notes, heirlooms). `note`: a written note/audio log (reveals information).
    Abstract { item: String, quantity: i32, note: bool, key_item: bool },
    /// `DisStatPickup`: `m_HealthChange` / `m_ManaChange` and per-ammo `m_AmmoRanges` (min, max).
    Stat { health: i32, mana: i32, food: bool, ammo: [(i16, i16); 8] },
    /// `DisElixirHealth` / `DisElixirMana`.
    Elixir(ElixirType),
    /// `DisWhaleBoneCharm` (index into the charm list when known).
    BoneCharm(Option<u8>),
    /// `DishonoredInventoryPickup`: an inventory item (weapon, gadget).
    Inventory { item: String },
}

/// A pickup actor placed in a level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlacedPickup {
    /// Actor name (`DisAbstractItemPickup_11`), how missions and spawners refer to it.
    pub name: String,
    /// Level package the actor lives in.
    pub package: String,
    pub position: Vec3,
    pub tweak: String,
    pub kind: PickupKind,
    /// Spawner whose `m_pStealablePickup` this is (carried by that NPC, pickpocketable).
    #[serde(default)]
    pub stealable_from: Option<String>,
}
