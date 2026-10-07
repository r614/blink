//! Query sets and the original's answers. The first probe is collision: batches of
//! `UWorld::SingleLineCheck` lines and boxes, as `probe_launch collide` runs them
//! (research/dishonored/INSTRUMENTATION.md).
//!
//! A [`CollisionSet`] carries the queries together with the answers, so the port side never
//! regenerates the queries (a navmesh or generator change cannot silently shift query ids).

use crate::cache::{fnv64, fnv64_more};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// No name (`-` in the probe output).
pub const NO_NAME: u32 = u32::MAX;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CollisionQuery {
    /// Generator tag `<kind>:<trace>` (`grid:P`, `head:V`...).
    pub tag: String,
    pub start: [f32; 3],
    pub end: [f32; 3],
    /// Box half size; zero for a line.
    pub extent: [f32; 3],
    /// UE3 / Dishonored trace flags.
    pub flags: u32,
    /// What the query was aimed at: a primitive (`prim:<object>`), the neighbourhood of an
    /// earlier disagreement (`near:<query id>`), or nothing for spatial sampling.
    #[serde(default)]
    pub target: Option<String>,
}

/// One way the game traces: the flags and extent of a trace call site. The full list is
/// enumerated from the exe's trace call sites (phase 3, the Ghidra index); until then the
/// profiles measured with the hook are listed with `call_site: None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceProfile {
    /// `Move_Player`, `pistol`, `vision`...
    pub name: String,
    /// Trace tag used in query tags (`P`, `J`, `V`...).
    pub tag: String,
    pub flags: u32,
    pub extent: [f32; 3],
    /// Address of the trace call in `Dishonored.exe`.
    #[serde(default)]
    pub call_site: Option<u32>,
    /// Where the profile comes from (measurement, index).
    #[serde(default)]
    pub source: String,
}

/// One setting of a toggleable thing (door, mover, collision toggle, Kismet-driven state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateSetting {
    /// `<package>:<actor>`.
    pub actor: String,
    /// The state (`EDisDoorState` number, `on` / `off`, a matinee position...).
    pub state: String,
}

impl CollisionQuery {
    pub fn is_box(&self) -> bool {
        self.extent != [0.0; 3]
    }
    pub fn length(&self) -> f32 {
        let d = [self.end[0] - self.start[0], self.end[1] - self.start[1], self.end[2] - self.start[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }
    pub fn kind(&self) -> &str {
        self.tag.split(':').next().unwrap_or("")
    }
    pub fn trace(&self) -> &str {
        self.tag.split(':').nth(1).unwrap_or("")
    }
    /// FNV-1a 32 of the coordinates rounded to 0.01 uu: the same per-query hash as the
    /// `harness collision` fixtures (`Query::hash`), so both stores identify a query alike.
    pub fn coord_hash(&self) -> u32 {
        let mut h: u32 = 0x811c_9dc5;
        for v in [self.start, self.end, self.extent] {
            for c in v {
                for b in ((c * 100.0).round() as i64).to_le_bytes() {
                    h ^= b as u32;
                    h = h.wrapping_mul(0x0100_0193);
                }
            }
        }
        h
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CollisionAnswer {
    pub hit: bool,
    /// Hit fraction of the way (1 when nothing was hit).
    pub time: f32,
    pub location: [f32; 3],
    pub normal: [f32; 3],
    /// Indices into [`CollisionSet::names`] ([`NO_NAME`] when nothing was hit).
    pub actor: u32,
    pub component: u32,
}

/// Queries and the original's answers for one map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CollisionSet {
    /// Generator and version (`harness-collision-v1`).
    pub generator: String,
    pub queries: Vec<CollisionQuery>,
    /// Parallel to `queries`; `None`: not answered.
    pub answers: Vec<Option<CollisionAnswer>>,
    /// `Class/Package.TheWorld.PersistentLevel.Actor[.Component]` names of hit objects.
    pub names: Vec<String>,
    /// `DisDoor` states when the checks ran: (`<package>:<actor>`, `EDisDoorState`).
    pub doors: Vec<(String, u8)>,
    /// The world state the set was answered in (`initial`, or a state sweep's name), with the
    /// settings that define it. Each state of a sweep is its own set (and cache entry).
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub state_settings: Vec<StateSetting>,
    /// The trace profiles the queries were expanded over.
    #[serde(default)]
    pub profiles: Vec<TraceProfile>,
}

/// Hash of a query set (order, tags, coordinates to 0.01 uu, flags): the cache key's
/// `query_hash`.
pub fn query_set_hash(generator: &str, queries: &[CollisionQuery]) -> u64 {
    let mut h = fnv64(generator.as_bytes());
    for q in queries {
        h = fnv64_more(h, q.tag.as_bytes());
        h = fnv64_more(h, &q.coord_hash().to_le_bytes());
        h = fnv64_more(h, &q.flags.to_le_bytes());
    }
    h
}

impl CollisionSet {
    /// The cache key's query hash: the queries and the world state they were answered in.
    pub fn query_hash(&self) -> u64 {
        let mut h = query_set_hash(&self.generator, &self.queries);
        if !self.state.is_empty() && self.state != "initial" {
            h = fnv64_more(h, self.state.as_bytes());
            for s in &self.state_settings {
                h = fnv64_more(fnv64_more(h, s.actor.as_bytes()), s.state.as_bytes());
            }
        }
        h
    }
    pub fn name(&self, i: u32) -> &str {
        if i == NO_NAME { "-" } else { self.names.get(i as usize).map(String::as_str).unwrap_or("?") }
    }
    /// The hit actor as `<package>:<actor>` (the port's naming), from
    /// `Class/Package.TheWorld.PersistentLevel.Actor`.
    pub fn actor_key(&self, a: &CollisionAnswer) -> String {
        actor_key(self.name(a.actor))
    }
    pub fn actor_class<'a>(&'a self, a: &CollisionAnswer) -> &'a str {
        self.name(a.actor).split('/').next().unwrap_or("")
    }
    pub fn answered(&self) -> usize {
        self.answers.iter().filter(|a| a.is_some()).count()
    }
}

/// `Class/Pkg.TheWorld.PersistentLevel.Actor` -> `Pkg:Actor`.
pub fn actor_key(name: &str) -> String {
    if name == "-" {
        return "-".into();
    }
    let path = name.split('/').nth(1).unwrap_or(name);
    let pkg = path.split('.').next().unwrap_or("");
    let actor = path.rsplit('.').next().unwrap_or("");
    format!("{pkg}:{actor}")
}

/// Parse a `probe_launch collide` query file and its output into a set. Query lines:
/// `id sx sy sz ex ey ez hx hy hz flags` where `id` is `<n>[:<kind>:<trace>]`; output lines:
/// `id hit time lx ly lz nx ny nz actor component`, and `# door <pkg>:<actor> <state>` comments.
pub fn parse_probe_text(generator: &str, queries_txt: &str, results_txt: &str) -> Result<CollisionSet> {
    let mut set = CollisionSet { generator: generator.into(), state: "initial".into(), ..Default::default() };
    let mut ids: HashMap<String, usize> = HashMap::new();
    for (ln, line) in queries_txt.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 11 {
            bail!("query line {}: {} fields", ln + 1, f.len());
        }
        let v: Vec<f32> = f[1..10].iter().map(|s| s.parse()).collect::<Result<_, _>>().with_context(|| format!("query line {}", ln + 1))?;
        let num = f[0].split(':').next().unwrap_or("");
        let tag = f[0].split_once(':').map(|x| x.1).unwrap_or("").to_string();
        let flags = parse_u32(f[10])?;
        ids.insert(num.to_string(), set.queries.len());
        set.queries.push(CollisionQuery { tag, start: [v[0], v[1], v[2]], end: [v[3], v[4], v[5]], extent: [v[6], v[7], v[8]], flags, target: f.get(11).map(|s| s.to_string()) });
    }
    set.answers = vec![None; set.queries.len()];
    let mut name_ids: HashMap<String, u32> = HashMap::new();
    let mut intern = |set: &mut CollisionSet, s: &str| -> u32 {
        if s == "-" {
            return NO_NAME;
        }
        *name_ids.entry(s.to_string()).or_insert_with(|| {
            set.names.push(s.to_string());
            (set.names.len() - 1) as u32
        })
    };
    for (ln, line) in results_txt.lines().enumerate() {
        if let Some(rest) = line.strip_prefix("# door ") {
            let mut f = rest.split_whitespace();
            if let (Some(a), Some(s)) = (f.next(), f.next().and_then(|s| s.parse::<u8>().ok())) {
                set.state_settings.push(StateSetting { actor: a.to_string(), state: s.to_string() });
                set.doors.push((a.to_string(), s));
            }
            continue;
        }
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 9 {
            bail!("result line {}: {} fields", ln + 1, f.len());
        }
        let num = f[0].split(':').next().unwrap_or("");
        let Some(&qi) = ids.get(num) else { bail!("result line {}: unknown query {num}", ln + 1) };
        let n = |i: usize| -> Result<f32> { f[i].parse::<f32>().with_context(|| format!("result line {} field {i}", ln + 1)) };
        let actor = f.get(9).copied().unwrap_or("-");
        let component = f.get(10).copied().unwrap_or("-");
        let (a, c) = (intern(&mut set, actor), intern(&mut set, component));
        set.answers[qi] = Some(CollisionAnswer { hit: f[1] == "1", time: n(2)?, location: [n(3)?, n(4)?, n(5)?], normal: [n(6)?, n(7)?, n(8)?], actor: a, component: c });
    }
    Ok(set)
}

fn parse_u32(s: &str) -> Result<u32> {
    Ok(match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16)?,
        None => s.parse()?,
    })
}

/// Write the queries in the `probe_launch collide` input format.
pub fn queries_text(map: &str, queries: &[CollisionQuery]) -> String {
    use std::fmt::Write as _;
    let mut s = format!("# {map}: {} queries (id sx sy sz ex ey ez hx hy hz flags)\n", queries.len());
    for (i, x) in queries.iter().enumerate() {
        let _ = writeln!(
            s,
            "{i}:{} {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} {:#x}{}",
            x.tag, x.start[0], x.start[1], x.start[2], x.end[0], x.end[1], x.end[2], x.extent[0], x.extent[1], x.extent[2], x.flags,
            x.target.as_ref().map(|t| format!(" {}", t.replace(char::is_whitespace, "_"))).unwrap_or_default()
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q: &str = "# m: 3 queries\n0:grid:P 0 0 100 0 0 -100 0 0 0 0x20de\n1:head:V 0 0 50 100 0 50 0 0 0 0x8023086\n2:pdrop:P 0 0 200 0 0 0 35 35 88 0x20df\n";
    const R: &str = "# id hit time lx ly lz nx ny nz actor component\n# door L_X:DisDoor_3 2\n0 1 0.50000 0.00 0.00 0.00 0.0000 0.0000 1.0000 StaticMeshCollectionActor/L_X_Env.TheWorld.PersistentLevel.SMCA_1 StaticMeshComponent/L_X_Env.TheWorld.PersistentLevel.SMCA_1.SMC_4\n1 0 1.00000 0 0 0 0 0 0 - -\n";

    #[test]
    fn parse_names_doors_and_hash() {
        let s = parse_probe_text("g1", Q, R).unwrap();
        assert_eq!(s.queries.len(), 3);
        assert_eq!(s.queries[1].tag, "head:V");
        assert_eq!(s.queries[1].flags, 0x0802_3086);
        assert!(s.queries[2].is_box());
        assert_eq!(s.answered(), 2);
        let a = s.answers[0].unwrap();
        assert!(a.hit && (a.time - 0.5).abs() < 1e-6);
        assert_eq!(s.actor_key(&a), "L_X_Env:SMCA_1");
        assert_eq!(s.actor_class(&a), "StaticMeshCollectionActor");
        assert_eq!(s.answers[1].unwrap().actor, NO_NAME);
        assert!(s.answers[2].is_none());
        assert_eq!(s.doors, vec![("L_X:DisDoor_3".to_string(), 2)]);
        // The set hash depends on the queries, not the answers.
        let h = s.query_hash();
        let mut t = s.clone();
        t.answers[0] = None;
        assert_eq!(t.query_hash(), h);
        t.queries[0].end[2] += 0.01;
        assert_ne!(t.query_hash(), h);
        // Round trip of the probe input format.
        let txt = queries_text("m", &s.queries);
        let back = parse_probe_text("g1", &txt, "").unwrap();
        assert_eq!(back.queries, s.queries);
    }

    #[test]
    fn coord_hash_matches_harness_rule() {
        // (0,0,0)->(0,0,0) ext 0: nine zero i64s.
        let q = CollisionQuery { tag: String::new(), start: [0.0; 3], end: [0.0; 3], extent: [0.0; 3], flags: 0, target: None };
        let mut h: u32 = 0x811c_9dc5;
        for _ in 0..9 * 8 {
            h ^= 0;
            h = h.wrapping_mul(0x0100_0193);
        }
        assert_eq!(q.coord_hash(), h);
    }
}
