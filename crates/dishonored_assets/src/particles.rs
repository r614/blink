//! Decoding of cooked UE3 `ParticleSystem`s (Dishonored): emitters -> LOD 0 ->
//! required / spawn / type-data modules and the module list, distributions from their cooked
//! lookup tables. Research notes: research/dishonored/EFFECTS.md.

use crate::material_chain::Key;
use dishonored_ir::MaterialId;
use dishonored_ir::particles::{Dist, ParticleBlend, ParticleEmitterAsset, ParticleSystemAsset};
use std::sync::Arc;
use ue3_package::properties::{Property, Value, find};
use ue3_package::{ObjRef, Package};

/// What the decoder needs from the level importer.
pub trait ParticleCtx {
    fn package(&self, slot: usize) -> Arc<Package>;
    fn resolve_ref(&mut self, slot: usize, r: ObjRef) -> Option<Key>;
    fn import_material(&mut self, k: Key) -> Option<MaterialId>;
    fn material_blend(&self, id: MaterialId) -> ParticleBlend;
    fn import_mesh(&mut self, k: Key) -> Option<usize>;
    /// Properties of `Default__<class>` (Engine), for fields the cooked objects omit.
    fn class_default(&mut self, class: &str) -> Vec<Property>;
}

fn floats(p: &Property) -> Vec<f32> {
    match p.array_raw() {
        Some((_, b)) => b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
        None => Vec::new(),
    }
}

thread_local! {
    /// Class defaults of the module being decoded (fields the cooked struct omits).
    static DEFAULTS: std::cell::RefCell<Vec<Property>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// A `RawDistributionFloat/Vector` struct property; omitted fields come from the module's
/// class default object.
fn dist(props: &[Property], name: &str, dims: u8) -> Option<Dist> {
    let p = find(props, name)?;
    let Value::Struct(_, s) = &p.value else { return None };
    let def: Vec<Property> = DEFAULTS.with(|d| match find(&d.borrow(), name).map(|p| &p.value) {
        Some(Value::Struct(_, v)) => v.clone(),
        _ => Vec::new(),
    });
    let get = |n: &str| find(s, n).or_else(|| find(&def, n)).map(|p| p.value.clone());
    let byte = |n: &str, d: u8| match get(n) {
        Some(Value::Byte(b)) => b,
        Some(Value::Int(i)) => i as u8,
        _ => d,
    };
    let float = |n: &str| match get(n) {
        Some(Value::Float(f)) => f,
        _ => 0.0,
    };
    let table = find(s, "LookupTable").map(floats).unwrap_or_default();
    let op = byte("Op", 1);
    let elems = byte("LookupTableNumElements", 1).max(1);
    let chunk = byte("LookupTableChunkSize", elems * dims).max(1) as usize;
    if table.len() < 2 + chunk {
        return None;
    }
    let samples = table[2..].to_vec();
    let ranged = elems >= 2 && matches!(op, 2 | 3);
    let d = Dist { table: samples, dims: (chunk / elems as usize).clamp(1, 3) as u8, ranged, extreme: op == 3, time_scale: float("LookupTableTimeScale"), start_time: float("LookupTableStartTime") };
    // A vector table read as a float (or the reverse) keeps its stored dimension.
    let _ = dims;
    Some(d)
}

fn bool_or(props: &[Property], name: &str, d: bool) -> bool {
    match find(props, name).map(|p| &p.value) {
        Some(Value::Bool(b)) => *b,
        _ => d,
    }
}

fn enum_of(props: &[Property], name: &str) -> Option<String> {
    match find(props, name).map(|p| &p.value) {
        Some(Value::Enum(e)) => Some(e.clone()),
        Some(Value::Byte(b)) => Some(b.to_string()),
        _ => None,
    }
}

fn int_or(props: &[Property], name: &str, d: i32) -> i32 {
    match find(props, name).map(|p| &p.value) {
        Some(Value::Int(i)) => *i,
        _ => d,
    }
}

/// `EParticleEventType` (`EPET_Any`, `Spawn`, `Death`, `Collision`, `Kismet`) as the IR's kind.
fn event_kind(e: Option<&str>) -> u8 {
    use dishonored_ir::particles::{EVENT_ANY, EVENT_COLLISION, EVENT_DEATH, EVENT_SPAWN};
    match e {
        Some("EPET_Spawn") => EVENT_SPAWN,
        Some("EPET_Death") => EVENT_DEATH,
        Some("EPET_Collision") => EVENT_COLLISION,
        _ => EVENT_ANY,
    }
}

fn bursts(pkg: &Package, props: &[Property]) -> Vec<(i32, i32, f32)> {
    find(props, "BurstList")
        .and_then(|p| p.array_structs(pkg).ok())
        .unwrap_or_default()
        .iter()
        .map(|b| (int_or(b, "Count", 0), int_or(b, "CountLow", -1), find(b, "Time").and_then(|p| p.as_float()).unwrap_or(0.0)))
        .collect()
}

/// Decode a ParticleSystem export.
pub fn decode(ctx: &mut dyn ParticleCtx, k: Key) -> ParticleSystemAsset {
    let pkg = ctx.package(k.0);
    let mut ps = ParticleSystemAsset { source: pkg.obj_path(ObjRef::Export(k.1)), ..Default::default() };
    let Ok(h) = pkg.read_object(k.1) else { return ps };
    ps.warmup = find(&h.properties, "WarmupTime").and_then(|p| p.as_float()).unwrap_or(0.0);
    let emitters = find(&h.properties, "Emitters").and_then(|p| p.array_objects().ok()).unwrap_or_default();
    for e in emitters {
        let Some(ek) = ctx.resolve_ref(k.0, ObjRef::from_index(e)) else { continue };
        if let Some(em) = decode_emitter(ctx, ek) {
            ps.emitters.push(em);
        }
    }
    ps
}

fn decode_emitter(ctx: &mut dyn ParticleCtx, k: Key) -> Option<ParticleEmitterAsset> {
    let pkg = ctx.package(k.0);
    let class = pkg.export_class(k.1);
    let h = pkg.read_object(k.1).ok()?;
    let mut em = ParticleEmitterAsset {
        name: match find(&h.properties, "EmitterName").map(|p| &p.value) {
            Some(Value::Name(n)) => n.clone(),
            _ => pkg.obj_name(ObjRef::Export(k.1)),
        },
        duration: 1.0,
        sub_h: 1,
        sub_v: 1,
        ..Default::default()
    };
    if class != "ParticleSpriteEmitter" {
        em.unsupported.push(class);
    }
    let lods = find(&h.properties, "LODLevels").and_then(|p| p.array_objects().ok()).unwrap_or_default();
    let lod_k = ctx.resolve_ref(k.0, ObjRef::from_index(*lods.first()?))?;
    let lpkg = ctx.package(lod_k.0);
    let lod = lpkg.read_object(lod_k.1).ok()?;
    if !bool_or(&lod.properties, "bEnabled", true) {
        return None;
    }
    let mut modules: Vec<i32> = Vec::new();
    for n in ["RequiredModule", "SpawnModule", "TypeDataModule"] {
        if let Some(o) = find(&lod.properties, n).and_then(|p| p.as_object()).filter(|o| *o != 0) {
            modules.push(o);
        }
    }
    modules.extend(find(&lod.properties, "Modules").and_then(|p| p.array_objects().ok()).unwrap_or_default());
    for m in modules {
        let Some(mk) = ctx.resolve_ref(lod_k.0, ObjRef::from_index(m)) else { continue };
        let mpkg = ctx.package(mk.0);
        let mclass = mpkg.export_class(mk.1);
        let Ok(mh) = mpkg.read_object(mk.1) else { continue };
        let p = &mh.properties;
        let defaults = ctx.class_default(&mclass);
        DEFAULTS.with(|d| *d.borrow_mut() = defaults);
        if !bool_or(p, "bEnabled", true) {
            continue;
        }
        match mclass.as_str() {
            "ParticleModuleRequired" => {
                if let Some(mat) = find(p, "Material").and_then(|x| x.as_object()).filter(|o| *o != 0) {
                    if let Some(mk2) = ctx.resolve_ref(mk.0, ObjRef::from_index(mat)) {
                        em.material = ctx.import_material(mk2);
                        em.blend = em.material.map(|m| ctx.material_blend(m)).unwrap_or_default();
                    }
                }
                em.screen_align = match enum_of(p, "ScreenAlignment").as_deref() {
                    Some("PSA_Rectangle") => 1,
                    Some("PSA_Velocity") => 2,
                    Some("PSA_TypeSpecific") => 3,
                    _ => 0,
                };
                em.local_space = bool_or(p, "bUseLocalSpace", false);
                em.duration = find(p, "EmitterDuration").and_then(|x| x.as_float()).unwrap_or(1.0);
                em.loops = int_or(p, "EmitterLoops", 0);
                em.delay = find(p, "EmitterDelay").and_then(|x| x.as_float()).unwrap_or(0.0);
                em.sub_h = int_or(p, "SubImages_Horizontal", 1).max(1) as u32;
                em.sub_v = int_or(p, "SubImages_Vertical", 1).max(1) as u32;
                em.sub_random = matches!(enum_of(p, "InterpolationMethod").as_deref(), Some("PSUVIM_Random") | Some("PSUVIM_Random_Blend"));
                // Older layout: spawn rate and bursts on the required module.
                if em.rate.is_none() {
                    em.rate = dist(p, "SpawnRate", 1);
                }
                em.bursts.extend(bursts(&mpkg, p));
            }
            "ParticleModuleSpawn" => {
                em.rate = dist(p, "Rate", 1).or(em.rate.take());
                em.rate_scale = dist(p, "RateScale", 1);
                em.bursts.extend(bursts(&mpkg, p));
            }
            "ParticleModuleLifetime" => em.lifetime = dist(p, "Lifetime", 1),
            "ParticleModuleSize" => em.size = dist(p, "StartSize", 3),
            "ParticleModuleSizeMultiplyLife" => {
                em.size_over_life = dist(p, "LifeMultiplier", 3);
                em.size_over_life_axes = [bool_or(p, "MultiplyX", true), bool_or(p, "MultiplyY", true), bool_or(p, "MultiplyZ", true)];
            }
            "ParticleModuleVelocity" => {
                em.velocity = dist(p, "StartVelocity", 3);
                em.velocity_radial = dist(p, "StartVelocityRadial", 1);
            }
            "ParticleModuleRotation" => em.rotation = dist(p, "StartRotation", 1),
            "ParticleModuleRotationRate" => em.rotation_rate = dist(p, "StartRotationRate", 1),
            "ParticleModuleColor" => {
                em.color = dist(p, "StartColor", 3);
                em.alpha = dist(p, "StartAlpha", 1);
            }
            "ParticleModuleColorOverLife" => {
                em.color_over_life = dist(p, "ColorOverLife", 3);
                em.alpha_over_life = dist(p, "AlphaOverLife", 1);
            }
            "ParticleModuleAcceleration" => em.acceleration = dist(p, "Acceleration", 3),
            "ParticleModuleVelocityOverLifetime" => {
                em.vel_over_life = dist(p, "VelOverLife", 3).map(|d| (d, bool_or(p, "Absolute", false)));
            }
            "ParticleModuleOrbit" => em.orbit.push(dishonored_ir::particles::Orbit {
                offset: dist(p, "OffsetAmount", 3),
                rotation: dist(p, "RotationAmount", 3),
                rotation_rate: dist(p, "RotationRateAmount", 3),
            }),
            "ParticleModuleAttractorPoint" => {
                em.attractor_point = Some(dishonored_ir::particles::AttractorPoint {
                    position: dist(p, "Position", 3).unwrap_or_else(|| Dist::constant([0.0; 3], 3)),
                    range: dist(p, "Range", 1).unwrap_or_else(|| Dist::constant([0.0; 3], 1)),
                    strength: dist(p, "Strength", 1).unwrap_or_else(|| Dist::constant([0.0; 3], 1)),
                    strength_by_distance: bool_or(p, "StrengthByDistance", true),
                    affect_base_velocity: bool_or(p, "bAffectBaseVelocity", false),
                    world_space: bool_or(p, "bUseWorldSpacePosition", false),
                });
            }
            "ParticleModuleAttractorLine" => {
                let v = |n: &str| find(p, n).and_then(|x| x.as_vector()).unwrap_or([0.0; 3]);
                em.attractor_line = Some((
                    v("EndPoint0"),
                    v("EndPoint1"),
                    dist(p, "Range", 1).unwrap_or_else(|| Dist::constant([0.0; 3], 1)),
                    dist(p, "Strength", 1).unwrap_or_else(|| Dist::constant([0.0; 3], 1)),
                ));
            }
            "ParticleModuleCollision" => {
                let opt = match enum_of(p, "CollisionCompletionOption").as_deref() {
                    Some("EPCC_Freeze") => 1,
                    Some("EPCC_HaltCollisions") => 2,
                    Some("EPCC_FreezeTranslation") => 3,
                    Some("EPCC_FreezeRotation") => 4,
                    Some("EPCC_FreezeMovement") => 5,
                    _ => 0,
                };
                em.collision = Some((
                    dist(p, "DampingFactor", 3).unwrap_or_else(|| Dist::constant([1.0; 3], 3)),
                    dist(p, "MaxCollisions", 1).unwrap_or_else(|| Dist::constant([1.0; 3], 1)),
                    opt,
                ));
            }
            "ParticleModuleTypeDataTrail2" | "ParticleModuleTypeDataRibbon" | "ParticleModuleTypeDataAnimTrail" => {
                em.trail = Some(dishonored_ir::particles::TrailParams {
                    tiling: 1.0,
                    ribbon: mclass != "ParticleModuleTypeDataTrail2",
                });
            }
            "ParticleModuleTypeDataBeam2" => {
                let b = em.beam.get_or_insert_with(Default::default);
                b.distance = dist(p, "Distance", 1);
                b.tiling = find(p, "TextureTile").and_then(|x| x.as_float()).or_else(|| match find(p, "TextureTile").map(|x| &x.value) {
                    Some(Value::Int(i)) => Some(*i as f32),
                    _ => None,
                }).unwrap_or(1.0);
            }
            "ParticleModuleBeamTarget" => {
                em.beam.get_or_insert_with(Default::default).target = dist(p, "Target", 3);
            }
            "ParticleModuleBeamNoise" => {
                let pts = int_or(p, "NoiseTessellation", 1).max(1) as u32;
                if let Some(r) = dist(p, "NoiseRange", 3) {
                    em.beam.get_or_insert_with(Default::default).noise = Some((r, pts));
                }
            }
            "ParticleModuleLocation" => em.location = dist(p, "StartLocation", 3),
            "ParticleModuleLocationPrimitiveSphere" => {
                em.sphere = Some((
                    dist(p, "StartRadius", 1).unwrap_or_else(|| Dist::constant([50.0; 3], 1)),
                    [bool_or(p, "Positive_X", true), bool_or(p, "Positive_Y", true), bool_or(p, "Positive_Z", true)],
                    bool_or(p, "SurfaceOnly", false),
                    bool_or(p, "Velocity", false).then(|| dist(p, "VelocityScale", 1).unwrap_or_else(|| Dist::constant([1.0; 3], 1))),
                ));
            }
            "ParticleModuleLocationPrimitiveCylinder" => {
                em.cylinder = Some((
                    dist(p, "StartRadius", 1).unwrap_or_else(|| Dist::constant([50.0; 3], 1)),
                    dist(p, "StartHeight", 1).unwrap_or_else(|| Dist::constant([50.0; 3], 1)),
                    bool_or(p, "SurfaceOnly", false),
                ));
            }
            "ParticleModuleSubImageIndex" => em.sub_image = dist(p, "SubImageIndex", 1),
            "ParticleModuleTypeDataMesh" => {
                if let Some(o) = find(p, "Mesh").and_then(|x| x.as_object()).filter(|o| *o != 0) {
                    em.mesh = Some(mpkg.obj_path(ObjRef::from_index(o)));
                    em.mesh_id = ctx.resolve_ref(mk.0, ObjRef::from_index(o)).and_then(|k| ctx.import_mesh(k));
                }
                em.mesh_override_material = bool_or(p, "bOverrideMaterial", false);
            }
            "ParticleModuleOrientationAxisLock" => {
                em.axis_lock = match enum_of(p, "LockAxisFlags").as_deref() {
                    Some("EPAL_X") => 1,
                    Some("EPAL_Y") => 2,
                    Some("EPAL_Z") => 3,
                    Some("EPAL_NEGATIVE_X") => 4,
                    Some("EPAL_NEGATIVE_Y") => 5,
                    Some("EPAL_NEGATIVE_Z") => 6,
                    Some("EPAL_ROTATE_X") => 7,
                    Some("EPAL_ROTATE_Y") => 8,
                    Some("EPAL_ROTATE_Z") => 9,
                    _ => 0,
                };
            }
            "ParticleModuleMeshRotation" => em.mesh_rotation = dist(p, "StartRotation", 3),
            "ParticleModuleMeshRotationRate" => em.mesh_rotation_rate = dist(p, "StartRotationRate", 3),
            "ParticleModuleMeshMaterial" => {
                // First material of `MeshMaterials` replaces the mesh's own.
                if let Some(o) = find(p, "MeshMaterials").and_then(|x| x.array_objects().ok()).and_then(|v| v.first().copied()).filter(|o| *o != 0) {
                    if let Some(k2) = ctx.resolve_ref(mk.0, ObjRef::from_index(o)) {
                        em.material = ctx.import_material(k2);
                        em.blend = em.material.map(|m| ctx.material_blend(m)).unwrap_or_default();
                        em.mesh_override_material = true;
                    }
                }
            }
            "ParticleModuleTypeDataDecal" => {
                // Every particle is a decal (`m_pDecalMaterial` replaces the required module's
                // sprite material); where it lies comes from the event that spawned it.
                let mut d = dishonored_ir::particles::DecalEmitter { no_clip: bool_or(p, "m_bNoClip", false), on_skeletal: bool_or(p, "m_bProjectOnSkeletalMeshes", false), ..Default::default() };
                if let Some(mat) = find(p, "m_pDecalMaterial").and_then(|x| x.as_object()).filter(|o| *o != 0) {
                    if let Some(mk2) = ctx.resolve_ref(mk.0, ObjRef::from_index(mat)) {
                        d.material = ctx.import_material(mk2);
                        if d.material.is_some() {
                            em.material = d.material;
                            em.blend = em.material.map(|m| ctx.material_blend(m)).unwrap_or_default();
                        }
                    }
                }
                em.decal = Some(d);
            }
            "ParticleModuleEventGenerator" => {
                for e in find(p, "Events").and_then(|x| x.array_structs(&mpkg).ok()).unwrap_or_default() {
                    let name = match find(&e, "CustomName").map(|x| &x.value) {
                        Some(Value::Name(n)) => n.clone(),
                        _ => String::new(),
                    };
                    em.events_out.push(dishonored_ir::particles::EventOut { kind: event_kind(enum_of(&e, "Type").as_deref()), name, first_only: bool_or(&e, "FirstTimeOnly", false) });
                }
            }
            "ParticleModuleEventReceiverSpawn" => {
                let name = match find(p, "EventName").map(|x| &x.value) {
                    Some(Value::Name(n)) => n.clone(),
                    _ => String::new(),
                };
                let kind = enum_of(p, "EventGeneratorType").or_else(|| DEFAULTS.with(|d| enum_of(&d.borrow(), "EventGeneratorType")));
                em.events_in.push(dishonored_ir::particles::EventIn { kind: event_kind(kind.as_deref()), name, count: dist(p, "SpawnCount", 1), inherit_velocity: dist(p, "InheritVelocityScale", 3) });
            }
            c => em.unsupported.push(c.to_string()),
        }
    }
    em.unsupported.sort();
    em.unsupported.dedup();
    Some(em)
}
