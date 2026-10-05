//! Install-wide event index: which bank package defines each event id.
//!
//! Built by reading only the BKHD/HIRC sections of every bank (DATA is skipped),
//! so it is cheap enough to build at startup on a worker thread.

use crate::bank::rd32;
use crate::library::pck_dirs;
use crate::pck::Pck;
use anyhow::Result;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone)]
pub struct EventIndex {
    /// Event id -> bank package name (without extension).
    pub events: HashMap<u32, String>,
    /// Bank package name -> path.
    pub packages: HashMap<String, PathBuf>,
}

impl EventIndex {
    pub fn build(root: &Path) -> Result<Self> {
        let mut idx = EventIndex::default();
        for d in pck_dirs(root) {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pck")) {
                    continue;
                }
                let name = p.file_stem().unwrap().to_string_lossy().into_owned();
                if let Ok(ids) = event_ids(&p) {
                    for id in ids {
                        idx.events.entry(id).or_insert_with(|| name.clone());
                    }
                }
                idx.packages.insert(name.to_ascii_lowercase(), p);
            }
        }
        Ok(idx)
    }

    pub fn bank_of(&self, event: &str) -> Option<&str> {
        self.events.get(&crate::fnv1_32(event)).map(String::as_str)
    }
}

/// Event ids defined by the soundbank(s) of one package.
pub fn event_ids(path: &Path) -> Result<Vec<u32>> {
    let pck = Pck::open(path)?;
    let mut f = File::open(path)?;
    let mut out = Vec::new();
    for be in &pck.banks {
        let mut o = 0u64;
        while o + 8 <= be.size as u64 {
            f.seek(SeekFrom::Start(be.offset + o))?;
            let mut h = [0u8; 8];
            f.read_exact(&mut h)?;
            let size = rd32(&h, 4)? as u64;
            if &h[..4] == b"HIRC" {
                let mut body = vec![0u8; size as usize];
                f.read_exact(&mut body)?;
                let n = rd32(&body, 0)? as usize;
                let mut p = 4;
                for _ in 0..n {
                    let Some(&kind) = body.get(p) else { break };
                    let sz = rd32(&body, p + 1)? as usize;
                    if kind == crate::hirc::HIRC_EVENT {
                        out.push(rd32(&body, p + 5)?);
                    }
                    p += 5 + sz;
                }
            }
            o += 8 + size;
        }
    }
    Ok(out)
}
