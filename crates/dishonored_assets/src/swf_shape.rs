//! Vector shapes of GFx movies drawn as the Flash player draws them: the edge records of a
//! `DefineShape` (straight and quadratic edges, flattened) filled per fill style with the
//! even-odd rule (every boundary edge is listed once for each fill style beside it:
//! `FillStyle0` / `FillStyle1`), solid colours and linear / radial / focal gradients evaluated
//! per pixel (SWF 10 spec, "Shapes" and "Gradients"). Bitmap fills keep the movie's textured
//! quads. The result is an RGBA image of the shape's bounds that the stages draw as a quad.

use crate::swf::{Fill, Gradient, Shape};

/// Pixels per twip of a rasterized shape: two pixels per stage pixel (1280 x 720 stage), so a
/// shape stays sharp up to 2560 x 1440 windows.
pub const RASTER_SCALE: f32 = 2.0 / 20.0;
/// Largest side of a raster (pixels); bigger shapes use a lower scale.
pub const RASTER_MAX: f32 = 2048.0;

/// A rasterized shape: straight-alpha sRGB pixels covering `origin` .. `origin + size / scale`
/// (twips).
#[derive(Debug, Clone)]
pub struct ShapeRaster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Twips of the image's top-left corner, and pixels per twip.
    pub origin: (f32, f32),
    pub scale: f32,
}

/// The raster's placement: (width, height, origin twips, pixels per twip).
pub fn raster_dims(shape: &Shape) -> (u32, u32, (f32, f32), f32) {
    let b = shape.bounds;
    let (w_t, h_t) = ((b.xmax - b.xmin).max(1.0), (b.ymax - b.ymin).max(1.0));
    let scale = RASTER_SCALE.min(RASTER_MAX / w_t.max(h_t));
    // One pixel of padding around the bounds (anti-aliased edges).
    let pad = 1.0 / scale;
    let w = ((w_t + 2.0 * pad) * scale).ceil().max(1.0) as u32;
    let h = ((h_t + 2.0 * pad) * scale).ceil().max(1.0) as u32;
    (w, h, (b.xmin - pad, b.ymin - pad), scale)
}

/// Whether the shape has edges under a solid or gradient fill (drawn from a raster).
pub fn has_vector_fill(shape: &Shape) -> bool {
    shape.edges.iter().any(|(f, _)| matches!(shape.fills.get(*f), Some(Fill::Solid(_)) | Some(Fill::Gradient(_))))
}

/// Gradient colour at a shape-space point (twips): the gradient square (-16384..16384 twips)
/// mapped by the fill's matrix; linear along x, radial from the centre, focal from the focal
/// point on the x axis; spread pad / reflect / repeat; stops by ratio (RGB interpolation).
pub fn gradient_color(g: &Gradient, x: f32, y: f32) -> [f32; 4] {
    let (gx, gy) = g.matrix.inverse().apply(x, y);
    let (u, v) = (gx / 16384.0, gy / 16384.0);
    let mut t = if !g.radial {
        (u + 1.0) * 0.5
    } else if g.focal.abs() < 1e-4 {
        (u * u + v * v).sqrt()
    } else {
        // The ray from the focal point F = (f, 0) through P meets the unit circle at F + s (P - F).
        let f = g.focal.clamp(-0.998, 0.998);
        let (dx, dy) = (u - f, v);
        let a = dx * dx + dy * dy;
        if a < 1e-12 {
            0.0
        } else {
            let bq = 2.0 * f * dx;
            let c = f * f - 1.0;
            let s = (-bq + (bq * bq - 4.0 * a * c).max(0.0).sqrt()) / (2.0 * a);
            if s > 1e-6 { 1.0 / s } else { 1.0 }
        }
    };
    t = match g.spread {
        1 => {
            let m = t.rem_euclid(2.0);
            if m > 1.0 { 2.0 - m } else { m }
        }
        2 => t.rem_euclid(1.0),
        _ => t.clamp(0.0, 1.0),
    };
    let stops = &g.stops;
    match stops.len() {
        0 => [0.0; 4],
        1 => stops[0].1,
        _ => {
            if t <= stops[0].0 {
                return stops[0].1;
            }
            for w in stops.windows(2) {
                let ((r0, c0), (r1, c1)) = (w[0], w[1]);
                if t <= r1 {
                    let k = if r1 > r0 { (t - r0) / (r1 - r0) } else { 1.0 };
                    return std::array::from_fn(|i| c0[i] + (c1[i] - c0[i]) * k);
                }
            }
            stops[stops.len() - 1].1
        }
    }
}

/// Rasterize the solid and gradient fills of a shape (None when it has none): per fill style,
/// even-odd coverage from 4 sub-scanlines per pixel row with exact horizontal span coverage, the
/// fill evaluated at the pixel centre, fill styles composited in order.
pub fn rasterize(shape: &Shape) -> Option<ShapeRaster> {
    if !has_vector_fill(shape) {
        return None;
    }
    let (w, h, origin, scale) = raster_dims(shape);
    let (wu, hu) = (w as usize, h as usize);
    let mut out = vec![[0.0f32; 4]; wu * hu];
    let mut cov = vec![0.0f32; wu * hu];
    let mut xs: Vec<f32> = Vec::new();
    for (fi, fill) in shape.fills.iter().enumerate() {
        if !matches!(fill, Fill::Solid(_) | Fill::Gradient(_)) {
            continue;
        }
        // The fill's segments in pixel space.
        let segs: Vec<[f32; 4]> = shape.edges.iter().filter(|(f, _)| *f == fi).map(|(_, s)| [(s[0] - origin.0) * scale, (s[1] - origin.1) * scale, (s[2] - origin.0) * scale, (s[3] - origin.1) * scale]).collect();
        if segs.is_empty() {
            continue;
        }
        cov.iter_mut().for_each(|c| *c = 0.0);
        let (ymin, ymax) = segs.iter().fold((f32::MAX, f32::MIN), |(a, b), s| (a.min(s[1]).min(s[3]), b.max(s[1]).max(s[3])));
        let r0 = (ymin.floor().max(0.0)) as usize;
        let r1 = (ymax.ceil().max(0.0) as usize).min(hu);
        for row in r0..r1 {
            for sub in 0..4 {
                let y = row as f32 + (sub as f32 + 0.5) / 4.0;
                xs.clear();
                for s in &segs {
                    let (y0, y1) = (s[1], s[3]);
                    if (y0 <= y && y < y1) || (y1 <= y && y < y0) {
                        xs.push(s[0] + (y - y0) * (s[2] - s[0]) / (y1 - y0));
                    }
                }
                xs.sort_by(f32::total_cmp);
                for pair in xs.chunks_exact(2) {
                    let (a, b) = (pair[0].clamp(0.0, w as f32), pair[1].clamp(0.0, w as f32));
                    if b <= a {
                        continue;
                    }
                    let (pa, pb) = (a.floor() as usize, (b.ceil() as usize).min(wu));
                    for px in pa..pb {
                        let o = (b.min(px as f32 + 1.0) - a.max(px as f32)).max(0.0);
                        cov[row * wu + px] += o * 0.25;
                    }
                }
            }
        }
        for row in r0..r1 {
            for px in 0..wu {
                let c = cov[row * wu + px].min(1.0);
                if c <= 0.0 {
                    continue;
                }
                let col = match fill {
                    Fill::Solid(s) => [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0, s[3] as f32 / 255.0],
                    Fill::Gradient(g) => gradient_color(g, origin.0 + (px as f32 + 0.5) / scale, origin.1 + (row as f32 + 0.5) / scale),
                    Fill::Bitmap { .. } => continue,
                };
                let sa = col[3] * c;
                let d = &mut out[row * wu + px];
                let da = d[3];
                let a = sa + da * (1.0 - sa);
                if a > 0.0 {
                    for i in 0..3 {
                        d[i] = (col[i] * sa + d[i] * da * (1.0 - sa)) / a;
                    }
                }
                d[3] = a;
            }
        }
    }
    let rgba = out.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect();
    Some(ShapeRaster { width: w, height: h, rgba, origin, scale })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swf::{Matrix, Rect};

    fn square(x0: f32, y0: f32, x1: f32, y1: f32, f: usize) -> Vec<(usize, [f32; 4])> {
        vec![(f, [x0, y0, x1, y0]), (f, [x1, y0, x1, y1]), (f, [x1, y1, x0, y1]), (f, [x0, y1, x0, y0])]
    }

    /// A frame (outer square minus an inner square, the two loops of one fill) leaves the centre
    /// empty: the even-odd rule over the fill's edges.
    #[test]
    fn a_frame_keeps_its_hole() {
        let mut edges = square(0.0, 0.0, 2000.0, 2000.0, 0);
        edges.extend(square(500.0, 500.0, 1500.0, 1500.0, 0));
        let shape = Shape { bounds: Rect { xmin: 0.0, xmax: 2000.0, ymin: 0.0, ymax: 2000.0 }, fills: vec![Fill::Solid([0, 0, 0, 255])], edges };
        let r = rasterize(&shape).unwrap();
        let at = |x: f32, y: f32| {
            let px = ((x - r.origin.0) * r.scale) as usize;
            let py = ((y - r.origin.1) * r.scale) as usize;
            r.rgba[(py * r.width as usize + px) * 4 + 3]
        };
        assert_eq!(at(250.0, 250.0), 255, "the frame is filled");
        assert_eq!(at(1000.0, 1000.0), 0, "the hole is empty");
    }

    /// A radial gradient from transparent at the centre to black at the edge (a vignette).
    #[test]
    fn a_radial_gradient_darkens_outward() {
        let g = Gradient { matrix: Matrix { a: 0.1, b: 0.0, c: 0.0, d: 0.1, tx: 0.0, ty: 0.0 }, radial: true, focal: 0.0, spread: 0, stops: vec![(0.0, [0.0, 0.0, 0.0, 0.0]), (1.0, [0.0, 0.0, 0.0, 1.0])] };
        assert!(gradient_color(&g, 0.0, 0.0)[3] < 0.01);
        assert!((gradient_color(&g, 819.2, 0.0)[3] - 0.5).abs() < 0.01);
        assert!((gradient_color(&g, 5000.0, 0.0)[3] - 1.0).abs() < 1e-4, "padded past the edge");
    }
}
