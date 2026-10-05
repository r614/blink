//! `package_inflate <in.upk> <out.upk>`: write a package with its LZO chunks inflated in place and
//! the compression flag cleared, for third-party readers that cannot decompress (e.g. UE
//! Explorer's UnrealScript decompiler). The output is retail data: write it outside the repo
//! (scratch space) and never commit it.
use anyhow::{Context, Result, bail};
use ue3_package::Package;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (src, dst) = (a.first().context("in.upk")?, a.get(1).context("out.upk")?);
    let pkg = Package::open(src)?;
    let s = &pkg.summary;
    let Some(first) = s.compressed_chunks.first() else {
        std::fs::copy(src, dst)?;
        println!("{src}: not compressed, copied");
        return Ok(());
    };
    // The header keeps `CompressionFlags, ChunkCount, chunks...`; find that run in the header
    // (it is unique: flags, count, then the first chunk's four fields), clear the flags, and
    // leave the table in place so no offset moves (readers skip it when the flags are 0).
    let mut data = pkg.data.clone();
    let mut want = Vec::new();
    for v in [s.compression_flags, s.compressed_chunks.len() as u32, first.uncompressed_offset, first.uncompressed_size, first.compressed_offset, first.compressed_size] {
        want.extend_from_slice(&v.to_le_bytes());
    }
    let limit = (first.uncompressed_offset as usize).min(data.len());
    let hits: Vec<usize> = (0..limit.saturating_sub(want.len())).filter(|&i| data[i..i + want.len()] == want[..]).collect();
    if hits.len() != 1 {
        bail!("compression header not found uniquely ({} hits)", hits.len());
    }
    data[hits[0]..hits[0] + 4].copy_from_slice(&0u32.to_le_bytes());
    std::fs::write(dst, &data)?;
    println!("{src}: {} chunks inflated, {} bytes, flags cleared at {:#x}", s.compressed_chunks.len(), data.len(), hits[0]);
    Ok(())
}
