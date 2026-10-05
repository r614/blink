//! A set of loaded soundbanks with event resolution and on-demand decoding.
//!
//! Events are addressed by name; the id is `fnv1_32(name)` (verified against all
//! 1,553 `AkEvent` names in the game packages, research/LOG.md).

use crate::bank::Bank;
use crate::hirc::{self, Hierarchy, Node, StreamType};
use crate::pck::Pck;
use crate::vorbis::CodebookLibrary;
use crate::wem::Pcm;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub struct LoadedBank {
    pub name: String,
    pub pck: Pck,
    pub bank: Bank,
    pub hier: Hierarchy,
}

/// Where a media file lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MediaKey {
    pub bank: usize,
    pub id: u32,
    pub streamed: bool,
}

/// Resolved playback structure of an event.
#[derive(Debug, Clone)]
pub enum Cue {
    Leaf(Leaf),
    /// Play every child together (layers, multiple play actions, music segments).
    All(Vec<Cue>),
    /// Play one child, chosen by weight.
    Random(Vec<(Cue, f32)>),
}

#[derive(Debug, Clone, Copy)]
pub struct Leaf {
    pub media: MediaKey,
    /// Sum of the Volume properties along the hierarchy (dB).
    pub volume_db: f32,
    /// Pitch offset in cents.
    pub pitch_cents: f32,
    pub looping: bool,
    /// End of the attenuation's dry volume curve; `None` = 2D (no attenuation).
    pub max_distance: Option<f32>,
    /// Attenuation object (distance -> volume curve), see [`SoundLibrary::attenuation_db`].
    pub attenuation: Option<u32>,
}

impl Cue {
    /// Choose concrete leaves with `rand01` supplying uniform numbers in [0, 1).
    pub fn pick(&self, rand01: &mut dyn FnMut() -> f32, out: &mut Vec<Leaf>) {
        match self {
            Cue::Leaf(l) => out.push(*l),
            Cue::All(v) => v.iter().for_each(|c| c.pick(rand01, out)),
            Cue::Random(v) => {
                let total: f32 = v.iter().map(|c| c.1.max(0.0)).sum();
                if v.is_empty() {
                    return;
                }
                let mut x = rand01() * total;
                for (c, w) in v {
                    if x < *w || total <= 0.0 {
                        c.pick(rand01, out);
                        return;
                    }
                    x -= w;
                }
                v.last().unwrap().0.pick(rand01, out);
            }
        }
    }

    pub fn leaves(&self, out: &mut Vec<Leaf>) {
        match self {
            Cue::Leaf(l) => out.push(*l),
            Cue::All(v) => v.iter().for_each(|c| c.leaves(out)),
            Cue::Random(v) => v.iter().for_each(|c| c.0.leaves(out)),
        }
    }
}

pub struct SoundLibrary {
    pub root: PathBuf,
    pub codebooks: CodebookLibrary,
    pub banks: Vec<LoadedBank>,
    /// HIRC object id -> bank index.
    objects: HashMap<u32, usize>,
    cache: Mutex<HashMap<MediaKey, Arc<Pcm>>>,
    info_cache: Mutex<HashMap<MediaKey, (u64, u32, u16)>>,
    /// Global state/switch values (group id -> value), as set by SetState actions.
    pub states: Mutex<HashMap<u32, u32>>,
    /// Extension of the dialog banks to load (`INT` = english(us)).
    pub vo_language: String,
}

/// (frames, rate, channels) from a RIFF/WEM header (tolerates a truncated file).
fn wem_header_info(b: &[u8]) -> Option<(u64, u32, u16)> {
    let rd16 = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let rd32 = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()));
    if b.get(0..4)? != b"RIFF" {
        return None;
    }
    let mut o = 12;
    let (mut codec, mut ch, mut rate, mut align, mut frames) = (0u16, 0u16, 0u32, 1u16, None);
    while o + 8 <= b.len() {
        let tag = &b[o..o + 4];
        let size = rd32(o + 4)? as usize;
        let body = o + 8;
        if tag == b"fmt " {
            codec = rd16(body)?;
            ch = rd16(body + 2)?;
            rate = rd32(body + 4)?;
            align = rd16(body + 12)?;
            if codec == crate::wem::CODEC_VORBIS {
                frames = Some(rd32(body + 0x18)? as u64);
            }
        } else if tag == b"data" {
            if codec == crate::wem::CODEC_PCM {
                frames = Some(size as u64 / align.max(1) as u64);
            }
            break;
        }
        o = body + size + (size & 1);
    }
    Some((frames?, rate, ch))
}

pub fn pck_dirs(root: &Path) -> Vec<PathBuf> {
    let g = root.join("DishonoredGame");
    let mut v = vec![g.join("CookedPCConsole")];
    for d in ["DLC05", "DLC06", "DLC07"] {
        v.push(g.join("DLC").join("PCConsole").join(d));
    }
    v
}

impl SoundLibrary {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let codebooks = CodebookLibrary::from_install(&root)?;
        Ok(Self { root, codebooks, banks: Vec::new(), objects: HashMap::new(), cache: Mutex::new(HashMap::new()), info_cache: Mutex::new(HashMap::new()), states: Mutex::new(HashMap::new()), vo_language: "INT".into() })
    }

    pub fn find_pck(&self, name: &str) -> Option<PathBuf> {
        let want = format!("{}.pck", name.to_ascii_lowercase());
        for d in pck_dirs(&self.root) {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().to_ascii_lowercase() == want {
                    return Some(e.path());
                }
            }
        }
        None
    }

    /// Per-voice dialog banks are AKPK files named `Bk_<GUID>.<LANG>` (`.INT`, `.DEU`, `.FRA`,
    /// ...) in `CookedPCConsole`, not `.pck` (research/dishonored/BOYLE_MISSION.md section 4).
    pub fn find_vo_bank(&self, name: &str, lang: &str) -> Option<PathBuf> {
        let want = format!("{}.{}", name, lang).to_ascii_lowercase();
        for d in pck_dirs(&self.root) {
            let direct = d.join(format!("{name}.{lang}"));
            if direct.is_file() {
                return Some(direct);
            }
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().to_ascii_lowercase() == want {
                    return Some(e.path());
                }
            }
        }
        None
    }

    /// Load a bank package by name (e.g. `Bank_Player`, or a dialog bank `Bk_<GUID>`, which
    /// is looked up as `Bk_<GUID>.INT`). Idempotent.
    pub fn load_bank(&mut self, name: &str) -> Result<()> {
        if self.banks.iter().any(|b| b.name.eq_ignore_ascii_case(name)) {
            return Ok(());
        }
        let path = self
            .find_pck(name)
            .or_else(|| name.starts_with("Bk_").then(|| self.find_vo_bank(name, &self.vo_language)).flatten())
            .with_context(|| format!("no package {name}.pck"))?;
        let pck = Pck::open(&path)?;
        let be = *pck.banks.first().context("package has no soundbank")?;
        let bank = Bank::parse(&pck.read(&be)?)?;
        let hier = Hierarchy::from_bank(&bank);
        let idx = self.banks.len();
        for id in hier.nodes.keys() {
            self.objects.entry(*id).or_insert(idx);
        }
        self.banks.push(LoadedBank { name: name.to_string(), pck, bank, hier });
        Ok(())
    }

    pub fn node(&self, id: u32) -> Option<(usize, &Node)> {
        let b = *self.objects.get(&id)?;
        self.banks[b].hier.nodes.get(&id).map(|n| (b, n))
    }

    pub fn has_event(&self, name: &str) -> bool {
        matches!(self.node(crate::fnv1_32(name)), Some((_, Node::Event { .. })))
    }

    /// Apply the SetState/SetSwitch actions of an event to the global state table.
    pub fn apply_state_event(&self, name: &str) -> bool {
        let Some((_, Node::Event { actions })) = self.node(crate::fnv1_32(name)) else { return false };
        let mut st = self.states.lock().unwrap();
        for a in actions {
            if let Some((_, Node::Action(act))) = self.node(*a) {
                if let Some((g, s)) = act.set_group {
                    st.insert(g, s);
                }
            }
        }
        true
    }

    /// Resolve an event by name to its playback structure (Play actions only).
    pub fn resolve(&self, name: &str) -> Option<Cue> {
        self.resolve_id(crate::fnv1_32(name))
    }

    pub fn resolve_id(&self, event: u32) -> Option<Cue> {
        let (_, Node::Event { actions }) = self.node(event)? else { return None };
        // State/switch assignments made by the same event select switch-container branches.
        let mut states = self.states.lock().unwrap().clone();
        for a in actions {
            if let Some((_, Node::Action(act))) = self.node(*a) {
                if let Some((g, s)) = act.set_group {
                    if s != 0 {
                        states.insert(g, s);
                    }
                }
            }
        }
        let mut v = Vec::new();
        for a in actions {
            if let Some((_, Node::Action(act))) = self.node(*a) {
                if act.is_play() {
                    if let Some(c) = self.resolve_node(act.target, 0, &states) {
                        v.push(c);
                    }
                }
            }
        }
        match v.len() {
            0 => None,
            1 => v.pop(),
            _ => Some(Cue::All(v)),
        }
    }

    fn chain_props(&self, mut id: u32) -> (f32, f32, bool, Option<(f32, u32)>) {
        let (mut vol, mut pitch, mut looping, mut att) = (0.0, 0.0, false, None);
        let mut first = true;
        for _ in 0..32 {
            let Some((_, n)) = self.node(id) else { break };
            let Some(base) = n.base() else { break };
            vol += base.prop(hirc::PROP_VOLUME).unwrap_or(0.0);
            pitch += base.prop(hirc::PROP_PITCH).unwrap_or(0.0);
            if first {
                looping = base.prop(hirc::PROP_LOOP).is_some_and(|l| l == 0.0 || l > 1.0);
            }
            if att.is_none() {
                if let Some(a) = base.attenuation {
                    if let Some((_, Node::Attenuation(at))) = self.node(a) {
                        att = Some((at.max_distance, a));
                    }
                }
            }
            first = false;
            if base.parent == 0 {
                break;
            }
            id = base.parent;
        }
        (vol, pitch, looping, att)
    }

    pub fn media_key(&self, bank: usize, s: &hirc::Source) -> Option<MediaKey> {
        let streamed = s.stream != StreamType::Embedded;
        let has = |b: &LoadedBank| {
            if streamed {
                b.pck.streams.iter().any(|e| e.id == s.media_id)
            } else {
                b.bank.media.iter().any(|m| m.id == s.media_id)
            }
        };
        if has(&self.banks[bank]) {
            return Some(MediaKey { bank, id: s.media_id, streamed });
        }
        self.banks.iter().position(has).map(|b| MediaKey { bank: b, id: s.media_id, streamed })
    }

    fn resolve_node(&self, id: u32, depth: u32, states: &HashMap<u32, u32>) -> Option<Cue> {
        if depth > 24 {
            return None;
        }
        let (bank, node) = self.node(id)?;
        match node {
            Node::Sound { source, .. } => {
                let media = self.media_key(bank, source)?;
                let (volume_db, pitch_cents, looping, att) = self.chain_props(id);
                let (max_distance, attenuation) = (att.map(|a| a.0), att.map(|a| a.1));
                Some(Cue::Leaf(Leaf { media, volume_db, pitch_cents, looping, max_distance, attenuation }))
            }
            Node::MusicTrack { sources, .. } => {
                let v: Vec<(Cue, f32)> = sources
                    .iter()
                    .filter_map(|s| {
                        let media = self.media_key(bank, s)?;
                        Some((Cue::Leaf(Leaf { media, volume_db: 0.0, pitch_cents: 0.0, looping: true, max_distance: None, attenuation: None }), 1.0))
                    })
                    .collect();
                (!v.is_empty()).then_some(Cue::Random(v))
            }
            Node::Layer { children, .. } | Node::MusicSegment { music: hirc::MusicNode { children, .. }, .. } => {
                let v: Vec<Cue> = children.iter().filter_map(|c| self.resolve_node(*c, depth + 1, states)).collect();
                (!v.is_empty()).then_some(Cue::All(v))
            }
            Node::MusicSwitch { music, group, default_switch, assocs, .. } => {
                let want = states.get(group).copied().unwrap_or(*default_switch);
                let mut ids: Vec<u32> = assocs.iter().filter(|a| a.0 == want).map(|a| a.1).collect();
                if ids.is_empty() {
                    ids = music.children.clone();
                }
                let v: Vec<(Cue, f32)> = ids.iter().filter_map(|c| Some((self.resolve_node(*c, depth + 1, states)?, 1.0))).collect();
                (!v.is_empty()).then_some(Cue::Random(v))
            }
            Node::MusicRanSeq { music, .. } => {
                let v: Vec<(Cue, f32)> = music.children.iter().filter_map(|c| Some((self.resolve_node(*c, depth + 1, states)?, 1.0))).collect();
                (!v.is_empty()).then_some(Cue::Random(v))
            }
            Node::Switch { group, default_switch, children, groups, .. } => {
                let g = groups
                    .iter()
                    .find(|g| Some(&g.0) == states.get(group) && !g.1.is_empty())
                    .or_else(|| groups.iter().find(|g| g.0 == *default_switch && !g.1.is_empty()))
                    .or_else(|| groups.iter().find(|g| !g.1.is_empty()));
                let ids = g.map(|g| g.1.clone()).unwrap_or_else(|| children.clone());
                let v: Vec<(Cue, f32)> = ids.iter().filter_map(|c| Some((self.resolve_node(*c, depth + 1, states)?, 1.0))).collect();
                (!v.is_empty()).then_some(Cue::Random(v))
            }
            Node::RanSeq { children, playlist, .. } => {
                let v: Vec<(Cue, f32)> = children
                    .iter()
                    .filter_map(|c| {
                        let w = playlist.iter().find(|w| w.0 == *c).map(|w| w.1 as f32).unwrap_or(50000.0);
                        Some((self.resolve_node(*c, depth + 1, states)?, w.max(1.0)))
                    })
                    .collect();
                (!v.is_empty()).then_some(Cue::Random(v))
            }
            Node::ActorMixer { children, .. } => {
                let v: Vec<(Cue, f32)> = children.iter().filter_map(|c| Some((self.resolve_node(*c, depth + 1, states)?, 1.0))).collect();
                (!v.is_empty()).then_some(Cue::Random(v))
            }
            _ => None,
        }
    }

    /// Volume (dB) of an attenuation's dry curve at `distance` (game units), linear
    /// interpolation between points; beyond the last point the last value holds.
    pub fn attenuation_db(&self, att: u32, distance: f32) -> f32 {
        let Some((_, Node::Attenuation(a))) = self.node(att) else { return 0.0 };
        let c = &a.volume_curve;
        if c.is_empty() {
            return 0.0;
        }
        if distance <= c[0].0 {
            return c[0].1;
        }
        for w in c.windows(2) {
            if distance <= w[1].0 {
                let t = (distance - w[0].0) / (w[1].0 - w[0].0).max(1e-3);
                return w[0].1 + (w[1].1 - w[0].1) * t;
            }
        }
        c.last().unwrap().1
    }

    pub fn media_bytes(&self, k: MediaKey) -> Result<Vec<u8>> {
        let b = &self.banks[k.bank];
        if k.streamed {
            let e = b.pck.streams.iter().find(|e| e.id == k.id).context("stream not in package")?;
            b.pck.read(e)
        } else {
            b.bank.media_by_id(k.id).map(<[u8]>::to_vec).context("media not in bank")
        }
    }

    /// (frames, sample rate, channels) from the first bytes of the media only (cached), so
    /// the playback engine can time sequences without reading a whole stream.
    pub fn media_info(&self, k: MediaKey) -> Result<(u64, u32, u16)> {
        if let Some(i) = self.info_cache.lock().unwrap().get(&k) {
            return Ok(*i);
        }
        let b = &self.banks[k.bank];
        let head = if k.streamed {
            let e = b.pck.streams.iter().find(|e| e.id == k.id).context("stream not in package")?;
            b.pck.read(&crate::pck::PckEntry { size: e.size.min(8192), ..*e })?
        } else {
            b.bank.media_by_id(k.id).context("media not in bank")?.to_vec()
        };
        let info = wem_header_info(&head).context("media header")?;
        self.info_cache.lock().unwrap().insert(k, info);
        Ok(info)
    }

    /// Length in seconds from the media header alone (Vorbis `vorb` sample count, PCM data
    /// size), without decoding.
    pub fn media_length(&self, k: MediaKey) -> Result<f32> {
        let b = self.media_bytes(k)?;
        let r = crate::wem::Riff::parse(&b)?;
        let frames = match r.codec {
            crate::wem::CODEC_VORBIS => crate::vorbis::vorb_info(&r)?.sample_count as f32,
            crate::wem::CODEC_PCM => r.data()?.len() as f32 / r.block_align.max(1) as f32,
            c => anyhow::bail!("no length rule for codec 0x{c:04x}"),
        };
        Ok(frames / r.sample_rate.max(1) as f32)
    }

    /// Decode without caching (long ambience/music picks that are played once).
    pub fn decode_uncached(&self, k: MediaKey) -> Result<Pcm> {
        if let Some(p) = self.cache.lock().unwrap().get(&k) {
            return Ok((**p).clone());
        }
        crate::wem::decode(&self.media_bytes(k)?, &self.codebooks)
    }

    /// Decode (and cache) a media file.
    pub fn decode(&self, k: MediaKey) -> Result<Arc<Pcm>> {
        if let Some(p) = self.cache.lock().unwrap().get(&k) {
            return Ok(p.clone());
        }
        let pcm = Arc::new(crate::wem::decode(&self.media_bytes(k)?, &self.codebooks)?);
        self.cache.lock().unwrap().insert(k, pcm.clone());
        Ok(pcm)
    }
}
