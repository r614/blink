//! Native-level instruments: scratch memory, calls into native functions, entry loggers on
//! native functions, and a filtered trace of `ProcessEvent` calls.
//!
//! - **Scratch memory** (`ALLOC`): a zeroed, never freed block (for FString data or fake
//!   parameter structs the game reads).
//! - **Native calls** (`NATIVE_CALL`): a thunk built per call pushes the arguments, loads `ecx`
//!   (thiscall when `this` is non-zero), calls, and restores `esp` from `ebp`, so cdecl,
//!   stdcall and thiscall callees all return cleanly. Runs on the game thread. The x87 top is
//!   stored too when asked (functions returning a float).
//! - **Entry loggers** (`HOOK_SET`): the first `prologue_len` bytes of a function (whole
//!   instructions, no relative branches: checked by the caller against a disassembly) move
//!   into a stub that saves the registers, logs `ecx`, `nargs` stack arguments and chosen
//!   memory reads, restores the registers, runs the moved bytes and jumps back.
//! - **Event trace** (`EVENT_TRACE`): `ProcessEvent` calls of chosen `UFunction`s, with the
//!   object, the function, the world time and the first bytes of the parameters.
use crate::{engine, hook, mem};
use std::sync::Mutex;
use std::sync::atomic::AtomicU32;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualAlloc};

pub fn alloc(size: u32) -> u32 {
    if size == 0 || size > 16 << 20 {
        return 0;
    }
    unsafe { VirtualAlloc(std::ptr::null(), size as usize, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) as u32 }
}

/// World time of the local player's level (0 before a level is up).
pub fn world_time() -> f32 {
    let pc = engine::local_pc();
    mem::u32(pc + engine::ACTOR_WORLDINFO).and_then(|wi| mem::read::<f32>(wi + engine::WI_TIME_SECONDS)).unwrap_or(0.0)
}

/// Result of a native call: `eax` and the x87 top stored after the call (when asked).
pub struct CallResult {
    pub eax: u32,
    pub st0: f32,
}

/// Call `func` with `args` (pushed right to left) and `this` in `ecx` (0: not set).
pub fn native_call(func: u32, this: u32, args: Vec<u32>, ret_float: bool) -> Result<CallResult, String> {
    if func < 0x401000 {
        return Err(format!("bad function address {func:#x}"));
    }
    hook::on_game_thread(move || unsafe { call_now(func, this, &args, ret_float) }, 10_000)?
}

/// `native_call` on the calling thread (which must be the game thread: tick-script actions run
/// there at the tick boundary).
pub unsafe fn call_now(func: u32, this: u32, args: &[u32], ret_float: bool) -> Result<CallResult, String> {
    unsafe {
        let page = VirtualAlloc(std::ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as u32;
        if page == 0 {
            return Err("VirtualAlloc failed".to_string());
        }
        // The float result slot sits at the end of the page.
        let slot = page + 4092;
        let mut c: Vec<u8> = vec![0x55, 0x89, 0xe5]; // push ebp; mov ebp, esp
        for a in args.iter().rev() {
            c.push(0x68);
            c.extend(a.to_le_bytes());
        }
        c.push(0xb9);
        c.extend(this.to_le_bytes()); // mov ecx, this
        c.push(0xb8);
        c.extend(func.to_le_bytes()); // mov eax, func
        c.extend([0xff, 0xd0]); // call eax
        if ret_float {
            c.extend([0xd9, 0x1d]);
            c.extend(slot.to_le_bytes()); // fstp dword [slot]
        }
        c.extend([0x89, 0xec, 0x5d, 0xc3]); // mov esp, ebp; pop ebp; ret
        std::ptr::copy_nonoverlapping(c.as_ptr(), page as *mut u8, c.len());
        let f: extern "C" fn() -> u32 = std::mem::transmute(page as usize);
        let eax = f();
        let st0 = *(slot as *const f32);
        // The page is leaked on purpose (a few calls per session).
        Ok(CallResult { eax, st0 })
    }
}

/// Where a logged memory read starts: a stack argument, `ecx` or a fixed address.
#[derive(Clone, Copy, Debug)]
pub enum DerefSrc {
    Arg(u8),
    Ecx,
    Abs,
}

#[derive(Clone, Debug)]
pub struct Deref {
    pub src: DerefSrc,
    /// Optional second pointer hop: read a u32 at `base + pre` first (u32::MAX: none).
    pub pre: u32,
    pub off: u32,
    pub len: u32,
}

pub struct HookRecord {
    pub time: f32,
    pub ecx: u32,
    pub ret: u32,
    pub args: Vec<u32>,
    pub data: Vec<u8>,
}

struct Hook {
    addr: u32,
    nargs: u8,
    derefs: Vec<Deref>,
    max: usize,
    records: Vec<HookRecord>,
    enabled: bool,
}

static HOOKS: Mutex<Vec<Hook>> = Mutex::new(Vec::new());

fn read_deref(d: &Deref, ecx: u32, args: &[u32]) -> Vec<u8> {
    let base = match d.src {
        DerefSrc::Arg(i) => args.get(i as usize).copied().unwrap_or(0),
        DerefSrc::Ecx => ecx,
        DerefSrc::Abs => 0,
    };
    let base = if d.pre != u32::MAX { mem::u32(base.wrapping_add(d.pre)).unwrap_or(0) } else { base };
    mem::read_bytes(base.wrapping_add(d.off), d.len as usize).unwrap_or_else(|| vec![0; d.len as usize])
}

/// Called from a hook stub: `regs` points at `[eflags, edi, esi, ebp, esp, ebx, edx, ecx, eax]`
/// (pushfd after pushad); the saved `esp` is the entry stack (return address first).
unsafe extern "stdcall" fn hook_log(id: u32, regs: *const u32) {
    let (esp, ecx) = unsafe { (*regs.add(4), *regs.add(7)) };
    let Ok(mut hooks) = HOOKS.lock() else { return };
    let Some(h) = hooks.get_mut(id as usize) else { return };
    if !h.enabled || h.records.len() >= h.max {
        return;
    }
    let ret = mem::u32(esp).unwrap_or(0);
    let args: Vec<u32> = (0..h.nargs as u32).map(|i| mem::u32(esp + 4 + 4 * i).unwrap_or(0)).collect();
    let mut data = Vec::new();
    for d in &h.derefs {
        data.extend(read_deref(d, ecx, &args));
    }
    h.records.push(HookRecord { time: world_time(), ecx, ret, args, data });
}

/// Install (or reconfigure and clear) an entry logger. Returns its id.
pub fn hook_set(addr: u32, prologue_len: u8, nargs: u8, derefs: Vec<Deref>, max: u32) -> Result<u32, String> {
    {
        let mut hooks = HOOKS.lock().map_err(|_| "poisoned")?;
        if let Some((i, h)) = hooks.iter_mut().enumerate().find(|(_, h)| h.addr == addr) {
            h.nargs = nargs;
            h.derefs = derefs;
            h.max = max as usize;
            h.records.clear();
            h.enabled = max > 0;
            return Ok(i as u32);
        }
    }
    if !(5..=16).contains(&prologue_len) {
        return Err("prologue length must be 5..16 bytes of whole instructions".into());
    }
    let orig = mem::read_bytes(addr, prologue_len as usize).ok_or("unreadable function")?;
    if orig[0] == 0xe9 {
        return Err("already patched (starts with a jmp)".into());
    }
    let id = {
        let mut hooks = HOOKS.lock().map_err(|_| "poisoned")?;
        hooks.push(Hook { addr, nargs, derefs, max: max as usize, records: Vec::new(), enabled: max > 0 });
        hooks.len() as u32 - 1
    };
    let stub = unsafe { VirtualAlloc(std::ptr::null(), 256, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as u32 };
    if stub == 0 {
        return Err("VirtualAlloc failed".into());
    }
    let logger = hook_log as *const () as usize as u32;
    let mut c: Vec<u8> = vec![0x60, 0x9c, 0x54]; // pushad; pushfd; push esp
    c.push(0x68);
    c.extend(id.to_le_bytes()); // push id
    c.push(0xb8);
    c.extend(logger.to_le_bytes()); // mov eax, hook_log
    c.extend([0xff, 0xd0, 0x9d, 0x61]); // call eax; popfd; popad
    c.extend(&orig);
    let back = addr + prologue_len as u32;
    let at = stub + c.len() as u32;
    c.push(0xe9);
    c.extend(back.wrapping_sub(at + 5).to_le_bytes());
    unsafe { std::ptr::copy_nonoverlapping(c.as_ptr(), stub as *mut u8, c.len()) };
    let mut jmp = vec![0xe9];
    jmp.extend(stub.wrapping_sub(addr + 5).to_le_bytes());
    jmp.resize(prologue_len as usize, 0x90);
    // Patch from the game thread so it is not inside the prologue right now.
    let ok = hook::on_game_thread(move || unsafe { mem::patch(addr, &jmp) }, 10_000)?;
    if !ok {
        return Err("patch failed".into());
    }
    Ok(id)
}

pub fn hook_fetch(id: u32) -> Result<Vec<HookRecord>, String> {
    let mut hooks = HOOKS.lock().map_err(|_| "poisoned")?;
    let h = hooks.get_mut(id as usize).ok_or("no such hook")?;
    Ok(std::mem::take(&mut h.records))
}

pub struct EventRecord {
    pub time: f32,
    /// World tick (`tick::GAME_TICK`) of the call.
    pub tick: u32,
    pub this: u32,
    pub func: u32,
    pub parms: Vec<u8>,
}

pub struct EventTrace {
    /// Sorted function addresses.
    pub funcs: Vec<u32>,
    /// Objects of these classes (or subclasses) are traced whatever the function.
    pub classes: Option<crate::reflect::IsACache>,
    pub parm_len: u32,
    pub max: usize,
    pub records: Vec<EventRecord>,
    /// Calls dropped because the buffer was full.
    pub dropped: u32,
}

pub static TRACE_ON: AtomicU32 = AtomicU32::new(0);
pub static TRACE: Mutex<EventTrace> = Mutex::new(EventTrace { funcs: Vec::new(), classes: None, parm_len: 0, max: 0, records: Vec::new(), dropped: 0 });

/// From the ProcessEvent detour, before the call.
pub fn trace_event(this: u32, func: u32, parms: u32) {
    let Ok(mut t) = TRACE.lock() else { return };
    let hit = t.funcs.binary_search(&func).is_ok() || t.classes.as_mut().is_some_and(|c| c.test(this));
    if !hit {
        return;
    }
    if t.records.len() >= t.max {
        t.dropped += 1;
        return;
    }
    let parms = if t.parm_len > 0 { mem::read_bytes(parms, t.parm_len as usize).unwrap_or_default() } else { Vec::new() };
    let tick = crate::tick::GAME_TICK.load(std::sync::atomic::Ordering::Relaxed);
    t.records.push(EventRecord { time: world_time(), tick, this, func, parms });
}

/// Function objects whose `Outer.Name` path contains one of `subs` (case-insensitive), from
/// a scan of every live `Function`.
pub fn functions_matching(subs: &[String]) -> Vec<u32> {
    if subs.is_empty() {
        return Vec::new();
    }
    let subs: Vec<String> = subs.iter().map(|s| s.to_ascii_lowercase()).collect();
    let mut out = Vec::new();
    for f in engine::find("", "Function") {
        let n = engine::obj_name(f).unwrap_or_default();
        let o = mem::u32(f + engine::OBJ_OUTER).and_then(engine::obj_name).unwrap_or_default();
        let path = format!("{o}.{n}").to_ascii_lowercase();
        if subs.iter().any(|s| path.contains(s.as_str())) {
            out.push(f);
        }
    }
    out
}

/// `GWorld` (the `UWorld*` global) and `UWorld::SingleLineCheck` (thiscall: `FCheckResult& Hit,
/// AActor* Source, const FVector& End, const FVector& Start, DWORD TraceFlags, const FVector&
/// Extent, ULightComponent* Light`; returns true when nothing was hit; `ret 0x1C`).
const GWORLD: u32 = 0x0144_9888;
const SINGLE_LINE_CHECK: u32 = 0x0064_E7A0;

/// Run collision checks with the engine's own `SingleLineCheck` on the game thread.
/// `FCheckResult` (0x4C bytes): Actor +0x4, Location +0x8, Normal +0x14, Time +0x20,
/// Item +0x24, Component +0x30.
pub fn line_checks(q: Vec<dishonored_probe::LineQuery>) -> Result<Vec<dishonored_probe::LineResult>, String> {
    hook::on_game_thread(move || run_line_checks(&q), 60_000)?
}

#[cfg(target_arch = "x86")]
fn run_line_checks(q: &[dishonored_probe::LineQuery]) -> Result<Vec<dishonored_probe::LineResult>, String> {
    type Slc = unsafe extern "thiscall" fn(u32, *mut u8, u32, *const f32, *const f32, u32, *const f32, u32) -> u32;
    let world = mem::u32(GWORLD).unwrap_or(0);
    if world == 0 {
        return Err("no GWorld".into());
    }
    let f: Slc = unsafe { std::mem::transmute(SINGLE_LINE_CHECK as usize) };
    let mut out = Vec::with_capacity(q.len());
    for x in q {
        let mut hit = [0u8; 0x60];
        hit[0x20..0x24].copy_from_slice(&1.0f32.to_le_bytes());
        hit[0x24..0x28].copy_from_slice(&(-1i32).to_le_bytes());
        let none = unsafe { f(world, hit.as_mut_ptr(), x.source, x.end.as_ptr(), x.start.as_ptr(), x.flags, x.extent.as_ptr(), 0) } & 0xff != 0;
        let fl = |o: usize| f32::from_le_bytes([hit[o], hit[o + 1], hit[o + 2], hit[o + 3]]);
        let u = |o: usize| u32::from_le_bytes([hit[o], hit[o + 1], hit[o + 2], hit[o + 3]]);
        out.push(dishonored_probe::LineResult {
            hit: !none,
            time: fl(0x20),
            location: [fl(0x8), fl(0xC), fl(0x10)],
            normal: [fl(0x14), fl(0x18), fl(0x1C)],
            actor: u(0x4),
            component: u(0x30),
        });
    }
    Ok(out)
}

#[cfg(not(target_arch = "x86"))]
fn run_line_checks(_q: &[dishonored_probe::LineQuery]) -> Result<Vec<dishonored_probe::LineResult>, String> {
    Err("line checks need the 32-bit game process".into())
}
