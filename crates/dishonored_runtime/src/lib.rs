//! Standalone Dishonored runtime (presentation + local input). Gameplay is
//! `dishonored_sim`, run locally (offline) or predicted against a dedicated server.
//!
//! ```text
//! dishonored_runtime                  (no arguments, or --launcher) the launcher: main menu,
//!                                     missions, multiplayer browser / server creator, options
//! dishonored_runtime [--map L_Boyle_Ext_P]
//!     (default)              offline FFA vs --bots N (default 3); --spectate to watch bots
//!     --connect HOST:PORT    join a dedicated server (--autopilot [duelist|gunner|wanderer]: a test bot plays)
//!     --browse MASTER        list servers from a control plane and join the best one
//!     --inspect              free camera, no gameplay (--pos x,y,z --yaw --pitch)
//!     --playground           the Playground test level, offline, every power and item
//!                            (missions/playground.ron; `--bots N` adds bots, none by default)
//!     --mission FILE         a custom mission; --load SAVE continues a saved game (F5 / F9
//!                            quicksave / quickload; saves in --saves DIR, default SaveData
//!                            next to the settings file)
//!   options: --name NAME --show-hidden --max-tex N --lat MS --loss P
//!            --settings FILE (default %APPDATA%/dishonored-rs/settings.ron) --fullscreen
//!            --from-launcher (set by the launcher: window from the settings, Esc menu returns)
//!   rendering (all on by default; for before/after comparisons):
//!            --no-lightmaps --no-materials --no-fx --no-dynamic-lights --no-fog
//!            --no-post --no-sky --no-lightmap-specular, or --baseline for all of them (diffuse-only renderer
//!            with the placeholder sun/ambient)
//!   harness: --script FILE --out DIR --control PORT --record DIR --record-every N
//!            --record-fps F --exit-after SECONDS --size WxH
//!            (--inspect also runs scripts: camera / screenshot / wait_ticks / quit)
//! ```

#[doc(hidden)]
pub mod coords;
#[doc(hidden)]
pub mod game_dir;
#[doc(hidden)]
pub mod characters;
#[doc(hidden)]
pub mod npc_anim;
#[doc(hidden)]
pub mod facefx;
#[doc(hidden)]
pub mod ragdoll;
#[doc(hidden)]
pub mod debug;
#[doc(hidden)]
pub mod effects;
#[doc(hidden)]
pub mod harness;
#[doc(hidden)]
pub mod hud;
#[doc(hidden)]
pub mod input_map;
#[doc(hidden)]
pub mod ark_post;
#[doc(hidden)]
pub mod distortion;
#[doc(hidden)]
pub mod sky;
#[doc(hidden)]
pub mod sky_disc;
#[doc(hidden)]
pub mod bloom;
#[doc(hidden)]
pub mod atmosphere;
#[doc(hidden)]
pub mod audio;
#[doc(hidden)]
pub mod level;
#[doc(hidden)]
pub mod level_props;
#[doc(hidden)]
pub mod decals;
#[doc(hidden)]
pub mod prop_light;
#[doc(hidden)]
pub mod loading_screen;
#[doc(hidden)]
pub mod lighting;
#[doc(hidden)]
pub mod map_render;
#[doc(hidden)]
pub mod players;
#[doc(hidden)]
pub mod session;
#[doc(hidden)]
pub mod gfx_hud;
#[doc(hidden)]
pub mod viewmodel;
#[doc(hidden)]
pub mod hud_dialogue;
#[doc(hidden)]
pub mod dialogue_vo;
#[doc(hidden)]
pub mod settings;
#[doc(hidden)]
pub mod menu_art;
#[doc(hidden)]
pub mod menu_ui;
#[doc(hidden)]
pub mod menu_audio;
#[doc(hidden)]
pub mod menu_options;
#[doc(hidden)]
pub mod launcher;
#[doc(hidden)]
pub mod launcher_bg;
#[doc(hidden)]
pub mod handoff;
#[doc(hidden)]
pub mod pause_menu;
#[doc(hidden)]
pub mod hud_settings;
#[doc(hidden)]
pub mod hud_popups;
#[doc(hidden)]
pub mod saves;
/// [playground] In-world labels of layout maps.
#[doc(hidden)]
pub mod playground_labels;
#[doc(hidden)]
pub mod profile;
#[doc(hidden)]
pub mod env_map;
/// [interp] Cutscene (matinee) presentation.
#[doc(hidden)]
pub mod cutscene;

use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow, WindowPosition};

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
    /// `--autopilot [duelist|gunner|wanderer]` (online only): a test bot drives the local player.
    autopilot: Option<dishonored_bots::BotStyle>,
    contracts: bool,
    /// `--mission FILE`: play a custom mission (RON, see missions/README.md).
    mission: Option<dishonored_mission::MissionDef>,
    /// The mission file (saves record it).
    mission_path: Option<String>,
    /// `--load FILE`: continue from a saved game (its mission unless `--mission` is given).
    load: Option<PathBuf>,
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
    let load = get("--load").map(PathBuf::from);
    // `--playground`: the Playground test level (missions/playground.ron on maps/playground.ron).
    let playground = has("--playground").then(|| playground_mission().unwrap_or_else(|| panic!("missions/playground.ron not found")).to_string_lossy().into_owned());
    let mission_path = get("--mission").or(playground).or_else(|| load.as_ref().map(|l| saves::header_of(l).unwrap_or_else(|e| panic!("save {}: {e:#}", l.display())).mission));
    let mission = mission_path.as_ref().map(|p| dishonored_mission::MissionDef::load(std::path::Path::new(p)).unwrap_or_else(|e| panic!("mission {p}: {e:#}")));
    // The Playground (a layout map) has no bots unless asked for.
    let bots = get("--bots").and_then(|s| s.parse().ok()).unwrap_or(if mission.as_ref().is_some_and(|m| dishonored_content::playground::is_layout_map(&m.map)) { 0 } else { 3 });
    Args {
        map: get("--map").or_else(|| mission.as_ref().map(|m| m.map.clone())).unwrap_or_else(|| dishonored_content::DEFAULT_MAP.into()),
        mission,
        mission_path,
        load,
        mode,
        name: get("--name").unwrap_or_else(|| "player".into()),
        // The Playground has no bots unless asked for.
        bots,
        spectate: has("--spectate"),
        autopilot: has("--autopilot").then(|| match get("--autopilot").as_deref() {
            Some("gunner") => dishonored_bots::BotStyle::Gunner,
            Some("wanderer") => dishonored_bots::BotStyle::Wanderer,
            _ => dishonored_bots::BotStyle::Duelist,
        }),
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

/// The Playground's mission file: `missions/playground.ron` next to the working directory or the
/// executable (or the repository root of a `target/<profile>` build).
fn playground_mission() -> Option<PathBuf> {
    let mut dirs = vec![PathBuf::from("missions"), PathBuf::from("../../missions")];
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        dirs.push(d.join("missions"));
        dirs.push(d.join("../../missions"));
    }
    dirs.into_iter().map(|d| d.join("playground.ron")).find(|p| p.is_file())
}

#[derive(Component)]
struct FreeCam {
    yaw: f32,
    pitch: f32,
}

pub fn main() {
    game_dir::ensure();
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
    // Started by the launcher: open hidden where the launcher window is, and show once frames
    // are coming (`handoff.rs`), so menu and match share one window on screen.
    let position = handoff::window_pos_arg().map(WindowPosition::At).unwrap_or_default();
    let mut plugins = DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: if from_launcher { "Dishonored".into() } else { format!("Blink - {}", args.map) },
            resolution: size.into(),
            mode,
            present_mode: present,
            position,
            visible: !from_launcher,
            ..default()
        }),
        ..default()
    });
    if !audio::AudioSettings::from_args().enabled {
        plugins = plugins.disable::<bevy::audio::AudioPlugin>();
    }
    app.add_plugins(plugins)
    .insert_resource(audio::global_volume())
    .add_plugins(FrameTimeDiagnosticsPlugin::default())
    .add_plugins(MaterialPlugin::<map_render::BakedMaterial>::default())
    .add_plugins(ark_post::ArkPostPlugin)
    .add_plugins(distortion::DistortionPlugin)
    .add_plugins(sky::SkyPlugin)
    .add_plugins(lighting::Ue3FalloffPlugin)
    .add_plugins(decals::DecalPlugin)
    .add_plugins(prop_light::PropLightPlugin)
    .add_plugins(bloom::BloomPlugin)
    .add_plugins(env_map::EnvMapOncePlugin)
    .add_plugins(bevy::diagnostic::EntityCountDiagnosticsPlugin::default())
    .insert_resource(ClearColor(Color::srgb(0.08, 0.09, 0.12)))
    // Placeholder ambient; with real lights it only fills non-lightmapped meshes at night level.
    .insert_resource(GlobalAmbientLight { color: Color::srgb(0.85, 0.88, 1.0), brightness: if args.features.dynamic_lights { 60.0 } else { 900.0 }, affects_lightmapped_meshes: !args.features.lightmaps })
    .insert_resource(args.clone())
    .insert_resource(args.features.clone())
    .init_resource::<atmosphere::FogLayers>()
    .add_systems(Startup, setup_world)
    .add_plugins(effects::EffectsPlugin)
    .add_systems(Update, atmosphere::update_fog)
    // Doors and usable objects (skeletal meshes), posed for their state.
    .init_resource::<level_props::LevelProps>()
    .add_systems(Update, (level_props::spawn_props, level_props::pose_props).chain());

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
            // The free camera uses the game's horizontal FOV too (75 degrees, `DefaultCamera.ini`),
            // so `--inspect` views and the fidelity captures frame like the original's.
            .add_systems(Update, (harness::inspect_script, inspect_cam, viewmodel::apply_fov).chain());
    } else {
        // Every persistent map the session can travel between (the mission's maps, or --map).
        let names = match &args.mission {
            Some(def) => dishonored_content::mission_maps(def),
            None => vec![args.map.clone()],
        };
        let content = Arc::new(dishonored_content::MapContents::load(&names).expect("loading match content from the install"));
        let session = match &args.mode {
            Mode::Offline => {
                let mut s = session::Session::offline(content.clone(), args.bots, args.spectate, args.contracts, args.mission.clone());
                if let Some(l) = &args.load {
                    match saves::load_into(&mut s, l) {
                        Ok(h) => info!("loaded {} (slot {}, {})", l.display(), h.slot, h.map),
                        Err(e) => error!("loading {}: {e:#}", l.display()),
                    }
                }
                s
            }
            Mode::Connect(addr) => session::Session::online(content, connect(addr, &args), autopilot_of(&args)),
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
                session::Session::online(content, connect(&best.0, &args), autopilot_of(&args))
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
        app.add_plugins(hud_dialogue::DialoguePlugin);
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
            .add_systems(Update, (settings::sync_bindings, settings::apply_window, settings::apply_camera, hud_settings::update.before(gfx_hud::update_gfx_hud).before(gfx_hud::update_anchored_text)))
            .init_resource::<hud_settings::HudOverrides>()
            .add_systems(Update, (pause_menu::toggle, menu_ui::gather_menu_input, pause_menu::handle, pause_menu::rebuild, menu_art::layout_prims).chain().before(input_map::gather_input));
        app.insert_resource(session)
            .insert_resource(session::SessionView::default())
            .insert_resource(input_map::LocalInput { enabled: true, ..default() })
            .insert_resource(Time::<Fixed>::from_hz(dishonored_sim::movement::TICK_HZ as f64))
            .insert_resource(runner)
            .insert_resource(recorder)
            .init_resource::<harness::ShotQueue>()
            .init_resource::<harness::ControlOverride>()
            .init_resource::<harness::ViewRequest>()
            .init_resource::<loading_screen::LoadingScreen>()
            .init_resource::<debug::DebugToggles>()
            .init_gizmo_group::<debug::XRayGizmos>()
            .add_systems(Startup, (players::setup_player_assets, hud::setup_hud, debug::setup_gizmos, (characters::load_library, characters::load_animations).chain(), characters::load_weapons, viewmodel::load_view_rig, gfx_hud::setup_gfx_hud, gfx_hud::load_game_fonts))
            .init_resource::<hud::DebugHud>()
            .init_resource::<hud_popups::HudPopups>()
            .add_systems(Update, hud_popups::track.after(input_map::gather_input).before(gfx_hud::update_gfx_hud).before(gfx_hud::update_anchored_text))
            .init_resource::<viewmodel::ViewAnim>()
            .add_systems(FixedUpdate, (harness::script_fixed, session::fixed_tick, saves::write_autosaves).chain().run_if(pause_menu::sim_running))
            .insert_resource(saves::Saves::new(args.mission_path.clone()))
            // [playground] station labels of layout maps
            .add_systems(Startup, playground_labels::setup_labels)
            .add_systems(Update, playground_labels::update_labels.after(players::camera_follow).after(cutscene::camera))
            .add_systems(Update, (saves::hotkeys, saves::process).chain().after(pause_menu::handle))
            // [interp] cutscenes: before the characters are animated; the director camera after the follow camera
            .init_resource::<cutscene::Cutscenes>()
            .init_resource::<cutscene::PawnPoints>()
            .add_systems(Update, (cutscene::drive.after(players::sync_players).before(characters::animate_characters), cutscene::props.after(cutscene::drive), cutscene::emitters.after(cutscene::drive), cutscene::camera.after(players::camera_follow).before(viewmodel::animate_view_rig), cutscene::look_limits.after(input_map::gather_input).before(players::camera_follow), level_props::follow_movers.after(cutscene::drive)))
            .add_systems(
                Update,
                (
                    input_map::gather_input,
                    input_map::cursor_lock,
                    players::sync_players,
                    characters::animate_characters,
                    characters::load_appearances,
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
                    loading_screen::update_loading_screen,
                    level::follow_level,
                    level::tag_level_entities,
                ),
            );
    }
    bevy::asset::embedded_asset!(app, "lightmap_ext.wgsl");
    if std::env::args().any(|a| a == "--profile") {
        app.add_plugins(profile::ProfilePlugin);
    }
    app.add_systems(Update, exit_after);
    app.init_resource::<handoff::ReturnToMenu>().add_systems(Update, handoff::return_to_menu);
    if from_launcher {
        app.init_resource::<handoff::Reveal>().add_systems(Update, handoff::reveal).add_systems(Last, handoff::report_exit);
    }
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

/// Level import options, kept for levels loaded later by travel.
#[derive(Resource)]
pub struct LevelSettings(pub level::LevelOptions);

fn setup_world(mut commands: Commands, args: Res<Args>, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>, mut materials: ResMut<Assets<StandardMaterial>>, mut baked: ResMut<Assets<map_render::BakedMaterial>>, mut fx: ResMut<effects::Effects>) {
    let opts = level::LevelOptions { max_tex: args.max_tex, show_hidden: args.show_hidden, legacy: std::env::args().any(|a| a == "--legacy-geometry") };
    let mut levels = level::Levels::default();
    let map = level::spawn_level(&args.map, &opts, &args.features, &mut commands, &mut meshes, &mut images, &mut materials, &mut baked, &mut levels, &mut fx);

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
    level::apply_look(&levels, &args.map, camera, &mut commands, &args.features);
    commands.insert_resource(levels);
    commands.insert_resource(LevelSettings(opts));
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

/// The `--autopilot` bot for an online session, seeded from the player name so clients differ.
fn autopilot_of(args: &Args) -> Option<(dishonored_bots::BotStyle, u64)> {
    let seed = args.name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    args.autopilot.map(|s| (s, seed))
}
