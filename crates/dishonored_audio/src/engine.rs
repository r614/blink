//! A software playback engine with the Wwise semantics the banks describe (bank v65):
//! events and their actions (play with delay / fade / probability, stop / pause / resume
//! with fades and object scope, set state / switch / game parameter, volume, pitch, LPF and
//! mute modifiers), the sound hierarchy (random/sequence containers in step and continuous
//! mode with loop counts, weights, avoid-repeat, shuffle and the five transition modes;
//! switch containers on states or per-object switches with continuous validation; blend
//! containers with RTPC crossfade curves), sound loops (Loop property and `smpl` loop
//! points), per-node volume / pitch / LPF with their randomizer ranges, state offsets and
//! RTPC curves, instance limits, 2D panning and 3D positioning with the attenuation curves
//! (dry volume, LPF, spread), and the interactive music hierarchy (`music` module).
//!
//! Everything is deterministic given the calls made on it (fixed-seed random numbers), and
//! renders interleaved stereo f32 at a fixed rate. Media are decoded on worker threads (or
//! synchronously for offline rendering); a voice whose media is not ready yet starts late.
//!
//! See research/dishonored/AUDIO_FORMAT.md, "Playback semantics", for what is decoded from
//! the data and what is approximated.

use crate::hirc::{self, CurvePoint, Node, NodeBase, RanSeqParams};
use crate::init::InitBank;
use crate::library::{MediaKey, SoundLibrary};
use crate::music::MusicInst;
use crate::wem::Pcm;
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

pub type ObjId = u64;
/// The listener's own object: 2D sounds posted without a position.
pub const GLOBAL_OBJ: ObjId = 0;
/// Frames between control updates (scheduling, gains). Starts are sample accurate.
pub const BLOCK: usize = 256;

/// Mixer categories mapped onto the options volumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Sfx = 0,
    Music = 1,
    Voice = 2,
}

/// Decoded media shared between the engine and its decode workers.
#[derive(Default)]
pub struct PcmStore {
    map: Mutex<HashMap<MediaKey, Arc<Pcm>>>,
    pending: Mutex<HashSet<MediaKey>>,
    failed: Mutex<HashSet<MediaKey>>,
}

impl PcmStore {
    pub fn get(&self, k: MediaKey) -> Option<Arc<Pcm>> {
        self.map.lock().unwrap().get(&k).cloned()
    }
    pub fn insert(&self, k: MediaKey, p: Arc<Pcm>) {
        self.map.lock().unwrap().insert(k, p);
        self.pending.lock().unwrap().remove(&k);
    }
    pub fn len(&self) -> usize {
        self.map.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Drop large decoded media no voice holds any more (long beds and music).
    fn evict_unused(&self, min_samples: usize) {
        self.map.lock().unwrap().retain(|_, p| Arc::strong_count(p) > 1 || p.samples.len() < min_samples);
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Ramp {
    pub(crate) start: u64,
    len: u64,
    from: f32,
    to: f32,
    curve: u32,
}

impl Ramp {
    fn value(&self, clock: u64) -> f32 {
        if self.len == 0 || clock >= self.start + self.len {
            return self.to;
        }
        if clock <= self.start {
            return self.from;
        }
        let t = (clock - self.start) as f32 / self.len as f32;
        let s = if self.to >= self.from { shape(self.curve, t) } else { 1.0 - shape(self.curve, 1.0 - t) };
        self.from + (self.to - self.from) * s
    }
    fn done(&self, clock: u64) -> bool {
        clock >= self.start + self.len
    }
}

pub(crate) fn ramp(start: u64, len: u64, from: f32, to: f32, curve: u32) -> Ramp {
    Ramp { start, len, from, to, curve }
}

/// AkCurveInterpolation shapes over t in [0, 1] (0 Log3, 1 Sine, 2 Log1, 3 InvSCurve,
/// 4 Linear, 5 SCurve, 6 Exp1, 7 SineRecip, 8 Exp3, 9 Constant).
pub fn shape(kind: u32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    use std::f32::consts::FRAC_PI_2;
    match kind {
        0 => 1.0 - (1.0 - t).powi(3),
        1 => (t * FRAC_PI_2).sin(),
        2 => 1.0 - (1.0 - t).powf(1.41),
        3 => {
            // Inverse S: fast-slow-fast.
            if t < 0.5 { 0.5 * (1.0 - (1.0 - 2.0 * t).powi(2)) } else { 0.5 + 0.5 * (2.0 * t - 1.0).powi(2) }
        }
        5 => 0.5 - 0.5 * (t * std::f32::consts::PI).cos(),
        6 => t.powf(1.41),
        7 => 1.0 - (t * FRAC_PI_2).cos(),
        8 => t.powi(3),
        9 => 0.0,
        _ => t,
    }
}

/// Evaluate a Wwise curve (points sorted by x; interpolation of each segment is the shape of
/// its first point).
pub fn eval_curve(points: &[CurvePoint], x: f32) -> f32 {
    let Some(first) = points.first() else { return 0.0 };
    if x <= first.x {
        return first.y;
    }
    for w in points.windows(2) {
        if x <= w[1].x {
            let span = (w[1].x - w[0].x).max(1e-6);
            let t = (x - w[0].x) / span;
            if w[0].interp == 9 {
                return w[0].y;
            }
            return w[0].y + (w[1].y - w[0].y) * shape(w[0].interp, t);
        }
    }
    points.last().unwrap().y
}

fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
}

/// Cutoff (Hz) for a Wwise LPF value 0..100 (APPROXIMATION: exponential from 20 kHz at 0 to
/// 20 Hz at 100; the runtime's coefficient table is not decoded, U-163).
pub fn lpf_cutoff(v: f32) -> f32 {
    20000.0 * 10f32.powf(-3.0 * (v.clamp(0.0, 100.0) / 100.0))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Listener {
    pub pos: [f32; 3],
    /// Unit vector to the listener's right (world space).
    pub right: [f32; 3],
}

#[derive(Debug, Clone, Copy, Default)]
struct Obj {
    pos: [f32; 3],
}

/// Per-voice static data rolled at start.
#[derive(Debug, Clone)]
pub(crate) struct Voice {
    pub id: u32,
    pub playing: u32,
    pub obj: ObjId,
    pub sound: u32,
    /// The sound and its parents up to the root (actor-mixer / music hierarchy).
    pub chain: Vec<u32>,
    /// Containers this voice was started through (for stops on containers that are not
    /// in the parent chain, e.g. music switches).
    pub via: Vec<u32>,
    pub media: MediaKey,
    pub pcm: Option<Arc<Pcm>>,
    pub start_at: u64,
    pub started: bool,
    /// Source position in frames.
    pub pos: f64,
    /// Source region [begin, end) in frames (music clips); `None` end = media end.
    pub src_begin: f64,
    pub src_end: Option<f64>,
    /// Remaining plays of the file: 0 = infinite.
    pub loops_left: u32,
    pub pitch_cents: f32,
    pub vol_db: f32,
    pub lpf: f32,
    pub att: Option<u32>,
    pub is_3d: bool,
    pub pan_lr: f32,
    pub fade: Option<Ramp>,
    /// Stop once the fade reaches zero.
    pub stop_on_fade: bool,
    /// Hard stop clock (music clips scheduled to end, crossfades).
    pub stop_at: Option<u64>,
    pub paused: bool,
    pub cat: Category,
    /// Layer crossfade curves: (game parameter, curve) in dB.
    pub layer_curves: Vec<(u32, Vec<CurvePoint>)>,
    pub lp: [f32; 2],
    pub gain: [f32; 2],
    pub done: bool,
    pub ended_at: Option<u64>,
    pub max_distance: f32,
    /// Media length and rate from the header (virtual voices advance without the PCM).
    pub media_frames: f64,
    pub media_rate: u32,
    /// Consecutive inaudible frames.
    pub virt_frames: u64,
}

/// Memory of a random/sequence container (per game object unless global).
#[derive(Debug, Clone, Default)]
struct RsMem {
    /// Sequence: next index; ping-pong direction.
    index: usize,
    backward: bool,
    /// Random: last picks (most recent last).
    history: Vec<usize>,
    /// Shuffle: items left in the current round.
    shuffle_left: Vec<usize>,
}

/// A continuous random/sequence container playing its playlist.
#[derive(Debug, Clone)]
struct SeqDriver {
    node: u32,
    obj: ObjId,
    playing: u32,
    via: Vec<u32>,
    params: RanSeqParams,
    /// Picks left in the current loop of the playlist.
    left_in_pass: usize,
    /// Loops left: 0 = infinite.
    loops_left: u32,
    cur: Vec<u32>,
    /// Clock at which the next item starts (when known in advance).
    next_at: Option<u64>,
    started_at: u64,
    stopped: bool,
    /// Per-play order for sequence containers that reset at each play.
    local: RsMem,
}

#[derive(Debug, Clone)]
struct SwitchWatch {
    node: u32,
    obj: ObjId,
    playing: u32,
    value: u32,
    via: Vec<u32>,
}

#[derive(Debug, Clone)]
enum Deferred {
    Play { node: u32, obj: ObjId, playing: u32, fade_ms: f32, fade_curve: u8 },
    Action { action: u32, obj: ObjId, playing: u32 },
}

/// Active SetVolume/SetPitch/SetLPF/Mute on a node (and its descendants).
#[derive(Debug, Clone, Default)]
struct Modifier {
    volume_db: f32,
    pitch: f32,
    lpf: f32,
    muted: bool,
}

#[derive(Debug, Default, Clone)]
pub struct EngineStats {
    pub events_posted: u64,
    pub voices_started: u64,
    pub voices_late: u64,
    pub max_voices: usize,
    /// Starts of the next item of continuous containers, by transition mode.
    pub transitions: [u64; 6],
    pub music_segments: u64,
    pub music_transitions: u64,
}

pub struct Engine {
    pub lib: Arc<SoundLibrary>,
    pub init: InitBank,
    pub rate: u32,
    pub store: Arc<PcmStore>,
    decode_tx: Option<Mutex<Sender<(Arc<SoundLibrary>, MediaKey)>>>,
    sync_decode: bool,
    pub clock: u64,
    rng: u64,
    pub states: HashMap<u32, u32>,
    switches: HashMap<(ObjId, u32), u32>,
    rtpc: HashMap<(ObjId, u32), f32>,
    objs: HashMap<ObjId, Obj>,
    pub listener: Listener,
    pub(crate) voices: Vec<Voice>,
    drivers: Vec<SeqDriver>,
    watches: Vec<SwitchWatch>,
    pub(crate) music: Vec<MusicInst>,
    deferred: Vec<(u64, Deferred)>,
    rs_mem: HashMap<(u32, ObjId), RsMem>,
    modifiers: HashMap<(u32, ObjId), Modifier>,
    next_voice: u32,
    next_playing: u32,
    /// Playing id -> event id.
    pub playing_events: HashMap<u32, u32>,
    /// Master and per-category gains (linear).
    pub master: f32,
    pub category_gain: [f32; 3],
    /// Bus id of the music hierarchy (voices under it use the music category).
    pub music_buses: HashSet<u32>,
    pub stats: EngineStats,
    /// Event ids posted (for tests and the harness): (clock, event, object).
    pub post_log: Vec<(u64, u32, ObjId)>,
    pub log_posts: bool,
    /// Segment history of finished music instances.
    pub music_log: Vec<(u64, u32)>,
}

impl Engine {
    pub fn new(lib: Arc<SoundLibrary>, rate: u32) -> Self {
        let init = InitBank::load(&lib.root).unwrap_or_default();
        // Buses the music hierarchy routes to: everything on them is music for the options.
        let mut music_buses = HashSet::new();
        for b in &lib.banks {
            for n in b.hier.nodes.values() {
                if matches!(n, Node::MusicSegment { .. } | Node::MusicSwitch { .. } | Node::MusicRanSeq { .. } | Node::MusicTrack { .. }) {
                    if let Some(base) = n.base() {
                        if base.bus != 0 {
                            music_buses.insert(base.bus);
                        }
                    }
                }
            }
        }
        Self {
            lib,
            init,
            rate,
            store: Arc::new(PcmStore::default()),
            decode_tx: None,
            sync_decode: true,
            clock: 0,
            rng: 0x2545_f491_4f6c_dd1d,
            states: HashMap::new(),
            switches: HashMap::new(),
            rtpc: HashMap::new(),
            objs: HashMap::new(),
            listener: Listener { pos: [0.0; 3], right: [0.0, 1.0, 0.0] },
            voices: Vec::new(),
            drivers: Vec::new(),
            watches: Vec::new(),
            music: Vec::new(),
            deferred: Vec::new(),
            rs_mem: HashMap::new(),
            modifiers: HashMap::new(),
            next_voice: 1,
            next_playing: 1,
            playing_events: HashMap::new(),
            master: 1.0,
            category_gain: [1.0; 3],
            music_buses,
            stats: EngineStats::default(),
            post_log: Vec::new(),
            log_posts: false,
            music_log: Vec::new(),
        }
    }

    /// Swap the loaded banks (level travel): everything playing stops; states, switches and
    /// game parameters persist (they are global in Wwise).
    pub fn replace_library(&mut self, lib: Arc<SoundLibrary>) {
        self.voices.clear();
        self.drivers.clear();
        self.watches.clear();
        self.music.clear();
        self.deferred.clear();
        self.modifiers.clear();
        self.objs.clear();
        self.store.map.lock().unwrap().clear();
        self.store.failed.lock().unwrap().clear();
        self.lib = lib;
    }

    /// Decode media on `threads` worker threads instead of inline.
    pub fn spawn_decoders(&mut self, threads: usize) {
        let (tx, rx) = channel::<(Arc<SoundLibrary>, MediaKey)>();
        let rx = Arc::new(Mutex::new(rx));
        for i in 0..threads.max(1) {
            let rx = rx.clone();
            let store = self.store.clone();
            std::thread::Builder::new()
                .name(format!("wwise-decode-{i}"))
                .spawn(move || {
                    loop {
                        let (lib, k) = match rx.lock().unwrap().recv() {
                            Ok(k) => k,
                            Err(_) => return,
                        };
                        match lib.decode_uncached(k) {
                            Ok(p) => store.insert(k, Arc::new(p)),
                            Err(_) => {
                                store.failed.lock().unwrap().insert(k);
                                store.pending.lock().unwrap().remove(&k);
                            }
                        }
                    }
                })
                .ok();
        }
        self.decode_tx = Some(Mutex::new(tx));
        self.sync_decode = false;
    }

    pub fn rand01(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn seed(&mut self, s: u64) {
        self.rng = s | 1;
    }

    /// Request a decode (non-blocking unless synchronous decoding is on).
    pub fn request(&self, k: MediaKey) {
        if self.store.get(k).is_some() || self.store.failed.lock().unwrap().contains(&k) {
            return;
        }
        if self.sync_decode {
            match self.lib.decode_uncached(k) {
                Ok(p) => self.store.insert(k, Arc::new(p)),
                Err(_) => {
                    self.store.failed.lock().unwrap().insert(k);
                }
            }
            return;
        }
        if self.store.pending.lock().unwrap().insert(k) {
            if let Some(tx) = &self.decode_tx {
                let _ = tx.lock().unwrap().send((self.lib.clone(), k));
            }
        }
    }

    // ----- game-side API -----

    pub fn set_position(&mut self, obj: ObjId, pos: [f32; 3]) {
        self.objs.entry(obj).or_default().pos = pos;
    }

    pub fn object_position(&self, obj: ObjId) -> Option<[f32; 3]> {
        self.objs.get(&obj).map(|o| o.pos)
    }

    /// Stop everything an object plays and forget it.
    pub fn remove_object(&mut self, obj: ObjId, fade_ms: f32) {
        self.stop_where(|v| v.obj == obj, fade_ms, 4);
        self.drivers.iter_mut().filter(|d| d.obj == obj).for_each(|d| d.stopped = true);
        self.objs.remove(&obj);
    }

    pub fn set_listener(&mut self, pos: [f32; 3], right: [f32; 3]) {
        self.listener = Listener { pos, right };
    }

    pub fn set_state(&mut self, group: u32, state: u32) {
        let old = self.states.insert(group, state);
        if old != Some(state) {
            self.on_group_change(group, None);
        }
    }

    pub fn set_switch(&mut self, obj: ObjId, group: u32, value: u32) {
        let old = self.switches.insert((obj, group), value);
        if old != Some(value) {
            self.on_group_change(group, Some(obj));
        }
    }

    pub fn set_rtpc(&mut self, obj: ObjId, id: u32, value: f32) {
        self.rtpc.insert((obj, id), value);
    }

    pub fn rtpc_value(&self, obj: ObjId, id: u32) -> f32 {
        self.rtpc
            .get(&(obj, id))
            .or_else(|| self.rtpc.get(&(GLOBAL_OBJ, id)))
            .copied()
            .or_else(|| self.init.params.get(&id).copied())
            .unwrap_or(0.0)
    }

    pub fn post_event_name(&mut self, name: &str, obj: ObjId) -> u32 {
        self.post_event(crate::fnv1_32(name), obj)
    }

    /// Post an event; returns its playing id (0 if the event is not loaded).
    pub fn post_event(&mut self, event: u32, obj: ObjId) -> u32 {
        let Some((_, Node::Event { actions })) = self.lib.node(event) else { return 0 };
        let actions = actions.clone();
        let playing = self.next_playing;
        self.next_playing += 1;
        self.playing_events.insert(playing, event);
        self.stats.events_posted += 1;
        if self.log_posts {
            self.post_log.push((self.clock, event, obj));
        }
        for a in actions {
            self.run_action(a, obj, playing, true);
        }
        playing
    }

    pub fn has_event(&self, name: &str) -> bool {
        self.lib.has_event(name)
    }

    /// Stop every voice of a playing id.
    pub fn stop_playing(&mut self, playing: u32, fade_ms: f32) {
        self.stop_where(|v| v.playing == playing, fade_ms, 4);
        self.drivers.iter_mut().filter(|d| d.playing == playing).for_each(|d| d.stopped = true);
        self.watches.retain(|w| w.playing != playing);
        let clock = self.clock;
        let rate = self.rate;
        self.music.iter_mut().filter(|m| m.playing == playing).for_each(|m| m.stop(clock, (fade_ms * rate as f32 / 1000.0) as u64));
    }

    pub fn is_playing(&self, playing: u32) -> bool {
        self.voices.iter().any(|v| v.playing == playing && !v.done)
            || self.drivers.iter().any(|d| d.playing == playing && !d.stopped)
            || self.music.iter().any(|m| m.playing == playing && !m.finished())
            || self.deferred.iter().any(|d| match &d.1 {
                Deferred::Play { playing: p, .. } | Deferred::Action { playing: p, .. } => *p == playing,
            })
    }

    pub fn active_voices(&self) -> usize {
        self.voices.iter().filter(|v| !v.done && v.started).count()
    }

    /// Ids of the sounds currently audible (started, not finished, not paused).
    pub fn playing_sounds(&self) -> Vec<(u32, u32, ObjId)> {
        self.voices.iter().filter(|v| v.started && !v.done && !v.paused).map(|v| (v.playing, v.sound, v.obj)).collect()
    }

    // ----- actions -----

    fn run_action(&mut self, id: u32, obj: ObjId, playing: u32, allow_delay: bool) {
        let Some((_, Node::Action(a))) = self.lib.node(id) else { return };
        let a = a.clone();
        let delay = a.delay_ms();
        if allow_delay && delay > 0.0 {
            let at = self.clock + (delay * self.rate as f32 / 1000.0) as u64;
            if a.is_play() {
                self.deferred.push((at, Deferred::Play { node: a.target, obj, playing, fade_ms: a.fade_ms(), fade_curve: a.fade_curve }));
            } else {
                self.deferred.push((at, Deferred::Action { action: id, obj, playing }));
            }
            return;
        }
        let class = a.action_type >> 8;
        let scope = (a.action_type & 0xFF) as u8;
        let only_obj = matches!(scope, 3 | 5 | 7);
        let fade = a.fade_ms();
        match class {
            0x04 => {
                if let Some(p) = a.props.iter().find(|p| p.0 == hirc::PROP_PROBABILITY).map(|p| p.1) {
                    if self.rand01() * 100.0 >= p {
                        return;
                    }
                }
                self.play_node(a.target, obj, playing, fade, a.fade_curve as u32, &[]);
            }
            0x01..=0x03 => {
                let target = a.target;
                let all = matches!(scope, 4..=7);
                let lib = self.lib.clone();
                let hit = |v: &Voice| (!only_obj || v.obj == obj) && (all || v.chain.contains(&target) || v.via.contains(&target));
                match class {
                    0x01 => {
                        let ids: Vec<u32> = self.voices.iter().filter(|v| hit(v)).map(|v| v.id).collect();
                        self.stop_where(|v| ids.contains(&v.id), fade, a.fade_curve as u32);
                        for d in &mut self.drivers {
                            if (!only_obj || d.obj == obj) && (all || d.node == target || d.via.contains(&target) || is_ancestor(&lib, target, d.node)) {
                                d.stopped = true;
                            }
                        }
                        let (clock, rate) = (self.clock, self.rate);
                        for m in &mut self.music {
                            if (!only_obj || m.obj == obj) && (all || m.root == target || m.contains_node(target)) {
                                m.stop(clock, (fade * rate as f32 / 1000.0) as u64);
                            }
                        }
                    }
                    0x02 => self.voices.iter_mut().filter(|v| hit(v)).for_each(|v| v.paused = true),
                    _ => self.voices.iter_mut().filter(|v| hit(v)).for_each(|v| v.paused = false),
                }
            }
            0x12 => {
                if let Some((g, s)) = a.set_group {
                    self.set_state(g, s);
                }
            }
            0x19 => {
                if let Some((g, s)) = a.set_group {
                    self.set_switch(obj, g, s);
                }
            }
            0x13 | 0x14 => {
                if let Some((meaning, base, min, max)) = a.value {
                    let r = self.rand01();
                    let v = base + min + (max - min) * r;
                    let key = (if only_obj { obj } else { GLOBAL_OBJ }, a.target);
                    let cur = self.rtpc_value(key.0, key.1);
                    let nv = if class == 0x14 {
                        self.init.params.get(&a.target).copied().unwrap_or(0.0)
                    } else if meaning == 1 {
                        cur + v
                    } else {
                        v
                    };
                    self.rtpc.insert(key, nv);
                }
            }
            0x06 | 0x07 | 0x08 | 0x09 | 0x0A | 0x0B | 0x0E | 0x0F => {
                let key = (a.target, if only_obj { obj } else { GLOBAL_OBJ });
                let m = self.modifiers.entry(key).or_default();
                let val = a.value.map(|v| v.1).unwrap_or(0.0);
                let rel = a.value.is_some_and(|v| v.0 == 1);
                match class {
                    0x06 => m.muted = true,
                    0x07 => m.muted = false,
                    0x08 => m.pitch = if rel { m.pitch + val } else { val },
                    0x09 => m.pitch = 0.0,
                    0x0A => m.volume_db = if rel { m.volume_db + val } else { val },
                    0x0B => m.volume_db = 0.0,
                    0x0E => m.lpf = if rel { m.lpf + val } else { val },
                    _ => m.lpf = 0.0,
                }
            }
            _ => {}
        }
    }

    fn stop_where(&mut self, f: impl Fn(&Voice) -> bool, fade_ms: f32, curve: u32) {
        let len = (fade_ms.max(0.0) * self.rate as f32 / 1000.0) as u64;
        let clock = self.clock;
        for v in self.voices.iter_mut().filter(|v| !v.done && f(v)) {
            if len == 0 || !v.started {
                v.done = true;
                v.ended_at = Some(clock);
            } else {
                let from = v.fade.map(|r| r.value(clock)).unwrap_or(1.0);
                v.fade = Some(Ramp { start: clock, len, from, to: 0.0, curve });
                v.stop_on_fade = true;
            }
        }
    }

    // ----- hierarchy -----

    fn base(&self, id: u32) -> Option<NodeBase> {
        self.lib.node(id).and_then(|(_, n)| n.base().cloned())
    }

    fn chain_of(&self, id: u32) -> Vec<u32> {
        let mut v = vec![id];
        let mut cur = id;
        for _ in 0..32 {
            let Some(b) = self.lib.node(cur).and_then(|(_, n)| n.base()) else { break };
            if b.parent == 0 || v.contains(&b.parent) {
                break;
            }
            v.push(b.parent);
            cur = b.parent;
        }
        v
    }

    /// Start a node. Returns the voices started directly (for continuous containers).
    pub(crate) fn play_node(&mut self, id: u32, obj: ObjId, playing: u32, fade_ms: f32, fade_curve: u32, via: &[u32]) -> Vec<u32> {
        self.play_node_at(id, obj, playing, fade_ms, fade_curve, via, self.clock)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn play_node_at(&mut self, id: u32, obj: ObjId, playing: u32, fade_ms: f32, fade_curve: u32, via: &[u32], at: u64) -> Vec<u32> {
        if via.len() > 24 {
            return Vec::new();
        }
        let Some((bank, node)) = self.lib.node(id) else { return Vec::new() };
        let node = node.clone();
        let mut path = via.to_vec();
        path.push(id);
        match node {
            Node::Sound { source, .. } => {
                let Some(media) = self.lib.media_key(bank, &source) else { return Vec::new() };
                self.start_voice(id, media, obj, playing, &path, fade_ms, fade_curve, at, Vec::new()).into_iter().collect()
            }
            Node::RanSeq { params, children, playlist, .. } => {
                if children.is_empty() {
                    return Vec::new();
                }
                if params.continuous {
                    let mut d = SeqDriver {
                        node: id,
                        obj,
                        playing,
                        via: path.clone(),
                        left_in_pass: playlist.len().max(children.len()),
                        loops_left: params.loop_count as u32,
                        params: params.clone(),
                        cur: Vec::new(),
                        next_at: None,
                        started_at: at,
                        stopped: false,
                        local: RsMem::default(),
                    };
                    let item = self.pick(&mut d, &children, &playlist);
                    let Some(item) = item else { return Vec::new() };
                    let started = self.play_node_at(item, obj, playing, fade_ms, fade_curve, &path, at);
                    d.cur = started.clone();
                    d.started_at = at;
                    self.schedule_next(&mut d);
                    self.drivers.push(d);
                    started
                } else {
                    let mut d = SeqDriver {
                        node: id,
                        obj,
                        playing,
                        via: path.clone(),
                        left_in_pass: 1,
                        loops_left: 1,
                        params: params.clone(),
                        cur: Vec::new(),
                        next_at: None,
                        started_at: at,
                        stopped: true,
                        local: RsMem::default(),
                    };
                    match self.pick(&mut d, &children, &playlist) {
                        Some(c) => self.play_node_at(c, obj, playing, fade_ms, fade_curve, &path, at),
                        None => Vec::new(),
                    }
                }
            }
            Node::Switch { group_type, group, default_switch, continuous, groups, .. } => {
                let value = self.group_value(group_type, group, obj, default_switch);
                let items = switch_items(&groups, value, default_switch);
                if continuous {
                    self.watches.push(SwitchWatch { node: id, obj, playing, value, via: path.clone() });
                }
                let mut out = Vec::new();
                for c in items {
                    out.extend(self.play_node_at(c, obj, playing, fade_ms, fade_curve, &path, at));
                }
                out
            }
            Node::Layer { children, layers, .. } => {
                let mut out = Vec::new();
                for c in children {
                    let started = self.play_node_at(c, obj, playing, fade_ms, fade_curve, &path, at);
                    for l in &layers {
                        for (child, pts) in &l.assocs {
                            if *child == c {
                                for vid in &started {
                                    if let Some(v) = self.voices.iter_mut().find(|v| v.id == *vid) {
                                        v.layer_curves.push((l.rtpc_id, pts.clone()));
                                    }
                                }
                            }
                        }
                    }
                    out.extend(started);
                }
                out
            }
            Node::ActorMixer { children, .. } => {
                // Not playable in Wwise; play all children as a layer (never seen posted).
                let mut out = Vec::new();
                for c in children {
                    out.extend(self.play_node_at(c, obj, playing, fade_ms, fade_curve, &path, at));
                }
                out
            }
            Node::MusicSwitch { .. } | Node::MusicRanSeq { .. } | Node::MusicSegment { .. } => {
                let m = MusicInst::new(self, id, obj, playing, at, fade_ms);
                self.music.push(m);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn group_value(&self, group_type: u8, group: u32, obj: ObjId, default: u32) -> u32 {
        if group_type == 1 {
            self.states.get(&group).copied().unwrap_or(default)
        } else {
            self.switches.get(&(obj, group)).or_else(|| self.switches.get(&(GLOBAL_OBJ, group))).copied().unwrap_or_else(|| {
                // Switch groups driven by a game parameter.
                self.init
                    .switch_groups
                    .iter()
                    .find(|g| g.id == group && g.rtpc != 0)
                    .and_then(|g| {
                        let x = self.rtpc_value(obj, g.rtpc);
                        g.points.iter().rev().find(|p| x >= p.0).or(g.points.first()).map(|p| p.1)
                    })
                    .unwrap_or(default)
            })
        }
    }

    /// Pick the next item of a random/sequence container (step or continuous).
    fn pick(&mut self, d: &mut SeqDriver, children: &[u32], playlist: &[(u32, i32)]) -> Option<u32> {
        let items: Vec<(u32, i32)> = if playlist.is_empty() { children.iter().map(|c| (*c, 50000)).collect() } else { playlist.to_vec() };
        if items.is_empty() {
            return None;
        }
        let key = (d.node, if d.params.global { GLOBAL_OBJ } else { d.obj });
        let use_local = d.params.continuous && d.params.reset_playlist;
        let mut mem = if use_local { std::mem::take(&mut d.local) } else { self.rs_mem.remove(&key).unwrap_or_default() };
        let n = items.len();
        let idx = if d.params.mode == 1 {
            // Sequence.
            let i = mem.index.min(n - 1);
            if d.params.restart_backward && n > 1 {
                if mem.backward {
                    if i == 0 {
                        mem.backward = false;
                        mem.index = 1;
                    } else {
                        mem.index = i - 1;
                    }
                } else if i + 1 >= n {
                    mem.backward = true;
                    mem.index = n.saturating_sub(2);
                } else {
                    mem.index = i + 1;
                }
            } else {
                mem.index = (i + 1) % n;
            }
            i
        } else if d.params.random_mode == 1 {
            // Shuffle: each item once per round, avoiding the last picks across rounds.
            if mem.shuffle_left.is_empty() {
                mem.shuffle_left = (0..n).collect();
            }
            let avoid = (d.params.avoid_repeat as usize).min(n.saturating_sub(1));
            let recent: Vec<usize> = mem.history.iter().rev().take(avoid).copied().collect();
            let cands: Vec<usize> = mem.shuffle_left.iter().copied().filter(|i| !recent.contains(i)).collect();
            let cands = if cands.is_empty() { mem.shuffle_left.clone() } else { cands };
            let r = self.rand01();
            let i = cands[((r * cands.len() as f32) as usize).min(cands.len() - 1)];
            mem.shuffle_left.retain(|x| *x != i);
            mem.history.push(i);
            i
        } else {
            // Standard random: weighted, excluding the last `avoid_repeat` picks.
            let avoid = (d.params.avoid_repeat as usize).min(n.saturating_sub(1));
            let recent: Vec<usize> = mem.history.iter().rev().take(avoid).copied().collect();
            let weights: Vec<f32> = (0..n)
                .map(|i| if recent.contains(&i) { 0.0 } else if d.params.using_weight { items[i].1.max(0) as f32 } else { 1.0 })
                .collect();
            let total: f32 = weights.iter().sum();
            let i = if total <= 0.0 {
                0
            } else {
                let mut x = self.rand01() * total;
                let mut pick = n - 1;
                for (i, w) in weights.iter().enumerate() {
                    if x < *w {
                        pick = i;
                        break;
                    }
                    x -= w;
                }
                pick
            };
            mem.history.push(i);
            i
        };
        if mem.history.len() > 64 {
            mem.history.drain(..32);
        }
        if use_local {
            d.local = mem;
        } else {
            self.rs_mem.insert(key, mem);
        }
        Some(items[idx].0)
    }

    /// Clock at which the given voices end, if all of them are finite and timed.
    fn voices_end(&self, ids: &[u32]) -> Option<u64> {
        let mut end = 0u64;
        for id in ids {
            let v = self.voices.iter().find(|v| v.id == *id)?;
            if v.loops_left == 0 {
                return None;
            }
            let (frames, rate, _) = self.lib.media_info(v.media).ok()?;
            let src_end = v.src_end.unwrap_or(frames as f64);
            let plays = v.loops_left as f64;
            let (ls, le) = v.pcm.as_ref().and_then(|p| p.loop_range).map(|l| (l.0 as f64, l.1 as f64)).unwrap_or((0.0, src_end));
            let src_frames = if plays > 1.0 { le - v.src_begin + (plays - 2.0) * (le - ls) + (src_end - ls) } else { src_end - v.src_begin };
            let ratio = rate as f64 / self.rate as f64 * 2f64.powf(v.pitch_cents as f64 / 1200.0);
            end = end.max(v.start_at + (src_frames / ratio).max(0.0) as u64);
        }
        Some(end)
    }

    fn schedule_next(&mut self, d: &mut SeqDriver) {
        let tt_ms = d.params.transition_time + d.params.transition_min + (d.params.transition_max - d.params.transition_min) * self.rand01();
        let tt = (tt_ms.max(0.0) * self.rate as f32 / 1000.0) as u64;
        d.next_at = match d.params.transition_mode {
            5 => Some(d.started_at + tt.max(1)),
            m => self.voices_end(&d.cur).map(|end| match m {
                3 => end + tt,
                1 | 2 => end.saturating_sub(tt).max(d.started_at + 1),
                _ => end,
            }),
        };
    }

    /// Advance continuous containers whose next item is due within this block.
    fn update_drivers(&mut self, block_end: u64) {
        let mut drivers = std::mem::take(&mut self.drivers);
        for d in drivers.iter_mut().filter(|d| !d.stopped) {
            let due = match d.next_at {
                Some(t) => t < block_end,
                None => !d.cur.is_empty() && d.cur.iter().all(|id| self.voices.iter().find(|v| v.id == *id).is_none_or(|v| v.done)),
            };
            if !due {
                continue;
            }
            let at = d.next_at.unwrap_or(self.clock).max(self.clock);
            // Next item of the pass, or the next loop.
            d.left_in_pass = d.left_in_pass.saturating_sub(1);
            if d.left_in_pass == 0 {
                if d.loops_left == 1 {
                    d.stopped = true;
                    continue;
                }
                if d.loops_left > 1 {
                    d.loops_left -= 1;
                }
                let Some((_, Node::RanSeq { children, playlist, .. })) = self.lib.node(d.node) else {
                    d.stopped = true;
                    continue;
                };
                d.left_in_pass = playlist.len().max(children.len());
            }
            let Some((_, Node::RanSeq { children, playlist, .. })) = self.lib.node(d.node) else {
                d.stopped = true;
                continue;
            };
            let (children, playlist) = (children.clone(), playlist.clone());
            let Some(item) = self.pick(d, &children, &playlist) else {
                d.stopped = true;
                continue;
            };
            let mode = d.params.transition_mode.min(5) as usize;
            self.stats.transitions[mode] += 1;
            let xf = if matches!(mode, 1 | 2) { d.params.transition_time } else { 0.0 };
            if xf > 0.0 {
                // Fade the current item out over the crossfade (its end falls at the end).
                let len = (xf * self.rate as f32 / 1000.0) as u64;
                let curve = if mode == 2 { 1 } else { 4 };
                for id in &d.cur {
                    if let Some(v) = self.voices.iter_mut().find(|v| v.id == *id && !v.done) {
                        v.fade = Some(Ramp { start: at, len, from: 1.0, to: 0.0, curve });
                        v.stop_on_fade = true;
                    }
                }
            }
            let via = d.via.clone();
            let started = self.play_node_at(item, d.obj, d.playing, xf, if mode == 2 { 1 } else { 4 }, &via, at);
            d.cur = started;
            d.started_at = at;
            let mut dd = d.clone();
            self.schedule_next(&mut dd);
            d.next_at = dd.next_at;
        }
        drivers.retain(|d| !d.stopped);
        drivers.extend(std::mem::take(&mut self.drivers));
        self.drivers = drivers;
    }

    fn on_group_change(&mut self, group: u32, obj: Option<ObjId>) {
        // Switch containers with continuous validation follow the new value.
        let watches = std::mem::take(&mut self.watches);
        let mut keep = Vec::new();
        for mut w in watches {
            if obj.is_some_and(|o| o != w.obj) {
                keep.push(w);
                continue;
            }
            let Some((_, Node::Switch { group_type, group: g, default_switch, groups, params, .. })) = self.lib.node(w.node) else { continue };
            if *g != group {
                keep.push(w);
                continue;
            }
            let (group_type, default_switch, groups, params) = (*group_type, *default_switch, groups.clone(), params.clone());
            let value = self.group_value(group_type, group, w.obj, default_switch);
            if value == w.value {
                keep.push(w);
                continue;
            }
            let old = switch_items(&groups, w.value, default_switch);
            let new = switch_items(&groups, value, default_switch);
            for c in &old {
                if new.contains(c) {
                    continue;
                }
                let p = params.iter().find(|p| p.node == *c);
                let fade = p.map(|p| p.fade_out_ms as f32).unwrap_or(0.0);
                let (playing, wobj) = (w.playing, w.obj);
                let c = *c;
                self.stop_where(|v| v.playing == playing && v.obj == wobj && (v.via.contains(&c) || v.chain.contains(&c)), fade, 4);
                self.drivers.iter_mut().filter(|d| d.playing == playing && d.obj == wobj && (d.node == c || d.via.contains(&c))).for_each(|d| d.stopped = true);
            }
            for c in &new {
                if old.contains(c) {
                    continue;
                }
                let p = params.iter().find(|p| p.node == *c);
                // "Play" mode (on_switch_mode 0) starts the new item; "stop" mode waits for a new post.
                if p.is_none_or(|p| p.on_switch_mode == 0) {
                    let fade = p.map(|p| p.fade_in_ms as f32).unwrap_or(0.0);
                    let via = w.via.clone();
                    self.play_node(*c, w.obj, w.playing, fade, 4, &via);
                }
            }
            w.value = value;
            keep.push(w);
        }
        keep.extend(std::mem::take(&mut self.watches));
        self.watches = keep;
        // Music switches.
        let mut music = std::mem::take(&mut self.music);
        for m in music.iter_mut() {
            m.on_group_change(self, group, obj);
        }
        music.extend(std::mem::take(&mut self.music));
        self.music = music;
    }

    /// Start one voice of a sound node.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_voice(
        &mut self,
        sound: u32,
        media: MediaKey,
        obj: ObjId,
        playing: u32,
        via: &[u32],
        fade_ms: f32,
        fade_curve: u32,
        at: u64,
        clip: Vec<f64>,
    ) -> Option<u32> {
        let chain = self.chain_of(sound);
        // Instance limit: the first node along the chain with a limit.
        for n in &chain {
            let Some(b) = self.base(*n) else { continue };
            if b.max_instances > 0 {
                let alive: Vec<(u32, u64)> = self
                    .voices
                    .iter()
                    .filter(|v| !v.done && !v.stop_on_fade && v.chain.contains(n) && (b.global_limit || v.obj == obj))
                    .map(|v| (v.id, v.start_at))
                    .collect();
                if alive.len() >= b.max_instances as usize {
                    if b.kill_newest {
                        return None;
                    }
                    if let Some(oldest) = alive.iter().min_by_key(|a| a.1) {
                        let id = oldest.0;
                        self.stop_where(|v| v.id == id, 10.0, 4);
                    }
                }
                break;
            }
        }
        let (mut vol, mut pitch, mut lpf) = (0.0f32, 0.0f32, 0.0f32);
        let (mut att, mut is_3d, mut pan_lr, mut loops) = (None, false, 0.0f32, 1u32);
        let mut positioned = false;
        let mut cat = Category::Sfx;
        for (i, n) in chain.iter().enumerate() {
            let Some((_, node)) = self.lib.node(*n) else { continue };
            if matches!(node, Node::MusicTrack { .. } | Node::MusicSegment { .. } | Node::MusicSwitch { .. } | Node::MusicRanSeq { .. }) {
                cat = Category::Music;
            }
            let Some(b) = node.base().cloned() else { continue };
            if self.music_buses.contains(&b.bus) {
                cat = Category::Music;
            }
            vol += b.prop(hirc::PROP_VOLUME).unwrap_or(0.0);
            pitch += b.prop(hirc::PROP_PITCH).unwrap_or(0.0);
            lpf += b.prop(hirc::PROP_LPF).unwrap_or(0.0);
            for (pid, lo, hi) in &b.ranges {
                let r = self.rand01();
                let x = lo + (hi - lo) * r;
                match *pid {
                    hirc::PROP_VOLUME => vol += x,
                    hirc::PROP_PITCH => pitch += x,
                    hirc::PROP_LPF => lpf += x,
                    _ => {}
                }
            }
            if i == 0 {
                if let Some(l) = b.prop(hirc::PROP_LOOP) {
                    loops = l.max(0.0) as u32;
                }
            }
            if !positioned {
                if let Some(p) = &b.positioning {
                    positioned = true;
                    is_3d = p.is_3d;
                    att = b.attenuation;
                    pan_lr = b.prop(hirc::PROP_PAN_LR).unwrap_or(0.0);
                }
            }
        }
        if !positioned {
            pan_lr = 0.0;
        }
        let max_distance = att
            .and_then(|a| match self.lib.node(a) {
                Some((_, Node::Attenuation(at))) => Some(at.max_distance),
                _ => None,
            })
            .unwrap_or(0.0);
        let (media_frames, media_rate) = self.lib.media_info(media).map(|i| (i.0 as f64, i.1)).unwrap_or((0.0, 48000));
        if !is_3d || media_frames <= 0.0 {
            self.request(media);
        }
        let pcm = self.store.get(media);
        let id = self.next_voice;
        self.next_voice += 1;
        let fade = (fade_ms > 0.0).then(|| Ramp { start: at, len: (fade_ms * self.rate as f32 / 1000.0) as u64, from: 0.0, to: 1.0, curve: fade_curve });
        let (src_begin, src_end) = match clip.as_slice() {
            [b, e] => (*b, Some(*e)),
            _ => (0.0, None),
        };
        self.voices.push(Voice {
            id,
            playing,
            obj,
            sound,
            chain,
            via: via.to_vec(),
            media,
            pcm,
            start_at: at,
            started: false,
            pos: src_begin,
            src_begin,
            src_end,
            loops_left: loops,
            pitch_cents: pitch,
            vol_db: vol,
            lpf,
            att,
            is_3d,
            pan_lr,
            fade,
            stop_on_fade: false,
            stop_at: None,
            paused: false,
            cat,
            layer_curves: Vec::new(),
            lp: [0.0; 2],
            gain: [0.0; 2],
            done: false,
            ended_at: None,
            max_distance,
            media_frames,
            media_rate,
            virt_frames: 0,
        });
        self.stats.voices_started += 1;
        Some(id)
    }

    pub(crate) fn voice_mut(&mut self, id: u32) -> Option<&mut Voice> {
        self.voices.iter_mut().find(|v| v.id == id)
    }

    pub(crate) fn voice(&self, id: u32) -> Option<&Voice> {
        self.voices.iter().find(|v| v.id == id)
    }

    // ----- mixing -----

    /// Dynamic volume (dB), pitch (cents) and LPF of a voice: states, RTPCs, modifiers.
    fn dynamic(&self, v: &Voice) -> (f32, f32, f32, bool) {
        let (mut vol, mut pitch, mut lpf, mut muted) = (0.0f32, 0.0f32, 0.0f32, false);
        for n in v.chain.iter().chain(v.via.iter()) {
            for o in [GLOBAL_OBJ, v.obj] {
                if let Some(m) = self.modifiers.get(&(*n, o)) {
                    vol += m.volume_db;
                    pitch += m.pitch;
                    lpf += m.lpf;
                    muted |= m.muted;
                }
                if o == GLOBAL_OBJ && v.obj == GLOBAL_OBJ {
                    break;
                }
            }
        }
        for n in &v.chain {
            let Some((_, node)) = self.lib.node(*n) else { continue };
            let Some(b) = node.base() else { continue };
            for sg in &b.states {
                let Some(cur) = self.states.get(&sg.group) else { continue };
                if let Some((_, st)) = sg.states.iter().find(|s| s.0 == *cur) {
                    if let Some((_, Node::State { props })) = self.lib.node(*st) {
                        for (pid, x) in props {
                            match *pid {
                                hirc::PROP_VOLUME => vol += x,
                                hirc::PROP_PITCH => pitch += x,
                                hirc::PROP_LPF => lpf += x,
                                _ => {}
                            }
                        }
                    }
                }
            }
            for r in &b.rtpcs {
                let x = self.rtpc_value(v.obj, r.rtpc_id);
                let y = eval_curve(&r.points, x);
                match r.param {
                    0 => vol += y,
                    2 => pitch += y,
                    3 => lpf += y,
                    _ => {}
                }
            }
        }
        for (rid, pts) in &v.layer_curves {
            vol += eval_curve(pts, self.rtpc_value(v.obj, *rid));
        }
        (vol, pitch, lpf, muted)
    }

    /// Target stereo gains and LPF of a voice for the current listener.
    fn spatial(&self, v: &Voice, base_db: f32) -> ([f32; 2], f32) {
        let lin = db(base_db);
        if !v.is_3d {
            // 2D: balance from the Pan L/R property (-100..100).
            let p = (v.pan_lr / 100.0).clamp(-1.0, 1.0);
            let l = if p > 0.0 { 1.0 - p } else { 1.0 };
            let r = if p < 0.0 { 1.0 + p } else { 1.0 };
            return ([lin * l, lin * r], 0.0);
        }
        let pos = self.objs.get(&v.obj).map(|o| o.pos).unwrap_or(self.listener.pos);
        let d = [pos[0] - self.listener.pos[0], pos[1] - self.listener.pos[1], pos[2] - self.listener.pos[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let (mut att_db, mut att_lpf, mut spread) = (0.0f32, 0.0f32, 0.0f32);
        if let Some(Node::Attenuation(a)) = v.att.and_then(|a| self.lib.node(a)).map(|n| n.1) {
            if dist > a.max_distance && a.max_distance > 0.0 {
                return ([0.0; 2], 0.0);
            }
            let curve = |i: i8| (i >= 0).then(|| a.curves.get(i as usize)).flatten().map(|c| eval_curve(&c.1, dist));
            att_db = curve(a.curve_use[0]).unwrap_or(0.0);
            att_lpf = curve(a.curve_use[2]).unwrap_or(0.0);
            spread = curve(a.curve_use[3]).unwrap_or(0.0) / 100.0;
        }
        let pan = if dist > 1e-3 {
            let r = self.listener.right;
            ((d[0] * r[0] + d[1] * r[1] + d[2] * r[2]) / dist).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let th = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
        let s = spread.clamp(0.0, 1.0);
        let c = std::f32::consts::FRAC_1_SQRT_2;
        let g = lin * db(att_db);
        ([g * (th.cos() * (1.0 - s) + c * s), g * (th.sin() * (1.0 - s) + c * s)], att_lpf)
    }

    /// Render interleaved stereo frames.
    pub fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        out.iter_mut().for_each(|s| *s = 0.0);
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(BLOCK);
            self.control(n as u64);
            self.mix(&mut out[done * 2..(done + n) * 2]);
            self.clock += n as u64;
            done += n;
        }
    }

    fn control(&mut self, n: u64) {
        let block_end = self.clock + n;
        // Deferred (delayed) actions due in this block start at their exact clock.
        let mut due: Vec<(u64, Deferred)> = Vec::new();
        self.deferred.retain(|d| {
            if d.0 < block_end {
                due.push(d.clone());
                false
            } else {
                true
            }
        });
        due.sort_by_key(|d| d.0);
        for (at, d) in due {
            match d {
                Deferred::Play { node, obj, playing, fade_ms, fade_curve } => {
                    self.play_node_at(node, obj, playing, fade_ms, fade_curve as u32, &[], at.max(self.clock));
                }
                Deferred::Action { action, obj, playing } => self.run_action(action, obj, playing, false),
            }
        }
        self.update_drivers(block_end);
        let mut music = std::mem::take(&mut self.music);
        for m in music.iter_mut() {
            m.update(self, block_end);
        }
        music.extend(std::mem::take(&mut self.music));
        for m in music.iter().filter(|m| m.finished()) {
            self.music_log.extend(m.history.iter().copied());
        }
        music.retain(|m| !m.finished());
        self.music = music;
        // Media that became ready.
        for v in self.voices.iter_mut().filter(|v| v.pcm.is_none() && !v.done) {
            v.pcm = self.store.get(v.media);
            if v.pcm.is_none() && self.store.failed.lock().unwrap().contains(&v.media) {
                v.done = true;
            }
        }
        self.voices.retain(|v| !v.done);
        let live = self.voices.len();
        self.stats.max_voices = self.stats.max_voices.max(live);
        if self.clock % (self.rate as u64 * 5) < n {
            self.store.evict_unused(self.rate as usize * 20);
        }
    }

    fn mix(&mut self, out: &mut [f32]) {
        let n = out.len() / 2;
        let clock = self.clock;
        let rate = self.rate as f32;
        let mut params: Vec<([f32; 2], f32, f32)> = Vec::with_capacity(self.voices.len());
        for v in &self.voices {
            let (dvol, dpitch, dlpf, muted) = self.dynamic(v);
            let (g, att_lpf) = self.spatial(v, v.vol_db + dvol);
            let cat = self.category_gain[v.cat as usize] * self.master;
            let g = if muted { [0.0; 2] } else { [g[0] * cat, g[1] * cat] };
            params.push((g, v.pitch_cents + dpitch, (v.lpf + dlpf + att_lpf).clamp(0.0, 100.0)));
        }
        let mut requests = Vec::new();
        for (vi, v) in self.voices.iter_mut().enumerate() {
            if v.done || v.paused {
                continue;
            }
            let (target, cents, lpf) = params[vi];
            let audible = target[0] + target[1] > 1e-7 || v.gain[0] + v.gain[1] > 1e-7;
            if !audible && v.is_3d && v.media_frames > 0.0 {
                // Virtual voice (out of range): time advances without decoding or mixing,
                // and the media is released after a while (resumes from the elapsed time).
                let first = if v.start_at > clock { (v.start_at - clock) as usize } else { 0 };
                if first >= n {
                    continue;
                }
                v.started = true;
                let step = v.media_rate as f64 / rate as f64 * 2f64.powf(cents as f64 / 1200.0);
                v.pos += (n - first) as f64 * step;
                let end = v.src_end.unwrap_or(v.media_frames).min(v.media_frames);
                if v.pos >= end {
                    if v.loops_left == 1 {
                        v.done = true;
                        v.ended_at = Some(clock + n as u64);
                        continue;
                    }
                    if v.loops_left > 1 {
                        v.loops_left -= 1;
                    }
                    let ls = v.pcm.as_ref().and_then(|p| p.loop_range).map(|l| l.0 as f64).unwrap_or(0.0);
                    v.pos = ls + (v.pos - end) % (end - ls).max(1.0);
                }
                if v.stop_at.is_some_and(|s| clock + n as u64 >= s) {
                    v.done = true;
                    continue;
                }
                v.virt_frames += n as u64;
                if v.virt_frames > rate as u64 * 5 {
                    v.pcm = None;
                }
                continue;
            }
            v.virt_frames = 0;
            let Some(pcm) = v.pcm.clone() else {
                // Media not ready: the voice waits (starts late).
                requests.push(v.media);
                if v.start_at < clock + n as u64 {
                    if v.started {
                        // Coming back from virtual: keep the elapsed position.
                    } else {
                        v.start_at = clock + n as u64;
                    }
                }
                continue;
            };
            let first = if v.start_at > clock { (v.start_at - clock) as usize } else { 0 };
            if first >= n {
                continue;
            }
            if !v.started {
                v.started = true;
            }
            let ch = pcm.channels.max(1) as usize;
            let frames = pcm.frames() as f64;
            let end = v.src_end.unwrap_or(frames).min(frames);
            let (ls, le) = pcm.loop_range.map(|l| (l.0 as f64, (l.1 as f64).min(end))).unwrap_or((0.0, end));
            let step = pcm.sample_rate as f64 / rate as f64 * 2f64.powf(cents as f64 / 1200.0);
            let a = if lpf > 0.0 { 1.0 - (-2.0 * std::f32::consts::PI * lpf_cutoff(lpf) / rate).exp() } else { 1.0 };
            let g0 = v.gain;
            for i in first..n {
                let t = (i - first) as f32 / (n - first) as f32;
                let gl = g0[0] + (target[0] - g0[0]) * t;
                let gr = g0[1] + (target[1] - g0[1]) * t;
                let c = clock + i as u64;
                let fg = match &v.fade {
                    Some(r) => r.value(c),
                    None => 1.0,
                };
                if v.stop_at.is_some_and(|s| c >= s) {
                    v.done = true;
                    v.ended_at = Some(c);
                    break;
                }
                // Loop / end handling.
                let loop_end = if v.loops_left != 1 { le } else { end };
                if v.pos >= loop_end {
                    if v.loops_left != 1 {
                        v.pos = ls + (v.pos - loop_end);
                        if v.loops_left > 1 {
                            v.loops_left -= 1;
                        }
                    } else {
                        v.done = true;
                        v.ended_at = Some(c);
                        break;
                    }
                }
                let p = v.pos;
                let i0 = p.floor() as usize;
                let fr = (p - i0 as f64) as f32;
                let i1 = if (i0 + 1) as f64 >= loop_end && v.loops_left != 1 { ls as usize } else { (i0 + 1).min(frames as usize - 1) };
                let s = |ix: usize, k: usize| pcm.samples.get(ix * ch + k.min(ch - 1)).copied().unwrap_or(0.0);
                let (mut l, mut r) = if ch == 1 {
                    let x = s(i0, 0) + (s(i1, 0) - s(i0, 0)) * fr;
                    (x, x)
                } else {
                    (s(i0, 0) + (s(i1, 0) - s(i0, 0)) * fr, s(i0, 1) + (s(i1, 1) - s(i0, 1)) * fr)
                };
                if a < 1.0 {
                    v.lp[0] += a * (l - v.lp[0]);
                    v.lp[1] += a * (r - v.lp[1]);
                    l = v.lp[0];
                    r = v.lp[1];
                }
                if v.is_3d && ch > 1 {
                    let m = 0.5 * (l + r);
                    l = m;
                    r = m;
                }
                out[i * 2] += l * gl * fg;
                out[i * 2 + 1] += r * gr * fg;
                v.pos += step;
            }
            v.gain = target;
            if let Some(r) = &v.fade {
                if v.stop_on_fade && r.done(clock + n as u64) && r.to <= 0.0 {
                    v.done = true;
                    v.ended_at = Some(clock + n as u64);
                }
            }
        }
        for k in requests {
            self.request(k);
        }
    }

    /// Segments started by the music instances (clock, segment id), oldest first.
    pub fn music_history(&self) -> Vec<(u64, u32)> {
        let mut v: Vec<(u64, u32)> = self.music.iter().flat_map(|m| m.history.iter().copied()).collect();
        v.extend(self.music_log.iter().copied());
        v.sort();
        v
    }

    /// Render `seconds` offline and return the stereo samples (harness, tests).
    pub fn render_seconds(&mut self, seconds: f32) -> Vec<f32> {
        let mut v = vec![0.0f32; (seconds * self.rate as f32) as usize * 2];
        self.render(&mut v);
        v
    }
}

fn switch_items(groups: &[(u32, Vec<u32>)], value: u32, default: u32) -> Vec<u32> {
    groups
        .iter()
        .find(|g| g.0 == value)
        .or_else(|| groups.iter().find(|g| g.0 == default))
        .map(|g| g.1.clone())
        .unwrap_or_default()
}

fn is_ancestor(lib: &SoundLibrary, anc: u32, mut id: u32) -> bool {
    for _ in 0..32 {
        if id == anc {
            return true;
        }
        let Some(b) = lib.node(id).and_then(|(_, n)| n.base()) else { return false };
        if b.parent == 0 {
            return false;
        }
        id = b.parent;
    }
    false
}

/// Seconds of non-silence and the longest silent gap (seconds) of a stereo render,
/// measured over 10 ms windows (harness checks of loop continuity).
pub fn silence_profile(samples: &[f32], rate: u32, threshold: f32) -> (f32, f32) {
    let win = (rate as usize / 100).max(1) * 2;
    let (mut loud, mut gap, mut max_gap) = (0usize, 0usize, 0usize);
    for w in samples.chunks(win) {
        let rms = (w.iter().map(|s| s * s).sum::<f32>() / w.len().max(1) as f32).sqrt();
        if rms > threshold {
            loud += 1;
            gap = 0;
        } else {
            gap += 1;
            max_gap = max_gap.max(gap);
        }
    }
    (loud as f32 / 100.0, max_gap as f32 / 100.0)
}
