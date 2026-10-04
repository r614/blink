//! Render decoded Edge clips as side-view stick figures (offline verification).
//! `anim_sheet <out.png> [clip names...]` (elite guard skeleton + Npc_EliteGuard_empty_as)
use anyhow::Result;
use dishonored_assets::Content;
use dishonored_assets::characters::{bind_globals, load_anim_set};
use dishonored_assets::skeletal_mesh::read_skeletal_mesh;
use dishonored_ir::BoneAsset;
use image::{Rgb, RgbImage};
use ue3_package::ObjRef;

fn line(img: &mut RgbImage, a: (f32, f32), b: (f32, f32), c: Rgb<u8>) {
    let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()) as usize).max(1);
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
        for (dx, dy) in [(0, 0), (1, 0), (0, 1)] {
            let (px, py) = (x as i32 + dx, y as i32 + dy);
            if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                img.put_pixel(px as u32, py as u32, c);
            }
        }
    }
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = a.first().cloned().unwrap_or("anim_sheet.png".into());
    let names: Vec<&str> = if a.len() > 1 { a[1..].iter().map(|s| s.as_str()).collect() } else { vec!["Empty_Idle", "Empty_WalkN", "Empty_RunN"] };
    let c = Content::locate()?;
    let pkg = c.package("L_Boyle_Ext_Script")?;
    let mi = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)) == "Npc_EliteGuard.Mesh.Skm_EliteGuard_Body").unwrap();
    let mesh = read_skeletal_mesh(&pkg, mi)?;
    let bones: Vec<BoneAsset> = mesh.bones.iter().map(|b| BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
    let lib = load_anim_set(&c, "L_Boyle_Ext_Script", "Npc_EliteGuard_empty_as.Npc_EliteGuard_empty_as", &names)?;
    let map: Vec<Option<usize>> = lib.tracks.iter().map(|t| bones.iter().position(|b| &b.name == t)).collect();
    let cols = 8u32;
    let (cw, ch) = (180u32, 260u32);
    let mut img = RgbImage::from_pixel(cw * cols, ch * names.len() as u32 + ch, Rgb([24, 24, 28]));
    let mut rows: Vec<(String, Option<&dishonored_assets::edge_anim::BakedClip>)> = vec![("BIND".into(), None)];
    for n in &names {
        rows.push((n.to_string(), lib.clips.get(*n)));
    }
    for (ri, (name, clip)) in rows.iter().enumerate() {
        println!("row {ri}: {name} {}", clip.map(|c| format!("{} frames", c.num_frames)).unwrap_or_default());
        for col in 0..cols {
            let mut posed = bones.clone();
            if let Some(clip) = clip {
                let t = clip.duration * col as f32 / cols as f32;
                for (ti, b) in map.iter().enumerate() {
                    if let (Some(b), (Some(q), _)) = (b, clip.sample(t, ti)) {
                        posed[*b].rotation = q;
                    }
                }
            }
            let g = bind_globals(&posed);
            // Mesh space is Y-down (height = -Y), forward = +Z: side view plots (Z, -Y).
            let ox = (col * cw) as f32 + cw as f32 * 0.5;
            let oy = (ri as u32 * ch) as f32 + ch as f32 - 20.0;
            let s = 1.2;
            for (i, b) in posed.iter().enumerate() {
                if b.parent < 0 || i == 0 { continue; }
                let p = g[i].w_axis.truncate();
                let q = g[b.parent as usize].w_axis.truncate();
                let n = b.name.to_ascii_lowercase();
                let col = if n.contains("_l_") || n.ends_with("_l_jnt") { Rgb([90, 170, 255]) } else if n.contains("_r_") || n.ends_with("_r_jnt") { Rgb([255, 120, 90]) } else { Rgb([230, 230, 230]) };
                line(&mut img, (ox + p.z * s, oy + p.y * s), (ox + q.z * s, oy + q.y * s), col);
            }
        }
    }
    img.save(&out)?;
    println!("wrote {out}");
    Ok(())
}
