//! UE3 bitmap fonts (`Engine.Font`), as the loading screen and other canvas UI draw text.
//!
//! Tagged properties: `Characters` (array of the native `FontCharacter` struct, 21 bytes each:
//! `StartU, StartV, USize, VSize` i32, `TextureIndex` u8, `VerticalOffset` i32), `Textures`
//! (glyph pages), `IsRemapped`, `EmScale`, `Ascent`, `Descent`, `Leading`, `Kerning`. After the
//! properties the native serialization holds `CharRemap` (`TMap<WORD, WORD>`: unicode -> index
//! into `Characters`, used when `IsRemapped`; otherwise the character code is the index).

use crate::content::Content;
use crate::texture::read_texture;
use anyhow::{Context, Result};
use dishonored_ir::TextureAsset;
use std::collections::HashMap;
use ue3_package::ObjRef;
use ue3_package::properties::{Value, find};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    pub u: i32,
    pub v: i32,
    pub w: i32,
    pub h: i32,
    pub page: u8,
    pub y_offset: i32,
}

#[derive(Debug, Clone)]
pub struct BitmapFont {
    pub name: String,
    pub glyphs: Vec<Glyph>,
    pub pages: Vec<TextureAsset>,
    pub remap: Option<HashMap<u16, u16>>,
    pub kerning: i32,
    /// Tallest glyph (the canvas line height, UE3 `Font::GetMaxCharHeight`).
    pub line_height: i32,
}

impl BitmapFont {
    /// Glyph for a character (`IsRemapped`: through `CharRemap`, else the code is the index).
    pub fn glyph(&self, c: char) -> Option<&Glyph> {
        let code = c as u32;
        let i = match &self.remap {
            Some(m) => *m.get(&(code as u16))? as usize,
            None => code as usize,
        };
        self.glyphs.get(i).filter(|g| g.w > 0 || c == ' ')
    }

    /// Unscaled width of a line (UE3 canvas: advance = glyph width + kerning).
    pub fn width(&self, text: &str) -> i32 {
        text.chars().filter_map(|c| self.glyph(c)).map(|g| g.w + self.kerning).sum()
    }
}

fn i32_at(b: &[u8], o: usize) -> Option<i32> {
    b.get(o..o + 4).map(|x| i32::from_le_bytes([x[0], x[1], x[2], x[3]]))
}

/// Load a font by object path (e.g. `UI_Loading.Emerge_BF`) from a package.
pub fn load_font(content: &Content, package: &str, path: &str) -> Result<BitmapFont> {
    let pkg = content.package(package)?;
    let idx = (0..pkg.exports.len())
        .find(|&i| pkg.export_class(i) == "Font" && pkg.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(path))
        .with_context(|| format!("font {path} not in {package}"))?;
    let h = pkg.read_object(idx)?;
    let mut glyphs = Vec::new();
    if let Some((n, raw)) = find(&h.properties, "Characters").and_then(|p| p.array_raw()) {
        for k in 0..n {
            let o = k * 21;
            let (Some(u), Some(v), Some(w), Some(hh)) = (i32_at(raw, o), i32_at(raw, o + 4), i32_at(raw, o + 8), i32_at(raw, o + 12)) else { break };
            let page = raw.get(o + 16).copied().unwrap_or(0);
            let y_offset = i32_at(raw, o + 17).unwrap_or(0);
            glyphs.push(Glyph { u, v, w, h: hh, page, y_offset });
        }
    }
    let mut pages = Vec::new();
    if let Some((n, raw)) = find(&h.properties, "Textures").and_then(|p| p.array_raw()) {
        for k in 0..n {
            let Some(r) = i32_at(raw, k * 4) else { break };
            if let ObjRef::Export(t) = ObjRef::from_index(r) {
                pages.push(read_texture(content, &pkg, t, 4096)?);
            }
        }
    }
    let remapped = matches!(find(&h.properties, "IsRemapped").map(|p| &p.value), Some(Value::Int(v)) if *v != 0);
    let kerning = match find(&h.properties, "Kerning").map(|p| &p.value) {
        Some(Value::Int(v)) => *v,
        _ => 0,
    };
    // Native tail: CharRemap (count, then (u16 key, u16 value) pairs).
    let remap = if remapped {
        let data = pkg.export_data(idx)?;
        let tail = &data[h.native_offset.min(data.len())..];
        let n = i32_at(tail, 0).unwrap_or(0).max(0) as usize;
        let mut m = HashMap::with_capacity(n);
        for k in 0..n {
            let o = 4 + k * 4;
            let Some(b) = tail.get(o..o + 4) else { break };
            m.insert(u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]]));
        }
        Some(m)
    } else {
        None
    };
    let line_height = glyphs.iter().map(|g| g.h).max().unwrap_or(0);
    Ok(BitmapFont { name: path.to_string(), glyphs, pages, remap, kerning, line_height })
}
