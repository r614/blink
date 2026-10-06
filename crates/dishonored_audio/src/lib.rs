//! Dishonored audio: Wwise file packages, soundbanks and media decoding.
//!
//! Everything is read from the local install at runtime; nothing decoded is cached
//! on disk by this crate.

pub mod adpcm;
pub mod anim_notify;
pub mod bank;
pub mod bus;
pub mod contact;
pub mod cues;
pub mod emitters;
pub mod engine;
pub mod index;
pub mod init;
pub mod hirc;
pub mod library;
pub mod mixer;
pub mod music;
pub mod music_manager;
pub mod pck;
pub mod vo;
pub mod volumes;
pub mod vorbis;
pub mod wem;

pub use wem::Pcm;

/// Wwise short-id hash: 32-bit FNV-1 over the lowercased ASCII name.
pub fn fnv1_32(name: &str) -> u32 {
    let mut h: u32 = 2166136261;
    for b in name.bytes() {
        h = h.wrapping_mul(16777619);
        h ^= b.to_ascii_lowercase() as u32;
    }
    h
}
