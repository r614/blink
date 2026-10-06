//! Exact geometric primitives used by collision queries.

use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };
    pub fn grow(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    pub fn union(&self, o: &Aabb) -> Aabb {
        Aabb { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    pub fn overlaps(&self, o: &Aabb) -> bool {
        self.min.cmple(o.max).all() && self.max.cmpge(o.min).all()
    }
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn expand(&self, r: f32) -> Aabb {
        Aabb { min: self.min - Vec3::splat(r), max: self.max + Vec3::splat(r) }
    }
    /// Slab test; returns entry distance if the ray hits within `max_t`.
    pub fn ray(&self, o: Vec3, inv_d: Vec3, max_t: f32) -> Option<f32> {
        let t1 = (self.min - o) * inv_d;
        let t2 = (self.max - o) * inv_d;
        let tmin = t1.min(t2).max_element().max(0.0);
        let tmax = t1.max(t2).min_element().min(max_t);
        (tmin <= tmax).then_some(tmin)
    }
}

/// Möller–Trumbore, two-sided. Returns distance along `d` (unnormalized units of `d`).
pub fn ray_triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t)
}

/// Closest point on triangle `abc` to `p` (Ericson, Real-Time Collision Detection 5.1.5).
pub fn closest_point_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    a + ab * v + ac * w
}

/// Closest points between segments p1q1 and p2q2 (Ericson 5.1.9).
pub fn closest_segment_segment(p1: Vec3, q1: Vec3, p2: Vec3, q2: Vec3) -> (Vec3, Vec3) {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(d1);
    let e = d2.dot(d2);
    let f = d2.dot(r);
    let (s, t);
    if a <= 1e-12 && e <= 1e-12 {
        return (p1, p2);
    }
    if a <= 1e-12 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e <= 1e-12 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s0 = if denom != 0.0 { ((b * f - c * e) / denom).clamp(0.0, 1.0) } else { 0.0 };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    (p1 + d1 * s, p2 + d2 * t)
}

/// Closest points between segment `p0p1` and triangle `abc`: (on segment, on triangle).
pub fn closest_segment_triangle(p0: Vec3, p1: Vec3, a: Vec3, b: Vec3, c: Vec3) -> (Vec3, Vec3) {
    let d = p1 - p0;
    if let Some(t) = ray_triangle(p0, d, a, b, c) {
        if t <= 1.0 {
            let p = p0 + d * t;
            return (p, p);
        }
    }
    let mut best = (p0, closest_point_triangle(p0, a, b, c));
    let mut best_d = best.0.distance_squared(best.1);
    let mut consider = |s: Vec3, q: Vec3| {
        let dd = s.distance_squared(q);
        if dd < best_d {
            best_d = dd;
            best = (s, q);
        }
    };
    let q1 = closest_point_triangle(p1, a, b, c);
    consider(p1, q1);
    for (e0, e1) in [(a, b), (b, c), (c, a)] {
        let (s, q) = closest_segment_segment(p0, p1, e0, e1);
        consider(s, q);
    }
    best
}

/// Overlap of an axis-aligned box (`center`, half extents `half`) with triangle `abc` by the
/// separating axis test (box axes, triangle normal, the nine edge cross products). Returns the
/// minimal push-out (direction away from the triangle, depth) when they overlap by more than
/// `-slack`. Face axes win ties against edge axes so a box resting on a floor reports the floor
/// normal.
pub fn box_triangle(center: Vec3, half: Vec3, a: Vec3, b: Vec3, c: Vec3, slack: f32) -> Option<(Vec3, f32)> {
    let v = [a - center, b - center, c - center];
    let e = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    let n = e[0].cross(e[1]);
    let mut best: Option<(Vec3, f32)> = None;
    let mut test = |axis: Vec3, bias: f32| -> bool {
        let len = axis.length();
        if len < 1e-6 {
            return true;
        }
        let ax = axis / len;
        let p = [v[0].dot(ax), v[1].dot(ax), v[2].dot(ax)];
        let pmin = p[0].min(p[1]).min(p[2]);
        let pmax = p[0].max(p[1]).max(p[2]);
        let r = half.x * ax.x.abs() + half.y * ax.y.abs() + half.z * ax.z.abs();
        if pmin > r + slack || pmax < -r - slack {
            return false;
        }
        // Moving the box by `up` along +ax clears the triangle's far side; by `down` along -ax
        // its near side.
        let up = pmax + r;
        let down = r - pmin;
        let (push, depth) = if up <= down { (ax, up) } else { (-ax, down) };
        if best.is_none_or(|(_, d)| depth + bias < d) {
            best = Some((push, depth));
        }
        true
    };
    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        if !test(axis, 0.0) {
            return None;
        }
    }
    if !test(n, 0.0) {
        return None;
    }
    for ei in [Vec3::X, Vec3::Y, Vec3::Z] {
        for ej in e {
            if !test(ei.cross(ej), 1e-3) {
                return None;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn box_triangle_floor_and_miss() {
        let (a, b, c) = (Vec3::new(-100., -100., 0.), Vec3::new(100., -100., 0.), Vec3::new(0., 100., 0.));
        // Box sunk 2 uu into the floor: push straight up by 2.
        let (n, d) = box_triangle(Vec3::new(0., 0., 85.5), Vec3::new(35., 35., 87.5), a, b, c, 0.0).unwrap();
        assert!((n - Vec3::Z).length() < 1e-5, "{n}");
        assert!((d - 2.0).abs() < 1e-4, "{d}");
        assert!(box_triangle(Vec3::new(0., 0., 90.0), Vec3::new(35., 35., 87.5), a, b, c, 0.0).is_none());
        // Beside the triangle (outside its edge by more than the half width): no overlap.
        assert!(box_triangle(Vec3::new(0., 200., 50.0), Vec3::new(35., 35., 87.5), a, b, c, 0.0).is_none());
    }

    #[test]
    fn segment_triangle_cases() {
        let (a, b, c) = (Vec3::new(-10., -10., 0.), Vec3::new(10., -10., 0.), Vec3::new(0., 10., 0.));
        // Vertical segment above the face.
        let (s, t) = closest_segment_triangle(Vec3::new(0., 0., 5.), Vec3::new(0., 0., 9.), a, b, c);
        assert!((s.z - 5.0).abs() < 1e-5 && t.z.abs() < 1e-5);
        // Piercing segment.
        let (s, t) = closest_segment_triangle(Vec3::new(0., 0., -5.), Vec3::new(0., 0., 5.), a, b, c);
        assert!(s.distance(t) < 1e-5);
        // Beside an edge.
        let (s, t) = closest_segment_triangle(Vec3::new(0., -20., -5.), Vec3::new(0., -20., 5.), a, b, c);
        assert!((s.distance(t) - 10.0).abs() < 1e-4);
    }
    #[test]
    fn ray_hits() {
        let t = ray_triangle(Vec3::new(0., 0., 10.), Vec3::new(0., 0., -1.), Vec3::new(-1., -1., 0.), Vec3::new(1., -1., 0.), Vec3::new(0., 1., 0.));
        assert_eq!(t, Some(10.0));
    }
}
