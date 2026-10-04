//! Minimal Unreal Engine 3 package reader for Dishonored (GOG Definitive Edition).
//!
//! Scope is deliberately narrow: file version 801 / licensee 30 as shipped by
//! Dishonored. Layout notes and evidence live in `research/dishonored/PACKAGE_FORMAT.md`.

pub mod compression;
pub mod properties;
pub mod reader;

use anyhow::{Context, Result, bail, ensure};
use compression::{PACKAGE_TAG, decompress_chunk};
use reader::Reader;
use std::path::Path;

pub const DISHONORED_FILE_VERSION: u16 = 801;
pub const DISHONORED_LICENSEE_VERSION: u16 = 30;

#[derive(Debug, Clone, Copy)]
pub struct CompressedChunk {
    pub uncompressed_offset: u32,
    pub uncompressed_size: u32,
    pub compressed_offset: u32,
    pub compressed_size: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct Generation {
    pub export_count: i32,
    pub name_count: i32,
    pub net_object_count: i32,
}

#[derive(Debug, Clone)]
pub struct Summary {
    pub file_version: u16,
    pub licensee_version: u16,
    pub header_size: i32,
    pub folder_name: String,
    pub package_flags: u32,
    pub name_count: i32,
    pub name_offset: i32,
    pub export_count: i32,
    pub export_offset: i32,
    pub import_count: i32,
    pub import_offset: i32,
    pub depends_offset: i32,
    pub import_export_guids_offset: i32,
    pub import_guids_count: i32,
    pub export_guids_count: i32,
    pub thumbnail_table_offset: i32,
    pub guid: [u32; 4],
    pub generations: Vec<Generation>,
    pub engine_version: i32,
    pub cooker_version: i32,
    pub compression_flags: u32,
    pub compressed_chunks: Vec<CompressedChunk>,
    pub package_source: u32,
    pub additional_packages_to_cook: Vec<String>,
    /// Offset of the first byte after the parsed summary fields.
    pub end_offset: usize,
}

impl Summary {
    pub fn read(r: &mut Reader) -> Result<Self> {
        let tag = r.u32()?;
        ensure!(tag == PACKAGE_TAG, "not a UE3 package (tag {tag:#x})");
        let file_version = r.u16()?;
        let licensee_version = r.u16()?;
        let header_size = r.i32()?;
        let folder_name = r.fstring()?;
        let package_flags = r.u32()?;
        let name_count = r.i32()?;
        let name_offset = r.i32()?;
        let export_count = r.i32()?;
        let export_offset = r.i32()?;
        let import_count = r.i32()?;
        let import_offset = r.i32()?;
        let depends_offset = r.i32()?;
        let import_export_guids_offset = r.i32()?;
        let import_guids_count = r.i32()?;
        let export_guids_count = r.i32()?;
        let thumbnail_table_offset = r.i32()?;
        let guid = r.guid()?;
        let ngen = r.count(12)?;
        let mut generations = Vec::with_capacity(ngen);
        for _ in 0..ngen {
            generations.push(Generation {
                export_count: r.i32()?,
                name_count: r.i32()?,
                net_object_count: r.i32()?,
            });
        }
        let engine_version = r.i32()?;
        let cooker_version = r.i32()?;
        let compression_flags = r.u32()?;
        let nchunks = r.count(16)?;
        let mut compressed_chunks = Vec::with_capacity(nchunks);
        for _ in 0..nchunks {
            compressed_chunks.push(CompressedChunk {
                uncompressed_offset: r.u32()?,
                uncompressed_size: r.u32()?,
                compressed_offset: r.u32()?,
                compressed_size: r.u32()?,
            });
        }
        let package_source = r.u32()?;
        let npkg = r.count(4)?;
        let mut additional_packages_to_cook = Vec::with_capacity(npkg);
        for _ in 0..npkg {
            additional_packages_to_cook.push(r.fstring()?);
        }
        Ok(Self {
            file_version,
            licensee_version,
            header_size,
            folder_name,
            package_flags,
            name_count,
            name_offset,
            export_count,
            export_offset,
            import_count,
            import_offset,
            depends_offset,
            import_export_guids_offset,
            import_guids_count,
            export_guids_count,
            thumbnail_table_offset,
            guid,
            generations,
            engine_version,
            cooker_version,
            compression_flags,
            compressed_chunks,
            package_source,
            additional_packages_to_cook,
            end_offset: r.pos(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct NameEntry {
    pub name: String,
    pub flags: u64,
}

/// `FName` reference: index into the name table plus instance number
/// (`number == 0` means no suffix, otherwise `_{number-1}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FName {
    pub index: i32,
    pub number: i32,
}

impl FName {
    pub fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            index: r.i32()?,
            number: r.i32()?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Import {
    pub class_package: FName,
    pub class_name: FName,
    pub outer: i32,
    pub object_name: FName,
}

#[derive(Debug, Clone)]
pub struct Export {
    pub class: i32,
    pub super_: i32,
    pub outer: i32,
    pub object_name: FName,
    pub archetype: i32,
    pub object_flags: u64,
    pub serial_size: i32,
    pub serial_offset: i32,
    pub export_flags: u32,
    pub generation_net_object_count: Vec<i32>,
    pub package_guid: [u32; 4],
    pub package_flags: u32,
}

/// A fully decompressed package held in memory.
pub struct Package {
    pub name: String,
    pub summary: Summary,
    pub names: Vec<NameEntry>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
    /// Decompressed package bytes, addressed by the summary's (uncompressed) offsets.
    pub data: Vec<u8>,
}

/// Reference to an object from inside a package: 0 = null, >0 = export (1-based),
/// <0 = import (-1-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjRef {
    Null,
    Export(usize),
    Import(usize),
}

impl ObjRef {
    pub fn from_index(i: i32) -> Self {
        match i {
            0 => ObjRef::Null,
            i if i > 0 => ObjRef::Export(i as usize - 1),
            i => ObjRef::Import((-i) as usize - 1),
        }
    }
}

impl Package {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::from_bytes(name, raw).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn from_bytes(name: String, raw: Vec<u8>) -> Result<Self> {
        let summary = Summary::read(&mut Reader::new(&raw))?;
        if summary.file_version != DISHONORED_FILE_VERSION
            || summary.licensee_version != DISHONORED_LICENSEE_VERSION
        {
            bail!(
                "unsupported package version {}/{} (expected {}/{})",
                summary.file_version,
                summary.licensee_version,
                DISHONORED_FILE_VERSION,
                DISHONORED_LICENSEE_VERSION
            );
        }
        let data = if summary.compressed_chunks.is_empty() {
            raw
        } else {
            decompress_package(&raw, &summary)?
        };
        let mut pkg = Package {
            name,
            summary,
            names: Vec::new(),
            imports: Vec::new(),
            exports: Vec::new(),
            data,
        };
        pkg.read_tables()?;
        Ok(pkg)
    }

    fn read_tables(&mut self) -> Result<()> {
        let s = &self.summary;
        let mut r = Reader::at(&self.data, s.name_offset as usize);
        self.names.reserve(s.name_count as usize);
        for i in 0..s.name_count {
            let name = r.fstring().with_context(|| format!("name {i}"))?;
            let flags = r.u64()?;
            self.names.push(NameEntry { name, flags });
        }

        r.seek(s.import_offset as usize)?;
        for i in 0..s.import_count {
            let imp = (|| -> Result<Import> {
                Ok(Import {
                    class_package: FName::read(&mut r)?,
                    class_name: FName::read(&mut r)?,
                    outer: r.i32()?,
                    object_name: FName::read(&mut r)?,
                })
            })()
            .with_context(|| format!("import {i}"))?;
            self.imports.push(imp);
        }

        r.seek(s.export_offset as usize)?;
        for i in 0..s.export_count {
            let exp = read_export(&mut r).with_context(|| format!("export {i}"))?;
            self.exports.push(exp);
        }
        // Evidence that the export layout is right: the table ends exactly where the
        // depends map begins (verified across every package in the GOG install).
        ensure!(
            r.pos() == s.depends_offset as usize,
            "export table ended at {:#x}, depends map starts at {:#x}",
            r.pos(),
            s.depends_offset
        );
        Ok(())
    }

    pub fn name(&self, n: FName) -> String {
        let base = self
            .names
            .get(n.index as usize)
            .map(|e| e.name.as_str())
            .unwrap_or("<bad-name>");
        if n.number == 0 {
            base.to_string()
        } else {
            format!("{base}_{}", n.number - 1)
        }
    }

    pub fn name_str(&self, n: FName) -> &str {
        self.names
            .get(n.index as usize)
            .map(|e| e.name.as_str())
            .unwrap_or("<bad-name>")
    }

    pub fn find_name(&self, s: &str) -> Option<i32> {
        self.names
            .iter()
            .position(|e| e.name.eq_ignore_ascii_case(s))
            .map(|i| i as i32)
    }

    pub fn obj_name(&self, r: ObjRef) -> String {
        match r {
            ObjRef::Null => "None".into(),
            ObjRef::Export(i) => self
                .exports
                .get(i)
                .map(|e| self.name(e.object_name))
                .unwrap_or_else(|| format!("<bad-export {i}>")),
            ObjRef::Import(i) => self
                .imports
                .get(i)
                .map(|e| self.name(e.object_name))
                .unwrap_or_else(|| format!("<bad-import {i}>")),
        }
    }

    pub fn obj_outer(&self, r: ObjRef) -> ObjRef {
        match r {
            ObjRef::Null => ObjRef::Null,
            ObjRef::Export(i) => self
                .exports
                .get(i)
                .map(|e| ObjRef::from_index(e.outer))
                .unwrap_or(ObjRef::Null),
            ObjRef::Import(i) => self
                .imports
                .get(i)
                .map(|e| ObjRef::from_index(e.outer))
                .unwrap_or(ObjRef::Null),
        }
    }

    /// Dotted path from the outermost object, e.g. `TheWorld.PersistentLevel.StaticMeshActor_3`.
    pub fn obj_path(&self, r: ObjRef) -> String {
        let mut parts = Vec::new();
        let mut cur = r;
        let mut guard = 0;
        while cur != ObjRef::Null && guard < 64 {
            parts.push(self.obj_name(cur));
            cur = self.obj_outer(cur);
            guard += 1;
        }
        parts.reverse();
        parts.join(".")
    }

    /// Class name of an object (`Class` for exports with class 0).
    pub fn class_name(&self, r: ObjRef) -> String {
        match r {
            ObjRef::Null => "None".into(),
            ObjRef::Export(i) => {
                let c = self.exports[i].class;
                if c == 0 {
                    "Class".into()
                } else {
                    self.obj_name(ObjRef::from_index(c))
                }
            }
            ObjRef::Import(i) => self.name(self.imports[i].class_name),
        }
    }

    pub fn export_class(&self, i: usize) -> String {
        self.class_name(ObjRef::Export(i))
    }

    /// Serialized bytes of an export.
    pub fn export_data(&self, i: usize) -> Result<&[u8]> {
        let e = &self.exports[i];
        let start = e.serial_offset as usize;
        let end = start + e.serial_size as usize;
        ensure!(
            end <= self.data.len(),
            "export {i} data {start:#x}..{end:#x} out of range"
        );
        Ok(&self.data[start..end])
    }

    /// Package name of an import (outermost outer), used to resolve cross-package references.
    pub fn import_package(&self, i: usize) -> String {
        let mut cur = ObjRef::Import(i);
        loop {
            let o = self.obj_outer(cur);
            if o == ObjRef::Null {
                return self.obj_name(cur);
            }
            cur = o;
        }
    }
}

fn read_export(r: &mut Reader) -> Result<Export> {
    let class = r.i32()?;
    let super_ = r.i32()?;
    let outer = r.i32()?;
    let object_name = FName::read(r)?;
    let archetype = r.i32()?;
    // 64-bit EObjectFlags are stored as two dwords, high half first.
    let flags_hi = r.u32()? as u64;
    let flags_lo = r.u32()? as u64;
    let object_flags = flags_hi << 32 | flags_lo;
    let serial_size = r.i32()?;
    let serial_offset = if serial_size > 0 { r.i32()? } else { r.i32()? };
    let export_flags = r.u32()?;
    let n = r.count(4)?;
    let mut generation_net_object_count = Vec::with_capacity(n);
    for _ in 0..n {
        generation_net_object_count.push(r.i32()?);
    }
    let package_guid = r.guid()?;
    let package_flags = r.u32()?;
    Ok(Export {
        class,
        super_,
        outer,
        object_name,
        archetype,
        object_flags,
        serial_size,
        serial_offset,
        export_flags,
        generation_net_object_count,
        package_guid,
        package_flags,
    })
}

/// Rebuild the uncompressed package image: the header up to the first chunk's
/// uncompressed offset is stored raw, then each chunk is inflated in place.
fn decompress_package(raw: &[u8], s: &Summary) -> Result<Vec<u8>> {
    let total = s
        .compressed_chunks
        .iter()
        .map(|c| (c.uncompressed_offset + c.uncompressed_size) as usize)
        .max()
        .unwrap_or(raw.len());
    let first = s
        .compressed_chunks
        .iter()
        .map(|c| c.uncompressed_offset as usize)
        .min()
        .unwrap_or(0);
    let mut out = vec![0u8; total];
    out[..first].copy_from_slice(&raw[..first]);
    for (i, c) in s.compressed_chunks.iter().enumerate() {
        let mut r = Reader::at(raw, c.compressed_offset as usize);
        let bytes = decompress_chunk(
            &mut r,
            s.compression_flags,
            Some(c.uncompressed_size as usize),
        )
        .with_context(|| format!("package chunk {i}"))?;
        let at = c.uncompressed_offset as usize;
        out[at..at + bytes.len()].copy_from_slice(&bytes);
    }
    Ok(out)
}
