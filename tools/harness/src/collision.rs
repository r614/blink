//! Collision against the original (research/dishonored/COLLISION.md).
//!
//! ```text
//! harness collision [--map M]...              re-check the port against the stored oracle
//!                                             results (fixtures/collision/<map>.oracle)
//! harness collision gen --map M --queries F   write the query batch for `probe_launch collide`
//! harness collision check --map M --queries F --results R [--store]
//!                                             compare the port with an oracle run, cluster the
//!                                             disagreements; --store writes the fixture
//! ```
//!
//! The queries are generated deterministically from the map's original navigation mesh: line
//! drops over a grid of the navmesh area, pawn-box drops on the navmesh, horizontal lines at
//! foot / waist / head height on the player's movement, projectile and vision trace types, and
//! pawn-box sweeps along the navmesh links and out of every polygon. The stored fixture holds
//! only the query id, a hash of its coordinates, the oracle's hit flag and hit fraction.

use crate::Ctx;
use crate::report::Report;
use anyhow::{Context, Result, bail};
use dishonored_content::{MatchContent, load_match};
use dishonored_world::{CollisionWorld, PawnBox, WorldQuery, channel, hit_kind};
use glam::Vec3;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

const S: &str = "Collision vs the original";

/// Trace types of the queries, with the UE3 / Dishonored trace flags the original uses for
/// them (see COLLISION.md "Trace flags") and the port's channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Chan {
    /// The player's movement (`Move_Player`).
    Player,
    Projectile,
    Vision,
}

impl Chan {
    pub fn flags(self) -> u32 {
        match self {
            // The player pawn's own movement checks (`MoveActor`), pawns left out so the
            // living NPCs of the original do not count.
            Chan::Player => MOVE_FLAGS & !TRACE_PAWNS,
            Chan::Projectile => PISTOL_FLAGS & !TRACE_PAWNS,
            Chan::Vision => VISION_FLAGS & !TRACE_PAWNS,
        }
    }
    pub fn channel(self) -> u8 {
        match self {
            Chan::Player => channel::PAWN,
            Chan::Projectile => channel::PROJECTILE,
            Chan::Vision => channel::VISION,
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Chan::Player => "P",
            Chan::Projectile => "J",
            Chan::Vision => "V",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "P" => Chan::Player,
            "J" => Chan::Projectile,
            "V" => Chan::Vision,
            _ => return None,
        })
    }
}

const TRACE_PAWNS: u32 = 0x1;
/// `UWorld::MoveActor` of the player pawn (measured with the engine hook, COLLISION.md).
pub const MOVE_FLAGS: u32 = 0x0000_20DF;
/// The pistol's shot trace (0x00C2A3E0 -> 0x00BE1560): projectile trace type.
pub const PISTOL_FLAGS: u32 = 0x020A_28DF;
/// A line of sight check: vision trace type.
pub const VISION_FLAGS: u32 = 0x0802_3086;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    GridDrop,
    PawnDrop,
    Foot,
    Waist,
    Head,
    LinkSweep,
    RadialSweep,
}

impl Kind {
    fn tag(self) -> &'static str {
        match self {
            Kind::GridDrop => "grid",
            Kind::PawnDrop => "pdrop",
            Kind::Foot => "foot",
            Kind::Waist => "waist",
            Kind::Head => "head",
            Kind::LinkSweep => "link",
            Kind::RadialSweep => "radial",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "grid" => Kind::GridDrop,
            "pdrop" => Kind::PawnDrop,
            "foot" => Kind::Foot,
            "waist" => Kind::Waist,
            "head" => Kind::Head,
            "link" => Kind::LinkSweep,
            "radial" => Kind::RadialSweep,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Query {
    pub kind: Kind,
    pub chan: Chan,
    pub start: Vec3,
    pub end: Vec3,
    pub extent: Vec3,
}

impl Query {
    pub fn name(&self, i: usize) -> String {
        format!("{i}:{}:{}", self.kind.tag(), self.chan.tag())
    }
    /// FNV-1a of the coordinates rounded to 0.01 uu (detects a generator change).
    pub fn hash(&self) -> u32 {
        let mut h: u32 = 0x811c_9dc5;
        for v in [self.start, self.end, self.extent] {
            for c in v.to_array() {
                for b in ((c * 100.0).round() as i64).to_le_bytes() {
                    h ^= b as u32;
                    h = h.wrapping_mul(0x0100_0193);
                }
            }
        }
        h
    }
}

/// The player's collision box (the cylinder's extent).
fn pawn_extent(mc: &MatchContent) -> Vec3 {
    let mv = &mc.sim.gameplay.movement;
    Vec3::new(mv.collision_radius, mv.collision_radius, mv.collision_half_height)
}

/// The query batch of a map (deterministic).
pub fn generate(mc: &MatchContent) -> Result<Vec<Query>> {
    let nav = mc.collision.nav().context("map without navigation mesh")?;
    let ext = pawn_extent(mc);
    let mut q = Vec::new();
    // Grid of line drops over the navmesh area, from above everything to below it.
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &nav.polys {
        lo = lo.min(p.min);
        hi = hi.max(p.max);
    }
    let step = 192.0;
    let (top, bottom) = (hi.z + 3000.0, lo.z - 1500.0);
    let mut x = (lo.x / step).floor() * step;
    while x <= hi.x {
        let mut y = (lo.y / step).floor() * step;
        while y <= hi.y {
            // Only columns within 600 uu of the navmesh (the playable area).
            let near = nav.closest_point(Vec3::new(x, y, (lo.z + hi.z) * 0.5), 600.0).is_some()
                || nav.polys.iter().any(|p| x >= p.min.x - 300.0 && x <= p.max.x + 300.0 && y >= p.min.y - 300.0 && y <= p.max.y + 300.0);
            if near {
                q.push(Query { kind: Kind::GridDrop, chan: Chan::Player, start: Vec3::new(x, y, top), end: Vec3::new(x, y, bottom), extent: Vec3::ZERO });
            }
            y += step;
        }
        x += step;
    }
    // Per navmesh polygon: its centre and (large polygons) a few points between the centre and
    // its corners.
    let dirs: Vec<Vec3> = (0..8).map(|k| { let a = k as f32 * std::f32::consts::FRAC_PI_4; Vec3::new(a.cos(), a.sin(), 0.0) }).collect();
    for (pi, p) in nav.polys.iter().enumerate() {
        let mut pts = vec![p.center];
        let span = (p.max - p.min).truncate().length();
        if span > 600.0 {
            for &v in p.verts.iter().step_by(2) {
                let c = nav.verts[v as usize];
                let m = p.center.lerp(c, 0.6);
                pts.push(Vec3::new(m.x, m.y, nav.poly_z(pi as u32, m.x, m.y)));
            }
        }
        for (k, &pt) in pts.iter().enumerate() {
            let feet = pt;
            // Pawn-box drop onto the polygon.
            q.push(Query { kind: Kind::PawnDrop, chan: Chan::Player, start: feet + Vec3::Z * (ext.z + 120.0), end: feet - Vec3::Z * 300.0, extent: ext });
            // Lines at foot, waist and head height (alternate directions per point).
            for (j, d) in dirs.iter().enumerate() {
                if (j + k + pi) % 2 == 1 && k > 0 {
                    continue;
                }
                for (kind, h) in [(Kind::Foot, 20.0), (Kind::Waist, 90.0), (Kind::Head, 160.0)] {
                    let s = feet + Vec3::Z * h;
                    q.push(Query { kind, chan: Chan::Player, start: s, end: s + *d * 600.0, extent: Vec3::ZERO });
                }
                let s = feet + Vec3::Z * 90.0;
                q.push(Query { kind: Kind::Waist, chan: Chan::Projectile, start: s, end: s + *d * 1200.0, extent: Vec3::ZERO });
                let s = feet + Vec3::Z * 160.0;
                q.push(Query { kind: Kind::Head, chan: Chan::Vision, start: s, end: s + *d * 1200.0, extent: Vec3::ZERO });
                // Pawn sweep out of the polygon (walls, railings, ledges, windows, fences).
                let c = feet + Vec3::Z * (ext.z + 3.0);
                q.push(Query { kind: Kind::RadialSweep, chan: Chan::Player, start: c, end: c + *d * 400.0, extent: ext });
            }
        }
        // Pawn sweeps along the links to the neighbours (doorways, stairs, edges).
        for l in nav.links.get(pi).map(|v| v.as_slice()).unwrap_or(&[]) {
            let Some(to) = nav.polys.get(l.to as usize) else { continue };
            let a = p.center + Vec3::Z * (ext.z + 3.0);
            let b = to.center + Vec3::Z * (ext.z + 3.0);
            if (b - a).length() < 1.0 || (b.z - a.z).abs() > 200.0 {
                continue;
            }
            q.push(Query { kind: Kind::LinkSweep, chan: Chan::Player, start: a, end: b, extent: ext });
        }
    }
    Ok(q)
}

/// The port's answer: the hit fraction of the way (`None`: nothing in the way).
pub fn port_check(w: &CollisionWorld, q: &Query) -> Option<f32> {
    port_hit(w, &[], q).map(|h| h.0)
}

/// The port's first hit over the static world and the movers: the fraction the original would
/// report (its back-off from the surface applied when the check is not `TRACE_Accurate`), the
/// kind of the hit triangle and its source.
pub fn port_hit(w: &CollisionWorld, movers: &[CollisionWorld], q: &Query) -> Option<(f32, u8, String)> {
    let d = q.end - q.start;
    let len = d.length();
    if len < 1e-3 {
        return None;
    }
    let one = |c: &CollisionWorld| -> Option<(f32, u32)> {
        if q.extent == Vec3::ZERO {
            c.raycast(q.start, d / len, len, q.chan.channel()).map(|h| (h.distance / len, h.tri))
        } else {
            let b = PawnBox { center: q.start, half: q.extent };
            c.sweep_box(&b, d, q.chan.channel()).map(|h| (h.fraction, h.tri))
        }
    };
    let mut best: Option<(f32, u8, String)> = None;
    for c in std::iter::once(w).chain(movers) {
        if let Some((t, tri)) = one(c) {
            if best.as_ref().is_none_or(|b| t < b.0) {
                let (kind, src) = match c.tris.get(tri as usize) {
                    Some(x) => (c.kind_of(tri), c.sources.get(x.source as usize).cloned().unwrap_or_default()),
                    None => (hit_kind::OTHER, String::new()),
                };
                best = Some((t, kind, src));
            }
        }
    }
    let accurate = q.chan.flags() & 0x8_0000 != 0;
    best.map(|(t, k, s)| (if accurate { t } else { hit_kind::engine_time(k, t, len) }, k, s))
}

/// One oracle answer: hit, fraction, and (from a probe run only) the hit component's name.
#[derive(Clone, Debug)]
pub struct Oracle {
    pub hit: bool,
    pub time: f32,
    pub what: String,
}

fn read_results(path: &PathBuf) -> Result<BTreeMap<usize, Oracle>> {
    let mut out = BTreeMap::new();
    for line in std::fs::read_to_string(path)?.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let id: usize = f[0].split(':').next().unwrap_or("").parse()?;
        let what = if f.len() > 10 { format!("{} {}", f[9], f[10]) } else { String::new() };
        out.insert(id, Oracle { hit: f[1] == "1", time: f[2].parse()?, what });
    }
    Ok(out)
}

fn fixture_path(map: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/collision").join(format!("{map}.oracle"))
}

/// Stored oracle: `id hash hit time`.
fn read_fixture(map: &str, queries: &[Query]) -> Result<BTreeMap<usize, Oracle>> {
    let p = fixture_path(map);
    let text = std::fs::read_to_string(&p).with_context(|| format!("{}", p.display()))?;
    let mut out = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let id: usize = f[0].parse()?;
        let hash = u32::from_str_radix(f[1], 16)?;
        let Some(q) = queries.get(id) else { bail!("fixture query {id} out of range") };
        if q.hash() != hash {
            bail!("query {id} differs from the stored one (generator or navmesh changed)");
        }
        out.insert(id, Oracle { hit: f[2] == "1", time: f[3].parse()?, what: String::new() });
    }
    Ok(out)
}

fn write_fixture(map: &str, queries: &[Query], oracle: &BTreeMap<usize, Oracle>) -> Result<PathBuf> {
    let p = fixture_path(map);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let mut s = format!("# {map}: oracle results of `harness collision gen` queries (UWorld::SingleLineCheck in the original)\n# id coord_hash hit fraction\n");
    for (id, o) in oracle {
        let _ = writeln!(s, "{id} {:08x} {} {:.4}", queries[*id].hash(), o.hit as u8, o.time);
    }
    std::fs::write(&p, s)?;
    Ok(p)
}

/// The port's world as the map starts: static collision plus the movers where they are placed.
fn start_world(mc: &MatchContent) -> Vec<dishonored_world::CollisionWorld> {
    mc.collision.movers.iter().map(|m| m.placed(glam::Mat4::IDENTITY)).collect()
}

/// Agreement of one query: equal hit flags, and both hits within `tol` uu along the way.
fn agrees(q: &Query, o: &Oracle, port: Option<f32>) -> bool {
    let len = (q.end - q.start).length();
    let tol = if q.extent == Vec3::ZERO { 2.0 } else { 4.0 };
    match (o.hit, port) {
        (false, None) => true,
        (true, Some(t)) => ((t - o.time) * len).abs() <= tol,
        _ => false,
    }
}

pub struct Stats {
    pub total: usize,
    pub agree: usize,
    pub by_kind: BTreeMap<(Kind, Chan), (usize, usize)>,
    /// Disagreements: (query id, oracle, port).
    pub diffs: Vec<(usize, Oracle, Option<f32>)>,
}

pub fn compare(mc: &MatchContent, queries: &[Query], oracle: &BTreeMap<usize, Oracle>) -> Stats {
    let movers = start_world(mc);
    let mut st = Stats { total: 0, agree: 0, by_kind: BTreeMap::new(), diffs: Vec::new() };
    for (id, o) in oracle {
        let Some(q) = queries.get(*id) else { continue };
        let p = port_hit(&mc.collision, &movers, q).map(|h| h.0);
        let ok = agrees(q, o, p);
        st.total += 1;
        let e = st.by_kind.entry((q.kind, q.chan)).or_default();
        e.0 += 1;
        if ok {
            st.agree += 1;
            e.1 += 1;
        } else {
            st.diffs.push((*id, o.clone(), p));
        }
    }
    st
}

fn pct(a: usize, n: usize) -> String {
    if n == 0 { "-".into() } else { format!("{:.2} %", 100.0 * a as f64 / n as f64) }
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let sub = ctx.args.get(1).map(String::as_str).unwrap_or("");
    let maps: Vec<String> = {
        let v: Vec<String> = ctx.args.windows(2).filter(|w| w[0] == "--map").map(|w| w[1].clone()).collect();
        if v.is_empty() { vec!["L_Boyle_Ext_P".into(), "L_Boyle_Int_P".into()] } else { v }
    };
    match sub {
        "gen" => {
            let map = &maps[0];
            let mc = load_match(map)?;
            let q = generate(&mc)?;
            let path = PathBuf::from(ctx.opt("--queries").context("--queries FILE")?);
            let mut s = format!("# {map}: {} queries (id sx sy sz ex ey ez hx hy hz flags)\n", q.len());
            for (i, x) in q.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "{} {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} {:.2} {:#x}",
                    x.name(i), x.start.x, x.start.y, x.start.z, x.end.x, x.end.y, x.end.z, x.extent.x, x.extent.y, x.extent.z, x.chan.flags()
                );
            }
            std::fs::write(&path, s)?;
            println!("{} queries -> {}", q.len(), path.display());
            return Ok(());
        }
        "probe" => {
            // harness collision probe --map M sx sy sz ex ey ez [hx hy hz]: the port's first hit
            // with its source and triangle.
            let map = &maps[0];
            let mc = load_match(map)?;
            let v: Vec<f32> = ctx.args.iter().skip(2).filter_map(|s| s.parse().ok()).collect();
            let (s, e) = (Vec3::new(v[0], v[1], v[2]), Vec3::new(v[3], v[4], v[5]));
            let ext = if v.len() >= 9 { Vec3::new(v[6], v[7], v[8]) } else { Vec3::ZERO };
            for chan in [Chan::Player, Chan::Projectile, Chan::Vision] {
                let q = Query { kind: Kind::GridDrop, chan, start: s, end: e, extent: ext };
                let c = &mc.collision;
                let d = e - s;
                if ext == Vec3::ZERO {
                    match c.raycast(s, d.normalize(), d.length(), chan.channel()) {
                        Some(h) => {
                            let t = &c.tris[h.tri as usize];
                            println!("{chan:?}: t {:.5} at {:.2?} n {:.3?} src {} tri {:.1?} {:.1?} {:.1?}", h.distance / d.length(), h.point, h.normal, c.sources[t.source as usize], t.a, t.b, t.c);
                        }
                        None => println!("{chan:?}: miss"),
                    }
                } else {
                    println!("{chan:?}: {:?} src {:?}", port_check(c, &q), port_source(&mc, &[], &q));
                }
            }
            return Ok(());
        }
        "check" => {
            let map = &maps[0];
            let mc = load_match(map)?;
            let q = generate(&mc)?;
            let res = read_results(&PathBuf::from(ctx.opt("--results").context("--results FILE")?))?;
            let st = compare(&mc, &q, &res);
            report(rep, map, &st, &q);
            clusters(ctx, map, &st, &q, &mc);
            if ctx.args.iter().any(|a| a == "--store") {
                let p = write_fixture(map, &q, &res)?;
                println!("stored {}", p.display());
            }
            return Ok(());
        }
        _ => {}
    }
    for map in &maps {
        let mc = load_match(map)?;
        let q = generate(&mc)?;
        let oracle = match read_fixture(map, &q) {
            Ok(o) => o,
            Err(e) => {
                rep.check(S, &format!("{map}: stored oracle"), false, format!("{e:#}"));
                continue;
            }
        };
        let st = compare(&mc, &q, &oracle);
        report(rep, map, &st, &q);
        let min = MIN_AGREEMENT.iter().find(|m| m.0 == map).map(|m| m.1).unwrap_or(0.99);
        let rate = st.agree as f64 / st.total.max(1) as f64;
        rep.check(S, &format!("{map}: agreement with the original"), rate >= min, format!("{} of {} ({}), at least {:.2} %", st.agree, st.total, pct(st.agree, st.total), min * 100.0));
    }
    Ok(())
}

/// The agreement each map reached (COLLISION.md); a lower rate is a regression.
const MIN_AGREEMENT: &[(&str, f64)] = &[("L_Boyle_Ext_P", 0.0), ("L_Boyle_Int_P", 0.0)];

fn report(rep: &mut Report, map: &str, st: &Stats, _q: &[Query]) {
    let rows = st
        .by_kind
        .iter()
        .map(|((k, c), (n, a))| vec![k.tag().to_string(), c.tag().to_string(), n.to_string(), a.to_string(), pct(*a, *n)])
        .collect();
    rep.table(&format!("{map}: agreement per query kind"), &["kind", "trace", "queries", "agree", "rate"], rows);
    println!("{map}: {} / {} agree ({})", st.agree, st.total, pct(st.agree, st.total));
}

/// Group the disagreements by what the original hit and what the port hit, with examples.
fn clusters(ctx: &Ctx, map: &str, st: &Stats, q: &[Query], mc: &MatchContent) {
    let movers = start_world(mc);
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (id, o, p) in &st.diffs {
        let x = &q[*id];
        let port_src = port_source(mc, &movers, x).unwrap_or_else(|| "-".into());
        let key = match (o.hit, p) {
            (true, None) => format!("orig hit {} / port miss", o.what),
            (false, Some(_)) => format!("orig miss / port hit {port_src}"),
            (true, Some(t)) => format!("{} {} / port {port_src}", if *t < o.time { "port nearer" } else { "port farther" }, o.what),
            _ => "?".into(),
        };
        groups.entry(key).or_default().push(*id);
    }
    let mut v: Vec<(String, Vec<usize>)> = groups.into_iter().collect();
    v.sort_by_key(|g| std::cmp::Reverse(g.1.len()));
    let mut s = String::new();
    for (k, ids) in &v {
        let _ = writeln!(s, "{:>6}  {k}", ids.len());
        for &i in ids.iter().take(3) {
            let x = &q[i];
            let o = st.diffs.iter().find(|d| d.0 == i).unwrap();
            let _ = writeln!(s, "          {} {:.0?} -> {:.0?} ext {:.0?} orig {} {:.3} port {:?}", x.name(i), x.start, x.end, x.extent, o.1.hit as u8, o.1.time, o.2);
        }
    }
    let path = ctx.out.join(format!("{map}_clusters.txt"));
    std::fs::write(&path, &s).ok();
    println!("{}", s.lines().take(80).collect::<Vec<_>>().join("\n"));
    println!("clusters: {}", path.display());
}

/// The port source (instance) a query hits first.
fn port_source(mc: &MatchContent, movers: &[CollisionWorld], q: &Query) -> Option<String> {
    port_hit(&mc.collision, movers, q).map(|h| h.2)
}
