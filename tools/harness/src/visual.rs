//! Visual verification: a rendered client joins a real server with bots and is played by
//! the harness through the runtime's control socket while recording video; plus the
//! scripted offline smoke run with screenshots and state dumps.

use crate::Ctx;
use crate::netsoak::{KillOnDrop, PROFILES, free_port, run_bot, spawn_server};
use crate::report::Report;
use anyhow::{Context, Result};
use dishonored_content::{DEFAULT_MAP, load_match};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const S: &str = "Visual / rendered client";

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
    fn call(&mut self, v: Value) -> Option<Value> {
        writeln!(self.w, "{v}").ok()?;
        let mut line = String::new();
        self.r.read_line(&mut line).ok()?;
        serde_json::from_str(&line).ok()
    }
}

fn ffmpeg() -> String {
    std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".into())
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let secs: f32 = ctx.opt("--seconds").and_then(|s| s.parse().ok()).unwrap_or(25.0);
    let map = ctx.opt("--map").unwrap_or_else(|| DEFAULT_MAP.into());
    let mc = Arc::new(load_match(&map)?);

    // Server + 3 bots.
    let sport = free_port();
    let _server = spawn_server(ctx, sport, None, secs as u64 + 40, &["--score-limit", "30", "--max-players", "8"], "visual_server.log")?;
    std::thread::sleep(Duration::from_secs(2));
    let server_addr: std::net::SocketAddr = format!("127.0.0.1:{sport}").parse()?;
    let bot_secs = secs + 25.0;
    let bots: Vec<_> = (0..3)
        .map(|i| {
            let mc = mc.clone();
            std::thread::spawn(move || run_bot(server_addr, 10 + i, PROFILES[i % 3], bot_secs, mc))
        })
        .collect();

    // Rendered client, played through the control socket, recording.
    let video_dir = ctx.out.join("video");
    let cport = free_port();
    let log = std::fs::File::create(ctx.out.join("viewer.log"))?;
    let mut viewer = KillOnDrop(
        Command::new(ctx.bin("dishonored_runtime"))
            .args(["--connect", &server_addr.to_string(), "--name", "harness-viewer", "--control", &cport.to_string()])
            .args(["--record", &video_dir.display().to_string(), "--record-every", "3", "--record-fps", "20", "--size", "1280x720"])
            .args(["--exit-after", &(secs + 20.0).to_string()])
            .env("FFMPEG", ffmpeg())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()
            .context("starting dishonored_runtime")?,
    );
    let mut ctl = Control::connect(cport, Duration::from_secs(60)).context("control socket")?;
    // Wait for the connection to be established.
    let t0 = Instant::now();
    let mut connected = false;
    while t0.elapsed() < Duration::from_secs(30) {
        if let Some(s) = ctl.call(serde_json::json!({"cmd":"state"})) {
            if s["connected"].as_bool() == Some(true) && !s["me"].is_null() {
                connected = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    rep.check(S, "rendered client connects to dedicated server", connected, format!("after {:.1?}", t0.elapsed()));

    // Play: chase the nearest opponent, attack in range, Blink when far; screenshot milestones.
    let shots_dir = ctx.out.join("screens");
    std::fs::create_dir_all(&shots_dir)?;
    let start = Instant::now();
    let mut shot_i = 0;
    let mut last_shot = Instant::now() - Duration::from_secs(10);
    let mut first_pos = None;
    let mut last_state = Value::Null;
    let mut max_rtt: f64 = 0.0;
    while start.elapsed() < Duration::from_secs_f32(secs) {
        let Some(s) = ctl.call(serde_json::json!({"cmd":"state"})) else { break };
        let me = &s["me"];
        if me.is_null() {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        max_rtt = max_rtt.max(s["rtt_ms"].as_f64().unwrap_or(0.0));
        let pos: Vec<f64> = me["pos"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()).unwrap_or_default();
        if first_pos.is_none() {
            first_pos = Some(pos.clone());
        }
        let nearest = s["others"].as_array().and_then(|o| {
            o.iter()
                .filter(|p| p["alive"].as_bool() == Some(true))
                .filter_map(|p| {
                    let q: Vec<f64> = p["pos"].as_array()?.iter().filter_map(|v| v.as_f64()).collect();
                    let d = ((q[0] - pos[0]).powi(2) + (q[1] - pos[1]).powi(2)).sqrt();
                    Some((q, d))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
        });
        let input = match nearest {
            Some((q, d)) => {
                let yaw = (q[1] - pos[1]).atan2(q[0] - pos[0]).to_degrees();
                let pitch = ((q[2] - pos[2]) / d.max(1.0)).atan().to_degrees();
                let mut buttons = vec![];
                let mut left = "Blink";
                if d < 180.0 {
                    buttons.push("PRIMARY");
                } else if d > 900.0 && (start.elapsed().as_millis() / 700) % 5 == 0 {
                    buttons.push("SECONDARY");
                } else if d < 700.0 && (start.elapsed().as_millis() / 900) % 4 == 0 {
                    left = "Pistol";
                    buttons.push("SECONDARY");
                }
                if d > 500.0 {
                    buttons.push("SPRINT");
                }
                serde_json::json!({"forward": if d > 140.0 {127} else {0}, "yaw": yaw, "pitch": pitch, "buttons": buttons, "left_hand": left})
            }
            None => serde_json::json!({"forward": 100, "yaw": (start.elapsed().as_secs_f64() * 20.0) % 360.0}),
        };
        ctl.call(serde_json::json!({"cmd":"input","ticks":6,"input": input}));
        if last_shot.elapsed() > Duration::from_secs_f32(secs / 6.0) {
            shot_i += 1;
            let p = shots_dir.join(format!("online_{shot_i:02}.png"));
            ctl.call(serde_json::json!({"cmd":"screenshot","path": p.display().to_string()}));
            last_shot = Instant::now();
        }
        last_state = s;
        std::thread::sleep(Duration::from_millis(100));
    }
    let moved = match (&first_pos, last_state["me"]["pos"].as_array()) {
        (Some(a), Some(b)) => {
            let b: Vec<f64> = b.iter().filter_map(|v| v.as_f64()).collect();
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
        }
        _ => 0.0,
    };
    rep.check(S, "harness-driven player moves through the map", moved > 300.0, format!("moved {moved:.0} uu, max RTT {max_rtt:.0} ms"));
    let others = last_state["others"].as_array().map(|a| a.len()).unwrap_or(0);
    rep.check(S, "remote players replicated to rendered client", others >= 3, format!("{others} remote players visible"));
    let state_path = ctx.out.join("viewer_final_state.json");
    std::fs::write(&state_path, serde_json::to_string_pretty(&last_state)?)?;
    rep.artifact("viewer final state", &state_path);
    ctl.call(serde_json::json!({"cmd":"quit"}));
    // The runtime encodes the MP4 after its window closes.
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(120) {
        if let Ok(Some(_)) = viewer.0.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let mp4 = video_dir.join("capture.mp4");
    let frames = std::fs::read_dir(&video_dir).map(|r| r.filter(|e| e.as_ref().is_ok_and(|e| e.path().extension().is_some_and(|x| x == "png"))).count()).unwrap_or(0);
    rep.check(S, "gameplay video recorded", mp4.exists() || frames > 20, format!("{frames} frames, mp4 {}", mp4.exists()));
    if mp4.exists() {
        rep.artifact("gameplay video (MP4)", &mp4);
        let sheet = ctx.out.join("contact_sheet.png");
        let ok = Command::new(ffmpeg())
            .args(["-y", "-loglevel", "error", "-i"])
            .arg(&mp4)
            .args(["-vf", "fps=1/2,scale=426:-1,tile=4x3", "-frames:v", "1"])
            .arg(&sheet)
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            rep.artifact("contact sheet (1 frame / 2 s)", &sheet);
        }
    }
    for i in 1..=shot_i {
        let p = shots_dir.join(format!("online_{i:02}.png"));
        if p.exists() {
            rep.artifact(&format!("online screenshot {i}"), &p);
        }
    }
    for b in bots {
        let _ = b.join();
    }

    // Scripted offline smoke run.
    let smoke = ctx.out.join("smoke");
    let script = std::path::Path::new("fixtures/scripts/smoke_offline.json");
    let st = Command::new(ctx.bin("dishonored_runtime"))
        .args(["--bots", "3", "--script", &script.display().to_string(), "--out", &smoke.display().to_string(), "--size", "1280x720"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    let read = |n: &str| -> Value { std::fs::read_to_string(smoke.join(n)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null) };
    let spawn = read("01_spawn.json");
    let sprint = read("02_after_sprint.json");
    let blink = read("04_after_blink.json");
    let p = |v: &Value| -> Vec<f64> { v["me"]["pos"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default() };
    let (a, b) = (p(&spawn), p(&sprint));
    let ran = if a.len() == 3 && b.len() == 3 { ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt() } else { 0.0 };
    rep.check(S, "scripted offline run: sprint 2 s covers ground", st.success() && ran > 400.0, format!("{ran:.0} uu"));
    let mana = blink["me"]["mana"].as_f64().unwrap_or(-1.0);
    rep.check(S, "scripted offline run: Blink spends 20 mana", (mana - 80.0).abs() < 0.5, format!("mana {mana}"));
    for n in ["01_spawn", "02_after_sprint", "03_blink_aim", "04_after_blink", "05_swing"] {
        let f = smoke.join(format!("{n}.png"));
        if f.exists() {
            rep.artifact(&format!("offline screenshot {n}"), &f);
        }
    }
    Ok(())
}
