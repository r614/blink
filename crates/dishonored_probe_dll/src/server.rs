//! The probe's named-pipe server: one client at a time, local clients only, DACL granting
//! GENERIC_ALL to the current user and nobody else.
use crate::{d3d, engine, hook, input, mem, native};
use dishonored_probe::{Reader, op, put_str, put_u32};
use std::sync::atomic::Ordering::Relaxed;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};

struct Pipe(HANDLE);
unsafe impl Send for Pipe {}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// SDDL "D:P(A;;GA;;;<user SID>)": protected DACL, full access for the current user only.
unsafe fn current_user_sd() -> Option<PSECURITY_DESCRIPTOR> {
    unsafe {
        let mut tok: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
            return None;
        }
        let mut buf = vec![0u8; 512];
        let mut len = 0u32;
        let ok = GetTokenInformation(tok, TokenUser, buf.as_mut_ptr().cast(), buf.len() as u32, &mut len);
        CloseHandle(tok);
        if ok == 0 {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut sid_str: *mut u16 = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid_str) == 0 {
            return None;
        }
        let mut n = 0;
        while *sid_str.add(n) != 0 {
            n += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(sid_str, n));
        LocalFree(sid_str.cast());
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) == 0 {
            return None;
        }
        Some(sd)
    }
}

/// Up to this many clients at once (one thread each).
const MAX_CLIENTS: u32 = 4;

struct Sd(PSECURITY_DESCRIPTOR);
unsafe impl Send for Sd {}

fn create_instance(name: &[u16], sd: &Sd, first: bool) -> Option<Pipe> {
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: 0 };
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 },
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            MAX_CLIENTS,
            1 << 20,
            1 << 20,
            0,
            &sa,
        )
    };
    (h != INVALID_HANDLE_VALUE).then_some(Pipe(h))
}

pub fn start(token: &str) -> bool {
    let name = wide(&format!("{}{}_{}", dishonored_probe::PIPE_PREFIX, unsafe { GetCurrentProcessId() }, token));
    let Some(sd) = (unsafe { current_user_sd() }) else { return false };
    let sd = Sd(sd);
    // The first instance is created here so a name clash fails the init.
    let Some(first) = create_instance(&name, &sd, true) else {
        unsafe { LocalFree(sd.0) };
        return false;
    };
    std::thread::spawn(move || {
        let mut next = Some(first);
        loop {
            let pipe = match next.take().or_else(|| create_instance(&name, &sd, false)) {
                Some(p) => p,
                None => {
                    // All instances busy: wait for one to free up.
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    continue;
                }
            };
            let ok = unsafe { ConnectNamedPipe(pipe.0, std::ptr::null_mut()) };
            // ERROR_PIPE_CONNECTED (535): a client connected between create and connect.
            if ok == 0 && unsafe { windows_sys::Win32::Foundation::GetLastError() } != 535 {
                unsafe { CloseHandle(pipe.0) };
                continue;
            }
            std::thread::spawn(move || serve(pipe));
        }
    });
    true
}

fn read_exact(h: HANDLE, buf: &mut [u8]) -> bool {
    let mut got = 0usize;
    while got < buf.len() {
        let mut n = 0u32;
        let ok = unsafe { ReadFile(h, buf[got..].as_mut_ptr(), (buf.len() - got) as u32, &mut n, std::ptr::null_mut()) };
        if ok == 0 || n == 0 {
            return false;
        }
        got += n as usize;
    }
    true
}

fn write_all(h: HANDLE, buf: &[u8]) -> bool {
    let mut put = 0usize;
    while put < buf.len() {
        let mut n = 0u32;
        let ok = unsafe { WriteFile(h, buf[put..].as_ptr(), (buf.len() - put) as u32, &mut n, std::ptr::null_mut()) };
        if ok == 0 {
            return false;
        }
        put += n as usize;
    }
    true
}

/// Serve one connected client until it disconnects, then close the instance.
fn serve(pipe: Pipe) {
    let h = pipe.0;
    {
        loop {
            let mut len = [0u8; 4];
            if !read_exact(h, &mut len) {
                break;
            }
            let n = u32::from_le_bytes(len) as usize;
            if n == 0 || n > 64 << 20 {
                break;
            }
            let mut body = vec![0u8; n];
            if !read_exact(h, &mut body) {
                break;
            }
            crate::tick::touch();
            let resp = match handle(body[0], &body[1..]) {
                Ok(p) => {
                    let mut r = vec![0u8];
                    r.extend(p);
                    r
                }
                Err(e) => {
                    let mut r = vec![1u8];
                    r.extend(e.as_bytes());
                    r
                }
            };
            let mut frame = Vec::with_capacity(resp.len() + 4);
            put_u32(&mut frame, resp.len() as u32);
            frame.extend(resp);
            if !write_all(h, &frame) {
                break;
            }
            unsafe { FlushFileBuffers(h) };
        }
        unsafe {
            DisconnectNamedPipe(h);
            CloseHandle(h);
        }
    }
}

fn io<T>(r: std::io::Result<T>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

/// UTF-16 FString {Data, Num, Max} for `s`, kept alive by the returned buffer.
fn fstring(s: &str) -> (Vec<u16>, [u32; 3]) {
    let w: Vec<u16> = s.encode_utf16().chain(Some(0)).collect();
    let arr = [w.as_ptr() as u32, w.len() as u32, w.len() as u32];
    (w, arr)
}

fn read_fstring(at: u32) -> String {
    let Some(a) = mem::read::<[u32; 3]>(at) else { return String::new() };
    if a[0] == 0 || a[1] == 0 || a[1] > 1 << 20 {
        return String::new();
    }
    let b = mem::read_bytes(a[0], a[1] as usize * 2).unwrap_or_default();
    let w: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    String::from_utf16_lossy(&w)
}

fn handle(opc: u8, p: &[u8]) -> Result<Vec<u8>, String> {
    let mut rd = Reader::new(p);
    let mut out = Vec::new();
    match opc {
        op::PING => {
            put_u32(&mut out, dishonored_probe::PROTOCOL_VERSION);
            put_str(&mut out, concat!("dishonored_probe_dll ", env!("CARGO_PKG_VERSION")));
        }
        op::READ => {
            let (a, n) = (io(rd.u32())?, io(rd.u32())?);
            if n > 64 << 20 {
                return Err("read too large".into());
            }
            out = mem::read_bytes(a, n as usize).ok_or_else(|| format!("unreadable {a:#x}+{n:#x}"))?;
        }
        op::WRITE => {
            let a = io(rd.u32())?;
            if !mem::write_bytes(a, rd.rest()) {
                return Err(format!("unwritable {a:#x}"));
            }
        }
        op::STATUS => {
            engine::discover();
            let pc = engine::local_pc();
            let pawn = mem::u32(pc + engine::CONTROLLER_PAWN).unwrap_or(0);
            let s = format!(
                "process_event_calls={}\nticks={}\ngobjects={:#x}\ngnames={:#x}\nname_text_off={:#x}\ngame_thread={}\npresent_thread={}\ngengine={:#x}\nlocal_pc={:#x}\nlocal_pc_class={}\npawn={:#x}\npawn_class={}\nplayer_input={:#x}\nd3d_state={}\nd3d_devices={}\npresents={}\nps_sets={}\nps_sets_known={}\ndraws={}\niat_patched={}\nfake_focus={}\nmap={}\n",
                hook::CALLS.load(Relaxed),
                hook::TICKS.load(Relaxed),
                engine::GOBJECTS.load(Relaxed),
                engine::GNAMES.load(Relaxed),
                engine::NAME_TEXT_OFF.load(Relaxed),
                hook::GAME_THREAD.load(Relaxed),
                hook::PRESENT_THREAD.load(Relaxed),
                engine::GENGINE.load(Relaxed),
                pc,
                engine::class_name(pc).unwrap_or_default(),
                pawn,
                engine::class_name(pawn).unwrap_or_default(),
                mem::u32(pc + engine::PC_PLAYER_INPUT).unwrap_or(0),
                d3d::STATE.load(Relaxed),
                d3d::DEVICES.load(Relaxed),
                d3d::PRESENTS.load(Relaxed),
                d3d::PS_SETS.load(Relaxed),
                d3d::PS_SETS_KNOWN.load(Relaxed),
                d3d::DRAWS.load(Relaxed),
                crate::iat::PATCHED.load(Relaxed),
                crate::iat::FAKE_FOCUS.load(Relaxed),
                // The level's package: WorldInfo's outermost object.
                mem::u32(pc + engine::ACTOR_WORLDINFO)
                    .map(|mut o| {
                        while let Some(n) = mem::u32(o + engine::OBJ_OUTER).filter(|&n| n != 0) {
                            o = n;
                        }
                        engine::obj_name(o).unwrap_or_default()
                    })
                    .unwrap_or_default(),
            );
            let s = format!(
                "{s}game_tick={}\ntick_hook={}\nlockstep={}\nparked={}\ntrace_dropped={}\n",
                crate::tick::GAME_TICK.load(Relaxed),
                crate::tick::INSTALLED.load(Relaxed),
                crate::tick::LOCKSTEP.load(Relaxed),
                crate::tick::PARKED.load(Relaxed),
                native::TRACE.lock().map(|t| t.dropped).unwrap_or(0),
            );
            out = s.into_bytes();
        }
        op::FIND => {
            let (name, class) = (io(rd.str())?, io(rd.str())?);
            if !engine::discover() {
                return Err("object arrays not found yet".into());
            }
            let v = engine::find(&name, &class);
            put_u32(&mut out, v.len() as u32);
            for a in v {
                put_u32(&mut out, a);
            }
        }
        op::NAME_OF => {
            let a = io(rd.u32())?;
            put_str(&mut out, &engine::full_name(a).unwrap_or_default());
        }
        op::CONSOLE => {
            let cmd = io(rd.str())?;
            let func = engine::find_function("PlayerController.ConsoleCommand");
            if func == 0 {
                return Err("PlayerController.ConsoleCommand not found".into());
            }
            let r = hook::on_game_thread(
                move || {
                    let pc = engine::local_pc();
                    if pc == 0 {
                        return Err("no local PlayerController".to_string());
                    }
                    // (string Command, bool bWriteToLog) -> string at +0x10.
                    let (_keep, fs) = fstring(&cmd);
                    let mut parms = [0u32; 16];
                    parms[..3].copy_from_slice(&fs);
                    parms[3] = 1;
                    unsafe { hook::process_event_virtual(pc, func, parms.as_mut_ptr() as u32) };
                    Ok(read_fstring(parms.as_ptr() as u32 + 0x10))
                },
                10_000,
            )??;
            put_str(&mut out, &r);
        }
        op::CALL => {
            let obj = io(rd.u32())?;
            let fname = io(rd.str())?;
            let parms = rd.rest().to_vec();
            let func = engine::find_function(&fname);
            if func == 0 {
                return Err(format!("function {fname} not found"));
            }
            out = hook::on_game_thread(
                move || {
                    let mut buf = parms;
                    buf.resize(buf.len().max(256), 0);
                    unsafe { hook::process_event_virtual(obj, func, buf.as_mut_ptr() as u32) };
                    buf
                },
                10_000,
            )?;
        }
        op::SAMPLE_START => {
            let max = io(rd.u32())?;
            let mut r = hook::RECORDER.lock().map_err(|_| "poisoned")?;
            r.samples.clear();
            r.max = max as usize;
            r.enabled = max > 0;
        }
        op::SAMPLE_FETCH => {
            let samples = std::mem::take(&mut hook::RECORDER.lock().map_err(|_| "poisoned")?.samples);
            put_u32(&mut out, samples.len() as u32);
            for s in &samples {
                s.encode(&mut out);
            }
        }
        op::WATCH => {
            let n = io(rd.u32())?;
            let mut w = Vec::new();
            for _ in 0..n {
                w.push((io(rd.u32())?, io(rd.u32())?));
            }
            hook::RECORDER.lock().map_err(|_| "poisoned")?.watch = w;
        }
        op::INPUT_AXES => {
            let _flags = io(rd.u8())?;
            let axes = [io(rd.f32())?, io(rd.f32())?, io(rd.f32())?, io(rd.f32())?];
            let frames = io(rd.u32())?;
            hook::set_input(axes, frames);
        }
        op::KEY => {
            let key = io(rd.str())?;
            let event = io(rd.u8())? as u32;
            let handled = input::input_key(&key, event)?;
            out.push(handled as u8);
        }
        op::WAIT_TICKS if crate::tick::INSTALLED.load(Relaxed) != 0 => {
            // World ticks from the boundary hook (in lockstep: released and parked again).
            let n = io(rd.u32())?;
            crate::tick::step(n, 120_000)?;
        }
        op::WAIT_TICKS => {
            let n = io(rd.u32())?;
            let start = hook::TICKS.load(Relaxed);
            let t0 = std::time::Instant::now();
            while hook::TICKS.load(Relaxed).wrapping_sub(start) < n {
                if t0.elapsed().as_secs() > 120 {
                    return Err("timed out waiting for ticks".into());
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        op::DRAW_SKIP => {
            let n = io(rd.u32())?;
            let mut v = Vec::new();
            for _ in 0..n.min(4096) {
                v.push(io(rd.u64())?);
            }
            let mut g = d3d::PS.lock().map_err(|_| "poisoned")?;
            g.as_mut().ok_or("d3d hooks not installed")?.skip = v;
        }
        op::DRAW_CENSUS => {
            let frames = io(rd.u32())?.clamp(1, 600);
            {
                let mut g = d3d::PS.lock().map_err(|_| "poisoned")?;
                let ps = g.as_mut().ok_or("d3d hooks not installed")?;
                ps.census.clear();
                ps.census_on = true;
            }
            let start = d3d::PRESENTS.load(Relaxed);
            let t0 = std::time::Instant::now();
            while d3d::PRESENTS.load(Relaxed).wrapping_sub(start) < frames && t0.elapsed().as_secs() < 60 {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let mut g = d3d::PS.lock().map_err(|_| "poisoned")?;
            let ps = g.as_mut().ok_or("d3d hooks not installed")?;
            ps.census_on = false;
            let mut v: Vec<(u64, (u32, u32))> = ps.census.drain().collect();
            v.sort_by_key(|e| std::cmp::Reverse(e.1.0));
            put_u32(&mut out, v.len() as u32);
            for (h, (d, p)) in v {
                out.extend(h.to_le_bytes());
                put_u32(&mut out, d);
                put_u32(&mut out, p);
            }
        }
        op::CAPTURE => {
            let mut c = d3d::CAPTURE.lock().map_err(|_| "poisoned")?;
            c.pending = true;
            c.result = None;
            let (mut c, _) = d3d::CAPTURE_CV.wait_timeout_while(c, std::time::Duration::from_secs(10), |c| c.result.is_none()).map_err(|_| "poisoned")?;
            c.pending = false;
            out = c.result.take().ok_or("no Present within 10 s")??;
        }
        op::SHADERS => {
            let g = d3d::PS.lock().map_err(|_| "poisoned")?;
            let ps = g.as_ref().ok_or("d3d hooks not installed")?;
            let mut v: Vec<&d3d::Shader> = ps.shaders.values().collect();
            v.sort_by_key(|s| s.id);
            put_u32(&mut out, v.len() as u32);
            for s in v {
                put_u32(&mut out, s.id);
                out.extend(s.hash.to_le_bytes());
                put_u32(&mut out, s.size);
                put_str(&mut out, &s.ctab);
            }
        }
        op::PS_CONSTANTS => {
            let hash = io(rd.u64())?;
            let draws = io(rd.u32())?;
            let filter = if rd.pos < rd.data.len() { io(rd.str())? } else { String::new() };
            {
                let mut g = d3d::PS.lock().map_err(|_| "poisoned")?;
                let ps = g.as_mut().ok_or("d3d hooks not installed")?;
                ps.want_hash = hash;
                ps.want_filter = filter;
                ps.want_draws = draws;
                ps.got.clear();
                ps.got_tex.clear();
            }
            let t0 = std::time::Instant::now();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(20));
                let mut g = d3d::PS.lock().map_err(|_| "poisoned")?;
                let ps = g.as_mut().ok_or("d3d hooks not installed")?;
                if ps.want_draws == 0 || t0.elapsed().as_secs() >= 5 {
                    ps.want_draws = 0;
                    let got = std::mem::take(&mut ps.got);
                    let got_tex = std::mem::take(&mut ps.got_tex);
                    put_u32(&mut out, got.len() as u32);
                    for (frame, hash, regs) in got {
                        put_u32(&mut out, frame);
                        out.extend(hash.to_le_bytes());
                        for r in regs {
                            for v in r {
                                out.extend(v.to_le_bytes());
                            }
                        }
                    }
                    // After the draws: the textures bound at each (a new client reads them).
                    for tx in got_tex {
                        put_u32(&mut out, tx.len() as u32);
                        for t in tx {
                            for v in [t.stage, t.ptr, t.ty, t.format, t.usage, t.pool, t.width, t.height, t.levels] {
                                put_u32(&mut out, v);
                            }
                            out.extend(t.hash.to_le_bytes());
                        }
                    }
                    break;
                }
            }
        }
        op::WATCH_ABS => {
            let n = io(rd.u32())?;
            let mut w = Vec::new();
            for _ in 0..n {
                w.push((io(rd.u32())?, io(rd.u32())?));
            }
            hook::RECORDER.lock().map_err(|_| "poisoned")?.watch_abs = w;
        }
        op::ALLOC => {
            let n = io(rd.u32())?;
            let a = native::alloc(n);
            if a == 0 {
                return Err("alloc failed".into());
            }
            put_u32(&mut out, a);
        }
        op::LINE_CHECKS => {
            let n = io(rd.u32())?;
            if n > 4096 {
                return Err("too many line checks".into());
            }
            let q = (0..n).map(|_| io(dishonored_probe::LineQuery::decode(&mut rd))).collect::<Result<Vec<_>, String>>()?;
            let r = native::line_checks(q)?;
            put_u32(&mut out, r.len() as u32);
            for x in &r {
                x.encode(&mut out);
            }
        }
        op::NATIVE_CALL => {
            let (func, this) = (io(rd.u32())?, io(rd.u32())?);
            let flags = io(rd.u8())?;
            let n = io(rd.u32())?;
            if n > 32 {
                return Err("too many arguments".into());
            }
            let args = (0..n).map(|_| io(rd.u32())).collect::<Result<Vec<u32>, String>>()?;
            let r = native::native_call(func, this, args, flags & 1 != 0)?;
            put_u32(&mut out, r.eax);
            out.extend(r.st0.to_le_bytes());
        }
        op::HOOK_SET => {
            let addr = io(rd.u32())?;
            let (plen, nargs) = (io(rd.u8())?, io(rd.u8())?);
            let max = io(rd.u32())?;
            let n = io(rd.u32())?;
            let mut derefs = Vec::new();
            for _ in 0..n {
                let src = match io(rd.u8())? {
                    0xff => native::DerefSrc::Ecx,
                    0xfe => native::DerefSrc::Abs,
                    i => native::DerefSrc::Arg(i),
                };
                derefs.push(native::Deref { src, pre: io(rd.u32())?, off: io(rd.u32())?, len: io(rd.u32())?.min(4096) });
            }
            put_u32(&mut out, native::hook_set(addr, plen, nargs.min(16), derefs, max)?);
        }
        op::HOOK_FETCH => {
            let recs = native::hook_fetch(io(rd.u32())?)?;
            put_u32(&mut out, recs.len() as u32);
            for r in recs {
                out.extend(r.time.to_le_bytes());
                put_u32(&mut out, r.ecx);
                put_u32(&mut out, r.ret);
                put_u32(&mut out, r.args.len() as u32);
                for a in r.args {
                    put_u32(&mut out, a);
                }
                dishonored_probe::put_bytes(&mut out, &r.data);
            }
        }
        op::EVENT_TRACE => {
            let (max, parm_len, n) = (io(rd.u32())?, io(rd.u32())?, io(rd.u32())?);
            let mut funcs = (0..n).map(|_| io(rd.u32())).collect::<Result<Vec<u32>, String>>()?;
            funcs.sort_unstable();
            let mut t = native::TRACE.lock().map_err(|_| "poisoned")?;
            t.funcs = funcs;
            t.classes = None;
            t.parm_len = parm_len.min(1024);
            t.max = max as usize;
            t.records.clear();
            t.dropped = 0;
            native::TRACE_ON.store((max > 0 && !t.funcs.is_empty()) as u32, Relaxed);
        }
        op::EVENT_FILTER => {
            let (max, parm_len) = (io(rd.u32())?, io(rd.u32())?);
            let nf = io(rd.u32())?.min(1024);
            let subs = (0..nf).map(|_| io(rd.str())).collect::<Result<Vec<String>, String>>()?;
            let nc = io(rd.u32())?.min(1024);
            let classes = (0..nc).map(|_| io(rd.str())).collect::<Result<Vec<String>, String>>()?;
            let na = io(rd.u32())?.min(1 << 20);
            let mut funcs = (0..na).map(|_| io(rd.u32())).collect::<Result<Vec<u32>, String>>()?;
            funcs.extend(native::functions_matching(&subs));
            funcs.sort_unstable();
            funcs.dedup();
            let class_idx: Vec<u32> = classes.iter().filter_map(|c| engine::name_index(c)).collect();
            if class_idx.len() != classes.len() {
                return Err(format!("unknown class name among {classes:?}"));
            }
            let mut t = native::TRACE.lock().map_err(|_| "poisoned")?;
            put_u32(&mut out, funcs.len() as u32);
            let on = max > 0 && (!funcs.is_empty() || !class_idx.is_empty());
            t.funcs = funcs;
            t.classes = (!class_idx.is_empty()).then(|| crate::reflect::IsACache::new(class_idx));
            t.parm_len = parm_len.min(1024);
            t.max = max as usize;
            t.records.clear();
            t.dropped = 0;
            native::TRACE_ON.store(on as u32, Relaxed);
        }
        op::TICK => {
            put_u32(&mut out, crate::tick::GAME_TICK.load(Relaxed));
            out.push(crate::tick::INSTALLED.load(Relaxed) as u8);
            out.push(crate::tick::LOCKSTEP.load(Relaxed) as u8);
            out.push(crate::tick::PARKED.load(Relaxed) as u8);
            put_u32(&mut out, d3d::PRESENTS.load(Relaxed));
        }
        op::LOCKSTEP => {
            let on = io(rd.u8())? != 0;
            let timeout = io(rd.u32())?.max(1000) as u64;
            if crate::tick::INSTALLED.load(Relaxed) == 0 {
                return Err("the tick boundary hook is not installed".into());
            }
            let t = if on {
                crate::tick::lockstep_on(timeout)?
            } else {
                crate::tick::lockstep_off();
                crate::tick::GAME_TICK.load(Relaxed)
            };
            put_u32(&mut out, t);
        }
        op::STEP => {
            let n = io(rd.u32())?;
            let timeout = io(rd.u32())?.max(1000) as u64;
            if crate::tick::INSTALLED.load(Relaxed) == 0 {
                return Err("the tick boundary hook is not installed".into());
            }
            put_u32(&mut out, crate::tick::step(n, timeout)?);
        }
        op::READS => {
            let n = io(rd.u32())?;
            if n > 1 << 16 {
                return Err("too many reads".into());
            }
            for _ in 0..n {
                let mut a = io(rd.u32())?;
                let k = io(rd.u8())?.max(1) as usize;
                let offs = (0..k).map(|_| io(rd.u32())).collect::<Result<Vec<u32>, String>>()?;
                let len = io(rd.u32())?.min(1 << 20) as usize;
                let mut ok = a != 0;
                for o in &offs[..k - 1] {
                    if !ok {
                        break;
                    }
                    match mem::u32(a.wrapping_add(*o)) {
                        Some(v) if v != 0 => a = v,
                        _ => ok = false,
                    }
                }
                let b = if ok { mem::read_bytes(a.wrapping_add(offs[k - 1]), len) } else { None };
                out.push(b.is_some() as u8);
                out.extend(b.unwrap_or_else(|| vec![0; len]));
            }
        }
        op::FIELDS => {
            let st = io(rd.u32())?;
            if engine::obj_name(st).is_none() {
                return Err(format!("{st:#x} is not an object"));
            }
            put_u32(&mut out, crate::reflect::super_of(st));
            put_u32(&mut out, crate::reflect::struct_size(st));
            put_str(&mut out, &engine::obj_name(st).unwrap_or_default());
            put_str(&mut out, &engine::class_name(st).unwrap_or_default());
            let f = crate::reflect::fields(st);
            put_u32(&mut out, f.len() as u32);
            for x in f {
                put_u32(&mut out, x.addr);
                put_str(&mut out, &x.name);
                put_str(&mut out, &x.class);
                put_u32(&mut out, x.offset);
                put_u32(&mut out, x.element_size);
                put_u32(&mut out, x.array_dim);
                out.extend(x.flags.to_le_bytes());
                put_u32(&mut out, x.aux);
                put_u32(&mut out, x.aux2);
            }
        }
        op::ENUM_NAMES => {
            let names = crate::reflect::enum_names(io(rd.u32())?);
            put_u32(&mut out, names.len() as u32);
            for n in names {
                put_str(&mut out, &n);
            }
        }
        op::SPAWN_WATCH => {
            crate::tick::spawn_watch(io(rd.u32())?)?;
        }
        op::SPAWN_FETCH => {
            let recs = std::mem::take(&mut crate::tick::SPAWNS.lock().map_err(|_| "poisoned")?.records);
            put_u32(&mut out, recs.len() as u32);
            for (t, o, c) in recs {
                put_u32(&mut out, t);
                put_u32(&mut out, o);
                put_u32(&mut out, c);
            }
        }
        op::CALL_FUNC => {
            let obj = io(rd.u32())?;
            let func = io(rd.u32())?;
            let parms = rd.rest().to_vec();
            if obj == 0 || engine::class_name(func).as_deref() != Some("Function") {
                return Err(format!("bad object {obj:#x} or function {func:#x}"));
            }
            out = hook::on_game_thread(
                move || {
                    let mut buf = parms;
                    buf.resize(buf.len().max(256), 0);
                    unsafe { hook::process_event_virtual(obj, func, buf.as_mut_ptr() as u32) };
                    buf
                },
                30_000,
            )?;
        }
        op::SCRIPT_START => {
            use crate::script::{self, Act};
            if crate::tick::LOCKSTEP.load(Relaxed) == 0 || crate::tick::PARKED.load(Relaxed) == 0 {
                return Err("a tick script needs the game parked in lockstep".into());
            }
            if script::STATE.load(Relaxed) == script::RUNNING || script::STATE.load(Relaxed) == script::PENDING {
                return Err("a tick script is already running".into());
            }
            let ticks = io(rd.u32())?;
            let nr = io(rd.u32())?.min(4096);
            let mut reads = Vec::new();
            for _ in 0..nr {
                let base = io(rd.u32())?;
                let k = io(rd.u8())?.max(1) as usize;
                let offs = (0..k).map(|_| io(rd.u32())).collect::<Result<Vec<u32>, String>>()?;
                reads.push((base, offs, io(rd.u32())?.min(4096)));
            }
            let na = io(rd.u32())?;
            let mut actions = Vec::new();
            let console = engine::find_function("PlayerController.ConsoleCommand");
            for _ in 0..na {
                let t = io(rd.u32())?;
                let a = match io(rd.u8())? {
                    0 => {
                        let key = io(rd.str())?;
                        let ev = io(rd.u8())? as u32;
                        Act::Key(engine::name_index(&key).ok_or_else(|| format!("no FName {key}"))?, ev)
                    }
                    1 => {
                        let addr = io(rd.u32())?;
                        Act::Write(addr, io(rd.bytes())?.to_vec())
                    }
                    2 => {
                        if console == 0 {
                            return Err("PlayerController.ConsoleCommand not found".into());
                        }
                        Act::Console(console, io(rd.str())?)
                    }
                    3 => {
                        let func = io(rd.u32())?;
                        let this = io(rd.u32())?;
                        let n = io(rd.u8())? as usize;
                        let args = (0..n).map(|_| io(rd.u32())).collect::<Result<Vec<u32>, String>>()?;
                        Act::Native(func, this, args)
                    }
                    k => return Err(format!("unknown script action {k}")),
                };
                actions.push((t, a));
            }
            actions.sort_by_key(|a| a.0);
            *script::SCRIPT.lock().map_err(|_| "poisoned")? = Some(script::Script { ticks, reads, actions, next: 0, base: 0, frames: Vec::new(), errors: Vec::new() });
            script::STATE.store(script::PENDING, Relaxed);
        }
        op::SCRIPT_WAIT => {
            use crate::script;
            let timeout = io(rd.u32())? as u64;
            let t0 = std::time::Instant::now();
            loop {
                crate::tick::touch();
                let st = script::STATE.load(Relaxed);
                if st == script::IDLE {
                    return Err("no tick script".into());
                }
                if st == script::DONE && crate::tick::PARKED.load(Relaxed) != 0 {
                    break;
                }
                if crate::tick::LOCKSTEP.load(Relaxed) == 0 {
                    script::STATE.store(script::IDLE, Relaxed);
                    return Err("lockstep was released during the script".into());
                }
                if t0.elapsed().as_millis() as u64 > timeout {
                    return Err(format!("tick script not done within {timeout} ms (tick {})", crate::tick::GAME_TICK.load(Relaxed)));
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let s = script::SCRIPT.lock().map_err(|_| "poisoned")?.take().ok_or("no script")?;
            script::STATE.store(script::IDLE, Relaxed);
            put_u32(&mut out, s.base);
            dishonored_probe::put_bytes(&mut out, &s.frames);
            put_str(&mut out, &s.errors.join("\n"));
        }
        op::SHADER_DUMP => {
            let since = io(rd.u32())?;
            let max = rd.u32().unwrap_or(u32::MAX) as usize;
            let v = d3d::SHADER_CODE.lock().map_err(|_| "poisoned")?;
            let sel: Vec<&d3d::StoredShader> = v.iter().filter(|s| s.id >= since).take(max).collect();
            put_u32(&mut out, sel.len() as u32);
            for s in sel {
                put_u32(&mut out, s.id);
                out.push(s.kind);
                out.extend(s.hash.to_le_bytes());
                put_u32(&mut out, s.tick);
                put_str(&mut out, &s.ctab);
                dishonored_probe::put_bytes(&mut out, &s.code);
            }
        }
        op::NAME_INDEX => {
            let s = io(rd.str())?;
            let (i, n) = match engine::name_index(&s) {
                Some(i) => (i, 0),
                None => {
                    let (b, k) = s.rsplit_once('_').ok_or_else(|| format!("no FName {s}"))?;
                    let k: u32 = k.parse().map_err(|_| format!("no FName {s}"))?;
                    (engine::name_index(b).ok_or_else(|| format!("no FName {s}"))?, k + 1)
                }
            };
            put_u32(&mut out, i);
            put_u32(&mut out, n);
        }
        op::NAMES_OF => {
            let n = io(rd.u32())?.min(1 << 16);
            for _ in 0..n {
                let a = io(rd.u32())?;
                put_str(&mut out, &engine::full_name(a).unwrap_or_default());
            }
        }
        op::EVENT_FETCH => {
            let recs = std::mem::take(&mut native::TRACE.lock().map_err(|_| "poisoned")?.records);
            put_u32(&mut out, recs.len() as u32);
            for r in recs {
                out.extend(r.time.to_le_bytes());
                put_u32(&mut out, r.tick);
                put_u32(&mut out, r.this);
                put_u32(&mut out, r.func);
                dishonored_probe::put_bytes(&mut out, &r.parms);
            }
        }
        _ => return Err(format!("unknown op {opc}")),
    }
    Ok(out)
}
