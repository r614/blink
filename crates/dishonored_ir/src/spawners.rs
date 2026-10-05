//! Original NPC spawner settings (`DishonoredSpawner` / `DisTravelSpawner`), engine-agnostic.

use serde::{Deserialize, Serialize};

/// `DishonoredSpawner` settings that decide when and how its pawn appears (class defaults from
/// `Default__DishonoredSpawner`: spawn on begin play, capable of fleeing).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpawnerProps {
    /// `m_bSpawnOnBeginPlay` (default true); false: spawned later by Kismet or alarms.
    pub on_begin_play: bool,
    /// `m_SpawnAtSuspicionLevel` (`DAISL_*`), when set: an alarm / pursuit reinforcement.
    pub at_suspicion: Option<String>,
    pub squad: String,
    pub patrol_on_startup: bool,
    pub aware_on_startup: bool,
    pub capable_of_fleeing: bool,
    /// Spawns as a corpse (`m_bSpawnDead`) or unconscious (`m_bTreatAsKnockedOut`).
    pub spawn_dead: bool,
    pub knocked_out: bool,
    /// `m_pFactionTweakOverride` object path (e.g. `Boyle_Factions.Neutral_Civilian`).
    pub faction: String,
    /// `DisTravelSpawner.m_PlayerTravelLocationTags`: spawns when the player arrives at one of
    /// these travel tags (empty for plain spawners).
    pub travel_tags: Vec<String>,
}

impl Default for SpawnerProps {
    fn default() -> Self {
        SpawnerProps { on_begin_play: true, at_suspicion: None, squad: String::new(), patrol_on_startup: false, aware_on_startup: false, capable_of_fleeing: true, spawn_dead: false, knocked_out: false, faction: String::new(), travel_tags: Vec::new() }
    }
}

/// One original spawner of a level, as missions use it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Spawner {
    pub position: glam::Vec3,
    pub yaw: f32,
    /// Pawn tweak object path (decides the character).
    pub tweak: String,
    pub guard: bool,
    pub props: SpawnerProps,
}
