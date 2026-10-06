//! Parse and bake every AnimSequence in the install; report flags and decode coverage.
use dishonored_assets::edge_anim::EdgeClip;
use std::collections::BTreeMap;
use ue3_package::properties::find;
use ue3_package::reader::Reader;
use ue3_package::Package;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or(r"C:\Program Files\GOG Galaxy\Games\Dishonored\DishonoredGame\CookedPCConsole".into());
    let mut flags: BTreeMap<u16, usize> = BTreeMap::new();
    let (mut total, mut parsed, mut baked) = (0usize, 0usize, 0usize);
    let mut fail: BTreeMap<String, usize> = BTreeMap::new();
    let mut fixed: Vec<f32> = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("upk"))).collect();
    paths.sort();
    for p in paths {
        let Ok(pkg) = Package::open(&p) else { continue };
        for i in 0..pkg.exports.len() {
            if pkg.export_class(i) != "AnimSequence" { continue; }
            let Ok(h) = pkg.read_object(i) else { continue };
            if find(&h.properties, "SequenceName").is_none() { continue; }
            total += 1;
            let Ok(data) = pkg.export_data(i) else { continue };
            let mut r = Reader::at(data, h.native_offset);
            let (Ok(_), Ok(len)) = (r.i32(), r.i32()) else { continue };
            let Ok(blob) = r.bytes(len.max(0) as usize) else { continue };
            match EdgeClip::parse(blob) {
                Ok(c) => {
                    parsed += 1;
                    *flags.entry(c.header.flags).or_default() += 1;
                    match c.bake() {
                        Ok(b) => {
                            baked += 1;
                            // Fixed-format rotations: largest frame-to-frame turn of any joint.
                            if c.header.flags & 1 == 0 {
                                let mut worst = 0.0f32;
                                for f in 1..b.num_frames {
                                    for j in 0..b.rotations[f].len() {
                                        if let (Some(a), Some(q)) = (b.rotations[f - 1][j], b.rotations[f][j]) {
                                            worst = worst.max(a.angle_between(q).to_degrees());
                                        }
                                    }
                                }
                                fixed.push(worst);
                                if fixed.len() <= 5 {
                                    println!("fixed-format {} {}: {} frames, worst step {worst:.1} deg", p.display(), pkg.obj_path(ue3_package::ObjRef::Export(i)), b.num_frames);
                                }
                            }
                        }
                        Err(e) => *fail.entry(format!("{e:#}").chars().take(60).collect()).or_default() += 1,
                    }
                }
                Err(e) => *fail.entry(format!("parse: {e:#}").chars().take(60).collect()).or_default() += 1,
            }
        }
    }
    println!("sequences {total}, parsed {parsed}, baked {baked}");
    println!("flags histogram {flags:?}");
    for (k, n) in fail { println!("fail x{n}: {k}"); }
    fixed.sort_by(f32::total_cmp);
    if !fixed.is_empty() {
        println!("fixed-format clips {}: worst frame step median {:.1} deg, 95% {:.1}, max {:.1}; over 45 deg: {}", fixed.len(), fixed[fixed.len() / 2], fixed[fixed.len() * 95 / 100], fixed[fixed.len() - 1], fixed.iter().filter(|w| **w > 45.0).count());
    }
}
