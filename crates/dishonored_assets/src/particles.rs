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
}

fn floats(p: &Property) -> Vec<f32> {
    match p.array_raw() {
        Some((_, b)) => b.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect(),
        None => Vec::new(),
    }
}

/// A `RawDistributionFloat/Vector` struct property.
fn dist(props: &[Property], name: &str, dims: u8) -> Option<Dist> {
    let p = find(props, name)?;
    let Value::Struct(_, s) = &p.value else { return None };
    let table = find(s, "LookupTable").map(floats).unwrap_or_default();
    if table.len() < 2 + dims as usize {
        return None;
    }
    let op = match find(s, "Op").map(|p| &p.value) {
        Some(Value::Byte(b)) => *b,
        _ => 1,
    };
    let d = Dist { table, dims, ranged: matches!(op, 2 | 3), extreme: op == 3 };
    // A ranged table must hold whole (min, max) chunks; otherwise treat it as plain values.
    if d.ranged && (d.table.len() - 2) % (dims as usize * 2) != 0 {
        return Some(Dist { ranged: false, extreme: false, ..d });
    }
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
            "ParticleModuleVelocityOverLifetime" => em.unsupported.push(mclass.clone()),
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
                em.mesh = find(p, "Mesh").and_then(|x| x.as_object()).filter(|o| *o != 0).map(|o| mpkg.obj_path(ObjRef::from_index(o)));
                em.unsupported.push(mclass.clone());
            }
            c => em.unsupported.push(c.to_string()),
        }
    }
    em.unsupported.sort();
    em.unsupported.dedup();
    Some(em)
}
