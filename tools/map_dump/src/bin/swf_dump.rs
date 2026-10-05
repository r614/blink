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
    }
    let (n, raw) = find(&h.properties, "RawData").and_then(|p| p.array_raw()).context("RawData")?;
    println!("RawData {n} bytes, magic {:?} version {} len {}", std::str::from_utf8(&raw[0..3]).unwrap_or("?"), raw[3], u32::from_le_bytes(raw[4..8].try_into()?));
    if let Some(out) = a.get(2).filter(|s| *s != "-") {
        std::fs::write(out, &raw[..n])?;
    }
    if a.iter().any(|x| x == "--tree") {
        tree(&raw[..n])?;
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
