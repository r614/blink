//! Standalone launcher: what `dishonored_runtime` opens when started without a game-mode
//! argument (or with `--launcher`). It is the original main menu - the `Dishonored_MainMenu`
//! level behind the `UI_MainMenu.MainMenu` movie, the menu music and the UI sound theme - with
//! this port's entries:
//!
//! - **Play**: offline match against bots (the `newGame_mc` screen: mode list + description).
//! - **Missions**: custom missions (`missions/*.ron`) with title and briefing.
//! - **Multiplayer**: *Join* (server browser from the control-plane master, ping per server,
//!   direct connect) and *Host* (server creator: map, mode FFA / Contracts / mission file,
//!   port, name, max players, score / time limits, master registration; runs a local
//!   `dishonored_server`, optionally a local `dishonored_master`, and shows status and log).
//! - **Options**: every original setting (`menu_options.rs`), saved to the settings file.
//! - **Quit**.
//!
//! Games run as a child `dishonored_runtime` process with the matching CLI arguments plus
//! `--from-launcher` (`--window-pos` / `--size` of the launcher window, fullscreen from the
//! settings). The player sees one window (`handoff.rs`): the launcher shows the original
//! loading screen for the map, the game's window opens hidden in the same place and replaces
//! it once it renders, and the launcher hides; the pause menu's "Back to Main Menu" (or the
//! game ending) shows the launcher's main menu in its place again.
//!
//! The main menu opens like the original's (`PlayOpenAnimation`): the button band slides in
//! and input starts 0.58 s after the menu is shown, see [`OPEN_INPUT_DELAY`].
//! `DISHONORED_LAUNCHER_CHILD_ARGS` (whitespace-separated) is prepended to every child's
//! arguments; the harness uses it to reach the game's control socket.
//!
//! Automation: `--control PORT` (JSON lines: `state`, `activate`, `step`, `back`, `text`,
//! `capture`, `screenshot`, `quit`) and `--script FILE` (`{"steps":[{"activate":"main:options"},
//! {"wait_frames":30},{"screenshot":"options"},{"quit":true}]}`), `--out DIR`.

use crate::launcher_bg::{self, MenuBackground};
use crate::menu_art::{self, DisplayList, FontKind, MovieArt, States, TEXT, TEXT_DIM, px_matrix};
use crate::menu_audio::{MenuAudioPlugin, MenuSounds};
use crate::menu_options::{GENERIC_LINE_H, LineWidget, OptionsMenu, Outcome, generic_line};
use crate::menu_ui::{self, MenuArt, MenuEvent, MenuNav, MenuTexts, SEC_BASE, SEC_MAIN, SEC_MENU};
use crate::settings::GameSettings;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::ecs::system::NonSendMarker;
use bevy::window::{PrimaryWindow, WindowMode, WindowPosition};
use bevy::winit::{UpdateMode, WinitSettings};
use crate::handoff;
use serde::Deserialize;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Main,
    Play,
    Missions,
    Multiplayer,
    Options,
    /// The save list (`UI_LoadGame.LoadGame`, `t_LoadGame`).
    Load,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Main => "main",
            Page::Play => "play",
            Page::Missions => "missions",
            Page::Load => "load",
            Page::Multiplayer => "multiplayer",
            Page::Options => "options",
        }
    }
}

#[derive(Clone)]
pub struct MissionEntry {
    pub path: PathBuf,
    pub title: String,
    pub briefing: String,
    pub map: String,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ServerRow {
    pub addr: String,
    pub name: String,
    pub map: String,
    pub mode: String,
    pub players: u32,
    pub max_players: u32,
    pub ping_ms: Option<f32>,
}

struct Proc {
    child: Child,
    log: Arc<Mutex<Vec<String>>>,
    /// Window hand-over lines from a game child (`handoff.rs`).
    handoff: Arc<Mutex<Handoff>>,
    what: String,
}

#[derive(Default, Clone)]
struct Handoff {
    /// The game window is up (the launcher steps aside).
    shown: bool,
    /// Back to Main Menu: where the game window was (the launcher takes its place).
    returned: Option<Option<handoff::Placement>>,
}

impl Proc {
    fn spawn(mut cmd: Command, what: &str) -> std::io::Result<Self> {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
        let mut child = cmd.spawn()?;
        let log = Arc::new(Mutex::new(Vec::new()));
        let marks = Arc::new(Mutex::new(Handoff::default()));
        for out in [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>), child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)].into_iter().flatten() {
            let log = log.clone();
            let marks = marks.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if line.contains(handoff::SHOWN) {
                        marks.lock().unwrap().shown = true;
                    } else if line.contains(handoff::RETURN) {
                        marks.lock().unwrap().returned = Some(handoff::Placement::parse_return(&line));
                    }
                    let mut l = log.lock().unwrap();
                    l.push(line);
                    let n = l.len();
                    if n > 400 {
                        l.drain(0..n - 400);
                    }
                }
            });
        }
        Ok(Self { child, log, handoff: marks, what: what.to_string() })
    }
    fn handoff(&self) -> Handoff {
        self.handoff.lock().unwrap().clone()
    }
    fn tail(&self, n: usize) -> Vec<String> {
        let l = self.log.lock().unwrap();
        l[l.len().saturating_sub(n)..].to_vec()
    }
    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Resource)]
pub struct Launcher {
    pub page: Page,
    pub options: OptionsMenu,
    pub play_mode: usize,
    pub bots: u32,
    pub missions: Vec<MissionEntry>,
    pub mission_sel: usize,
    pub mp_host: bool,
    pub servers: Vec<ServerRow>,
    browse_rx: Option<Mutex<Receiver<Result<Vec<ServerRow>, String>>>>,
    pub browse_status: String,
    game: Option<Proc>,
    server: Option<Proc>,
    master: Option<Proc>,
    pub server_status: String,
    pub last_game_exit: Option<i32>,
    pub dirty: bool,
    root: Option<Entity>,
    /// The launcher window is hidden while the game's window stands in for it.
    hidden: bool,
    /// Map whose loading screen goes up (set by [`Launcher::launch`]).
    begin_loading: Option<String>,
    /// When the main menu last opened (real seconds): its opening animation and input lock.
    opened_at: Option<f64>,
    /// Real seconds at which the menu was first drawn / first took input (harness timings).
    first_drawn: Option<f64>,
    first_input: Option<f64>,
    /// Frames slower than [`HITCH_MS`] after the menu was first drawn: (seconds, ms).
    hitches: Vec<(f32, f32)>,
    pub maps: Vec<String>,
    /// Saved games, newest first (refreshed on the main and load pages).
    pub saves: Vec<(PathBuf, dishonored_save::SaveHeader)>,
    pub save_sel: usize,
}

/// The game's saves folder: `SaveData` next to the settings file the launcher hands the game.
fn saves_dir(gs: &GameSettings) -> PathBuf {
    gs.path.parent().map(|p| p.join("SaveData")).unwrap_or_else(dishonored_save::saves_dir)
}

const PLAY_MODES: &[(&str, &str)] = &[
    ("FREE FOR ALL", "Deathmatch on the Boyle estate against bots. First to the score limit wins."),
    ("CONTRACTS", "Every player has one target and one pursuer among the party guests. Observe, identify, stalk, assassinate, escape."),
    ("SPECTATE", "Watch the bots play a free-for-all match from their point of view."),
];
const HOST_MODES: &[&str] = &["ffa", "contracts", "mission"];

pub fn missions_dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DISHONORED_MISSIONS") {
        return Some(PathBuf::from(p));
    }
    let mut cands = vec![PathBuf::from("missions")];
    if let Ok(exe) = std::env::current_exe() {
        for a in exe.ancestors().skip(1).take(4) {
            cands.push(a.join("missions"));
        }
    }
    cands.into_iter().find(|p| p.is_dir())
}

fn load_missions() -> Vec<MissionEntry> {
    let Some(dir) = missions_dir() else { return Vec::new() };
    let mut v: Vec<MissionEntry> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ron"))
        .filter_map(|p| {
            let d = dishonored_mission::MissionDef::load(&p).map_err(|e| warn!("mission {}: {e:#}", p.display())).ok()?;
            Some(MissionEntry { path: p, title: d.title, briefing: d.briefing, map: d.map })
        })
        .collect();
    v.sort_by(|a, b| a.title.cmp(&b.title));
    v
}

fn sibling_exe(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("dishonored_runtime"));
    exe.with_file_name(if cfg!(windows) { format!("{name}.exe") } else { name.to_string() })
}

impl Launcher {
    fn new() -> Self {
        let missions = load_missions();
        let mut maps = vec![dishonored_content::DEFAULT_MAP.to_string()];
        for m in &missions {
            if !maps.contains(&m.map) {
                maps.push(m.map.clone());
            }
        }
        Self {
            page: Page::Main,
            options: OptionsMenu::default(),
            play_mode: 0,
            bots: 3,
            missions,
            mission_sel: 0,
            mp_host: false,
            servers: Vec::new(),
            browse_rx: None,
            browse_status: String::new(),
            game: None,
            server: None,
            master: None,
            server_status: "stopped".into(),
            last_game_exit: None,
            dirty: true,
            root: None,
            hidden: false,
            begin_loading: None,
            opened_at: None,
            first_drawn: None,
            first_input: None,
            hitches: Vec::new(),
            maps,
            saves: Vec::new(),
            save_sel: 0,
        }
    }

    /// A game process exists (from launch until it exits).
    pub fn game_running(&self) -> bool {
        self.game.is_some()
    }

    /// The game is loading or being played (not yet handed back to the menu).
    fn in_game(&self) -> bool {
        self.game.as_ref().is_some_and(|g| g.handoff().returned.is_none())
    }

    /// Start a game child process with `args` (plus launcher-provided window settings): it
    /// opens hidden at the launcher window's place and size and takes over once it renders
    /// (`handoff.rs`); meanwhile the launcher shows `map`'s loading screen.
    fn launch(&mut self, args: Vec<String>, gs: &GameSettings, place: Option<handoff::Placement>, map: &str) {
        let mut cmd = Command::new(sibling_exe("dishonored_runtime"));
        if let Ok(extra) = std::env::var("DISHONORED_LAUNCHER_CHILD_ARGS") {
            cmd.args(extra.split_whitespace());
        }
        cmd.args(&args).arg("--from-launcher").arg("--settings").arg(&gs.path);
        let size = place.map(|p| p.size).or(gs.s.graphics.resolution).unwrap_or((1600, 900));
        cmd.args(["--size", &format!("{}x{}", size.0, size.1)]);
        if let Some(pos) = place.and_then(|p| p.pos) {
            cmd.args(["--window-pos", &format!("{},{}", pos.x, pos.y)]);
        }
        if gs.s.graphics.fullscreen {
            cmd.arg("--fullscreen");
        }
        info!("launching game: {args:?}");
        match Proc::spawn(cmd, &args.join(" ")) {
            Ok(p) => {
                self.game = Some(p);
                self.last_game_exit = None;
                self.begin_loading = Some(map.to_string());
            }
            Err(e) => warn!("could not start the game: {e}"),
        }
        self.dirty = true;
    }

    fn start_server(&mut self, gs: &GameSettings) {
        let h = &gs.s.multiplayer.host;
        if h.register && h.local_master && self.master.is_none() {
            let bind = match h_master_port(&gs.s.multiplayer.master) {
                Some(p) => format!("0.0.0.0:{p}"),
                None => "0.0.0.0:27900".into(),
            };
            let mut cmd = Command::new(sibling_exe("dishonored_master"));
            cmd.args(["--bind", &bind]);
            match Proc::spawn(cmd, "master") {
                Ok(p) => self.master = Some(p),
                Err(e) => warn!("could not start dishonored_master: {e}"),
            }
        }
        let mut cmd = Command::new(sibling_exe("dishonored_server"));
        cmd.args(["--port", &h.port.to_string(), "--name", &h.name, "--max-players", &h.max_players.to_string(), "--score-limit", &h.score_limit.to_string(), "--time-limit", &h.time_limit.to_string()]);
        match h.mode.as_str() {
            "mission" => {
                if let Some(m) = self.missions.iter().find(|m| m.path.file_name().is_some_and(|f| f.to_string_lossy() == h.mission)).or(self.missions.first()) {
                    cmd.arg("--mission").arg(&m.path);
                }
            }
            "contracts" => {
                cmd.args(["--mode", "contracts", "--map", &h.map]);
            }
            _ => {
                cmd.args(["--map", &h.map]);
            }
        }
        if h.register {
            cmd.args(["--master", &gs.s.multiplayer.master]);
        }
        match Proc::spawn(cmd, "server") {
            Ok(p) => {
                self.server = Some(p);
                self.server_status = "starting".into();
            }
            Err(e) => self.server_status = format!("failed: {e}"),
        }
        self.dirty = true;
    }

    fn stop_server(&mut self) {
        self.server = None;
        self.master = None;
        self.server_status = "stopped".into();
        self.dirty = true;
    }

    fn refresh(&mut self, master: &str) {
        let (tx, rx) = channel();
        let master = master.to_string();
        std::thread::spawn(move || {
            let r = dishonored_net::control_plane::list_servers(&master).map_err(|e| format!("{e:#}")).map(|list| {
                list.into_iter()
                    .map(|s| {
                        let ping = s.addr.parse().ok().and_then(|a| dishonored_net::client::query_info(a, std::time::Duration::from_millis(500))).map(|(_, rtt)| rtt);
                        ServerRow { addr: s.addr, name: s.info.name, map: s.info.map, mode: s.info.mode, players: s.info.players, max_players: s.info.max_players, ping_ms: ping }
                    })
                    .collect()
            });
            let _ = tx.send(r);
        });
        self.browse_rx = Some(Mutex::new(rx));
        self.browse_status = "Refreshing...".into();
        self.dirty = true;
    }

    fn poll(&mut self) {
        let got = self.browse_rx.as_ref().and_then(|rx| rx.lock().unwrap().try_recv().ok());
        {
            if let Some(r) = got {
                match r {
                    Ok(v) => {
                        self.browse_status = format!("{} servers", v.len());
                        self.servers = v;
                    }
                    Err(e) => {
                        self.browse_status = format!("Master unreachable: {e}");
                        self.servers.clear();
                    }
                }
                self.browse_rx = None;
                self.dirty = true;
            }
        }
        if let Some(g) = &mut self.game {
            if let Ok(Some(st)) = g.child.try_wait() {
                info!("game exited: {st}");
                self.last_game_exit = st.code();
                self.game = None;
                self.dirty = true;
            }
        }
        if let Some(s) = &mut self.server {
            let up = s.running();
            let listening = s.log.lock().unwrap().iter().any(|l| l.contains("listening on"));
            let st = match (up, listening) {
                (false, _) => format!("stopped ({})", s.child.try_wait().ok().flatten().map(|c| c.to_string()).unwrap_or_default()),
                (true, true) => "running".into(),
                (true, false) => "starting".into(),
            };
            if st != self.server_status {
                self.server_status = st;
                self.dirty = true;
            }
        }
    }
}

fn h_master_port(master: &str) -> Option<u16> {
    master.rsplit(':').next()?.parse().ok()
}

/// Script step for `--script` in launcher mode.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // marker fields of the untagged JSON steps
#[serde(untagged)]
pub enum LStep {
    Activate { activate: String },
    Step { step: (String, i32) },
    Back { back: bool },
    Text { text: (String, String) },
    Screenshot { screenshot: String },
    Wait { wait_frames: u32 },
    /// Wait until the menu level has loaded (or failed).
    WaitLevel { wait_level: bool },
    Quit { quit: bool },
}

#[derive(Deserialize)]
struct LScript {
    steps: Vec<LStep>,
}

#[derive(Resource, Default)]
pub struct LauncherScript {
    steps: Vec<LStep>,
    index: usize,
    wait: u32,
    out: PathBuf,
    quit_in: Option<u32>,
    shots: Vec<PathBuf>,
}

pub struct LauncherArgs {
    pub size: Option<(u32, u32)>,
    pub control: Option<u16>,
    pub script: Option<PathBuf>,
    pub out: PathBuf,
    pub menu_level: bool,
    pub exit_after: Option<f32>,
}

impl LauncherArgs {
    pub fn parse() -> Self {
        let a: Vec<String> = std::env::args().skip(1).collect();
        let get = |f: &str| a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).cloned();
        Self {
            size: get("--size").and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))),
            control: get("--control").and_then(|s| s.parse().ok()),
            script: get("--script").map(PathBuf::from),
            out: get("--out").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("harness_out")),
            menu_level: !a.iter().any(|x| x == "--no-menu-level"),
            exit_after: get("--exit-after").and_then(|s| s.parse().ok()),
        }
    }
}

/// True when `dishonored_runtime` should open the launcher: no arguments, or `--launcher`.
pub fn wanted() -> bool {
    let a: Vec<String> = std::env::args().skip(1).collect();
    a.is_empty() || a.iter().any(|x| x == "--launcher")
}

#[derive(Resource)]
struct ExitAfter(f32);

pub fn run() -> AppExit {
    let la = LauncherArgs::parse();
    let gs = GameSettings::load();
    let (size, mode, present) = crate::settings::startup_window(la.size, la.size.is_none() && gs.s.graphics.fullscreen, &gs.s);
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window { title: "Dishonored".into(), resolution: size.into(), mode, present_mode: present, ..default() }),
        ..default()
    }))
    .add_plugins(MaterialPlugin::<crate::map_render::BakedMaterial>::default())
    .add_plugins(crate::ark_post::ArkPostPlugin)
    .add_plugins(crate::sky::SkyPlugin)
    .add_plugins(crate::bloom::BloomPlugin)
    .add_plugins(crate::env_map::EnvMapOncePlugin)
    .add_plugins(MenuAudioPlugin { register_source: true, music: true })
    .insert_resource(crate::audio::global_volume())
    .insert_resource(ClearColor(Color::srgb(0.05, 0.055, 0.06)))
    .insert_resource(GlobalAmbientLight { color: Color::srgb(0.85, 0.88, 1.0), brightness: 60.0, affects_lightmapped_meshes: false })
    .insert_resource(crate::map_render::RenderFeatures::from_flags(&|f: &str| std::env::args().any(|a| a == f)))
    .init_resource::<crate::atmosphere::FogLayers>()
    .insert_resource(crate::settings::Bindings::from_settings(&gs.s))
    .insert_resource(gs)
    .insert_resource(MenuTexts::load())
    .insert_resource(MenuNav { enabled: true, ..default() })
    .init_resource::<crate::loading_screen::LoadingScreen>()
    .insert_resource(Launcher::new());
    if let Some(port) = la.control {
        app.insert_resource(crate::harness::start_control_server(port));
    }
    let mut script = LauncherScript { out: la.out.clone(), ..default() };
    if let Some(p) = &la.script {
        let s: LScript = serde_json::from_str(&std::fs::read_to_string(p).expect("launcher script")).expect("launcher script json");
        script.steps = s.steps;
        std::fs::create_dir_all(&la.out).ok();
    }
    if let Some(s) = la.exit_after {
        app.insert_resource(ExitAfter(s));
    }
    let level = la.menu_level;
    app.insert_resource(script)
        .add_systems(Startup, (move |c: Commands| launcher_bg::start(c, level), setup, crate::gfx_hud::load_game_fonts))
        .add_systems(
            Update,
            (
                frame_timing,
                launcher_bg::finish,
                crate::atmosphere::update_fog,
                menu_lock,
                menu_ui::gather_menu_input,
                control,
                run_script,
                handle,
                window_handoff,
                rebuild,
                crate::loading_screen::update_loading_screen,
                menu_art::layout_prims,
                crate::settings::apply_window,
                crate::settings::apply_camera,
                exit_after,
            )
                .chain(),
        );
    bevy::asset::embedded_asset!(app, "lightmap_ext.wgsl");
    app.run()
}

fn exit_after(time: Res<Time>, e: Option<Res<ExitAfter>>, mut exit: MessageWriter<AppExit>) {
    if e.is_some_and(|e| time.elapsed_secs() >= e.0) {
        exit.write(AppExit::Success);
    }
}

fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, mut l: ResMut<Launcher>) {
    launcher_bg::spawn_camera(&mut commands);
    commands.insert_resource(menu_ui::load_menu_art(&["main", "options", "gamma"], &mut images));
    l.root = Some(menu_art::spawn_layer(&mut commands, 10));
}

/// The main menu's opening (`MainMenu.PlayOpenAnimation`, AS2 class sprite 291 of
/// `UI_MainMenu.MainMenu`): after a 180 ms interval `OpenMenu` slides the button band
/// (`_menu_mc`) in from `width + 150` px to the right with alpha 0 -> 100 over 0.35 s
/// (`Strong.easeOut`); the band's `MainMenuButtonBar.Open` (sprite 289) then waits a
/// 400 ms interval before `OnOpened` -> `InitInputs`. Until then the menu takes no input.
const OPEN_BAND_DELAY: f32 = 0.18;
const OPEN_BAND_TWEEN: f32 = 0.35;
const OPEN_INPUT_DELAY: f32 = 0.18 + 0.40;
/// A frame this slow after the menu is drawn counts as a hitch (harness timing evidence).
const HITCH_MS: f32 = 100.0;

/// `mx.transitions.easing.Strong.easeOut` (quintic).
fn strong_ease_out(t: f32) -> f32 {
    let u = t.clamp(0.0, 1.0) - 1.0;
    u * u * u * u * u + 1.0
}

/// Seconds since the main menu opened (None before it is first drawn).
fn open_t(l: &Launcher, time: &Time<Real>) -> Option<f32> {
    l.opened_at.map(|t| (time.elapsed_secs_f64() - t) as f32)
}

/// Real frame times once the menu is drawn: slow frames are where clicks wait.
fn frame_timing(time: Res<Time<Real>>, mut l: ResMut<Launcher>) {
    let dt = time.delta_secs() * 1000.0;
    if l.first_drawn.is_some() && !l.hidden && dt > HITCH_MS && l.hitches.len() < 64 {
        let at = time.elapsed_secs();
        l.hitches.push((at, dt));
    }
}

/// The original's input lock while the main menu opens; nothing else gates input.
fn menu_lock(time: Res<Time<Real>>, mut l: ResMut<Launcher>, mut nav: ResMut<MenuNav>) {
    let opening = l.page == Page::Main && open_t(&l, &time).is_none_or(|t| t < OPEN_INPUT_DELAY);
    let enabled = !opening && !l.game_running();
    if enabled && l.first_input.is_none() {
        l.first_input = Some(time.elapsed_secs_f64());
    }
    if nav.enabled != enabled {
        nav.enabled = enabled;
    }
    // The band slides in: redraw each frame of the opening.
    if l.page == Page::Main && open_t(&l, &time).is_some_and(|t| t < OPEN_BAND_DELAY + OPEN_BAND_TWEEN + 0.05) {
        l.dirty = true;
    }
}

/// Hand the window to the game and take it back (`handoff.rs`), with the loading screen in
/// between.
#[allow(clippy::too_many_arguments)]
fn window_handoff(
    mut commands: Commands,
    mut l: ResMut<Launcher>,
    mut loading: ResMut<crate::loading_screen::LoadingScreen>,
    mut images: ResMut<Assets<Image>>,
    mut sounds: ResMut<MenuSounds>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut cams: Query<&mut Camera, With<launcher_bg::MenuCamera>>,
    mut winit: ResMut<WinitSettings>,
    mut nav: ResMut<MenuNav>,
) {
    if let Some(map) = l.begin_loading.take() {
        // `m_pStopMusicEvent`: the menu music stops as the level loads.
        sounds.want_music = false;
        loading.begin(&mut commands, &mut images, &map);
    }
    let marks = l.game.as_ref().map(|g| g.handoff()).unwrap_or_default();
    let back = l.game.is_none() || marks.returned.is_some();
    if !l.hidden && !back && marks.shown {
        // The game window is up in our place: step aside, render nothing, poll slowly.
        if let Some(mut w) = windows.iter_mut().next() {
            w.visible = false;
        }
        for mut c in &mut cams {
            c.is_active = false;
        }
        *winit = WinitSettings { focused_mode: UpdateMode::reactive(std::time::Duration::from_millis(30)), unfocused_mode: UpdateMode::reactive(std::time::Duration::from_millis(30)) };
        l.hidden = true;
    } else if l.hidden && loading.active {
        // A frame after hiding (the screen is no longer shown): stop the movie.
        loading.close(&mut commands);
    }
    if back && (l.hidden || loading.active) {
        // Back to Main Menu (or the game ended / failed to start): the menu in the game
        // window's place.
        loading.close(&mut commands);
        if l.hidden {
            if let Some(mut w) = windows.iter_mut().next() {
                if let Some(Some(p)) = marks.returned {
                    if matches!(w.mode, WindowMode::Windowed) {
                        if let Some(pos) = p.pos {
                            w.position = WindowPosition::At(pos);
                        }
                        w.resolution.set_physical_resolution(p.size.0, p.size.1);
                    }
                }
                w.visible = true;
                w.focused = true;
            }
            for mut c in &mut cams {
                c.is_active = true;
            }
            *winit = WinitSettings::game();
            l.hidden = false;
        }
        sounds.want_music = true;
        l.page = Page::Main;
        l.opened_at = None;
        nav.focus = None;
        l.dirty = true;
    }
}

/// Screen content for the current page.
#[allow(clippy::too_many_arguments)]
fn build(l: &Launcher, art: &MenuArt, t: &MenuTexts, gs: &GameSettings, nav: &MenuNav, bg: &str, open_t: Option<f32>) -> DisplayList {
    let mut dl = DisplayList::default();
    let focus = nav.focus.as_deref().unwrap_or("");
    let Some(main) = art.get("main") else {
        // No install: plain text menu so the launcher still works.
        dl.text("Blink: Dishonored install not found (set DISHONORED_DIR)", Rect::new(100.0, 100.0, 1200.0, 140.0), 24.0, FontKind::Normal, TEXT, Justify::Left);
        return dl;
    };
    // Loading a game: the loading screen (`loading_screen.rs`) covers the window.
    if l.in_game() {
        return dl;
    }
    // Without the 3D level, the new-game screen's painted background stands in.
    if bg != "level" {
        if let Some((ng, m)) = main.root_child("newGame_mc") {
            if let Some((c, cm)) = main.child(ng, "_bkgdTexture_mc") {
                dl.movie(main, c, m.mul(&cm), &States::default(), 1.0);
            }
        }
    }
    match l.page {
        Page::Main => build_main(&mut dl, main, t, focus, !l.saves.is_empty(), open_t.unwrap_or(0.0)),
        Page::Load => build_load(&mut dl, art.get("options"), l, t, focus),
        Page::Play => build_play(&mut dl, main, art.get("options"), l, t, focus),
        Page::Missions => build_missions(&mut dl, main, art.get("options"), l, t, focus),
        Page::Multiplayer => {
            if let Some(o) = art.get("options") {
                build_mp(&mut dl, o, l, t, gs, nav);
            }
        }
        Page::Options => {
            if let Some(o) = art.get("options") {
                let _ = o;
                l.options.build(&mut dl, art, t, gs, nav, true);
            }
        }
    }
    dl
}

fn main_entries(t: &MenuTexts, has_saves: bool) -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    // The original's Continue (`t_ContinueGame`: the last save, `OnLoadLastSaveClicked`) and
    // Load (`t_LoadGame`), shown when saves exist.
    if has_saves {
        v.push(("main:continue", t.t(SEC_MAIN, "t_ContinueGame").to_uppercase()));
    }
    v.extend([
        ("main:play", "PLAY".into()),
        ("main:missions", t.t(SEC_MAIN, "t_Missions").to_uppercase()),
        ("main:playground", "PLAYGROUND".into()),
    ]);
    if has_saves {
        v.push(("main:load", t.t(SEC_MENU, "t_LoadGame").to_uppercase()));
    }
    v.extend([
        ("main:multiplayer", "MULTIPLAYER".into()),
        ("main:options", t.t(SEC_BASE, "t_Options").to_uppercase()),
        ("main:quit", t.t(SEC_MENU, "t_QuitGame").to_uppercase()),
    ]);
    v
}

fn build_main(dl: &mut DisplayList, art: &MovieArt, t: &MenuTexts, focus: &str, has_saves: bool, open_t: f32) {
    let Some((mm, m)) = art.root_child("mainMenu_mc") else { return };
    let sp = art.sprite(mm).cloned().unwrap_or_default();
    // The band's opening tween (see `OPEN_BAND_DELAY`): hidden, then sliding in from the right.
    let Some((menu, mmx)) = art.child(mm, "_menu_mc") else { return };
    let band_rest = m.mul(&mmx);
    let band = art.child(menu, "_bkgd_mc").and_then(|(b, bm)| art.bounds(b, band_rest.mul(&bm), &States::default()));
    let p = if open_t < OPEN_BAND_DELAY { 0.0 } else { strong_ease_out((open_t - OPEN_BAND_DELAY) / OPEN_BAND_TWEEN) };
    let band_w = band.map(|r| r.width()).unwrap_or(1280.0);
    let slide = (band_w + 150.0) * (1.0 - p);
    // Everything of mainMenu_mc but the DLC button (console store entry).
    for e in art.movie.display_list(&sp, 0) {
        let Some(c) = e.character else { continue };
        let is_band = e.name.as_deref() == Some("_menu_mc");
        if e.name.as_deref() == Some("_DLCButton_mc") || (is_band && p <= 0.0) {
            continue;
        }
        let mut cm = m.mul(&e.matrix.unwrap_or_default());
        if is_band {
            cm.tx += slide * 20.0;
        }
        dl.movie(art, c, cm, &States::default(), if is_band { p } else { 1.0 });
    }
    if p <= 0.0 {
        return;
    }
    // The horizontal button bar (MainMenuButtonBar) along the _menu_mc band.
    let band_m = band_rest;
    let entries = main_entries(t, has_saves);
    // Band-local layout: buttons spread over the visible part of the band.
    let (x0, x1) = match band {
        Some(r) => (r.min.x.max(40.0) + 60.0, r.max.x.min(1240.0) - 60.0),
        None => (200.0, 1080.0),
    };
    let cy = band.map(|r| r.center().y).unwrap_or(590.0);
    let n = entries.len() as f32;
    for (i, (id, label)) in entries.iter().enumerate() {
        let x = x0 + (x1 - x0) * (i as f32 + 0.5) / n + slide;
        let bm = dishonored_assets::swf::Matrix { a: band_m.a, b: band_m.b, c: band_m.c, d: band_m.d, tx: x * 20.0, ty: cy * 20.0 };
        menu_ui::movie_button(dl, art, "m_MainMenu_btn_", bm, id, label, focus == *id, Vec2::new((x1 - x0) / n - 10.0, 44.0));
    }
}

fn build_play(dl: &mut DisplayList, art: &MovieArt, opts: Option<&MovieArt>, l: &Launcher, t: &MenuTexts, focus: &str) {
    let Some((ng, m)) = art.root_child("newGame_mc") else { return };
    let sp = art.sprite(ng).cloned().unwrap_or_default();
    for p in art.movie.display_list(&sp, 0) {
        let Some(c) = p.character else { continue };
        if matches!(p.name.as_deref(), Some("_bkgdTexture_mc") | Some("_title_mc")) {
            continue;
        }
        dl.movie(art, c, m.mul(&p.matrix.unwrap_or_default()), &States::default(), 1.0);
    }
    dl.text("PLAY", Rect::new(110.0, 50.0, 700.0, 100.0), 44.0, FontKind::Title, TEXT, Justify::Left);
    // Mode buttons (m_nGame_btn, the difficulty list) inside the menu panel.
    let menu = art.child(ng, "_menu_mc").map(|(c, cm)| (c, m.mul(&cm)));
    let (mx, my, rot) = menu.map(|(_, mm)| (mm.tx / 20.0, mm.ty / 20.0, mm)).unwrap_or((214.0, 334.0, m));
    for (i, (name, _)) in PLAY_MODES.iter().enumerate() {
        let id = format!("play:mode:{i}");
        let (x, y) = (mx + 140.0 - i as f32 * 4.0, my + 60.0 + i as f32 * 56.0);
        let bm = dishonored_assets::swf::Matrix { a: rot.a, b: rot.b, c: rot.c, d: rot.d, tx: x * 20.0, ty: y * 20.0 };
        menu_ui::movie_button(dl, art, "m_nGame_btn", bm, &id, name, focus == id || l.play_mode == i, Vec2::new(300.0, 46.0));
    }
    // Description box (_description_mc text field).
    if let Some((mc, mm)) = menu {
        for f in DisplayList::fields(art, mc, mm, &States::default()) {
            dl.field_text(art, &f, PLAY_MODES[l.play_mode.min(PLAY_MODES.len() - 1)].1, Some(TEXT));
        }
    }
    if let Some(o) = opts {
        let y = 560.0;
        generic_line(dl, o, y, "play:bots", "Bots", &l.bots.to_string(), LineWidget::Stepper, focus == "play:bots");
        generic_line(dl, o, y + GENERIC_LINE_H, "play:start", &t.t(SEC_BASE, "t_Confirm"), &t.t(SEC_BASE, "t_Select"), LineWidget::Button, focus == "play:start");
    }
    back_hint(dl, t);
}

fn back_hint(dl: &mut DisplayList, t: &MenuTexts) {
    dl.text(format!("Esc   {}", t.t(SEC_BASE, "t_Back")), Rect::new(900.0, 660.0, 1180.0, 690.0), 22.0, FontKind::Normal, TEXT, Justify::Right);
    dl.item("back", Rect::new(1040.0, 660.0, 1180.0, 690.0), false);
}

fn build_missions(dl: &mut DisplayList, art: &MovieArt, opts: Option<&MovieArt>, l: &Launcher, t: &MenuTexts, focus: &str) {
    if let Some(o) = opts {
        crate::menu_options::backdrop(dl, o);
    }
    let title = t.t(SEC_MAIN, "t_Missions").to_uppercase();
    dl.text(&title, Rect::new(110.0, 50.0, 700.0, 100.0), 44.0, FontKind::Title, TEXT, Justify::Left);
    dl.text(t.t(SEC_MAIN, "t_MissionsScreenDesc"), Rect::new(110.0, 100.0, 900.0, 130.0), 22.0, FontKind::Normal, TEXT_DIM, Justify::Left);
    if l.missions.is_empty() {
        dl.text(t.t(SEC_BASE, "t_EmptyList"), Rect::new(0.0, 330.0, 1280.0, 370.0), 26.0, FontKind::Normal, TEXT, Justify::Center);
        back_hint(dl, t);
        return;
    }
    // Mission list in the options-line style (the original's card slide needs per-mission
    // artwork that custom missions do not have); the selected mission's briefing below.
    let _ = art;
    if let Some(o) = opts {
        let n = l.missions.len().min(8);
        let first = l.mission_sel.saturating_sub(7).min(l.missions.len() - n);
        for (vi, i) in (first..first + n).enumerate() {
            let me = &l.missions[i];
            let id = format!("missions:item:{i}");
            let label = format!("{:02}   {}", i + 1, me.title.to_uppercase());
            generic_line(dl, o, 190.0 + vi as f32 * GENERIC_LINE_H, &id, &label, &me.map, LineWidget::Button, focus == id || l.mission_sel == i);
        }
    }
    let me = &l.missions[l.mission_sel.min(l.missions.len() - 1)];
    dl.text(format!("{}  -  {}", me.title, me.map), Rect::new(110.0, 540.0, 1180.0, 570.0), 26.0, FontKind::Title, TEXT, Justify::Left);
    dl.paragraph(&me.briefing, Rect::new(110.0, 575.0, 1180.0, 650.0), 20.0, FontKind::Normal, TEXT, Justify::Left);
    if opts.is_some() {
        dl.text(format!("Enter   {}", t.t(SEC_BASE, "t_Select")), Rect::new(110.0, 660.0, 500.0, 690.0), 22.0, FontKind::Normal, TEXT, Justify::Left);
        dl.item("missions:launch", Rect::new(110.0, 660.0, 400.0, 690.0), false);
    }
    back_hint(dl, t);
}

/// The save list (`t_LoadGame`): newest first, the original's prefixes and date format.
fn build_load(dl: &mut DisplayList, opts: Option<&MovieArt>, l: &Launcher, t: &MenuTexts, focus: &str) {
    if let Some(o) = opts {
        crate::menu_options::backdrop(dl, o);
    }
    dl.text(t.t(SEC_MENU, "t_LoadGame").to_uppercase(), Rect::new(110.0, 50.0, 700.0, 100.0), 44.0, FontKind::Title, TEXT, Justify::Left);
    if l.saves.is_empty() {
        dl.text(t.t(SEC_BASE, "t_EmptyList"), Rect::new(0.0, 330.0, 1280.0, 370.0), 26.0, FontKind::Normal, TEXT, Justify::Center);
        back_hint(dl, t);
        return;
    }
    if let Some(o) = opts {
        let n = l.saves.len().min(9);
        let first = l.save_sel.saturating_sub(8).min(l.saves.len() - n);
        for (vi, i) in (first..first + n).enumerate() {
            let (_, h) = &l.saves[i];
            let id = format!("load:item:{i}");
            generic_line(dl, o, 170.0 + vi as f32 * GENERIC_LINE_H, &id, &crate::saves::label(h, t), &h.title, LineWidget::Button, focus == id || l.save_sel == i);
        }
    }
    back_hint(dl, t);
}

fn host_lines(l: &Launcher, gs: &GameSettings) -> Vec<(&'static str, String, String, LineWidget)> {
    let h = &gs.s.multiplayer.host;
    let mp = &gs.s.multiplayer;
    let mission = l.missions.iter().find(|m| m.path.file_name().is_some_and(|f| f.to_string_lossy() == h.mission)).map(|m| m.title.clone()).unwrap_or_else(|| l.missions.first().map(|m| m.title.clone()).unwrap_or("-".into()));
    let mut v = vec![
        ("host:name", "Server name".into(), h.name.clone(), LineWidget::Text { editing: false }),
        ("host:mode", "Mode".into(), match h.mode.as_str() { "contracts" => "Contracts".into(), "mission" => "Mission".into(), _ => "Free for all".into() }, LineWidget::Stepper),
    ];
    if h.mode == "mission" {
        v.push(("host:mission", "Mission".into(), mission, LineWidget::Stepper));
    } else {
        v.push(("host:map", "Map".into(), h.map.clone(), LineWidget::Stepper));
    }
    v.extend([
        ("host:port", "Port".into(), h.port.to_string(), LineWidget::Text { editing: false }),
        ("host:max", "Max players".into(), h.max_players.to_string(), LineWidget::Stepper),
        ("host:score", "Score limit".into(), h.score_limit.to_string(), LineWidget::Stepper),
        ("host:time", "Time limit".into(), format!("{} min", h.time_limit / 60), LineWidget::Stepper),
        ("host:register", "Register with master".into(), if h.register { "On" } else { "Off" }.into(), LineWidget::Stepper),
        ("host:master", "Master server".into(), mp.master.clone(), LineWidget::Text { editing: false }),
        ("host:localmaster", "Start local master".into(), if h.local_master { "On" } else { "Off" }.into(), LineWidget::Stepper),
    ]);
    let running = l.server.is_some();
    v.push(("host:start", if running { "Stop server".into() } else { "Start server".into() }, String::new(), LineWidget::Button));
    if running {
        v.push(("host:join", "Join my server".into(), String::new(), LineWidget::Button));
    }
    v
}

fn build_mp(dl: &mut DisplayList, o: &MovieArt, l: &Launcher, t: &MenuTexts, gs: &GameSettings, nav: &MenuNav) {
    let focus = nav.focus.as_deref().unwrap_or("");
    let editing = nav.editing.as_ref();
    crate::menu_options::backdrop(dl, o);
    dl.text("MULTIPLAYER", Rect::new(110.0, 28.0, 700.0, 70.0), 38.0, FontKind::Title, TEXT, Justify::Left);
    // Join / Host tabs in the options movie's tab style.
    for (i, (id, label)) in [("mp:tab:join", "JOIN"), ("mp:tab:host", "HOST")].iter().enumerate() {
        let selected = (i == 1) == l.mp_host;
        let state = if selected { "selected" } else if focus == *id { "over" } else { "unselected" };
        if let Some(ch) = o.export("SubTab") {
            let x = 535.0 + i as f32 * 210.0;
            let mm = px_matrix(x, 105.0);
            let st = States::all(state);
            dl.movie(o, ch, mm, &st, 1.0);
            for f in DisplayList::fields(o, ch, mm, &st) {
                dl.field_text(o, &f, label, Some(if selected || focus == *id { TEXT } else { TEXT_DIM }));
            }
            dl.item(*id, Rect::from_center_size(Vec2::new(x, 105.0), Vec2::new(150.0, 40.0)), true);
        }
    }
    let ed = |id: &str| editing.filter(|e| e.0 == id).map(|e| e.1.clone());
    if !l.mp_host {
        let mp = &gs.s.multiplayer;
        let mut y = 170.0;
        for (id, label, value) in [("join:name", "Player name", mp.name.clone()), ("join:master", "Master server", mp.master.clone()), ("join:direct", "Direct connect", mp.last_connect.clone())] {
            let e = ed(id);
            generic_line(dl, o, y, id, label, e.as_deref().unwrap_or(&value), LineWidget::Text { editing: e.is_some() }, focus == id);
            y += GENERIC_LINE_H;
        }
        generic_line(dl, o, y, "join:connect", "Connect", &t.t(SEC_BASE, "t_Select"), LineWidget::Button, focus == "join:connect");
        y += GENERIC_LINE_H;
        generic_line(dl, o, y, "join:refresh", "Refresh server list", &l.browse_status, LineWidget::Button, focus == "join:refresh");
        y += GENERIC_LINE_H + 10.0;
        // Server list.
        // Columns: server, map, mode, players, ping.
        const COLS: [f32; 6] = [LIST_X_TEXT, 580.0, 800.0, 940.0, 1050.0, 1150.0];
        let row = |dl: &mut DisplayList, y: f32, cells: [String; 5], size: f32, col: Color| {
            for (i, c) in cells.into_iter().enumerate() {
                dl.text(c, Rect::new(COLS[i], y - 14.0, COLS[i + 1] - 10.0, y + 14.0), size, FontKind::Normal, col, Justify::Left);
            }
        };
        row(dl, y, ["SERVER".into(), "MAP".into(), "MODE".into(), "PLAYERS".into(), "PING".into()], 18.0, TEXT_DIM);
        y += 34.0;
        if l.servers.is_empty() {
            dl.text(t.t(SEC_BASE, "t_EmptyList"), Rect::new(LIST_X_TEXT, y - 14.0, 1180.0, y + 14.0), 20.0, FontKind::Normal, TEXT, Justify::Left);
        }
        for (i, s) in l.servers.iter().enumerate().take(6) {
            let id = format!("join:server:{i}");
            let ping = s.ping_ms.map(|p| format!("{p:.0} ms")).unwrap_or_else(|| "-".into());
            if focus == id {
                dl.fill(Rect::new(LIST_X_TEXT - 10.0, y - 16.0, 1150.0, y + 16.0), Color::srgba(0.89, 0.95, 0.84, 0.15));
            }
            row(dl, y, [truncate(&s.name, 34), truncate(&s.map, 20), s.mode.clone(), format!("{}/{}", s.players, s.max_players), ping], 20.0, TEXT);
            dl.item(&id, Rect::new(LIST_X_TEXT - 10.0, y - 16.0, 1150.0, y + 16.0), true);
            y += 34.0;
        }
    } else {
        let lines = host_lines(l, gs);
        let mut y = 160.0;
        for (id, label, value, w) in &lines {
            let e = ed(id);
            let w = match w {
                LineWidget::Text { .. } => LineWidget::Text { editing: e.is_some() },
                w => *w,
            };
            let v = if *id == "host:start" { l.server_status.clone() } else { e.unwrap_or_else(|| value.clone()) };
            generic_line(dl, o, y, id, label, &v, w, focus == *id);
            y += GENERIC_LINE_H - 8.0;
        }
        // Server log tail.
        if let Some(s) = &l.server {
            let tail = s.tail(4).join("\n");
            dl.text(tail, Rect::new(LIST_X_TEXT, y, 1180.0, y + 80.0), 15.0, FontKind::Normal, TEXT_DIM, Justify::Left);
        }
    }
    back_hint(dl, t);
}

const LIST_X_TEXT: f32 = 180.0;

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

#[allow(clippy::too_many_arguments)]
fn rebuild(
    mut commands: Commands,
    mut l: ResMut<Launcher>,
    art: Option<Res<MenuArt>>,
    t: Res<MenuTexts>,
    gs: Res<GameSettings>,
    mut nav: ResMut<MenuNav>,
    bg: Option<Res<MenuBackground>>,
    fonts: Option<Res<crate::gfx_hud::GameFonts>>,
    time: Res<Time<Real>>,
    mut sounds: ResMut<MenuSounds>,
    mut last: Local<(Option<String>, Option<(String, String)>, Option<String>, &'static str, bool)>,
) {
    let Some(art) = art else { return };
    let Some(root) = l.root else { return };
    if l.page == Page::Main && l.opened_at.is_none() && !l.in_game() {
        // `PlayOpenAnimation`: on first show, on return from a screen or a game.
        let now = time.elapsed_secs_f64();
        l.opened_at = Some(now);
        l.first_drawn.get_or_insert(now);
        sounds.play("M_OpenMainMenu");
        l.dirty = true;
    }
    let bgs = bg.map(|b| b.state).unwrap_or("movie");
    let key = (nav.focus.clone(), nav.editing.clone(), nav.capturing.clone(), bgs, fonts.is_some());
    if !l.dirty && *last == key {
        return;
    }
    *last = key;
    l.dirty = false;
    if l.page == Page::Main && !l.game_running() {
        // Continue / Load appear once a save exists (headers only).
        l.saves = dishonored_save::list(&saves_dir(&gs));
    }
    let dl = build(&l, &art, &t, &gs, &nav, bgs, open_t(&l, &time));
    nav.set_items(dl.items.clone());
    menu_art::spawn_list(&mut commands, root, &dl, fonts.as_deref());
}

#[allow(clippy::too_many_arguments)]
fn handle(
    mut l: ResMut<Launcher>,
    mut nav: ResMut<MenuNav>,
    mut gs: ResMut<GameSettings>,
    mut sounds: ResMut<MenuSounds>,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    monitors: Query<&bevy::window::Monitor>,
    mut exit: MessageWriter<AppExit>,
    main_thread: NonSendMarker,
) {
    l.poll();
    if l.options.resolutions.is_empty() {
        let mut res: Vec<(u32, u32)> = monitors.iter().flat_map(|m| m.video_modes.iter().map(|v| (v.physical_size.x, v.physical_size.y))).filter(|r| r.0 >= 1024).collect();
        res.sort();
        res.dedup();
        l.options.resolutions = res;
    }
    let events = std::mem::take(&mut nav.events);
    for ev in events {
        if l.game_running() {
            continue;
        }
        let window = windows.iter().next().map(|(e, w)| handoff::placement(e, w, &main_thread));
        if l.page == Page::Options {
            let o = &mut l.options;
            match o.handle(&ev, &mut gs, &mut nav, &mut sounds) {
                Outcome::Close => {
                    l.page = Page::Main;
                    l.opened_at = None;
                    nav.focus = Some("main:options".into());
                }
                Outcome::None => {}
                Outcome::Redraw | Outcome::Changed => {}
            }
            l.dirty = true;
            continue;
        }
        match ev {
            MenuEvent::Focus(_) => sounds.play("Move"),
            MenuEvent::Back => {
                if l.page != Page::Main {
                    sounds.play("back");
                    let from = l.page;
                    l.page = Page::Main;
                    l.opened_at = None;
                    nav.focus = Some(format!("main:{}", from.name()));
                    l.dirty = true;
                }
            }
            MenuEvent::Scroll(_) => {}
            MenuEvent::Captured { .. } => {}
            MenuEvent::TextCommit { id, text } => {
                commit_text(&mut gs, &id, &text);
                l.dirty = true;
            }
            MenuEvent::Step { id, dir } => step(&mut l, &mut gs, &id, dir, &mut sounds),
            MenuEvent::Activate { id, .. } => activate(&mut l, &mut gs, &mut nav, &id, &mut sounds, window, &mut exit),
        }
    }
}

fn commit_text(gs: &mut GameSettings, id: &str, text: &str) {
    let mp = &mut gs.s.multiplayer;
    let text = text.trim().to_string();
    match id {
        "join:name" if !text.is_empty() => mp.name = text,
        "join:master" | "host:master" if !text.is_empty() => mp.master = text,
        "join:direct" if !text.is_empty() => mp.last_connect = text,
        "host:name" if !text.is_empty() => mp.host.name = text,
        "host:port" => {
            if let Ok(p) = text.parse::<u16>() {
                if p > 0 {
                    mp.host.port = p;
                }
            }
        }
        _ => return,
    }
    gs.commit();
}

fn step(l: &mut Launcher, gs: &mut GameSettings, id: &str, dir: i32, sounds: &mut MenuSounds) {
    let id = id.trim_end_matches(":-1").trim_end_matches(":+1");
    let h = &mut gs.s.multiplayer.host;
    let wrap = |i: usize, n: usize| (i as i32 + dir).rem_euclid(n.max(1) as i32) as usize;
    match id {
        "play:bots" => l.bots = (l.bots as i32 + dir).clamp(0, 15) as u32,
        "host:mode" => {
            let i = HOST_MODES.iter().position(|m| *m == h.mode).unwrap_or(0);
            h.mode = HOST_MODES[wrap(i, HOST_MODES.len())].into();
        }
        "host:map" => {
            let i = l.maps.iter().position(|m| *m == h.map).unwrap_or(0);
            h.map = l.maps[wrap(i, l.maps.len())].clone();
        }
        "host:mission" if !l.missions.is_empty() => {
            let i = l.missions.iter().position(|m| m.path.file_name().is_some_and(|f| f.to_string_lossy() == h.mission)).unwrap_or(0);
            h.mission = l.missions[wrap(i, l.missions.len())].path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        }
        "host:max" => h.max_players = (h.max_players as i32 + dir).clamp(2, 32) as u32,
        "host:score" => h.score_limit = (h.score_limit as i32 + dir * 5).clamp(5, 100) as u32,
        "host:time" => h.time_limit = (h.time_limit as i32 + dir * 60).clamp(60, 3600) as u32,
        "host:register" => h.register = !h.register,
        "host:localmaster" => h.local_master = !h.local_master,
        "mp:tab:join" | "mp:tab:host" => {
            l.mp_host = !l.mp_host;
            sounds.play("ChangeMainTab");
            l.dirty = true;
            return;
        }
        x if x.starts_with("play:mode:") => l.play_mode = wrap(l.play_mode, PLAY_MODES.len()),
        x if x.starts_with("missions:item:") && !l.missions.is_empty() => l.mission_sel = wrap(l.mission_sel, l.missions.len()),
        _ => return,
    }
    if id.starts_with("host:") {
        gs.commit();
    }
    sounds.play("change");
    l.dirty = true;
}

#[allow(clippy::too_many_arguments)]
fn activate(l: &mut Launcher, gs: &mut GameSettings, nav: &mut MenuNav, id: &str, sounds: &mut MenuSounds, window: Option<handoff::Placement>, exit: &mut MessageWriter<AppExit>) {
    l.dirty = true;
    // Arrow items of stepper lines.
    if id.ends_with(":-1") || id.ends_with(":+1") {
        return step(l, gs, id, if id.ends_with(":-1") { -1 } else { 1 }, sounds);
    }
    let goto = |l: &mut Launcher, nav: &mut MenuNav, p: Page, sounds: &mut MenuSounds| {
        l.page = p;
        nav.focus = None;
        sounds.play("validation");
    };
    match id {
        "main:play" => goto(l, nav, Page::Play, sounds),
        "main:missions" => goto(l, nav, Page::Missions, sounds),
        // [playground] The test level, offline, every power and item.
        "main:playground" => {
            sounds.play("validation");
            l.launch(vec!["--playground".into(), "--name".into(), gs.s.multiplayer.name.clone()], gs, window, "");
        }
        "main:load" => {
            l.saves = dishonored_save::list(&saves_dir(gs));
            l.save_sel = 0;
            goto(l, nav, Page::Load, sounds)
        }
        "main:continue" => {
            // `OnLoadLastSaveClicked`: the newest save.
            l.saves = dishonored_save::list(&saves_dir(gs));
            if let Some((p, h)) = l.saves.first().cloned() {
                sounds.play("validation");
                l.launch(vec!["--load".into(), p.display().to_string(), "--name".into(), gs.s.multiplayer.name.clone()], gs, window, &h.map);
            }
        }
        x if x.starts_with("load:item:") => {
            let i: usize = x.rsplit(':').next().and_then(|i| i.parse().ok()).unwrap_or(0);
            if i == l.save_sel {
                if let Some((p, h)) = l.saves.get(i).cloned() {
                    sounds.play("validation");
                    l.launch(vec!["--load".into(), p.display().to_string(), "--name".into(), gs.s.multiplayer.name.clone()], gs, window, &h.map);
                }
            } else {
                l.save_sel = i;
                sounds.play("change");
            }
        }
        "main:multiplayer" => goto(l, nav, Page::Multiplayer, sounds),
        "main:options" => {
            goto(l, nav, Page::Options, sounds);
            l.options.secondary = None;
        }
        "main:quit" => {
            sounds.play("validation");
            exit.write(AppExit::Success);
        }
        "back" => {
            sounds.play("back");
            let from = l.page;
            l.page = Page::Main;
            l.opened_at = None;
            nav.focus = Some(format!("main:{}", from.name()));
        }
        "play:start" => {
            sounds.play("validation");
            let mut args = vec!["--name".to_string(), gs.s.multiplayer.name.clone()];
            match l.play_mode {
                1 => args.extend(["--mode".into(), "contracts".into(), "--bots".into(), l.bots.max(1).to_string()]),
                2 => args.extend(["--spectate".into(), "--bots".into(), l.bots.to_string()]),
                _ => args.extend(["--bots".into(), l.bots.to_string()]),
            }
            l.launch(args, gs, window, dishonored_content::DEFAULT_MAP);
        }
        "missions:launch" => {
            if let Some(m) = l.missions.get(l.mission_sel).cloned() {
                sounds.play("validation");
                l.launch(vec!["--mission".into(), m.path.display().to_string(), "--name".into(), gs.s.multiplayer.name.clone()], gs, window, &m.map);
            }
        }
        "mp:tab:join" => {
            if l.mp_host {
                l.mp_host = false;
                sounds.play("ChangeMainTab");
            }
        }
        "mp:tab:host" => {
            if !l.mp_host {
                l.mp_host = true;
                sounds.play("ChangeMainTab");
            }
        }
        "join:refresh" => {
            sounds.play("validation");
            let m = gs.s.multiplayer.master.clone();
            l.refresh(&m);
        }
        "join:connect" => {
            sounds.play("validation");
            let addr = gs.s.multiplayer.last_connect.clone();
            l.launch(vec!["--connect".into(), addr, "--name".into(), gs.s.multiplayer.name.clone()], gs, window, "");
        }
        "host:start" => {
            sounds.play("validation");
            if l.server.is_some() {
                l.stop_server();
            } else {
                l.start_server(gs);
            }
        }
        "host:join" => {
            sounds.play("validation");
            let addr = format!("127.0.0.1:{}", gs.s.multiplayer.host.port);
            let h = &gs.s.multiplayer.host;
            let map = if h.mode == "mission" { l.missions.iter().find(|m| m.path.file_name().is_some_and(|f| f.to_string_lossy() == h.mission)).or(l.missions.first()).map(|m| m.map.clone()).unwrap_or_default() } else { h.map.clone() };
            l.launch(vec!["--connect".into(), addr, "--name".into(), gs.s.multiplayer.name.clone()], gs, window, &map);
        }
        x if x.starts_with("join:server:") => {
            if let Some(s) = x.rsplit(':').next().and_then(|i| i.parse::<usize>().ok()).and_then(|i| l.servers.get(i)).cloned() {
                sounds.play("validation");
                l.launch(vec!["--connect".into(), s.addr, "--name".into(), gs.s.multiplayer.name.clone()], gs, window, &s.map);
            }
        }
        x if x.starts_with("play:mode:") => {
            l.play_mode = x.rsplit(':').next().and_then(|i| i.parse().ok()).unwrap_or(0);
            sounds.play("change");
        }
        x if x.starts_with("missions:item:") => {
            let i: usize = x.rsplit(':').next().and_then(|i| i.parse().ok()).unwrap_or(0);
            if i == l.mission_sel {
                activate(l, gs, nav, "missions:launch", sounds, window, exit);
            } else {
                l.mission_sel = i;
                sounds.play("change");
            }
        }
        x if matches!(x, "join:name" | "join:master" | "join:direct" | "host:name" | "host:port" | "host:master") => {
            let mp = &gs.s.multiplayer;
            let cur = match x {
                "join:name" => mp.name.clone(),
                "join:master" | "host:master" => mp.master.clone(),
                "join:direct" => mp.last_connect.clone(),
                "host:name" => mp.host.name.clone(),
                _ => mp.host.port.to_string(),
            };
            nav.editing = Some((x.to_string(), cur));
            sounds.play("validation");
        }
        x if x.starts_with("host:") || x == "play:bots" => step(l, gs, x, 1, sounds),
        _ => {}
    }
}

/// Harness control socket (`--control PORT`).
#[allow(clippy::too_many_arguments)]
fn control(
    mut commands: Commands,
    rx: Option<Res<crate::harness::ControlRx>>,
    mut nav: ResMut<MenuNav>,
    l: Res<Launcher>,
    gs: Res<GameSettings>,
    bg: Option<Res<MenuBackground>>,
    sounds: Res<MenuSounds>,
    mut script: ResMut<LauncherScript>,
    loading: Res<crate::loading_screen::LoadingScreen>,
    time: Res<Time<Real>>,
) {
    let Some(rx) = rx else { return };
    let rx = rx.0.lock().unwrap();
    while let Ok(msg) = rx.try_recv() {
        let v: serde_json::Value = serde_json::from_str(&msg.line).unwrap_or_default();
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let reply = match v.get("cmd").and_then(|c| c.as_str()).unwrap_or("") {
            "state" => state_json(&l, &gs, &nav, bg.as_deref(), &sounds, &loading, &time).to_string(),
            "activate" => {
                nav.events.push(MenuEvent::Activate { id, at: None });
                "{\"ok\":true}".into()
            }
            "step" => {
                nav.events.push(MenuEvent::Step { id, dir: v.get("dir").and_then(|d| d.as_i64()).unwrap_or(1) as i32 });
                "{\"ok\":true}".into()
            }
            "back" => {
                nav.events.push(MenuEvent::Back);
                "{\"ok\":true}".into()
            }
            "text" => {
                nav.editing = None;
                nav.events.push(MenuEvent::TextCommit { id, text: v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string() });
                "{\"ok\":true}".into()
            }
            "capture" => {
                nav.capturing = None;
                nav.events.push(MenuEvent::Captured { id, key: v.get("key").and_then(|x| x.as_str()).map(String::from) });
                "{\"ok\":true}".into()
            }
            "screenshot" => {
                let p = PathBuf::from(v.get("path").and_then(|x| x.as_str()).unwrap_or("launcher.png"));
                script.shots.push(p);
                "{\"ok\":true}".into()
            }
            "quit" => {
                script.quit_in = Some(10);
                "{\"ok\":true}".into()
            }
            c => format!("{{\"error\":\"unknown cmd '{c}'\"}}"),
        };
        let _ = msg.reply.send(reply);
    }
    let _ = &mut commands;
}

fn state_json(l: &Launcher, gs: &GameSettings, nav: &MenuNav, bg: Option<&MenuBackground>, sounds: &MenuSounds, loading: &crate::loading_screen::LoadingScreen, time: &Time<Real>) -> serde_json::Value {
    serde_json::json!({
        "page": l.page.name(),
        "options": { "category": format!("{:?}", l.options.category()), "sub": format!("{:?}", l.options.sub_category()), "secondary": l.options.secondary.map(|s| format!("{s:?}")) },
        "focus": nav.focus,
        "editing": nav.editing.as_ref().map(|e| &e.0),
        "capturing": nav.capturing,
        "items": nav.items.iter().map(|i| &i.id).collect::<Vec<_>>(),
        "settings_path": gs.path.display().to_string(),
        "settings": serde_json::to_value(&gs.s).unwrap_or_default(),
        "background": bg.map(|b| b.state).unwrap_or("movie"),
        "game_running": l.game_running(),
        "in_game": l.in_game(),
        "launcher_visible": !l.hidden,
        "game_window_shown": l.game.as_ref().is_some_and(|g| g.handoff().shown),
        "loading_screen": loading.active,
        "timing": { "uptime_s": time.elapsed_secs(), "menu_drawn_s": l.first_drawn, "input_ready_s": l.first_input, "hitches": l.hitches },
        "last_game_exit": l.last_game_exit,
        "server": { "status": l.server_status, "log": l.server.as_ref().map(|s| s.tail(20)).unwrap_or_default(), "what": l.server.as_ref().map(|s| s.what.clone()) },
        "master_running": l.master.is_some(),
        "servers": l.servers,
        "browse_status": l.browse_status,
        "missions": l.missions.iter().map(|m| serde_json::json!({"title": m.title, "map": m.map, "path": m.path.display().to_string()})).collect::<Vec<_>>(),
        "sounds": { "loaded": sounds.loaded, "played": sounds.played },
    })
}

/// `--script` runner (one step per frame, `wait_frames` pauses) and screenshot pacing.
fn run_script(mut commands: Commands, mut s: ResMut<LauncherScript>, mut nav: ResMut<MenuNav>, bg: Option<Res<MenuBackground>>, mut exit: MessageWriter<AppExit>) {
    if let Some(p) = (!s.shots.is_empty()).then(|| s.shots.remove(0)) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        return;
    }
    if let Some(n) = s.quit_in.as_mut() {
        if *n == 0 {
            exit.write(AppExit::Success);
        } else {
            *n -= 1;
        }
        return;
    }
    if s.wait > 0 {
        s.wait -= 1;
        return;
    }
    let Some(step) = s.steps.get(s.index).cloned() else { return };
    if matches!(step, LStep::WaitLevel { .. }) && bg.as_ref().is_some_and(|b| b.state == "loading") {
        return;
    }
    s.index += 1;
    match step {
        LStep::Activate { activate } => nav.events.push(MenuEvent::Activate { id: activate, at: None }),
        LStep::Step { step: (id, dir) } => nav.events.push(MenuEvent::Step { id, dir }),
        LStep::Back { .. } => nav.events.push(MenuEvent::Back),
        LStep::Text { text: (id, t) } => nav.events.push(MenuEvent::TextCommit { id, text: t }),
        LStep::Screenshot { screenshot } => {
            let p = s.out.join(format!("{screenshot}.png"));
            s.shots.push(p);
            s.wait = 2;
        }
        LStep::Wait { wait_frames } => s.wait = wait_frames,
        LStep::WaitLevel { .. } => s.wait = 30,
        LStep::Quit { .. } => s.quit_in = Some(20),
    }
}
