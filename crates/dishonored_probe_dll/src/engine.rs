//! UE3 object system of the running game: `GObjObjects` / `GNames` found by scanning the
//! executable's .data section, names, object lookup, and the local player's objects.
//!
//! UObject layout (class_layout Core Object): vtable +0x0, ObjectInternalInteger +0x4,
//! Outer +0x24, Name (index, number) +0x28, Class +0x30; size 0x38.
use crate::mem;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

pub const OBJ_INDEX: u32 = 0x04;
pub const OBJ_OUTER: u32 = 0x24;
pub const OBJ_NAME: u32 = 0x28;
pub const OBJ_CLASS: u32 = 0x30;
/// GameEngine.GamePlayers (TArray<LocalPlayer>).
pub const ENGINE_GAME_PLAYERS: u32 = 0x480;
/// Player.Actor (the PlayerController).
pub const PLAYER_ACTOR: u32 = 0x3c;
/// Controller.Pawn.
pub const CONTROLLER_PAWN: u32 = 0x248;
/// PlayerController.PlayerInput.
pub const PC_PLAYER_INPUT: u32 = 0x464;
/// Actor fields.
pub const ACTOR_LOCATION: u32 = 0xc4;
pub const ACTOR_PHYSICS: u32 = 0x104;
pub const ACTOR_WORLDINFO: u32 = 0x160;
pub const ACTOR_VELOCITY: u32 = 0x1b4;
/// Pawn.Health.
pub const PAWN_HEALTH: u32 = 0x344;
/// WorldInfo.TimeSeconds / RealTimeSeconds.
pub const WI_TIME_SECONDS: u32 = 0x38c;
pub const WI_REAL_TIME_SECONDS: u32 = 0x390;

/// .data of Dishonored.exe (image base 0x400000).
const DATA_START: u32 = 0x0126_9000;
const DATA_END: u32 = 0x0126_9000 + 0x0021_b3bc;

pub static GOBJECTS: AtomicU32 = AtomicU32::new(0);
pub static GNAMES: AtomicU32 = AtomicU32::new(0);
/// Offset of the characters in an FNameEntry.
pub static NAME_TEXT_OFF: AtomicU32 = AtomicU32::new(0);
pub static GENGINE: AtomicU32 = AtomicU32::new(0);

fn tarray(addr: u32) -> Option<(u32, u32, u32)> {
    let b = mem::read::<[u32; 3]>(addr)?;
    Some((b[0], b[1], b[2]))
}

fn looks_like_gobjects(addr: u32) -> bool {
    let Some((data, num, max)) = tarray(addr) else { return false };
    if data < 0x10000 || !(10_000..4_000_000).contains(&num) || num > max || max > 8_000_000 {
        return false;
    }
    let Some(ptrs) = mem::read_bytes(data, 4 * 200) else { return false };
    let mut good = 0;
    for i in 0..200u32 {
        let p = u32::from_le_bytes(ptrs[i as usize * 4..i as usize * 4 + 4].try_into().unwrap());
        if p == 0 {
            continue;
        }
        match mem::u32(p + OBJ_INDEX) {
            Some(v) if v == i => good += 1,
            _ => return false,
        }
    }
    good >= 150
}

fn name_text_offset(entry: u32) -> Option<u32> {
    let b = mem::read_bytes(entry, 0x30)?;
    (0..0x20).find(|&o| b[o..].starts_with(b"None\0")).map(|o| o as u32)
}

fn looks_like_gnames(addr: u32) -> Option<u32> {
    let (data, num, max) = tarray(addr)?;
    if data < 0x10000 || !(10_000..4_000_000).contains(&num) || num > max {
        return None;
    }
    let e0 = mem::u32(data)?;
    let off = name_text_offset(e0)?;
    let e1 = mem::u32(data + 4)?;
    let t1 = mem::read_bytes(e1 + off, 12)?;
    t1.starts_with(b"ByteProperty").then_some(off)
}

/// Scan .data for the two global arrays. Cheap after success.
pub fn discover() -> bool {
    if GOBJECTS.load(Relaxed) != 0 && GNAMES.load(Relaxed) != 0 {
        return true;
    }
    let Some(data) = mem::read_bytes(DATA_START, (DATA_END - DATA_START) as usize) else { return false };
    let mut objs = 0;
    let mut names = 0;
    let mut off = 0;
    let mut i = 0;
    while i + 12 <= data.len() {
        let a = DATA_START + i as u32;
        let d = u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
        let n = u32::from_le_bytes(data[i + 4..i + 8].try_into().unwrap());
        if d >= 0x10000 && (10_000..4_000_000).contains(&n) {
            if objs == 0 && looks_like_gobjects(a) {
                objs = a;
            } else if names == 0
                && let Some(o) = looks_like_gnames(a) {
                    names = a;
                    off = o;
                }
        }
        i += 4;
    }
    if objs != 0 && names != 0 {
        NAME_TEXT_OFF.store(off, Relaxed);
        GNAMES.store(names, Relaxed);
        GOBJECTS.store(objs, Relaxed);
        return true;
    }
    false
}

/// Background discovery: the arrays exist once the engine has initialised.
pub fn start_discovery() {
    std::thread::spawn(|| {
        loop {
            if discover() && find_engine() != 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

/// Text of an FName index.
pub fn name_str(index: u32) -> Option<String> {
    let names = GNAMES.load(Relaxed);
    if names == 0 {
        return None;
    }
    let (data, num, _) = tarray(names)?;
    if index >= num {
        return None;
    }
    let e = mem::u32(data + 4 * index)?;
    let off = NAME_TEXT_OFF.load(Relaxed);
    let b = mem::read_bytes(e + off, 128).or_else(|| mem::read_bytes(e + off, 32))?;
    // Wide entries: second byte zero after a printable first byte.
    if b.len() >= 4 && b[0] != 0 && b[1] == 0 && b[2] != 0 {
        let w: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        return Some(String::from_utf16_lossy(&w));
    }
    Some(b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect())
}

/// FName index of a string (case-insensitive like FName).
pub fn name_index(s: &str) -> Option<u32> {
    let names = GNAMES.load(Relaxed);
    let (data, num, _) = tarray(names)?;
    let ptrs = mem::read_bytes(data, 4 * num as usize)?;
    let off = NAME_TEXT_OFF.load(Relaxed);
    let want = s.as_bytes();
    for i in 0..num as usize {
        let e = u32::from_le_bytes(ptrs[i * 4..i * 4 + 4].try_into().unwrap());
        if e == 0 {
            continue;
        }
        let Some(b) = mem::read_bytes(e + off, want.len() + 1) else { continue };
        if b[want.len()] == 0 && b[..want.len()].eq_ignore_ascii_case(want) {
            return Some(i as u32);
        }
    }
    None
}

pub fn obj_name(obj: u32) -> Option<String> {
    let n = mem::read::<[u32; 2]>(obj + OBJ_NAME)?;
    let s = name_str(n[0])?;
    Some(if n[1] > 0 { format!("{}_{}", s, n[1] - 1) } else { s })
}

pub fn class_name(obj: u32) -> Option<String> {
    obj_name(mem::u32(obj + OBJ_CLASS)?)
}

pub fn full_name(obj: u32) -> Option<String> {
    let mut path = obj_name(obj)?;
    let mut o = mem::u32(obj + OBJ_OUTER).unwrap_or(0);
    let mut guard = 0;
    while o != 0 && guard < 16 {
        path = format!("{}.{}", obj_name(o).unwrap_or_default(), path);
        o = mem::u32(o + OBJ_OUTER).unwrap_or(0);
        guard += 1;
    }
    Some(format!("{} {}", class_name(obj).unwrap_or_default(), path))
}

/// Snapshot of the object array.
pub fn objects() -> Vec<u32> {
    let g = GOBJECTS.load(Relaxed);
    let Some((data, num, _)) = tarray(g) else { return Vec::new() };
    let Some(b) = mem::read_bytes(data, 4 * num as usize) else { return Vec::new() };
    b.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
}

/// Superclass chain names of an object's class (UStruct.SuperField is not decoded; the class
/// name alone is matched).
fn is_default(obj: u32) -> bool {
    obj_name(obj).is_some_and(|n| n.starts_with("Default__"))
}

/// Live objects (class defaults excluded) named `name` (any if empty) of class `class` (any if
/// empty). `name` may be `Outer.Name`.
pub fn find(name: &str, class: &str) -> Vec<u32> {
    let (outer, name) = match name.rsplit_once('.') {
        Some((o, n)) => (Some(o.to_string()), n.to_string()),
        None => (None, name.to_string()),
    };
    let ni = if name.is_empty() { None } else { name_index(&name) };
    if !name.is_empty() && ni.is_none() {
        return Vec::new();
    }
    let ci = if class.is_empty() { None } else { name_index(class) };
    if !class.is_empty() && ci.is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for o in objects() {
        if o == 0 {
            continue;
        }
        let Some(n) = mem::read::<[u32; 2]>(o + OBJ_NAME) else { continue };
        if let Some(ni) = ni
            && n[0] != ni {
                continue;
            }
        if let Some(ci) = ci {
            let Some(c) = mem::u32(o + OBJ_CLASS) else { continue };
            if mem::u32(c + OBJ_NAME) != Some(ci) {
                continue;
            }
        }
        if let Some(outer) = &outer {
            let ok = mem::u32(o + OBJ_OUTER).and_then(obj_name).is_some_and(|on| on.eq_ignore_ascii_case(outer));
            if !ok {
                continue;
            }
        }
        if is_default(o) && !name.starts_with("Default__") {
            continue;
        }
        out.push(o);
        if out.len() >= 4096 {
            break;
        }
    }
    out
}

/// The game engine object (`Transient.DishonoredEngine_0`, a GameEngine subclass).
pub fn find_engine() -> u32 {
    let e = GENGINE.load(Relaxed);
    if e != 0 {
        return e;
    }
    let Some(want) = name_index("DishonoredEngine") else { return 0 };
    for o in objects() {
        if o == 0 {
            continue;
        }
        if mem::u32(o + OBJ_CLASS).and_then(|c| mem::u32(c + OBJ_NAME)) == Some(want) && !is_default(o) {
            GENGINE.store(o, Relaxed);
            return o;
        }
    }
    0
}

/// GEngine.GamePlayers[0].Actor.
pub fn local_pc() -> u32 {
    let e = GENGINE.load(Relaxed);
    if e == 0 {
        return 0;
    }
    let Some((data, num, _)) = tarray(e + ENGINE_GAME_PLAYERS) else { return 0 };
    if num == 0 {
        return 0;
    }
    mem::u32(data).and_then(|lp| mem::u32(lp + PLAYER_ACTOR)).unwrap_or(0)
}

/// A function object: `Class.Func` (outer name match) or the first `Func`.
pub fn find_function(spec: &str) -> u32 {
    find(spec, "Function").first().copied().unwrap_or(0)
}
