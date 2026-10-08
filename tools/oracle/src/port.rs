//! The headless port side: the port's derived data for a map (a snapshot), and the query and
//! scenario runners that call the port's library code (`dishonored_world`, `dishonored_sim`)
//! directly: no Bevy, no window, no runtime crate.

use anyhow::{Context, Result, ensure};
use dishonored_oracle::diff::{PortAnswers, PortHit};
use dishonored_oracle::query::CollisionSet;
use dishonored_world::nav::NavWorld;
use dishonored_world::{CollisionWorld, channel};
use glam::{Mat4, Vec3};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SNAP_MAGIC: &[u8; 8] = b"BLKPSNAP";
/// Bump when [`PortSnapshot`] or the port's collision import changes (2: the collision audit). A `dishonored_world` serde change also invalidates
/// snapshots: decoding fails and the tool asks for `oracle snapshot`.
const SNAP_VERSION: u32 = 3;

/// The port's view of a map, as `dishonored_content::load_match` builds it for the game.
#[derive(Serialize, Deserialize)]
pub struct PortSnapshot {
    pub map: String,
    /// What it was built from (content cache file names), for staleness reports.
    pub built_from: String,
    pub built_unix: u64,
    pub collision: CollisionWorld,
    pub nav: Option<NavWorld>,
    pub water: Vec<dishonored_world::WaterVolume>,
    pub volumes: Vec<dishonored_world::PhysicsVolume>,
    pub climbables: Vec<dishonored_world::Climbable>,
    /// `GameplayConfig` as JSON (the sim config is rebuilt from it).
    pub gameplay_json: String,
    pub spawns: Vec<(Vec3, f32)>,
    /// [streaming] The Kismet-streamed sublevels' collision (not in [Self::collision]).
    #[serde(default)]
    pub levels: Vec<(String, CollisionWorld)>,
}

impl PortSnapshot {
    /// The collision world with its navigation and volumes attached, ready for queries.
    pub fn world(self) -> (CollisionWorld, String) {
        self.world_with(&[])
    }

    /// [`Self::world`] with these Kismet-streamed sublevels visible (the level streaming state
    /// to compare against; a capture of a map opened from the console has none of them).
    pub fn world_with(mut self, stream: &[String]) -> (CollisionWorld, String) {
        let visible: Vec<&CollisionWorld> = self.levels.iter().filter(|l| stream.iter().any(|s| s.eq_ignore_ascii_case(&l.0))).map(|l| &l.1).collect();
        let mut base = std::mem::take(&mut self.collision);
        // Movers of hidden streamed sublevels are not in the world either.
        let hidden: Vec<&str> = self.levels.iter().map(|l| l.0.as_str()).filter(|l| !stream.iter().any(|s| s.eq_ignore_ascii_case(l))).collect();
        base.movers.retain(|m| dishonored_world::source_package(&m.actor).is_none_or(|p| !hidden.iter().any(|h| h.eq_ignore_ascii_case(p))));
        let mut w = base.merged(&visible);
        w.nav = self.nav.take().map(|mut n| {
            n.finish_load();
            Arc::new(n)
        });
        w.water = Some(Arc::new(std::mem::take(&mut self.water)));
        w.volumes = Some(Arc::new(std::mem::take(&mut self.volumes)));
        w.climbables = Some(Arc::new(std::mem::take(&mut self.climbables)));
        (w, self.gameplay_json)
    }
}

pub fn snapshot_path(root: &Path, map: &str) -> PathBuf {
    root.join("port").join(format!("{map}.snap"))
}

pub fn save_snapshot(root: &Path, s: &PortSnapshot) -> Result<PathBuf> {
    let p = snapshot_path(root, &s.map);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let mut bytes = Vec::with_capacity(64 << 20);
    bytes.extend_from_slice(SNAP_MAGIC);
    bytes.extend_from_slice(&SNAP_VERSION.to_le_bytes());
    bytes.extend(bincode::serde::encode_to_vec(s, bincode::config::standard())?);
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

pub fn load_snapshot(root: &Path, map: &str) -> Result<PortSnapshot> {
    let p = snapshot_path(root, map);
    let bytes = std::fs::read(&p).with_context(|| format!("no port snapshot {} (run `oracle snapshot --map {map}`)", p.display()))?;
    ensure!(bytes.len() > 12 && &bytes[..8] == SNAP_MAGIC, "{}: not a port snapshot", p.display());
    let v = u32::from_le_bytes(bytes[8..12].try_into()?);
    ensure!(v == SNAP_VERSION, "{}: snapshot version {v}, expected {SNAP_VERSION} (run `oracle snapshot --map {map}`)", p.display());
    let (s, _): (PortSnapshot, _) = bincode::serde::decode_from_slice(&bytes[12..], bincode::config::standard())
        .with_context(|| format!("{}: stale snapshot layout (run `oracle snapshot --map {map}`)", p.display()))?;
    Ok(s)
}

/// Build the snapshot through the game's own loader (`dishonored_content::load_match`).
#[cfg(feature = "import")]
pub fn build_snapshot(map: &str) -> Result<PortSnapshot> {
    let mc = dishonored_content::load_match(map)?;
    let gameplay_json = serde_json::to_string(&mc.sim.gameplay)?;
    let levels = mc.levels.levels.clone();
    let mut c = mc.collision;
    let nav = c.nav.take().map(|n| (*n).clone());
    let water = c.water.take().map(|w| (*w).clone()).unwrap_or_default();
    let volumes = c.volumes.take().map(|w| (*w).clone()).unwrap_or_default();
    let climbables = c.climbables.take().map(|w| (*w).clone()).unwrap_or_default();
    Ok(PortSnapshot {
        map: map.to_string(),
        built_from: format!("dishonored_content::load_match (cache {})", dishonored_content::cache_dir().display()),
        built_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        collision: c,
        nav,
        water,
        volumes,
        climbables,
        gameplay_json,
        spawns: mc.spawns.iter().map(|s| (s.position, s.yaw)).collect(),
        levels,
    })
}

#[cfg(not(feature = "import"))]
pub fn build_snapshot(_map: &str) -> Result<PortSnapshot> {
    anyhow::bail!("this build has no map import: rebuild with the default `import` feature (cargo build -p oracle)")
}

/// The snapshot from the cache, else built (and stored).
pub fn snapshot(root: &Path, map: &str, rebuild: bool) -> Result<PortSnapshot> {
    if !rebuild {
        match load_snapshot(root, map) {
            Ok(s) => return Ok(s),
            Err(e) if cfg!(not(feature = "import")) => return Err(e),
            Err(_) => {}
        }
    }
    let s = build_snapshot(map)?;
    save_snapshot(root, &s)?;
    Ok(s)
}

/// The port's channel for a query's trace tag (`P` player movement, `J` projectile, `V`
/// vision; see the `harness collision` generator).
pub fn trace_channel(tag: &str) -> u8 {
    match tag.rsplit(':').next().unwrap_or("") {
        "J" => channel::PROJECTILE,
        "V" => channel::VISION,
        _ => channel::PAWN,
    }
}

/// The port's movers posed as when the set was answered: doors in the recorded states (else
/// their initial state), other movers where the map places them.
pub fn movers(w: &CollisionWorld, doors: &[(String, u8)]) -> Vec<CollisionWorld> {
    w.movers
        .iter()
        .map(|m| {
            let state = doors.iter().find(|d| d.0.eq_ignore_ascii_case(&m.actor)).map_or(m.initial, |d| d.1);
            m.placed_in(state, Mat4::IDENTITY)
        })
        .collect()
}

/// Run every query (or `ids`) of a set against the port, in parallel, with the original's
/// semantics (`dishonored_world::engine_check`: back-off per hit kind, movers posed).
pub fn collision_answers(w: &CollisionWorld, movers: &[CollisionWorld], set: &CollisionSet, ids: Option<&[u32]>) -> PortAnswers {
    let mut names: Vec<String> = w.sources.clone();
    let mover_base = names.len() as u32;
    names.extend(w.movers.iter().map(|m| m.actor.clone()));
    let wanted = |i: usize| ids.is_none_or(|v| v.contains(&(i as u32)));
    let hits: Vec<Option<PortHit>> = set
        .queries
        .par_iter()
        .enumerate()
        .map(|(i, q)| {
            if !wanted(i) {
                return None;
            }
            let h = dishonored_world::engine_check(w, movers, Vec3::from(q.start), Vec3::from(q.end), Vec3::from(q.extent), trace_channel(&q.tag), q.flags)?;
            let source = if h.world == 0 { w.tris.get(h.tri as usize).map_or(u32::MAX, |t| t.source) } else { mover_base + (h.world - 1) as u32 };
            Some(PortHit { time: h.time, source, kind: h.kind })
        })
        .collect();
    names.push("(source unknown)".into());
    let unknown = (names.len() - 1) as u32;
    let mut out = PortAnswers { hits, names };
    for h in out.hits.iter_mut().flatten() {
        if h.source == u32::MAX {
            h.source = unknown;
        }
    }
    out
}
