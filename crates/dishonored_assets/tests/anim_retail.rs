//! Character presentation data against the canonical GOG build (skipped without the install):
//! physics assets, fixed-format Edge channels, partial clips, sockets and accessories.

use dishonored_assets::Content;
use dishonored_assets::edge_anim::EdgeClip;
use dishonored_assets::skeletal_mesh::read_skeletal_mesh;
use ue3_package::properties::{Value, find};
use ue3_package::{ObjRef, Package};

fn content() -> Option<Content> {
    Content::locate().ok()
}

fn export(pkg: &Package, path: &str) -> usize {
    (0..pkg.exports.len()).find(|&i| pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path)).unwrap_or_else(|| panic!("{path}"))
}

/// (sequence name, Edge clip) of every sequence of an AnimSet.
fn clips(pkg: &Package, set: usize) -> Vec<(String, EdgeClip)> {
    let h = pkg.read_object(set).unwrap();
    let mut out = Vec::new();
    for s in find(&h.properties, "Sequences").unwrap().array_objects().unwrap() {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { continue };
        let sh = pkg.read_object(e).unwrap();
        let Some(Value::Name(name)) = find(&sh.properties, "SequenceName").map(|p| p.value.clone()) else { continue };
        let data = pkg.export_data(e).unwrap();
        let mut r = ue3_package::reader::Reader::at(data, sh.native_offset);
        let _ = r.i32().unwrap();
        let len = r.count(1).unwrap();
        out.push((name, EdgeClip::parse(r.bytes(len).unwrap()).unwrap()));
    }
    out
}

fn tracks(pkg: &Package, set: usize) -> Vec<String> {
    let h = pkg.read_object(set).unwrap();
    let (n, raw) = find(&h.properties, "TrackBoneNames").and_then(|p| p.array_raw()).unwrap();
    let mut r = ue3_package::reader::Reader::new(raw);
    (0..n).map(|_| pkg.name(ue3_package::FName::read(&mut r).unwrap())).collect()
}

#[test]
fn elite_guard_physics_asset() {
    let Some(c) = content() else { return };
    let r = ("L_Boyle_Ext_Script".to_string(), "Npc_EliteGuard.Physics.Skm_EliteGuard_Physics".to_string());
    let a = dishonored_assets::npc_anim::load_physics_asset(&c, &r).unwrap();
    assert_eq!(a.bodies.len(), 18);
    assert_eq!(a.constraints.len(), 17);
    let spine = a.constraints.iter().find(|k| k.bone1 == "spine_1_jnt").unwrap();
    assert_eq!(spine.bone2, "Root_jnt");
    assert!(spine.swing_limited && (spine.swing2 - 30.0).abs() < 1e-3 && (spine.twist - 6.0).abs() < 1e-3);
    let arm = a.bodies.iter().find(|b| b.bone == "upper_arm_L_jnt").unwrap();
    assert!((arm.radius - 9.6676).abs() < 1e-3 && (arm.length - 23.0218).abs() < 1e-3);
    // The pelvis body is a box: its radius is half its smallest side.
    let root = a.bodies.iter().find(|b| b.bone == "Root_jnt").unwrap();
    assert!((root.radius - 19.0355 / 2.0).abs() < 1e-3);
}

/// Clips whose rotations use the fixed 48-bit format (flag bit 0 clear) bake to unit
/// quaternions that move smoothly from frame to frame.
#[test]
fn fixed_format_rotations_bake_smoothly() {
    let Some(c) = content() else { return };
    let pkg = c.package("Startup").unwrap();
    let mut checked = 0;
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "AnimSet" || pkg.obj_path(ObjRef::Export(i)).starts_with("Default__") {
            continue;
        }
        for (name, clip) in clips(&pkg, i) {
            if clip.header.flags & 1 != 0 || clip.header.num_anim_r == 0 {
                continue;
            }
            let b = clip.bake().unwrap_or_else(|e| panic!("{name}: {e:#}"));
            let mut worst = 0.0f32;
            for j in 0..b.rotations[0].len() {
                for f in 1..b.num_frames {
                    if let (Some(a), Some(q)) = (b.rotations[f - 1][j], b.rotations[f][j]) {
                        assert!((q.length() - 1.0).abs() < 1e-3, "{name}");
                        worst = worst.max(a.angle_between(q).to_degrees());
                    }
                }
            }
            assert!(worst < 45.0, "{name}: {worst} degrees between frames");
            checked += 1;
        }
    }
    assert!(checked > 0, "no fixed-format clip in Startup");
}

/// U-024: a clip with fewer joints than its set's tracks lacks exactly the tracks its
/// skeleton does not have (they sort last in Edge order).
#[test]
fn partial_clips_drop_tracks_missing_from_the_skeleton() {
    let Some(c) = content() else { return };
    let eng = c.package("Engine").unwrap();
    let skel = read_skeletal_mesh(&eng, export(&eng, "Ply_Player.Skm_Player")).unwrap();
    let pkg = c.package("Startup").unwrap();
    let mut seen = 0;
    for set in ["Ply_Powers_as.Ply_Powers_as", "Ply_Guns_as.Ply_Guns_as", "Ply_Sword_Idle_as.Ply_Sword_Idle_as"] {
        let s = export(&pkg, set);
        let t = tracks(&pkg, s);
        let missing = t.iter().filter(|n| !skel.bones.iter().any(|b| b.name.eq_ignore_ascii_case(n))).count();
        for (name, clip) in clips(&pkg, s) {
            let nj = clip.header.num_joints as usize;
            if nj < t.len() {
                println!("{set} {name}: {nj} of {} joints, {missing} tracks not in Skm_Player", t.len());
                assert_eq!(t.len() - nj, missing, "{name}");
                seen += 1;
            }
        }
    }
    println!("{seen} partial clips");
}

#[test]
fn player_rig_weapon_sockets() {
    let Some(c) = content() else { return };
    let s = dishonored_assets::characters::mesh_sockets(&c, "Engine", "Ply_Player.Skm_Player").unwrap();
    let r = s.iter().find(|x| x.name == "RightHandWpn").unwrap();
    assert_eq!(r.bone, "handAttachment_R_jnt");
    assert_eq!(r.rotation, [0, 16384, 0]);
    let l = s.iter().find(|x| x.name == "LeftHandWpn").unwrap();
    assert_eq!(l.rotation, [0, -16384, 16384]);
}

#[test]
fn helmeted_guard_wears_the_elite_helmet() {
    let Some(c) = content() else { return };
    let pkgs: Vec<String> = ["L_Boyle_Ext_Script", "L_Boyle_Int_Script", "L_BoyleStreet_Ext_Script"].iter().map(|s| s.to_string()).collect();
    let t = "Twk_Pawn_Boyle_Party.Pwn_EliteGuard_MTall_2_Helm1_Party".to_string();
    let looks = dishonored_assets::characters::resolve_appearances(&c, &pkgs, std::slice::from_ref(&t)).unwrap();
    let a = &looks[&t];
    assert_eq!(a.anim.physics.as_ref().map(|p| p.1.as_str()), Some("Npc_EliteGuard.Physics.Skm_EliteGuard_Physics"));
    let acc = a.accessories(7);
    assert_eq!(acc.len(), 1, "{:?}", a.accessories);
    assert_eq!(acc[0].0.1, "Npc_EliteGuard.Mesh.Stm_EliteHelmet");
    assert_eq!(acc[0].1, "Helmet");
    let m = dishonored_assets::characters::load_static_model(&c, &acc[0].0.0, &acc[0].0.1, 64).unwrap();
    assert!(!m.parts[0].indices.is_empty());
}
