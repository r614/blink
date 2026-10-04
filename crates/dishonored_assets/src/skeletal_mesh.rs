//! `USkeletalMesh` at file version 801 / licensee 30 (LOD0 only).
//!
//! Generic UE3 v801 layout with two Dishonored additions (cross-checked with UEViewer's
//! `#if DISHONORED` branches, re-verified against the data):
//! - after tagged properties: `FUserBounds { FName BoneName; FVector Offset; f32 Radius }`
//! - after `MeshOrigin/RotOrigin`: `TArray<u8> EdgeSkeleton` (Sony Edge skeleton blob)

use crate::util::{half_to_f32, packed_normal};
use anyhow::{Context, Result, bail, ensure};
use glam::{Quat, Vec2, Vec3};
use ue3_package::reader::Reader;
use ue3_package::{FName, ObjRef, Package};

#[derive(Debug, Clone)]
pub struct Bone {
    pub name: String,
    pub parent: i32,
    /// Local (parent-relative) bind pose, UE3 convention.
    pub orientation: Quat,
    pub position: Vec3,
}

#[derive(Debug, Clone)]
pub struct SkelSection {
    pub material_index: i16,
    pub first_index: u32,
    pub num_triangles: u32,
}

#[derive(Debug, Clone, Default)]
pub struct RawSkeletalMesh {
    pub materials: Vec<i32>,
    pub mesh_origin: Vec3,
    pub rot_origin: [i32; 3],
    pub bones: Vec<Bone>,
    pub sections: Vec<SkelSection>,
    pub indices: Vec<u32>,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub tangents: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    /// Global bone indices (chunk-local indices already remapped).
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub edge_skeleton_bytes: usize,
    pub edge_skeleton: Vec<u8>,
}

fn tarray_skip(r: &mut Reader, elem: usize) -> Result<usize> {
    let n = r.count(elem)?;
    r.skip(n * elem)?;
    Ok(n)
}

pub fn read_skeletal_mesh(pkg: &Package, export: usize) -> Result<RawSkeletalMesh> {
    let hdr = pkg.read_object(export)?;
    let has_vertex_colors = hdr.properties.iter().any(|p| p.name == "bHasVertexColors" && matches!(p.value, ue3_package::properties::Value::Bool(true)));
    let data = pkg.export_data(export)?;
    let mut r = Reader::at(data, hdr.native_offset);
    decode(pkg, &mut r, has_vertex_colors).with_context(|| format!("SkeletalMesh {} (native at {:#x})", pkg.obj_path(ObjRef::Export(export)), hdr.native_offset))
}

fn decode(pkg: &Package, r: &mut Reader, has_vertex_colors: bool) -> Result<RawSkeletalMesh> {
    let mut m = RawSkeletalMesh::default();
    // [Dishonored] FUserBounds
    let _bone = FName::read(r)?;
    let _offset = r.vec3()?;
    let _radius = r.f32()?;
    // FBoxSphereBounds
    r.skip(28)?;
    let nmat = r.count(4)?;
    for _ in 0..nmat {
        m.materials.push(r.i32()?);
    }
    m.mesh_origin = Vec3::from(r.vec3()?);
    m.rot_origin = [r.i32()?, r.i32()?, r.i32()?];
    // [Dishonored] TArray<byte> EdgeSkeleton
    let n = r.count(1)?;
    m.edge_skeleton = r.bytes(n)?.to_vec();
    m.edge_skeleton_bytes = n;
    let nbones = r.count(52)?;
    for _ in 0..nbones {
        let name = pkg.name(FName::read(r)?);
        let _flags = r.u32()?;
        let q = Quat::from_xyzw(r.f32()?, r.f32()?, r.f32()?, r.f32()?);
        let p = Vec3::from(r.vec3()?);
        let _num_children = r.i32()?;
        let parent = r.i32()?;
        let _unk = r.i32()?;
        m.bones.push(Bone { name, parent, orientation: q, position: p });
    }
    let _depth = r.i32()?;
    let nlods = r.count(4)?;
    ensure!(nlods >= 1, "no LODs");

    // FStaticLODModel3 (LOD0)
    let nsec = r.count(11)?;
    for _ in 0..nsec {
        let material_index = r.i16()?;
        let _chunk = r.i16()?;
        let first_index = r.u32()?;
        let num_triangles = r.u16()? as u32;
        let _unk = r.u8()?;
        m.sections.push(SkelSection { material_index, first_index, num_triangles });
    }
    let (esz, n, idx) = r.bulk_array()?;
    ensure!(esz == 2, "index elem size {esz}");
    let mut ir = Reader::new(idx);
    for _ in 0..n {
        m.indices.push(ir.u16()? as u32);
    }
    tarray_skip(r, 2)?; // UsedBones
    // Chunks
    let nchunks = r.count(16)?;
    let mut chunks: Vec<(u32, Vec<u16>)> = Vec::new();
    for _ in 0..nchunks {
        let first_vertex = r.u32()?;
        let nrigid = r.count(1)?;
        let nsoft = r.count(1)?;
        if nrigid > 0 || nsoft > 0 {
            bail!("CPU-skinned chunk vertices present ({nrigid} rigid, {nsoft} soft); not expected in cooked PC data");
        }
        let nb = r.count(2)?;
        let mut bones = Vec::with_capacity(nb);
        for _ in 0..nb {
            bones.push(r.u16()?);
        }
        let _nr = r.i32()?;
        let _ns = r.i32()?;
        let _maxinf = r.i32()?;
        chunks.push((first_vertex, bones));
    }
    let _size = r.i32()?;
    let num_vertices = r.u32()? as usize;
    tarray_skip(r, 1)?; // RequiredBones
    // FWordBulkData raw point indices
    let flags = r.u32()?;
    let _cnt = r.i32()?;
    let size = r.i32()?;
    let _off = r.i32()?;
    if flags & (0x01 | 0x20 | 0x40) == 0 && size > 0 {
        r.skip(size as usize)?;
    }
    let _num_uv_sets_lod = r.i32()?;
    // GPU skin vertex buffer
    let num_uv = r.u32()? as usize;
    let full_uv = r.u32()? != 0;
    let packed_pos = r.u32()? != 0;
    // The flag is set in cooked PC data but positions are full floats (stride 32 with one
    // half-float UV set); the PC renderer ignores it (matches UEViewer).
    let _ = packed_pos;
    let _ext = r.vec3()?;
    let _origin = r.vec3()?;
    let (esz, nv, verts) = r.bulk_array()?;
    let uv_size = if full_uv { 8 } else { 4 };
    ensure!(esz == 8 + 8 + 12 + num_uv * uv_size, "gpu vertex size {esz} (uv sets {num_uv}, full {full_uv})");
    ensure!(nv == num_vertices, "vertex count {nv} vs {num_vertices}");
    let mut vr = Reader::new(verts);
    let mut chunk_i = 0usize;
    for v in 0..nv {
        while chunk_i + 1 < chunks.len() && v as u32 >= chunks[chunk_i + 1].0 {
            chunk_i += 1;
        }
        let t = packed_normal(vr.u32()?);
        let n = packed_normal(vr.u32()?);
        let bi = vr.array::<4>()?;
        let bw = vr.array::<4>()?;
        let pos = Vec3::from(vr.vec3()?);
        let mut uv = Vec2::ZERO;
        for k in 0..num_uv {
            let u = if full_uv { Vec2::new(vr.f32()?, vr.f32()?) } else { Vec2::new(half_to_f32(vr.u16()?), half_to_f32(vr.u16()?)) };
            if k == 0 {
                uv = u;
            }
        }
        let map = &chunks.get(chunk_i).map(|c| c.1.as_slice()).unwrap_or(&[]);
        let j = bi.map(|b| map.get(b as usize).copied().unwrap_or(0));
        let wsum: f32 = bw.iter().map(|&w| w as f32).sum::<f32>().max(1.0);
        m.positions.push(pos);
        m.normals.push(n);
        m.tangents.push(t);
        m.uvs.push(uv);
        m.joints.push(j);
        m.weights.push(bw.map(|w| w as f32 / wsum));
    }
    if has_vertex_colors {
        let _ = r.bulk_array();
    }
    Ok(m)
}

