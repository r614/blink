//! `level_volume <package.upk> [sample_size]`: locate the precomputed light volume in a level's
//! `Level` export (research). Scans the native data for an array whose elements look like
//! `FVolumeLightingSample`s (position, radius, packed directions and RGBE radiances) and prints
//! the candidates and a few decoded samples. Output is derived from retail data: keep it out of
//! the repository.
use anyhow::Result;
use ue3_package::{ObjRef, Package};

fn f32_at(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn plausible(d: &[u8], o: usize) -> bool {
    if o + 16 > d.len() {
        return false;
    }
    let p = [f32_at(d, o), f32_at(d, o + 4), f32_at(d, o + 8)];
    let r = f32_at(d, o + 12);
    p.iter().all(|v| v.is_finite() && v.abs() < 1.0e6) && r.is_finite() && r > 1.0 && r < 1.0e5
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let pkg = Package::open(&a[0])?;
    let sizes: Vec<usize> = a.get(1).map(|s| vec![s.parse().unwrap()]).unwrap_or_else(|| vec![33, 36, 37, 40]);
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "Level" {
            continue;
        }
        let d = pkg.export_data(i)?;
        let native = pkg.read_object(i).map(|h| h.native_offset).unwrap_or(0);
        println!("=== [{i}] {} {} bytes, native at {native}", pkg.obj_path(ObjRef::Export(i)), d.len());
        for o in native..d.len().saturating_sub(4) {
            let n = u32_at(d, o) as usize;
            if !(50..=500_000).contains(&n) {
                continue;
            }
            for &s in &sizes {
                if o + 4 + n * s > d.len() {
                    continue;
                }
                let checks = [0, 1, 2, n / 2, n - 1];
                if checks.iter().all(|&k| plausible(d, o + 4 + k * s)) {
                    println!("candidate at {o}: count {n} x {s} bytes, ends at {} (export end {})", o + 4 + n * s, d.len());
                    if o >= 33 {
                        // FBox (min, max, valid byte) + spacing before the array in UE3's layout.
                        let b = o - 29;
                        println!(
                            "  preceding: init {} box [{:.0} {:.0} {:.0}]..[{:.0} {:.0} {:.0}] valid {} spacing {}",
                            u32_at(d, b - 4),
                            f32_at(d, b),
                            f32_at(d, b + 4),
                            f32_at(d, b + 8),
                            f32_at(d, b + 12),
                            f32_at(d, b + 16),
                            f32_at(d, b + 20),
                            d[b + 24],
                            f32_at(d, b + 25)
                        );
                    }
                    for k in [0, 1, 2, n / 2] {
                        let e = o + 4 + k * s;
                        let bytes: Vec<String> = d[e + 16..e + s].iter().map(|b| format!("{b:02x}")).collect();
                        println!(
                            "  [{k}] pos ({:.1}, {:.1}, {:.1}) r {:.1} rest {}",
                            f32_at(d, e),
                            f32_at(d, e + 4),
                            f32_at(d, e + 8),
                            f32_at(d, e + 12),
                            bytes.join(" ")
                        );
                    }
                }
            }
        }
    }
    Ok(())
}
