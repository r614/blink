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
    Gradient(Gradient),
}

/// A gradient fill (`FILLSTYLE` 0x10 linear, 0x12 radial, 0x13 focal radial): the matrix maps the
/// gradient square (-16384..16384 twips) into the shape; `SpreadMode` (0 pad, 1 reflect, 2
/// repeat); stops as (ratio 0..1, RGBA 0..1); the focal point (-1..1 on the x axis).
#[derive(Debug, Clone)]
pub struct Gradient {
    pub matrix: Matrix,
    pub radial: bool,
    pub focal: f32,
    pub spread: u8,
    pub stops: Vec<(f32, [f32; 4])>,
}

#[derive(Debug, Clone)]
pub struct Shape {
    pub bounds: Rect,
    /// Every fill style of the shape (the records' `NewStyles` arrays appended).
    pub fills: Vec<Fill>,
    /// The edges (twips; curves flattened) beside each fill style: (index into `fills`,
    /// [x0, y0, x1, y1]); an edge between two fills is listed for both.
    pub edges: Vec<(usize, [f32; 4])>,
}

#[derive(Debug, Clone, Default)]
pub struct Place {
    pub depth: u16,
    pub character: Option<u16>,
    pub matrix: Option<Matrix>,
    pub color: Option<ColorTransform>,
    pub name: Option<String>,
    pub move_only: bool,
    /// Mask layer: this placement clips the depths above it up to `clip_depth`.
    pub clip_depth: Option<u16>,
}

#[derive(Debug, Clone)]
pub enum FrameOp {
    Place(Place),
    Remove(u16),
    Label(String),
    /// A frame script (`DoAction`), reduced to the timeline commands it issues.
    Script(Vec<ScriptOp>),
}

/// Timeline commands of a frame script. The HUD movies' frame scripts are `stop()`, `play()`,
/// `gotoAndPlay(label | frame)` and `this._visible = bool`; anything else is `Other`.
#[derive(Debug, Clone, PartialEq)]
pub enum ScriptOp {
    Stop,
    Play,
    /// `ActionGotoFrame` (0-based frame).
    GotoFrame(u16),
    GotoLabel(String),
    SetVisible(bool),
    Other,
}

/// Decode a `DoAction` body into [`ScriptOp`]s (constant pools, pushes and the `this._visible`
/// pattern are recognised; other actions become `Other`).
pub fn parse_frame_script(body: &[u8]) -> Vec<ScriptOp> {
    let mut out = Vec::new();
    let mut p = 0usize;
    // Last pushed values (strings / booleans), for `push 'this'; getVar; push '_visible', b; setMember`.
    let mut pool: Vec<String> = Vec::new();
    let mut pushed: Vec<PushVal> = Vec::new();
    #[derive(Clone, PartialEq)]
    enum PushVal {
        Str(String),
        Bool(bool),
        Other,
    }
    while p < body.len() {
        let op = body[p];
        p += 1;
        if op == 0 {
            break;
        }
        let (len, start) = if op >= 0x80 && p + 2 <= body.len() {
            let l = u16::from_le_bytes([body[p], body[p + 1]]) as usize;
            p += 2;
            (l, p)
        } else {
            (0, p)
        };
        let data = body.get(start..(start + len).min(body.len())).unwrap_or(&[]);
        p = start + len;
        match op {
            0x07 => out.push(ScriptOp::Stop),
            0x06 => out.push(ScriptOp::Play),
            0x81 if data.len() >= 2 => out.push(ScriptOp::GotoFrame(u16::from_le_bytes([data[0], data[1]]))),
            0x8C => {
                let end = data.iter().position(|&c| c == 0).unwrap_or(data.len());
                out.push(ScriptOp::GotoLabel(String::from_utf8_lossy(&data[..end]).into_owned()));
            }
            0x88 if data.len() >= 2 => {
                let n = u16::from_le_bytes([data[0], data[1]]) as usize;
                let mut q = 2;
                pool.clear();
                for _ in 0..n {
                    let end = data[q..].iter().position(|&c| c == 0).map(|e| q + e).unwrap_or(data.len());
                    pool.push(String::from_utf8_lossy(&data[q..end]).into_owned());
                    q = end + 1;
                    if q >= data.len() {
                        break;
                    }
                }
            }
            0x96 => {
                let mut q = 0;
                while q < data.len() {
                    let t = data[q];
                    q += 1;
                    match t {
                        0 => {
                            let end = data[q..].iter().position(|&c| c == 0).map(|e| q + e).unwrap_or(data.len());
                            pushed.push(PushVal::Str(String::from_utf8_lossy(&data[q..end]).into_owned()));
                            q = end + 1;
                        }
                        1 | 7 => {
                            pushed.push(PushVal::Other);
                            q += 4;
                        }
                        2 | 3 => pushed.push(PushVal::Other),
                        4 => {
                            pushed.push(PushVal::Other);
                            q += 1;
                        }
                        5 => {
                            pushed.push(PushVal::Bool(data.get(q).copied().unwrap_or(0) != 0));
                            q += 1;
                        }
                        6 => {
                            pushed.push(PushVal::Other);
                            q += 8;
                        }
                        8 => {
                            let i = data.get(q).copied().unwrap_or(0) as usize;
                            pushed.push(pool.get(i).cloned().map(PushVal::Str).unwrap_or(PushVal::Other));
                            q += 1;
                        }
                        9 => {
                            let i = u16::from_le_bytes([data.get(q).copied().unwrap_or(0), data.get(q + 1).copied().unwrap_or(0)]) as usize;
                            pushed.push(pool.get(i).cloned().map(PushVal::Str).unwrap_or(PushVal::Other));
                            q += 2;
                        }
                        _ => break,
                    }
                }
            }
            // SetMember: `this._visible = b` (pushed: 'this' (getVar), '_visible', b).
            0x4F => {
                let n = pushed.len();
                if n >= 2 && pushed[n - 2] == PushVal::Str("_visible".into()) {
                    if let PushVal::Bool(b) = pushed[n - 1] {
                        out.push(ScriptOp::SetVisible(b));
                    }
                }
                pushed.clear();
            }
            0x1C | 0x4E => {}
            0x17 => pushed.clear(),
            _ => out.push(ScriptOp::Other),
        }
    }
    out
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
    /// A hash of the movie's data (keys the textures made from its shapes).
    pub key: u64,
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
    /// Text fields (DefineEditText): font, size, colour, alignment, variable and initial text.
    pub edit_texts: BTreeMap<u16, EditText>,
    /// Local font character id -> font name (DefineFont2/3).
    pub font_names: BTreeMap<u16, String>,
    /// Imported characters (ImportAssets / ImportAssets2): id -> (source movie URL, export name).
    /// Scaleform resolves the URL against the movie's `ReferencedSwfs` (e.g. `gfxfontlib.swf`
    /// for fonts, the options movie for shared screens).
    pub imports: BTreeMap<u16, (String, String)>,
    /// DefineButton2 characters: (state flags, character, depth, matrix) per record.
    pub buttons: BTreeMap<u16, Vec<ButtonRecord>>,
}

/// DefineEditText. Heights are in pixels (twips / 20); `bounds` in twips.
#[derive(Debug, Clone, Default)]
pub struct EditText {
    pub bounds: Rect,
    pub font: Option<u16>,
    pub height: f32,
    pub color: Option<[u8; 4]>,
    /// 0 left, 1 right, 2 centre, 3 justify.
    pub align: u8,
    pub multiline: bool,
    pub html: bool,
    pub variable: String,
    pub initial: Option<String>,
    /// `wordWrap` (flag 0x40 of the first flag byte).
    pub word_wrap: bool,
    /// `autoSize` (flag 0x40 of the second flag byte).
    pub auto_size: bool,
    /// Layout: left / right margins, indent and leading (pixels).
    pub margins: [f32; 2],
    pub leading: f32,
}

/// One DefineButton2 record. `states`: bit 0 up, 1 over, 2 down, 3 hit test.
#[derive(Debug, Clone)]
pub struct ButtonRecord {
    pub states: u8,
    pub character: u16,
    pub depth: u16,
    pub matrix: Matrix,
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
                let matrix = b.matrix();
                b.align();
                let flags = b.u8();
                let count = (flags & 15) as usize;
                let mut stops = Vec::with_capacity(count);
                for _ in 0..count {
                    let ratio = b.u8() as f32 / 255.0;
                    let c = if shape_ver >= 3 { [b.u8(), b.u8(), b.u8(), b.u8()] } else { [b.u8(), b.u8(), b.u8(), 255] };
                    stops.push((ratio, c.map(|v| v as f32 / 255.0)));
                }
                // FIXED8 focal point.
                let focal = if t == 0x13 { b.u16() as i16 as f32 / 256.0 } else { 0.0 };
                out.push(Fill::Gradient(Gradient { matrix, radial: t != 0x10, focal, spread: flags >> 6, stops }));
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

/// Skip a `LINESTYLEARRAY` (lines are not drawn by the HUD; `LINESTYLE2` of DefineShape4 may
/// carry a fill).
fn skip_line_styles(b: &mut Bits, shape_ver: u8) {
    let mut n = b.u8() as usize;
    if n == 0xff {
        n = b.u16() as usize;
    }
    for _ in 0..n {
        b.u16();
        if shape_ver >= 4 {
            let _start_cap = b.ub(2);
            let join = b.ub(2);
            let has_fill = b.ub(1) == 1;
            b.ub(5);
            b.ub(1);
            let _end_cap = b.ub(2);
            if join == 2 {
                b.u16();
            }
            if has_fill {
                // One fill style (a FILLSTYLEARRAY of one, without its count).
                let t = b.u8();
                match t {
                    0x00 => {
                        b.u32();
                    }
                    0x10 | 0x12 | 0x13 => {
                        b.matrix();
                        b.align();
                        let flags = b.u8();
                        for _ in 0..(flags & 15) {
                            b.u8();
                            b.u32();
                        }
                        if t == 0x13 {
                            b.u16();
                        }
                    }
                    _ => {
                        b.u16();
                        b.matrix();
                    }
                }
            } else {
                b.u32();
            }
        } else if shape_ver >= 3 {
            b.u32();
        } else {
            b.u8();
            b.u8();
            b.u8();
        }
    }
}

/// The edge records of a shape (`SHAPE` records after the style arrays): style changes (move,
/// FillStyle0 / FillStyle1 / LineStyle, NewStyles appending to `fills`), straight edges and
/// quadratic curves (flattened), each edge listed for the fill styles on both sides.
fn parse_shape_records(b: &mut Bits, shape_ver: u8, end: usize, fills: &mut Vec<Fill>) -> Vec<(usize, [f32; 4])> {
    let mut edges = Vec::new();
    b.align();
    let mut fill_bits = b.ub(4);
    let mut line_bits = b.ub(4);
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut f0, mut f1) = (0u32, 0u32);
    let mut base = 0usize;
    let push = |edges: &mut Vec<(usize, [f32; 4])>, base: usize, f0: u32, f1: u32, s: [f32; 4]| {
        for f in [f0, f1] {
            if f > 0 {
                edges.push((base + f as usize - 1, s));
            }
        }
    };
    while b.pos < end {
        if b.ub(1) == 0 {
            let flags = b.ub(5);
            if flags == 0 {
                break;
            }
            if flags & 1 != 0 {
                let n = b.ub(5);
                x = b.sb(n) as f32;
                y = b.sb(n) as f32;
            }
            if flags & 2 != 0 {
                f0 = b.ub(fill_bits);
            }
            if flags & 4 != 0 {
                f1 = b.ub(fill_bits);
            }
            if flags & 8 != 0 {
                b.ub(line_bits);
            }
            if flags & 16 != 0 && shape_ver >= 2 {
                base = fills.len();
                fills.extend(parse_fills(b, shape_ver));
                skip_line_styles(b, shape_ver);
                b.align();
                fill_bits = b.ub(4);
                line_bits = b.ub(4);
            }
        } else if b.ub(1) == 1 {
            let n = b.ub(4) + 2;
            let (dx, dy) = if b.ub(1) == 1 {
                (b.sb(n) as f32, b.sb(n) as f32)
            } else if b.ub(1) == 1 {
                (0.0, b.sb(n) as f32)
            } else {
                (b.sb(n) as f32, 0.0)
            };
            push(&mut edges, base, f0, f1, [x, y, x + dx, y + dy]);
            x += dx;
            y += dy;
        } else {
            let n = b.ub(4) + 2;
            let (cx, cy) = (x + b.sb(n) as f32, y + b.sb(n) as f32);
            let (ax, ay) = (cx + b.sb(n) as f32, cy + b.sb(n) as f32);
            // Flattened by length (about one segment per 2 pixels of a 20-twip pixel, 2 .. 32).
            let len = ((cx - x).hypot(cy - y) + (ax - cx).hypot(ay - cy)) / 40.0;
            let steps = (len.ceil() as usize).clamp(2, 32);
            let (mut px, mut py) = (x, y);
            for k in 1..=steps {
                let t = k as f32 / steps as f32;
                let u = 1.0 - t;
                let qx = u * u * x + 2.0 * u * t * cx + t * t * ax;
                let qy = u * u * y + 2.0 * u * t * cy + t * t * ay;
                push(&mut edges, base, f0, f1, [px, py, qx, qy]);
                px = qx;
                py = qy;
            }
            x = ax;
            y = ay;
        }
    }
    edges
}

fn parse_edit_text(b: &mut Bits, bounds: Rect, end: usize) -> EditText {
    let f1 = b.u8();
    let f2 = b.u8();
    let mut t = EditText { bounds, multiline: f1 & 0x20 != 0, html: f2 & 0x02 != 0, word_wrap: f1 & 0x40 != 0, auto_size: f2 & 0x40 != 0, ..Default::default() };
    let (has_text, has_color, has_max, has_font) = (f1 & 0x80 != 0, f1 & 0x04 != 0, f1 & 0x02 != 0, f1 & 0x01 != 0);
    let (has_font_class, has_layout) = (f2 & 0x80 != 0, f2 & 0x20 != 0);
    if has_font {
        t.font = Some(b.u16());
    }
    if has_font_class {
        b.cstr();
    }
    if has_font || has_font_class {
        t.height = b.u16() as f32 / 20.0;
    }
    if has_color {
        t.color = Some([b.u8(), b.u8(), b.u8(), b.u8()]);
    }
    if has_max {
        b.u16();
    }
    if has_layout {
        t.align = b.u8();
        t.margins = [b.u16() as f32 / 20.0, b.u16() as f32 / 20.0];
        b.u16();
        t.leading = b.u16() as i16 as f32 / 20.0;
    }
    if b.pos < end {
        t.variable = b.cstr();
    }
    if has_text && b.pos < end {
        t.initial = Some(b.cstr());
    }
    t
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
    if flags & 0x40 != 0 && b.pos + 2 <= end {
        p.clip_depth = Some(b.u16());
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
                let mut fills = parse_fills(&mut b, ver);
                skip_line_styles(&mut b, ver);
                let edges = parse_shape_records(&mut b, ver, tend, &mut fills);
                movie.shapes.insert(id, Shape { bounds, fills, edges });
            }
            26 | 70 => frame.push(FrameOp::Place(parse_place(&mut b, if code == 26 { 2 } else { 3 }, tend))),
            28 => {
                frame.push(FrameOp::Remove(b.u16()));
            }
            43 => frame.push(FrameOp::Label(b.cstr())),
            // DoAction inside a sprite (the root's frame scripts are the movie's debug harness).
            12 if !top => frame.push(FrameOp::Script(parse_frame_script(&d[body..tend]))),
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
                movie.edit_texts.insert(id, parse_edit_text(&mut b, r, tend));
            }
            57 | 71 => {
                // ImportAssets(2): URL, [reserved u16 for v2], count, (id, name)*.
                let url = b.cstr();
                if code == 71 {
                    b.u16();
                }
                let n = b.u16();
                for _ in 0..n {
                    if b.pos + 2 > tend {
                        break;
                    }
                    let id = b.u16();
                    let name = b.cstr();
                    movie.imports.insert(id, (url.clone(), name));
                }
            }
            34 => {
                // DefineButton2: id, flags, action offset, records until a 0 flag byte.
                let id = b.u16();
                b.u8();
                b.u16();
                let mut recs = Vec::new();
                while b.pos < tend {
                    let flags = b.u8();
                    if flags == 0 {
                        break;
                    }
                    let character = b.u16();
                    let depth = b.u16();
                    let matrix = b.matrix();
                    b.cxform(true);
                    if flags & 0x10 != 0 {
                        // filter list: skip the rest (not used by these menus)
                        break;
                    }
                    if flags & 0x20 != 0 {
                        b.u8();
                    }
                    recs.push(ButtonRecord { states: flags & 0x0f, character, depth, matrix });
                }
                movie.buttons.insert(id, recs);
            }
            56 => {
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
                    movie.font_names.insert(u16::from_le_bytes([d[body], d[body + 1]]), f.name.clone());
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
        // FNV-1a of the data.
        let key = raw.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3));
        let mut m = Movie { key, version: raw[3], ..Default::default() };
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

/// A text field placed in the stage by [`Movie::flatten_texts`].
#[derive(Debug, Clone)]
pub struct TextPlacement {
    pub id: u16,
    pub text: EditText,
    /// Field space (twips) -> stage (twips).
    pub matrix: Matrix,
    pub color: ColorTransform,
    pub path: String,
}

impl TextPlacement {
    /// Stage-pixel rectangle (axis-aligned bounds of the transformed field).
    pub fn stage_rect(&self) -> (f32, f32, f32, f32) {
        let b = self.text.bounds;
        let pts = [(b.xmin, b.ymin), (b.xmax, b.ymin), (b.xmax, b.ymax), (b.xmin, b.ymax)].map(|(x, y)| self.matrix.apply(x, y));
        let xs = pts.iter().map(|p| p.0 / 20.0);
        let ys = pts.iter().map(|p| p.1 / 20.0);
        (xs.clone().fold(f32::MAX, f32::min), ys.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max), ys.fold(f32::MIN, f32::max))
    }
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
                    FrameOp::Label(_) | FrameOp::Script(_) => {}
                }
            }
        }
        list.into_values().collect()
    }

    /// Frame index of a label inside a sprite.
    pub fn label_frame(sprite: &Sprite, label: &str) -> Option<usize> {
        sprite.frames.iter().position(|f| f.iter().any(|op| matches!(op, FrameOp::Label(l) if l.eq_ignore_ascii_case(label))))
    }

    /// Every text field reachable from a character (same traversal as [`Movie::flatten`]).
    pub fn flatten_texts(&self, character: u16, m: Matrix, color: ColorTransform, path: &str, frame_of: &dyn Fn(&str, &Sprite) -> usize, out: &mut Vec<TextPlacement>) {
        if let Some(t) = self.edit_texts.get(&character) {
            out.push(TextPlacement { id: character, text: t.clone(), matrix: m, color, path: path.to_string() });
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
            self.flatten_texts(ch, pm, pc, &child, frame_of, out);
        }
    }

    /// Solid-colour shapes reachable from a character (backdrops such as the options
    /// screens' `_black_mc`), as stage-pixel corner quads with RGBA (colour transform applied).
    pub fn flatten_solids(&self, character: u16, m: Matrix, color: ColorTransform, path: &str, frame_of: &dyn Fn(&str, &Sprite) -> usize, out: &mut Vec<([(f32, f32); 4], [f32; 4], String)>) {
        if let Some(shape) = self.shapes.get(&character) {
            if let Some(Fill::Solid(c)) = shape.fills.first() {
                let b = shape.bounds;
                let dst = [(b.xmin, b.ymin), (b.xmax, b.ymin), (b.xmax, b.ymax), (b.xmin, b.ymax)].map(|(x, y)| {
                    let (px, py) = m.apply(x, y);
                    (px / 20.0, py / 20.0)
                });
                let rgba = std::array::from_fn(|i| (c[i] as f32 / 255.0 * color.mul[i] + color.add[i]).clamp(0.0, 1.0));
                out.push((dst, rgba, path.to_string()));
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
            self.flatten_solids(ch, pm, pc, &child, frame_of, out);
        }
    }

    /// Font name of a text field's font id (local DefineFont or an imported font export).
    pub fn font_name(&self, id: u16) -> Option<&str> {
        self.font_names.get(&id).map(String::as_str).or_else(|| self.imports.get(&id).map(|(_, n)| n.as_str()))
    }

    /// Frame labels of a sprite in timeline order: (frame, label).
    pub fn labels(sprite: &Sprite) -> Vec<(usize, String)> {
        let mut v = Vec::new();
        for (i, f) in sprite.frames.iter().enumerate() {
            for op in f {
                if let FrameOp::Label(l) = op {
                    v.push((i, l.clone()));
                }
            }
        }
        v
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
