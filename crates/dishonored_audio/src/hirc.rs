//! HIRC objects of Dishonored's soundbanks (bank version 65), decoded as far as
//! event playback needs: events, actions, sounds, containers, music objects and
//! attenuations. Layouts were measured on the retail banks (research/dishonored/AUDIO_FORMAT.md).

use crate::bank::{Bank, HircObject};
use std::collections::HashMap;

pub const HIRC_STATE: u8 = 1;
pub const HIRC_SOUND: u8 = 2;
pub const HIRC_ACTION: u8 = 3;
pub const HIRC_EVENT: u8 = 4;
pub const HIRC_RANSEQ: u8 = 5;
pub const HIRC_SWITCH: u8 = 6;
pub const HIRC_ACTOR_MIXER: u8 = 7;
pub const HIRC_LAYER: u8 = 9;
pub const HIRC_MUSIC_SEGMENT: u8 = 10;
pub const HIRC_MUSIC_TRACK: u8 = 11;
pub const HIRC_MUSIC_SWITCH: u8 = 12;
pub const HIRC_MUSIC_RANSEQ: u8 = 13;
pub const HIRC_ATTENUATION: u8 = 14;

/// Property ids of the v65 property bundle that playback uses.
pub const PROP_VOLUME: u8 = 0;
pub const PROP_PITCH: u8 = 2;
pub const PROP_LOOP: u8 = 7;

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
}

impl Source {
    pub fn is_vorbis(&self) -> bool {
        self.plugin == 0x0004_0001
    }
}

#[derive(Debug, Clone, Default)]
pub struct NodeBase {
    pub parent: u32,
    pub bus: u32,
    pub props: Vec<(u8, f32)>,
    pub attenuation: Option<u32>,
    /// Offset in the body just after the property bundles.
    pub after_props: usize,
}

impl NodeBase {
    pub fn prop(&self, id: u8) -> Option<f32> {
        self.props.iter().find(|p| p.0 == id).map(|p| p.1)
    }
}

#[derive(Debug, Clone)]
pub struct Action {
    pub action_type: u16,
    pub target: u32,
    /// SetState / SetSwitch: (group id, state or switch id).
    pub set_group: Option<(u32, u32)>,
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
}

/// v65 action: type u16, target u32, property bundle (u8 n, ids, values), ranged
/// bundle (u8 n, ids, min/max), then type-specific parameters.
fn action(c: &mut Cur) -> Option<Action> {
    let action_type = c.u16()?;
    let target = c.u32()?;
    let n = c.u8()? as usize;
    c.o += n * 5;
    let nr = c.u8()? as usize;
    c.o += nr * 9;
    let mut a = Action { action_type, target, set_group: None };
    if a.is_set_state() || a.is_set_switch() {
        a.set_group = Some((c.u32()?, c.u32()?));
    }
    Some(a)
}

#[derive(Debug, Clone, Default)]
pub struct Attenuation {
    /// Distance at the end of the dry volume curve (Wwise units = game units).
    pub max_distance: f32,
    /// (distance, volume dB) points of the dry volume curve.
    pub volume_curve: Vec<(f32, f32)>,
}

#[derive(Debug, Clone)]
pub enum Node {
    Sound { base: NodeBase, source: Source },
    Container { kind: u8, base: Option<NodeBase>, children: Vec<u32>, weights: Vec<(u32, i32)>, switch_groups: Vec<(u32, Vec<u32>)>, default_switch: u32, group_id: u32 },
    MusicTrack { sources: Vec<Source> },
    Event { actions: Vec<u32> },
    Action(Action),
    Attenuation(Attenuation),
    Other(u8),
}

struct Cur<'a> {
    b: &'a [u8],
    o: usize,
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
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
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
    let _bits = c.u8()?;
    Some(Source { plugin, stream, media_id, file_id })
}

/// NodeBaseParams up to and including the positioning block.
fn node_base(c: &mut Cur) -> Option<NodeBase> {
    let _override_fx = c.u8()?;
    let nfx = c.u8()?;
    if nfx > 0 {
        let _bypass = c.u8()?;
        for _ in 0..nfx {
            c.o += 1 + 4 + 2;
        }
    }
    let bus = c.u32()?;
    let parent = c.u32()?;
    let _prio_override = c.u8()?;
    let _prio_dist = c.u8()?;
    let n = c.u8()? as usize;
    let ids: Vec<u8> = (0..n).map(|_| c.u8()).collect::<Option<_>>()?;
    let mut props = Vec::with_capacity(n);
    for id in ids {
        let raw = c.u32()?;
        // Integer-valued props (loop count) are stored as ints; keep both readable.
        let v = if id == PROP_LOOP { raw as i32 as f32 } else { f32::from_bits(raw) };
        props.push((id, v));
    }
    let nr = c.u8()? as usize;
    c.o += nr + nr * 8;
    let after_props = c.o;
    let mut attenuation = None;
    if c.u8()? != 0 {
        // Positioning overridden: 3D flag, then for 3D: type, attenuation, spatialized.
        if c.u8()? != 0 {
            let _ty = c.u32()?;
            let att = c.u32()?;
            if att != 0 {
                attenuation = Some(att);
            }
        }
    }
    Some(NodeBase { parent, bus, props, attenuation, after_props })
}

fn attenuation(c: &mut Cur) -> Option<Attenuation> {
    if c.u8()? != 0 {
        c.o += 16;
    }
    let mut use_curve = [0i8; 4];
    for u in &mut use_curve {
        *u = c.u8()? as i8;
    }
    let ncurves = c.u8()? as usize;
    let mut curves = Vec::new();
    for _ in 0..ncurves {
        let _scaling = c.u8()?;
        let n = c.u16()? as usize;
        let mut pts = Vec::with_capacity(n);
        for _ in 0..n {
            let x = c.f32()?;
            let y = c.f32()?;
            let _interp = c.u32()?;
            pts.push((x, y));
        }
        curves.push(pts);
    }
    let dry = curves.get(use_curve[0].max(0) as usize).cloned().unwrap_or_default();
    Some(Attenuation { max_distance: dry.last().map(|p| p.0).unwrap_or(0.0), volume_curve: dry })
}

/// Find `u32 n` followed by exactly the ids of `want` (any order).
fn find_id_list(b: &[u8], from: usize, want: &[u32]) -> Option<usize> {
    if want.is_empty() {
        return None;
    }
    let n = want.len();
    let mut o = from;
    while o + 4 + 4 * n <= b.len() {
        if u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize == n {
            let ok = (0..n).all(|i| {
                let v = u32::from_le_bytes(b[o + 4 + 4 * i..o + 8 + 4 * i].try_into().unwrap());
                want.contains(&v)
            });
            if ok {
                return Some(o);
            }
        }
        o += 1;
    }
    None
}

/// Find the longest `u32 n` + n ids list whose ids are all in `known`.
fn scan_id_list(b: &[u8], from: usize, self_id: u32, known: &HashMap<u32, u8>) -> Option<(usize, Vec<u32>)> {
    let mut best: Option<(usize, Vec<u32>)> = None;
    let mut o = from;
    while o + 8 <= b.len() {
        let n = u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
        if n >= 1 && n <= 4096 && o + 4 + 4 * n <= b.len() {
            let ids: Vec<u32> = (0..n)
                .map(|i| u32::from_le_bytes(b[o + 4 + 4 * i..o + 8 + 4 * i].try_into().unwrap()))
                .collect();
            if ids.iter().all(|i| *i != self_id && known.contains_key(i)) && best.as_ref().is_none_or(|b| ids.len() > b.1.len()) {
                best = Some((o, ids));
            }
        }
        o += 1;
    }
    best
}

/// Music switch associations: (switch/state id, child node id) pairs that end the
/// object. The count field before them does not match the number of pairs on the
/// retail banks (e.g. 10 declared, 9 present after 12 zero bytes in Bank_Music_Boyle),
/// so the pairs are read backwards from the end while the node is a child.
fn music_switch_assocs(b: &[u8], _from: usize, children: &[u32]) -> Vec<(u32, Vec<u32>)> {
    let mut out = Vec::new();
    let mut end = b.len();
    while end >= 8 {
        let p = end - 8;
        let s = u32::from_le_bytes(b[p..p + 4].try_into().unwrap());
        let c = u32::from_le_bytes(b[p + 4..p + 8].try_into().unwrap());
        if s == 0 || !children.contains(&c) || children.contains(&s) {
            break;
        }
        out.push((s, vec![c]));
        end = p;
    }
    out.reverse();
    out
}

/// All decoded objects of one bank.
#[derive(Debug, Default, Clone)]
pub struct Hierarchy {
    pub nodes: HashMap<u32, Node>,
}

impl Hierarchy {
    pub fn from_bank(bank: &Bank) -> Self {
        let kinds: HashMap<u32, u8> = bank.hirc.iter().map(|o| (o.id, o.kind)).collect();
        let mut nodes = HashMap::new();
        let mut parent_of: Vec<(u32, u32)> = Vec::new();
        for o in &bank.hirc {
            let n = Self::decode(o, &kinds);
            match &n {
                Node::Sound { base, .. } => parent_of.push((o.id, base.parent)),
                Node::Container { base: Some(b), .. } => parent_of.push((o.id, b.parent)),
                _ => {}
            }
            nodes.insert(o.id, n);
        }
        // Children of sound-hierarchy containers are exactly the nodes naming them as parent.
        let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
        for (c, p) in parent_of {
            kids.entry(p).or_default().push(c);
        }
        let raw: HashMap<u32, &HircObject> = bank.hirc.iter().map(|o| (o.id, o)).collect();
        for (id, node) in nodes.iter_mut() {
            if let Node::Container { kind, base: Some(base), children, weights, switch_groups, default_switch, group_id } = node {
                let Some(k) = kids.get(id) else { continue };
                let body = &raw[id].body;
                let Some(at) = find_id_list(body, base.after_props, k) else {
                    *children = k.clone();
                    continue;
                };
                let n = k.len();
                children.extend((0..n).map(|i| u32::from_le_bytes(body[at + 4 + 4 * i..at + 8 + 4 * i].try_into().unwrap())));
                let mut c = Cur { b: body, o: at + 4 + 4 * n };
                match *kind {
                    HIRC_RANSEQ => {
                        if let Some(np) = c.u16() {
                            for _ in 0..np {
                                if let (Some(i), Some(w)) = (c.u32(), c.u32()) {
                                    weights.push((i, w as i32));
                                }
                            }
                        }
                    }
                    HIRC_SWITCH => {
                        // Group type/id/default switch precede the children list.
                        if at >= 9 {
                            *default_switch = u32::from_le_bytes(body[at - 5..at - 1].try_into().unwrap());
                            *group_id = u32::from_le_bytes(body[at - 9..at - 5].try_into().unwrap());
                        }
                        if let Some(ng) = c.u32() {
                            for _ in 0..ng.min(1024) {
                                let (Some(sw), Some(ni)) = (c.u32(), c.u32()) else { break };
                                let items: Vec<u32> = (0..ni.min(4096)).filter_map(|_| c.u32()).collect();
                                switch_groups.push((sw, items));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Music containers: children lists found by scanning (their node base is not decoded).
        for o in &bank.hirc {
            if matches!(o.kind, HIRC_MUSIC_SEGMENT | HIRC_MUSIC_SWITCH | HIRC_MUSIC_RANSEQ) {
                if let Some((at, ids)) = scan_id_list(&o.body, 0, o.id, &kinds) {
                    let assocs = if o.kind == HIRC_MUSIC_SWITCH { music_switch_assocs(&o.body, at + 4 + 4 * ids.len(), &ids) } else { Vec::new() };
                    if let Some(Node::Container { children, switch_groups, .. }) = nodes.get_mut(&o.id) {
                        *children = ids;
                        *switch_groups = assocs;
                    }
                }
            }
        }
        Self { nodes }
    }

    fn decode(o: &HircObject, _kinds: &HashMap<u32, u8>) -> Node {
        let mut c = Cur { b: &o.body, o: 0 };
        let r = match o.kind {
            HIRC_SOUND => (|| {
                let source = source(&mut c)?;
                let base = node_base(&mut c)?;
                Some(Node::Sound { base, source })
            })(),
            HIRC_EVENT => (|| {
                let n = c.u32()?;
                Some(Node::Event { actions: (0..n.min(1024)).filter_map(|_| c.u32()).collect() })
            })(),
            HIRC_ACTION => action(&mut c).map(Node::Action),
            HIRC_RANSEQ | HIRC_SWITCH | HIRC_ACTOR_MIXER | HIRC_LAYER => Some(Node::Container {
                kind: o.kind,
                base: node_base(&mut c),
                children: Vec::new(),
                weights: Vec::new(),
                switch_groups: Vec::new(),
                default_switch: 0,
                group_id: 0,
            }),
            HIRC_MUSIC_SEGMENT | HIRC_MUSIC_SWITCH | HIRC_MUSIC_RANSEQ => Some(Node::Container {
                kind: o.kind,
                base: None,
                children: Vec::new(),
                weights: Vec::new(),
                switch_groups: Vec::new(),
                default_switch: 0,
                group_id: 0,
            }),
            HIRC_MUSIC_TRACK => (|| {
                let n = c.u32()?;
                let sources = (0..n.min(256)).map(|_| source(&mut c)).collect::<Option<Vec<_>>>()?;
                Some(Node::MusicTrack { sources })
            })(),
            HIRC_ATTENUATION => attenuation(&mut c).map(Node::Attenuation),
            k => Some(Node::Other(k)),
        };
        r.unwrap_or(Node::Other(o.kind))
    }
}
