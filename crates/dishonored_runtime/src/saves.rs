//! Saved games in the game process (research/dishonored/SAVES.md): quicksave and quickload on
//! the original bindings (`GBA_QuickSave` = `Dis_Save 15` on F5, `GBA_QuickLoad` =
//! `Dis_Load 15` on F9), the autosaves the mission takes (each map load, `DisSeqAct_AutoSave`),
//! the pause menu's save / load lists and `--load FILE` (the launcher's Continue / Load).
//! Saving and loading work on an offline mission; the match state is the whole
//! `MissionMatch` (`dishonored_multiplayer::mission::save`).

use crate::session::{Session, SessionKind, SessionView};
use bevy::prelude::*;
use dishonored_multiplayer::game::Game;
use dishonored_multiplayer::mission::save;
use dishonored_save::{SaveHeader, slot};
use std::path::{Path, PathBuf};

#[derive(Resource, Default)]
pub struct Saves {
    /// Mission file the session was started with (saves record it; loads rebuild from it).
    pub mission_path: Option<String>,
    pub dir: PathBuf,
    pending: Vec<Cmd>,
    /// Last result, shown on the HUD feed.
    pub last: Option<String>,
}

enum Cmd {
    Save(i32),
    Load(PathBuf),
}

impl Saves {
    pub fn new(mission_path: Option<String>) -> Self {
        Saves { mission_path, dir: dishonored_save::saves_dir(), pending: Vec::new(), last: None }
    }
    /// `Dis_Save(slot)`.
    pub fn save(&mut self, slot: i32) {
        self.pending.push(Cmd::Save(slot));
    }
    /// `Dis_Load(slot)` (0: the newest save).
    pub fn load_slot(&mut self, s: i32) {
        let list = dishonored_save::list(&self.dir);
        if let Some(s) = dishonored_save::resolve_load_slot(&list, s) {
            self.pending.push(Cmd::Load(dishonored_save::slot_path(&self.dir, s)));
        }
    }
    pub fn load_file(&mut self, p: PathBuf) {
        self.pending.push(Cmd::Load(p));
    }
    pub fn list(&self) -> Vec<(PathBuf, SaveHeader)> {
        dishonored_save::list(&self.dir)
    }
    /// First free manual slot (16..55), else the oldest manual save's.
    pub fn new_manual_slot(&self) -> i32 {
        let list = self.list();
        (slot::MANUAL0..=slot::MAX).find(|s| !list.iter().any(|x| x.1.slot == *s)).unwrap_or_else(|| list.iter().rev().find(|x| x.1.slot >= slot::MANUAL0).map(|x| x.1.slot).unwrap_or(slot::MANUAL0))
    }
}

/// The original's label of a save (`m_QuicksavePrefix`, `m_AutosavePrefix`,
/// `m_MissionSavePrefix` of `DisTweaks_GFxMoviePlayerMenuBase`), with the map and play time.
pub fn label(h: &SaveHeader, t: &crate::menu_ui::MenuTexts) -> String {
    // `Twk_InGameUI.int`: "QUICKSAVE - ", "AUTOSAVE - ", "MISSION - " (the dash is part of it).
    let tw = |k: &str| {
        let s = t.t(crate::menu_ui::SEC_TWK_MENU, k);
        if s.is_empty() { s } else { format!("{} ", s.trim_end()) }
    };
    let kind = match h.kind() {
        dishonored_save::SlotKind::Quicksave => tw("m_QuicksavePrefix"),
        dishonored_save::SlotKind::Autosave => tw("m_AutosavePrefix"),
        dishonored_save::SlotKind::Mission(_) => tw("m_MissionSavePrefix"),
        _ => String::new(),
    };
    format!("{kind}{}   {}", map_name(&h.map), date(h.saved_at))
}

/// `m_DateFormat` ("month/day/year - hour:minute:second"), in UTC.
fn date(unix: u64) -> String {
    let days = (unix / 86400) as i64;
    let s = unix % 86400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{m:02}/{d:02}/{y} - {:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// `m_MapConfig.m_FriendlyName` (DefaultEngine.ini) of the maps the port plays.
fn map_name(map: &str) -> String {
    match map.to_ascii_lowercase().as_str() {
        "l_boyle_ext_p" | "l_boyle_int_p" => "The Boyle Estate".into(),
        "l_tower_p" => "Dunwall Tower".into(),
        "l_pub_day_p" | "l_pub_dusk_p" | "l_pub_night_p" | "l_pub_morning_p" => "The Hound Pits Pub".into(),
        _ => map.to_string(),
    }
}

/// Quicksave / quickload keys (only while playing, not in a menu).
pub fn hotkeys(keys: Res<ButtonInput<KeyCode>>, mouse: Res<ButtonInput<MouseButton>>, bindings: Res<crate::settings::Bindings>, pause: Res<crate::pause_menu::PauseMenu>, mut saves: ResMut<Saves>) {
    if pause.open {
        return;
    }
    if bindings.just_pressed("GBA_QuickSave", &keys, &mouse, 0.0) {
        saves.save(slot::QUICKSAVE);
    }
    if bindings.just_pressed("GBA_QuickLoad", &keys, &mouse, 0.0) {
        saves.load_slot(slot::QUICKSAVE);
    }
}

/// Run the queued commands (between fixed ticks).
pub fn process(mut saves: ResMut<Saves>, mut session: ResMut<Session>, mut view: ResMut<SessionView>) {
    let cmds = std::mem::take(&mut saves.pending);
    for c in cmds {
        let r = match c {
            Cmd::Save(s) => save_now(&saves, &mut session, s),
            Cmd::Load(p) => load_into(&mut session, &p).map(|h| format!("Loaded {}", h.title)),
        };
        let msg = r.unwrap_or_else(|e| format!("{e:#}"));
        info!("save: {msg}");
        view.feed.push(msg.clone());
        saves.last = Some(msg);
    }
}

fn mission(session: &mut Session) -> Option<&mut dishonored_multiplayer::mission::MissionMatch> {
    match &mut session.kind {
        SessionKind::Offline { game: Game::Mission(m), .. } => Some(m),
        _ => None,
    }
}

fn save_now(saves: &Saves, session: &mut Session, s: i32) -> anyhow::Result<String> {
    let path = saves.mission_path.clone().unwrap_or_default();
    let m = mission(session).ok_or_else(|| anyhow::anyhow!("saving needs an offline mission"))?;
    if !save::can_save(m) {
        anyhow::bail!("You cannot save right now");
    }
    let bytes = save::encode(m, s, &path)?;
    dishonored_save::write_file(&dishonored_save::slot_path(&saves.dir, s), &bytes)?;
    Ok(if s == slot::QUICKSAVE { "Quicksave".into() } else { "Game saved".into() })
}

/// Replace the session's mission with a save (same mission setup).
pub fn load_into(session: &mut Session, file: &Path) -> anyhow::Result<SaveHeader> {
    let bytes = std::fs::read(file)?;
    let maps = session.maps.clone();
    let m = mission(session).ok_or_else(|| anyhow::anyhow!("loading needs an offline mission"))?;
    let (h, restored) = save::restore(&bytes, m.setup.clone())?;
    *m = restored;
    let map = m.current_map().to_string();
    if let Some(c) = maps.get(&map) {
        maps.set.set_active(&map);
        session.content = c.clone();
    }
    Ok(h)
}

/// The saves the mission took this tick (autosaves, mission-start saves) into their slots.
pub fn write_autosaves(mut saves: ResMut<Saves>, mut session: ResMut<Session>) {
    let path = saves.mission_path.clone().unwrap_or_default();
    let dir = saves.dir.clone();
    let Some(m) = mission(&mut session) else { return };
    for r in save::take_requests(m) {
        let s = match r {
            save::SaveRequest::Autosave => dishonored_save::next_autosave_slot(&dishonored_save::list(&dir)),
            save::SaveRequest::MissionStart(s) => s,
        };
        match save::encode(m, s, &path).and_then(|b| dishonored_save::write_file(&dishonored_save::slot_path(&dir, s), &b)) {
            Ok(()) => saves.last = Some(format!("autosave {}", dishonored_save::slot_file(s))),
            Err(e) => warn!("autosave failed: {e:#}"),
        }
    }
}

/// `--load FILE`: the save's header names the mission the session must be started with.
pub fn header_of(file: &Path) -> anyhow::Result<SaveHeader> {
    dishonored_save::read_header(file)
}
