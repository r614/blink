use dishonored_ir::TextureFormat;

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8]
}

/// Decode one BC1 colour block into 16 RGBA texels.
fn bc1_block(b: &[u8], out: &mut [[u8; 4]; 16], force4: bool) {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let mix = |a: u8, b: u8, wa: u32, wb: u32, d: u32| ((a as u32 * wa + b as u32 * wb) / d) as u8;
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [p0[0], p0[1], p0[2], 255];
    pal[1] = [p1[0], p1[1], p1[2], 255];
    if c0 > c1 || force4 {
        pal[2] = [mix(p0[0], p1[0], 2, 1, 3), mix(p0[1], p1[1], 2, 1, 3), mix(p0[2], p1[2], 2, 1, 3), 255];
        pal[3] = [mix(p0[0], p1[0], 1, 2, 3), mix(p0[1], p1[1], 1, 2, 3), mix(p0[2], p1[2], 1, 2, 3), 255];
    } else {
        pal[2] = [mix(p0[0], p1[0], 1, 1, 2), mix(p0[1], p1[1], 1, 1, 2), mix(p0[2], p1[2], 1, 1, 2), 255];
        pal[3] = [0, 0, 0, 0];
    }
    let idx = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    for i in 0..16 {
        out[i] = pal[((idx >> (2 * i)) & 3) as usize];
    }
}

fn bc3_alpha(b: &[u8], out: &mut [[u8; 4]; 16]) {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut pal = [0u32; 8];
    pal[0] = a0;
    pal[1] = a1;
    if a0 > a1 {
        for i in 1..7 {
            pal[i + 1] = ((7 - i as u32) * a0 + i as u32 * a1) / 7;
        }
    } else {
        for i in 1..5 {
            pal[i + 1] = ((5 - i as u32) * a0 + i as u32 * a1) / 5;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits: u64 = 0;
    for i in 0..6 {
        bits |= (b[2 + i] as u64) << (8 * i);
    }
    for i in 0..16 {
        out[i][3] = pal[((bits >> (3 * i)) & 7) as usize] as u8;
    }
}

pub fn decode(fmt: TextureFormat, w: u32, h: u32, data: &[u8]) -> Option<Vec<u8>> {
    let mut img = vec![0u8; (w * h * 4) as usize];
    let put = |img: &mut Vec<u8>, bx: u32, by: u32, t: &[[u8; 4]; 16]| {
        for i in 0..16u32 {
            let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
            if x < w && y < h {
                let o = ((y * w + x) * 4) as usize;
                img[o..o + 4].copy_from_slice(&t[i as usize]);
            }
        }
    };
    let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
    match fmt {
        TextureFormat::Bc1 | TextureFormat::Bc3 | TextureFormat::Bc2 => {
            let bs = if matches!(fmt, TextureFormat::Bc1) { 8 } else { 16 };
            for by in 0..bh {
                for bx in 0..bw {
                    let o = ((by * bw + bx) * bs) as usize;
                    let blk = data.get(o..o + bs as usize)?;
                    let mut t = [[0u8; 4]; 16];
                    match fmt {
                        TextureFormat::Bc1 => bc1_block(blk, &mut t, false),
                        TextureFormat::Bc3 => {
                            bc1_block(&blk[8..], &mut t, true);
                            bc3_alpha(blk, &mut t);
                        }
                        _ => {
                            bc1_block(&blk[8..], &mut t, true);
                            for i in 0..16 {
                                let nib = (blk[i / 2] >> (4 * (i % 2))) & 15;
                                t[i][3] = nib * 17;
                            }
                        }
                    }
                    put(&mut img, bx, by, &t);
                }
            }
        }
        TextureFormat::Bgra8 => {
            for (i, px) in data.chunks_exact(4).take((w * h) as usize).enumerate() {
                img[i * 4..i * 4 + 4].copy_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
        TextureFormat::R8 => {
            for (i, v) in data.iter().take((w * h) as usize).enumerate() {
                img[i * 4..i * 4 + 4].copy_from_slice(&[*v, *v, *v, 255]);
            }
        }
        _ => return None,
    }
    Some(img)
}

