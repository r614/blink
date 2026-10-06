//! `fp_probe [AnimSet] [Sequence] [time]`: pose Corvo's first-person rig (Engine
//! Ply_Player.Skm_Player) with a decoded player clip and print key bones in pawn space.
use anyhow::Result;
use dishonored_assets::Content;
use dishonored_assets::characters::{bind_globals, load_anim_set};
use dishonored_assets::skeletal_mesh::read_skeletal_mesh;
use dishonored_ir::BoneAsset;
use ue3_package::ObjRef;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let set = a.first().cloned().unwrap_or_else(|| "Ply_Sword_Idle_as".into());
    let seq = a.get(1).cloned().unwrap_or_else(|| "Sword_Ready_Idle".into());
    let time: f32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let c = Content::locate()?;
    let pkg = c.package("Engine")?;
    let idx = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SkeletalMesh" && pkg.obj_path(ObjRef::Export(i)) == "Ply_Player.Skm_Player").unwrap();
    let m = read_skeletal_mesh(&pkg, idx)?;
    let rm = dishonored_assets::util::rotator_matrix(m.rot_origin[0], m.rot_origin[1], m.rot_origin[2]);
    let m2p = rm * glam::Mat4::from_translation(m.mesh_origin);
    let mut bones: Vec<BoneAsset> = m.bones.iter().map(|b| BoneAsset { name: b.name.clone(), parent: b.parent, rotation: b.orientation, translation: b.position }).collect();
    if std::env::var_os("FP_COUNTS").is_some() {
        for s in ["Ply_Generic_as", "Ply_Sword_Idle_as", "Ply_Guns_as", "Ply_Powers_as", "Ply_Sword_Attack_as"] {
            let l = load_anim_set(&c, "Startup", &format!("{s}.{s}"), &[])?;
            let mut counts: std::collections::BTreeMap<usize, Vec<String>> = Default::default();
            for (n, clip) in &l.clips {
                counts.entry(clip.rotations.first().map(|r| r.len()).unwrap_or(0)).or_default().push(n.clone());
            }
            println!("{s}: {} tracks; clip joint counts: {:?}", l.tracks.len(), counts.iter().map(|(k, v)| (k, v.len(), v.first().cloned())).collect::<Vec<_>>());
        }
        return Ok(());
    }
    // FP_PKG: package holding the set (default Startup), e.g. a level script package.
    let set_pkg = std::env::var("FP_PKG").unwrap_or_else(|_| "Startup".into());
    let lib = load_anim_set(&c, &set_pkg, &format!("{set}.{set}"), &[&seq])?;
    let clip = lib.clips.values().next().expect("clip");
    let mut hit = 0;
    // Edge joint order = the set's tracks sorted by skeleton bone index.
    let mut order: Vec<String> = lib.tracks.clone();
    if std::env::var_os("FP_TRACKORDER").is_none() {
        order.sort_by_key(|n| bones.iter().position(|b| b.name.eq_ignore_ascii_case(n)).unwrap_or(usize::MAX));
    }
    for (t, name) in order.iter().enumerate() {
        let Some(b) = bones.iter().position(|b| b.name.eq_ignore_ascii_case(name)) else { continue };
        hit += 1;
        let (mut r, p) = clip.sample(time, t);
        if std::env::var_os("FP_CONJ").is_some() { r = r.map(|q| q.conjugate()); }
        if std::env::var_os("FP_VERBOSE").is_some() { println!("  track {name:22} bind q {:.3} t {:.1} | clip q {:.3?} t {:.1?}", bones[b].rotation, bones[b].translation, r, p); }
        if std::env::var_os("FP_ROTONLY").is_some() { if let Some(r) = r { bones[b].rotation = r; } continue; }
        if let Some(r) = r { bones[b].rotation = r; }
        if let Some(p) = p { bones[b].translation = p; }
    }
    println!("{} tracks, {hit} matched, {} frames", lib.tracks.len(), clip.num_frames);
    if let Ok(spec) = std::env::var("FP_ADD") {
        let (aset, aseq) = spec.split_once(':').unwrap();
        let alib = load_anim_set(&c, "Startup", &format!("{aset}.{aset}"), &[aseq])?;
        let aclip = alib.clips.values().next().expect("additive clip");
        let mut aorder: Vec<String> = alib.tracks.clone();
        aorder.sort_by_key(|n| bones.iter().position(|b| b.name.eq_ignore_ascii_case(n)).unwrap_or(usize::MAX));
        for (t, name) in aorder.iter().enumerate() {
            let Some(b) = bones.iter().position(|b| b.name.eq_ignore_ascii_case(name)) else { continue };
            let (r, p) = aclip.sample(0.0, t);
            if std::env::var_os("FP_VERBOSE_ADD").is_some() { println!("  add {name:22} q {r:.3?} t {p:.1?}"); }
            if let Some(r) = r { bones[b].rotation = r * bones[b].rotation; }
            if let Some(p) = p { bones[b].translation += p; }
        }
    }
    let g = bind_globals(&bones);
    for n in ["camera_jnt", "head_jnt", "hand_R_jnt", "hand_L_jnt", "handAttachment_R_jnt", "handAttachment_L_jnt", "Root_jnt", "spine_3_jnt"] {
        if let Some(i) = bones.iter().position(|b| b.name == n) {
            let w = m2p * g[i];
            println!("  {n:22} pos {:7.1} X {:5.2} Y {:5.2} Z {:5.2}", w.w_axis.truncate(), w.x_axis.truncate(), w.y_axis.truncate(), w.z_axis.truncate());
        }
    }
    {
        let gp = |n: &str| (m2p * g[bones.iter().position(|b| b.name == n).unwrap()]).w_axis.truncate();
        for side in ["R", "L"] {
            let grip_axis = (gp(&format!("index_0_{side}_jnt")) - gp(&format!("pinky_0_{side}_jnt"))).normalize();
            let sock = m2p * g[bones.iter().position(|b| b.name == format!("handAttachment_{side}_jnt")).unwrap()];
            println!("  {side}: grip axis (pinky->index) {grip_axis:.2}; socket X {:.2} Y {:.2} Z {:.2}", grip_axis.dot(sock.x_axis.truncate().normalize()), grip_axis.dot(sock.y_axis.truncate().normalize()), grip_axis.dot(sock.z_axis.truncate().normalize()));
        }
    }
    let cam = m2p * g[bones.iter().position(|b| b.name == "camera_jnt").unwrap()];
    let wpkg = c.package("Startup")?;
    for (wn, sock, tip) in [("Wpn_PlySwords.Wpn_PlySword01", "handAttachment_R_jnt", glam::Vec3::new(0.0, 0.0, 65.0)), ("Wpn_PlyGuns.Wpn_PlyGunElite", "handAttachment_L_jnt", glam::Vec3::new(0.0, 0.0, 44.0))] {
        let wi = (0..wpkg.exports.len()).find(|&i| wpkg.export_class(i) == "SkeletalMesh" && wpkg.obj_path(ObjRef::Export(i)) == wn).unwrap();
        let w = read_skeletal_mesh(&wpkg, wi)?;
        let wm2p = dishonored_assets::util::rotator_matrix(w.rot_origin[0], w.rot_origin[1], w.rot_origin[2]) * glam::Mat4::from_translation(w.mesh_origin);
        {
            let ra = glam::Mat4::from_quat(w.bones[0].orientation);
            for (label, m) in [("I", glam::Mat4::IDENTITY), ("Wm2p", wm2p), ("Wm2p^-1", wm2p.inverse()), ("Ratt", ra), ("Ratt^-1", ra.inverse())] {
                let d = m.transform_vector3(glam::Vec3::Z);
                println!("    blade (+Z mesh) in socket frame via {label:8}: {d:.2}");
            }
        }
        println!("{wn} on {sock}:");
        attach_report(m2p, g[bones.iter().position(|b| b.name == sock).unwrap()], cam, wm2p, tip);
    }
    Ok(())
}

/// Where a weapon's blade tip lands for candidate attachment conventions (diagnostic).
#[allow(dead_code)]
pub fn attach_report(m2p: glam::Mat4, g_attach: glam::Mat4, cam: glam::Mat4, wm2p: glam::Mat4, tip: glam::Vec3) {
    let inv = cam.inverse();
    for (label, m) in [("A bone*v", m2p * g_attach), ("B bone*Wm2p*v", m2p * g_attach * wm2p), ("C bone*Wm2p^-1*v", m2p * g_attach * wm2p.inverse())] {
        let grip = inv.transform_point3(m.transform_point3(glam::Vec3::ZERO));
        let t = inv.transform_point3(m.transform_point3(tip));
        println!("  {label:14} grip(cam: X up,Y right,-Z fwd) {grip:.1} tip {t:.1}");
    }
}
