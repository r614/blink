//! Runtime probe of the original game: the wire protocol spoken over the probe's local named
//! pipe, and a blocking client for it.
//!
//! The server side is `dishonored_probe_dll`, a 32-bit DLL that `probe_launch` injects into a
//! `Dishonored.exe` it started itself. The pipe is named
//! `\\.\pipe\dishonored_probe_<pid>_<token>`, is local-only, and only the current user may open
//! it. See `research/dishonored/INSTRUMENTATION.md`.
//!
//! Framing: every message is `u32 length (LE)` followed by `length` bytes. A request body is
//! `u8 op` + op payload; a response body is `u8 status` (0 ok, 1 error with a UTF-8 message) +
//! payload. All integers are little endian. Addresses are 32-bit (the game is a 32-bit process).
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 5;
pub const PIPE_PREFIX: &str = r"\\.\pipe\dishonored_probe_";
/// Environment variable through which the launcher hands the session token to the DLL.
pub const TOKEN_ENV: &str = "DISHONORED_PROBE_TOKEN";
/// "0" turns off the DLL's focus faking (on by default: the engine pauses the world when its
/// window loses the focus, `bPauseOnLossOfFocus`).
pub const FAKE_FOCUS_ENV: &str = "DISHONORED_PROBE_FAKE_FOCUS";

/// Request opcodes.
pub mod op {
    /// -> u32 protocol version, then UTF-8 build string.
    pub const PING: u8 = 1;
    /// u32 addr, u32 len -> bytes (error if unreadable).
    pub const READ: u8 = 2;
    /// u32 addr, bytes -> () (game thread not required; uses WriteProcessMemory on itself).
    pub const WRITE: u8 = 3;
    /// -> UTF-8 `key=value` lines (hook state, engine globals, player objects, counters).
    pub const STATUS: u8 = 4;
    /// str name, str class ("" = any) -> u32 count, u32 addrs (live objects, defaults excluded).
    pub const FIND: u8 = 5;
    /// str command -> str result. Runs `PlayerController.ConsoleCommand` on the game thread.
    pub const CONSOLE: u8 = 6;
    /// u32 max samples kept (0 = stop) -> (). Clears the per-tick sample buffer.
    pub const SAMPLE_START: u8 = 7;
    /// -> u32 count, count * Sample (and the buffer is emptied).
    pub const SAMPLE_FETCH: u8 = 8;
    /// u8 flags (bit0 hold), f32 forward, f32 strafe, f32 turn, f32 lookup, u32 frames
    /// (0 = until changed) -> (). Written into the PlayerInput axes every tick.
    pub const INPUT_AXES: u8 = 9;
    /// -> u32 width, u32 height, u32 D3DFORMAT, u32 pitch, bytes. Back buffer at the next Present.
    pub const CAPTURE: u8 = 10;
    /// -> u32 count, per shader: u32 id, u64 fnv hash, u32 size, str CTAB names.
    pub const SHADERS: u8 = 11;
    /// u64 hash (0: match by filter), u32 draws, str CTAB filter -> u32 count, per draw:
    /// u32 frame, u64 shader hash, 224 * [f32;4] PS constants.
    pub const PS_CONSTANTS: u8 = 12;
    /// u32 object, str function ("Class.Func" or "Func"), bytes parms -> bytes parms after call.
    pub const CALL: u8 = 13;
    /// u32 object -> str full name ("Class Outer.Name").
    pub const NAME_OF: u8 = 14;
    /// u32 count, count * (u32 offset, u32 len) -> (). Extra pawn bytes recorded per sample.
    pub const WATCH: u8 = 15;
    /// u32 frames -> (). Blocks until that many frames (Presents) have run.
    pub const WAIT_TICKS: u8 = 16;
    /// str engine key name, u8 event (0 pressed, 1 released) -> u8 handled. Game thread,
    /// through `FViewportClient::InputKey`.
    pub const KEY: u8 = 17;
    /// u32 size -> u32 address of a zeroed scratch block (never freed).
    pub const ALLOC: u8 = 18;
    /// u32 function, u32 this (0: ecx not set), u8 flags (bit0: store the x87 top), u32 n,
    /// n * u32 stack arguments -> u32 eax, f32 st0. Game thread.
    pub const NATIVE_CALL: u8 = 19;
    /// u32 address, u8 prologue length, u8 stack args logged, u32 max records, u32 n derefs,
    /// n * (u8 source: arg index, 0xFF ecx, 0xFE absolute; u32 pre (u32::MAX: no hop), u32
    /// offset, u32 length) -> u32 hook id. An existing hook at the address is reconfigured and
    /// cleared (max 0 disables it).
    pub const HOOK_SET: u8 = 20;
    /// u32 hook id -> u32 count, per record: f32 time, u32 ecx, u32 return address, u32 n,
    /// n * u32 args, bytes (the derefs, concatenated).
    pub const HOOK_FETCH: u8 = 21;
    /// u32 max records (0: off), u32 parm bytes, u32 n, n * u32 UFunction addresses -> ().
    pub const EVENT_TRACE: u8 = 22;
    /// -> u32 count, per record: f32 time, u32 object, u32 function, bytes parms.
    pub const EVENT_FETCH: u8 = 23;
    /// Like WATCH, with absolute addresses (recorded after the pawn ranges).
    pub const WATCH_ABS: u8 = 24;
    /// u32 n, n * u64 pixel shader hashes -> (). Draws made with these shaders are skipped
    /// (rendering only; n = 0 restores every draw).
    pub const DRAW_SKIP: u8 = 25;
    /// u32 frames -> u32 count, per shader: u64 hash, u32 draws, u32 primitives, counted over
    /// the next `frames` Presents.
    pub const DRAW_CENSUS: u8 = 26;
    /// u32 n, n * LineQuery -> u32 n, n * LineResult. Runs the engine's own collision check
    /// (`UWorld::SingleLineCheck`, 0x0064E7A0, on `GWorld`) for each query on the game thread.
    pub const LINE_CHECKS: u8 = 27;
    /// -> u32 world tick, u8 tick hook installed, u8 lockstep, u8 parked, u32 presents.
    pub const TICK: u8 = 28;
    /// u8 on (1) / off (0), u32 timeout ms -> u32 world tick (on: the tick it parked at).
    pub const LOCKSTEP: u8 = 29;
    /// u32 n, u32 timeout ms -> u32 world tick. Lockstep: release n ticks and park again;
    /// otherwise wait until n world ticks have run.
    pub const STEP: u8 = 30;
    /// u32 n, n * (u32 base, u8 k >= 1, k * u32 offsets, u32 len) -> n * (u8 ok, len bytes).
    /// Each read follows a pointer chain: every offset but the last is added and dereferenced.
    pub const READS: u8 = 31;
    /// u32 struct -> u32 super, u32 size, str name, str class, u32 n, n * FieldInfo.
    pub const FIELDS: u8 = 32;
    /// u32 enum -> u32 n, n * str.
    pub const ENUM_NAMES: u8 = 33;
    /// u32 max, u32 parm bytes, u32 n, n * str function filters (substrings of `Outer.Name`),
    /// u32 m, m * str class names (is-a), u32 k, k * u32 function addresses -> u32 functions.
    pub const EVENT_FILTER: u8 = 34;
    /// u32 max (0: off) -> (). Records objects that appear with a `Level` outer.
    pub const SPAWN_WATCH: u8 = 35;
    /// -> u32 n, n * (u32 tick, u32 object, u32 class).
    pub const SPAWN_FETCH: u8 = 36;
    /// u32 object, u32 function, bytes parms -> bytes parms after the call (ProcessEvent through
    /// the object's vtable, game thread).
    pub const CALL_FUNC: u8 = 37;
    /// u32 n, n * u32 objects -> n * str full name ("Class Outer.Name").
    pub const NAMES_OF: u8 = 38;
    /// str name -> u32 FName index, u32 number (`Base_N` -> (Base, N + 1) when only the base
    /// exists). Error when the name is unknown.
    pub const NAME_INDEX: u8 = 39;
}

/// One field of a struct's own `Children` chain (`FIELDS`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldInfo {
    pub addr: u32,
    pub name: String,
    /// `FloatProperty`, `StructProperty`, `Function`, `Enum`, ...
    pub class: String,
    pub offset: u32,
    /// Property element size; for a function, its native index.
    pub element_size: u32,
    /// Property array dimension; for a function, its parameter count.
    pub array_dim: u32,
    pub flags: u64,
    /// Struct / PropertyClass / Enum / Inner property / BitMask; function flags.
    pub aux: u32,
    /// ClassProperty MetaClass; for a function `ParmsSize | ReturnValueOffset << 16`.
    pub aux2: u32,
}

/// The `FIELDS` answer.
#[derive(Clone, Debug, Default)]
pub struct StructInfo {
    pub addr: u32,
    pub super_struct: u32,
    pub size: u32,
    pub name: String,
    pub class: String,
    pub fields: Vec<FieldInfo>,
}

/// A pointer-chain read (`READS`): start at `base`, add each offset, dereference after every
/// offset but the last, then read `len` bytes.
#[derive(Clone, Debug)]
pub struct ReadPath {
    pub base: u32,
    pub offsets: Vec<u32>,
    pub len: u32,
}

/// A spawned object seen by the spawn watch.
#[derive(Clone, Debug)]
pub struct SpawnRecord {
    pub tick: u32,
    pub object: u32,
    pub class: u32,
}

/// One `UWorld::SingleLineCheck` (research/dishonored/COLLISION.md): from `start` to `end`,
/// a box of half size `extent` (zero: a line), UE3 trace flags (plus Dishonored's trace type
/// bits), and the source actor (0: none; it is ignored by the check and picks the movement
/// trace type).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LineQuery {
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub extent: [f32; 3],
    pub flags: u32,
    pub source: u32,
}

/// What the check returned: `hit` false when nothing blocks; else the fraction of the way,
/// the location (the box centre for extent checks), the normal, the actor and component hit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LineResult {
    pub hit: bool,
    pub time: f32,
    pub location: [f32; 3],
    pub normal: [f32; 3],
    pub actor: u32,
    pub component: u32,
}

impl LineQuery {
    pub fn encode(&self, out: &mut Vec<u8>) {
        for v in self.start.iter().chain(&self.end).chain(&self.extent) {
            put_f32(out, *v);
        }
        put_u32(out, self.flags);
        put_u32(out, self.source);
    }
    pub fn decode(r: &mut Reader) -> io::Result<Self> {
        let mut v = [0f32; 9];
        for x in v.iter_mut() {
            *x = r.f32()?;
        }
        Ok(LineQuery { start: [v[0], v[1], v[2]], end: [v[3], v[4], v[5]], extent: [v[6], v[7], v[8]], flags: r.u32()?, source: r.u32()? })
    }
}

impl LineResult {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.push(self.hit as u8);
        put_f32(out, self.time);
        for v in self.location.iter().chain(&self.normal) {
            put_f32(out, *v);
        }
        put_u32(out, self.actor);
        put_u32(out, self.component);
    }
    pub fn decode(r: &mut Reader) -> io::Result<Self> {
        let hit = r.u8()? != 0;
        let time = r.f32()?;
        let mut v = [0f32; 6];
        for x in v.iter_mut() {
            *x = r.f32()?;
        }
        Ok(LineResult { hit, time, location: [v[0], v[1], v[2]], normal: [v[3], v[4], v[5]], actor: r.u32()?, component: r.u32()? })
    }
}

/// One game tick of the local player's pawn, recorded in the `PlayerTick` event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub tick: u32,
    pub delta: f32,
    pub time_seconds: f32,
    pub real_time_seconds: f32,
    pub pawn: u32,
    pub location: [f32; 3],
    pub velocity: [f32; 3],
    pub physics: u8,
    pub health: i32,
    /// Bytes of the `WATCH` ranges, concatenated.
    pub extra: Vec<u8>,
}

impl Sample {
    pub fn encode(&self, out: &mut Vec<u8>) {
        out.extend(self.tick.to_le_bytes());
        out.extend(self.delta.to_le_bytes());
        out.extend(self.time_seconds.to_le_bytes());
        out.extend(self.real_time_seconds.to_le_bytes());
        out.extend(self.pawn.to_le_bytes());
        for v in self.location.iter().chain(self.velocity.iter()) {
            out.extend(v.to_le_bytes());
        }
        out.push(self.physics);
        out.extend(self.health.to_le_bytes());
        put_bytes(out, &self.extra);
    }

    pub fn decode(r: &mut Reader) -> io::Result<Sample> {
        let mut s = Sample { tick: r.u32()?, delta: r.f32()?, time_seconds: r.f32()?, real_time_seconds: r.f32()?, pawn: r.u32()?, ..Default::default() };
        for i in 0..3 {
            s.location[i] = r.f32()?;
        }
        for i in 0..3 {
            s.velocity[i] = r.f32()?;
        }
        s.physics = r.u8()?;
        s.health = r.u32()? as i32;
        s.extra = r.bytes()?.to_vec();
        Ok(s)
    }
}

/// Unreal `EPhysics` values (Actor.uc).
pub fn physics_name(p: u8) -> &'static str {
    match p {
        0 => "None",
        1 => "Walking",
        2 => "Falling",
        3 => "Swimming",
        4 => "Flying",
        5 => "Rotating",
        6 => "Projectile",
        7 => "Interpolating",
        8 => "Spider",
        9 => "Ladder",
        10 => "RigidBody",
        11 => "SoftBody",
        12 => "NavMeshWalking",
        13 => "Unused",
        14 => "Custom",
        _ => "?",
    }
}

pub fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend(v.to_le_bytes());
}
pub fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend(v.to_le_bytes());
}
pub fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    put_u32(out, b.len() as u32);
    out.extend_from_slice(b);
}
pub fn put_str(out: &mut Vec<u8>, s: &str) {
    put_bytes(out, s.as_bytes());
}

/// Cursor over a message payload.
pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

fn short() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "probe message too short")
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    pub fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let s = self.data.get(self.pos..self.pos + n).ok_or_else(short)?;
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn bytes(&mut self) -> io::Result<&'a [u8]> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    pub fn str(&mut self) -> io::Result<String> {
        Ok(String::from_utf8_lossy(self.bytes()?).into_owned())
    }
    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.data[self.pos..];
        self.pos = self.data.len();
        s
    }
}

/// Write one framed message.
pub fn write_frame(w: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(body.len() + 4);
    put_u32(&mut buf, body.len() as u32);
    buf.extend_from_slice(body);
    w.write_all(&buf)?;
    w.flush()
}

/// Read one framed message (at most 256 MiB).
pub fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n > 256 << 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "probe frame too large"));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)?;
    Ok(body)
}

/// FNV-1a 64 of a shader's bytecode (the id the `SHADERS` / `PS_CONSTANTS` ops use).
pub fn fnv64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// A back-buffer capture.
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub pitch: u32,
    pub data: Vec<u8>,
}

impl Capture {
    /// Tightly packed RGBA8 (X8R8G8B8 / A8R8G8B8 back buffers; alpha forced to 255).
    pub fn to_rgba(&self) -> Option<Vec<u8>> {
        if self.format != 21 && self.format != 22 {
            return None;
        }
        let mut out = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height as usize {
            let row = &self.data[y * self.pitch as usize..];
            for x in 0..self.width as usize {
                let p = &row[x * 4..x * 4 + 4];
                out.extend_from_slice(&[p[2], p[1], p[0], 255]);
            }
        }
        Some(out)
    }
}

/// A created pixel shader seen by the D3D9 hook.
#[derive(Clone, Debug)]
pub struct ShaderInfo {
    pub id: u32,
    pub hash: u64,
    pub size: u32,
    /// CTAB entries as `name=c12x2` (register set, index, count).
    pub ctab: String,
}

/// A memory read logged by an entry hook: from stack argument `src` (0-based), `ecx`
/// (`HookDeref::ECX`) or an absolute address (`HookDeref::ABS`, the address in `off`); with
/// `pre != u32::MAX` a u32 pointer at `base + pre` is followed first.
#[derive(Clone, Copy, Debug)]
pub struct HookDeref {
    pub src: u8,
    pub pre: u32,
    pub off: u32,
    pub len: u32,
}

impl HookDeref {
    pub const ECX: u8 = 0xff;
    pub const ABS: u8 = 0xfe;
    pub fn arg(i: u8, off: u32, len: u32) -> Self {
        HookDeref { src: i, pre: u32::MAX, off, len }
    }
    pub fn ecx(off: u32, len: u32) -> Self {
        HookDeref { src: Self::ECX, pre: u32::MAX, off, len }
    }
}

/// One call seen by an entry hook.
#[derive(Clone, Debug)]
pub struct HookRecord {
    pub time: f32,
    pub ecx: u32,
    pub ret: u32,
    pub args: Vec<u32>,
    pub data: Vec<u8>,
}

impl HookRecord {
    pub fn f32_at(&self, off: usize) -> f32 {
        self.data.get(off..off + 4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).unwrap_or(f32::NAN)
    }
    pub fn u32_at(&self, off: usize) -> u32 {
        self.data.get(off..off + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).unwrap_or(0)
    }
}

/// One traced ProcessEvent call.
#[derive(Clone, Debug)]
pub struct EventRecord {
    pub time: f32,
    /// World tick of the call (the DLL's tick-boundary counter).
    pub tick: u32,
    pub object: u32,
    pub func: u32,
    pub parms: Vec<u8>,
}

/// Blocking client over the probe pipe.
pub struct Client {
    pipe: std::fs::File,
}

impl Client {
    /// Connect to a pipe by full name (`\\.\pipe\dishonored_probe_...`).
    pub fn connect(name: &str) -> io::Result<Client> {
        let pipe = std::fs::OpenOptions::new().read(true).write(true).open(name)?;
        Ok(Client { pipe })
    }

    /// Connect, retrying while the game starts (the pipe appears once the DLL is initialised).
    pub fn connect_wait(name: &str, timeout: std::time::Duration) -> io::Result<Client> {
        let start = std::time::Instant::now();
        loop {
            match Client::connect(name) {
                Ok(c) => return Ok(c),
                Err(e) if start.elapsed() < timeout => {
                    let _ = e;
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Send a raw request and return the response payload (errors carry the server's message).
    pub fn request(&mut self, op: u8, payload: &[u8]) -> io::Result<Vec<u8>> {
        let mut body = Vec::with_capacity(payload.len() + 1);
        body.push(op);
        body.extend_from_slice(payload);
        write_frame(&mut self.pipe, &body)?;
        let resp = read_frame(&mut self.pipe)?;
        match resp.first() {
            Some(0) => Ok(resp[1..].to_vec()),
            Some(_) => Err(io::Error::other(String::from_utf8_lossy(&resp[1..]).into_owned())),
            None => Err(short()),
        }
    }

    pub fn ping(&mut self) -> io::Result<(u32, String)> {
        let r = self.request(op::PING, &[])?;
        let mut rd = Reader::new(&r);
        Ok((rd.u32()?, rd.str()?))
    }

    pub fn read(&mut self, addr: u32, len: u32) -> io::Result<Vec<u8>> {
        let mut p = Vec::new();
        put_u32(&mut p, addr);
        put_u32(&mut p, len);
        self.request(op::READ, &p)
    }

    pub fn read_u32(&mut self, addr: u32) -> io::Result<u32> {
        let b = self.read(addr, 4)?;
        Ok(u32::from_le_bytes(b[..4].try_into().unwrap()))
    }

    pub fn read_f32(&mut self, addr: u32) -> io::Result<f32> {
        Ok(f32::from_bits(self.read_u32(addr)?))
    }

    pub fn write(&mut self, addr: u32, data: &[u8]) -> io::Result<()> {
        let mut p = Vec::new();
        put_u32(&mut p, addr);
        p.extend_from_slice(data);
        self.request(op::WRITE, &p).map(|_| ())
    }

    pub fn status(&mut self) -> io::Result<String> {
        Ok(String::from_utf8_lossy(&self.request(op::STATUS, &[])?).into_owned())
    }

    /// `key=value` from `status()`, parsed (hex values with `0x` are returned as text).
    pub fn status_value(&mut self, key: &str) -> io::Result<Option<String>> {
        let s = self.status()?;
        Ok(s.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')).map(str::to_string)))
    }

    pub fn find(&mut self, name: &str, class: &str) -> io::Result<Vec<u32>> {
        let mut p = Vec::new();
        put_str(&mut p, name);
        put_str(&mut p, class);
        let r = self.request(op::FIND, &p)?;
        let mut rd = Reader::new(&r);
        let n = rd.u32()?;
        (0..n).map(|_| rd.u32()).collect()
    }

    pub fn name_of(&mut self, obj: u32) -> io::Result<String> {
        let mut p = Vec::new();
        put_u32(&mut p, obj);
        let r = self.request(op::NAME_OF, &p)?;
        Reader::new(&r).str()
    }

    pub fn console(&mut self, cmd: &str) -> io::Result<String> {
        let mut p = Vec::new();
        put_str(&mut p, cmd);
        let r = self.request(op::CONSOLE, &p)?;
        Reader::new(&r).str()
    }

    pub fn call(&mut self, obj: u32, func: &str, parms: &[u8]) -> io::Result<Vec<u8>> {
        let mut p = Vec::new();
        put_u32(&mut p, obj);
        put_str(&mut p, func);
        p.extend_from_slice(parms);
        self.request(op::CALL, &p)
    }

    pub fn sample_start(&mut self, max: u32) -> io::Result<()> {
        let mut p = Vec::new();
        put_u32(&mut p, max);
        self.request(op::SAMPLE_START, &p).map(|_| ())
    }

    pub fn sample_fetch(&mut self) -> io::Result<Vec<Sample>> {
        let r = self.request(op::SAMPLE_FETCH, &[])?;
        let mut rd = Reader::new(&r);
        let n = rd.u32()?;
        (0..n).map(|_| Sample::decode(&mut rd)).collect()
    }

    pub fn watch(&mut self, ranges: &[(u32, u32)]) -> io::Result<()> {
        let mut p = Vec::new();
        put_u32(&mut p, ranges.len() as u32);
        for &(o, l) in ranges {
            put_u32(&mut p, o);
            put_u32(&mut p, l);
        }
        self.request(op::WATCH, &p).map(|_| ())
    }

    /// Absolute memory ranges recorded with every sample (after the pawn ranges).
    pub fn watch_abs(&mut self, ranges: &[(u32, u32)]) -> io::Result<()> {
        let mut p = Vec::new();
        put_u32(&mut p, ranges.len() as u32);
        for &(a, l) in ranges {
            put_u32(&mut p, a);
            put_u32(&mut p, l);
        }
        self.request(op::WATCH_ABS, &p).map(|_| ())
    }

    /// A zeroed scratch block in the game process.
    pub fn alloc(&mut self, size: u32) -> io::Result<u32> {
        let mut p = Vec::new();
        put_u32(&mut p, size);
        let r = self.request(op::ALLOC, &p)?;
        Reader::new(&r).u32()
    }

    /// An FString's UTF-16 data written into scratch memory: returns `(data, num, max)`.
    pub fn fstring(&mut self, s: &str) -> io::Result<[u32; 3]> {
        let w: Vec<u16> = s.encode_utf16().chain(Some(0)).collect();
        let a = self.alloc((w.len() * 2) as u32)?;
        let b: Vec<u8> = w.iter().flat_map(|c| c.to_le_bytes()).collect();
        self.write(a, &b)?;
        Ok([a, w.len() as u32, w.len() as u32])
    }

    /// Call a native function on the game thread: (eax, x87 top when `ret_float`).
    pub fn native_call(&mut self, func: u32, this: u32, args: &[u32], ret_float: bool) -> io::Result<(u32, f32)> {
        let mut p = Vec::new();
        put_u32(&mut p, func);
        put_u32(&mut p, this);
        p.push(ret_float as u8);
        put_u32(&mut p, args.len() as u32);
        for &a in args {
            put_u32(&mut p, a);
        }
        let r = self.request(op::NATIVE_CALL, &p)?;
        let mut rd = Reader::new(&r);
        Ok((rd.u32()?, rd.f32()?))
    }

    /// Install or reconfigure an entry logger (see `op::HOOK_SET`).
    pub fn hook_set(&mut self, addr: u32, prologue_len: u8, nargs: u8, max: u32, derefs: &[HookDeref]) -> io::Result<u32> {
        let mut p = Vec::new();
        put_u32(&mut p, addr);
        p.push(prologue_len);
        p.push(nargs);
        put_u32(&mut p, max);
        put_u32(&mut p, derefs.len() as u32);
        for d in derefs {
            p.push(d.src);
            put_u32(&mut p, d.pre);
            put_u32(&mut p, d.off);
            put_u32(&mut p, d.len);
        }
        let r = self.request(op::HOOK_SET, &p)?;
        Reader::new(&r).u32()
    }

    pub fn hook_fetch(&mut self, id: u32) -> io::Result<Vec<HookRecord>> {
        let mut p = Vec::new();
        put_u32(&mut p, id);
        let r = self.request(op::HOOK_FETCH, &p)?;
        let mut rd = Reader::new(&r);
        let n = rd.u32()?;
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let time = rd.f32()?;
            let ecx = rd.u32()?;
            let ret = rd.u32()?;
            let na = rd.u32()?;
            let args = (0..na).map(|_| rd.u32()).collect::<io::Result<Vec<u32>>>()?;
            let data = rd.bytes()?.to_vec();
            out.push(HookRecord { time, ecx, ret, args, data });
        }
        Ok(out)
    }

    /// Run engine collision checks (`LINE_CHECKS`), at most 4096 per request.
    pub fn line_checks(&mut self, q: &[LineQuery]) -> io::Result<Vec<LineResult>> {
        let mut out = Vec::with_capacity(q.len());
        for chunk in q.chunks(4096) {
            let mut p = Vec::new();
            put_u32(&mut p, chunk.len() as u32);
            for x in chunk {
                x.encode(&mut p);
            }
            let r = self.request(op::LINE_CHECKS, &p)?;
            let mut rd = Reader::new(&r);
            let n = rd.u32()?;
            for _ in 0..n {
                out.push(LineResult::decode(&mut rd)?);
            }
        }
        Ok(out)
    }

    /// Trace ProcessEvent calls of the given UFunctions (empty or `max` 0: off).
    pub fn event_trace(&mut self, funcs: &[u32], parm_len: u32, max: u32) -> io::Result<()> {
        let mut p = Vec::new();
        put_u32(&mut p, max);
        put_u32(&mut p, parm_len);
        put_u32(&mut p, funcs.len() as u32);
        for &f in funcs {
            put_u32(&mut p, f);
        }
        self.request(op::EVENT_TRACE, &p).map(|_| ())
    }

    pub fn event_fetch(&mut self) -> io::Result<Vec<EventRecord>> {
        let r = self.request(op::EVENT_FETCH, &[])?;
        let mut rd = Reader::new(&r);
        let n = rd.u32()?;
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            out.push(EventRecord { time: rd.f32()?, tick: rd.u32()?, object: rd.u32()?, func: rd.u32()?, parms: rd.bytes()?.to_vec() });
        }
        Ok(out)
    }

    /// Trace ProcessEvent calls by function-name substrings (`Outer.Name`), by the object's
    /// class (is-a, exact class names) and by function address. Returns the number of
    /// functions selected by name and address. `max` 0 turns the trace off.
    pub fn event_filter(&mut self, funcs: &[String], classes: &[String], addrs: &[u32], parm_len: u32, max: u32) -> io::Result<u32> {
        let mut p = Vec::new();
        put_u32(&mut p, max);
        put_u32(&mut p, parm_len);
        put_u32(&mut p, funcs.len() as u32);
        for f in funcs {
            put_str(&mut p, f);
        }
        put_u32(&mut p, classes.len() as u32);
        for c in classes {
            put_str(&mut p, c);
        }
        put_u32(&mut p, addrs.len() as u32);
        for a in addrs {
            put_u32(&mut p, *a);
        }
        Reader::new(&self.request(op::EVENT_FILTER, &p)?).u32()
    }

    /// (world tick, tick hook installed, lockstep, parked, presents).
    pub fn tick(&mut self) -> io::Result<(u32, bool, bool, bool, u32)> {
        let r = self.request(op::TICK, &[])?;
        let mut rd = Reader::new(&r);
        Ok((rd.u32()?, rd.u8()? != 0, rd.u8()? != 0, rd.u8()? != 0, rd.u32()?))
    }

    /// Lockstep on (park the game thread at the next tick boundary) or off. Returns the tick.
    pub fn lockstep(&mut self, on: bool, timeout_ms: u32) -> io::Result<u32> {
        let mut p = vec![on as u8];
        put_u32(&mut p, timeout_ms);
        Reader::new(&self.request(op::LOCKSTEP, &p)?).u32()
    }

    /// Run `n` world ticks (lockstep: release and park again). Returns the tick.
    pub fn step(&mut self, n: u32, timeout_ms: u32) -> io::Result<u32> {
        let mut p = Vec::new();
        put_u32(&mut p, n);
        put_u32(&mut p, timeout_ms);
        Reader::new(&self.request(op::STEP, &p)?).u32()
    }

    /// Pointer-chain reads in one request; `None` where unreadable.
    pub fn reads(&mut self, paths: &[ReadPath]) -> io::Result<Vec<Option<Vec<u8>>>> {
        let mut p = Vec::new();
        put_u32(&mut p, paths.len() as u32);
        for x in paths {
            put_u32(&mut p, x.base);
            p.push(x.offsets.len().max(1) as u8);
            if x.offsets.is_empty() {
                put_u32(&mut p, 0);
            }
            for o in &x.offsets {
                put_u32(&mut p, *o);
            }
            put_u32(&mut p, x.len);
        }
        let r = self.request(op::READS, &p)?;
        let mut rd = Reader::new(&r);
        let mut out = Vec::with_capacity(paths.len());
        for x in paths {
            let ok = rd.u8()? != 0;
            let b = rd.take(x.len as usize)?;
            out.push(ok.then(|| b.to_vec()));
        }
        Ok(out)
    }

    /// The own fields of a struct, class or function, with its super and size.
    pub fn fields(&mut self, st: u32) -> io::Result<StructInfo> {
        let mut p = Vec::new();
        put_u32(&mut p, st);
        let r = self.request(op::FIELDS, &p)?;
        let mut rd = Reader::new(&r);
        let mut s = StructInfo { addr: st, super_struct: rd.u32()?, size: rd.u32()?, name: rd.str()?, class: rd.str()?, fields: Vec::new() };
        let n = rd.u32()?;
        for _ in 0..n {
            s.fields.push(FieldInfo {
                addr: rd.u32()?,
                name: rd.str()?,
                class: rd.str()?,
                offset: rd.u32()?,
                element_size: rd.u32()?,
                array_dim: rd.u32()?,
                flags: rd.u64()?,
                aux: rd.u32()?,
                aux2: rd.u32()?,
            });
        }
        Ok(s)
    }

    pub fn enum_names(&mut self, e: u32) -> io::Result<Vec<String>> {
        let mut p = Vec::new();
        put_u32(&mut p, e);
        let r = self.request(op::ENUM_NAMES, &p)?;
        let mut rd = Reader::new(&r);
        let n = rd.u32()?;
        (0..n).map(|_| rd.str()).collect()
    }

    pub fn spawn_watch(&mut self, max: u32) -> io::Result<()> {
        let mut p = Vec::new();
        put_u32(&mut p, max);
        self.request(op::SPAWN_WATCH, &p).map(|_| ())
    }

    pub fn spawn_fetch(&mut self) -> io::Result<Vec<SpawnRecord>> {
        let r = self.request(op::SPAWN_FETCH, &[])?;
        let mut rd = Reader::new(&r);
        let n = rd.u32()?;
        (0..n).map(|_| Ok(SpawnRecord { tick: rd.u32()?, object: rd.u32()?, class: rd.u32()? })).collect()
    }

    /// ProcessEvent of a function object on an object with a parameter block (at least the
    /// function's ParmsSize); returns the block after the call.
    pub fn call_func(&mut self, obj: u32, func: u32, parms: &[u8]) -> io::Result<Vec<u8>> {
        let mut p = Vec::new();
        put_u32(&mut p, obj);
        put_u32(&mut p, func);
        p.extend_from_slice(parms);
        self.request(op::CALL_FUNC, &p)
    }

    /// FName (index, number) of a name.
    pub fn name_index(&mut self, name: &str) -> io::Result<(u32, u32)> {
        let mut p = Vec::new();
        put_str(&mut p, name);
        let r = self.request(op::NAME_INDEX, &p)?;
        let mut rd = Reader::new(&r);
        Ok((rd.u32()?, rd.u32()?))
    }

    /// Full names of many objects in one request.
    pub fn names_of(&mut self, objs: &[u32]) -> io::Result<Vec<String>> {
        let mut p = Vec::new();
        put_u32(&mut p, objs.len() as u32);
        for o in objs {
            put_u32(&mut p, *o);
        }
        let r = self.request(op::NAMES_OF, &p)?;
        let mut rd = Reader::new(&r);
        objs.iter().map(|_| rd.str()).collect()
    }

    /// Hold PlayerInput axes (`aBaseY` forward, `aStrafe`, `aTurn`, `aLookUp`) for `frames`
    /// ticks (0 = until the next call). All zero releases.
    pub fn input_axes(&mut self, forward: f32, strafe: f32, turn: f32, lookup: f32, frames: u32) -> io::Result<()> {
        let mut p = vec![1u8];
        for v in [forward, strafe, turn, lookup] {
            put_f32(&mut p, v);
        }
        put_u32(&mut p, frames);
        self.request(op::INPUT_AXES, &p).map(|_| ())
    }

    /// Send a key event to the engine's input (`key` is an engine key name such as
    /// `SpaceBar`, `W`, `LeftShift`, `C`, `Enter`). Returns whether a handler took it.
    pub fn key(&mut self, key: &str, down: bool) -> io::Result<bool> {
        let mut p = Vec::new();
        put_str(&mut p, key);
        p.push(if down { 0 } else { 1 });
        Ok(self.request(op::KEY, &p)?.first() == Some(&1))
    }

    pub fn wait_ticks(&mut self, n: u32) -> io::Result<()> {
        let mut p = Vec::new();
        put_u32(&mut p, n);
        self.request(op::WAIT_TICKS, &p).map(|_| ())
    }

    pub fn capture(&mut self) -> io::Result<Capture> {
        let r = self.request(op::CAPTURE, &[])?;
        let mut rd = Reader::new(&r);
        Ok(Capture { width: rd.u32()?, height: rd.u32()?, format: rd.u32()?, pitch: rd.u32()?, data: rd.rest().to_vec() })
    }

    pub fn shaders(&mut self) -> io::Result<Vec<ShaderInfo>> {
        let r = self.request(op::SHADERS, &[])?;
        let mut rd = Reader::new(&r);
        let n = rd.u32()?;
        (0..n).map(|_| Ok(ShaderInfo { id: rd.u32()?, hash: rd.u64()?, size: rd.u32()?, ctab: rd.str()? })).collect()
    }

    /// Skip every draw made with these pixel shaders (empty: draw everything again).
    pub fn draw_skip(&mut self, hashes: &[u64]) -> io::Result<()> {
        let mut p = Vec::new();
        put_u32(&mut p, hashes.len() as u32);
        for h in hashes {
            p.extend(h.to_le_bytes());
        }
        self.request(op::DRAW_SKIP, &p)?;
        Ok(())
    }

    /// Draws and primitives per pixel shader over the next `frames` frames.
    pub fn draw_census(&mut self, frames: u32) -> io::Result<Vec<(u64, u32, u32)>> {
        let mut p = Vec::new();
        put_u32(&mut p, frames);
        let r = self.request(op::DRAW_CENSUS, &p)?;
        let mut rd = Reader::new(&r);
        let n = rd.u32()?;
        (0..n).map(|_| Ok((rd.u64()?, rd.u32()?, rd.u32()?))).collect()
    }

    /// PS constant registers (224 float4) at the next `draws` draws (within 5 s) made with
    /// shader `hash`, or, with `hash` 0, with any shader whose CTAB names contain `filter`.
    /// Returns (frame, shader hash, registers) per draw.
    pub fn ps_constants(&mut self, hash: u64, draws: u32, filter: &str) -> io::Result<Vec<(u32, u64, Vec<[f32; 4]>)>> {
        let mut p = Vec::new();
        p.extend(hash.to_le_bytes());
        put_u32(&mut p, draws);
        put_str(&mut p, filter);
        let r = self.request(op::PS_CONSTANTS, &p)?;
        let mut rd = Reader::new(&r);
        let n = rd.u32()?;
        let mut out = Vec::new();
        for _ in 0..n {
            let frame = rd.u32()?;
            let h = rd.u64()?;
            let mut regs = Vec::with_capacity(224);
            for _ in 0..224 {
                regs.push([rd.f32()?, rd.f32()?, rd.f32()?, rd.f32()?]);
            }
            out.push((frame, h, regs));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_roundtrip() {
        let s = Sample { tick: 7, delta: 0.016, time_seconds: 1.5, real_time_seconds: 2.0, pawn: 0x1234, location: [1.0, 2.0, 3.0], velocity: [4.0, 5.0, -6.0], physics: 2, health: 70, extra: vec![9, 8] };
        let mut b = Vec::new();
        s.encode(&mut b);
        assert_eq!(Sample::decode(&mut Reader::new(&b)).unwrap(), s);
    }

    #[test]
    fn frame_roundtrip() {
        let mut b = Vec::new();
        write_frame(&mut b, b"\x04hello").unwrap();
        assert_eq!(read_frame(&mut &b[..]).unwrap(), b"\x04hello");
    }
}
