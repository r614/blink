//! SWF `DefineFont2/3` glyph outlines -> an in-memory TrueType font, so UI text can use the
//! game's own typefaces (loaded from the install at runtime; never written to disk).
//!
//! DefineFont3 glyphs are quadratic outlines on a 20480-unit EM (1024 * 20), Y down; TrueType
//! `glyf` outlines are quadratic too, so curves convert exactly (scaled to a 1024-unit EM).

use anyhow::{Result, bail};

#[derive(Debug, Clone, Default)]
pub struct SwfFont {
    pub name: String,
    /// Per glyph: contours of (x, y, on_curve) in TrueType units (Y up).
    pub glyphs: Vec<Vec<Vec<(i16, i16, bool)>>>,
    pub codes: Vec<u16>,
    pub advances: Vec<i16>,
    pub ascent: i16,
    pub descent: i16,
    pub leading: i16,
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    bit: u32,
}

impl Bits<'_> {
    fn ub(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            let byte = *self.d.get(self.pos).unwrap_or(&0);
            v = (v << 1) | ((byte >> (7 - self.bit)) & 1) as u32;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        v
    }
    fn sb(&mut self, n: u32) -> i32 {
        if n == 0 {
            return 0;
        }
        let v = self.ub(n);
        ((v << (32 - n)) as i32) >> (32 - n)
    }
}

fn u16le(d: &[u8], p: usize) -> u16 {
    u16::from_le_bytes([d[p], d[p + 1]])
}

/// Parse a DefineFont2/3 tag body. `font3` selects the 20x EM scale.
pub fn parse_define_font(body: &[u8], font3: bool) -> Result<SwfFont> {
    if body.len() < 6 {
        bail!("short font tag");
    }
    let flags = body[2];
    let wide_offsets = flags & 0x08 != 0;
    let has_layout = flags & 0x80 != 0;
    let name_len = body[4] as usize;
    let name = String::from_utf8_lossy(&body[5..5 + name_len]).trim_end_matches('\0').to_string();
    let mut p = 5 + name_len;
    let n = u16le(body, p) as usize;
    p += 2;
    let table = p;
    let off = |i: usize| -> usize { if wide_offsets { u32::from_le_bytes(body[table + i * 4..table + i * 4 + 4].try_into().unwrap()) as usize } else { u16le(body, table + i * 2) as usize } };
    let entry = if wide_offsets { 4 } else { 2 };
    let code_off = off(n);
    let scale = if font3 { 20.0 } else { 1.0 };
    let to_tt = |x: i32, y: i32| -> (i16, i16) { ((x as f32 / scale).round() as i16, (-(y as f32) / scale).round() as i16) };
    let mut glyphs = Vec::with_capacity(n);
    for g in 0..n {
        let start = table + off(g);
        let end = table + if g + 1 < n { off(g + 1) } else { code_off };
        let mut b = Bits { d: &body[..end.min(body.len())], pos: start, bit: 0 };
        let fill_bits = b.ub(4);
        let line_bits = b.ub(4);
        let (mut x, mut y) = (0i32, 0i32);
        let mut contours: Vec<Vec<(i16, i16, bool)>> = Vec::new();
        let mut cur: Vec<(i16, i16, bool)> = Vec::new();
        loop {
            if b.pos >= end {
                break;
            }
            if b.ub(1) == 0 {
                let new_styles = b.ub(1);
                let line = b.ub(1);
                let fill1 = b.ub(1);
                let fill0 = b.ub(1);
                let move_to = b.ub(1);
                if new_styles | line | fill1 | fill0 | move_to == 0 {
                    break;
                }
                if move_to == 1 {
                    let nb = b.ub(5);
                    x = b.sb(nb);
                    y = b.sb(nb);
                    if cur.len() > 2 {
                        contours.push(std::mem::take(&mut cur));
                    } else {
                        cur.clear();
                    }
                    let (tx, ty) = to_tt(x, y);
                    cur.push((tx, ty, true));
                }
                if fill0 == 1 {
                    b.ub(fill_bits);
                }
                if fill1 == 1 {
                    b.ub(fill_bits);
                }
                if line == 1 {
                    b.ub(line_bits);
                }
                if new_styles == 1 {
                    break; // not used in font glyphs
                }
            } else if b.ub(1) == 1 {
                let nb = b.ub(4) + 2;
                if b.ub(1) == 1 {
                    x += b.sb(nb);
                    y += b.sb(nb);
                } else if b.ub(1) == 1 {
                    y += b.sb(nb);
                } else {
                    x += b.sb(nb);
                }
                let (tx, ty) = to_tt(x, y);
                cur.push((tx, ty, true));
            } else {
                let nb = b.ub(4) + 2;
                let cx = x + b.sb(nb);
                let cy = y + b.sb(nb);
                x = cx + b.sb(nb);
                y = cy + b.sb(nb);
                let (tcx, tcy) = to_tt(cx, cy);
                let (tx, ty) = to_tt(x, y);
                cur.push((tcx, tcy, false));
                cur.push((tx, ty, true));
            }
        }
        if cur.len() > 2 {
            contours.push(cur);
        }
        // Closed contours repeat the start point at the end; TrueType closes implicitly.
        for c in contours.iter_mut() {
            if c.len() > 1 && c.first() == c.last() {
                c.pop();
            }
        }
        glyphs.push(contours);
    }
    let mut p = table + code_off;
    let mut codes = Vec::with_capacity(n);
    for _ in 0..n {
        codes.push(u16le(body, p));
        p += 2;
    }
    let _ = entry;
    let mut font = SwfFont { name, glyphs, codes, advances: vec![512; n], ascent: 900, descent: 250, leading: 0 };
    if has_layout && p + 6 <= body.len() {
        font.ascent = (u16le(body, p) as f32 / scale) as i16;
        font.descent = (u16le(body, p + 2) as f32 / scale) as i16;
        font.leading = (u16le(body, p + 4) as i16 as f32 / scale) as i16;
        p += 6;
        for i in 0..n {
            if p + 2 > body.len() {
                break;
            }
            font.advances[i] = (u16le(body, p) as i16 as f32 / scale).round() as i16;
            p += 2;
        }
    }
    Ok(font)
}

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn checksum(d: &[u8]) -> u32 {
    let mut s = 0u32;
    for c in d.chunks(4) {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        s = s.wrapping_add(u32::from_be_bytes(b));
    }
    s
}

impl SwfFont {
    /// Build a TrueType font (glyph 0 = .notdef) usable by any TTF loader.
    pub fn to_ttf(&self, family: &str) -> Vec<u8> {
        let n = self.glyphs.len() + 1;
        let mut glyf = Vec::new();
        let mut loca: Vec<u32> = vec![0];
        let (mut gx0, mut gy0, mut gx1, mut gy1) = (0i16, 0i16, 0i16, 0i16);
        let (mut max_pts, mut max_cont) = (0u16, 0u16);
        let mut lsbs = vec![0i16];
        glyf.extend_from_slice(&[]);
        loca.push(0);
        for contours in &self.glyphs {
            let pts: Vec<&(i16, i16, bool)> = contours.iter().flatten().collect();
            if pts.is_empty() {
                lsbs.push(0);
                loca.push(glyf.len() as u32);
                continue;
            }
            let (x0, x1) = (pts.iter().map(|p| p.0).min().unwrap(), pts.iter().map(|p| p.0).max().unwrap());
            let (y0, y1) = (pts.iter().map(|p| p.1).min().unwrap(), pts.iter().map(|p| p.1).max().unwrap());
            gx0 = gx0.min(x0);
            gy0 = gy0.min(y0);
            gx1 = gx1.max(x1);
            gy1 = gy1.max(y1);
            max_pts = max_pts.max(pts.len() as u16);
            max_cont = max_cont.max(contours.len() as u16);
            lsbs.push(x0);
            be16(&mut glyf, contours.len() as u16);
            for v in [x0, y0, x1, y1] {
                be16(&mut glyf, v as u16);
            }
            let mut end = 0u16;
            for c in contours {
                end += c.len() as u16;
                be16(&mut glyf, end - 1);
            }
            be16(&mut glyf, 0); // instructions
            for p in &pts {
                glyf.push(if p.2 { 1 } else { 0 });
            }
            let (mut px, mut py) = (0i16, 0i16);
            for p in &pts {
                be16(&mut glyf, p.0.wrapping_sub(px) as u16);
                px = p.0;
            }
            for p in &pts {
                be16(&mut glyf, p.1.wrapping_sub(py) as u16);
                py = p.1;
            }
            while glyf.len() % 4 != 0 {
                glyf.push(0);
            }
            loca.push(glyf.len() as u32);
        }
        let advances: Vec<i16> = std::iter::once(500).chain(self.advances.iter().copied()).collect();
        let adv_max = advances.iter().copied().max().unwrap_or(500);
        let asc = self.ascent.max(gy1);
        let desc = self.descent.max(-gy0);

        let mut head = Vec::new();
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0);
        be32(&mut head, 0x5F0F_3CF5);
        be16(&mut head, 0x000B);
        be16(&mut head, 1024);
        head.extend_from_slice(&[0; 16]);
        for v in [gx0, gy0, gx1, gy1] {
            be16(&mut head, v as u16);
        }
        be16(&mut head, 0);
        be16(&mut head, 8);
        be16(&mut head, 2);
        be16(&mut head, 1); // long loca
        be16(&mut head, 0);

        let mut hhea = Vec::new();
        be32(&mut hhea, 0x0001_0000);
        be16(&mut hhea, asc as u16);
        be16(&mut hhea, (-desc) as u16);
        be16(&mut hhea, self.leading as u16);
        be16(&mut hhea, adv_max as u16);
        be16(&mut hhea, gx0 as u16);
        be16(&mut hhea, 0);
        be16(&mut hhea, gx1 as u16);
        be16(&mut hhea, 1);
        be16(&mut hhea, 0);
        be16(&mut hhea, 0);
        hhea.extend_from_slice(&[0; 8]);
        be16(&mut hhea, 0);
        be16(&mut hhea, n as u16);

        let mut maxp = Vec::new();
        be32(&mut maxp, 0x0001_0000);
        be16(&mut maxp, n as u16);
        be16(&mut maxp, max_pts);
        be16(&mut maxp, max_cont);
        be16(&mut maxp, 0);
        be16(&mut maxp, 0);
        be16(&mut maxp, 2);
        for _ in 0..8 {
            be16(&mut maxp, 0);
        }

        let mut hmtx = Vec::new();
        for (a, l) in advances.iter().zip(&lsbs) {
            be16(&mut hmtx, *a as u16);
            be16(&mut hmtx, *l as u16);
        }

        let mut locab = Vec::new();
        for l in &loca[1..] {
            be32(&mut locab, *l);
        }
        // loca[0] was a placeholder for .notdef start; rebuild: .notdef empty at 0.
        let mut loca_full = Vec::new();
        be32(&mut loca_full, 0);
        loca_full.extend_from_slice(&locab);

        // cmap format 4, one segment per code.
        let mut pairs: Vec<(u16, u16)> = self.codes.iter().enumerate().map(|(i, c)| (*c, i as u16 + 1)).filter(|(c, _)| *c != 0xFFFF).collect();
        pairs.sort();
        pairs.dedup_by_key(|p| p.0);
        let seg = pairs.len() + 1;
        let mut sub = Vec::new();
        be16(&mut sub, 4);
        be16(&mut sub, 0);
        be16(&mut sub, 0);
        be16(&mut sub, (seg * 2) as u16);
        let mut sr = 1u16;
        let mut es = 0u16;
        while (sr as usize) * 2 <= seg {
            sr *= 2;
            es += 1;
        }
        be16(&mut sub, sr * 2);
        be16(&mut sub, es);
        be16(&mut sub, (seg as u16) * 2 - sr * 2);
        for (c, _) in &pairs {
            be16(&mut sub, *c);
        }
        be16(&mut sub, 0xFFFF);
        be16(&mut sub, 0);
        for (c, _) in &pairs {
            be16(&mut sub, *c);
        }
        be16(&mut sub, 0xFFFF);
        for (c, g) in &pairs {
            be16(&mut sub, g.wrapping_sub(*c));
        }
        be16(&mut sub, 1);
        for _ in 0..seg {
            be16(&mut sub, 0);
        }
        let sl = sub.len() as u16;
        sub[2..4].copy_from_slice(&sl.to_be_bytes());
        let mut cmap = Vec::new();
        be16(&mut cmap, 0);
        be16(&mut cmap, 1);
        be16(&mut cmap, 3);
        be16(&mut cmap, 1);
        be32(&mut cmap, 12);
        cmap.extend_from_slice(&sub);

        let strings = [(1u16, family.to_string()), (2, "Regular".to_string()), (4, format!("{family} Regular")), (6, family.replace(' ', ""))];
        let mut name = Vec::new();
        be16(&mut name, 0);
        be16(&mut name, strings.len() as u16);
        be16(&mut name, (6 + 12 * strings.len()) as u16);
        let mut store = Vec::new();
        for (id, s) in &strings {
            let utf16: Vec<u8> = s.encode_utf16().flat_map(|c| c.to_be_bytes()).collect();
            be16(&mut name, 3);
            be16(&mut name, 1);
            be16(&mut name, 0x409);
            be16(&mut name, *id);
            be16(&mut name, utf16.len() as u16);
            be16(&mut name, store.len() as u16);
            store.extend_from_slice(&utf16);
        }
        name.extend_from_slice(&store);

        let mut os2 = Vec::new();
        be16(&mut os2, 4);
        be16(&mut os2, (advances.iter().map(|a| *a as i32).sum::<i32>() / n as i32) as u16);
        be16(&mut os2, 400);
        be16(&mut os2, 5);
        be16(&mut os2, 0);
        for _ in 0..10 {
            be16(&mut os2, 0);
        }
        be16(&mut os2, 0);
        os2.extend_from_slice(&[0; 10]);
        for _ in 0..4 {
            be32(&mut os2, 0);
        }
        os2.extend_from_slice(b"ARKN");
        be16(&mut os2, 0x40);
        be16(&mut os2, pairs.first().map(|p| p.0).unwrap_or(32));
        be16(&mut os2, pairs.last().map(|p| p.0).unwrap_or(126));
        be16(&mut os2, asc as u16);
        be16(&mut os2, (-desc) as u16);
        be16(&mut os2, self.leading as u16);
        be16(&mut os2, asc as u16);
        be16(&mut os2, desc as u16);
        be32(&mut os2, 1);
        be32(&mut os2, 0);
        be16(&mut os2, (asc as f32 * 0.5) as u16);
        be16(&mut os2, (asc as f32 * 0.7) as u16);
        be16(&mut os2, 0);
        be16(&mut os2, 32);
        be16(&mut os2, 1);

        let mut post = Vec::new();
        be32(&mut post, 0x0003_0000);
        be32(&mut post, 0);
        be16(&mut post, (-100i16) as u16);
        be16(&mut post, 50);
        be32(&mut post, 0);
        for _ in 0..4 {
            be32(&mut post, 0);
        }

        let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"OS/2", os2),
            (b"cmap", cmap),
            (b"glyf", glyf),
            (b"head", head),
            (b"hhea", hhea),
            (b"hmtx", hmtx),
            (b"loca", loca_full),
            (b"maxp", maxp),
            (b"name", name),
            (b"post", post),
        ];
        tables.sort_by_key(|t| *t.0);
        let nt = tables.len() as u16;
        let mut out = Vec::new();
        be32(&mut out, 0x0001_0000);
        be16(&mut out, nt);
        be16(&mut out, 128);
        be16(&mut out, 3);
        be16(&mut out, nt * 16 - 128);
        let mut offset = 12 + 16 * tables.len();
        let mut body = Vec::new();
        for (tag, data) in &tables {
            out.extend_from_slice(*tag);
            be32(&mut out, checksum(data));
            be32(&mut out, offset as u32);
            be32(&mut out, data.len() as u32);
            body.extend_from_slice(data);
            while body.len() % 4 != 0 {
                body.push(0);
            }
            offset = 12 + 16 * tables.len() + body.len();
        }
        out.extend_from_slice(&body);
        out
    }
}
