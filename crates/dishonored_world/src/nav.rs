//! Pathfinding over Dishonored's original navigation mesh (`dishonored_ir::nav`, read from
//! the `L_*_Nav` packages; see `research/dishonored/NAVIGATION.md`).
//!
//! The graph follows the data as UE3's navmesh does: polys are nodes; edges are the cooked
//! `FNavMeshEdgeBase` (two-way), `FNavMeshBasicOneWayEdge` (owner poly -> other poly),
//! `FNavMeshCrossPylonEdge` and `FNavMeshPathObjectEdge` (from `poly_refs[0]` to
//! `poly_refs[1]`; path-object edges are gated by their door / breakable blocker).
//! `NAVEDGE_Removed` edges are skipped. The shipped meshes contain no mantle, drop-down or
//! jump edges, so paths never leave the walkable surface except through those edges.
//!
//! `ArkDynamicPylon`s (pylons riding movers) keep a disconnected flag and follow their base
//! in 50 uu translation steps; while disconnected their cross-pylon edges cost 10,000,000
//! (`research/dishonored/systems/dynamic_pylons.md`).
//!
//! A* runs over polys with edge centres as entry points (UE3 also costs moves between edge
//! centres); the corridor is then string-pulled with a funnel over the shared edges.

use dishonored_ir::nav::{NavData, NavEdgeClass, NavEdgeType, NavPylon};
use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// Agent radius used to keep string-pulled corners off walls (pawn cylinder radius 31).
pub const AGENT_RADIUS: f32 = 31.0;
const CELL: f32 = 256.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkKind {
    /// Shared two-way edge.
    Walk,
    /// `FNavMeshBasicOneWayEdge`.
    OneWay,
    /// `FNavMeshCrossPylonEdge`.
    CrossPylon,
    /// `FNavMeshPathObjectEdge` owned by `path_objects[i]` (`u16::MAX`: unknown actor).
    PathObject(u16),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavLink {
    pub to: u32,
    /// Portal endpoints (the shared edge).
    pub a: Vec3,
    pub b: Vec3,
    pub kind: LinkKind,
    /// Extra traversal cost on top of distance (`EffectiveEdgeLength - EdgeLength` for
    /// path-object edges).
    pub extra: f32,
    /// The pylon whose mesh holds the edge (its vertices give `a` / `b`).
    pub owner: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavPolyRt {
    pub verts: Vec<u32>,
    pub center: Vec3,
    pub normal: Vec3,
    pub min: Vec3,
    pub max: Vec3,
    pub height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathObjectState {
    pub guid: [u32; 4],
    pub name: String,
    pub class: String,
    pub blocking: bool,
}

/// One pylon of the map: where its polys and vertices sit in the [`NavWorld`] arrays.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PylonRt {
    pub name: String,
    pub package: String,
    /// First poly and poly count.
    pub polys: (u32, u32),
    /// First vertex and vertex count.
    pub verts: (u32, u32),
    /// `Location` where the level places it.
    pub placed: Vec3,
    /// `ArkDynamicPylon` state (None: a static `DisPylon`).
    pub dynamic: Option<DynamicPylonRt>,
}

/// An `ArkDynamicPylon` at run time (`research/dishonored/systems/dynamic_pylons.md`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DynamicPylonRt {
    /// The `Base` chain (nearest first) with each base's placed location and rotation.
    pub bases: Vec<dishonored_ir::nav::PylonBase>,
    /// `bNeedsCostCheck`: the cross-pylon edge cost hook (`CostFor`, slot 251) runs.
    pub needs_cost_check: bool,
    pub state: PylonLink,
}

/// The run-time state of an `ArkDynamicPylon` (kept by the mission host between steps).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PylonLink {
    /// `m_bIsDisconnected` (+0x39c bit 0).
    pub disconnected: bool,
    /// `Actor.Location` now.
    pub location: Vec3,
    /// `m_PreviousPosition` (+0x3ac): where the mesh was last moved to.
    pub previous: Vec3,
    /// How many times the pawns moving through it were told to repath (slot 245, 0x00934c40).
    pub aborts: u32,
}

impl PylonLink {
    /// `ArkDynamicPylon::PostBeginPlay` (0x00934b60): connected, `m_PreviousPosition` =
    /// `Location`.
    pub fn placed(location: Vec3) -> Self {
        PylonLink { disconnected: false, location, previous: location, aborts: 0 }
    }

    /// `ArkSeqAct_ChangePylonConnection` (0x00937090) on this pylon: Connect clears
    /// `m_bIsDisconnected`; Disconnect sets it and, when it was clear, aborts the pawns moving
    /// through the pylon (slot 245).
    pub fn set_connected(&mut self, connected: bool) {
        if connected {
            self.disconnected = false;
        } else {
            self.disconnect();
        }
    }

    fn disconnect(&mut self) {
        if !self.disconnected {
            self.disconnected = true;
            self.aborts += 1;
        }
    }

    /// The pylon is at `location` after its base moved (`UpdateComponents` -> slot 249,
    /// 0x009345b0, in game): it disconnects, and once it is at least 50 uu from
    /// `m_PreviousPosition` on some axis the mesh moves by the whole offset (returned) and
    /// `m_PreviousPosition` becomes `location`.
    pub fn moved(&mut self, location: Vec3) -> Option<Vec3> {
        self.location = location;
        self.disconnect();
        let delta = location - self.previous;
        if delta.abs().max_element() < DYNAMIC_PYLON_MOVE_STEP {
            return None;
        }
        self.previous = location;
        Some(delta)
    }
}

/// `ArkDynamicPylon` slot 249 (0x009345b0): the mesh is moved only once the pylon is this far
/// (on any axis) from where the mesh was last moved to.
pub const DYNAMIC_PYLON_MOVE_STEP: f32 = 50.0;
/// `ArkDynamicPylon::CostFor` (slot 251, 0x00933340): the cost of a cross-pylon edge of a
/// disconnected pylon.
pub const DISCONNECTED_EDGE_COST: f32 = 10_000_000.0;

/// The map's navigation, ready for queries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NavWorld {
    pub verts: Vec<Vec3>,
    pub polys: Vec<NavPolyRt>,
    pub links: Vec<Vec<NavLink>>,
    pub path_objects: Vec<PathObjectState>,
    /// Every pylon, in [`NavData::pylons`] order.
    pub pylons: Vec<PylonRt>,
    /// Pylon of each poly.
    pub poly_pylon: Vec<u16>,
    #[serde(skip)]
    grid: HashMap<(i32, i32), Vec<u32>>,
}

impl NavWorld {
    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        std::fs::write(path, bincode::serde::encode_to_vec(self, bincode::config::standard())?)?;
        Ok(())
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)?;
        let (mut w, _): (NavWorld, _) = bincode::serde::decode_from_slice(&bytes, bincode::config::standard())?;
        w.finish_load();
        Ok(w)
    }
}

/// Options for a path query.
#[derive(Debug, Clone, Copy)]
pub struct PathOpts {
    /// Traverse path objects that are currently blocking (locked doors, intact blockers).
    pub through_blocking: bool,
    /// Expansion budget.
    pub max_nodes: usize,
}

impl Default for PathOpts {
    fn default() -> Self {
        PathOpts { through_blocking: false, max_nodes: 20_000 }
    }
}

fn cell(x: f32, y: f32) -> (i32, i32) {
    ((x / CELL).floor() as i32, (y / CELL).floor() as i32)
}

fn cross2(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

/// Closest point on segment `a`-`b` to `p` (2D).
fn closest_on_segment(p: Vec2, a: Vec2, b: Vec2) -> (Vec2, f32) {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-6 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t, t)
}

/// Closest point on segment `a`-`b` to `p` in 3D (0x0040F270).
fn closest_on_segment3(p: Vec3, a: Vec3, b: Vec3) -> (Vec3, f32) {
    let ab = b - a;
    let t = if ab.length_squared() > 1e-6 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t, t)
}

/// Open-list entry of the cheapest-first searches.
#[derive(PartialEq)]
struct MinCost(f32, u32);
impl Eq for MinCost {}
impl Ord for MinCost {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
    }
}
impl PartialOrd for MinCost {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// The parameters of [`NavWorld::safe_from_threat`]: `DisNavMeshGoal_SafeFromThreat` and
/// `DisFleeConstraint` as `DisAISubStateFlee` sets them (0x00B331F0 / 0x00B33350).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SafeFromThreat {
    /// `m_vCachedThreatViewPoint` (0x00D12F40): a pawn's eyes, another actor's location 50 uu
    /// up.
    pub threat_view: Vec3,
    /// `DisFleeConstraint.m_vFleeFromLoc`: the threat's location.
    pub flee_from: Vec3,
    /// `m_fSystemicFleeMinEnvellope` / `m_fSystemicFleeMaxEnvellope`.
    pub min: f32,
    pub max: f32,
    /// The pathing pawn's collision radius.
    pub pawn_radius: f32,
    /// `m_fForbiddenRadius` / `m_fForbiddenAngle` (degrees).
    pub forbidden_radius: f32,
    pub forbidden_angle_deg: f32,
    /// `MaxPathVisits` (the goal's class default 2048).
    pub max_visits: usize,
}

impl NavWorld {
    pub fn from_data(data: &NavData) -> NavWorld {
        let mut w = NavWorld::default();
        for po in &data.path_objects {
            w.path_objects.push(PathObjectState { guid: po.guid, name: po.name.clone(), class: po.class.clone(), blocking: po.blocking });
        }
        // Global poly index of (pylon, local poly).
        let mut poly_base = Vec::with_capacity(data.pylons.len());
        for (pi, p) in data.pylons.iter().enumerate() {
            let vb = w.verts.len() as u32;
            let pb = w.polys.len() as u32;
            poly_base.push(pb);
            w.verts.extend(p.mesh.verts.iter().copied());
            for poly in &p.mesh.polys {
                w.polys.push(NavPolyRt { verts: poly.verts.iter().map(|&v| vb + v as u32).collect(), center: poly.center, normal: poly.normal, min: poly.min, max: poly.max, height: poly.height });
                w.poly_pylon.push(pi as u16);
            }
            // `ArkDynamicPylon::PostBeginPlay` (0x00934b60): connected, `m_PreviousPosition` =
            // `Location`.
            let dynamic = p.dynamic.as_ref().map(|d| DynamicPylonRt { bases: d.bases.clone(), needs_cost_check: d.needs_cost_check, state: PylonLink::placed(p.location) });
            w.pylons.push(PylonRt { name: p.name.clone(), package: p.package.clone(), polys: (pb, p.mesh.polys.len() as u32), verts: (vb, p.mesh.verts.len() as u32), placed: p.location, dynamic });
        }
        w.links = vec![Vec::new(); w.polys.len()];
        let find_pylon = |owner: usize, obj: i32, guid: &[u32; 4]| -> Option<usize> {
            let same_pkg = &data.pylons[owner].package;
            if obj > 0 {
                if let Some(i) = data.pylons.iter().position(|q: &NavPylon| &q.package == same_pkg && q.object_ref == obj) {
                    return Some(i);
                }
            }
            if *guid != [0; 4] {
                return data.pylons.iter().position(|q| q.nav_guid == *guid);
            }
            None
        };
        for (pi, p) in data.pylons.iter().enumerate() {
            let m = &p.mesh;
            let base = poly_base[pi];
            let n = m.polys.len() as u32;
            let vert = |i: u16| m.verts.get(i as usize).copied();
            for (ei, e) in m.edges.iter().enumerate() {
                if e.kind == NavEdgeType::Removed {
                    continue;
                }
                let (Some(a), Some(b)) = (vert(e.v0), vert(e.v1)) else { continue };
                match e.class {
                    NavEdgeClass::Base => {
                        let (p0, p1) = (e.poly0 as u32, e.poly1 as u32);
                        if p0 < n && p1 < n && p0 != p1 {
                            w.links[(base + p0) as usize].push(NavLink { to: base + p1, a, b, kind: LinkKind::Walk, extra: 0.0, owner: pi as u16 });
                            w.links[(base + p1) as usize].push(NavLink { to: base + p0, a, b, kind: LinkKind::Walk, extra: 0.0, owner: pi as u16 });
                        }
                    }
                    NavEdgeClass::OneWay => {
                        let (p0, p1) = (e.poly0 as u32, e.poly1 as u32);
                        if p0 >= n || p1 >= n || p0 == p1 {
                            continue;
                        }
                        // The owner (the poly listing the edge) can leave through it.
                        let owns = |q: u32| m.polys[q as usize].edges.contains(&(ei as u16));
                        let (from, to) = if owns(p1) && !owns(p0) { (p1, p0) } else { (p0, p1) };
                        w.links[(base + from) as usize].push(NavLink { to: base + to, a, b, kind: LinkKind::OneWay, extra: 0.0, owner: pi as u16 });
                    }
                    NavEdgeClass::CrossPylon | NavEdgeClass::PathObject => {
                        let Some(l) = &e.path_object else { continue };
                        let ends: Vec<Option<u32>> = l
                            .poly_refs
                            .iter()
                            .map(|(obj, guid, poly, _sub)| {
                                let q = find_pylon(pi, *obj, guid)?;
                                let qn = data.pylons[q].mesh.polys.len() as u32;
                                ((*poly as u32) < qn).then(|| poly_base[q] + *poly as u32)
                            })
                            .collect();
                        let (Some(from), Some(to)) = (ends[0], ends[1]) else { continue };
                        if from == to {
                            continue;
                        }
                        let kind = if e.class == NavEdgeClass::PathObject {
                            let idx = w.path_objects.iter().position(|o| o.guid == l.path_object.1).map(|i| i as u16).unwrap_or(u16::MAX);
                            LinkKind::PathObject(idx)
                        } else {
                            LinkKind::CrossPylon
                        };
                        w.links[from as usize].push(NavLink { to, a, b, kind, extra: (e.effective_length - e.length).max(0.0), owner: pi as u16 });
                    }
                }
            }
        }
        w.build_grid();
        w
    }

    fn build_grid(&mut self) {
        self.grid.clear();
        for (i, p) in self.polys.iter().enumerate() {
            let (x0, y0) = cell(p.min.x, p.min.y);
            let (x1, y1) = cell(p.max.x, p.max.y);
            for x in x0..=x1 {
                for y in y0..=y1 {
                    self.grid.entry((x, y)).or_default().push(i as u32);
                }
            }
        }
    }

    /// Rebuild the spatial index after deserialization.
    pub fn finish_load(&mut self) {
        if self.grid.is_empty() {
            self.build_grid();
        }
    }

    pub fn link_count(&self) -> usize {
        self.links.iter().map(|l| l.len()).sum()
    }

    /// Height of poly `i`'s plane at `xy`.
    pub fn poly_z(&self, i: u32, x: f32, y: f32) -> f32 {
        let p = &self.polys[i as usize];
        if p.normal.z.abs() < 1e-3 {
            return p.center.z;
        }
        p.center.z - (p.normal.x * (x - p.center.x) + p.normal.y * (y - p.center.y)) / p.normal.z
    }

    fn contains_xy(&self, i: u32, q: Vec2, eps: f32) -> bool {
        let p = &self.polys[i as usize];
        if q.x < p.min.x - eps || q.x > p.max.x + eps || q.y < p.min.y - eps || q.y > p.max.y + eps {
            return false;
        }
        let n = p.verts.len();
        let (mut pos, mut neg) = (false, false);
        for k in 0..n {
            let a = self.verts[p.verts[k] as usize].truncate();
            let b = self.verts[p.verts[(k + 1) % n] as usize].truncate();
            let e = b - a;
            let len = e.length().max(1e-3);
            let c = cross2(e, q - a) / len;
            if c > eps {
                pos = true;
            } else if c < -eps {
                neg = true;
            }
            if pos && neg {
                return false;
            }
        }
        true
    }

    fn candidates(&self, q: Vec2, radius: f32) -> Vec<u32> {
        let (x0, y0) = cell(q.x - radius, q.y - radius);
        let (x1, y1) = cell(q.x + radius, q.y + radius);
        let mut out = Vec::new();
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.grid.get(&(x, y)) {
                    out.extend_from_slice(v);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The poly under `p` (a floor point or a pawn centre): the highest poly whose plane is at
    /// most `up` above and `down` below `p`.
    pub fn locate(&self, p: Vec3, up: f32, down: f32) -> Option<u32> {
        let q = p.truncate();
        self.candidates(q, 0.0)
            .into_iter()
            .filter(|&i| self.contains_xy(i, q, 0.5))
            .map(|i| (i, self.poly_z(i, q.x, q.y)))
            .filter(|&(_, z)| z <= p.z + up && z >= p.z - down)
            .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
            .map(|(i, _)| i)
    }

    /// Closest point on the navmesh to `p` within `radius` (XY) and a vertical window
    /// (pawn centres sit ~90 uu above the floor).
    pub fn closest_point(&self, p: Vec3, radius: f32) -> Option<(u32, Vec3)> {
        self.closest_point_in(p, radius, 60.0, 260.0)
    }

    /// [`NavWorld::closest_point`] with an explicit vertical window: floor points at most `up`
    /// above and `down` below `p`.
    pub fn closest_point_in(&self, p: Vec3, radius: f32, up: f32, down: f32) -> Option<(u32, Vec3)> {
        if let Some(i) = self.locate(p, up, down) {
            return Some((i, Vec3::new(p.x, p.y, self.poly_z(i, p.x, p.y))));
        }
        let q = p.truncate();
        let mut best: Option<(f32, u32, Vec3)> = None;
        for i in self.candidates(q, radius) {
            let poly = &self.polys[i as usize];
            let n = poly.verts.len();
            let mut cp = q;
            let mut bd = f32::INFINITY;
            for k in 0..n {
                let a = self.verts[poly.verts[k] as usize].truncate();
                let b = self.verts[poly.verts[(k + 1) % n] as usize].truncate();
                let (c, _) = closest_on_segment(q, a, b);
                let d = c.distance(q);
                if d < bd {
                    bd = d;
                    cp = c;
                }
            }
            // Nudge inside so `locate` on the result succeeds.
            let inward = (poly.center.truncate() - cp).normalize_or_zero() * 2.0;
            let cp = cp + inward;
            let z = self.poly_z(i, cp.x, cp.y);
            if z > p.z + up || z < p.z - down || bd > radius {
                continue;
            }
            let score = bd + (p.z - z - 90.0).abs() * 0.25;
            if best.is_none_or(|b| score < b.0) {
                best = Some((score, i, Vec3::new(cp.x, cp.y, z)));
            }
        }
        best.map(|(_, i, v)| (i, v))
    }

    fn link_open(&self, l: &NavLink, opts: &PathOpts) -> bool {
        match l.kind {
            LinkKind::PathObject(i) if !opts.through_blocking => self.path_objects.get(i as usize).is_none_or(|o| !o.blocking),
            _ => true,
        }
    }

    /// The cost a dynamic pylon puts on a link leaving poly `from`, when its cost hook decides
    /// it. Cross-pylon edges (0x0067ab00) ask the pylon of their first poly, else of their
    /// second, whichever has `bNeedsCostCheck`; `ArkDynamicPylon::CostFor` (0x00933340) then
    /// costs a cross-pylon edge (`+0x2d` set) 10,000,000 while `m_bIsDisconnected`. A
    /// path-object edge whose actor is known asks that actor instead (0x0067ace0); in-mesh
    /// edges are never cross-pylon, so a moving or disconnected pylon keeps its own polys
    /// walkable.
    pub fn pylon_cost(&self, from: u32, l: &NavLink) -> Option<f32> {
        match l.kind {
            LinkKind::CrossPylon | LinkKind::PathObject(u16::MAX) => {}
            _ => return None,
        }
        let pylon = |poly: u32| self.poly_pylon.get(poly as usize).and_then(|&i| self.pylons.get(i as usize));
        let judge = [pylon(from), pylon(l.to)].into_iter().flatten().find_map(|p| p.dynamic.as_ref().filter(|d| d.needs_cost_check))?;
        judge.state.disconnected.then_some(DISCONNECTED_EDGE_COST)
    }

    /// Pylon index by name (and package when given).
    pub fn pylon_index(&self, name: &str, package: Option<&str>) -> Option<usize> {
        self.pylons.iter().position(|p| p.name.eq_ignore_ascii_case(name) && package.is_none_or(|k| p.package.eq_ignore_ascii_case(k)))
    }

    /// `ArkSeqAct_ChangePylonConnection` on one pylon (0x00937090): Connect clears
    /// `m_bIsDisconnected`; Disconnect sets it and, when it was clear, makes the pawns moving
    /// through the pylon repath (slot 245). False when the pylon is not dynamic.
    pub fn set_pylon_connected(&mut self, pylon: usize, connected: bool) -> bool {
        let Some(d) = self.pylons.get_mut(pylon).and_then(|p| p.dynamic.as_mut()) else { return false };
        d.state.set_connected(connected);
        true
    }

    /// A dynamic pylon was moved to `location` (its base moved; `UpdateComponents` -> slot 249,
    /// 0x009345b0, in game): it disconnects (aborting the pawns moving through it when it was
    /// connected), and once it is at least 50 uu from `m_PreviousPosition` on some axis its
    /// walkable and obstacle meshes are translated by the whole offset (no rotation: the
    /// vertices, poly centres and bounds move, edges keep their vertices) and
    /// `m_PreviousPosition` becomes `location`. True when the mesh moved.
    pub fn move_dynamic_pylon(&mut self, pylon: usize, location: Vec3) -> bool {
        let Some(d) = self.pylons.get_mut(pylon).and_then(|p| p.dynamic.as_mut()) else { return false };
        let Some(delta) = d.state.moved(location) else { return false };
        self.translate_pylon(pylon, delta);
        true
    }

    /// Give a dynamic pylon a state kept elsewhere (the mission host's): its mesh moves to
    /// `state.previous`.
    pub fn apply_pylon_state(&mut self, pylon: usize, state: PylonLink) {
        let Some(d) = self.pylons.get_mut(pylon).and_then(|p| p.dynamic.as_mut()) else { return };
        let delta = state.previous - d.state.previous;
        d.state = state;
        if delta != Vec3::ZERO {
            self.translate_pylon(pylon, delta);
        }
    }

    /// Move a pylon's mesh by `delta` (vertices, polys, the portals of the edges it holds).
    pub fn translate_pylon(&mut self, pylon: usize, delta: Vec3) {
        let Some(p) = self.pylons.get(pylon) else { return };
        let (v0, vn) = p.verts;
        let (p0, pn) = p.polys;
        for v in &mut self.verts[v0 as usize..(v0 + vn) as usize] {
            *v += delta;
        }
        for poly in &mut self.polys[p0 as usize..(p0 + pn) as usize] {
            poly.center += delta;
            poly.min += delta;
            poly.max += delta;
        }
        for l in self.links.iter_mut().flatten().filter(|l| l.owner as usize == pylon) {
            l.a += delta;
            l.b += delta;
        }
        self.build_grid();
    }

    /// Dynamic pylons whose polys a corridor crosses, with their abort counts (a follower
    /// repaths when one changes: the pylon's `m_PawnsMovingThruPylon` were aborted).
    pub fn corridor_pylons(&self, corridor: &[(u32, Option<usize>)]) -> Vec<(u16, u32)> {
        let mut out: Vec<(u16, u32)> = Vec::new();
        for &(poly, _) in corridor {
            let Some(&pi) = self.poly_pylon.get(poly as usize) else { continue };
            if let Some(d) = self.pylons[pi as usize].dynamic.as_ref() {
                if !out.iter().any(|w| w.0 == pi) {
                    out.push((pi, d.state.aborts));
                }
            }
        }
        out
    }

    /// Set a path object's blocking state by name (door unlocked, blocker broken...).
    pub fn set_blocking(&mut self, name: &str, blocking: bool) -> bool {
        let mut any = false;
        for o in self.path_objects.iter_mut().filter(|o| o.name == name) {
            o.blocking = blocking;
            any = true;
        }
        any
    }

    /// A* over polys: the corridor (poly ids) and the links taken.
    pub fn corridor(&self, start: u32, goal: u32, goal_pos: Vec3, start_pos: Vec3, opts: &PathOpts) -> Option<Vec<(u32, Option<usize>)>> {
        #[derive(PartialEq)]
        struct E(f32, u32);
        impl Eq for E {}
        impl Ord for E {
            fn cmp(&self, o: &Self) -> Ordering {
                o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
            }
        }
        impl PartialOrd for E {
            fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
                Some(self.cmp(o))
            }
        }
        let n = self.polys.len();
        let mut g = vec![f32::INFINITY; n];
        let mut pos = vec![Vec3::ZERO; n];
        let mut came: Vec<(u32, usize)> = vec![(u32::MAX, 0); n];
        let mut closed = vec![false; n];
        let mut open = BinaryHeap::new();
        g[start as usize] = 0.0;
        pos[start as usize] = start_pos;
        open.push(E(start_pos.distance(goal_pos), start));
        let mut expanded = 0;
        while let Some(E(_, u)) = open.pop() {
            let ui = u as usize;
            if closed[ui] {
                continue;
            }
            closed[ui] = true;
            if u == goal {
                let mut out = vec![(u, None)];
                let mut c = u;
                while came[c as usize].0 != u32::MAX {
                    let (prev, li) = came[c as usize];
                    out.push((prev, Some(li)));
                    c = prev;
                }
                // Each entry holds the link leaving that poly (None for the goal).
                out.reverse();
                return Some(out);
            }
            expanded += 1;
            if expanded > opts.max_nodes {
                return None;
            }
            for (li, l) in self.links[ui].iter().enumerate() {
                let v = l.to as usize;
                if closed[v] || !self.link_open(l, opts) {
                    continue;
                }
                let mid = (l.a + l.b) * 0.5;
                let ng = g[ui] + self.pylon_cost(u, l).unwrap_or_else(|| pos[ui].distance(mid) + l.extra);
                if ng < g[v] {
                    g[v] = ng;
                    pos[v] = mid;
                    came[v] = (u, li);
                    open.push(E(ng + mid.distance(goal_pos), l.to));
                }
            }
        }
        None
    }

    /// A path search with a goal evaluator (UE3 `UNavigationHandle` path search with a
    /// `NavMeshGoal_*` evaluator and `NavMeshPathConstraint`s): polys are expanded from the one
    /// under `start` cheapest first, each entered through its link's edge centre as in
    /// [`NavWorld::corridor`]; `step(entry, link, edge centre, cost)` returns the link's cost
    /// after the constraints, or None to reject it. The first poly after the start for which
    /// `goal` returns a point ends the search; at most `max_visits` polys are expanded
    /// (`MaxPathVisits`). Returns that poly and the point.
    pub fn search_goal(&self, start: Vec3, opts: &PathOpts, max_visits: usize, mut step: impl FnMut(Vec3, &NavLink, Vec3, f32) -> Option<f32>, mut goal: impl FnMut(u32) -> Option<Vec3>) -> Option<(u32, Vec3)> {
        let (sp, s) = self.closest_point(start, 200.0)?;
        let n = self.polys.len();
        let mut g = vec![f32::INFINITY; n];
        let mut pos = vec![Vec3::ZERO; n];
        let mut closed = vec![false; n];
        let mut open = BinaryHeap::new();
        g[sp as usize] = 0.0;
        pos[sp as usize] = s;
        open.push(MinCost(0.0, sp));
        let mut visits = 0;
        while let Some(MinCost(_, u)) = open.pop() {
            let ui = u as usize;
            if closed[ui] {
                continue;
            }
            closed[ui] = true;
            if u != sp {
                if let Some(p) = goal(u) {
                    return Some((u, p));
                }
            }
            visits += 1;
            if visits > max_visits {
                return None;
            }
            for l in &self.links[ui] {
                let v = l.to as usize;
                if closed[v] || !self.link_open(l, opts) {
                    continue;
                }
                let mid = (l.a + l.b) * 0.5;
                let base = self.pylon_cost(u, l).unwrap_or_else(|| pos[ui].distance(mid) + l.extra);
                let Some(c) = step(pos[ui], l, mid, base) else { continue };
                let ng = g[ui] + c;
                if ng < g[v] {
                    g[v] = ng;
                    pos[v] = mid;
                    open.push(MinCost(ng, l.to));
                }
            }
        }
        None
    }

    /// The systemic flee destination (`DisAISubStateFlee` mode 2, 0x00B3BEA0): the path search
    /// with `DisNavMeshGoal_SafeFromThreat` (0x00D12F40 / 0x00D122D0) and `DisFleeConstraint`
    /// (0x00B054C0). Returns a floor point.
    ///
    /// - The goal tests each poly's edges in vertex order: the point of the edge closest to the
    ///   threat's view point (0x0040F270), moved toward the poly's centre by the pawn's radius
    ///   (to the centre when nearer), is the answer when its distance from the view point is
    ///   strictly between `min` and `max` (`m_fMinDistance` / `m_fMaxDistance`, the
    ///   sub-state's systemic flee envelopes; 0x00BC01A0). The start poly is never the goal.
    ///   (`m_bCheckVisibility`, off in every shipped goal, is not ported.)
    /// - The constraint lowers each link's cost by the distance from the threat to its edge
    ///   (0x00672A00, truncated to an int), so the search runs away from the threat, and rejects
    ///   a link whose edge centre lies within `forbidden_radius` of the threat in the cone of
    ///   `forbidden_angle_deg` from the start toward the threat.
    pub fn safe_from_threat(&self, start: Vec3, flee: &SafeFromThreat) -> Option<Vec3> {
        let step = |_from: Vec3, l: &NavLink, mid: Vec3, cost: f32| {
            let (c, _) = closest_on_segment3(flee.flee_from, l.a, l.b);
            let cost = cost - c.distance(flee.flee_from).trunc();
            if flee.forbidden_radius > 0.0 && mid.distance_squared(flee.flee_from) < flee.forbidden_radius * flee.forbidden_radius {
                let to_edge = (mid - start).normalize_or_zero();
                let to_threat = (flee.flee_from - start).normalize_or_zero();
                if to_edge.dot(to_threat) >= flee.forbidden_angle_deg.to_radians().cos() {
                    return None;
                }
            }
            Some(cost)
        };
        let goal = |i: u32| {
            let p = &self.polys[i as usize];
            let n = p.verts.len();
            for k in 0..n {
                let a = self.verts[p.verts[k] as usize];
                let b = self.verts[p.verts[(k + 1) % n] as usize];
                let (c, _) = closest_on_segment3(flee.threat_view, a, b);
                let to_centre = Vec3::new(p.center.x - c.x, p.center.y - c.y, 0.0);
                let cand = if flee.pawn_radius * flee.pawn_radius < to_centre.length_squared() { c + to_centre.normalize() * flee.pawn_radius } else { p.center };
                let d2 = cand.distance_squared(flee.threat_view);
                if flee.min * flee.min < d2 && d2 < flee.max * flee.max {
                    return Some(cand);
                }
            }
            None
        };
        self.search_goal(start, &PathOpts::default(), flee.max_visits, step, goal).map(|(_, p)| p)
    }

    /// Waypoints (floor points) from `start` to `goal`, excluding the start. `None` when either
    /// end is off the mesh or no route exists.
    pub fn find_path(&self, start: Vec3, goal: Vec3, opts: &PathOpts) -> Option<Vec<Vec3>> {
        self.find_path_watch(start, goal, opts).map(|(p, _)| p)
    }

    /// [`NavWorld::find_path`] with the dynamic pylons the route crosses
    /// ([`NavWorld::corridor_pylons`]).
    pub fn find_path_watch(&self, start: Vec3, goal: Vec3, opts: &PathOpts) -> Option<(Vec<Vec3>, Vec<(u16, u32)>)> {
        let (sp, s) = self.closest_point(start, 200.0)?;
        let (gp, g) = self.closest_point(goal, 300.0)?;
        if sp == gp {
            return Some((vec![g], self.corridor_pylons(&[(sp, None)])));
        }
        let corr = self.corridor(sp, gp, g, s, opts)?;
        let watch = self.corridor_pylons(&corr);
        // Portals oriented (left, right) along the direction of travel.
        let mut portals: Vec<(Vec3, Vec3)> = Vec::with_capacity(corr.len() + 1);
        portals.push((s, s));
        for &(p, li) in &corr {
            let Some(li) = li else { continue };
            let l = &self.links[p as usize][li];
            let from_c = self.polys[p as usize].center.truncate();
            let to_c = self.polys[l.to as usize].center.truncate();
            let dir = to_c - from_c;
            let (mut left, mut right) = if cross2(dir, l.a.truncate() - from_c) > cross2(dir, l.b.truncate() - from_c) { (l.a, l.b) } else { (l.b, l.a) };
            // Keep corners an agent radius away from the portal ends.
            let len = left.truncate().distance(right.truncate());
            let r = AGENT_RADIUS.min(len * 0.5 - 1.0).max(0.0);
            if len > 1.0 {
                let d = (right - left) / len;
                left += d * r;
                right -= d * r;
            }
            portals.push((left, right));
        }
        portals.push((g, g));
        Some((string_pull(&portals), watch))
    }
}

/// Simple stupid funnel (2D, Z carried from the portal points).
fn string_pull(portals: &[(Vec3, Vec3)]) -> Vec<Vec3> {
    let mut out: Vec<Vec3> = Vec::new();
    if portals.is_empty() {
        return out;
    }
    let mut apex = portals[0].0;
    let (mut left, mut right) = (portals[0].0, portals[0].1);
    let (mut left_i, mut right_i) = (0usize, 0usize);
    let mut apex_i;
    let tri = |a: Vec3, b: Vec3, c: Vec3| cross2(b.truncate() - a.truncate(), c.truncate() - a.truncate());
    let mut i = 1;
    let mut guard = 0;
    while i < portals.len() {
        guard += 1;
        if guard > portals.len() * 8 + 64 {
            break;
        }
        let (pl, pr) = portals[i];
        // Right side: narrowing when the new right is left of (or on) the current right.
        if tri(apex, right, pr) >= 0.0 {
            if apex.truncate().distance_squared(right.truncate()) < 1e-4 || tri(apex, left, pr) < 0.0 {
                right = pr;
                right_i = i;
            } else {
                // Right crossed left: left becomes a corner.
                out.push(left);
                apex = left;
                apex_i = left_i;
                left = apex;
                right = apex;
                left_i = apex_i;
                right_i = apex_i;
                i = apex_i + 1;
                continue;
            }
        }
        if tri(apex, left, pl) <= 0.0 {
            if apex.truncate().distance_squared(left.truncate()) < 1e-4 || tri(apex, right, pl) > 0.0 {
                left = pl;
                left_i = i;
            } else {
                out.push(right);
                apex = right;
                apex_i = right_i;
                left = apex;
                right = apex;
                left_i = apex_i;
                right_i = apex_i;
                i = apex_i + 1;
                continue;
            }
        }
        i += 1;
    }
    let last = portals[portals.len() - 1].0;
    if out.last().is_none_or(|p| p.distance(last) > 1.0) {
        out.push(last);
    }
    out.dedup_by(|a, b| a.distance(*b) < 1.0);
    out
}

/// Per-agent path following over a [`NavWorld`] (bots and NPC brains). Plain data so brains
/// stay serializable and deterministic.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Follower {
    pub path: Vec<Vec3>,
    pub goal: Option<Vec3>,
    /// Ticks until the next replan.
    pub replan_in: u32,
    /// The last query found no route (callers may give up on the goal).
    pub failed: bool,
    /// Dynamic pylons the path crosses with their abort counts when it was planned: the path
    /// is replanned when one of them aborts its pawns (`ArkDynamicPylon` slot 245).
    #[serde(default)]
    pub pylons: Vec<(u16, u32)>,
}

impl Follower {
    /// Point to steer toward this tick to reach `goal` from `me` (pawn centre or floor point).
    /// Without navigation (or off the mesh) it returns `goal` (straight line).
    pub fn steer(&mut self, nav: Option<&NavWorld>, me: Vec3, goal: Vec3) -> Vec3 {
        let Some(nav) = nav else {
            self.failed = false;
            return goal;
        };
        self.replan_in = self.replan_in.saturating_sub(1);
        let moved = self.goal.is_none_or(|g| g.truncate().distance(goal.truncate()) > 150.0 || (g.z - goal.z).abs() > 120.0);
        let aborted = self.pylons.iter().any(|&(p, n)| nav.pylons.get(p as usize).and_then(|p| p.dynamic.as_ref()).is_some_and(|d| d.state.aborts != n));
        if self.replan_in == 0 || moved || aborted {
            self.replan_in = 60;
            self.goal = Some(goal);
            match nav.find_path_watch(me, goal, &PathOpts::default()) {
                Some((p, w)) => {
                    self.path = p;
                    self.pylons = w;
                    self.failed = false;
                }
                None => {
                    self.path.clear();
                    self.pylons.clear();
                    self.failed = true;
                }
            }
        }
        let feet = me.truncate();
        while self.path.len() > 1 && self.path[0].truncate().distance(feet) < 48.0 {
            self.path.remove(0);
        }
        match self.path.first() {
            Some(&w) => Vec3::new(w.x, w.y, me.z.max(w.z)),
            None => goal,
        }
    }

    pub fn force_replan(&mut self) {
        self.replan_in = 0;
    }

    pub fn clear(&mut self) {
        *self = Follower::default();
    }

    /// Remaining path length from `me` (XY), if a path is held.
    pub fn remaining(&self, me: Vec3) -> Option<f32> {
        let first = self.path.first()?;
        let mut d = first.truncate().distance(me.truncate());
        for w in self.path.windows(2) {
            d += w[0].truncate().distance(w[1].truncate());
        }
        Some(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::nav::{NavEdge, NavMesh, NavPoly};

    /// Two unit squares side by side sharing an edge, plus a third reachable one-way.
    fn mesh() -> NavData {
        let v = |x: f32, y: f32| Vec3::new(x, y, 0.0);
        let verts = vec![v(0.0, 0.0), v(200.0, 0.0), v(200.0, 200.0), v(0.0, 200.0), v(400.0, 0.0), v(400.0, 200.0), v(600.0, 0.0), v(600.0, 200.0)];
        let poly = |vs: Vec<u16>, edges: Vec<u16>, cx: f32| NavPoly { verts: vs, edges, center: Vec3::new(cx, 100.0, 0.0), normal: Vec3::Z, min: Vec3::new(cx - 100.0, 0.0, 0.0), max: Vec3::new(cx + 100.0, 200.0, 0.0), height: 200.0 };
        let edge = |class, v0, v1, p0, p1| NavEdge { class, kind: NavEdgeType::Normal, v0, v1, poly0: p0, poly1: p1, length: 200.0, effective_length: 200.0, center: Vec3::ZERO, path_object: None };
        let m = NavMesh {
            source: "test".into(),
            version: (28, 28),
            verts,
            polys: vec![poly(vec![0, 1, 2, 3], vec![0], 100.0), poly(vec![1, 4, 5, 2], vec![0, 1], 300.0), poly(vec![4, 6, 7, 5], vec![], 500.0)],
            edges: vec![edge(NavEdgeClass::Base, 1, 2, 0, 1), edge(NavEdgeClass::OneWay, 4, 5, 2, 1)],
            min: Vec3::ZERO,
            max: Vec3::new(600.0, 200.0, 0.0),
        };
        NavData { pylons: vec![NavPylon { name: "p".into(), package: "t".into(), object_ref: 1, mesh: m, ..Default::default() }], path_objects: Vec::new() }
    }

    #[test]
    fn locate_and_path() {
        let w = NavWorld::from_data(&mesh());
        assert_eq!(w.locate(Vec3::new(50.0, 50.0, 90.0), 60.0, 260.0), Some(0));
        assert_eq!(w.locate(Vec3::new(350.0, 50.0, 0.0), 60.0, 260.0), Some(1));
        let p = w.find_path(Vec3::new(50.0, 50.0, 90.0), Vec3::new(350.0, 150.0, 90.0), &PathOpts::default()).unwrap();
        assert!(p.last().unwrap().distance(Vec3::new(350.0, 150.0, 0.0)) < 1.0, "{p:?}");
        // One-way: poly 1 owns the edge to poly 2.
        assert!(w.find_path(Vec3::new(50.0, 50.0, 0.0), Vec3::new(550.0, 100.0, 0.0), &PathOpts::default()).is_some());
        assert!(w.find_path(Vec3::new(550.0, 100.0, 0.0), Vec3::new(50.0, 50.0, 0.0), &PathOpts::default()).is_none());
    }

    /// Five 200 uu squares in a row along x (0..1000), each sharing an edge with the next.
    fn strip() -> NavData {
        let v = |x: f32, y: f32| Vec3::new(x, y, 0.0);
        let verts: Vec<Vec3> = (0..6).flat_map(|i| [v(i as f32 * 200.0, 0.0), v(i as f32 * 200.0, 200.0)]).collect();
        let poly = |i: u16| {
            let cx = i as f32 * 200.0 + 100.0;
            NavPoly { verts: vec![2 * i, 2 * i + 2, 2 * i + 3, 2 * i + 1], edges: vec![], center: Vec3::new(cx, 100.0, 0.0), normal: Vec3::Z, min: Vec3::new(cx - 100.0, 0.0, 0.0), max: Vec3::new(cx + 100.0, 200.0, 0.0), height: 200.0 }
        };
        let edge = |i: u16| NavEdge { class: NavEdgeClass::Base, kind: NavEdgeType::Normal, v0: 2 * i + 2, v1: 2 * i + 3, poly0: i, poly1: i + 1, length: 200.0, effective_length: 200.0, center: Vec3::ZERO, path_object: None };
        let m = NavMesh { source: "strip".into(), version: (28, 28), verts, polys: (0..5).map(poly).collect(), edges: (0..4).map(edge).collect(), min: Vec3::ZERO, max: Vec3::new(1000.0, 200.0, 0.0) };
        NavData { pylons: vec![NavPylon { name: "s".into(), package: "t".into(), object_ref: 1, mesh: m, ..Default::default() }], path_objects: Vec::new() }
    }

    #[test]
    fn safe_from_threat_takes_the_first_edge_point_beyond_the_envelope() {
        let w = NavWorld::from_data(&strip());
        let threat = Vec3::new(0.0, 100.0, 0.0);
        let mut f = SafeFromThreat { threat_view: threat + Vec3::Z * 50.0, flee_from: threat, min: 500.0, max: 30000.0, pawn_radius: 31.0, forbidden_radius: 0.0, forbidden_angle_deg: 0.0, max_visits: 2048 };
        let p = w.safe_from_threat(Vec3::new(100.0, 100.0, 90.0), &f).unwrap();
        // Poly 2's far edge (x 600), pulled 31 uu toward its centre: the first edge point of the
        // first poly beyond 500 uu.
        assert!((p - Vec3::new(569.0, 100.0, 0.0)).length() < 0.5, "{p:?}");
        f.min = 5000.0;
        assert_eq!(w.safe_from_threat(Vec3::new(100.0, 100.0, 90.0), &f), None);
    }

    #[test]
    fn the_flee_constraint_forbids_running_at_the_threat() {
        let w = NavWorld::from_data(&strip());
        // Fleeing from a threat on the right, from the middle: the cone toward it is closed, so
        // the answer lies to the left.
        let threat = Vec3::new(900.0, 100.0, 0.0);
        let f = SafeFromThreat { threat_view: threat, flee_from: threat, min: 450.0, max: 30000.0, pawn_radius: 31.0, forbidden_radius: 2000.0, forbidden_angle_deg: 60.0, max_visits: 2048 };
        let p = w.safe_from_threat(Vec3::new(500.0, 100.0, 90.0), &f).unwrap();
        assert!(p.x < 500.0, "{p:?}");
    }

    #[test]
    fn funnel_straight_line_when_clear() {
        let w = NavWorld::from_data(&mesh());
        let p = w.find_path(Vec3::new(20.0, 100.0, 0.0), Vec3::new(380.0, 100.0, 0.0), &PathOpts::default()).unwrap();
        assert_eq!(p.len(), 1, "{p:?}");
    }

    /// A static pylon `A` (polys at x 0..200 and 400..600, joined only by a long detour edge
    /// through y = 2000) and a dynamic pylon `B` (x 200..400) bridging them with cross-pylon
    /// edges: `A0 -> B0` held by A, `B0 -> A1` held by B (and the reverse ones).
    fn bridge() -> NavData {
        use dishonored_ir::nav::{DynamicPylon, NavPathObjectLink};
        let v = |x: f32, y: f32| Vec3::new(x, y, 0.0);
        let poly = |vs: Vec<u16>, cx: f32| NavPoly { verts: vs, edges: vec![], center: Vec3::new(cx, 100.0, 0.0), normal: Vec3::Z, min: Vec3::new(cx - 100.0, 0.0, 0.0), max: Vec3::new(cx + 100.0, 200.0, 0.0), height: 200.0 };
        let cross = |v0, v1, from: (i32, u16), to: (i32, u16)| NavEdge {
            class: NavEdgeClass::CrossPylon,
            kind: NavEdgeType::Normal,
            v0,
            v1,
            poly0: 0,
            poly1: 0,
            length: 200.0,
            effective_length: 200.0,
            center: Vec3::ZERO,
            path_object: Some(NavPathObjectLink { poly_refs: [(from.0, [0; 4], from.1, 0), (to.0, [0; 4], to.1, 0)], path_object: (0, [0; 4]), internal_id: -1 }),
        };
        let detour = |p0, p1| NavEdge { class: NavEdgeClass::Base, kind: NavEdgeType::Normal, v0: 8, v1: 9, poly0: p0, poly1: p1, length: 1.0, effective_length: 1.0, center: Vec3::ZERO, path_object: None };
        let a = NavMesh {
            source: "a".into(),
            version: (28, 28),
            verts: vec![v(0.0, 0.0), v(200.0, 0.0), v(200.0, 200.0), v(0.0, 200.0), v(400.0, 0.0), v(600.0, 0.0), v(600.0, 200.0), v(400.0, 200.0), v(300.0, 2000.0), v(301.0, 2000.0)],
            polys: vec![poly(vec![0, 1, 2, 3], 100.0), poly(vec![4, 5, 6, 7], 500.0)],
            edges: vec![detour(0, 1), cross(1, 2, (1, 0), (2, 0)), cross(4, 7, (1, 1), (2, 0))],
            min: Vec3::ZERO,
            max: Vec3::new(600.0, 2000.0, 0.0),
        };
        let b = NavMesh {
            source: "b".into(),
            version: (28, 28),
            verts: vec![v(200.0, 0.0), v(400.0, 0.0), v(400.0, 200.0), v(200.0, 200.0)],
            polys: vec![poly(vec![0, 1, 2, 3], 300.0)],
            edges: vec![cross(1, 2, (2, 0), (1, 1)), cross(0, 3, (2, 0), (1, 0))],
            min: Vec3::new(200.0, 0.0, 0.0),
            max: Vec3::new(400.0, 200.0, 0.0),
        };
        let dynamic = Some(DynamicPylon { needs_cost_check: true, ceiling_height: 700.0, ..Default::default() });
        NavData {
            pylons: vec![
                NavPylon { name: "A".into(), package: "t".into(), object_ref: 1, mesh: a, ..Default::default() },
                NavPylon { name: "B".into(), package: "t".into(), object_ref: 2, location: Vec3::new(300.0, 100.0, 0.0), mesh: b, dynamic, ..Default::default() },
            ],
            path_objects: Vec::new(),
        }
    }

    fn max_y(p: &[Vec3]) -> f32 {
        p.iter().map(|w| w.y).fold(f32::MIN, f32::max)
    }

    #[test]
    fn disconnected_dynamic_pylon_costs_its_cross_edges() {
        let mut w = NavWorld::from_data(&bridge());
        let b = w.pylon_index("B", Some("t")).unwrap();
        assert!(w.pylons[b].dynamic.is_some() && w.pylons[w.pylon_index("A", None).unwrap()].dynamic.is_none());
        let (s, g) = (Vec3::new(50.0, 100.0, 0.0), Vec3::new(550.0, 100.0, 0.0));
        // Connected: straight across the dynamic pylon.
        let p = w.find_path(s, g, &PathOpts::default()).unwrap();
        assert!(max_y(&p) < 300.0, "{p:?}");
        // Disconnect: its cross-pylon edges cost 10,000,000, so the detour wins.
        assert!(w.set_pylon_connected(b, false));
        assert_eq!(w.pylons[b].dynamic.as_ref().unwrap().state.aborts, 1);
        let p = w.find_path(s, g, &PathOpts::default()).unwrap();
        assert!(max_y(&p) > 1900.0, "{p:?}");
        // A second Disconnect does not abort again; Connect restores the short route.
        w.set_pylon_connected(b, false);
        assert_eq!(w.pylons[b].dynamic.as_ref().unwrap().state.aborts, 1);
        w.set_pylon_connected(b, true);
        assert!(max_y(&w.find_path(s, g, &PathOpts::default()).unwrap()) < 300.0);
        // The pylon's own polys stay walkable while it is disconnected.
        w.set_pylon_connected(b, false);
        assert!(w.find_path(Vec3::new(250.0, 50.0, 0.0), Vec3::new(350.0, 150.0, 0.0), &PathOpts::default()).is_some());
    }

    #[test]
    fn disconnected_edge_is_a_cost_not_a_wall() {
        // Without the detour the disconnected crossing is still the only route (10,000,000).
        let mut data = bridge();
        data.pylons[0].mesh.edges.remove(0);
        let mut w = NavWorld::from_data(&data);
        let b = w.pylon_index("B", None).unwrap();
        w.set_pylon_connected(b, false);
        let start = w.locate(Vec3::new(50.0, 100.0, 0.0), 60.0, 260.0).unwrap();
        let goal = w.locate(Vec3::new(550.0, 100.0, 0.0), 60.0, 260.0).unwrap();
        assert!(w.corridor(start, goal, Vec3::new(550.0, 100.0, 0.0), Vec3::new(50.0, 100.0, 0.0), &PathOpts::default()).is_some());
        let l = w.links[start as usize].iter().find(|l| l.kind == LinkKind::CrossPylon).unwrap();
        assert_eq!(w.pylon_cost(start, l), Some(DISCONNECTED_EDGE_COST));
    }

    #[test]
    fn moving_dynamic_pylon_disconnects_then_moves_its_mesh_in_50_uu_steps() {
        let mut w = NavWorld::from_data(&bridge());
        let b = w.pylon_index("B", None).unwrap();
        let placed = w.pylons[b].placed;
        let v0 = w.pylons[b].verts.0 as usize;
        let z = |w: &NavWorld| w.verts[v0].z;
        // Under 50 uu on every axis: disconnected (pawns aborted), mesh unchanged.
        assert!(!w.move_dynamic_pylon(b, placed + Vec3::new(30.0, -49.0, 49.9)));
        let d = w.pylons[b].dynamic.clone().unwrap().state;
        assert!(d.disconnected && d.aborts == 1 && d.previous == placed);
        assert_eq!(z(&w), 0.0);
        // 50 uu on one axis from where the mesh was: the whole offset is applied.
        let to = placed + Vec3::new(0.0, 0.0, 60.0);
        assert!(w.move_dynamic_pylon(b, to));
        let d = w.pylons[b].dynamic.clone().unwrap().state;
        assert_eq!((d.previous, d.aborts), (to, 1));
        assert_eq!(z(&w), 60.0);
        let pb = w.pylons[b].polys.0 as usize;
        assert_eq!((w.polys[pb].center.z, w.polys[pb].min.z, w.polys[pb].max.z), (60.0, 60.0, 60.0));
        // Edges held by B moved with it; the ones A holds keep A's vertices.
        assert!(w.links.iter().flatten().filter(|l| l.owner as usize == b).all(|l| l.a.z == 60.0 && l.b.z == 60.0));
        assert!(w.links.iter().flatten().filter(|l| l.owner as usize != b).all(|l| l.a.z == 0.0));
        // The spatial index follows: B is found at its new height only.
        assert_eq!(w.locate(Vec3::new(300.0, 100.0, 60.0), 1.0, 1.0), Some(pb as u32));
        assert_eq!(w.locate(Vec3::new(300.0, 100.0, 0.0), 1.0, 1.0), None);
        // Still disconnected until the Kismet connects it.
        assert!(w.pylons[b].dynamic.as_ref().unwrap().state.disconnected);
    }

    #[test]
    fn follower_repaths_when_a_crossed_pylon_aborts() {
        let mut w = NavWorld::from_data(&bridge());
        let b = w.pylon_index("B", None).unwrap();
        let (s, g) = (Vec3::new(50.0, 100.0, 0.0), Vec3::new(550.0, 100.0, 0.0));
        let mut f = Follower::default();
        f.steer(Some(&w), s, g);
        assert_eq!(f.pylons, vec![(b as u16, 0)]);
        assert!(max_y(&f.path) < 300.0);
        w.set_pylon_connected(b, false);
        // The next tick replans at once (not after the 60-tick period).
        f.steer(Some(&w), s, g);
        assert!(max_y(&f.path) > 1900.0, "{:?}", f.path);
        assert!(f.pylons.is_empty());
    }
}
