//! The original-shader programs (`ue3mat`) composed off the render thread, in parallel, and
//! cached on disk.
//!
//! Bevy composes a pipeline's shader (naga_oil: `#import`s, shader defs, parse) inside
//! `ShaderCache::get`, under the pipeline cache's single shader cache lock, so a level's hundreds
//! of large programs were composed one after another. Here every (program, shader defs) variant
//! is composed on a pool of worker threads, each with its own naga_oil `Composer` holding Bevy's
//! import modules (the same sources and defs Bevy's own composer gets, so the module is the one
//! Bevy would have built), and handed to the vendored bevy_render as naga IR
//! (`BlinkShaderModules`, [blink patch]): the pipeline task makes the wgpu module itself, outside
//! the shader cache lock, so Bevy does no composition for these shaders.
//!
//! Variants. A program's pipelines differ only by the shader defs Bevy (and `Ue3Material::
//! specialize`) put on the descriptor, which are a function of the view (MSAA, HDR, tonemapping,
//! prepass textures, projection, shadow filtering...), the mesh layout and the material key
//! (alpha mode, foreground): main pass (`MeshPipeline`: MESH_PIPELINE, VERTEX_*, MULTISAMPLED,
//! DEPTH_PREPASS, VIEW_PROJECTION_*, TONEMAP_*, SHADOW_FILTER_METHOD_*, BLEND_*, MAY_DISCARD,
//! MATERIAL_BIND_GROUP...), depth prepass and shadow views (`PrepassPipeline`: PREPASS_PIPELINE,
//! DEPTH_PREPASS, MAY_DISCARD, VERTEX_*, UNCLIPPED_DEPTH_ORTHO_EMULATION...), plus UE_FOREGROUND /
//! UE_FOREGROUND_WORLD. `specialize` hands its final def list here (`use_precomposed`: Bevy adds
//! nothing after it, the material is not bindless), so the variant is exact by construction; the
//! global def (AVAILABLE_STORAGE_BUFFER_BINDINGS, from the device) is added as Bevy's extraction
//! does. Each def list is recorded per material class (`Class`) in `<cache>/shaders/
//! variants.json`; a level's worker (`ue3_level::prepare`) then starts every program it built
//! times its class's recorded def lists before the level spawns (`prefetch`). Def lists seen for
//! the first time are composed when `specialize` asks for them (the pipeline waits for the
//! module: `ShaderNotLoaded` until it is registered).
//!
//! Disk cache: `<cache>/shaders/<key>.bin`, the key a hash of the program's WGSL, the defs, the
//! backend, the import sources and the crate / Bevy / naga versions. The payload is SPIR-V (naga
//! spv-out; checked by reading it back and validating when written, else WGSL is stored for that
//! variant); a later run parses it on the pool (spv-in) and registers the IR, with no
//! composition. `DISHONORED_SHADER_FORMAT=wgsl` stores WGSL instead.
//!
//! Switches: `DISHONORED_NO_SHADER_CACHE=1` composes every run (no disk cache);
//! `DISHONORED_NO_SHADER_CACHE=all` leaves composition to Bevy (the old path). A variant that
//! fails to compose here also falls back to Bevy (its WGSL with the same defs).

use bevy::asset::uuid::Uuid;
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::render::render_resource::{BlinkShaderModules, PipelineCache, RenderPipelineDescriptor};
use bevy::render::renderer::{RenderAdapterInfo, RenderDevice};
use bevy::render::{ExtractSchedule, RenderApp};
use bevy::shader::{ShaderDefVal, ShaderImport};
use naga_oil::compose::{Composer, NagaModuleDescriptor, ShaderDefValue, ShaderType};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock};
use std::time::Instant;

/// Part of every cache key: a change to the format or to the composing toolchain invalidates it.
const CACHE_VERSION: &str = concat!("blink-shader-cache 1 / ", env!("CARGO_PKG_VERSION"), " / bevy 0.19.1 / naga 29.0.4 / naga_oil 0.22.0");
const MAGIC: &[u8; 4] = b"BLSH";
const FORMAT_SPIRV: u8 = 1;
const FORMAT_WGSL: u8 = 2;
/// Low half of the variants' asset uuids (the high half is the variant key).
const UUID_SALT: u64 = 0xb11c_5ade_c0de_0001;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Bevy composes (the old path)
    Off,
    /// composed here, every run
    NoDisk,
    /// composed here, cached on disk
    Disk,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("DISHONORED_NO_SHADER_CACHE").as_deref() {
        Err(_) | Ok("") | Ok("0") => Mode::Disk,
        Ok("all") => Mode::Off,
        Ok(_) => Mode::NoDisk,
    })
}

/// Whether programs are composed here (else Bevy composes them).
pub fn enabled() -> bool {
    mode() != Mode::Off
}

fn preferred_format() -> u8 {
    match std::env::var("DISHONORED_SHADER_FORMAT").as_deref() {
        Ok("wgsl") => FORMAT_WGSL,
        _ => FORMAT_SPIRV,
    }
}

fn cache_dir() -> PathBuf {
    dishonored_content::cache_dir().join("shaders")
}

/// FNV-1a, 64 bits: stable across runs and toolchains (the disk keys).
pub fn stable_hash(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for &b in *p {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        // part separator
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A program's WGSL (with Bevy `#import`s and `#ifdef`s) and its stable hash.
#[derive(Clone)]
pub struct ProgramSource {
    pub wgsl: Arc<str>,
    pub hash: u64,
}

impl ProgramSource {
    pub fn new(wgsl: Arc<str>) -> Self {
        let hash = stable_hash(&[wgsl.as_bytes()]);
        ProgramSource { wgsl, hash }
    }
}

/// What a pipeline of a `Ue3Material` draws, as far as its shader defs depend on the material:
/// the shader (the depth-only program, else the colour program), the pass (prepass / shadow
/// views, else the main pass), the alpha mode (`Ue3Key::blend`) and the foreground flags. The def
/// lists of each class are recorded (`variants.json`); the view-dependent defs make one class
/// have one def list per kind of view drawing it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct Class {
    pub depth: bool,
    pub prepass: bool,
    pub blend: u8,
    pub fg: bool,
    pub fg_world: bool,
}

impl Class {
    pub fn main(blend: u8) -> Self {
        Class { depth: false, prepass: false, blend, fg: false, fg_world: false }
    }
}

// ---------------------------------------------------------------------------------------------
// Bevy's import modules and global defs, from the main world

/// The modules our programs import (transitively), as the render world's shader cache has them.
struct Env {
    generation: u64,
    /// import path -> shader, with the global defs appended as Bevy's extraction does
    imports: HashMap<ShaderImport, Shader>,
    global_defs: Vec<ShaderDefVal>,
    /// the import closure's sources (part of the keys)
    imports_hash: u64,
    backend: String,
}

static ENV: RwLock<Option<Arc<Env>>> = RwLock::new(None);

fn env() -> Option<Arc<Env>> {
    ENV.read().unwrap().clone()
}

/// Snapshot Bevy's import modules (the closure of `ue3prog::PLATFORM_BEVY`'s imports) whenever
/// a module shader is added or changed.
fn track_imports(
    shaders: Res<Assets<Shader>>,
    mut events: MessageReader<AssetEvent<Shader>>,
    device: Option<Res<RenderDevice>>,
    adapter: Option<Res<RenderAdapterInfo>>,
    mut dirty: Local<bool>,
    mut tried: Local<bool>,
    mut generation: Local<u64>,
) {
    if mode() == Mode::Off {
        return;
    }
    for e in events.read() {
        if let AssetEvent::Added { id } | AssetEvent::Modified { id } = e {
            if shaders.get(*id).is_some_and(|s| matches!(s.import_path, ShaderImport::Custom(_))) {
                *dirty = true;
            }
        }
    }
    if *tried && !*dirty {
        return;
    }
    let (Some(device), Some(adapter)) = (device, adapter) else { return };
    *tried = true;
    *dirty = false;
    // PipelineCache::new's global defs (the webgl / simulator ones do not apply here)
    let global_defs = vec![ShaderDefVal::UInt("AVAILABLE_STORAGE_BUFFER_BINDINGS".into(), device.limits().max_storage_buffers_per_shader_stage)];
    let by_path: HashMap<&ShaderImport, &Shader> = shaders.iter().map(|(_, s)| (&s.import_path, s)).collect();
    let roots = Shader::from_wgsl(ue3_shader::ue3prog::PLATFORM_BEVY, "ue3/platform.wgsl").imports;
    let mut imports = HashMap::new();
    let mut stack: Vec<ShaderImport> = roots;
    let mut missing = Vec::new();
    while let Some(i) = stack.pop() {
        if imports.contains_key(&i) {
            continue;
        }
        let Some(s) = by_path.get(&i) else {
            missing.push(i.module_name().into_owned());
            continue;
        };
        let mut s = (*s).clone();
        s.shader_defs.extend(global_defs.iter().cloned());
        stack.extend(s.imports.iter().cloned());
        imports.insert(i, s);
    }
    if !missing.is_empty() {
        // (startup: Bevy's embedded modules may still be loading)
        debug!("shader cache: import modules not loaded yet: {missing:?}");
        *ENV.write().unwrap() = None;
        return;
    }
    // Imports under `#ifdef`s (e.g. bevy_pbr::skinning behind SKINNED) are not in a module's
    // listed imports, so the closure above misses them: every named module is offered too.
    for (path, s) in &by_path {
        if matches!(path, ShaderImport::Custom(_)) && !imports.contains_key(*path) {
            let mut s = (*s).clone();
            s.shader_defs.extend(global_defs.iter().cloned());
            imports.insert((*path).clone(), s);
        }
    }
    let mut names: Vec<(String, &Shader)> = imports.iter().map(|(i, s)| (i.module_name().into_owned(), s)).collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    let mut parts: Vec<&[u8]> = Vec::new();
    for (n, s) in &names {
        parts.push(n.as_bytes());
        parts.push(s.source.as_str().as_bytes());
    }
    let imports_hash = stable_hash(&parts);
    let backend = format!("{:?}", adapter.backend);
    if let Some(e) = env() {
        if e.imports_hash == imports_hash && e.backend == backend && e.global_defs == global_defs {
            return;
        }
    }
    *generation += 1;
    debug!("shader cache: {} import modules ({backend})", imports.len());
    *ENV.write().unwrap() = Some(Arc::new(Env { generation: *generation, imports, global_defs, imports_hash, backend }));
}

// ---------------------------------------------------------------------------------------------
// The render world's module registry

static MODULES: OnceLock<BlinkShaderModules> = OnceLock::new();
/// modules finished before the registry was reachable
static PENDING: Mutex<Vec<(Uuid, naga::Module)>> = Mutex::new(Vec::new());

fn hook_render(cache: Option<Res<PipelineCache>>) {
    if MODULES.get().is_some() {
        return;
    }
    let Some(cache) = cache else { return };
    let _ = MODULES.set(cache.blink_shader_modules());
    let mut pending = PENDING.lock().unwrap();
    for (uuid, m) in pending.drain(..) {
        MODULES.get().unwrap().insert(AssetId::Uuid { uuid }, m);
    }
}

fn deliver(uuid: Uuid, module: naga::Module) {
    let mut pending = PENDING.lock().unwrap();
    match MODULES.get() {
        Some(m) => m.insert(AssetId::Uuid { uuid }, module),
        None => pending.push((uuid, module)),
    }
}

/// Variants that did not compose here: Bevy composes them (their WGSL with the same defs, at the
/// variant's id), inserted from the main world.
static FALLBACKS: Mutex<Vec<(Uuid, Shader)>> = Mutex::new(Vec::new());
static HAS_FALLBACKS: AtomicBool = AtomicBool::new(false);

fn insert_fallbacks(mut shaders: ResMut<Assets<Shader>>) {
    if !HAS_FALLBACKS.swap(false, Ordering::AcqRel) {
        return;
    }
    let v = std::mem::take(&mut *FALLBACKS.lock().unwrap());
    for (uuid, s) in v {
        let _ = shaders.insert(AssetId::Uuid { uuid }, s);
    }
}

pub struct Ue3ShaderCachePlugin;

impl Plugin for Ue3ShaderCachePlugin {
    fn build(&self, app: &mut App) {
        if mode() == Mode::Off {
            return;
        }
        app.add_systems(First, (track_imports, insert_fallbacks));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(ExtractSchedule, hook_render);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Variants

/// The merged defs (pipeline defs, then the global ones; later wins, as in Bevy's composition)
/// as a canonical string.
fn canonical_defs(defs: &[ShaderDefVal], global: &[ShaderDefVal]) -> String {
    let mut m: BTreeMap<&str, String> = BTreeMap::new();
    for d in defs.iter().chain(global) {
        let (k, v) = match d {
            ShaderDefVal::Bool(k, v) => (k, format!("b{v}")),
            ShaderDefVal::Int(k, v) => (k, format!("i{v}")),
            ShaderDefVal::UInt(k, v) => (k, format!("u{v}")),
        };
        m.insert(k.as_str(), v);
    }
    let mut s = String::new();
    for (k, v) in m {
        s.push_str(k);
        s.push('=');
        s.push_str(&v);
        s.push(';');
    }
    s
}

/// (variant key, hash of the defs alone)
fn variant_key(env: &Env, program: u64, defs: &[ShaderDefVal]) -> (u64, u64) {
    let canon = canonical_defs(defs, &env.global_defs);
    let defs_hash = stable_hash(&[canon.as_bytes()]);
    let key = stable_hash(&[
        CACHE_VERSION.as_bytes(),
        env.backend.as_bytes(),
        &env.imports_hash.to_le_bytes(),
        &program.to_le_bytes(),
        canon.as_bytes(),
    ]);
    (key, defs_hash)
}

/// Variant key -> its asset uuid (registered, or composing).
static VARIANTS: Mutex<Option<HashMap<u64, Uuid>>> = Mutex::new(None);

/// The id of a variant's module, starting its job the first time it is asked for.
fn ensure(env: &Arc<Env>, program: &ProgramSource, defs: &[ShaderDefVal]) -> Uuid {
    let (key, defs_hash) = variant_key(env, program.hash, defs);
    let uuid = Uuid::from_u64_pair(key, UUID_SALT);
    let fresh = VARIANTS.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, uuid).is_none();
    if fresh {
        submit(Job { key, uuid, program: program.clone(), defs_hash, defs: defs.to_vec(), env: env.clone() });
    }
    uuid
}

/// `Ue3Material::specialize`'s last step: the pipeline's shader (the program, with the defs on
/// the descriptor) becomes the variant's module composed here, with no defs left for Bevy.
pub fn use_precomposed(program: &ProgramSource, class: Class, d: &mut RenderPipelineDescriptor) {
    if mode() == Mode::Off || MODULES.get().is_none() {
        return;
    }
    let Some(env) = env() else { return };
    let shader = d.vertex.shader.id();
    let vdefs = d.vertex.shader_defs.clone();
    let fdefs = d.fragment.as_ref().filter(|f| f.shader.id() == shader).map(|f| f.shader_defs.clone());
    record(class, &vdefs);
    let vu = ensure(&env, program, &vdefs);
    let fu = fdefs.map(|fd| {
        if fd == vdefs {
            vu
        } else {
            record(class, &fd);
            ensure(&env, program, &fd)
        }
    });
    d.vertex.shader = Handle::Uuid(vu, std::marker::PhantomData);
    d.vertex.shader_defs.clear();
    if let (Some(f), Some(fu)) = (d.fragment.as_mut(), fu) {
        f.shader = Handle::Uuid(fu, std::marker::PhantomData);
        f.shader_defs.clear();
    }
}

/// Start the variants a level's programs will need (each program times its class's recorded
/// def lists), from the level's worker before the spawn. Returns without waiting.
pub fn prefetch(items: Vec<(ProgramSource, Class)>) {
    if mode() == Mode::Off {
        return;
    }
    let Some(env) = env() else { return };
    let classes = manifest_defs();
    for (p, class) in items {
        for defs in classes.get(&class).into_iter().flatten() {
            ensure(&env, &p, defs);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The recorded def lists (variants.json)

#[derive(Serialize, Deserialize, Clone)]
struct ManifestEntry {
    class: Class,
    defs: Vec<ShaderDefVal>,
}

struct Manifest {
    entries: Vec<ManifestEntry>,
    seen: HashSet<(Class, Vec<ShaderDefVal>)>,
}

static MANIFEST: Mutex<Option<Manifest>> = Mutex::new(None);

fn manifest_path() -> PathBuf {
    cache_dir().join("variants.json")
}

fn with_manifest<R>(f: impl FnOnce(&mut Manifest) -> R) -> R {
    let mut g = MANIFEST.lock().unwrap();
    let m = g.get_or_insert_with(|| {
        let entries: Vec<ManifestEntry> = if mode() == Mode::Disk {
            std::fs::read(manifest_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let seen = entries.iter().map(|e| (e.class, e.defs.clone())).collect();
        Manifest { entries, seen }
    });
    f(m)
}

fn record(class: Class, defs: &[ShaderDefVal]) {
    let save = with_manifest(|m| {
        if !m.seen.insert((class, defs.to_vec())) {
            return None;
        }
        m.entries.push(ManifestEntry { class, defs: defs.to_vec() });
        info!("shader variants: new def list for {class:?} ({} defs; {} recorded)", defs.len(), m.entries.len());
        (mode() == Mode::Disk).then(|| serde_json::to_vec_pretty(&m.entries).ok()).flatten()
    });
    if let Some(bytes) = save {
        let _ = write_atomic(&manifest_path(), &bytes);
    }
}

fn manifest_defs() -> HashMap<Class, Vec<Vec<ShaderDefVal>>> {
    with_manifest(|m| {
        let mut r: HashMap<Class, Vec<Vec<ShaderDefVal>>> = HashMap::new();
        for e in &m.entries {
            r.entry(e.class).or_default().push(e.defs.clone());
        }
        r
    })
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("tmp{:?}", std::thread::current().id()).replace(['(', ')'], ""));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

// ---------------------------------------------------------------------------------------------
// The worker pool

struct Job {
    key: u64,
    uuid: Uuid,
    program: ProgramSource,
    defs_hash: u64,
    defs: Vec<ShaderDefVal>,
    env: Arc<Env>,
}

struct Batch {
    start: Instant,
    programs: HashSet<u64>,
    variants: HashSet<u64>,
    modules: usize,
    disk: usize,
    composed: usize,
    failed: usize,
}

struct PoolState {
    jobs: VecDeque<Job>,
    active: usize,
    batch: Option<Batch>,
}

struct Pool {
    state: Mutex<PoolState>,
    ready: Condvar,
    threads: usize,
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<&'static Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        // (one core left to the main and render threads)
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).saturating_sub(1).max(2);
        let pool: &'static Pool = Box::leak(Box::new(Pool { state: Mutex::new(PoolState { jobs: VecDeque::new(), active: 0, batch: None }), ready: Condvar::new(), threads }));
        for i in 0..threads {
            let _ = std::thread::Builder::new().name(format!("shader compose {i}")).spawn(move || worker(pool));
        }
        pool
    })
}

fn submit(job: Job) {
    let pool = pool();
    let mut s = pool.state.lock().unwrap();
    let b = s.batch.get_or_insert_with(|| Batch { start: Instant::now(), programs: HashSet::new(), variants: HashSet::new(), modules: 0, disk: 0, composed: 0, failed: 0 });
    b.programs.insert(job.program.hash);
    b.variants.insert(job.defs_hash);
    b.modules += 1;
    s.jobs.push_back(job);
    pool.ready.notify_one();
}

enum Outcome {
    Disk,
    Composed,
    Failed,
}

fn worker(pool: &'static Pool) {
    loop {
        let job = {
            let mut s = pool.state.lock().unwrap();
            loop {
                if let Some(j) = s.jobs.pop_front() {
                    s.active += 1;
                    break j;
                }
                let (g, timeout) = pool.ready.wait_timeout(s, std::time::Duration::from_secs(2)).unwrap();
                s = g;
                // idle: free this thread's composer (its import modules per def set)
                if timeout.timed_out() && s.jobs.is_empty() && s.active == 0 {
                    COMPOSER.with_borrow_mut(|c| *c = None);
                }
            }
        };
        let outcome = run(&job);
        let mut s = pool.state.lock().unwrap();
        s.active -= 1;
        if let Some(b) = s.batch.as_mut() {
            match outcome {
                Outcome::Disk => b.disk += 1,
                Outcome::Composed => b.composed += 1,
                Outcome::Failed => b.failed += 1,
            }
        }
        if s.jobs.is_empty() && s.active == 0 {
            if let Some(b) = s.batch.take() {
                let fallback = if b.failed > 0 { format!(", {} left to Bevy", b.failed) } else { String::new() };
                info!(
                    "shaders: {} programs x {} variants ({} modules), {} from disk cache, {} composed in {:.2} s on {} threads{fallback}",
                    b.programs.len(),
                    b.variants.len(),
                    b.modules,
                    b.disk,
                    b.composed,
                    b.start.elapsed().as_secs_f32(),
                    pool.threads
                );
            }
        }
    }
}

fn run(job: &Job) -> Outcome {
    let path = cache_dir().join(format!("{:016x}.bin", job.key));
    if mode() == Mode::Disk {
        if let Some(module) = std::fs::read(&path).ok().and_then(|b| decode_file(&b)) {
            deliver(job.uuid, module);
            return Outcome::Disk;
        }
    }
    let name = format!("ue3/{:016x}.wgsl", job.program.hash);
    // (validated only to be written out: wgpu validates the module it is given anyway)
    let composed = compose(&job.env, &job.program.wgsl, &name, &job.defs).and_then(|m| {
        let info = if mode() == Mode::Disk { Some(validate(&m)?) } else { None };
        Ok((m, info))
    });
    match composed {
        Ok((module, info)) => {
            if let Some(info) = info {
                match encode(&module, &info, preferred_format()) {
                    Ok((format, payload)) => {
                        let _ = write_atomic(&path, &encode_file(format, &payload));
                    }
                    Err(e) => warn!("shader cache: {name}: not stored: {e}"),
                }
            }
            deliver(job.uuid, module);
            Outcome::Composed
        }
        Err(e) => {
            warn!("shader cache: {name}: composing here failed, Bevy composes it: {e}");
            FALLBACKS.lock().unwrap().push((job.uuid, Shader::from_wgsl_with_defs(job.program.wgsl.to_string(), name, job.defs.clone())));
            HAS_FALLBACKS.store(true, Ordering::Release);
            Outcome::Failed
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Composition (Bevy's `ShaderCache::get`, per thread)

thread_local! {
    static COMPOSER: RefCell<Option<(u64, Composer)>> = const { RefCell::new(None) };
}

fn add_import(composer: &mut Composer, env: &Env, import: &ShaderImport) -> Result<(), String> {
    if composer.contains_module(&import.module_name()) {
        return Ok(());
    }
    let s = env.imports.get(import).ok_or_else(|| format!("import {} not loaded", import.module_name()))?;
    for i in &s.imports {
        add_import(composer, env, i)?;
    }
    if let Err(e) = composer.add_composable_module(s.into()).map(|_| ()) {
        return Err(e.emit_to_string(composer));
    }
    Ok(())
}

fn to_naga_oil(d: &ShaderDefVal) -> (String, ShaderDefValue) {
    match d.clone() {
        ShaderDefVal::Bool(k, v) => (k, ShaderDefValue::Bool(v)),
        ShaderDefVal::Int(k, v) => (k, ShaderDefValue::Int(v)),
        ShaderDefVal::UInt(k, v) => (k, ShaderDefValue::UInt(v)),
    }
}

fn compose(env: &Env, wgsl: &str, path: &str, defs: &[ShaderDefVal]) -> Result<naga::Module, String> {
    let imports = Shader::from_wgsl(wgsl.to_owned(), path.to_owned()).imports;
    COMPOSER.with_borrow_mut(|slot| {
        if slot.as_ref().map(|(g, _)| *g) != Some(env.generation) {
            // Bevy's release composer (no validation of its own; validated below)
            *slot = Some((env.generation, Composer::non_validating().with_capabilities(naga::valid::Capabilities::all())));
        }
        let composer = &mut slot.as_mut().unwrap().1;
        for i in &imports {
            add_import(composer, env, i)?;
        }
        // the pipeline's defs then the shader's own (the global ones), as Bevy merges them
        let shader_defs: std::collections::HashMap<String, ShaderDefValue> = defs.iter().chain(env.global_defs.iter()).map(to_naga_oil).collect();
        composer
            .make_naga_module(NagaModuleDescriptor { source: wgsl, file_path: path, shader_type: ShaderType::Wgsl, shader_defs, additional_imports: &[] })
            .map_err(|e| e.emit_to_string(composer))
    })
}

fn validate(m: &naga::Module) -> Result<naga::valid::ModuleInfo, String> {
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(m).map_err(|e| format!("validation: {}", e.as_inner()))
}

// ---------------------------------------------------------------------------------------------
// Disk format: magic, format, payload length and hash, payload

fn spv_options() -> naga::back::spv::Options<'static> {
    let mut o = naga::back::spv::Options::default();
    // the IR's own conventions: no Y flip (read back with `adjust_coordinate_space: false`), and
    // no checks baked in (wgpu applies its own when it writes the backend's code)
    o.flags = naga::back::spv::WriterFlags::empty();
    o.force_loop_bounding = false;
    o.ray_query_initialization_tracking = false;
    o
}

fn spv_read(bytes: &[u8]) -> Result<naga::Module, String> {
    let o = naga::front::spv::Options { adjust_coordinate_space: false, strict_capabilities: false, block_ctx_dump_prefix: None };
    naga::front::spv::parse_u8_slice(bytes, &o).map_err(|e| format!("spv-in: {e}"))
}

fn wgsl_read(bytes: &[u8]) -> Result<naga::Module, String> {
    let s = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
    naga::front::wgsl::parse_str(s).map_err(|e| format!("wgsl-in: {e}"))
}

/// The payload of a composed module: SPIR-V (read back and validated here, so a later run can
/// trust it), else WGSL.
fn encode(m: &naga::Module, info: &naga::valid::ModuleInfo, format: u8) -> Result<(u8, Vec<u8>), String> {
    if format == FORMAT_SPIRV {
        let spv = naga::back::spv::write_vec(m, info, &spv_options(), None).map_err(|e| format!("spv-out: {e}"));
        let bytes = spv.map(|w| w.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>());
        match bytes.and_then(|b| spv_read(&b).and_then(|r| validate(&r)).map(|_| b)) {
            Ok(b) => return Ok((FORMAT_SPIRV, b)),
            Err(e) => debug!("shader cache: SPIR-V round trip failed ({e}); stored as WGSL"),
        }
    }
    let s = naga::back::wgsl::write_string(m, info, naga::back::wgsl::WriterFlags::empty()).map_err(|e| format!("wgsl-out: {e}"))?;
    validate(&wgsl_read(s.as_bytes())?)?;
    Ok((FORMAT_WGSL, s.into_bytes()))
}

fn encode_file(format: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 24);
    v.extend_from_slice(MAGIC);
    v.extend_from_slice(&[format, 0, 0, 0]);
    v.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    v.extend_from_slice(&stable_hash(&[payload]).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

fn decode_file(b: &[u8]) -> Option<naga::Module> {
    if b.len() < 24 || &b[0..4] != MAGIC {
        return None;
    }
    let format = b[4];
    let len = u64::from_le_bytes(b[8..16].try_into().ok()?) as usize;
    let hash = u64::from_le_bytes(b[16..24].try_into().ok()?);
    let payload = b.get(24..24 + len)?;
    if stable_hash(&[payload]) != hash {
        return None;
    }
    let r = match format {
        FORMAT_SPIRV => spv_read(payload),
        FORMAT_WGSL => wgsl_read(payload),
        _ => return None,
    };
    r.map_err(|e| warn!("shader cache: unreadable entry ({e}); composing again")).ok()
}
