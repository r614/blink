//! Research: look for the Wwise Vorbis codebook table inside Dishonored.exe
//! (runs of ascending pointers with small strides).
fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        r"C:\Program Files\GOG Galaxy\Games\Dishonored\Binaries\Win32\Dishonored.exe".into()
    });
    let b = std::fs::read(&path).unwrap();
    let r16 = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let rd = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let pe = rd(0x3c) as usize;
    let opt = pe + 24;
    let base = rd(opt + 28);
    println!("image base 0x{base:x}");
    let mut secs = Vec::new();
    for i in 0..r16(pe + 6) {
        let s = opt + r16(pe + 20) + i * 40;
        println!(
            "{:8} va 0x{:x} vsz 0x{:x} raw 0x{:x} rsz 0x{:x}",
            String::from_utf8_lossy(&b[s..s + 8]),
            rd(s + 12),
            rd(s + 8),
            rd(s + 20),
            rd(s + 16)
        );
        secs.push((rd(s + 12), rd(s + 8), rd(s + 20)));
    }
    let to_off = |va: u32| {
        let rva = va.wrapping_sub(base);
        secs.iter()
            .find(|&&(sva, vsz, _)| rva >= sva && rva < sva + vsz)
            .map(|&(sva, _, raw)| (rva - sva + raw) as usize)
    };
    let mut o = 0;
    while o + 8 < b.len() {
        let v0 = rd(o);
        if to_off(v0).is_some() {
            let mut n = 1;
            let mut p = o + 4;
            while p + 4 <= b.len() {
                let d = rd(p).wrapping_sub(rd(p - 4));
                if d == 0 || d > 2000 {
                    break;
                }
                n += 1;
                p += 4;
            }
            if n >= 300 {
                let t = to_off(v0);
                println!("ptr run at 0x{o:x}: {n} entries, first 0x{v0:x} (file {t:x?}) last 0x{:x}", rd(p - 4));
                if let Some(t) = t {
                    println!("  bytes {:02x?}", &b[t..t + 48]);
                    for i in 0..8 {
                        let a = to_off(rd(o + i * 4)).unwrap();
                        let z = to_off(rd(o + i * 4 + 4)).unwrap();
                        println!(
                            "  cb {i}: stride {} parse {:?}",
                            z - a,
                            dishonored_audio::vorbis::packed_codebook_size(&b[a..])
                        );
                    }
                }
                o = p;
                continue;
            }
        }
        o += 4;
    }
}
