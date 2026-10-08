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
//! Each model is drawn in its bind pose at the object's transform: projectiles along their
//! velocity, razors on their surface, rats at the pawn facing its yaw. Not yet played: the
//! razor's and the rats' animations (`SpringRazor_as`, the rat anim sets).

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

#[derive(Resource, Default)]
pub struct WorldModels {
    models: HashMap<&'static str, CharacterGpu>,
    /// Live instances: (kind 0 projectile / 1 trap / 2 rat, id) -> root entity.
    live: HashMap<(u8, u32), Entity>,
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
}

/// Spawn a model in its bind pose under a new root (joints and sections as children).
fn spawn_model(commands: &mut Commands, gpu: &CharacterGpu, xf: Transform) -> Entity {
    let root = commands.spawn((xf, Visibility::Inherited)).id();
    for part in &gpu.parts {
        let joints: Vec<Entity> = part.bind.iter().map(|b| commands.spawn((Transform::from_matrix(*b), ChildOf(root))).id()).collect();
        for (mesh, mat) in &part.sections {
            commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), SkinnedMesh { inverse_bindposes: part.inverse_bindposes.clone(), joints: joints.clone() }, Transform::default(), bevy::camera::visibility::NoFrustumCulling, ChildOf(root)));
        }
    }
    root
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

fn sync(mut commands: Commands, mut wm: ResMut<WorldModels>, view: Option<Res<SessionView>>, mut q: Query<&mut Transform>) {
    let Some(view) = view else { return };
    let mut want: Vec<((u8, u32), &'static str, Transform)> = Vec::new();
    for p in &view.projectiles {
        let key = projectile_key(p.kind);
        // Grenades stuck to something keep their last facing (the velocity is zero).
        want.push(((0, p.id), key, oriented(p.position, p.velocity, glam::Vec3::Z)));
    }
    for t in &view.traps {
        // On its surface: up along the surface normal.
        let fwd = if t.normal.z.abs() > 0.9 { glam::Vec3::X } else { glam::Vec3::Z.cross(t.normal) };
        want.push(((1, t.id), "razor", oriented(t.position, fwd, t.normal)));
    }
    for r in view.others.iter().filter(|o| o.kind == PawnKind::Rat && o.alive) {
        let fwd = glam::Vec3::new(r.yaw.to_radians().cos(), r.yaw.to_radians().sin(), 0.0);
        want.push(((2, r.id), "rat", oriented(r.position, fwd, glam::Vec3::Z)));
    }
    let gone: Vec<(u8, u32)> = wm.live.keys().filter(|k| !want.iter().any(|w| w.0 == **k)).copied().collect();
    for k in gone {
        if let Some(e) = wm.live.remove(&k) {
            commands.entity(e).try_despawn();
        }
    }
    for (k, key, xf) in want {
        if let Some(e) = wm.live.get(&k) {
            if let Ok(mut t) = q.get_mut(*e) {
                *t = xf;
            }
            continue;
        }
        let Some(gpu) = wm.models.get(key).cloned() else { continue };
        let e = spawn_model(&mut commands, &gpu, xf);
        wm.live.insert(k, e);
    }
}
