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

/// The map's navigation, ready for queries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NavWorld {
    pub verts: Vec<Vec3>,
    pub polys: Vec<NavPolyRt>,
    pub links: Vec<Vec<NavLink>>,
    pub path_objects: Vec<PathObjectState>,
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

impl NavWorld {
    pub fn from_data(data: &NavData) -> NavWorld {
        let mut w = NavWorld::default();
        for po in &data.path_objects {
            w.path_objects.push(PathObjectState { guid: po.guid, name: po.name.clone(), class: po.class.clone(), blocking: po.blocking });
        }
        // Global poly index of (pylon, local poly).
        let mut poly_base = Vec::with_capacity(data.pylons.len());
        for p in &data.pylons {
            let vb = w.verts.len() as u32;
            poly_base.push(w.polys.len() as u32);
            w.verts.extend(p.mesh.verts.iter().copied());
            for poly in &p.mesh.polys {
                w.polys.push(NavPolyRt { verts: poly.verts.iter().map(|&v| vb + v as u32).collect(), center: poly.center, normal: poly.normal, min: poly.min, max: poly.max, height: poly.height });
            }
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
                            w.links[(base + p0) as usize].push(NavLink { to: base + p1, a, b, kind: LinkKind::Walk, extra: 0.0 });
                            w.links[(base + p1) as usize].push(NavLink { to: base + p0, a, b, kind: LinkKind::Walk, extra: 0.0 });
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
                        w.links[(base + from) as usize].push(NavLink { to: base + to, a, b, kind: LinkKind::OneWay, extra: 0.0 });
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
                        w.links[from as usize].push(NavLink { to, a, b, kind, extra: (e.effective_length - e.length).max(0.0) });
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
                let ng = g[ui] + pos[ui].distance(mid) + l.extra;
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

    /// Waypoints (floor points) from `start` to `goal`, excluding the start. `None` when either
    /// end is off the mesh or no route exists.
    pub fn find_path(&self, start: Vec3, goal: Vec3, opts: &PathOpts) -> Option<Vec<Vec3>> {
        let (sp, s) = self.closest_point(start, 200.0)?;
        let (gp, g) = self.closest_point(goal, 300.0)?;
        if sp == gp {
            return Some(vec![g]);
        }
        let corr = self.corridor(sp, gp, g, s, opts)?;
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
        Some(string_pull(&portals))
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
        if self.replan_in == 0 || moved {
            self.replan_in = 60;
            self.goal = Some(goal);
            match nav.find_path(me, goal, &PathOpts::default()) {
                Some(p) => {
                    self.path = p;
                    self.failed = false;
                }
                None => {
                    self.path.clear();
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

    #[test]
    fn funnel_straight_line_when_clear() {
        let w = NavWorld::from_data(&mesh());
        let p = w.find_path(Vec3::new(20.0, 100.0, 0.0), Vec3::new(380.0, 100.0, 0.0), &PathOpts::default()).unwrap();
        assert_eq!(p.len(), 1, "{p:?}");
    }
}
