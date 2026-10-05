//! `tex_dump <package> <out_dir> [filter]`: decode Texture2D exports to PNG for inspection
//! (local research only; output must never be committed).
use anyhow::Result;
use dishonored_assets::Content;
use dishonored_assets::texture::read_texture;

use ue3_package::ObjRef;

#[path = "../shared/bcdec.rs"]
mod bcdec;
use bcdec::decode;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    let pkg = c.package(&a[0])?;
    let out = std::path::PathBuf::from(&a[1]);
    std::fs::create_dir_all(&out)?;
    let filt = a.get(2).cloned().unwrap_or_default().to_ascii_lowercase();
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "Texture2D" {
            continue;
        }
        let path = pkg.obj_path(ObjRef::Export(i));
        if !path.to_ascii_lowercase().contains(&filt) {
            continue;
        }
        match read_texture(&c, &pkg, i, 4096) {
            Ok(t) => match decode(t.format, t.width, t.height, &t.mips[0]) {
                Some(rgba) => {
                    let file = out.join(format!("{}.png", path.replace('.', "_")));
                    image::save_buffer(&file, &rgba, t.width, t.height, image::ExtendedColorType::Rgba8)?;
                    println!("{path}: {}x{} {:?}", t.width, t.height, t.format);
                }
                None => println!("{path}: {:?} not decoded", t.format),
            },
            Err(e) => println!("{path}: {e:#}"),
        }
    }
    Ok(())
}
