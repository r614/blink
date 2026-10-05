//! HIRC objects of Dishonored's soundbanks (bank version 65), decoded completely:
//! every object type the retail banks use is read field by field and must end exactly
//! at the end of its body (`tools/audio_dump hirc` checks all 503 banks). Layouts were
//! measured on the retail banks; see research/dishonored/AUDIO_FORMAT.md.

use crate::bank::{Bank, HircObject};
use std::collections::HashMap;

pub const HIRC_STATE: u8 = 1;
pub const HIRC_SOUND: u8 = 2;
pub const HIRC_ACTION: u8 = 3;
pub const HIRC_EVENT: u8 = 4;
pub const HIRC_RANSEQ: u8 = 5;
pub const HIRC_SWITCH: u8 = 6;
pub const HIRC_ACTOR_MIXER: u8 = 7;
pub const HIRC_BUS: u8 = 8;
pub const HIRC_LAYER: u8 = 9;
pub const HIRC_MUSIC_SEGMENT: u8 = 10;
pub const HIRC_MUSIC_TRACK: u8 = 11;
pub const HIRC_MUSIC_SWITCH: u8 = 12;
pub const HIRC_MUSIC_RANSEQ: u8 = 13;
pub const HIRC_ATTENUATION: u8 = 14;
pub const HIRC_FX_SHARESET: u8 = 16;
pub const HIRC_FX_CUSTOM: u8 = 17;

/// Property ids of the v65 property bundle (AkPropID).
/// v65 has no bus-volume property, so the ids after LPF are one lower than in later
/// Wwise versions. Measured: Loop (6) is an int, 0 = infinite, on 967 sounds; actions
/// carry Delay (13, ms) on plays and set-states and TransitionTime (14, ms) on stops;
/// Probability (15) appears on 35 plays; Pan L/R and F/R (10, 11) are -100..100 on 2D
/// sounds; PriorityDistanceOffset (5) is -40 / -20 (floats).
pub const PROP_VOLUME: u8 = 0;
pub const PROP_LFE: u8 = 1;
pub const PROP_PITCH: u8 = 2;
pub const PROP_LPF: u8 = 3;
pub const PROP_PRIORITY: u8 = 4;
pub const PROP_PRIORITY_DISTANCE_OFFSET: u8 = 5;
pub const PROP_LOOP: u8 = 6;
pub const PROP_FEEDBACK_VOLUME: u8 = 7;
pub const PROP_FEEDBACK_LPF: u8 = 8;
pub const PROP_MUTE_RATIO: u8 = 9;
pub const PROP_PAN_LR: u8 = 10;
pub const PROP_PAN_FR: u8 = 11;
pub const PROP_CENTER_PCT: u8 = 12;
pub const PROP_DELAY_TIME: u8 = 13;
pub const PROP_TRANSITION_TIME: u8 = 14;
pub const PROP_PROBABILITY: u8 = 15;

/// Integer-valued properties (stored as i32 in the bundle).
fn int_prop(id: u8) -> bool {
    matches!(id, PROP_PRIORITY | PROP_LOOP | PROP_DELAY_TIME | PROP_TRANSITION_TIME)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    /// Media in the bank's DATA section.
    Embedded,
    /// Media streamed from the package's stream table.
    Streamed,
    /// Prefetched head in the bank, rest streamed.
    Prefetch,
}

#[derive(Debug, Clone, Copy)]
pub struct Source {
    pub plugin: u32,
    pub stream: StreamType,
    pub media_id: u32,
    pub file_id: u32,
    /// Source bits (bit 0: language specific, 1: prefetch, 3: non-cachable stream).
    pub bits: u8,
}

impl Source {
    pub fn is_vorbis(&self) -> bool {
        self.plugin == 0x0004_0001
    }
}

/// One RTPC curve on a node: game parameter -> property.
#[derive(Debug, Clone, Default)]
pub struct Rtpc {
    pub fx_id: u32,
    pub rtpc_id: u32,
    /// Target parameter (AkRTPC_ParameterID: 0 volume, 2 pitch, 3 LPF, ...).
    pub param: u32,
    pub curve_id: u32,
    /// Curve scaling (0 none, 2 dB, ...).
    pub scaling: u32,
    pub points: Vec<CurvePoint>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CurvePoint {
    pub x: f32,
    pub y: f32,
    /// AkCurveInterpolation (4 = linear, 9 = constant, ...).
    pub interp: u32,
}

/// Per-state property offsets of a node.
#[derive(Debug, Clone, Default)]
pub struct StateGroupRef {
    pub group: u32,
    pub sync_type: u8,
    /// (state id, state object id holding the property offsets).
    pub states: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Default)]
pub struct Positioning {
    pub is_2d: bool,
    pub is_3d: bool,
    pub panner: bool,
    /// 1 = 2D (none), 2 = user defined (paths), 3 = game defined.
    pub kind: u32,
    pub attenuation: u32,
    pub spatialized: bool,
    pub dynamic: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NodeBase {
    pub override_fx: bool,
    pub fx: Vec<u32>,
    pub bus: u32,
    pub parent: u32,
    pub props: Vec<(u8, f32)>,
    /// Randomizer ranges: (prop id, min, max).
    pub ranges: Vec<(u8, f32, f32)>,
    pub positioning: Option<Positioning>,
    pub attenuation: Option<u32>,
    /// Advanced settings: max instances (0 = no limit), kill newest, global limit.
    pub max_instances: u16,
    /// Virtual voice queue behaviour (0 from beginning, 1 from elapsed time, 2 resume).
    pub virtual_queue: u8,
    pub kill_newest: bool,
    pub use_virtual: bool,
    pub global_limit: bool,
    pub below_threshold: u8,
    pub states: Vec<StateGroupRef>,
    pub rtpcs: Vec<Rtpc>,
    /// Offset in the body just after the property bundles.
    pub after_props: usize,
}

impl NodeBase {
    pub fn prop(&self, id: u8) -> Option<f32> {
        self.props.iter().find(|p| p.0 == id).map(|p| p.1)
    }
    pub fn range(&self, id: u8) -> Option<(f32, f32)> {
        self.ranges.iter().find(|p| p.0 == id).map(|p| (p.1, p.2))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Action {
    pub action_type: u16,
    pub target: u32,
    pub props: Vec<(u8, f32)>,
    pub ranges: Vec<(u8, f32, f32)>,
    /// SetState / SetSwitch: (group id, state or switch id).
    pub set_group: Option<(u32, u32)>,
    /// Fade curve of play/stop/pause actions.
    pub fade_curve: u8,
    /// Play: bank holding the target.
    pub bank: u32,
    /// SetRTPC / SetVolume-like: value meaning, base, min, max.
    pub value: Option<(u8, f32, f32, f32)>,
    pub rtpc: u32,
}

impl Action {
    pub fn is_play(&self) -> bool {
        self.action_type >> 8 == 0x04
    }
    pub fn is_stop(&self) -> bool {
        self.action_type >> 8 == 0x01
    }
    pub fn is_set_state(&self) -> bool {
        self.action_type >> 8 == 0x12
    }
    pub fn is_set_switch(&self) -> bool {
        self.action_type >> 8 == 0x19
    }
    pub fn is_set_rtpc(&self) -> bool {
        self.action_type >> 8 == 0x13
    }
    /// Delay before the action executes (ms), from the action's properties.
    pub fn delay_ms(&self) -> f32 {
        self.props.iter().find(|p| p.0 == PROP_DELAY_TIME).map(|p| p.1).unwrap_or(0.0)
    }
    /// Fade time (ms): fade-in of a play, fade-out of a stop.
    pub fn fade_ms(&self) -> f32 {
        self.props.iter().find(|p| p.0 == PROP_TRANSITION_TIME).map(|p| p.1).unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Attenuation {
    /// Distance at the end of the dry volume curve (Wwise units = game units).
    pub max_distance: f32,
    /// (distance, volume dB) points of the dry volume curve.
    pub volume_curve: Vec<(f32, f32)>,
    /// Curve index used for dry volume, wet volume, LPF, spread (-1 = none).
    pub curve_use: [i8; 4],
    pub curves: Vec<(u8, Vec<CurvePoint>)>,
    pub cone: Option<[f32; 4]>,
    pub rtpcs: Vec<Rtpc>,
}

/// Random/sequence container parameters (CAkRanSeqCntr).
#[derive(Debug, Clone, Default)]
pub struct RanSeqParams {
    /// 0 = infinite, 1 = play once.
    pub loop_count: u16,
    /// Transition duration (seconds) for crossfade / delay / trigger rate.
    pub transition_time: f32,
    pub transition_min: f32,
    pub transition_max: f32,
    /// Random: avoid repeating the last N picks.
    pub avoid_repeat: u16,
    /// 0 none, 1 crossfade (amp), 2 crossfade (power), 3 delay, 4 sample accurate, 5 trigger rate.
    pub transition_mode: u8,
    /// 0 normal (avoid repeat), 1 shuffle.
    pub random_mode: u8,
    /// 0 random, 1 sequence.
    pub mode: u8,
    pub using_weight: bool,
    pub reset_playlist: bool,
    pub restart_backward: bool,
    pub continuous: bool,
    pub global: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SwitchParam {
    pub node: u32,
    pub first_only: bool,
    pub continue_playback: bool,
    pub on_switch_mode: u32,
    pub fade_out_ms: i32,
    pub fade_in_ms: i32,
}

#[derive(Debug, Clone, Default)]
pub struct LayerDef {
    pub id: u32,
    pub rtpcs: Vec<Rtpc>,
    pub rtpc_id: u32,
    /// (child, crossfade curve points).
    pub assocs: Vec<(u32, Vec<CurvePoint>)>,
}

#[derive(Debug, Clone, Default)]
pub struct Stinger {
    pub trigger: u32,
    pub segment: u32,
    pub sync_play_at: u32,
    pub dont_repeat_ms: i32,
    pub look_ahead: u32,
}

/// Common part of music nodes (CAkMusicNode).
#[derive(Debug, Clone, Default)]
pub struct MusicNode {
    pub base: NodeBase,
    pub children: Vec<u32>,
    pub grid_period_ms: f64,
    pub grid_offset_ms: f64,
    pub tempo: f32,
    pub beats_per_bar: u8,
    pub beat_value: u8,
    pub meter_override: bool,
    pub stingers: Vec<Stinger>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Fade {
    pub time_ms: i32,
    pub curve: u32,
    pub offset_ms: i32,
}

/// Music transition rule (AkMusicTransitionRule).
#[derive(Debug, Clone, Default)]
pub struct TransRule {
    /// Source/destination ids (-1 as u32 = any).
    pub src: Vec<u32>,
    pub dst: Vec<u32>,
    pub src_fade: Fade,
    /// AkSyncType: 0 immediate, 1 next grid, 2 next bar, 3 next beat, 4 next cue,
    /// 5 custom cue, 6 entry cue, 7 exit cue.
    pub sync: u32,
    pub play_post_exit: bool,
    pub dst_fade: Fade,
    pub jump_to: u32,
    /// 0 entry cue, 1 same time, 2 random cue.
    pub entry_type: u16,
    pub play_pre_entry: bool,
    pub transition_segment: Option<(u32, Fade, Fade, bool, bool)>,
}

#[derive(Debug, Clone, Default)]
pub struct MusicPlaylistItem {
    pub segment: u32,
    pub item_id: u32,
    /// 0 sequence continuous, 1 sequence step, 2 random continuous, 3 random step.
    pub rs_type: u32,
    pub loop_count: i16,
    pub weight: u32,
    pub avoid_repeat: u16,
    pub using_weight: bool,
    pub shuffle: bool,
    pub children: Vec<MusicPlaylistItem>,
}

/// One clip of a music track (AkTrackSrcInfo), times in ms.
#[derive(Debug, Clone, Copy, Default)]
pub struct TrackClip {
    pub track: u32,
    pub source: u32,
    pub play_at: f64,
    pub begin_trim: f64,
    pub end_trim: f64,
    pub src_duration: f64,
}

#[derive(Debug, Clone)]
pub enum Node {
    Sound { base: NodeBase, source: Source },
    RanSeq { base: NodeBase, params: RanSeqParams, children: Vec<u32>, playlist: Vec<(u32, i32)> },
    Switch { base: NodeBase, group_type: u8, group: u32, default_switch: u32, continuous: bool, children: Vec<u32>, groups: Vec<(u32, Vec<u32>)>, params: Vec<SwitchParam> },
    ActorMixer { base: NodeBase, children: Vec<u32> },
    Layer { base: NodeBase, children: Vec<u32>, layers: Vec<LayerDef> },
    MusicSegment { music: MusicNode, duration_ms: f64, markers: Vec<(u32, f64)> },
    MusicTrack { base: NodeBase, sources: Vec<Source>, clips: Vec<TrackClip>, subtracks: u32, rs_type: u32, look_ahead_ms: i32 },
    MusicSwitch { music: MusicNode, rules: Vec<TransRule>, group_type: u32, group: u32, default_switch: u32, continue_playback: bool, assocs: Vec<(u32, u32)> },
    MusicRanSeq { music: MusicNode, rules: Vec<TransRule>, playlist: Vec<MusicPlaylistItem> },
    Event { actions: Vec<u32> },
    Action(Action),
    Attenuation(Attenuation),
    /// Property offsets applied while a state is active (volume dB, LPF, pitch...).
    State { props: Vec<(u8, f32)> },
    Other(u8),
}

impl Node {
    pub fn base(&self) -> Option<&NodeBase> {
        match self {
            Node::Sound { base, .. } | Node::RanSeq { base, .. } | Node::Switch { base, .. } | Node::ActorMixer { base, .. } | Node::Layer { base, .. } | Node::MusicTrack { base, .. } => Some(base),
            Node::MusicSegment { music, .. } | Node::MusicSwitch { music, .. } | Node::MusicRanSeq { music, .. } => Some(&music.base),
            _ => None,
        }
    }
    pub fn children(&self) -> &[u32] {
        match self {
            Node::RanSeq { children, .. } | Node::Switch { children, .. } | Node::ActorMixer { children, .. } | Node::Layer { children, .. } => children,
            Node::MusicSegment { music, .. } | Node::MusicSwitch { music, .. } | Node::MusicRanSeq { music, .. } => &music.children,
            _ => &[],
        }
    }
    pub fn kind(&self) -> u8 {
        match self {
            Node::Sound { .. } => HIRC_SOUND,
            Node::RanSeq { .. } => HIRC_RANSEQ,
            Node::Switch { .. } => HIRC_SWITCH,
            Node::ActorMixer { .. } => HIRC_ACTOR_MIXER,
            Node::Layer { .. } => HIRC_LAYER,
            Node::MusicSegment { .. } => HIRC_MUSIC_SEGMENT,
            Node::MusicTrack { .. } => HIRC_MUSIC_TRACK,
            Node::MusicSwitch { .. } => HIRC_MUSIC_SWITCH,
            Node::MusicRanSeq { .. } => HIRC_MUSIC_RANSEQ,
            Node::Event { .. } => HIRC_EVENT,
            Node::Action(_) => HIRC_ACTION,
            Node::Attenuation(_) => HIRC_ATTENUATION,
            Node::State { .. } => HIRC_STATE,
            Node::Other(k) => *k,
        }
    }
}

pub(crate) struct Cur<'a> {
    pub b: &'a [u8],
    pub o: usize,
}

impl<'a> Cur<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.o)?;
        self.o += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let v = u16::from_le_bytes(self.b.get(self.o..self.o + 2)?.try_into().ok()?);
        self.o += 2;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let v = u32::from_le_bytes(self.b.get(self.o..self.o + 4)?.try_into().ok()?);
        self.o += 4;
        Some(v)
    }
    fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn f64(&mut self) -> Option<f64> {
        let v = f64::from_le_bytes(self.b.get(self.o..self.o + 8)?.try_into().ok()?);
        self.o += 8;
        Some(v)
    }
    fn ids(&mut self, n: usize) -> Option<Vec<u32>> {
        if n > 1 << 16 {
            return None;
        }
        (0..n).map(|_| self.u32()).collect()
    }
}

fn source(c: &mut Cur) -> Option<Source> {
    let plugin = c.u32()?;
    let st = c.u32()?;
    let media_id = c.u32()?;
    let file_id = c.u32()?;
    let stream = match st {
        0 => StreamType::Embedded,
        1 => StreamType::Streamed,
        _ => StreamType::Prefetch,
    };
    if stream != StreamType::Streamed {
        let _offset = c.u32()?;
        let _size = c.u32()?;
    }
    let bits = c.u8()?;
    // Plugin sources (type 2 = source plugin) carry their parameter block.
    if plugin & 0x0F == 2 {
        let size = c.u32()? as usize;
        c.o += size;
    }
    Some(Source { plugin, stream, media_id, file_id, bits })
}

fn prop_bundle(c: &mut Cur) -> Option<Vec<(u8, f32)>> {
    let n = c.u8()? as usize;
    let ids: Vec<u8> = (0..n).map(|_| c.u8()).collect::<Option<_>>()?;
    let mut props = Vec::with_capacity(n);
    for id in ids {
        let raw = c.u32()?;
        let v = if int_prop(id) { raw as i32 as f32 } else { f32::from_bits(raw) };
        props.push((id, v));
    }
    Some(props)
}

fn range_bundle(c: &mut Cur) -> Option<Vec<(u8, f32, f32)>> {
    let n = c.u8()? as usize;
    let ids: Vec<u8> = (0..n).map(|_| c.u8()).collect::<Option<_>>()?;
    let mut v = Vec::with_capacity(n);
    for id in ids {
        let (a, b) = if int_prop(id) { (c.i32()? as f32, c.i32()? as f32) } else { (c.f32()?, c.f32()?) };
        v.push((id, a, b));
    }
    Some(v)
}

fn curve_points(c: &mut Cur, n: usize) -> Option<Vec<CurvePoint>> {
    if n > 4096 {
        return None;
    }
    (0..n).map(|_| Some(CurvePoint { x: c.f32()?, y: c.f32()?, interp: c.u32()? })).collect()
}

/// InitialRTPC: u16 count, {u32 rtpc id, u32 param, u32 curve id, u8 scaling, u16 n, points}.
fn rtpcs(c: &mut Cur) -> Option<Vec<Rtpc>> {
    let n = c.u16()? as usize;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let fx_id = 0;
        let rtpc_id = c.u32()?;
        let param = c.u32()?;
        let curve_id = c.u32()?;
        let scaling = c.u8()? as u32;
        let np = c.u16()? as usize;
        let points = curve_points(c, np)?;
        v.push(Rtpc { fx_id, rtpc_id, param, curve_id, scaling, points });
    }
    Some(v)
}

fn node_base(c: &mut Cur) -> Option<NodeBase> {
    let mut nb = NodeBase { override_fx: c.u8()? != 0, ..Default::default() };
    let nfx = c.u8()?;
    if nfx > 0 {
        let _bypass = c.u8()?;
        for _ in 0..nfx {
            let _index = c.u8()?;
            nb.fx.push(c.u32()?);
            let _shareset = c.u8()?;
            let _rendered = c.u8()?;
        }
    }
    nb.bus = c.u32()?;
    nb.parent = c.u32()?;
    let _prio_override = c.u8()?;
    let _prio_dist = c.u8()?;
    nb.props = prop_bundle(c)?;
    nb.ranges = range_bundle(c)?;
    nb.after_props = c.o;
    // Positioning.
    if c.u8()? != 0 {
        // One flag: 3D, else 2D with the panner flag.
        let is_3d = c.u8()? != 0;
        let mut p = Positioning { is_2d: !is_3d, is_3d, ..Default::default() };
        if !is_3d {
            p.panner = c.u8()? != 0;
        }
        if is_3d {
            p.kind = c.u32()?;
            p.attenuation = c.u32()?;
            p.spatialized = c.u8()? != 0;
            match p.kind {
                2 => {
                    // User-defined positioning: path mode, looping, transition, follow
                    // orientation, vertices, playlist and per-path random ranges.
                    let _path_mode = c.u32()?;
                    let _looping = c.u8()?;
                    let _transition = c.i32()?;
                    let _follow = c.u8()?;
                    let nv = c.u32()? as usize;
                    if nv > 1 << 16 {
                        return None;
                    }
                    c.o += nv * 16;
                    let np = c.u32()? as usize;
                    if np > 1 << 16 {
                        return None;
                    }
                    c.o += np * 8;
                    c.o += np * 8;
                }
                3 => p.dynamic = c.u8()? != 0,
                _ => {}
            }
            if p.attenuation != 0 {
                nb.attenuation = Some(p.attenuation);
            }
        }
        nb.positioning = Some(p);
    }
    // Advanced settings.
    nb.virtual_queue = c.u8()?;
    nb.kill_newest = c.u8()? != 0;
    nb.use_virtual = c.u8()? != 0;
    nb.max_instances = c.u16()?;
    nb.global_limit = c.u8()? != 0;
    nb.below_threshold = c.u8()?;
    let _max_override = c.u8()?;
    let _vvoice_override = c.u8()?;
    // States.
    let ng = c.u32()? as usize;
    if ng > 1024 {
        return None;
    }
    for _ in 0..ng {
        let group = c.u32()?;
        let sync_type = c.u8()?;
        let ns = c.u16()? as usize;
        let states = (0..ns).map(|_| Some((c.u32()?, c.u32()?))).collect::<Option<Vec<_>>>()?;
        nb.states.push(StateGroupRef { group, sync_type, states });
    }
    nb.rtpcs = rtpcs(c)?;
    Some(nb)
}

/// v65 action: type u16, target u32, property bundle, ranged bundle, then type-specific parameters.
fn action(c: &mut Cur) -> Option<Action> {
    let action_type = c.u16()?;
    let target = c.u32()?;
    let props = prop_bundle(c)?;
    let ranges = range_bundle(c)?;
    let mut a = Action { action_type, target, props, ranges, ..Default::default() };
    let class = action_type >> 8;
    let except = |c: &mut Cur| -> Option<()> {
        let n = c.u32()? as usize;
        if n > 4096 {
            return None;
        }
        c.o += n * 5;
        Some(())
    };
    match class {
        // Stop, Pause, Resume.
        0x01..=0x03 => {
            a.fade_curve = c.u8()?;
            if class == 0x02 || class == 0x03 {
                let _include_pending = c.u8()?;
            }
            except(c)?;
        }
        // Play.
        0x04 => {
            a.fade_curve = c.u8()?;
            a.bank = c.u32()?;
        }
        // Mute / unmute, set/reset pitch, volume, LFE, LPF, bus volume.
        0x06 | 0x07 | 0x08 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D | 0x0E | 0x0F | 0x20 | 0x30 => {
            a.fade_curve = c.u8()?;
            if !matches!(class, 0x06 | 0x07) {
                let meaning = c.u8()?;
                let (base, min, max) = (c.f32()?, c.f32()?, c.f32()?);
                a.value = Some((meaning, base, min, max));
            }
            except(c)?;
        }
        0x12 => a.set_group = Some((c.u32()?, c.u32()?)),
        0x19 => a.set_group = Some((c.u32()?, c.u32()?)),
        // Set / reset game parameter.
        0x13 | 0x14 => {
            a.fade_curve = c.u8()?;
            let meaning = c.u8()?;
            let (base, min, max) = (c.f32()?, c.f32()?, c.f32()?);
            a.value = Some((meaning, base, min, max));
            except(c)?;
        }
        // Use state / unuse state, bypass FX, break, trigger, seek, events.
        0x1A => {
            let _bypass = c.u8()?;
            let _mask = c.u8()?;
            except(c)?;
        }
        0x1E => {
            let _relative = c.u8()?;
            let _v = (c.f32()?, c.f32()?, c.f32()?);
            let _snap = c.u8()?;
            except(c)?;
        }
        _ => {}
    }
    Some(a)
}

fn attenuation(c: &mut Cur) -> Option<Attenuation> {
    let mut a = Attenuation::default();
    if c.u8()? != 0 {
        a.cone = Some([c.f32()?, c.f32()?, c.f32()?, c.f32()?]);
    }
    for u in &mut a.curve_use {
        *u = c.u8()? as i8;
    }
    let ncurves = c.u8()? as usize;
    for _ in 0..ncurves {
        let scaling = c.u8()?;
        let n = c.u16()? as usize;
        a.curves.push((scaling, curve_points(c, n)?));
    }
    a.rtpcs = rtpcs(c)?;
    let dry: Vec<(f32, f32)> = a.curves.get(a.curve_use[0].max(0) as usize).map(|c| c.1.iter().map(|p| (p.x, p.y)).collect()).unwrap_or_default();
    a.max_distance = dry.last().map(|p| p.0).unwrap_or(0.0);
    a.volume_curve = dry;
    Some(a)
}

fn children(c: &mut Cur) -> Option<Vec<u32>> {
    let n = c.u32()? as usize;
    c.ids(n)
}

fn ranseq(c: &mut Cur) -> Option<Node> {
    let base = node_base(c)?;
    let p = RanSeqParams {
        loop_count: c.u16()?,
        transition_time: c.f32()?,
        transition_min: c.f32()?,
        transition_max: c.f32()?,
        avoid_repeat: c.u16()?,
        transition_mode: c.u8()?,
        random_mode: c.u8()?,
        mode: c.u8()?,
        using_weight: c.u8()? != 0,
        reset_playlist: c.u8()? != 0,
        restart_backward: c.u8()? != 0,
        continuous: c.u8()? != 0,
        global: c.u8()? != 0,
    };
    let children = children(c)?;
    let n = c.u16()? as usize;
    let playlist = (0..n).map(|_| Some((c.u32()?, c.i32()?))).collect::<Option<Vec<_>>>()?;
    Some(Node::RanSeq { base, params: p, children, playlist })
}

fn switch(c: &mut Cur) -> Option<Node> {
    let base = node_base(c)?;
    let group_type = c.u32()? as u8;
    let group = c.u32()?;
    let default_switch = c.u32()?;
    let continuous = c.u8()? != 0;
    let children = children(c)?;
    let ng = c.u32()? as usize;
    if ng > 4096 {
        return None;
    }
    let mut groups = Vec::with_capacity(ng);
    for _ in 0..ng {
        let sw = c.u32()?;
        let n = c.u32()? as usize;
        groups.push((sw, c.ids(n)?));
    }
    let np = c.u32()? as usize;
    if np > 4096 {
        return None;
    }
    let mut params = Vec::with_capacity(np);
    for _ in 0..np {
        params.push(SwitchParam {
            node: c.u32()?,
            first_only: c.u8()? != 0,
            continue_playback: c.u8()? != 0,
            on_switch_mode: c.u32()?,
            fade_out_ms: c.i32()?,
            fade_in_ms: c.i32()?,
        });
    }
    Some(Node::Switch { base, group_type, group, default_switch, continuous, children, groups, params })
}

fn layer(c: &mut Cur) -> Option<Node> {
    let base = node_base(c)?;
    let children = children(c)?;
    let n = c.u32()? as usize;
    if n > 4096 {
        return None;
    }
    let mut layers = Vec::with_capacity(n);
    for _ in 0..n {
        let id = c.u32()?;
        let r = rtpcs(c)?;
        let rtpc_id = c.u32()?;
        let na = c.u32()? as usize;
        if na > 4096 {
            return None;
        }
        let mut assocs = Vec::with_capacity(na);
        for _ in 0..na {
            let child = c.u32()?;
            let np = c.u32()? as usize;
            assocs.push((child, curve_points(c, np)?));
        }
        layers.push(LayerDef { id, rtpcs: r, rtpc_id, assocs });
    }
    Some(Node::Layer { base, children, layers })
}

fn music_node(c: &mut Cur) -> Option<MusicNode> {
    let base = node_base(c)?;
    let children = children(c)?;
    let mut m = MusicNode {
        base,
        children,
        grid_period_ms: c.f64()?,
        grid_offset_ms: c.f64()?,
        tempo: c.f32()?,
        beats_per_bar: c.u8()?,
        beat_value: c.u8()?,
        meter_override: c.u8()? != 0,
        stingers: Vec::new(),
    };
    let ns = c.u32()? as usize;
    if ns > 4096 {
        return None;
    }
    for _ in 0..ns {
        m.stingers.push(Stinger { trigger: c.u32()?, segment: c.u32()?, sync_play_at: c.u32()?, dont_repeat_ms: c.i32()?, look_ahead: c.u32()? });
    }
    Some(m)
}

fn fade(c: &mut Cur) -> Option<Fade> {
    Some(Fade { time_ms: c.i32()?, curve: c.u32()?, offset_ms: c.i32()? })
}

fn trans_rules(c: &mut Cur) -> Option<Vec<TransRule>> {
    let n = c.u32()? as usize;
    if n > 4096 {
        return None;
    }
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        // v65: one source and one destination id per rule (-1 = any).
        let src = vec![c.u32()?];
        let dst = vec![c.u32()?];
        let src_fade = fade(c)?;
        let sync = c.u32()?;
        let _cue_filter = c.u32()?;
        let play_post_exit = c.u8()? != 0;
        let dst_fade = fade(c)?;
        let _cue_filter = c.u32()?;
        let jump_to = c.u32()?;
        let entry_type = c.u16()?;
        let play_pre_entry = c.u8()? != 0;
        let _match_cue_name = c.u8()?;
        let mut r = TransRule { src, dst, src_fade, sync, play_post_exit, dst_fade, jump_to, entry_type, play_pre_entry, transition_segment: None };
        // The transition object is always present; the flag says whether it is used.
        let used = c.u8()? != 0;
        let seg = c.u32()?;
        let fin = fade(c)?;
        let fout = fade(c)?;
        let pre = c.u8()? != 0;
        let post = c.u8()? != 0;
        if used {
            r.transition_segment = Some((seg, fin, fout, pre, post));
        }
        v.push(r);
    }
    Some(v)
}

fn music_playlist(c: &mut Cur, depth: u32) -> Option<MusicPlaylistItem> {
    if depth > 32 {
        return None;
    }
    let segment = c.u32()?;
    let item_id = c.u32()?;
    let nchildren = c.u32()? as usize;
    let rs_type = c.u32()?;
    let loop_count = c.u16()? as i16;
    let weight = c.u32()?;
    let avoid_repeat = c.u16()?;
    let using_weight = c.u8()? != 0;
    let shuffle = c.u8()? != 0;
    if nchildren > 4096 {
        return None;
    }
    let children = (0..nchildren).map(|_| music_playlist(c, depth + 1)).collect::<Option<Vec<_>>>()?;
    Some(MusicPlaylistItem { segment, item_id, rs_type, loop_count, weight, avoid_repeat, using_weight, shuffle, children })
}

fn music_track(c: &mut Cur) -> Option<Node> {
    let n = c.u32()? as usize;
    if n > 4096 {
        return None;
    }
    let sources = (0..n).map(|_| source(c)).collect::<Option<Vec<_>>>()?;
    let np = c.u32()? as usize;
    if np > 4096 {
        return None;
    }
    let mut clips = Vec::with_capacity(np);
    for _ in 0..np {
        clips.push(TrackClip { track: c.u32()?, source: c.u32()?, play_at: c.f64()?, begin_trim: c.f64()?, end_trim: c.f64()?, src_duration: c.f64()? });
    }
    let subtracks = if np > 0 { c.u32()? } else { 0 };
    let na = c.u32()? as usize;
    if na > 4096 {
        return None;
    }
    for _ in 0..na {
        let _clip = c.u32()?;
        let _ty = c.u32()?;
        let npts = c.u32()? as usize;
        curve_points(c, npts)?;
    }
    let base = node_base(c)?;
    let rs_type = c.u32()?;
    let look_ahead_ms = c.i32()?;
    Some(Node::MusicTrack { base, sources, clips, subtracks, rs_type, look_ahead_ms })
}

/// Decode one object; `None` if it does not parse. The caller checks exact length.
pub fn decode_object(o: &HircObject) -> (Option<Node>, usize) {
    let mut c = Cur { b: &o.body, o: 0 };
    let r = match o.kind {
        HIRC_SOUND => (|| {
            let source = source(&mut c)?;
            let base = node_base(&mut c)?;
            Some(Node::Sound { base, source })
        })(),
        HIRC_EVENT => (|| {
            let n = c.u32()? as usize;
            Some(Node::Event { actions: c.ids(n)? })
        })(),
        HIRC_ACTION => action(&mut c).map(Node::Action),
        HIRC_RANSEQ => ranseq(&mut c),
        HIRC_SWITCH => switch(&mut c),
        HIRC_ACTOR_MIXER => (|| {
            let base = node_base(&mut c)?;
            Some(Node::ActorMixer { base, children: children(&mut c)? })
        })(),
        HIRC_LAYER => layer(&mut c),
        HIRC_MUSIC_SEGMENT => (|| {
            let music = music_node(&mut c)?;
            let duration_ms = c.f64()?;
            let n = c.u32()? as usize;
            if n > 4096 {
                return None;
            }
            // Markers: id, position (ms), name (u32 length + bytes; empty in the retail banks).
            let markers = (0..n)
                .map(|_| {
                    let m = (c.u32()?, c.f64()?);
                    let len = c.u32()? as usize;
                    if len > 4096 {
                        return None;
                    }
                    c.o += len;
                    Some(m)
                })
                .collect::<Option<Vec<_>>>()?;
            Some(Node::MusicSegment { music, duration_ms, markers })
        })(),
        HIRC_MUSIC_TRACK => music_track(&mut c),
        HIRC_MUSIC_SWITCH => (|| {
            let music = music_node(&mut c)?;
            let rules = trans_rules(&mut c)?;
            let group_type = c.u32()?;
            let group = c.u32()?;
            let default_switch = c.u32()?;
            let continue_playback = c.u8()? != 0;
            let n = c.u32()? as usize;
            if n > 4096 {
                return None;
            }
            let assocs = (0..n).map(|_| Some((c.u32()?, c.u32()?))).collect::<Option<Vec<_>>>()?;
            Some(Node::MusicSwitch { music, rules, group_type, group, default_switch, continue_playback, assocs })
        })(),
        HIRC_MUSIC_RANSEQ => (|| {
            let music = music_node(&mut c)?;
            let rules = trans_rules(&mut c)?;
            let n = c.u32()? as usize;
            let _ = n;
            let root = music_playlist(&mut c, 0)?;
            Some(Node::MusicRanSeq { music, rules, playlist: vec![root] })
        })(),
        HIRC_ATTENUATION => attenuation(&mut c).map(Node::Attenuation),
        HIRC_STATE => prop_bundle(&mut c).map(|props| Node::State { props }),
        k => {
            c.o = o.body.len();
            Some(Node::Other(k))
        }
    };
    (r, c.o)
}

/// All decoded objects of one bank.
#[derive(Debug, Default, Clone)]
pub struct Hierarchy {
    pub nodes: HashMap<u32, Node>,
    /// Objects that did not decode exactly (kind, id, bytes consumed, body length).
    pub inexact: Vec<(u8, u32, usize, usize)>,
}

impl Hierarchy {
    pub fn from_bank(bank: &Bank) -> Self {
        let mut h = Hierarchy::default();
        for o in &bank.hirc {
            let (n, used) = decode_object(o);
            match n {
                Some(n) if used == o.body.len() => {
                    h.nodes.insert(o.id, n);
                }
                n => {
                    h.inexact.push((o.kind, o.id, used, o.body.len()));
                    h.nodes.insert(o.id, n.unwrap_or(Node::Other(o.kind)));
                }
            }
        }
        h
    }
}
