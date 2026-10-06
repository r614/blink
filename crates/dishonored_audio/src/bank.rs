//! Wwise SoundBank (`.bnk`) sections: BKHD header, DIDX/DATA embedded media,
//! HIRC hierarchy objects and the optional STID bank-name table.

use anyhow::{Context, Result, ensure};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
pub struct MediaRef {
    pub id: u32,
    /// Offset into the DATA payload.
    pub offset: u32,
    pub size: u32,
}

/// One raw HIRC object: type byte, id and body (the bytes after the id).
#[derive(Debug, Clone)]
pub struct HircObject {
    pub kind: u8,
    pub id: u32,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Bank {
    pub version: u32,
    pub id: u32,
    pub sections: Vec<([u8; 4], usize)>,
    pub media: Vec<MediaRef>,
    pub data: Vec<u8>,
    pub hirc: Vec<HircObject>,
    /// STID: bank id -> bank name.
    pub bank_names: HashMap<u32, String>,
    /// STMG section (Init.bnk: state groups, switch groups, game parameters).
    pub stmg: Vec<u8>,
    /// ENVS section (Init.bnk: obstruction / occlusion curves).
    pub envs: Vec<u8>,
}

pub(crate) fn rd32(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .with_context(|| format!("truncated read at {o}"))
}

impl Bank {
    pub fn parse(b: &[u8]) -> Result<Self> {
        let mut bank = Bank::default();
        let mut o = 0usize;
        while o + 8 <= b.len() {
            let tag: [u8; 4] = b[o..o + 4].try_into().unwrap();
            let size = rd32(b, o + 4)? as usize;
            let body = b.get(o + 8..o + 8 + size).context("section past end of bank")?;
            bank.sections.push((tag, size));
            match &tag {
                b"BKHD" => {
                    bank.version = rd32(body, 0)?;
                    bank.id = rd32(body, 4)?;
                }
                b"DIDX" => {
                    for e in body.chunks_exact(12) {
                        bank.media.push(MediaRef {
                            id: rd32(e, 0)?,
                            offset: rd32(e, 4)?,
                            size: rd32(e, 8)?,
                        });
                    }
                }
                b"DATA" => bank.data = body.to_vec(),
                b"STMG" => bank.stmg = body.to_vec(),
                b"ENVS" => bank.envs = body.to_vec(),
                b"HIRC" => {
                    let n = rd32(body, 0)? as usize;
                    let mut p = 4;
                    for _ in 0..n {
                        let kind = *body.get(p).context("HIRC truncated")?;
                        let sz = rd32(body, p + 1)? as usize;
                        ensure!(sz >= 4, "HIRC object too small");
                        let id = rd32(body, p + 5)?;
                        let ob = body.get(p + 9..p + 5 + sz).context("HIRC object past end")?;
                        bank.hirc.push(HircObject { kind, id, body: ob.to_vec() });
                        p += 5 + sz;
                    }
                }
                b"STID" => {
                    let n = rd32(body, 4)? as usize;
                    let mut p = 8;
                    for _ in 0..n {
                        let id = rd32(body, p)?;
                        let len = *body.get(p + 4).context("STID truncated")? as usize;
                        let s = body.get(p + 5..p + 5 + len).context("STID truncated")?;
                        bank.bank_names.insert(id, String::from_utf8_lossy(s).into_owned());
                        p += 5 + len;
                    }
                }
                _ => {}
            }
            o += 8 + size;
        }
        ensure!(bank.version != 0, "no BKHD section");
        Ok(bank)
    }

    pub fn media_bytes(&self, m: &MediaRef) -> Option<&[u8]> {
        self.data.get(m.offset as usize..(m.offset + m.size) as usize)
    }

    pub fn media_by_id(&self, id: u32) -> Option<&[u8]> {
        self.media.iter().find(|m| m.id == id).and_then(|m| self.media_bytes(m))
    }
}
