//! Precomputed lighting on static mesh components: the per-LOD `FStaticMeshComponentLODInfo`
//! native data (shadow maps + `FLightMapRef`), Dishonored's two-texture directional
//! lightmap encoding, and baking it into linear irradiance atlases for the renderer.
//!
//! Layout (verified on L_Boyle_Ext_Env, see research/dishonored/PACKAGE_FORMAT.md):
//! ```text
//! StaticMeshComponent native data (after tagged properties):
//!   TArray<LODInfo>:
//!     TArray<UShadowMap2D*> ShadowMaps
//!     TArray<UShadowMap1D*> ShadowVertexBuffers
//!     FLightMapRef: u32 Type (0 none, 1 = 1D/vertex, 2 = 2D/texture)
//!       FLightMap: TArray<FGuid> LightGuids
//!       FLightMap2D: 3 x { UTexture2D* Texture, FVector Scale }   <- FVector, not FVector4
//!                    FVector2D CoordinateScale, FVector2D CoordinateBias
//!     u8 bLoadVertexColorData [+ FColorVertexBuffer when non-zero]
//! ```
//! Texture slot 0 is `NormalizedAverageColor*` (scale (1,1,1)), slot 1 is
//! `DirectionalMaxComponent*` (HDR scale), slot 2 (the UE3 "simple" coefficient) is null
//! in every cooked component on Boyle Ext.

use crate::texture::read_bulk_header;
use anyhow::{Result, bail, ensure};
use ue3_package::reader::Reader;

#[derive(Debug, Clone, Default)]
pub struct LightMap2DRef {
    /// Object references (package-relative) of the three coefficient textures.
    pub textures: [i32; 3],
    /// Per-coefficient RGB scale applied after sampling.
    pub scales: [[f32; 3]; 3],
    /// Lightmap UV = UV1 * coordinate_scale + coordinate_bias.
    pub coordinate_scale: [f32; 2],
    pub coordinate_bias: [f32; 2],
    pub light_guids: usize,
}

#[derive(Debug, Clone, Default)]
pub enum LightMapRef {
    #[default]
    None,
    /// Vertex lightmap (`FLightMap1D`); payload not decoded for rendering yet.
    Vertex { light_guids: usize },
    Texture(LightMap2DRef),
}

#[derive(Debug, Clone, Default)]
pub struct LodLighting {
    pub shadow_maps: Vec<i32>,
    pub shadow_vertex_buffers: Vec<i32>,
    pub lightmap: LightMapRef,
}

fn obj_array(r: &mut Reader) -> Result<Vec<i32>> {
    let n = r.count(4)?;
    (0..n).map(|_| r.i32()).collect()
}

fn vec3(r: &mut Reader) -> Result<[f32; 3]> {
    Ok([r.f32()?, r.f32()?, r.f32()?])
}

fn read_lightmap(r: &mut Reader) -> Result<LightMapRef> {
    let kind = r.u32()?;
    match kind {
        0 => Ok(LightMapRef::None),
        1 => {
            let light_guids = r.count(16)?;
            r.skip(light_guids * 16)?;
            let _owner = r.i32()?;
            // DirectionalSamples (bulk), coefficient scales, SimpleSamples (bulk).
            let _dir = read_bulk_header(r)?;
            r.skip(2 * 12)?;
            let _simple = read_bulk_header(r)?;
            Ok(LightMapRef::Vertex { light_guids })
        }
        2 => {
            let light_guids = r.count(16)?;
            r.skip(light_guids * 16)?;
            let mut lm = LightMap2DRef { light_guids, ..Default::default() };
            for i in 0..3 {
                lm.textures[i] = r.i32()?;
                lm.scales[i] = vec3(r)?;
            }
            lm.coordinate_scale = [r.f32()?, r.f32()?];
            lm.coordinate_bias = [r.f32()?, r.f32()?];
            Ok(LightMapRef::Texture(lm))
        }
        k => bail!("unknown lightmap type {k}"),
    }
}

/// Parse the `LODData` array that follows a StaticMeshComponent's tagged properties.
/// Strict: the stream must end exactly at the end of the export.
pub fn read_static_mesh_component_lods(data: &[u8], native_offset: usize) -> Result<Vec<LodLighting>> {
    let mut r = Reader::at(data, native_offset);
    let n = r.count(8)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let shadow_maps = obj_array(&mut r)?;
        let shadow_vertex_buffers = obj_array(&mut r)?;
        let lightmap = read_lightmap(&mut r)?;
        let load_colors = r.u8()?;
        if load_colors != 0 {
            // FColorVertexBuffer: stride, vertex count, bulk array of FColor.
            let _stride = r.u32()?;
            let nverts = r.u32()?;
            if nverts > 0 {
                r.bulk_array()?;
            }
        }
        out.push(LodLighting { shadow_maps, shadow_vertex_buffers, lightmap });
    }
    ensure!(r.pos() == data.len(), "LODData ended at {:#x}, export ends at {:#x}", r.pos(), data.len());
    Ok(out)
}

/// Decode BC1/DXT1 into RGB8 (alpha ignored).
pub fn decode_bc1(data: &[u8], width: u32, height: u32) -> Vec<[u8; 3]> {
    let (w, h) = (width as usize, height as usize);
    let mut out = vec![[0u8; 3]; w * h];
    let bw = w.div_ceil(4);
    let expand = |c: u16| -> [u32; 3] {
        let r = ((c >> 11) & 31) as u32;
        let g = ((c >> 5) & 63) as u32;
        let b = (c & 31) as u32;
        [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
    };
    for (bi, block) in data.chunks_exact(8).enumerate() {
        let (bx, by) = (bi % bw, bi / bw);
        if by * 4 >= h {
            break;
        }
        let c0 = u16::from_le_bytes([block[0], block[1]]);
        let c1 = u16::from_le_bytes([block[2], block[3]]);
        let (a, b) = (expand(c0), expand(c1));
        let mut pal = [[0u32; 3]; 4];
        pal[0] = a;
        pal[1] = b;
        for k in 0..3 {
            if c0 > c1 {
                pal[2][k] = (2 * a[k] + b[k]) / 3;
                pal[3][k] = (a[k] + 2 * b[k]) / 3;
            } else {
                pal[2][k] = (a[k] + b[k]) / 2;
                pal[3][k] = 0;
            }
        }
        let bits = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        for py in 0..4 {
            for px in 0..4 {
                let (x, y) = (bx * 4 + px, by * 4 + py);
                if x < w && y < h {
                    let idx = (bits >> (2 * (py * 4 + px))) & 3;
                    let c = pal[idx as usize];
                    out[y * w + x] = [c[0] as u8, c[1] as u8, c[2] as u8];
                }
            }
        }
    }
    out
}

pub fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Pack linear RGB into `RGB9E5` (shared exponent), matching `wgpu::TextureFormat::Rgb9e5Ufloat`.
pub fn pack_rgb9e5(rgb: [f32; 3]) -> u32 {
    const N: i32 = 9;
    const B: i32 = 15;
    const EMAX: i32 = 31;
    let max_val = ((1 << N) - 1) as f32 / (1 << N) as f32 * 2f32.powi(EMAX - B);
    let c = rgb.map(|v| if v.is_finite() { v.clamp(0.0, max_val) } else { 0.0 });
    let maxc = c[0].max(c[1]).max(c[2]);
    if maxc <= 0.0 {
        return 0;
    }
    let mut exp = (maxc.log2().floor() as i32).max(-B - 1) + 1 + B;
    let mut denom = 2f32.powi(exp - B - N);
    if (maxc / denom + 0.5).floor() as i32 == 1 << N {
        denom *= 2.0;
        exp += 1;
    }
    let q = c.map(|v| ((v / denom + 0.5).floor() as u32).min((1 << N) - 1));
    q[0] | (q[1] << 9) | (q[2] << 18) | ((exp as u32) << 27)
}

pub fn unpack_rgb9e5(v: u32) -> [f32; 3] {
    let e = (v >> 27) as i32 - 15 - 9;
    let s = 2f32.powi(e);
    [(v & 511) as f32 * s, ((v >> 9) & 511) as f32 * s, ((v >> 18) & 511) as f32 * s]
}

/// Reconstruct linear irradiance for a flat (geometric) normal from Dishonored's two
/// lightmap coefficients. `avg` is the sRGB-decoded NormalizedAverageColor texel and `dir`
/// the sRGB-decoded DirectionalMaxComponent texel (one intensity per tangent-space basis
/// direction in R, G, B); each is multiplied by its per-lightmap scale.
///
/// APPROXIMATION: the original shader weights the three basis intensities by the
/// per-pixel normal; for an unperturbed normal all three weights are equal, so the
/// average is used. Normal-map interaction with the directional lightmap is lost.
pub fn flat_irradiance(avg: [f32; 3], dir: [f32; 3], s_avg: [f32; 3], s_dir: [f32; 3]) -> [f32; 3] {
    let intensity = (dir[0] * s_dir[0] + dir[1] * s_dir[1] + dir[2] * s_dir[2]) / 3.0;
    [avg[0] * s_avg[0] * intensity, avg[1] * s_avg[1] * intensity, avg[2] * s_avg[2] * intensity]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb9e5_roundtrip() {
        for v in [[0.0, 0.0, 0.0], [1.0, 0.5, 0.25], [7.5, 0.01, 3.0], [0.001, 0.002, 0.0005]] {
            let r = unpack_rgb9e5(pack_rgb9e5(v));
            for k in 0..3 {
                assert!((r[k] - v[k]).abs() <= v[0].max(v[1]).max(v[2]) / 256.0 + 1e-6, "{v:?} -> {r:?}");
            }
        }
    }

    #[test]
    fn bc1_solid_block() {
        // c0 = c1 = pure red (0xF800), all indices 0.
        let block = [0x00, 0xF8, 0x00, 0xF8, 0, 0, 0, 0];
        let px = decode_bc1(&block, 4, 4);
        assert!(px.iter().all(|p| *p == [255, 0, 0]));
    }
}
