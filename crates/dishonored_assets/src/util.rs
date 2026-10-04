use glam::{Mat4, Vec3, Vec4};

/// `FPackedNormal`: four unsigned bytes, each mapped `b / 127.5 - 1`.
pub fn packed_normal(v: u32) -> Vec3 {
    let f = |s: u32| ((v >> s) & 0xFF) as f32 / 127.5 - 1.0;
    Vec3::new(f(0), f(8), f(16))
}

pub fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let man = (h & 0x3FF) as f32;
    match exp {
        0 => sign * man * 2f32.powi(-24),
        31 => {
            if man == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => sign * (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Unreal rotation units: 65536 per full turn.
pub fn urot_to_rad(u: i32) -> f32 {
    u as f32 * (std::f32::consts::TAU / 65536.0)
}

/// `FRotationMatrix(Pitch, Yaw, Roll)` in UE3's row-vector convention, returned as a
/// glam (column-vector) matrix so `M * v` matches UE3's `v * M`.
pub fn rotator_matrix(pitch: i32, yaw: i32, roll: i32) -> Mat4 {
    let (sp, cp) = urot_to_rad(pitch).sin_cos();
    let (sy, cy) = urot_to_rad(yaw).sin_cos();
    let (sr, cr) = urot_to_rad(roll).sin_cos();
    let x = Vec3::new(cp * cy, cp * sy, sp);
    let y = Vec3::new(sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp);
    let z = Vec3::new(-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), Vec4::W)
}

/// A UE3 `FMatrix` as serialized (4 rows of 4 floats, row-vector convention).
pub fn fmatrix_rows(rows: [[f32; 4]; 4]) -> Mat4 {
    // Row i of the UE matrix is the image of basis vector i, i.e. glam column i.
    Mat4::from_cols_array_2d(&rows)
}
