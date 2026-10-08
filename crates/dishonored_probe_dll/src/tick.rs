//! The engine loop's tick boundary, on the game thread.
//!
//! `FEngineLoop::Tick` (0x009E03B0) calls the time update (0x009DDDF0: `GUseFixedTimeStep`,
//! `GFixedDeltaTime`, `GDeltaTime`) once per iteration, before the engine tick. Its prologue
//! `push ebp; mov ebp,esp; sub esp,0x20` (6 bytes, no relative branch) moves into a stub that
//! first calls [`boundary`]. At that point the previous world tick is complete and the next one
//! has not started, so:
//!
//! - `GAME_TICK` counts world ticks exactly (the `Present` counter counts rendered frames, on
//!   the render thread);
//! - queued game-thread jobs (console commands, calls, key events) run here too, between ticks;
//! - **lockstep**: the game thread parks at the boundary while `GAME_TICK >= HOLD_UNTIL`,
//!   running jobs, so a client reads and writes a frozen world and releases exactly N ticks
//!   (`STEP`). With the fixed time step every released tick advances the world by 1/Hz, so the
//!   parked wall-clock time does not leak into the simulation. A parked game whose client has
//!   gone quiet for `LOCKSTEP_IDLE_MS` is released (lockstep off) so it never stays frozen;
//! - **spawn watch**: objects that appear in `GObjObjects` with a `Level` outer (spawned actors:
//!   projectiles, emitters, pickups...) are recorded with their tick.
use crate::{engine, hook, mem};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};

pub const TIME_UPDATE: u32 = 0x009d_ddf0;
const PROLOGUE: [u8; 6] = [0x55, 0x8b, 0xec, 0x83, 0xec, 0x20];
const LOCKSTEP_IDLE_MS: u64 = 300_000;

pub static INSTALLED: AtomicU32 = AtomicU32::new(0);
pub static GAME_TICK: AtomicU32 = AtomicU32::new(0);
pub static LOCKSTEP: AtomicU32 = AtomicU32::new(0);
pub static HOLD_UNTIL: AtomicU32 = AtomicU32::new(0);
pub static PARKED: AtomicU32 = AtomicU32::new(0);
/// Milliseconds (since the DLL started) of the last client request.
pub static LAST_CONTACT: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

pub fn touch() {
    LAST_CONTACT.store(now_ms(), Relaxed);
}

/// Patch the time update. Called from `probe_init` while the game is still suspended.
pub fn install() -> bool {
    let Some(cur) = mem::read_bytes(TIME_UPDATE, PROLOGUE.len()) else { return false };
    if cur != PROLOGUE {
        return false;
    }
    let stub = unsafe { VirtualAlloc(std::ptr::null(), 64, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as u32 };
    if stub == 0 {
        return false;
    }
    let cb = boundary as *const () as usize as u32;
    let mut c: Vec<u8> = vec![0x60, 0x9c]; // pushad; pushfd
    c.push(0xb8);
    c.extend(cb.to_le_bytes()); // mov eax, boundary
    c.extend([0xff, 0xd0, 0x9d, 0x61]); // call eax; popfd; popad
    c.extend(PROLOGUE);
    let back = TIME_UPDATE + PROLOGUE.len() as u32;
    let at = stub + c.len() as u32;
    c.push(0xe9);
    c.extend(back.wrapping_sub(at + 5).to_le_bytes());
    unsafe { std::ptr::copy_nonoverlapping(c.as_ptr(), stub as *mut u8, c.len()) };
    let mut jmp = vec![0xe9];
    jmp.extend(stub.wrapping_sub(TIME_UPDATE + 5).to_le_bytes());
    jmp.push(0x90);
    if !unsafe { mem::patch(TIME_UPDATE, &jmp) } {
        return false;
    }
    INSTALLED.store(1, Relaxed);
    true
}

extern "C" fn boundary() {
    let tick = GAME_TICK.fetch_add(1, Relaxed) + 1;
    hook::GAME_THREAD.store(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }, Relaxed);
    hook::run_pending_jobs();
    // The per-tick sample / input countdown (`hook::frame`) runs here instead of in Present.
    hook::frame();
    if SPAWN_ON.load(Relaxed) != 0 {
        spawn_scan(tick);
    }
    crate::script::on_boundary(tick);
    if LOCKSTEP.load(Relaxed) != 0 {
        let since = std::time::Instant::now();
        while LOCKSTEP.load(Relaxed) != 0 && GAME_TICK.load(Relaxed) >= HOLD_UNTIL.load(Relaxed) {
            PARKED.store(1, Relaxed);
            hook::run_pending_jobs();
            crate::script::start_if_pending();
            if now_ms().saturating_sub(LAST_CONTACT.load(Relaxed)) > LOCKSTEP_IDLE_MS {
                LOCKSTEP.store(0, Relaxed);
                break;
            }
            pause(since);
        }
        PARKED.store(0, Relaxed);
    }
}

/// Busy-yield for the first 50 ms of a wait (a Windows sleep lasts a timer period, up to
/// 15.6 ms, which would dominate a lockstep tick), then sleep 1 ms at a time.
fn pause(since: std::time::Instant) {
    if since.elapsed().as_millis() < 50 {
        std::thread::yield_now();
    } else {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Wait (from a server thread) until the game is parked at `tick` or later.
pub fn wait_parked(min_tick: u32, timeout_ms: u64) -> Result<u32, String> {
    let t0 = std::time::Instant::now();
    loop {
        let t = GAME_TICK.load(Relaxed);
        if PARKED.load(Relaxed) != 0 && t >= min_tick {
            return Ok(t);
        }
        if LOCKSTEP.load(Relaxed) == 0 {
            return Err("lockstep was released".into());
        }
        if t0.elapsed().as_millis() as u64 > timeout_ms {
            return Err(format!("game thread not parked within {timeout_ms} ms (tick {t})"));
        }
        pause(t0);
    }
}

/// Wait until `n` more world ticks have run (free-running).
pub fn wait_ticks(n: u32, timeout_ms: u64) -> Result<u32, String> {
    let start = GAME_TICK.load(Relaxed);
    let t0 = std::time::Instant::now();
    loop {
        let t = GAME_TICK.load(Relaxed);
        if t.wrapping_sub(start) >= n {
            return Ok(t);
        }
        if t0.elapsed().as_millis() as u64 > timeout_ms {
            return Err(format!("{} of {n} ticks within {timeout_ms} ms", t.wrapping_sub(start)));
        }
        pause(t0);
    }
}

/// Lockstep on: park at the next boundary. Returns the parked tick.
pub fn lockstep_on(timeout_ms: u64) -> Result<u32, String> {
    touch();
    if LOCKSTEP.load(Relaxed) == 0 {
        HOLD_UNTIL.store(0, Relaxed);
        LOCKSTEP.store(1, Relaxed);
    }
    wait_parked(0, timeout_ms)
}

pub fn lockstep_off() {
    LOCKSTEP.store(0, Relaxed);
}

/// Release `n` ticks and wait until parked again (lockstep), or wait `n` ticks (free-running).
pub fn step(n: u32, timeout_ms: u64) -> Result<u32, String> {
    touch();
    if LOCKSTEP.load(Relaxed) == 0 {
        return wait_ticks(n, timeout_ms);
    }
    let cur = GAME_TICK.load(Relaxed);
    let target = cur + n;
    HOLD_UNTIL.store(target, Relaxed);
    if n == 0 {
        return wait_parked(cur, timeout_ms);
    }
    // Leave the parked state of the current boundary first.
    wait_parked(target, timeout_ms)
}

// ----- spawn watch -----

pub static SPAWN_ON: AtomicU32 = AtomicU32::new(0);

pub struct SpawnWatch {
    pub prev: Vec<u32>,
    pub level_name: u32,
    pub max: usize,
    pub records: Vec<(u32, u32, u32)>,
}
pub static SPAWNS: Mutex<SpawnWatch> = Mutex::new(SpawnWatch { prev: Vec::new(), level_name: 0, max: 0, records: Vec::new() });

/// Start (max > 0) or stop the spawn watch; the current objects are the baseline.
pub fn spawn_watch(max: u32) -> Result<(), String> {
    let level = engine::name_index("Level").ok_or("no FName Level")?;
    let objs = engine::objects();
    let mut s = SPAWNS.lock().map_err(|_| "poisoned")?;
    s.prev = objs;
    s.level_name = level;
    s.max = max as usize;
    s.records.clear();
    SPAWN_ON.store((max > 0) as u32, Relaxed);
    Ok(())
}

fn spawn_scan(tick: u32) {
    let Ok(mut s) = SPAWNS.lock() else { return };
    let g = engine::GOBJECTS.load(Relaxed);
    let Some(arr) = mem::read::<[u32; 3]>(g) else { return };
    let (data, num) = (arr[0], arr[1] as usize);
    if data == 0 || num > 8_000_000 {
        return;
    }
    // Game thread: the array and its live objects are stable here.
    let cur: &[u32] = unsafe { std::slice::from_raw_parts(data as *const u32, num) };
    let level = s.level_name;
    let mut found = Vec::new();
    for (i, &p) in cur.iter().enumerate() {
        if p == 0 || s.prev.get(i) == Some(&p) {
            continue;
        }
        unsafe {
            let outer = *((p + engine::OBJ_OUTER) as *const u32);
            if outer == 0 {
                continue;
            }
            let oc = *((outer + engine::OBJ_CLASS) as *const u32);
            if oc == 0 || *((oc + engine::OBJ_NAME) as *const u32) != level {
                continue;
            }
            found.push((tick, p, *((p + engine::OBJ_CLASS) as *const u32)));
        }
    }
    s.prev.clear();
    s.prev.extend_from_slice(cur);
    let room = s.max.saturating_sub(s.records.len());
    s.records.extend(found.into_iter().take(room));
}
