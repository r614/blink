//! Minimal reader for Scaleform GFx movies (`SwfMovie.RawData`): enough structure to rebuild
//! static HUD layouts. Textures are external (GFx tag 1009 names a Texture2D in the same
//! package); shapes reference them through bitmap fills. ActionScript is not executed.

use anyhow::{Result, bail};
use std::collections::BTreeMap;

/// 2x3 affine matrix in twips (translation) / unit scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Default for Matrix {
    fn default() -> Self {
        Self { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 }
    }
}

impl Matrix {
    /// self * other (apply `other` first).
    pub fn mul(&self, o: &Matrix) -> Matrix {
        Matrix {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            tx: self.a * o.tx + self.c * o.ty + self.tx,
            ty: self.b * o.tx + self.d * o.ty + self.ty,
        }
    }
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.tx, self.b * x + self.d * y + self.ty)
    }
    pub fn inverse(&self) -> Matrix {
        let det = self.a * self.d - self.b * self.c;
        let det = if det.abs() < 1e-12 { 1e-12 } else { det };
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Matrix { a, b, c, d, tx: -(a * self.tx + c * self.ty), ty: -(b * self.tx + d * self.ty) }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub xmin: f32,
    pub xmax: f32,
    pub ymin: f32,
    pub ymax: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorTransform {
    pub mul: [f32; 4],
    pub add: [f32; 4],
}

impl Default for ColorTransform {
    fn default() -> Self {
        Self { mul: [1.0; 4], add: [0.0; 4] }
    }
}

#[derive(Debug, Clone)]
pub enum Fill {
    Solid([u8; 4]),
    /// Bitmap fill: character id of the bitmap and the bitmap->shape matrix (twips).
    Bitmap { bitmap: u16, matrix: Matrix, smoothed: bool, clipped: bool },
    Gradient,
}

#[derive(Debug, Clone)]
pub struct Shape {
    pub bounds: Rect,
    pub fills: Vec<Fill>,
}

#[derive(Debug, Clone, Default)]
pub struct Place {
    pub depth: u16,
    pub character: Option<u16>,
    pub matrix: Option<Matrix>,
    pub color: Option<ColorTransform>,
    pub name: Option<String>,
    pub move_only: bool,
}

#[derive(Debug, Clone)]
pub enum FrameOp {
    Place(Place),
    Remove(u16),
    Label(String),
}

#[derive(Debug, Clone, Default)]
pub struct Sprite {
    /// Ops per frame (frame boundary = ShowFrame).
    pub frames: Vec<Vec<FrameOp>>,
}

#[derive(Debug, Clone)]
pub struct ExternalImage {
    pub export_name: String,
    pub file_name: String,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Default)]
pub struct Movie {
    pub version: u8,
    pub frame_size: Rect,
    pub frame_rate: f32,
    /// External images by GFx 32-bit id. Texture-atlas pages use ids `0x0009_xxxx`; sub-images
    /// name their page by the low 16 bits.
    pub images: BTreeMap<u32, ExternalImage>,
    /// Sub-image: (image character, x1, y1, x2, y2) in pixels.
    pub sub_images: BTreeMap<u16, (u16, u16, u16, u16, u16)>,
    pub shapes: BTreeMap<u16, Shape>,
    pub sprites: BTreeMap<u16, Sprite>,
    pub texts: BTreeMap<u16, Rect>,
    pub exports: BTreeMap<String, u16>,
    pub root: Sprite,
    pub tag_counts: BTreeMap<u16, usize>,
    /// Embedded fonts (DefineFont2/3).
    pub fonts: Vec<crate::swf_font::SwfFont>,
}

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    bit: u32,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8], pos: usize) -> Self {
        Self { d, pos, bit: 0 }
    }
    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.pos += 1;
        }
    }
    fn ub(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
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
    fn fb(&mut self, n: u32) -> f32 {
        self.sb(n) as f32 / 65536.0
    }
    fn u8(&mut self) -> u8 {
        self.align();
        let v = *self.d.get(self.pos).unwrap_or(&0);
        self.pos += 1;
        v
    }
    fn u16(&mut self) -> u16 {
        self.align();
        let v = u16::from_le_bytes([*self.d.get(self.pos).unwrap_or(&0), *self.d.get(self.pos + 1).unwrap_or(&0)]);
        self.pos += 2;
        v
    }
    fn u32(&mut self) -> u32 {
        let lo = self.u16() as u32;
        let hi = self.u16() as u32;
        lo | (hi << 16)
    }
    fn cstr(&mut self) -> String {
        self.align();
        let start = self.pos;
        while self.pos < self.d.len() && self.d[self.pos] != 0 {
            self.pos += 1;
        }
        let s = String::from_utf8_lossy(&self.d[start..self.pos]).into_owned();
        self.pos += 1;
        s
    }
    fn pstr(&mut self) -> String {
        let n = self.u8() as usize;
        let s = String::from_utf8_lossy(self.d.get(self.pos..self.pos + n).unwrap_or(&[])).into_owned();
        self.pos += n;
        s
    }
    fn rect(&mut self) -> Rect {
        self.align();
        let n = self.ub(5);
        let r = Rect { xmin: self.sb(n) as f32, xmax: self.sb(n) as f32, ymin: self.sb(n) as f32, ymax: self.sb(n) as f32 };
        self.align();
        r
    }
    fn matrix(&mut self) -> Matrix {
        self.align();
        let mut m = Matrix::default();
        if self.ub(1) == 1 {
            let n = self.ub(5);
            m.a = self.fb(n);
            m.d = self.fb(n);
        }
        if self.ub(1) == 1 {
            let n = self.ub(5);
            m.b = self.fb(n);
            m.c = self.fb(n);
        }
        let n = self.ub(5);
        m.tx = self.sb(n) as f32;
        m.ty = self.sb(n) as f32;
        self.align();
        m
    }
    fn cxform(&mut self, alpha: bool) -> ColorTransform {
        self.align();
        let has_add = self.ub(1) == 1;
        let has_mul = self.ub(1) == 1;
        let n = self.ub(4);
        let mut c = ColorTransform::default();
        let k = if alpha { 4 } else { 3 };
        if has_mul {
            for i in 0..k {
                c.mul[i] = self.sb(n) as f32 / 256.0;
            }
        }
        if has_add {
            for i in 0..k {
                c.add[i] = self.sb(n) as f32 / 255.0;
            }
        }
        self.align();
        c
    }
}

fn parse_fills(b: &mut Bits, shape_ver: u8) -> Vec<Fill> {
    let mut n = b.u8() as usize;
    if n == 0xff {
        n = b.u16() as usize;
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let t = b.u8();
        match t {
            0x00 => {
                let rgba = if shape_ver >= 3 { [b.u8(), b.u8(), b.u8(), b.u8()] } else { [b.u8(), b.u8(), b.u8(), 255] };
                out.push(Fill::Solid(rgba));
            }
            0x10 | 0x12 | 0x13 => {
                let _m = b.matrix();
                b.align();
                let flags = b.u8();
                let count = (flags & 15) as usize;
                for _ in 0..count {
                    b.u8();
                    if shape_ver >= 3 {
                        b.u32();
                    } else {
                        b.u8();
                        b.u8();
                        b.u8();
                    }
                }
                if t == 0x13 {
                    b.u16();
                }
                out.push(Fill::Gradient);
            }
            0x40..=0x43 => {
                let bitmap = b.u16();
                let matrix = b.matrix();
                out.push(Fill::Bitmap { bitmap, matrix, smoothed: t & 2 == 0, clipped: t & 1 == 1 });
            }
            _ => break,
        }
    }
    out
}

fn parse_place(b: &mut Bits, ver: u8, end: usize) -> Place {
    let flags = b.u8();
    let flags2 = if ver == 3 { b.u8() } else { 0 };
    let depth = b.u16();
    let mut p = Place { depth, move_only: flags & 1 == 1, ..Default::default() };
    if ver == 3 && (flags2 & 0x08 != 0 || (flags2 & 0x10 != 0 && flags & 0x02 != 0)) {
        b.cstr(); // class name
    }
    if flags & 0x02 != 0 {
        p.character = Some(b.u16());
    }
    if flags & 0x04 != 0 {
        p.matrix = Some(b.matrix());
    }
    if flags & 0x08 != 0 {
        p.color = Some(b.cxform(true));
    }
    if flags & 0x10 != 0 {
        b.u16(); // ratio
    }
    if flags & 0x20 != 0 && b.pos < end {
        p.name = Some(b.cstr());
    }
    p
}

fn parse_tags(d: &[u8], mut pos: usize, end: usize, movie: &mut Movie, sprite: &mut Sprite, top: bool) -> Result<()> {
    let mut frame: Vec<FrameOp> = Vec::new();
    while pos + 2 <= end {
        let hdr = u16::from_le_bytes([d[pos], d[pos + 1]]);
        pos += 2;
        let code = hdr >> 6;
        let mut len = (hdr & 0x3f) as usize;
        if len == 0x3f {
            len = u32::from_le_bytes(d[pos..pos + 4].try_into()?) as usize;
            pos += 4;
        }
        let body = pos;
        let tend = body + len;
        if tend > end {
            bail!("tag {code} overruns ({tend} > {end})");
        }
        *movie.tag_counts.entry(code).or_default() += 1;
        let mut b = Bits::new(d, body);
        match code {
            0 => break,
            1 => sprite.frames.push(std::mem::take(&mut frame)),
            2 | 22 | 32 | 83 => {
                let id = b.u16();
                let bounds = b.rect();
                if code == 83 {
                    b.rect();
                    b.u8();
                }
                let ver = match code {
                    2 => 1,
                    22 => 2,
                    32 => 3,
                    _ => 4,
                };
                let fills = parse_fills(&mut b, ver);
                movie.shapes.insert(id, Shape { bounds, fills });
            }
            26 | 70 => frame.push(FrameOp::Place(parse_place(&mut b, if code == 26 { 2 } else { 3 }, tend))),
            28 => {
                frame.push(FrameOp::Remove(b.u16()));
            }
            43 => frame.push(FrameOp::Label(b.cstr())),
            39 if top => {
                let id = b.u16();
                let _frames = b.u16();
                let mut sp = Sprite::default();
                parse_tags(d, b.pos, tend, movie, &mut sp, false)?;
                movie.sprites.insert(id, sp);
            }
            37 => {
                let id = b.u16();
                let r = b.rect();
                movie.texts.insert(id, r);
            }
            56 | 57 => {
                let n = b.u16();
                for _ in 0..n {
                    let id = b.u16();
                    let name = b.cstr();
                    movie.exports.insert(name, id);
                }
            }
            1001 | 1009 => {
                let id = if code == 1009 { b.u32() } else { b.u16() as u32 };
                let _fmt = b.u16();
                let width = b.u16();
                let height = b.u16();
                let export_name = b.pstr();
                let file_name = b.pstr();
                movie.images.insert(id, ExternalImage { export_name, file_name, width, height });
            }
            48 | 75 => {
                if let Ok(f) = crate::swf_font::parse_define_font(&d[body..tend], code == 75) {
                    movie.fonts.push(f);
                }
            }
            1008 => {
                let id = b.u16();
                let img = b.u16();
                movie.sub_images.insert(id, (img, b.u16(), b.u16(), b.u16(), b.u16()));
            }
            _ => {}
        }
        pos = tend;
    }
    if !frame.is_empty() {
        sprite.frames.push(frame);
    }
    Ok(())
}

impl Movie {
    pub fn parse(raw: &[u8]) -> Result<Movie> {
        if raw.len() < 8 || !(&raw[0..3] == b"GFX" || &raw[0..3] == b"FWS") {
            bail!("not an uncompressed GFx/SWF movie");
        }
        let mut m = Movie { version: raw[3], ..Default::default() };
        let mut b = Bits::new(raw, 8);
        m.frame_size = b.rect();
        let fr = b.u16();
        m.frame_rate = fr as f32 / 256.0;
        let _count = b.u16();
        let mut root = Sprite::default();
        parse_tags(raw, b.pos, raw.len(), &mut m, &mut root, true)?;
        m.root = root;
        Ok(m)
    }
}

/// A textured quad produced by flattening a sprite: source rectangle in the external
/// texture (pixels) and destination corners in stage pixels (TL, TR, BR, BL of the source).
#[derive(Debug, Clone)]
pub struct Quad {
    /// External texture file name without extension (a Texture2D in the movie's package).
    pub texture: String,
    pub src: [f32; 4],
    pub dst: [(f32, f32); 4],
    pub color: ColorTransform,
    /// Instance-name path of the placement chain (e.g. "masterHUD_mc/health_mc").
    pub path: String,
    /// Shape character the quad came from.
    pub character: u16,
}

impl Movie {
    /// Display list of a sprite after running frames `0..=frame` (placements only).
    pub fn display_list(&self, sprite: &Sprite, frame: usize) -> Vec<Place> {
        let mut list: BTreeMap<u16, Place> = BTreeMap::new();
        for f in sprite.frames.iter().take(frame + 1) {
            for op in f {
                match op {
                    FrameOp::Place(p) => {
                        let e = list.entry(p.depth).or_default();
                        if p.move_only && p.character.is_none() {
                            if p.matrix.is_some() {
                                e.matrix = p.matrix;
                            }
                            if p.color.is_some() {
                                e.color = p.color;
                            }
                        } else {
                            let keep_name = if p.name.is_none() && e.character == p.character { e.name.clone() } else { None };
                            *e = p.clone();
                            if e.name.is_none() {
                                e.name = keep_name;
                            }
                        }
                    }
                    FrameOp::Remove(d) => {
                        list.remove(d);
                    }
                    FrameOp::Label(_) => {}
                }
            }
        }
        list.into_values().collect()
    }

    /// Frame index of a label inside a sprite.
    pub fn label_frame(sprite: &Sprite, label: &str) -> Option<usize> {
        sprite.frames.iter().position(|f| f.iter().any(|op| matches!(op, FrameOp::Label(l) if l.eq_ignore_ascii_case(label))))
    }

    /// Flatten a character into quads. `frame_of` picks the frame for each sprite path
    /// (default 0).
    pub fn flatten(&self, character: u16, m: Matrix, color: ColorTransform, path: &str, frame_of: &dyn Fn(&str, &Sprite) -> usize, out: &mut Vec<Quad>) {
        if let Some(shape) = self.shapes.get(&character) {
            for fill in &shape.fills {
                let Fill::Bitmap { bitmap, matrix, .. } = fill else { continue };
                let (tex, off, size) = match (self.sub_images.get(bitmap), self.images.get(&(*bitmap as u32))) {
                    (None, Some(im)) => (im.file_name.clone(), (0.0, 0.0), (im.width as f32, im.height as f32)),
                    (Some((img, x1, y1, x2, y2)), _) => match self.images.get(&(0x0009_0000 | *img as u32)).or_else(|| self.images.get(&(*img as u32))) {
                        Some(im) => (im.file_name.clone(), (*x1 as f32, *y1 as f32), ((*x2 - *x1) as f32, (*y2 - *y1) as f32)),
                        None => continue,
                    },
                    _ => continue,
                };
                let _ = size;
                let b = shape.bounds;
                let inv = matrix.inverse();
                let corners = [(b.xmin, b.ymin), (b.xmax, b.ymin), (b.xmax, b.ymax), (b.xmin, b.ymax)];
                let (s0x, s0y) = inv.apply(b.xmin, b.ymin);
                let (s1x, s1y) = inv.apply(b.xmax, b.ymax);
                let dst = corners.map(|(x, y)| {
                    let (px, py) = m.apply(x, y);
                    (px / 20.0, py / 20.0)
                });
                let texture = tex.trim_end_matches(".tga").trim_end_matches(".dds").to_string();
                out.push(Quad { texture, src: [s0x.min(s1x) + off.0, s0y.min(s1y) + off.1, s0x.max(s1x) + off.0, s0y.max(s1y) + off.1], dst, color, path: path.to_string(), character });
                break;
            }
            return;
        }
        let Some(sprite) = self.sprites.get(&character) else { return };
        let frame = frame_of(path, sprite);
        for p in self.display_list(sprite, frame) {
            let Some(ch) = p.character else { continue };
            let pm = m.mul(&p.matrix.unwrap_or_default());
            let pc = match p.color {
                Some(c) => ColorTransform { mul: std::array::from_fn(|i| color.mul[i] * c.mul[i]), add: std::array::from_fn(|i| color.add[i] + c.add[i] * color.mul[i]) },
                None => color,
            };
            let child = match &p.name {
                Some(n) if path.is_empty() => n.clone(),
                Some(n) => format!("{path}/{n}"),
                None => path.to_string(),
            };
            self.flatten(ch, pm, pc, &child, frame_of, out);
        }
    }
}
