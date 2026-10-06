//! The probe's named-pipe server: one client at a time, local clients only, DACL granting
//! GENERIC_ALL to the current user and nobody else.
use crate::{d3d, engine, hook, input, mem};
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

pub fn start(token: &str) -> bool {
    let name = format!("{}{}_{}", dishonored_probe::PIPE_PREFIX, unsafe { GetCurrentProcessId() }, token);
    let Some(sd) = (unsafe { current_user_sd() }) else { return false };
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
    let h = unsafe {
        CreateNamedPipeW(
            wide(&name).as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            1 << 20,
            1 << 20,
            0,
            &sa,
        )
    };
    unsafe { LocalFree(sd) };
    if h == INVALID_HANDLE_VALUE {
        return false;
    }
    let pipe = Pipe(h);
    std::thread::spawn(move || serve(pipe));
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

fn serve(pipe: Pipe) {
    let h = pipe.0;
    loop {
        let ok = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) };
        // ERROR_PIPE_CONNECTED (535) means a client connected between create and connect.
        if ok == 0 && unsafe { windows_sys::Win32::Foundation::GetLastError() } != 535 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
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
        unsafe { DisconnectNamedPipe(h) };
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
                "process_event_calls={}\nticks={}\ngobjects={:#x}\ngnames={:#x}\nname_text_off={:#x}\ngame_thread={}\npresent_thread={}\ngengine={:#x}\nlocal_pc={:#x}\nlocal_pc_class={}\npawn={:#x}\npawn_class={}\nplayer_input={:#x}\nd3d_state={}\nd3d_devices={}\npresents={}\niat_patched={}\nfake_focus={}\nmap={}\n",
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
            {
                let mut g = d3d::PS.lock().map_err(|_| "poisoned")?;
                let ps = g.as_mut().ok_or("d3d hooks not installed")?;
                ps.want_hash = hash;
                ps.want_draws = draws;
                ps.got.clear();
            }
            let t0 = std::time::Instant::now();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(20));
                let mut g = d3d::PS.lock().map_err(|_| "poisoned")?;
                let ps = g.as_mut().ok_or("d3d hooks not installed")?;
                if ps.want_draws == 0 || t0.elapsed().as_secs() >= 5 {
                    ps.want_draws = 0;
                    let got = std::mem::take(&mut ps.got);
                    put_u32(&mut out, got.len() as u32);
                    for (frame, regs) in got {
                        put_u32(&mut out, frame);
                        for r in regs {
                            for v in r {
                                out.extend(v.to_le_bytes());
                            }
                        }
                    }
                    break;
                }
            }
        }
        _ => return Err(format!("unknown op {opc}")),
    }
    Ok(out)
}
