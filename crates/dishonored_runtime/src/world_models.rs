//! World models of projectiles, gadgets and swarm rats, from the original's tweaks:
//!
//! - `Twk_Proj_Grenade` `DisTweaks_Grenade.m_pSkeletalMesh` = `Grenade.Grenade`;
//!   `Twk_Proj_StickyGrenade` = `StickyBomb.StickyBomb_world_SKM` (Startup);
//! - `Twk_Proj_Arrow` / `_Sleep` / `_Flare` `m_ArrowMeshInfo.m_pArrowMesh` = `Crossbows.bolt_01`,
//!   `Crossbows.Bolt_Sleep`, `Crossbows.Bolt_Flare`; `Twk_Proj_Bullet.m_pBulletMesh` =
//!   `Wpn_Bullet.skm_bullet`;
//! - the placed Spring Razor `SpringRazor.SpringRazor`;
//! - the rat crowd agents (`DisGameCrowdAgentSkeletalRat`) `Npc_SmallRat.Mesh.Npc_SmallRat`
//!   (DishonoredGame).
//!
//! Each model is drawn at the object's transform: projectiles along their velocity, razors on
//! their surface, rats at the pawn facing its yaw. Animations:
//! - the grenade and the placed razor loop their only sequences (`Grenade_as` `Grenade_Idle`,
//!   `SpringRazor_as` `SpringRazor_Idle`: `Grenade_at` / `SpringRazor_at` play nothing else);
//! - rats play `Npc_SmallRat_as` through `ArkAnimNodeRat` (`RatSwarm.CrowdAgent_SmallRat_at`;
//!   the node's move table from `Default__ArkAnimNodeRat`, its blend from 0x00913ED0, below),
//!   a random idle of the agent's `IdleAnimNames` when slower than `m_fMinMovementVelocity`,
//!   the attached attack (`m_AttachedAttackAnimNames` `SwarmAttack` / `SwarmAttack1`) while
//!   biting, and a random mesh scale in `MeshMinScale3D`..`MeshMaxScale3D` (0.8..1.2).

use crate::characters::{CharacterGpu, upload};
use crate::coords;
use crate::session::SessionView;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::prelude::*;
use dishonored_sim::state::{PawnKind, ProjectileKind};
use std::collections::HashMap;

/// (key, package, skeletal mesh path).
const MODELS: &[(&str, &str, &str)] = &[
    ("grenade", "Startup", "Grenade.Grenade"),
    ("sticky", "Startup", "StickyBomb.StickyBomb_world_SKM"),
    ("bolt", "Startup", "Crossbows.bolt_01"),
    ("sleep", "Startup", "Crossbows.Bolt_Sleep"),
    ("flare", "Startup", "Crossbows.Bolt_Flare"),
    ("bullet", "Startup", "Wpn_Bullet.skm_bullet"),
    ("razor", "Startup", "SpringRazor.SpringRazor"),
    ("rat", "DishonoredGame", "Npc_SmallRat.Mesh.Npc_SmallRat"),
];

/// `Default__ArkAnimNodeRat.m_MoveAnims` (Engine): (sequence, speed in uu/s).
const RAT_MOVES: [(&str, f32); 3] = [("WalkSlowN", 55.0), ("WalkN", 98.0), ("RunN", 302.0)];
/// `Default__ArkAnimNodeRat.m_fMaxPlayRate`.
const RAT_MAX_RATE: f32 = 1.5;
/// `RatSwarm.CrowdAgent_SmallRat.m_fMinMovementVelocity`.
const RAT_MIN_SPEED: f32 = 12.5;
/// `RatSwarm.CrowdAgent_SmallRat.IdleAnimNames` (5) and `Twk_Rat.m_AttachedAttackAnimNames`.
const RAT_IDLES: [&str; 5] = ["Idle", "Idle1", "Idle2", "Idle3", "Idle4"];
const RAT_ATTACKS: [&str; 2] = ["SwarmAttack", "SwarmAttack1"];
/// `MeshMinScale3D` / `MeshMaxScale3D`.
const RAT_SCALE: (f32, f32) = (0.8, 1.2);

/// (model key, package, AnimSet path, looped sequence played alone or None for the rat node).
const ANIMS: &[(&str, &str, &str, Option<&str>)] = &[
    ("grenade", "Startup", "Grenade_as.Grenade_as", Some("Grenade_Idle")),
    ("razor", "Startup", "SpringRazor_as.SpringRazor_as", Some("SpringRazor_Idle")),
    ("rat", "DishonoredGame", "Npc_SmallRat_as.Npc_SmallRat_as", None),
];

/// One live model: its root, joints and animation clock.
struct Live {
    root: Entity,
    joints: Vec<Entity>,
    key: &'static str,
    time: f32,
    /// `ArkAnimNodeRat.m_fMovePct`.
    move_pct: f32,
    /// The idle or attack being played and its start.
    specific: Option<(&'static str, f32)>,
    seed: u32,
}

#[derive(Resource, Default)]
pub struct WorldModels {
    models: HashMap<&'static str, CharacterGpu>,
    libs: HashMap<&'static str, crate::viewmodel::ClipLib>,
    /// Live instances: (kind 0 projectile / 1 trap / 2 rat, id).
    live: HashMap<(u8, u32), Live>,
}

impl WorldModels {
    /// Whether a projectile kind is drawn by its model (the generic tracer then stays out).
    pub fn draws(&self, k: ProjectileKind) -> bool {
        self.models.contains_key(projectile_key(k))
    }
    /// Whether rats are drawn by the rat model (the character archetypes then skip them).
    pub fn draws_rats(&self) -> bool {
        self.models.contains_key("rat")
    }
}

fn projectile_key(k: ProjectileKind) -> &'static str {
    match k {
        ProjectileKind::Bullet => "bullet",
        ProjectileKind::Bolt => "bolt",
        ProjectileKind::SleepDart => "sleep",
        ProjectileKind::IncendiaryBolt => "flare",
        ProjectileKind::Grenade => "grenade",
        ProjectileKind::StickyGrenade => "sticky",
        // The Tallboy arrow (`Wpn_TallBoyBow.Wpn_TB_Arrow`) is not in the model set: the flare bolt.
        ProjectileKind::TallboyArrow => "flare",
    }
}

pub struct WorldModelsPlugin;

impl Plugin for WorldModelsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldModels>().add_systems(Startup, load).add_systems(Update, sync);
    }
}

fn load(
    mut wm: ResMut<WorldModels>,
    features: Option<Res<crate::map_render::RenderFeatures>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut binds: ResMut<Assets<SkinnedMeshInverseBindposes>>,
) {
    let feat = features.map(|f| f.clone()).unwrap_or_else(|| crate::map_render::RenderFeatures::from_flags(&|_| false));
    let Ok(content) = dishonored_assets::Content::locate() else { return };
    for &(key, pkg, path) in MODELS {
        // The rat is a pawn: stood on its cylinder bottom (`RAT_HALF_HEIGHT`) facing +X.
        let stance = (key == "rat").then_some(dishonored_sim::movement::RAT_HALF_HEIGHT);
        match dishonored_assets::characters::load_model(&content, path, &[(pkg, path)], stance, 512) {
            Ok(m) => {
                let gpu = upload(&m, &mut meshes, &mut images, &mut mats, &mut binds, &feat);
                wm.models.insert(key, gpu);
            }
            Err(e) => warn!("world model {path}: {e:#}"),
        }
    }
    for &(key, pkg, set, _) in ANIMS {
        let Some(bones) = wm.models.get(key).and_then(|g| g.parts.first()).map(|p| p.bones.clone()) else { continue };
        let lib = crate::viewmodel::ClipLib::load_from(&content, pkg, &bones, &[set.to_string()], &[]);
        wm.libs.insert(key, lib);
    }
}

/// Spawn a model in its bind pose under a new root (joints and sections as children); returns
/// the root and the first part's joints.
fn spawn_model(commands: &mut Commands, gpu: &CharacterGpu, xf: Transform) -> (Entity, Vec<Entity>) {
    let root = commands.spawn((xf, Visibility::Inherited)).id();
    let mut first = Vec::new();
    for (pi, part) in gpu.parts.iter().enumerate() {
        let joints: Vec<Entity> = part.bind.iter().map(|b| commands.spawn((Transform::from_matrix(*b), ChildOf(root))).id()).collect();
        for (mesh, mat) in &part.sections {
            commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: joints.clone() }, Transform::default(), bevy::camera::visibility::NoFrustumCulling, ChildOf(root)));
        }
        if pi == 0 {
            first = joints;
        }
    }
    (root, first)
}

/// `ArkAnimNodeRat` move blend (0x00913ED0): the first move whose speed exceeds the agent's
/// and the one below it (a single one at either end); weights by the speed between theirs;
/// the play rate is the dominant sequence's length over the weighted "length per speed"
/// (`(lo.speed lo.w lo.len + hi.speed hi.w hi.len) / speed`), at most `m_fMaxPlayRate`; one
/// shared phase (`m_fMovePct`) advances by rate dt / length and places both sequences.
/// Returns [(sequence, weight, time)] and the new phase.
pub fn rat_move(speed: f32, move_pct: f32, dt: f32, len: &dyn Fn(&str) -> f32) -> (Vec<(&'static str, f32, f32)>, f32) {
    let last = RAT_MOVES.len() - 1;
    let mut hi = last;
    if RAT_MOVES[last].1 > speed {
        while hi > 0 && RAT_MOVES[hi - 1].1 > speed {
            hi -= 1;
        }
    }
    let lo = if RAT_MOVES[last].1 <= speed { last } else { hi.saturating_sub(1) };
    let (ln, ll) = (RAT_MOVES[lo].0, len(RAT_MOVES[lo].0).max(1e-3));
    if lo == hi {
        let rate = (speed / RAT_MOVES[lo].1).min(RAT_MAX_RATE);
        let pct = (move_pct + rate * dt / ll).rem_euclid(1.0);
        return (vec![(ln, 1.0, pct * ll)], pct);
    }
    let (hn, hl) = (RAT_MOVES[hi].0, len(RAT_MOVES[hi].0).max(1e-3));
    let wh = (speed - RAT_MOVES[lo].1) / (RAT_MOVES[hi].1 - RAT_MOVES[lo].1);
    let wl = 1.0 - wh;
    let per = (RAT_MOVES[lo].1 * wl * ll + wh * RAT_MOVES[hi].1 * hl) / speed.max(1e-3);
    let dom = if wh > wl { hl } else { ll };
    let rate = (dom / per).min(RAT_MAX_RATE);
    let pct = (move_pct + rate * dt / dom).rem_euclid(1.0);
    (vec![(ln, wl, pct * ll), (hn, wh, pct * hl)], pct)
}

/// A Bevy transform at a Dishonored position whose +X (mesh forward) points along `dir`,
/// with `up` as the up hint.
fn oriented(at: glam::Vec3, dir: glam::Vec3, up: glam::Vec3) -> Transform {
    let x = if dir.length_squared() > 1e-6 { dir.normalize() } else { glam::Vec3::X };
    let up = if x.cross(up).length_squared() < 1e-6 { glam::Vec3::Y } else { up };
    let y = up.cross(x).normalize();
    let z = x.cross(y);
    let m = glam::Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), at.extend(1.0));
    Transform::from_matrix(coords::transform(m))
}

fn sync(mut commands: Commands, mut wm: ResMut<WorldModels>, view: Option<Res<SessionView>>, time: Res<Time>, mut q: Query<&mut Transform>) {
    let Some(view) = view else { return };
    let dt = time.delta_secs();
    // (key, transform, horizontal speed, biting) per wanted instance.
    let mut want: Vec<((u8, u32), &'static str, Transform, f32, bool)> = Vec::new();
    for p in &view.projectiles {
        let key = projectile_key(p.kind);
        // Grenades stuck to something keep their last facing (the velocity is zero).
        want.push(((0, p.id), key, oriented(p.position, p.velocity, glam::Vec3::Z), 0.0, false));
    }
    for t in &view.traps {
        // On its surface: up along the surface normal.
        let fwd = if t.normal.z.abs() > 0.9 { glam::Vec3::X } else { glam::Vec3::Z.cross(t.normal) };
        want.push(((1, t.id), "razor", oriented(t.position, fwd, t.normal), 0.0, false));
    }
    for r in view.others.iter().filter(|o| o.kind == PawnKind::Rat && o.alive) {
        let fwd = glam::Vec3::new(r.yaw.to_radians().cos(), r.yaw.to_radians().sin(), 0.0);
        let mut xf = oriented(r.position, fwd, glam::Vec3::Z);
        // The agent's random mesh scale.
        let h = r.id.wrapping_mul(2654435761) >> 8;
        xf.scale = Vec3::splat(RAT_SCALE.0 + (RAT_SCALE.1 - RAT_SCALE.0) * (h % 1000) as f32 / 999.0);
        let biting = r.last_input.pressed(dishonored_sim::input::buttons::PRIMARY);
        want.push(((2, r.id), "rat", xf, r.velocity.truncate().length(), biting));
    }
    let gone: Vec<(u8, u32)> = wm.live.keys().filter(|k| !want.iter().any(|w| w.0 == **k)).copied().collect();
    for k in gone {
        if let Some(l) = wm.live.remove(&k) {
            commands.entity(l.root).try_despawn();
        }
    }
    let wm = &mut *wm;
    for (k, key, xf, speed, biting) in want {
        if !wm.live.contains_key(&k) {
            let Some(gpu) = wm.models.get(key).cloned() else { continue };
            let (root, joints) = spawn_model(&mut commands, &gpu, xf);
            wm.live.insert(k, Live { root, joints, key, time: 0.0, move_pct: 0.0, specific: None, seed: k.1.wrapping_mul(2246822519) });
            continue;
        }
        let Some(l) = wm.live.get_mut(&k) else { continue };
        if let Ok(mut t) = q.get_mut(l.root) {
            *t = xf;
        }
        l.time += dt;
        let (Some(lib), Some(part)) = (wm.libs.get(l.key), wm.models.get(l.key).and_then(|g| g.parts.first())) else { continue };
        let mut pose: crate::viewmodel::Pose = part.bones.iter().map(|b| (b.rotation, b.translation)).collect();
        match ANIMS.iter().find(|a| a.0 == l.key).and_then(|a| a.3) {
            Some(seq) => lib.sample(seq, l.time, true, &mut pose),
            None => rat_pose(l, lib, speed, biting, dt, &mut pose),
        }
        let bones: Vec<dishonored_ir::BoneAsset> = part.bones.iter().zip(&pose).map(|(b, (r, t))| dishonored_ir::BoneAsset { rotation: *r, translation: *t, ..b.clone() }).collect();
        let g = dishonored_assets::characters::bind_globals(&bones);
        for (i, e) in l.joints.iter().enumerate() {
            if let (Ok(mut t), Some(m)) = (q.get_mut(*e), g.get(i)) {
                *t = Transform::from_matrix(crate::characters::to_bevy(part.mesh_to_pawn * *m));
            }
        }
    }
}

/// A rat's pose: an idle or the bite (`ArkAnimNodeRat` specific slots) or the move blend.
fn rat_pose(l: &mut Live, lib: &crate::viewmodel::ClipLib, speed: f32, biting: bool, dt: f32, pose: &mut [(Quat, Vec3)]) {
    let pick = |names: &[&'static str], n: u32| names[(n as usize) % names.len()];
    if biting && speed < RAT_MIN_SPEED {
        if !l.specific.is_some_and(|s| RAT_ATTACKS.contains(&s.0)) {
            l.specific = Some((pick(&RAT_ATTACKS, l.seed), l.time));
        }
    } else if speed < RAT_MIN_SPEED {
        // A random idle; another when it ends.
        let ended = l.specific.is_none_or(|(c, t0)| !RAT_IDLES.contains(&c) || l.time - t0 >= lib.length(c));
        if ended {
            l.seed = l.seed.wrapping_mul(1664525).wrapping_add(1013904223);
            l.specific = Some((pick(&RAT_IDLES, l.seed >> 16), l.time));
        }
    } else {
        l.specific = None;
    }
    if let Some((c, t0)) = l.specific {
        lib.sample(c, l.time - t0, RAT_ATTACKS.contains(&c), pose);
        return;
    }
    let (parts, pct) = rat_move(speed, l.move_pct, dt, &|n| lib.length(n));
    l.move_pct = pct;
    // Weighted blend of the move sequences (local space, like the node's children).
    let base = pose.to_vec();
    let mut acc: Option<crate::viewmodel::Pose> = None;
    for (seq, w, t) in parts {
        let mut p = base.clone();
        lib.sample(seq, t, true, &mut p);
        acc = Some(match acc {
            None => p,
            Some(a) => a.iter().zip(&p).map(|(x, y)| (x.0.slerp(y.0, w), x.1.lerp(y.1, w))).collect(),
        });
    }
    if let Some(a) = acc {
        pose.copy_from_slice(&a);
    }
}