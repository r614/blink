//! Dialog VO. Each line (`DisConv_Blurb`) names a Wwise event `Play_<32 hex>` and the voice's
//! soundbank `Bk_<GUID>` (from `DisDialogVoiceData[_OneShot].m_VoiceData[].m_Blurbs[]` and
//! `m_pAkBank`). The bank is a per-voice AKPK file `CookedPCConsole/Bk_<GUID>.<LANG>`
//! (`INT` = english(us)); its HIRC maps the event to a Sound whose media is embedded or
//! streamed from the same file. Event ids are FNV-1 of the lowercased name as everywhere else.

use crate::library::{Leaf, SoundLibrary};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy)]
pub struct VoClip {
    pub leaf: Leaf,
    /// Seconds, from the media header.
    pub length: f32,
}

/// Resolves dialog lines to playable media, loading voice banks on demand and remembering
/// banks that are absent (one Boyle street bank is missing from the install).
pub struct VoResolver {
    pub lib: SoundLibrary,
    missing: HashSet<String>,
}

impl VoResolver {
    pub fn new(lib: SoundLibrary) -> Self {
        Self { lib, missing: HashSet::new() }
    }

    pub fn open(root: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        Ok(Self::new(SoundLibrary::new(root)?))
    }

    /// Event + bank -> the clip (first leaf) and its length.
    pub fn resolve(&mut self, event: &str, bank: &str) -> Option<VoClip> {
        if self.missing.contains(bank) {
            return None;
        }
        if self.lib.load_bank(bank).is_err() {
            self.missing.insert(bank.to_string());
            return None;
        }
        let cue = self.lib.resolve(event)?;
        let mut leaves = Vec::new();
        cue.leaves(&mut leaves);
        let leaf = *leaves.first()?;
        let length = self.lib.media_length(leaf.media).ok()?;
        Some(VoClip { leaf, length })
    }

    pub fn bank_missing(&self, bank: &str) -> bool {
        self.missing.contains(bank)
    }
}
