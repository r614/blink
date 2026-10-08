//! Automation hooks so the game can be played and verified without a human:
//! - `--script file.json`: timed inputs, screenshots and state dumps
//! - `--control PORT`: line-delimited JSON commands over TCP (localhost only)
//! - `--record DIR --record-every N`: frame capture (encoded to MP4 with ffmpeg on exit)
//!
//! Script format:
//! ```json
//! {"steps":[
//!   {"ticks":60,"input":{"forward":127,"yaw":90,"buttons":["SPRINT"],"left_hand":"Blink"}},
//!   {"screenshot":"after_run"},
//!   {"dump":"state_after_run"},
//!   {"quit":true}
//! ]}
//! ```

use crate::input_map::LocalInput;
use crate::session::{Session, SessionView};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ScriptInput {
    pub forward: i8,
    pub right: i8,
    pub yaw: f32,
    pub pitch: f32,
    pub buttons: Vec<String>,
    pub left_hand: Option<LeftHand>,
}

impl ScriptInput {
    pub fn to_input(&self) -> PlayerInput {
        let mut b = 0u16;
        for name in &self.buttons {
            b |= match name.to_ascii_uppercase().as_str() {
                "JUMP" => buttons::JUMP,
                "CROUCH" => buttons::CROUCH,
                "SPRINT" => buttons::SPRINT,
                "WALK" => buttons::WALK,
                "PRIMARY" | "ATTACK" => buttons::PRIMARY,
                "SECONDARY" | "LEFT" => buttons::SECONDARY,
                "BLOCK" => buttons::BLOCK,
                "LEAN_LEFT" => buttons::LEAN_LEFT,
                "LEAN_RIGHT" => buttons::LEAN_RIGHT,
                "USE" => buttons::USE,
                "RELOAD" => buttons::RELOAD,
                "CHOICE_1" => buttons::CHOICE_1,
                "CHOICE_2" => buttons::CHOICE_2,
                "CHOICE_3" => buttons::CHOICE_3,
                "CHOICE_4" => buttons::CHOICE_4,
                _ => 0,
            };
        }
        PlayerInput { tick: 0, forward: self.forward, right: self.right, yaw: self.yaw, pitch: self.pitch, buttons: b, left_hand: self.left_hand.unwrap_or_default(), action: 0, npc_move: 0, npc_speed: 0.0 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Step {
    Input { ticks: u32, input: ScriptInput },
    Screenshot { screenshot: String },
    Dump { dump: String },
    Wait { wait_ticks: u32 },
    /// Inspect mode only: move the free camera (Dishonored units, degrees).
    Camera { camera: CameraStep },
    /// Offline sessions only: place the local player (Dishonored units, degrees).
    Teleport { teleport: CameraStep },
    /// Game camera: third person, follow a pawn id, orbit angle (degrees) around it.
    View { view: ViewStep },
    Quit { quit: bool },
}

#[derive(Debug, Clone, Deserialize)]
pub struct CameraStep {
    pub pos: [f32; 3],
    pub yaw: f32,
    #[serde(default)]
    pub pitch: f32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ViewStep {
    #[serde(default)]
    pub third_person: bool,
    #[serde(default)]
    pub follow: Option<u32>,
    #[serde(default)]
    pub orbit: f32,
    /// Camera distance and height above the eye (Bevy units; defaults 3.0 and 0.6).
    #[serde(default)]
    pub distance: Option<f32>,
    #[serde(default)]
    pub height: Option<f32>,
}

/// Camera view requested by a script `view` step (read by `players::camera_follow`).
#[derive(Resource, Default)]
pub struct ViewRequest {
    pub pending: Option<ViewStep>,
    pub orbit: Option<f32>,
    pub distance: Option<f32>,
    pub height: Option<f32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Script {
    pub steps: Vec<Step>,
}

#[derive(Resource, Default)]
pub struct ScriptRunner {
    pub steps: Vec<Step>,
    pub index: usize,
    pub ticks_left: u32,
    pub out_dir: PathBuf,
    pub done: bool,
}

#[derive(Resource, Default)]
pub struct ShotQueue {
    pub pending: Vec<PathBuf>,
    pub quit_in: Option<u32>,
    pub frames_since_last_shot: u32,
}

#[derive(Resource, Default)]
pub struct Recorder {
    pub dir: Option<PathBuf>,
    pub every: u32,
    pub frame: u32,
    pub written: u32,
}

/// Commands from the control socket, each with a reply channel.
pub struct ControlMsg {
    pub line: String,
    pub reply: Sender<String>,
}

#[derive(Resource)]
pub struct ControlRx(pub Mutex<Receiver<ControlMsg>>);

#[derive(Resource, Default)]
pub struct ControlOverride {
    pub ticks_left: Option<u32>,
}

pub fn start_control_server(port: u16) -> ControlRx {
    let (tx, rx) = channel::<ControlMsg>();
    std::thread::spawn(move || {
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("control port");
        eprintln!("control socket on 127.0.0.1:{port}");
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut w = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let (rtx, rrx) = channel();
                    if tx.send(ControlMsg { line, reply: rtx }).is_err() {
                        return;
                    }
                    let resp = rrx.recv_timeout(std::time::Duration::from_secs(30)).unwrap_or_else(|_| "{\"error\":\"timeout\"}".into());
                    if writeln!(w, "{resp}").is_err() {
                        return;
                    }
                }
            });
        }
    });
    ControlRx(Mutex::new(rx))
}

pub fn state_json(view: &SessionView, session: &Session) -> serde_json::Value {
    let p = |p: &dishonored_sim::state::PlayerState| {
        serde_json::json!({
            "id": p.id, "pos": [p.position.x, p.position.y, p.position.z], "vel": [p.velocity.x, p.velocity.y, p.velocity.z],
            "yaw": p.yaw, "pitch": p.pitch, "mode": format!("{:?}", p.mode), "crouched": p.crouched, "alive": p.alive,
            "health": p.vitals.health, "mana": p.vitals.mana, "sword": format!("{:?}", p.sword.phase),
            "left": format!("{:?}", p.left.item), "left_phase": format!("{:?}", p.left.phase),
            "blink_target": p.left.blink_target.map(|t| [t.x, t.y, t.z]),
            "pistol": [p.left.pistol_loaded, p.left.pistol_reserve as u8], "dark_vision": p.vitals.dark_vision_remaining,
            "kind": format!("{:?}", p.kind), "archetype": p.archetype,
            "stance": view.matchv.npc_poses.iter().find(|x| x.0 == p.id).map(|x| format!("{:?}", x.1)),
            "look": (p.archetype as usize).checked_sub(dishonored_multiplayer::mission::APPEARANCE_BASE as usize).and_then(|i| view.matchv.appearances.get(i)).and_then(|l| l.parts.first()).map(|b| b.1.clone()),
        })
    };
    serde_json::json!({
        "tick": view.tick, "status": view.status, "connected": view.connected,
        "rtt_ms": view.rtt_ms, "prediction_error": view.prediction_error, "correction": view.correction,
        "me": view.me.as_ref().map(p),
        "others": view.others.iter().map(p).collect::<Vec<_>>(),
        "projectiles": view.projectiles.len(),
        "match": { "phase": format!("{:?}", view.matchv.phase), "time_left": view.matchv.time_left, "winner": view.matchv.winner,
                   "scores": view.matchv.scores.iter().map(|s| serde_json::json!({"id": s.id, "name": s.name, "score": s.score, "kills": s.kills, "deaths": s.deaths})).collect::<Vec<_>>() },
        "feed": view.feed,
        "map": session.content.map_name,
    })
}

/// Fixed-tick side of scripting: choose the override input for this tick.
pub fn script_fixed(mut runner: ResMut<ScriptRunner>, mut local: ResMut<LocalInput>, mut shots: ResMut<ShotQueue>, view: Res<SessionView>, mut session: ResMut<Session>, mut ctrl: ResMut<ControlOverride>, mut vreq: ResMut<ViewRequest>) {
    if let Some(t) = ctrl.ticks_left.as_mut() {
        if *t == 0 {
            local.override_input = None;
            ctrl.ticks_left = None;
        } else {
            *t -= 1;
        }
        return;
    }
    if runner.done || runner.steps.is_empty() {
        return;
    }
    loop {
        if runner.index >= runner.steps.len() {
            runner.done = true;
            local.override_input = None;
            return;
        }
        if runner.ticks_left > 0 {
            runner.ticks_left -= 1;
            return;
        }
        let step = runner.steps[runner.index].clone();
        runner.index += 1;
        match step {
            Step::Input { ticks, input } => {
                local.override_input = Some(input.to_input());
                runner.ticks_left = ticks.saturating_sub(1);
                return;
            }
            Step::Wait { wait_ticks } => {
                runner.ticks_left = wait_ticks;
                return;
            }
            Step::Screenshot { screenshot } => {
                let p = runner.out_dir.join(format!("{screenshot}.png"));
                shots.pending.push(p);
            }
            Step::Dump { dump } => {
                let p = runner.out_dir.join(format!("{dump}.json"));
                let _ = std::fs::write(&p, serde_json::to_string_pretty(&state_json(&view, &session)).unwrap());
            }
            Step::Camera { .. } => {}
            Step::View { view } => {
                vreq.orbit = Some(view.orbit);
                vreq.distance = view.distance;
                vreq.height = view.height;
                vreq.pending = Some(view);
            }
            Step::Teleport { teleport } => {
                // The view comes from the local input (camera, and every later tick's input),
                // so the step turns it too; setting only the pawn's yaw / pitch was undone on the
                // next tick and the camera kept looking where it was.
                local.yaw = teleport.yaw;
                local.pitch = teleport.pitch;
                if let crate::session::SessionKind::Offline { game, .. } = &mut session.kind {
                    if let Some(p) = game.world_mut().player_mut(crate::session::LOCAL_ID) {
                        p.position = glam::Vec3::from(teleport.pos);
                        p.velocity = glam::Vec3::ZERO;
                        p.yaw = teleport.yaw;
                        p.pitch = teleport.pitch;
                    }
                }
            }
            Step::Quit { .. } => {
                runner.done = true;
                shots.quit_in = Some(30);
                return;
            }
        }
    }
}

/// Requested free-camera pose from an inspect-mode script (consumed by `inspect_cam`).
#[derive(Resource, Default)]
pub struct CameraRequest(pub Option<CameraStep>);

/// Inspect-mode script runner: one step per rendered frame (`wait_ticks` counts frames).
/// Supports `camera`, `screenshot`, `wait_ticks` and `quit`; used by `harness fidelity`.
pub fn inspect_script(
    mut commands: Commands,
    mut runner: ResMut<ScriptRunner>,
    mut shots: ResMut<ShotQueue>,
    mut cam: ResMut<CameraRequest>,
    mut exit: MessageWriter<AppExit>,
    mut prof: Option<ResMut<crate::profile::Profile>>,
) {
    if let Some(p) = shots.pending.first().cloned() {
        shots.pending.remove(0);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        shots.frames_since_last_shot = 0;
        return;
    }
    shots.frames_since_last_shot += 1;
    if let Some(n) = shots.quit_in.as_mut() {
        if *n == 0 {
            exit.write(AppExit::Success);
        } else {
            *n -= 1;
        }
        return;
    }
    if runner.done || runner.steps.is_empty() {
        return;
    }
    if runner.ticks_left > 0 {
        runner.ticks_left -= 1;
        return;
    }
    let Some(step) = runner.steps.get(runner.index).cloned() else {
        runner.done = true;
        return;
    };
    runner.index += 1;
    match step {
        Step::Wait { wait_ticks } => runner.ticks_left = wait_ticks,
        Step::Input { ticks, .. } => runner.ticks_left = ticks,
        Step::Camera { camera } => cam.0 = Some(camera),
        Step::Screenshot { screenshot } => {
            // `--profile`: frame statistics of this viewpoint (the frames since the last shot).
            if let Some(p) = prof.as_mut() {
                p.mark = Some(screenshot.clone());
            }
            shots.pending.push(runner.out_dir.join(format!("{screenshot}.png")))
        }
        Step::Dump { .. } | Step::Teleport { .. } | Step::View { .. } => {}
        Step::Quit { .. } => {
            runner.done = true;
            shots.quit_in = Some(30);
        }
    }
}

/// Frame side: screenshots, recording, control commands, delayed quit.
pub fn harness_frame(
    mut commands: Commands,
    mut shots: ResMut<ShotQueue>,
    mut rec: ResMut<Recorder>,
    ctrl_rx: Option<Res<ControlRx>>,
    mut local: ResMut<LocalInput>,
    mut ctrl: ResMut<ControlOverride>,
    view: Res<SessionView>,
    session: Res<Session>,
    settings: Option<Res<crate::settings::GameSettings>>,
    mut pause: Option<ResMut<crate::pause_menu::PauseMenu>>,
    fx: Option<Res<crate::effects::Effects>>,
    mut exit: MessageWriter<AppExit>,
    mut nav: Option<ResMut<crate::menu_ui::MenuNav>>,
) {
    shots.frames_since_last_shot += 1;
    // Pace screenshots: at most one per frame (the capture pipeline is asynchronous).
    if let Some(p) = shots.pending.first().cloned() {
        shots.pending.remove(0);
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        shots.frames_since_last_shot = 0;
    }
    if let Some(dir) = rec.dir.clone() {
        rec.frame += 1;
        if rec.frame % rec.every.max(1) == 0 {
            rec.written += 1;
            let p = dir.join(format!("frame_{:05}.png", rec.written));
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        }
    }
    if let Some(rx) = ctrl_rx {
        let rx = rx.0.lock().unwrap();
        while let Ok(msg) = rx.try_recv() {
            let v: serde_json::Value = serde_json::from_str(&msg.line).unwrap_or(serde_json::Value::Null);
            let cmd = v.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
            let reply = match cmd {
                "state" => {
                    // Plus the settings the game runs with and the pause menu state (launcher checks).
                    let mut j = state_json(&view, &session);
                    if let Some(s) = &settings {
                        j["settings"] = serde_json::json!({ "path": s.path.display().to_string(), "fov": s.s.graphics.fov, "mouse_sensitivity": s.s.mouse.sensitivity, "invert_y": s.s.mouse.invert_y, "jump_key": s.s.key_for("GBA_Jump"), "global_volume": s.s.audio.global });
                    }
                    j["paused"] = serde_json::json!(pause.as_ref().is_some_and(|p| p.open));
                    j.to_string()
                }
                "input" => {
                    let si: ScriptInput = serde_json::from_value(v.get("input").cloned().unwrap_or_default()).unwrap_or_default();
                    local.override_input = Some(si.to_input());
                    ctrl.ticks_left = Some(v.get("ticks").and_then(|t| t.as_u64()).unwrap_or(1) as u32);
                    "{\"ok\":true}".into()
                }
                "release" => {
                    local.override_input = None;
                    ctrl.ticks_left = None;
                    "{\"ok\":true}".into()
                }
                "screenshot" => {
                    let path = v.get("path").and_then(|p| p.as_str()).unwrap_or("shot.png");
                    shots.pending.push(PathBuf::from(path));
                    "{\"ok\":true,\"queued\":true}".into()
                }
                "effects" => {
                    // Gameplay effects asked for lately (path, spawned) and the live particle count.
                    let recent: Vec<serde_json::Value> = fx.as_ref().map(|f| f.recent.iter().map(|(p, ok)| serde_json::json!({"path": p, "spawned": ok})).collect()).unwrap_or_default();
                    serde_json::json!({
                        "recent": recent,
                        "spawned_oneshots": fx.as_ref().map(|f| f.spawned_oneshots),
                        "live_oneshots": fx.as_ref().map(|f| f.live_oneshots),
                        "live_particles": fx.as_ref().map(|f| f.live_particles),
                        "blink_indicator": fx.as_ref().and_then(|f| crate::effects::blink_indicator_live(f)),
                    })
                    .to_string()
                }
                "pause" => {
                    if let Some(p) = pause.as_mut() {
                        p.request = true;
                    }
                    "{\"ok\":true}".into()
                }
                "menu" => {
                    // Activate a pause-menu entry like a click (e.g. `pause:quit`, Back to Main Menu).
                    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if let Some(n) = nav.as_mut() {
                        n.events.push(crate::menu_ui::MenuEvent::Activate { id, at: None });
                    }
                    "{\"ok\":true}".into()
                }
                "quit" => {
                    shots.quit_in = Some(20);
                    "{\"ok\":true}".into()
                }
                _ => format!("{{\"error\":\"unknown cmd '{cmd}'\"}}"),
            };
            let _ = msg.reply.send(reply);
        }
    }
    let idle = shots.pending.is_empty();
    if let Some(n) = shots.quit_in.as_mut() {
        if idle {
            if *n == 0 {
                exit.write(AppExit::Success);
            } else {
                *n -= 1;
            }
        }
    }
}

/// Encode recorded frames into an MP4 with ffmpeg (if available).
pub fn encode_video(dir: &std::path::Path, fps: u32) -> Option<PathBuf> {
    let ffmpeg = std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".into());
    let out = dir.join("capture.mp4");
    let status = std::process::Command::new(ffmpeg)
        .args(["-y", "-loglevel", "error", "-framerate", &fps.to_string(), "-i"])
        .arg(dir.join("frame_%05d.png"))
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2"])
        .arg(&out)
        .status()
        .ok()?;
    status.success().then_some(out)
}
