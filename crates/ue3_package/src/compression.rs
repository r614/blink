//! Chunked package / bulk-data decompression.
//!
//! A compressed chunk on disk is an `FCompressedChunkHeader`:
//! `tag:u32 (0x9E2A83C1)`, `block_size:u32`, `summary {compressed:u32, uncompressed:u32}`,
//! then one `{compressed:u32, uncompressed:u32}` pair per block, then the block payloads.

use crate::reader::Reader;
use anyhow::{Context, Result, bail, ensure};

pub const PACKAGE_TAG: u32 = 0x9E2A_83C1;

pub const COMPRESS_ZLIB: u32 = 1;
pub const COMPRESS_LZO: u32 = 2;
pub const COMPRESS_LZX: u32 = 4;

/// Decompress one `FCompressedChunkHeader`-framed chunk starting at `r.pos()`.
/// Returns the decompressed bytes; `r` is left after the last block.
pub fn decompress_chunk(r: &mut Reader, flags: u32, expected_size: Option<usize>) -> Result<Vec<u8>> {
    let start = r.pos();
    let tag = r.u32()?;
    ensure!(
        tag == PACKAGE_TAG,
        "bad compressed chunk tag {tag:#x} at {start:#x}"
    );
    let mut block_size = r.u32()? as usize;
    if block_size == PACKAGE_TAG as usize {
        block_size = 0x20000;
    }
    let total_compressed = r.u32()? as usize;
    let total_uncompressed = r.u32()? as usize;
    if let Some(exp) = expected_size {
        ensure!(
            exp == total_uncompressed,
            "chunk at {start:#x}: header says {total_uncompressed} bytes, expected {exp}"
        );
    }
    let nblocks = total_uncompressed.div_ceil(block_size.max(1));
    let mut blocks = Vec::with_capacity(nblocks);
    for _ in 0..nblocks {
        blocks.push((r.u32()? as usize, r.u32()? as usize));
    }
    let mut out = Vec::with_capacity(total_uncompressed);
    let mut consumed = 0usize;
    for (i, (csize, usize_)) in blocks.into_iter().enumerate() {
        let src = r.bytes(csize)?;
        consumed += csize;
        let before = out.len();
        out.resize(before + usize_, 0);
        decompress_block(flags, src, &mut out[before..])
            .with_context(|| format!("block {i} of chunk at {start:#x}"))?;
    }
    ensure!(
        consumed == total_compressed,
        "chunk at {start:#x}: consumed {consumed} compressed bytes, header says {total_compressed}"
    );
    Ok(out)
}

pub fn decompress_block(flags: u32, src: &[u8], dst: &mut [u8]) -> Result<()> {
    match flags & 0xFF {
        COMPRESS_LZO => {
            let v = lzokay_native::decompress_all(src, Some(dst.len()))
                .map_err(|e| anyhow::anyhow!("LZO: {e:?}"))?;
            ensure!(
                v.len() == dst.len(),
                "LZO produced {} bytes, expected {}",
                v.len(),
                dst.len()
            );
            dst.copy_from_slice(&v);
            Ok(())
        }
        other => bail!("unsupported compression method {other:#x}"),
    }
}
