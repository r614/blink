//! AI hearing through the audio cells (`DishonoredAudioVolume` / `DishonoredAudioPortal` /
//! `DisAudioPropagationInfo`), decoded from 0x00bb50d0, 0x00ba3700, 0x00b92320, 0x00b9d5e0.
//! See `research/dishonored/AI.md` ("Hearing through audio cells").
//!
//! - A noise is heard only strictly inside its radius.
//! - Listener or noise outside every cell: a blocking trace decides (0x00bb50d0).
//! - Both in the same cell: heard.
//! - Different cells: for every pair (portal of the listener's cell, portal of the noise's cell)
//!   the precomputed path between them is looked up (0x00b92320; key from the cells' and
//!   portals' propagation indices, path length `u16 * m_fDistCoeff`, intermediate portals).
//!   A quick estimate (path length plus the distances from each end to its nearest portal point)
//!   must be under the radius; then the path is string-pulled through the portal rectangles (up
//!   to 10 passes, 0x00b9d5e0) and its length divided by the product of `(1 -
//!   m_fOcclusion_HeardByAI)` over the portals (at least the straight distance) must be under the
//!   radius. The first pair that passes makes the noise heard.

use dishonored_ir::ai::{AudioCells, AudioPortal};
use glam::Vec3;

/// The cell containing `p` (the first whose hulls contain it).
pub fn cell_of(audio: &AudioCells, p: Vec3) -> Option<usize> {
    audio.cells.iter().position(|c| c.contains(p))
}

/// Closest point of the portal rectangle to a point.
fn closest_point(p: &AudioPortal, q: Vec3) -> Vec3 {
    let d = q - p.origin;
    let x = d.dot(p.x_axis).clamp(-p.half[0], p.half[0]);
    let y = d.dot(p.y_axis).clamp(-p.half[1], p.half[1]);
    p.origin + p.x_axis * x + p.y_axis * y
}

/// Point of the portal rectangle the segment `a`-`b` should pass through (0x00b92670): where
/// it crosses the portal plane, clamped onto the rectangle; for a segment that does not cross
/// it, the clamped point of either end that makes the shorter path.
fn through_portal(p: &AudioPortal, a: Vec3, b: Vec3) -> Vec3 {
    let ab = b - a;
    let den = p.normal.dot(ab);
    if den.abs() > 1e-8 {
        let t = p.normal.dot(p.origin - a) / den;
        if (0.0..=1.0).contains(&t) {
            return closest_point(p, a + ab * t);
        }
    }
    let qa = closest_point(p, a);
    let qb = closest_point(p, b);
    if a.distance(qa) + qa.distance(b) <= a.distance(qb) + qb.distance(b) { qa } else { qb }
}

/// Is `q` past the portal while in `cell` (0x00b86e20): on the portal's far side for that cell
/// and within its rectangle. Such an end skips the portal.
fn past_portal(p: &AudioPortal, cell: usize, q: Vec3) -> bool {
    let d = q - p.origin;
    let side = p.normal.dot(d);
    let facing = (p.cell_a == Some(cell) && side > 0.0) || (p.cell_b == Some(cell) && side < 0.0);
    facing && d.dot(p.x_axis).abs() <= p.half[0] && d.dot(p.y_axis).abs() <= p.half[1]
}

/// The precomputed path from `pa` (a portal of cell `ca`) to `pb` (of cell `cb`), as
/// (length, portals from `pa` to `pb`), if shorter than `limit` (0x00b92320).
fn path(audio: &AudioCells, ca: usize, pa: usize, cb: usize, pb: usize, limit: f32) -> Option<(f32, Vec<usize>)> {
    if pa == pb {
        return Some((0.0, vec![pa]));
    }
    let prop = audio.propagation.as_ref()?;
    let cells = &audio.cells;
    let ports = &audio.portals;
    let key = ports[pb].index as u32 | (cells[cb].index as u32) << 8 | (ports[pa].index as u32) << 16 | (cells[ca].index as u32) << 24;
    let i = prop.keys.binary_search(&key).ok()?;
    let v = *prop.values.get(i)?;
    let (off, n) = ((v >> 8) as usize, (v & 0xff) as usize);
    let len = u16::from_le_bytes([*prop.data.get(off)?, *prop.data.get(off + 1)?]) as f32 * prop.dist_coeff;
    if len >= limit {
        return None;
    }
    let mut out = vec![pa];
    for k in 0..n {
        let m = *prop.data.get(off + 2 + k)? as usize;
        out.push(*prop.portals.get(m).filter(|p| **p < audio.portals.len())?);
    }
    out.push(pb);
    Some((len, out))
}

/// String-pulled length of the path `from` -> portals -> `to` (0x00b9d5e0), or `None` when it
/// reaches `limit` while being summed.
fn pulled_length(ports: &[AudioPortal], from: Vec3, to: Vec3, path: &[usize], d2: f32, limit: f32) -> Option<f32> {
    let n = path.len();
    let mut pts = Vec::with_capacity(n + 2);
    pts.push(from);
    for &k in path {
        let p = &ports[k];
        pts.push((closest_point(p, from) + closest_point(p, to)) * 0.5);
    }
    pts.push(to);
    let mut dirty = vec![true; n + 2];
    let threshold = (n as f32 * d2 * 0.0016).max(400.0);
    for _ in 0..10 {
        let mut moved = 0.0;
        for j in 0..n {
            if !dirty[j + 1] {
                continue;
            }
            dirty[j + 1] = false;
            let q = through_portal(&ports[path[j]], pts[j], pts[j + 2]);
            if q != pts[j + 1] {
                moved += q.distance_squared(pts[j + 1]);
                pts[j + 1] = q;
                dirty[j] = true;
                dirty[j + 2] = true;
            }
        }
        if moved <= threshold {
            break;
        }
    }
    let mut len = 0.0;
    for w in pts.windows(2) {
        let s = w[0].distance(w[1]);
        if s > 0.0 {
            len += s;
            if len >= limit {
                return None;
            }
        }
    }
    Some(len)
}

/// Does a listener at `listener` (in cell `lc`) hear a noise at `noise` (in cell `nc`) of
/// radius `radius`? `trace_clear` is the blocking trace used when either end is outside the
/// cells.
pub fn heard(audio: &AudioCells, listener: Vec3, lc: Option<usize>, noise: Vec3, nc: Option<usize>, radius: f32, trace_clear: impl FnOnce() -> bool) -> bool {
    let d2 = listener.distance_squared(noise);
    if d2 >= radius * radius {
        return false;
    }
    let (Some(lc), Some(nc)) = (lc, nc) else { return trace_clear() };
    if lc == nc {
        return true;
    }
    let cells = &audio.cells;
    let ports = &audio.portals;
    for &pl in &cells[lc].portals {
        for &pn in &cells[nc].portals {
            // Cell A is the one with the lower propagation index; the path is stored from A.
            let listener_first = cells[lc].index < cells[nc].index;
            let found = if listener_first { path(audio, lc, pl, nc, pn, radius) } else { path(audio, nc, pn, lc, pl, radius).map(|(l, mut p)| (l, { p.reverse(); p })) };
            let Some((len, route)) = found else { continue };
            // Ends already past their portal skip it.
            let skip_first = past_portal(&ports[route[0]], lc, listener) as usize;
            let skip_last = (route.len() > skip_first + 1 && past_portal(&ports[*route.last().unwrap()], nc, noise)) as usize;
            let inner = &route[skip_first..route.len() - skip_last];
            if !inner.is_empty() {
                let est = len + listener.distance(closest_point(&ports[inner[0]], listener)) + noise.distance(closest_point(&ports[*inner.last().unwrap()], noise));
                if est >= radius {
                    continue;
                }
            }
            let Some(pulled) = pulled_length(ports, listener, noise, inner, d2, radius) else { continue };
            let open: f32 = inner.iter().map(|&k| 1.0 - ports[k].occlusion_ai).product();
            let effective = if (open).abs() < 1e-4 { f32::MAX } else { (pulled / open).max(d2.sqrt()) };
            if effective < radius {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use dishonored_ir::ai::{AudioCell, AudioPropagation};

    fn box_hull(min: Vec3, max: Vec3) -> Vec<[f32; 4]> {
        vec![[1.0, 0.0, 0.0, max.x], [-1.0, 0.0, 0.0, -min.x], [0.0, 1.0, 0.0, max.y], [0.0, -1.0, 0.0, -min.y], [0.0, 0.0, 1.0, max.z], [0.0, 0.0, -1.0, -min.z]]
    }

    /// Three rooms in a row along x (0-1000, 1000-2000, 2000-3000), doors at x = 1000 and 2000
    /// (y in -100..100), the second door half occluding.
    fn rooms() -> AudioCells {
        let cell = |i: u8, x0: f32, portals: Vec<usize>| AudioCell { name: format!("c{i}"), index: i, hulls: vec![box_hull(Vec3::new(x0, -500.0, -100.0), Vec3::new(x0 + 1000.0, 500.0, 300.0))], portals };
        let door = |i: u8, x: f32, a: usize, b: usize, occ: f32| AudioPortal { name: format!("p{i}"), index: i, origin: Vec3::new(x, 0.0, 100.0), x_axis: Vec3::Y, y_axis: Vec3::Z, normal: Vec3::X, half: [100.0, 100.0], cell_a: Some(a), cell_b: Some(b), occlusion_ai: occ };
        let key = |pb: u32, cb: u32, pa: u32, ca: u32| pb | cb << 8 | pa << 16 | ca << 24;
        // Path from portal 0 (cell 0) to portal 1 (cell 2): length 1000, no intermediates.
        let prop = AudioPropagation { dist_coeff: 1.0, max_audible_distance: 10000.0, keys: vec![key(1, 2, 0, 0)], values: vec![0], data: 1000u16.to_le_bytes().to_vec(), portals: vec![0, 1] };
        AudioCells { cells: vec![cell(0, 0.0, vec![0]), cell(1, 1000.0, vec![0, 1]), cell(2, 2000.0, vec![1])], portals: vec![door(0, 1000.0, 0, 1, 0.0), door(1, 2000.0, 1, 2, 0.5)], propagation: Some(prop) }
    }

    #[test]
    fn cells_portals_and_occlusion() {
        let a = rooms();
        let l = Vec3::new(500.0, 0.0, 100.0);
        assert_eq!(cell_of(&a, l), Some(0));
        // Same cell: heard without a trace.
        assert!(heard(&a, l, Some(0), Vec3::new(900.0, 300.0, 100.0), Some(0), 600.0, || false));
        // Next cell through one open door: the path length is the straight line (800 uu).
        let n = Vec3::new(1300.0, 0.0, 100.0);
        assert!(heard(&a, l, Some(0), n, Some(1), 900.0, || false));
        assert!(!heard(&a, l, Some(0), n, Some(1), 790.0, || false));
        // Around the corner: the noise is off to the side, the path bends at the door.
        let side = Vec3::new(1100.0, 450.0, 100.0);
        let bent = l.distance(Vec3::new(1000.0, 100.0, 100.0)) + Vec3::new(1000.0, 100.0, 100.0).distance(side);
        assert!(heard(&a, l, Some(0), side, Some(1), bent + 5.0, || false));
        assert!(!heard(&a, l, Some(0), side, Some(1), bent - 5.0, || false));
        // Two cells away through the half-occluding door: length 2000 / 0.5 = 4000.
        let far = Vec3::new(2500.0, 0.0, 100.0);
        assert!(heard(&a, l, Some(0), far, Some(2), 4100.0, || false));
        assert!(!heard(&a, l, Some(0), far, Some(2), 3900.0, || false));
        // Outside the cells: the trace decides.
        assert!(heard(&a, l, None, n, Some(1), 900.0, || true));
        assert!(!heard(&a, l, None, n, Some(1), 900.0, || false));
        // Never beyond the radius.
        assert!(!heard(&a, l, Some(0), n, Some(0), 700.0, || true));
    }
}
