use anyhow::Result;
use ue3_package::{ObjRef, Package};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: package_dump <file.upk> [--names] [--imports] [--exports] [--hex N]");
        std::process::exit(2);
    };
    let pkg = Package::open(path)?;
    let s = &pkg.summary;
    println!("package: {}", pkg.name);
    println!(
        "version {}/{} engine {} cooker {} flags {:#010x} compression {:#x} chunks {}",
        s.file_version,
        s.licensee_version,
        s.engine_version,
        s.cooker_version,
        s.package_flags,
        s.compression_flags,
        s.compressed_chunks.len()
    );
    println!(
        "names {} @{:#x}  imports {} @{:#x}  exports {} @{:#x}  depends @{:#x}  summary end @{:#x}",
        s.name_count,
        s.name_offset,
        s.import_count,
        s.import_offset,
        s.export_count,
        s.export_offset,
        s.depends_offset,
        s.end_offset
    );
    println!("decompressed size {}", pkg.data.len());

    let flag = |f: &str| args.iter().any(|a| a == f);
    if flag("--names") {
        for (i, n) in pkg.names.iter().enumerate() {
            println!("name[{i}] {} flags={:#x}", n.name, n.flags);
        }
    }
    if flag("--imports") {
        for i in 0..pkg.imports.len() {
            println!(
                "import[{i}] {} ({})",
                pkg.obj_path(ObjRef::Import(i)),
                pkg.class_name(ObjRef::Import(i))
            );
        }
    }
    if flag("--exports") {
        for (i, e) in pkg.exports.iter().enumerate() {
            println!(
                "export[{i}] {} ({}) size={} off={:#x} flags={:#x}",
                pkg.obj_path(ObjRef::Export(i)),
                pkg.export_class(i),
                e.serial_size,
                e.serial_offset,
                e.object_flags
            );
        }
    }
    if let Some(p) = args.iter().position(|a| a == "--hex") {
        let i: usize = args[p + 1].parse()?;
        let d = pkg.export_data(i)?;
        for (row, chunk) in d.chunks(16).enumerate().take(64) {
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            println!("{:06x}: {}", row * 16, hex.join(" "));
        }
    }
    Ok(())
}
