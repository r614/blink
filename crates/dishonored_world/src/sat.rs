//! The original's extent checks: an axis-aligned box swept against a triangle (the kDOP
//! per-polygon collision, 0x005DEB70) or a convex element (simplified collision, brush hulls and
//! physics bodies, 0x007AF030), by separating axes (0x005DEA10 / 0x005DE780; research/dishonored/
//! COLLISION.md "Extent checks").
//!
//! Each axis gives the times the box enters and leaves the shape's slab (the shape's projection
//! widened by the box's); the latest entry over all axes is the hit, unless an axis separates the
//! whole move. A box that starts overlapping the shape (latest entry before the start) is stopped
//! where it starts when it is no deeper than it has left to travel before leaving (`-entry <=
//! exit`) or when it does not move out through the face it leaves by; else the shape is ignored.
//!
//! Touching (0x005DE780 for kDOP triangles, 0x007A8C80 for convex elements): an axis along which the
//! box moves less than [`KDOP_TOUCH`] / [`CONVEX_TOUCH`] (uu, along the axis as given) and which it
//! starts outside of by less than that much records the first such axis's normal; a box that
//! touches is then blocked where it starts, its normal the touch normal less 0.05 of the move
//! direction, unless its own entry comes after the check's best hit so far through another axis.

use glam::Vec3;

/// The slab state over the axes tested so far.
#[derive(Debug, Clone, Copy)]
pub struct Sat {
    /// The touch band (`KDOP_TOUCH` or `CONVEX_TOUCH`).
    pub eps: f32,
    /// The first touching axis's outward normal.
    pub touch: Option<Vec3>,
    pub entry: f32,
    pub exit: f32,
    pub entry_normal: Vec3,
    pub exit_normal: Vec3,
}

/// The touch band of the kDOP check (0x005DE780).
pub const KDOP_TOUCH: f32 = 1e-4;
/// The touch band of the convex element check (0x007A8C80).
pub const CONVEX_TOUCH: f32 = 0.01;

impl Default for Sat {
    fn default() -> Self {
        Sat { eps: KDOP_TOUCH, touch: None, entry: -3.4e38, exit: 3.4e38, entry_normal: Vec3::ZERO, exit_normal: Vec3::ZERO }
    }
}

/// The result of one sweep against one shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SatHit {
    /// Hit at this fraction of the move with this (unnormalized) normal.
    At(f32, Vec3),
    /// Blocked where it starts (start penetrating).
    Start,
    /// Touching a face it moves almost along: blocked where it starts, with this normal.
    Touch(Vec3),
}

impl Sat {
    /// One axis (any length): the shape projects to `[lo, hi]` on it, the box to `p +- r` at the
    /// start, and moves by `dp` along it. False when the axis separates the whole move.
    pub fn axis(&mut self, axis: Vec3, lo: f32, hi: f32, r: f32, p: f32, dp: f32) -> bool {
        let (lo, hi) = (lo - r, hi + r);
        if dp.abs() < self.eps {
            if self.touch.is_none() {
                if p < lo && lo - self.eps < p {
                    self.touch = Some(-axis);
                }
                if hi < p && p < hi + self.eps {
                    self.touch = Some(axis);
                }
            }
            if dp.abs() < 1e-8 {
                return p >= lo && p <= hi;
            }
        }
        let inv = 1.0 / dp;
        let (t_lo, t_hi) = ((lo - p) * inv, (hi - p) * inv);
        let (entry, exit, n) = if dp <= 0.0 { (t_hi, t_lo, axis) } else { (t_lo, t_hi, -axis) };
        if entry > self.entry {
            self.entry = entry;
            self.entry_normal = n;
        }
        if exit < self.exit {
            self.exit = exit;
            self.exit_normal = -n;
        }
        self.exit >= self.entry && self.exit >= 0.0
    }

    /// Test `axis` for a shape given by its vertices and a box of half size `ext` (world axes).
    pub fn shape_axis(&mut self, axis: Vec3, verts: &[Vec3], ext: Vec3, start: Vec3, delta: Vec3) -> bool {
        if axis.length_squared() < 1e-8 {
            return true;
        }
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for v in verts {
            let d = axis.dot(*v);
            lo = lo.min(d);
            hi = hi.max(d);
        }
        let r = axis.x.abs() * ext.x + axis.y.abs() * ext.y + axis.z.abs() * ext.z;
        self.axis(axis, lo, hi, r, axis.dot(start), axis.dot(delta))
    }

    /// The outcome once every axis overlapped, against the check's best hit time so far: a touch
    /// blocks at the start (unless the entry is later than `best` through another axis); a hit at
    /// the entry when it is within `best`; the start-penetration rule otherwise.
    pub fn finish(&self, delta: Vec3, best: f32) -> Option<SatHit> {
        let dir = delta.normalize_or_zero();
        if let Some(tn) = self.touch {
            let later_elsewhere = best < self.entry && tn != self.entry_normal && tn != -self.entry_normal;
            if !later_elsewhere {
                let tn = if self.eps >= CONVEX_TOUCH { tn.normalize_or_zero() } else { tn };
                return Some(SatHit::Touch((tn - dir * 0.05).normalize_or_zero()));
            }
        }
        if self.entry > best {
            return None;
        }
        if self.entry >= 0.0 {
            return Some(SatHit::At(self.entry, self.entry_normal));
        }
        if -self.entry <= self.exit || self.exit_normal.normalize_or_zero().dot(dir) <= 0.0 {
            Some(SatHit::Start)
        } else {
            None
        }
    }
}

const BOX_AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// A box of half size `ext` at `start` moved by `delta` against triangle `(a, b, c)`: the
/// original's 13 axes (the box's faces, the triangle's normal, box axes x triangle edges).
pub fn box_triangle(start: Vec3, ext: Vec3, delta: Vec3, a: Vec3, b: Vec3, c: Vec3, best: f32) -> Option<SatHit> {
    let verts = [a, b, c];
    let e = [(b - a).normalize_or_zero(), (c - b).normalize_or_zero(), (a - c).normalize_or_zero()];
    let mut s = Sat::default();
    let mut axes: Vec<Vec3> = vec![-Vec3::Z, -Vec3::X, -Vec3::Y, e[0].cross(e[1])];
    for x in BOX_AXES {
        for ei in e {
            axes.push(x.cross(ei));
        }
    }
    for ax in axes {
        if !s.shape_axis(ax, &verts, ext, start, delta) {
            return None;
        }
    }
    s.finish(delta, best)
}

/// A box swept against a convex element given by its triangles: the box's faces, the element's
/// face normals and box axes x the element's edge directions.
pub fn box_convex(start: Vec3, ext: Vec3, delta: Vec3, tris: &[[Vec3; 3]], best: f32) -> Option<SatHit> {
    let mut verts: Vec<Vec3> = Vec::with_capacity(tris.len() * 3);
    let mut normals: Vec<Vec3> = Vec::new();
    let mut edges: Vec<Vec3> = Vec::new();
    let push_unique = |list: &mut Vec<Vec3>, v: Vec3| {
        if v.length_squared() < 1e-12 {
            return;
        }
        let v = v.normalize();
        if !list.iter().any(|u| u.dot(v).abs() > 0.99999) {
            list.push(v);
        }
    };
    for t in tris {
        verts.extend_from_slice(t);
        push_unique(&mut normals, (t[1] - t[0]).cross(t[2] - t[0]));
        for (p, q) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            push_unique(&mut edges, q - p);
        }
    }
    let mut s = Sat { eps: CONVEX_TOUCH, ..Sat::default() };
    for ax in [-Vec3::Z, -Vec3::X, -Vec3::Y] {
        if !s.shape_axis(ax, &verts, ext, start, delta) {
            return None;
        }
    }
    for n in &normals {
        if !s.shape_axis(*n, &verts, ext, start, delta) {
            return None;
        }
    }
    for e in &edges {
        for x in BOX_AXES {
            if !s.shape_axis(x.cross(*e), &verts, ext, start, delta) {
                return None;
            }
        }
    }
    s.finish(delta, best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor() -> [Vec3; 3] {
        [Vec3::new(-1000.0, -1000.0, 0.0), Vec3::new(1000.0, -1000.0, 0.0), Vec3::new(0.0, 1000.0, 0.0)]
    }

    #[test]
    fn box_lands_on_a_triangle() {
        let t = floor();
        let ext = Vec3::new(35.0, 35.0, 87.5);
        let h = box_triangle(Vec3::new(0.0, 0.0, 200.0), ext, Vec3::new(0.0, 0.0, -300.0), t[0], t[1], t[2], 1.0);
        match h {
            Some(SatHit::At(f, n)) => {
                assert!((200.0 - 300.0 * f - 87.5).abs() < 1e-3, "{f}");
                assert!(n.normalize().z > 0.999, "{n}");
            }
            other => panic!("{other:?}"),
        }
        // Passing beside it: nothing.
        assert!(box_triangle(Vec3::new(3000.0, 0.0, 200.0), ext, Vec3::new(0.0, 0.0, -300.0), t[0], t[1], t[2], 1.0).is_none());
    }

    #[test]
    fn start_penetration_rule() {
        let t = floor();
        let ext = Vec3::new(35.0, 35.0, 87.5);
        // 2.5 uu into the floor and moving down into it: stopped where it starts.
        let h = box_triangle(Vec3::new(0.0, 0.0, 85.0), ext, Vec3::new(0.0, 0.0, -300.0), t[0], t[1], t[2], 1.0);
        assert_eq!(h, Some(SatHit::Start));
        // 2.5 uu into the floor and moving up out of it: it entered long before (172.5 uu of
        // slab behind it) and leaves through the top within 2.5 uu: the floor is ignored.
        let h = box_triangle(Vec3::new(0.0, 0.0, 85.0), ext, Vec3::new(0.0, 0.0, 300.0), t[0], t[1], t[2], 1.0);
        assert_eq!(h, None);
    }

    #[test]
    fn convex_box_element() {
        // A 100 uu cube as twelve triangles; a box moving into it along x.
        let m = |x: f32, y: f32, z: f32| Vec3::new(x, y, z) * 50.0 + Vec3::new(500.0, 0.0, 0.0);
        let v = [m(-1., -1., -1.), m(1., -1., -1.), m(1., 1., -1.), m(-1., 1., -1.), m(-1., -1., 1.), m(1., -1., 1.), m(1., 1., 1.), m(-1., 1., 1.)];
        let q = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [2, 3, 7, 6], [1, 2, 6, 5], [3, 0, 4, 7]];
        let tris: Vec<[Vec3; 3]> = q.iter().flat_map(|q| [[v[q[0]], v[q[1]], v[q[2]]], [v[q[0]], v[q[2]], v[q[3]]]]).collect();
        let ext = Vec3::new(35.0, 35.0, 87.5);
        match box_convex(Vec3::ZERO, ext, Vec3::new(1000.0, 0.0, 0.0), &tris, 1.0) {
            Some(SatHit::At(f, n)) => {
                assert!((1000.0 * f - (450.0 - 35.0)).abs() < 1e-2, "{f}");
                assert!(n.normalize().x < -0.999);
            }
            other => panic!("{other:?}"),
        }
    }

    /// 0x007A8C80: a box 0.005 uu outside a convex face, moving almost along it (0.001 uu toward
    /// it), touches: blocked where it starts, its normal the face's less 0.05 of the move. The
    /// same move 0.5 uu away passes; a kDOP triangle's band is 1e-4.
    #[test]
    fn touching_a_face_blocks_at_the_start() {
        let m = |x: f32, y: f32, z: f32| Vec3::new(x, y, z) * 50.0 + Vec3::new(500.0, 0.0, 0.0);
        let v = [m(-1., -1., -1.), m(1., -1., -1.), m(1., 1., -1.), m(-1., 1., -1.), m(-1., -1., 1.), m(1., -1., 1.), m(1., 1., 1.), m(-1., 1., 1.)];
        let q = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [2, 3, 7, 6], [1, 2, 6, 5], [3, 0, 4, 7]];
        let tris: Vec<[Vec3; 3]> = q.iter().flat_map(|q| [[v[q[0]], v[q[1]], v[q[2]]], [v[q[0]], v[q[2]], v[q[3]]]]).collect();
        let ext = Vec3::new(35.0, 35.0, 87.5);
        // Face at x = 450: the box's +x face 0.005 short of it, moving up along it.
        let start = Vec3::new(450.0 - 35.0 - 0.005, 0.0, 0.0);
        let d = Vec3::new(0.001, 0.0, 5.0);
        match box_convex(start, ext, d, &tris, 1.0) {
            Some(SatHit::Touch(n)) => assert!(n.x < -0.99, "{n}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(box_convex(start - Vec3::X * 0.5, ext, d, &tris, 1.0), None);
        // The kDOP band is narrower: 0.005 outside is not a touch for a triangle of that face.
        let face = [m(-1., -1., -1.), m(-1., 1., -1.), m(-1., 1., 1.)];
        assert_eq!(box_triangle(start, ext, d, face[0], face[1], face[2], 1.0), None);
    }
}