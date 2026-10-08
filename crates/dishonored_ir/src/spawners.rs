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
    /// The pawn tweak and the tweaks it inherits from (its `m_FallbackChainCooked`, nearest
    /// first), as `Pkg.Obj` paths: what "is or inherits from" tests (0x00C6F450).
    #[serde(default)]
    pub lineage: Vec<String>,
    /// The level package that places the spawner (a Kismet-streamed sublevel's spawners exist
    /// only while it is visible).
    #[serde(default)]
    pub package: String,
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
    /// `m_RandomAccessoryInfo[eDisAccessoryType]` with the matching mesh list (`m_HatMeshes`,
    /// `m_MaskMeshes`).
    #[serde(default)]
    pub accessories: Vec<AccessorySlot>,
}

/// One accessory type of a pawn tweak: the chance the pawn wears one, and the options.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AccessorySlot {
    /// `DisPawnAccessory.m_fRandomChanceOfUse`.
    pub chance_of_use: f32,
    pub meshes: Vec<AccessoryMesh>,
    /// An `m_PotentialNPCArmor` head armour that hits can knock off (`DisTweaks_Armor` with
    /// `m_ProtectedBodyRegion` DHR_HEAD and a `m_DamageTypesThatKnockOff` list); its
    /// `m_fPercentChance` is `chance_of_use`.
    #[serde(default)]
    pub helmet: bool,
}

/// `DisPawnAccessoryMesh`: a static mesh on a socket of the pawn's skeletal mesh.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AccessoryMesh {
    /// (package, object path) of the `StaticMesh`.
    pub mesh: (String, String),
    pub socket: String,
    pub chance: f32,
}

/// Deterministic stand-in for `rand() / 32768` (one value per pawn and roll).
fn roll(seed: u32, k: u32) -> f32 {
    let h = seed.wrapping_mul(0x9E37_79B1) ^ k.wrapping_mul(0x85EB_CA77).wrapping_add(0x2545_F491);
    let h = (h ^ (h >> 15)).wrapping_mul(0x2C1B_3C6D);
    let h = h ^ (h >> 12);
    (h >> 8) as f32 / (1u32 << 24) as f32
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

    /// Accessories for one pawn (0x00B7F8A0): for each accessory type, one roll against
    /// `m_fRandomChanceOfUse`, then a pick weighted by `m_fRandomChance` (0x00B764B0, the same
    /// running-sum rule as the heads). Returns (static mesh, socket).
    pub fn accessories(&self, seed: u32) -> Vec<((String, String), String)> {
        let mut out = Vec::new();
        for (i, slot) in self.accessories.iter().enumerate() {
            if slot.meshes.is_empty() || roll(seed, 200 + i as u32) >= slot.chance_of_use {
                continue;
            }
            let total: f32 = slot.meshes.iter().map(|m| m.chance.max(0.0)).sum();
            let mut x = roll(seed, 300 + i as u32) * total;
            let mut pick = slot.meshes.len() - 1;
            for (k, m) in slot.meshes.iter().enumerate() {
                if x < m.chance.max(0.0) {
                    pick = k;
                    break;
                }
                x -= m.chance.max(0.0);
            }
            let m = &slot.meshes[pick];
            out.push((m.mesh.clone(), m.socket.clone()));
        }
        out
    }

    /// Whether the pawn for `seed` wears a helmet: an armour slot whose chance roll passes (the
    /// same roll [`Self::accessories`] makes for its mesh).
    pub fn helmet(&self, seed: u32) -> bool {
        self.accessories.iter().enumerate().any(|(i, slot)| slot.helmet && roll(seed, 200 + i as u32) < slot.chance_of_use)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helmet_follows_the_armour_roll() {
        let slot = |chance: f32, helmet: bool| AccessorySlot { chance_of_use: chance, meshes: Vec::new(), helmet };
        let always = Appearance { accessories: vec![slot(0.0, false), slot(1.0, true)], ..Default::default() };
        assert!((0..50).all(|s| always.helmet(s)));
        let never = Appearance { accessories: vec![slot(1.0, false), slot(0.0, true)], ..Default::default() };
        assert!((0..50).all(|s| !never.helmet(s)));
        // A half chance (city guards' m_fPercentChance 0.5) gives some of each.
        let half = Appearance { accessories: vec![slot(0.5, true)], ..Default::default() };
        let worn = (0..200).filter(|s| half.helmet(*s)).count();
        assert!(worn > 50 && worn < 150, "{worn}");
        // Mesh-less armour slots add no accessory mesh.
        assert!(always.accessories(3).is_empty());
    }
}

