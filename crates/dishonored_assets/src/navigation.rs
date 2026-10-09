//! Reader for Dishonored's original navigation: UE3 navigation meshes owned by `DisPylon`
//! actors (one per `L_*_Nav` level). Format notes and evidence:
//! `research/dishonored/NAVIGATION.md`.
//!
//! `NavigationMeshBase` native serialization (version 28/28, after the tagged properties):
//! ```text
//! i32 NavMeshVersionNum, i32 VersionAtGenerationTime
//! TArray<FMeshVertex>        { FVector; TArray<u16> ContainingPolys }
//! TArray<FEdgeStorageDatum>  { u32 DataPtrOffset; u16 DataSize; FName ClassName }
//! TArray<FNavMeshPolyBase>   { TArray<u16> PolyVerts; TArray<u16> PolyEdges; FVector Center;
//!                              FVector Normal; FBox Bounds; f32 PolyHeight }
//! FMatrix LocalToWorld, FMatrix WorldToLocal, FBox BoxBounds
//! edge records in storage order, by class:
//!   FNavMeshEdgeBase / FNavMeshBasicOneWayEdge (29 B):
//!     u16 Vert0, Vert1, Poly0, Poly1; f32 EdgeLength, EffectiveEdgeLength; FVector EdgeCenter; u8 EdgeType
//!   FNavMeshCrossPylonEdge (81 B) / FNavMeshPathObjectEdge (105 B, adds the PathObject fields):
//!     u16 Vert0, Vert1, ?, ?; i32 ?(-1); f32 EdgeLength, EffectiveEdgeLength; FVector EdgeCenter;
//!     u8 EdgeType; FPolyReference x2 { i32 Pylon; FGuid; u16 Poly; u16 SubPoly };
//!     FActorReference PathObject { i32 Actor; FGuid }; i32 InternalPathObjectID
//! ```

use crate::content::Content;
use crate::util::fmatrix_rows;
use anyhow::{Context, Result, bail, ensure};
use dishonored_ir::nav::{DynamicPylon, NavData, PylonBase, NavEdge, NavEdgeClass, NavEdgeType, NavMesh, NavPathObject, NavPathObjectLink, NavPoly, NavPylon};
use glam::Vec3;
use ue3_package::properties::{Value, find};
use ue3_package::reader::Reader;
use ue3_package::{ObjRef, Package};

fn vec3(r: &mut Reader) -> Result<Vec3> {
    Ok(Vec3::from(r.vec3()?))
}

fn u16s(r: &mut Reader) -> Result<Vec<u16>> {
    let n = r.count(2)?;
    (0..n).map(|_| r.u16()).collect()
}

fn fbox(r: &mut Reader) -> Result<(Vec3, Vec3)> {
    let min = vec3(r)?;
    let max = vec3(r)?;
    r.u8()?;
    Ok((min, max))
}

fn fmatrix(r: &mut Reader) -> Result<glam::Mat4> {
    let mut rows = [[0f32; 4]; 4];
    for row in rows.iter_mut() {
        for v in row.iter_mut() {
            *v = r.f32()?;
        }
    }
    Ok(fmatrix_rows(rows))
}

/// Decode one `NavigationMeshBase` export.
pub fn read_nav_mesh(pkg: &Package, export: usize) -> Result<NavMesh> {
    read_nav_mesh_ex(pkg, export, false)
}

/// [`read_nav_mesh`] for the meshes of a pylon, `dynamic` for an `ArkDynamicPylon`. Its meshes serialize `TArray<FBorderEdgeInfo> { u16 Vert0, Vert1, Poly }` between
/// `WorldToLocal` and `BoxBounds` (the outline of the mesh; every shipped dynamic pylon mesh has
/// it, no static `DisPylon` mesh does), and `ArkDynamicPylon::PostBeginPlay` (0x00934b60) resets
/// both meshes' `LocalToWorld` / `WorldToLocal` to identity, so their vertices are used as stored.
pub fn read_nav_mesh_ex(pkg: &Package, export: usize, dynamic: bool) -> Result<NavMesh> {
    let data = pkg.export_data(export)?;
    let h = pkg.read_object(export)?;
    let mut r = Reader::at(data, h.native_offset);
    let version = (r.i32()?, r.i32()?);
    ensure!(version.0 == 28, "unsupported navmesh version {version:?}");
    let nv = r.count(14)?;
    let mut verts = Vec::with_capacity(nv);
    for _ in 0..nv {
        verts.push(vec3(&mut r)?);
        let n = r.count(2)?;
        r.skip(n * 2)?;
    }
    let ne = r.count(14)?;
    let mut classes = Vec::with_capacity(ne);
    for _ in 0..ne {
        let _offset = r.u32()?;
        let _size = r.u16()?;
        let name = r.i32()?;
        let _num = r.i32()?;
        let class = match pkg.names.get(name as usize).map(|n| n.name.as_str()) {
            Some("FNavMeshEdgeBase") => NavEdgeClass::Base,
            Some("FNavMeshBasicOneWayEdge") => NavEdgeClass::OneWay,
            Some("FNavMeshCrossPylonEdge") => NavEdgeClass::CrossPylon,
            Some("FNavMeshPathObjectEdge") => NavEdgeClass::PathObject,
            other => bail!("unknown navmesh edge class {other:?}"),
        };
        classes.push(class);
    }
    let np = r.count(40)?;
    let mut polys = Vec::with_capacity(np);
    for _ in 0..np {
        let pv = u16s(&mut r)?;
        let pe = u16s(&mut r)?;
        let center = vec3(&mut r)?;
        let normal = vec3(&mut r)?;
        let (min, max) = fbox(&mut r)?;
        let height = r.f32()?;
        polys.push(NavPoly { verts: pv, edges: pe, center, normal, min, max, height });
    }
    let local_to_world = fmatrix(&mut r)?;
    let _world_to_local = fmatrix(&mut r)?;
    if dynamic {
        let n = r.count(6)?;
        r.skip(n * 6)?;
    }
    let (min, max) = fbox(&mut r)?;
    let mut edges = Vec::with_capacity(ne);
    for class in classes {
        let v0 = r.u16()?;
        let v1 = r.u16()?;
        let poly0 = r.u16()?;
        let poly1 = r.u16()?;
        let cross = matches!(class, NavEdgeClass::PathObject | NavEdgeClass::CrossPylon);
        if cross {
            r.i32()?;
        }
        let length = r.f32()?;
        let effective_length = r.f32()?;
        let center = vec3(&mut r)?;
        let kind = NavEdgeType::from_u8(r.u8()?);
        let path_object = if cross {
            let mut poly_refs = [(0, [0; 4], 0, 0); 2];
            for pr in poly_refs.iter_mut() {
                *pr = (r.i32()?, r.guid()?, r.u16()?, r.u16()?);
            }
            let (path_object, internal_id) = if class == NavEdgeClass::PathObject { ((r.i32()?, r.guid()?), r.i32()?) } else { ((0, [0; 4]), -1) };
            Some(NavPathObjectLink { poly_refs, path_object, internal_id })
        } else {
            None
        };
        edges.push(NavEdge { class, kind, v0, v1, poly0, poly1, length, effective_length, center, path_object });
    }
    ensure!(r.remaining() == 0, "{} trailing bytes after navmesh edges", r.remaining());
    let mut mesh = NavMesh { source: format!("{}:{}", pkg.name, pkg.obj_path(ObjRef::Export(export))), version, verts, polys, edges, min, max };
    if !dynamic && local_to_world != glam::Mat4::IDENTITY {
        for v in mesh.verts.iter_mut() {
            *v = local_to_world.transform_point3(*v);
        }
        for p in mesh.polys.iter_mut() {
            p.center = local_to_world.transform_point3(p.center);
            p.normal = local_to_world.transform_vector3(p.normal).normalize_or_zero();
        }
        for e in mesh.edges.iter_mut() {
            e.center = local_to_world.transform_point3(e.center);
        }
    }
    Ok(mesh)
}

/// Every `Pylon`-derived actor of a package with its meshes.
pub fn read_package_pylons(pkg: &Package) -> Result<Vec<NavPylon>> {
    let mut out = Vec::new();
    for i in 0..pkg.exports.len() {
        let class = pkg.export_class(i);
        let dynamic = class == "ArkDynamicPylon";
        if class != "DisPylon" && class != "Pylon" && !dynamic {
            continue;
        }
        let h = pkg.read_object(i)?;
        let data = pkg.export_data(i)?;
        // Pylon native data: NavMeshPtr and ObstacleMesh object references.
        let mut r = Reader::at(data, h.native_offset);
        let nav_ref = r.i32()?;
        let obs_ref = r.i32()?;
        let location = find(&h.properties, "Location").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
        let nav_guid = match find(&h.properties, "NavGuid").map(|p| &p.value) {
            Some(Value::Guid(g)) => *g,
            _ => [0; 4],
        };
        let tall_boy = matches!(find(&h.properties, "m_bEnableTallBoyNavigation").map(|p| &p.value), Some(Value::Bool(true)));
        let ObjRef::Export(ne) = ObjRef::from_index(nav_ref) else { continue };
        let mesh = read_nav_mesh_ex(pkg, ne, dynamic).with_context(|| format!("{}: navmesh of {}", pkg.name, pkg.obj_path(ObjRef::Export(i))))?;
        let obstacle = match ObjRef::from_index(obs_ref) {
            ObjRef::Export(oe) => Some(read_nav_mesh_ex(pkg, oe, dynamic).with_context(|| format!("{}: obstacle mesh", pkg.name))?),
            _ => None,
        };
        // `ArkDynamicPylon` (Engine.u): `m_fCeilingHeight` 700 and `bNeedsCostCheck` true by
        // class default; `Base` / `RelativeLocation` / `bHardAttach` are the mover attachment.
        let dynamic = dynamic.then(|| {
            // The `Base` chain inside the package (a based actor's own `Base`, and so on).
            let mut bases = Vec::new();
            let mut props = h.properties.clone();
            while let Some(Value::Object(r)) = find(&props, "Base").map(|p| p.value.clone()) {
                let ObjRef::Export(b) = ObjRef::from_index(r) else { break };
                if bases.len() > 8 {
                    break;
                }
                props = match pkg.read_object(b) {
                    Ok(o) => o.properties,
                    Err(_) => break,
                };
                let location = find(&props, "Location").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
                let rotation = match find(&props, "Rotation").map(|p| &p.value) {
                    Some(Value::Rotator(r)) => *r,
                    _ => [0; 3],
                };
                bases.push(PylonBase { name: pkg.obj_name(ObjRef::Export(b)).to_string(), location, rotation });
            }
            let relative_location = find(&h.properties, "RelativeLocation").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
            let flag = |n: &str, d: bool| match find(&h.properties, n).map(|p| &p.value) {
                Some(Value::Bool(b)) => *b,
                _ => d,
            };
            let ceiling_height = match find(&h.properties, "m_fCeilingHeight").map(|p| &p.value) {
                Some(Value::Float(f)) => *f,
                _ => 700.0,
            };
            DynamicPylon { bases, relative_location, hard_attach: flag("bHardAttach", false), ceiling_height, needs_cost_check: flag("bNeedsCostCheck", true) }
        });
        out.push(NavPylon { name: pkg.obj_name(ObjRef::Export(i)).to_string(), package: pkg.name.clone(), object_ref: i as i32 + 1, location, nav_guid, tall_boy, mesh, obstacle, dynamic });
    }
    Ok(out)
}

/// The navigation of a map: every pylon in its packages (each package read once).
pub fn load_nav_data(content: &Content, packages: &[String]) -> Result<NavData> {
    let mut out = NavData::default();
    let mut seen = std::collections::BTreeSet::new();
    for name in packages {
        if !content.has_package(name) || !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let pkg = content.package(name)?;
        out.path_objects.extend(read_path_objects(&pkg));
        if !(0..pkg.exports.len()).any(|i| pkg.export_class(i).ends_with("Pylon")) {
            continue;
        }
        out.pylons.extend(read_package_pylons(&pkg)?);
    }
    Ok(out)
}

/// Doors and breakable nav blockers placed in a level (the actors behind path-object edges).
pub fn read_path_objects(pkg: &Package) -> Vec<NavPathObject> {
    let mut out = Vec::new();
    for i in 0..pkg.exports.len() {
        let class = pkg.export_class(i);
        let (guid_prop, block_prop) = match class.as_str() {
            "DisDoor" => ("NavGuid", "m_bLocked"),
            "DishonoredBreakableNavBlock" => ("m_NavGuid", "m_bIsCurrentlyBlockingNavigation"),
            _ => continue,
        };
        if pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
            continue;
        }
        let Ok(h) = pkg.read_object(i) else { continue };
        let Some(Value::Guid(guid)) = find(&h.properties, guid_prop).map(|p| &p.value) else { continue };
        let blocking = matches!(find(&h.properties, block_prop).map(|p| &p.value), Some(Value::Bool(true)));
        let location = find(&h.properties, "Location").and_then(|p| p.as_vector()).map(Vec3::from).unwrap_or(Vec3::ZERO);
        out.push(NavPathObject { guid: *guid, class: class.to_string(), name: pkg.obj_name(ObjRef::Export(i)).to_string(), package: pkg.name.clone(), location, blocking });
    }
    out
}
