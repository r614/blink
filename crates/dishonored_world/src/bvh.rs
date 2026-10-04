//! Static triangle BVH (binned SAH-lite: median split on the longest axis).

use crate::geom::Aabb;
use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub bounds: Aabb,
    /// Leaf: first index into `order`; inner: index of left child (right = left + 1).
    pub first: u32,
    /// Leaf triangle count; 0 for inner nodes.
    pub count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Bvh {
    pub nodes: Vec<Node>,
    pub order: Vec<u32>,
}

const LEAF_SIZE: usize = 4;

impl Bvh {
    pub fn build(bounds: &[Aabb]) -> Bvh {
        let mut order: Vec<u32> = (0..bounds.len() as u32).collect();
        let centers: Vec<Vec3> = bounds.iter().map(|b| b.center()).collect();
        let mut nodes = Vec::with_capacity(bounds.len() * 2 / LEAF_SIZE + 1);
        nodes.push(Node { bounds: Aabb::EMPTY, first: 0, count: 0 });
        if bounds.is_empty() {
            return Bvh { nodes, order };
        }
        // Iterative build: (node index, start, end)
        let mut stack = vec![(0usize, 0usize, order.len())];
        while let Some((ni, start, end)) = stack.pop() {
            let mut bb = Aabb::EMPTY;
            let mut cb = Aabb::EMPTY;
            for &i in &order[start..end] {
                bb = bb.union(&bounds[i as usize]);
                cb.grow(centers[i as usize]);
            }
            nodes[ni].bounds = bb;
            let n = end - start;
            let ext = cb.max - cb.min;
            if n <= LEAF_SIZE || ext.max_element() < 1e-4 {
                nodes[ni].first = start as u32;
                nodes[ni].count = n as u32;
                continue;
            }
            let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
            let mid = start + n / 2;
            order[start..end].select_nth_unstable_by(n / 2, |a, b| {
                centers[*a as usize][axis].total_cmp(&centers[*b as usize][axis])
            });
            let left = nodes.len();
            nodes.push(Node { bounds: Aabb::EMPTY, first: 0, count: 0 });
            nodes.push(Node { bounds: Aabb::EMPTY, first: 0, count: 0 });
            nodes[ni].first = left as u32;
            nodes[ni].count = 0;
            stack.push((left, start, mid));
            stack.push((left + 1, mid, end));
        }
        Bvh { nodes, order }
    }

    /// Visit every primitive whose bounds overlap `query`.
    pub fn query_aabb(&self, query: &Aabb, mut f: impl FnMut(u32)) {
        if self.order.is_empty() {
            return;
        }
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let n = &self.nodes[stack[sp] as usize];
            if !n.bounds.overlaps(query) {
                continue;
            }
            if n.count > 0 {
                for &i in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    f(i);
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = n.first;
                stack[sp + 1] = n.first + 1;
                sp += 2;
            }
        }
    }

    /// Visit primitives along a ray; `f` returns the new max distance (for early-out).
    pub fn query_ray(&self, o: Vec3, d: Vec3, mut max_t: f32, mut f: impl FnMut(u32, f32) -> f32) {
        if self.order.is_empty() {
            return;
        }
        let inv = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let n = &self.nodes[stack[sp] as usize];
            if n.bounds.ray(o, inv, max_t).is_none() {
                continue;
            }
            if n.count > 0 {
                for &i in &self.order[n.first as usize..(n.first + n.count) as usize] {
                    max_t = f(i, max_t);
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = n.first;
                stack[sp + 1] = n.first + 1;
                sp += 2;
            }
        }
    }
}
