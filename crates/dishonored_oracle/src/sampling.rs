//! Query sampling beyond spatial grids:
//! - **primitive-directed**: every collision primitive (a kDOP triangle group, a convex element,
//!   a volume) is hit from several directions, so no object can be missed by the sampling;
//! - **profile expansion**: every query shape runs with every trace profile the exe uses
//!   ([`TraceProfile`], enumerated from the trace call sites);
//! - **adaptive densification**: new queries around each disagreement, so a residual's extent
//!   and cause are measured rather than guessed.
//!
//! Everything is deterministic (fixed directions, a fixed-seed generator), so a query set's
//! hash identifies it and cached answers stay valid.

use crate::query::{CollisionQuery, TraceProfile};

/// A primitive to aim at: a stable name (`<package>:<actor>.<component>[#element]`), a point
/// on it and its outward normal (zero when unknown).
#[derive(Debug, Clone, PartialEq)]
pub struct Primitive {
    pub name: String,
    pub point: [f32; 3],
    pub normal: [f32; 3],
}

fn add(a: [f32; 3], b: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s]
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-6 { [0.0, 0.0, 1.0] } else { [v[0] / l, v[1] / l, v[2] / l] }
}

/// Directions to approach a primitive from: against its normal, the four 45-degree tilts
/// around it, plus straight down and horizontally from the four sides when the normal is
/// unknown.
pub fn approach_dirs(normal: [f32; 3]) -> Vec<[f32; 3]> {
    if normal == [0.0; 3] {
        return vec![[0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let n = norm(normal);
    // A tangent basis.
    let t = if n[2].abs() < 0.9 { norm([-n[1], n[0], 0.0]) } else { [1.0, 0.0, 0.0] };
    let b = [n[1] * t[2] - n[2] * t[1], n[2] * t[0] - n[0] * t[2], n[0] * t[1] - n[1] * t[0]];
    let mut out = vec![[-n[0], -n[1], -n[2]]];
    for (u, s) in [(t, 1.0), (t, -1.0), (b, 1.0), (b, -1.0)] {
        out.push(norm([-n[0] + u[0] * s, -n[1] + u[1] * s, -n[2] + u[2] * s]));
    }
    out
}

/// Lines (and the profiles' boxes) through every primitive from every approach direction:
/// from `reach` uu before the point to `reach` uu past it.
pub fn primitive_directed(prims: &[Primitive], profiles: &[TraceProfile], reach: f32) -> Vec<CollisionQuery> {
    let mut out = Vec::new();
    for p in prims {
        for d in approach_dirs(p.normal) {
            for pr in profiles {
                let start = add(p.point, d, -reach - pr.extent[0].max(pr.extent[2]));
                out.push(CollisionQuery { tag: format!("prim:{}", pr.tag), start, end: add(p.point, d, reach), extent: pr.extent, flags: pr.flags, target: Some(format!("prim:{}", p.name)) });
            }
        }
    }
    out
}

/// Every query shape with every profile: the query's start and end, the profile's flags and
/// extent (tag `<kind>:<profile tag>`).
pub fn expand_profiles(shapes: &[CollisionQuery], profiles: &[TraceProfile]) -> Vec<CollisionQuery> {
    let mut out = Vec::with_capacity(shapes.len() * profiles.len());
    for q in shapes {
        for p in profiles {
            out.push(CollisionQuery { tag: format!("{}:{}", q.kind(), p.tag), extent: p.extent, flags: p.flags, ..q.clone() });
        }
    }
    out
}

/// A small deterministic generator (SplitMix64).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [-1, 1).
    pub fn signed(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

/// `per` queries around each listed query: start and end jittered by up to `radius` uu in each
/// axis, same profile. Tagged `dense:<trace>`, targeted `near:<id>`.
pub fn densify(queries: &[CollisionQuery], ids: &[u32], per: usize, radius: f32, seed: u64) -> Vec<CollisionQuery> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(ids.len() * per);
    for &id in ids {
        let Some(q) = queries.get(id as usize) else { continue };
        for _ in 0..per {
            let j = [rng.signed() * radius, rng.signed() * radius, rng.signed() * radius];
            out.push(CollisionQuery { tag: format!("dense:{}", q.trace()), start: add(q.start, j, 1.0), end: add(q.end, j, 1.0), target: Some(format!("near:{id}")), ..q.clone() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles() -> Vec<TraceProfile> {
        vec![
            TraceProfile { name: "move".into(), tag: "P".into(), flags: 0x20df, extent: [35.0, 35.0, 88.0], call_site: None, source: "hook".into() },
            TraceProfile { name: "vision".into(), tag: "V".into(), flags: 0x0802_3086, extent: [0.0; 3], call_site: None, source: "hook".into() },
        ]
    }

    #[test]
    fn primitive_queries_pass_through_the_primitive() {
        let prims = vec![Primitive { name: "L:Wall.C0".into(), point: [100.0, 0.0, 50.0], normal: [-1.0, 0.0, 0.0] }];
        let q = primitive_directed(&prims, &profiles(), 64.0);
        assert_eq!(q.len(), 5 * 2);
        for x in &q {
            // The segment passes through the point.
            let (s, e, p) = (x.start, x.end, [100.0f32, 0.0, 50.0]);
            let d = [e[0] - s[0], e[1] - s[1], e[2] - s[2]];
            let t = ((p[0] - s[0]) * d[0] + (p[1] - s[1]) * d[1] + (p[2] - s[2]) * d[2]) / (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
            let c = [s[0] + d[0] * t - p[0], s[1] + d[1] * t - p[1], s[2] + d[2] * t - p[2]];
            assert!(c.iter().all(|v| v.abs() < 1e-3) && t > 0.0 && t < 1.0);
            assert_eq!(x.target.as_deref(), Some("prim:L:Wall.C0"));
        }
        // The first direction goes against the normal (into the wall from outside).
        assert!(q[0].end[0] > q[0].start[0]);
        assert_eq!(approach_dirs([0.0; 3]).len(), 6);
    }

    #[test]
    fn profiles_and_densify_are_deterministic() {
        let shape = CollisionQuery { tag: "foot:P".into(), start: [0.0; 3], end: [100.0, 0.0, 0.0], extent: [0.0; 3], flags: 1, target: None };
        let e = expand_profiles(std::slice::from_ref(&shape), &profiles());
        assert_eq!(e.iter().map(|q| q.tag.as_str()).collect::<Vec<_>>(), vec!["foot:P", "foot:V"]);
        assert_eq!(e[0].extent, [35.0, 35.0, 88.0]);
        let a = densify(&e, &[1], 4, 8.0, 7);
        let b = densify(&e, &[1], 4, 8.0, 7);
        assert_eq!(a, b);
        assert_eq!(a.len(), 4);
        assert!(a.iter().all(|q| q.tag == "dense:V" && q.target.as_deref() == Some("near:1") && q.start.iter().all(|v| v.abs() <= 8.0)));
    }
}
