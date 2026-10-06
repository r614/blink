//! Bus hierarchy and environments of Init.bnk (decoded in `hirc` / `init`), and the names
//! of their game parameters, states and environment sharesets.
//!
//! Wwise ids are FNV-1 hashes of names (`fnv1_32`). The game parameter names come from the
//! executable's audio system (`UDishonoredAudioSystem`: `Global_Volume`, `Music_Volume`,
//! `SFX_Volume`, `Voices_Volume`, `Ambient_Volume`, `Menu_Volume`, `TimeDilation`); the
//! environment names from the levels' `DishonoredAudioVolume.m_Environment`.
//! See research/dishonored/AUDIO_FORMAT.md "Buses and environments".

use crate::fnv1_32;
use crate::hirc::Fx;
use std::collections::HashMap;

/// Game parameters the executable sets on the buses.
pub const GLOBAL_VOLUME: &str = "Global_Volume";
pub const MUSIC_VOLUME: &str = "Music_Volume";
pub const SFX_VOLUME: &str = "SFX_Volume";
pub const VOICES_VOLUME: &str = "Voices_Volume";
pub const AMBIENT_VOLUME: &str = "Ambient_Volume";
pub const MENU_VOLUME: &str = "Menu_Volume";
pub const TIME_DILATION: &str = "TimeDilation";

/// Names known to hash to Init.bnk ids (for dumps and reports).
pub fn known_names() -> HashMap<u32, String> {
    let names = [
        GLOBAL_VOLUME,
        MUSIC_VOLUME,
        SFX_VOLUME,
        VOICES_VOLUME,
        AMBIENT_VOLUME,
        MENU_VOLUME,
        TIME_DILATION,
        "Scale_RTPC",
        "Overseer_Gapper_RTPC",
        "PC_streets_01",
        "PC_Streets_02",
        "Room_Small_Wood",
        "sewer",
        "Hall_Stone_Wood",
        "Plate_Small",
        "Verb_Ext_Flooded",
    ];
    names.iter().map(|n| (fnv1_32(n), n.to_string())).collect()
}

/// Parameters of a Wwise RoomVerb shareset (plugin 0x00760003, 186-byte block in v65).
/// Offsets measured on Init.bnk: the first 21 floats follow the plugin's documented parameter
/// order (decay, HF damping, diffusion, stereo width, three tone filters, channel levels, dry,
/// early-reflection and reverb levels); then u8 early reflections on, u32 ER pattern, five
/// floats (reverb delay ms, ER front/back delay ms, room size, density, room shape: names
/// LIKELY, from their ranges) and u32 quality (reverb units, 12 or 16).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RoomVerb {
    pub decay_s: f32,
    pub hf_damping: f32,
    pub diffusion: f32,
    pub stereo_width: f32,
    /// (gain dB, frequency Hz, Q) of the three tone filters (low shelf, peak, high shelf).
    pub filters: [(f32, f32, f32); 3],
    pub lfe_db: f32,
    pub dry_db: f32,
    pub er_db: f32,
    pub reverb_db: f32,
    pub er_enabled: bool,
    pub er_pattern: u32,
    pub reverb_delay_ms: f32,
    pub er_front_back_ms: f32,
    pub room_size: f32,
    pub density: f32,
    pub room_shape: f32,
    /// Number of reverb units (quality).
    pub quality: u32,
}

impl RoomVerb {
    pub fn from_fx(f: &Fx) -> Option<Self> {
        if f.plugin != Fx::ROOMVERB || f.params.len() < 113 {
            return None;
        }
        let g = |o: usize| f.f32_at(o).unwrap_or(0.0);
        Some(RoomVerb {
            decay_s: g(0),
            hf_damping: g(4),
            diffusion: g(8),
            stereo_width: g(12),
            filters: [(g(16), g(20), g(24)), (g(28), g(32), g(36)), (g(40), g(44), g(48))],
            lfe_db: g(64),
            dry_db: g(68),
            er_db: g(72),
            reverb_db: g(76),
            er_enabled: f.params[80] != 0,
            er_pattern: f.u32_at(81).unwrap_or(0),
            reverb_delay_ms: g(85),
            er_front_back_ms: g(89),
            room_size: g(93),
            density: g(97),
            room_shape: g(101),
            quality: f.u32_at(105).unwrap_or(0),
        })
    }
}

/// Parameters of a Wwise Matrix Reverb shareset (plugin 0x00730003, 29-byte block):
/// f32 reverb time, f32 HF ratio, u32 delay lines, f32 dry level (dB), f32 wet level (dB),
/// f32 modulation, ... (LIKELY).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MatrixReverb {
    pub decay_s: f32,
    pub hf_ratio: f32,
    pub delay_lines: u32,
    pub dry_db: f32,
    pub wet_db: f32,
}

impl MatrixReverb {
    pub fn from_fx(f: &Fx) -> Option<Self> {
        if f.plugin != Fx::MATRIX_REVERB || f.params.len() < 20 {
            return None;
        }
        Some(MatrixReverb {
            decay_s: f.f32_at(0)?,
            hf_ratio: f.f32_at(4)?,
            delay_lines: f.u32_at(8)?,
            dry_db: f.f32_at(12)?,
            wet_db: f.f32_at(16)?,
        })
    }
}
