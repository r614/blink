//! Per-map AI data from the level packages: factions (`DisTweaks_Faction`) and the pawn
//! tweaks' factions, forbidden zones (`DisForbiddenZone`), alarm bells (`DisAlarmBell` with
//! `DisTweaks_AlarmBell`) and the lights the AI light probe samples. See
//! `research/dishonored/AI.md` ("Factions", "Alarm bells", "Light at the player").

use crate::content::Content;
use crate::tweaks::{TMap, TVal, TweakDb, get_f32, get_obj};
use anyhow::Result;
use dishonored_ir::ai::{AiLevel, AlarmBellDef, AudioCell, AudioCells, AudioPortal, AudioPropagation, Faction, FactionTable, ForbiddenZone, ProbeLight};
use dishonored_ir::{Light, LightKind};
use glam::{Mat3, Vec3};
use ue3_package::properties::{Property, Value, find};
use ue3_package::{ObjRef, Package};

/// The player pawn tweak (its `m_pFactionTweak` is the player's faction).
const CORVO: &str = "Twk_Pawn_Corvo.Twk_Pawn_Corvo_Release";

/// `EAINoiseLoudness` names in enum order (index into `m_LoudnessDistanceArray`).
pub const LOUDNESS_NAMES: [&str; 17] = [
    "EAINoiseSilent",
    "EAINoiseLoudness2m",
    "EAINoiseWhisper",
    "EAINoiseLoudness4m",
    "EAINoiseLoudness1",
    "EAINoiseLoudness6m",
    "EAINoiseLoudness8m",
    "EAINoiseLoudness10m",
    "EAINoiseLoudness12m",
    "EAINoiseLoudness2",
    "EAINoiseLoudness20m",
    "EAINoiseLoudness25m",
    "EAINoiseLoudness3",
    "EAINoiseLoudness40m",
    "EAINoiseLoudness50m",
    "EAINoiseLoudness4",
    "EAINoise_CUSTOM_KISMET_SPECIFIED",
];

fn objs(m: &TMap, key: &str) -> Vec<String> {
    match m.get(key) {
        Some(TVal::List(l)) => l.iter().filter_map(|e| get_obj(e, "obj").map(str::to_string)).filter(|s| !s.is_empty()).collect(),
        _ => Vec::new(),
    }
}

fn faction(db: &TweakDb, path: &str) -> Option<Faction> {
    let m = db.try_resolve(path)?;
    // The fallback chain is a list of `Pkg.Obj` names (nearest first).
    let parents = match m.get("m_FallbackChainCooked") {
        Some(TVal::List(l)) => l.iter().filter_map(|e| e.values().find_map(|v| if let TVal::Str(s) | TVal::Name(s) = v { Some(s.clone()) } else { None })).filter(|s| !s.is_empty()).collect(),
        _ => Vec::new(),
    };
    Some(Faction { name: path.to_string(), allies: objs(&m, "m_AlliedFactions"), enemies: objs(&m, "m_EnemyFactions"), parents })
}

/// Read the AI level data. `packages` are the map's packages (persistent level and
/// sublevels); `tweaks` are the pawn tweaks of the map's spawners.
pub fn extract_ai_level(content: &Content, packages: &[String], tweaks: &[String], loudness: &[f32]) -> Result<AiLevel> {
    let pkgs: Vec<String> = packages.iter().filter(|p| content.has_package(p)).cloned().collect();
    let db = TweakDb::load_with(content, &pkgs)?;
    let mut out = AiLevel::default();
    let mut table = FactionTable::default();
    for path in db.paths_with_class("DisTweaks_Faction") {
        if path.starts_with("Default__") || table.index(&path).is_some() {
            continue;
        }
        if let Some(f) = faction(&db, &path) {
            table.factions.push(f);
        }
    }
    // Factions named by others but not loaded relate to nothing but themselves.
    let named: Vec<String> = table.factions.iter().flat_map(|f| f.allies.iter().chain(f.enemies.iter()).cloned()).collect();
    for n in named {
        table.intern(&n);
    }
    for t in tweaks {
        if out.tweak_factions.contains_key(t) {
            continue;
        }
        if let Some(f) = db.try_resolve(t).and_then(|m| get_obj(&m, "m_pFactionTweak").map(str::to_string)).filter(|f| !f.is_empty()) {
            table.intern(&f);
            out.tweak_factions.insert(t.clone(), f);
        }
    }
    out.player_faction = db.try_resolve(CORVO).and_then(|m| get_obj(&m, "m_pFactionTweak").map(str::to_string)).unwrap_or_else(|| "DisFaction_Defaults.Faction_Corvo_Default".into());
    table.intern(&out.player_faction);
    // Generic dialog voices of the pawn tweaks (`m_GenericDialogData`).
    for t in tweaks {
        if out.generic_trees.contains_key(t) {
            continue;
        }
        if let Some(m) = db.try_resolve(t) {
            let voices = objs(&m, "m_GenericDialogData");
            if !voices.is_empty() {
                out.generic_trees.insert(t.clone(), voices);
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in &pkgs {
        if !seen.insert(name.to_ascii_lowercase()) {
            continue;
        }
        let pkg = content.package(name)?;
        if out.audio.propagation.is_none() {
            if let Some(a) = read_audio_cells(&pkg) {
                out.audio = a;
            }
        }
        out.sky_light += sky_lights(&pkg);
        for i in 0..pkg.exports.len() {
            if pkg.obj_name(ObjRef::from_index(pkg.exports[i].outer)) != "PersistentLevel" {
                continue;
            }
            let class = pkg.export_class(i);
            match class.as_str() {
                "DisForbiddenZone" => {
                    if let Some(z) = read_zone(&pkg, i) {
                        for f in z.owning.iter().chain(z.forbidden.iter()) {
                            table.intern(f);
                        }
                        out.zones.push(z);
                    }
                }
                "DisAlarmBell" => {
                    if let Some(b) = read_bell(&pkg, i, name, &db, loudness) {
                        out.alarms.push(b);
                    }
                }
                c if c.ends_with("Light") || c.contains("LightToggleable") || c.contains("LightMovable") => {
                    if let Ok(Some(l)) = crate::level::read_light(&pkg, i, c) {
                        out.lights.push(probe_light(&l));
                    }
                }
                _ => {}
            }
        }
    }
    out.factions = table;
    Ok(out)
}

fn probe_light(l: &Light) -> ProbeLight {
    // `FLinearColor::GetLuminance` (0.3, 0.59, 0.11), as the light environment reduces its SH
    // to a brightness (0x004da330).
    let lum = luminance(l.color);
    let (kind, inner, outer) = match l.kind {
        LightKind::Point => (0, 0.0, 0.0),
        LightKind::Spot { inner_cone_deg, outer_cone_deg } => (1, inner_cone_deg, outer_cone_deg),
        LightKind::Directional => (2, 0.0, 0.0),
    };
    ProbeLight { kind, position: l.position, direction: l.direction, intensity: l.brightness * lum, radius: l.radius, falloff_exponent: l.falloff_exponent, inner_cone: inner, outer_cone: outer, casts_shadows: l.casts_shadows }
}

fn luminance(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

/// The package's enabled `SkyLightComponent`s as an ambient brightness for the light probe:
/// the light environment adds a sky light as upper and lower hemisphere functions, whose
/// band-0 coefficient times 2 sqrt(pi) is half of each hemisphere's brightness (LIKELY: the
/// hemisphere normalization of the engine's sky functions was not decompiled, U-121).
fn sky_lights(pkg: &Package) -> f32 {
    let mut sum = 0.0;
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "SkyLightComponent" {
            continue;
        }
        let Ok(h) = pkg.read_object(i) else { continue };
        let c = &h.properties;
        if matches!(find(c, "bEnabled").map(|x| &x.value), Some(Value::Bool(false))) {
            continue;
        }
        let color = |k: &str| match find(c, k).map(|p| &p.value) {
            Some(Value::Color(rgba)) => [0, 1, 2].map(|j| crate::lightmap::srgb_to_linear(rgba[j])),
            _ => [1.0; 3],
        };
        let upper = find(c, "Brightness").and_then(|x| x.as_float()).unwrap_or(1.0) * luminance(color("LightColor"));
        let lower = find(c, "LowerBrightness").and_then(|x| x.as_float()).unwrap_or(0.0) * luminance(color("LowerColor"));
        sum += 0.5 * (upper + lower);
    }
    sum
}

/// Convex hulls of a brush actor (`BrushComponent.BrushAggGeom.ConvexElems[].FacePlaneData`,
/// actor-local planes) in world space (location, rotation, `DrawScale3D * DrawScale`,
/// `PrePivot`).
fn brush_hulls(pkg: &Package, p: &[Property]) -> Vec<Vec<[f32; 4]>> {
    brush_world_hulls(pkg, p).unwrap_or_default()
}

/// World-space convex hulls of a brush actor (volumes); `None` without a readable
/// `BrushComponent`. Planes are `(n, d)`, inside when `n . p <= d` for every plane of one hull.
pub fn brush_world_hulls(pkg: &Package, p: &[Property]) -> Option<Vec<Vec<[f32; 4]>>> {
    let loc = Vec3::from(find(p, "Location").and_then(|x| x.as_vector()).unwrap_or([0.0; 3]));
    let rot = find(p, "Rotation").and_then(|x| x.as_rotator()).unwrap_or([0; 3]);
    let ds = find(p, "DrawScale").and_then(|x| x.as_float()).unwrap_or(1.0);
    let ds3 = Vec3::from(find(p, "DrawScale3D").and_then(|x| x.as_vector()).unwrap_or([1.0; 3])) * ds;
    let pivot = Vec3::from(find(p, "PrePivot").and_then(|x| x.as_vector()).unwrap_or([0.0; 3]));
    let r = Mat3::from_mat4(crate::util::rotator_matrix(rot[0], rot[1], rot[2]));
    let mut hulls = Vec::new();
    let comp = find(p, "BrushComponent").and_then(|x| x.as_object()).map(ObjRef::from_index)?;
    let ObjRef::Export(ci) = comp else { return None };
    let ch = pkg.read_object(ci).ok()?;
    if let Some(Value::Struct(_, geom)) = find(&ch.properties, "BrushAggGeom").map(|x| &x.value) {
        if let Some(elems) = find(geom, "ConvexElems").and_then(|x| x.array_structs(pkg).ok()) {
            for e in elems {
                let Some((n, raw)) = find(&e, "FacePlaneData").and_then(|x| x.array_raw()) else { continue };
                let mut planes = Vec::new();
                for k in 0..n {
                    let Some(b) = raw.get(k * 16..k * 16 + 16) else { break };
                    let f = |o: usize| f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
                    // Serialized (W, X, Y, Z), like tagged matrix rows.
                    let (nl, w) = (Vec3::new(f(4), f(8), f(12)), f(0));
                    // local q = S^-1 R^-1 (x - loc) + pivot; n.q <= w.
                    let nw = r * (nl / ds3.max(Vec3::splat(1e-6)));
                    let d = w - nl.dot(pivot) + nw.dot(loc);
                    planes.push([nw.x, nw.y, nw.z, d]);
                }
                if !planes.is_empty() {
                    hulls.push(planes);
                }
            }
        }
    }
    Some(hulls)
}

/// The audio cells of a package that holds a `DisAudioPropagationInfo` (its WorldInfo's):
/// `DishonoredAudioVolume`s (hulls, `m_AudioPropagationIndex`, `m_LinkedPortals`),
/// `DishonoredAudioPortal`s (`m_WidthDir`, `m_HeightDir`, `m_PlaneNormal`, `m_fHalfWidth`,
/// `m_fHalfHeight`, `m_pCellA/B`, `m_fOcclusion_HeardByAI`) and the path table.
fn read_audio_cells(pkg: &Package) -> Option<AudioCells> {
    let pi = (0..pkg.exports.len()).find(|&i| pkg.export_class(i) == "DisAudioPropagationInfo")?;
    let ph = pkg.read_object(pi).ok()?;
    let pp = &ph.properties;
    let f32_of = |p: &[Property], k: &str, d: f32| find(p, k).and_then(|x| x.as_float()).unwrap_or(d);
    let byte_of = |p: &[Property], k: &str| match find(p, k).map(|x| &x.value) {
        Some(Value::Byte(b)) => *b,
        _ => 0,
    };
    let num = match find(pp, "m_NumPaths").map(|x| &x.value) {
        Some(Value::Int(n)) => (*n).max(0) as usize,
        _ => 0,
    };
    let data = find(pp, "m_PathData").and_then(|x| x.array_raw()).map(|(n, raw)| raw[..n.min(raw.len())].to_vec()).unwrap_or_default();
    let word = |k: usize| data.get(k * 4..k * 4 + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let keys: Vec<u32> = (0..num).filter_map(word).collect();
    let values: Vec<u32> = (num..2 * num).filter_map(word).collect();
    let mut cells = Vec::new();
    let mut cell_ix = std::collections::BTreeMap::new();
    let mut portals = Vec::new();
    let mut portal_ix = std::collections::BTreeMap::new();
    let mut raw_portals = Vec::new();
    for i in 0..pkg.exports.len() {
        match pkg.export_class(i).as_str() {
            "DishonoredAudioVolume" => {
                let Ok(h) = pkg.read_object(i) else { continue };
                cell_ix.insert(i as i32 + 1, cells.len());
                cells.push((AudioCell { name: pkg.obj_name(ObjRef::Export(i)).to_string(), index: byte_of(&h.properties, "m_AudioPropagationIndex"), hulls: brush_hulls(pkg, &h.properties), portals: Vec::new() }, obj_list(pkg, &h.properties, "m_LinkedPortals")));
            }
            "DishonoredAudioPortal" => {
                let Ok(h) = pkg.read_object(i) else { continue };
                let p = &h.properties;
                let v = |k: &str| Vec3::from(find(p, k).and_then(|x| x.as_vector()).unwrap_or([0.0; 3]));
                portal_ix.insert(i as i32 + 1, portals.len());
                portals.push(AudioPortal {
                    name: pkg.obj_name(ObjRef::Export(i)).to_string(),
                    index: byte_of(p, "m_AudioPropagationIndex"),
                    origin: v("Location"),
                    x_axis: v("m_WidthDir"),
                    y_axis: v("m_HeightDir"),
                    normal: v("m_PlaneNormal"),
                    half: [f32_of(p, "m_fHalfWidth", 0.0), f32_of(p, "m_fHalfHeight", 0.0)],
                    cell_a: None,
                    cell_b: None,
                    occlusion_ai: f32_of(p, "m_fOcclusion_HeardByAI", 0.0),
                });
                raw_portals.push((find(p, "m_pCellA").and_then(|x| x.as_object()), find(p, "m_pCellB").and_then(|x| x.as_object())));
            }
            _ => {}
        }
    }
    for (p, (a, b)) in portals.iter_mut().zip(raw_portals) {
        p.cell_a = a.and_then(|o| cell_ix.get(&o).copied());
        p.cell_b = b.and_then(|o| cell_ix.get(&o).copied());
    }
    let cells: Vec<AudioCell> = cells
        .into_iter()
        .map(|(mut c, linked)| {
            c.portals = linked.into_iter().filter_map(|o| portal_ix.get(&o).copied()).collect();
            c
        })
        .collect();
    let prop_portals: Vec<usize> = obj_list(pkg, pp, "m_Portals").into_iter().map(|o| portal_ix.get(&o).copied().unwrap_or(usize::MAX)).collect();
    let propagation = AudioPropagation { dist_coeff: f32_of(pp, "m_fDistCoeff", 1.0), max_audible_distance: f32_of(pp, "m_fMaxAudibleDistance", 0.0), keys, values, data, portals: prop_portals };
    Some(AudioCells { cells, portals, propagation: Some(propagation) })
}

fn obj_list(_pkg: &Package, props: &[Property], name: &str) -> Vec<i32> {
    find(props, name).and_then(|p| p.array_objects().ok()).unwrap_or_default()
}

/// A forbidden zone: factions, `m_bEnabled` (class default true) and the brush's convex
/// hulls (`BrushComponent.BrushAggGeom.ConvexElems[].FacePlaneData`, actor-local planes)
/// moved to world space with the actor's location, rotation, `DrawScale3D * DrawScale` and
/// `PrePivot`.
fn read_zone(pkg: &Package, i: usize) -> Option<ForbiddenZone> {
    let h = pkg.read_object(i).ok()?;
    let p = &h.properties;
    let names = |n: &str| obj_list(pkg, p, n).into_iter().map(|o| pkg.obj_path(ObjRef::from_index(o))).filter(|s| !s.is_empty()).collect::<Vec<_>>();
    let enabled = !matches!(find(p, "m_bEnabled").map(|x| &x.value), Some(Value::Bool(false)));
    find(p, "BrushComponent")?;
    let hulls = brush_hulls(pkg, p);
    Some(ForbiddenZone { name: pkg.obj_name(ObjRef::Export(i)).to_string(), enabled, owning: names("m_OwningFactions"), forbidden: names("m_ForbiddenFactions"), hulls })
}

fn read_bell(pkg: &Package, i: usize, package: &str, db: &TweakDb, loudness: &[f32]) -> Option<AlarmBellDef> {
    let h = pkg.read_object(i).ok()?;
    let p = &h.properties;
    let tweak = find(p, "m_pAlarmBellTweaks").and_then(|x| x.as_object()).map(|o| pkg.obj_path(ObjRef::from_index(o))).unwrap_or_default();
    let t = db.try_resolve(&tweak).unwrap_or_default();
    let f = |k: &str, d: f32| get_f32(&t, k).unwrap_or(d);
    let loud = match t.get("m_AlarmLoudness") {
        Some(TVal::Name(n)) => LOUDNESS_NAMES.iter().position(|x| x == n).and_then(|i| loudness.get(i).copied()),
        Some(TVal::I(v)) => loudness.get(*v as usize).copied(),
        _ => None,
    };
    let over = matches!(find(p, "m_bOverrideMaxSpawnedPawnsPerRing").map(|x| &x.value), Some(Value::Bool(true)));
    let max_spawns = if over {
        match find(p, "m_MaxSpawnedPawnsPerRingOverride").map(|x| &x.value) {
            Some(Value::Int(v)) => *v,
            _ => 0,
        }
    } else {
        get_f32(&t, "m_MaxSpawnedPawnsPerRing").map(|v| v as i32).unwrap_or(0)
    };
    let spawners = obj_list(pkg, p, "m_TriggeredSpawners").into_iter().map(|o| pkg.obj_name(ObjRef::from_index(o)).to_string()).filter(|s| !s.is_empty() && s != "None").collect();
    Some(AlarmBellDef {
        name: pkg.obj_name(ObjRef::Export(i)).to_string(),
        package: package.to_string(),
        position: Vec3::from(find(p, "Location").and_then(|x| x.as_vector()).unwrap_or([0.0; 3])),
        loudness: loud.unwrap_or(0.0),
        go_closeness: f("m_fNPCGoToAlarmCloseness", 0.0),
        use_closeness: f("m_fNPCUseCloseness", 0.0),
        time_between_uses: f("m_fTimeInBetweenUses", 0.0),
        spawner_min_dist: f("m_fSpawnerMinDistFromPlayer", 0.0),
        max_spawns,
        ring_delay: f("m_fAlarmRingDelayBeforeAINoise", 0.0),
        spawners,
    })
}
