//! A map that is not one of the original's: a layout of original assets (static meshes,
//! materials, placed actors used as archetypes, lights) plus brush geometry, volumes and player
//! starts. The Playground (`maps/playground.ron`) is one. The importer turns a layout into the
//! same [`crate::DishonoredMap`] a cooked level gives, so collision, navigation and the
//! simulation run on it unchanged. Format: research/dishonored/PLAYGROUND.md.
//!
//! Coordinates are Dishonored world space (Z up, Unreal units); angles are degrees.

use serde::{Deserialize, Serialize};

/// The whole layout file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MapLayout {
    /// Map name (what `--map` and missions call it).
    pub name: String,
    pub description: String,
    /// Cooked packages the layout's paths refer to (loaded to resolve meshes, materials and the
    /// actors used as archetypes; their own actors are not placed).
    pub packages: Vec<String>,
    /// Package whose `WorldInfo` gives the post-process settings and reflection cubemap.
    pub environment: Option<String>,
    /// Material of brushes that name none.
    pub default_material: String,
    /// `WorldInfo.KillZ`: falling below it kills.
    pub kill_z: f32,
    /// Ambient brightness the AI light probe adds everywhere (`AiLevel::sky_light`).
    pub sky_light: f32,
    pub brushes: Vec<Brush>,
    pub meshes: Vec<MeshPlacement>,
    pub actors: Vec<ActorPlacement>,
    pub lights: Vec<LayoutLight>,
    pub starts: Vec<LayoutStart>,
    /// Named points missions can anchor to (`Actor("<name>")`).
    pub points: Vec<LayoutPoint>,
    pub water: Vec<WaterBox>,
    pub pain: Vec<PainBox>,
    pub labels: Vec<Label>,
    /// Navigation mesh generated over the layout's walkable floors (None: no navigation).
    pub nav: Option<NavGrid>,
}

/// Brush geometry with an original material (a blockout primitive).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Brush {
    /// Centre of the bottom face (box), or of the bottom front edge (ramp, stairs).
    pub at: (f32, f32, f32),
    /// Rotation about Z (degrees): ramps and stairs rise along the rotated +X.
    #[serde(default)]
    pub yaw: f32,
    pub shape: Shape,
    /// Material object path (`Env_Blockout.materials_inst.stone_blk_064_INST`).
    #[serde(default)]
    pub material: Option<String>,
    /// Blocks pawns and traces (every trace type).
    #[serde(default = "yes")]
    pub collide: bool,
    /// World units per texture repeat (planar mapping in world space).
    #[serde(default = "d_uv")]
    pub uv_scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    /// Size along local X, Y, Z.
    Box((f32, f32, f32)),
    /// A wedge rising along local X: `length` horizontally at `angle` degrees, `width` along Y.
    Ramp { width: f32, length: f32, angle: f32 },
    /// `steps` boxes, each `rise` higher and `run` further along local X, `width` along Y.
    Stairs { width: f32, steps: u32, rise: f32, run: f32 },
}

/// An original static mesh placed by its object path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshPlacement {
    pub mesh: String,
    pub at: (f32, f32, f32),
    /// (pitch, yaw, roll), degrees.
    #[serde(default)]
    pub rotation: (f32, f32, f32),
    #[serde(default = "d_scale")]
    pub scale: (f32, f32, f32),
    /// Per-section material overrides (object paths; `None` keeps the mesh's).
    #[serde(default)]
    pub materials: Vec<Option<String>>,
    #[serde(default = "yes")]
    pub collide: bool,
}

/// An original placed actor used as an archetype (`"L_Boyle_Ext_Audio:DisDoor_21"`): copied with
/// its components, settings and tweaks to a new place. Doors, usable objects, climbable chains
/// and pickups keep their original behaviour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActorPlacement {
    pub from: String,
    /// Name of the copy (default: the original's name); actors are `<layout name>:<name>`.
    #[serde(default)]
    pub name: Option<String>,
    /// New `Location`.
    pub at: (f32, f32, f32),
    /// New yaw (degrees); pitch and roll are kept.
    #[serde(default)]
    pub yaw: f32,
    /// A door's `m_bLocked` (default: the original's).
    #[serde(default)]
    pub locked: Option<bool>,
    /// A door's `m_InitialDoorState` (0 closed, 1 open CW, 2 open CCW; default: the original's).
    #[serde(default)]
    pub door_state: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightType {
    Point,
    Spot,
    Directional,
}

/// A dynamic light (the layout has no lightmaps).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutLight {
    #[serde(default = "d_point")]
    pub kind: LightType,
    #[serde(default)]
    pub at: (f32, f32, f32),
    /// Direction the light points (spot, directional).
    #[serde(default = "d_down")]
    pub direction: (f32, f32, f32),
    /// Linear colour.
    #[serde(default = "d_white")]
    pub color: (f32, f32, f32),
    #[serde(default = "d_one")]
    pub brightness: f32,
    #[serde(default = "d_radius")]
    pub radius: f32,
    #[serde(default = "d_falloff")]
    pub falloff_exponent: f32,
    #[serde(default)]
    pub cone: Option<(f32, f32)>,
    #[serde(default)]
    pub shadows: bool,
    /// Copy an original light actor instead (`"L_Boyle_Ext_Light:DominantDirectionalLight_0"`);
    /// `at` then moves point and spot lights there.
    #[serde(default)]
    pub from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutStart {
    pub at: (f32, f32, f32),
    #[serde(default)]
    pub yaw: f32,
    /// `PlayerStart.Tag` (travel / mission starts).
    #[serde(default)]
    pub tag: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutPoint {
    pub name: String,
    pub at: (f32, f32, f32),
    #[serde(default)]
    pub yaw: f32,
}

/// A `DishonoredWaterVolume` box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaterBox {
    pub name: String,
    pub min: (f32, f32, f32),
    pub max: (f32, f32, f32),
    /// `FluidFriction` (the class default 2.4).
    #[serde(default = "d_fluid")]
    pub fluid_friction: f32,
    #[serde(default)]
    pub shallow: bool,
}

/// A pain-causing `PhysicsVolume` box (`bPainCausing`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PainBox {
    pub name: String,
    pub min: (f32, f32, f32),
    pub max: (f32, f32, f32),
    pub damage_per_sec: f32,
    #[serde(default = "d_one")]
    pub interval: f32,
    #[serde(default = "yes")]
    pub entry: bool,
    pub damage_type: String,
}

/// In-world text (a station name and what it tests).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub at: (f32, f32, f32),
    pub text: String,
    /// Shown within this distance.
    #[serde(default = "d_label_range")]
    pub range: f32,
}

/// Navigation generated on a grid: a cell is walkable where a pawn stands on a walkable floor
/// with room to stand; neighbouring cells link when the step between them is climbable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavGrid {
    pub min: (f32, f32),
    pub max: (f32, f32),
    #[serde(default = "d_cell")]
    pub cell: f32,
}

fn yes() -> bool {
    true
}
fn d_uv() -> f32 {
    256.0
}
fn d_scale() -> (f32, f32, f32) {
    (1.0, 1.0, 1.0)
}
fn d_point() -> LightType {
    LightType::Point
}
fn d_down() -> (f32, f32, f32) {
    (0.0, 0.0, -1.0)
}
fn d_white() -> (f32, f32, f32) {
    (1.0, 1.0, 1.0)
}
fn d_one() -> f32 {
    1.0
}
fn d_radius() -> f32 {
    1024.0
}
fn d_falloff() -> f32 {
    2.0
}
fn d_fluid() -> f32 {
    2.4
}
fn d_label_range() -> f32 {
    1500.0
}
fn d_cell() -> f32 {
    100.0
}

pub fn v3(t: (f32, f32, f32)) -> glam::Vec3 {
    glam::Vec3::new(t.0, t.1, t.2)
}

impl ActorPlacement {
    /// (package, actor name) of the original.
    pub fn source(&self) -> (&str, &str) {
        self.from.split_once(':').unwrap_or(("", self.from.as_str()))
    }
    /// `<layout>:<name>` of the copy.
    pub fn actor_name(&self, layout: &str) -> String {
        format!("{layout}:{}", self.name.clone().unwrap_or_else(|| self.source().1.to_string()))
    }
    /// World transform from the original's placed `Location`/`Rotation` (`original`) to the copy:
    /// the copy stands at `at` with yaw `yaw`, pitch and roll kept.
    pub fn delta(&self, original: glam::Mat4) -> glam::Mat4 {
        let (_, rot, loc) = original.to_scale_rotation_translation();
        let (yaw0, _, _) = rot.to_euler(glam::EulerRot::ZYX);
        let new = glam::Mat4::from_translation(v3(self.at)) * glam::Mat4::from_rotation_z(self.yaw.to_radians() - yaw0) * glam::Mat4::from_quat(rot);
        new * (glam::Mat4::from_translation(loc) * glam::Mat4::from_quat(rot)).inverse()
    }
}

impl Brush {
    /// Local-to-world: translation to `at`, rotation by `yaw`.
    pub fn frame(&self) -> glam::Mat4 {
        glam::Mat4::from_translation(v3(self.at)) * glam::Mat4::from_rotation_z(self.yaw.to_radians())
    }

    /// Local boxes `(min, max)` of a box or stairs brush (ramps have none).
    pub fn local_boxes(&self) -> Vec<(glam::Vec3, glam::Vec3)> {
        use glam::Vec3;
        match &self.shape {
            Shape::Box((x, y, z)) => vec![(Vec3::new(-x / 2.0, -y / 2.0, 0.0), Vec3::new(x / 2.0, y / 2.0, *z))],
            Shape::Stairs { width, steps, rise, run } => (0..*steps).map(|i| (Vec3::new(i as f32 * run, -width / 2.0, 0.0), Vec3::new((i + 1) as f32 * run, width / 2.0, (i + 1) as f32 * rise))).collect(),
            Shape::Ramp { .. } => Vec::new(),
        }
    }
}

impl MapLayout {
    pub fn point(&self, name: &str) -> Option<&LayoutPoint> {
        self.points.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_moves_and_turns_the_original() {
        let a = ActorPlacement { from: "P:A".into(), name: None, at: (100.0, 0.0, 0.0), yaw: 90.0, locked: None, door_state: None };
        let orig = glam::Mat4::from_translation(glam::Vec3::new(5.0, 5.0, 5.0)) * glam::Mat4::from_rotation_z(0.3);
        let d = a.delta(orig);
        // The original's location lands on `at`; its X axis turns to the new yaw (90: +Y).
        assert!((d * orig).w_axis.truncate().distance(glam::Vec3::new(100.0, 0.0, 0.0)) < 1e-3);
        assert!((d * orig).x_axis.truncate().distance(glam::Vec3::Y) < 1e-4);
        assert_eq!(a.source(), ("P", "A"));
        assert_eq!(a.actor_name("Playground"), "Playground:A");
    }

    #[test]
    fn stairs_boxes_climb() {
        let b = Brush { at: (0.0, 0.0, 0.0), yaw: 0.0, shape: Shape::Stairs { width: 200.0, steps: 3, rise: 40.0, run: 50.0 }, material: None, collide: true, uv_scale: 256.0 };
        let bx = b.local_boxes();
        assert_eq!(bx.len(), 3);
        assert_eq!(bx[2].1.z, 120.0);
        assert_eq!(bx[2].0.x, 100.0);
    }
}
