//! Saving and loading a mission session (research/dishonored/SAVES.md).
//!
//! A save is the whole [`MissionMatch`] minus its setup: the world (players, NPCs, pickups,
//! projectiles and their RNG), the brains and the level data of the crowd, the level states
//! of the maps left behind (`DisSeqAct_SaveLevelState`), the mission interpreter, the
//! conversation runner, the original Kismet's runtime and its host state. Everything
//! serializes through `serde` on the existing structs; the static data (mission setup,
//! Kismet and dialog databases) is rebuilt from the mission file by the loader and
//! re-attached ([`restore`]). A run continued after a reload is bit-identical to the
//! uninterrupted run (`harness save`).
//!
//! Autosaves follow the original engine (`DishonoredEngine`, 0x00a0c530 / 0x00a0d7f0 /
//! 0x00a09640): `DisSeqAct_AutoSave` queues a pending autosave (or, with `m_bMissionStart`,
//! the mission-start save in slot `4 + m_MissionNumber`), the engine takes it when saving
//! is allowed, and the action outputs "Accepted" once the save is written or "Denied" when
//! the pending autosave is dropped (the player in combat and no requester has
//! `m_bEvenInCombat`). Every map load that is not a reload also queues an autosave from code
//! (0x00a152f0), which skips the combat check.

use super::{MissionMatch, MissionSetup};
use anyhow::{Result, bail};
use dishonored_save::SaveHeader;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `EDisAutoSaveActionStatus`.
pub mod status {
    pub const WAITING: u8 = 0;
    pub const SAVING: u8 = 1;
    pub const ACCEPTED: u8 = 2;
    pub const DENIED: u8 = 3;
}

/// A save the mission took this step; the host writes it after the step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveRequest {
    /// One of the two autosave slots (the host picks 13 / 14 from its save list).
    Autosave,
    /// A mission-start save (`ESaveGameSlot` 4..12).
    MissionStart(i32),
}

/// The engine's autosave bookkeeping (`m_PendingAutosaveSlot`, `m_PendingAutosaveActions`,
/// `m_fPendingAutosaveTimer`, `m_PendingMissionStartSaveSlot` / `Action`, the actions to
/// notify), saved with the match.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AutosaveState {
    /// Pending autosave: requesting Kismet ops and their `m_bEvenInCombat`.
    #[serde(default)]
    pub pending: Vec<(u32, bool)>,
    /// An autosave is pending (also with no requester: queued from code).
    #[serde(default)]
    pub pending_active: bool,
    /// Queued from code (map load): no combat check (engine flag 0x10).
    #[serde(default)]
    pub from_code: bool,
    /// `m_fPendingAutosaveTimer`.
    #[serde(default)]
    pub timer: f32,
    /// Pending mission-start save: slot and requesting op.
    #[serde(default)]
    pub mission_start: Option<(i32, u32)>,
    /// `m_AutoSaveStatus` per requesting op.
    #[serde(default)]
    pub status: BTreeMap<u32, u8>,
    /// Ops waiting for the save they are part of to be written (`m_SaveActionsToNotify`).
    #[serde(default)]
    pub notify: Vec<u32>,
    /// Saves due for the host (drained with [`take_requests`]).
    #[serde(default)]
    pub due: Vec<SaveRequest>,
    /// Map of the last step: a different map is a map load (autosave from code).
    #[serde(default)]
    pub last_map: String,
    /// Autosaves written / denied so far (reports).
    #[serde(default)]
    pub written: u32,
    #[serde(default)]
    pub denied: u32,
}

/// `m_fAutosaveWaitTime`: a config value absent from the shipped ini files, so 0 (U-341).
pub const AUTOSAVE_WAIT: f32 = 0.0;

/// `DisSeqAct_AutoSave::Activated` (0x00b8f240).
pub fn activated(m: &mut MissionMatch, op: u32, mission_start: bool, mission_number: i32, even_in_combat: bool, skip_delay: bool) {
    let a = &mut m.autosave;
    if mission_start && (0..dishonored_save::slot::MISSIONS).contains(&mission_number) {
        // FUN_00a09700: replaces a pending mission-start save (its action is denied).
        if let Some((_, old)) = a.mission_start.take() {
            a.status.insert(old, status::DENIED);
        }
        a.status.insert(op, status::WAITING);
        a.mission_start = Some((dishonored_save::slot::MISSION0 + mission_number, op));
        return;
    }
    // FUN_00a0d7f0: start a pending autosave (timer = m_fAutosaveWaitTime) or join it.
    if !a.pending_active {
        a.pending_active = true;
        a.timer = AUTOSAVE_WAIT;
        a.from_code = false;
    }
    a.status.insert(op, status::WAITING);
    a.pending.push((op, even_in_combat));
    if skip_delay {
        a.timer = 0.0;
    }
}

/// `DisSeqAct_AutoSave::Update` (0x00b8f2e0): `Some(true)` fires "Accepted", `Some(false)`
/// "Denied"; `None` keeps waiting.
pub fn update(m: &mut MissionMatch, op: u32) -> Option<bool> {
    match m.autosave.status.get(&op).copied() {
        Some(status::ACCEPTED) => {
            m.autosave.status.remove(&op);
            Some(true)
        }
        Some(status::DENIED) => {
            m.autosave.status.remove(&op);
            Some(false)
        }
        Some(_) => None,
        // Not known (activated before this build tracked it): the original's default status.
        None => Some(true),
    }
}

/// `FUN_009e41c0`: saving is allowed (no `EDisDisableSaveType` pushed). The port knows these
/// of the original's reasons: the player dead, in a scripted or dialog choice, choking,
/// blinking, or possessing (U-342).
pub fn saving_allowed(m: &MissionMatch) -> bool {
    save_blocked(m).is_none()
}

/// Why saving is disallowed now (`EDisDisableSaveType` names), if it is.
pub fn save_blocked(m: &MissionMatch) -> Option<&'static str> {
    use dishonored_sim::state::{ChokePhase, MoveMode, PossessStage};
    if m.humans.is_empty() {
        return Some("no player");
    }
    if m.view.phase == crate::MatchPhase::Ended {
        return Some("DDST_GameOverPending");
    }
    for h in &m.humans {
        let Some(p) = m.world.player(*h) else { return Some("no player") };
        if !p.alive {
            return Some("DDST_StatePlayerMasterDead");
        }
        if p.choke.phase != ChokePhase::None {
            return Some("DDST_StatePlayerMasterChoke");
        }
        if p.mode == MoveMode::Blinking {
            return Some("DDST_Blink");
        }
        if matches!(p.possess.stage, PossessStage::PrePossess | PossessStage::Active) {
            return Some("DDST_StatePlayerMasterPrePossess");
        }
    }
    if !m.choosing.is_empty() || m.state.view.choice.is_some() || m.humans.iter().any(|h| m.kismet.as_ref().is_some_and(|k| k.choosing(*h))) {
        return Some("DDST_StatePlayerMasterChoice_Base");
    }
    // `DDST_CinematicMode` / `DDST_Kismet` / `DDST_Soiree` are not modelled: the port's Kismet
    // host does not track cinematic mode, and "a matinee is playing" is no stand-in (the
    // mansion runs looping ambient matinees all the time; U-342).
    None
}

/// `DIAT_PlayerInCombat`: a guard is fighting a player (U-340).
fn player_in_combat(m: &MissionMatch) -> bool {
    !m.engaged.is_empty()
}

/// Once per step, after the mission and the Kismet ran (engine tick order: timers, the
/// pending autosave 0x00a0c530, the pending mission-start save 0x00a09640).
pub(super) fn tick(m: &mut MissionMatch, dt: f32) {
    // The saves taken last step were written by the host: their actions are accepted
    // (vtable slot 71, 0x00b85f30: Saving -> Accepted).
    for op in std::mem::take(&mut m.autosave.notify) {
        if let Some(s) = m.autosave.status.get_mut(&op) {
            if *s == status::SAVING {
                *s = status::ACCEPTED;
            }
        }
    }
    // A map load (not a reload) queues an autosave from code (0x00a152f0).
    if m.autosave.last_map != m.current_map {
        m.autosave.last_map = m.current_map.clone();
        if !m.autosave.pending_active {
            m.autosave.pending_active = true;
            m.autosave.timer = AUTOSAVE_WAIT;
        }
        m.autosave.from_code = true;
    }
    m.autosave.timer -= dt;
    let allowed = saving_allowed(m);
    if m.autosave.pending_active && m.autosave.timer <= 0.0 {
        let a = &m.autosave;
        let even = a.pending.iter().any(|p| p.1);
        let game_over = m.state.outcome().is_some_and(|o| !o.success);
        let drop = !a.from_code && ((!even && player_in_combat(m)) || game_over);
        if drop {
            // FUN_00a070b0: every pending action is denied.
            let a = &mut m.autosave;
            for (op, _) in std::mem::take(&mut a.pending) {
                a.status.insert(op, status::DENIED);
            }
            a.pending_active = false;
            a.denied += 1;
        } else if allowed {
            let a = &mut m.autosave;
            for (op, _) in std::mem::take(&mut a.pending) {
                a.status.insert(op, status::SAVING);
                a.notify.push(op);
            }
            a.pending_active = false;
            a.from_code = false;
            a.due.push(SaveRequest::Autosave);
            a.written += 1;
        }
    }
    if allowed {
        if let Some((slot, op)) = m.autosave.mission_start.take() {
            m.autosave.status.insert(op, status::SAVING);
            m.autosave.notify.push(op);
            m.autosave.due.push(SaveRequest::MissionStart(slot));
        }
    }
}

/// The saves the mission took this step (the host writes each into its slot).
pub fn take_requests(m: &mut MissionMatch) -> Vec<SaveRequest> {
    std::mem::take(&mut m.autosave.due)
}

/// `Dis_Save` (0x00a0b680): a manual or quick save is possible now.
pub fn can_save(m: &MissionMatch) -> bool {
    saving_allowed(m)
}

/// Hash of the mission setup a save belongs to.
pub fn setup_fingerprint(setup: &MissionSetup) -> u64 {
    dishonored_save::state_hash(setup)
}

/// Exact hash of the whole match state (everything a save holds).
pub fn state_hash(m: &MissionMatch) -> u64 {
    dishonored_save::state_hash(m)
}

/// The header of a save of `m` into `slot`.
pub fn header(m: &MissionMatch, slot: i32, mission_path: &str) -> SaveHeader {
    SaveHeader {
        format: dishonored_save::FORMAT,
        slot,
        mission: mission_path.to_string(),
        title: m.setup.def.title.clone(),
        map: m.current_map.clone(),
        play_time: m.state.view.stats.time as f64,
        tick: m.world.tick,
        saved_at: dishonored_save::now_unix(),
        setup_fingerprint: setup_fingerprint(&m.setup),
        state_hash: state_hash(m),
    }
}

/// Serialize a save of the match.
pub fn encode(m: &MissionMatch, slot: i32, mission_path: &str) -> Result<Vec<u8>> {
    dishonored_save::encode(&header(m, slot, mission_path), m)
}

/// Rebuild a match from a save and the mission's setup (built from the mission file the way
/// a new game builds it). Fails when the setup is not the one the save was made with.
pub fn restore(bytes: &[u8], setup: MissionSetup) -> Result<(SaveHeader, MissionMatch)> {
    let (h, mut m): (SaveHeader, MissionMatch) = dishonored_save::decode(bytes)?;
    let fp = setup_fingerprint(&setup);
    if h.setup_fingerprint != 0 && h.setup_fingerprint != fp {
        bail!("the save belongs to a different version of mission '{}' (setup {:016x}, this build {:016x})", h.title, h.setup_fingerprint, fp);
    }
    reattach(&mut m, setup);
    Ok((h, m))
}

/// Give a deserialized match its static data back: the setup, the dialog database of the
/// conversation runner and the Kismet database.
pub fn reattach(m: &mut MissionMatch, setup: MissionSetup) {
    if let (Some(r), Some(db)) = (m.state.conv.as_mut(), setup.conversations.as_ref()) {
        r.db = Arc::new(db.clone());
    }
    if let (Some(k), Some(ks)) = (m.kismet.as_mut(), setup.kismet.as_ref()) {
        k.k.db = Arc::new(ks.db.clone());
        k.setup = Arc::new(ks.clone());
    }
    m.setup = setup;
}
