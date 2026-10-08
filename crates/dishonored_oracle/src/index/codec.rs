//! A small binary codec for the index files: little-endian fixed-width numbers, length-prefixed
//! strings and vectors, no per-field dispatch. The index is decoded on every query, and this is
//! several times faster than the generic serde path for its hundreds of thousands of rows.

use anyhow::{Result, bail};

pub struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Rd { b, p: 0 }
    }
    #[inline]
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.p + n > self.b.len() {
            bail!("index file truncated at {}", self.p);
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    pub fn done(&self) -> bool {
        self.p == self.b.len()
    }
}

pub trait Codec: Sized {
    fn enc(&self, w: &mut Vec<u8>);
    fn dec(r: &mut Rd) -> Result<Self>;
}

macro_rules! num {
    ($($t:ty),*) => {$(
        impl Codec for $t {
            #[inline]
            fn enc(&self, w: &mut Vec<u8>) {
                w.extend_from_slice(&self.to_le_bytes());
            }
            #[inline]
            fn dec(r: &mut Rd) -> Result<Self> {
                Ok(<$t>::from_le_bytes(r.take(std::mem::size_of::<$t>())?.try_into().unwrap()))
            }
        }
    )*};
}
num!(u8, u16, u32, u64, i32, f32);

impl Codec for bool {
    fn enc(&self, w: &mut Vec<u8>) {
        w.push(*self as u8);
    }
    fn dec(r: &mut Rd) -> Result<Self> {
        Ok(u8::dec(r)? != 0)
    }
}

impl Codec for char {
    fn enc(&self, w: &mut Vec<u8>) {
        (*self as u32).enc(w);
    }
    fn dec(r: &mut Rd) -> Result<Self> {
        Ok(char::from_u32(u32::dec(r)?).unwrap_or('?'))
    }
}

impl Codec for String {
    fn enc(&self, w: &mut Vec<u8>) {
        (self.len() as u32).enc(w);
        w.extend_from_slice(self.as_bytes());
    }
    fn dec(r: &mut Rd) -> Result<Self> {
        let n = u32::dec(r)? as usize;
        if n == 0 {
            return Ok(String::new());
        }
        Ok(std::str::from_utf8(r.take(n)?)?.to_owned())
    }
}

impl<T: Codec> Codec for Option<T> {
    fn enc(&self, w: &mut Vec<u8>) {
        match self {
            None => w.push(0),
            Some(v) => {
                w.push(1);
                v.enc(w);
            }
        }
    }
    fn dec(r: &mut Rd) -> Result<Self> {
        Ok(if u8::dec(r)? == 0 { None } else { Some(T::dec(r)?) })
    }
}

impl<T: Codec> Codec for Vec<T> {
    fn enc(&self, w: &mut Vec<u8>) {
        (self.len() as u32).enc(w);
        for v in self {
            v.enc(w);
        }
    }
    fn dec(r: &mut Rd) -> Result<Self> {
        let n = u32::dec(r)? as usize;
        if n > r.b.len() {
            bail!("implausible vector length {n}");
        }
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            v.push(T::dec(r)?);
        }
        Ok(v)
    }
}

impl<A: Codec, B: Codec> Codec for (A, B) {
    fn enc(&self, w: &mut Vec<u8>) {
        self.0.enc(w);
        self.1.enc(w);
    }
    fn dec(r: &mut Rd) -> Result<Self> {
        Ok((A::dec(r)?, B::dec(r)?))
    }
}

impl<A: Codec, B: Codec, C: Codec> Codec for (A, B, C) {
    fn enc(&self, w: &mut Vec<u8>) {
        self.0.enc(w);
        self.1.enc(w);
        self.2.enc(w);
    }
    fn dec(r: &mut Rd) -> Result<Self> {
        Ok((A::dec(r)?, B::dec(r)?, C::dec(r)?))
    }
}

impl<A: Codec, B: Codec, C: Codec, D: Codec, E: Codec> Codec for (A, B, C, D, E) {
    fn enc(&self, w: &mut Vec<u8>) {
        self.0.enc(w);
        self.1.enc(w);
        self.2.enc(w);
        self.3.enc(w);
        self.4.enc(w);
    }
    fn dec(r: &mut Rd) -> Result<Self> {
        Ok((A::dec(r)?, B::dec(r)?, C::dec(r)?, D::dec(r)?, E::dec(r)?))
    }
}

impl<const N: usize> Codec for [f32; N] {
    fn enc(&self, w: &mut Vec<u8>) {
        for v in self {
            v.enc(w);
        }
    }
    fn dec(r: &mut Rd) -> Result<Self> {
        let mut a = [0f32; N];
        for v in a.iter_mut() {
            *v = f32::dec(r)?;
        }
        Ok(a)
    }
}

/// `codec!(Type { field, ... });` implements [`Codec`] field by field.
#[macro_export]
macro_rules! codec {
    ($t:ty { $($f:ident),* $(,)? }) => {
        impl $crate::index::codec::Codec for $t {
            fn enc(&self, w: &mut Vec<u8>) {
                $( $crate::index::codec::Codec::enc(&self.$f, w); )*
            }
            fn dec(r: &mut $crate::index::codec::Rd) -> anyhow::Result<Self> {
                Ok(Self { $( $f: $crate::index::codec::Codec::dec(r)?, )* ..Default::default() })
            }
        }
    };
}

pub fn encode<T: Codec>(magic: &[u8; 8], v: &T) -> Vec<u8> {
    let mut w = Vec::with_capacity(1 << 20);
    w.extend_from_slice(magic);
    v.enc(&mut w);
    w
}

pub fn decode<T: Codec>(magic: &[u8; 8], b: &[u8]) -> Result<T> {
    if b.len() < 8 || &b[..8] != magic {
        bail!("not an index file of this version (rebuild with `oracle index build`)");
    }
    let mut r = Rd::new(&b[8..]);
    let v = T::dec(&mut r)?;
    if !r.done() {
        bail!("trailing bytes in the index file");
    }
    Ok(v)
}
