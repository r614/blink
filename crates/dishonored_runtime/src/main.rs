//! Standalone Dishonored runtime (presentation layer).
//!
//! `dishonored_runtime [--map L_Boyle_Ext_P] [--pos x,y,z] [--yaw deg] [--pitch deg]
//!                     [--show-hidden] [--max-tex 1024] [--screenshot out.png] [--frames N]`
//! Positions are in Dishonored world units (Z-up).

mod coords;
mod map_render;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use dishonored_assets::{Content, ImportOptions, import_level, map_packages};

#[derive(Resource, Clone)]
struct Args {
    map: String,
    pos: Option<Vec3>,
    yaw: Option<f32>,
    pitch: f32,
    show_hidden: bool,
    show_fx: bool,
    max_tex: u32,
    screenshot: Option<String>,
    frames: u32,
}

fn parse_args() -> Args {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let get = |f: &str| a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).cloned();
    Args {
        map: get("--map").unwrap_or_else(|| "L_Boyle_Ext_P".into()),
        pos: get("--pos").and_then(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
        }),
        yaw: get("--yaw").and_then(|s| s.parse().ok()),
        pitch: get("--pitch").and_then(|s| s.parse().ok()).unwrap_or(-10.0),
        show_hidden: a.iter().any(|x| x == "--show-hidden"),
        show_fx: a.iter().any(|x| x == "--show-fx"),
        max_tex: get("--max-tex").and_then(|s| s.parse().ok()).unwrap_or(1024),
        screenshot: get("--screenshot"),
        frames: get("--frames").and_then(|s| s.parse().ok()).unwrap_or(30),
    }
}

/// Free inspection camera. Yaw/pitch in degrees, Dishonored convention
/// (yaw around Z, 0 = +X).
#[derive(Component)]
struct FreeCam {
    yaw: f32,
    pitch: f32,
    speed_uu: f32,
}

#[derive(Component)]
struct Hud;

#[derive(Resource, Default)]
struct MapInfo {
    summary: String,
}

fn main() {
    let args = parse_args();
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: format!("dishonored-rs — {}", args.map),
                resolution: (1600u32, 900u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FrameTimeDiagnosticsPlugin::default())
        .insert_resource(args)
        .insert_resource(ClearColor(Color::srgb(0.08, 0.09, 0.12)))
        .insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.85, 0.88, 1.0),
            brightness: 900.0,
            affects_lightmapped_meshes: true,
        })
        .init_resource::<MapInfo>()
        .add_systems(Startup, setup)
        .add_systems(Update, (free_cam, cursor_toggle, hud, screenshot_driver))
        .run();
}

fn setup(
    mut commands: Commands,
    args: Res<Args>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut info: ResMut<MapInfo>,
) {
    let content = Content::locate().expect("Dishonored install not found (set DISHONORED_DIR)");
    let pkgs = map_packages(&content, &args.map);
    let t = std::time::Instant::now();
    let opts = ImportOptions {
        max_texture_size: args.max_tex,
        ..Default::default()
    };
    let (map, stats) = import_level(&content, &args.map, &pkgs, &opts).expect("map import failed");
    let import_time = t.elapsed();
    let spawned = map_render::spawn_map(
        &map,
        &mut commands,
        &mut meshes,
        &mut images,
        &mut materials,
        args.show_hidden,
        args.show_fx,
    );
    info.summary = format!(
        "{}  |  {} packages  {} instances  {} meshes  {} textures  {} entities  import {:.1?}",
        args.map,
        pkgs.len(),
        stats.instances,
        stats.meshes,
        stats.textures,
        spawned,
        import_time
    );
    info!("{}", info.summary);

    let spawn = map.spawn_candidates.first();
    let start = args
        .pos
        .or(spawn.map(|s| s.position + Vec3::new(0.0, 0.0, 64.0)))
        .unwrap_or(Vec3::new(0.0, 0.0, 500.0));
    let yaw = args.yaw.or(spawn.map(|s| s.yaw_deg)).unwrap_or(0.0);

    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 75f32.to_radians(),
            near: 0.05,
            far: 6000.0,
            ..default()
        }),
        Transform::from_translation(coords::pos(start)),
        FreeCam {
            yaw,
            pitch: args.pitch,
            speed_uu: 800.0,
        },
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 6000.0,
            shadow_maps_enabled: false,
            color: Color::srgb(0.75, 0.8, 1.0),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 0.0).looking_to(Vec3::new(-0.4, -1.0, -0.3), Vec3::Y),
    ));
    commands.spawn((
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        Node {
            position_type: PositionType::Absolute,
            top: px(8),
            left: px(8),
            ..default()
        },
        Hud,
    ));
}

/// Dishonored yaw/pitch (degrees, Z-up) -> Bevy forward vector.
fn forward_dir(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    coords::dir(Vec3::new(cp * cy, cp * sy, sp))
}

fn free_cam(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut q: Query<(&mut Transform, &mut FreeCam)>,
) {
    let locked = cursor.iter().any(|c| c.grab_mode != CursorGrabMode::None);
    for (mut tf, mut cam) in &mut q {
        if locked || mouse.pressed(MouseButton::Right) {
            // Unreal yaw increases clockwise seen from above; mouse right turns right.
            cam.yaw += motion.delta.x * 0.12;
            cam.pitch = (cam.pitch - motion.delta.y * 0.12).clamp(-89.0, 89.0);
        }
        let fwd = forward_dir(cam.yaw, cam.pitch);
        let right_uu = Vec3::new(-cam.yaw.to_radians().sin(), cam.yaw.to_radians().cos(), 0.0);
        let right = coords::dir(right_uu);
        let mut v = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) { v += fwd; }
        if keys.pressed(KeyCode::KeyS) { v -= fwd; }
        if keys.pressed(KeyCode::KeyD) { v += right; }
        if keys.pressed(KeyCode::KeyA) { v -= right; }
        if keys.pressed(KeyCode::KeyE) || keys.pressed(KeyCode::Space) { v += Vec3::Y; }
        if keys.pressed(KeyCode::KeyQ) || keys.pressed(KeyCode::ControlLeft) { v -= Vec3::Y; }
        let mut speed = cam.speed_uu;
        if keys.pressed(KeyCode::ShiftLeft) { speed *= 5.0; }
        if keys.pressed(KeyCode::AltLeft) { speed *= 0.2; }
        tf.translation += v.normalize_or_zero() * speed * coords::UU_TO_BEVY * time.delta_secs();
        let target = tf.translation + fwd;
        tf.look_at(target, Vec3::Y);
    }
}

fn cursor_toggle(keys: Res<ButtonInput<KeyCode>>, mut q: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    if keys.just_pressed(KeyCode::Tab) || keys.just_pressed(KeyCode::Escape) {
        for mut c in &mut q {
            let lock = keys.just_pressed(KeyCode::Tab) && c.grab_mode == CursorGrabMode::None;
            c.grab_mode = if lock { CursorGrabMode::Locked } else { CursorGrabMode::None };
            c.visible = !lock;
        }
    }
}

fn hud(
    diag: Res<DiagnosticsStore>,
    info: Res<MapInfo>,
    cam: Query<(&Transform, &FreeCam)>,
    mut text: Query<&mut Text, With<Hud>>,
) {
    let fps = diag
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);
    let Ok((tf, c)) = cam.single() else { return };
    let p = tf.translation / coords::UU_TO_BEVY;
    let uu = Vec3::new(p.x, p.z, p.y);
    for mut t in &mut text {
        t.0 = format!(
            "{}\nfps {fps:.0}   pos (uu) {:.0}, {:.0}, {:.0}   yaw {:.1}  pitch {:.1}\nRMB/Tab look  WASD move  Q/E down/up  Shift fast  Alt slow",
            info.summary, uu.x, uu.y, uu.z, c.yaw, c.pitch
        );
    }
}

fn screenshot_driver(
    mut commands: Commands,
    args: Res<Args>,
    mut frame: Local<u32>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(path) = &args.screenshot else { return };
    *frame += 1;
    if *frame == args.frames {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path.clone()));
    }
    if *frame == args.frames + 240 {
        exit.write(AppExit::Success);
    }
}
