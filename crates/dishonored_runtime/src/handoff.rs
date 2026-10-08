//! One window from menu to match: the launcher and the game it starts are two processes, but
//! the player sees one window, as in the original (main menu -> loading screen -> level ->
//! Back to Main Menu -> main menu).
//!
//! - The launcher shows the original loading screen for the chosen map and starts the game
//!   with `--from-launcher --window-pos X,Y --size WxH` (its own window's place and size).
//! - The game opens its window hidden at that place, renders a few frames, then shows it on
//!   top and prints [`SHOWN`]; the launcher hides its own window on that line.
//! - Back to Main Menu prints [`RETURN`] with the game window's place and size; the launcher
//!   shows its window there (under the game window, which closes right after). If the game
//!   exits any other way (crash, closed window, `--exit-after`), the launcher shows itself
//!   when the process ends.
//!
//! The lines go over the child's stdout, which the launcher already reads for its log.

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowMode, WindowPosition};

/// Printed by the game once its window is shown and has presented frames.
pub const SHOWN: &str = "BLINK_HANDOFF_SHOWN";
/// Printed by the game before it exits back to the menu: `BLINK_HANDOFF_RETURN x y w h`.
pub const RETURN: &str = "BLINK_HANDOFF_RETURN";
/// Printed by the game for the pause menu's Back to Windows (`t_BackToWindows`): the launcher
/// quits too.
pub const QUIT: &str = "BLINK_HANDOFF_QUIT";

/// Game side: Back to Windows. Tell the launcher, then exit.
pub fn quit_to_desktop(exit: &mut MessageWriter<AppExit>) {
    use std::io::Write;
    println!("{QUIT}");
    let _ = std::io::stdout().flush();
    exit.write(AppExit::Success);
}

/// Window placement handed between the processes (outer position, physical inner size).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub pos: Option<IVec2>,
    pub size: (u32, u32),
}

impl Placement {
    pub fn parse_return(line: &str) -> Option<Self> {
        let rest = line.split(RETURN).nth(1)?;
        let v: Vec<i64> = rest.split_whitespace().map_while(|x| x.parse().ok()).collect();
        if v.len() < 4 || v[2] <= 0 || v[3] <= 0 {
            return None;
        }
        Some(Self { pos: Some(IVec2::new(v[0] as i32, v[1] as i32)), size: (v[2] as u32, v[3] as u32) })
    }
    pub fn return_line(&self) -> String {
        let p = self.pos.unwrap_or_default();
        format!("{RETURN} {} {} {} {}", p.x, p.y, self.size.0, self.size.1)
    }
}

/// `--window-pos X,Y` from the command line.
pub fn window_pos_arg() -> Option<IVec2> {
    let a: Vec<String> = std::env::args().collect();
    let v = a.iter().position(|x| x == "--window-pos").and_then(|i| a.get(i + 1))?;
    let (x, y) = v.split_once(',')?;
    Some(IVec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// The primary window's outer position and physical size, read from winit (main thread).
pub fn placement(entity: Entity, w: &Window, _main: &NonSendMarker) -> Placement {
    let pos = bevy::winit::WINIT_WINDOWS.with_borrow(|ww| ww.get_window(entity).and_then(|x| x.outer_position().ok()).map(|p| IVec2::new(p.x, p.y)));
    let pos = pos.or(match w.position {
        WindowPosition::At(p) => Some(p),
        _ => None,
    });
    Placement { pos, size: (w.resolution.physical_width(), w.resolution.physical_height()) }
}

/// Game side: the window starts hidden; show it on top once frames are coming, then tell the
/// launcher.
#[derive(Resource, Default)]
pub struct Reveal {
    frames: u32,
    first: Option<f64>,
    shown_at: Option<u32>,
    done: bool,
}

/// Frames to render before showing the window, and after showing it before the launcher
/// steps aside (the shown window has presented by then).
const SHOW_AFTER_FRAMES: u32 = 4;
const SHOW_AFTER_SECS: f64 = 0.25;
const ANNOUNCE_AFTER_FRAMES: u32 = 3;

pub fn reveal(time: Res<Time<Real>>, mut r: ResMut<Reveal>, mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    if r.done {
        return;
    }
    r.frames += 1;
    let now = time.elapsed_secs_f64();
    let first = *r.first.get_or_insert(now);
    match r.shown_at {
        None if r.frames >= SHOW_AFTER_FRAMES && now - first >= SHOW_AFTER_SECS => {
            for mut w in &mut windows {
                w.visible = true;
                w.focused = true;
            }
            r.shown_at = Some(r.frames);
        }
        Some(f) if r.frames >= f + ANNOUNCE_AFTER_FRAMES => {
            use std::io::Write;
            println!("{SHOWN}");
            let _ = std::io::stdout().flush();
            r.done = true;
        }
        _ => {}
    }
}

/// Game side: Back to Main Menu (`request`), and the placement report.
#[derive(Resource, Default)]
pub struct ReturnToMenu {
    /// Set by the pause menu.
    pub request: bool,
    exit_at: Option<f64>,
    sent: bool,
}

/// How long the game window stays after announcing the return, so the launcher window is
/// already shown under it when it closes (no gap, focus passes to the launcher).
const RETURN_LINGER_SECS: f64 = 0.2;

fn announce(r: &mut ReturnToMenu, windows: &Query<(Entity, &Window), With<PrimaryWindow>>, main: &NonSendMarker) {
    if r.sent {
        return;
    }
    r.sent = true;
    use std::io::Write;
    let line = match windows.iter().next() {
        Some((e, w)) if matches!(w.mode, WindowMode::Windowed) => placement(e, w, main).return_line(),
        _ => RETURN.to_string(),
    };
    println!("{line}");
    let _ = std::io::stdout().flush();
}

pub fn return_to_menu(time: Res<Time<Real>>, mut r: ResMut<ReturnToMenu>, windows: Query<(Entity, &Window), With<PrimaryWindow>>, main: NonSendMarker, mut exit: MessageWriter<AppExit>) {
    if r.request && r.exit_at.is_none() {
        announce(&mut r, &windows, &main);
        r.exit_at = Some(time.elapsed_secs_f64() + RETURN_LINGER_SECS);
    }
    if r.exit_at.is_some_and(|t| time.elapsed_secs_f64() >= t) {
        exit.write(AppExit::Success);
    }
}

/// Game side, `Last`: any other exit (harness `quit`, `--exit-after`) also reports the
/// placement.
pub fn report_exit(mut exits: MessageReader<AppExit>, mut r: ResMut<ReturnToMenu>, windows: Query<(Entity, &Window), With<PrimaryWindow>>, main: NonSendMarker) {
    if exits.read().next().is_some() {
        announce(&mut r, &windows, &main);
    }
}
