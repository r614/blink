//! Focus faking: the game only takes keyboard input while its window has the focus. The
//! executable's imports of `GetFocus`, `GetForegroundWindow` and `GetActiveWindow` (user32)
//! are redirected so that, while focus faking is on, they report the game's own window. Only
//! this process's import table is touched; the desktop focus stays with the user.
use crate::mem;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

const IMAGE_BASE: u32 = 0x0040_0000;

static ORIG: [AtomicU32; 7] = [const { AtomicU32::new(0) }; 7];
pub static FAKE_FOCUS: AtomicU32 = AtomicU32::new(0);
pub static PATCHED: AtomicU32 = AtomicU32::new(0);

fn fake() -> Option<u32> {
    (FAKE_FOCUS.load(Relaxed) != 0).then(|| crate::input::game_window().map(|h| h as u32)).flatten()
}

unsafe extern "system" fn get_focus() -> u32 {
    if let Some(h) = fake() {
        return h;
    }
    let f: unsafe extern "system" fn() -> u32 = unsafe { std::mem::transmute(ORIG[0].load(Relaxed) as usize) };
    unsafe { f() }
}
unsafe extern "system" fn get_foreground_window() -> u32 {
    if let Some(h) = fake() {
        return h;
    }
    let f: unsafe extern "system" fn() -> u32 = unsafe { std::mem::transmute(ORIG[1].load(Relaxed) as usize) };
    unsafe { f() }
}
unsafe extern "system" fn get_active_window() -> u32 {
    if let Some(h) = fake() {
        return h;
    }
    let f: unsafe extern "system" fn() -> u32 = unsafe { std::mem::transmute(ORIG[2].load(Relaxed) as usize) };
    unsafe { f() }
}

/// Never move, confine or take the user's cursor and focus from the background game.
unsafe extern "system" fn clip_cursor(_rect: u32) -> i32 {
    1
}
static CURSOR: std::sync::Mutex<Option<(i32, i32)>> = std::sync::Mutex::new(None);
unsafe extern "system" fn set_cursor_pos(x: i32, y: i32) -> i32 {
    if let Ok(mut c) = CURSOR.lock() {
        *c = Some((x, y));
    }
    1
}
/// While focus is faked the game sees the cursor where it last tried to put it (no mouse
/// look from the user's real cursor).
unsafe extern "system" fn get_cursor_pos(out: *mut i32) -> i32 {
    if FAKE_FOCUS.load(Relaxed) != 0 && !out.is_null() {
        if let Some((x, y)) = CURSOR.lock().ok().and_then(|c| *c) {
            unsafe {
                *out = x;
                *out.add(1) = y;
            }
            return 1;
        }
    }
    let f: unsafe extern "system" fn(*mut i32) -> i32 = unsafe { std::mem::transmute(ORIG[6].load(Relaxed) as usize) };
    unsafe { f(out) }
}
unsafe extern "system" fn set_foreground_window(_h: u32) -> i32 {
    1
}

fn cstr(addr: u32) -> String {
    mem::read_bytes(addr, 64).map(|b| b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()).unwrap_or_default()
}

pub fn install() {
    let Some(pe) = mem::u32(IMAGE_BASE + 0x3c) else { return };
    let Some(import_rva) = mem::u32(IMAGE_BASE + pe + 0x80) else { return };
    let mut desc = IMAGE_BASE + import_rva;
    let wanted = [("GetFocus", get_focus as *const () as u32), ("GetForegroundWindow", get_foreground_window as *const () as u32), ("GetActiveWindow", get_active_window as *const () as u32),
        ("ClipCursor", clip_cursor as *const () as u32),
        ("SetCursorPos", set_cursor_pos as *const () as u32),
        ("SetForegroundWindow", set_foreground_window as *const () as u32),
        ("GetCursorPos", get_cursor_pos as *const () as u32),
    ];
    loop {
        let Some(d) = mem::read::<[u32; 5]>(desc) else { return };
        if d[3] == 0 {
            break;
        }
        if cstr(IMAGE_BASE + d[3]).eq_ignore_ascii_case("user32.dll") {
            let (names, iat) = (if d[0] != 0 { d[0] } else { d[4] }, d[4]);
            let mut i = 0;
            while let Some(thunk) = mem::u32(IMAGE_BASE + names + 4 * i) {
                if thunk == 0 {
                    break;
                }
                if thunk & 0x8000_0000 == 0 {
                    let name = cstr(IMAGE_BASE + thunk + 2);
                    if let Some(k) = wanted.iter().position(|(n, _)| *n == name) {
                        let slot = IMAGE_BASE + iat + 4 * i;
                        if let Some(old) = mem::u32(slot) {
                            if old != wanted[k].1 {
                                ORIG[k].store(old, Relaxed);
                                if unsafe { mem::patch(slot, &wanted[k].1.to_le_bytes()) } {
                                    PATCHED.fetch_add(1, Relaxed);
                                }
                            }
                        }
                    }
                }
                i += 1;
            }
        }
        desc += 20;
    }
}
