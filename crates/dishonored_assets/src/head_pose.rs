//! The NPC vision bone (`camera_jnt`) baked from the locomotion loops, so the headless AI can
//! place the eye and the vision axis without an animated skeleton.
//!
//! The vision component's eye update (0x00C676A0) takes the bone's component-space matrix
//! (`FUN_00718630`), then `FUN_00B57110` turns its axes by the pawn body tweak's
//! `m_AngleOffsetMatrix` (`DisTweaks_Pawn_Body.m_fBone_AngleOffset_Yaw / _Pitch / _Roll`; NPCs:
//! pitch -90 in `Twk_Pawn_DefaultNPC`) and moves it to the world by the mesh's LocalToWorld.
//! The vision forward is the resulting X axis: `row0(R(offset))` applied to the bone's axes.

use crate::content::Content;
use anyhow::{Context, Result};
use dishonored_ir::ai::{HeadLoop, HeadPose};
use dishonored_ir::npc_anim::LocoConfig;
use glam::{Mat4, Vec3};
use ue3_package::ObjRef;
use ue3_package::properties::{Value, find};

/// `Twk_Vision_Default.m_Vision_BoneName`.
pub const VISION_BONE: &str = "camera_jnt";

/// The (package, export index) of an AnimSet path in the first of `packages` cooking it.
fn locate_set(content: &Content, packages: &[&str], path: &str) -> Option<(String, usize)> {
    for p in packages {
        let Ok(pkg) = content.package(p) else { continue };
        if let Some(i) = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "AnimSet" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path)) {
            return Some((p.to_string(), i));
        }
    }
    None
}

/// Does the AnimSet hold `sequence`?
fn set_has(content: &Content, pkg_name: &str, set: usize, sequence: &str) -> bool {
    let Ok(pkg) = content.package(pkg_name) else { return false };
    let Ok(h) = pkg.read_object(set) else { return false };
    let Some(seqs) = find(&h.properties, "Sequences").and_then(|s| s.array_objects().ok()) else { return false };
    seqs.into_iter().any(|s| {
        let ObjRef::Export(e) = ObjRef::from_index(s) else { return false };
        pkg.read_object(e).ok().is_some_and(|sh| matches!(find(&sh.properties, "SequenceName").map(|v| &v.value), Some(Value::Name(n)) if n.eq_ignore_ascii_case(sequence)))
    })
}

/// The vision forward of a bone matrix: the X axis after the body tweak's angle offset,
/// `R = FRotationMatrix(pitch, yaw, roll)`, new X = `R[0][0] X + R[0][1] Y + R[0][2] Z`
/// (row 0 of a UE3 rotation matrix: `(cos p cos y, cos p sin y, sin p)`).
pub fn vision_forward(g: &Mat4, offset_deg: [f32; 3]) -> Vec3 {
    vision_axes(g, offset_deg).0
}

/// The three axes of the eye matrix (`O * M`, rows of `FRotationMatrix(pitch, yaw, roll)` of the
/// body tweak applied to the bone's axes): forward (X), right (Y) and up (Z).
pub fn vision_axes(g: &Mat4, offset_deg: [f32; 3]) -> (Vec3, Vec3, Vec3) {
    let (yaw, pitch, roll) = (offset_deg[0].to_radians(), offset_deg[1].to_radians(), offset_deg[2].to_radians());
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    let (sr, cr) = roll.sin_cos();
    let rows = [
        Vec3::new(cp * cy, cp * sy, sp),
        Vec3::new(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp),
        Vec3::new(-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp),
    ];
    let (x, y, z) = (g.transform_vector3(Vec3::X), g.transform_vector3(Vec3::Y), g.transform_vector3(Vec3::Z));
    let axis = |r: Vec3| (x * r.x + y * r.y + z * r.z).normalize_or_zero();
    (axis(rows[0]), axis(rows[1]), axis(rows[2]))
}

/// Bake `camera_jnt` over the idle and walk loops (forward loop) of every stance modifier of
/// `loco`, on the body mesh `body` (package, path), with the archetype's AnimSets `sets` (later
/// sets win a name, as the runtime's bank): per frame the bone's position and vision forward
/// in the pawn's frame. The mesh is placed by its `RotOrigin` / `Origin` only
/// (`DishonoredNPCPawn.AnimMesh` sets no `Translation`); the walk loops' root motion is
/// discarded on X / Y (the root bone keeps its bind X / Y, the loops play in place).
pub fn bake(content: &Content, archetype: &str, body: (&str, &str), loco: &LocoConfig, sets: &[&str], packages: &[&str], angle_offset: [f32; 3]) -> Result<HeadPose> {
    let rig = Rig::load(content, archetype, body, sets, packages)?;
    let mut loops: Vec<HeadLoop> = Vec::new();
    for (mi, m) in loco.modifiers.iter().enumerate() {
        for speed in 0..loco.speed_names.len().min(2) {
            let Some(mode) = loco.mode(mi, speed) else { continue };
            // Walk: the forward loop (`m_MoveAnimProps` order S, W, N, E); idle: its one loop.
            let seq = if mode.move_anims.len() >= 4 { mode.move_anims[2].0.clone() } else if let Some(a) = mode.move_anims.first() { a.0.clone() } else { continue };
            if let Some(l) = loops.iter().find(|l| l.sequence.eq_ignore_ascii_case(&seq)).cloned() {
                loops.push(HeadLoop { modifier: m.name.clone(), speed: speed as u8, ..l });
                continue;
            }
            let Some(l) = rig.sequence(content, &seq, angle_offset)? else { continue };
            loops.push(HeadLoop { modifier: m.name.clone(), speed: speed as u8, ..l });
        }
    }
    // The ambient idles (`DisAISubProcessAmbientAnims`: `Empty_Idle_01..` / `Sword_Patrol_Idle_01..`,
    // numbered from 01 while the sequence exists), played once: `modifier` "Ambient", speed 2.
    for prefix in AMBIENT_PREFIXES {
        for n in 1..=AMBIENT_MAX {
            let seq = format!("{prefix}{n:02}");
            if let Some(l) = rig.sequence(content, &seq, angle_offset)? {
                loops.push(HeadLoop { modifier: AMBIENT_MODIFIER.to_string(), speed: AMBIENT_SPEED, ..l });
            }
        }
    }
    Ok(HeadPose { archetype: archetype.to_string(), mesh: body.1.to_string(), loops })
}

/// The ambient idle sequences of the elite guard (`DisNPCAnimDefinitions_EliteGuard.AmbientIdle`
/// groups: `Empty_Idle_01..10`, `Sword_Patrol_Idle_01..10`, the later ones absent from the sets).
pub const AMBIENT_PREFIXES: [&str; 2] = ["Empty_Idle_", "Sword_Patrol_Idle_"];
/// The highest ambient idle number in the definitions.
pub const AMBIENT_MAX: u32 = 10;
/// `HeadLoop::modifier` of an ambient idle.
pub const AMBIENT_MODIFIER: &str = "Ambient";
/// `HeadLoop::speed` of an ambient idle.
pub const AMBIENT_SPEED: u8 = 2;

/// A body mesh's rig and the located AnimSets, to sample `camera_jnt` over sequences.
struct Rig {
    part: dishonored_ir::SkeletalMeshAsset,
    bone: usize,
    bind_root: Vec3,
    located: Vec<(String, usize)>,
}

impl Rig {
    fn load(content: &Content, archetype: &str, body: (&str, &str), sets: &[&str], packages: &[&str]) -> Result<Rig> {
        let rig = crate::characters::load_model(content, archetype, &[body], None, 16)?;
        let part = rig.parts.into_iter().next().context("no body part")?;
        let bone = part.bones.iter().position(|b| b.name.eq_ignore_ascii_case(VISION_BONE)).with_context(|| format!("{} has no {VISION_BONE}", body.1))?;
        let located: Vec<(String, usize)> = sets.iter().filter_map(|s| locate_set(content, packages, s)).collect();
        let bind_root = (part.mesh_to_pawn * crate::characters::bind_globals(&part.bones)[0]).transform_point3(Vec3::ZERO);
        Ok(Rig { part, bone, bind_root, located })
    }

    /// `camera_jnt` over one sequence (later sets win a name); `None` when no set holds it.
    fn sequence(&self, content: &Content, seq: &str, angle_offset: [f32; 3]) -> Result<Option<HeadLoop>> {
        let part = &self.part;
        let Some((pkg_name, set)) = self.located.iter().rev().find(|(p, i)| set_has(content, p, *i, seq)).cloned() else { return Ok(None) };
        let set_path = content.package(&pkg_name)?.obj_path(ObjRef::Export(set));
        let mut lib = crate::characters::load_anim_set(content, &pkg_name, &set_path, &[seq])?;
        lib.edge_order(&part.bones);
        let Some(clip) = lib.clips.get(seq) else { return Ok(None) };
        let track: Vec<Option<usize>> = part.bones.iter().map(|b| lib.tracks.iter().position(|t| t.eq_ignore_ascii_case(&b.name))).collect();
        let mut samples = Vec::with_capacity(clip.num_frames);
        let mut axes = Vec::with_capacity(clip.num_frames);
        for f in 0..clip.num_frames {
            let mut bones = part.bones.clone();
            for (bi, b) in bones.iter_mut().enumerate() {
                let Some(j) = track[bi] else { continue };
                if let Some(Some(q)) = clip.rotations.get(f).and_then(|r| r.get(j)) {
                    b.rotation = *q;
                }
                // Translations for the root and its children only, as the runtime's sampler
                // (deeper bones keep their bind lengths).
                let par = part.bones[bi].parent;
                let shallow = bi == 0 || par <= 0 || part.bones.get(par as usize).is_some_and(|x| x.parent < 0);
                if shallow {
                    if let Some(Some(t)) = clip.translations.get(f).and_then(|r| r.get(j)) {
                        b.translation = *t;
                    }
                }
            }
            let globals = crate::characters::bind_globals(&bones);
            let g: Mat4 = part.mesh_to_pawn * globals[self.bone];
            let root = (part.mesh_to_pawn * globals[0]).transform_point3(Vec3::ZERO);
            let p = g.transform_point3(Vec3::ZERO) - Vec3::new(root.x - self.bind_root.x, root.y - self.bind_root.y, 0.0);
            let (x, y, z) = vision_axes(&g, angle_offset);
            let r = |v: f32| (v * 1000.0).round() / 1000.0;
            samples.push([r(p.x), r(p.y), r(p.z), r(x.x), r(x.y), r(x.z)]);
            axes.push([r(y.x), r(y.y), r(y.z), r(z.x), r(z.y), r(z.z)]);
        }
        Ok(Some(HeadLoop { modifier: String::new(), speed: 0, sequence: seq.to_string(), duration: clip.duration, sample_rate: clip.sample_rate, samples, axes }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints the elite guard's baked head (needs the game data).
    #[test]
    #[ignore]
    fn print_guard_head() {
        let content = Content::locate().unwrap();
        let locos = crate::npc_anim::package_loco_configs(&content, crate::ai_config::NPC_ANIM_PACKAGES);
        let loco = locos.iter().find(|l| l.name.contains("EliteGuard")).unwrap();
        let h = bake(&content, "elite_guard", ("L_Boyle_Ext_Script", "Npc_EliteGuard.Mesh.Skm_EliteGuard_Body"), loco, crate::characters::GUARD_SETS, crate::ai_config::NPC_ANIM_PACKAGES, [0.0, -90.0, 0.0]).unwrap();
        for l in &h.loops {
            let (mut lo, mut hi) = ([f32::MAX; 6], [f32::MIN; 6]);
            for s in &l.samples {
                for k in 0..6 {
                    lo[k] = lo[k].min(s[k]);
                    hi[k] = hi[k].max(s[k]);
                }
            }
            println!("{} speed {} {} {:.2}s {} frames min {:?} max {:?}", l.modifier, l.speed, l.sequence, l.duration, l.samples.len(), lo, hi);
        }
    }
}


