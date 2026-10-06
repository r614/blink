//! Key injection at the engine's own input entry. The game takes the keyboard from window
//! messages that it defers and processes only while its window has the focus, and DirectInput
//! only for the mouse (0x009CA1C0 creates the one device), so injected keys go straight to
//! the call the message processor makes:
//! `FViewportClient::InputKey(FViewport*, ControllerId, FName Key, EInputEvent, Amount, bGamepad)`,
//! vtable +0x18 of the `FViewportClient` interface (as `UpdateModifierState`, 0x009C7D70, calls
//! it). The client is `GEngine.GameViewport` (+0x48C) + 0x38 and the viewport is its
//! `Viewport` (+0x40). Runs on the game thread.
use crate::{engine, hook, mem};
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId};

const ENGINE_GAME_VIEWPORT: u32 = 0x48c;
const GVC_FVIEWPORTCLIENT: u32 = 0x38;
const GVC_VIEWPORT: u32 = 0x40;
const VT_INPUT_KEY: u32 = 0x18;

type InputKeyFn = unsafe extern "thiscall" fn(u32, u32, i32, u32, u32, u32, f32, u32) -> u32;

unsafe extern "system" fn enum_cb(hwnd: HWND, out: LPARAM) -> i32 {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != GetCurrentProcessId() {
            return 1;
        }
        let mut buf = [0u16; 64];
        let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) as usize;
        if String::from_utf16_lossy(&buf[..n]) == "LaunchUnrealUWindowsClient" {
            *(out as *mut HWND) = hwnd;
            return 0;
        }
        1
    }
}

/// The game's viewport window (class `LaunchUnrealUWindowsClient` of this process).
pub fn game_window() -> Option<HWND> {
    static CACHED: AtomicU32 = AtomicU32::new(0);
    let c = CACHED.load(Relaxed);
    if c != 0 {
        return Some(c as HWND);
    }
    let mut h: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(enum_cb), &mut h as *mut HWND as LPARAM) };
    if h.is_null() {
        return None;
    }
    CACHED.store(h as u32, Relaxed);
    Some(h)
}

/// Send one key event (`event`: 0 pressed, 1 released, 2 repeat) named like the engine's keys
/// (`SpaceBar`, `W`, `LeftShift`, `C`, `Enter`, `Escape`, ...). Returns the handled flag.
pub fn input_key(key: &str, event: u32) -> Result<bool, String> {
    let idx = engine::name_index(key).ok_or_else(|| format!("no FName {key}"))?;
    let gvc = mem::u32(engine::GENGINE.load(Relaxed) + ENGINE_GAME_VIEWPORT).filter(|&g| g != 0).ok_or("no GameViewport")?;
    hook::on_game_thread(
        move || unsafe {
            let client = gvc + GVC_FVIEWPORTCLIENT;
            let viewport = *((gvc + GVC_VIEWPORT) as *const u32);
            let vt = *(client as *const u32);
            let f: InputKeyFn = std::mem::transmute(*((vt + VT_INPUT_KEY) as *const u32) as usize);
            f(client, viewport, 0, idx, 0, event, 1.0, 0) != 0
        },
        10_000,
    )
}
