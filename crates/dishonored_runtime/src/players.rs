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
    /// Player being followed (None = local player). Cycle with N.
    pub follow: Option<PlayerId>,
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

/// Transform of a pawn's body. Skinned characters stay upright (their death sequences and
/// ragdoll lay them down); the capsule fallback is tipped over.
fn body_transform(p: &PlayerState, mv: &dishonored_ir::gameplay::MovementConfig, skinned: bool) -> Transform {
    let mut t = Transform::from_translation(coords::pos(p.position));
    if skinned {
        // Bodies lying down keep the standing frame: the model's feet are on the standing
        // cylinder's bottom whatever the collision height.
        let hh = half_height(p, mv);
        t.translation = coords::pos(p.position - glam::Vec3::Z * (hh - mv.collision_half_height));
        t.rotation = Quat::from_rotation_y(-p.yaw.to_radians());
    } else if !p.alive {
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
    mut bodies: Query<(Entity, &PlayerBody, &mut Transform, Option<&mut Mesh3d>, Option<&mut MeshMaterial3d<StandardMaterial>>, &mut Visibility, Option<&crate::characters::CharacterRoot>), Without<SwordVisual>>,
    lib: Option<Res<crate::characters::CharacterLibrary>>,
    weapons: Option<Res<crate::characters::WeaponLibrary>>,
    mut swords: Query<(Entity, &SwordVisual, &mut Transform, &mut Visibility), Without<PlayerBody>>,
    cams: Query<&GameCamera>,
    world_models: Option<Res<crate::world_models::WorldModels>>,
) {
    let mv = &session.content.sim.gameplay.movement;
    let third = cams.iter().any(|c| c.third_person);
    // Rats are drawn by their own model (`world_models`) once it is loaded.
    let rats_elsewhere = world_models.as_ref().is_some_and(|w| w.draws_rats());
    let mut all: Vec<&PlayerState> = view.others.iter().filter(|p| !(rats_elsewhere && p.kind == dishonored_sim::state::PawnKind::Rat)).collect();
    if let Some(me) = view.me.as_ref() {
        all.push(me);
    }
    let me_id = view.me.as_ref().map(|m| m.id);
    let mut seen: HashMap<PlayerId, bool> = HashMap::new();
    for (e, b, mut t, mut mesh, mut mat, mut vis, root) in &mut bodies {
        match all.iter().find(|p| p.id == b.0) {
            // The NPC's look changed (`DisSeqAct_NPCSetMaterials`) and the new model is loaded:
            // rebuild the body below.
            Some(p) if root.is_some_and(|r| r.archetype != p.archetype as usize) && lib.as_ref().is_some_and(|l| (p.archetype as usize) < l.models.len()) => {
                commands.entity(e).despawn();
            }
            Some(p) => {
                seen.insert(p.id, true);
                *t = { let carrier = p.carried_by.and_then(|c| all.iter().find(|x| x.id == c)).filter(|_| root.is_some()); body_transform(carrier.copied().unwrap_or(p), mv, root.is_some()) };
                if let Some(mesh) = mesh.as_mut() { mesh.0 = if p.crouched { assets.crouch_body.clone() } else { assets.body.clone() }; }
                if let Some(mat) = mat.as_mut() { mat.0 = if p.alive { assets.materials[p.id as usize % assets.materials.len()].clone() } else { assets.dead.clone() }; }
                let hide = Some(p.id) == me_id && !third && cams.iter().all(|c| c.follow.is_none_or(|f| f == p.id));
                // A teleporting NPC is gone while fully faded out (`teleport_spell`).
                *vis = if hide || p.mode == MoveMode::Blinking || dishonored_sim::teleport_spell::hidden(p) { Visibility::Hidden } else { Visibility::Inherited };
            }
            None => { commands.entity(e).despawn(); }
        }
    }
    for (e, s, mut t, mut vis) in &mut swords {
        match all.iter().find(|p| p.id == s.0) {
            Some(p) => {
                let (off, rot) = sword_offset(p);
                let base = body_transform(p, mv, false);
                let eye = coords::pos(eye_position(p, mv));
                let yaw_rot = Quat::from_rotation_y(-p.yaw.to_radians() - std::f32::consts::FRAC_PI_2);
                t.translation = eye + yaw_rot * off - Vec3::Y * 0.3;
                t.rotation = yaw_rot * rot;
                let _ = base;
                // The local first-person view draws the real sword on the arms rig instead.
                let own_fp = Some(p.id) == me_id && !third && cams.iter().all(|c| c.follow.is_none_or(|f| f == p.id));
                *vis = if p.alive && p.mode != MoveMode::Blinking && !own_fp { Visibility::Inherited } else { Visibility::Hidden };
            }
            None => { commands.entity(e).despawn(); }
        }
    }
    for p in all {
        if seen.contains_key(&p.id) {
            continue;
        }
        // A host-published look that is still loading: spawn the body once its model exists.
        if p.archetype as usize >= dishonored_multiplayer::mission::APPEARANCE_BASE as usize && lib.as_ref().is_some_and(|l| p.archetype as usize >= l.models.len()) {
            continue;
        }
        match lib.as_ref().filter(|l| !l.models.is_empty()) {
            Some(lib) => {
                // Dishonored character model (archetype by player id).
                let root = commands.spawn((body_transform(p, mv, true), Visibility::Inherited, PlayerBody(p.id))).id();
                crate::characters::spawn_character(&mut commands, root, lib, p.archetype as usize);
                if let Some(w) = weapons.as_ref() {
                    crate::characters::spawn_weapons(&mut commands, root, w, p.kind);
                }
            }
            None => {
                commands.spawn((
                    Mesh3d(assets.body.clone()),
                    MeshMaterial3d(assets.materials[p.id as usize % assets.materials.len()].clone()),
                    body_transform(p, mv, false),
                    PlayerBody(p.id),
                ));
                commands.spawn((Mesh3d(assets.sword.clone()), MeshMaterial3d(assets.steel.clone()), Transform::default(), SwordVisual(p.id)));
            }
        }
    }
}

pub fn sync_projectiles(mut commands: Commands, view: Res<SessionView>, assets: Res<PlayerAssets>, existing: Query<Entity, With<ProjectileVisual>>, world_models: Option<Res<crate::world_models::WorldModels>>) {
    for e in &existing {
        commands.entity(e).despawn();
    }
    // Kinds with their original model (`world_models`) are drawn there.
    // The trap's explosive arrow is invisible (`TrapExplosiveArrow_twk.m_bInvisible`): no tracer either.
    for p in view.projectiles.iter().filter(|p| p.kind != dishonored_sim::state::ProjectileKind::TrapArrow && !world_models.as_ref().is_some_and(|w| w.draws(p.kind))) {
        commands.spawn((Mesh3d(assets.projectile.clone()), MeshMaterial3d(assets.tracer.clone()), Transform::from_translation(coords::pos(p.position)), ProjectileVisual));
    }
}

fn session_is_spectating(view: &SessionView) -> bool {
    view.status.starts_with("spectating")
}

pub fn camera_follow(
    view: Res<SessionView>,
    session: Res<Session>,
    local: Res<LocalInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut q: Query<(&mut Transform, &mut GameCamera)>,
    vreq: Option<ResMut<crate::harness::ViewRequest>>,
) {
    let mut vreq = vreq;
    let req = vreq.as_mut().and_then(|v| v.pending.take());
    let req_orbit = vreq.as_ref().and_then(|v| v.orbit);
    let dist = vreq.as_ref().and_then(|v| v.distance).unwrap_or(3.0);
    let height = vreq.as_ref().and_then(|v| v.height).unwrap_or(0.6);
    let aim_at_body = vreq.as_ref().is_some_and(|v| v.distance.is_some());
    let mv = &session.content.sim.gameplay.movement;
    let Some(local_me) = view.me.as_ref() else { return };
    for (mut t, mut cam) in &mut q {
        if let Some(r) = req.as_ref() {
            cam.third_person = r.third_person;
            cam.follow = r.follow;
        }
        if keys.just_pressed(KeyCode::KeyV) {
            cam.third_person = !cam.third_person;
        }
        if keys.just_pressed(KeyCode::KeyN) {
            let mut ids: Vec<PlayerId> = view.others.iter().map(|p| p.id).chain(std::iter::once(local_me.id)).collect();
            ids.sort();
            let cur = cam.follow.unwrap_or(local_me.id);
            let i = ids.iter().position(|x| *x == cur).unwrap_or(0);
            cam.follow = Some(ids[(i + 1) % ids.len()]);
        }
        let me = cam.follow.and_then(|id| view.others.iter().find(|p| p.id == id)).unwrap_or(local_me);
        // Look comes from local input (immediate) for the local player, otherwise from
        // the followed player's replicated view.
        let (yaw, pitch) = if me.id == local_me.id && local.override_input.is_none() && !session_is_spectating(&view) { (local.yaw, local.pitch) } else { (me.yaw, me.pitch.min(10.0) - 10.0) };
        // At a keyhole the eye is the socket (StatePlayerMasterHolePeeking, interactables.md).
        let peek = crate::tripwires::peek_view(&view).filter(|_| me.id == local_me.id);
        let eye = peek.map(|v| coords::pos(v.eye)).unwrap_or_else(|| coords::pos(eye_position(me, mv)));
        let fwd = coords::dir(dishonored_sim::movement::view_vector(yaw, pitch));
        // Harness: DISH_TP_ORBIT=<deg> orbits the third-person camera around the followed pawn.
        let orbit = req_orbit.or_else(|| std::env::var("DISH_TP_ORBIT").ok().and_then(|s| s.parse::<f32>().ok())).unwrap_or(0.0);
        let ofwd = Quat::from_rotation_y(orbit.to_radians()) * fwd;
        let pos = if cam.third_person && peek.is_none() { eye - ofwd * dist + Vec3::Y * height } else { eye };
        let third = cam.third_person && peek.is_none();
        *t = Transform::from_translation(pos).looking_to(if third { ofwd } else { fwd }, Vec3::Y);
        if !third && peek.is_none() {
            // Lean tilt (`m_fCameraTiltPercent`): a right lean rolls the view clockwise.
            t.rotate_local_z(-dishonored_sim::movement::lean_roll(me, mv).to_radians());
        }
        if cam.third_person && aim_at_body {
            // Script view: frame the followed pawn's body.
            t.look_at(eye - Vec3::Y * 0.7, Vec3::Y);
        }
    }
}
