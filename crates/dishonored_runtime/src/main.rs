//! Standalone Dishonored runtime (presentation + local input). Gameplay is
//! `dishonored_sim`, run locally (offline) or predicted against a dedicated server.
//!
//! ```text
//! dishonored_runtime                  (no arguments, or --launcher) the launcher: main menu,
//!                                     missions, multiplayer browser / server creator, options
//! dishonored_runtime [--map L_Boyle_Ext_P]
//!     (default)              offline FFA vs --bots N (default 3); --spectate to watch bots
//!     --connect HOST:PORT    join a dedicated server
//!     --browse MASTER        list servers from a control plane and join the best one
//!     --inspect              free camera, no gameplay (--pos x,y,z --yaw --pitch)
//!   options: --name NAME --show-hidden --max-tex N --lat MS --loss P
//!            --settings FILE (default %APPDATA%/dishonored-rs/settings.ron) --fullscreen
//!            --from-launcher (set by the launcher: window from the settings, Esc menu returns)
//!   rendering (all on by default; for before/after comparisons):
//!            --no-lightmaps --no-materials --no-fx --no-dynamic-lights --no-fog
//!            --no-post --no-sky, or --baseline for all of them (diffuse-only renderer
//!            with the placeholder sun/ambient)
//!   harness: --script FILE --out DIR --control PORT --record DIR --record-every N
//!            --record-fps F --exit-after SECONDS --size WxH
//!            (--inspect also runs scripts: camera / screenshot / wait_ticks / quit)
//! ```

mod coords;
mod characters;
mod debug;
mod harness;
mod hud;
mod input_map;
mod atmosphere;
mod audio;
mod lighting;
mod map_render;
mod players;
mod session;
mod gfx_hud;
mod viewmodel;
mod settings;
mod menu_art;
mod menu_ui;
mod menu_audio;
mod menu_options;
mod launcher;
mod launcher_bg;
mod pause_menu;
mod hud_settings;

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
    /// `--mission FILE`: play a custom mission (RON, see missions/README.md).
    mission: Option<dishonored_mission::MissionDef>,
    pos: Option<Vec3>,
    yaw: Option<f32>,
    pitch: f32,
    show_hidden: bool,
    features: map_render::RenderFeatures,
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
    let mission = get("--mission").map(|p| dishonored_mission::MissionDef::load(std::path::Path::new(&p)).unwrap_or_else(|e| panic!("mission {p}: {e:#}")));
    Args {
        map: get("--map").or_else(|| mission.as_ref().map(|m| m.map.clone())).unwrap_or_else(|| dishonored_content::DEFAULT_MAP.into()),
        mission,
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
        features: map_render::RenderFeatures::from_flags(&has),
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
    // No game-mode argument (or `--launcher`): the launcher (main menu, options, server creator).
    if launcher::wanted() {
        if let AppExit::Error(c) = launcher::run() {
            std::process::exit(c.get() as i32);
        }
        return;
    }
    let mut args = parse_args();
    let game_settings = settings::GameSettings::load();
    if !std::env::args().any(|a| a == "--max-tex") {
        args.max_tex = settings::max_texture_size(&game_settings.s);
    }
    // Window: unchanged for command-line runs; from the settings when the launcher started us.
    let from_launcher = std::env::args().any(|a| a == "--from-launcher");
    let (size, mode, present) = if from_launcher || std::env::args().any(|a| a == "--fullscreen") {
        settings::startup_window(Some(args.size), std::env::args().any(|a| a == "--fullscreen"), &game_settings.s)
    } else {
        (args.size, bevy::window::WindowMode::Windowed, if std::env::var_os("DISHONORED_NOVSYNC").is_some() { bevy::window::PresentMode::AutoNoVsync } else { bevy::window::PresentMode::AutoVsync })
    };
    let mut app = App::new();
    let mut plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: format!("dishonored-rs - {}", args.map),
            resolution: size.into(),
            mode,
            present_mode: present,
            ..default()
        }),
        ..default()
    });
    if !audio::AudioSettings::from_args().enabled {
        plugins = plugins.disable::<bevy::audio::AudioPlugin>();
    }
    app.add_plugins(plugins)
    .add_plugins(FrameTimeDiagnosticsPlugin::default())
    .add_plugins(MaterialPlugin::<map_render::BakedMaterial>::default())
    .add_plugins(bevy::diagnostic::EntityCountDiagnosticsPlugin::default())
    .insert_resource(ClearColor(Color::srgb(0.08, 0.09, 0.12)))
    // Placeholder ambient; with real lights it only fills non-lightmapped meshes at night level.
    .insert_resource(GlobalAmbientLight { color: Color::srgb(0.85, 0.88, 1.0), brightness: if args.features.dynamic_lights { 60.0 } else { 900.0 }, affects_lightmapped_meshes: !args.features.lightmaps })
    .insert_resource(args.clone())
    .insert_resource(args.features.clone())
    .init_resource::<atmosphere::FogLayers>()
    .add_systems(Startup, setup_world)
    .add_systems(Update, atmosphere::update_fog);

    if args.mode == Mode::Inspect {
        let mut runner = harness::ScriptRunner { out_dir: args.out.clone(), ..default() };
        if let Some(p) = &args.script {
            let s: harness::Script = serde_json::from_str(&std::fs::read_to_string(p).expect("script")).expect("script json");
            runner.steps = s.steps;
            std::fs::create_dir_all(&args.out).ok();
        }
        app.insert_resource(runner)
            .init_resource::<harness::ShotQueue>()
            .init_resource::<harness::CameraRequest>()
            .add_systems(Update, (harness::inspect_script, inspect_cam).chain());
    } else {
        let content = Arc::new(dishonored_content::load_match(&args.map).expect("loading match content from the install"));
        let session = match &args.mode {
            Mode::Offline => session::Session::offline(content, args.bots, args.spectate, args.contracts, args.mission.clone()),
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
        app.add_plugins(audio::GameAudioPlugin);
        // Settings, pause menu (Esc) and the HUD options.
        if audio::AudioSettings::from_args().enabled {
            app.add_plugins(menu_audio::MenuAudioPlugin { register_source: false, music: false });
        } else {
            app.init_resource::<menu_audio::MenuSounds>();
        }
        app.insert_resource(settings::Bindings::from_settings(&game_settings.s))
            .insert_resource(game_settings.clone())
            .init_resource::<menu_ui::MenuNav>()
            .add_systems(Startup, pause_menu::setup)
            .add_systems(Update, (settings::sync_bindings, settings::apply_window, settings::apply_camera, hud_settings::apply.after(gfx_hud::update_gfx_hud).after(gfx_hud::update_anchored_text)))
            .add_systems(Update, (pause_menu::toggle, menu_ui::gather_menu_input, pause_menu::handle, pause_menu::rebuild, menu_art::layout_prims).chain().before(input_map::gather_input));
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
            .add_systems(Startup, (players::setup_player_assets, hud::setup_hud, debug::setup_gizmos, (characters::load_library, characters::load_animations).chain(), characters::load_weapons, viewmodel::load_view_rig, gfx_hud::setup_gfx_hud, gfx_hud::load_game_fonts))
            .init_resource::<hud::DebugHud>()
            .init_resource::<viewmodel::ViewAnim>()
            .add_systems(FixedUpdate, (harness::script_fixed, session::fixed_tick).chain().run_if(pause_menu::sim_running))
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
                    gfx_hud::update_gfx_hud,
                    gfx_hud::apply_game_fonts,
                    gfx_hud::update_anchored_text,
                    debug::draw_debug,
                    harness::harness_frame,
                ),
            );
    }
    bevy::asset::embedded_asset!(app, "lightmap_ext.wgsl");
    if std::env::args().any(|a| a == "--profile") {
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin).add_systems(Update, profile_report);
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

fn setup_world(mut commands: Commands, args: Res<Args>, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, mut materials: ResMut<Assets<StandardMaterial>>, mut baked: ResMut<Assets<map_render::BakedMaterial>>) {
    let content = Content::locate().expect("Dishonored install not found (set DISHONORED_DIR)");
    // --legacy-geometry: the pre-fidelity package/actor coverage (prefix-matched sublevels,
    // StaticMeshActor + StaticMeshCollectionActor only), for before/after comparisons.
    let legacy = std::env::args().any(|a| a == "--legacy-geometry");
    let pkgs = if legacy { dishonored_assets::map_packages_by_prefix(&content, &args.map) } else { map_packages(&content, &args.map) };
    let opts = ImportOptions { max_texture_size: args.max_tex, other_actors: !legacy, ..Default::default() };
    let (map, stats) = import_level(&content, &args.map, &pkgs, &opts).expect("map import failed");
    let spawned = map_render::spawn_map(&map, &mut commands, &mut meshes, &mut images, &mut materials, &mut baked, args.show_hidden, &args.features);
    info!("{}: {} instances, {} meshes, {} textures, {} entities", args.map, stats.instances, stats.meshes, stats.textures, spawned);

    let spawn = map.spawn_candidates.first();
    let start = args.pos.or(spawn.map(|s| s.position + glam::Vec3::Z * 64.0)).unwrap_or(glam::Vec3::new(0.0, 0.0, 500.0));
    let yaw = args.yaw.or(spawn.map(|s| s.yaw_deg)).unwrap_or(0.0);
    // The Boyle sky dome reaches ~4.9 km from the estate (radius 205,852 uu around (35379, -6871)).
    let far = if args.features.sky { 12000.0 } else { 6000.0 };
    let projection = Projection::Perspective(PerspectiveProjection { fov: 75f32.to_radians(), near: 0.005, far, ..default() });
    let camera = if args.mode == Mode::Inspect {
        commands.spawn((Camera3d::default(), projection, Transform::from_translation(coords::pos(start)), FreeCam { yaw, pitch: args.pitch })).id()
    } else {
        commands.spawn((Camera3d::default(), projection, Transform::default(), players::GameCamera { third_person: args.spectate, follow: None }, if std::env::var_os("DISHONORED_NOMSAA").is_some() { Msaa::Off } else { Msaa::Sample4 })).id()
    };
    if args.features.materials {
        if let Some(env) = map_render::environment_light(&map, &mut images) {
            commands.entity(camera).insert(env);
        }
    }
    let fog = atmosphere::setup_camera(&map, &mut commands.entity(camera), &args.features);
    commands.insert_resource(fog);
    let lights = lighting::spawn_lights(&map, &mut commands, &args.features);
    info!("{} map lights", lights);
    if lights == 0 {
        // Placeholder sun when real lights are disabled (or the map has none).
        commands.spawn((
            DirectionalLight { illuminance: 6000.0, shadow_maps_enabled: false, color: Color::srgb(0.75, 0.8, 1.0), affects_lightmapped_mesh_diffuse: !args.features.lightmaps, ..default() },
            Transform::default().looking_to(Vec3::new(-0.4, -1.0, -0.3), Vec3::Y),
        ));
    }
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
    request: Option<ResMut<harness::CameraRequest>>,
) {
    if let Some(mut req) = request {
        if let Some(c) = req.0.take() {
            for (mut tf, mut cam) in &mut q {
                tf.translation = coords::pos(glam::Vec3::from(c.pos));
                cam.yaw = c.yaw;
                cam.pitch = c.pitch;
            }
        }
    }
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

/// `--profile`: every 5 s log fps, frame time, entity count and the most expensive render
/// passes (GPU and CPU time from `RenderDiagnosticsPlugin`).
fn profile_report(diag: Res<bevy::diagnostic::DiagnosticsStore>, time: Res<Time>, mut last: Local<f32>) {
    if time.elapsed_secs() - *last < 5.0 {
        return;
    }
    *last = time.elapsed_secs();
    let val = |d: &bevy::diagnostic::Diagnostic| d.smoothed().or(d.value()).unwrap_or(0.0);
    let mut gpu: Vec<(String, f64)> = Vec::new();
    let mut cpu: Vec<(String, f64)> = Vec::new();
    let mut head = String::new();
    for d in diag.iter() {
        let p = d.path().as_str().to_string();
        if p.ends_with("elapsed_gpu") {
            gpu.push((p, val(d)));
        } else if p.ends_with("elapsed_cpu") {
            cpu.push((p, val(d)));
        } else if p == "fps" || p == "frame_time" || p == "entity_count" {
            head += &format!("{p} {:.1}  ", val(d));
        }
    }
    gpu.sort_by(|a, b| b.1.total_cmp(&a.1));
    cpu.sort_by(|a, b| b.1.total_cmp(&a.1));
    let fmt = |v: &[(String, f64)]| v.iter().take(8).map(|(p, t)| format!("{}={t:.2}ms", p.trim_start_matches("render/").trim_end_matches("/elapsed_gpu").trim_end_matches("/elapsed_cpu"))).collect::<Vec<_>>().join(" ");
    info!("PROFILE {head}\n  GPU: {}\n  CPU: {}", fmt(&gpu), fmt(&cpu));
}
