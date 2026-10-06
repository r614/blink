//! Saved games, modelled on Dishonored's save system (research/dishonored/SAVES.md).
//!
//! The original keeps its saves in numbered slots (`DishonoredEngine.ESaveGameSlot`): 0 is "the
//! last save" (resolved to the newest one when loading), 4..12 the nine mission-start saves,
//! 13 and 14 two autosaves used in turn, 15 the quicksave (`GBA_QuickSave` = `Dis_Save 15` on
//! F5, `GBA_QuickLoad` = `Dis_Load 15` on F9) and 16 on the manual saves; `Dis_Load` clamps the
//! slot to [0, 55] and the save path to [2, 55]. Files are named after the slot
//! ([`slot_file`]).
//!
//! This crate is the port's generic container: a small header (what the save lists show and
//! what the loader checks) and the serialized game state. The state is any `serde` type; it is
//! written as RON (self-describing, so fields added later with `#[serde(default)]` keep older
//! saves loading) and compressed with deflate. [`state_hash`] hashes the exact binary encoding
//! of a state, so a reload can be checked bit for bit.

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// `ESaveGameSlot` values (DishonoredEngine).
pub mod slot {
    /// `SGS_LastSave`: loading it loads the newest save.
    pub const LAST_SAVE: i32 = 0;
    /// `SGS_DisMission0` .. `SGS_DisMission8`: the save made at the start of each mission
    /// (`DisSeqAct_AutoSave` with `m_bMissionStart`, slot `4 + m_MissionNumber`).
    pub const MISSION0: i32 = 4;
    pub const MISSIONS: i32 = 9;
    /// `SGS_DisAutoSave0` / `1`: autosaves alternate between the two.
    pub const AUTOSAVE0: i32 = 13;
    pub const AUTOSAVE1: i32 = 14;
    /// `SGS_DisQuickSave` (the F5 / F9 bindings).
    pub const QUICKSAVE: i32 = 15;
    /// `SGS_Dishonored0`: first manual slot.
    pub const MANUAL0: i32 = 16;
    /// Highest slot `Dis_Save` / `Dis_Load` accept (they clamp to 0x37).
    pub const MAX: i32 = 55;
}

/// What a slot is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotKind {
    LastSave,
    Mission(u8),
    Autosave,
    Quicksave,
    Manual(u8),
    Other,
}

pub fn slot_kind(s: i32) -> SlotKind {
    match s {
        slot::LAST_SAVE => SlotKind::LastSave,
        x if (slot::MISSION0..slot::MISSION0 + slot::MISSIONS).contains(&x) => SlotKind::Mission((x - slot::MISSION0) as u8),
        slot::AUTOSAVE0 | slot::AUTOSAVE1 => SlotKind::Autosave,
        slot::QUICKSAVE => SlotKind::Quicksave,
        x if (slot::MANUAL0..=slot::MAX).contains(&x) => SlotKind::Manual((x - slot::MANUAL0) as u8),
        _ => SlotKind::Other,
    }
}

/// Container format version (bump when the container itself changes, not the state).
pub const FORMAT: u32 = 1;
const MAGIC: &[u8; 8] = b"DRSAVE\0\x01";

/// What a save list shows and what the loader checks before restoring.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SaveHeader {
    #[serde(default)]
    pub format: u32,
    /// `ESaveGameSlot`.
    pub slot: i32,
    /// Mission file (as the game was started with it) and its title.
    pub mission: String,
    #[serde(default)]
    pub title: String,
    /// Active persistent map when saved.
    pub map: String,
    /// Mission time (seconds) and simulation tick.
    #[serde(default)]
    pub play_time: f64,
    #[serde(default)]
    pub tick: u64,
    /// Wall-clock time of the save (Unix seconds), for the list's date.
    #[serde(default)]
    pub saved_at: u64,
    /// Hash of the mission setup the state belongs to (the loader rebuilds the setup).
    #[serde(default)]
    pub setup_fingerprint: u64,
    /// [`state_hash`] of the saved state.
    #[serde(default)]
    pub state_hash: u64,
}

impl SaveHeader {
    pub fn kind(&self) -> SlotKind {
        slot_kind(self.slot)
    }
}

/// Order-stable 64-bit FNV-1a over the exact binary (bincode) encoding of a value: equal
/// hashes mean the same bits in every field that serializes.
pub fn state_hash<T: Serialize + ?Sized>(state: &T) -> u64 {
    let bytes = bincode::serde::encode_to_vec(state, bincode::config::standard()).unwrap_or_default();
    fnv(&bytes)
}

/// Binary encoding of a state (for exact comparisons).
pub fn state_bytes<T: Serialize + ?Sized>(state: &T) -> Vec<u8> {
    bincode::serde::encode_to_vec(state, bincode::config::standard()).unwrap_or_default()
}

pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Serialize a save (header + state).
pub fn encode<T: Serialize + ?Sized>(header: &SaveHeader, state: &T) -> Result<Vec<u8>> {
    let mut h = header.clone();
    h.format = FORMAT;
    let head = ron::to_string(&h).context("save header")?;
    let body = ron::to_string(state).context("save state")?;
    let mut out = Vec::with_capacity(body.len() / 4 + head.len() + 16);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(head.len() as u32).to_le_bytes());
    out.extend_from_slice(head.as_bytes());
    let mut z = flate2::write::DeflateEncoder::new(out, flate2::Compression::fast());
    z.write_all(body.as_bytes())?;
    Ok(z.finish()?)
}

fn split(bytes: &[u8]) -> Result<(SaveHeader, &[u8])> {
    if bytes.len() < 12 || &bytes[..8] != MAGIC {
        bail!("not a saved game");
    }
    let n = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let head = bytes.get(12..12 + n).context("truncated save header")?;
    let h: SaveHeader = ron::from_str(std::str::from_utf8(head)?).context("save header")?;
    if h.format > FORMAT {
        bail!("save format {} is newer than this build ({FORMAT})", h.format);
    }
    Ok((h, &bytes[12 + n..]))
}

/// The header alone (save lists).
pub fn decode_header(bytes: &[u8]) -> Result<SaveHeader> {
    Ok(split(bytes)?.0)
}

/// Header and state.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<(SaveHeader, T)> {
    let (h, body) = split(bytes)?;
    let mut text = String::new();
    flate2::read::DeflateDecoder::new(body).read_to_string(&mut text).context("save data corrupted")?;
    let state = ron::from_str(&text).map_err(|e| anyhow::anyhow!("save state: {e}"))?;
    Ok((h, state))
}

/// Read only the header of a save file.
pub fn read_header(path: &Path) -> Result<SaveHeader> {
    let mut f = std::fs::File::open(path)?;
    let mut first = [0u8; 12];
    f.read_exact(&mut first)?;
    if &first[..8] != MAGIC {
        bail!("{} is not a saved game", path.display());
    }
    let n = u32::from_le_bytes(first[8..12].try_into().unwrap()) as usize;
    let mut head = vec![0u8; n];
    f.read_exact(&mut head)?;
    let mut all = first.to_vec();
    all.extend(head);
    decode_header(&all)
}

/// Write a save file (through a temporary file, so a failed write keeps the old save).
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Saves folder: `--saves DIR`, else `DISHONORED_RS_SAVES`, else `SaveData` next to the
/// settings file (`--settings FILE` / `DISHONORED_RS_SETTINGS` / `%APPDATA%/dishonored-rs`).
pub fn saves_dir() -> PathBuf {
    let a: Vec<String> = std::env::args().collect();
    if let Some(p) = a.iter().position(|x| x == "--saves").and_then(|i| a.get(i + 1)) {
        return PathBuf::from(p);
    }
    if let Some(p) = std::env::var_os("DISHONORED_RS_SAVES") {
        return PathBuf::from(p);
    }
    let settings = if let Some(p) = a.iter().position(|x| x == "--settings").and_then(|i| a.get(i + 1)) {
        Some(PathBuf::from(p))
    } else {
        std::env::var_os("DISHONORED_RS_SETTINGS").map(PathBuf::from)
    };
    if let Some(dir) = settings.as_deref().and_then(Path::parent) {
        return dir.join("SaveData");
    }
    let base = std::env::var_os("APPDATA").or_else(|| std::env::var_os("XDG_CONFIG_HOME")).map(PathBuf::from).unwrap_or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")).unwrap_or_else(|| PathBuf::from(".")));
    base.join("dishonored-rs").join("SaveData")
}

/// File name of a slot, as the original names them (the cloud file list at 0x00b75ed0):
/// slots 4..15 by their enum names (`DisMission0.SAV` .. `DisMission8.SAV`,
/// `DisAutoSave0.SAV`, `DisAutoSave1.SAV`, `DisQuickSave.SAV`), manual slots 16..55 as
/// `DISHONORED<slot - 16>.SAV`.
pub fn slot_file(slot: i32) -> String {
    match slot {
        x if (slot::MISSION0..slot::MISSION0 + slot::MISSIONS).contains(&x) => format!("DisMission{}.SAV", x - slot::MISSION0),
        slot::AUTOSAVE0 => "DisAutoSave0.SAV".into(),
        slot::AUTOSAVE1 => "DisAutoSave1.SAV".into(),
        slot::QUICKSAVE => "DisQuickSave.SAV".into(),
        x => format!("DISHONORED{}.SAV", x - slot::MANUAL0),
    }
}

pub fn slot_path(dir: &Path, slot: i32) -> PathBuf {
    dir.join(slot_file(slot))
}

/// Every save in a folder, newest first (the order of the original's save list, whose first
/// entry is what "last save" / `Dis_Load 0` loads).
pub fn list(dir: &Path) -> Vec<(PathBuf, SaveHeader)> {
    let mut v: Vec<(PathBuf, SaveHeader)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("sav")))
        .filter_map(|p| read_header(&p).ok().map(|h| (p, h)))
        .collect();
    v.sort_by(|a, b| b.1.saved_at.cmp(&a.1.saved_at).then(b.1.slot.cmp(&a.1.slot)));
    v
}

/// `Dis_Load` slot resolution: clamp to [0, 55]; slot 0 is the newest save.
pub fn resolve_load_slot(saves: &[(PathBuf, SaveHeader)], slot: i32) -> Option<i32> {
    let s = slot.clamp(0, slot::MAX);
    if s == slot::LAST_SAVE {
        return saves.first().map(|x| x.1.slot);
    }
    saves.iter().any(|x| x.1.slot == s).then_some(s)
}

/// The autosave slot to write next (`FUN_00a0b050`): slot 13 unless the save list's newest
/// autosave already is 13, then 14 (and back), so the previous autosave survives.
pub fn next_autosave_slot(saves: &[(PathBuf, SaveHeader)]) -> i32 {
    match saves.iter().map(|x| x.1.slot).find(|s| *s == slot::AUTOSAVE0 || *s == slot::AUTOSAVE1) {
        Some(slot::AUTOSAVE0) => slot::AUTOSAVE1,
        _ => slot::AUTOSAVE0,
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct S {
        f: Vec<f32>,
        d: f64,
        m: BTreeMap<(u32, String), Option<i64>>,
        big: u64,
    }

    #[test]
    fn floats_and_maps_round_trip_bit_exactly() {
        let f = vec![0.1, -0.0, f32::MIN_POSITIVE, 1.0e-45, f32::MAX, -3.4028235e38, 1.0 / 3.0, f32::INFINITY, f32::NEG_INFINITY, 123456.79];
        let mut m = BTreeMap::new();
        m.insert((3, "a\"b".to_string()), Some(-5));
        m.insert((1, String::new()), None);
        let s = S { f, d: std::f64::consts::PI / 7.0, m, big: u64::MAX };
        let h = SaveHeader { slot: slot::QUICKSAVE, mission: "missions/x.ron".into(), map: "L_Boyle_Int_P".into(), tick: 99, ..Default::default() };
        let bytes = encode(&h, &s).unwrap();
        let (h2, s2): (SaveHeader, S) = decode(&bytes).unwrap();
        assert_eq!(h2.slot, h.slot);
        assert_eq!(h2.format, FORMAT);
        assert_eq!(state_bytes(&s), state_bytes(&s2));
        for (a, b) in s.f.iter().zip(&s2.f) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
        assert_eq!(decode_header(&bytes).unwrap().map, "L_Boyle_Int_P");
    }

    #[test]
    fn slots_follow_the_original_enum() {
        assert_eq!(slot_kind(15), SlotKind::Quicksave);
        assert_eq!(slot_kind(13), SlotKind::Autosave);
        assert_eq!(slot_kind(4), SlotKind::Mission(0));
        assert_eq!(slot_kind(12), SlotKind::Mission(8));
        assert_eq!(slot_kind(16), SlotKind::Manual(0));
        assert_eq!(slot_file(15), "DisQuickSave.SAV");
        assert_eq!(slot_file(14), "DisAutoSave1.SAV");
        assert_eq!(slot_file(4), "DisMission0.SAV");
        assert_eq!(slot_file(18), "DISHONORED2.SAV");
        let mk = |slot, at| (PathBuf::new(), SaveHeader { slot, saved_at: at, ..Default::default() });
        assert_eq!(next_autosave_slot(&[]), 13);
        assert_eq!(next_autosave_slot(&[mk(15, 9), mk(13, 8), mk(14, 1)]), 14);
        assert_eq!(next_autosave_slot(&[mk(14, 8), mk(13, 1)]), 13);
        assert_eq!(resolve_load_slot(&[mk(15, 9), mk(13, 8)], 0), Some(15));
        assert_eq!(resolve_load_slot(&[mk(15, 9)], 99), None);
    }
}
