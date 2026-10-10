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

    /// A vector value with the engine's per-component random draws (`RDO_Random` tables,
    /// 0x0042ACA0 for `FRawDistributionVector::GetValue` 0x004353B0): one `appFrand` per axis
    /// unless the table's `Type` locks axes together (`EDVLF_XY` 1, `XZ` 2, `YZ` 3, `XYZ` 4).
    /// Constant and extreme tables are as [`Dist::eval`].
    pub fn eval_rand(&self, t: f32, lock: u8, rnd: &mut dyn FnMut() -> f32) -> [f32; 3] {
        if !self.ranged || self.extreme || self.dims != 3 {
            return self.eval(t, rnd());
        }
        let r = match lock & 7 {
            1 => {
                let a = rnd();
                [a, a, rnd()]
            }
            2 => {
                let a = rnd();
                let b = rnd();
                [a, b, a]
            }
            3 => {
                let a = rnd();
                let b = rnd();
                [a, b, b]
            }
            4 => {
                let a = rnd();
                [a, a, a]
            }
            _ => [rnd(), rnd(), rnd()],
        };
        [self.eval(t, r[0])[0], self.eval(t, r[1])[1], self.eval(t, r[2])[2]]
    }
}

/// `DistributionFloatParticleParameter`: the value of a `ParticleSystemComponent` instance
/// parameter (`SetFloatParameter`), mapped by `ParamMode` (0x005E1FA0,
/// `DistributionFloatParameterBase::GetValue`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParticleParam {
    pub name: String,
    /// Used when the instance has no such parameter.
    pub constant: f32,
    pub min_input: f32,
    pub max_input: f32,
    pub min_output: f32,
    pub max_output: f32,
    /// `DistributionParamMode`: 0 `DPM_Normal`, 1 `DPM_Abs`, 2 `DPM_Direct`.
    pub mode: u8,
}

impl ParticleParam {
    /// 0x005E1FA0: the instance's value (else `Constant`); `DPM_Direct` returns it, `DPM_Abs`
    /// takes its absolute value first; otherwise it is clamped to [MinInput, MaxInput] and
    /// mapped linearly onto [MinOutput, MaxOutput] (slope 0 when MinInput >= MaxInput).
    pub fn value(&self, instance: Option<f32>) -> f32 {
        let mut v = instance.unwrap_or(self.constant);
        if self.mode == 2 {
            return v;
        }
        if self.mode == 1 {
            v = v.abs();
        }
        let (lo, hi) = (self.min_input, self.max_input);
        let slope = if lo < hi { (self.max_output - self.min_output) / (hi - lo) } else { 0.0 };
        let x = if lo <= v { if hi <= v { hi } else { v } } else { lo };
        (x - lo) * slope + self.min_output
    }
}

/// A `RawDistributionFloat` that may be a particle parameter: the cooked table, or (when its
/// `Distribution` is a `DistributionFloatParticleParameter`, which the cooker cannot bake) the
/// parameter (`FRawDistributionFloat::GetValue` 0x00434EF0 calls the distribution object then).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FloatDist {
    pub table: Option<Dist>,
    pub param: Option<ParticleParam>,
}

impl FloatDist {
    pub fn value(&self, t: f32, r: f32, params: &dyn Fn(&str) -> Option<f32>) -> f32 {
        match (&self.param, &self.table) {
            (Some(p), _) => p.value(params(&p.name)),
            (None, Some(d)) => d.eval1(t, r),
            _ => 0.0,
        }
    }
}

/// `DisParticleModuleRainDrops` (DishonoredGame): drops kept inside a box about the emitter
/// (the player camera moves the emitter, `DishonoredPlayerCamera::SetRainEmitter`).
/// research/dishonored/systems/rain.md.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RainDrops {
    /// `m_Extent`: half size of the box.
    pub extent: [f32; 3],
    /// `m_MaxParticles` (the camera's `MaxParticles` parameter).
    pub max_particles: FloatDist,
    /// `m_SpawnKillRate` (the camera's `SpawnKillRate` parameter).
    pub spawn_kill_rate: FloatDist,
    pub start_velocity: Option<Dist>,
    pub start_color: Option<Dist>,
    pub start_alpha: Option<Dist>,
    pub start_size: Option<Dist>,
    pub fading_rate: Option<Dist>,
    /// The `Type` (axis lock) of the vector tables: velocity, colour, size.
    pub locks: [u8; 3],
    /// The module is the LOD's first module (`Modules[0]`): what `DisSeqAct_SetRainEmitter`
    /// (0x00B9ADF0) requires of the template's first emitter.
    pub first_module: bool,
}

/// `DisParticleModuleRainImpacts`: particles placed at the impacts the camera traced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RainImpacts {
    /// `m_bFixedLocation`: the impact's (reflected) direction is added to the velocity.
    pub fixed_location: bool,
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
    /// `InterpolationMethod` = `PSUVIM_Random` / `PSUVIM_Random_Blend`: each particle shows a
    /// random sub-image (without a SubImageIndex module).
    #[serde(default)]
    pub sub_random: bool,
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
    /// `ParticleModuleLocationPrimitiveSphere` / `Cylinder` (the cylinder carries its height).
    #[serde(default)]
    pub sphere: Option<LocationPrimitive>,
    #[serde(default)]
    pub cylinder: Option<LocationPrimitive>,
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
    /// `ParticleModuleTypeDataDecal`: every particle is a decal lying on the surface it was
    /// placed on (`Particle::normal`).
    #[serde(default)]
    pub decal: Option<DecalEmitter>,
    /// `ParticleModuleEventGenerator.Events`: what the emitter reports to the system.
    #[serde(default)]
    pub events_out: Vec<EventOut>,
    /// `ParticleModuleEventReceiverSpawn`: events that spawn this emitter's particles.
    #[serde(default)]
    pub events_in: Vec<EventIn>,
    /// `DisParticleModuleRainDrops`.
    #[serde(default)]
    pub rain_drops: Option<RainDrops>,
    /// `DisParticleModuleRainImpacts`.
    #[serde(default)]
    pub rain_impacts: Option<RainImpacts>,
    /// `ParticleModuleSizeMultiplyVelocity`: `VelocityMultiplier` (over relative time) and
    /// MultiplyX/Y/Z: Size *= multiplier x |Velocity| on the enabled axes, at spawn and every
    /// update.
    #[serde(default)]
    pub size_mult_velocity: Option<(Dist, [bool; 3])>,
    /// `ParticleModuleVelocity.bInWorldSpace`: the start velocity is not turned by the component
    /// (0x008B8C20).
    #[serde(default)]
    pub velocity_world: bool,
    /// `ParticleModuleAcceleration.bAlwaysInWorldSpace` (0x008B5A20 / 0x008B5D50).
    #[serde(default)]
    pub accel_world: bool,
    /// `ParticleModuleAccelerationOverLifetime`: `AccelOverLife` at the relative time,
    /// `bAlwaysInWorldSpace` (0x008B67A0).
    #[serde(default)]
    pub accel_over_life: Option<(Dist, bool)>,
    /// `ParticleModuleColorScaleOverLife`: `ColorScaleOverLife`, `AlphaScaleOverLife`,
    /// `bEmitterTime` (0x0089F040 / 0x008BA1E0).
    #[serde(default)]
    pub color_scale_over_life: Option<(Option<Dist>, Option<Dist>, bool)>,
    /// `ParticleModuleRotationRateMultiplyLife.LifeMultiplier` (0x0089E270).
    #[serde(default)]
    pub rotation_rate_life: Option<Dist>,
    /// `ParticleModuleMeshRotationRateOverLife`: `RotRate` (turns), `bScaleRotRate` (0x008A5000).
    #[serde(default)]
    pub mesh_rotation_rate_life: Option<(Dist, bool)>,
    /// `ParticleModuleSizeScale.SizeScale`: Size = BaseSize x scale (0x0089E040).
    #[serde(default)]
    pub size_scale: Option<Dist>,
    /// `ParticleModuleSizeScaleByTime`: `SizeScaleByTime` over the particle's own clock,
    /// `bEnableX/Y/Z` (0x0089E120).
    #[serde(default)]
    pub size_scale_by_time: Option<(Dist, [bool; 3])>,
    /// `ParticleModuleKillHeight`: `Height`, `bAbsolute`, `bFloor` (0x008BA960).
    #[serde(default)]
    pub kill_height: Option<(Dist, bool, bool)>,
    /// `ParticleModuleVelocityInheritParent.Scale` (0x008B92A0).
    #[serde(default)]
    pub velocity_inherit_parent: Option<Dist>,
    /// `ParticleModuleLocationEmitter` (0x008ABCE0).
    #[serde(default)]
    pub location_emitter: Option<LocationEmitter>,
    /// `ParticleModuleRequired.InterpolationMethod` (`EParticleSubUVInterpMethod`: 0 none,
    /// 1 linear, 2 linear blend, 3 random, 4 random blend).
    #[serde(default)]
    pub sub_method: u8,
    /// `ParticleModuleRequired.SortMode` (`EParticleSortMode`: 0 none, 1 view-projection depth,
    /// 2 distance to the view, 3 oldest first, 4 newest first).
    #[serde(default)]
    pub sort_mode: u8,
    /// `EmitterDelayLow` / `EmitterDurationLow` with `bEmitterDelayUseRange` /
    /// `bEmitterDurationUseRange`, `bDelayFirstLoopOnly`, `bDurationRecalcEachLoop`
    /// (0x00882DF0, 0x00887270).
    #[serde(default)]
    pub delay_low: Option<f32>,
    #[serde(default)]
    pub duration_low: Option<f32>,
    #[serde(default)]
    pub delay_first_loop_only: bool,
    #[serde(default)]
    pub duration_recalc: bool,
    /// `ParticleModuleRequired.bKillOnDeactivate`: a deactivated instance kills the emitter's
    /// particles at once (others let them live out their lifetime).
    #[serde(default)]
    pub kill_on_deactivate: bool,
    /// Module classes present but not simulated (for the import report).
    pub unsupported: Vec<String>,
}

/// `ParticleModuleLocationPrimitiveBase` as a sphere or a cylinder (spawn 0x008B42A0 /
/// 0x008B3A40): the signs a sample may take on each axis (`Positive_X/Y/Z`, `Negative_X/Y/Z`;
/// all on in `Default__ParticleModuleLocationPrimitiveBase`), `SurfaceOnly`, `Velocity` (the
/// offset becomes velocity x `VelocityScale`) and `StartLocation`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocationPrimitive {
    pub positive: [bool; 3],
    pub negative: [bool; 3],
    pub surface_only: bool,
    pub velocity: Option<Dist>,
    pub start_location: Option<Dist>,
    pub radius: Dist,
    /// Cylinder: `StartHeight`, `HeightAxis` (0 X, 1 Y, 2 Z), `RadialVelocity`.
    pub height: Option<Dist>,
    pub height_axis: u8,
    pub radial_velocity: bool,
}

/// `ParticleModuleLocationEmitter`: spawn at a live particle of the emitter named `EmitterName`
/// (`SelectionMethod` random or sequential), adding its velocity x `InheritSourceVelocityScale`
/// with `InheritSourceVelocity`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocationEmitter {
    pub emitter: String,
    pub sequential: bool,
    pub inherit_velocity: Option<f32>,
}

/// `ParticleModuleTypeDataDecal` (`m_pDecalMaterial`, `m_bNoClip`, `m_bProjectOnSkeletalMeshes`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DecalEmitter {
    pub material: Option<MaterialId>,
    pub no_clip: bool,
    pub on_skeletal: bool,
}

/// `EParticleEventType` of the event modules: 0 spawn, 1 death, 2 collision, 3 any (a receiver
/// only).
pub const EVENT_SPAWN: u8 = 0;
pub const EVENT_DEATH: u8 = 1;
pub const EVENT_COLLISION: u8 = 2;
pub const EVENT_ANY: u8 = 3;

/// One `ParticleModuleEventGenerator` event: its type, `CustomName` and `FirstTimeOnly`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventOut {
    pub kind: u8,
    pub name: String,
    pub first_only: bool,
}

/// `ParticleModuleEventReceiverSpawn`: `EventGeneratorType`, `EventName`, `SpawnCount` and
/// `InheritVelocityScale`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventIn {
    pub kind: u8,
    pub name: String,
    pub count: Option<Dist>,
    pub inherit_velocity: Option<Dist>,
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
    /// `ParticleModuleTypeDataAnimTrail`: knots span the two edges the game sets each update
    /// (a weapon's melee extent corners) instead of facing the camera.
    #[serde(default)]
    pub anim: bool,
    /// AnimTrail `bRenderTessellation`: segments longer than `DistanceTessellationStepSize` are
    /// subdivided along the knots' spline.
    #[serde(default)]
    pub tessellation_step: Option<f32>,
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
