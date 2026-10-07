//! Wwise Vorbis (WAVE codec 0xFFFF) -> standard Vorbis packets -> PCM.
//!
//! Wwise stores Vorbis without Ogg framing and with a stripped setup header:
//! codebooks are replaced by 10-bit indices into a codebook library that ships
//! inside the Wwise runtime, and fields that only have one legal value (floor,
//! residue and mapping types, window/transform types) are omitted. Audio packets
//! have a 2-byte size prefix and drop the packet-type bit and window flags.
//!
//! This module rebuilds the three standard headers and the audio packets
//! (following the layout documented by the ww2ogg project) and hands them to
//! `lewton`. The codebook library is not shipped with this project: it is located
//! inside the game's own executable at runtime (see [`CodebookLibrary::from_exe`]).

use crate::wem::{Pcm, Riff};
use anyhow::{Context, Result, bail, ensure};
use std::path::Path;

// ---------------------------------------------------------------------------
// Bit I/O (Vorbis packs bits LSB first)

pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn bits_read(&self) -> usize {
        self.pos
    }
    pub fn read(&mut self, n: u32) -> Result<u32> {
        let mut v = 0u32;
        for i in 0..n {
            let byte = *self.data.get(self.pos >> 3).context("bitstream exhausted")?;
            if byte >> (self.pos & 7) & 1 != 0 {
                v |= 1 << i;
            }
            self.pos += 1;
        }
        Ok(v)
    }
}

#[derive(Default)]
pub(crate) struct BitWriter {
    pub bytes: Vec<u8>,
    bit: usize,
}

impl BitWriter {
    pub fn write(&mut self, v: u32, n: u32) {
        for i in 0..n {
            if self.bit & 7 == 0 {
                self.bytes.push(0);
            }
            if v >> i & 1 != 0 {
                *self.bytes.last_mut().unwrap() |= 1 << (self.bit & 7);
            }
            self.bit += 1;
        }
    }
    pub fn write_bytes(&mut self, b: &[u8]) {
        let sh = self.bit & 7;
        if sh == 0 {
            self.bytes.extend_from_slice(b);
            self.bit += b.len() * 8;
            return;
        }
        self.bytes.reserve(b.len());
        for &x in b {
            *self.bytes.last_mut().unwrap() |= x << sh;
            self.bytes.push(x >> (8 - sh));
        }
        self.bit += b.len() * 8;
    }
}

/// Copy `n` bits from `r` to `w`, returning the value.
fn cp(r: &mut BitReader, w: &mut BitWriter, n: u32) -> Result<u32> {
    let v = r.read(n)?;
    w.write(v, n);
    Ok(v)
}

fn ilog(mut v: u32) -> u32 {
    let mut n = 0;
    while v != 0 {
        n += 1;
        v >>= 1;
    }
    n
}

fn maptype1_quantvals(entries: u32, dims: u32) -> u32 {
    let bits = ilog(entries) as i64;
    let mut vals = (entries as i64) >> ((bits - 1) * (dims as i64 - 1) / dims as i64);
    loop {
        let mut acc: i64 = 1;
        let mut acc1: i64 = 1;
        for _ in 0..dims {
            acc = acc.saturating_mul(vals);
            acc1 = acc1.saturating_mul(vals + 1);
        }
        if acc <= entries as i64 && acc1 > entries as i64 {
            return vals as u32;
        }
        if acc > entries as i64 {
            vals -= 1;
        } else {
            vals += 1;
        }
    }
}

/// Expand one codebook from the Wwise packed form to the standard Vorbis form.
/// Returns the number of input bits consumed.
fn rebuild_codebook(r: &mut BitReader, w: &mut BitWriter) -> Result<()> {
    let dims = r.read(4)?;
    let entries = r.read(14)?;
    ensure!(dims > 0 && entries > 0, "empty codebook");
    w.write(0x564342, 24);
    w.write(dims, 16);
    w.write(entries, 24);
    let ordered = cp(r, w, 1)?;
    if ordered != 0 {
        cp(r, w, 5)?;
        let mut cur = 0;
        while cur < entries {
            cur += cp(r, w, ilog(entries - cur))?;
        }
        ensure!(cur == entries, "ordered codebook overrun");
    } else {
        let len_len = r.read(3)?;
        let sparse = cp(r, w, 1)?;
        ensure!(len_len != 0 && len_len <= 5, "bad codeword length length {len_len}");
        for _ in 0..entries {
            let present = if sparse != 0 { cp(r, w, 1)? } else { 1 };
            if present != 0 {
                let l = r.read(len_len)?;
                w.write(l, 5);
            }
        }
    }
    let lookup = r.read(1)?;
    w.write(lookup, 4);
    if lookup == 1 {
        cp(r, w, 32)?;
        cp(r, w, 32)?;
        let value_len = cp(r, w, 4)?;
        cp(r, w, 1)?;
        for _ in 0..maptype1_quantvals(entries, dims) {
            cp(r, w, value_len + 1)?;
        }
    } else {
        ensure!(lookup == 0, "bad lookup type");
    }
    Ok(())
}

/// Research helper: bytes consumed by expanding the packed codebook at the start of `b`.
pub fn packed_codebook_size(b: &[u8]) -> Result<usize> {
    let mut r = BitReader::new(b);
    rebuild_codebook(&mut r, &mut BitWriter::default())?;
    Ok(r.bits_read() / 8 + 1)
}

// ---------------------------------------------------------------------------
// Codebook library

/// The Wwise Vorbis codebook library (packed codebooks addressed by index).
#[derive(Clone)]
pub struct CodebookLibrary {
    blob: Vec<u8>,
    offsets: Vec<usize>,
}

impl CodebookLibrary {
    pub fn len(&self) -> usize {
        self.offsets.len()
    }
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    pub fn get(&self, id: usize) -> Option<&[u8]> {
        let start = *self.offsets.get(id)?;
        let end = self.offsets.get(id + 1).copied().unwrap_or(self.blob.len());
        self.blob.get(start..end)
    }

    /// Find the codebook table inside a Win32 PE image of the game.
    ///
    /// The table is an array of absolute pointers (one per codebook, ascending,
    /// small strides) into `.rdata`. Every candidate entry is validated by
    /// expanding the codebook and checking it consumes exactly its stride.
    pub fn from_exe_bytes(exe: &[u8]) -> Result<Self> {
        let pe = Pe::parse(exe)?;
        let rd = |o: usize| u32::from_le_bytes(exe[o..o + 4].try_into().unwrap());
        let mut o = 0;
        let mut best: Option<Self> = None;
        while o + 8 <= exe.len() {
            let v0 = rd(o);
            if pe.va_to_off(v0).is_none() {
                o += 4;
                continue;
            }
            let mut ptrs = vec![v0];
            let mut p = o + 4;
            while p + 4 <= exe.len() {
                let v = rd(p);
                let d = v.wrapping_sub(*ptrs.last().unwrap());
                if d == 0 || d > 4096 || pe.va_to_off(v).is_none() {
                    break;
                }
                ptrs.push(v);
                p += 4;
            }
            if ptrs.len() >= 200 {
                if let Some(lib) = Self::validate(exe, &pe, &ptrs) {
                    if best.as_ref().is_none_or(|b| lib.len() > b.len()) {
                        best = Some(lib);
                    }
                }
                o = p;
                continue;
            }
            o += 4;
        }
        best.context("no Wwise Vorbis codebook table found in the executable")
    }

    /// Largest run of consecutive entries whose codebooks expand to exactly their stride.
    fn validate(exe: &[u8], pe: &Pe, ptrs: &[u32]) -> Option<Self> {
        let ok = |i: usize| -> bool {
            let (Some(a), Some(b)) = (pe.va_to_off(ptrs[i]), pe.va_to_off(ptrs[i + 1])) else {
                return false;
            };
            let mut r = BitReader::new(&exe[a..b]);
            let mut w = BitWriter::default();
            rebuild_codebook(&mut r, &mut w).is_ok() && r.bits_read() / 8 + 1 == b - a
        };
        let (mut best, mut start) = ((0, 0), 0);
        for i in 0..ptrs.len() - 1 {
            if !ok(i) {
                start = i + 1;
            } else if i + 1 - start > best.1 - best.0 {
                best = (start, i + 1);
            }
        }
        if best.1 - best.0 < 100 {
            return None;
        }
        // Include the entry after the run (validated by parsing below, stride unknown).
        let end = (best.1 + 1).min(ptrs.len());
        Self::build(exe, pe, &ptrs[best.0..end])
    }

    fn build(exe: &[u8], pe: &Pe, ptrs: &[u32]) -> Option<Self> {
        let base = pe.va_to_off(ptrs[0])?;
        let last = pe.va_to_off(*ptrs.last().unwrap())?;
        // The last codebook has no successor pointer; give it room to parse.
        let end = (last + 8192).min(exe.len());
        let blob = exe[base..end].to_vec();
        let offsets: Vec<usize> = ptrs.iter().map(|&p| pe.va_to_off(p).unwrap() - base).collect();
        let mut lib = Self { blob, offsets };
        let n = lib.offsets.len();
        for i in 0..n - 1 {
            let cb = lib.get(i)?;
            if packed_codebook_size(cb).ok()? != cb.len() {
                return None;
            }
        }
        // The final pointer is either one more codebook or the end of the table.
        let last_start = lib.offsets[n - 1];
        match packed_codebook_size(&lib.blob[last_start..]) {
            Ok(used) => lib.blob.truncate(last_start + used),
            Err(_) => {
                lib.offsets.pop();
                lib.blob.truncate(last_start);
            }
        }
        Some(lib)
    }

    /// Load from `Binaries/Win32/Dishonored.exe` of an install root.
    pub fn from_install(root: &Path) -> Result<Self> {
        let exe = root.join("Binaries").join("Win32").join("Dishonored.exe");
        let b = std::fs::read(&exe).with_context(|| format!("read {}", exe.display()))?;
        Self::from_exe_bytes(&b)
    }

    /// Load from the packed library file layout (codebooks, offset table, table offset).
    pub fn from_packed_file(b: &[u8]) -> Result<Self> {
        ensure!(b.len() >= 4, "codebook file too short");
        let table = u32::from_le_bytes(b[b.len() - 4..].try_into().unwrap()) as usize;
        ensure!(table < b.len(), "bad codebook table offset");
        let n = (b.len() - table) / 4 - 1;
        let offsets = (0..n)
            .map(|i| u32::from_le_bytes(b[table + i * 4..table + i * 4 + 4].try_into().unwrap()) as usize)
            .collect();
        Ok(Self { blob: b[..table].to_vec(), offsets })
    }
}

struct Pe {
    image_base: u32,
    sections: Vec<(u32, u32, u32, u32)>, // va, vsize, raw ptr, raw size
}

impl Pe {
    fn parse(b: &[u8]) -> Result<Self> {
        ensure!(b.len() > 0x40 && &b[..2] == b"MZ", "not a PE image");
        let r16 = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
        let r32 = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let pe = r32(0x3c) as usize;
        ensure!(b.get(pe..pe + 4) == Some(b"PE\0\0"), "no PE signature");
        let nsec = r16(pe + 6);
        let opt = pe + 24;
        let opt_size = r16(pe + 20);
        ensure!(r16(opt) == 0x10b, "only PE32 images are supported");
        let image_base = r32(opt + 28);
        let mut sections = Vec::new();
        for i in 0..nsec {
            let s = opt + opt_size + i * 40;
            sections.push((r32(s + 12), r32(s + 8), r32(s + 20), r32(s + 16)));
        }
        Ok(Self { image_base, sections })
    }

    fn va_to_off(&self, va: u32) -> Option<usize> {
        let rva = va.checked_sub(self.image_base)?;
        self.sections.iter().find_map(|&(sva, vsz, raw, rsz)| {
            (rva >= sva && rva < sva + vsz.min(rsz)).then(|| (rva - sva + raw) as usize)
        })
    }
}

// ---------------------------------------------------------------------------
// Stream decoding

/// Fields of the Wwise `vorb` block (embedded in a 0x42-byte `fmt ` chunk at +0x18,
/// or a separate chunk).
#[derive(Debug, Clone)]
pub struct VorbInfo {
    pub sample_count: u32,
    pub mod_packets: bool,
    pub setup_offset: u32,
    pub first_audio_offset: u32,
    pub blocksize0_pow: u8,
    pub blocksize1_pow: u8,
    /// 2-byte packet headers (size only) vs 6-byte (size + granule).
    pub no_granule: bool,
}

pub fn vorb_info(r: &Riff) -> Result<VorbInfo> {
    let vorb: &[u8] = if let Some(v) = r.chunk(b"vorb") {
        v
    } else {
        ensure!(r.fmt.len() == 0x42, "expected 0x42 fmt when the vorb chunk is missing");
        &r.fmt[0x18..]
    };
    let r32 = |o: usize| -> Result<u32> {
        vorb.get(o..o + 4)
            .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
            .context("vorb truncated")
    };
    match vorb.len() {
        0x2A => {
            let mod_signal = r32(0x04)?;
            Ok(VorbInfo {
                sample_count: r32(0)?,
                mod_packets: !matches!(mod_signal, 0x4A | 0x4B | 0x69 | 0x70),
                setup_offset: r32(0x10)?,
                first_audio_offset: r32(0x14)?,
                blocksize0_pow: vorb[0x28],
                blocksize1_pow: vorb[0x29],
                no_granule: true,
            })
        }
        0x32 | 0x34 => Ok(VorbInfo {
            sample_count: r32(0)?,
            mod_packets: false,
            setup_offset: r32(0x18)?,
            first_audio_offset: r32(0x1C)?,
            blocksize0_pow: vorb[0x30],
            blocksize1_pow: vorb[0x31],
            no_granule: false,
        }),
        n => bail!("unsupported vorb layout of {n:#x} bytes"),
    }
}

struct Headers {
    ident: Vec<u8>,
    comment: Vec<u8>,
    setup: Vec<u8>,
    mode_blockflag: Vec<bool>,
    mode_bits: u32,
}

fn build_headers(r: &Riff, vi: &VorbInfo, setup_in: &[u8], lib: &CodebookLibrary) -> Result<Headers> {
    let mut id = BitWriter::default();
    id.write(1, 8);
    id.write_bytes(b"vorbis");
    id.write(0, 32);
    id.write(r.channels as u32, 8);
    id.write(r.sample_rate, 32);
    id.write(0, 32);
    id.write(r.avg_bytes.wrapping_mul(8), 32);
    id.write(0, 32);
    id.write(vi.blocksize0_pow as u32, 4);
    id.write(vi.blocksize1_pow as u32, 4);
    id.write(1, 1);

    let mut cm = BitWriter::default();
    cm.write(3, 8);
    cm.write_bytes(b"vorbis");
    let vendor = b"blink wwise rebuild";
    cm.write(vendor.len() as u32, 32);
    cm.write_bytes(vendor);
    cm.write(0, 32);
    cm.write(1, 1);

    let channels = r.channels as u32;
    let mut s = BitReader::new(setup_in);
    let mut w = BitWriter::default();
    w.write(5, 8);
    w.write_bytes(b"vorbis");

    let cb_count = cp(&mut s, &mut w, 8)? + 1;
    for _ in 0..cb_count {
        let cid = s.read(10)? as usize;
        let cb = lib.get(cid).with_context(|| format!("codebook id {cid} not in library ({})", lib.len()))?;
        rebuild_codebook(&mut BitReader::new(cb), &mut w)?;
    }
    // Time domain transforms: one placeholder.
    w.write(0, 6);
    w.write(0, 16);

    let floors = cp(&mut s, &mut w, 6)? + 1;
    for _ in 0..floors {
        w.write(1, 16);
        let parts = cp(&mut s, &mut w, 5)?;
        let mut class_list = Vec::with_capacity(parts as usize);
        let mut max_class = 0;
        for _ in 0..parts {
            let c = cp(&mut s, &mut w, 4)?;
            max_class = max_class.max(c);
            class_list.push(c);
        }
        let mut dims = Vec::new();
        for _ in 0..=max_class {
            dims.push(cp(&mut s, &mut w, 3)? + 1);
            let subclasses = cp(&mut s, &mut w, 2)?;
            if subclasses != 0 {
                let mb = cp(&mut s, &mut w, 8)?;
                ensure!(mb < cb_count, "floor masterbook out of range");
            }
            for _ in 0..1u32 << subclasses {
                let b = cp(&mut s, &mut w, 8)? as i32 - 1;
                ensure!(b < cb_count as i32, "floor subclass book out of range");
            }
        }
        cp(&mut s, &mut w, 2)?;
        let rangebits = cp(&mut s, &mut w, 4)?;
        for c in class_list {
            for _ in 0..dims[c as usize] {
                cp(&mut s, &mut w, rangebits)?;
            }
        }
    }

    let residues = cp(&mut s, &mut w, 6)? + 1;
    for _ in 0..residues {
        let ty = s.read(2)?;
        ensure!(ty <= 2, "bad residue type");
        w.write(ty, 16);
        cp(&mut s, &mut w, 24)?;
        cp(&mut s, &mut w, 24)?;
        cp(&mut s, &mut w, 24)?;
        let classifications = cp(&mut s, &mut w, 6)? + 1;
        let classbook = cp(&mut s, &mut w, 8)?;
        ensure!(classbook < cb_count, "residue classbook out of range");
        let mut cascade = Vec::new();
        for _ in 0..classifications {
            let low = cp(&mut s, &mut w, 3)?;
            let flag = cp(&mut s, &mut w, 1)?;
            let high = if flag != 0 { cp(&mut s, &mut w, 5)? } else { 0 };
            cascade.push(high * 8 + low);
        }
        for c in cascade {
            for k in 0..8 {
                if c & (1 << k) != 0 {
                    let b = cp(&mut s, &mut w, 8)?;
                    ensure!(b < cb_count, "residue book out of range");
                }
            }
        }
    }

    let mappings = cp(&mut s, &mut w, 6)? + 1;
    for _ in 0..mappings {
        w.write(0, 16);
        let submaps = if cp(&mut s, &mut w, 1)? != 0 { cp(&mut s, &mut w, 4)? + 1 } else { 1 };
        if cp(&mut s, &mut w, 1)? != 0 {
            let steps = cp(&mut s, &mut w, 8)? + 1;
            let bits = ilog(channels - 1);
            for _ in 0..steps {
                let m = cp(&mut s, &mut w, bits)?;
                let a = cp(&mut s, &mut w, bits)?;
                ensure!(m != a && m < channels && a < channels, "bad coupling");
            }
        }
        ensure!(cp(&mut s, &mut w, 2)? == 0, "mapping reserved bits set");
        if submaps > 1 {
            for _ in 0..channels {
                cp(&mut s, &mut w, 4)?;
            }
        }
        for _ in 0..submaps {
            cp(&mut s, &mut w, 8)?;
            let f = cp(&mut s, &mut w, 8)?;
            let r = cp(&mut s, &mut w, 8)?;
            ensure!(f < floors && r < residues, "mapping floor/residue out of range");
        }
    }

    let modes = cp(&mut s, &mut w, 6)? + 1;
    let mut mode_blockflag = Vec::new();
    for _ in 0..modes {
        mode_blockflag.push(cp(&mut s, &mut w, 1)? != 0);
        w.write(0, 16);
        w.write(0, 16);
        ensure!(cp(&mut s, &mut w, 8)? < mappings, "mode mapping out of range");
    }
    w.write(1, 1);

    Ok(Headers {
        ident: id.bytes,
        comment: cm.bytes,
        setup: w.bytes,
        mode_bits: ilog(modes - 1),
        mode_blockflag,
    })
}

/// Iterate the Wwise packets of a data chunk: yields (offset of payload, size).
fn packets(data: &[u8], start: usize, header: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut o = start;
    while o + header <= data.len() {
        let size = u16::from_le_bytes([data[o], data[o + 1]]) as usize;
        let p = o + header;
        if size == 0 || p + size > data.len() {
            break;
        }
        out.push((p, size));
        o = p + size;
    }
    out
}

/// Decode a Wwise Vorbis WEM to interleaved f32 PCM.
pub fn decode(r: &Riff, lib: &CodebookLibrary) -> Result<Pcm> {
    let vi = vorb_info(r)?;
    let data = r.data()?;
    let hdr = if vi.no_granule { 2 } else { 6 };
    let so = vi.setup_offset as usize;
    ensure!(so + hdr <= data.len(), "setup packet offset past data");
    let setup_size = u16::from_le_bytes([data[so], data[so + 1]]) as usize;
    let setup_in = data.get(so + hdr..so + hdr + setup_size).context("setup packet truncated")?;
    let h = build_headers(r, &vi, setup_in, lib)?;

    let ident = lewton::header::read_header_ident(&h.ident).map_err(|e| anyhow::anyhow!("ident: {e:?}"))?;
    let setup = lewton::header::read_header_setup(&h.setup, ident.audio_channels, (ident.blocksize_0, ident.blocksize_1))
        .map_err(|e| anyhow::anyhow!("setup: {e:?}"))?;
    let _ = &h.comment;

    let pk = packets(data, vi.first_audio_offset as usize, hdr);
    let mut pwr = lewton::audio::PreviousWindowRight::new();
    let mut samples = Vec::with_capacity(vi.sample_count as usize * r.channels as usize);
    let mut prev_blockflag = false;
    let mut buf = Vec::new();
    for (i, &(off, size)) in pk.iter().enumerate() {
        let pkt = &data[off..off + size];
        let packet: &[u8] = if vi.mod_packets {
            buf.clear();
            let mut rd = BitReader::new(pkt);
            let mut w = BitWriter::default();
            w.write(0, 1);
            let mode = cp(&mut rd, &mut w, h.mode_bits)? as usize;
            let flag = *h.mode_blockflag.get(mode).context("audio packet mode out of range")?;
            if flag {
                let next = pk
                    .get(i + 1)
                    .and_then(|&(no, _)| data.get(no))
                    .map(|&b| {
                        let nm = (b as u32 & ((1u32 << h.mode_bits) - 1)) as usize;
                        h.mode_blockflag.get(nm).copied().unwrap_or(false)
                    })
                    .unwrap_or(false);
                w.write(prev_blockflag as u32, 1);
                w.write(next as u32, 1);
            }
            prev_blockflag = flag;
            cp(&mut rd, &mut w, 8 - h.mode_bits)?;
            // The rebuilt first byte is 1-3 bits longer, so the rest is shifted.
            w.write_bytes(&pkt[1..]);
            buf.extend_from_slice(&w.bytes);
            &buf
        } else {
            pkt
        };
        match lewton::audio::read_audio_packet_generic::<lewton::samples::InterleavedSamples<f32>>(
            &ident, &setup, packet, &mut pwr,
        ) {
            Ok(s) => samples.extend_from_slice(&s.samples),
            Err(e) => bail!("audio packet {i}: {e:?}"),
        }
    }
    let want = vi.sample_count as usize * r.channels as usize;
    if samples.len() > want && want > 0 {
        samples.truncate(want);
    }
    Ok(Pcm { channels: r.channels, sample_rate: r.sample_rate, samples, loop_range: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_writer_reader_roundtrip() {
        let mut w = BitWriter::default();
        let vals = [(5u32, 3u32), (0x3ff, 10), (1, 1), (0x12345, 20), (0, 7), (0xdead_beef, 32)];
        for (v, n) in vals {
            w.write(v, n);
        }
        w.write_bytes(&[0xa5, 0x5a]);
        let mut r = BitReader::new(&w.bytes);
        for (v, n) in vals {
            assert_eq!(r.read(n).unwrap(), v & if n == 32 { u32::MAX } else { (1 << n) - 1 });
        }
        assert_eq!(r.read(8).unwrap(), 0xa5);
        assert_eq!(r.read(8).unwrap(), 0x5a);
    }

    #[test]
    fn quantvals() {
        assert_eq!(maptype1_quantvals(81, 4), 3);
        assert_eq!(maptype1_quantvals(1024, 2), 32);
        assert_eq!(maptype1_quantvals(9, 1), 9);
    }
}
