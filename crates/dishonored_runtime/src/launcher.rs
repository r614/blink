//! Standalone launcher: what `dishonored_runtime` opens when started without a game-mode
//! argument (or with `--launcher`). It is the original front end - the `Dishonored_MainMenu`
//! level behind the `UI_MainMenu.MainMenu` movie, the menu music and the UI sound theme - with
//! the original's screens and this port's entries:
//!
//! - the start screen (`startScreen_mc`, "press any key");
//! - the main menu bar (`MainMenuButtonBar`): Continue (newest save), **Play** (offline match:
//!   free for all, contracts, spectate, the Playground; map and bots), **Missions** (custom
//!   missions, on the original Missions screen's card slide), **Free Roam** (any of the 42 base
//!   maps with its original Kismet, on the same card slide), Load, **Multiplayer** (server
//!   browser and server creator), Options (every original setting, `menu_options.rs`), Quit
//!   (`t_Q_QuitGame` confirmation);
//! - the confirmation box of `UI_Global` (`menu_widgets.rs`).
//!
//! Every screen opens, closes and reacts with the movies' own timings (`PlayOpenAnimation`,
//! `MainMenuButtonBar`, `GenericMenu`, `NewGameMenu`, `MissionsMenu` / `SlideMenu`, ...; see
//! `research/dishonored/MENUS.md`). The main menu takes input 0.58 s after it opens.
//!
//! Games run as a child `dishonored_runtime` process with the matching CLI arguments plus
//! `--from-launcher` (`--window-pos` / `--size` of the launcher window, fullscreen from the
//! settings). The player sees one window (`handoff.rs`): the launcher shows the original
//! loading screen for the map, the game's window opens hidden in the same place and replaces
//! it once it renders, and the launcher hides; the pause menu's "Back to Main Menu" (or the
//! game ending) shows the launcher's main menu in its place again, "Back to Windows" quits.
//! `DISHONORED_LAUNCHER_CHILD_ARGS` (whitespace-separated) is prepended to every child's
//! arguments; the harness uses it to reach the game's control socket.
//!
//! Automation: `--control PORT` (JSON lines: `state`, `activate`, `step`, `back`, `button`,
//! `text`, `capture`, `screenshot`, `quit`) and `--script FILE` (`{"steps":[{"activate":
//! "main:options"},{"wait_secs":1.0},{"screenshot":"options"},{"quit":true}]}`), `--out DIR`.
//! Activations that arrive while the screen does not take input yet (opening animations) are
//! held until it does.

use crate::launcher_bg::{self, MenuBackground};
use crate::menu_anim::{Act, Anim, Ease, P};
use crate::menu_art::{self, DisplayList, FontKind, LayerCache, MovieArt, Props, States, TEXT, TEXT_DIM, px_matrix};
use crate::menu_audio::{MenuAudioPlugin, MenuSounds};
use crate::menu_maps::{self, MapEntry};
use crate::menu_options::{GENERIC_LINE_H, LineWidget, OptionsMenu, Outcome, generic_line_at};
use crate::menu_ui::{self, MenuArt, MenuEvent, MenuNav, MenuTexts, SEC_BASE, SEC_MAIN, SEC_MENU};
use crate::menu_widgets::{HelpBar, MsgBox, TitleBar, author_tree, chain, help_list, texts_any};
use crate::settings::GameSettings;
use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::window::{PrimaryWindow, WindowMode, WindowPosition};
use bevy::winit::{UpdateMode, WinitSettings};
use crate::handoff;
use dishonored_assets::swf::Matrix;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    /// "Press any key" (`startScreen_mc`).
    Start,
    Main,
    Play,
    Missions,
    FreeRoam,
    Multiplayer,
    Options,
    /// The save list (`UI_LoadGame.LoadGame`, `t_LoadGame`).
    Load,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Start => "start",
            Page::Main => "main",
            Page::Play => "play",
            Page::Missions => "missions",
            Page::FreeRoam => "freeroam",
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
    /// Back to Windows: quit.
    quit: bool,
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
                    } else if line.contains(handoff::QUIT) {
                        marks.lock().unwrap().quit = true;
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

/// What a confirmed main-menu entry does once the confirm delay (150 ms) is over.
#[derive(Clone, Debug)]
struct Pending {
    at: f64,
    id: String,
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
    cache: LayerCache,
    /// The launcher window is hidden while the game's window stands in for it.
    hidden: bool,
    /// Map whose loading screen goes up (set by [`Launcher::launch`]).
    begin_loading: Option<String>,
    /// When the main menu last opened (`PlayOpenAnimation`), real seconds.
    opened_at: Option<f64>,
    /// When the main menu started closing (a screen was chosen).
    main_closed_at: Option<f64>,
    /// Real seconds at which the menu was first drawn / first took input (harness timings).
    first_drawn: Option<f64>,
    first_input: Option<f64>,
    /// Frames slower than [`HITCH_MS`] after the menu was first drawn: (seconds, ms).
    hitches: Vec<(f32, f32)>,
    /// The 42 base maps (free roam, the offline match, the server creator).
    pub maps: Vec<MapEntry>,
    /// Saved games, newest first (refreshed on the main and load pages).
    pub saves: Vec<(PathBuf, dishonored_save::SaveHeader)>,
    pub save_sel: usize,
    /// Clips of the launcher's screens (main menu, sub screens, message box).
    pub anim: Anim,
    /// Main menu selection (`_curSelectionIdx`) and the button ids of the current bar.
    main_sel: usize,
    main_ids: Vec<&'static str>,
    pending: Option<Pending>,
    /// The screen takes input from this time (opening animations, confirm delays).
    input_from: f64,
    /// When the current page opened, and the page that is closing (drawn until its close ends).
    page_at: f64,
    closing_page: Option<(Page, f64)>,
    msg: Option<MsgBox>,
    msg_focus: (usize, f64),
    /// Free roam selection.
    pub freeroam_sel: usize,
    /// Card pictures (`UI_Mission_Large`), by image name.
    card_images: HashMap<String, (Handle<Image>, Vec2)>,
    /// Activations received while the screen did not take input (automation), run when it does.
    deferred: Vec<MenuEvent>,
    /// Focus last seen (focus-in / focus-out timelines of the sub screens).
    last_focus: Option<String>,
    now: f64,
}

/// The game's saves folder: `SaveData` next to the settings file the launcher hands the game.
fn saves_dir(gs: &GameSettings) -> PathBuf {
    gs.path.parent().map(|p| p.join("SaveData")).unwrap_or_else(dishonored_save::saves_dir)
}

/// Offline modes of the Play screen: (label, description, needs a map).
const PLAY_MODES: &[(&str, &str, bool)] = &[
    ("FREE FOR ALL", "Deathmatch against bots. First to the score limit wins.", true),
    ("CONTRACTS", "Every player has one target and one pursuer among the party guests. Observe, identify, stalk, assassinate, escape.", true),
    ("SPECTATE", "Watch the bots play a free-for-all match from their point of view.", true),
    ("PLAYGROUND", "The test level, offline: every power and item.", false),
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
    fn new(gs: &GameSettings) -> Self {
        let missions = load_missions();
        let maps = menu_maps::catalog();
        let lp = &gs.s.launcher;
        let freeroam_sel = maps.iter().position(|m| m.map.eq_ignore_ascii_case(&lp.freeroam_map)).unwrap_or(0);
        Self {
            page: Page::Start,
            options: OptionsMenu::default(),
            play_mode: lp.play_mode.min(PLAY_MODES.len() - 1),
            bots: lp.bots,
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
            cache: LayerCache::default(),
            hidden: false,
            begin_loading: None,
            opened_at: None,
            main_closed_at: None,
            first_drawn: None,
            first_input: None,
            hitches: Vec::new(),
            maps,
            saves: Vec::new(),
            save_sel: 0,
            anim: Anim::new(0x5eed),
            main_sel: 0,
            main_ids: Vec::new(),
            pending: None,
            input_from: f64::INFINITY,
            page_at: 0.0,
            closing_page: None,
            msg: None,
            msg_focus: (0, 0.0),
            freeroam_sel,
            card_images: HashMap::new(),
            deferred: Vec::new(),
            last_focus: None,
            now: 0.0,
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

    /// Returns true when the game exited (the settings file may have changed).
    fn poll(&mut self) -> bool {
        let got = self.browse_rx.as_ref().and_then(|rx| rx.lock().unwrap().try_recv().ok());
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
        let mut exited = false;
        if let Some(g) = &mut self.game {
            if let Ok(Some(st)) = g.child.try_wait() {
                info!("game exited: {st}");
                self.last_game_exit = st.code();
                self.game = None;
                self.dirty = true;
                exited = true;
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
        exited
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
    /// A pad button: "X", "LB", "RB", "LT", "RT".
    Button { button: String },
    /// Move the focus to an item (as hovering it does).
    Focus { focus: String },
    Text { text: (String, String) },
    Screenshot { screenshot: String },
    Wait { wait_frames: u32 },
    /// Wait real seconds (timeline-exact screenshots).
    WaitSecs { wait_secs: f32 },
    /// Wait until the menu level has loaded (or failed).
    WaitLevel { wait_level: bool },
    /// Wait until the screen takes input (its opening animation is over).
    WaitInput { wait_input: bool },
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
    wait_until: Option<f64>,
    out: PathBuf,
    quit_in: Option<u32>,
    shots: Vec<PathBuf>,
    /// Screenshots taken: (name, real seconds, page, seconds since the page opened).
    log: Vec<(String, f64, &'static str, f64)>,
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
    let launcher = Launcher::new(&gs);
    crate::warmup::configure_pipeline_cache();
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
    .add_plugins(crate::warmup::WarmupPlugin)
    .add_plugins(MenuAudioPlugin { register_source: true, music: true })
    .insert_resource(crate::audio::global_volume())
    .insert_resource(ClearColor(Color::srgb(0.05, 0.055, 0.06)))
    .insert_resource(GlobalAmbientLight { color: Color::srgb(0.85, 0.88, 1.0), brightness: 60.0, affects_lightmapped_meshes: false })
    .insert_resource(crate::map_render::RenderFeatures::from_flags(&|f: &str| std::env::args().any(|a| a == f)))
    .init_resource::<crate::atmosphere::FogLayers>()
    .insert_resource(crate::settings::Bindings::from_settings(&gs.s))
    .insert_resource(gs)
    .insert_resource(MenuTexts::load())
    .insert_resource(MenuNav { enabled: false, ..default() })
    .init_resource::<crate::loading_screen::LoadingScreen>()
    .insert_resource(launcher);
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
        .add_systems(Startup, (move |c: Commands, f: Res<crate::map_render::RenderFeatures>| launcher_bg::start(c, level, f), setup, crate::gfx_hud::load_game_fonts))
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
                menu_art::black_stripes,
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
    commands.insert_resource(menu_ui::load_menu_art(&["main", "options", "gamma", "common", "global", "load"], &mut images));
    let root = menu_art::spawn_layer(&mut commands, 10);
    commands.entity(root).insert(menu_art::DrawsBlackStripes);
    l.root = Some(root);
    // Mission card pictures (`UI_Mission_Large.MissionsScreen_<image>_Large`).
    if let Ok(content) = dishonored_assets::Content::locate() {
        let names: Vec<String> = l.maps.iter().filter_map(|m| m.image.clone()).collect();
        for n in names {
            if l.card_images.contains_key(&n) {
                continue;
            }
            if let Some(t) = menu_art::load_texture(&content, "UI_Mission_Large", &format!("MissionsScreen_{n}_Large"), &mut images) {
                l.card_images.insert(n, t);
            }
        }
    }
}

/// The main menu's opening (`MainMenu.PlayOpenAnimation`, class 291): after a 180 ms interval
/// `OpenMenu` slides the button band in from `width + 150` px to the right (0.35 s); the bar's
/// `Open` then waits 400 ms before `InitInputs`. Until then the menu takes no input.
const OPEN_BAND_DELAY: f64 = 0.18;
const OPEN_INPUT_DELAY: f64 = 0.18 + 0.40;
/// `GenericMenu.Confirm`: the choice runs 150 ms after A (`OnPlayerChoiceConfirm`).
const CONFIRM_DELAY: f64 = 0.15;
/// A frame this slow after the menu is drawn counts as a hitch (harness timing evidence).
const HITCH_MS: f32 = 100.0;
/// `_offsetOriginX = 10 + 0.1 * Stage.width`.
const BAR_ORIGIN_X: f32 = 10.0 + 0.1 * 1280.0;

/// Real frame times once the menu is drawn: slow frames are where clicks wait.
fn frame_timing(time: Res<Time<Real>>, mut l: ResMut<Launcher>) {
    let dt = time.delta_secs() * 1000.0;
    if l.first_drawn.is_some() && !l.hidden && dt > HITCH_MS && l.hitches.len() < 64 {
        let at = time.elapsed_secs();
        l.hitches.push((at, dt));
    }
}

/// Whether the current screen takes input now (opening animations, confirm delays, the
/// message box's 250 ms).
fn input_ready(l: &Launcher, now: f64) -> bool {
    if l.game_running() {
        return false;
    }
    if let Some(m) = &l.msg {
        return m.input_ready(now);
    }
    l.pending.is_none() && now >= l.input_from
}

/// The input lock and the list kind of the current screen.
fn menu_lock(time: Res<Time<Real>>, mut l: ResMut<Launcher>, mut nav: ResMut<MenuNav>) {
    let now = time.elapsed_secs_f64();
    l.now = now;
    let enabled = input_ready(&l, now);
    if enabled && l.first_input.is_none() && l.page == Page::Main {
        l.first_input = Some(now);
    }
    if nav.enabled != enabled {
        nav.enabled = enabled;
    }
    let msg = l.msg.is_some();
    nav.horizontal = msg || matches!(l.page, Page::Main | Page::Missions | Page::FreeRoam);
    nav.wrap = !msg && matches!(l.page, Page::Main | Page::Play);
    // Queued script steps (staggers, intervals).
    l.anim.advance(now);
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
    art: Option<Res<MenuArt>>,
    t: Res<MenuTexts>,
    gs: Res<GameSettings>,
    mut exit: MessageWriter<AppExit>,
) {
    if let Some(map) = l.begin_loading.take() {
        // `m_pStopMusicEvent`: the menu music stops as the level loads.
        sounds.want_music = false;
        loading.begin(&mut commands, &mut images, &map);
    }
    let marks = l.game.as_ref().map(|g| g.handoff()).unwrap_or_default();
    if marks.quit {
        exit.write(AppExit::Success);
        return;
    }
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
        nav.focus = None;
        let now = l.now;
        l.page = Page::Main;
        l.closing_page = None;
        l.msg = None;
        // New saves give the bar Continue / Load.
        l.saves = dishonored_save::list(&saves_dir(&gs));
        if let Some(a) = art.as_deref() {
            open_main(&mut l, a, &t, &mut sounds, now);
        }
        l.dirty = true;
    }
}

// ---------------------------------------------------------------------------------------------
// Main menu

fn main_entries(t: &MenuTexts, has_saves: bool) -> Vec<(&'static str, String)> {
    let mut v = Vec::new();
    // The original's Continue (`t_ContinueGame`: the newest save, `OnLoadLastSaveClicked`) and
    // Load (`t_LoadGame`), shown when saves exist. `t_ContinueGame` is a key of
    // `[DisGFxMoviePlayerBase_Texts]` ("Continue"); looked up in the main menu's section it fell
    // back to the key's name ("CONTINUEGAME", wider than its button).
    if has_saves {
        v.push(("main:continue", t.t(SEC_BASE, "t_ContinueGame")));
    }
    v.extend([("main:play", "Play".to_string()), ("main:missions", t.t(SEC_MAIN, "t_Missions")), ("main:freeroam", "Free Roam".to_string())]);
    if has_saves {
        v.push(("main:load", t.t(SEC_MENU, "t_LoadGame")));
    }
    v.extend([("main:multiplayer", "Multiplayer".to_string()), ("main:options", t.t(SEC_BASE, "t_Options")), ("main:quit", t.t(SEC_MENU, "t_QuitGame"))]);
    v
}

/// Width of a main menu button: its autoSize label (`$TitleFont` 25 px, `textWidth + 4`) plus
/// the 50 px its top / bottom lines extend (`MainMenuButton.SetText`).
fn main_button_width(metrics: &crate::menu_art::FontMetrics, label: &str) -> (f32, f32) {
    let tw = metrics.field_width(FontKind::Title, &label.to_uppercase(), 25.0);
    (tw, tw + 50.0)
}

/// `MainMenu.PlayOpenAnimation` (and `BackToMainMenu`) at `now`.
fn open_main(l: &mut Launcher, art: &MenuArt, t: &MenuTexts, sounds: &mut MenuSounds, now: f64) {
    let Some(main) = art.get("main") else {
        l.opened_at = Some(now);
        l.input_from = now;
        return;
    };
    let Some((mm, mmm)) = main.root_child("mainMenu_mc") else { return };
    l.opened_at = Some(now);
    l.first_drawn.get_or_insert(now);
    l.main_closed_at = None;
    l.input_from = now + OPEN_INPUT_DELAY;
    l.pending = None;
    sounds.play("M_OpenMainMenu");
    let a = &mut l.anim;
    a.author("mm", Props::from_matrix(&mmm, 100.0));
    author_tree(a, main, "mm", mm, 3);
    // Root: back to its start properties (0.85 s), visible.
    a.now(now, "mm", Act::Set(P::new().visible(true)));
    a.now(now, "mm", Act::To(P::all(a.authored("mm")), 0.85, Ease::StrongOut));
    a.now(now, "mm", Act::Set(P::new().alpha(100.0)));
    // Splatters (`SplattersMainMenu.Open`): from 30 %, away from their side, then one every
    // 50 ms after 10 ms (0.35 s Strong.easeInOut).
    for i in 0..3 {
        let k = format!("mm/_splatters_mc/splatter{i}_mc");
        let p = a.authored(&k);
        let x = if p.x < 0.0 { p.x - 250.0 } else { p.x + 250.0 };
        let y = if p.y < 0.0 { p.y - 550.0 } else { p.y + 550.0 };
        a.now(now, &k, Act::End);
        a.now(now, &k, Act::Set(P::new().scale(30.0).alpha(0.0).x(x).y(y).rot(p.rotation - 50.0)));
        let mut target = P::all(p);
        target.v[5] = Some(100.0);
        a.at(now + 0.01 + 0.05 * i as f64, &k, Act::To(target, 0.35, Ease::StrongInOut));
    }
    // Logo (`OpenLogo` + `LogoMainMenu.Open`).
    let lk = "mm/_logo_mc";
    let lp = a.authored(lk);
    a.now(now, lk, Act::End);
    a.now(now, lk, Act::Set(P::new().alpha(0.0).rot(-25.0).scale(450.0).x(lp.x - 150.0)));
    a.now(now, lk, Act::To(P::all(lp), 0.2, Ease::StrongOut));
    for i in 0..5 {
        let k = format!("mm/_logo_mc/line{i}_mc");
        let p = a.authored(&k);
        let y = if p.y < 0.0 { p.y - 150.0 } else { p.y + 150.0 };
        a.now(now, &k, Act::Set(P::new().scale(350.0).alpha(0.0).y(y)));
        a.at(now + 0.075 * i as f64, &k, Act::To(P::all(p), 0.25, Ease::StrongOut));
    }
    for c in ["_logoGlow_mc", "_logo_mc"] {
        a.now(now, &format!("mm/_logo_mc/{c}"), Act::Set(P::new().alpha(0.0)));
    }
    let bk = "mm/_logo_mc/_bkgd_mc";
    let bp = a.authored(bk);
    a.now(now, bk, Act::Set(P::new().alpha(0.0).rot(10.0)));
    a.at(now + 0.15, bk, Act::Set(P::new().scale(250.0)));
    a.at(now + 0.15, bk, Act::To(P::all(bp), 0.2, Ease::StrongOut));
    let ik = "mm/_logo_mc/_logo_mc";
    let ip = a.authored(ik);
    a.at(now + 0.35, ik, Act::Set(P::new().scale(250.0)));
    a.at(now + 0.35, ik, Act::To(P::all(ip), 0.2, Ease::StrongOut));
    a.at(now + 0.35, "mm/_logo_mc/_logoGlow_mc", Act::To(P::new().alpha(100.0), 0.2, Ease::StrongOut));
    // Corvo (`OpenCorvo`): from 350 px right, rotated 5 deg (0.3 s Strong.easeInOut).
    let ck = "mm/_corvo_mc";
    let cp = a.authored(ck);
    a.now(now, ck, Act::End);
    a.now(now, ck, Act::Set(P::new().x(cp.x + 350.0).rot(5.0)));
    a.now(now, ck, Act::To(P::all(cp), 0.3, Ease::StrongInOut));
    // Compass (`OpenCompass`).
    for (i, s, d, e) in [(0, 150.0, 0.35, Ease::BackOut), (1, 150.0, 0.35, Ease::BackOut), (2, 250.0, 0.25, Ease::StrongOut)] {
        let k = format!("mm/_compass_mc/compass{i}_mc");
        let p = a.authored(&k);
        a.now(now, &k, Act::End);
        a.now(now, &k, Act::Set(P::new().scale(s).alpha(0.0)));
        a.now(now, &k, Act::To(P::all(p), d, e));
    }
    // Blades (`AnimatedBlades.Open`): each blade tweens from a few px / deg off over
    // 0.55..1.05 s; then they drift every 2.7 s (3.5 s tweens).
    let d = 0.01 * a.rand_range(55.0, 105.0);
    for i in 0..7 {
        let k = format!("mm/_blades_mc/_blades_mc/blade{i}_mc");
        let p = a.authored(&k);
        let r = (p.rotation + a.rand_range(-5.5, 7.5)).clamp(-170.0, 170.0);
        let x = p.x + a.rand_range(-5.0, 5.0);
        let y = p.y + a.rand_range(-5.0, 5.0);
        a.now(now, &k, Act::Finish);
        a.now(now, &k, Act::From(P::new().alpha(p.alpha).x(x).y(y).rot(r), d, Ease::StrongOut));
        let mut t = now + 2.7;
        let mut lp_ = false;
        while t < now + 600.0 {
            let rr = (p.rotation + a.rand_range(-5.5, 7.5)).clamp(-170.0, 170.0);
            let xx = p.x + a.rand_range(-5.0, 5.0);
            let yy = p.y + a.rand_range(-5.0, 5.0);
            a.at(t, &k, Act::To(P::new().alpha(p.alpha).x(xx).y(yy).rot(rr), 3.5, Ease::StrongOut));
            t += 2.7;
            lp_ = !lp_;
        }
    }
    // The band (`OpenMenu` at 180 ms): from `width + 150` px right, alpha 0, in 0.35 s.
    let bk = "mm/_menu_mc";
    let bandp = a.authored(bk);
    a.now(now, bk, Act::Set(P::new().alpha(0.0)));
    // `_menu_mc._width` once `SetMenu` sized its strip: the buttons plus the bar's offset.
    let total: f32 = main_entries(t, !l.saves.is_empty()).iter().map(|(_, s)| main_button_width(&art.metrics, s).1).sum();
    let band_w = (total + BAR_ORIGIN_X - 10.0) * bandp.xscale.abs() / 100.0;
    a.at(now + OPEN_BAND_DELAY, bk, Act::Set(P::new().x(bandp.x + band_w + 150.0).alpha(0.0)));
    a.at(now + OPEN_BAND_DELAY, bk, Act::To(P::all(bandp), 0.35, Ease::StrongOut));
    // The bar (`SetMenu` + `Open` at 180 ms).
    let t_bar = now + OPEN_BAND_DELAY;
    let bg = "mm/_menu_mc/_bkgd_mc";
    a.at(t_bar, bg, Act::Set(P::new().alpha(0.0).x(250.0)));
    a.at(t_bar, bg, Act::To(P::new().alpha(100.0).x(0.0).y(0.0), 0.25, Ease::StrongOut));
    for ln in ["_lineUp_mc", "_lineDown_mc"] {
        let k = format!("mm/_menu_mc/{ln}");
        let p = a.authored(&k);
        a.at(t_bar, &k, Act::Set(P::new().scale(250.0)));
        a.at(t_bar, &k, Act::To(P::new().x(p.x).y(p.y).scale(100.0).alpha(100.0), 0.25, Ease::StrongOut));
    }
    set_main_bar(l, art, t, t_bar);
    l.dirty = true;
}

/// `MainMenuButtonBar.SetMenu` + `GenericMenu.Open` at `t`: the buttons packed right to left
/// along the band, each opening 50 ms after the previous one.
fn set_main_bar(l: &mut Launcher, art: &MenuArt, texts: &MenuTexts, t: f64) {
    let entries = main_entries(texts, !l.saves.is_empty());
    let a = &mut l.anim;
    a.forget("mm/btn/");
    let widths: Vec<(f32, f32)> = entries.iter().map(|(_, s)| main_button_width(&art.metrics, s)).collect();
    let total: f32 = widths.iter().map(|w| w.1).sum();
    let mut x = -total - BAR_ORIGIN_X;
    l.main_ids = entries.iter().map(|e| e.0).collect();
    if l.main_sel >= entries.len() {
        l.main_sel = 0;
    }
    // `_bkgd_mc._width = container width + offset - 10` (the strip extends left from 0).
    a.reauthor("mm/_menu_mc/_bkgd_mc", Props { xscale: 100.0 * (total + BAR_ORIGIN_X - 10.0) / 444.8, ..Props::default() });
    let text_h = 29.0;
    for (i, (tw, w)) in widths.iter().enumerate() {
        let k = format!("mm/btn/{i}");
        let cx = x + w * 0.5;
        x += w;
        a.reauthor(&k, Props { x: cx, y: 0.0, ..Props::default() });
        // `SetText`: side lines at the text's ends, top / bottom lines `text + 50` wide.
        a.reauthor(&format!("{k}/_leftLine_mc"), Props { x: -0.5 * tw - 10.0, yscale: 100.0 * text_h / 32.5, ..Props::default() });
        a.reauthor(&format!("{k}/_rightLine_mc"), Props { x: 0.5 * tw + 10.0, yscale: 100.0 * text_h / 32.5, ..Props::default() });
        a.reauthor(&format!("{k}/_upLine_mc"), Props { y: -0.5 * text_h - 10.0, xscale: 100.0 * (tw + 50.0) / 194.5, ..Props::default() });
        a.reauthor(&format!("{k}/_downLine_mc"), Props { y: 0.5 * text_h + 10.0, xscale: 100.0 * (tw + 50.0) / 244.75, ..Props::default() });
        // Every line is placed by script (drawn from these values, not the movie's).
        for ln in ["_leftLine_mc", "_rightLine_mc", "_upLine_mc", "_downLine_mc"] {
            let kk = format!("{k}/{ln}");
            let p = a.authored(&kk);
            a.now(t, &kk, Act::Set(P::all(p)));
        }
        for ln in ["_upLine_mc", "_downLine_mc"] {
            a.now(t, &format!("{k}/{ln}"), Act::Set(P::new().alpha(0.0)));
        }
        a.now(t, &k, Act::Set(P::new().alpha(0.0)));
        // `PlayOpenButtonAnim`: from 250 % to the over / out props (0.35 s Back.easeInOut).
        let st = t + 0.05 * i as f64;
        let sel = i == l.main_sel;
        a.at(st, &k, Act::Set(P::new().scale(250.0).alpha(0.0)));
        a.at(st, &k, Act::To(if sel { P::new().scale(110.0).y(5.0).alpha(100.0) } else { P::new().scale(100.0).y(0.0).alpha(100.0) }, 0.35, Ease::BackInOut));
    }
    // `UpdateSelection(cur)` right after the buttons are made: the current one plays `over`.
    let k = format!("mm/btn/{}", l.main_sel);
    main_select_on(&mut l.anim, &k, t);
    main_parallax(&mut l.anim, l.main_sel, entries.len(), t);
}

/// `MainMenuButtonBar.playSelectionOn`.
fn main_select_on(a: &mut Anim, k: &str, now: f64) {
    let ll = a.authored(&format!("{k}/_leftLine_mc"));
    let rl = a.authored(&format!("{k}/_rightLine_mc"));
    a.now(now, &format!("{k}/_leftLine_mc"), Act::To(P::new().x(ll.x - 25.0).alpha(0.0), 0.2, Ease::StrongOut));
    a.now(now, &format!("{k}/_rightLine_mc"), Act::To(P::new().x(rl.x + 25.0).alpha(0.0), 0.2, Ease::StrongOut));
    for (ln, x) in [("_upLine_mc", -30.0), ("_downLine_mc", 30.0)] {
        let kk = format!("{k}/{ln}");
        let p = a.authored(&kk);
        a.now(now, &kk, Act::Set(P::new().alpha(0.0).x(x)));
        a.now(now, &kk, Act::To(P::new().x(p.x).y(p.y).alpha(100.0), 0.35, Ease::StrongOut));
    }
    a.now(now, k, Act::To(P::new().scale(110.0).y(5.0).alpha(100.0), 0.25, Ease::BackOut));
    a.now(now, k, Act::Play("over".into()));
}

/// `MainMenuButtonBar.playSelectionOut`.
fn main_select_out(a: &mut Anim, k: &str, now: f64) {
    for ln in ["_leftLine_mc", "_rightLine_mc"] {
        let kk = format!("{k}/{ln}");
        let p = a.authored(&kk);
        a.now(now, &kk, Act::To(P::new().x(p.x).y(p.y).alpha(100.0), 0.25, Ease::BackOut));
    }
    a.now(now, &format!("{k}/_upLine_mc"), Act::To(P::new().x(-40.0).alpha(0.0), 0.2, Ease::StrongOut));
    a.now(now, &format!("{k}/_downLine_mc"), Act::To(P::new().x(40.0).alpha(0.0), 0.2, Ease::StrongOut));
    a.now(now, k, Act::To(P::new().scale(100.0).y(0.0).alpha(100.0), 0.15, Ease::StrongOut));
    a.now(now, k, Act::Play("out".into()));
}

/// `MainMenuButtonBar.onSelectionUpdated`: the compass turns, Corvo and the blades shift.
fn main_parallax(a: &mut Anim, idx: usize, n: usize, now: f64) {
    let last = n.saturating_sub(1).max(1) as f32;
    let r2 = idx as f32 * 100.0 / last;
    a.now(now, "mm/_compass_mc/compass0_mc", Act::To(P::new().rot(idx as f32 * -10.25), 0.25, Ease::StrongOut));
    a.now(now, "mm/_compass_mc/compass1_mc", Act::To(P::new().rot(idx as f32 * 7.5), 0.25, Ease::StrongOut));
    let cp = a.authored("mm/_corvo_mc");
    a.now(now, "mm/_corvo_mc", Act::Finish);
    a.now(now, "mm/_corvo_mc", Act::To(P::new().x(cp.x - 0.01 * r2).y(cp.y - 0.02 * r2), 0.25, Ease::StrongOut));
    let bp = a.authored("mm/_blades_mc");
    a.now(now, "mm/_blades_mc", Act::Finish);
    a.now(now, "mm/_blades_mc", Act::To(P::new().x(bp.x - 0.07 * r2).y(bp.y - 0.085 * r2), 0.25, Ease::StrongOut));
}

/// `MainMenu.Close`: Corvo and the splatters leave, the buttons close one by one (50 ms),
/// the whole menu fades and drops 50 px (0.35 s).
fn close_main(l: &mut Launcher, now: f64) {
    l.main_closed_at = Some(now);
    let a = &mut l.anim;
    let cp = a.authored("mm/_corvo_mc");
    a.now(now, "mm/_corvo_mc", Act::End);
    a.now(now, "mm/_corvo_mc", Act::To(P::new().x(cp.x + 350.0).alpha(0.0), 0.3, Ease::StrongOut));
    for i in 0..3 {
        let k = format!("mm/_splatters_mc/splatter{i}_mc");
        let p = a.authored(&k);
        a.now(now, &k, Act::End);
        a.now(now, &k, Act::To(P::new().x(p.x + 350.0).alpha(0.0), 0.25, Ease::StrongOut));
    }
    for i in 0..l.main_ids.len() {
        let k = format!("mm/btn/{i}");
        let x = a.get(&k, now).x;
        a.cancel_queued(&k);
        a.at(now + 0.05 * i as f64, &k, Act::To(P::new().alpha(0.0).scale(80.0).x(x - 30.0), 0.2, Ease::StrongOut));
    }
    let start = a.authored("mm");
    a.now(now, "mm", Act::End);
    a.now(now, "mm", Act::To(P::new().alpha(0.0).y(start.y + 50.0), 0.35, Ease::StrongOut));
}


/// Screen content.
#[allow(clippy::too_many_arguments)]
fn build(l: &Launcher, art: &MenuArt, t: &MenuTexts, gs: &GameSettings, nav: &MenuNav, bg: &str, now: f64) -> DisplayList {
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
    // The main menu while it is open or closing.
    let main_visible = l.page == Page::Main || l.main_closed_at.is_some_and(|c| now - c < 0.45);
    if main_visible && l.opened_at.is_some() {
        build_main(&mut dl, l, art, main, t, focus, now);
    }
    if l.page == Page::Start || l.closing_page.is_some_and(|(p, c)| p == Page::Start && now - c < 0.3) {
        build_start(&mut dl, l, main, t, now);
    }
    let mut pages = vec![l.page];
    if let Some((p, _)) = l.closing_page {
        if p != l.page {
            pages.insert(0, p);
        }
    }
    for p in pages {
        match p {
            Page::Load => build_load(&mut dl, art, l, t, focus, now),
            Page::Play => build_play(&mut dl, art, main, l, t, gs, focus, now),
            Page::Missions | Page::FreeRoam => build_slide(&mut dl, art, main, l, t, p, now),
            Page::Multiplayer => build_mp(&mut dl, art, l, t, gs, nav, now),
            Page::Options => l.options.build(&mut dl, art, t, gs, nav, now),
            Page::Main | Page::Start => {}
        }
    }
    if let Some(m) = &l.msg {
        // Only the box's buttons take the keyboard while it is up.
        for it in dl.items.iter_mut() {
            it.nav = false;
        }
        let (sel, since) = l.msg_focus;
        let f = |i: usize| -> (bool, f32) { if i == sel { (true, (now - since) as f32) } else { (false, (now - since) as f32) } };
        m.draw(&mut dl, art, &l.anim, now, &f);
    }
    dl
}

#[allow(clippy::too_many_arguments)]
fn build_main(dl: &mut DisplayList, l: &Launcher, art: &MenuArt, main: &MovieArt, t: &MenuTexts, focus: &str, now: f64) {
    let Some((mm, _)) = main.root_child("mainMenu_mc") else { return };
    let root = l.anim.get("mm", now);
    if !root.visible || root.alpha <= 0.0 {
        return;
    }
    let rm = root.matrix();
    let a = root.alpha / 100.0;
    let clock = l.opened_at.map(|o| (now - o) as f32).unwrap_or(0.0) + 10.0;
    let ovr = l.anim.ovr("mm", now);
    // Everything of mainMenu_mc but the DLC button (console store entry); the band's dark
    // strip is a solid fill.
    let hide_dlc = |p: &str| if p == "_DLCButton_mc" { Some(Props { visible: false, ..Props::default() }) } else { ovr(p) };
    let st = States::default().with_timed("_compass_mc", "loop", clock).clock(clock);
    dl.movie_fx_solids(main, mm, rm, &st, a, &hide_dlc, &|p: &str| p.starts_with("_menu_mc/_bkgd_mc"));
    // The bar's buttons, in the band (`_menu_mc/_btnContainer_mc`).
    let Some((_, band)) = chain(main, &l.anim, "mm", now, mm, rm, &["_menu_mc"]) else { return };
    let band_a = a * l.anim.value("mm/_menu_mc", now).map(|p| p.alpha / 100.0).unwrap_or(1.0);
    let Some(btn) = main.export("m_MainMenu_btn_") else { return };
    let entries = main_entries(t, !l.saves.is_empty());
    for (i, (id, label)) in entries.iter().enumerate() {
        let k = format!("mm/btn/{i}");
        let p = l.anim.get(&k, now);
        if !p.visible || p.alpha <= 0.0 {
            continue;
        }
        let bm = band.mul(&p.matrix());
        let (lab, secs) = l.anim.timeline(&k, now).map(|(s, x)| (s.to_string(), x)).unwrap_or(("default".into(), 0.0));
        let st = States::default().with_timed("*", &lab, secs).with("_bkgdOver_mc", "#1");
        let bo = l.anim.ovr(&k, now);
        let ba = band_a * p.alpha / 100.0;
        dl.movie_fx(main, btn, bm, &st, ba, &bo, false);
        let fields = DisplayList::fields_fx(main, btn, bm, &st, &bo);
        if let Some(f) = DisplayList::label_field(&fields) {
            let mut f = f.clone();
            centre_field_on(&mut f, &bm);
            f.color.mul[3] *= ba;
            dl.field_text(main, &f, &label.to_uppercase(), None);
        }
        let (_, w) = main_button_width(&art.metrics, label);
        let (cx, cy) = bm.apply(0.0, 0.0);
        dl.item(*id, Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(w, 44.0)), true);
    }
    let _ = focus;
}

/// Move a left-aligned label field so its text is centred on the clip origin (`txt._x =
/// -0.5 * width`, `MainMenuButton.SetText`).
fn centre_field_on(f: &mut dishonored_assets::swf::TextPlacement, clip: &Matrix) {
    let b = f.text.bounds;
    let (cx, cy) = f.matrix.apply((b.xmin + b.xmax) * 0.5, (b.ymin + b.ymax) * 0.5);
    let inv = clip.inverse();
    let (lx, _ly) = inv.apply(cx, cy);
    // Shift along the clip's x axis by -lx (twips in clip space).
    f.matrix.tx -= clip.a * lx;
    f.matrix.ty -= clip.b * lx;
    f.text.align = 2;
}

/// "Press any key" (`StartScreen`): fades in (0.45 s, logo 0.85 s, text 0.25 s).
fn build_start(dl: &mut DisplayList, l: &Launcher, main: &MovieArt, t: &MenuTexts, now: f64) {
    let Some((ch, m)) = main.root_child("startScreen_mc") else { return };
    let p = l.anim.get("start", now);
    if p.alpha <= 0.0 {
        return;
    }
    let ovr = l.anim.ovr("start", now);
    let a = p.alpha / 100.0;
    let m = Matrix { tx: p.x * 20.0, ty: p.y * 20.0, ..m };
    dl.movie_fx(main, ch, m, &States::default(), a, &ovr, false);
    for f in DisplayList::fields_fx(main, ch, m, &States::default(), &ovr) {
        let mut f = f;
        f.color.mul[3] *= a;
        dl.field_text(main, &f, &t.t(SEC_MAIN, "t_PressAnyKey"), None);
    }
    dl.item("start:any", Rect::new(0.0, 0.0, 1280.0, 720.0), false);
}

fn open_start(l: &mut Launcher, art: &MenuArt, now: f64) {
    let Some(main) = art.get("main") else { return };
    if let Some((ch, m)) = main.root_child("startScreen_mc") {
        let a = &mut l.anim;
        a.author("start", Props::from_matrix(&m, 100.0));
        author_tree(a, main, "start", ch, 1);
        a.now(now, "start", Act::Set(P::new().alpha(0.0)));
        a.now(now, "start", Act::To(P::new().alpha(100.0), 0.45, Ease::StrongOut));
        a.now(now, "start/_logo_mc", Act::Set(P::new().alpha(0.0)));
        a.now(now, "start/_logo_mc", Act::To(P::new().alpha(100.0), 0.85, Ease::StrongOut));
        a.now(now, "start/_txt_mc", Act::Set(P::new().alpha(0.0)));
        a.now(now, "start/_txt_mc", Act::To(P::new().alpha(100.0), 0.25, Ease::StrongOut));
    }
    l.page = Page::Start;
    l.page_at = now;
    l.input_from = now;
    l.dirty = true;
}

// ---------------------------------------------------------------------------------------------
// Play (the new-game screen: `NewGameScreen` / `NewGameMenu`)

/// `newGame_mc` button layout: vertical, `_offsetBtnY` 2, 51.5 px buttons from
/// `10 + 0.075 * Stage.width` (menu-local).
const NG_BTN_H: f32 = 51.5;
const NG_ORIGIN_X: f32 = 10.0 + 0.075 * 1280.0;

fn open_play(l: &mut Launcher, art: &MenuArt, now: f64) {
    let Some(main) = art.get("main") else { return };
    let Some((ng, m)) = main.root_child("newGame_mc") else { return };
    let a = &mut l.anim;
    a.forget("ng");
    a.author("ng", Props::from_matrix(&m, 100.0));
    author_tree(a, main, "ng", ng, 2);
    let n = PLAY_MODES.len();
    for i in 0..n {
        let k = format!("ng/btn/{i}");
        a.reauthor(&k, Props { x: 0.0, y: i as f32 * (NG_BTN_H + 2.0) + NG_BTN_H * 0.5, ..Props::default() });
        a.now(now, &k, Act::Set(P::new().alpha(0.0)));
        // `OpenButton`: one every 50 ms from 250 % (0.35 s Back.easeInOut) to the out props.
        let t = now + 0.05 * i as f64;
        a.at(t, &k, Act::Set(P::new().scale(250.0)));
        a.at(t, &k, Act::To(P::new().scale(100.0).x(0.0).alpha(100.0), 0.35, Ease::BackInOut));
    }
    let sel = l.play_mode.min(n - 1);
    a.now(now, &format!("ng/btn/{sel}"), Act::Play("over".into()));
    // `NewGameMenu.Open`: the panel art at 150 ms, the description at 300 ms.
    a.now(now, "ng/_menu_mc/_description_mc", Act::Set(P::new().alpha(0.0)));
    a.now(now, "ng/_menu_mc/_bkgd_mc", Act::Set(P::new().alpha(0.0)));
    let bp = a.authored("ng/_menu_mc/_bkgd_mc");
    a.at(now + 0.15, "ng/_menu_mc/_bkgd_mc", Act::Set(P::new().scale(250.0).rot(-5.0)));
    a.at(now + 0.15, "ng/_menu_mc/_bkgd_mc", Act::To(P::new().alpha(100.0).scale(100.0).rot(bp.rotation), 0.25, Ease::StrongOut));
    a.at(now + 0.3, "ng/_menu_mc/_description_mc", Act::Set(P::new().scale(450.0)));
    a.at(now + 0.3, "ng/_menu_mc/_description_mc", Act::To(P::new().alpha(100.0).scale(100.0), 0.25, Ease::StrongOut));
    // `NewGameScreen.Open`: Corvo and the texture from 500 px left.
    for (c, e) in [("_corvo_mc", Ease::StrongOut), ("_bkgdTexture_mc", Ease::BackInOut)] {
        let k = format!("ng/{c}");
        let p = a.authored(&k);
        a.now(now, &k, Act::Set(P::new().x(p.x - 500.0)));
        a.now(now, &k, Act::To(P::new().x(p.x), 0.25, e));
    }
    if let Some(c) = art.get("common") {
        TitleBar::open(a, c, "ng/title", Props { x: -545.75, y: -282.3, rotation: -2.0, ..Props::default() }, now);
    }
    // `OnOpened` at 550 ms: the help bar.
    a.at(now + 0.55, "ng/help", Act::Set(P::new()));
    HelpBar::open(a, "ng/help", now + 0.55);
    l.input_from = now + 0.16;
    l.last_focus = None;
}

/// `NewGameMenu.Close` (the bar closes button by button, 50 ms apart; the screen hides).
fn close_play(l: &mut Launcher, now: f64) {
    let a = &mut l.anim;
    for i in 0..PLAY_MODES.len() {
        let k = format!("ng/btn/{i}");
        let x = a.get(&k, now).x;
        a.cancel_queued(&k);
        a.at(now + 0.05 * i as f64, &k, Act::To(P::new().alpha(0.0).scale(80.0).x(x - 30.0), 0.2, Ease::StrongOut));
    }
    // `NewGameScreen.Close` hides the screen at once (only the buttons animate).
    a.now(now, "ng", Act::Set(P::new().visible(false)));
    HelpBar::close(a, "ng/help", now);
}

#[allow(clippy::too_many_arguments)]
fn build_play(dl: &mut DisplayList, art: &MenuArt, main: &MovieArt, l: &Launcher, t: &MenuTexts, gs: &GameSettings, focus: &str, now: f64) {
    let Some((ng, _)) = main.root_child("newGame_mc") else { return };
    let root = l.anim.get("ng", now);
    let rm = root.matrix();
    let ovr = l.anim.ovr("ng", now);
    let sel = l.play_mode.min(PLAY_MODES.len() - 1);
    // The portrait follows the selection (`SetPortrait(idx + 1)`).
    let st = States::default().with("_corvo_mc/portrait_mc", &format!("#{}", sel + 1)).clock(((now - l.page_at) as f32).max(0.0));
    if root.visible {
        let skip = |p: &str| match p {
            "_title_mc" => Some(Props { visible: false, ..Props::default() }),
            other => ovr(other),
        };
        dl.movie_fx(main, ng, rm, &st, 1.0, &skip, false);
        if let Some(c) = art.get("common") {
            TitleBar::draw(dl, c, &art.metrics, &l.anim, "ng/title", now, rm, "Play");
        }
        // Description (`_description_mc.txt`, swapped at once on focus).
        if let Some((dc, dm)) = chain(main, &l.anim, "ng", now, ng, rm, &["_menu_mc", "_description_mc"]) {
            let da = l.anim.value("ng/_menu_mc/_description_mc", now).map(|p| p.alpha / 100.0).unwrap_or(1.0);
            for f in DisplayList::fields(main, dc, dm, &States::default()) {
                let mut f = f;
                f.color.mul[3] *= da;
                dl.field_text(main, &f, PLAY_MODES[sel].1, None);
            }
        }
    }
    // The mode buttons (`m_nGame_btn`) in the menu panel.
    if let (Some(btn), Some((_, mm))) = (main.export("m_nGame_btn"), chain(main, &l.anim, "ng", now, ng, rm, &["_menu_mc"])) {
        let bar = mm.mul(&px_matrix(NG_ORIGIN_X, 0.0));
        for (i, (name, _, _)) in PLAY_MODES.iter().enumerate() {
            let k = format!("ng/btn/{i}");
            let p = l.anim.get(&k, now);
            if p.alpha <= 0.0 {
                continue;
            }
            let id = format!("play:mode:{i}");
            let bm = bar.mul(&p.matrix());
            let (lab, secs) = l.anim.timeline(&k, now).map(|(s, x)| (s.to_string(), x)).unwrap_or(("default".into(), 0.0));
            let st = States::default().with_timed("*", &lab, secs).with("_bkgd_mc", &format!("#{}", i % 3));
            let a = p.alpha / 100.0;
            dl.movie(main, btn, bm, &st, a);
            for f in DisplayList::fields(main, btn, bm, &st) {
                if !f.path.contains("_txt_mc") {
                    continue;
                }
                let mut f = f;
                f.color.mul[3] *= a;
                dl.field_text(main, &f, name, None);
            }
            let (cx, cy) = bm.apply(100.0 * 20.0, 0.0);
            dl.item(&id, Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(280.0, NG_BTN_H - 4.0)), true);
        }
    }
    // Map and bots (port lines, in the options line style) under the panel.
    if let Some(o) = art.get("options") {
        let a = l.anim.get("ng/btn/0", now).alpha / 100.0;
        let mode = PLAY_MODES[sel];
        let mut y = 560.0;
        if mode.2 {
            let map = menu_maps::entry(&l.maps, &gs.s.launcher.play_map);
            generic_line_at(dl, o, 190.0, y, 0, "play:map", "Map", &map.name, LineWidget::Stepper, line_state(l, "play:map", focus, now), a);
            y += GENERIC_LINE_H;
            generic_line_at(dl, o, 190.0, y, 1, "play:bots", "Bots", &l.bots.to_string(), LineWidget::Stepper, line_state(l, "play:bots", focus, now), a);
        }
    }
    if let Some(c) = art.get("common") {
        HelpBar::draw(dl, c, &art.metrics, t, &l.anim, "ng/help", now, help_list("SecondaryScreen"));
    }
}

/// Focus timeline of a free-form line: `over` while focused, `out` after.
fn line_state<'a>(l: &'a Launcher, id: &str, focus: &str, now: f64) -> (&'a str, f32) {
    match l.anim.timeline(&format!("line/{id}"), now) {
        Some((lab, s)) => (lab, s),
        None => {
            if focus == id {
                ("over", 1.0)
            } else {
                ("default", 0.0)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Card slide (`MissionsScreen` / `MissionsMenu` / `SlideMenu`): custom missions and free roam

struct Card {
    id: String,
    name: String,
    image: Option<String>,
}

fn cards(l: &Launcher, page: Page) -> Vec<Card> {
    match page {
        Page::FreeRoam => l.maps.iter().map(|m| Card { id: format!("freeroam:item:{}", m.map), name: m.name.clone(), image: m.image.clone() }).collect(),
        _ => l.missions.iter().enumerate().map(|(i, m)| Card { id: format!("missions:item:{i}"), name: m.title.clone(), image: menu_maps::entry(&l.maps, &m.map).image }).collect(),
    }
}

fn slide_sel(l: &Launcher, page: Page) -> usize {
    if page == Page::FreeRoam { l.freeroam_sel } else { l.mission_sel }
}

/// Card width (`m_missions_item` `_width`, its hit area) and the spacing offset (-100).
const CARD_W: f32 = 482.0;
const CARD_OFFSET: f32 = -100.0;

/// `SetItemsSize(cur)`: scale / alpha by distance and the x positions (each card advances by
/// its width at the previous card's scale, minus 100 at that scale).
fn card_layout(n: usize, cur: usize) -> Vec<(f32, f32, f32, f32)> {
    let mut v = Vec::with_capacity(n);
    let mut x = CARD_W * 0.5;
    let mut prev_scale = 100.0;
    for i in 0..n {
        let d = (i as i32 - cur as i32).unsigned_abs();
        let s = match d {
            0 => 100.0,
            1 => 80.0,
            2 => 75.0,
            _ => 70.0,
        };
        if i > 0 {
            x += CARD_W * prev_scale / 100.0 + prev_scale * CARD_OFFSET / 100.0;
        }
        let y = if d == 0 { -35.0 } else { 0.0 };
        v.push((x, y, s, s));
        prev_scale = s;
    }
    v
}

fn open_slide(l: &mut Launcher, art: &MenuArt, page: Page, now: f64) {
    let n = cards(l, page).len();
    let sel = slide_sel(l, page).min(n.saturating_sub(1));
    let a = &mut l.anim;
    a.forget("ms");
    a.author("ms", Props::default());
    // `SlideMenu.Open`: cards snap to their sizes; the strip slides from the last card to the
    // first in n * 0.08 s; the slide fades in over 2.25 s.
    let lay = card_layout(n, 0);
    for (i, (x, y, s, _)) in lay.iter().enumerate() {
        let k = format!("ms/card/{i}");
        // Snapped while opening: full alpha.
        a.reauthor(&k, Props { x: *x, y: *y + CARD_DEF_Y, xscale: *s, yscale: *s, alpha: 100.0, ..Props::default() });
    }
    let last = lay.last().map(|c| c.0).unwrap_or(0.0);
    a.reauthor("ms/items", Props { x: -last, y: -10.0, ..Props::default() });
    let first = lay.first().map(|c| c.0).unwrap_or(0.0);
    a.now(now, "ms/items", Act::To(P::new().x(-first + 0.5 * CARD_W), n as f32 * 0.08, Ease::StrongOut));
    a.now(now, "ms", Act::Set(P::new().alpha(0.0)));
    a.now(now, "ms", Act::To(P::new().alpha(100.0), 2.25, Ease::StrongOut));
    if let Some(c) = art.get("common") {
        TitleBar::open(a, c, "ms/title", Props { x: -545.0, y: -282.3, rotation: -2.0, ..Props::default() }, now);
    }
    HelpBar::open(a, "ms/help", now);
    // `OnOpened` at 150 ms: the selection and inputs.
    l.input_from = now + 0.15;
    if page == Page::FreeRoam {
        l.freeroam_sel = 0;
    } else {
        l.mission_sel = 0;
    }
    let _ = sel;
    slide_select(l, page, 0, now + 0.15, false);
}

/// Card centre y in the slide (`_defPosY`, the card's half height).
const CARD_DEF_Y: f32 = 222.0;

/// `UpdateSelection(i)`: the new card's strokes come in, sizes tween (0.2 s), the strip
/// centres the card (0.35 s).
fn slide_select(l: &mut Launcher, page: Page, i: usize, now: f64, mouse: bool) {
    let n = cards(l, page).len();
    if n == 0 {
        return;
    }
    let old = slide_sel(l, page);
    if page == Page::FreeRoam {
        l.freeroam_sel = i;
    } else {
        l.mission_sel = i;
    }
    let a = &mut l.anim;
    let ko = format!("ms/card/{old}");
    a.now(now, &ko, Act::Play("out".into()));
    a.now(now, &format!("{ko}/sel"), Act::To(P::new().alpha(0.0), 0.25, Ease::StrongOut));
    let kn = format!("ms/card/{i}");
    a.now(now, &kn, Act::Play("over".into()));
    a.now(now, &format!("{kn}/sel"), Act::Set(P::new().alpha(0.0)));
    a.now(now, &format!("{kn}/sel"), Act::To(P::new().alpha(100.0), 0.25, Ease::StrongOut));
    let lay = card_layout(n, i);
    for (j, (x, y, s, al)) in lay.iter().enumerate() {
        a.now(now, &format!("ms/card/{j}"), Act::To(P::new().x(*x).y(*y + CARD_DEF_Y).scale(*s).alpha(*al), 0.2, Ease::StrongOut));
    }
    if !mouse {
        a.now(now, "ms/items", Act::To(P::new().x(-lay[i].0 + 0.5 * CARD_W), 0.35, Ease::StrongOut));
    }
}

fn build_slide(dl: &mut DisplayList, art: &MenuArt, main: &MovieArt, l: &Launcher, t: &MenuTexts, page: Page, now: f64) {
    let Some((ms, msm)) = main.root_child("missions_mc") else { return };
    let root = l.anim.get("ms", now);
    let a = root.alpha / 100.0;
    let title = if page == Page::FreeRoam { "Free Roam".to_string() } else { t.t(SEC_MAIN, "t_MissionsScreenDesc") };
    if let Some(c) = art.get("common") {
        TitleBar::draw(dl, c, &art.metrics, &l.anim, "ms/title", now, msm, &title);
    }
    let list = cards(l, page);
    if list.is_empty() {
        dl.text(t.t(SEC_BASE, "t_EmptyList"), Rect::new(0.0, 330.0, 1280.0, 370.0), 26.0, FontKind::Normal, TEXT, Justify::Center);
    }
    let Some(card) = main.export("m_missions_item") else { return };
    let Some((_, slide_m)) = main.child(ms, "_slide_mc") else { return };
    let items = l.anim.get("ms/items", now);
    let base = msm.mul(&slide_m).mul(&px_matrix(items.x, items.y));
    let sel = slide_sel(l, page);
    // Draw far cards first so the selected one is on top.
    let mut order: Vec<usize> = (0..list.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse((*i as i32 - sel as i32).abs()));
    for i in order {
        let c = &list[i];
        let k = format!("ms/card/{i}");
        let p = l.anim.get(&k, now);
        let ca = a * p.alpha / 100.0;
        if ca <= 0.002 {
            continue;
        }
        let m = base.mul(&p.matrix());
        // Skip cards far outside the screen.
        if !(-700.0..2000.0).contains(&(m.tx / 20.0)) {
            continue;
        }
        let (lab, secs) = l.anim.timeline(&k, now).map(|(s, x)| (s.to_string(), x)).unwrap_or(("default".into(), 0.0));
        let sa = l.anim.value(&format!("{k}/sel"), now).map(|v| v.alpha).unwrap_or(0.0);
        let st = States::default().with_timed("*", &lab, secs);
        // The picture (`ImgLoader` on `_img_mc/loader_mc`, centred, not scaled).
        if let Some((h, size)) = c.image.as_ref().and_then(|n| l.card_images.get(n)) {
            let im = m.mul(&px_matrix(7.0, -252.0 + 22.4));
            let pts = [(-0.5, -0.5), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)].map(|(fx, fy)| {
                let (x, y) = im.apply(fx * size.x * 20.0, fy * size.y * 20.0);
                (x / 20.0, y / 20.0)
            });
            dl.prims.push(crate::menu_art::Prim::Quad { tex: h.clone(), src: [0.0, 0.0, size.x, size.y], dst: pts, color: [1.0, 1.0, 1.0, ca] });
        }
        let ovr = |q: &str| match q {
            "_stroke_mc" | "_selStroke_mc" => Some(Props { alpha: sa, ..Props::from_matrix(&main.child(card, q).map(|c| c.1).unwrap_or_default(), sa) }),
            "_nameBkgd_mc" if i == sel => main.child(card, q).map(|c| {
                let mut pp = Props::from_matrix(&c.1, 100.0);
                pp.y -= 10.0 * (sa / 100.0);
                pp
            }),
            _ => None,
        };
        dl.movie_fx(main, card, m, &st, ca, &ovr, false);
        for f in DisplayList::fields_fx(main, card, m, &st, &ovr) {
            let mut f = f;
            f.color.mul[3] *= ca;
            let text = if f.path.contains("Number") { format!("{:02}", i + 1) } else { c.name.to_uppercase() };
            dl.field_text(main, &f, &text, None);
        }
        let (cx, cy) = m.apply(0.0, -140.0 * 20.0);
        dl.item(&c.id, Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(380.0 * p.xscale / 100.0, 440.0 * p.yscale / 100.0)), true);
    }
    // Briefing / map details of the selected card (port text, under the cards).
    if page == Page::Missions {
        if let Some(me) = l.missions.get(sel) {
            dl.paragraph(&me.briefing, Rect::new(110.0, 600.0, 1100.0, 640.0), 20.0, FontKind::Normal, TEXT.with_alpha(a), Justify::Left);
        }
    } else if let Some(m) = l.maps.get(sel) {
        let note = if m.redirected { format!("{}  -  {} (level script only: entered through the story in the original)", m.name, m.map) } else { format!("{}  -  {}", m.name, m.map) };
        dl.paragraph(note, Rect::new(110.0, 600.0, 1100.0, 640.0), 20.0, FontKind::Normal, TEXT_DIM.with_alpha(a), Justify::Left);
    }
    if let Some(c) = art.get("common") {
        // `MissionsScreen`: X (stats) is locked here: no stats for these.
        HelpBar::draw(dl, c, &art.metrics, t, &l.anim, "ms/help", now, &[("A", "t_Validate"), ("B", "t_Back")]);
    }
}

// ---------------------------------------------------------------------------------------------
// Load (`UI_LoadGame` save list)

fn open_load(l: &mut Launcher, art: &MenuArt, now: f64) {
    let a = &mut l.anim;
    a.forget("lg");
    a.author("lg", Props::default());
    // `ItemsList.ShowList('l')`: from 40 px left, alpha 0, in 0.3 s; rows every 20 ms.
    a.now(now, "lg", Act::Set(P::new().x(-40.0).alpha(0.0)));
    a.now(now, "lg", Act::To(P::new().x(0.0).alpha(100.0), 0.3, Ease::StrongOut));
    for i in 0..7 {
        let k = format!("lg/row/{i}");
        a.author(&k, Props::default());
        a.now(now, &k, Act::Set(P::new().alpha(0.0).scale(120.0)));
        a.at(now + 0.02 * i as f64, &k, Act::Set(P::new().x(20.0)));
        a.at(now + 0.02 * i as f64, &k, Act::To(P::new().alpha(100.0).x(0.0).scale(100.0), 0.25, Ease::StrongOut));
    }
    if let Some(c) = art.get("common") {
        TitleBar::open(a, c, "lg/title", Props { x: -545.0, y: -285.0, rotation: -2.0, ..Props::default() }, now);
    }
    HelpBar::open(a, "lg/help", now);
    l.input_from = now + 0.02 * 7.0;
    l.last_focus = None;
}

fn build_load(dl: &mut DisplayList, art: &MenuArt, l: &Launcher, t: &MenuTexts, focus: &str, now: f64) {
    let Some(load) = art.get("load") else { return };
    let o = origin_px();
    if let Some((ch, m)) = load.root_child("loadGame_mc").or_else(|| load.export("m_loadGame_").map(|c| (c, px_matrix(640.0, 360.0)))) {
        // Backdrop texture only (the list is drawn below).
        if let Some((bc, bm)) = load.child(ch, "_bkgdTexture_mc") {
            dl.movie(load, bc, m.mul(&bm), &States::default(), 1.0);
        }
    }
    if let Some(c) = art.get("common") {
        TitleBar::draw(dl, c, &art.metrics, &l.anim, "lg/title", now, o, &t.t(SEC_MENU, "t_LoadGame"));
    }
    let lp = l.anim.get("lg", now);
    let la = lp.alpha / 100.0;
    if l.saves.is_empty() {
        dl.text(t.t(SEC_BASE, "t_EmptyList"), Rect::new(0.0, 330.0, 1280.0, 370.0), 26.0, FontKind::Normal, TEXT.with_alpha(la), Justify::Center);
    } else if let Some(row) = load.export("m_loadGameList_item") {
        let lm = o.mul(&Props { x: -515.0 + lp.x, y: -205.0, rotation: -2.5, ..Props::default() }.matrix());
        let first = l.save_sel.saturating_sub(6).min(l.saves.len().saturating_sub(7));
        for (vi, i) in (first..(first + 7).min(l.saves.len())).enumerate() {
            let (_, h) = &l.saves[i];
            let id = format!("load:item:{i}");
            let k = format!("lg/row/{vi}");
            let p = l.anim.get(&k, now);
            let fx = l.anim.value(&format!("line/{id}"), now).map(|v| v.x).unwrap_or(0.0);
            let rm = lm.mul(&px_matrix(p.x + fx, (36.0 + 71.5 * vi as f32).round())).mul(&Matrix { a: p.xscale / 100.0, d: p.yscale / 100.0, ..Matrix::default() });
            let (lab, secs) = l.anim.timeline(&format!("line/{id}"), now).map(|(s, x)| (s.to_string(), x)).unwrap_or(("default".into(), 0.0));
            let st = States::default().with_timed("*", &lab, secs).with("_bkgd_mc/mc", &format!("#{}", vi % 4));
            let a = la * p.alpha / 100.0;
            dl.movie(load, row, rm, &st, a);
            for f in DisplayList::fields(load, row, rm, &st) {
                let text = if f.path.contains("date") {
                    crate::saves::label(h, t)
                } else if f.path.contains("chapter") {
                    h.title.clone()
                } else {
                    continue;
                };
                let mut f = f;
                f.color.mul[3] *= a;
                dl.field_text(load, &f, &text, None);
            }
            let (cx, cy) = rm.apply(380.0 * 20.0, 0.0);
            dl.item(&id, Rect::from_center_size(Vec2::new(cx, cy) / 20.0, Vec2::new(753.0, 62.0)), true);
        }
    }
    let _ = focus;
    if let Some(c) = art.get("common") {
        HelpBar::draw(dl, c, &art.metrics, t, &l.anim, "lg/help", now, &[("A", "t_Validate"), ("B", "t_Back")]);
    }
}

fn origin_px() -> Matrix {
    px_matrix(640.0, 360.0)
}

// ---------------------------------------------------------------------------------------------
// Multiplayer (port screen in the options style)

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
        v.push(("host:map", "Map".into(), menu_maps::entry(&l.maps, &h.map).name, LineWidget::Stepper));
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

fn open_mp(l: &mut Launcher, art: &MenuArt, now: f64) {
    let a = &mut l.anim;
    a.forget("mp");
    a.author("mp", Props::default());
    a.now(now, "mp", Act::Set(P::new().alpha(0.0)));
    a.now(now, "mp", Act::To(P::new().alpha(100.0), 0.3, Ease::StrongOut));
    if let Some(c) = art.get("common") {
        TitleBar::open(a, c, "mp/title", Props { x: -545.0, y: -295.0, rotation: -4.0, ..Props::default() }, now);
    }
    HelpBar::open(a, "mp/help", now);
    l.input_from = now;
    l.last_focus = None;
}

fn build_mp(dl: &mut DisplayList, art: &MenuArt, l: &Launcher, t: &MenuTexts, gs: &GameSettings, nav: &MenuNav, now: f64) {
    let Some(o) = art.get("options") else { return };
    let focus = nav.focus.as_deref().unwrap_or("");
    let editing = nav.editing.as_ref();
    let a = l.anim.get("mp", now).alpha / 100.0;
    if let Some((ch, m)) = o.root_child("options_mc") {
        dl.movie(o, ch, m, &States::default(), a);
    }
    if let Some(c) = art.get("common") {
        TitleBar::draw(dl, c, &art.metrics, &l.anim, "mp/title", now, origin_px(), "Multiplayer");
    }
    // Join / Host tabs (`lib_mTabs_tab_s`, the options tabs).
    if let Some(common) = art.get("common") {
        if let Some(tab) = common.export("lib_mTabs_tab_s") {
            let tabs_m = origin_px().mul(&Props { y: -210.0, rotation: -4.0, ..Props::default() }.matrix());
            for (i, (id, label)) in [("mp:tab:join", "JOIN"), ("mp:tab:host", "HOST")].iter().enumerate() {
                let selected = (i == 1) == l.mp_host;
                let state = if selected { "selected" } else if focus == *id { "over" } else { "unselected" };
                let m = tabs_m.mul(&px_matrix(-103.0 + 206.0 * i as f32, if selected { -20.0 } else { 0.0 }));
                let st = States::default().with("tab_mc", state);
                dl.movie(common, tab, m, &st, a);
                for f in DisplayList::fields(common, tab, m, &st) {
                    let mut f = f;
                    f.color.mul[3] *= a;
                    dl.field_text(common, &f, label, None);
                }
                dl.item(*id, Rect::from_center_size(Vec2::new(m.tx, m.ty) / 20.0, Vec2::new(200.0, 50.0)), false);
            }
        }
    }
    let ed = |id: &str| editing.filter(|e| e.0 == id).map(|e| e.1.clone());
    let x0 = 190.0;
    if !l.mp_host {
        let mp = &gs.s.multiplayer;
        let mut y = 230.0;
        let mut idx = 0;
        for (id, label, value) in [("join:name", "Player name", mp.name.clone()), ("join:master", "Master server", mp.master.clone()), ("join:direct", "Direct connect", mp.last_connect.clone())] {
            let e = ed(id);
            generic_line_at(dl, o, x0, y, idx, id, label, e.as_deref().unwrap_or(&value), LineWidget::Text { editing: e.is_some() }, line_state(l, id, focus, now), a);
            y += GENERIC_LINE_H;
            idx += 1;
        }
        generic_line_at(dl, o, x0, y, idx, "join:connect", "Connect", &t.t(SEC_BASE, "t_Select"), LineWidget::Button, line_state(l, "join:connect", focus, now), a);
        y += GENERIC_LINE_H;
        idx += 1;
        generic_line_at(dl, o, x0, y, idx, "join:refresh", "Refresh server list", &l.browse_status, LineWidget::Button, line_state(l, "join:refresh", focus, now), a);
        y += GENERIC_LINE_H + 10.0;
        // Server list: server, map, mode, players, ping.
        const COLS: [f32; 6] = [200.0, 580.0, 800.0, 940.0, 1050.0, 1150.0];
        let row = |dl: &mut DisplayList, y: f32, cells: [String; 5], size: f32, col: Color| {
            for (i, c) in cells.into_iter().enumerate() {
                dl.text(c, Rect::new(COLS[i], y - 14.0, COLS[i + 1] - 10.0, y + 14.0), size, FontKind::Normal, col, Justify::Left);
            }
        };
        row(dl, y, ["SERVER".into(), "MAP".into(), "MODE".into(), "PLAYERS".into(), "PING".into()], 18.0, TEXT_DIM.with_alpha(a));
        y += 34.0;
        if l.servers.is_empty() {
            dl.text(t.t(SEC_BASE, "t_EmptyList"), Rect::new(200.0, y - 14.0, 1180.0, y + 14.0), 20.0, FontKind::Normal, TEXT.with_alpha(a), Justify::Left);
        }
        for (i, s) in l.servers.iter().enumerate().take(5) {
            let id = format!("join:server:{i}");
            let ping = s.ping_ms.map(|p| format!("{p:.0} ms")).unwrap_or_else(|| "-".into());
            if focus == id {
                dl.fill(Rect::new(190.0, y - 16.0, 1150.0, y + 16.0), Color::srgba(0.89, 0.95, 0.84, 0.15 * a));
            }
            row(dl, y, [truncate(&s.name, 34), truncate(&s.map, 20), s.mode.clone(), format!("{}/{}", s.players, s.max_players), ping], 20.0, TEXT.with_alpha(a));
            dl.item(&id, Rect::new(190.0, y - 16.0, 1150.0, y + 16.0), true);
            y += 34.0;
        }
    } else {
        let lines = host_lines(l, gs);
        let mut y = 200.0;
        for (i, (id, label, value, w)) in lines.iter().enumerate() {
            let e = ed(id);
            let w = match w {
                LineWidget::Text { .. } => LineWidget::Text { editing: e.is_some() },
                w => *w,
            };
            let v = if *id == "host:start" { l.server_status.clone() } else { e.unwrap_or_else(|| value.clone()) };
            generic_line_at(dl, o, x0, y, i, id, label, &v, w, line_state(l, id, focus, now), a);
            y += GENERIC_LINE_H - 8.0;
        }
        if let Some(s) = &l.server {
            let tail = s.tail(3).join("\n");
            dl.text(tail, Rect::new(200.0, y, 1180.0, y + 60.0), 15.0, FontKind::Normal, TEXT_DIM.with_alpha(a), Justify::Left);
        }
    }
    if let Some(c) = art.get("common") {
        HelpBar::draw(dl, c, &art.metrics, t, &l.anim, "mp/help", now, help_list("back"));
    }
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ---------------------------------------------------------------------------------------------
// Systems

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
    mut started: Local<bool>,
) {
    let Some(art) = art else { return };
    let Some(root) = l.root else { return };
    let now = time.elapsed_secs_f64();

    if !*started {
        *started = true;
        // Saves first (Continue / Load are entries of the bar).
        l.saves = dishonored_save::list(&saves_dir(&gs));
        open_start(&mut l, &art, now);
    }
    let _ = &mut sounds;
    // Animated screens rebuild every frame while anything moves.
    let moving = l.anim.busy(now, 2.0) || l.page == Page::Main || l.page == Page::Start || l.msg.is_some() || l.closing_page.is_some();
    let opt_moving = if l.page == Page::Options || l.closing_page.is_some_and(|c| c.0 == Page::Options) { l.options.tick(&nav, &mut sounds, now) } else { false };
    let bgs = bg.map(|b| b.state).unwrap_or("movie");
    let key = (nav.focus.clone(), nav.editing.clone(), nav.capturing.clone(), bgs, fonts.is_some());
    if !l.dirty && *last == key && !moving && !opt_moving {
        return;
    }
    *last = key;
    l.dirty = false;
    if let Some((p, t0)) = l.closing_page {
        let done = match p {
            Page::Options => l.options.closed(now),
            _ => now - t0 > 0.5,
        };
        if done {
            l.closing_page = None;
        }
    }
    let dl = build(&l, &art, &t, &gs, &nav, bgs, now);
    nav.set_items(dl.items.clone());
    // Keyboard focus starts on the screen's current selection (`_curSelectionIdx`).
    let want = match l.page {
        Page::Main => l.main_ids.get(l.main_sel).map(|s| s.to_string()),
        Page::Play if !nav.focus.as_deref().is_some_and(|f| f.starts_with("play:")) => Some(format!("play:mode:{}", l.play_mode)),
        Page::Missions | Page::FreeRoam => cards(&l, l.page).get(slide_sel(&l, l.page)).map(|c| c.id.clone()),
        _ => None,
    };
    if l.msg.is_none() {
        if let Some(w) = want {
            if nav.items.iter().any(|i| i.id == w) && nav.focus.as_deref() != Some(w.as_str()) && !(l.page == Page::Play && nav.focus.as_deref().is_some_and(|f| f.starts_with("play:"))) {
                nav.focus = Some(w);
            }
        }
    } else if !nav.focus.as_deref().is_some_and(|f| f.starts_with("msg:")) {
        let sel = l.msg.as_ref().map(|m| m.sel).unwrap_or(0);
        nav.focus = Some(format!("msg:btn:{sel}"));
    }
    let l = &mut *l;
    menu_art::spawn_list_cached(&mut commands, root, &dl, fonts.as_deref(), &mut l.cache);
}

/// Open `page` at `now` (its own opening sequence).
fn open_page(l: &mut Launcher, art: &MenuArt, page: Page, gs: &GameSettings, nav: &mut MenuNav, now: f64) {
    l.page = page;
    l.page_at = now;
    nav.focus = None;
    nav.editing = None;
    match page {
        Page::Play => open_play(l, art, now),
        Page::Missions | Page::FreeRoam => {
            if page == Page::FreeRoam {
                let want = gs.s.launcher.freeroam_map.clone();
                open_slide(l, art, page, now);
                if let Some(i) = l.maps.iter().position(|m| m.map.eq_ignore_ascii_case(&want)) {
                    slide_select(l, page, i, now + 0.15, false);
                }
            } else {
                open_slide(l, art, page, now);
            }
        }
        Page::Load => {
            l.saves = dishonored_save::list(&saves_dir(gs));
            l.save_sel = 0;
            open_load(l, art, now);
        }
        Page::Multiplayer => open_mp(l, art, now),
        Page::Options => {
            l.options.open(art, now, false);
            l.input_from = now;
        }
        Page::Main | Page::Start => {}
    }
    l.dirty = true;
}

/// Back from a sub screen: it closes, the main menu opens again (`BackToMainMenu`).
fn back_to_main(l: &mut Launcher, art: &MenuArt, t: &MenuTexts, sounds: &mut MenuSounds, nav: &mut MenuNav, now: f64) {
    let from = l.page;
    match from {
        Page::Play => close_play(l, now),
        Page::Missions | Page::FreeRoam => {
            TitleBar::close(&mut l.anim, "ms/title", now);
            HelpBar::close(&mut l.anim, "ms/help", now);
            l.anim.now(now, "ms", Act::To(P::new().alpha(0.0), 0.25, Ease::StrongOut));
        }
        Page::Load => {
            TitleBar::close(&mut l.anim, "lg/title", now);
            HelpBar::close(&mut l.anim, "lg/help", now);
            let x = l.anim.get("lg", now).x;
            l.anim.now(now, "lg", Act::To(P::new().alpha(0.0).x(x - 20.0), 0.25, Ease::StrongOut));
        }
        Page::Multiplayer => {
            TitleBar::close(&mut l.anim, "mp/title", now);
            HelpBar::close(&mut l.anim, "mp/help", now);
            l.anim.now(now, "mp", Act::To(P::new().alpha(0.0), 0.25, Ease::StrongOut));
        }
        _ => {}
    }
    l.closing_page = Some((from, now));
    l.page = Page::Main;
    if let Some(i) = l.main_ids.iter().position(|x| *x == format!("main:{}", from.name())) {
        l.main_sel = i;
    }
    nav.focus = None;
    open_main(l, art, t, sounds, now);
}

#[allow(clippy::too_many_arguments)]
fn handle(
    mut l: ResMut<Launcher>,
    mut nav: ResMut<MenuNav>,
    mut gs: ResMut<GameSettings>,
    mut sounds: ResMut<MenuSounds>,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    monitors: Query<&bevy::window::Monitor>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    art: Option<Res<MenuArt>>,
    t: Res<MenuTexts>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
    main_thread: NonSendMarker,
) {
    let now = time.elapsed_secs_f64();
    if l.poll() {
        // The game may have changed the settings (its pause menu's options).
        let p = gs.path.clone();
        if let Ok(s) = dishonored_settings::Settings::load(&p) {
            if s != gs.s {
                gs.s = s;
                gs.revision += 1;
            }
        }
    }
    let Some(art) = art else {
        nav.events.clear();
        return;
    };
    if l.options.resolutions.is_empty() {
        let mut res: Vec<(u32, u32)> = monitors.iter().flat_map(|m| m.video_modes.iter().map(|v| (v.physical_size.x, v.physical_size.y))).filter(|r| r.0 >= 1024).collect();
        res.sort();
        res.dedup();
        l.options.resolutions = res;
    }
    let window = windows.iter().next().map(|(e, w)| handoff::placement(e, w, &main_thread));
    // The confirm delay of a main-menu entry.
    if let Some(p) = l.pending.clone() {
        if now >= p.at {
            l.pending = None;
            run_main_entry(&mut l, &art, &t, &mut gs, &mut nav, &mut sounds, &p.id, window, &mut exit, now);
        }
    }
    // The message box closed: its answer.
    if let Some(idx) = l.msg.as_ref().and_then(|m| m.finished(now)) {
        let purpose = l.msg.take().map(|m| m.purpose).unwrap_or_default();
        answer(&mut l, &art, &t, &mut gs, &mut nav, &mut sounds, &purpose, idx, now, &mut exit);
    }
    let mut events = std::mem::take(&mut nav.events);
    // Held automation activations, once the screen takes input.
    if input_ready(&l, now) && !l.deferred.is_empty() {
        let mut d = std::mem::take(&mut l.deferred);
        d.append(&mut events);
        events = d;
    }
    // The start screen: any key or click.
    if l.page == Page::Start && input_ready(&l, now) && (keys.get_just_pressed().next().is_some() || mouse.get_just_pressed().next().is_some() || events.iter().any(|e| !matches!(e, MenuEvent::Focus(_) | MenuEvent::Scroll(_)))) {
        events.clear();
        sounds.play("M_PressStart");
        l.anim.now(now, "start", Act::To(P::new().alpha(0.0), 0.25, Ease::StrongOut));
        l.closing_page = Some((Page::Start, now));
        l.page = Page::Main;
        nav.focus = None;
        open_main(&mut l, &art, &t, &mut sounds, now);
        return;
    }
    for ev in events {
        if l.game_running() {
            continue;
        }
        if !input_ready(&l, now) {
            // Automation that arrives during an opening runs when the screen is ready.
            // (Pointer / key input is not gathered while the screen is locked, so a focus event
            // here also comes from automation.)
            if matches!(ev, MenuEvent::Activate { .. } | MenuEvent::Step { .. } | MenuEvent::Back | MenuEvent::Button(_) | MenuEvent::TextCommit { .. } | MenuEvent::Captured { .. } | MenuEvent::Focus(_)) && l.deferred.len() < 16 {
                l.deferred.push(ev);
            }
            continue;
        }
        // The confirmation box takes everything while it is up.
        if l.msg.is_some() {
            msg_event(&mut l, &ev, &mut sounds, now);
            continue;
        }
        if l.page == Page::Options {
            let out = l.options.handle(&ev, &mut gs, &mut nav, &mut sounds, now);
            match out {
                Outcome::Close => {
                    l.closing_page = Some((Page::Options, now));
                    back_to_main(&mut l, &art, &t, &mut sounds, &mut nav, now);
                }
                Outcome::Ask(purpose, payload) => {
                    let text = crate::menu_options::question(&t, &purpose, &payload);
                    ask(&mut l, &art, &t, &mut sounds, &format!("opt:{purpose}"), text, now);
                }
                _ => {}
            }
            if let MenuEvent::Captured { .. } = ev {
                l.options.capture_done(now);
            }
            l.dirty = true;
            continue;
        }
        match ev {
            MenuEvent::Focus(id) => focus_changed(&mut l, &id, &mut sounds, now),
            MenuEvent::Button("Stop") => sounds.play("Stop"),
            MenuEvent::Button(_) => {}
            MenuEvent::Back => {
                if l.page == Page::Main {
                    // `BPressed`: back to the start screen.
                    close_main(&mut l, now);
                    open_start(&mut l, &art, now + 0.0);
                } else {
                    let snd = if matches!(l.page, Page::Missions | Page::FreeRoam | Page::Load) { "back" } else { "Back" };
                    sounds.play(snd);
                    back_to_main(&mut l, &art, &t, &mut sounds, &mut nav, now);
                }
            }
            MenuEvent::Scroll(d) => {
                if matches!(l.page, Page::Missions | Page::FreeRoam) {
                    slide_step(&mut l, d, &mut sounds, now);
                }
            }
            MenuEvent::Captured { .. } => {}
            MenuEvent::TextCommit { id, text } => {
                commit_text(&mut gs, &id, &text);
                l.dirty = true;
            }
            MenuEvent::Step { id, dir } => {
                if matches!(l.page, Page::Missions | Page::FreeRoam) {
                    slide_step(&mut l, dir, &mut sounds, now);
                } else {
                    step(&mut l, &mut gs, &id, dir, &mut sounds);
                }
            }
            MenuEvent::Activate { id, .. } => activate(&mut l, &art, &t, &mut gs, &mut nav, &id, &mut sounds, window, &mut exit, now),
        }
    }
}

/// Focus moved to `id`: the hover / key selection of the current screen.
fn focus_changed(l: &mut Launcher, id: &str, sounds: &mut MenuSounds, now: f64) {
    sounds.play("Move");
    match l.page {
        Page::Main => {
            if let Some(i) = l.main_ids.iter().position(|x| *x == id) {
                if i != l.main_sel {
                    let old = format!("mm/btn/{}", l.main_sel);
                    main_select_out(&mut l.anim, &old, now);
                    l.main_sel = i;
                    main_select_on(&mut l.anim, &format!("mm/btn/{i}"), now);
                    let n = l.main_ids.len();
                    main_parallax(&mut l.anim, i, n, now);
                }
            }
        }
        Page::Play => {
            if let Some(i) = id.strip_prefix("play:mode:").and_then(|x| x.parse::<usize>().ok()) {
                if i != l.play_mode {
                    // `GenericMenu.playSelectionOn / Out` (x 25, Back.easeOut / back to 0).
                    let ko = format!("ng/btn/{}", l.play_mode);
                    l.anim.now(now, &ko, Act::To(P::new().scale(100.0).x(0.0).alpha(100.0), 0.25, Ease::StrongOut));
                    l.anim.now(now, &ko, Act::Play("out".into()));
                    let kn = format!("ng/btn/{i}");
                    l.anim.now(now, &kn, Act::To(P::new().scale(100.0).x(25.0).alpha(100.0), 0.25, Ease::BackOut));
                    l.anim.now(now, &kn, Act::Play("over".into()));
                    l.play_mode = i;
                }
            }
            line_focus(l, id, now);
        }
        Page::Missions | Page::FreeRoam => {
            let list = cards(l, l.page);
            if let Some(i) = list.iter().position(|c| c.id == id) {
                if i != slide_sel(l, l.page) {
                    let page = l.page;
                    slide_select(l, page, i, now, true);
                }
            }
        }
        _ => line_focus(l, id, now),
    }
    l.dirty = true;
}

/// Lines of the port's screens: the focused line plays `over`, the previous one `out`.
fn line_focus(l: &mut Launcher, id: &str, now: f64) {
    if l.last_focus.as_deref() == Some(id) {
        return;
    }
    if let Some(old) = l.last_focus.take() {
        l.anim.now(now, &format!("line/{old}"), Act::Play("out".into()));
        l.anim.now(now, &format!("line/{old}"), Act::To(P::new().x(0.0), 0.15, Ease::StrongOut));
    }
    let k = format!("line/{id}");
    l.anim.author(&k, Props::default());
    l.anim.now(now, &k, Act::Play("over".into()));
    if id.starts_with("load:item:") {
        l.anim.now(now, &k, Act::To(P::new().x(15.0), 0.25, Ease::StrongOut));
    }
    l.last_focus = Some(id.to_string());
}

/// Left / right on the card slide (no wrap: `Stop` at the ends).
fn slide_step(l: &mut Launcher, dir: i32, sounds: &mut MenuSounds, now: f64) {
    let page = l.page;
    let n = cards(l, page).len() as i32;
    let cur = slide_sel(l, page) as i32;
    let next = cur + dir.signum();
    if n == 0 || next < 0 || next >= n {
        sounds.play("Stop");
        return;
    }
    sounds.play("Move");
    slide_select(l, page, next as usize, now, false);
    l.dirty = true;
}

/// The confirmation box's input (PC: Enter confirms the focused button, left / right move
/// without wrapping; Esc does nothing).
fn msg_event(l: &mut Launcher, ev: &MenuEvent, sounds: &mut MenuSounds, now: f64) {
    let Some(m) = l.msg.as_mut() else { return };
    match ev {
        MenuEvent::Focus(id) => {
            if let Some(i) = id.strip_prefix("msg:btn:").and_then(|x| x.parse::<usize>().ok()) {
                if i != m.sel {
                    m.sel = i;
                    l.msg_focus = (i, now);
                    sounds.play("Move");
                }
            }
        }
        MenuEvent::Step { dir, .. } => {
            let s = m.step(*dir);
            sounds.play(s);
            if s == "Move" {
                l.msg_focus = (m.sel, now);
            }
        }
        MenuEvent::Activate { id, .. } => {
            let Some(i) = id.strip_prefix("msg:btn:").and_then(|x| x.parse::<usize>().ok()) else { return };
            m.sel = i.min(m.buttons.len().saturating_sub(1));
            sounds.play("Confirmation");
            m.confirm(&mut l.anim, now);
        }
        _ => {}
    }
    l.dirty = true;
}

/// A confirmation box's answer (0 = the first button, Yes).
#[allow(clippy::too_many_arguments)]
fn answer(l: &mut Launcher, art: &MenuArt, t: &MenuTexts, gs: &mut GameSettings, nav: &mut MenuNav, sounds: &mut MenuSounds, purpose: &str, idx: i32, now: f64, exit: &mut MessageWriter<AppExit>) {
    match purpose {
        // `OnQuitGameConfirm`: Yes quits, No reopens the main menu (`BackToMainMenu`).
        "quit" => {
            if idx == 0 {
                exit.write(AppExit::Success);
            } else {
                l.page = Page::Main;
                open_main(l, art, t, sounds, now);
            }
        }
        "load" => {
            if idx == 0 {
                if let Some((p, h)) = l.saves.get(l.save_sel).cloned() {
                    let args = vec!["--load".into(), p.display().to_string(), "--name".into(), gs.s.multiplayer.name.clone()];
                    l.launch(args, gs, None, &h.map);
                }
            }
        }
        p if p.starts_with("opt:") => {
            let _ = l.options.answer(&p[4..], idx, gs, now);
        }
        _ => {}
    }
    nav.focus = None;
    l.dirty = true;
}

/// Ask a Yes / No question (`MessageBoxInvoke.InvokeMessageBox`).
fn ask(l: &mut Launcher, art: &MenuArt, t: &MenuTexts, sounds: &mut MenuSounds, purpose: &str, text: String, now: f64) {
    let Some(g) = art.get("global") else {
        // No global movie: answer Yes.
        l.msg = Some(MsgBox { purpose: purpose.into(), text, buttons: vec![], sel: 0, opened: now, closing: Some((now - 1.0, 0)) });
        return;
    };
    let mb = MsgBox::open(&mut l.anim, g, purpose, text, vec![texts_any(t, "t_Yes"), texts_any(t, "t_No")], now);
    sounds.play("G_MsgBoxOpen");
    l.msg = Some(mb);
    l.msg_focus = (0, now);
}

/// What a main-menu entry does 150 ms after it was confirmed.
#[allow(clippy::too_many_arguments)]
fn run_main_entry(l: &mut Launcher, art: &MenuArt, t: &MenuTexts, gs: &mut GameSettings, nav: &mut MenuNav, sounds: &mut MenuSounds, id: &str, window: Option<handoff::Placement>, exit: &mut MessageWriter<AppExit>, now: f64) {
    let _ = exit;
    match id {
        "main:continue" => {
            // `bSkipCloseMenu`: the menu stays; the newest save loads.
            l.saves = dishonored_save::list(&saves_dir(gs));
            l.input_from = now;
            if let Some((p, h)) = l.saves.first().cloned() {
                l.launch(vec!["--load".into(), p.display().to_string(), "--name".into(), gs.s.multiplayer.name.clone()], gs, window, &h.map);
            }
        }
        "main:quit" => {
            // `OnQuitGameClicked`: the menu closes behind the question.
            close_main(l, now);
            ask(l, art, t, sounds, "quit", texts_any(t, "t_Q_QuitGame"), now);
        }
        x => {
            let page = match x {
                "main:play" => Page::Play,
                "main:missions" => Page::Missions,
                "main:freeroam" => Page::FreeRoam,
                "main:load" => Page::Load,
                "main:multiplayer" => Page::Multiplayer,
                "main:options" => Page::Options,
                _ => return,
            };
            close_main(l, now);
            open_page(l, art, page, gs, nav, now);
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

/// Maps the offline match and the server creator cycle through (free roam has them all).
fn match_maps(l: &Launcher) -> Vec<String> {
    l.maps.iter().filter(|m| m.map != "DishonoredGameFull_P").map(|m| m.map.clone()).collect()
}

fn step(l: &mut Launcher, gs: &mut GameSettings, id: &str, dir: i32, sounds: &mut MenuSounds) {
    let id = id.trim_end_matches(":-1").trim_end_matches(":+1");
    let maps = match_maps(l);
    let h = &mut gs.s.multiplayer.host;
    let wrap = |i: usize, n: usize| (i as i32 + dir).rem_euclid(n.max(1) as i32) as usize;
    match id {
        "play:bots" => {
            l.bots = (l.bots as i32 + dir).clamp(0, 15) as u32;
            gs.s.launcher.bots = l.bots;
        }
        "play:map" => {
            let i = maps.iter().position(|m| m.eq_ignore_ascii_case(&gs.s.launcher.play_map)).unwrap_or(0);
            if let Some(m) = maps.get(wrap(i, maps.len())) {
                gs.s.launcher.play_map = m.clone();
            }
        }
        "host:mode" => {
            let i = HOST_MODES.iter().position(|m| *m == h.mode).unwrap_or(0);
            h.mode = HOST_MODES[wrap(i, HOST_MODES.len())].into();
        }
        "host:map" => {
            let i = maps.iter().position(|m| m.eq_ignore_ascii_case(&h.map)).unwrap_or(0);
            if let Some(m) = maps.get(wrap(i, maps.len())) {
                h.map = m.clone();
            }
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
        x if x.starts_with("play:mode:") => {
            l.play_mode = wrap(l.play_mode, PLAY_MODES.len());
            gs.s.launcher.play_mode = l.play_mode;
        }
        x if x.starts_with("load:item:") && !l.saves.is_empty() => {
            l.save_sel = (l.save_sel as i32 + dir).clamp(0, l.saves.len() as i32 - 1) as usize;
        }
        _ => return,
    }
    gs.commit();
    sounds.play("change");
    l.dirty = true;
}

#[allow(clippy::too_many_arguments)]
fn activate(l: &mut Launcher, art: &MenuArt, t: &MenuTexts, gs: &mut GameSettings, nav: &mut MenuNav, id: &str, sounds: &mut MenuSounds, window: Option<handoff::Placement>, exit: &mut MessageWriter<AppExit>, now: f64) {
    l.dirty = true;
    // Arrow items of stepper lines.
    if id.ends_with(":-1") || id.ends_with(":+1") {
        return step(l, gs, id, if id.ends_with(":-1") { -1 } else { 1 }, sounds);
    }
    // Help bar entries act as their keys.
    if let Some(b) = id.strip_prefix("help:") {
        match b {
            "B" => {
                nav.events.push(MenuEvent::Back);
            }
            "A" => {
                if let Some(f) = nav.focus.clone() {
                    nav.events.push(MenuEvent::Activate { id: f, at: None });
                }
            }
            _ => {}
        }
        return;
    }
    if let Some(x) = id.strip_prefix("main:") {
        let _ = x;
        if l.page != Page::Main {
            return;
        }
        // Select it (a click selects then confirms), then `Confirm`: `Validation`, inputs off,
        // the choice 150 ms later.
        if let Some(i) = l.main_ids.iter().position(|e| *e == id) {
            if i != l.main_sel {
                focus_changed(l, id, sounds, now);
            }
        }
        sounds.play("Validation");
        l.pending = Some(Pending { at: now + CONFIRM_DELAY, id: id.to_string() });
        return;
    }
    match id {
        "back" => {
            nav.events.push(MenuEvent::Back);
        }
        "start:any" => {}
        x if x.starts_with("play:mode:") => {
            let i: usize = x.rsplit(':').next().and_then(|i| i.parse().ok()).unwrap_or(0);
            if i != l.play_mode {
                focus_changed(l, x, sounds, now);
            }
            l.play_mode = i;
            gs.s.launcher.play_mode = i;
            gs.commit();
            // A on a mode starts it (`Validation`, then 150 ms).
            sounds.play("Validation");
            start_play(l, gs, window);
        }
        "play:start" => {
            sounds.play("Validation");
            start_play(l, gs, window);
        }
        x if x.starts_with("missions:item:") => {
            let i: usize = x.rsplit(':').next().and_then(|i| i.parse().ok()).unwrap_or(0);
            if i == l.mission_sel {
                activate(l, art, t, gs, nav, "missions:launch", sounds, window, exit, now);
            }
        }
        "missions:launch" => {
            if let Some(m) = l.missions.get(l.mission_sel).cloned() {
                sounds.play("Validation");
                l.launch(vec!["--mission".into(), m.path.display().to_string(), "--name".into(), gs.s.multiplayer.name.clone()], gs, window, &m.map);
            } else {
                sounds.play("Failure");
            }
        }
        x if x.starts_with("freeroam:item:") => {
            let map = &x["freeroam:item:".len()..];
            let i = l.maps.iter().position(|m| m.map == map).unwrap_or(0);
            if i == l.freeroam_sel {
                activate(l, art, t, gs, nav, "freeroam:launch", sounds, window, exit, now);
            }
        }
        "freeroam:launch" => {
            if let Some(m) = l.maps.get(l.freeroam_sel).cloned() {
                sounds.play("Validation");
                gs.s.launcher.freeroam_map = m.map.clone();
                gs.commit();
                let mut args = vec!["--freeroam".into(), "--map".into(), m.map.clone(), "--name".into(), gs.s.multiplayer.name.clone()];
                if let Some(ev) = &m.start_event {
                    args.extend(["--start-event".into(), ev.clone()]);
                }
                l.launch(args, gs, window, &m.map);
            }
        }
        x if x.starts_with("load:item:") => {
            let i: usize = x.rsplit(':').next().and_then(|i| i.parse().ok()).unwrap_or(0);
            l.save_sel = i;
            // `LoadGameList.APressed`: `validation`, then `t_Q_LoadGameMainMenu`.
            sounds.play("validation");
            ask(l, art, t, sounds, "load", texts_any(t, "t_Q_LoadGameMainMenu"), now);
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
        x if x.starts_with("host:") || x == "play:bots" || x == "play:map" => step(l, gs, x, 1, sounds),
        _ => {}
    }
}

/// Start the offline match of the Play screen.
fn start_play(l: &mut Launcher, gs: &mut GameSettings, window: Option<handoff::Placement>) {
    let mut args = vec!["--name".to_string(), gs.s.multiplayer.name.clone()];
    let map = gs.s.launcher.play_map.clone();
    match l.play_mode {
        1 => args.extend(["--map".into(), map.clone(), "--mode".into(), "contracts".into(), "--bots".into(), l.bots.max(1).to_string()]),
        2 => args.extend(["--map".into(), map.clone(), "--spectate".into(), "--bots".into(), l.bots.to_string()]),
        3 => {
            args.push("--playground".into());
            l.launch(args, gs, window, "");
            return;
        }
        _ => args.extend(["--map".into(), map.clone(), "--bots".into(), l.bots.to_string()]),
    }
    l.launch(args, gs, window, &map);
}

/// Harness control socket (`--control PORT`).
#[allow(clippy::too_many_arguments)]
fn control(
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
            "state" => state_json(&l, &gs, &nav, bg.as_deref(), &sounds, &loading, &time, &script).to_string(),
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
            "button" => {
                let b = match v.get("button").and_then(|x| x.as_str()).unwrap_or("") {
                    "X" => "X",
                    "LB" => "LB",
                    "RB" => "RB",
                    "LT" => "LT",
                    _ => "RT",
                };
                nav.events.push(MenuEvent::Button(b));
                "{\"ok\":true}".into()
            }
            "focus" => {
                nav.focus = Some(id.clone());
                nav.events.push(MenuEvent::Focus(id));
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
}

#[allow(clippy::too_many_arguments)]
fn state_json(l: &Launcher, gs: &GameSettings, nav: &MenuNav, bg: Option<&MenuBackground>, sounds: &MenuSounds, loading: &crate::loading_screen::LoadingScreen, time: &Time<Real>, script: &LauncherScript) -> serde_json::Value {
    let now = time.elapsed_secs_f64();
    serde_json::json!({
        "page": l.page.name(),
        "page_open_s": now - l.page_at,
        "input_ready": input_ready(l, now),
        "message_box": l.msg.as_ref().map(|m| serde_json::json!({"purpose": m.purpose, "text": m.text, "buttons": m.buttons, "selected": m.sel})),
        "options": { "category": format!("{:?}", l.options.category()), "sub": format!("{:?}", l.options.sub_category()), "secondary": l.options.secondary.map(|s| format!("{s:?}")) },
        "main": { "selected": l.main_ids.get(l.main_sel), "entries": l.main_ids, "opened_s": l.opened_at.map(|o| now - o) },
        "focus": nav.focus,
        "editing": nav.editing.as_ref().map(|e| &e.0),
        "capturing": nav.capturing,
        "items": nav.items.iter().map(|i| &i.id).collect::<Vec<_>>(),
        "settings_path": gs.path.display().to_string(),
        "settings": serde_json::to_value(&gs.s).unwrap_or_default(),
        "effects": dishonored_settings::OPTIONS.iter().map(|o| (o.id, format!("{:?}", o.effect))).collect::<std::collections::BTreeMap<_, _>>(),
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
        "maps": l.maps.iter().map(|m| serde_json::json!({"map": m.map, "name": m.name, "image": m.image, "start_event": m.start_event, "redirected": m.redirected})).collect::<Vec<_>>(),
        "freeroam_selected": l.maps.get(l.freeroam_sel).map(|m| m.map.clone()),
        "sounds": { "loaded": sounds.loaded, "played": sounds.played, "recent": sounds.history() },
        "shots": script.log.iter().map(|(n, t, p, s)| serde_json::json!({"name": n, "t": t, "page": p, "page_open_s": s})).collect::<Vec<_>>(),
    })
}

/// `--script` runner (one step per frame, `wait_frames` / `wait_secs` pauses) and screenshot
/// pacing.
#[allow(clippy::too_many_arguments)]
fn run_script(mut commands: Commands, mut s: ResMut<LauncherScript>, mut nav: ResMut<MenuNav>, bg: Option<Res<MenuBackground>>, l: Res<Launcher>, time: Res<Time<Real>>, mut exit: MessageWriter<AppExit>) {
    let now = time.elapsed_secs_f64();
    if let Some(p) = (!s.shots.is_empty()).then(|| s.shots.remove(0)) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let name = p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        s.log.push((name, now, l.page.name(), now - l.page_at));
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        return;
    }
    if let Some(n) = s.quit_in.as_mut() {
        if *n == 0 {
            // Timing log of the shots next to them.
            if !s.log.is_empty() {
                let j = serde_json::json!({ "shots": s.log.iter().map(|(n, t, p, o)| serde_json::json!({"name": n, "t": t, "page": p, "page_open_s": o})).collect::<Vec<_>>(), "menu_drawn_s": l.first_drawn, "input_ready_s": l.first_input });
                let _ = std::fs::write(s.out.join("shots.json"), serde_json::to_string_pretty(&j).unwrap_or_default());
            }
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
    if let Some(u) = s.wait_until {
        if now < u {
            return;
        }
        s.wait_until = None;
    }
    let Some(step) = s.steps.get(s.index).cloned() else { return };
    if matches!(step, LStep::WaitLevel { .. }) && bg.as_ref().is_some_and(|b| b.state == "loading") {
        return;
    }
    if matches!(step, LStep::WaitInput { .. }) && !input_ready(&l, now) {
        return;
    }
    s.index += 1;
    match step {
        LStep::Activate { activate } => nav.events.push(MenuEvent::Activate { id: activate, at: None }),
        LStep::Step { step: (id, dir) } => nav.events.push(MenuEvent::Step { id, dir }),
        LStep::Back { .. } => nav.events.push(MenuEvent::Back),
        LStep::Button { button } => {
            let b = match button.as_str() {
                "X" => "X",
                "LB" => "LB",
                "RB" => "RB",
                "LT" => "LT",
                _ => "RT",
            };
            nav.events.push(MenuEvent::Button(b));
        }
        LStep::Focus { focus } => {
            nav.focus = Some(focus.clone());
            nav.events.push(MenuEvent::Focus(focus));
        }
        LStep::Text { text: (id, t) } => nav.events.push(MenuEvent::TextCommit { id, text: t }),
        LStep::Screenshot { screenshot } => {
            let p = s.out.join(format!("{screenshot}.png"));
            s.shots.push(p);
            s.wait = 2;
        }
        LStep::Wait { wait_frames } => s.wait = wait_frames,
        LStep::WaitSecs { wait_secs } => s.wait_until = Some(now + wait_secs as f64),
        LStep::WaitLevel { .. } => s.wait = 30,
        LStep::WaitInput { .. } => {}
        LStep::Quit { .. } => s.quit_in = Some(20),
    }
}
