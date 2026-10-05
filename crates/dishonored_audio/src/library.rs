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
    /// Global state/switch values (group id -> value), as set by SetState actions.
    pub states: Mutex<HashMap<u32, u32>>,
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
        Ok(Self { root, codebooks, banks: Vec::new(), objects: HashMap::new(), cache: Mutex::new(HashMap::new()), states: Mutex::new(HashMap::new()) })
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

    /// Load a bank package by name (e.g. `Bank_Player`). Idempotent.
    pub fn load_bank(&mut self, name: &str) -> Result<()> {
        if self.banks.iter().any(|b| b.name.eq_ignore_ascii_case(name)) {
            return Ok(());
        }
        let path = self.find_pck(name).with_context(|| format!("no package {name}.pck"))?;
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
            let base = match n {
                Node::Sound { base, .. } => base,
                Node::Container { base: Some(b), .. } => b,
                _ => break,
            };
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

    fn media_key(&self, bank: usize, s: &hirc::Source) -> Option<MediaKey> {
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
            Node::MusicTrack { sources } => {
                let v: Vec<(Cue, f32)> = sources
                    .iter()
                    .filter_map(|s| {
                        let media = self.media_key(bank, s)?;
                        Some((Cue::Leaf(Leaf { media, volume_db: 0.0, pitch_cents: 0.0, looping: true, max_distance: None, attenuation: None }), 1.0))
                    })
                    .collect();
                (!v.is_empty()).then_some(Cue::Random(v))
            }
            Node::Container { kind, children, weights, switch_groups, default_switch, group_id, .. } => {
                let sub = |ids: &[u32]| -> Vec<Cue> { ids.iter().filter_map(|c| self.resolve_node(*c, depth + 1, states)).collect() };
                match *kind {
                    hirc::HIRC_LAYER | hirc::HIRC_MUSIC_SEGMENT => {
                        let v = sub(children);
                        (!v.is_empty()).then_some(Cue::All(v))
                    }
                    hirc::HIRC_MUSIC_SWITCH if !switch_groups.is_empty() => {
                        // Group ids are not decoded for music switches: match any active state value.
                        let ids: Vec<u32> = switch_groups
                            .iter()
                            .filter(|g| states.values().any(|v| *v == g.0))
                            .flat_map(|g| g.1.iter().copied())
                            .collect();
                        let ids = if ids.is_empty() { children.clone() } else { ids };
                        let v = sub(&ids);
                        (!v.is_empty()).then(|| Cue::Random(v.into_iter().map(|c| (c, 1.0)).collect()))
                    }
                    hirc::HIRC_SWITCH => {
                        // Without game switch state: the default switch's group, else
                        // the first non-empty group, else any child.
                        let group = switch_groups
                            .iter()
                            .find(|g| Some(&g.0) == states.get(group_id) && !g.1.is_empty())
                            .or_else(|| switch_groups.iter().find(|g| g.0 == *default_switch && !g.1.is_empty()))
                            .or_else(|| switch_groups.iter().find(|g| !g.1.is_empty()));
                        let ids = group.map(|g| g.1.clone()).unwrap_or_else(|| children.clone());
                        let v = sub(&ids);
                        (!v.is_empty()).then(|| Cue::Random(v.into_iter().map(|c| (c, 1.0)).collect()))
                    }
                    _ => {
                        let v: Vec<(Cue, f32)> = children
                            .iter()
                            .filter_map(|c| {
                                let w = weights.iter().find(|w| w.0 == *c).map(|w| w.1 as f32).unwrap_or(50000.0);
                                Some((self.resolve_node(*c, depth + 1, states)?, w.max(1.0)))
                            })
                            .collect();
                        (!v.is_empty()).then_some(Cue::Random(v))
                    }
                }
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
