//! `export_hex <package.upk> <class-or-path substring> [max] [bytes]`: hex dump of matching
//! exports' serialized data (research; output is derived from retail data, keep it out of the
//! repository).
use anyhow::Result;
use ue3_package::{ObjRef, Package};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let pkg = Package::open(&a[0])?;
    let filt = a[1].to_ascii_lowercase();
    let max: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);
    let bytes: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(512);
    let mut n = 0;
    for i in 0..pkg.exports.len() {
        let class = pkg.export_class(i);
        let path = pkg.obj_path(ObjRef::Export(i));
        if !class.to_ascii_lowercase().contains(&filt) && !path.to_ascii_lowercase().contains(&filt) {
            continue;
        }
        let d = pkg.export_data(i)?;
        println!("=== [{i}] {path} ({class}) {} bytes", d.len());
        for (row, chunk) in d[..d.len().min(bytes)].chunks(16).enumerate() {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            let asc: String = chunk.iter().map(|&b| if (32..127).contains(&b) { b as char } else { '.' }).collect();
            println!("{:06x}: {:<48} {asc}", row * 16, hex.join(" "));
        }
        n += 1;
        if n >= max {
            break;
        }
    }
    Ok(())
}
