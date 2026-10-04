//! Static analysis of the retail executable (read-only). Findings are only valid for
//! Dishonored.exe SHA-256 70da96138fe2171cca18888a0adbb731447cd15758e55535672625f09f6bc6fb.
//!
//! exe_probe sections
//! exe_probe find-bytes HEX                  file offsets + VAs of a byte pattern
//! exe_probe find-imm VALUE                  instructions using an immediate/displacement
//! exe_probe dis VA [COUNT]                  disassemble from a virtual address
//! exe_probe func VA                         disassemble the function containing VA
//! exe_probe xrefs VA                        call/jmp sites targeting VA
use anyhow::{Result, bail};
use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, IntelFormatter, OpKind, FlowControl};

const EXE: &str = r"C:\Program Files\GOG Galaxy\Games\Dishonored\Binaries\Win32\Dishonored.exe";

struct Section { name: String, va: u32, vsize: u32, raw: u32, rsize: u32 }
struct Pe { data: Vec<u8>, base: u32, sections: Vec<Section> }

impl Pe {
    fn load() -> Result<Pe> {
        let data = std::fs::read(std::env::var("DISHONORED_EXE").unwrap_or(EXE.into()))?;
        let pe = u32::from_le_bytes(data[0x3c..0x40].try_into()?) as usize;
        if &data[pe..pe + 4] != b"PE\0\0" { bail!("not PE"); }
        let nsec = u16::from_le_bytes(data[pe + 6..pe + 8].try_into()?) as usize;
        let opt = pe + 24;
        let opt_size = u16::from_le_bytes(data[pe + 20..pe + 22].try_into()?) as usize;
        let base = u32::from_le_bytes(data[opt + 28..opt + 32].try_into()?);
        let mut sections = Vec::new();
        for i in 0..nsec {
            let s = opt + opt_size + i * 40;
            let name = String::from_utf8_lossy(&data[s..s + 8]).trim_end_matches('\0').to_string();
            let u = |o: usize| u32::from_le_bytes(data[s + o..s + o + 4].try_into().unwrap());
            sections.push(Section { name, vsize: u(8), va: u(12), rsize: u(16), raw: u(20) });
        }
        Ok(Pe { data, base, sections })
    }
    fn off_to_va(&self, off: usize) -> Option<u32> {
        self.sections.iter().find(|s| (off as u32) >= s.raw && (off as u32) < s.raw + s.rsize).map(|s| self.base + s.va + (off as u32 - s.raw))
    }
    fn va_to_off(&self, va: u32) -> Option<usize> {
        let rva = va.checked_sub(self.base)?;
        self.sections.iter().find(|s| rva >= s.va && rva < s.va + s.vsize.max(s.rsize)).map(|s| (rva - s.va + s.raw) as usize)
    }
    fn text(&self) -> &Section { self.sections.iter().find(|s| s.name == ".text").unwrap() }
    fn decode_all(&self, mut f: impl FnMut(&Instruction)) {
        let t = self.text();
        let bytes = &self.data[t.raw as usize..(t.raw + t.rsize) as usize];
        let mut d = Decoder::with_ip(32, bytes, (self.base + t.va) as u64, DecoderOptions::NONE);
        let mut ins = Instruction::default();
        while d.can_decode() { d.decode_out(&mut ins); f(&ins); }
    }
}

fn fmt(ins: &Instruction) -> String { let mut s = String::new(); IntelFormatter::new().format(ins, &mut s); s }

fn dis(pe: &Pe, va: u32, count: usize) {
    let Some(off) = pe.va_to_off(va) else { println!("bad va"); return };
    let mut d = Decoder::with_ip(32, &pe.data[off..(off + count * 16).min(pe.data.len())], va as u64, DecoderOptions::NONE);
    let mut ins = Instruction::default();
    for _ in 0..count { if !d.can_decode() { break; } d.decode_out(&mut ins); println!("{:08x}  {}", ins.ip(), fmt(&ins)); if ins.mnemonic() == iced_x86::Mnemonic::Int3 { break; } }
}

fn func_start(pe: &Pe, va: u32) -> u32 {
    // Walk back to the previous run of int3/nop padding (MSVC aligns functions).
    let off = pe.va_to_off(va).unwrap();
    let mut o = off;
    while o > 2 { if pe.data[o - 1] == 0xCC && pe.data[o - 2] == 0xCC { break; } o -= 1; }
    pe.off_to_va(o).unwrap()
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let pe = Pe::load()?;
    let num = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap();
    match a.first().map(String::as_str) {
        Some("sections") => for s in &pe.sections { println!("{:8} va {:08x} vsize {:08x} raw {:08x} rsize {:08x}", s.name, pe.base + s.va, s.vsize, s.raw, s.rsize); },
        Some("find-bytes") => {
            let pat: Vec<u8> = (0..a[1].len()).step_by(2).map(|i| u8::from_str_radix(&a[1][i..i + 2], 16).unwrap()).collect();
            for (i, w) in pe.data.windows(pat.len()).enumerate() { if w == pat.as_slice() { println!("off {i:#x} va {:?}", pe.off_to_va(i).map(|v| format!("{v:08x}"))); } }
        }
        Some("find-imm") => {
            let v = num(&a[1]) as u64;
            pe.decode_all(|ins| {
                let mut hit = ins.memory_displacement64() == v && ins.memory_displacement64() != 0;
                for i in 0..ins.op_count() { if matches!(ins.op_kind(i), OpKind::Immediate32 | OpKind::Immediate8to32 | OpKind::Immediate16) && ins.immediate(i) == v { hit = true; } }
                if hit { println!("{:08x}  {}", ins.ip(), fmt(ins)); }
            });
        }
        Some("dis") => dis(&pe, num(&a[1]), a.get(2).and_then(|s| s.parse().ok()).unwrap_or(60)),
        Some("func") => { let s = func_start(&pe, num(&a[1])); println!("function starts ~{s:08x}"); dis(&pe, s, a.get(2).and_then(|s| s.parse().ok()).unwrap_or(400)); }
        Some("xrefs") => { let t = num(&a[1]) as u64; pe.decode_all(|ins| { if matches!(ins.flow_control(), FlowControl::Call | FlowControl::UnconditionalBranch | FlowControl::ConditionalBranch) && ins.near_branch_target() == t { println!("{:08x}  {}", ins.ip(), fmt(ins)); } }); }
        Some("funcs") => {
            // Function starts (after int3 padding) in [lo, hi).
            let (lo, hi) = (num(&a[1]), num(&a[2]));
            let (ol, oh) = (pe.va_to_off(lo).unwrap(), pe.va_to_off(hi).unwrap());
            let mut prev = None;
            for o in ol + 1..oh { if pe.data[o - 1] == 0xCC && pe.data[o] != 0xCC { let va = pe.off_to_va(o).unwrap(); if let Some(p) = prev { println!("{p:08x} size {}", va - p); } prev = Some(va); } }
        }
        _ => println!("see source for usage"),
    }
    Ok(())
}
