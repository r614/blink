//! Original navigation data: the UE3 navigation mesh Dishonored ships per level
//! (`DisPylon` actors in the `L_*_Nav` packages, each owning a `NavigationMeshBase`).
//! See `research/dishonored/NAVIGATION.md`.

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// `Engine.Pylon.ENavMeshEdgeType` (value order read from the `Engine.upk` enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NavEdgeType {
    Normal,
    Mantle,
    Coverslip,
    SwatTurn,
    DropDown,
    PathObject,
    BackRefDummy,
    Jump,
    Removed,
    Unknown(u8),
}

impl NavEdgeType {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Normal,
            1 => Self::Mantle,
            2 => Self::Coverslip,
            3 => Self::SwatTurn,
            4 => Self::DropDown,
            5 => Self::PathObject,
            6 => Self::BackRefDummy,
            7 => Self::Jump,
            8 => Self::Removed,
            o => Self::Unknown(o),
        }
    }
}

/// Native edge class (`FEdgeStorageDatum.ClassName`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NavEdgeClass {
    /// `FNavMeshEdgeBase`: two-way edge shared by `poly0` and `poly1`.
    Base,
    /// `FNavMeshBasicOneWayEdge`: traversable only from `poly1` (the owning poly) to `poly0`.
    OneWay,
    /// `FNavMeshCrossPylonEdge`: links polys of two pylons (`poly_refs`).
    CrossPylon,
    /// `FNavMeshPathObjectEdge` (a cross-pylon edge subclass): an edge owned by a path object
    /// (door, breakable blocker).
    PathObject,
}

/// Cross-pylon / path-object edge data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavPathObjectLink {
    /// `FPolyReference` x2: (pylon object ref, guid, poly index) of each side.
    pub poly_refs: [(i32, [u32; 4], u16, u16); 2],
    /// Plain cross-pylon edges: `path_object` = (0, zero guid), `internal_id` = -1.
    /// `FActorReference PathObject`: object ref (0 when the actor lives in another level) and
    /// the actor's GUID.
    pub path_object: (i32, [u32; 4]),
    pub internal_id: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavEdge {
    pub class: NavEdgeClass,
    pub kind: NavEdgeType,
    pub v0: u16,
    pub v1: u16,
    pub poly0: u16,
    pub poly1: u16,
    pub length: f32,
    pub effective_length: f32,
    pub center: Vec3,
    #[serde(default)]
    pub path_object: Option<NavPathObjectLink>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavPoly {
    /// Vertex indices (convex, ordered).
    pub verts: Vec<u16>,
    /// Indices into `NavMesh::edges` of the edges this poly owns.
    pub edges: Vec<u16>,
    pub center: Vec3,
    pub normal: Vec3,
    pub min: Vec3,
    pub max: Vec3,
    /// Free height above the poly (uu): what can stand on it.
    pub height: f32,
}

/// One `NavigationMeshBase` in world space.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct NavMesh {
    /// `package:object path` of the pylon that owns the mesh.
    pub source: String,
    pub version: (i32, i32),
    pub verts: Vec<Vec3>,
    pub polys: Vec<NavPoly>,
    pub edges: Vec<NavEdge>,
    pub min: Vec3,
    pub max: Vec3,
}

/// A pylon: its walkable mesh and its obstacle (boundary) mesh.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct NavPylon {
    pub name: String,
    pub package: String,
    pub location: Vec3,
    pub nav_guid: [u32; 4],
    pub tall_boy: bool,
    pub mesh: NavMesh,
    pub obstacle: Option<NavMesh>,
}

/// Every pylon of a map.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct NavData {
    pub pylons: Vec<NavPylon>,
    #[serde(default)]
    pub path_objects: Vec<NavPathObject>,
}

/// A level actor that owns path-object edges (matched by GUID): doors (`DisDoor`, GUID in
/// `NavGuid`) and breakable blockers (`DishonoredBreakableNavBlock`, `m_NavGuid`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavPathObject {
    pub guid: [u32; 4],
    pub class: String,
    pub name: String,
    pub package: String,
    pub location: Vec3,
    /// Door `m_bLocked` / blocker `m_bIsCurrentlyBlockingNavigation` at level start.
    pub blocking: bool,
}
