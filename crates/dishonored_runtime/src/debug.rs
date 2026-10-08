//! Collision debugger (F3): nearby collision triangles and normals, character capsule, Blink
//! target query, sword sweep, Dark Vision reveal capsules (drawn through walls).

use crate::coords;
use crate::session::{Session, SessionView};
use bevy::prelude::*;
use dishonored_sim::movement::{eye_position, half_height, view_vector};
use dishonored_sim::state::{PlayerState, SwordPhase};
use dishonored_world::geom::Aabb;

#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct XRayGizmos;

#[derive(Resource, Default)]
pub struct DebugToggles {
    pub collision: bool,
}

pub fn setup_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (cfg, _) = store.config_mut::<XRayGizmos>();
    cfg.depth_bias = -1.0;
    cfg.line.width = 3.0;
}

fn capsule_lines<C: GizmoConfigGroup>(g: &mut Gizmos<C>, p: &PlayerState, mv: &dishonored_ir::gameplay::MovementConfig, color: Color) {
    let hh = half_height(p, mv);
    let r = mv.collision_radius;
    let c = p.position;
    for k in 0..8 {
        let a = k as f32 / 8.0 * std::f32::consts::TAU;
        let o = glam::Vec3::new(a.cos() * r, a.sin() * r, 0.0);
        let lo = c + o - glam::Vec3::Z * (hh - r);
        let hi = c + o + glam::Vec3::Z * (hh - r);
        g.line(coords::pos(lo), coords::pos(hi), color);
    }
    g.sphere(Isometry3d::from_translation(coords::pos(c + glam::Vec3::Z * (hh - r))), r * coords::UU_TO_BEVY, color);
    g.sphere(Isometry3d::from_translation(coords::pos(c - glam::Vec3::Z * (hh - r))), r * coords::UU_TO_BEVY, color);
}

pub fn draw_debug(
    keys: Res<ButtonInput<KeyCode>>,
    mut toggles: ResMut<DebugToggles>,
    view: Res<SessionView>,
    session: Res<Session>,
    mut gizmos: Gizmos,
    mut xray: Gizmos<XRayGizmos>,
) {
    if keys.just_pressed(KeyCode::F3) {
        toggles.collision = !toggles.collision;
    }
    let mv = &session.content.sim.gameplay.movement;
    let Some(me) = view.me.as_ref() else { return };

    // Debug drawing only (F3): the original's Blink indicator is a particle system
    // (`effects::blink_indicator`) and Dark Vision draws silhouettes (`dark_vision.rs`).
    if !toggles.collision {
        return;
    }
    // Blink target query.
    if let Some(t) = me.left.blink_target {
        let feet = t - glam::Vec3::Z * mv.collision_half_height;
        xray.sphere(Isometry3d::from_translation(coords::pos(feet)), 0.4, Color::srgb(0.4, 0.8, 1.0));
        xray.line(coords::pos(eye_position(me, mv)), coords::pos(feet), Color::srgba(0.4, 0.8, 1.0, 0.4));
    }
    // Dark Vision: the revealed pawns' capsules.
    for id in &view.dark_vision_reveals {
        if let Some(o) = view.others.iter().find(|o| o.id == *id) {
            capsule_lines(&mut xray, o, mv, Color::srgb(1.0, 0.85, 0.3));
        }
    }
    capsule_lines(&mut gizmos, me, mv, Color::srgb(0.2, 1.0, 0.2));
    // Sword sweep ray during the hit window.
    for p in view.others.iter().chain(std::iter::once(me)) {
        if p.sword.phase == SwordPhase::Active {
            let eye = eye_position(p, mv);
            let end = eye + view_vector(p.yaw, p.pitch) * session.content.sim.tuning.attack_range;
            gizmos.line(coords::pos(eye), coords::pos(end), Color::srgb(1.0, 0.2, 0.2));
        }
    }
    // Collision triangles + normals within 600 uu.
    let world = &session.content.collision;
    let q = Aabb { min: me.position - glam::Vec3::splat(600.0), max: me.position + glam::Vec3::splat(600.0) };
    let mut n = 0;
    world.bvh.query_aabb(&q, |i| {
        if n > 4000 {
            return;
        }
        n += 1;
        let t = &world.tris[i as usize];
        let col = if t.n.z >= mv.walkable_floor_z { Color::srgba(0.2, 0.9, 0.3, 0.6) } else { Color::srgba(1.0, 0.5, 0.1, 0.6) };
        let (a, b, c) = (coords::pos(t.a), coords::pos(t.b), coords::pos(t.c));
        gizmos.line(a, b, col);
        gizmos.line(b, c, col);
        gizmos.line(c, a, col);
        let center = (t.a + t.b + t.c) / 3.0;
        gizmos.line(coords::pos(center), coords::pos(center + t.n * 12.0), Color::srgb(0.3, 0.5, 1.0));
    });
    // Ground probe ray.
    let feet = me.position - glam::Vec3::Z * half_height(me, mv);
    gizmos.line(coords::pos(feet), coords::pos(feet - glam::Vec3::Z * 60.0), Color::WHITE);
}
