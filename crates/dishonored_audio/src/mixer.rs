//! DSP for the effects the banks reference: the environments' reverbs (RoomVerb / Matrix
//! Reverb sharesets of Init.bnk, sent from game objects inside `DishonoredAudioVolume`s) and
//! the Parametric EQ custom effects inserted on sounds.
//!
//! The parameters are the retail ones (`bus::RoomVerb`, `ParamEq::from_fx`); the algorithms
//! are not Audiokinetic's (APPROXIMATION, AUDIO_FORMAT.md "Buses and environments"): the
//! reverb is a feedback delay network (eight lines, Householder matrix) with a pre-delay,
//! an early-reflection tap line, input diffusion, per-line high-frequency damping and the
//! three tone filters; the EQ is RBJ biquads. Everything is deterministic f32 arithmetic.

use crate::bus::{MatrixReverb, RoomVerb};
use crate::hirc::Fx;

fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
}

/// RBJ biquad (transposed direct form II), one per channel.
#[derive(Debug, Clone, Copy, Default)]
pub struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [[f32; 2]; 2],
}

impl Biquad {
    /// Wwise Parametric EQ filter types: 0 low pass, 1 high pass, 2 band pass, 3 notch,
    /// 4 low shelf, 5 high shelf, 6 peaking.
    pub fn new(kind: u32, gain_db: f32, freq: f32, q: f32, rate: f32) -> Self {
        let f = freq.clamp(10.0, rate * 0.45);
        let w = 2.0 * std::f32::consts::PI * f / rate;
        let (s, c) = w.sin_cos();
        let q = q.max(0.05);
        let alpha = s / (2.0 * q);
        let a = 10f32.powf(gain_db / 40.0);
        let (b0, b1, b2, a0, a1, a2) = match kind {
            0 => ((1.0 - c) / 2.0, 1.0 - c, (1.0 - c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
            1 => ((1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
            2 => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
            3 => (1.0, -2.0 * c, 1.0, 1.0 + alpha, -2.0 * c, 1.0 - alpha),
            4 => {
                let sq = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * c + sq),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * c),
                    a * ((a + 1.0) - (a - 1.0) * c - sq),
                    (a + 1.0) + (a - 1.0) * c + sq,
                    -2.0 * ((a - 1.0) + (a + 1.0) * c),
                    (a + 1.0) + (a - 1.0) * c - sq,
                )
            }
            5 => {
                let sq = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * c + sq),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * c),
                    a * ((a + 1.0) + (a - 1.0) * c - sq),
                    (a + 1.0) - (a - 1.0) * c + sq,
                    2.0 * ((a - 1.0) - (a + 1.0) * c),
                    (a + 1.0) - (a - 1.0) * c - sq,
                )
            }
            _ => (1.0 + alpha * a, -2.0 * c, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * c, 1.0 - alpha / a),
        };
        Biquad { b: [b0 / a0, b1 / a0, b2 / a0], a: [a1 / a0, a2 / a0], z: [[0.0; 2]; 2] }
    }

    #[inline]
    pub fn process(&mut self, ch: usize, x: f32) -> f32 {
        let z = &mut self.z[ch];
        let y = self.b[0] * x + z[0];
        z[0] = self.b[1] * x - self.a[0] * y + z[1];
        z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// Wwise Parametric EQ (plugin 0x00690003, 56-byte block): three bands {u32 type, f32 gain dB,
/// f32 frequency, f32 Q, u8 on}, f32 output level dB, u8 process LFE.
#[derive(Debug, Clone)]
pub struct ParamEq {
    bands: Vec<Biquad>,
    out_gain: f32,
}

impl ParamEq {
    pub const PLUGIN: u32 = 0x0069_0003;

    pub fn from_fx(f: &Fx, rate: f32) -> Option<Self> {
        if f.plugin != Self::PLUGIN || f.params.len() < 56 {
            return None;
        }
        let mut bands = Vec::new();
        for k in 0..3 {
            let o = k * 17;
            if f.params[o + 16] != 0 {
                bands.push(Biquad::new(f.u32_at(o)?, f.f32_at(o + 4)?, f.f32_at(o + 8)?, f.f32_at(o + 12)?, rate));
            }
        }
        Some(ParamEq { bands, out_gain: db(f.f32_at(51)?) })
    }

    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (mut l, mut r) = (l, r);
        for b in &mut self.bands {
            l = b.process(0, l);
            r = b.process(1, r);
        }
        (l * self.out_gain, r * self.out_gain)
    }
}

struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    fn new(n: usize) -> Self {
        Delay { buf: vec![0.0; n.max(1)], pos: 0 }
    }
    #[inline]
    fn read(&self, back: usize) -> f32 {
        let n = self.buf.len();
        self.buf[(self.pos + n - (back % n)) % n]
    }
    #[inline]
    fn push(&mut self, x: f32) {
        self.pos = (self.pos + 1) % self.buf.len();
        self.buf[self.pos] = x;
    }
}

struct Allpass {
    d: Delay,
    len: usize,
    g: f32,
}

impl Allpass {
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let delayed = self.d.read(self.len - 1);
        let v = x + self.g * delayed;
        self.d.push(v);
        delayed - self.g * v
    }
}

const LINES: usize = 8;
/// Late-reverb line lengths (ms) for a medium room; scaled by the room size.
const LINE_MS: [f32; LINES] = [29.7, 37.1, 41.1, 43.7, 53.1, 59.9, 67.7, 73.9];
/// Early-reflection taps (ms, gain) at room size 0; the ER pattern rotates them.
const ER_TAPS: [(f32, f32); 8] = [(4.3, 0.9), (7.9, -0.8), (11.3, 0.7), (16.7, -0.65), (21.1, 0.55), (26.9, -0.5), (33.7, 0.4), (41.3, -0.35)];

/// One environment's reverb instance (shared by every object sending to it).
pub struct Reverb {
    pre: Delay,
    pre_len: usize,
    er: Delay,
    er_taps: Vec<(usize, f32, usize)>,
    diff: Vec<Allpass>,
    lines: Vec<Delay>,
    lens: Vec<usize>,
    gains: Vec<f32>,
    damp: Vec<f32>,
    lp: Vec<f32>,
    tone: Vec<Biquad>,
    er_gain: f32,
    late_gain: f32,
    width: f32,
    /// Seconds of output below the silence threshold (the instance can be dropped).
    pub quiet: f32,
}

impl Reverb {
    pub fn from_fx(f: &Fx, rate: f32) -> Option<Self> {
        if let Some(r) = RoomVerb::from_fx(f) {
            return Some(Self::new(&r, rate));
        }
        let m = MatrixReverb::from_fx(f)?;
        // Matrix Reverb: no early reflections; its wet level drives the tail.
        let r = RoomVerb {
            decay_s: m.decay_s,
            hf_damping: m.hf_ratio.max(0.5),
            diffusion: 100.0,
            stereo_width: 180.0,
            filters: [(0.0, 100.0, 1.0), (0.0, 1000.0, 1.0), (0.0, 10000.0, 1.0)],
            er_db: -96.3,
            reverb_db: m.wet_db,
            er_enabled: false,
            density: 70.0,
            quality: m.delay_lines,
            ..Default::default()
        };
        Some(Self::new(&r, rate))
    }

    pub fn new(p: &RoomVerb, rate: f32) -> Self {
        let ms = |v: f32| ((v.max(0.0) * rate / 1000.0) as usize).max(1);
        let size = (1.0 + p.room_size / 100.0).clamp(0.3, 2.0);
        let decay = p.decay_s.max(0.1);
        let lens: Vec<usize> = LINE_MS.iter().map(|l| ms(l * size * (0.75 + p.density.clamp(0.0, 100.0) / 400.0))).collect();
        // Per-line feedback gain for a -60 dB decay over `decay` seconds; HF decays
        // `hf_damping` times faster (one-pole low pass in each line).
        let gains: Vec<f32> = lens.iter().map(|&l| 10f32.powf(-3.0 * l as f32 / (decay * rate))).collect();
        let hf = p.hf_damping.clamp(0.5, 10.0);
        let damp: Vec<f32> = gains.iter().map(|g| (1.0 - g.powf(hf - 1.0).clamp(0.0, 1.0)).clamp(0.0, 0.95)).collect();
        let g = 0.7 * (p.diffusion / 100.0).clamp(0.0, 1.0);
        let diff = [1.3f32, 2.9, 4.7, 6.1]
            .iter()
            .map(|d| {
                let len = ms(*d);
                Allpass { d: Delay::new(len + 1), len, g }
            })
            .collect();
        let rot = (p.er_pattern as usize) % ER_TAPS.len();
        let er_taps: Vec<(usize, f32, usize)> = (0..ER_TAPS.len())
            .map(|k| {
                let (t, g) = ER_TAPS[(k + rot) % ER_TAPS.len()];
                // Odd taps come back from the rear: later by the front/back delay.
                let back = if k % 2 == 1 { p.er_front_back_ms } else { 0.0 };
                (ms(t * size + back), g, k % 2)
            })
            .collect();
        let er_len = er_taps.iter().map(|t| t.0).max().unwrap_or(1) + 1;
        let pre_len = ms(p.reverb_delay_ms);
        let tone = p.filters.iter().enumerate().filter(|(_, f)| f.0.abs() > 0.01).map(|(k, f)| Biquad::new([4, 6, 5][k], f.0, f.1, f.2, rate)).collect();
        Reverb {
            pre: Delay::new(pre_len + 1),
            pre_len,
            er: Delay::new(er_len),
            er_taps,
            diff,
            lines: lens.iter().map(|&l| Delay::new(l + 1)).collect(),
            lens,
            gains,
            damp,
            lp: vec![0.0; LINES],
            tone,
            er_gain: if p.er_enabled { db(p.er_db) } else { 0.0 },
            late_gain: db(p.reverb_db),
            width: (p.stereo_width / 180.0).clamp(0.0, 1.0),
            quiet: 0.0,
        }
    }

    /// Process interleaved stereo `input` (the sends) and add the wet signal to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut [f32], rate: f32) {
        let mut peak = 0.0f32;
        for (i, fr) in input.chunks_exact(2).enumerate() {
            let x = 0.5 * (fr[0] + fr[1]);
            // Early reflections from the dry send, late reverb after the pre-delay.
            self.er.push(x);
            let (mut el, mut er) = (0.0, 0.0);
            for &(d, g, side) in &self.er_taps {
                let v = self.er.read(d) * g;
                if side == 0 {
                    el += v;
                } else {
                    er += v;
                }
            }
            self.pre.push(x);
            let mut d = self.pre.read(self.pre_len);
            for a in &mut self.diff {
                d = a.process(d);
            }
            let mut outs = [0.0f32; LINES];
            let mut sum = 0.0;
            for k in 0..LINES {
                let v = self.lines[k].read(self.lens[k]);
                self.lp[k] += (1.0 - self.damp[k]) * (v - self.lp[k]);
                outs[k] = self.lp[k] * self.gains[k];
                sum += outs[k];
            }
            // Householder feedback: y = x - 2/N * sum(x).
            let h = 2.0 / LINES as f32 * sum;
            for k in 0..LINES {
                self.lines[k].push(outs[k] - h + d);
            }
            let ll = outs[0] - outs[2] + outs[4] - outs[6];
            let lr = outs[1] - outs[3] + outs[5] - outs[7];
            let (mid, side) = (0.5 * (ll + lr), 0.5 * (ll - lr) * self.width);
            let mut wl = (mid + side) * self.late_gain + el * self.er_gain;
            let mut wr = (mid - side) * self.late_gain + er * self.er_gain;
            for t in &mut self.tone {
                wl = t.process(0, wl);
                wr = t.process(1, wr);
            }
            out[i * 2] += wl;
            out[i * 2 + 1] += wr;
            peak = peak.max(wl.abs()).max(wr.abs());
        }
        let secs = input.len() as f32 / 2.0 / rate;
        if peak < 1e-5 {
            self.quiet += secs;
        } else {
            self.quiet = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverb_decays_at_its_decay_time() {
        let rate = 48000.0;
        let p = RoomVerb { decay_s: 1.0, hf_damping: 1.0, diffusion: 50.0, stereo_width: 180.0, reverb_db: 0.0, er_db: -96.3, density: 50.0, ..Default::default() };
        let mut r = Reverb::new(&p, rate);
        let mut input = vec![0.0f32; 48000 * 2 * 2];
        input[0] = 1.0;
        input[1] = 1.0;
        let mut out = vec![0.0f32; input.len()];
        r.process(&input, &mut out, rate);
        let energy = |a: usize, b: usize| out[a * 2..b * 2].iter().map(|x| x * x).sum::<f32>();
        let early = energy(4800, 9600);
        let late = energy(52800, 57600);
        // -60 dB per second: 1 s later the energy is ~1e-6 of the early window (+- an order).
        let ratio = late / early.max(1e-20);
        assert!(early > 0.0 && ratio < 1e-4 && ratio > 1e-9, "early {early} late {late} ratio {ratio}");
    }

    #[test]
    fn peaking_eq_boosts_its_band() {
        let rate = 48000.0;
        let mut b = Biquad::new(6, 12.0, 1000.0, 1.0, rate);
        let mut peak = 0.0f32;
        for i in 0..48000 {
            let x = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / rate).sin();
            let y = b.process(0, x);
            if i > 24000 {
                peak = peak.max(y.abs());
            }
        }
        assert!((peak - db(12.0)).abs() < 0.2, "peak {peak}");
    }
}
