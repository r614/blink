//! `anim_root <package> <animset path> <sequence> [track...]`: sample the translation and
//! rotation of a sequence's tracks (default: the first three in Edge order) over its length,
//! e.g. to see where a cutscene clip's root bone carries the character.
use anyhow::{Context, Result};
use dishonored_assets::Content;
use dishonored_assets::characters::load_anim_set;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(a.len() >= 3, "usage: anim_root <package> <animset path> <sequence> [track...]");
    let c = Content::locate()?;
    let lib = load_anim_set(&c, &a[0], &a[1], &[a[2].as_str()])?;
    let clip = lib.clips.get(&a[2]).context("sequence not decoded")?;
    println!("{} tracks; {} frames at {} Hz, {:.3} s", lib.tracks.len(), clip.num_frames, clip.sample_rate, clip.duration);
    let wanted: Vec<usize> = if a.len() > 3 { a[3..].iter().filter_map(|t| lib.tracks.iter().position(|x| x.eq_ignore_ascii_case(t))).collect() } else { (0..3.min(lib.tracks.len())).collect() };
    for j in wanted {
        println!("track {j} {}", lib.tracks.get(j).map(String::as_str).unwrap_or("?"));
        let n = clip.num_frames.max(1);
        for k in (0..n).step_by((n / 12).max(1)).chain(std::iter::once(n - 1)) {
            let t = clip.translations.get(j).and_then(|v| v.get(k)).copied().flatten();
            let r = clip.rotations.get(j).and_then(|v| v.get(k)).copied().flatten();
            println!("  frame {k:4} ({:6.2} s): t {:?} r {:?}", k as f32 / clip.sample_rate.max(1.0), t.map(|v| (v.x.round(), v.y.round(), v.z.round())), r.map(|q| q.to_euler(glam::EulerRot::ZYX)));
        }
    }
    Ok(())
}
