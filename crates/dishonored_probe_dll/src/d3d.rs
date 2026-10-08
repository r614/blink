//! D3D9 hooks. A throwaway `Direct3DCreate9` object gives d3d9's shared `IDirect3D9` vtable,
//! whose `CreateDevice` (slot 16) is patched; each device the game creates then gets its
//! vtable patched: Present (17), DrawPrimitive (81), DrawIndexedPrimitive (82),
//! CreatePixelShader (106), SetPixelShader (107), SetPixelShaderConstantF (109).
use crate::mem;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Condvar, Mutex};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

const D3D_CREATE_DEVICE: usize = 16;
const DEV_PRESENT: usize = 17;
const DEV_GET_BACK_BUFFER: usize = 18;
const DEV_CREATE_RENDER_TARGET: usize = 28;
const DEV_GET_RENDER_TARGET_DATA: usize = 32;
const DEV_STRETCH_RECT: usize = 34;
const DEV_CREATE_OFFSCREEN_PLAIN: usize = 36;
const DEV_DRAW_PRIMITIVE: usize = 81;
const DEV_DRAW_INDEXED_PRIMITIVE: usize = 82;
// UE3 draws its full-screen passes (post process, fog, LUT bakes) with the UP variants.
const DEV_DRAW_PRIMITIVE_UP: usize = 83;
const DEV_DRAW_INDEXED_PRIMITIVE_UP: usize = 84;
const DEV_CREATE_PIXEL_SHADER: usize = 106;
const DEV_SET_PIXEL_SHADER: usize = 107;
const DEV_SET_PS_CONST_F: usize = 109;
const DEV_CREATE_VERTEX_SHADER: usize = 91;

/// Bytecode of every distinct shader the game created (pixel and vertex), with the world tick
/// of its creation, for `SHADER_DUMP`.
pub struct StoredShader {
    pub id: u32,
    /// b'p' or b'v'.
    pub kind: u8,
    pub hash: u64,
    pub tick: u32,
    pub ctab: String,
    pub code: Vec<u8>,
}
pub static SHADER_CODE: Mutex<Vec<StoredShader>> = Mutex::new(Vec::new());

fn store_shader(kind: u8, code: &[u8]) {
    let hash = dishonored_probe::fnv64(code);
    if let Ok(mut v) = SHADER_CODE.lock()
        && !v.iter().any(|s| s.hash == hash && s.kind == kind) {
            let id = v.len() as u32;
            v.push(StoredShader { id, kind, hash, tick: crate::tick::GAME_TICK.load(Relaxed), ctab: ctab_names(code), code: code.to_vec() });
        }
}
const SURF_GET_DESC: usize = 12;
const SURF_LOCK_RECT: usize = 13;
const SURF_UNLOCK_RECT: usize = 14;
const UNK_RELEASE: usize = 2;

static ORIG: [AtomicU32; 128] = [const { AtomicU32::new(0) }; 128];
static ORIG_CREATE_DEVICE: AtomicU32 = AtomicU32::new(0);
pub static DEVICES: AtomicU32 = AtomicU32::new(0);
pub static PRESENTS: AtomicU32 = AtomicU32::new(0);
pub static STATE: AtomicU32 = AtomicU32::new(0);
/// Diagnostics: SetPixelShader calls, of them with a shader the hook saw created, and draws.
pub static PS_SETS: AtomicU32 = AtomicU32::new(0);
pub static PS_SETS_KNOWN: AtomicU32 = AtomicU32::new(0);
pub static DRAWS: AtomicU32 = AtomicU32::new(0);

pub struct Shader {
    pub id: u32,
    pub hash: u64,
    pub size: u32,
    pub ctab: String,
}

pub struct PsState {
    pub shaders: HashMap<u32, Shader>,
    pub current: u32,
    pub regs: Vec<[f32; 4]>,
    pub want_hash: u64,
    pub want_draws: u32,
    pub want_filter: String,
    pub got: Vec<(u32, u64, Vec<[f32; 4]>)>,
    /// Shader hashes whose draws are skipped (`DRAW_SKIP`).
    pub skip: Vec<u64>,
    /// Per-shader (draws, primitives) while `census` is on (`DRAW_CENSUS`).
    pub census_on: bool,
    pub census: HashMap<u64, (u32, u32)>,
}
pub static PS: Mutex<Option<PsState>> = Mutex::new(None);

pub struct CaptureReq {
    pub pending: bool,
    pub result: Option<Result<Vec<u8>, String>>,
}
pub static CAPTURE: Mutex<CaptureReq> = Mutex::new(CaptureReq { pending: false, result: None });
pub static CAPTURE_CV: Condvar = Condvar::new();

unsafe fn vfn(obj: u32, slot: usize) -> u32 {
    unsafe { *((*(obj as *const u32)) as *const u32).add(slot) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn install() {
    unsafe {
        let m = LoadLibraryW(wide("d3d9.dll").as_ptr());
        if m.is_null() {
            STATE.store(1, Relaxed);
            return;
        }
        let Some(create) = GetProcAddress(m, c"Direct3DCreate9".as_ptr().cast()) else {
            STATE.store(2, Relaxed);
            return;
        };
        let create: unsafe extern "system" fn(u32) -> u32 = std::mem::transmute(create);
        let d3d = create(32);
        if d3d == 0 {
            STATE.store(3, Relaxed);
            return;
        }
        let vt = *(d3d as *const u32);
        match mem::patch_vtable(vt, D3D_CREATE_DEVICE, create_device_hook as *const () as usize as u32) {
            Some(old) => {
                if old != create_device_hook as *const () as usize as u32 {
                    ORIG_CREATE_DEVICE.store(old, Relaxed);
                }
                STATE.store(10, Relaxed);
            }
            None => STATE.store(4, Relaxed),
        }
        let release: unsafe extern "system" fn(u32) -> u32 = std::mem::transmute(vfn(d3d, UNK_RELEASE) as usize);
        release(d3d);
        if let Ok(mut ps) = PS.lock() {
            *ps = Some(PsState { shaders: HashMap::new(), current: 0, regs: vec![[0.0; 4]; 224], want_hash: 0, want_draws: 0, want_filter: String::new(), got: Vec::new(), skip: Vec::new(), census_on: false, census: HashMap::new() });
        }
    }
}

unsafe extern "system" fn create_device_hook(this: u32, adapter: u32, ty: u32, hwnd: u32, flags: u32, pp: u32, out: *mut u32) -> i32 {
    let orig: unsafe extern "system" fn(u32, u32, u32, u32, u32, u32, *mut u32) -> i32 = unsafe { std::mem::transmute(ORIG_CREATE_DEVICE.load(Relaxed) as usize) };
    let hr = unsafe { orig(this, adapter, ty, hwnd, flags, pp, out) };
    if hr >= 0 && !out.is_null() {
        let dev = unsafe { *out };
        if dev != 0 {
            DEVICES.fetch_add(1, Relaxed);
            unsafe { hook_device(dev) };
        }
    }
    hr
}

unsafe fn hook_device(dev: u32) {
    let vt = unsafe { *(dev as *const u32) };
    let hooks: [(usize, u32); 9] = [
        (DEV_CREATE_VERTEX_SHADER, create_vs_hook as *const () as usize as u32),
        (DEV_PRESENT, present_hook as *const () as usize as u32),
        (DEV_DRAW_PRIMITIVE, draw_primitive_hook as *const () as usize as u32),
        (DEV_DRAW_INDEXED_PRIMITIVE, draw_indexed_hook as *const () as usize as u32),
        (DEV_DRAW_PRIMITIVE_UP, draw_primitive_up_hook as *const () as usize as u32),
        (DEV_DRAW_INDEXED_PRIMITIVE_UP, draw_indexed_up_hook as *const () as usize as u32),
        (DEV_CREATE_PIXEL_SHADER, create_ps_hook as *const () as usize as u32),
        (DEV_SET_PIXEL_SHADER, set_ps_hook as *const () as usize as u32),
        (DEV_SET_PS_CONST_F, set_ps_const_hook as *const () as usize as u32),
    ];
    for (slot, f) in hooks {
        if let Some(old) = unsafe { mem::patch_vtable(vt, slot, f) }
            && old != f {
                ORIG[slot].store(old, Relaxed);
            }
    }
    STATE.store(20, Relaxed);
}

fn orig(slot: usize) -> usize {
    ORIG[slot].load(Relaxed) as usize
}

unsafe extern "system" fn present_hook(dev: u32, a: u32, b: u32, c: u32, d: u32) -> i32 {
    PRESENTS.fetch_add(1, Relaxed);
    // With the tick boundary hook the per-tick work runs there, on the game thread.
    if crate::tick::INSTALLED.load(Relaxed) == 0 {
        crate::hook::frame();
    }
    let want = CAPTURE.lock().map(|c| c.pending).unwrap_or(false);
    if want {
        let r = unsafe { capture_back_buffer(dev) };
        if let Ok(mut c) = CAPTURE.lock() {
            c.pending = false;
            c.result = Some(r);
            CAPTURE_CV.notify_all();
        }
    }
    let f: unsafe extern "system" fn(u32, u32, u32, u32, u32) -> i32 = unsafe { std::mem::transmute(orig(DEV_PRESENT)) };
    unsafe { f(dev, a, b, c, d) }
}

/// Copy the back buffer: header (width, height, format, pitch) + rows.
unsafe fn capture_back_buffer(dev: u32) -> Result<Vec<u8>, String> {
    unsafe {
        let release = |o: u32| {
            if o != 0 {
                let f: unsafe extern "system" fn(u32) -> u32 = std::mem::transmute(vfn(o, UNK_RELEASE) as usize);
                f(o);
            }
        };
        let mut bb = 0u32;
        let get_bb: unsafe extern "system" fn(u32, u32, u32, u32, *mut u32) -> i32 = std::mem::transmute(vfn(dev, DEV_GET_BACK_BUFFER) as usize);
        if get_bb(dev, 0, 0, 0, &mut bb) < 0 || bb == 0 {
            return Err("GetBackBuffer failed".into());
        }
        let mut desc = [0u32; 8];
        let get_desc: unsafe extern "system" fn(u32, *mut u32) -> i32 = std::mem::transmute(vfn(bb, SURF_GET_DESC) as usize);
        get_desc(bb, desc.as_mut_ptr());
        let (format, ms, w, h) = (desc[0], desc[4], desc[6], desc[7]);
        let mut src = bb;
        let mut rt = 0u32;
        if ms != 0 {
            let create_rt: unsafe extern "system" fn(u32, u32, u32, u32, u32, u32, i32, *mut u32, u32) -> i32 = std::mem::transmute(vfn(dev, DEV_CREATE_RENDER_TARGET) as usize);
            if create_rt(dev, w, h, format, 0, 0, 0, &mut rt, 0) < 0 {
                release(bb);
                return Err("CreateRenderTarget failed".into());
            }
            let stretch: unsafe extern "system" fn(u32, u32, u32, u32, u32, u32) -> i32 = std::mem::transmute(vfn(dev, DEV_STRETCH_RECT) as usize);
            stretch(dev, bb, 0, rt, 0, 0);
            src = rt;
        }
        let mut sys = 0u32;
        let create_off: unsafe extern "system" fn(u32, u32, u32, u32, u32, *mut u32, u32) -> i32 = std::mem::transmute(vfn(dev, DEV_CREATE_OFFSCREEN_PLAIN) as usize);
        if create_off(dev, w, h, format, 2, &mut sys, 0) < 0 {
            release(rt);
            release(bb);
            return Err("CreateOffscreenPlainSurface failed".into());
        }
        let get_rtd: unsafe extern "system" fn(u32, u32, u32) -> i32 = std::mem::transmute(vfn(dev, DEV_GET_RENDER_TARGET_DATA) as usize);
        let hr = get_rtd(dev, src, sys);
        let mut out = Vec::new();
        let mut result = Err(format!("GetRenderTargetData failed {hr:#x}"));
        if hr >= 0 {
            let mut locked = [0u32; 2];
            let lock: unsafe extern "system" fn(u32, *mut u32, u32, u32) -> i32 = std::mem::transmute(vfn(sys, SURF_LOCK_RECT) as usize);
            if lock(sys, locked.as_mut_ptr(), 0, 0x10) >= 0 {
                let pitch = locked[0];
                for v in [w, h, format, pitch] {
                    out.extend(v.to_le_bytes());
                }
                out.extend_from_slice(std::slice::from_raw_parts(locked[1] as *const u8, (pitch * h) as usize));
                let unlock: unsafe extern "system" fn(u32) -> i32 = std::mem::transmute(vfn(sys, SURF_UNLOCK_RECT) as usize);
                unlock(sys);
                result = Ok(out);
            } else {
                result = Err("LockRect failed".into());
            }
        }
        release(sys);
        release(rt);
        release(bb);
        result
    }
}

/// Length of SM3 bytecode (tokens up to and including the end token 0x0000FFFF).
unsafe fn bytecode_len(p: *const u32) -> usize {
    unsafe {
        let mut i = 1usize;
        while i < 1 << 20 {
            let t = *p.add(i);
            if t == 0x0000_ffff {
                return (i + 1) * 4;
            }
            if t & 0xffff == 0xfffe {
                i += 1 + ((t >> 16) & 0x7fff) as usize;
            } else {
                i += 1;
            }
        }
        0
    }
}

fn ctab_names(code: &[u8]) -> String {
    let rd = |p: usize| code.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let mut out = Vec::new();
    let Some(tok) = rd(4) else { return String::new() };
    if tok & 0xffff != 0xfffe || code.get(8..12) != Some(b"CTAB") {
        return String::new();
    }
    let len = ((tok >> 16) & 0x7fff) as usize * 4;
    let ctab = &code[12..(8 + len).min(code.len())];
    let rc = |p: usize| ctab.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (Some(n), Some(info)) = (rc(12), rc(16)) else { return String::new() };
    for i in 0..n as usize {
        let e = info as usize + i * 20;
        let Some(name_off) = rc(e) else { break };
        let Some(r) = ctab.get(e + 4..e + 10) else { break };
        let (set, idx, cnt) = (u16::from_le_bytes([r[0], r[1]]), u16::from_le_bytes([r[2], r[3]]), u16::from_le_bytes([r[4], r[5]]));
        let s: Vec<u8> = ctab.iter().skip(name_off as usize).take_while(|&&c| c != 0).copied().collect();
        let set = ["b", "i", "c", "s"].get(set as usize).copied().unwrap_or("?");
        out.push(format!("{}={set}{idx}x{cnt}", String::from_utf8_lossy(&s)));
    }
    out.join(" ")
}

unsafe extern "system" fn create_ps_hook(dev: u32, func: *const u32, out: *mut u32) -> i32 {
    let f: unsafe extern "system" fn(u32, *const u32, *mut u32) -> i32 = unsafe { std::mem::transmute(orig(DEV_CREATE_PIXEL_SHADER)) };
    let hr = unsafe { f(dev, func, out) };
    if hr >= 0 && !func.is_null() && !out.is_null() {
        let len = unsafe { bytecode_len(func) };
        if len > 0 {
            let code = unsafe { std::slice::from_raw_parts(func as *const u8, len) };
            store_shader(b'p', code);
            let shader = unsafe { *out };
            if let Ok(mut g) = PS.lock()
                && let Some(ps) = g.as_mut() {
                    let id = ps.shaders.len() as u32;
                    ps.shaders.insert(shader, Shader { id, hash: dishonored_probe::fnv64(code), size: len as u32, ctab: ctab_names(code) });
                }
        }
    }
    hr
}

unsafe extern "system" fn create_vs_hook(dev: u32, func: *const u32, out: *mut u32) -> i32 {
    let f: unsafe extern "system" fn(u32, *const u32, *mut u32) -> i32 = unsafe { std::mem::transmute(orig(DEV_CREATE_VERTEX_SHADER)) };
    let hr = unsafe { f(dev, func, out) };
    if hr >= 0 && !func.is_null() {
        let len = unsafe { bytecode_len(func) };
        if len > 0 {
            store_shader(b'v', unsafe { std::slice::from_raw_parts(func as *const u8, len) });
        }
    }
    hr
}

unsafe extern "system" fn set_ps_hook(dev: u32, shader: u32) -> i32 {
    PS_SETS.fetch_add(1, Relaxed);
    if let Ok(mut g) = PS.lock()
        && let Some(ps) = g.as_mut() {
            ps.current = shader;
            if ps.shaders.contains_key(&shader) {
                PS_SETS_KNOWN.fetch_add(1, Relaxed);
            }
        }
    let f: unsafe extern "system" fn(u32, u32) -> i32 = unsafe { std::mem::transmute(orig(DEV_SET_PIXEL_SHADER)) };
    unsafe { f(dev, shader) }
}

unsafe extern "system" fn set_ps_const_hook(dev: u32, start: u32, data: *const f32, count: u32) -> i32 {
    if !data.is_null()
        && let Ok(mut g) = PS.lock()
            && let Some(ps) = g.as_mut() {
                for i in 0..count {
                    let r = (start + i) as usize;
                    if r < ps.regs.len() {
                        for k in 0..4 {
                            ps.regs[r][k] = unsafe { *data.add((i * 4) as usize + k) };
                        }
                    }
                }
            }
    let f: unsafe extern "system" fn(u32, u32, *const f32, u32) -> i32 = unsafe { std::mem::transmute(orig(DEV_SET_PS_CONST_F)) };
    unsafe { f(dev, start, data, count) }
}

/// Records the draw (constants, census); true when the draw must be skipped.
fn on_draw(prims: u32) -> bool {
    DRAWS.fetch_add(1, Relaxed);
    let Ok(mut g) = PS.lock() else { return false };
    let Some(ps) = g.as_mut() else { return false };
    let hash = ps.shaders.get(&ps.current).map(|s| s.hash).unwrap_or(0);
    if ps.census_on {
        let e = ps.census.entry(hash).or_insert((0, 0));
        e.0 += 1;
        e.1 += prims;
    }
    if ps.want_draws > 0 {
        let hit = ps.shaders.get(&ps.current).and_then(|s| {
            let ok = if ps.want_hash != 0 { s.hash == ps.want_hash } else { !ps.want_filter.is_empty() && s.ctab.contains(ps.want_filter.as_str()) };
            ok.then_some(s.hash)
        });
        if let Some(h) = hit {
            let regs = ps.regs.clone();
            ps.got.push((PRESENTS.load(Relaxed), h, regs));
            ps.want_draws -= 1;
        }
    }
    hash != 0 && ps.skip.contains(&hash)
}

unsafe extern "system" fn draw_primitive_hook(dev: u32, ty: u32, start: u32, count: u32) -> i32 {
    if on_draw(count) {
        return 0;
    }
    let f: unsafe extern "system" fn(u32, u32, u32, u32) -> i32 = unsafe { std::mem::transmute(orig(DEV_DRAW_PRIMITIVE)) };
    unsafe { f(dev, ty, start, count) }
}

unsafe extern "system" fn draw_primitive_up_hook(dev: u32, ty: u32, count: u32, data: u32, stride: u32) -> i32 {
    if on_draw(count) {
        return 0;
    }
    let f: unsafe extern "system" fn(u32, u32, u32, u32, u32) -> i32 = unsafe { std::mem::transmute(orig(DEV_DRAW_PRIMITIVE_UP)) };
    unsafe { f(dev, ty, count, data, stride) }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn draw_indexed_up_hook(dev: u32, ty: u32, min: u32, num: u32, count: u32, idx: u32, fmt: u32, data: u32, stride: u32) -> i32 {
    if on_draw(count) {
        return 0;
    }
    let f: unsafe extern "system" fn(u32, u32, u32, u32, u32, u32, u32, u32, u32) -> i32 = unsafe { std::mem::transmute(orig(DEV_DRAW_INDEXED_PRIMITIVE_UP)) };
    unsafe { f(dev, ty, min, num, count, idx, fmt, data, stride) }
}

unsafe extern "system" fn draw_indexed_hook(dev: u32, ty: u32, base: i32, min: u32, num: u32, start: u32, count: u32) -> i32 {
    if on_draw(count) {
        return 0;
    }
    let f: unsafe extern "system" fn(u32, u32, i32, u32, u32, u32, u32) -> i32 = unsafe { std::mem::transmute(orig(DEV_DRAW_INDEXED_PRIMITIVE)) };
    unsafe { f(dev, ty, base, min, num, start, count) }
}
