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
    /// The pawn tweak's look (empty when unresolved).
    #[serde(default)]
    pub appearance: Appearance,
}
/// An NPC's look from its pawn tweak (`DisTweaks_NPCPawn`, resolved through Arkane's fallback
/// chain): the body `m_pSkeletalMesh` and the `m_RandomHeadMeshes` options. Mesh references are
/// (package, object path) of the level package that cooks each `SkeletalMesh`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Appearance {
    pub body: Option<(String, String)>,
    /// (mesh, m_fRandomChance)
    pub heads: Vec<((String, String), f32)>,
}

impl Appearance {
    /// Parts for one pawn: body plus the head picked by `seed` (the original picks a random head
    /// weighted by `m_fRandomChance`; `seed` makes the pick deterministic per pawn).
    pub fn parts(&self, seed: u32) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self.body.iter().cloned().collect();
        let total: f32 = self.heads.iter().map(|h| h.1.max(0.0)).sum();
        if total > 0.0 {
            let mut x = (seed.wrapping_mul(2654435761) >> 8) as f32 / (1u32 << 24) as f32 * total;
            for (m, w) in &self.heads {
                if x < w.max(0.0) {
                    v.push(m.clone());
                    break;
                }
                x -= w.max(0.0);
            }
        } else if let Some((m, _)) = self.heads.first() {
            v.push(m.clone());
        }
        v
    }
}

