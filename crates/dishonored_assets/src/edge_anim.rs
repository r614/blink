//! Sony Edge animation clips as stored in Dishonored `AnimSequence.CompressedByteStream`
//! (`ACF_EdgeAnim`, all 113,232 sequences in the install).
//!
//! Reverse engineered from Dishonored.exe (SHA-256 70da9613…c6fb); see
//! research/dishonored/animation/EDGE_ANIM.md for addresses and evidence:
//! - blend-tree evaluator 0x009DBC10 checks tag 'EA05' (0x45413035)
//! - frame-set decompressor 0x009DADA0
//! - constant-channel decoder 0x009CD3A0 (fully reproduced here)
//! - animated-channel decoder 0x009CE380 (in progress)
//!
//! All header integers are little-endian (converted at cook time); the channel bit
//! streams stay big-endian, MSB first, as on the PS3 SPU.

use anyhow::{Result, ensure};
use glam::{Quat, Vec3};

pub const TAG: u32 = 0x4541_3035; // "50AE" in memory

#[derive(Debug, Clone)]
pub struct EdgeHeader {
    pub duration: f32,
    pub sample_frequency: f32,
    pub size_header: u16,
    pub num_joints: u16,
    pub num_frames: u16,
    pub num_frame_sets: u16,
    pub unk14: u16,
    pub num_const_r: u16,
    pub num_const_t: u16,
    pub num_const_s: u16,
    pub num_const_user: u16,
    pub num_anim_r: u16,
    pub num_anim_t: u16,
    pub num_anim_s: u16,
    pub num_anim_user: u16,
    /// bit0/1/2: rotation/translation/scale channels use generic bit-packing specs.
    pub flags: u16,
    /// Self-relative offsets resolved to blob offsets (0 = absent).
    pub frame_set_dma: usize,
    pub frame_set_info: usize,
    pub const_r_data: usize,
    pub const_t_data: usize,
    pub const_s_data: usize,
    pub const_user_data: usize,
    pub packing_specs: usize,
}

/// Decoded fields of a 32-bit Edge packing spec.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PackingSpec {
    pub raw: u32,
    /// (sign bits 0/1, exponent bits, mantissa bits) for X, Y, Z.
    pub comp: [(u32, u32, u32); 3],
    /// Quaternion slot (0..3) rebuilt from the others.
    pub recompose: u32,
}

impl PackingSpec {
    pub fn new(raw: u32) -> Self {
        let f = |shift: u32, bits: u32| (raw >> shift) & ((1 << bits) - 1);
        // Field positions from the shifts at 0x009CD437..0x009CD549.
        PackingSpec {
            raw,
            comp: [(f(31, 1), f(27, 4), f(22, 5)), (f(21, 1), f(17, 4), f(12, 5)), (f(11, 1), f(7, 4), f(2, 5))],
            recompose: f(0, 2),
        }
    }
    pub fn bits(&self) -> u32 {
        self.comp.iter().map(|(s, e, m)| s + e + m).sum()
    }
}

#[derive(Debug, Clone)]
pub struct EdgeClip {
    pub header: EdgeHeader,
    pub const_r_joints: Vec<u16>,
    pub const_t_joints: Vec<u16>,
    pub const_s_joints: Vec<u16>,
    pub anim_r_joints: Vec<u16>,
    pub anim_t_joints: Vec<u16>,
    pub anim_s_joints: Vec<u16>,
    pub specs: Vec<PackingSpec>,
    pub blob: Vec<u8>,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn rel(b: &[u8], o: usize) -> usize {
    let v = u32le(b, o) as usize;
    if v == 0 { 0 } else { o + v }
}

/// MSB-first reader over a big-endian bit stream.
pub struct BitReader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }
    pub fn read(&mut self, bits: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..bits {
            let byte = self.data.get(self.pos >> 3).copied().unwrap_or(0);
            let bit = (byte >> (7 - (self.pos & 7))) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        v
    }
}

/// One component: zero exponent bits -> signed fixed point normalised by 2^m - 1;
/// otherwise a small float re-biased into IEEE single precision.
pub fn decode_component(sign: u32, e_bits: u32, m_bits: u32, s: u32, e: u32, m: u32) -> f32 {
    if e_bits == 0 {
        let denom = ((1u64 << m_bits) - 1).max(1) as f32;
        let v = m as i64 - if sign != 0 && s != 0 { 1i64 << m_bits } else { 0 };
        v as f32 / denom
    } else {
        let bias = 128 - (1u32 << (e_bits - 1));
        let bits = (s << 31) | ((e + bias) << 23) | (m << (23 - m_bits));
        f32::from_bits(bits)
    }
}

pub fn read_value(br: &mut BitReader, spec: &PackingSpec) -> [f32; 3] {
    let mut out = [0f32; 3];
    for (i, &(sb, eb, mb)) in spec.comp.iter().enumerate() {
        let s = br.read(sb);
        let e = br.read(eb);
        let m = br.read(mb);
        out[i] = decode_component(sb, eb, mb, s, e, m);
    }
    out
}

/// Smallest-three reconstruction (0x009CE235): the missing slot is
/// sqrt(clamp(1 - x^2 - y^2 - z^2)), placed at `recompose`; result order x,y,z,w.
pub fn recompose_quat(v: [f32; 3], slot: u32) -> Quat {
    let w = (1.0 - v[0] * v[0] - v[1] * v[1] - v[2] * v[2]).clamp(0.0, 1.0).sqrt();
    let q = match slot {
        0 => [w, v[0], v[1], v[2]],
        1 => [v[0], w, v[1], v[2]],
        2 => [v[0], v[1], w, v[2]],
        _ => [v[0], v[1], v[2], w],
    };
    Quat::from_xyzw(q[0], q[1], q[2], q[3])
}

impl EdgeClip {
    pub fn parse(blob: &[u8]) -> Result<Self> {
        ensure!(blob.len() >= 0x60 && u32le(blob, 0) == TAG, "not an Edge animation (tag {:#x})", u32le(blob, 0));
        let h = EdgeHeader {
            duration: f32::from_le_bytes(blob[4..8].try_into()?),
            sample_frequency: f32::from_le_bytes(blob[8..12].try_into()?),
            size_header: u16le(blob, 0x0c),
            num_joints: u16le(blob, 0x0e),
            num_frames: u16le(blob, 0x10),
            num_frame_sets: u16le(blob, 0x12),
            unk14: u16le(blob, 0x14),
            num_const_r: u16le(blob, 0x16),
            num_const_t: u16le(blob, 0x18),
            num_const_s: u16le(blob, 0x1a),
            num_const_user: u16le(blob, 0x1c),
            num_anim_r: u16le(blob, 0x1e),
            num_anim_t: u16le(blob, 0x20),
            num_anim_s: u16le(blob, 0x22),
            num_anim_user: u16le(blob, 0x24),
            flags: u16le(blob, 0x26),
            frame_set_dma: rel(blob, 0x38),
            frame_set_info: rel(blob, 0x3c),
            const_r_data: rel(blob, 0x40),
            const_t_data: rel(blob, 0x44),
            const_s_data: rel(blob, 0x48),
            const_user_data: rel(blob, 0x4c),
            packing_specs: rel(blob, 0x50),
        };
        // Joint tables at +0x60: first rounded to 8 entries, the rest to 4 (0x009DAF3A).
        let mut at = 0x60;
        let mut table = |n: u16, round: usize| -> Vec<u16> {
            let padded = (n as usize).div_ceil(round) * round;
            let v: Vec<u16> = (0..n as usize).map(|i| u16le(blob, at + i * 2)).collect();
            at += padded * 2;
            v
        };
        let const_r_joints = table(h.num_const_r, 8);
        let const_t_joints = table(h.num_const_t, 4);
        let const_s_joints = table(h.num_const_s, 4);
        let _const_u = table(h.num_const_user, 4);
        let anim_r_joints = table(h.num_anim_r, 4);
        let anim_t_joints = table(h.num_anim_t, 4);
        let anim_s_joints = table(h.num_anim_s, 4);
        let mut specs = Vec::new();
        if h.packing_specs != 0 {
            let mut o = h.packing_specs;
            while o + 4 <= blob.len() && specs.len() < 4096 {
                specs.push(PackingSpec::new(u32le(blob, o)));
                o += 4;
                // Specs: [constR, animR...] [constT, animT...] [constS, animS...] for generic types.
                let expected = [(1, h.num_anim_r), (2, h.num_anim_t), (4, h.num_anim_s)]
                    .iter()
                    .filter(|(bit, _)| h.flags & bit != 0)
                    .map(|(_, n)| 1 + *n as usize)
                    .sum::<usize>();
                if specs.len() >= expected {
                    break;
                }
            }
        }
        Ok(EdgeClip { header: h, const_r_joints, const_t_joints, const_s_joints, anim_r_joints, anim_t_joints, anim_s_joints, specs, blob: blob.to_vec() })
    }

    /// Constant rotation channels (generic path), as 0x009CD3A0 decodes them.
    pub fn const_rotations(&self) -> Vec<(u16, Quat)> {
        if self.header.flags & 1 == 0 || self.header.const_r_data == 0 || self.specs.is_empty() {
            return Vec::new();
        }
        let spec = self.specs[0];
        let mut br = BitReader::new(&self.blob, self.header.const_r_data * 8);
        self.const_r_joints.iter().map(|&j| (j, recompose_quat(read_value(&mut br, &spec), spec.recompose))).collect()
    }

    /// Constant translation channels (generic path).
    pub fn const_translations(&self) -> Vec<(u16, Vec3)> {
        if self.header.flags & 2 == 0 || self.header.const_t_data == 0 {
            return Vec::new();
        }
        let idx = if self.header.flags & 1 != 0 { 1 + self.header.num_anim_r as usize } else { 0 };
        let Some(spec) = self.specs.get(idx).copied() else { return Vec::new() };
        let mut br = BitReader::new(&self.blob, self.header.const_t_data * 8);
        self.const_t_joints.iter().map(|&j| (j, Vec3::from(read_value(&mut br, &spec)))).collect()
    }
}

/// Frame-set layout (from 0x009DBF68 and 0x009DAE3A).
#[derive(Debug, Clone)]
pub struct FrameSetLayout {
    pub index: usize,
    pub start_frame: u16,
    pub blob_offset: usize,
    pub size: usize,
    /// Section sizes from the frame-set header (8 x u16).
    pub sizes: [u16; 8],
    /// Second block sizes (3 x u16) when present.
    pub sizes2: [u16; 3],
}

impl EdgeClip {
    pub fn frame_sets(&self) -> Vec<FrameSetLayout> {
        let h = &self.header;
        let b = &self.blob;
        let mut out = Vec::new();
        if h.frame_set_dma == 0 {
            return out;
        }
        for i in 0..h.num_frame_sets as usize {
            let e = h.frame_set_dma + i * 8;
            if e + 8 > b.len() {
                break;
            }
            let size = u16le(b, e + 2) as usize;
            let off = u32le(b, e + 4) as usize;
            let start_frame = if h.frame_set_info != 0 { u16le(b, h.frame_set_info + i * 4) } else { 0 };
            let mut sizes = [0u16; 8];
            let mut sizes2 = [0u16; 3];
            if off + 16 <= b.len() {
                for k in 0..8 {
                    sizes[k] = u16le(b, off + k * 2);
                }
            }
            let _ = &mut sizes2;
            out.push(FrameSetLayout { index: i, start_frame, blob_offset: off, size, sizes, sizes2 });
        }
        out
    }
}

/// Absolute section offsets inside the blob for one frame set (0x009DAEA3..0x009DAF37).
#[derive(Debug, Clone, Default)]
pub struct FrameSetSections {
    pub intra: [usize; 4],
    pub mask: usize,
    pub mask_bytes: usize,
    pub keys: [usize; 4],
    pub q_header: usize,
    pub q_sizes: [u16; 3],
    pub q: [usize; 4],
    pub end: usize,
}

impl EdgeClip {
    /// `frames` = number of frames covered by the set (bits per channel in the key mask).
    pub fn sections(&self, fs: &FrameSetLayout, frames: usize) -> FrameSetSections {
        let h = &self.header;
        let b = &self.blob;
        let s = fs.sizes.map(|x| x as usize);
        let base = fs.blob_offset;
        let mut o = FrameSetSections::default();
        o.intra[0] = base + 0x10;
        o.intra[1] = o.intra[0] + s[0];
        o.intra[2] = o.intra[1] + s[1];
        o.intra[3] = o.intra[2] + s[2];
        o.mask = o.intra[3] + s[3];
        let channels = (h.num_anim_r + h.num_anim_t + h.num_anim_s + h.num_anim_user) as usize;
        // `frames` excludes the set's initial frame; the next set's initial frame closes it.
        o.mask_bytes = (channels * frames).div_ceil(8);
        o.keys[0] = o.mask + o.mask_bytes;
        o.keys[1] = o.keys[0] + s[4];
        o.keys[2] = o.keys[1] + s[5];
        o.keys[3] = (o.keys[2] + s[6] + 3) & !3;
        // Alignment is relative to the blob start (16-byte aligned in memory on load).
        o.q_header = (o.keys[3] + s[7] + 15) & !15;
        if o.q_header + 16 <= b.len() {
            o.q_sizes = [u16le(b, o.q_header), u16le(b, o.q_header + 2), u16le(b, o.q_header + 4)];
            o.q[0] = o.q_header + 0x10;
            o.q[1] = o.q[0] + o.q_sizes[0] as usize;
            o.q[2] = o.q[1] + o.q_sizes[1] as usize;
            o.q[3] = o.q[2] + o.q_sizes[2] as usize;
            o.end = o.q[3];
        }
        o
    }
}

impl EdgeClip {
    /// Number of key-mask bits per channel for a set (frames strictly between the set's
    /// initial frame and the next set's initial frame).
    pub fn inner_frames(&self, sets: &[FrameSetLayout], k: usize) -> usize {
        let next = sets.get(k + 1).map(|n| n.start_frame as usize).unwrap_or(self.header.num_frames as usize);
        next.saturating_sub(sets[k].start_frame as usize + 1)
    }

    /// Key-mask bit for (channel, inner frame) — channel-major, MSB-first.
    pub fn key_bit(&self, sec: &FrameSetSections, frames: usize, channel: usize, f: usize) -> bool {
        let bit = channel * frames + f;
        let byte = self.blob.get(sec.mask + bit / 8).copied().unwrap_or(0);
        (byte >> (7 - (bit % 8))) & 1 != 0
    }
}

/// Key storage order inside a type's key section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyOrder {
    ChannelMajor,
    FrameMajor,
}

/// All samples of one animated channel within a frame set: (local frame, value xyz).
pub type ChannelKeys = Vec<(usize, [f32; 3])>;

impl EdgeClip {
    fn type_specs(&self, ty: usize) -> (usize, usize, usize) {
        // (first anim spec index, channel count, mask channel offset) for R=0, T=1.
        let h = &self.header;
        let nr = h.num_anim_r as usize;
        let nt = h.num_anim_t as usize;
        match ty {
            0 => (1, nr, 0),
            _ => (if h.flags & 1 != 0 { nr + 2 } else { 1 }, nt, nr),
        }
    }

    /// Decode every key of every channel of type `ty` in set `k` (start/end initial frames
    /// included), using the given key order.
    pub fn set_keys(&self, sets: &[FrameSetLayout], k: usize, ty: usize, order: KeyOrder) -> Vec<ChannelKeys> {
        let (spec0, n, mask_off) = self.type_specs(ty);
        let inner = self.inner_frames(sets, k);
        let sec = self.sections(&sets[k], inner);
        let mut out: Vec<ChannelKeys> = vec![Vec::new(); n];
        // Start initial frame.
        let mut br = BitReader::new(&self.blob, sec.intra[ty] * 8);
        for c in 0..n {
            out[c].push((0, read_value(&mut br, &self.specs[spec0 + c])));
        }
        // Inner keys.
        let mut br = BitReader::new(&self.blob, sec.keys[ty] * 8);
        match order {
            KeyOrder::ChannelMajor => {
                for c in 0..n {
                    for f in 0..inner {
                        if self.key_bit(&sec, inner, mask_off + c, f) {
                            out[c].push((f + 1, read_value(&mut br, &self.specs[spec0 + c])));
                        }
                    }
                }
            }
            KeyOrder::FrameMajor => {
                for f in 0..inner {
                    for c in 0..n {
                        if self.key_bit(&sec, inner, mask_off + c, f) {
                            out[c].push((f + 1, read_value(&mut br, &self.specs[spec0 + c])));
                        }
                    }
                }
            }
        }
        // End frame = next set's initial frame (q block).
        if sec.q_header != 0 && k + 1 < sets.len() {
            let next = &sets[k + 1];
            let nsec = self.sections(next, self.inner_frames(sets, k + 1));
            let mut br = BitReader::new(&self.blob, nsec.intra[ty] * 8);
            for c in 0..n {
                out[c].push((inner + 1, read_value(&mut br, &self.specs[spec0 + c])));
            }
        }
        out
    }
}

/// A clip decoded to per-frame local joint transforms (`None` = joint not driven).
#[derive(Debug, Clone)]
pub struct BakedClip {
    pub num_frames: usize,
    pub sample_rate: f32,
    pub duration: f32,
    /// [frame][joint]
    pub rotations: Vec<Vec<Option<Quat>>>,
    pub translations: Vec<Vec<Option<Vec3>>>,
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

impl EdgeClip {
    /// Decode every frame. Frames without a stored key are linearly interpolated between
    /// the neighbouring keys (component-wise on the packed xyz before recomposition for
    /// translations; normalised lerp with hemisphere fix for rotations).
    pub fn bake(&self) -> Result<BakedClip> {
        let h = &self.header;
        ensure!(h.flags & 3 == 3, "fixed-format (non bit-packed) channels not supported yet (flags {})", h.flags);
        let nj = h.num_joints as usize;
        let nf = h.num_frames as usize;
        let mut rot = vec![vec![None; nj]; nf];
        let mut tr = vec![vec![None; nj]; nf];
        for (j, q) in self.const_rotations() {
            for f in rot.iter_mut() {
                if (j as usize) < nj {
                    f[j as usize] = Some(q);
                }
            }
        }
        for (j, t) in self.const_translations() {
            for f in tr.iter_mut() {
                if (j as usize) < nj {
                    f[j as usize] = Some(t);
                }
            }
        }
        let sets = self.frame_sets();
        for k in 0..sets.len() {
            let start = sets[k].start_frame as usize;
            let inner = self.inner_frames(&sets, k);
            for ty in 0..2 {
                let (spec0, _, _) = self.type_specs(ty);
                let joints = if ty == 0 { &self.anim_r_joints } else { &self.anim_t_joints };
                let keys = self.set_keys(&sets, k, ty, KeyOrder::ChannelMajor);
                for (c, ch) in keys.iter().enumerate() {
                    let j = joints[c] as usize;
                    if j >= nj {
                        continue;
                    }
                    let slot = self.specs[spec0 + c].recompose;
                    // Last set has a single frame (its initial frame).
                    let last = if k + 1 < sets.len() { inner + 1 } else { 0 };
                    for lf in 0..=last {
                        let f = start + lf;
                        if f >= nf {
                            continue;
                        }
                        let i = ch.partition_point(|(kf, _)| *kf <= lf);
                        let (a, b) = (ch[i.saturating_sub(1)], ch.get(i).copied().unwrap_or(ch[i.saturating_sub(1)]));
                        let t = if b.0 > a.0 { (lf - a.0) as f32 / (b.0 - a.0) as f32 } else { 0.0 };
                        if ty == 0 {
                            let qa = recompose_quat(a.1, slot);
                            let mut qb = recompose_quat(b.1, slot);
                            if qa.dot(qb) < 0.0 {
                                qb = -qb;
                            }
                            rot[f][j] = Some(qa.lerp(qb, t).normalize());
                        } else {
                            tr[f][j] = Some(Vec3::from(lerp3(a.1, b.1, t)));
                        }
                    }
                }
            }
        }
        Ok(BakedClip { num_frames: nf, sample_rate: h.sample_frequency, duration: h.duration, rotations: rot, translations: tr })
    }
}

impl BakedClip {
    /// Sample at `time` seconds (looping), blending neighbouring frames.
    pub fn sample(&self, time: f32, joint: usize) -> (Option<Quat>, Option<Vec3>) {
        if self.num_frames == 0 || self.rotations.first().is_none_or(|r| joint >= r.len()) {
            return (None, None);
        }
        let len = (self.num_frames - 1).max(1) as f32;
        let fpos = (time * self.sample_rate).rem_euclid(len);
        let f0 = fpos.floor() as usize;
        let f1 = (f0 + 1).min(self.num_frames - 1);
        let t = fpos - f0 as f32;
        let r = match (self.rotations[f0][joint], self.rotations[f1][joint]) {
            (Some(a), Some(mut b)) => {
                if a.dot(b) < 0.0 {
                    b = -b;
                }
                Some(a.lerp(b, t).normalize())
            }
            (a, b) => a.or(b),
        };
        let p = match (self.translations[f0][joint], self.translations[f1][joint]) {
            (Some(a), Some(b)) => Some(a.lerp(b, t)),
            (a, b) => a.or(b),
        };
        (r, p)
    }
}
