//! Keyboard input injected as window messages to the game's own viewport window (class
//! `LaunchUnrealUWindowsClient` of this process), so it takes the game's normal binding path.
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VK_TO_VSC, MapVirtualKeyW};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowThreadProcessId, PostMessageW, WM_KEYDOWN, WM_KEYUP};

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

pub fn game_window() -> Option<HWND> {
    let mut h: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(enum_cb), &mut h as *mut HWND as LPARAM) };
    (!h.is_null()).then_some(h)
}

pub fn post_key(vk: u32, down: bool) -> Result<(), String> {
    let hwnd = game_window().ok_or("game window not found")?;
    let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) };
    let mut lp = 1u32 | (scan << 16);
    if !down {
        lp |= (1 << 30) | (1 << 31);
    }
    let ok = unsafe { PostMessageW(hwnd, if down { WM_KEYDOWN } else { WM_KEYUP }, vk as usize, lp as isize) };
    if ok == 0 { Err("PostMessage failed".into()) } else { Ok(()) }
}
