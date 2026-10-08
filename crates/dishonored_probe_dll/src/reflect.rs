//! UE3 reflection of the running game: the field chains of classes, structs and functions,
//! enum names, an FName lookup cache and the `is-a` test, so clients can read, write and call
//! by name instead of by hard-coded offset.
//!
//! Layouts (this build; read live from `Actor`, `Actor.Location`, `Actor.SetLocation`,
//! `Actor.EPhysics`; UObject is 0x38 bytes):
//!
//! | class | field | offset |
//! |---|---|---|
//! | UField | Next | +0x38 |
//! | UStruct | SuperField / Children / PropertiesSize | +0x44 / +0x48 / +0x4C |
//! | UFunction | FunctionFlags / iNative (u16) | +0x80 / +0x84 |
//! | UFunction | NumParms (u8) / ParmsSize (u16) / ReturnValueOffset (u16) | +0x91 / +0x92 / +0x94 |
//! | UProperty | ArrayDim / ElementSize / PropertyFlags (u64) / Offset | +0x3C / +0x40 / +0x44 / +0x5C |
//! | UBoolProperty | BitMask | +0x6C |
//! | UStructProperty / UObjectProperty / UByteProperty / UArrayProperty | Struct / PropertyClass / Enum / Inner | +0x6C |
//! | UClassProperty | MetaClass | +0x70 |
//! | UEnum | Names (TArray<FName>) | +0x3C |
use crate::{engine, mem};
use std::collections::HashMap;
use std::sync::Mutex;

pub const FIELD_NEXT: u32 = 0x38;
pub const STRUCT_SUPER: u32 = 0x44;
pub const STRUCT_CHILDREN: u32 = 0x48;
pub const STRUCT_SIZE: u32 = 0x4c;
pub const FUNC_FLAGS: u32 = 0x80;
pub const FUNC_NATIVE: u32 = 0x84;
pub const FUNC_NUM_PARMS: u32 = 0x91;
pub const FUNC_PARMS_SIZE: u32 = 0x92;
pub const FUNC_RETURN_OFFSET: u32 = 0x94;
pub const PROP_ARRAY_DIM: u32 = 0x3c;
pub const PROP_ELEMENT_SIZE: u32 = 0x40;
pub const PROP_FLAGS: u32 = 0x44;
pub const PROP_OFFSET: u32 = 0x5c;
pub const PROP_AUX: u32 = 0x6c;
pub const PROP_AUX2: u32 = 0x70;
pub const ENUM_NAMES: u32 = 0x3c;

/// One field of a struct's own `Children` chain.
pub struct Field {
    pub addr: u32,
    pub name: String,
    /// The field's class name (`FloatProperty`, `Function`, `Enum`, ...).
    pub class: String,
    pub offset: u32,
    pub element_size: u32,
    pub array_dim: u32,
    pub flags: u64,
    /// Struct / PropertyClass / Enum / Inner / BitMask (by property class); for a function, its
    /// flags.
    pub aux: u32,
    /// ClassProperty MetaClass; for a function, ParmsSize | ReturnValueOffset << 16.
    pub aux2: u32,
}

fn u16_at(a: u32) -> u32 {
    mem::read::<u16>(a).unwrap_or(0) as u32
}

/// The own fields of a struct / class / function (not the super's), in declaration order.
pub fn fields(st: u32) -> Vec<Field> {
    let mut out = Vec::new();
    let mut f = mem::u32(st + STRUCT_CHILDREN).unwrap_or(0);
    let mut guard = 0;
    while f != 0 && guard < 4096 {
        guard += 1;
        let class = engine::class_name(f).unwrap_or_default();
        let name = engine::obj_name(f).unwrap_or_default();
        let is_prop = class.ends_with("Property");
        let mut fd = Field { addr: f, name, class, offset: 0, element_size: 0, array_dim: 0, flags: 0, aux: 0, aux2: 0 };
        if is_prop {
            fd.array_dim = mem::u32(f + PROP_ARRAY_DIM).unwrap_or(0);
            fd.element_size = mem::u32(f + PROP_ELEMENT_SIZE).unwrap_or(0);
            fd.flags = mem::read::<u64>(f + PROP_FLAGS).unwrap_or(0);
            fd.offset = mem::u32(f + PROP_OFFSET).unwrap_or(0);
            fd.aux = mem::u32(f + PROP_AUX).unwrap_or(0);
            if fd.class == "ClassProperty" {
                fd.aux2 = mem::u32(f + PROP_AUX2).unwrap_or(0);
            }
        } else if fd.class == "Function" {
            fd.aux = mem::u32(f + FUNC_FLAGS).unwrap_or(0);
            fd.aux2 = u16_at(f + FUNC_PARMS_SIZE) | u16_at(f + FUNC_RETURN_OFFSET) << 16;
            fd.element_size = u16_at(f + FUNC_NATIVE);
            fd.array_dim = mem::read::<u8>(f + FUNC_NUM_PARMS).unwrap_or(0) as u32;
        }
        out.push(fd);
        f = mem::u32(f + FIELD_NEXT).unwrap_or(0);
    }
    out
}

pub fn super_of(st: u32) -> u32 {
    mem::u32(st + STRUCT_SUPER).unwrap_or(0)
}

pub fn struct_size(st: u32) -> u32 {
    mem::u32(st + STRUCT_SIZE).unwrap_or(0)
}

pub fn enum_names(e: u32) -> Vec<String> {
    let Some(a) = mem::read::<[u32; 3]>(e + ENUM_NAMES) else { return Vec::new() };
    if a[0] == 0 || a[1] > 4096 {
        return Vec::new();
    }
    let Some(b) = mem::read_bytes(a[0], a[1] as usize * 8) else { return Vec::new() };
    b.chunks(8)
        .map(|c| {
            let i = u32::from_le_bytes(c[..4].try_into().unwrap());
            let n = u32::from_le_bytes(c[4..].try_into().unwrap());
            let s = engine::name_str(i).unwrap_or_default();
            if n > 0 { format!("{s}_{}", n - 1) } else { s }
        })
        .collect()
}

/// Per-class memo of `is_a` against a fixed set of class names (the event filter).
pub struct IsACache {
    pub names: Vec<u32>,
    pub memo: HashMap<u32, bool>,
}

impl IsACache {
    pub fn new(names: Vec<u32>) -> Self {
        IsACache { names, memo: HashMap::new() }
    }
    pub fn test(&mut self, obj: u32) -> bool {
        if self.names.is_empty() {
            return false;
        }
        let Some(cls) = mem::u32(obj + engine::OBJ_CLASS) else { return false };
        if let Some(&v) = self.memo.get(&cls) {
            return v;
        }
        let mut v = false;
        let mut c = cls;
        let mut guard = 0;
        while c != 0 && guard < 64 && !v {
            v = mem::u32(c + engine::OBJ_NAME).is_some_and(|n| self.names.contains(&n));
            c = super_of(c);
            guard += 1;
        }
        self.memo.insert(cls, v);
        v
    }
}

// ----- FName cache -----

struct NameCache {
    count: u32,
    by_lower: Option<HashMap<String, u32>>,
}
static NAMES: Mutex<NameCache> = Mutex::new(NameCache { count: 0, by_lower: None });

/// FName index of `s` (case-insensitive), from a cache filled incrementally as the name table
/// grows (entries are never removed).
pub fn name_index(s: &str) -> Option<u32> {
    let want = s.to_ascii_lowercase();
    let mut guard = NAMES.lock().ok()?;
    let c = &mut *guard;
    let map = c.by_lower.get_or_insert_with(HashMap::new);
    if let Some(&i) = map.get(&want) {
        return Some(i);
    }
    let names = engine::GNAMES.load(std::sync::atomic::Ordering::Relaxed);
    let arr = mem::read::<[u32; 3]>(names)?;
    let num = arr[1];
    if num > c.count {
        let ptrs = mem::read_bytes(arr[0] + 4 * c.count, 4 * (num - c.count) as usize)?;
        let off = engine::NAME_TEXT_OFF.load(std::sync::atomic::Ordering::Relaxed);
        for (k, p) in ptrs.chunks(4).enumerate() {
            let e = u32::from_le_bytes(p.try_into().unwrap());
            if e == 0 {
                continue;
            }
            let Some(b) = mem::read_bytes(e + off, 64).or_else(|| mem::read_bytes(e + off, 16)) else { continue };
            let text: String = if b.len() >= 4 && b[0] != 0 && b[1] == 0 && b[2] != 0 {
                let w: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
                String::from_utf16_lossy(&w)
            } else {
                b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect()
            };
            // Keep the first index of a spelling, as FName does.
            map.entry(text.to_ascii_lowercase()).or_insert(c.count + k as u32);
        }
        c.count = num;
    }
    map.get(&want).copied()
}
