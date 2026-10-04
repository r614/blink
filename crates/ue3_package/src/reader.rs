//! Little-endian byte cursor used by every UE3 decoder in this crate.

use anyhow::{Result, bail};

#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            bail!("seek to {pos:#x} past end ({:#x})", self.data.len());
        }
        self.pos = pos;
        Ok(())
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.seek(self.pos + n)
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            bail!(
                "read of {n} bytes at {:#x} overruns buffer ({:#x})",
                self.pos,
                self.data.len()
            );
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.array()?))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    pub fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }

    /// Array count with a sanity limit so corrupt data fails fast instead of allocating.
    pub fn count(&mut self, elem_size_hint: usize) -> Result<usize> {
        let n = self.i32()?;
        if n < 0 {
            bail!("negative array count {n} at {:#x}", self.pos - 4);
        }
        let n = n as usize;
        if n.saturating_mul(elem_size_hint.max(1)) > self.remaining() {
            bail!(
                "array count {n} (x{elem_size_hint}B) at {:#x} exceeds remaining {}",
                self.pos - 4,
                self.remaining()
            );
        }
        Ok(n)
    }

    /// UE3 `FString`: i32 length including terminator; negative => UTF-16.
    pub fn fstring(&mut self) -> Result<String> {
        let len = self.i32()?;
        if len == 0 {
            return Ok(String::new());
        }
        if len > 0 {
            let len = len as usize;
            if len > 1 << 20 {
                bail!("implausible FString length {len} at {:#x}", self.pos - 4);
            }
            let b = self.bytes(len)?;
            let b = b.strip_suffix(&[0]).unwrap_or(b);
            Ok(b.iter().map(|&c| c as char).collect())
        } else {
            let len = len.unsigned_abs() as usize;
            if len > 1 << 20 {
                bail!("implausible FString length -{len} at {:#x}", self.pos - 4);
            }
            let b = self.bytes(len * 2)?;
            let units: Vec<u16> = b
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|&c| c != 0)
                .collect();
            Ok(String::from_utf16_lossy(&units))
        }
    }

    /// UE3 `FGuid`.
    pub fn guid(&mut self) -> Result<[u32; 4]> {
        Ok([self.u32()?, self.u32()?, self.u32()?, self.u32()?])
    }
}
