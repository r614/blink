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
    let mut lib = load_anim_set(&c, &a[0], &a[1], &[a[2].as_str()])?;
    // ANIM_ROOT_SKEL=<package>:<mesh path>: put the tracks in that skeleton's Edge joint order.
    if let Some((p, m)) = std::env::var("ANIM_ROOT_SKEL").ok().as_deref().and_then(|s| s.split_once(':')) {
        let pkg = c.package(p)?;
        let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SkeletalMesh" && pkg.obj_path(ue3_package::ObjRef::Export(i)).eq_ignore_ascii_case(m)).context("mesh")?;
        let raw = dishonored_assets::skeletal_mesh::read_skeletal_mesh(&pkg, i)?;
        let bones: Vec<dishonored_ir::BoneAsset> = raw.bones.iter().map(|b| dishonored_ir::BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
        lib.edge_order(&bones);
    }
    let clip = lib.clips.get(&a[2]).context("sequence not decoded")?;
    println!("{} tracks; {} frames at {} Hz, {:.3} s", lib.tracks.len(), clip.num_frames, clip.sample_rate, clip.duration);
    let wanted: Vec<usize> = if a.len() > 3 { a[3..].iter().filter_map(|t| lib.tracks.iter().position(|x| x.eq_ignore_ascii_case(t))).collect() } else { (0..3.min(lib.tracks.len())).collect() };
    if std::env::var("ANIM_ROOT_SUMMARY").is_ok() {
        // Per track: how far its evaluated translation and rotation range over the clip.
        for (j, name) in lib.tracks.iter().enumerate() {
            let (mut tmax, mut rmax, mut any) = (0.0f32, 0.0f32, false);
            let (q0, t0) = clip.sample(0.0, j);
            let mut t = 0.0f32;
            while t <= clip.duration + 1e-4 {
                let (q, p) = clip.sample(t, j);
                if let (Some(a), Some(b)) = (p, t0) {
                    tmax = tmax.max(a.distance(b));
                }
                if let (Some(a), Some(b)) = (q, q0) {
                    rmax = rmax.max(a.angle_between(b).to_degrees());
                }
                any |= q.is_some() || p.is_some();
                t += 0.05;
            }
            println!("track {j:3} {name:24} data {any} translation range {tmax:7.2} rotation range {rmax:7.2} deg, at 0: t {:?} q {:?}", t0.map(|v| (v.x.round(), v.y.round(), v.z.round())), q0);
        }
        return Ok(());
    }
    let sampled = std::env::var("ANIM_ROOT_SAMPLE").is_ok();
    for j in wanted {
        println!("track {j} {}", lib.tracks.get(j).map(String::as_str).unwrap_or("?"));
        if sampled {
            // Evaluated (interpolated) transform every 0.1 s.
            let mut t = 0.0f32;
            while t <= clip.duration + 1e-4 {
                let (q, p) = clip.sample(t, j);
                println!("  {t:6.2} s: t {:?} r(yaw,pitch,roll deg) {:?}", p.map(|v| (v.x.round(), v.y.round(), v.z.round())), q.map(|q| { let e = q.to_euler(glam::EulerRot::ZYX); (e.0.to_degrees().round(), e.1.to_degrees().round(), e.2.to_degrees().round()) }));
                t += 0.1;
            }
            continue;
        }
        let n = clip.num_frames.max(1);
        for k in (0..n).step_by((n / 12).max(1)).chain(std::iter::once(n - 1)) {
            let t = clip.translations.get(j).and_then(|v| v.get(k)).copied().flatten();
            let r = clip.rotations.get(j).and_then(|v| v.get(k)).copied().flatten();
            println!("  frame {k:4} ({:6.2} s): t {:?} r {:?}", k as f32 / clip.sample_rate.max(1.0), t.map(|v| (v.x.round(), v.y.round(), v.z.round())), r.map(|q| q.to_euler(glam::EulerRot::ZYX)));
        }
    }
    Ok(())
}
