//! Debug HUD and scoreboard: position, velocity, speed, movement state, health, mana,
//! weapon, power, tick, ping, prediction error, match state.

use crate::input_map::LocalInput;
use crate::session::{Session, SessionView};
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use dishonored_multiplayer::MatchPhase;

#[derive(Component)]
pub struct HudText;
#[derive(Component)]
pub struct Crosshair;
#[derive(Component)]
pub struct Scoreboard;

pub fn setup_hud(mut commands: Commands) {
    commands.spawn((
        Text::new(""),
        TextFont { font_size: FontSize::Px(14.0), ..default() },
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        Node { position_type: PositionType::Absolute, top: px(8), left: px(8), ..default() },
        HudText,
    ));
    commands.spawn((
        Text::new(""),
        TextFont { font_size: FontSize::Px(16.0), ..default() },
        TextColor(Color::srgb(1.0, 0.95, 0.8)),
        Node { position_type: PositionType::Absolute, top: px(8), right: px(12), ..default() },
        Scoreboard,
    ));
}

pub fn update_hud(
    diag: Res<DiagnosticsStore>,
    view: Res<SessionView>,
    session: Res<Session>,
    local: Res<LocalInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mut debug: ResMut<DebugHud>,
    mut hud: Query<&mut Text, (With<HudText>, Without<Scoreboard>)>,
    mut board: Query<&mut Text, (With<Scoreboard>, Without<HudText>)>,
) {
    if keys.just_pressed(KeyCode::F1) {
        debug.0 = !debug.0;
    }
    let fps = diag.get(&FrameTimeDiagnosticsPlugin::FPS).and_then(|d| d.smoothed()).unwrap_or(0.0);
    // The game HUD (gfx_hud) shows gauges, item and crosshair; this text is the F1 debug
    // readout plus match information the original HUD has no slot for.
    let mut s = if debug.0 { format!("{}  |  fps {fps:.0}  tick {}  ping {:.0} ms  pred err {:.1}  corr {:.1}\n", view.status, view.tick, view.rtt_ms, view.prediction_error, view.correction) } else { String::new() };
    if view.me.as_ref().is_some_and(|m| !m.alive) {
        s += &match view.respawn_in {
            Some(t) => format!("YOU DIED - respawn in {t:.1}s\n"),
            None => "YOU DIED\n".into(),
        };
    }
    if let Some(me) = view.me.as_ref().filter(|_| debug.0) {
        let speed = me.velocity.truncate().length();
        let ammo = match local.left_hand {
            dishonored_sim::input::LeftHand::Pistol => format!(" [{}|{}]", me.left.pistol_loaded, me.left.pistol_reserve),
            dishonored_sim::input::LeftHand::Crossbow => format!(" [{}|{}]", me.left.bolts_loaded, me.left.bolts_reserve),
            _ => String::new(),
        };
        s += &format!(
            "HEALTH {:.0}/{:.0}   MANA {:.0}/{:.0}   left hand {:?}{ammo} ({:?})   sword {:?}\n",
            me.vitals.health,
            session.content.sim.gameplay.vitals.health_max,
            me.vitals.mana,
            session.content.sim.gameplay.vitals.mana_max,
            local.left_hand,
            me.left.phase,
            me.sword.phase
        );
        s += &format!(
            "pos {:.0}, {:.0}, {:.0}   vel {:.0}, {:.0}, {:.0}   speed {speed:.0}   {:?}{}{}\n",
            me.position.x, me.position.y, me.position.z, me.velocity.x, me.velocity.y, me.velocity.z, me.mode,
            if me.crouched { " sneaking" } else { "" },
            if me.vitals.dark_vision_remaining > 0.0 { format!("  DARK VISION {:.0}s ({} revealed)", me.vitals.dark_vision_remaining, view.dark_vision_reveals.len()) } else { String::new() }
        );
    }
    // Contract clues are drawn on the target paper (gfx_hud); the debug readout keeps them too.
    if let Some(ti) = view.target_info.as_ref().filter(|_| debug.0) {
        use dishonored_multiplayer::contracts::{Proximity, PursuitAwareness};
        let arrows = ["E", "NE", "N", "NW", "W", "SW", "S", "SE"];
        let looks = match ti.appearance {
            Some(0) => "masked gentleman, dark coat, metal mask",
            Some(2) => "masked gentleman, shirtsleeves, metal mask",
            Some(3) => "masked lady, black mask",
            Some(1) => "guard uniform",
            Some(_) => "unknown",
            None => "-",
        };
        // Bearing relative to where you look (octant arrow), never coordinates.
        let rel = ti.bearing_octant.map(|o| {
            let me_yaw = local.yaw.rem_euclid(360.0);
            let ang = (o as f32 * 45.0 - me_yaw).rem_euclid(360.0);
            ["ahead", "ahead-left", "left", "behind-left", "behind", "behind-right", "right", "ahead-right"][(((ang + 22.5) / 45.0) as usize) % 8]
        });
        s += &format!(
            "CONTRACT  target looks like: {looks}   direction: {}   distance: {}   compass {}\n",
            rel.unwrap_or("reacquiring..."),
            match ti.proximity {
                Some(Proximity::Far) => "far",
                Some(Proximity::Medium) => "medium",
                Some(Proximity::Near) => "near",
                Some(Proximity::VeryNear) => "VERY NEAR",
                None => "-",
            },
            ti.bearing_octant.map(|o| arrows[o as usize]).unwrap_or("-")
        );
        s += &format!(
            "target is {}   exposure {}   {}{}\n",
            match ti.target_awareness {
                PursuitAwareness::Unaware => "unaware of you",
                PursuitAwareness::Suspicious => "suspicious",
                PursuitAwareness::Aware => "AWARE of you",
                PursuitAwareness::Pursuit => "FLEEING (chase!)",
            },
            "#".repeat((ti.exposure * 4.0).round() as usize),
            if ti.being_chased { "YOU ARE BEING CHASED  " } else if ti.pursuer_near { "you sense your pursuer nearby  " } else { "" },
            ti.escaping.map(|t| format!("ESCAPE! {t:.0}s")).unwrap_or_default()
        );
    }
    if debug.0 {
        s += "\nWASD move  mouse look  Space jump  C sneak  Shift sprint  X walk  LMB sword  RMB left hand  Ctrl block  F use  Q/E lean  1-5 hand  V 3rd person  F1 debug  F3 collision  Tab scores  Esc release mouse\n";
    }
    for f in &view.feed {
        s += &format!("{f}\n");
    }
    for mut t in &mut hud {
        crate::gfx_hud::set_text(&mut t, s.clone());
    }
    let m = &view.matchv;
    let show = keys.pressed(KeyCode::Tab) || m.phase == MatchPhase::Ended;
    // Mode and clock live on the objectives banner (gfx_hud); the board is Tab / match end.
    let mut b = if show { format!("{} {:?}  to {}\n", m.mode.to_uppercase(), m.phase, m.score_limit) } else { String::new() };
    if show {
        for e in &m.scores {
            b += &format!("{:<12} {:>3}  K {:>3}  D {:>3}\n", e.name, e.score, e.kills, e.deaths);
        }
    }
    if let Some(w) = m.winner {
        if m.phase == MatchPhase::Ended {
            let name = m.scores.iter().find(|s| s.id == w).map(|s| s.name.clone()).unwrap_or_default();
            b += &format!("WINNER: {name}\n");
        }
    }
    for mut t in &mut board {
        crate::gfx_hud::set_text(&mut t, b.clone());
    }
}

/// F1: full debug readout (position, velocity, net stats, controls).
#[derive(Resource, Default)]
pub struct DebugHud(pub bool);
