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
    /// Spawner actor name (`DishonoredSpawner_11`): how missions address an original NPC.
    #[serde(default)]
    pub name: String,
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
    /// Per head option (parallel to `heads`), per material slot of the head mesh
    /// (`m_MaterialVariation.m_VariationMaterialIndex[slot]`): the material options
    /// (material object path, `m_fRandomChance`).
    #[serde(default)]
    pub head_variations: Vec<Vec<Vec<(String, f32)>>>,
    /// `m_BodyVariationMaterials`: per body material slot, the options.
    #[serde(default)]
    pub body_variations: Vec<Vec<(String, f32)>>,
    /// The pawn tweak's animation setup (AnimSets, locomotion config, animation definitions).
    #[serde(default)]
    pub anim: crate::npc_anim::AnimProfile,
}

impl Appearance {
    /// Parts for one pawn: body plus the head picked by `seed` (the original picks a random head
    /// weighted by `m_fRandomChance`; `seed` makes the pick deterministic per pawn).
    pub fn parts(&self, seed: u32) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self.body.iter().cloned().collect();
        if let Some(h) = self.head_index(seed) {
            v.push(self.heads[h].0.clone());
        }
        v
    }

    /// The head option picked for `seed` (weighted by `m_fRandomChance`, 0x00B76390).
    pub fn head_index(&self, seed: u32) -> Option<usize> {
        let total: f32 = self.heads.iter().map(|h| h.1.max(0.0)).sum();
        if total > 0.0 {
            let mut x = (seed.wrapping_mul(2654435761) >> 8) as f32 / (1u32 << 24) as f32 * total;
            for (i, (_, w)) in self.heads.iter().enumerate() {
                if x < w.max(0.0) {
                    return Some(i);
                }
                x -= w.max(0.0);
            }
            None
        } else if self.heads.is_empty() {
            None
        } else {
            Some(0)
        }
    }

    /// Material variations for one pawn as `(part, material slot, material path)` (part 0 the
    /// body, 1 the head). The original (0x00B7B0D0) rolls one `rand()/32767` per material slot
    /// and takes the first option whose running sum of `m_fRandomChance` exceeds it; when the
    /// chances sum below the roll the mesh keeps its own material. `seed` makes the rolls
    /// deterministic per pawn.
    pub fn materials(&self, seed: u32) -> Vec<(u8, u32, String)> {
        let mut out = Vec::new();
        let roll = |k: u32| -> f32 {
            let h = seed.wrapping_mul(0x9E37_79B1) ^ k.wrapping_mul(0x85EB_CA77).wrapping_add(0x2545_F491);
            let h = (h ^ (h >> 15)).wrapping_mul(0x2C1B_3C6D);
            let h = h ^ (h >> 12);
            (h >> 8) as f32 / (1u32 << 24) as f32
        };
        let pick = |part: u8, slots: &[Vec<(String, f32)>], base: u32, out: &mut Vec<(u8, u32, String)>| {
            for (slot, opts) in slots.iter().enumerate() {
                let r = roll(base + slot as u32);
                let mut acc = 0.0;
                for (m, c) in opts {
                    acc += c;
                    if r < acc {
                        out.push((part, slot as u32, m.clone()));
                        break;
                    }
                }
            }
        };
        if self.body.is_some() {
            pick(0, &self.body_variations, 0, &mut out);
        }
        if let Some(h) = self.head_index(seed) {
            if let Some(v) = self.head_variations.get(h) {
                pick(if self.body.is_some() { 1 } else { 0 }, v, 64, &mut out);
            }
        }
        out
    }
}

