//! Fault-tolerant memory access inside the game process (Read/WriteProcessMemory on the
//! current process: a bad pointer yields `None` instead of an access violation).
use std::ffi::c_void;
use windows_sys::Win32::System::Diagnostics::Debug::{FlushInstructionCache, ReadProcessMemory, WriteProcessMemory};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

pub fn read_bytes(addr: u32, len: usize) -> Option<Vec<u8>> {
    if addr < 0x10000 {
        return None;
    }
    let mut buf = vec![0u8; len];
    let mut got = 0usize;
    let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), addr as *const c_void, buf.as_mut_ptr().cast(), len, &mut got) };
    (ok != 0 && got == len).then_some(buf)
}

pub fn read<T: Copy + Default>(addr: u32) -> Option<T> {
    if addr < 0x10000 {
        return None;
    }
    let mut v = T::default();
    let mut got = 0usize;
    let n = std::mem::size_of::<T>();
    let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), addr as *const c_void, (&mut v as *mut T).cast(), n, &mut got) };
    (ok != 0 && got == n).then_some(v)
}

pub fn u32(addr: u32) -> Option<u32> {
    read::<u32>(addr)
}

pub fn write_bytes(addr: u32, data: &[u8]) -> bool {
    if addr < 0x10000 {
        return false;
    }
    let mut put = 0usize;
    let ok = unsafe { WriteProcessMemory(GetCurrentProcess(), addr as *const c_void, data.as_ptr().cast(), data.len(), &mut put) };
    ok != 0 && put == data.len()
}

/// Overwrite code or read-only data (vtables), restoring the page protection afterwards.
pub unsafe fn patch(addr: u32, data: &[u8]) -> bool {
    unsafe {
        let mut old = 0u32;
        if VirtualProtect(addr as *const c_void, data.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return false;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), addr as *mut u8, data.len());
        let mut tmp = 0u32;
        VirtualProtect(addr as *const c_void, data.len(), old, &mut tmp);
        FlushInstructionCache(GetCurrentProcess(), addr as *const c_void, data.len());
        true
    }
}

/// Replace a vtable slot; returns the previous function.
pub unsafe fn patch_vtable(vtable: u32, slot: usize, func: u32) -> Option<u32> {
    let at = vtable + 4 * slot as u32;
    let old = u32(at)?;
    if old == func {
        return Some(old);
    }
    unsafe { patch(at, &func.to_le_bytes()) }.then_some(old)
}
