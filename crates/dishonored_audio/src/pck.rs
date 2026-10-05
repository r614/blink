//! Wwise file packages (`AKPK`, `.pck`): a header with a language map and lookup
//! tables for soundbanks and streamed media files, followed by the payloads.

use anyhow::{Context, Result, bail, ensure};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct PckEntry {
    pub id: u32,
    pub offset: u64,
    pub size: u32,
    pub language: u32,
}

#[derive(Debug, Clone)]
pub struct Pck {
    pub path: PathBuf,
    pub languages: Vec<(u32, String)>,
    pub banks: Vec<PckEntry>,
    pub streams: Vec<PckEntry>,
}

fn u32le(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .context("AKPK header truncated")
}

impl Pck {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut f = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let mut head = [0u8; 8];
        f.read_exact(&mut head)?;
        ensure!(&head[..4] == b"AKPK", "{} is not an AKPK package", path.display());
        let hsize = u32::from_le_bytes(head[4..8].try_into().unwrap()) as usize;
        ensure!(hsize < 64 << 20, "implausible AKPK header size {hsize}");
        let mut h = vec![0u8; hsize];
        f.read_exact(&mut h)?;
        Self::parse_header(path, &h)
    }

    /// `h` starts after the 8-byte `AKPK` + size prefix.
    fn parse_header(path: PathBuf, h: &[u8]) -> Result<Self> {
        let version = u32le(h, 0)?;
        ensure!(version == 1, "unsupported AKPK version {version}");
        let lang = u32le(h, 4)? as usize;
        let banks = u32le(h, 8)? as usize;
        let stms = u32le(h, 12)? as usize;
        // Dishonored's packages carry a fourth table (externals, always empty) after the
        // stream table; older layouts have three.
        let base = if 20 + lang + banks + stms + u32le(h, 16).unwrap_or(0) as usize == h.len() {
            20
        } else if 16 + lang + banks + stms == h.len() {
            16
        } else {
            bail!("AKPK table sizes do not add up to the header size");
        };
        let lang_blob = &h[base..base + lang];
        let bank_blob = &h[base + lang..base + lang + banks];
        let stm_blob = &h[base + lang + banks..base + lang + banks + stms];

        let mut languages = Vec::new();
        let n = u32le(lang_blob, 0)? as usize;
        for i in 0..n {
            let off = u32le(lang_blob, 4 + i * 8)? as usize;
            let id = u32le(lang_blob, 8 + i * 8)?;
            let mut s = String::new();
            let mut o = off;
            while o + 1 < lang_blob.len() {
                let c = u16::from_le_bytes([lang_blob[o], lang_blob[o + 1]]);
                if c == 0 {
                    break;
                }
                s.push(char::from_u32(c as u32).unwrap_or('?'));
                o += 2;
            }
            languages.push((id, s));
        }
        Ok(Self { path, languages, banks: Self::lut(bank_blob)?, streams: Self::lut(stm_blob)? })
    }

    fn lut(b: &[u8]) -> Result<Vec<PckEntry>> {
        if b.len() < 4 {
            return Ok(Vec::new());
        }
        let n = u32le(b, 0)? as usize;
        if n == 0 {
            return Ok(Vec::new());
        }
        let stride = (b.len() - 4) / n;
        ensure!(stride == 20 || stride == 24, "unexpected AKPK LUT stride {stride}");
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let o = 4 + i * stride;
            let id = u32le(b, o)?;
            let block = u32le(b, o + 4)?.max(1) as u64;
            let size = u32le(b, o + 8)?;
            let (start, lang) = if stride == 20 {
                (u32le(b, o + 12)? as u64, u32le(b, o + 16)?)
            } else {
                (u32le(b, o + 12)? as u64 | (u32le(b, o + 16)? as u64) << 32, u32le(b, o + 20)?)
            };
            out.push(PckEntry { id, offset: start * block, size, language: lang });
        }
        Ok(out)
    }

    pub fn read(&self, e: &PckEntry) -> Result<Vec<u8>> {
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(e.offset))?;
        let mut v = vec![0u8; e.size as usize];
        f.read_exact(&mut v).with_context(|| {
            format!("read {} bytes at {} in {}", e.size, e.offset, self.path.display())
        })?;
        Ok(v)
    }
}
