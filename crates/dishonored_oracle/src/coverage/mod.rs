//! Coverage scan: what the original's packages use, against what the port handles
//! ([`source::PortRegistry`]), per persistent map and over every package.

pub mod scan;
pub mod source;
pub mod ufunc;

use crate::report::WorkItem;
use rayon::prelude::*;
use scan::{FunctionDef, PackageFile, PackageScan};
use serde::{Deserialize, Serialize};
use source::{Handling, KISMET_SCOPE, PORT_SCOPE, PortRegistry};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// Properties with no effect in a cooked game, left out of the property coverage (listed so
/// the omission is explicit; everything else counts, including values the port derives another
/// way, such as texture sizes read from the mips):
/// - Kismet editor layout and bookkeeping (`ObjPosX`, `DrawWidth`, comment boxes);
/// - editor colours, previews, snapping, selection and collapse state;
/// - lighting-build and cook metadata (GUIDs, Lightmass settings, "built" flags, versions);
/// - the dialog editor's bookkeeping (`m_BlurbVersionNum`, `m_bDirtiedWithoutExport`).
pub const EDITOR_ONLY_PROPS: &[&str] = &[
    // Kismet editor
    "ObjPosX", "ObjPosY", "DrawWidth", "DrawHeight", "ObjColor", "ObjComment", "bDrawFirst", "bDrawLast", "bOutputObjCommentToScreen",
    "ObjInstanceVersion", "ParentSequence", "OverrideDelta", "PinDrawWidth", "MaxWidth", "bSuppressAutoComment", "DrawConnector",
    "LinkDesc", "VarName", "FillColor", "BorderColor", "BorderWidth", "bDrawBox", "bFilled", "bTileFill", "OldPosition",
    // editor colours, previews, selection
    "ModuleEditorColor", "EmitterEditorColor", "GroupColor", "PreviewEnvironmentShadowing", "bCollapsed", "SavedSelections", "bEdShouldSnap",
    // lighting build and cook metadata
    "ExpressionGUID", "LightingGuid", "ParentLightingGuid", "ReferencedTextureGuids", "LightmapGuid", "LightmassSettings",
    "bHasLightEverBeenBuiltIntoLightMap", "OriginalSizeX", "OriginalSizeY", "m_iMatineeDataVersion", "m_VersionNum", "m_BaseClassVersions",
    // dialog editor
    "m_BlurbVersionNum", "m_bDirtiedWithoutExport",
];

/// Class names of the class hierarchy, from the scanned script packages.
#[derive(Debug, Default)]
pub struct Hierarchy {
    supers: HashMap<String, (String, String)>,
}

impl Hierarchy {
    pub fn from_scans<'a>(scans: impl IntoIterator<Item = &'a PackageScan>) -> Self {
        let mut supers = HashMap::new();
        for s in scans {
            for (c, sup) in &s.class_defs {
                supers.entry(c.to_ascii_lowercase()).or_insert_with(|| (c.clone(), sup.clone()));
            }
        }
        Self { supers }
    }
    /// Super chain, nearest first (without the class itself).
    pub fn ancestors(&self, class: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = class.to_ascii_lowercase();
        for _ in 0..64 {
            let Some((_, sup)) = self.supers.get(&cur) else { break };
            if sup.is_empty() {
                break;
            }
            out.push(sup.clone());
            cur = sup.to_ascii_lowercase();
        }
        out
    }
    pub fn is_a(&self, class: &str, base: &str) -> bool {
        class.eq_ignore_ascii_case(base) || self.ancestors(class).iter().any(|a| a.eq_ignore_ascii_case(base))
    }
    pub fn known(&self, class: &str) -> bool {
        self.supers.contains_key(&class.to_ascii_lowercase())
    }
    pub fn len(&self) -> usize {
        self.supers.len()
    }
    pub fn is_empty(&self) -> bool {
        self.supers.is_empty()
    }
}

/// `handled` of `total`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rate {
    pub total: u64,
    pub handled: u64,
}

impl Rate {
    pub fn add(&mut self, n: u64, ok: bool) {
        self.total += n;
        if ok {
            self.handled += n;
        }
    }
    pub fn frac(&self) -> f64 {
        if self.total == 0 { 1.0 } else { self.handled as f64 / self.total as f64 }
    }
    pub fn pct(&self) -> String {
        if self.total == 0 { "-".into() } else { format!("{:.1} %", 100.0 * self.frac()) }
    }
    pub fn cell(&self) -> String {
        format!("{}/{} ({})", self.handled, self.total, self.pct())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FunctionStats {
    /// Classes in use (with their ancestors) that declare functions.
    pub classes: u32,
    pub native: u32,
    pub script: u32,
    /// Script functions with bytecode (not native, size > 0) and their bytes.
    pub script_with_code: u32,
    pub script_bytes: u64,
    /// Functions whose name the port uses as a literal anywhere (informational: the port does
    /// not run UnrealScript, so this is not "implemented").
    pub named_in_port: u32,
}

/// Coverage of one persistent map (its persistent package plus streaming levels).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MapCoverage {
    pub map: String,
    pub dlc: Option<String>,
    pub packages: Vec<String>,
    pub missing_packages: Vec<String>,
    /// Placed actors: instances and distinct classes.
    pub actors: Rate,
    pub actor_classes: Rate,
    /// Every object (actors, components, Kismet, materials, archetypes...).
    pub objects: Rate,
    pub object_classes: Rate,
    /// Kismet ops (events, conditions, actions) and variables, implemented by the interpreter
    /// or the game's Kismet host.
    pub kismet_ops: Rate,
    pub kismet_op_classes: Rate,
    pub kismet_vars: Rate,
    /// Property settings on objects of handled classes that the port reads (weighted by
    /// objects) and distinct (class, property) pairs.
    pub props: Rate,
    pub prop_pairs: Rate,
    pub mat_exprs: Rate,
    pub mat_expr_classes: Rate,
    /// Material blend modes / lighting models (`Material.BlendMode=BLEND_Masked`): uses, and
    /// whether the port names the value.
    pub material_features: Vec<(String, u32, bool)>,
    pub functions: FunctionStats,
    pub unhandled_actor_classes: Vec<(String, u32)>,
    pub unhandled_object_classes: Vec<(String, u32)>,
    pub unimplemented_ops: Vec<(String, u32)>,
    pub ignored_props: Vec<(String, String, u32)>,
    pub unhandled_mat_exprs: Vec<(String, u32)>,
    /// Mean of the actor-instance, Kismet-op and property-setting rates.
    pub overall: f64,
}

/// A whole coverage run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoverageRun {
    pub packages: usize,
    pub packages_from_cache: usize,
    pub parse_failures: u32,
    pub scan_seconds: f64,
    pub registry_files: usize,
    pub registry_lines: usize,
    pub classes_known: usize,
    pub maps: Vec<MapCoverage>,
    /// Over every scanned package (not only map packages).
    pub all: MapCoverage,
    /// Over every class the install defines: how much gameplay is UnrealScript.
    pub script_all: FunctionStats,
}

/// Scan packages in parallel (through the cache when given).
pub fn scan_all(files: &[PackageFile], cache: Option<&Path>) -> (Vec<PackageScan>, usize, Vec<String>) {
    let res: Vec<_> = files.par_iter().map(|f| scan::scan_cached(f, cache)).collect();
    let mut scans = Vec::with_capacity(res.len());
    let mut hits = 0;
    let mut errors = Vec::new();
    for r in res {
        match r {
            Ok((s, hit)) => {
                hits += hit as usize;
                scans.push(s);
            }
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    (scans, hits, errors)
}

/// Persistent maps among the scans (`*_P` packages with a `WorldInfo`, or that stream levels).
pub fn persistent_maps(scans: &[PackageScan]) -> Vec<&PackageScan> {
    let mut v: Vec<&PackageScan> = scans.iter().filter(|s| s.name.to_ascii_lowercase().ends_with("_p") && s.classes.contains_key("WorldInfo")).collect();
    v.sort_by(|a, b| (a.dlc.is_some(), &a.dlc, a.name.to_ascii_lowercase()).cmp(&(b.dlc.is_some(), &b.dlc, b.name.to_ascii_lowercase())));
    v
}

/// The packages of a persistent map: itself plus its streaming levels (the port's
/// `map_packages` rule); without streaming levels, every package sharing its prefix.
pub fn map_package_names(map: &PackageScan, by_name: &HashMap<String, &PackageScan>) -> (Vec<String>, Vec<String>) {
    let mut out = vec![map.name.clone()];
    let mut missing = Vec::new();
    if !map.streaming_levels.is_empty() {
        for l in &map.streaming_levels {
            if by_name.contains_key(&l.to_ascii_lowercase()) {
                if !out.iter().any(|o| o.eq_ignore_ascii_case(l)) {
                    out.push(l.clone());
                }
            } else {
                missing.push(l.clone());
            }
        }
    } else {
        let prefix = format!("{}_", map.name.strip_suffix("_P").unwrap_or(&map.name).to_ascii_lowercase());
        let mut rest: Vec<String> = by_name.values().filter(|s| s.name.to_ascii_lowercase().starts_with(&prefix) && !s.name.eq_ignore_ascii_case(&map.name)).map(|s| s.name.clone()).collect();
        rest.sort();
        out.extend(rest);
    }
    (out, missing)
}

struct Ctx<'a> {
    reg: &'a PortRegistry,
    hier: &'a Hierarchy,
    functions: HashMap<String, Vec<&'a FunctionDef>>,
    handled_cache: std::sync::Mutex<HashMap<(String, bool), Handling>>,
}

impl<'a> Ctx<'a> {
    fn handling(&self, class: &str, kismet: bool) -> Handling {
        let key = (class.to_ascii_lowercase(), kismet);
        if let Some(h) = self.handled_cache.lock().unwrap().get(&key) {
            return h.clone();
        }
        let mut h = self.reg.class_handling(class, &self.hier.ancestors(class), Some(if kismet { &KISMET_SCOPE } else { &PORT_SCOPE }));
        if kismet && !h.handled() && self.hier.is_a(class, "Sequence") {
            // Sequences (and prefab sequences) are classified by `is_a` in the loader
            // (`dishonored_kismet::load::kind_of` -> `Kind::Sequence`) and run by the
            // interpreter's sequence arm, with no class literal.
            h = Handling::Inherited { ancestor: "Sequence".into(), at: "crates/dishonored_kismet/src/load.rs (kind_of)".into() };
        }
        self.handled_cache.lock().unwrap().insert(key, h.clone());
        h
    }
    fn is_kismet(&self, class: &str) -> bool {
        self.hier.is_a(class, "SequenceObject") && !self.hier.is_a(class, "SequenceFrame")
    }
    fn is_kismet_var(&self, class: &str) -> bool {
        self.hier.is_a(class, "SequenceVariable")
    }
}

fn top(m: &BTreeMap<String, u32>, n: usize) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = m.iter().map(|(k, v)| (k.clone(), *v)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

fn coverage_of(name: &str, dlc: Option<String>, pkgs: &[&PackageScan], cx: &Ctx, list_limit: usize) -> MapCoverage {
    let mut actors: BTreeMap<String, u32> = BTreeMap::new();
    let mut objects: BTreeMap<String, u32> = BTreeMap::new();
    let mut props: BTreeMap<(String, String), u32> = BTreeMap::new();
    let mut enums: BTreeMap<String, u32> = BTreeMap::new();
    for p in pkgs {
        for (c, n) in &p.actors {
            *actors.entry(c.clone()).or_default() += n;
        }
        for (c, n) in &p.classes {
            *objects.entry(c.clone()).or_default() += n;
        }
        for (c, m) in &p.props {
            for (k, n) in m {
                *props.entry((c.clone(), k.clone())).or_default() += n;
            }
        }
        for (k, m) in &p.enums {
            if k == "Material.BlendMode" || k == "Material.LightingModel" {
                for (v, n) in m {
                    *enums.entry(format!("{k}={v}")).or_default() += n;
                }
            }
        }
    }
    let mut mc = MapCoverage { map: name.to_string(), dlc, packages: pkgs.iter().map(|p| p.name.clone()).collect(), ..Default::default() };
    let mut un_actor = BTreeMap::new();
    // `PersistentLevel` also owns non-actors (BSP `Model` / `Polys`, the level's Kismet root):
    // only `Actor` subclasses count as placed actors.
    actors.retain(|c, _| cx.hier.is_a(c, "Actor"));
    for (c, n) in &actors {
        let ok = cx.handling(c, false).handled();
        mc.actors.add(*n as u64, ok);
        mc.actor_classes.add(1, ok);
        if !ok {
            un_actor.insert(c.clone(), *n);
        }
    }
    let mut un_obj = BTreeMap::new();
    let mut un_ops = BTreeMap::new();
    let mut un_mat = BTreeMap::new();
    let mut handled_classes: BTreeSet<String> = BTreeSet::new();
    for (c, n) in &objects {
        let ok = cx.handling(c, false).handled();
        mc.objects.add(*n as u64, ok);
        mc.object_classes.add(1, ok);
        if ok {
            handled_classes.insert(c.clone());
        } else {
            un_obj.insert(c.clone(), *n);
        }
        if cx.is_kismet(c) {
            let kok = cx.handling(c, true).handled();
            if cx.is_kismet_var(c) {
                mc.kismet_vars.add(*n as u64, kok);
            } else {
                mc.kismet_ops.add(*n as u64, kok);
                mc.kismet_op_classes.add(1, kok);
                if !kok {
                    un_ops.insert(c.clone(), *n);
                }
            }
        }
        if c.starts_with("MaterialExpression") {
            mc.mat_exprs.add(*n as u64, ok);
            mc.mat_expr_classes.add(1, ok);
            if !ok {
                un_mat.insert(c.clone(), *n);
            }
        }
    }
    let mut ignored: Vec<(String, String, u32)> = Vec::new();
    for ((c, k), n) in &props {
        if !handled_classes.contains(c) || EDITOR_ONLY_PROPS.iter().any(|e| e.eq_ignore_ascii_case(k)) {
            continue;
        }
        let ok = cx.reg.names(k);
        mc.props.add(*n as u64, ok);
        mc.prop_pairs.add(1, ok);
        if !ok {
            ignored.push((c.clone(), k.clone(), *n));
        }
    }
    ignored.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    ignored.truncate(list_limit);
    mc.material_features = enums.iter().map(|(k, n)| (k.clone(), *n, cx.reg.names(k.rsplit('=').next().unwrap_or("")))).collect();
    // Functions declared by the classes in use and their ancestors.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for c in objects.keys() {
        seen.insert(c.to_ascii_lowercase());
        for a in cx.hier.ancestors(c) {
            seen.insert(a.to_ascii_lowercase());
        }
    }
    mc.functions = function_stats(seen.iter().filter_map(|c| cx.functions.get(c)), cx.reg);
    mc.unhandled_actor_classes = top(&un_actor, list_limit);
    mc.unhandled_object_classes = top(&un_obj, list_limit);
    mc.unimplemented_ops = top(&un_ops, list_limit);
    mc.ignored_props = ignored;
    mc.unhandled_mat_exprs = top(&un_mat, list_limit);
    mc.overall = (mc.actors.frac() + mc.kismet_ops.frac() + mc.props.frac()) / 3.0;
    mc
}

fn function_stats<'a>(groups: impl Iterator<Item = &'a Vec<&'a FunctionDef>>, reg: &PortRegistry) -> FunctionStats {
    let mut st = FunctionStats::default();
    let mut dedup: BTreeSet<(String, Option<String>, String)> = BTreeSet::new();
    for g in groups {
        if g.is_empty() {
            continue;
        }
        st.classes += 1;
        for f in g {
            if !dedup.insert((f.class.to_ascii_lowercase(), f.state.clone(), f.name.to_ascii_lowercase())) {
                continue;
            }
            if f.info().is_native() {
                st.native += 1;
            } else {
                st.script += 1;
                if f.script_size > 0 {
                    st.script_with_code += 1;
                    st.script_bytes += f.script_size as u64;
                }
            }
            if reg.names(&f.name) {
                st.named_in_port += 1;
            }
        }
    }
    st
}

/// How the port handles one class the install uses (the evidence behind every rate).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassHandling {
    pub class: String,
    pub objects: u64,
    pub handling: Handling,
    /// For Kismet classes: the handling counted for ops (interpreter and Kismet host only).
    pub kismet: Option<Handling>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoverageResult {
    pub maps: Vec<MapCoverage>,
    pub all: MapCoverage,
    pub script_all: FunctionStats,
    pub classes_known: usize,
    pub classes: Vec<ClassHandling>,
}

/// Coverage of every persistent map (and of all packages together).
pub fn coverage(scans: &[PackageScan], reg: &PortRegistry, list_limit: usize) -> CoverageResult {
    let hier = Hierarchy::from_scans(scans);
    let mut functions: HashMap<String, Vec<&FunctionDef>> = HashMap::new();
    for s in scans {
        for f in &s.functions {
            functions.entry(f.class.to_ascii_lowercase()).or_default().push(f);
        }
    }
    let cx = Ctx { reg, hier: &hier, functions, handled_cache: Default::default() };
    let by_name: HashMap<String, &PackageScan> = scans.iter().map(|s| (s.name.to_ascii_lowercase(), s)).collect();
    let maps = persistent_maps(scans);
    let out: Vec<MapCoverage> = maps
        .par_iter()
        .map(|m| {
            let (names, missing) = map_package_names(m, &by_name);
            let pk: Vec<&PackageScan> = names.iter().filter_map(|n| by_name.get(&n.to_ascii_lowercase()).copied()).collect();
            let mut c = coverage_of(&m.name, m.dlc.clone(), &pk, &cx, list_limit);
            c.missing_packages = missing;
            c
        })
        .collect();
    let all_refs: Vec<&PackageScan> = scans.iter().collect();
    let all = coverage_of("(all packages)", None, &all_refs, &cx, list_limit.max(200));
    let script_all = function_stats(cx.functions.values(), reg);
    let mut objects: BTreeMap<String, u64> = BTreeMap::new();
    for s in scans {
        for (c, n) in &s.classes {
            *objects.entry(c.clone()).or_default() += *n as u64;
        }
    }
    let classes = objects
        .into_iter()
        .map(|(class, n)| {
            let kismet = cx.is_kismet(&class).then(|| cx.handling(&class, true));
            ClassHandling { handling: cx.handling(&class, false), kismet, objects: n, class }
        })
        .collect();
    CoverageResult { maps: out, all, script_all, classes_known: hier.len(), classes }
}

/// Work items from a coverage run: unhandled actor classes, unimplemented Kismet ops, ignored
/// properties and unhandled material expressions, ranked by instance counts across maps.
pub fn work_items(maps: &[MapCoverage], examples: &HashMap<String, Vec<String>>, notes: &HashMap<String, String>) -> Vec<WorkItem> {
    struct Acc {
        count: u64,
        maps: BTreeMap<String, u32>,
    }
    let mut groups: BTreeMap<(&'static str, String), Acc> = BTreeMap::new();
    for m in maps {
        let mut add = |kind: &'static str, key: String, n: u32| {
            let a = groups.entry((kind, key)).or_insert(Acc { count: 0, maps: BTreeMap::new() });
            a.count += n as u64;
            *a.maps.entry(m.map.clone()).or_default() += n;
        };
        for (c, n) in &m.unhandled_actor_classes {
            add("actor-class", c.clone(), *n);
        }
        for (c, n) in &m.unimplemented_ops {
            add("kismet-op", c.clone(), *n);
        }
        for (c, p, n) in &m.ignored_props {
            add("property", format!("{c}.{p}"), *n);
        }
        for (c, n) in &m.unhandled_mat_exprs {
            add("material-expression", c.clone(), *n);
        }
    }
    let mut out = Vec::new();
    for ((kind, key), a) in groups {
        let (weight, system, code) = match kind {
            "actor-class" => (1.0, "level data", "crates/dishonored_assets/src/level.rs (import_actors), the extractor modules of dishonored_assets"),
            "kismet-op" => (2.0, "Kismet and mission flow", "crates/dishonored_multiplayer/src/kismet_host.rs (Host::activated) or crates/dishonored_kismet/src/runtime.rs (engine_activated)"),
            "property" => (0.2, "level data", "the importer of the class (dishonored_assets)"),
            _ => (0.5, "materials", "crates/dishonored_assets/src/material.rs, crates/dishonored_runtime shaders"),
        };
        let nmaps = a.maps.len();
        let class = key.split('.').next().unwrap_or(&key).to_string();
        let mut evidence = vec![format!("{} instances over {nmaps} maps", a.count)];
        let mut per: Vec<(&String, &u32)> = a.maps.iter().collect();
        per.sort_by(|x, y| y.1.cmp(x.1));
        evidence.push(format!("per map: {}", per.iter().take(12).map(|(m, n)| format!("{m} {n}")).collect::<Vec<_>>().join(", ")));
        if let Some(n) = notes.get(&class) {
            evidence.push(n.clone());
        }
        if let Some(ex) = examples.get(&class) {
            evidence.push(format!("example objects: {}", ex.join(", ")));
        }
        out.push(WorkItem {
            id: format!("coverage:{kind}:{key}"),
            title: match kind {
                "actor-class" => format!("Import placed `{key}` actors"),
                "kismet-op" => format!("Implement Kismet op `{key}`"),
                "property" => format!("Read `{key}`"),
                _ => format!("Translate material expression `{key}`"),
            },
            probe: "coverage".into(),
            system: system.into(),
            count: a.count,
            impact: weight * a.count as f64,
            maps: per.iter().map(|(m, _)| (*m).clone()).collect(),
            evidence,
            repro: vec![format!("oracle coverage --map {}", per.first().map(|x| x.0.as_str()).unwrap_or(""))],
            code: vec![code.into()],
        });
    }
    out.sort_by(|a, b| b.impact.total_cmp(&a.impact).then(a.id.cmp(&b.id)));
    out
}
