//! `DishonoredAudioVolume` actors of a map (class layout in `DishonoredGame`): brush volumes
//! carrying the ambience state event posted when the listener enters (`m_pSoundEvent`, e.g.
//! `Set_State_Amb_LVL_Boyle_Ext_Streets` / `_Int_Streets` / `_Sewers`), the Wwise environment
//! of the objects inside (`m_Environment`, a name hashed to an Init.bnk effect shareset such as
//! `PC_streets_01` or `Room_Small_Wood`), RTPCs (`m_SoundParameters`) and the
//! `InOutVolume.m_VolumeKind` (exterior by default). Boyle Ext has 45 of them.

use ue3_package::properties::{Value, find};
use ue3_package::{ObjRef, Package};

#[derive(Debug, Clone, Default)]
pub struct AudioVolume {
    pub actor: String,
    /// AkEvent name posted on entry.
    pub event: Option<String>,
    /// Environment name (`fnv1_32` = the shareset id).
    pub environment: Option<String>,
    /// `m_VolumeKind` enum name (`VK_EXTERIOR` when absent: the class default).
    pub kind: String,
    /// (game parameter, value, change rate) of `m_SoundParameters`.
    pub rtpcs: Vec<(String, f32, f32)>,
    /// World-space convex hulls as planes (nx, ny, nz, d): inside when n.p <= d for every plane
    /// of one hull.
    pub hulls: Vec<Vec<[f32; 4]>>,
    /// World bounds (min, max) of the hull vertices (for the smallest-volume rule).
    pub volume: f32,
}

impl AudioVolume {
    pub fn contains(&self, p: [f32; 3]) -> bool {
        self.hulls.iter().any(|h| !h.is_empty() && h.iter().all(|pl| pl[0] * p[0] + pl[1] * p[1] + pl[2] * p[2] <= pl[3] + 1e-3))
    }
}

/// UE3 rotator (pitch, yaw, roll in 65536ths of a turn) to the rotation's axes (x, y, z).
fn rotator_axes(r: [i32; 3]) -> [[f32; 3]; 3] {
    let rad = |u: i32| u as f32 * std::f32::consts::TAU / 65536.0;
    let (sp, cp) = rad(r[0]).sin_cos();
    let (sy, cy) = rad(r[1]).sin_cos();
    let (sr, cr) = rad(r[2]).sin_cos();
    [
        [cp * cy, cp * sy, sp],
        [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp],
        [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp],
    ]
}

pub fn audio_volumes(pkg: &Package) -> Vec<AudioVolume> {
    let mut out = Vec::new();
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "DishonoredAudioVolume" {
            continue;
        }
        let name = pkg.obj_name(ObjRef::Export(i));
        if name.starts_with("Default__") {
            continue;
        }
        let Ok(h) = pkg.read_object(i) else { continue };
        let p = &h.properties;
        let mut v = AudioVolume { actor: pkg.obj_path(ObjRef::Export(i)), kind: "VK_EXTERIOR".into(), ..Default::default() };
        v.event = find(p, "m_pSoundEvent").and_then(|x| x.as_object()).filter(|o| *o != 0).map(|o| pkg.obj_name(ObjRef::from_index(o)));
        v.environment = match find(p, "m_Environment").map(|x| &x.value) {
            Some(Value::Name(n)) if !n.is_empty() && n != "None" => Some(n.clone()),
            _ => None,
        };
        if let Some(Value::Enum(e)) = find(p, "m_VolumeKind").map(|x| &x.value) {
            v.kind = e.clone();
        }
        for s in find(p, "m_SoundParameters").and_then(|x| x.array_structs(pkg).ok()).unwrap_or_default() {
            let n = match find(&s, "m_Parameter").map(|x| &x.value) {
                Some(Value::Name(n)) => n.clone(),
                _ => continue,
            };
            let val = find(&s, "m_fValue").and_then(|x| x.as_float()).unwrap_or(0.0);
            let rate = find(&s, "m_fChangeRate").and_then(|x| x.as_float()).unwrap_or(0.0);
            v.rtpcs.push((n, val, rate));
        }
        let loc = find(p, "Location").and_then(|x| x.as_vector()).unwrap_or([0.0; 3]);
        let rot = find(p, "Rotation").and_then(|x| x.as_rotator()).unwrap_or([0; 3]);
        let ds = find(p, "DrawScale").and_then(|x| x.as_float()).unwrap_or(1.0);
        let ds3 = find(p, "DrawScale3D").and_then(|x| x.as_vector()).unwrap_or([1.0; 3]).map(|s| s * ds);
        let pivot = find(p, "PrePivot").and_then(|x| x.as_vector()).unwrap_or([0.0; 3]);
        let ax = rotator_axes(rot);
        let Some(ObjRef::Export(ci)) = find(p, "BrushComponent").and_then(|x| x.as_object()).map(ObjRef::from_index) else { continue };
        let Ok(ch) = pkg.read_object(ci) else { continue };
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        if let Some(Value::Struct(_, geom)) = find(&ch.properties, "BrushAggGeom").map(|x| &x.value) {
            for e in find(geom, "ConvexElems").and_then(|x| x.array_structs(pkg).ok()).unwrap_or_default() {
                // Same transform as the forbidden zones (dishonored_assets::ai_level): local
                // q = S^-1 R^-1 (x - loc) + pivot, plane n.q <= w (serialized W, X, Y, Z).
                let Some((n, raw)) = find(&e, "FacePlaneData").and_then(|x| x.array_raw()) else { continue };
                let mut planes = Vec::new();
                for k in 0..n {
                    let Some(b) = raw.get(k * 16..k * 16 + 16) else { break };
                    let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
                    let nl = [f(4), f(8), f(12)];
                    let w = f(0);
                    let ns = [nl[0] / ds3[0].max(1e-6), nl[1] / ds3[1].max(1e-6), nl[2] / ds3[2].max(1e-6)];
                    let nw = [0, 1, 2].map(|j| ax[0][j] * ns[0] + ax[1][j] * ns[1] + ax[2][j] * ns[2]);
                    let d = w - (nl[0] * pivot[0] + nl[1] * pivot[1] + nl[2] * pivot[2]) + (nw[0] * loc[0] + nw[1] * loc[1] + nw[2] * loc[2]);
                    planes.push([nw[0], nw[1], nw[2], d]);
                }
                if let Some((nv, raw)) = find(&e, "VertexData").and_then(|x| x.array_raw()) {
                    for k in 0..nv {
                        let Some(b) = raw.get(k * 12..k * 12 + 12) else { break };
                        let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
                        let q = [(f(0) - pivot[0]) * ds3[0], (f(4) - pivot[1]) * ds3[1], (f(8) - pivot[2]) * ds3[2]];
                        for j in 0..3 {
                            let w = loc[j] + ax[0][j] * q[0] + ax[1][j] * q[1] + ax[2][j] * q[2];
                            lo[j] = lo[j].min(w);
                            hi[j] = hi[j].max(w);
                        }
                    }
                }
                if !planes.is_empty() {
                    v.hulls.push(planes);
                }
            }
        }
        v.volume = if lo[0] <= hi[0] { (hi[0] - lo[0]) * (hi[1] - lo[1]) * (hi[2] - lo[2]) } else { f32::MAX };
        if !v.hulls.is_empty() {
            out.push(v);
        }
    }
    out
}

/// The audio volume that applies at a point: the smallest containing one (nested rooms
/// inside a street volume). LIKELY: the original keeps per-object lists of touched volumes
/// (`m_ListenerVolumes`, audio propagation cells); the smallest is the innermost room.
pub fn volume_at(vols: &[AudioVolume], p: [f32; 3]) -> Option<usize> {
    vols.iter().enumerate().filter(|(_, v)| v.contains(p)).min_by(|a, b| a.1.volume.total_cmp(&b.1.volume)).map(|x| x.0)
}
