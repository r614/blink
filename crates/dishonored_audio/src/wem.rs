//! WEM media files: RIFF/WAVE containers whose `fmt ` codec tag selects PCM,
//! IMA ADPCM or Wwise Vorbis (0xFFFF).

use anyhow::{Context, Result, bail, ensure};

pub const CODEC_PCM: u16 = 0x0001;
pub const CODEC_IMA: u16 = 0x0002;
pub const CODEC_VORBIS: u16 = 0xFFFF;

#[derive(Debug, Clone)]
pub struct Riff<'a> {
    pub codec: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub avg_bytes: u32,
    pub block_align: u16,
    pub bits: u16,
    /// Full `fmt ` chunk body.
    pub fmt: &'a [u8],
    pub chunks: Vec<([u8; 4], &'a [u8])>,
}

impl<'a> Riff<'a> {
    pub fn parse(b: &'a [u8]) -> Result<Self> {
        ensure!(b.len() >= 12, "WEM too short");
        ensure!(&b[0..4] == b"RIFF", "not RIFF (got {:02x?})", &b[0..4]);
        ensure!(&b[8..12] == b"WAVE", "not WAVE");
        let riff_end = (u32::from_le_bytes(b[4..8].try_into().unwrap()) as usize + 8).min(b.len());
        let mut chunks = Vec::new();
        let mut o = 12;
        while o + 8 <= riff_end {
            let tag: [u8; 4] = b[o..o + 4].try_into().unwrap();
            let sz = u32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap()) as usize;
            let end = (o + 8 + sz).min(b.len());
            chunks.push((tag, &b[o + 8..end]));
            o += 8 + sz + (sz & 1);
        }
        let fmt = chunks
            .iter()
            .find(|(t, _)| t == b"fmt ")
            .map(|(_, c)| *c)
            .context("no fmt chunk")?;
        ensure!(fmt.len() >= 16, "fmt chunk too short");
        let u16a = |o: usize| u16::from_le_bytes([fmt[o], fmt[o + 1]]);
        let u32a = |o: usize| u32::from_le_bytes(fmt[o..o + 4].try_into().unwrap());
        Ok(Self {
            codec: u16a(0),
            channels: u16a(2),
            sample_rate: u32a(4),
            avg_bytes: u32a(8),
            block_align: u16a(12),
            bits: u16a(14),
            fmt,
            chunks,
        })
    }

    pub fn chunk(&self, tag: &[u8; 4]) -> Option<&'a [u8]> {
        self.chunks.iter().find(|(t, _)| t == tag).map(|(_, c)| *c)
    }

    pub fn data(&self) -> Result<&'a [u8]> {
        self.chunk(b"data").context("no data chunk")
    }

    pub fn codec_name(&self) -> &'static str {
        match self.codec {
            CODEC_PCM => "PCM",
            CODEC_IMA => "IMA-ADPCM",
            CODEC_VORBIS => "Vorbis",
            0x0166 => "XMA2",
            0x0165 => "XMA",
            _ => "unknown",
        }
    }
}

/// Decoded audio as interleaved f32 samples.
#[derive(Debug, Clone, Default)]
pub struct Pcm {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
    pub fn duration(&self) -> f32 {
        self.frames() as f32 / self.sample_rate.max(1) as f32
    }
    pub fn peak(&self) -> f32 {
        self.samples.iter().fold(0.0f32, |a, s| a.max(s.abs()))
    }
    pub fn rms(&self) -> f32 {
        if self.samples.is_empty() {
            return 0.0;
        }
        (self.samples.iter().map(|s| (*s as f64) * (*s as f64)).sum::<f64>()
            / self.samples.len() as f64)
            .sqrt() as f32
    }

    /// 16-bit PCM WAV bytes (for listening checks under `harness_out/`).
    pub fn to_wav(&self) -> Vec<u8> {
        let data_len = self.samples.len() * 2;
        let mut v = Vec::with_capacity(44 + data_len);
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&self.channels.to_le_bytes());
        v.extend_from_slice(&self.sample_rate.to_le_bytes());
        v.extend_from_slice(&(self.sample_rate * self.channels as u32 * 2).to_le_bytes());
        v.extend_from_slice(&(self.channels * 2).to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data_len as u32).to_le_bytes());
        for s in &self.samples {
            v.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        v
    }
}

/// Decode any supported WEM.
pub fn decode(b: &[u8], codebooks: &crate::vorbis::CodebookLibrary) -> Result<Pcm> {
    let r = Riff::parse(b)?;
    match r.codec {
        CODEC_PCM => decode_pcm(&r),
        CODEC_IMA => crate::adpcm::decode_ima(&r),
        CODEC_VORBIS => crate::vorbis::decode(&r, codebooks),
        c => bail!("unsupported WEM codec 0x{c:04x}"),
    }
}

fn decode_pcm(r: &Riff) -> Result<Pcm> {
    ensure!(r.bits == 16, "PCM with {} bits", r.bits);
    let d = r.data()?;
    Ok(Pcm {
        channels: r.channels,
        sample_rate: r.sample_rate,
        samples: d
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
    })
}
