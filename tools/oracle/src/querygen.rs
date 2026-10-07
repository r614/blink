//! Collision query generation from a map's original navigation mesh: the same deterministic
//! rule as `harness collision gen` (line drops over a grid of the navmesh area, pawn-box drops
//! on the navmesh, foot / waist / head lines for movement, projectile and vision traces, and
//! pawn-box sweeps along links and out of every polygon), so captures made with either tool
//! share query ids.

use dishonored_oracle::query::{CollisionQuery, TraceProfile};
use dishonored_oracle::sampling::Primitive;
use dishonored_world::CollisionWorld;
use dishonored_world::nav::NavWorld;
use glam::Vec3;

/// Generator id stored with the query sets (bump on any change to the rule).
pub const GENERATOR: &str = "harness-collision-v1";

/// UE3 / Dishonored trace flags per trace type (COLLISION notes of the collision audit).
pub const MOVE_FLAGS: u32 = 0x0000_20DF;
pub const PISTOL_FLAGS: u32 = 0x020A_28DF;
pub const VISION_FLAGS: u32 = 0x0802_3086;
const TRACE_PAWNS: u32 = 0x1;

fn flags(trace: &str) -> u32 {
    match trace {
        "J" => PISTOL_FLAGS & !TRACE_PAWNS,
        "V" => VISION_FLAGS & !TRACE_PAWNS,
        _ => MOVE_FLAGS,
    }
}

fn q(kind: &str, trace: &str, start: Vec3, end: Vec3, extent: Vec3) -> CollisionQuery {
    CollisionQuery { tag: format!("{kind}:{trace}"), start: start.to_array(), end: end.to_array(), extent: extent.to_array(), flags: flags(trace), target: None }
}

/// Generator id of the primitive-directed queries.
pub const PRIM_GENERATOR: &str = "primitive-directed";

/// The trace profiles measured with the hook so far (COLLISION notes of the collision audit):
/// the player's movement as a line and as its box, the pistol shot and a line of sight. The
/// exe's full list comes from its trace call sites (phase 3).
pub fn measured_profiles(pawn: (f32, f32)) -> Vec<TraceProfile> {
    let p = |name: &str, tag: &str, flags: u32, extent: [f32; 3]| TraceProfile { name: name.into(), tag: tag.into(), flags, extent, call_site: None, source: "hook measurement 2026-10-07".into() };
    vec![
        p("player move (line)", "P", MOVE_FLAGS, [0.0; 3]),
        p("player move (box)", "PB", MOVE_FLAGS, [pawn.0, pawn.0, pawn.1]),
        p("pistol shot", "J", PISTOL_FLAGS & !TRACE_PAWNS, [0.0; 3]),
        p("line of sight", "V", VISION_FLAGS & !TRACE_PAWNS, [0.0; 3]),
    ]
}

/// One primitive per port collision source: the centroid and normal of its largest triangle.
pub fn port_primitives(w: &CollisionWorld) -> Vec<Primitive> {
    let mut best: Vec<Option<(f32, usize)>> = vec![None; w.sources.len()];
    for (i, t) in w.tris.iter().enumerate() {
        let area = (t.b - t.a).cross(t.c - t.a).length();
        if let Some(b) = best.get_mut(t.source as usize) {
            if b.is_none_or(|x| area > x.0) {
                *b = Some((area, i));
            }
        }
    }
    best.iter()
        .enumerate()
        .filter_map(|(s, b)| {
            let t = &w.tris[(*b)?.1];
            let c = (t.a + t.b + t.c) / 3.0;
            Some(Primitive { name: w.sources[s].clone(), point: c.to_array(), normal: t.n.to_array() })
        })
        .collect()
}

/// The query batch of a map (deterministic). `pawn`: the player's (radius, half height).
pub fn generate(nav: &NavWorld, pawn: (f32, f32)) -> Vec<CollisionQuery> {
    let ext = Vec3::new(pawn.0, pawn.0, pawn.1);
    let mut out = Vec::new();
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
            let near = nav.closest_point(Vec3::new(x, y, (lo.z + hi.z) * 0.5), 600.0).is_some()
                || nav.polys.iter().any(|p| x >= p.min.x - 300.0 && x <= p.max.x + 300.0 && y >= p.min.y - 300.0 && y <= p.max.y + 300.0);
            if near {
                out.push(q("grid", "P", Vec3::new(x, y, top), Vec3::new(x, y, bottom), Vec3::ZERO));
            }
            y += step;
        }
        x += step;
    }
    let dirs: Vec<Vec3> = (0..8)
        .map(|k| {
            let a = k as f32 * std::f32::consts::FRAC_PI_4;
            Vec3::new(a.cos(), a.sin(), 0.0)
        })
        .collect();
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
        for (k, &feet) in pts.iter().enumerate() {
            out.push(q("pdrop", "P", feet + Vec3::Z * (ext.z + 120.0), feet - Vec3::Z * 300.0, ext));
            for (j, d) in dirs.iter().enumerate() {
                if (j + k + pi) % 2 == 1 && k > 0 {
                    continue;
                }
                for (kind, h) in [("foot", 20.0), ("waist", 90.0), ("head", 160.0)] {
                    let s = feet + Vec3::Z * h;
                    out.push(q(kind, "P", s, s + *d * 600.0, Vec3::ZERO));
                }
                let s = feet + Vec3::Z * 90.0;
                out.push(q("waist", "J", s, s + *d * 1200.0, Vec3::ZERO));
                let s = feet + Vec3::Z * 160.0;
                out.push(q("head", "V", s, s + *d * 1200.0, Vec3::ZERO));
                let c = feet + Vec3::Z * (ext.z + 3.0);
                out.push(q("radial", "P", c, c + *d * 400.0, ext));
            }
        }
        for l in nav.links.get(pi).map(|v| v.as_slice()).unwrap_or(&[]) {
            let Some(to) = nav.polys.get(l.to as usize) else { continue };
            let a = p.center + Vec3::Z * (ext.z + 3.0);
            let b = to.center + Vec3::Z * (ext.z + 3.0);
            if (b - a).length() < 1.0 || (b.z - a.z).abs() > 200.0 {
                continue;
            }
            out.push(q("link", "P", a, b, ext));
        }
    }
    out
}
