//! Waypoint navigation for bots over the map's original AI points (nav/target/watch
//! points plus player starts). Edges are straight walkable segments verified against the
//! collision world; paths are A* over that graph. Presentation of intent only: bots still
//! act purely through `PlayerInput`.

use dishonored_world::{WorldQuery, channel};
use glam::Vec3;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Max edge length (uu).
const MAX_EDGE: f32 = 1400.0;
/// Max height change along an edge relative to its length (stairs/ramps).
const MAX_SLOPE: f32 = 0.75;

#[derive(Debug, Clone, Default)]
pub struct NavGraph {
    /// Node positions on the floor.
    pub nodes: Vec<Vec3>,
    pub edges: Vec<Vec<(u32, f32)>>,
}

/// Floor directly below `p` (within `depth`).
pub fn floor_below(world: &dyn WorldQuery, p: Vec3, depth: f32) -> Option<Vec3> {
    world.raycast(p + Vec3::Z * 40.0, -Vec3::Z, depth + 40.0, channel::PAWN).filter(|h| h.normal.z > 0.6).map(|h| h.point)
}

/// Whether a pawn can walk the straight segment between two floor points.
pub fn walkable(world: &dyn WorldQuery, a: Vec3, b: Vec3) -> bool {
    let d = b - a;
    let len = d.truncate().length();
    if len < 1.0 {
        return true;
    }
    if d.z.abs() > 40.0 + len * MAX_SLOPE {
        return false;
    }
    // Body clearance at knee and chest height.
    for h in [50.0, 130.0] {
        let o = a + Vec3::Z * h;
        let t = b + Vec3::Z * h - o;
        if world.raycast(o, t, t.length(), channel::PAWN).is_some() {
            return false;
        }
    }
    // Ground under the path (no gaps or drops).
    let steps = (len / 100.0).ceil() as usize;
    let mut prev_z = a.z;
    for i in 1..steps {
        let p = a + d * (i as f32 / steps as f32);
        match floor_below(world, Vec3::new(p.x, p.y, p.z.max(prev_z) + 60.0), 200.0) {
            Some(f) if (f.z - prev_z).abs() < 70.0 => prev_z = f.z,
            _ => return false,
        }
    }
    true
}

impl NavGraph {
    pub fn build(world: &dyn WorldQuery, points: impl IntoIterator<Item = Vec3>) -> Self {
        let mut nodes: Vec<Vec3> = Vec::new();
        for p in points {
            let Some(f) = floor_below(world, p + Vec3::Z * 60.0, 400.0) else { continue };
            if nodes.iter().all(|n| n.distance(f) > 60.0) {
                nodes.push(f);
            }
        }
        let mut edges = vec![Vec::new(); nodes.len()];
        for i in 0..nodes.len() {
            for j in i + 1..nodes.len() {
                let d = nodes[i].distance(nodes[j]);
                if d <= MAX_EDGE && walkable(world, nodes[i], nodes[j]) {
                    edges[i].push((j as u32, d));
                    edges[j].push((i as u32, d));
                }
            }
        }
        NavGraph { nodes, edges }
    }

    pub fn edge_count(&self) -> usize {
        self.edges.iter().map(|e| e.len()).sum::<usize>() / 2
    }

    /// Nearest nodes reachable in a straight walk from `p` (closest first, at most `k`).
    pub fn visible_nodes(&self, world: &dyn WorldQuery, p: Vec3, k: usize) -> Vec<u32> {
        let Some(f) = floor_below(world, p, 300.0) else { return Vec::new() };
        let mut idx: Vec<u32> = (0..self.nodes.len() as u32).filter(|&i| self.nodes[i as usize].distance(f) <= MAX_EDGE * 1.5).collect();
        idx.sort_by(|a, b| self.nodes[*a as usize].distance(f).total_cmp(&self.nodes[*b as usize].distance(f)));
        idx.into_iter().filter(|&i| walkable(world, f, self.nodes[i as usize])).take(k).collect()
    }

    /// A* from any of `starts` to any of `goals`; returns node indices.
    pub fn path(&self, starts: &[u32], goals: &[u32], goal_pos: Vec3) -> Option<Vec<u32>> {
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
        let n = self.nodes.len();
        let mut g = vec![f32::INFINITY; n];
        let mut came = vec![u32::MAX; n];
        let mut open = BinaryHeap::new();
        for &s in starts {
            g[s as usize] = 0.0;
            open.push(E(self.nodes[s as usize].distance(goal_pos), s));
        }
        while let Some(E(_, u)) = open.pop() {
            if goals.contains(&u) {
                let mut out = vec![u];
                let mut c = u;
                while came[c as usize] != u32::MAX {
                    c = came[c as usize];
                    out.push(c);
                }
                out.reverse();
                return Some(out);
            }
            for &(v, w) in &self.edges[u as usize] {
                let ng = g[u as usize] + w;
                if ng < g[v as usize] {
                    g[v as usize] = ng;
                    came[v as usize] = u;
                    open.push(E(ng + self.nodes[v as usize].distance(goal_pos), v));
                }
            }
        }
        None
    }
}

/// Per-bot path-following state.
#[derive(Debug, Clone, Default)]
pub struct Follower {
    path: Vec<Vec3>,
    replan_in: u32,
    goal: Vec3,
}

impl Follower {
    /// Point to steer toward this tick to reach `goal` (direct when walkable).
    pub fn steer(&mut self, nav: &NavGraph, world: &dyn WorldQuery, me: Vec3, goal: Vec3) -> Vec3 {
        let me_f = floor_below(world, me, 300.0).unwrap_or(me);
        let goal_f = floor_below(world, goal, 300.0).unwrap_or(goal);
        if self.replan_in > 0 {
            self.replan_in -= 1;
        }
        if self.replan_in == 0 || self.goal.distance(goal) > 400.0 {
            self.replan_in = 60;
            self.goal = goal;
            self.path.clear();
            if !walkable(world, me_f, goal_f) {
                let s = nav.visible_nodes(world, me, 4);
                let g = nav.visible_nodes(world, goal, 4);
                if let Some(p) = nav.path(&s, &g, goal_f) {
                    self.path = p.into_iter().map(|i| nav.nodes[i as usize]).collect();
                }
            }
        }
        // Drop reached waypoints; skip ahead when the next one is directly walkable.
        while let Some(&w) = self.path.first() {
            let reached = (w - me_f).truncate().length() < 70.0;
            let skip = self.path.get(1).is_some_and(|&n| (n - me_f).truncate().length() < 600.0 && walkable(world, me_f, n));
            if reached || skip {
                self.path.remove(0);
            } else {
                break;
            }
        }
        self.path.first().copied().unwrap_or(goal)
    }

    pub fn force_replan(&mut self) {
        self.replan_in = 0;
    }
}
