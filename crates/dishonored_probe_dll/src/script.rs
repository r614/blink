//! Tick scripts: a whole scenario run executed at the tick boundary on the game thread, so a
//! trace costs no pipe round trip per tick.
//!
//! A script is started while the game is parked in lockstep. At that boundary (tick k = 0) and
//! at every following one (k = 1..=ticks) the reads are sampled first (the end-of-tick state:
//! input processed, pawn physics done, before the next world tick), then the actions of tick k
//! are applied (they act during the world tick that produces sample k + 1). After sample
//! `ticks` the game parks again and the client fetches the frames.
use crate::{engine, hook, input, mem, tick};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

pub enum Act {
    /// FName index of the key, event (0 pressed, 1 released).
    Key(u32, u32),
    Write(u32, Vec<u8>),
    /// `PlayerController.ConsoleCommand` function, command.
    Console(u32, String),
    /// A native call at the boundary: function address, `this` (ecx), stack arguments. Used with
    /// a client-built `FFrame` (a native `UFunction`'s exec thunk), e.g. `Actor.SetLocation`.
    Native(u32, u32, Vec<u32>),
}

pub struct Script {
    pub ticks: u32,
    /// (base, offsets, len): pointer chains as in `READS`.
    pub reads: Vec<(u32, Vec<u32>, u32)>,
    /// Sorted by tick.
    pub actions: Vec<(u32, Act)>,
    pub next: usize,
    pub base: u32,
    pub frames: Vec<u8>,
    pub errors: Vec<String>,
}

pub const IDLE: u32 = 0;
pub const PENDING: u32 = 1;
pub const RUNNING: u32 = 2;
pub const DONE: u32 = 3;
pub static STATE: AtomicU32 = AtomicU32::new(IDLE);
pub static SCRIPT: Mutex<Option<Script>> = Mutex::new(None);

fn sample(s: &mut Script, k: u32) {
    s.frames.extend(k.to_le_bytes());
    for (base, offs, len) in &s.reads {
        let mut a = *base;
        let mut ok = a != 0;
        for o in &offs[..offs.len() - 1] {
            if !ok {
                break;
            }
            match mem::u32(a.wrapping_add(*o)) {
                Some(v) if v != 0 => a = v,
                _ => ok = false,
            }
        }
        let b = if ok { mem::read_bytes(a.wrapping_add(*offs.last().unwrap()), *len as usize) } else { None };
        s.frames.push(b.is_some() as u8);
        s.frames.extend(b.unwrap_or_else(|| vec![0; *len as usize]));
    }
}

fn act(s: &mut Script, k: u32) {
    while s.next < s.actions.len() && s.actions[s.next].0 <= k {
        let r = match &s.actions[s.next].1 {
            Act::Key(idx, ev) => input::input_key_now(*idx, *ev).map(|_| ()),
            Act::Write(a, b) => {
                if mem::write_bytes(*a, b) {
                    Ok(())
                } else {
                    Err(format!("unwritable {a:#x}"))
                }
            }
            Act::Native(func, this, args) => {
                if *func < 0x401000 {
                    Err(format!("bad function address {func:#x}"))
                } else {
                    unsafe { crate::native::call_now(*func, *this, args, false) }.map(|_| ())
                }
            }
            Act::Console(func, cmd) => {
                let pc = engine::local_pc();
                if pc == 0 {
                    Err("no PlayerController".into())
                } else {
                    let w: Vec<u16> = cmd.encode_utf16().chain(Some(0)).collect();
                    let mut parms = [0u32; 16];
                    parms[0] = w.as_ptr() as u32;
                    parms[1] = w.len() as u32;
                    parms[2] = w.len() as u32;
                    parms[3] = 1;
                    unsafe { hook::process_event_virtual(pc, *func, parms.as_mut_ptr() as u32) };
                    Ok(())
                }
            }
        };
        if let Err(e) = r {
            s.errors.push(format!("tick {k}: {e}"));
        }
        s.next += 1;
    }
}

/// From the park loop: start a pending script at this boundary (k = 0).
pub fn start_if_pending() {
    if STATE.load(Relaxed) != PENDING {
        return;
    }
    let Ok(mut g) = SCRIPT.lock() else { return };
    let Some(s) = g.as_mut() else { return };
    let now = tick::GAME_TICK.load(Relaxed);
    s.base = now;
    sample(s, 0);
    if s.ticks == 0 {
        STATE.store(DONE, Relaxed);
        return;
    }
    act(s, 0);
    tick::HOLD_UNTIL.store(now + s.ticks, Relaxed);
    STATE.store(RUNNING, Relaxed);
}

/// From every boundary (before parking).
pub fn on_boundary(now: u32) {
    if STATE.load(Relaxed) != RUNNING {
        return;
    }
    let Ok(mut g) = SCRIPT.lock() else { return };
    let Some(s) = g.as_mut() else { return };
    let k = now.wrapping_sub(s.base);
    sample(s, k);
    if k >= s.ticks {
        STATE.store(DONE, Relaxed);
    } else {
        act(s, k);
    }
}
