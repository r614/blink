//! `edge_probe <package> <animset path> <skeletal mesh path> [seq index]`: decode Edge
//! clip headers/constant channels and compare constant rotations to the bind pose.
use anyhow::{Context, Result};
use dishonored_assets::Content;
use dishonored_assets::edge_anim::EdgeClip;
use dishonored_assets::skeletal_mesh::read_skeletal_mesh;
use ue3_package::properties::{Value, find};
use ue3_package::reader::Reader;
use ue3_package::{FName, ObjRef};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    let pkg = c.package(&a[0])?;
    let find_exp = |path: &str| (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path));
    let set = find_exp(&a[1]).context("animset")?;
    let mesh = read_skeletal_mesh(&pkg, find_exp(&a[2]).context("mesh")?)?;
    let sh = pkg.read_object(set)?;
    let names_prop = find(&sh.properties, "TrackBoneNames").context("TrackBoneNames")?;
    let (n, raw) = names_prop.array_raw().unwrap();
    let mut r = Reader::new(raw);
    let tracks: Vec<String> = (0..n).map(|_| pkg.name(FName::read(&mut r).unwrap())).collect();
    let seqs = find(&sh.properties, "Sequences").unwrap().array_objects()?;
    println!("{} tracks, {} sequences", tracks.len(), seqs.len());
    if a.iter().any(|x| x == "--skel") {
        let b = &mesh.edge_skeleton;
        println!("edge skeleton {} bytes", b.len());
        for row in b.chunks(16).take(40).enumerate() { println!("{:04x}: {}", row.0 * 16, row.1.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")); }
        for (i, t) in tracks.iter().enumerate().take(80) { print!("{i}:{t} "); }
        println!();
        return Ok(());
    }
    if a.iter().any(|x| x == "--stats") {
        let mut acc: std::collections::BTreeMap<(i32, i32), (f64, usize)> = Default::default();
        for s in &seqs {
            let ObjRef::Export(e) = ObjRef::from_index(*s) else { continue };
            let h = pkg.read_object(e)?; let data = pkg.export_data(e)?; let mut r = Reader::at(data, h.native_offset); let _ = r.i32()?; let len = r.i32()? as usize;
            let clip = EdgeClip::parse(r.bytes(len)?)?;
            let vals = clip.const_rotations();
            for pos_shift in -3i32..=3 { for joint_shift in -3i32..=3 {
                for k in 0..vals.len() as i32 {
                    let src = k - pos_shift; if src < 0 || src >= vals.len() as i32 { continue; }
                    let j = vals[k as usize].0 as i32 + joint_shift; if j < 0 { continue; }
                    let q = vals[src as usize].1;
                    let Some(bone) = tracks.get(j as usize) else { continue };
                    let Some(b) = mesh.bones.iter().find(|x| &x.name == bone) else { continue };
                    let ang = (q.dot(b.orientation).abs().min(1.0).acos() * 2.0).to_degrees() as f64;
                    let ent = acc.entry((pos_shift, joint_shift)).or_default(); ent.0 += ang; ent.1 += 1;
                }
            }}
        }
        let mut v: Vec<_> = acc.into_iter().map(|(k, (s, n))| (s / n as f64, k, n)).collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (m, k, n) in v.iter().take(8) { println!("pos_shift {} joint_shift {}: mean angle {m:.1} over {n}", k.0, k.1); }
        return Ok(());
    }
    let pick: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut shown = 0;
    for (si, s) in seqs.iter().enumerate() {
        let ObjRef::Export(e) = ObjRef::from_index(*s) else { continue };
        let h = pkg.read_object(e)?;
        let name = match find(&h.properties, "SequenceName").map(|p| &p.value) { Some(Value::Name(n)) => n.clone(), _ => "?".into() };
        let data = pkg.export_data(e)?;
        let mut r = Reader::at(data, h.native_offset);
        let _raw = r.i32()?;
        let len = r.i32()? as usize;
        let blob = r.bytes(len)?;
        let clip = EdgeClip::parse(blob)?;
        let hd = &clip.header;
        if si < 400 || si == pick {
            println!("[{si}] {name}: joints {} frames {} sets {} dur {:.2} freq {:.1} flags {} constR {} constT {} constS {} animR {} animT {} animS {} specs {}", hd.num_joints, hd.num_frames, hd.num_frame_sets, hd.duration, hd.sample_frequency, hd.flags, hd.num_const_r, hd.num_const_t, hd.num_const_s, hd.num_anim_r, hd.num_anim_t, hd.num_anim_s, clip.specs.len());
        }
        if si == pick && shown == 0 {
            shown += 1;
            println!("  spec0 {:#010x} {:?}", clip.specs[0].raw, clip.specs[0]);
            println!("  blob {} bytes; dma@{:#x} info@{:#x} constR@{:#x} constT@{:#x} specs@{:#x} sizeHeader {} unk14 {}", clip.blob.len(), hd.frame_set_dma, hd.frame_set_info, hd.const_r_data, hd.const_t_data, hd.packing_specs, hd.size_header, hd.unk14);
            let sets = clip.frame_sets();
            for (k, fs) in sets.iter().enumerate() {
                let next = sets.get(k + 1).map(|n| n.start_frame as usize).unwrap_or(hd.num_frames as usize);
                let frames = next - fs.start_frame as usize;
                println!("  frameset {:?}", fs);
                let inner = clip.inner_frames(&sets, k); let sc = clip.sections(fs, inner);
                println!("    inner {inner}: keys end {} next set {:?} q_header {}", sc.keys[3] + fs.sizes[7] as usize, sets.get(k + 1).map(|n| n.blob_offset), sc.q_header);
                let nr = hd.num_anim_r as usize; let nt = hd.num_anim_t as usize;
                let mut rbits = 0u64; let mut tbits = 0u64; let mut rkeys = 0; let mut tkeys = 0;
                for c in 0..nr { let n = (0..inner).filter(|&f| clip.key_bit(&sc, inner, c, f)).count(); rkeys += n; rbits += n as u64 * clip.specs[1 + c].bits() as u64; }
                for c in 0..nt { let n = (0..inner).filter(|&f| clip.key_bit(&sc, inner, nr + c, f)).count(); tkeys += n; tbits += n as u64 * clip.specs[nr + 2 + c].bits() as u64; }
                for order in [dishonored_assets::edge_anim::KeyOrder::ChannelMajor, dishonored_assets::edge_anim::KeyOrder::FrameMajor] {
                    let keys = clip.set_keys(&sets, k, 0, order);
                    let (mut sum, mut n) = (0f64, 0usize);
                    for (c, ch) in keys.iter().enumerate() { let slot = clip.specs[1 + c].recompose; for w in ch.windows(2) { let a = dishonored_assets::edge_anim::recompose_quat(w[0].1, slot); let b = dishonored_assets::edge_anim::recompose_quat(w[1].1, slot); sum += (a.dot(b).abs().min(1.0).acos() * 2.0).to_degrees() as f64; n += 1; } }
                    println!("    {order:?}: mean angular step between consecutive keys {:.2} deg over {n}", sum / n.max(1) as f64);
                }
                println!("    R keys {rkeys} bits {rbits} ({} bytes) vs s4 {}   T keys {tkeys} bits {tbits} ({} bytes) vs s5 {}", rbits.div_ceil(8), fs.sizes[4], tbits.div_ceil(8), fs.sizes[5]);
            }
            let dma = hd.frame_set_dma; println!("  dma raw: {}", clip.blob[dma..(dma + 8 * hd.num_frame_sets as usize + 8).min(clip.blob.len())].iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "));
            let inf = hd.frame_set_info; println!("  info raw: {}", clip.blob[inf..(inf + 4 * hd.num_frame_sets as usize + 8).min(clip.blob.len())].iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "));
            let nr = hd.num_anim_r as usize; let nt = hd.num_anim_t as usize;
            let rb: u32 = clip.specs[1..=nr].iter().map(|s| s.bits()).sum(); let tb: u32 = clip.specs[nr + 2..nr + 2 + nt].iter().map(|s| s.bits()).sum();
            println!("  animR spec bits total {rb} ({} bytes)  animT spec bits total {tb} ({} bytes)", rb.div_ceil(8), tb.div_ceil(8));
            for (k, sp) in clip.specs.iter().enumerate().take(6) { println!("  spec[{k}] {:#010x} bits {} {:?}", sp.raw, sp.bits(), sp.comp); }
            for (j, q) in clip.const_rotations() {
                let bone = tracks.get(j as usize).cloned().unwrap_or("?".into());
                let bind = mesh.bones.iter().find(|b| b.name == bone).map(|b| b.orientation);
                let ang = bind.map(|b| (q.dot(b).abs().min(1.0).acos() * 2.0).to_degrees());
                println!("  constR joint {j} {bone}: {:.4} |q|={:.4}  bind {:?}  angle-to-bind {:?}", q, q.length(), bind.map(|b| format!("{b:.4}")), ang);
            }
            for (j, t) in clip.const_translations() {
                let bone = tracks.get(j as usize).cloned().unwrap_or("?".into());
                let bind = mesh.bones.iter().find(|b| b.name == bone).map(|b| b.position);
                println!("  constT joint {j} {bone}: {:.3}  bind {:?}", t, bind);
            }
        }
    }
    Ok(())
}
