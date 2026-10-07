//! On-disk cache of the baked lightmap pages (`Importer::bake_lightmaps`): decoding every
//! lightmap and distance-field shadow texture and resampling them into the atlas layout costs
//! most of a level load, and the result depends only on the retail data and the bake code.
//!
//! The file holds a signature of the bake's inputs (every pending component's lightmap and
//! shadow-map references, the packages' export counts, the dominant lights) and the outputs:
//! the pages, each component's `InstanceLightmap` and the bake's statistics. A different
//! signature (or [`FORMAT`]) makes it a miss. Bump [`FORMAT`] when the bake's output changes.
use dishonored_ir::{InstanceLightmap, LightmapAtlas};
use glam::Vec2;
use std::path::Path;

/// Version of the bake output and of this file format.
pub const FORMAT: u32 = 1;
const MAGIC: &[u8; 4] = b"DLMC";

/// FNV-1a 64 over the bake's inputs.
#[derive(Clone, Copy)]
pub struct Signature(pub u64);

impl Default for Signature {
    fn default() -> Self {
        Signature(0xcbf2_9ce4_8422_2325)
    }
}

impl Signature {
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.bytes(&v.to_bits().to_le_bytes());
    }
    pub fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }
}

/// What the bake produces.
#[derive(Default)]
pub struct Baked {
    pub pages: Vec<LightmapAtlas>,
    /// (instance index, its lightmap)
    pub assignments: Vec<(usize, InstanceLightmap)>,
    pub atlases: usize,
    pub shadow_map_components: usize,
    pub local_shadow_map_components: usize,
    pub texture_failures: usize,
    pub errors: Vec<String>,
}

struct W(Vec<u8>);
impl W {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len() as u32);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn u32s(&mut self, v: &[u32]) {
        self.u32(v.len() as u32);
        self.0.reserve(v.len() * 4);
        for x in v {
            self.0.extend_from_slice(&x.to_le_bytes());
        }
    }
    fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
    }
}

struct R<'a>(&'a [u8], usize);
impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.0.get(self.1..self.1.checked_add(n)?)?;
        self.1 += n;
        Some(s)
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u32()? as usize;
        self.take(n).map(|b| String::from_utf8_lossy(b).into_owned())
    }
    fn u32s(&mut self) -> Option<Vec<u32>> {
        let n = self.u32()? as usize;
        let b = self.take(n.checked_mul(4)?)?;
        Some(b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
    }
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let n = self.u32()? as usize;
        self.take(n).map(<[u8]>::to_vec)
    }
}

pub fn save(path: &Path, sig: Signature, b: &Baked) -> std::io::Result<()> {
    let mut w = W(Vec::new());
    w.0.extend_from_slice(MAGIC);
    w.u32(FORMAT);
    w.0.extend_from_slice(&sig.0.to_le_bytes());
    for v in [b.atlases, b.shadow_map_components, b.local_shadow_map_components, b.texture_failures] {
        w.u32(v as u32);
    }
    w.u32(b.errors.len() as u32);
    for e in &b.errors {
        w.str(e);
    }
    w.u32(b.pages.len() as u32);
    for p in &b.pages {
        w.str(&p.source);
        w.u32(p.width);
        w.u32(p.height);
        w.u32s(&p.texels);
        w.u32s(&p.directional);
        w.bytes(&p.shadow);
        w.bytes(&p.shadow_local);
    }
    w.u32(b.assignments.len() as u32);
    for (i, l) in &b.assignments {
        w.u32(*i as u32);
        w.u32(l.atlas as u32);
        for v in [l.uv_min.x, l.uv_min.y, l.uv_max.x, l.uv_max.y] {
            w.f32(v);
        }
        w.u32(l.df_shadow as u32);
        w.u32(l.df_light.map(|x| x as u32).unwrap_or(u32::MAX));
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    // Write then rename, so a concurrent reader never sees a partial file.
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, &w.0)?;
    std::fs::rename(&tmp, path)
}

/// The cached bake, when the file exists and matches `sig`.
pub fn load(path: &Path, sig: Signature) -> Option<Baked> {
    let data = std::fs::read(path).ok()?;
    let mut r = R(&data, 0);
    if r.take(4)? != MAGIC || r.u32()? != FORMAT {
        return None;
    }
    let s = r.take(8)?;
    if u64::from_le_bytes(s.try_into().ok()?) != sig.0 {
        return None;
    }
    let mut b = Baked { atlases: r.u32()? as usize, shadow_map_components: r.u32()? as usize, local_shadow_map_components: r.u32()? as usize, texture_failures: r.u32()? as usize, ..Default::default() };
    for _ in 0..r.u32()? {
        b.errors.push(r.str()?);
    }
    for _ in 0..r.u32()? {
        let source = r.str()?;
        let (width, height) = (r.u32()?, r.u32()?);
        b.pages.push(LightmapAtlas { source, width, height, texels: r.u32s()?, directional: r.u32s()?, shadow: r.bytes()?, shadow_local: r.bytes()? });
    }
    for _ in 0..r.u32()? {
        let i = r.u32()? as usize;
        let atlas = r.u32()? as usize;
        let uv_min = Vec2::new(r.f32()?, r.f32()?);
        let uv_max = Vec2::new(r.f32()?, r.f32()?);
        let df_shadow = r.u32()? != 0;
        let df_light = match r.u32()? {
            u32::MAX => None,
            x => Some(x as usize),
        };
        b.assignments.push((i, InstanceLightmap { atlas, uv_min, uv_max, df_shadow, df_light }));
    }
    (r.1 == data.len()).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_another_signature() {
        let b = Baked {
            pages: vec![LightmapAtlas { source: "lightmap page 0".into(), width: 2, height: 1, texels: vec![1, 2], directional: vec![3, 4], shadow: vec![5, 6], shadow_local: vec![] }],
            assignments: vec![(7, InstanceLightmap { atlas: 0, uv_min: Vec2::new(0.25, 0.5), uv_max: Vec2::ONE, df_shadow: true, df_light: Some(3) }), (9, InstanceLightmap { atlas: 0, uv_min: Vec2::ZERO, uv_max: Vec2::ONE, df_shadow: false, df_light: None })],
            atlases: 4,
            shadow_map_components: 1,
            local_shadow_map_components: 1,
            texture_failures: 0,
            errors: vec!["x".into()],
        };
        let dir = std::env::temp_dir().join(format!("dlmc_test_{}", std::process::id()));
        let p = dir.join("t.lightmaps.bin");
        save(&p, Signature(42), &b).unwrap();
        let l = load(&p, Signature(42)).unwrap();
        assert_eq!(l.pages[0].texels, vec![1, 2]);
        assert_eq!(l.pages[0].shadow, vec![5, 6]);
        assert_eq!(l.assignments[0].1.df_light, Some(3));
        assert_eq!(l.assignments[1].1.df_light, None);
        assert_eq!(l.assignments[0].1.uv_min, Vec2::new(0.25, 0.5));
        assert_eq!((l.atlases, l.errors.len()), (4, 1));
        assert!(load(&p, Signature(43)).is_none());
        std::fs::remove_dir_all(dir).ok();
    }
}
