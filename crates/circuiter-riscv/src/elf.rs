//! Loading RV32 ELF executables (from C or Rust) into a [`Program`]: segments below
//! [`DATA_BASE`] are instructions, those at or above it are data.

use crate::asm::{DATA_BASE, Program, TEXT_BASE};

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2).map(|s| u16::from_le_bytes([s[0], s[1]])).ok_or("truncated ELF file".into())
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or("truncated ELF file".into())
}

/// Reads an executable linked for Circuiter's memory map: code at 0, data at
/// [`DATA_BASE`], entry point 0.
pub fn load(bytes: &[u8]) -> Result<Program, String> {
    if bytes.get(..4) != Some(b"\x7fELF") {
        return Err("not an ELF file".into());
    }
    if bytes[4] != 1 || bytes[5] != 1 {
        return Err("not a 32-bit little-endian ELF file (build for rv32)".into());
    }
    if u16_at(bytes, 18)? != 243 {
        return Err("not a RISC-V executable".into());
    }
    let entry = u32_at(bytes, 24)?;
    if entry != TEXT_BASE {
        return Err(format!(
            "the program starts at 0x{entry:08x}; link it to start at 0 (see Circuiter's linker script)"
        ));
    }
    let phoff = u32_at(bytes, 28)? as usize;
    let phentsize = u16_at(bytes, 42)? as usize;
    let phnum = u16_at(bytes, 44)? as usize;
    let mut program = Program::default();
    for k in 0..phnum {
        let ph = phoff + k * phentsize;
        if u32_at(bytes, ph)? != 1 {
            continue; // not PT_LOAD
        }
        let offset = u32_at(bytes, ph + 4)? as usize;
        let vaddr = u32_at(bytes, ph + 8)?;
        let filesz = u32_at(bytes, ph + 16)? as usize;
        let memsz = u32_at(bytes, ph + 20)? as usize;
        let contents = bytes.get(offset..offset + filesz).ok_or("truncated ELF segment")?;
        let (out, base) = if vaddr >= DATA_BASE {
            (&mut program.data, DATA_BASE)
        } else {
            (&mut program.text, TEXT_BASE)
        };
        let start = (vaddr - base) as usize;
        if out.len() < start + memsz {
            out.resize(start + memsz, 0);
        }
        out[start..start + filesz].copy_from_slice(contents);
    }
    if program.text.is_empty() {
        return Err("the ELF file has no code".into());
    }
    let pad = program.text.len().next_multiple_of(4);
    program.text.resize(pad, 0);
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built ELF: one code segment at 0 (`li a0, 7; ecall`), one data segment.
    fn tiny_elf() -> Vec<u8> {
        let code: Vec<u8> =
            [0x0070_0513u32, 0x0000_0073].iter().flat_map(|w| w.to_le_bytes()).collect();
        let data = [1u8, 2, 3, 4];
        let mut b = vec![0u8; 52 + 2 * 32];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = 1;
        b[5] = 1;
        b[18..20].copy_from_slice(&243u16.to_le_bytes());
        b[28..32].copy_from_slice(&52u32.to_le_bytes());
        b[42..44].copy_from_slice(&32u16.to_le_bytes());
        b[44..46].copy_from_slice(&2u16.to_le_bytes());
        let segs = [(0u32, &code[..], 8u32), (DATA_BASE, &data[..], 16u32)];
        let mut at = b.len();
        for (k, (vaddr, contents, memsz)) in segs.iter().enumerate() {
            let ph = 52 + 32 * k;
            b[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes());
            b[ph + 4..ph + 8].copy_from_slice(&(at as u32).to_le_bytes());
            b[ph + 8..ph + 12].copy_from_slice(&vaddr.to_le_bytes());
            b[ph + 16..ph + 20].copy_from_slice(&(contents.len() as u32).to_le_bytes());
            b[ph + 20..ph + 24].copy_from_slice(&memsz.to_le_bytes());
            at += contents.len();
        }
        for (_, contents, _) in segs {
            b.extend_from_slice(contents);
        }
        b
    }

    #[test]
    fn loads_code_and_data_segments() {
        let p = load(&tiny_elf()).unwrap();
        assert_eq!(p.words(), [0x0070_0513, 0x0000_0073]);
        assert_eq!(&p.data[..4], &[1, 2, 3, 4]);
        assert_eq!(p.data.len(), 16, "zero-filled to the segment's memory size");
        let mut cpu = crate::iss::Cpu::new(&p, 16);
        cpu.run(10).unwrap();
        assert_eq!(cpu.regs[10], 7);
        assert!(load(b"not an elf").is_err());
    }
}
