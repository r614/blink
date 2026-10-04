//! Normalized Dishonored content, independent of UE3 object semantics.
//!
//! Coordinate frame: **Dishonored world space** — Z-up, left-handed, Unreal units (uu),
//! exactly as the original game stores it. Simulation code works in this frame so
//! original constants (speeds, gravity, ranges) apply unchanged. Presentation layers
//! convert to their own frame (see `dishonored_runtime::coords`).

use glam::{Mat4, Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// Index into [`DishonoredMap::meshes`].
pub type MeshId = usize;
/// Index into [`DishonoredMap::materials`].
pub type MaterialId = usize;
/// Index into [`DishonoredMap::textures`].
pub type TextureId = usize;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct DishonoredMap {
    pub name: String,
    pub meshes: Vec<StaticMeshAsset>,
    pub materials: Vec<MaterialAsset>,
    pub textures: Vec<TextureAsset>,
    pub instances: Vec<MeshInstance>,
    pub collision: CollisionWorld,
    pub lights: Vec<Light>,
    pub spawn_candidates: Vec<SpawnCandidate>,
    /// Objects the importer saw but did not convert, kept for later work.
    pub unconverted: Vec<UnconvertedObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticMeshAsset {
    /// Source object path, e.g. `air_duct_01.air_duct_09`.
    pub source: String,
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub tangents: Vec<Vec3>,
    pub uv0: Vec<Vec2>,
    pub uv1: Vec<Vec2>,
    pub indices: Vec<u32>,
    pub sections: Vec<MeshSection>,
    /// Collision triangles from the mesh's kDOP tree (indices into `positions`) with the
    /// per-triangle material index. Empty when the mesh has no per-poly collision.
    pub collision_triangles: Vec<[u32; 3]>,
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshSection {
    pub first_index: u32,
    pub num_triangles: u32,
    /// Default material for this section (may be overridden per instance).
    pub material: Option<MaterialId>,
    pub enable_collision: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaterialAsset {
    pub source: String,
    pub diffuse: Option<TextureId>,
    pub normal: Option<TextureId>,
    /// Masked/translucent materials (foliage, glass) where known.
    pub blend: BlendHint,
    /// Every texture parameter found while resolving, for later material work.
    pub texture_params: Vec<(String, TextureId)>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendHint {
    #[default]
    Opaque,
    Masked,
    Translucent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureFormat {
    /// BC1 / DXT1
    Bc1,
    /// BC2 / DXT3
    Bc2,
    /// BC3 / DXT5
    Bc3,
    /// BC5 / ATI2 (two-channel normal maps)
    Bc5,
    /// 8-bit BGRA
    Bgra8,
    /// 8-bit single channel
    R8,
    /// Signed two-channel (UE3 PF_V8U8 normal maps)
    Rg8Snorm,
}

impl TextureFormat {
    /// Bytes per 4x4 block for block formats, or per pixel for uncompressed.
    pub fn is_block(self) -> bool {
        matches!(self, Self::Bc1 | Self::Bc2 | Self::Bc3 | Self::Bc5)
    }
    pub fn block_bytes(self) -> usize {
        match self {
            Self::Bc1 => 8,
            Self::Bc2 | Self::Bc3 | Self::Bc5 => 16,
            Self::Bgra8 => 4,
            Self::R8 => 1,
            Self::Rg8Snorm => 2,
        }
    }
    pub fn mip_size(self, w: u32, h: u32) -> usize {
        if self.is_block() {
            (w.div_ceil(4) * h.div_ceil(4)) as usize * self.block_bytes()
        } else {
            (w * h) as usize * self.block_bytes()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureAsset {
    pub source: String,
    pub format: TextureFormat,
    pub width: u32,
    pub height: u32,
    /// Mip chain, largest first, starting at `width`x`height`.
    pub mips: Vec<Vec<u8>>,
    pub srgb: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshInstance {
    pub mesh: MeshId,
    /// Mesh-local to world (Dishonored world space), column-major glam matrix.
    pub transform: Mat4,
    /// Per-section material overrides (`None` = use the mesh default).
    pub materials: Vec<Option<MaterialId>>,
    pub hidden: bool,
    pub collides: bool,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollisionWorld {
    /// World-space collision triangles (filled by the collision builder).
    pub triangles: Vec<[Vec3; 3]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LightKind {
    Point,
    Spot { inner_cone_deg: f32, outer_cone_deg: f32 },
    Directional,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Light {
    pub kind: LightKind,
    pub position: Vec3,
    pub direction: Vec3,
    pub color: [f32; 3],
    pub brightness: f32,
    pub radius: f32,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnCandidate {
    pub position: Vec3,
    pub yaw_deg: f32,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnconvertedObject {
    pub class: String,
    pub path: String,
}
