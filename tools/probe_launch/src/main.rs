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
//! probe_launch session start [--map M] [--hz 60] | stop | repl | <op> ['<json>' | key=value...]
//!     The persistent hooked session and its JSON ops (session.rs, ORACLE.md).
//! probe_launch scenario <NAME | file.ron>... [--no-reload] [--partial] [--channel ..] [--json OUT]
//!     Per-tick trace of a scenario spec into the oracle cache (capture.rs).
//! probe_launch queue run [FILE] [--out RESULTS.json] [--skip-layouts] [--only PREFIX]
//!     The capture queue (fixtures/oracle/capture_queue.txt) in the running session (queue.rs).
use anyhow::{Context, Result, bail};
use dishonored_probe::Client;
use std::path::PathBuf;

mod capture;
mod lab;
mod measure;
mod playground;
mod queue;
mod reflect;
mod session;
mod oracle;
mod oracle_powers;
mod oracle_stealth;
mod oracle_combat;
mod oracle_misc;
mod oracle_collision;

#[cfg_attr(not(all(windows, target_arch = "x86")), allow(dead_code))]
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

/// Start the game with the probe (one instance per session file) and connect: (pid, pipe,
/// protocol, build).
pub fn launch_session(dll: Option<PathBuf>, mut game_args: Vec<String>) -> Result<(u32, String, u32, String)> {
    let dll = match dll {
        Some(d) => d,
        None => std::env::current_exe()?.with_file_name("dishonored_probe_dll.dll"),
    };
    if game_args.is_empty() {
        // -nomovie: no Bink startup / loading movies (the loading movie of a level otherwise
        // waits for a key the engine only reads from a focused window).
        game_args = ["-windowed", "-resx=1280", "-resy=720", "-nostartupmovies", "-nomovie"].iter().map(|s| s.to_string()).collect();
    }
    let (pid, pipe) = launch::launch(&dll, &game_args)?;
    std::fs::write(session_file(), format!("{pid}\n{pipe}\n"))?;
    let mut c = Client::connect_wait(&pipe, std::time::Duration::from_secs(30))?;
    let (v, b) = c.ping()?;
    if v != dishonored_probe::PROTOCOL_VERSION {
        bail!("probe DLL protocol {v}, launcher {}", dishonored_probe::PROTOCOL_VERSION);
    }
    Ok((pid, pipe, v, b))
}

/// End the session's game (by pid) and forget the session.
pub fn kill_session() -> Result<()> {
    let (pid, _) = session()?;
    let r = launch::kill(pid);
    let _ = std::fs::remove_file(session_file());
    r
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
    // The oracle cache the captures go into: the same default as scripts/dev-env (the shared
    // data cache), so `oracle` finds them whatever shell ran the capture.
    if std::env::var_os("DISHONORED_CACHE").is_none()
        && std::env::var_os("DISHONORED_ORACLE_CACHE").is_none()
        && let Some(l) = std::env::var_os("LOCALAPPDATA")
    {
        // Single-threaded here, before anything reads the environment.
        unsafe { std::env::set_var("DISHONORED_CACHE", PathBuf::from(l).join("dishonored-rs").join("cache")) };
    }
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
            let (pid, pipe, v, b) = launch_session(dll, game_args)?;
            println!("pid {pid}\npipe {pipe}");
            println!("probe protocol {v} ({b})");
        }
        "kill" => kill_session()?,
        "session" => session::run(&args[1..])?,
        "queue" => queue::run(&args[1..])?,
        "scenario" => capture::run(&args[1..])?,
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
        "fnv" => {
            // fnv <file>...: the probe's shader hash (FNV-1a 64) of shader blobs, e.g. the
            // `shader_maps` output, to match live shaders from `shaders` / `census`.
            for f in &args[1..] {
                println!("{:016x} {f}", dishonored_probe::fnv64(&std::fs::read(f)?));
            }
        }
        "census" => {
            // census [frames]: pixel shaders drawn over the next frames, with draw counts.
            let frames = args.get(1).map(|s| parse_u32(s)).transpose()?.unwrap_or(1);
            let mut c = connect()?;
            let shaders: std::collections::HashMap<u64, (u32, String)> = c.shaders()?.into_iter().map(|s| (s.hash, (s.id, s.ctab))).collect();
            for (h, d, p) in c.draw_census(frames)? {
                let (id, ctab) = shaders.get(&h).cloned().unwrap_or((u32::MAX, String::new()));
                println!("{id} {h:016x} draws {d} prims {p} {ctab}");
            }
        }
        "skip" => {
            // skip [0xHASH | CTAB-filter]...: skip draws with these pixel shaders; no argument
            // restores every draw.
            let mut c = connect()?;
            let shaders = c.shaders()?;
            let mut hashes = Vec::new();
            for a in &args[1..] {
                if let Some(h) = a.strip_prefix("0x") {
                    hashes.push(u64::from_str_radix(h, 16)?);
                } else {
                    hashes.extend(shaders.iter().filter(|s| s.ctab.contains(a.as_str())).map(|s| s.hash));
                }
            }
            c.draw_skip(&hashes)?;
            println!("skipping {} shaders", hashes.len());
        }
        "capture" => {
            let out = PathBuf::from(args.get(1).context("out.bmp")?);
            let cap = connect()?.capture()?;
            write_bmp(&out, &cap)?;
            println!("{}x{} format {} -> {}", cap.width, cap.height, cap.format, out.display());
        }
        "measure" => measure::run(&args[1..])?,
        "lab" => lab::run(&args[1..])?,
        "collide" => oracle_collision::run(&args[1..])?,
        "place" => {
            // place X Y Z [YAW_DEG]: write the pawn's Location (and Controller yaw), step forward
            // one frame so physWalking looks for a floor, then let it settle for a second.
            let f = |i: usize| -> Result<f32> { Ok(args.get(i).context("place X Y Z [YAW]")?.parse()?) };
            let (x, y, z) = (f(1)?, f(2)?, f(3)?);
            let mut c = connect()?;
            let st = c.status()?;
            let get = |k: &str| st.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).and_then(|v| parse_u32(v).ok()).unwrap_or(0);
            let (pawn, pc) = (get("pawn"), get("local_pc"));
            if let Ok(yaw) = f(4) {
                // Controller.Rotation (Actor +0xD0): pitch, yaw, roll in 65536 units.
                let u = (yaw / 360.0 * 65536.0).round() as i32;
                c.write(pc + 0xd4, &u.to_le_bytes())?;
            }
            let mut b = Vec::new();
            for v in [x, y, z] {
                b.extend(v.to_le_bytes());
            }
            c.write(pawn + 0xc4, &b)?;
            c.key("W", true)?;
            c.wait_ticks(1)?;
            c.key("W", false)?;
            c.wait_ticks(60)?;
            let r = c.read(pawn + 0xc4, 12)?;
            let v: Vec<f32> = r.chunks(4).map(|w| f32::from_le_bytes(w.try_into().unwrap())).collect();
            println!("at {:.3} {:.3} {:.3}", v[0], v[1], v[2]);
        }
        "key" => {
            // key <EngineKeyName> [down|up|tap]
            let k = args.get(1).context("key name")?;
            let mut c = connect()?;
            let handled = match args.get(2).map(String::as_str).unwrap_or("tap") {
                "down" => c.key(k, true)?,
                "up" => c.key(k, false)?,
                _ => {
                    let h = c.key(k, true)?;
                    c.wait_ticks(3)?;
                    c.key(k, false)? || h
                }
            };
            println!("handled {handled}");
        }
        "axes" => {
            // axes <forward> <strafe> <turn> <lookup> <frames>
            let f = |i: usize| args.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            connect()?.input_axes(f(1), f(2), f(3), f(4), f(5) as u32)?;
        }
        "peek" => {
            // peek <pc|pawn|engine|input|ADDR> [+OFF | * ]... LEN   (`*` dereferences)
            let mut c = connect()?;
            let st = c.status()?;
            let get = |k: &str| st.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).and_then(|v| parse_u32(v).ok()).unwrap_or(0);
            let base = args.get(1).context("base")?;
            let mut a = match base.as_str() {
                "pc" => get("local_pc"),
                "pawn" => get("pawn"),
                "engine" => get("gengine"),
                "input" => get("player_input"),
                s => parse_u32(s)?,
            };
            let n = args.len();
            for t in &args[2..n - 1] {
                if t == "*" {
                    a = c.read_u32(a)?;
                } else if let Some(o) = t.strip_prefix('+') {
                    a = a.wrapping_add(parse_u32(o)?);
                }
            }
            let len = parse_u32(&args[n - 1])?;
            let b = c.read(a, len)?;
            println!("{a:#010x}");
            for (i, ch) in b.chunks(16).enumerate() {
                let hex: Vec<String> = ch.iter().map(|x| format!("{x:02x}")).collect();
                let f: Vec<String> = ch.chunks(4).filter(|w| w.len() == 4).map(|w| format!("{:.3}", f32::from_le_bytes(w.try_into().unwrap()))).collect();
                println!("+{:04x} {}  | {}", i * 16, hex.join(" "), f.join(" "));
            }
        }
        "call" => {
            // call <pc|pawn|ADDR> <Class.Function> [f:1.5 | i:3 | x:hex]...  (parms in order)
            let mut c = connect()?;
            let st = c.status()?;
            let get = |k: &str| st.lines().find_map(|l| l.strip_prefix(&format!("{k}="))).and_then(|v| parse_u32(v).ok()).unwrap_or(0);
            let obj = match args.get(1).context("object")?.as_str() {
                "pc" => get("local_pc"),
                "pawn" => get("pawn"),
                s => parse_u32(s)?,
            };
            let mut parms = Vec::new();
            for a in &args[3..] {
                if let Some(v) = a.strip_prefix("f:") {
                    parms.extend(v.parse::<f32>()?.to_le_bytes());
                } else if let Some(v) = a.strip_prefix("i:") {
                    parms.extend(v.parse::<i32>()?.to_le_bytes());
                } else if let Some(h) = a.strip_prefix("x:") {
                    for i in 0..h.len() / 2 {
                        parms.push(u8::from_str_radix(&h[i * 2..i * 2 + 2], 16)?);
                    }
                }
            }
            let n = parms.len().max(16);
            let r = c.call(obj, args.get(2).context("function")?, &parms)?;
            let hex: Vec<String> = r[..n.min(r.len()) + 16].iter().map(|x| format!("{x:02x}")).collect();
            println!("{}", hex.join(" "));
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

    /// End the session's game: only if that pid is still a `Dishonored.exe`, and wait for it
    /// to exit (a new game refuses to start while the old one lives).
    pub fn kill(pid: u32) -> Result<()> {
        use windows_sys::Win32::System::Threading::{PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW};
        unsafe {
            let h = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                bail!("process {pid} not found");
            }
            let mut buf = [0u16; 520];
            let mut n = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n) != 0;
            let name = String::from_utf16_lossy(&buf[..n as usize]);
            if !ok || !name.rsplit('\\').next().is_some_and(|f| f.eq_ignore_ascii_case("Dishonored.exe")) {
                CloseHandle(h);
                bail!("process {pid} is not the session's Dishonored.exe ({name})");
            }
            TerminateProcess(h, 0);
            WaitForSingleObject(h, 30_000);
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
