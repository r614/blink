//! Collision world and spatial queries (M5).
//!
//! Geometry comes from the per-poly kDOP collision triangles cooked into every
//! `UStaticMesh` (falling back to render triangles for meshes without them), transformed
//! by each colliding instance. Coordinates: Dishonored world space (Z-up, uu).
//!
//! Character shape: the original pawn has a `CylinderComponent` (radius 35, half-height
//! 87.5 for the player), but every world trace of its movement passes the cylinder's
//! *extent* `(radius, radius, height)` (`GetCylinderExtent`, e.g. the air-control wall test
//! of `APawn::physFalling` 0x006BF0E0), and extent traces against static-mesh kDOP trees and
//! BSP are axis-aligned box tests in UE3. Pawn movement therefore uses [`PawnBox`] queries
//! (U-016). The capsule queries remain for other shapes.
//!
//! Water: `DishonoredWaterVolume` brushes (convex hulls, [`WaterVolume`]) are attached after
//! loading, like the navigation mesh, and answered by [`WorldQuery::water_at`].

pub mod bvh;
pub mod geom;
pub mod nav;

use anyhow::Result;
use bvh::Bvh;
use dishonored_ir::DishonoredMap;
use geom::{Aabb, box_triangle, closest_point_triangle, closest_segment_triangle, ray_triangle};
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

/// An axis-aligned box: what a UE3 pawn sweeps against world geometry (its cylinder's
/// extent, `half = (radius, radius, height)`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PawnBox {
    pub center: Vec3,
    pub half: Vec3,
}

impl PawnBox {
    pub fn new(center: Vec3, radius: f32, half_height: f32) -> Self {
        PawnBox { center, half: Vec3::new(radius, radius, half_height) }
    }
    pub fn at(&self, center: Vec3) -> PawnBox {
        PawnBox { center, ..*self }
    }
    pub fn aabb(&self) -> Aabb {
        Aabb { min: self.center - self.half, max: self.center + self.half }
    }
    /// The capsule of the same radius and height (fallback for worlds without box queries).
    pub fn capsule(&self) -> Capsule {
        Capsule { center: self.center, radius: self.half.x.max(self.half.y), half_height: self.half.z }
    }
}

/// One `DishonoredWaterVolume` (a `PhysicsVolume` with `bWaterVolume`): its brush's convex
/// elements in world space as planes `(n, d)` with `n . p <= d` inside.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct WaterVolume {
    pub name: String,
    pub hulls: Vec<Vec<[f32; 4]>>,
    pub min: Vec3,
    pub max: Vec3,
    /// `PhysicsVolume.FluidFriction` (2.4 for `DishonoredWaterVolume`).
    pub fluid_friction: f32,
    /// `PhysicsVolume.TerminalVelocity`.
    pub terminal_velocity: f32,
    /// `m_bShallowWater`.
    pub shallow: bool,
}

impl WaterVolume {
    pub fn contains(&self, p: Vec3) -> bool {
        if p.cmplt(self.min).any() || p.cmpgt(self.max).any() {
            return false;
        }
        self.hulls.iter().any(|h| h.iter().all(|pl| p.x * pl[0] + p.y * pl[1] + p.z * pl[2] <= pl[3] + 1e-3))
    }
}

/// What [`WorldQuery::water_at`] reports for a point inside a water volume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterInfo {
    /// Top of the volume above the point (the water surface).
    pub surface_z: f32,
    pub fluid_friction: f32,
    pub terminal_velocity: f32,
    pub shallow: bool,
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
    /// The map's original navigation mesh, when loaded.
    fn nav(&self) -> Option<&nav::NavWorld> {
        None
    }
    /// Overlaps of an axis-aligned pawn box (worlds without box support answer for the capsule
    /// of the same radius and height).
    fn box_contacts(&self, b: &PawnBox, channels: u8, out: &mut Vec<Contact>) {
        self.capsule_contacts(&b.capsule(), channels, out)
    }
    /// Sweep an axis-aligned pawn box (UE3 `MoveActor` with the pawn's extent).
    fn sweep_box(&self, b: &PawnBox, delta: Vec3, channels: u8) -> Option<SweepHit> {
        self.sweep_capsule(&b.capsule(), delta, channels)
    }
    /// The water volume containing `p`, if any.
    fn water_at(&self, _p: Vec3) -> Option<WaterInfo> {
        None
    }
}

/// Water lookup over a set of volumes (shared by the world implementations).
pub fn water_lookup(volumes: &[WaterVolume], p: Vec3) -> Option<WaterInfo> {
    for v in volumes {
        if !v.contains(p) {
            continue;
        }
        let mut surface = v.max.z;
        for h in &v.hulls {
            if !h.iter().all(|pl| p.x * pl[0] + p.y * pl[1] + p.z * pl[2] <= pl[3] + 1e-3) {
                continue;
            }
            for pl in h {
                if pl[2] > 1e-3 {
                    surface = surface.min((pl[3] - pl[0] * p.x - pl[1] * p.y) / pl[2]);
                }
            }
        }
        return Some(WaterInfo { surface_z: surface, fluid_friction: v.fluid_friction, terminal_velocity: v.terminal_velocity, shallow: v.shallow });
    }
    None
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollisionWorld {
    pub tris: Vec<Tri>,
    pub bvh: Bvh,
    pub sources: Vec<String>,
    pub bounds: Option<Aabb>,
    /// Per triangle (parallel to `tris`): index into `surfaces` (0 = none). Presentation only
    /// (footsteps and impact sounds); the simulation never reads it.
    #[serde(default)]
    pub tri_surface: Vec<u16>,
    /// Contact type class names of the physical materials (`surfaces[0]` is empty).
    #[serde(default)]
    pub surfaces: Vec<String>,
    /// Original navigation (attached after loading; not part of the collision cache).
    #[serde(skip)]
    pub nav: Option<std::sync::Arc<nav::NavWorld>>,
    /// Water volumes of the map (attached after loading; not part of the collision cache).
    #[serde(skip)]
    pub water: Option<std::sync::Arc<Vec<WaterVolume>>>,
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
        CollisionWorld { tris, bvh, sources, bounds: Some(total), nav: None, water: None, tri_surface: Vec::new(), surfaces: Vec::new() }
    }

    /// Build from an imported map: every colliding instance (hidden blockers included).
    pub fn from_map(map: &DishonoredMap) -> Self {
        let mut tris = Vec::new();
        let mut sources = Vec::new();
        let mut tri_surface = Vec::new();
        let mut surfaces: Vec<String> = vec![String::new()];
        let mut surface_id = |name: Option<&String>| -> u16 {
            let Some(n) = name else { return 0 };
            match surfaces.iter().position(|s| s == n) {
                Some(i) => i as u16,
                None => {
                    surfaces.push(n.clone());
                    (surfaces.len() - 1) as u16
                }
            }
        };
        for inst in map.instances.iter().filter(|i| i.collides) {
            let mesh = &map.meshes[inst.mesh];
            let src = sources.len() as u32;
            sources.push(format!("{} <- {}", mesh.source, inst.source));
            // Surface (contact type) per section: instance override, else the mesh material.
            let section_surface: Vec<u16> = (0..mesh.sections.len())
                .map(|i| {
                    let m = inst.materials.get(i).copied().flatten().or(mesh.sections[i].material);
                    surface_id(m.and_then(|m| map.materials.get(m)).and_then(|m| m.contact_type.as_ref()))
                })
                .collect();
            let xf = inst.transform;
            let mirrored = xf.determinant() < 0.0;
            let mut push = |i0: u32, i1: u32, i2: u32, surface: u16| {
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
                tri_surface.push(surface);
            };
            if !mesh.collision_triangles.is_empty() {
                for (i, t) in mesh.collision_triangles.iter().enumerate() {
                    // The kDOP tree keeps every triangle; UE3's static mesh collision provider
                    // skips those of sections without `EnableCollision` at query time
                    // (`FStaticMeshCollisionDataProvider::ShouldCheckMaterial`).
                    let m = mesh.collision_materials.get(i).map(|m| *m as usize);
                    if m.and_then(|m| mesh.sections.get(m)).is_some_and(|s| !s.enable_collision) {
                        continue;
                    }
                    let s = m.and_then(|m| section_surface.get(m)).copied().unwrap_or(0);
                    push(t[0], t[1], t[2], s);
                }
            } else {
                for (si, s) in mesh.sections.iter().enumerate().filter(|s| s.1.enable_collision) {
                    let start = s.first_index as usize;
                    let end = (start + s.num_triangles as usize * 3).min(mesh.indices.len());
                    for t in mesh.indices[start..end].chunks_exact(3) {
                        push(t[0], t[1], t[2], section_surface[si]);
                    }
                }
            }
        }
        let mut w = Self::from_triangles(tris, sources);
        w.tri_surface = tri_surface;
        w.surfaces = surfaces;
        w
    }

    /// Contact type (`DisContactType_*` class) of a triangle's physical material; `None` when
    /// the material sets none (the game then uses `DisContactType_Environment`).
    pub fn surface_of(&self, tri: u32) -> Option<&str> {
        let s = *self.tri_surface.get(tri as usize)? as usize;
        self.surfaces.get(s).map(String::as_str).filter(|s| !s.is_empty())
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
    fn nav(&self) -> Option<&nav::NavWorld> {
        self.nav.as_deref()
    }

    fn water_at(&self, p: Vec3) -> Option<WaterInfo> {
        self.water.as_deref().and_then(|w| water_lookup(w, p))
    }

    fn box_contacts(&self, b: &PawnBox, channels: u8, out: &mut Vec<Contact>) {
        self.bvh.query_aabb(&b.aabb().expand(SKIN), |i| {
            let t = &self.tris[i as usize];
            if t.channels & channels == 0 {
                return;
            }
            if let Some((normal, depth)) = box_triangle(b.center, b.half, t.a, t.b, t.c, 0.0) {
                if depth > 0.0 {
                    let point = closest_point_triangle(b.center, t.a, t.b, t.c);
                    out.push(Contact { normal, depth, point, tri: i });
                }
            }
        });
    }

    fn sweep_box(&self, b: &PawnBox, delta: Vec3, channels: u8) -> Option<SweepHit> {
        let mut contacts = Vec::new();
        let overlapping = |bx: &PawnBox, contacts: &mut Vec<Contact>| -> bool {
            contacts.clear();
            self.box_contacts(bx, channels, contacts);
            contacts.iter().any(|k| k.depth > SKIN)
        };
        if overlapping(b, &mut contacts) {
            let k = contacts.iter().max_by(|a, c| a.depth.total_cmp(&c.depth)).copied().unwrap();
            let face_up = self.face_up(&contacts);
            return Some(SweepHit { fraction: 0.0, normal: k.normal, point: k.point, start_penetrating: true, face_up });
        }
        let len = delta.length();
        if len < 1e-6 {
            return None;
        }
        // Conservative sub-steps (half the smaller half extent) so thin geometry is not
        // tunneled through, then bisection to the first overlap.
        let step = (b.half.x.min(b.half.y).min(b.half.z) * 0.5).max(1.0);
        let n = (len / step).ceil().max(1.0) as usize;
        let mut lo = 0.0f32;
        let mut hit_hi = None;
        for k in 1..=n {
            let t = k as f32 / n as f32;
            if overlapping(&b.at(b.center + delta * t), &mut contacts) {
                hit_hi = Some(t);
                break;
            }
            lo = t;
        }
        let mut hi = hit_hi?;
        for _ in 0..12 {
            let mid = (lo + hi) * 0.5;
            if overlapping(&b.at(b.center + delta * mid), &mut contacts) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        overlapping(&b.at(b.center + delta * hi), &mut contacts);
        // Hit normal: the push-out of the deepest contact that opposes the move (a box sliding
        // along a floor also touches it; the blocking surface is the one in the way).
        let dir = delta / len;
        let blocking = contacts
            .iter()
            .filter(|c| c.normal.dot(dir) < -1e-3)
            .max_by(|a, c| (a.depth * -a.normal.dot(dir)).total_cmp(&(c.depth * -c.normal.dot(dir))))
            .or_else(|| contacts.iter().max_by(|a, c| a.depth.total_cmp(&c.depth)))
            .copied();
        let (normal, point) = blocking.map_or((-dir, b.center + delta * hi), |c| (c.normal, c.point));
        let back = (SKIN / len).min(lo);
        let face_up = self.face_up(&contacts);
        Some(SweepHit { fraction: (lo - back).max(0.0), normal, point, start_penetrating: false, face_up })
    }

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
    fn box_rests_on_floor_and_stops_at_wall() {
        let w = floor_and_wall();
        let b = PawnBox::new(Vec3::new(0., 0., 200.), 35.0, 87.5);
        let hit = w.sweep_box(&b, Vec3::new(0., 0., -300.), channel::ALL).unwrap();
        let z = 200.0 - 300.0 * hit.fraction;
        assert!((z - 87.5).abs() < 0.2, "rest z {z}");
        assert!(hit.normal.z > 0.99 && hit.face_up.z > 0.99);
        // Flat bottom: a box standing beside a ledge edge is still supported by it (a capsule
        // would roll off): the floor ends at x = 1000, the box centre is 20 past it.
        let edge = PawnBox::new(Vec3::new(1020., 0., 88.0), 35.0, 87.5);
        assert!(w.sweep_box(&edge, Vec3::new(0., 0., -10.), channel::ALL).is_some());
        let side = PawnBox::new(Vec3::new(0., 0., 100.), 35.0, 87.5);
        let hit = w.sweep_box(&side, Vec3::new(1000., 0., 0.), channel::ALL).unwrap();
        let x = 1000.0 * hit.fraction;
        assert!((x - (500.0 - 35.0)).abs() < 0.2, "stop x {x}");
        assert!(hit.normal.x < -0.99, "{:?}", hit.normal);
    }

    #[test]
    fn water_volume_lookup() {
        let v = WaterVolume {
            name: "w".into(),
            hulls: vec![vec![[0., 0., 1., 100.], [0., 0., -1., 0.], [1., 0., 0., 50.], [-1., 0., 0., 50.], [0., 1., 0., 50.], [0., -1., 0., 50.]]],
            min: Vec3::new(-50., -50., 0.),
            max: Vec3::new(50., 50., 100.),
            fluid_friction: 2.4,
            terminal_velocity: 4000.0,
            shallow: false,
        };
        let i = water_lookup(std::slice::from_ref(&v), Vec3::new(0., 0., 40.)).unwrap();
        assert_eq!(i.surface_z, 100.0);
        assert!(water_lookup(std::slice::from_ref(&v), Vec3::new(0., 0., 140.)).is_none());
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
    fn nav(&self) -> Option<&nav::NavWorld> {
        self.cur().nav()
    }
    fn box_contacts(&self, b: &PawnBox, channels: u8, out: &mut Vec<Contact>) {
        self.cur().box_contacts(b, channels, out)
    }
    fn sweep_box(&self, b: &PawnBox, delta: Vec3, channels: u8) -> Option<SweepHit> {
        self.cur().sweep_box(b, delta, channels)
    }
    fn water_at(&self, p: Vec3) -> Option<WaterInfo> {
        self.cur().water_at(p)
    }
}
