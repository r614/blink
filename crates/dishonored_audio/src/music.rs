//! Interactive music (bank v65): music switch containers on states, playlist containers
//! (sequence/random, continuous/step, loop counts, weights, avoid-repeat, shuffle), segments
//! timed by their entry/exit cues with pre-entry and post-exit overlap, tracks and clips
//! (play-at, trims; random/sequence sub-tracks), and transition rules (source/destination
//! fades with offsets, synchronisation on immediate / grid / bar / beat / cue / exit cue,
//! transition segments). Scheduling is sample accurate on the engine clock.

use crate::engine::{Engine, ObjId};
use crate::hirc::{Fade, MusicPlaylistItem, Node, TransRule};
use std::collections::HashSet;

const ANY: u32 = u32::MAX;

#[derive(Debug, Clone)]
struct Frame {
    item: MusicPlaylistItem,
    /// Leaf: plays left (0 = infinite). Group: loops left (0 = infinite).
    left: i32,
    /// Group: position in the pass / picks left in the pass.
    pos: usize,
    picks_left: usize,
    history: Vec<usize>,
    shuffle_left: Vec<usize>,
    stepped: bool,
}

impl Frame {
    fn new(item: MusicPlaylistItem) -> Self {
        let left = item.loop_count as i32;
        let n = item.children.len();
        Frame { left, pos: 0, picks_left: n, history: Vec::new(), shuffle_left: (0..n).collect(), stepped: false, item }
    }
}

/// Segment sequence of a playlist container (or a lone segment).
#[derive(Debug, Clone)]
enum Seq {
    Once(Option<u32>),
    Playlist(Vec<Frame>),
}

impl Seq {
    fn next(&mut self, eng: &mut Engine) -> Option<u32> {
        match self {
            Seq::Once(s) => s.take(),
            Seq::Playlist(stack) => {
                for _ in 0..4096 {
                    let top = stack.last_mut()?;
                    if top.item.children.is_empty() {
                        // Leaf: its segment, `loop_count` times (0 = forever).
                        let seg = top.item.segment;
                        if top.left == 0 {
                            return (seg != 0).then_some(seg);
                        }
                        if top.left > 0 {
                            top.left -= 1;
                            let done = top.left == 0;
                            if done {
                                stack.pop();
                            }
                            if seg != 0 {
                                return Some(seg);
                            }
                            continue;
                        }
                        stack.pop();
                        continue;
                    }
                    let n = top.item.children.len();
                    let step = matches!(top.item.rs_type, 1 | 3);
                    let random = matches!(top.item.rs_type, 2 | 3);
                    let pass_over = if step { top.stepped } else if random { top.picks_left == 0 } else { top.pos >= n };
                    if pass_over {
                        // End of a pass: loop again or finish the group.
                        if top.left == 1 {
                            stack.pop();
                            continue;
                        }
                        if top.left > 1 {
                            top.left -= 1;
                        }
                        top.pos = 0;
                        top.picks_left = n;
                        top.stepped = false;
                        top.shuffle_left = (0..n).collect();
                        if step {
                            // A step container plays one item per pass.
                            stack.pop();
                            continue;
                        }
                    }
                    let ci = if random {
                        let avoid = (top.item.avoid_repeat as usize).min(n.saturating_sub(1));
                        let recent: Vec<usize> = top.history.iter().rev().take(avoid).copied().collect();
                        let pool: Vec<usize> = if top.item.shuffle { top.shuffle_left.clone() } else { (0..n).collect() };
                        let mut cands: Vec<usize> = pool.iter().copied().filter(|i| !recent.contains(i)).collect();
                        if cands.is_empty() {
                            cands = pool;
                        }
                        let w: Vec<f32> = cands.iter().map(|i| if top.item.using_weight { top.item.children[*i].weight as f32 } else { 1.0 }).collect();
                        let total: f32 = w.iter().sum();
                        let mut x = eng.rand01() * total.max(1e-6);
                        let mut pick = *cands.last().unwrap_or(&0);
                        for (k, wk) in w.iter().enumerate() {
                            if x < *wk {
                                pick = cands[k];
                                break;
                            }
                            x -= wk;
                        }
                        top.history.push(pick);
                        top.shuffle_left.retain(|i| *i != pick);
                        top.picks_left = top.picks_left.saturating_sub(1);
                        pick
                    } else {
                        let i = top.pos.min(n - 1);
                        top.pos += 1;
                        i
                    };
                    top.stepped = true;
                    let child = top.item.children[ci].clone();
                    stack.push(Frame::new(child));
                }
                None
            }
        }
    }
}

#[derive(Debug, Clone)]
struct SegPlay {
    seg: u32,
    entry: u64,
    exit: u64,
    voices: Vec<u32>,
    spawned: bool,
    fade_in: Option<Fade>,
}

#[derive(Debug, Clone)]
pub struct MusicInst {
    pub root: u32,
    pub obj: ObjId,
    pub playing: u32,
    /// Leaf source (playlist container or segment) currently playing.
    leaf: u32,
    seq: Seq,
    cur: Option<SegPlay>,
    queued: Option<SegPlay>,
    stop_at: Option<u64>,
    finished: bool,
    nodes: HashSet<u32>,
    start_fade_ms: f32,
    /// Segment ids played (in order) for tests and the harness.
    pub history: Vec<(u64, u32)>,
}

fn seg_cues(eng: &Engine, seg: u32) -> (f64, f64, f64) {
    match eng.lib.node(seg) {
        Some((_, Node::MusicSegment { duration_ms, markers, .. })) => {
            let entry = markers.iter().map(|m| m.1).fold(f64::MAX, f64::min);
            let exit = markers.iter().map(|m| m.1).fold(f64::MIN, f64::max);
            if markers.is_empty() { (0.0, *duration_ms, *duration_ms) } else { (entry.min(*duration_ms), exit, *duration_ms) }
        }
        _ => (0.0, 0.0, 0.0),
    }
}

fn ms(eng: &Engine, t: f64) -> i64 {
    (t * eng.rate as f64 / 1000.0).round() as i64
}

impl MusicInst {
    pub(crate) fn new(eng: &mut Engine, root: u32, obj: ObjId, playing: u32, _at: u64, fade_ms: f32) -> Self {
        let mut m = MusicInst {
            root,
            obj,
            playing,
            leaf: 0,
            seq: Seq::Once(None),
            cur: None,
            queued: None,
            stop_at: None,
            finished: false,
            nodes: HashSet::new(),
            start_fade_ms: fade_ms,
            history: Vec::new(),
        };
        m.nodes.insert(root);
        let leaf = m.resolve_leaf(eng, root, 0);
        m.set_leaf(eng, leaf);
        m
    }

    pub fn finished(&self) -> bool {
        self.finished
    }

    pub fn contains_node(&self, id: u32) -> bool {
        self.nodes.contains(&id)
    }

    pub fn current_segment(&self) -> Option<u32> {
        self.cur.as_ref().map(|c| c.seg)
    }

    pub fn leaf(&self) -> u32 {
        self.leaf
    }

    /// Follow music switches down to the playlist/segment selected by the current states.
    fn resolve_leaf(&mut self, eng: &Engine, id: u32, depth: u32) -> u32 {
        if depth > 8 {
            return 0;
        }
        self.nodes.insert(id);
        match eng.lib.node(id) {
            Some((_, Node::MusicSwitch { group_type, group, default_switch, assocs, .. })) => {
                let v = if *group_type == 1 { eng.states.get(group).copied().unwrap_or(*default_switch) } else { *default_switch };
                let child = assocs.iter().find(|a| a.0 == v).or_else(|| assocs.iter().find(|a| a.0 == *default_switch)).map(|a| a.1).unwrap_or(0);
                if child == 0 { 0 } else { self.resolve_leaf(eng, child, depth + 1) }
            }
            Some((_, Node::MusicRanSeq { .. })) | Some((_, Node::MusicSegment { .. })) => id,
            _ => 0,
        }
    }

    fn set_leaf(&mut self, eng: &Engine, leaf: u32) {
        self.leaf = leaf;
        self.nodes.insert(leaf);
        self.seq = match eng.lib.node(leaf) {
            Some((_, Node::MusicRanSeq { playlist, .. })) => Seq::Playlist(playlist.iter().cloned().map(Frame::new).collect()),
            Some((_, Node::MusicSegment { .. })) => Seq::Once(Some(leaf)),
            _ => Seq::Once(None),
        };
    }

    pub fn stop(&mut self, clock: u64, fade_len: u64) {
        self.stop_at = Some(clock + fade_len);
        self.fade_out_all(clock, fade_len);
    }

    fn fade_out_all(&mut self, clock: u64, len: u64) {
        let _ = (clock, len);
    }

    /// Spawn the voices of a segment whose entry cue falls at `seg.entry`.
    fn spawn(&mut self, eng: &mut Engine, sp: &mut SegPlay) {
        sp.spawned = true;
        eng.stats.music_segments += 1;
        self.history.push((sp.entry, sp.seg));
        self.nodes.insert(sp.seg);
        let (entry_ms, _, _) = seg_cues(eng, sp.seg);
        let Some((_, Node::MusicSegment { music, .. })) = eng.lib.node(sp.seg) else { return };
        let tracks = music.children.clone();
        for t in tracks {
            let Some((bank, Node::MusicTrack { sources, clips, subtracks, rs_type, .. })) = eng.lib.node(t) else { continue };
            let (sources, clips, subtracks, rs_type) = (sources.clone(), clips.clone(), *subtracks, *rs_type);
            // Random (1) / sequence (2) tracks play one sub-track per segment play.
            let sub = if subtracks > 1 && rs_type == 1 {
                Some(((eng.rand01() * subtracks as f32) as u32).min(subtracks - 1))
            } else if subtracks > 1 && rs_type == 2 {
                let k = self.history.iter().filter(|h| h.1 == sp.seg).count() as u32;
                Some((k.saturating_sub(1)) % subtracks)
            } else {
                None
            };
            for c in clips.iter().filter(|c| sub.is_none_or(|s| c.track == s)) {
                let Some(src) = sources.iter().find(|s| s.media_id == c.source).or(sources.first()) else { continue };
                let Some(media) = eng.lib.media_key(bank, src) else { continue };
                let Ok((_, srate, _)) = eng.lib.media_info(media) else { continue };
                let start_ms = c.play_at + c.begin_trim;
                let end_ms = c.play_at + c.src_duration + c.end_trim;
                let mut start = sp.entry as i64 + ms(eng, start_ms - entry_ms);
                let end = sp.entry as i64 + ms(eng, end_ms - entry_ms);
                let mut src_begin = c.begin_trim * srate as f64 / 1000.0;
                let now = eng.clock as i64;
                if end <= now {
                    continue;
                }
                if start < now {
                    // Started part-way (first segment from its entry cue, late spawn).
                    src_begin += (now - start) as f64 * srate as f64 / eng.rate as f64;
                    start = now;
                }
                let src_end = (c.src_duration + c.end_trim.min(0.0)) * srate as f64 / 1000.0;
                let fade = sp.fade_in.map(|f| f.time_ms as f32).unwrap_or(0.0);
                let Some(id) = eng.start_voice(t, media, self.obj, self.playing, &[self.root, self.leaf, sp.seg], fade, sp.fade_in.map(|f| f.curve).unwrap_or(4), start as u64, vec![src_begin, src_end]) else { continue };
                let fade_at = sp.fade_in.map(|f| (sp.entry as i64 + ms(eng, f.offset_ms as f64)).max(start) as u64);
                if let Some(v) = eng.voice_mut(id) {
                    v.loops_left = if c.end_trim > 0.0 { 0 } else { 1 };
                    v.stop_at = Some(end as u64);
                    if let Some(fa) = fade_at {
                        if let Some(r) = v.fade.as_mut() {
                            r.start = fa;
                        }
                    }
                }
                sp.voices.push(id);
            }
        }
    }

    pub(crate) fn update(&mut self, eng: &mut Engine, block_end: u64) {
        if self.finished {
            return;
        }
        if let Some(s) = self.stop_at {
            if eng.clock >= s {
                for sp in self.cur.iter().chain(self.queued.iter()) {
                    for id in &sp.voices {
                        if let Some(v) = eng.voice_mut(*id) {
                            v.done = true;
                        }
                    }
                }
                self.finished = true;
            }
            return;
        }
        let lookahead = eng.rate as u64 / 2;
        if self.cur.is_none() && self.queued.is_none() {
            let Some(seg) = self.seq.next(eng) else {
                self.finished = true;
                return;
            };
            let (entry, exit, _) = seg_cues(eng, seg);
            let start = eng.clock;
            let fade = (self.start_fade_ms > 0.0).then_some(Fade { time_ms: self.start_fade_ms as i32, curve: 4, offset_ms: 0 });
            let mut sp = SegPlay { seg, entry: start, exit: start + ms(eng, exit - entry).max(1) as u64, voices: Vec::new(), spawned: false, fade_in: fade };
            self.spawn(eng, &mut sp);
            self.cur = Some(sp);
        }
        // Queue the next segment at the current one's exit cue.
        if self.queued.is_none() {
            if let Some(cur) = &self.cur {
                let exit = cur.exit;
                if let Some(seg) = self.seq.next(eng) {
                    let (entry, ex, _) = seg_cues(eng, seg);
                    self.queued = Some(SegPlay { seg, entry: exit, exit: exit + ms(eng, ex - entry).max(1) as u64, voices: Vec::new(), spawned: false, fade_in: None });
                }
            }
        }
        if let Some(mut q) = self.queued.take() {
            let pre = self.pre_entry_frames(eng, q.seg);
            if !q.spawned && block_end + lookahead + pre >= q.entry {
                self.spawn(eng, &mut q);
            }
            if eng.clock >= q.entry {
                self.cur = Some(q);
            } else {
                self.queued = Some(q);
            }
        }
        // Nothing queued and the last segment ended: done.
        if self.queued.is_none() {
            if let Some(cur) = &self.cur {
                let alive = cur.voices.iter().any(|id| eng.voice(*id).is_some_and(|v| !v.done));
                if !alive && eng.clock >= cur.exit {
                    self.finished = true;
                }
            }
        }
    }

    fn pre_entry_frames(&self, eng: &Engine, seg: u32) -> u64 {
        let (entry, _, _) = seg_cues(eng, seg);
        let Some((_, Node::MusicSegment { music, .. })) = eng.lib.node(seg) else { return 0 };
        let mut earliest = entry;
        for t in &music.children {
            if let Some((_, Node::MusicTrack { clips, .. })) = eng.lib.node(*t) {
                for c in clips {
                    earliest = earliest.min(c.play_at + c.begin_trim);
                }
            }
        }
        ms(eng, entry - earliest).max(0) as u64
    }

    /// A state changed: if the music switch now selects another playlist, transition.
    pub(crate) fn on_group_change(&mut self, eng: &mut Engine, group: u32, obj: Option<ObjId>) {
        if self.finished || self.stop_at.is_some() || obj.is_some_and(|o| o != self.obj) {
            return;
        }
        let Some((_, Node::MusicSwitch { group: g, rules, .. })) = eng.lib.node(self.root) else { return };
        if *g != group {
            return;
        }
        let rules = rules.clone();
        let new_leaf = self.resolve_leaf(eng, self.root, 0);
        if new_leaf == self.leaf {
            return;
        }
        eng.stats.music_transitions += 1;
        let cur_seg = self.cur.as_ref().map(|c| c.seg).unwrap_or(0);
        let rule = pick_rule(&rules, self.leaf, cur_seg, new_leaf);
        let now = eng.clock;
        let t = match &self.cur {
            Some(cur) => sync_point(eng, cur, rule.as_ref().map(|r| r.sync).unwrap_or(0), now),
            None => now,
        };
        // Source: fade out from the sync point (+ offset); stop at the exit cue unless the
        // post-exit plays.
        let (sf, post_exit) = rule.as_ref().map(|r| (r.src_fade, r.play_post_exit)).unwrap_or((Fade { time_ms: 0, curve: 4, offset_ms: 0 }, false));
        let fade_start = (t as i64 + ms(eng, sf.offset_ms as f64)).max(now as i64) as u64;
        let fade_len = ms(eng, sf.time_ms.max(0) as f64) as u64;
        for sp in self.cur.iter().chain(self.queued.iter()) {
            let queued_only = Some(sp.seg) != self.cur.as_ref().map(|c| c.seg) || sp.entry > t;
            for id in &sp.voices {
                if let Some(v) = eng.voice_mut(*id) {
                    if queued_only && v.start_at >= t {
                        v.done = true;
                        continue;
                    }
                    if fade_len > 0 {
                        v.fade = Some(crate::engine::ramp(fade_start, fade_len, 1.0, 0.0, sf.curve));
                        v.stop_on_fade = true;
                    } else {
                        v.stop_at = Some(v.stop_at.map_or(t, |s| s.min(t)));
                    }
                    if !post_exit && fade_len > 0 {
                        let s = fade_start + fade_len;
                        v.stop_at = Some(v.stop_at.map_or(s, |x| x.min(s)));
                    }
                }
            }
        }
        self.queued = None;
        self.cur = None;
        self.set_leaf(eng, new_leaf);
        if new_leaf == 0 {
            // Silence for this state.
            return;
        }
        let df = rule.as_ref().map(|r| r.dst_fade).unwrap_or(Fade { time_ms: 0, curve: 4, offset_ms: 0 });
        let mut entry = t;
        if let Some((tseg, fin, _fout, _pre, _post)) = rule.as_ref().and_then(|r| r.transition_segment) {
            if tseg != 0 && eng.lib.node(tseg).is_some() {
                let (e, x, _) = seg_cues(eng, tseg);
                let mut sp = SegPlay { seg: tseg, entry: t, exit: t + ms(eng, x - e).max(1) as u64, voices: Vec::new(), spawned: false, fade_in: (fin.time_ms > 0).then_some(fin) };
                self.spawn(eng, &mut sp);
                entry = sp.exit;
                self.cur = Some(sp);
            }
        }
        let Some(seg) = self.seq.next(eng) else { return };
        let (e, x, _) = seg_cues(eng, seg);
        let sp = SegPlay { seg, entry, exit: entry + ms(eng, x - e).max(1) as u64, voices: Vec::new(), spawned: false, fade_in: (df.time_ms > 0).then_some(df) };
        if self.cur.is_none() {
            let mut sp = sp;
            if entry <= now + eng.rate as u64 / 2 {
                self.spawn(eng, &mut sp);
            }
            if entry <= now {
                self.cur = Some(sp);
            } else {
                // Keep the old segment's timing as the reference until the new one enters.
                self.queued = Some(sp);
                self.cur = None;
                self.cur_placeholder(t);
            }
        } else {
            self.queued = Some(sp);
        }
    }

    fn cur_placeholder(&mut self, t: u64) {
        // While waiting for the sync point there is no current segment: use an empty one
        // that exits at the sync point so `update` does not start another sequence.
        self.cur = Some(SegPlay { seg: 0, entry: t, exit: t, voices: Vec::new(), spawned: true, fade_in: None });
    }
}

/// The most specific rule for (source leaf / segment -> destination): exact source and
/// destination, then exact source, then exact destination, then any -> any; among equals
/// the last one listed wins.
fn pick_rule(rules: &[TransRule], src_leaf: u32, src_seg: u32, dst: u32) -> Option<TransRule> {
    let mut best: Option<(u8, &TransRule)> = None;
    for r in rules {
        let s = r.src.first().copied().unwrap_or(ANY);
        let d = r.dst.first().copied().unwrap_or(ANY);
        let sm = s == ANY || s == src_leaf || s == src_seg;
        let dm = d == ANY || d == dst;
        if !(sm && dm) {
            continue;
        }
        let score = ((s != ANY) as u8) * 2 + (d != ANY) as u8;
        if best.is_none_or(|b| score >= b.0) {
            best = Some((score, r));
        }
    }
    best.map(|b| b.1.clone())
}

/// Next synchronisation point after `now` in the playing segment (AkSyncType).
fn sync_point(eng: &Engine, cur: &SegPlay, sync: u32, now: u64) -> u64 {
    let Some((_, Node::MusicSegment { music, markers, .. })) = eng.lib.node(cur.seg) else { return now };
    let (entry_ms, _, _) = seg_cues(eng, cur.seg);
    let beat_ms = 60_000.0 / music.tempo.max(1.0) as f64 * 4.0 / music.beat_value.max(1) as f64;
    let unit_ms = match sync {
        1 => music.grid_period_ms.max(1.0),
        2 => beat_ms * music.beats_per_bar.max(1) as f64,
        3 => beat_ms,
        _ => 0.0,
    };
    match sync {
        1..=3 => {
            let unit = ms(eng, unit_ms).max(1) as u64;
            let origin = cur.entry + ms(eng, music.grid_offset_ms.max(0.0) * (sync == 1) as u8 as f64) as u64;
            if now <= origin {
                return origin;
            }
            let k = (now - origin).div_ceil(unit);
            (origin + k * unit).min(cur.exit.max(now))
        }
        4 | 5 => markers
            .iter()
            .map(|m| (cur.entry as i64 + ms(eng, m.1 - entry_ms)) as u64)
            .filter(|t| *t >= now)
            .min()
            .unwrap_or(cur.exit.max(now)),
        6 | 7 => cur.exit.max(now),
        _ => now,
    }
}
