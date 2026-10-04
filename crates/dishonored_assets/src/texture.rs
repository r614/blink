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
