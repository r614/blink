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
//!
//! Climbables: the `DisClimbable` chains of the map ([`Climbable`], their rest-pose bones) are
//! attached the same way and listed by [`WorldQuery::climbables`].

pub mod bvh;
pub mod geom;
pub mod nav;
pub mod sat;

use anyhow::Result;
use bvh::Bvh;
use dishonored_ir::DishonoredMap;
use geom::{Aabb, box_triangle, closest_point_triangle, closest_segment_triangle, ray_triangle};
use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Trace channels, mirroring Dishonored's `DisPrimTraceMask` (research/dishonored/COLLISION.md):
/// a check of one trace type is blocked only by primitives with that type's bit.
pub mod channel {
    /// The player's movement (`m_bTraceForMove_Player`).
    pub const PAWN: u8 = 1 << 0;
    pub const CROSSHAIR: u8 = 1 << 1;
    pub const PROJECTILE: u8 = 1 << 2;
    pub const MELEE: u8 = 1 << 3;
    pub const VISION: u8 = 1 << 4;
    /// Other pawns' movement (`m_bTraceForMove_NonPlayerPawn`).
    pub const NPC: u8 = 1 << 5;
    /// Movement of actors that are not pawns (`m_bTraceForMove_NonPawn`).
    pub const NON_PAWN: u8 = 1 << 6;
    pub const ALL: u8 = 0x7F;

    /// The channels of a `DisPrimTraceMask` (bit 0 move non-pawn, 1 move NPC, 2 move player,
    /// 3 crosshair, 4 projectile, 5 melee, 6 vision).
    pub fn from_trace_mask(m: u8) -> u8 {
        let mut c = 0;
        for (bit, ch) in [(0, NON_PAWN), (1, NPC), (2, PAWN), (3, CROSSHAIR), (4, PROJECTILE), (5, MELEE), (6, VISION)] {
            if m & (1 << bit) != 0 {
                c |= ch;
            }
        }
        c
    }
}

/// Which checks a triangle answers: `UStaticMeshComponent::LineCheck` uses a mesh's simplified
/// collision for zero-extent checks when `UseSimpleLineCollision` and for extent checks when
/// `UseSimpleBoxCollision`, its kDOP triangles otherwise.
pub mod shape {
    /// Zero-extent checks (lines: `raycast`).
    pub const LINE: u8 = 1 << 0;
    /// Extent checks (boxes and capsules).
    pub const EXTENT: u8 = 1 << 1;
    /// Checks that ask for complex collision (`TRACE_ComplexCollision`, 0x20000): the kDOP
    /// triangles whatever the mesh's flags.
    pub const COMPLEX: u8 = 1 << 2;
    pub const BOTH: u8 = LINE | EXTENT;
    pub const ALL: u8 = LINE | EXTENT | COMPLEX;

    /// The gameplay checks that ask for complex collision in the original: the pistol shot
    /// (flags 0x020A28DF), the crosshair (0x01022097) and the vision line of sight (0x08023086),
    /// measured with the engine hook (research/dishonored/COLLISION.md "Trace flags").
    pub const COMPLEX_CHANNELS: u8 = super::channel::CROSSHAIR | super::channel::PROJECTILE | super::channel::VISION;

    /// The shape bit a line check on `channels` needs.
    pub fn line_for(channels: u8) -> u8 {
        if channels & COMPLEX_CHANNELS != 0 { COMPLEX } else { LINE }
    }

    /// The shape bit an extent check on `channels` needs.
    pub fn extent_for(channels: u8) -> u8 {
        if channels & COMPLEX_CHANNELS != 0 { COMPLEX } else { EXTENT }
    }
}

/// How the original backs a hit off along a check that is not `TRACE_Accurate`
/// (`UStaticMeshComponent::LineCheck` 0x007717D0): what a triangle came from.
pub mod hit_kind {
    /// kDOP (per-polygon) triangles: 4 uu (at most a tenth of the check).
    pub const COMPLEX: u8 = 0;
    /// Simplified collision (`AggGeom`): 1 uu (at most a tenth of the check).
    pub const SIMPLE: u8 = 1;
    /// Brushes and physics-asset bodies (not measured, U-653).
    pub const OTHER: u8 = 2;

    /// The fraction the original reports for a hit at fraction `t` of a check of length `len`.
    pub fn engine_time(kind: u8, t: f32, len: f32) -> f32 {
        if len <= 0.0 {
            return t;
        }
        let back = match kind {
            COMPLEX => {
                let a = 0.1 / len;
                if a > 0.1 { a } else { (4.0 / len).min(0.1) }
            }
            SIMPLE => {
                let a = 0.1 / len;
                if a > 0.1 { a } else { (1.0 / len).min(0.1) }
            }
            _ => 0.0,
        };
        (t - back).clamp(0.0, 1.0)
    }
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

/// One `DisClimbable` (a rigid-body chain `KAsset`) in its rest pose: the climbing player follows
/// its closest bone (research/dishonored/PLAYER.md "Climbing").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Climbable {
    pub name: String,
    /// The actor's `Location` (the anchor: the chain hangs below it).
    pub anchor: Vec3,
    /// World positions of every bone of the skeletal mesh (`FindClosestBone` searches them all).
    pub bones: Vec<Vec3>,
    /// `SkeletalMeshComponent.Bounds.SphereRadius` (the physics asset's bodies, rest pose).
    pub sphere_radius: f32,
    /// Half-width of the chain's collision bodies (their box half extent across the chain).
    pub body_half_width: f32,
    /// World-space segment covered by the bodies (top, bottom), for touch and aim tests.
    pub top: Vec3,
    pub bottom: Vec3,
}

impl Climbable {
    /// Index of the bone closest to `p` (the first on ties: the original compares with `<`).
    pub fn closest_bone(&self, p: Vec3) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for (i, b) in self.bones.iter().enumerate() {
            let d = b.distance_squared(p);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        best.map(|b| b.0)
    }
    /// Closest point of the body segment to `p`.
    pub fn closest_point(&self, p: Vec3) -> Vec3 {
        let ab = self.bottom - self.top;
        let l2 = ab.length_squared();
        let t = if l2 > 1e-6 { ((p - self.top).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
        self.top + ab * t
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
    /// The blocking triangle (of the world that answered; `u32::MAX` when unknown).
    pub tri: u32,
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
    /// Mover collision (`InterpActor`s) where the map placed it; the game places it where the
    /// actors are at run time ([`Layered`]).
    fn movers(&self) -> &[MoverCollision] {
        &[]
    }
    /// The map's non-water physics volumes (zone velocity, pain).
    fn physics_volumes(&self) -> &[PhysicsVolume] {
        &[]
    }
    /// The map's climbable chains.
    fn climbables(&self) -> &[Climbable] {
        &[]
    }
}


/// A level `PhysicsVolume` (not water): zone velocity and pain (Engine `PhysicsVolume`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PhysicsVolume {
    pub name: String,
    /// Planes `(nx, ny, nz, d)`, inside when `n . p <= d` for every plane of one hull.
    pub hulls: Vec<Vec<[f32; 4]>>,
    pub min: Vec3,
    pub max: Vec3,
    pub priority: i32,
    pub zone_velocity: Vec3,
    /// `bVelocityAffectsWalking` (default true).
    pub velocity_affects_walking: bool,
    /// `bPainCausing`: `DamagePerSec`, `PainInterval` (default 1), `bEntryPain` (default true)
    /// and the `DamageType` class name.
    pub pain: Option<VolumePain>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct VolumePain {
    pub damage_per_sec: f32,
    pub interval: f32,
    pub entry: bool,
    pub damage_type: String,
}

impl PhysicsVolume {
    /// UE3 `Encompasses`: the point is inside one of the brush's hulls.
    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all() && self.hulls.iter().any(|h| h.iter().all(|pl| p.x * pl[0] + p.y * pl[1] + p.z * pl[2] <= pl[3] + 1e-3))
    }

    /// The pawn's cylinder (centre, radius, half height) touches the brush: inside a hull grown
    /// by the cylinder's box extent along each plane normal.
    pub fn touches(&self, c: Vec3, radius: f32, half_height: f32) -> bool {
        let e = Vec3::new(radius, radius, half_height);
        if c.cmplt(self.min - e).any() || c.cmpgt(self.max + e).any() {
            return false;
        }
        self.hulls.iter().any(|h| h.iter().all(|pl| c.x * pl[0] + c.y * pl[1] + c.z * pl[2] <= pl[3] + pl[0].abs() * e.x + pl[1].abs() * e.y + pl[2].abs() * e.z))
    }
}

/// The physics volume of a point: the highest `Priority` one containing it (UE3 `SetZone`).
pub fn volume_at(volumes: &[PhysicsVolume], p: Vec3) -> Option<&PhysicsVolume> {
    volumes.iter().filter(|v| v.contains(p)).max_by_key(|v| v.priority)
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
    /// Per triangle (parallel to `tris`, empty: every triangle answers every check): the
    /// checks it answers ([`shape`] bits).
    #[serde(default)]
    pub tri_shape: Vec<u8>,
    /// Per triangle (parallel to `tris`): what it came from ([`hit_kind`]).
    #[serde(default)]
    pub tri_kind: Vec<u8>,
    /// Per triangle (parallel to `tris`, empty: none): the convex element it belongs to
    /// (`u32::MAX`: a lone per-polygon triangle). Elements are contiguous runs of triangles.
    #[serde(default)]
    pub tri_elem: Vec<u32>,
    /// The convex elements (simplified collision, brush hulls, physics bodies).
    #[serde(default)]
    pub elems: Vec<Elem>,
    #[serde(default)]
    pub elem_bvh: Bvh,
    /// Original navigation (attached after loading; not part of the collision cache).
    #[serde(skip)]
    pub nav: Option<std::sync::Arc<nav::NavWorld>>,
    /// Water volumes of the map (attached after loading; not part of the collision cache).
    #[serde(skip)]
    pub water: Option<std::sync::Arc<Vec<WaterVolume>>>,
    /// Physics volumes of the map (attached after loading; not part of the collision cache).
    #[serde(skip)]
    pub volumes: Option<std::sync::Arc<Vec<PhysicsVolume>>>,
    /// Mover collision (`InterpActor` components), not part of the static triangles.
    #[serde(default)]
    pub movers: Vec<MoverCollision>,
    /// `DisClimbable` chains of the map (attached after loading; not part of the collision cache).
    #[serde(skip)]
    pub climbables: Option<std::sync::Arc<Vec<Climbable>>>,
}

/// The colliding triangles of one mover actor, world space where the map placed it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoverCollision {
    /// `<package>:<actor name>`.
    pub actor: String,
    /// The actor's placed location and rotation.
    pub actor_to_world: glam::Mat4,
    pub tris: Vec<[Vec3; 3]>,
    /// Per triangle: channels, [`shape`] bits and [`hit_kind`] (empty: every channel and check).
    #[serde(default)]
    pub info: Vec<TriInfo>,
    /// A door's other rest poses: (`EDisDoorState`, its bodies' elements), answering like its
    /// first triangle; `tris` is the closed pose.
    #[serde(default)]
    pub poses: Vec<(u8, Vec<Vec<[Vec3; 3]>>)>,
    /// The state the map starts the door in (`m_InitialDoorState`).
    #[serde(default)]
    pub initial: u8,
}

/// What one collision triangle answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriInfo {
    pub channels: u8,
    pub shape: u8,
    pub kind: u8,
    /// Its convex element (an index local to the instance or mover; `u32::MAX`: none).
    #[serde(default = "no_elem")]
    pub elem: u32,
}

fn no_elem() -> u32 {
    u32::MAX
}

impl Default for TriInfo {
    fn default() -> Self {
        TriInfo { channels: channel::ALL, shape: shape::ALL, kind: hit_kind::COMPLEX, elem: u32::MAX }
    }
}

/// A convex element: `count` triangles from `first`, wound outward, and their bounds.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Elem {
    pub first: u32,
    pub count: u32,
    pub min: Vec3,
    pub max: Vec3,
}

impl MoverCollision {
    /// The mover's collision moved by `delta` (current actor transform x inverse placed one), a
    /// door in the state the map starts it in.
    pub fn placed(&self, delta: glam::Mat4) -> CollisionWorld {
        self.placed_in(self.initial, delta)
    }

    /// Whether this is a door (it has poses per state).
    pub fn is_door(&self) -> bool {
        !self.poses.is_empty()
    }

    /// The collision in door state `state` (the closed pose for a state without a pose), moved
    /// by `delta`.
    pub fn placed_in(&self, state: u8, delta: glam::Mat4) -> CollisionWorld {
        // The triangles of the pose with their info (a pose's elements answer like the first
        // triangle of the closed pose).
        let mut src: Vec<([Vec3; 3], TriInfo)> = Vec::new();
        match self.poses.iter().find(|p| p.0 == state && state != 0) {
            Some(p) => {
                let base = self.info.first().copied().unwrap_or_default();
                for (ei, e) in p.1.iter().enumerate() {
                    src.extend(e.iter().map(|t| (*t, TriInfo { elem: ei as u32, ..base })));
                }
            }
            None => src.extend(self.tris.iter().enumerate().map(|(i, t)| (*t, self.info.get(i).copied().unwrap_or_default()))),
        }
        let mirrored = delta.determinant() < 0.0;
        let mut tris = Vec::new();
        let (mut shapes, mut kinds, mut elems) = (Vec::new(), Vec::new(), Vec::new());
        for (t, info) in src {
            let (a, b, c) = (delta.transform_point3(t[0]), delta.transform_point3(t[1]), delta.transform_point3(t[2]));
            let (b, c) = if mirrored { (c, b) } else { (b, c) };
            let n = (b - a).cross(c - a);
            if n.length_squared() < 1e-10 {
                continue;
            }
            tris.push(Tri { a, b, c, n: n.normalize(), channels: info.channels, source: 0 });
            shapes.push(info.shape);
            kinds.push(info.kind);
            elems.push(info.elem);
        }
        let mut w = CollisionWorld::from_triangles(tris, vec![self.actor.clone()]);
        w.tri_shape = shapes;
        w.tri_kind = kinds;
        w.tri_elem = elems;
        w.index_elements();
        w
    }
}

const SKIN: f32 = 0.05;

/// The faces of a convex hull given as planes `(nx, ny, nz, d)` (inside when `n . p <= d`), as
/// triangles wound so their normals point out (UE3 brush volumes, `KConvexElem` face planes).
pub fn hull_triangles(planes: &[[f32; 4]]) -> Vec<[Vec3; 3]> {
    use glam::DVec3;
    let mut out = Vec::new();
    for (i, pl) in planes.iter().enumerate() {
        let n = DVec3::new(pl[0] as f64, pl[1] as f64, pl[2] as f64);
        let len = n.length();
        if len < 1e-6 {
            continue;
        }
        let (n, d) = (n / len, pl[3] as f64 / len);
        // A large square on the plane (f64), clipped by every other plane.
        let u = if n.z.abs() < 0.9 { n.cross(DVec3::Z).normalize() } else { n.cross(DVec3::X).normalize() };
        let v = n.cross(u);
        let c = n * d;
        const R: f64 = 1.0e7;
        let mut poly = vec![c + (u + v) * R, c + (v - u) * R, c - (u + v) * R, c + (u - v) * R];
        for (j, q) in planes.iter().enumerate() {
            if j == i || poly.is_empty() {
                continue;
            }
            let qn = DVec3::new(q[0] as f64, q[1] as f64, q[2] as f64);
            let ql = qn.length().max(1e-12);
            let dist = |p: DVec3| (qn.dot(p) - q[3] as f64) / ql;
            let mut next = Vec::with_capacity(poly.len() + 1);
            for k in 0..poly.len() {
                let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
                let (da, db) = (dist(a), dist(b));
                if da <= 0.0 {
                    next.push(a);
                }
                if (da < 0.0) != (db < 0.0) && (da - db).abs() > 1e-9 {
                    next.push(a + (b - a) * (da / (da - db)));
                }
            }
            poly = next;
        }
        if poly.len() < 3 {
            continue;
        }
        for k in 1..poly.len() - 1 {
            let (a, b, c) = (poly[0], poly[k], poly[k + 1]);
            let tn = (b - a).cross(c - a);
            if tn.length_squared() < 1e-6 {
                continue;
            }
            let (a, b, c) = (a.as_vec3(), b.as_vec3(), c.as_vec3());
            out.push(if tn.dot(n) >= 0.0 { [a, b, c] } else { [a, c, b] });
        }
    }
    out
}

/// The twelve triangles of a box: the unit cube `[-1, 1]^3` under `m` (half extents in its
/// scale), normals out.
pub fn box_triangles(m: glam::Mat4) -> Vec<[Vec3; 3]> {
    let planes: Vec<[f32; 4]> = [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z]
        .iter()
        .map(|&a| {
            // Plane of the face through m(a), normal m^-T a.
            let p = m.transform_point3(a);
            let n = m.inverse().transpose().transform_vector3(a).normalize();
            [n.x, n.y, n.z, n.dot(p)]
        })
        .collect();
    hull_triangles(&planes)
}

impl CollisionWorld {
    /// This world plus extra blockers `(source, channels, triangles)`; the bounding volume
    /// hierarchy is rebuilt once.
    pub fn with_extra(self, extra: &[(String, u8, Vec<[Vec3; 3]>)]) -> Self {
        let extra: Vec<(String, u8, u8, Vec<Vec<[Vec3; 3]>>)> = extra.iter().map(|e| (e.0.clone(), e.1, hit_kind::OTHER, vec![e.2.clone()])).collect();
        self.with_extra_kinds(&extra)
    }

    /// [`Self::with_extra`] with each blocker's [`hit_kind`] and convex elements:
    /// `(source, channels, kind, elements)` (each element a closed convex hull wound outward).
    pub fn with_extra_kinds(self, extra: &[(String, u8, u8, Vec<Vec<[Vec3; 3]>>)]) -> Self {
        if extra.iter().all(|e| e.3.iter().all(Vec::is_empty)) {
            return self;
        }
        let CollisionWorld { mut tris, mut sources, mut tri_surface, surfaces, mut tri_shape, mut tri_kind, mut tri_elem, nav, water, volumes, movers, climbables, .. } = self;
        tri_surface.resize(tris.len(), 0);
        tri_shape.resize(tris.len(), shape::ALL);
        tri_kind.resize(tris.len(), hit_kind::COMPLEX);
        tri_elem.resize(tris.len(), u32::MAX);
        // Element ids only need to differ from their neighbours' (`index_elements`).
        let mut next = tri_elem.iter().filter(|e| **e != u32::MAX).max().map_or(0, |m| m + 1);
        for (source, channels, kind, elems) in extra {
            let src = sources.len() as u32;
            sources.push(source.clone());
            for e in elems {
                for t in e {
                    let n = (t[1] - t[0]).cross(t[2] - t[0]);
                    if n.length_squared() < 1e-10 {
                        continue;
                    }
                    tris.push(Tri { a: t[0], b: t[1], c: t[2], n: n.normalize(), channels: *channels, source: src });
                    tri_surface.push(0);
                    tri_shape.push(shape::ALL);
                    tri_kind.push(*kind);
                    tri_elem.push(next);
                }
                next += 1;
            }
        }
        let mut w = CollisionWorld::from_triangles(tris, sources);
        w.tri_surface = tri_surface;
        w.tri_shape = tri_shape;
        w.tri_kind = tri_kind;
        w.tri_elem = tri_elem;
        w.index_elements();
        w.surfaces = surfaces;
        w.nav = nav;
        w.water = water;
        w.volumes = volumes;
        w.movers = movers;
        w.climbables = climbables;
        w
    }
}

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
        CollisionWorld { tris, bvh, sources, bounds: Some(total), nav: None, water: None, volumes: None, climbables: None, tri_surface: Vec::new(), surfaces: Vec::new(), tri_shape: Vec::new(), tri_kind: Vec::new(), tri_elem: Vec::new(), elems: Vec::new(), elem_bvh: Bvh::default(), movers: Vec::new() }
    }

    /// Build the element table from `tri_elem` (runs of equal ids; consecutive elements must
    /// have different ids) and renumber `tri_elem` to it.
    pub fn index_elements(&mut self) {
        self.elems.clear();
        let mut i = 0usize;
        while i < self.tri_elem.len().min(self.tris.len()) {
            let id = self.tri_elem[i];
            if id == u32::MAX {
                i += 1;
                continue;
            }
            let start = i;
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            while i < self.tri_elem.len() && self.tri_elem[i] == id {
                let t = &self.tris[i];
                lo = lo.min(t.a).min(t.b).min(t.c);
                hi = hi.max(t.a).max(t.b).max(t.c);
                i += 1;
            }
            let e = self.elems.len() as u32;
            for k in start..i {
                self.tri_elem[k] = e;
            }
            self.elems.push(Elem { first: start as u32, count: (i - start) as u32, min: lo, max: hi });
        }
        let bounds: Vec<Aabb> = self.elems.iter().map(|e| Aabb { min: e.min, max: e.max }).collect();
        self.elem_bvh = Bvh::build(&bounds);
    }

    /// An element answering `channels` and the check shape `want` that contains `p`.
    pub fn element_at(&self, p: Vec3, channels: u8, want: u8) -> Option<u32> {
        if self.elems.is_empty() {
            return None;
        }
        let mut found = None;
        self.elem_bvh.query_aabb(&Aabb { min: p, max: p }, |e| {
            if found.is_some() {
                return;
            }
            let el = &self.elems[e as usize];
            if self.tris[el.first as usize].channels & channels == 0 || self.shape_of(el.first) & want == 0 {
                return;
            }
            if (el.first..el.first + el.count).all(|i| {
                let t = &self.tris[i as usize];
                t.n.dot(p - t.a) <= 1e-3
            }) {
                found = Some(e);
            }
        });
        found
    }

    /// The element of a triangle, if it belongs to one.
    pub fn elem_of(&self, tri: u32) -> Option<u32> {
        self.tri_elem.get(tri as usize).copied().filter(|e| *e != u32::MAX)
    }

    /// The checks a triangle answers ([`shape`] bits).
    pub fn shape_of(&self, tri: u32) -> u8 {
        self.tri_shape.get(tri as usize).copied().unwrap_or(shape::ALL)
    }

    /// What a triangle came from ([`hit_kind`]).
    pub fn kind_of(&self, tri: u32) -> u8 {
        self.tri_kind.get(tri as usize).copied().unwrap_or(hit_kind::COMPLEX)
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
        let (mut tri_shape, mut tri_kind, mut tri_elem) = (Vec::new(), Vec::new(), Vec::new());
        // Element ids: per instance local ids made distinct from the neighbours'.
        let mut elem_base = 0u32;
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
            let mut used = 0u32;
            for (t, sec, info) in instance_triangles(map, inst) {
                let n = (t[1] - t[0]).cross(t[2] - t[0]);
                if n.length_squared() < 1e-10 {
                    continue;
                }
                tris.push(Tri { a: t[0], b: t[1], c: t[2], n: n.normalize(), channels: info.channels, source: src });
                tri_surface.push(sec.and_then(|s| section_surface.get(s).copied()).unwrap_or(section_surface.first().copied().unwrap_or(0)));
                tri_shape.push(info.shape);
                tri_kind.push(info.kind);
                tri_elem.push(if info.elem == u32::MAX { u32::MAX } else { used = used.max(info.elem + 1); elem_base + info.elem });
            }
            elem_base += used;
        }
        let mut w = Self::from_triangles(tris, sources);
        w.tri_surface = tri_surface;
        w.surfaces = surfaces;
        w.tri_shape = tri_shape;
        w.tri_kind = tri_kind;
        w.tri_elem = tri_elem;
        w.index_elements();
        // Movers: their triangles where the map placed them, one entry per actor.
        for inst in map.instances.iter().filter(|i| i.mover.is_some()) {
            let m = inst.mover.as_ref().unwrap();
            let t = instance_triangles(map, inst);
            if t.is_empty() {
                continue;
            }
            let x = match w.movers.iter().position(|x| x.actor == m.actor) {
                Some(i) => &mut w.movers[i],
                None => {
                    w.movers.push(MoverCollision { actor: m.actor.clone(), actor_to_world: m.actor_to_world, tris: Vec::new(), info: Vec::new(), poses: Vec::new(), initial: 0 });
                    w.movers.last_mut().unwrap()
                }
            };
            let base = x.info.iter().filter(|i| i.elem != u32::MAX).map(|i| i.elem + 1).max().unwrap_or(0);
            for (tri, _, mut info) in t {
                if info.elem != u32::MAX {
                    info.elem += base;
                }
                x.tris.push(tri);
                x.info.push(info);
            }
        }
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

    fn movers(&self) -> &[MoverCollision] {
        &self.movers
    }

    fn physics_volumes(&self) -> &[PhysicsVolume] {
        self.volumes.as_deref().map(Vec::as_slice).unwrap_or(&[])
    }

    fn climbables(&self) -> &[Climbable] {
        self.climbables.as_deref().map_or(&[], |c| c.as_slice())
    }

    fn box_contacts(&self, b: &PawnBox, channels: u8, out: &mut Vec<Contact>) {
        self.bvh.query_aabb(&b.aabb().expand(SKIN), |i| {
            let t = &self.tris[i as usize];
            if t.channels & channels == 0 || self.shape_of(i) & shape::extent_for(channels) == 0 {
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

    /// The original's extent check ([`sat`]): every per-polygon triangle and convex element in
    /// the way, swept by separating axes; the earliest entry wins, backed off along the move as
    /// the engine reports it ([`hit_kind::engine_time`]). Without a move it reports an overlap.
    fn sweep_box(&self, b: &PawnBox, delta: Vec3, channels: u8) -> Option<SweepHit> {
        let mut contacts = Vec::new();
        let len = delta.length();
        if len < 1e-6 {
            self.box_contacts(b, channels, &mut contacts);
            let k = contacts.iter().filter(|k| k.depth > SKIN).max_by(|a, c| a.depth.total_cmp(&c.depth)).copied()?;
            let face_up = self.face_up(&contacts);
            return Some(SweepHit { fraction: 0.0, normal: k.normal, point: k.point, start_penetrating: true, face_up, tri: k.tri });
        }
        let dir = delta / len;
        let want = shape::extent_for(channels);
        let swept = b.aabb().union(&b.at(b.center + delta).aabb()).expand(1.0);
        // Best: (fraction, normal, triangle, start penetrating).
        let mut best: Option<(f32, Vec3, u32, bool)> = None;
        let mut consider = |r: Option<sat::SatHit>, i: u32| {
            let cand = match r {
                Some(sat::SatHit::At(f, n)) => (f, n.normalize_or(-dir), false),
                Some(sat::SatHit::Start) => (0.0, -dir, true),
                None => return,
            };
            if best.is_none_or(|x| cand.0 < x.0) {
                best = Some((cand.0, cand.1, i, cand.2));
            }
        };
        // Loose triangles (per-polygon collision) whose leaves overlap the swept box.
        self.bvh.query_aabb(&swept, |i| {
            let t = &self.tris[i as usize];
            if t.channels & channels == 0 || self.shape_of(i) & want == 0 || self.elem_of(i).is_some() {
                return;
            }
            consider(sat::box_triangle(b.center, b.half, delta, t.a, t.b, t.c), i);
        });
        // Convex elements by their own bounds: a box inside a large element overlaps none of its
        // faces' leaves.
        let mut elem_tris: Vec<[Vec3; 3]> = Vec::new();
        self.elem_bvh.query_aabb(&swept, |e| {
            let el = self.elems[e as usize];
            if !(Aabb { min: el.min, max: el.max }).overlaps(&swept) {
                return;
            }
            let t = &self.tris[el.first as usize];
            if t.channels & channels == 0 || self.shape_of(el.first) & want == 0 {
                return;
            }
            elem_tris.clear();
            elem_tris.extend((el.first..el.first + el.count).map(|k| {
                let x = &self.tris[k as usize];
                [x.a, x.b, x.c]
            }));
            consider(sat::box_convex(b.center, b.half, delta, &elem_tris), el.first);
        });
        let (f, normal, mut tri, start) = best?;
        // Contacts just past the hit (or where it starts): the faces it rests on, for walking.
        let at = b.at(b.center + delta * f + dir * if start { 0.0 } else { 0.1 });
        // An element's hit face: the one facing the hit normal best.
        if let Some(e) = self.elem_of(tri) {
            let el = self.elems[e as usize];
            tri = (el.first..el.first + el.count).max_by(|&x, &y| self.tris[x as usize].n.dot(normal).total_cmp(&self.tris[y as usize].n.dot(normal))).unwrap_or(tri);
        }
        self.box_contacts(&at, channels, &mut contacts);
        let face_up = self.face_up(&contacts);
        let t = &self.tris[tri as usize];
        let point = closest_point_triangle(at.center, t.a, t.b, t.c);
        let fraction = match self.kind_of(tri) {
            hit_kind::OTHER => (f - SKIN / len).max(0.0),
            k => hit_kind::engine_time(k, f, len),
        };
        Some(SweepHit { fraction, normal, point, start_penetrating: start, face_up, tri })
    }

    fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32, channels: u8) -> Option<RayHit> {
        let d = dir.normalize_or_zero();
        if d == Vec3::ZERO {
            return None;
        }
        let mut best: Option<RayHit> = None;
        self.bvh.query_ray(origin, d, max_dist, |i, max_t| {
            let t = &self.tris[i as usize];
            if t.channels & channels == 0 || self.shape_of(i) & shape::line_for(channels) == 0 {
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
        // A line that starts inside a convex element (simplified collision, a brush hull, a
        // physics body) hits it where it starts, as the original's convex and box element checks
        // report (time 0).
        if let Some(e) = self.element_at(origin, channels, shape::line_for(channels)) {
            let t = self.elems[e as usize].first;
            return Some(RayHit { distance: 0.0, point: origin, normal: -d, tri: t });
        }
        best
    }

    fn capsule_contacts(&self, cap: &Capsule, channels: u8, out: &mut Vec<Contact>) {
        let (p0, p1) = cap.segment();
        let r = cap.radius;
        self.bvh.query_aabb(&cap.aabb().expand(SKIN), |i| {
            let t = &self.tris[i as usize];
            if t.channels & channels == 0 || self.shape_of(i) & shape::extent_for(channels) == 0 {
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
            return Some(SweepHit { fraction: 0.0, normal: k.normal, point: k.point, start_penetrating: true, face_up, tri: k.tri });
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
        Some(SweepHit { fraction: (lo - back).max(0.0), normal, point, start_penetrating: false, face_up, tri: contacts.iter().max_by(|a, c| a.depth.total_cmp(&c.depth)).map_or(u32::MAX, |c| c.tri) })
    }
}

/// The instance's world-space triangles with their section (kDOP and render triangles) and what
/// they answer: the channels of its trace types; the simplified collision (one element per
/// `AggGeom` element) for the checks its mesh's `UseSimple*Collision` flags give it, the kDOP
/// triangles (render triangles of colliding sections when the mesh has none) for the others
/// and for checks that ask for complex collision (a mesh that wants its simplified collision but
/// has none answers those checks with nothing).
fn instance_triangles(map: &DishonoredMap, inst: &dishonored_ir::MeshInstance) -> Vec<([Vec3; 3], Option<usize>, TriInfo)> {
    let mesh = &map.meshes[inst.mesh];
    let xf = inst.transform;
    let mirrored = xf.determinant() < 0.0;
    let channels = channel::from_trace_mask(inst.trace_mask);
    let simple = (if mesh.simple_line { shape::LINE } else { 0 }) | (if mesh.simple_box { shape::EXTENT } else { 0 });
    let complex = shape::BOTH & !simple;
    let mut out = Vec::new();
    if let Some(agg) = &mesh.simple_collision {
        if simple != 0 {
            for (ei, e) in agg.iter().enumerate() {
                let info = TriInfo { channels, shape: simple, kind: hit_kind::SIMPLE, elem: ei as u32 };
                for t in e {
                    let (a, b, c) = (xf.transform_point3(t[0]), xf.transform_point3(t[1]), xf.transform_point3(t[2]));
                    out.push((if mirrored { [a, c, b] } else { [a, b, c] }, None, info));
                }
            }
        }
    }
    // The kDOP triangles answer the checks the simplified collision does not, and every check
    // that asks for complex collision.
    let info = TriInfo { channels, shape: complex | shape::COMPLEX, kind: hit_kind::COMPLEX, elem: u32::MAX };
    let mut push = |i0: u32, i1: u32, i2: u32, sec: usize| {
        let (Some(a), Some(b), Some(c)) = (mesh.positions.get(i0 as usize), mesh.positions.get(i1 as usize), mesh.positions.get(i2 as usize)) else {
            return;
        };
        let (a, b, c) = (xf.transform_point3(*a), xf.transform_point3(*b), xf.transform_point3(*c));
        out.push((if mirrored { [a, c, b] } else { [a, b, c] }, Some(sec), info));
    };
    if !mesh.collision_triangles.is_empty() {
        for (i, t) in mesh.collision_triangles.iter().enumerate() {
            // The kDOP tree keeps every triangle; UE3's static mesh collision provider skips
            // those of sections without `EnableCollision` at query time
            // (`FStaticMeshCollisionDataProvider::ShouldCheckMaterial`).
            let m = mesh.collision_materials.get(i).map(|m| *m as usize);
            if m.and_then(|m| mesh.sections.get(m)).is_some_and(|s| !s.enable_collision) {
                continue;
            }
            push(t[0], t[1], t[2], m.unwrap_or(0));
        }
    } else {
        for (si, s) in mesh.sections.iter().enumerate().filter(|s| s.1.enable_collision) {
            let start = s.first_index as usize;
            let end = (start + s.num_triangles as usize * 3).min(mesh.indices.len());
            for t in mesh.indices[start..end].chunks_exact(3) {
                push(t[0], t[1], t[2], si);
            }
        }
    }
    out
}

/// The movers of a world where the map places them and its doors in their initial state: the
/// collision of game modes that do not run the level's Kismet (free-for-all, Contracts).
/// Rebuilt when the world's mover list changes.
#[derive(Default)]
pub struct StartLayers {
    key: (usize, usize),
    pub layers: Vec<CollisionWorld>,
}

impl StartLayers {
    pub fn update(&mut self, world: &dyn WorldQuery) {
        let m = world.movers();
        let key = (m.as_ptr() as usize, m.len());
        if key != self.key {
            self.layers = m.iter().map(|x| x.placed(glam::Mat4::IDENTITY)).collect();
            self.key = key;
        }
    }
}

/// A world with extra collision layered over it (movers placed where their actors are):
/// queries answer for the nearest hit over all layers.
pub struct Layered<'a> {
    pub base: &'a dyn WorldQuery,
    pub layers: &'a [CollisionWorld],
}

fn nearer(a: Option<SweepHit>, b: Option<SweepHit>) -> Option<SweepHit> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.start_penetrating && !x.start_penetrating || !x.start_penetrating && y.fraction < x.fraction { y } else { x }),
        (x, None) => x,
        (None, y) => y,
    }
}

impl WorldQuery for Layered<'_> {
    fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32, channels: u8) -> Option<RayHit> {
        let mut best = self.base.raycast(origin, dir, max_dist, channels);
        for l in self.layers {
            let reach = best.map(|h| h.distance).unwrap_or(max_dist);
            if let Some(h) = l.raycast(origin, dir, reach, channels) {
                if best.is_none_or(|b| h.distance < b.distance) {
                    best = Some(h);
                }
            }
        }
        best
    }
    fn capsule_contacts(&self, cap: &Capsule, channels: u8, out: &mut Vec<Contact>) {
        self.base.capsule_contacts(cap, channels, out);
        for l in self.layers {
            l.capsule_contacts(cap, channels, out);
        }
    }
    fn sweep_capsule(&self, cap: &Capsule, delta: Vec3, channels: u8) -> Option<SweepHit> {
        self.layers.iter().fold(self.base.sweep_capsule(cap, delta, channels), |acc, l| nearer(acc, l.sweep_capsule(cap, delta, channels)))
    }
    fn nav(&self) -> Option<&nav::NavWorld> {
        self.base.nav()
    }
    fn box_contacts(&self, b: &PawnBox, channels: u8, out: &mut Vec<Contact>) {
        self.base.box_contacts(b, channels, out);
        for l in self.layers {
            l.box_contacts(b, channels, out);
        }
    }
    fn sweep_box(&self, b: &PawnBox, delta: Vec3, channels: u8) -> Option<SweepHit> {
        self.layers.iter().fold(self.base.sweep_box(b, delta, channels), |acc, l| nearer(acc, l.sweep_box(b, delta, channels)))
    }
    fn water_at(&self, p: Vec3) -> Option<WaterInfo> {
        self.base.water_at(p)
    }
    fn movers(&self) -> &[MoverCollision] {
        self.base.movers()
    }
    fn physics_volumes(&self) -> &[PhysicsVolume] {
        self.base.physics_volumes()
    }
    fn climbables(&self) -> &[Climbable] {
        self.base.climbables()
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
    fn hull_faces_close_a_box() {
        // x in [0, 100], y in [-50, 50], z in [0, 20].
        let planes = [[1., 0., 0., 100.], [-1., 0., 0., 0.], [0., 1., 0., 50.], [0., -1., 0., 50.], [0., 0., 1., 20.], [0., 0., -1., 0.]];
        let t = hull_triangles(&planes);
        assert_eq!(t.len(), 12);
        let centre = Vec3::new(50.0, 0.0, 10.0);
        for f in &t {
            let n = (f[1] - f[0]).cross(f[2] - f[0]);
            assert!(n.dot(f[0] - centre) > 0.0, "inward face {f:?}");
            for p in f {
                assert!(p.x > -0.01 && p.x < 100.01 && p.y.abs() < 50.01 && p.z > -0.01 && p.z < 20.01, "{p:?}");
            }
        }
        // A rotated, scaled box gives the same twelve faces.
        let m = glam::Mat4::from_scale_rotation_translation(Vec3::new(10.0, 20.0, 30.0), glam::Quat::from_rotation_z(0.7), Vec3::new(5.0, 6.0, 7.0));
        let b = box_triangles(m);
        assert_eq!(b.len(), 12);
        let area: f32 = b.iter().map(|f| (f[1] - f[0]).cross(f[2] - f[0]).length() * 0.5).sum();
        assert!((area - 2.0 * 4.0 * (10.0 * 20.0 + 20.0 * 30.0 + 10.0 * 30.0)).abs() < 1.0, "{area}");
    }

    #[test]
    fn blockers_added_to_a_world_stop_pawns_only() {
        let w = floor_and_wall();
        let n = w.tris.len();
        // A wall of a blocking volume at x = -300, pawn channel only.
        let blocker = box_triangles(glam::Mat4::from_scale_rotation_translation(Vec3::new(10.0, 500.0, 300.0), glam::Quat::IDENTITY, Vec3::new(-300.0, 0.0, 300.0)));
        let w = w.with_extra(&[("BlockingVolume_0".to_string(), channel::PAWN, blocker)]);
        assert_eq!(w.tris.len(), n + 12);
        assert_eq!(w.sources.last().map(String::as_str), Some("BlockingVolume_0"));
        let b = PawnBox::new(Vec3::new(0.0, 0.0, 100.0), 30.0, 90.0);
        let h = w.sweep_box(&b, Vec3::new(-600.0, 0.0, 0.0), channel::PAWN).expect("blocked");
        assert!(h.fraction > 0.4 && h.fraction < 0.5, "{}", h.fraction);
        // Traces (vision, projectiles) pass through.
        assert!(w.raycast(Vec3::new(0.0, 0.0, 100.0), -Vec3::X, 600.0, channel::VISION | channel::PROJECTILE).is_none());
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
        // Per-polygon (kDOP) triangles: the engine reports the hit 4 uu back along the move.
        let hit = w.sweep_box(&b, Vec3::new(0., 0., -300.), channel::ALL).unwrap();
        let z = 200.0 - 300.0 * hit.fraction;
        assert!((z - 91.5).abs() < 0.2, "rest z {z}");
        assert!(hit.normal.z > 0.99 && hit.face_up.z > 0.99);
        // Flat bottom: a box standing beside a ledge edge is still supported by it (a capsule
        // would roll off): the floor ends at x = 1000, the box centre is 20 past it.
        let edge = PawnBox::new(Vec3::new(1020., 0., 88.0), 35.0, 87.5);
        assert!(w.sweep_box(&edge, Vec3::new(0., 0., -10.), channel::ALL).is_some());
        let side = PawnBox::new(Vec3::new(0., 0., 100.), 35.0, 87.5);
        let hit = w.sweep_box(&side, Vec3::new(1000., 0., 0.), channel::ALL).unwrap();
        let x = 1000.0 * hit.fraction;
        assert!((x - (500.0 - 35.0 - 4.0)).abs() < 0.2, "stop x {x}");
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
    fn movers(&self) -> &[MoverCollision] {
        self.cur().movers()
    }
    fn physics_volumes(&self) -> &[PhysicsVolume] {
        self.cur().physics_volumes()
    }
    fn climbables(&self) -> &[Climbable] {
        self.cur().climbables()
    }
}
