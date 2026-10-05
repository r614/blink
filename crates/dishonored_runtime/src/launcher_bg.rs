//! The launcher's 3D background: the original main-menu level `Dishonored_MainMenu`
//! (`TheWorld` + `Dishonored_MainMenu_Env` / `_FX`), imported with the regular map importer on
//! a worker thread and shown from the menu camera.
//!
//! Camera: `CameraActor_11` (and five copies at the same pose: `_6`, `_20`.. `_23`), the pose
//! the `MainMenu` matinee (`SeqAct_Interp_1`) holds - location (53.6, -4835.0, 3036.1),
//! rotator pitch 4011 / yaw 12916 (22.0 deg / 71.0 deg), CameraActor default FOV 90.
//! APPROXIMATION: the matinee's camera motion (InterpTrackMove keys) is not decoded; the
//! camera is static.

use crate::map_render;
use bevy::prelude::*;
use dishonored_ir::DishonoredMap;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};

pub const MENU_MAP: &str = "Dishonored_MainMenu";
const CAM_POS: [f32; 3] = [53.61914, -4834.9624, 3036.0576];
const CAM_PITCH: f32 = 4011.0 * 360.0 / 65536.0;
const CAM_YAW: f32 = 12916.0 * 360.0 / 65536.0;

#[derive(Resource, Default)]
pub struct MenuBackground {
    rx: Option<Mutex<Receiver<Option<DishonoredMap>>>>,
    /// "loading", "level", "movie" (no level: the movie's own background art).
    pub state: &'static str,
}

pub fn start(mut commands: Commands, enabled: bool) {
    if !enabled {
        commands.insert_resource(MenuBackground { rx: None, state: "movie" });
        return;
    }
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("menu-level".into())
        .spawn(move || {
            let r = (|| -> anyhow::Result<DishonoredMap> {
                let content = dishonored_assets::Content::locate()?;
                let pkgs = dishonored_assets::map_packages(&content, MENU_MAP);
                let opts = dishonored_assets::ImportOptions { max_texture_size: 1024, other_actors: true, ..Default::default() };
                Ok(dishonored_assets::import_level(&content, MENU_MAP, &pkgs, &opts)?.0)
            })();
            if let Err(e) = &r {
                eprintln!("menu level unavailable: {e:#}");
            }
            let _ = tx.send(r.ok());
        })
        .ok();
    commands.insert_resource(MenuBackground { rx: Some(Mutex::new(rx)), state: "loading" });
}

#[allow(clippy::too_many_arguments)]
pub fn finish(
    mut commands: Commands,
    mut bg: ResMut<MenuBackground>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut baked: ResMut<Assets<map_render::BakedMaterial>>,
    features: Res<map_render::RenderFeatures>,
    cams: Query<Entity, With<MenuCamera>>,
) {
    let Some(rx) = &bg.rx else { return };
    let Ok(map) = rx.lock().unwrap().try_recv() else { return };
    bg.rx = None;
    let Some(map) = map else {
        bg.state = "movie";
        return;
    };
    let n = map_render::spawn_map(&map, &mut commands, &mut meshes, &mut images, &mut materials, &mut baked, false, &features);
    crate::lighting::spawn_lights(&map, &mut commands, &features);
    if let Some(cam) = cams.iter().next() {
        if features.materials {
            if let Some(env) = map_render::environment_light(&map, &mut images) {
                commands.entity(cam).insert(env);
            }
        }
        let fog = crate::atmosphere::setup_camera(&map, &mut commands.entity(cam), &features);
        commands.insert_resource(fog);
    }
    info!("menu level: {n} entities");
    bg.state = "level";
}

#[derive(Component)]
pub struct MenuCamera;

/// The menu camera (renders the level once loaded, and the menu UI).
pub fn spawn_camera(commands: &mut Commands) {
    let pos = crate::coords::pos(Vec3::from(CAM_POS));
    let fwd = crate::coords::dir(dishonored_sim::movement::view_vector(CAM_YAW, CAM_PITCH));
    // CameraActor FOVAngle 90 (horizontal in UE3) -> vertical for 16:9.
    let vfov = 2.0 * ((90f32.to_radians() * 0.5).tan() / (16.0 / 9.0)).atan();
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: vfov, near: 0.05, far: 12000.0, ..default() }),
        Transform::from_translation(pos).looking_to(fwd, Vec3::Y),
        Msaa::Sample4,
        MenuCamera,
    ));
}
