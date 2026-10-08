//! `UStaticMesh` native serialization at file version 801 / licensee 30.
//!
//! Layout verified by hex inspection of `L_Boyle_Ext_Env.upk` export
//! `air_duct_01.air_duct_09` and cross-checked against UEViewer's generic UE3 path
//! (see research/dishonored/PACKAGE_FORMAT.md). Dishonored quirk: the kDOP tree uses
//! the legacy 32-byte node format despite version >= 770.

use crate::util::{half_to_f32, packed_normal};
use anyhow::{Context, Result, bail, ensure};
use glam::{Vec2, Vec3};
use ue3_package::Package;
use ue3_package::reader::Reader;

#[derive(Debug, Clone)]
pub struct RawSection {
    pub material: i32,
    pub enable_collision: bool,
    pub first_index: u32,
    pub num_triangles: u32,
    pub material_index: i32,
}

#[derive(Debug, Clone, Default)]
pub struct RawStaticMesh {
    pub bounds_origin: Vec3,
    pub bounds_extent: Vec3,
    pub body_setup: i32,
    pub kdop_triangles: Vec<[u16; 4]>,
    pub sections: Vec<RawSection>,
    pub positions: Vec<Vec3>,
    pub tangent_x: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<Vec<Vec2>>,
    /// The mesh's vertex colours (R, G, B, A bytes); empty when it has none.
    pub colors: Vec<[u8; 4]>,
    pub indices: Vec<u16>,
}

pub fn read_static_mesh(pkg: &Package, export: usize) -> Result<RawStaticMesh> {
    let hdr = pkg.read_object(export)?;
    let data = pkg.export_data(export)?;
    let mut r = Reader::at(data, hdr.native_offset);
    decode(&mut r).with_context(|| {
        format!(
            "StaticMesh {} (native data at {:#x})",
            pkg.obj_path(ue3_package::ObjRef::Export(export)),
            hdr.native_offset
        )
    })
}

fn decode(r: &mut Reader) -> Result<RawStaticMesh> {
    let mut m = RawStaticMesh {
        bounds_origin: Vec3::from(r.vec3()?),
        bounds_extent: Vec3::from(r.vec3()?),
        ..Default::default()
    };
    let _sphere_radius = r.f32()?;
    m.body_setup = r.i32()?;

    // Legacy kDOP: 32-byte nodes {min, max, bIsLeaf:i32, i16, i16}.
    let (node_size, _nodes, _) = r.bulk_array()?;
    ensure!(
        node_size == 32 || node_size == 0,
        "unexpected kDOP node size {node_size}"
    );
    let (tri_size, ntris, tris) = r.bulk_array()?;
    ensure!(
        tri_size == 8 || ntris == 0,
        "unexpected kDOP triangle size {tri_size}"
    );
    let mut tr = Reader::new(tris);
    for _ in 0..ntris {
        m.kdop_triangles
            .push([tr.u16()?, tr.u16()?, tr.u16()?, tr.u16()?]);
    }

    let _internal_version = r.i32()?;
    let nlods = r.count(4)?;
    ensure!(nlods >= 1, "static mesh without LODs");
    // Only LOD0 is needed; it is serialized first.

    // FByteBulkData RawTriangles: flags, element count, size on disk, offset in file.
    let flags = r.u32()?;
    let _count = r.i32()?;
    let size_on_disk = r.i32()?;
    let _offset = r.i32()?;
    if flags & (0x01 | 0x20 | 0x40) == 0 && size_on_disk > 0 {
        r.skip(size_on_disk as usize)?;
    }

    let nsections = r.count(40)?;
    for _ in 0..nsections {
        let material = r.i32()?;
        let enable_collision = r.u32()? != 0;
        let _old_enable_collision = r.u32()?;
        let _shadow_casting = r.u32()?;
        let first_index = r.u32()?;
        let num_triangles = r.u32()?;
        let _min_vertex = r.u32()?;
        let _max_vertex = r.u32()?;
        let material_index = r.i32()?;
        let nfrag = r.count(8)?;
        r.skip(nfrag * 8)?;
        let ps3 = r.u8()?;
        if ps3 != 0 {
            bail!("section carries PS3 static mesh data; not expected on PC");
        }
        m.sections.push(RawSection {
            material,
            enable_collision,
            first_index,
            num_triangles,
            material_index,
        });
    }

    // Position stream.
    let _stride = r.u32()?;
    let nverts = r.u32()? as usize;
    let (esz, n, pos) = r.bulk_array()?;
    ensure!(esz == 12 && n == nverts, "position stream {esz}x{n}, expected 12x{nverts}");
    let mut pr = Reader::new(pos);
    for _ in 0..n {
        m.positions.push(Vec3::from(pr.vec3()?));
    }

    // Tangent/UV stream.
    let num_uv = r.u32()? as usize;
    let item_size = r.u32()? as usize;
    let nverts2 = r.u32()? as usize;
    let full_precision = r.u32()? != 0;
    let (esz, n, items) = r.bulk_array()?;
    ensure!(
        n == nverts2 && esz == item_size,
        "uv stream {esz}x{n}, expected {item_size}x{nverts2}"
    );
    let uv_bytes = if full_precision { 8 } else { 4 };
    ensure!(
        item_size == 8 + num_uv * uv_bytes,
        "uv item size {item_size} does not match {num_uv} UV sets (full={full_precision})"
    );
    m.uvs = vec![Vec::with_capacity(n); num_uv];
    let mut ur = Reader::new(items);
    for _ in 0..n {
        m.tangent_x.push(packed_normal(ur.u32()?));
        m.normals.push(packed_normal(ur.u32()?));
        for set in m.uvs.iter_mut() {
            let uv = if full_precision {
                Vec2::new(ur.f32()?, ur.f32()?)
            } else {
                Vec2::new(half_to_f32(ur.u16()?), half_to_f32(ur.u16()?))
            };
            set.push(uv);
        }
    }

    // Color stream (array present only when it has vertices).
    let _color_stride = r.u32()?;
    let ncolor = r.u32()?;
    if ncolor > 0 {
        // FColor per vertex, stored B, G, R, A (e.g. 34 colours on
        // `Hub_FX.Light.Hub_Medium_Window`, white at the window and black at the far end of the
        // light beam: Volume_Light_PMAT's `Use_Vertex_Color` reads the red channel).
        let (esz, n, colors) = r.bulk_array()?;
        if esz == 4 && n == m.positions.len() {
            m.colors = colors.chunks_exact(4).map(|c| [c[2], c[1], c[0], c[3]]).collect();
        }
    }
    let _num_vertices = r.u32()?;

    let (esz, n, idx) = r.bulk_array()?;
    ensure!(esz == 2, "index size {esz}");
    let mut ir = Reader::new(idx);
    for _ in 0..n {
        m.indices.push(ir.u16()?);
    }
    Ok(m)
}
