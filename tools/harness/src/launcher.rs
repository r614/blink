//! `harness launcher`: drives the launcher (`dishonored_runtime --launcher`) through its
//! control socket like a player would:
//!
//! 1. scripted screenshots of every launcher screen (`fixtures/scripts/launcher_screens.json`);
//! 2. opens each screen and checks the launcher reports it; the main menu takes input when
//!    the original's does (0.58 s after it opens) and no later;
//! 3. changes settings through the options screen (FOV stepper, invert-Y toggle, Jump
//!    rebound to V) and checks the settings file on disk;
//! 4. starts an offline game from Play and checks, through the game's own control socket,
//!    that it runs with those settings, in one window (loading screen in the launcher
//!    window, then the game window in its place); opens the pause menu; Back to Main Menu
//!    shows the launcher again;
//! 5. hosts a server with the server creator (local master + `dishonored_server`), finds it
//!    in the Join browser through the master, joins it and checks the client connects.
//!
//! The settings file is private to the run (`DISHONORED_RS_SETTINGS`); the game children
//! get `--control PORT` through `DISHONORED_LAUNCHER_CHILD_ARGS`.

use crate::Ctx;
use crate::netsoak::{KillOnDrop, free_port};
use crate::report::Report;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const S: &str = "Launcher";

struct Control {
    w: TcpStream,
    r: BufReader<TcpStream>,
}

impl Control {
    fn connect(port: u16, timeout: Duration) -> Option<Self> {
        let t0 = Instant::now();
        while t0.elapsed() < timeout {
            if let Ok(s) = TcpStream::connect(("127.0.0.1", port)) {
                s.set_read_timeout(Some(Duration::from_secs(30))).ok();
                return Some(Self { w: s.try_clone().ok()?, r: BufReader::new(s) });
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        None
    }
    fn call(&mut self, v: Value) -> Value {
        if writeln!(self.w, "{v}").is_err() {
            return Value::Null;
        }
        let mut line = String::new();
        if self.r.read_line(&mut line).is_err() {
            return Value::Null;
        }
        serde_json::from_str(&line).unwrap_or(Value::Null)
    }
    fn state(&mut self) -> Value {
        self.call(json!({"cmd":"state"}))
    }
    fn activate(&mut self, id: &str) {
        self.call(json!({"cmd":"activate","id":id}));
        std::thread::sleep(Duration::from_millis(150));
    }
    /// Poll `state` until `pred` holds.
    fn wait(&mut self, secs: f32, pred: impl Fn(&Value) -> bool) -> (bool, Value) {
        let t0 = Instant::now();
        let mut last = Value::Null;
        while t0.elapsed() < Duration::from_secs_f32(secs) {
            last = self.state();
            if pred(&last) {
                return (true, last);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        (false, last)
    }
    fn screenshot(&mut self, path: &Path) {
        self.call(json!({"cmd":"screenshot","path": path.display().to_string()}));
        std::thread::sleep(Duration::from_millis(400));
    }
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let out = ctx.out.join("launcher");
    std::fs::create_dir_all(&out)?;
    let settings = out.join("settings.ron");
    let _ = std::fs::remove_file(&settings);

    // 1. Scripted screenshots of every screen.
    let shots = out.join("screens");
    let st = Command::new(ctx.bin("dishonored_runtime"))
        .args(["--launcher", "--size", "1280x720", "--script", "fixtures/scripts/launcher_screens.json", "--out", &shots.display().to_string(), "--exit-after", "150"])
        .env("DISHONORED_RS_SETTINGS", &settings)
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(out.join("script_launcher.log"))?))
        .status()
        .context("starting dishonored_runtime --launcher")?;
    let mut pngs: Vec<_> = std::fs::read_dir(&shots).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png")).collect()).unwrap_or_default();
    pngs.sort();
    rep.check(S, "scripted screenshots of every launcher screen", st.success() && pngs.len() >= 15, format!("{} screenshots, exit {:?}", pngs.len(), st.code()));
    for p in &pngs {
        rep.artifact(&format!("launcher {}", p.file_stem().unwrap_or_default().to_string_lossy()), p);
    }

    // 2. Interactive run.
    let cport = free_port();
    let gport = free_port();
    let log = std::fs::File::create(out.join("launcher.log"))?;
    let _launcher = KillOnDrop(
        Command::new(ctx.bin("dishonored_runtime"))
            .args(["--launcher", "--size", "1280x720", "--control", &cport.to_string(), "--exit-after", "600"])
            .env("DISHONORED_RS_SETTINGS", &settings)
            .env("DISHONORED_LAUNCHER_CHILD_ARGS", format!("--control {gport} --no-audio --exit-after 120"))
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()?,
    );
    let mut c = Control::connect(cport, Duration::from_secs(60)).context("launcher control socket")?;
    let (bg_ok, s0) = c.wait(90.0, |s| s["background"].as_str().is_some_and(|b| b != "loading"));
    rep.check(S, "launcher opens on the main menu with the 3D menu level", bg_ok && s0["page"] == "main" && s0["background"] == "level", format!("page {}, background {}", s0["page"], s0["background"]));
    let (snd, s1) = c.wait(20.0, |s| s["sounds"]["loaded"].as_bool() == Some(true));
    rep.check(S, "menu music and UI sound theme decoded", snd, format!("sounds {}", s1["sounds"]));
    // The original takes input 0.58 s after the main menu opens (`PlayOpenAnimation`: 180 ms,
    // then `MainMenuButtonBar.Open` 400 ms before `InitInputs`); nothing else may hold it back
    // (asset or menu-level loading). Allowance: a few frames at a busy machine's rate.
    let tm = &s1["timing"];
    let (drawn, ready) = (tm["menu_drawn_s"].as_f64(), tm["input_ready_s"].as_f64());
    let delay = drawn.zip(ready).map(|(d, r)| r - d);
    rep.check(
        S,
        "main menu takes input 0.58 s after it is drawn (the original's opening), not later",
        delay.is_some_and(|d| (0.57..=0.58 + 0.25).contains(&d)),
        format!("drawn at {:.2} s, input at {:.2} s ({}), hitches after drawing (s, ms) {}", drawn.unwrap_or(-1.0), ready.unwrap_or(-1.0), delay.map(|d| format!("{:.0} ms", d * 1000.0)).unwrap_or("-".into()), tm["hitches"]),
    );

    // Every screen.
    let mut visited = Vec::new();
    for (id, page) in [("main:play", "play"), ("main:missions", "missions"), ("main:multiplayer", "multiplayer"), ("main:options", "options")] {
        c.activate(id);
        let (ok, s) = c.wait(5.0, |s| s["page"] == page);
        visited.push(format!("{page}:{}", if ok { "ok" } else { "FAIL" }));
        c.screenshot(&out.join(format!("ctl_{page}.png")));
        if page == "options" {
            let mut cats = Vec::new();
            for i in 0..4 {
                c.activate(&format!("opt:tab:{i}"));
                let s = c.state();
                cats.push(s["options"]["category"].as_str().unwrap_or("?").to_string());
            }
            visited.push(format!("tabs {}", cats.join("/")));
        }
        c.call(json!({"cmd":"back"}));
        let _ = c.wait(5.0, |s| s["page"] == "main");
        let _ = s;
    }
    let all = visited.iter().all(|v| !v.contains("FAIL")) && visited.iter().any(|v| v.contains("General/Controls/Graphics/Audio"));
    rep.check(S, "every screen opens (Play, Missions, Multiplayer, Options and its four tabs)", all, visited.join(", "));
    let played = c.state()["sounds"]["played"].as_u64().unwrap_or(0);
    rep.check(S, "UI sound theme plays on navigation", played > 0, format!("{played} UI sounds posted"));
    let missions = c.state()["missions"].as_array().map(|a| a.len()).unwrap_or(0);
    rep.check(S, "missions screen lists missions/*.ron", missions >= 1, format!("{missions} missions"));

    // 3. Settings through the options screen.
    c.activate("main:options");
    c.activate("opt:tab:2");
    for _ in 0..3 {
        c.call(json!({"cmd":"step","id":"opt:line:PSI_GraphicsPC_FOV","dir":1}));
        std::thread::sleep(Duration::from_millis(100));
    }
    // Video Settings screen: windowed (the original default is fullscreen), then Accept.
    c.activate("opt:line:t_Modify_VideoSettings");
    let video = c.state()["options"]["secondary"] == "Video";
    c.call(json!({"cmd":"step","id":"opt:line:PSI_GraphicsPC_bFullScreen","dir":1}));
    std::thread::sleep(Duration::from_millis(100));
    c.activate("video:accept");
    c.activate("opt:tab:1");
    c.activate("opt:sub:1");
    c.activate("opt:line:PSI_Mouse_bInvertY");
    c.activate("opt:sub:0");
    c.activate("opt:bind:GBA_Jump");
    let capturing = c.state()["capturing"].as_str().map(String::from);
    c.call(json!({"cmd":"capture","id":"opt:bind:GBA_Jump","key":"V"}));
    std::thread::sleep(Duration::from_millis(300));
    c.screenshot(&out.join("ctl_options_rebound.png"));
    c.call(json!({"cmd":"back"}));
    let disk = dishonored_settings::Settings::load(&settings).unwrap_or_default();
    let ok = video && !disk.graphics.fullscreen && disk.graphics.fov == 90.0 && disk.mouse.invert_y && disk.key_for("GBA_Jump") == Some("V");
    rep.check(S, "options change persisted to the settings file", ok, format!("video screen {video} fullscreen {} fov {} invert {} jump {:?} (capture state {:?}) in {}", disk.graphics.fullscreen, disk.graphics.fov, disk.mouse.invert_y, disk.key_for("GBA_Jump"), capturing, settings.display()));

    // 4. A launched game picks the settings up.
    c.activate("main:play");
    c.activate("play:start");
    let t_launch = Instant::now();
    let (running, sl) = c.wait(10.0, |s| s["game_running"].as_bool() == Some(true));
    rep.check(S, "starting a game puts the original loading screen up in the launcher window", running && sl["loading_screen"].as_bool() == Some(true) && sl["launcher_visible"].as_bool() == Some(true), format!("loading screen {} launcher visible {}", sl["loading_screen"], sl["launcher_visible"]));
    c.screenshot(&out.join("ctl_loading.png"));
    // One window: the game's window replaces the launcher's once it renders.
    let (handed, sh) = c.wait(120.0, |s| s["game_window_shown"].as_bool() == Some(true) && s["launcher_visible"].as_bool() == Some(false));
    let t_handed = t_launch.elapsed().as_secs_f32();
    std::thread::sleep(Duration::from_millis(500));
    let sh2 = c.state();
    rep.check(
        S,
        "the game window takes the launcher window's place (launcher hidden, loading screen down)",
        handed && sh2["launcher_visible"].as_bool() == Some(false) && sh2["loading_screen"].as_bool() == Some(false),
        format!("game window shown {} after {t_handed:.1} s; launcher visible {} loading screen {}", sh["game_window_shown"], sh2["launcher_visible"], sh2["loading_screen"]),
    );
    let mut game_ok = false;
    let mut detail = String::from("game did not start");
    let mut via_menu = false;
    if running {
        if let Some(mut g) = Control::connect(gport, Duration::from_secs(90)) {
            let (ok, s) = g.wait(90.0, |s| !s["me"].is_null() && s["settings"]["fov"].as_f64() == Some(90.0));
            game_ok = ok && s["settings"]["invert_y"].as_bool() == Some(true) && s["settings"]["jump_key"] == "V";
            detail = format!("game settings {} ", s["settings"]);
            std::thread::sleep(Duration::from_secs(3));
            g.screenshot(&out.join("game_offline.png"));
            g.call(json!({"cmd":"pause"}));
            let (paused, _) = g.wait(15.0, |s| s["paused"].as_bool() == Some(true));
            std::thread::sleep(Duration::from_millis(500));
            g.screenshot(&out.join("game_pause_menu.png"));
            rep.check(S, "in-game pause menu opens (offline match paused)", paused, String::new());
            std::thread::sleep(Duration::from_secs(1));
            // Back to Main Menu, as the player picks it.
            g.call(json!({"cmd":"menu","id":"pause:quit"}));
            via_menu = true;
        }
    }
    rep.check(S, "game launched from the launcher runs with the changed settings", game_ok, detail);
    let t_back = Instant::now();
    let (shown_again, sv) = c.wait(30.0, |s| s["launcher_visible"].as_bool() == Some(true) && s["in_game"].as_bool() == Some(false));
    let t_shown = t_back.elapsed().as_secs_f32();
    let (back, s) = c.wait(30.0, |s| s["game_running"].as_bool() == Some(false));
    std::thread::sleep(Duration::from_millis(300));
    let s_after = c.state();
    rep.check(
        S,
        "Back to Main Menu returns to the launcher's main menu in the same place (no loading screen left up)",
        via_menu && shown_again && back && s_after["page"] == "main" && s_after["loading_screen"].as_bool() == Some(false) && s_after["launcher_visible"].as_bool() == Some(true),
        format!("menu shown after {t_shown:.2} s (page {}), game exited {back} (last exit {}), loading screen {}", sv["page"], s["last_game_exit"], s_after["loading_screen"]),
    );
    c.screenshot(&out.join("ctl_back_to_menu.png"));
    for n in ["ctl_play", "ctl_missions", "ctl_multiplayer", "ctl_options", "ctl_options_rebound", "ctl_loading", "game_offline", "game_pause_menu", "ctl_back_to_menu"] {
        let p = out.join(format!("{n}.png"));
        if p.exists() {
            rep.artifact(n, &p);
        }
    }

    // 5. Host + join through the launcher.
    let sport = free_port();
    let mport = free_port();
    let master = format!("127.0.0.1:{mport}");
    c.call(json!({"cmd":"back"}));
    c.activate("main:multiplayer");
    c.activate("mp:tab:host");
    c.call(json!({"cmd":"text","id":"host:port","text": sport.to_string()}));
    c.call(json!({"cmd":"text","id":"host:master","text": master}));
    c.call(json!({"cmd":"text","id":"host:name","text":"harness launcher server"}));
    std::thread::sleep(Duration::from_millis(200));
    c.activate("host:start");
    let (up, s) = c.wait(90.0, |s| s["server"]["status"] == "running");
    c.screenshot(&out.join("ctl_host_running.png"));
    rep.check(S, "server creator starts dishonored_server (+ local master)", up && s["master_running"].as_bool() == Some(true), format!("status {} master {} log {:?}", s["server"]["status"], s["master_running"], s["server"]["log"].as_array().map(|a| a.iter().rev().take(2).collect::<Vec<_>>())));
    c.activate("mp:tab:join");
    c.call(json!({"cmd":"text","id":"join:master","text": master}));
    let mut found: Option<usize> = None;
    let t0 = Instant::now();
    let mut s = Value::Null;
    while found.is_none() && t0.elapsed() < Duration::from_secs(30) {
        c.activate("join:refresh");
        let (_, st) = c.wait(6.0, |s| !s["browse_status"].as_str().unwrap_or("").starts_with("Refreshing"));
        s = st;
        found = s["servers"].as_array().and_then(|a| a.iter().position(|x| x["addr"].as_str().is_some_and(|ad| ad.ends_with(&format!(":{sport}")))));
        if found.is_none() {
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    c.screenshot(&out.join("ctl_join_list.png"));
    rep.check(S, "server browser lists the hosted server through the master", found.is_some(), format!("{} / {}", s["browse_status"], s["servers"]));
    let mut joined = false;
    let mut jdetail = "not attempted".to_string();
    if let Some(i) = found {
        c.activate(&format!("join:server:{i}"));
        if c.wait(10.0, |s| s["game_running"].as_bool() == Some(true)).0 {
            if let Some(mut g) = Control::connect(gport, Duration::from_secs(90)) {
                let (ok, gs) = g.wait(90.0, |s| s["connected"].as_bool() == Some(true) && !s["me"].is_null());
                joined = ok;
                jdetail = format!("status {} rtt {}", gs["status"], gs["rtt_ms"]);
                std::thread::sleep(Duration::from_secs(3));
                g.screenshot(&out.join("game_joined.png"));
                g.call(json!({"cmd":"quit"}));
            }
        }
    }
    rep.check(S, "joining from the browser connects a client to the hosted server", joined, jdetail);
    let _ = c.wait(30.0, |s| s["game_running"].as_bool() == Some(false));
    c.activate("mp:tab:host");
    c.activate("host:start");
    let (stopped, _) = c.wait(10.0, |s| s["server"]["status"] == "stopped");
    rep.check(S, "server creator stops the server", stopped, String::new());
    for n in ["ctl_host_running", "ctl_join_list", "game_joined"] {
        let p = out.join(format!("{n}.png"));
        if p.exists() {
            rep.artifact(n, &p);
        }
    }
    c.call(json!({"cmd":"quit"}));
    std::thread::sleep(Duration::from_secs(1));
    Ok(())
}
