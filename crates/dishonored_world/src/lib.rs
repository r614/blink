//! Collision world and spatial queries (M5).
//!
//! Geometry comes from the per-poly kDOP collision triangles cooked into every
//! `UStaticMesh` (falling back to render triangles for meshes without them), transformed
//! by each colliding instance. Coordinates: Dishonored world space (Z-up, uu).
//!
//! Character shape: the original pawn uses a `CylinderComponent` (radius 31, half-height
//! 87.5). This crate models it as a capsule of the same radius and half-height. The
//! difference matters at ledge corners and is tracked as UNKNOWNS U-016.

pub mod bvh;
pub mod geom;

use anyhow::Result;
use bvh::Bvh;
use dishonored_ir::DishonoredMap;
use geom::{Aabb, closest_segment_triangle, ray_triangle};
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Gameplay trace channels, mirroring Dishonored's `DisPrimTraceMask`.
pub mod channel {
    pub const PAWN: u8 = 1 << 0;
    pub const CROSSHAIR: u8 = 1 << 1;
    pub const PROJECTILE: u8 = 1 << 2;
    pub const MELEE: u8 = 1 << 3;
    pub const VISION: u8 = 1 << 4;
    pub const ALL: u8 = 0x1F;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Tri {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
    pub n: Vec3,
    pub channels: u8,
    /// Index into [`CollisionWorld::sources`] (instance that produced the triangle).
    pub source: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub distance: f32,
    pub point: Vec3,
    pub normal: Vec3,
    pub tri: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capsule {
    pub center: Vec3,
    pub radius: f32,
    /// Half of the full height, caps included (UE `CollisionHeight` convention).
    pub half_height: f32,
}

impl Capsule {
    pub fn segment(&self) -> (Vec3, Vec3) {
        let h = (self.half_height - self.radius).max(0.0);
        (self.center - Vec3::Z * h, self.center + Vec3::Z * h)
    }
    pub fn aabb(&self) -> Aabb {
        Aabb {
            min: self.center - Vec3::new(self.radius, self.radius, self.half_height),
            max: self.center + Vec3::new(self.radius, self.radius, self.half_height),
        }
    }
    pub fn at(&self, center: Vec3) -> Capsule {
        Capsule { center, ..*self }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    /// Push-out direction (from surface toward the capsule).
    pub normal: Vec3,
    pub depth: f32,
    pub point: Vec3,
    pub tri: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepHit {
    /// Fraction of the requested move that is free (0..1).
    pub fraction: f32,
    pub normal: Vec3,
    pub point: Vec3,
    /// True when the capsule already overlapped at the start.
    pub start_penetrating: bool,
    /// Most upward-facing triangle face normal among the contacts. Used for walkability
    /// so a rounded capsule bottom behaves like UE3's flat-bottomed cylinder on edges.
    pub face_up: Vec3,
}

/// The query surface the simulation is written against.
pub trait WorldQuery {
    fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32, channels: u8) -> Option<RayHit>;
    fn capsule_contacts(&self, cap: &Capsule, channels: u8, out: &mut Vec<Contact>);
    fn sweep_capsule(&self, cap: &Capsule, delta: Vec3, channels: u8) -> Option<SweepHit>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollisionWorld {
    pub tris: Vec<Tri>,
    pub bvh: Bvh,
    pub sources: Vec<String>,
    pub bounds: Option<Aabb>,
}

const SKIN: f32 = 0.05;

impl CollisionWorld {
    pub fn from_triangles(tris: Vec<Tri>, sources: Vec<String>) -> Self {
        let bounds: Vec<Aabb> = tris
            .iter()
            .map(|t| {
                let mut b = Aabb::EMPTY;
                b.grow(t.a);
                b.grow(t.b);
                b.grow(t.c);
                b
            })
            .collect();
        let total = bounds.iter().fold(Aabb::EMPTY, |acc, b| acc.union(b));
        let bvh = Bvh::build(&bounds);
        CollisionWorld { tris, bvh, sources, bounds: Some(total) }
    }

    /// Build from an imported map: every colliding instance (hidden blockers included).
    pub fn from_map(map: &DishonoredMap) -> Self {
        let mut tris = Vec::new();
        let mut sources = Vec::new();
        for inst in map.instances.iter().filter(|i| i.collides) {
            let mesh = &map.meshes[inst.mesh];
            let src = sources.len() as u32;
            sources.push(format!("{} <- {}", mesh.source, inst.source));
            let xf = inst.transform;
            let mirrored = xf.determinant() < 0.0;
            let mut push = |i0: u32, i1: u32, i2: u32| {
                let (Some(a), Some(b), Some(c)) = (
                    mesh.positions.get(i0 as usize),
                    mesh.positions.get(i1 as usize),
                    mesh.positions.get(i2 as usize),
                ) else {
                    return;
                };
                let (a, b, c) = (xf.transform_point3(*a), xf.transform_point3(*b), xf.transform_point3(*c));
                let (b, c) = if mirrored { (c, b) } else { (b, c) };
                let n = (b - a).cross(c - a);
                if n.length_squared() < 1e-10 {
                    return;
                }
                tris.push(Tri { a, b, c, n: n.normalize(), channels: channel::ALL, source: src });
            };
            if !mesh.collision_triangles.is_empty() {
                for t in &mesh.collision_triangles {
                    push(t[0], t[1], t[2]);
                }
            } else {
                for s in mesh.sections.iter().filter(|s| s.enable_collision) {
                    let start = s.first_index as usize;
                    let end = (start + s.num_triangles as usize * 3).min(mesh.indices.len());
                    for t in mesh.indices[start..end].chunks_exact(3) {
                        push(t[0], t[1], t[2]);
                    }
                }
            }
        }
        Self::from_triangles(tris, sources)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let bytes = bincode::serde::encode_to_vec(self, bincode::config::standard())?;
        std::fs::write(path, bytes)?;
        Ok(())
    }

    pub fn load(path: &std::path::Path) -> Result<Self> {
        let bytes = std::fs::read(path)?;
        let (w, _) = bincode::serde::decode_from_slice(&bytes, bincode::config::standard())?;
        Ok(w)
    }

    fn face_up(&self, contacts: &[Contact]) -> Vec3 {
        contacts
            .iter()
            .map(|c| { let n = self.tris[c.tri as usize].n; if n.dot(c.normal) < 0.0 { -n } else { n } })
            .max_by(|a, b| a.z.total_cmp(&b.z))
            .unwrap_or(Vec3::Z)
    }

    /// Push a capsule out of all penetrations (a few relaxation iterations).
    pub fn depenetrate(&self, cap: &Capsule, channels: u8) -> Vec3 {
        let mut c = *cap;
        let mut contacts = Vec::new();
        for _ in 0..8 {
            contacts.clear();
            self.capsule_contacts(&c, channels, &mut contacts);
            let Some(deepest) = contacts.iter().max_by(|a, b| a.depth.total_cmp(&b.depth)) else {
                break;
            };
            if deepest.depth <= SKIN * 0.5 {
                break;
            }
            c.center += deepest.normal * (deepest.depth + SKIN * 0.5);
        }
        c.center
    }
}

impl WorldQuery for CollisionWorld {
    fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32, channels: u8) -> Option<RayHit> {
        let d = dir.normalize_or_zero();
        if d == Vec3::ZERO {
            return None;
        }
        let mut best: Option<RayHit> = None;
        self.bvh.query_ray(origin, d, max_dist, |i, max_t| {
            let t = &self.tris[i as usize];
            if t.channels & channels == 0 {
                return max_t;
            }
            if let Some(dist) = ray_triangle(origin, d, t.a, t.b, t.c) {
                if dist <= max_t {
                    let n = if t.n.dot(d) > 0.0 { -t.n } else { t.n };
                    best = Some(RayHit { distance: dist, point: origin + d * dist, normal: n, tri: i });
                    return dist;
                }
            }
            max_t
        });
        best
    }

    fn capsule_contacts(&self, cap: &Capsule, channels: u8, out: &mut Vec<Contact>) {
        let (p0, p1) = cap.segment();
        let r = cap.radius;
        self.bvh.query_aabb(&cap.aabb().expand(SKIN), |i| {
            let t = &self.tris[i as usize];
            if t.channels & channels == 0 {
                return;
            }
            let (s, q) = closest_segment_triangle(p0, p1, t.a, t.b, t.c);
            let delta = s - q;
            let dist = delta.length();
            if dist < r {
                let normal = if dist > 1e-4 {
                    delta / dist
                } else if t.n.dot(cap.center - q) >= 0.0 {
                    t.n
                } else {
                    -t.n
                };
                out.push(Contact { normal, depth: r - dist, point: q, tri: i });
            }
        });
    }

    fn sweep_capsule(&self, cap: &Capsule, delta: Vec3, channels: u8) -> Option<SweepHit> {
        let mut contacts = Vec::new();
        let overlapping = |c: &Capsule, contacts: &mut Vec<Contact>| -> bool {
            contacts.clear();
            self.capsule_contacts(c, channels, contacts);
            contacts.iter().any(|k| k.depth > SKIN)
        };
        if overlapping(cap, &mut contacts) {
            let k = contacts.iter().max_by(|a, b| a.depth.total_cmp(&b.depth)).copied().unwrap();
            let face_up = self.face_up(&contacts);
            return Some(SweepHit { fraction: 0.0, normal: k.normal, point: k.point, start_penetrating: true, face_up });
        }
        let len = delta.length();
        if len < 1e-6 {
            return None;
        }
        // Conservative sub-stepping so thin geometry cannot be tunneled through.
        let step = (cap.radius * 0.5).max(1.0);
        let n = (len / step).ceil().max(1.0) as usize;
        let mut lo = 0.0f32;
        let mut hit_hi = None;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            if overlapping(&cap.at(cap.center + delta * t), &mut contacts) {
                hit_hi = Some(t);
                break;
            }
            lo = t;
        }
        let mut hi = hit_hi?;
        for _ in 0..10 {
            let mid = (lo + hi) * 0.5;
            if overlapping(&cap.at(cap.center + delta * mid), &mut contacts) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        // Contact normal at the first overlapping position.
        overlapping(&cap.at(cap.center + delta * hi), &mut contacts);
        let mut normal = Vec3::ZERO;
        let mut point = cap.center + delta * hi;
        let mut best = 0.0;
        for c in &contacts {
            normal += c.normal * c.depth;
            if c.depth > best {
                best = c.depth;
                point = c.point;
            }
        }
        let normal = normal.normalize_or(-delta / len);
        // Back off by the skin so the next query starts clear.
        let back = (SKIN / len).min(lo);
        let face_up = self.face_up(&contacts);
        Some(SweepHit { fraction: (lo - back).max(0.0), normal, point, start_penetrating: false, face_up })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor_and_wall() -> CollisionWorld {
        let mut tris = Vec::new();
        let quad = |tris: &mut Vec<Tri>, a: Vec3, b: Vec3, c: Vec3, d: Vec3| {
            for (x, y, z) in [(a, b, c), (a, c, d)] {
                let n = (y - x).cross(z - x).normalize();
                tris.push(Tri { a: x, b: y, c: z, n, channels: channel::ALL, source: 0 });
            }
        };
        // Floor at z=0 spanning +-1000.
        quad(&mut tris, Vec3::new(-1000., -1000., 0.), Vec3::new(1000., -1000., 0.), Vec3::new(1000., 1000., 0.), Vec3::new(-1000., 1000., 0.));
        // Wall at x=500 facing -X.
        quad(&mut tris, Vec3::new(500., -1000., 0.), Vec3::new(500., -1000., 500.), Vec3::new(500., 1000., 500.), Vec3::new(500., 1000., 0.));
        CollisionWorld::from_triangles(tris, vec!["test".into()])
    }

    #[test]
    fn raycast_floor() {
        let w = floor_and_wall();
        let h = w.raycast(Vec3::new(0., 0., 100.), -Vec3::Z, 1000.0, channel::ALL).unwrap();
        assert!((h.distance - 100.0).abs() < 1e-3);
        assert!((h.normal - Vec3::Z).length() < 1e-5);
    }

    #[test]
    fn capsule_rests_on_floor() {
        let w = floor_and_wall();
        let cap = Capsule { center: Vec3::new(0., 0., 87.5), radius: 31.0, half_height: 87.5 };
        let hit = w.sweep_capsule(&cap.at(Vec3::new(0., 0., 200.)), Vec3::new(0., 0., -300.), channel::ALL).unwrap();
        let z = 200.0 - 300.0 * hit.fraction;
        assert!((z - 87.5).abs() < 0.5, "rest z {z}");
        assert!(hit.normal.z > 0.99);
    }

    #[test]
    fn capsule_stops_at_wall() {
        let w = floor_and_wall();
        let cap = Capsule { center: Vec3::new(0., 0., 100.), radius: 31.0, half_height: 87.5 };
        let hit = w.sweep_capsule(&cap, Vec3::new(1000., 0., 0.), channel::ALL).unwrap();
        let x = 1000.0 * hit.fraction;
        assert!((x - (500.0 - 31.0)).abs() < 0.5, "stop x {x}");
        assert!(hit.normal.x < -0.99);
    }

    #[test]
    fn depenetrate_from_floor() {
        let w = floor_and_wall();
        let cap = Capsule { center: Vec3::new(0., 0., 80.0), radius: 31.0, half_height: 87.5 };
        let p = w.depenetrate(&cap, channel::ALL);
        assert!(p.z >= 87.4, "{p}");
    }
}

/// The persistent maps of a multi-map session (e.g. Boyle exterior + interior). The original
/// swaps the persistent map on level travel (`SeqAct_PrepareMapChange` /
/// `SeqAct_CommitMapChange`), so queries go to the active map only.
pub struct MapSet {
    maps: Vec<(String, std::sync::Arc<dyn WorldQuery + Send + Sync>)>,
    active: std::sync::atomic::AtomicUsize,
}

impl MapSet {
    pub fn new(maps: Vec<(String, std::sync::Arc<dyn WorldQuery + Send + Sync>)>) -> Self {
        MapSet { maps, active: std::sync::atomic::AtomicUsize::new(0) }
    }
    /// Activate a map by name (case-insensitive). Returns false if unknown.
    pub fn set_active(&self, name: &str) -> bool {
        match self.maps.iter().position(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some(i) => {
                self.active.store(i, std::sync::atomic::Ordering::Relaxed);
                true
            }
            None => false,
        }
    }
    pub fn active_name(&self) -> &str {
        &self.maps[self.active.load(std::sync::atomic::Ordering::Relaxed)].0
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.maps.iter().map(|(n, _)| n.as_str())
    }
    fn cur(&self) -> &dyn WorldQuery {
        self.maps[self.active.load(std::sync::atomic::Ordering::Relaxed)].1.as_ref()
    }
}

impl WorldQuery for MapSet {
    fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32, channels: u8) -> Option<RayHit> {
        self.cur().raycast(origin, dir, max_dist, channels)
    }
    fn capsule_contacts(&self, cap: &Capsule, channels: u8, out: &mut Vec<Contact>) {
        self.cur().capsule_contacts(cap, channels, out)
    }
    fn sweep_capsule(&self, cap: &Capsule, delta: Vec3, channels: u8) -> Option<SweepHit> {
        self.cur().sweep_capsule(cap, delta, channels)
    }
}
