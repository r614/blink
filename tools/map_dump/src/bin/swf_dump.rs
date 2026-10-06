//! `swf_dump <package> [movie]`: print the tag structure of a Scaleform (GFx) SwfMovie.
use anyhow::{Context, Result};
use dishonored_assets::Content;
use ue3_package::ObjRef;
use ue3_package::properties::find;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    let pkg = c.package(&a[0])?;
    let filt = a.get(1).cloned().unwrap_or_default();
    let i = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "SwfMovie" && pkg.obj_path(ObjRef::Export(i)).contains(&filt)).context("no SwfMovie")?;
    let h = pkg.read_object(i)?;
    for p in &h.properties {
        println!("prop {}", p.name);
        // Imported movies (GFx ImportAssets resolve against these package paths).
        if p.name == "ReferencedSwfs" {
            if let Some((n, raw)) = p.array_raw() {
                let mut o = 0usize;
                for _ in 0..n {
                    let Some(len) = raw.get(o..o + 4).map(|b| i32::from_le_bytes(b.try_into().unwrap())) else { break };
                    o += 4;
                    let l = len.unsigned_abs() as usize * if len < 0 { 2 } else { 1 };
                    let s = raw.get(o..o + l).map(|b| String::from_utf8_lossy(b).trim_end_matches('\0').to_string()).unwrap_or_default();
                    o += l;
                    println!("  referenced swf: {s}");
                }
            }
        }
    }
    let (n, raw) = find(&h.properties, "RawData").and_then(|p| p.array_raw()).context("RawData")?;
    println!("RawData {n} bytes, magic {:?} version {} len {}", std::str::from_utf8(&raw[0..3]).unwrap_or("?"), raw[3], u32::from_le_bytes(raw[4..8].try_into()?));
    if let Some(out) = a.get(2).filter(|s| *s != "-") {
        std::fs::write(out, &raw[..n])?;
    }
    // `--flatten EXPORT`: the exported sprite's textured quads at the stage origin (path,
    // stage-pixel corners), as the HUD flattens them.
    if let Some(ex) = a.iter().position(|x| x == "--flatten").and_then(|i| a.get(i + 1)) {
        let m = dishonored_assets::swf::Movie::parse(&raw[..n])?;
        let id = *m.exports.get(ex.as_str()).context("no such export")?;
        let mut qs = Vec::new();
        m.flatten(id, Default::default(), Default::default(), ex, &|_, _| 0, &mut qs);
        for q in qs {
            println!("{} char {} dst {:?}", q.path, q.character, q.dst);
        }
    }
    // `--actions SPRITE`: disassemble the DoInitAction bytecode of an AS2 class sprite.
    if let Some(id) = a.iter().position(|x| x == "--actions").and_then(|i| a.get(i + 1)).and_then(|s| s.parse::<u16>().ok()) {
        actions(&raw[..n], id);
    }
    if a.iter().any(|x| x == "--tree") {
        tree(&raw[..n])?;
    }
    if a.iter().any(|x| x == "--texts") {
        // Text fields, imports and every sprite with frame labels (menu states).
        let m = dishonored_assets::swf::Movie::parse(&raw[..n])?;
        for (id, (url, name)) in &m.imports {
            println!("import {id}: {name} from {url}");
        }
        for (id, t) in &m.edit_texts {
            println!("text {id}: font {:?} ({:?}) {}px colour {:?} align {} var {:?} initial {:?} bounds {:?}", t.font, t.font.and_then(|f| m.font_name(f)), t.height, t.color, t.align, t.variable, t.initial, (t.bounds.xmin / 20.0, t.bounds.ymin / 20.0, t.bounds.xmax / 20.0, t.bounds.ymax / 20.0));
        }
        for (id, s) in &m.sprites {
            let l = dishonored_assets::swf::Movie::labels(s);
            if !l.is_empty() {
                println!("labels {id} ({} frames): {:?}", s.frames.len(), l);
            }
        }
        for (id, b) in &m.buttons {
            println!("button {id}: {:?}", b.iter().map(|r| (r.states, r.character)).collect::<Vec<_>>());
        }
    }
    if let Some(dir) = a.iter().position(|x| x == "--fonts").and_then(|i| a.get(i + 1)) {
        fonts(&raw[..n], dir)?;
    }
    if let Some(ids) = a.iter().position(|x| x == "--sub").and_then(|i| a.get(i + 1)) {
        subs(&raw[..n], ids)?;
    }
    if let Some(id) = a.iter().position(|x| x == "--sprite").and_then(|i| a.get(i + 1)).and_then(|s| s.parse().ok()) {
        sprite(&raw[..n], id)?;
    }
    Ok(())
}

/// Print the parsed structure of a movie (called when a 4th arg "--tree" is given).
pub fn tree(raw: &[u8]) -> Result<()> {
    use dishonored_assets::swf::{FrameOp, Movie};
    let m = Movie::parse(raw)?;
    println!("version {} stage {:?} fps {}", m.version, m.frame_size, m.frame_rate);
    println!("tags {:?}", m.tag_counts);
    for (id, im) in &m.images {
        println!("image {id}: {} file {} {}x{}", im.export_name, im.file_name, im.width, im.height);
    }
    println!("{} sub-images, {} shapes, {} sprites, {} texts", m.sub_images.len(), m.shapes.len(), m.sprites.len(), m.texts.len());
    for (n, id) in &m.exports {
        println!("export {n} -> {id}");
    }
    for (fi, f) in m.root.frames.iter().enumerate().take(3) {
        for op in f {
            if let FrameOp::Place(p) = op {
                println!("root f{fi} depth {} char {:?} name {:?} m {:?}", p.depth, p.character, p.name, p.matrix.map(|m| (m.tx / 20.0, m.ty / 20.0, m.a, m.d)));
            }
        }
    }
    Ok(())
}

/// Describe one sprite: frame count, labels and frame-0 placements (`--sprite ID`).
pub fn sprite(raw: &[u8], id: u16) -> Result<()> {
    use dishonored_assets::swf::{FrameOp, Movie};
    let m = Movie::parse(raw)?;
    if let Some(sh) = m.shapes.get(&id) {
        println!("shape {id}: bounds {:?} fills {:?}", sh.bounds, sh.fills);
        return Ok(());
    }
    let s = m.sprites.get(&id).context("no sprite")?;
    println!("sprite {id}: {} frames", s.frames.len());
    for (fi, f) in s.frames.iter().enumerate() {
        for op in f {
            match op {
                FrameOp::Label(l) => println!("  label f{fi} {l}"),
                FrameOp::Place(p) if fi < 2 || p.character.is_some() => println!("  f{fi} place d{} ch {:?} name {:?} m {:?} c {:?}", p.depth, p.character, p.name, p.matrix.map(|m| (m.tx / 20.0, m.ty / 20.0, m.a, m.b, m.c, m.d)), p.color.map(|c| (c.mul, c.add))),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Print sub-image definitions for the given ids (`--sub a,b,c`).
pub fn subs(raw: &[u8], ids: &str) -> Result<()> {
    let m = dishonored_assets::swf::Movie::parse(raw)?;
    for id in ids.split(',').filter_map(|s| s.parse::<u16>().ok()) {
        println!("sub {id}: {:?} image {:?}", m.sub_images.get(&id), m.images.get(&(id as u32)).map(|i| &i.file_name));
    }
    println!("first subs: {:?}", m.sub_images.iter().take(5).collect::<Vec<_>>());
    Ok(())
}

/// Convert embedded fonts to TTF files in a scratch directory (`--fonts DIR`).
pub fn fonts(raw: &[u8], dir: &str) -> Result<()> {
    let m = dishonored_assets::swf::Movie::parse(raw)?;
    std::fs::create_dir_all(dir)?;
    for f in &m.fonts {
        let ttf = f.to_ttf(&f.name);
        let path = std::path::Path::new(dir).join(format!("{}.ttf", f.name.replace([' ', '-'], "_")));
        std::fs::write(&path, &ttf)?;
        println!("font {:?}: {} glyphs, ascent {} descent {}, {} bytes -> {}", f.name, f.glyphs.len(), f.ascent, f.descent, ttf.len(), path.display());
    }
    Ok(())
}

/// Walk the top-level tags and disassemble the `DoInitAction` (59) of sprite `id`: one AS2
/// action per line, with the constant pool resolved in pushes.
fn actions(raw: &[u8], id: u16) {
    let nbits = (raw[8] >> 3) as usize;
    let mut pos = 8 + (5 + 4 * nbits).div_ceil(8) + 4;
    while pos + 2 <= raw.len() {
        let h = u16::from_le_bytes([raw[pos], raw[pos + 1]]);
        pos += 2;
        let code = h >> 6;
        let mut len = (h & 0x3f) as usize;
        if len == 0x3f {
            len = u32::from_le_bytes(raw[pos..pos + 4].try_into().unwrap()) as usize;
            pos += 4;
        }
        let body = &raw[pos..(pos + len).min(raw.len())];
        pos += len;
        if code == 0 {
            break;
        }
        if code == 59 && u16::from_le_bytes([body[0], body[1]]) == id {
            disasm(&body[2..]);
        }
    }
}

fn cstr(b: &[u8], o: &mut usize) -> String {
    let s = *o;
    while *o < b.len() && b[*o] != 0 {
        *o += 1;
    }
    let r = String::from_utf8_lossy(&b[s..*o]).to_string();
    *o += 1;
    r
}

fn op_name(op: u8) -> &'static str {
    match op {
        0x04 => "NextFrame", 0x05 => "PrevFrame", 0x06 => "Play", 0x07 => "Stop", 0x0A => "Add", 0x0B => "Subtract", 0x0C => "Multiply", 0x0D => "Divide",
        0x0E => "Equals", 0x0F => "Less", 0x10 => "And", 0x11 => "Or", 0x12 => "Not", 0x13 => "StringEquals", 0x14 => "StringLength", 0x15 => "StringExtract",
        0x17 => "Pop", 0x18 => "ToInteger", 0x1C => "GetVariable", 0x1D => "SetVariable", 0x20 => "SetTarget2", 0x21 => "StringAdd", 0x22 => "GetProperty",
        0x23 => "SetProperty", 0x24 => "CloneSprite", 0x26 => "Trace", 0x2B => "CastOp", 0x2C => "ImplementsOp", 0x30 => "RandomNumber", 0x34 => "GetTime",
        0x3A => "Delete", 0x3B => "Delete2", 0x3C => "DefineLocal", 0x3D => "CallFunction", 0x3E => "Return", 0x3F => "Modulo", 0x40 => "NewObject",
        0x41 => "DefineLocal2", 0x42 => "InitArray", 0x43 => "InitObject", 0x44 => "TypeOf", 0x45 => "TargetPath", 0x46 => "Enumerate", 0x47 => "Add2",
        0x48 => "Less2", 0x49 => "Equals2", 0x4A => "ToNumber", 0x4B => "ToString", 0x4C => "PushDuplicate", 0x4D => "StackSwap", 0x4E => "GetMember",
        0x4F => "SetMember", 0x50 => "Increment", 0x51 => "Decrement", 0x52 => "CallMethod", 0x53 => "NewMethod", 0x54 => "InstanceOf", 0x55 => "Enumerate2",
        0x60 => "BitAnd", 0x61 => "BitOr", 0x62 => "BitXor", 0x63 => "BitLShift", 0x64 => "BitRShift", 0x65 => "BitURShift", 0x66 => "StrictEquals",
        0x67 => "Greater", 0x68 => "StringGreater", 0x69 => "Extends", 0x81 => "GotoFrame", 0x83 => "GetURL", 0x87 => "StoreRegister", 0x88 => "ConstantPool",
        0x8B => "SetTarget", 0x8C => "GotoLabel", 0x8E => "DefineFunction2", 0x94 => "With", 0x96 => "Push", 0x99 => "Jump", 0x9A => "GetURL2",
        0x9B => "DefineFunction", 0x9D => "If", 0x9F => "GotoFrame2", _ => "?",
    }
}

fn push_args(data: &[u8], pool: &[String]) -> String {
    let mut o = 0;
    let mut v = Vec::new();
    let rd4 = |o: usize| -> [u8; 4] { data[o..o + 4].try_into().unwrap() };
    while o < data.len() {
        let t = data[o];
        o += 1;
        v.push(match t {
            0 => format!("\"{}\"", cstr(data, &mut o)),
            1 => {
                o += 4;
                format!("{}", f32::from_le_bytes(rd4(o - 4)))
            }
            2 => "null".into(),
            3 => "undefined".into(),
            4 => {
                o += 1;
                format!("r{}", data[o - 1])
            }
            5 => {
                o += 1;
                format!("{}", data[o - 1] != 0)
            }
            6 => {
                let mut x = [0u8; 8];
                x[..4].copy_from_slice(&data[o + 4..o + 8]);
                x[4..].copy_from_slice(&data[o..o + 4]);
                o += 8;
                format!("{}", f64::from_le_bytes(x))
            }
            7 => {
                o += 4;
                format!("{}", i32::from_le_bytes(rd4(o - 4)))
            }
            8 => {
                o += 1;
                format!("'{}'", pool.get(data[o - 1] as usize).cloned().unwrap_or_default())
            }
            _ => {
                o += 2;
                format!("'{}'", pool.get(u16::from_le_bytes([data[o - 2], data[o - 1]]) as usize).cloned().unwrap_or_default())
            }
        });
    }
    v.join(", ")
}

fn disasm(b: &[u8]) {
    let mut pool: Vec<String> = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let op = b[i];
        let at = i;
        i += 1;
        let (data, next) = if op >= 0x80 {
            let l = u16::from_le_bytes([b[i], b[i + 1]]) as usize;
            (&b[i + 2..(i + 2 + l).min(b.len())], i + 2 + l)
        } else {
            (&b[0..0], i)
        };
        let arg = match op {
            0x88 => {
                let n = u16::from_le_bytes([data[0], data[1]]) as usize;
                let mut o = 2;
                pool = (0..n).map(|_| cstr(data, &mut o)).collect();
                format!("{} strings", pool.len())
            }
            0x96 => push_args(data, &pool),
            0x87 => format!("r{}", data[0]),
            0x99 | 0x9D => format!("-> {}", next as i64 + i16::from_le_bytes([data[0], data[1]]) as i64),
            0x8E => {
                let mut o = 0;
                let nm = cstr(data, &mut o);
                let np = u16::from_le_bytes([data[o], data[o + 1]]) as usize;
                o += 5;
                let ps: Vec<String> = (0..np)
                    .map(|_| {
                        o += 1;
                        let r = data[o - 1];
                        format!("r{r}:{}", cstr(data, &mut o))
                    })
                    .collect();
                format!("{nm}({}) size {}", ps.join(", "), u16::from_le_bytes([data[o], data[o + 1]]))
            }
            0x9B => {
                let mut o = 0;
                let nm = cstr(data, &mut o);
                let np = u16::from_le_bytes([data[o], data[o + 1]]) as usize;
                o += 2;
                let ps: Vec<String> = (0..np).map(|_| cstr(data, &mut o)).collect();
                format!("{nm}({})", ps.join(", "))
            }
            _ if op >= 0x80 => format!("{data:?}"),
            _ => String::new(),
        };
        println!("{at:5}: {} {arg}", if op == 0 { "End" } else { op_name(op) });
        i = next;
    }
}
