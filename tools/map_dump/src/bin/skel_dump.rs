//! `skel_dump <package> [export-substring]`: decode skeletal meshes and print a summary.
use anyhow::Result;
use dishonored_assets::Content;
use dishonored_assets::skeletal_mesh::read_skeletal_mesh;
use ue3_package::ObjRef;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    let pkg = c.package(&a[0])?;
    let filt = a.get(1).cloned().unwrap_or_default().to_ascii_lowercase();
    let (mut ok, mut fail) = (0, 0);
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "SkeletalMesh" { continue; }
        let path = pkg.obj_path(ObjRef::Export(i));
        if !path.to_ascii_lowercase().contains(&filt) { continue; }
        match read_skeletal_mesh(&pkg, i) {
            Ok(m) => {
                ok += 1;
                let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
                for p in &m.positions { lo = lo.min(*p); hi = hi.max(*p); }
                let rm = dishonored_assets::util::rotator_matrix(m.rot_origin[0], m.rot_origin[1], m.rot_origin[2]);
                for (label, f) in [("R*(v-o)", 0), ("R*v-o", 1), ("R*(v+o)", 2)] { let (mut zl, mut zh) = (f32::MAX, f32::MIN); let mut fwd = glam::Vec3::ZERO; for p in &m.positions { let q = match f { 0 => rm.transform_vector3(*p - m.mesh_origin), 1 => rm.transform_vector3(*p) - m.mesh_origin, _ => rm.transform_vector3(*p + m.mesh_origin) }; zl = zl.min(q.z); zh = zh.max(q.z); fwd += q; } println!("   {label}: z {zl:.0}..{zh:.0} mean {:.1}", fwd / m.positions.len() as f32); }
                println!("OK {path}: {} bones, {} verts, {} tris, {} sections, {} mats, edge skel {} B, origin {:?} rot {:?}, bounds {lo:.0}..{hi:.0}", m.bones.len(), m.positions.len(), m.indices.len() / 3, m.sections.len(), m.materials.len(), m.edge_skeleton_bytes, m.mesh_origin, m.rot_origin);
                if a.iter().any(|x| x == "--conv") { for conj in [false, true] { let g = globals(&m.bones, conj); for (bi, b) in m.bones.iter().enumerate() { if ["wrist", "ankle", "toe", "hand_jnt", "knee", "elbow"].iter().any(|k| b.name.contains(k)) { println!("  conj={conj} {} -> {:.1}", b.name, g[bi]); } } } }
                if a.iter().any(|x| x == "--bones") {
                    for (bi, b) in m.bones.iter().enumerate().take(80) { println!("  bone {bi} {} parent {} pos {:.1} rot {:.3}", b.name, b.parent, b.position, b.orientation); }
                }
            }
            Err(e) => { fail += 1; println!("FAIL {path}: {e:#}"); }
        }
    }
    println!("ok {ok} failed {fail}");
    Ok(())
}

/// Global bind positions of bones under two quaternion conventions (diagnostic).
#[allow(dead_code)]
pub fn globals(bones: &[dishonored_assets::skeletal_mesh::Bone], conj_nonroot: bool) -> Vec<glam::Vec3> {
    let mut rot: Vec<glam::Quat> = Vec::new();
    let mut pos: Vec<glam::Vec3> = Vec::new();
    for (i, b) in bones.iter().enumerate() {
        let q = if conj_nonroot && i > 0 { b.orientation.conjugate() } else { b.orientation };
        if b.parent < 0 || i == 0 {
            rot.push(q);
            pos.push(b.position);
        } else {
            let p = b.parent as usize;
            pos.push(pos[p] + rot[p] * b.position);
            rot.push(rot[p] * q);
        }
    }
    pos
}
