//! Particle systems (UE3 `ParticleSystem` as cooked by Dishonored) in engine-agnostic form.
//! Distributions keep the cooked lookup tables (`FRawDistribution`): the editor-side
//! distribution objects are stripped by the cooker, the baked table is what the game samples.
//! See research/dishonored/EFFECTS.md.

use crate::MaterialId;
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};


/// A cooked `FRawDistributionFloat/Vector`. The cooked `LookupTable` is
/// [min, max, samples...]; each sample is `LookupTableChunkSize` floats made of
/// `LookupTableNumElements` values (1, or 2 = min then max for the random / extreme ops) of
/// `dims` components. Sample index = (t - start time) x time scale, linear between samples
/// (time scale 0 = constant). Fields absent from the cooked struct take the module class
/// default (`Default__ParticleModuleX` in Engine).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Dist {
    /// Samples only (the min/max header removed).
    pub table: Vec<f32>,
    /// 1 (float) or 3 (vector).
    pub dims: u8,
    /// `RDO_Random` / `RDO_Extreme`: samples hold (min, max).
    pub ranged: bool,
    pub extreme: bool,
    pub time_scale: f32,
    pub start_time: f32,
}

impl Dist {
    pub fn constant(v: [f32; 3], dims: u8) -> Self {
        Dist { table: v[..dims as usize].to_vec(), dims, ..Default::default() }
    }

    fn chunk(&self) -> usize {
        self.dims as usize * if self.ranged { 2 } else { 1 }
    }

    pub fn samples(&self) -> usize {
        self.table.len() / self.chunk().max(1)
    }

    /// Value at `t` (curve time: particle relative age, or emitter time) for a per-particle
    /// random number `r` in [0, 1).
    pub fn eval(&self, t: f32, r: f32) -> [f32; 3] {
        let n = self.samples();
        if n == 0 {
            return [0.0; 3];
        }
        let c = self.chunk();
        let x = ((t - self.start_time) * self.time_scale).max(0.0);
        let i = (x.floor() as usize).min(n - 1);
        let j = (i + 1).min(n - 1);
        let a = if i == j { 0.0 } else { x - i as f32 };
        let d = self.dims as usize;
        let mut out = [0.0; 3];
        for k in 0..d {
            let at = |e: usize, off: usize| self.table[e * c + off];
            let v = if self.ranged {
                let lo = at(i, k) + (at(j, k) - at(i, k)) * a;
                let hi = at(i, d + k) + (at(j, d + k) - at(i, d + k)) * a;
                if self.extreme { if r < 0.5 { lo } else { hi } } else { lo + (hi - lo) * r }
            } else {
                at(i, k) + (at(j, k) - at(i, k)) * a
            };
            out[k] = v;
        }
        if d == 1 {
            out[1] = out[0];
            out[2] = out[0];
        }
        out
    }

    pub fn eval1(&self, t: f32, r: f32) -> f32 {
        self.eval(t, r)[0]
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParticleBlend {
    #[default]
    Translucent,
    Additive,
    Masked,
    Opaque,
    Modulate,
}

/// One emitter (LOD 0) of a particle system.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParticleEmitterAsset {
    pub name: String,
    pub material: Option<MaterialId>,
    pub blend: ParticleBlend,
    /// `EParticleScreenAlignment`: 0 square, 1 rectangle, 2 velocity, 3 type-specific...
    pub screen_align: u8,
    pub local_space: bool,
    pub duration: f32,
    /// 0 = loop forever.
    pub loops: i32,
    pub delay: f32,
    pub sub_h: u32,
    pub sub_v: u32,
    /// Spawn rate (per second) over emitter time, times the rate scale.
    pub rate: Option<Dist>,
    pub rate_scale: Option<Dist>,
    /// (count, count low (-1 = none), emitter relative time).
    pub bursts: Vec<(i32, i32, f32)>,
    pub lifetime: Option<Dist>,
    pub size: Option<Dist>,
    pub velocity: Option<Dist>,
    pub velocity_radial: Option<Dist>,
    pub rotation: Option<Dist>,
    pub rotation_rate: Option<Dist>,
    pub color: Option<Dist>,
    pub alpha: Option<Dist>,
    pub color_over_life: Option<Dist>,
    pub alpha_over_life: Option<Dist>,
    pub size_over_life: Option<Dist>,
    /// MultiplyX/Y/Z of `ParticleModuleSizeMultiplyLife`.
    pub size_over_life_axes: [bool; 3],
    pub acceleration: Option<Dist>,
    pub drag: Option<Dist>,
    pub location: Option<Dist>,
    /// `ParticleModuleLocationPrimitiveSphere`: radius, positive axes, surface only, velocity
    /// (scale).
    pub sphere: Option<(Dist, [bool; 3], bool, Option<Dist>)>,
    /// `ParticleModuleLocationPrimitiveCylinder`: radius, height, surface only.
    pub cylinder: Option<(Dist, Dist, bool)>,
    pub sub_image: Option<Dist>,
    /// Mesh emitter (`ParticleModuleTypeDataMesh`).
    pub mesh: Option<String>,
    /// The mesh in the map's mesh table (mesh emitters).
    #[serde(default)]
    pub mesh_id: Option<usize>,
    /// Draw mesh particles with the emitter material (`bOverrideMaterial` / MeshMaterial).
    #[serde(default)]
    pub mesh_override_material: bool,
    /// `ParticleModuleMeshRotation` / `MeshRotationRate` (turns about X, Y, Z).
    #[serde(default)]
    pub mesh_rotation: Option<Dist>,
    #[serde(default)]
    pub mesh_rotation_rate: Option<Dist>,
    /// `ParticleModuleOrientationAxisLock` (`EParticleAxisLock`: 0 none, 1-3 X/Y/Z, 4-6 -X/-Y/-Z,
    /// 7-9 rotate about X/Y/Z).
    #[serde(default)]
    pub axis_lock: u8,
    /// `ParticleModuleVelocityOverLifetime`: VelOverLife over relative age, Absolute (replace the
    /// velocity) or scale it.
    #[serde(default)]
    pub vel_over_life: Option<(Dist, bool)>,
    /// `ParticleModuleOrbit` (chained by sum): offset, rotation and rotation rate (turns about
    /// X/Y/Z), sampled at spawn (emitter time).
    #[serde(default)]
    pub orbit: Vec<Orbit>,
    /// `ParticleModuleAttractorPoint`.
    #[serde(default)]
    pub attractor_point: Option<AttractorPoint>,
    /// `ParticleModuleAttractorLine`: end points (emitter space), range, strength (over the
    /// line-relative position).
    #[serde(default)]
    pub attractor_line: Option<([f32; 3], [f32; 3], Dist, Dist)>,
    /// `ParticleModuleCollision`: damping factor (vector), max collisions, completion option
    /// (0 kill, 1 freeze, 2 halt collisions, 3 freeze translation, 4 freeze rotation, 5 freeze
    /// movement).
    #[serde(default)]
    pub collision: Option<(Dist, Dist, u8)>,
    /// Trail / ribbon type data (`ParticleModuleTypeDataTrail2`, `ParticleModuleTypeDataRibbon`):
    /// particles of the emitter are drawn as one strip in spawn order; texture tiling.
    #[serde(default)]
    pub trail: Option<TrailParams>,
    /// Beam type data (`ParticleModuleTypeDataBeam2`): each particle is a beam from the emitter
    /// origin along its X axis (or to the `ParticleModuleBeamTarget` point).
    #[serde(default)]
    pub beam: Option<BeamParams>,
    /// Module classes present but not simulated (for the import report).
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Orbit {
    pub offset: Option<Dist>,
    pub rotation: Option<Dist>,
    pub rotation_rate: Option<Dist>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttractorPoint {
    pub position: Dist,
    pub range: Dist,
    pub strength: Dist,
    pub strength_by_distance: bool,
    pub affect_base_velocity: bool,
    pub world_space: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrailParams {
    /// Texture repeats along the whole strip.
    pub tiling: f32,
    pub ribbon: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BeamParams {
    /// Beam length along the emitter X axis (`Distance`, method distance).
    pub distance: Option<Dist>,
    /// `ParticleModuleBeamTarget.Target` (emitter space) when present.
    pub target: Option<Dist>,
    /// Texture tiling along the beam.
    pub tiling: f32,
    /// Noise displacement (`ParticleModuleBeamNoise.NoiseRange`) and point count.
    pub noise: Option<(Dist, u32)>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParticleSystemAsset {
    pub source: String,
    pub emitters: Vec<ParticleEmitterAsset>,
    pub warmup: f32,
}

/// A placed `Emitter` actor (or `ParticleSystemComponent` of another actor).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmitterInstance {
    pub system: usize,
    pub transform: Mat4,
    pub source: String,
    pub auto_activate: bool,
}

impl EmitterInstance {
    pub fn position(&self) -> Vec3 {
        self.transform.w_axis.truncate()
    }
}
