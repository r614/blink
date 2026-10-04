//! `UTexture2D` decoding: inline mips and mips streamed from `.tfc` caches.
//!
//! Native layout after tagged properties (file version 801):
//! `SourceArt: FByteBulkData`, `Mips: TArray<{Data: FByteBulkData, SizeX:i32, SizeY:i32}>`,
//! `TextureFileCacheGuid: FGuid`, then optional extra mip arrays (ignored).

use crate::content::Content;
use anyhow::{Context, Result, bail};
use dishonored_ir::{TextureAsset, TextureFormat};
use ue3_package::compression::{COMPRESS_LZO, decompress_chunk};
use ue3_package::properties::{Value, find};
use ue3_package::reader::Reader;
use ue3_package::{ObjRef, Package};

pub const BULK_SEPARATE_FILE: u32 = 0x01;
pub const BULK_COMPRESSED_ZLIB: u32 = 0x02;
pub const BULK_COMPRESSED_LZO: u32 = 0x10;
pub const BULK_UNUSED: u32 = 0x20;
pub const BULK_SEPARATE_DATA: u32 = 0x40;

#[derive(Debug, Clone, Copy)]
pub struct BulkHeader {
    pub flags: u32,
    pub element_count: i32,
    pub size_on_disk: i32,
    pub offset_in_file: i32,
    /// Position of the inline payload inside the export data.
    pub inline_at: usize,
}

pub fn read_bulk_header(r: &mut Reader) -> Result<BulkHeader> {
    let flags = r.u32()?;
    let element_count = r.i32()?;
    let size_on_disk = r.i32()?;
    let offset_in_file = r.i32()?;
    let inline_at = r.pos();
    if flags & (BULK_SEPARATE_FILE | BULK_UNUSED | BULK_SEPARATE_DATA) == 0 && size_on_disk > 0 {
        r.skip(size_on_disk as usize)?;
    }
    Ok(BulkHeader {
        flags,
        element_count,
        size_on_disk,
        offset_in_file,
        inline_at,
    })
}

fn decode_payload(flags: u32, bytes: &[u8], expected: usize) -> Result<Vec<u8>> {
    if flags & BULK_COMPRESSED_LZO != 0 {
        let mut r = Reader::new(bytes);
        decompress_chunk(&mut r, COMPRESS_LZO, Some(expected))
    } else if flags & BULK_COMPRESSED_ZLIB != 0 {
        bail!("zlib bulk data not supported")
    } else {
        Ok(bytes[..expected.min(bytes.len())].to_vec())
    }
}

fn map_format(name: &str) -> Option<TextureFormat> {
    Some(match name {
        "PF_DXT1" => TextureFormat::Bc1,
        "PF_DXT3" => TextureFormat::Bc2,
        "PF_DXT5" => TextureFormat::Bc3,
        "PF_BC5" | "PF_ATI2" => TextureFormat::Bc5,
        "PF_A8R8G8B8" => TextureFormat::Bgra8,
        "PF_G8" | "PF_L8" => TextureFormat::R8,
        "PF_V8U8" => TextureFormat::Rg8Snorm,
        _ => return None,
    })
}

/// Decode a Texture2D export, keeping mips whose largest side is <= `max_size`.
pub fn read_texture(
    content: &Content,
    pkg: &Package,
    export: usize,
    max_size: u32,
) -> Result<TextureAsset> {
    let path = pkg.obj_path(ObjRef::Export(export));
    let hdr = pkg.read_object(export)?;
    let props = &hdr.properties;
    let fmt_name = match find(props, "Format").map(|p| &p.value) {
        Some(Value::Enum(s)) => s.clone(),
        Some(Value::Byte(b)) => format!("byte:{b}"),
        _ => "PF_DXT1".into(), // class default
    };
    let format = map_format(&fmt_name)
        .with_context(|| format!("{path}: unsupported pixel format {fmt_name}"))?;
    let srgb = !matches!(find(props, "SRGB").map(|p| &p.value), Some(Value::Bool(false)));
    let tfc = match find(props, "TextureFileCacheName").map(|p| &p.value) {
        Some(Value::Name(n)) if n != "None" => Some(n.clone()),
        _ => None,
    };

    let data = pkg.export_data(export)?;
    let mut r = Reader::at(data, hdr.native_offset);
    let _source_art = read_bulk_header(&mut r)?;
    let nmips = r.count(24)?;
    let mut mips = Vec::with_capacity(nmips);
    for _ in 0..nmips {
        let b = read_bulk_header(&mut r)?;
        let w = r.i32()? as u32;
        let h = r.i32()? as u32;
        mips.push((b, w, h));
    }

    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut size: Option<(u32, u32)> = None;
    for (b, w, h) in mips {
        if b.flags & BULK_UNUSED != 0 || b.element_count <= 0 {
            continue;
        }
        if w.max(h) > max_size {
            continue;
        }
        if format.is_block() && (w < 4 || h < 4) {
            break;
        }
        if let Some((pw, ph)) = size {
            // Keep a contiguous chain.
            let n = out.len() as u32;
            if (w, h) != ((pw >> n).max(1), (ph >> n).max(1)) {
                break;
            }
        }
        let expected = format.mip_size(w, h);
        let bytes = if b.flags & BULK_SEPARATE_FILE != 0 {
            let name = tfc
                .as_deref()
                .with_context(|| format!("{path}: separate-file mip without TextureFileCacheName"))?;
            let raw = content.read_tfc(name, b.offset_in_file as u64, b.size_on_disk as usize)?;
            decode_payload(b.flags, &raw, expected)?
        } else if b.flags & BULK_SEPARATE_DATA != 0 {
            let start = b.offset_in_file as usize;
            let raw = pkg
                .data
                .get(start..start + b.size_on_disk as usize)
                .context("separate-data mip out of range")?;
            decode_payload(b.flags, raw, expected)?
        } else {
            let raw = &data[b.inline_at..b.inline_at + b.size_on_disk as usize];
            decode_payload(b.flags, raw, expected)?
        };
        if bytes.len() < expected {
            bail!("{path}: mip {w}x{h} has {} bytes, expected {expected}", bytes.len());
        }
        if size.is_none() {
            size = Some((w, h));
        }
        out.push(bytes);
    }
    let (width, height) = size.with_context(|| format!("{path}: no usable mips"))?;
    Ok(TextureAsset {
        source: path,
        format,
        width,
        height,
        mips: out,
        srgb,
    })
}

/// Decode one mip of a texture to RGBA8 (stored values; no colour-space conversion).
/// Supports the formats seen in level content: BC1, BC2 (colour only), BC3, BGRA8, G8, V8U8.
pub fn decode_rgba8(t: &TextureAsset, mip: usize) -> Option<(u32, u32, Vec<[u8; 4]>)> {
    let data = t.mips.get(mip)?;
    let w = (t.width >> mip).max(1);
    let h = (t.height >> mip).max(1);
    let n = (w * h) as usize;
    let px = match t.format {
        TextureFormat::Bc1 => decode_bc_blocks(data, w, h, 8, false),
        TextureFormat::Bc2 => decode_bc_blocks(data, w, h, 16, false),
        TextureFormat::Bc3 => decode_bc_blocks(data, w, h, 16, true),
        TextureFormat::Bgra8 => data.chunks_exact(4).take(n).map(|c| [c[2], c[1], c[0], c[3]]).collect(),
        TextureFormat::R8 => data.iter().take(n).map(|&v| [v, v, v, 255]).collect(),
        TextureFormat::Rg8Snorm => data
            .chunks_exact(2)
            .take(n)
            .map(|c| [(c[0] as i8 as i16 + 128) as u8, (c[1] as i8 as i16 + 128) as u8, 255, 255])
            .collect(),
        TextureFormat::Bc5 => return None,
    };
    Some((w, h, px))
}

fn bc_color_block(block: &[u8], out: &mut [[u8; 4]; 16], force_four: bool) {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let expand = |c: u16| -> [u32; 3] {
        let r = ((c >> 11) & 31) as u32;
        let g = ((c >> 5) & 63) as u32;
        let b = (c & 31) as u32;
        [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
    };
    let (a, b) = (expand(c0), expand(c1));
    let mut pal = [[0u32; 4]; 4];
    pal[0] = [a[0], a[1], a[2], 255];
    pal[1] = [b[0], b[1], b[2], 255];
    for k in 0..3 {
        if c0 > c1 || force_four {
            pal[2][k] = (2 * a[k] + b[k]) / 3;
            pal[3][k] = (a[k] + 2 * b[k]) / 3;
        } else {
            pal[2][k] = (a[k] + b[k]) / 2;
            pal[3][k] = 0;
        }
    }
    pal[2][3] = 255;
    pal[3][3] = if c0 > c1 || force_four { 255 } else { 0 };
    let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
    for (i, o) in out.iter_mut().enumerate() {
        let c = pal[((bits >> (2 * i)) & 3) as usize];
        *o = [c[0] as u8, c[1] as u8, c[2] as u8, c[3] as u8];
    }
}

fn bc3_alpha_block(block: &[u8], out: &mut [[u8; 4]; 16]) {
    let (a0, a1) = (block[0] as u32, block[1] as u32);
    let mut pal = [0u32; 8];
    pal[0] = a0;
    pal[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            pal[i + 1] = ((7 - i as u32) * a0 + i as u32 * a1) / 7;
        }
    } else {
        for i in 1..5 {
            pal[i + 1] = ((5 - i as u32) * a0 + i as u32 * a1) / 5;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (i, b) in block[2..8].iter().enumerate() {
        bits |= (*b as u64) << (8 * i);
    }
    for (i, o) in out.iter_mut().enumerate() {
        o[3] = pal[((bits >> (3 * i)) & 7) as usize] as u8;
    }
}

fn decode_bc_blocks(data: &[u8], w: u32, h: u32, block_bytes: usize, bc3: bool) -> Vec<[u8; 4]> {
    let (w, h) = (w as usize, h as usize);
    let bw = w.div_ceil(4);
    let mut out = vec![[0u8, 0, 0, 255]; w * h];
    let mut px = [[0u8; 4]; 16];
    for (bi, block) in data.chunks_exact(block_bytes).enumerate() {
        let (bx, by) = (bi % bw, bi / bw);
        if by * 4 >= h {
            break;
        }
        let color = &block[block_bytes - 8..];
        bc_color_block(color, &mut px, block_bytes == 16);
        if bc3 {
            bc3_alpha_block(&block[..8], &mut px);
        } else if block_bytes == 16 {
            for (i, o) in px.iter_mut().enumerate() {
                let nib = (block[i / 2] >> (4 * (i % 2))) & 15;
                o[3] = nib * 17;
            }
        }
        for py in 0..4 {
            for pxx in 0..4 {
                let (x, y) = (bx * 4 + pxx, by * 4 + py);
                if x < w && y < h {
                    out[y * w + x] = px[py * 4 + pxx];
                }
            }
        }
    }
    out
}
