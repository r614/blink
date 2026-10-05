//! Standalone Dishonored runtime (presentation + local input). Gameplay is
//! `dishonored_sim`, run locally (offline) or predicted against a dedicated server.
//!
//! ```text
//! dishonored_runtime [--map L_Boyle_Ext_P]
//!     (default)              offline FFA vs --bots N (default 3); --spectate to watch bots
//!     --connect HOST:PORT    join a dedicated server
//!     --browse MASTER        list servers from a control plane and join the best one
//!     --inspect              free camera, no gameplay (--pos x,y,z --yaw --pitch)
//!   options: --name NAME --show-hidden --show-fx --max-tex N --lat MS --loss P
//!   harness: --script FILE --out DIR --control PORT --record DIR --record-every N
//!            --record-fps F --exit-after SECONDS --size WxH
//! ```

mod coords;
mod characters;
mod debug;
mod harness;
mod hud;
mod input_map;
mod map_render;
mod players;
mod session;
mod viewmodel;

use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use dishonored_assets::{Content, ImportOptions, import_level, map_packages};
use dishonored_net::client::NetClient;
use dishonored_net::socket::{LinkConditions, NetSocket};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Resource, Clone)]
struct Args {
    map: String,
    mode: Mode,
    name: String,
    bots: usize,
    spectate: bool,
    contracts: bool,
    pos: Option<Vec3>,
    yaw: Option<f32>,
    pitch: f32,
    show_hidden: bool,
    show_fx: bool,
    max_tex: u32,
    exit_after: Option<f32>,
    record: Option<PathBuf>,
    record_every: u32,
    record_fps: u32,
    script: Option<PathBuf>,
    out: PathBuf,
    control: Option<u16>,
    lat: f32,
    loss: f32,
    size: (u32, u32),
}

#[derive(Clone, PartialEq)]
enum Mode {
    Offline,
    Connect(String),
    Browse(String),
    Inspect,
}

fn parse_args() -> Args {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let get = |f: &str| a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).cloned();
    let has = |f: &str| a.iter().any(|x| x == f);
    let mode = if has("--inspect") {
        Mode::Inspect
    } else if let Some(c) = get("--connect") {
        Mode::Connect(c)
    } else if let Some(m) = get("--browse") {
        Mode::Browse(m)
    } else {
        Mode::Offline
    };
    let size = get("--size")
        .and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))))
        .unwrap_or((1600, 900));
    Args {
        map: get("--map").unwrap_or_else(|| dishonored_content::DEFAULT_MAP.into()),
        mode,
        name: get("--name").unwrap_or_else(|| "player".into()),
        bots: get("--bots").and_then(|s| s.parse().ok()).unwrap_or(3),
        spectate: has("--spectate"),
        contracts: get("--mode").as_deref() == Some("contracts"),
        pos: get("--pos").and_then(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2]))
        }),
        yaw: get("--yaw").and_then(|s| s.parse().ok()),
        pitch: get("--pitch").and_then(|s| s.parse().ok()).unwrap_or(-10.0),
        show_hidden: has("--show-hidden"),
        show_fx: has("--show-fx"),
        max_tex: get("--max-tex").and_then(|s| s.parse().ok()).unwrap_or(1024),
        exit_after: get("--exit-after").and_then(|s| s.parse().ok()),
        record: get("--record").map(PathBuf::from),
        record_every: get("--record-every").and_then(|s| s.parse().ok()).unwrap_or(2),
        record_fps: get("--record-fps").and_then(|s| s.parse().ok()).unwrap_or(30),
        script: get("--script").map(PathBuf::from),
        out: get("--out").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("harness_out")),
        control: get("--control").and_then(|s| s.parse().ok()),
        lat: get("--lat").and_then(|s| s.parse().ok()).unwrap_or(0.0),
        loss: get("--loss").and_then(|s| s.parse().ok()).unwrap_or(0.0),
        size,
    }
}

#[derive(Component)]
struct FreeCam {
    yaw: f32,
    pitch: f32,
}

fn main() {
    let args = parse_args();
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window { title: format!("dishonored-rs - {}", args.map), resolution: args.size.into(), ..default() }),
        ..default()
    }))
    .add_plugins(FrameTimeDiagnosticsPlugin::default())
    .insert_resource(ClearColor(Color::srgb(0.08, 0.09, 0.12)))
    .insert_resource(GlobalAmbientLight { color: Color::srgb(0.85, 0.88, 1.0), brightness: 900.0, affects_lightmapped_meshes: true })
    .insert_resource(args.clone())
    .add_systems(Startup, setup_world);

    if args.mode == Mode::Inspect {
        app.add_systems(Update, inspect_cam);
    } else {
        let content = Arc::new(dishonored_content::load_match(&args.map).expect("loading match content from the install"));
        let session = match &args.mode {
            Mode::Offline => session::Session::offline(content, args.bots, args.spectate, args.contracts),
            Mode::Connect(addr) => session::Session::online(content, connect(addr, &args)),
            Mode::Browse(master) => {
                let servers = dishonored_net::control_plane::list_servers(master).expect("listing servers");
                let best = servers
                    .iter()
                    .filter_map(|s| {
                        let addr = s.addr.parse().ok()?;
                        dishonored_net::client::query_info(addr, std::time::Duration::from_millis(500)).map(|(_, rtt)| (s.addr.clone(), rtt))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .expect("no reachable servers");
                info!("joining {} ({:.0} ms)", best.0, best.1);
                session::Session::online(content, connect(&best.0, &args))
            }
            Mode::Inspect => unreachable!(),
        };
        let mut runner = harness::ScriptRunner { out_dir: args.out.clone(), ..default() };
        if let Some(p) = &args.script {
            let s: harness::Script = serde_json::from_str(&std::fs::read_to_string(p).expect("script")).expect("script json");
            runner.steps = s.steps;
            std::fs::create_dir_all(&args.out).ok();
        }
        let recorder = harness::Recorder { dir: args.record.clone(), every: args.record_every, ..default() };
        if let Some(d) = &args.record {
            std::fs::create_dir_all(d).ok();
        }
        if let Some(port) = args.control {
            app.insert_resource(harness::start_control_server(port));
        }
        app.insert_resource(session)
            .insert_resource(session::SessionView::default())
            .insert_resource(input_map::LocalInput { enabled: true, ..default() })
            .insert_resource(Time::<Fixed>::from_hz(dishonored_sim::movement::TICK_HZ as f64))
            .insert_resource(runner)
            .insert_resource(recorder)
            .init_resource::<harness::ShotQueue>()
            .init_resource::<harness::ControlOverride>()
            .init_resource::<debug::DebugToggles>()
            .init_gizmo_group::<debug::XRayGizmos>()
            .add_systems(Startup, (players::setup_player_assets, hud::setup_hud, debug::setup_gizmos, (characters::load_library, characters::load_animations).chain(), characters::load_weapons, viewmodel::load_view_rig))
            .init_resource::<viewmodel::ViewAnim>()
            .add_systems(FixedUpdate, (harness::script_fixed, session::fixed_tick).chain())
            .add_systems(
                Update,
                (
                    input_map::gather_input,
                    input_map::cursor_lock,
                    players::sync_players,
                    characters::animate_characters,
                    viewmodel::spawn_view_rig,
                    viewmodel::apply_fov,
                    viewmodel::animate_view_rig.after(players::camera_follow),
                    players::sync_projectiles,
                    players::camera_follow,
                    hud::update_hud,
                    debug::draw_debug,
                    harness::harness_frame,
                ),
            );
    }
    app.add_systems(Update, exit_after);
    let exit = app.run();
    if let Some(dir) = &args.record {
        match harness::encode_video(dir, args.record_fps) {
            Some(p) => println!("video written to {}", p.display()),
            None => println!("frames left in {} (ffmpeg not available or failed)", dir.display()),
        }
    }
    if let AppExit::Error(c) = exit {
        std::process::exit(c.get() as i32);
    }
}

fn connect(addr: &str, args: &Args) -> NetClient {
    let server = std::net::ToSocketAddrs::to_socket_addrs(addr).ok().and_then(|mut i| i.next()).expect("server address");
    let mut sock = NetSocket::bind("0.0.0.0:0").expect("udp socket");
    if args.lat > 0.0 || args.loss > 0.0 {
        sock = sock.with_conditions(LinkConditions { latency_ms: args.lat, jitter_ms: args.lat * 0.1, loss: args.loss }, 7);
    }
    NetClient::new(sock, server, &args.name)
}

fn setup_world(mut commands: Commands, args: Res<Args>, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let content = Content::locate().expect("Dishonored install not found (set DISHONORED_DIR)");
    let pkgs = map_packages(&content, &args.map);
    let opts = ImportOptions { max_texture_size: args.max_tex, ..Default::default() };
    let (map, stats) = import_level(&content, &args.map, &pkgs, &opts).expect("map import failed");
    let spawned = map_render::spawn_map(&map, &mut commands, &mut meshes, &mut images, &mut materials, args.show_hidden, args.show_fx);
    info!("{}: {} instances, {} meshes, {} textures, {} entities", args.map, stats.instances, stats.meshes, stats.textures, spawned);

    let spawn = map.spawn_candidates.first();
    let start = args.pos.or(spawn.map(|s| s.position + glam::Vec3::Z * 64.0)).unwrap_or(glam::Vec3::new(0.0, 0.0, 500.0));
    let yaw = args.yaw.or(spawn.map(|s| s.yaw_deg)).unwrap_or(0.0);
    let projection = Projection::Perspective(PerspectiveProjection { fov: 75f32.to_radians(), near: 0.05, far: 6000.0, ..default() });
    if args.mode == Mode::Inspect {
        commands.spawn((Camera3d::default(), projection, Transform::from_translation(coords::pos(start)), FreeCam { yaw, pitch: args.pitch }));
    } else {
        commands.spawn((Camera3d::default(), projection, Transform::default(), players::GameCamera { third_person: args.spectate, follow: None }));
    }
    commands.spawn((
        DirectionalLight { illuminance: 6000.0, shadow_maps_enabled: false, color: Color::srgb(0.75, 0.8, 1.0), ..default() },
        bevy::camera::visibility::RenderLayers::from_layers(&[0, viewmodel::VIEW_LAYER]),
        Transform::default().looking_to(Vec3::new(-0.4, -1.0, -0.3), Vec3::Y),
    ));
}

fn exit_after(time: Res<Time>, args: Res<Args>, mut exit: MessageWriter<AppExit>) {
    if args.exit_after.is_some_and(|s| time.elapsed_secs() >= s) {
        exit.write(AppExit::Success);
    }
}

fn inspect_cam(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut q: Query<(&mut Transform, &mut FreeCam)>,
) {
    let mut locked = false;
    for mut c in &mut cursor {
        if keys.just_pressed(KeyCode::Tab) {
            let lock = c.grab_mode == CursorGrabMode::None;
            c.grab_mode = if lock { CursorGrabMode::Locked } else { CursorGrabMode::None };
            c.visible = !lock;
        }
        locked = c.grab_mode != CursorGrabMode::None;
    }
    for (mut tf, mut cam) in &mut q {
        if locked || mouse.pressed(MouseButton::Right) {
            cam.yaw += motion.delta.x * 0.12;
            cam.pitch = (cam.pitch - motion.delta.y * 0.12).clamp(-89.0, 89.0);
        }
        let fwd = coords::dir(dishonored_sim::movement::view_vector(cam.yaw, cam.pitch));
        let right = coords::dir(dishonored_sim::movement::right_vector(cam.yaw));
        let mut v = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) { v += fwd; }
        if keys.pressed(KeyCode::KeyS) { v -= fwd; }
        if keys.pressed(KeyCode::KeyD) { v += right; }
        if keys.pressed(KeyCode::KeyA) { v -= right; }
        if keys.pressed(KeyCode::KeyE) { v += Vec3::Y; }
        if keys.pressed(KeyCode::KeyQ) { v -= Vec3::Y; }
        let mut speed = 800.0;
        if keys.pressed(KeyCode::ShiftLeft) { speed *= 5.0; }
        tf.translation += v.normalize_or_zero() * speed * coords::UU_TO_BEVY * time.delta_secs();
        let target = tf.translation + fwd;
        tf.look_at(target, Vec3::Y);
    }
}
