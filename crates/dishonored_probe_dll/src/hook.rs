//! `UObject::ProcessEvent` detour (0x00470640, thiscall `(UFunction*, void* Parms, void*
//! Result)`, ret 0xC; reached from `AActor::ProcessEvent`, vtable slot 59 = +0xEC). The local
//! PlayerController's `PlayerTick` event drives everything that must run on the game thread.
use crate::engine::{self, *};
use crate::mem;
use dishonored_probe::Sample;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Mutex, mpsc};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;

pub const PROCESS_EVENT: u32 = 0x0047_0640;
/// `push ebp; mov ebp,esp; push -1` (5 bytes, relocated into the trampoline).
const PROLOGUE: [u8; 5] = [0x55, 0x8b, 0xec, 0x6a, 0xff];
/// vtable offset of the virtual ProcessEvent (AActor override at 0x00567A20).
pub const VT_PROCESS_EVENT: u32 = 0xec;

type ProcessEventFn = unsafe extern "thiscall" fn(u32, u32, u32, u32);

static TRAMPOLINE: AtomicU32 = AtomicU32::new(0);
pub static CALLS: AtomicU32 = AtomicU32::new(0);
pub static TICKS: AtomicU32 = AtomicU32::new(0);

pub type Job = Box<dyn FnOnce() + Send>;
static JOBS: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());

pub struct Recorder {
    pub enabled: bool,
    pub max: usize,
    pub samples: Vec<Sample>,
    pub watch: Vec<(u32, u32)>,
}
pub static RECORDER: Mutex<Recorder> = Mutex::new(Recorder { enabled: false, max: 0, samples: Vec::new(), watch: Vec::new() });

#[derive(Clone, Copy, Default)]
pub struct Input {
    pub active: bool,
    pub axes: [f32; 4],
    pub frames: u32,
}
pub static INPUT: Mutex<Input> = Mutex::new(Input { active: false, axes: [0.0; 4], frames: 0 });
/// PlayerInput axis offsets: aBaseY, aStrafe, aTurn, aLookUp.
const AXES: [u32; 4] = [0x138, 0x150, 0x14c, 0x158];

pub fn install_process_event() -> bool {
    let Some(cur) = mem::read_bytes(PROCESS_EVENT, 5) else { return false };
    if cur != PROLOGUE {
        return false;
    }
    unsafe {
        let t = VirtualAlloc(std::ptr::null(), 32, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as u32;
        if t == 0 {
            return false;
        }
        let mut code = PROLOGUE.to_vec();
        code.push(0xe9);
        code.extend((PROCESS_EVENT + 5).wrapping_sub(t + 10).to_le_bytes());
        std::ptr::copy_nonoverlapping(code.as_ptr(), t as *mut u8, code.len());
        TRAMPOLINE.store(t, Relaxed);
        let detour = process_event_detour as *const () as usize as u32;
        let mut jmp = vec![0xe9];
        jmp.extend(detour.wrapping_sub(PROCESS_EVENT + 5).to_le_bytes());
        mem::patch(PROCESS_EVENT, &jmp)
    }
}

/// Call the original (unhooked) `UObject::ProcessEvent`.
pub unsafe fn process_event_original(obj: u32, func: u32, parms: u32, result: u32) {
    unsafe {
        let f: ProcessEventFn = std::mem::transmute(TRAMPOLINE.load(Relaxed) as usize);
        f(obj, func, parms, result)
    }
}

/// Call ProcessEvent through the object's vtable (keeps AActor's checks). Game thread only.
pub unsafe fn process_event_virtual(obj: u32, func: u32, parms: u32) {
    unsafe {
        let vt = *(obj as *const u32);
        let f: ProcessEventFn = std::mem::transmute(*((vt + VT_PROCESS_EVENT) as *const u32) as usize);
        f(obj, func, parms, 0)
    }
}

static JOBS_PENDING: AtomicU32 = AtomicU32::new(0);
static IN_JOB: AtomicU32 = AtomicU32::new(0);
static INPUT_ACTIVE: AtomicU32 = AtomicU32::new(0);
pub static GAME_THREAD: AtomicU32 = AtomicU32::new(0);
pub static PRESENT_THREAD: AtomicU32 = AtomicU32::new(0);
static LAST_TIME: Mutex<f32> = Mutex::new(0.0);

unsafe extern "thiscall" fn process_event_detour(this: u32, func: u32, parms: u32, result: u32) {
    CALLS.fetch_add(1, Relaxed);
    GAME_THREAD.store(unsafe { GetCurrentThreadId() }, Relaxed);
    if JOBS_PENDING.load(Relaxed) != 0 && IN_JOB.swap(1, Relaxed) == 0 {
        run_jobs();
        IN_JOB.store(0, Relaxed);
    }
    if INPUT_ACTIVE.load(Relaxed) != 0 {
        apply_input();
    }
    unsafe { process_event_original(this, func, parms, result) };
}

fn run_jobs() {
    loop {
        let job = JOBS.lock().ok().and_then(|mut q| {
            let j = q.pop_front();
            JOBS_PENDING.store(q.len() as u32, Relaxed);
            j
        });
        match job {
            Some(j) => j(),
            None => break,
        }
    }
}

fn apply_input() {
    let pc = engine::local_pc();
    let Some(pi) = mem::u32(pc + PC_PLAYER_INPUT) else { return };
    if let Ok(inp) = INPUT.lock() {
        if inp.active {
            for (k, off) in AXES.iter().enumerate() {
                mem::write_bytes(pi + off, &inp.axes[k].to_le_bytes());
            }
        }
    }
}

pub fn set_input(axes: [f32; 4], frames: u32) {
    if let Ok(mut i) = INPUT.lock() {
        i.axes = axes;
        i.frames = frames;
        i.active = axes.iter().any(|&a| a != 0.0);
        INPUT_ACTIVE.store(i.active as u32, Relaxed);
    }
}

/// Once per frame, from the D3D9 `Present` hook (after the frame's world tick).
pub fn frame() {
    PRESENT_THREAD.store(unsafe { GetCurrentThreadId() }, Relaxed);
    let tick = TICKS.fetch_add(1, Relaxed) + 1;
    if let Ok(mut inp) = INPUT.lock() {
        if inp.active && inp.frames > 0 {
            inp.frames -= 1;
            if inp.frames == 0 {
                inp.active = false;
                INPUT_ACTIVE.store(0, Relaxed);
            }
        }
    }
    let Ok(mut rec) = RECORDER.lock() else { return };
    if !rec.enabled || rec.samples.len() >= rec.max {
        return;
    }
    let pc = engine::local_pc();
    if pc == 0 {
        return;
    }
    let pawn = mem::u32(pc + CONTROLLER_PAWN).unwrap_or(0);
    let mut s = Sample { tick, pawn, ..Default::default() };
    if pawn != 0 {
        s.location = mem::read::<[f32; 3]>(pawn + ACTOR_LOCATION).unwrap_or_default();
        s.velocity = mem::read::<[f32; 3]>(pawn + ACTOR_VELOCITY).unwrap_or_default();
        s.physics = mem::read::<u8>(pawn + ACTOR_PHYSICS).unwrap_or(0);
        s.health = mem::read::<i32>(pawn + PAWN_HEALTH).unwrap_or(0);
        for &(off, len) in &rec.watch {
            s.extra.extend(mem::read_bytes(pawn + off, len as usize).unwrap_or_else(|| vec![0; len as usize]));
        }
    }
    if let Some(wi) = mem::u32(pc + ACTOR_WORLDINFO) {
        s.time_seconds = mem::read::<f32>(wi + WI_TIME_SECONDS).unwrap_or(0.0);
        s.real_time_seconds = mem::read::<f32>(wi + WI_REAL_TIME_SECONDS).unwrap_or(0.0);
    }
    if let Ok(mut last) = LAST_TIME.lock() {
        s.delta = s.time_seconds - *last;
        *last = s.time_seconds;
    }
    rec.samples.push(s);
}

/// Run `f` on the game thread (at its next ProcessEvent) and wait for its result.
pub fn on_game_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static, timeout_ms: u64) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    {
        let mut q = JOBS.lock().map_err(|_| "job queue poisoned")?;
        q.push_back(Box::new(move || {
            let _ = tx.send(f());
        }));
        JOBS_PENDING.store(q.len() as u32, Relaxed);
    }
    rx.recv_timeout(std::time::Duration::from_millis(timeout_ms)).map_err(|_| "game thread did not run the job".to_string())
}
