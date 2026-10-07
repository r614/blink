//! Minimal reader of serialized `UFunction` exports (UE3 file version 801, licensee 30):
//! enough to tell native functions from script ones and to size their bytecode.
//!
//! Layout (after the UObject header, i.e. NetIndex + tagged properties ending in `None`):
//! - `UField`: `Next` (object index);
//! - `UStruct`: `SuperStruct`, `ScriptText`, `Children`, `CppText` (object indices), `Line`,
//!   `TextPos`, `ScriptBytecodeSize` (in-memory size), `ScriptStorageSize` (serialized size),
//!   then `ScriptStorageSize` bytes of bytecode;
//! - `UFunction`: `iNative` (u16), `OperPrecedence` (u8), `FunctionFlags` (u32),
//!   `RepOffset` (u16, only with `FUNC_Net`), `FriendlyName` (FName).
//!
//! The reader checks that it ends exactly at the end of the export, so a layout mismatch is
//! reported instead of producing wrong numbers.

use anyhow::{Result, ensure};
use ue3_package::Package;
use ue3_package::properties::read_properties;
use ue3_package::reader::Reader;

pub const FUNC_FINAL: u32 = 0x0000_0001;
pub const FUNC_DEFINED: u32 = 0x0000_0002;
pub const FUNC_ITERATOR: u32 = 0x0000_0004;
pub const FUNC_LATENT: u32 = 0x0000_0008;
pub const FUNC_SIMULATED: u32 = 0x0000_0100;
pub const FUNC_NET: u32 = 0x0000_0040;
pub const FUNC_EXEC: u32 = 0x0000_0200;
pub const FUNC_NATIVE: u32 = 0x0000_0400;
pub const FUNC_EVENT: u32 = 0x0000_0800;
pub const FUNC_STATIC: u32 = 0x0000_2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionInfo {
    /// Native function index (`iNative`, 0 for functions bound by name).
    pub native_index: u16,
    pub flags: u32,
    /// Serialized bytecode bytes.
    pub script_size: u32,
}

impl FunctionInfo {
    pub fn is_native(&self) -> bool {
        self.flags & FUNC_NATIVE != 0
    }
    pub fn is_event(&self) -> bool {
        self.flags & FUNC_EVENT != 0
    }
}

pub fn read_function(pkg: &Package, export: usize) -> Result<FunctionInfo> {
    let data = pkg.export_data(export)?;
    let mut r = Reader::new(data);
    let _net_index = r.i32()?;
    let _props = read_properties(pkg, &mut r)?;
    let _next = r.i32()?;
    let _super_struct = r.i32()?;
    let _script_text = r.i32()?;
    let _children = r.i32()?;
    let _cpp_text = r.i32()?;
    let _line = r.i32()?;
    let _text_pos = r.i32()?;
    let _bytecode_size = r.i32()?;
    let storage = r.i32()?;
    ensure!((0..=data.len() as i32).contains(&storage), "implausible script size {storage}");
    r.skip(storage as usize)?;
    let native_index = r.u16()?;
    let _oper_precedence = r.u8()?;
    let flags = r.u32()?;
    if flags & FUNC_NET != 0 {
        let _rep_offset = r.u16()?;
    }
    let _friendly = (r.i32()?, r.i32()?);
    ensure!(r.remaining() == 0, "{} bytes left after the function", r.remaining());
    Ok(FunctionInfo { native_index, flags, script_size: storage as u32 })
}
