//! Runtime probe of the original game (U-011). See research/dishonored/INSTRUMENTATION.md.
//!
//! probe_launch launch [--dll PATH] [-- GAME ARGS...]
//!     Start Dishonored.exe (GOG install) suspended with a fresh session token, inject
//!     dishonored_probe_dll, run its `probe_init`, resume. Prints the pipe name and writes it to
//!     the session file (%TEMP%\dishonored_probe_session.txt). i686 build only.
//! probe_launch status | ping
//! probe_launch console "<command>"
//! probe_launch find <Name|Outer.Name> [Class]
//! probe_launch name <addr>
//! probe_launch read <addr> <len>
//! probe_launch shaders [filter]
//! probe_launch capture <out.bmp>
//! probe_launch measure <jump|fall|sprint|slide|all> [--out DIR]
//! probe_launch kill
//!     Ends the game process of the current session.
use anyhow::{Context, Result, bail};
use dishonored_probe::Client;
use std::path::PathBuf;

mod measure;

const GAME_DIR: &str = r"C:\Program Files\GOG Galaxy\Games\Dishonored\Binaries\Win32";

fn session_file() -> PathBuf {
    std::env::temp_dir().join("dishonored_probe_session.txt")
}

/// (pid, pipe name) of the current session.
fn session() -> Result<(u32, String)> {
    let s = std::fs::read_to_string(session_file()).context("no probe session (run `probe_launch launch`)")?;
    let mut it = s.lines();
    let pid = it.next().unwrap_or("0").trim().parse()?;
    let pipe = it.next().context("bad session file")?.trim().to_string();
    Ok((pid, pipe))
}

pub fn connect() -> Result<Client> {
    let (_, pipe) = session()?;
    Ok(Client::connect_wait(&pipe, std::time::Duration::from_secs(10))?)
}

fn parse_u32(s: &str) -> Result<u32> {
    Ok(match s.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16)?,
        None => s.parse()?,
    })
}

/// Write a 24-bit BMP (bottom-up) from a capture.
pub fn write_bmp(path: &std::path::Path, cap: &dishonored_probe::Capture) -> Result<()> {
    let rgba = cap.to_rgba().context("unsupported back buffer format")?;
    let (w, h) = (cap.width as usize, cap.height as usize);
    let row = (w * 3).div_ceil(4) * 4;
    let mut out = Vec::with_capacity(54 + row * h);
    out.extend(b"BM");
    out.extend(((54 + row * h) as u32).to_le_bytes());
    out.extend([0u8; 4]);
    out.extend(54u32.to_le_bytes());
    out.extend(40u32.to_le_bytes());
    out.extend((w as i32).to_le_bytes());
    out.extend((h as i32).to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(24u16.to_le_bytes());
    out.extend([0u8; 24]);
    for y in (0..h).rev() {
        let start = out.len();
        for x in 0..w {
            let p = &rgba[(y * w + x) * 4..];
            out.extend([p[2], p[1], p[0]]);
        }
        out.resize(start + row, 0);
    }
    std::fs::write(path, out)?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    match cmd {
        "launch" => {
            let mut dll = None;
            let mut game_args = Vec::new();
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--dll" => {
                        dll = Some(PathBuf::from(&args[i + 1]));
                        i += 1;
                    }
                    "--" => {
                        game_args.extend(args[i + 1..].iter().cloned());
                        break;
                    }
                    a => bail!("unknown launch option {a}"),
                }
                i += 1;
            }
            let dll = match dll {
                Some(d) => d,
                None => std::env::current_exe()?.with_file_name("dishonored_probe_dll.dll"),
            };
            if game_args.is_empty() {
                game_args = ["-windowed", "-resx=1280", "-resy=720", "-nostartupmovies"].iter().map(|s| s.to_string()).collect();
            }
            let (pid, pipe) = launch::launch(&dll, &game_args)?;
            std::fs::write(session_file(), format!("{pid}\n{pipe}\n"))?;
            println!("pid {pid}\npipe {pipe}");
            let mut c = Client::connect_wait(&pipe, std::time::Duration::from_secs(30))?;
            let (v, b) = c.ping()?;
            println!("probe protocol {v} ({b})");
        }
        "kill" => {
            let (pid, _) = session()?;
            launch::kill(pid)?;
            let _ = std::fs::remove_file(session_file());
        }
        "ping" => println!("{:?}", connect()?.ping()?),
        "status" => print!("{}", connect()?.status()?),
        "console" => println!("{}", connect()?.console(args.get(1).context("command")?)?),
        "find" => {
            let mut c = connect()?;
            let v = c.find(args.get(1).context("name")?, args.get(2).map(String::as_str).unwrap_or(""))?;
            for a in v.iter().take(200) {
                println!("{a:#010x} {}", c.name_of(*a)?);
            }
            println!("{} objects", v.len());
        }
        "name" => println!("{}", connect()?.name_of(parse_u32(args.get(1).context("addr")?)?)?),
        "read" => {
            let b = connect()?.read(parse_u32(args.get(1).context("addr")?)?, parse_u32(args.get(2).context("len")?)?)?;
            for (i, ch) in b.chunks(16).enumerate() {
                let hex: Vec<String> = ch.iter().map(|x| format!("{x:02x}")).collect();
                println!("+{:04x} {}", i * 16, hex.join(" "));
            }
        }
        "shaders" => {
            let f = args.get(1).map(|s| s.to_lowercase()).unwrap_or_default();
            for s in connect()?.shaders()? {
                if s.ctab.to_lowercase().contains(&f) {
                    println!("{} {:016x} {} {}", s.id, s.hash, s.size, s.ctab);
                }
            }
        }
        "capture" => {
            let out = PathBuf::from(args.get(1).context("out.bmp")?);
            let cap = connect()?.capture()?;
            write_bmp(&out, &cap)?;
            println!("{}x{} format {} -> {}", cap.width, cap.height, cap.format, out.display());
        }
        "measure" => measure::run(&args[1..])?,
        "key" => {
            // key <vk> [down|up|tap]
            let vk = parse_u32(args.get(1).context("vk")?)?;
            let mut c = connect()?;
            match args.get(2).map(String::as_str).unwrap_or("tap") {
                "down" => c.key(vk, true)?,
                "up" => c.key(vk, false)?,
                _ => {
                    c.key(vk, true)?;
                    c.wait_ticks(3)?;
                    c.key(vk, false)?;
                }
            }
        }
        "axes" => {
            // axes <forward> <strafe> <turn> <lookup> <frames>
            let f = |i: usize| args.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            connect()?.input_axes(f(1), f(2), f(3), f(4), f(5) as u32)?;
        }
        "write" => {
            // write <addr> <hex bytes>
            let a = parse_u32(args.get(1).context("addr")?)?;
            let h = args.get(2).context("hex")?;
            let b: Vec<u8> = (0..h.len() / 2).map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16)).collect::<Result<_, _>>()?;
            connect()?.write(a, &b)?;
        }
        _ => {
            eprintln!("usage: see the header of tools/probe_launch/src/main.rs");
            std::process::exit(2);
        }
    }
    Ok(())
}

#[cfg(all(windows, target_arch = "x86"))]
mod launch {
    use anyhow::{Context, Result, bail};
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, FreeLibrary, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
    use windows_sys::Win32::System::LibraryLoader::{DONT_RESOLVE_DLL_REFERENCES, GetModuleHandleW, GetProcAddress, LoadLibraryExW};
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx};
    use windows_sys::Win32::System::Threading::{CREATE_SUSPENDED, CreateRemoteThread, GetExitCodeThread, OpenProcess, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn token() -> String {
        use std::hash::{BuildHasher, Hasher};
        let mut s = String::new();
        for i in 0..2u64 {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(i);
            h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
            h.write_u32(std::process::id());
            s.push_str(&format!("{:016x}", h.finish()));
        }
        s
    }

    unsafe fn remote_call(proc_h: *mut c_void, func: usize, arg: u32) -> Result<u32> {
        unsafe {
            let start: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(func);
            let th = CreateRemoteThread(proc_h, std::ptr::null(), 0, Some(start), arg as usize as *const c_void, 0, std::ptr::null_mut());
            if th.is_null() {
                bail!("CreateRemoteThread failed: {}", std::io::Error::last_os_error());
            }
            if WaitForSingleObject(th, 60_000) != WAIT_OBJECT_0 {
                CloseHandle(th);
                bail!("remote thread did not finish");
            }
            let mut code = 0u32;
            GetExitCodeThread(th, &mut code);
            CloseHandle(th);
            Ok(code)
        }
    }

    pub fn launch(dll: &Path, game_args: &[String]) -> Result<(u32, String)> {
        let tok = token();
        // Inject a per-session copy so the build output is never locked by a running game.
        let dir = std::env::temp_dir().join("dishonored_probe");
        std::fs::create_dir_all(&dir)?;
        for e in std::fs::read_dir(&dir)?.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
        let copy = dir.join(format!("dishonored_probe_{}.dll", &tok[..8]));
        std::fs::copy(dll, &copy).with_context(|| format!("DLL {}", dll.display()))?;
        let dll = copy.canonicalize()?;
        let dll_s = dll.to_string_lossy().trim_start_matches(r"\\?\").to_string();
        let exe = Path::new(super::GAME_DIR).join("Dishonored.exe");
        let child = std::process::Command::new(&exe)
            .args(game_args)
            .current_dir(super::GAME_DIR)
            .env(dishonored_probe::TOKEN_ENV, &tok)
            .creation_flags(CREATE_SUSPENDED)
            .spawn()
            .with_context(|| format!("start {}", exe.display()))?;
        let pid = child.id();
        let ph = child.as_raw_handle() as *mut c_void;
        let r = (|| -> Result<()> {
            unsafe {
                // Path of the DLL into the game.
                let w = wide(&dll_s);
                let mem = VirtualAllocEx(ph, std::ptr::null(), w.len() * 2, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
                if mem.is_null() {
                    bail!("VirtualAllocEx failed");
                }
                let mut put = 0usize;
                if WriteProcessMemory(ph, mem, w.as_ptr().cast(), w.len() * 2, &mut put) == 0 {
                    bail!("WriteProcessMemory failed");
                }
                // kernel32 is mapped at the same base in every 32-bit process of the session.
                let k32 = GetModuleHandleW(wide("kernel32.dll").as_ptr());
                let load = GetProcAddress(k32, c"LoadLibraryW".as_ptr().cast()).context("LoadLibraryW")? as usize;
                let base = remote_call(ph, load, mem as u32)?;
                if base == 0 {
                    bail!("LoadLibraryW in the game failed (wrong host or missing token?)");
                }
                // probe_init's RVA from a non-initialised local mapping.
                let local = LoadLibraryExW(wide(&dll_s).as_ptr(), std::ptr::null_mut(), DONT_RESOLVE_DLL_REFERENCES);
                if local.is_null() {
                    bail!("LoadLibraryExW (local) failed");
                }
                let init = GetProcAddress(local, c"probe_init".as_ptr().cast()).context("probe_init export")? as usize;
                let rva = init - local as usize;
                FreeLibrary(local);
                let ok = remote_call(ph, base as usize + rva, 0)?;
                if ok != 1 {
                    bail!("probe_init failed in the game");
                }
                // Resume the suspended main thread.
                let ntdll = GetModuleHandleW(wide("ntdll.dll").as_ptr());
                let resume = GetProcAddress(ntdll, c"NtResumeProcess".as_ptr().cast()).context("NtResumeProcess")?;
                let resume: unsafe extern "system" fn(*mut c_void) -> i32 = std::mem::transmute(resume);
                if resume(ph) < 0 {
                    bail!("NtResumeProcess failed");
                }
            }
            Ok(())
        })();
        if let Err(e) = r {
            unsafe { TerminateProcess(ph, 1) };
            return Err(e);
        }
        let pipe = format!("{}{}_{}", dishonored_probe::PIPE_PREFIX, pid, tok);
        std::mem::forget(child);
        Ok((pid, pipe))
    }

    pub fn kill(pid: u32) -> Result<()> {
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if h.is_null() {
                bail!("process {pid} not found");
            }
            TerminateProcess(h, 0);
            CloseHandle(h);
        }
        Ok(())
    }
}

#[cfg(not(all(windows, target_arch = "x86")))]
mod launch {
    use anyhow::{Result, bail};
    pub fn launch(_dll: &std::path::Path, _args: &[String]) -> Result<(u32, String)> {
        bail!("launch needs the 32-bit build: cargo build -p probe_launch --target i686-pc-windows-msvc")
    }
    pub fn kill(_pid: u32) -> Result<()> {
        bail!("kill needs the 32-bit build")
    }
}
