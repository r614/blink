//! Presentation of players, projectiles and the first-person camera. Animation follows
//! simulation state; nothing here feeds back into gameplay.

use crate::coords;
use crate::input_map::LocalInput;
use crate::session::{Session, SessionView};
use bevy::prelude::*;
use dishonored_sim::movement::{eye_position, half_height};
use dishonored_sim::state::{MoveMode, PlayerId, PlayerState, SwordPhase};
use std::collections::HashMap;

#[derive(Component)]
pub struct PlayerBody(pub PlayerId);

#[derive(Component)]
pub struct SwordVisual(pub PlayerId);

#[derive(Component)]
pub struct ProjectileVisual;

#[derive(Component)]
pub struct GameCamera {
    pub third_person: bool,
}

#[derive(Resource)]
pub struct PlayerAssets {
    pub body: Handle<Mesh>,
    pub crouch_body: Handle<Mesh>,
    pub sword: Handle<Mesh>,
    pub projectile: Handle<Mesh>,
    pub materials: Vec<Handle<StandardMaterial>>,
    pub dead: Handle<StandardMaterial>,
    pub steel: Handle<StandardMaterial>,
    pub tracer: Handle<StandardMaterial>,
}

pub fn setup_player_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<StandardMaterial>>, session: Res<Session>) {
    let mv = &session.content.sim.gameplay.movement;
    let r = mv.collision_radius * coords::UU_TO_BEVY;
    let body_len = (mv.collision_half_height * 2.0 - mv.collision_radius * 2.0) * coords::UU_TO_BEVY;
    let crouch_len = (mv.crouch_half_height * 2.0 - mv.collision_radius * 2.0).max(1.0) * coords::UU_TO_BEVY;
    let palette = [
        Color::srgb(0.75, 0.2, 0.2),
        Color::srgb(0.2, 0.45, 0.8),
        Color::srgb(0.25, 0.65, 0.3),
        Color::srgb(0.8, 0.65, 0.2),
        Color::srgb(0.6, 0.3, 0.7),
        Color::srgb(0.2, 0.7, 0.7),
        Color::srgb(0.85, 0.45, 0.15),
        Color::srgb(0.6, 0.6, 0.6),
    ];
    commands.insert_resource(PlayerAssets {
        body: meshes.add(Capsule3d::new(r, body_len)),
        crouch_body: meshes.add(Capsule3d::new(r, crouch_len)),
        sword: meshes.add(Cuboid::new(0.03, 0.03, 1.6)),
        projectile: meshes.add(Sphere::new(0.06)),
        materials: palette.iter().map(|c| mats.add(StandardMaterial { base_color: *c, perceptual_roughness: 0.7, ..default() })).collect(),
        dead: mats.add(StandardMaterial { base_color: Color::srgb(0.15, 0.15, 0.15), ..default() }),
        steel: mats.add(StandardMaterial { base_color: Color::srgb(0.8, 0.82, 0.85), metallic: 0.9, perceptual_roughness: 0.3, ..default() }),
        tracer: mats.add(StandardMaterial { base_color: Color::srgb(1.0, 0.85, 0.4), emissive: LinearRgba::rgb(8.0, 6.0, 2.0), unlit: true, ..default() }),
    });
}

fn body_transform(p: &PlayerState, mv: &dishonored_ir::gameplay::MovementConfig) -> Transform {
    let mut t = Transform::from_translation(coords::pos(p.position));
    if !p.alive {
        // Lying on the ground.
        let hh = half_height(p, mv);
        t.translation = coords::pos(p.position - glam::Vec3::Z * (hh - mv.collision_radius));
        t.rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_y(-p.yaw.to_radians());
    } else {
        t.rotation = Quat::from_rotation_y(-p.yaw.to_radians());
    }
    t
}

/// Sword pose from the authoritative swing phase (simulation owns timing).
fn sword_offset(p: &PlayerState) -> (Vec3, Quat) {
    let swing = match p.sword.phase {
        SwordPhase::Startup => -0.6,
        SwordPhase::Active => 1.2,
        SwordPhase::Recovery => 0.6,
        SwordPhase::Blocking => 0.0,
        SwordPhase::Assassinating => 1.4,
        SwordPhase::Staggered => -1.0,
        SwordPhase::Idle => 0.2,
    };
    let block = p.sword.phase == SwordPhase::Blocking;
    let local = if block { Vec3::new(0.0, 0.4, -0.5) } else { Vec3::new(0.45, 0.2, -0.4) };
    let rot = if block { Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) } else { Quat::from_rotation_y(-swing) * Quat::from_rotation_x(-0.5) };
    (local, rot)
}

pub fn sync_players(
    mut commands: Commands,
    view: Res<SessionView>,
    session: Res<Session>,
    assets: Res<PlayerAssets>,
    mut bodies: Query<(Entity, &PlayerBody, &mut Transform, &mut Mesh3d, &mut MeshMaterial3d<StandardMaterial>, &mut Visibility), Without<SwordVisual>>,
    mut swords: Query<(Entity, &SwordVisual, &mut Transform, &mut Visibility), Without<PlayerBody>>,
    cams: Query<&GameCamera>,
) {
    let mv = &session.content.sim.gameplay.movement;
    let third = cams.iter().any(|c| c.third_person);
    let mut all: Vec<&PlayerState> = view.others.iter().collect();
    if let Some(me) = view.me.as_ref() {
        all.push(me);
    }
    let me_id = view.me.as_ref().map(|m| m.id);
    let mut seen: HashMap<PlayerId, bool> = HashMap::new();
    for (e, b, mut t, mut mesh, mut mat, mut vis) in &mut bodies {
        match all.iter().find(|p| p.id == b.0) {
            Some(p) => {
                seen.insert(p.id, true);
                *t = body_transform(p, mv);
                mesh.0 = if p.crouched { assets.crouch_body.clone() } else { assets.body.clone() };
                mat.0 = if p.alive { assets.materials[p.id as usize % assets.materials.len()].clone() } else { assets.dead.clone() };
                let hide = Some(p.id) == me_id && !third;
                *vis = if hide || p.mode == MoveMode::Blinking { Visibility::Hidden } else { Visibility::Inherited };
            }
            None => commands.entity(e).despawn(),
        }
    }
    for (e, s, mut t, mut vis) in &mut swords {
        match all.iter().find(|p| p.id == s.0) {
            Some(p) => {
                let (off, rot) = sword_offset(p);
                let base = body_transform(p, mv);
                let eye = coords::pos(eye_position(p, mv));
                let yaw_rot = Quat::from_rotation_y(-p.yaw.to_radians() - std::f32::consts::FRAC_PI_2);
                t.translation = eye + yaw_rot * off - Vec3::Y * 0.3;
                t.rotation = yaw_rot * rot;
                let _ = base;
                *vis = if p.alive && p.mode != MoveMode::Blinking { Visibility::Inherited } else { Visibility::Hidden };
            }
            None => commands.entity(e).despawn(),
        }
    }
    for p in all {
        if seen.contains_key(&p.id) {
            continue;
        }
        commands.spawn((
            Mesh3d(assets.body.clone()),
            MeshMaterial3d(assets.materials[p.id as usize % assets.materials.len()].clone()),
            body_transform(p, mv),
            PlayerBody(p.id),
        ));
        commands.spawn((Mesh3d(assets.sword.clone()), MeshMaterial3d(assets.steel.clone()), Transform::default(), SwordVisual(p.id)));
    }
}

pub fn sync_projectiles(mut commands: Commands, view: Res<SessionView>, assets: Res<PlayerAssets>, existing: Query<Entity, With<ProjectileVisual>>) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    for p in &view.projectiles {
        commands.spawn((Mesh3d(assets.projectile.clone()), MeshMaterial3d(assets.tracer.clone()), Transform::from_translation(coords::pos(p.position)), ProjectileVisual));
    }
}

pub fn camera_follow(
    view: Res<SessionView>,
    session: Res<Session>,
    local: Res<LocalInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut q: Query<(&mut Transform, &mut GameCamera)>,
) {
    let mv = &session.content.sim.gameplay.movement;
    let Some(me) = view.me.as_ref() else { return };
    for (mut t, mut cam) in &mut q {
        if keys.just_pressed(KeyCode::KeyV) {
            cam.third_person = !cam.third_person;
        }
        // Look comes from local input (immediate), position from the simulation.
        let yaw = local.yaw;
        let pitch = local.pitch;
        let eye = coords::pos(eye_position(me, mv));
        let fwd = coords::dir(dishonored_sim::movement::view_vector(yaw, pitch));
        let pos = if cam.third_person { eye - fwd * 3.0 + Vec3::Y * 0.6 } else { eye };
        *t = Transform::from_translation(pos).looking_to(fwd, Vec3::Y);
    }
}
