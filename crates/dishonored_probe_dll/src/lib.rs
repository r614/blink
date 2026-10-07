//! Runtime probe DLL for the original `Dishonored.exe` (GOG build, SHA-256 70da9613..., image
//! base 0x400000 without ASLR). Only meaningful for `i686-pc-windows-msvc`; on other targets
//! the crate is empty.
//!
//! - `DllMain` refuses to load unless the host is `Dishonored.exe` and the launcher's session
//!   token is in the environment (so it only runs in a game `probe_launch` started).
//! - `probe_init` (called by the launcher on a remote thread while the game is still
//!   suspended) installs the hooks and starts the pipe server:
//!   - `UObject::ProcessEvent` (0x00470640) is detoured; the local `PlayerController`'s
//!     `PlayerTick` event is the per-tick game-thread callback (samples, input, queued jobs).
//!   - `IDirect3D9::CreateDevice` is patched in d3d9's shared vtable; the created device's
//!     `Present`, pixel-shader and draw entries are then patched (capture, PS constants).
//! - The pipe `\\.\pipe\dishonored_probe_<pid>_<token>` is local-only and its DACL grants
//!   access to the current user only. It lives as long as the game process.
#![allow(clippy::missing_safety_doc)]

#[cfg(all(windows, target_arch = "x86"))]
mod d3d;
#[cfg(all(windows, target_arch = "x86"))]
mod engine;
#[cfg(all(windows, target_arch = "x86"))]
mod hook;
#[cfg(all(windows, target_arch = "x86"))]
mod input;
#[cfg(all(windows, target_arch = "x86"))]
mod iat;
#[cfg(all(windows, target_arch = "x86"))]
mod mem;
#[cfg(all(windows, target_arch = "x86"))]
mod native;
#[cfg(all(windows, target_arch = "x86"))]
mod server;

#[cfg(all(windows, target_arch = "x86"))]
pub use imp::*;

#[cfg(all(windows, target_arch = "x86"))]
mod imp {
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::Environment::GetEnvironmentVariableW;
    use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
    use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

    pub(crate) static TOKEN: OnceLock<String> = OnceLock::new();

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// The launcher's session token, if this process was started by it.
    fn session_token() -> Option<String> {
        let name = wide(dishonored_probe::TOKEN_ENV);
        let mut buf = [0u16; 128];
        let n = unsafe { GetEnvironmentVariableW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        if n == 0 || n >= buf.len() {
            return None;
        }
        let t = String::from_utf16_lossy(&buf[..n]);
        (t.len() >= 16 && t.chars().all(|c| c.is_ascii_hexdigit())).then_some(t)
    }

    fn host_is_dishonored() -> bool {
        let mut buf = [0u16; 520];
        let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let path = String::from_utf16_lossy(&buf[..n]);
        path.rsplit(['\\', '/']).next().is_some_and(|f| f.eq_ignore_ascii_case("Dishonored.exe"))
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn DllMain(_module: HMODULE, reason: u32, _reserved: *mut c_void) -> i32 {
        if reason == DLL_PROCESS_ATTACH {
            if !host_is_dishonored() {
                return 0;
            }
            match session_token() {
                Some(t) => {
                    let _ = TOKEN.set(t);
                }
                None => return 0,
            }
        }
        1
    }

    /// Thread entry the launcher runs once the DLL is loaded. Returns 1 when the hooks and the
    /// pipe server are up, 0 otherwise (the launcher then kills the game).
    #[unsafe(no_mangle)]
    pub extern "system" fn probe_init(_param: *mut c_void) -> u32 {
        let Some(token) = TOKEN.get() else { return 0 };
        if !crate::hook::install_process_event() {
            return 0;
        }
        crate::d3d::install();
        crate::iat::install();
        // Focus faking (the engine pauses on focus loss): on unless the launcher says "0".
        let mut buf = [0u16; 8];
        let name = wide(dishonored_probe::FAKE_FOCUS_ENV);
        let n = unsafe { GetEnvironmentVariableW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let off = n > 0 && n < buf.len() && buf[0] == b'0' as u16;
        crate::iat::FAKE_FOCUS.store(!off as u32, std::sync::atomic::Ordering::Relaxed);
        if !crate::server::start(token) {
            return 0;
        }
        crate::engine::start_discovery();
        1
    }
}
