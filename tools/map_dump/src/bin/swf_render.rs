//! `swf_render <package> <movie> <out.png> [background.png] [--only path-prefix] [--label path=label ...]`
//! Flatten a GFx movie's root timeline into textured quads and rasterize them at stage size
//! (local research only; output must never be committed).
use anyhow::{Context, Result};
use dishonored_assets::Content;
use dishonored_assets::swf::{ColorTransform, Matrix, Movie, Quad, Sprite};
use dishonored_assets::texture::read_texture;
use std::collections::HashMap;
use ue3_package::ObjRef;
use ue3_package::properties::find;

#[path = "../shared/bcdec.rs"]
mod bcdec;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    let pkg = c.package(&a[0])?;
    let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SwfMovie" && pkg.obj_path(ObjRef::Export(i)).contains(&a[1])).context("no SwfMovie")?;
    let h = pkg.read_object(i)?;
    let (n, raw) = find(&h.properties, "RawData").and_then(|p| p.array_raw()).context("RawData")?;
    let movie = Movie::parse(&raw[..n])?;
    let opt = |f: &str| a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).cloned();
    let only = opt("--only");
    let labels: Vec<(String, String)> = a.windows(2).filter(|w| w[0] == "--label").filter_map(|w| w[1].split_once('=').map(|(p, l)| (p.to_string(), l.to_string()))).collect();
    let frame_of = |path: &str, s: &Sprite| -> usize { labels.iter().find(|(p, _)| p == path).and_then(|(_, l)| Movie::label_frame(s, l)).unwrap_or(0) };
    let mut quads: Vec<Quad> = Vec::new();
    // Root timeline frame 0.
    for p in movie.display_list(&movie.root, 0) {
        let Some(ch) = p.character else { continue };
        let name = p.name.clone().unwrap_or_default();
        movie.flatten(ch, p.matrix.unwrap_or_default(), ColorTransform::default(), &name, &frame_of, &mut quads);
    }
    if let Some(o) = &only {
        quads.retain(|q| q.path.starts_with(o.as_str()));
    }
    let (w, hgt) = ((movie.frame_size.xmax / 20.0) as u32, (movie.frame_size.ymax / 20.0) as u32);
    let mut img: Vec<u8> = match a.get(3).filter(|s| !s.starts_with("--")) {
        Some(bg) => {
            let b = image::open(bg)?.resize_exact(w, hgt, image::imageops::FilterType::Triangle).to_rgba8();
            b.into_raw()
        }
        None => vec![60; (w * hgt * 4) as usize],
    };
    let mut texs: HashMap<String, (u32, u32, Vec<u8>)> = HashMap::new();
    for q in &quads {
        if !texs.contains_key(&q.texture) {
            let ti = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "Texture2D" && pkg.obj_path(ObjRef::Export(i)).ends_with(&format!(".{}", q.texture)));
            let t = ti.and_then(|ti| read_texture(&c, &pkg, ti, 4096).ok()).and_then(|t| bcdec::decode(t.format, t.width, t.height, &t.mips[0]).map(|d| (t.width, t.height, d)));
            texs.insert(q.texture.clone(), t.unwrap_or((1, 1, vec![255, 0, 255, 255])));
        }
        let (tw, th, td) = &texs[&q.texture];
        // Affine from src rect (u,v in [0,1]) to dst: P = TL + u*(TR-TL) + v*(BL-TL).
        let (tl, tr, bl) = (q.dst[0], q.dst[1], q.dst[3]);
        let (ex, ey) = (tr.0 - tl.0, tr.1 - tl.1);
        let (fx, fy) = (bl.0 - tl.0, bl.1 - tl.1);
        let det = ex * fy - ey * fx;
        if det.abs() < 1e-6 {
            continue;
        }
        let xs = q.dst.iter().map(|p| p.0);
        let ys = q.dst.iter().map(|p| p.1);
        let (x0, x1) = (xs.clone().fold(f32::MAX, f32::min).max(0.0) as i32, xs.fold(f32::MIN, f32::max).min(w as f32 - 1.0) as i32);
        let (y0, y1) = (ys.clone().fold(f32::MAX, f32::min).max(0.0) as i32, ys.fold(f32::MIN, f32::max).min(hgt as f32 - 1.0) as i32);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5 - tl.0, y as f32 + 0.5 - tl.1);
                let u = (px * fy - py * fx) / det;
                let v = (ex * py - ey * px) / det;
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let sx = (q.src[0] + u * (q.src[2] - q.src[0])) as u32;
                let sy = (q.src[1] + v * (q.src[3] - q.src[1])) as u32;
                if sx >= *tw || sy >= *th {
                    continue;
                }
                let s = ((sy * tw + sx) * 4) as usize;
                let mut col = [0f32; 4];
                for k in 0..4 {
                    col[k] = (td[s + k] as f32 / 255.0 * q.color.mul[k] + q.color.add[k]).clamp(0.0, 1.0);
                }
                let o = ((y as u32 * w + x as u32) * 4) as usize;
                for k in 0..3 {
                    img[o + k] = (img[o + k] as f32 * (1.0 - col[3]) + col[k] * 255.0 * col[3]) as u8;
                }
            }
        }
    }
    let mut paths: Vec<&str> = quads.iter().map(|q| q.path.as_str()).collect();
    paths.dedup();
    println!("{} quads; paths: {:?}", quads.len(), paths.iter().take(60).collect::<Vec<_>>());
    image::save_buffer(&a[2], &img, w, hgt, image::ExtendedColorType::Rgba8)?;
    let _ = Matrix::default();
    Ok(())
}
