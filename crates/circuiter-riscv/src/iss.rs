//! A reference RV32I instruction-set simulator: the golden model processors built from
//! parts are checked against. Its memory map matches Circuiter's example processor:
//! instructions from address 0 (read-only), data memory of `depth` words, every data
//! address wrapping into it (`(addr >> 2) % depth`).

use crate::asm::{DATA_BASE, Program};

#[derive(Debug, Clone)]
pub struct Cpu {
    pub regs: [u32; 32],
    pub pc: u32,
    pub text: Vec<u32>,
    pub data: Vec<u32>,
    /// Stopped by ECALL or EBREAK.
    pub halted: bool,
    pub steps: u64,
}

impl Cpu {
    /// A processor with `program` loaded and `depth` words of data memory.
    pub fn new(program: &Program, depth: usize) -> Cpu {
        let mut data = vec![0u32; depth];
        for (i, c) in program.data.chunks(4).take(depth).enumerate() {
            let mut b = [0; 4];
            b[..c.len()].copy_from_slice(c);
            data[i] = u32::from_le_bytes(b);
        }
        Cpu { regs: [0; 32], pc: 0, text: program.words(), data, halted: false, steps: 0 }
    }

    /// The stack pointer convention for this memory: the top of data memory.
    pub fn stack_top(&self) -> u32 {
        DATA_BASE + 4 * self.data.len() as u32
    }

    fn index(&self, addr: u32) -> usize {
        (addr >> 2) as usize % self.data.len()
    }

    fn load(&self, addr: u32, f3: u32) -> u32 {
        let w = self.data[self.index(addr)];
        let v = w >> ((addr & 3) * 8);
        match f3 {
            0 => v as u8 as i8 as i32 as u32,
            1 => v as u16 as i16 as i32 as u32,
            4 => v & 0xFF,
            5 => v & 0xFFFF,
            _ => w,
        }
    }

    fn store(&mut self, addr: u32, value: u32, f3: u32) {
        let i = self.index(addr);
        let shift = (addr & 3) * 8;
        let mask: u32 = match f3 {
            0 => 0xFF,
            1 => 0xFFFF,
            _ => 0xFFFF_FFFF,
        };
        let m = mask.wrapping_shl(shift);
        self.data[i] = (self.data[i] & !m) | (value.wrapping_shl(shift) & m);
    }

    /// Runs one instruction.
    pub fn step(&mut self) -> Result<(), String> {
        if self.halted {
            return Ok(());
        }
        let instr = *self
            .text
            .get((self.pc >> 2) as usize)
            .ok_or_else(|| format!("pc 0x{:08x} is past the end of the program", self.pc))?;
        let rd = ((instr >> 7) & 31) as usize;
        let rs1 = self.regs[((instr >> 15) & 31) as usize];
        let rs2 = self.regs[((instr >> 20) & 31) as usize];
        let f3 = (instr >> 12) & 7;
        let imm = circuiter_imm(instr);
        let mut next = self.pc.wrapping_add(4);
        let mut write = None;
        match instr & 0x7F {
            0x33 => {
                let op = (((instr >> 30) & 1) << 3) | f3;
                write = Some(alu(op, rs1, rs2));
            }
            0x13 => {
                let op = if f3 == 5 { (((instr >> 30) & 1) << 3) | f3 } else { f3 };
                write = Some(alu(op, rs1, imm));
            }
            0x03 => write = Some(self.load(rs1.wrapping_add(imm), f3)),
            0x23 => self.store(rs1.wrapping_add(imm), rs2, f3),
            0x63 => {
                let taken = match f3 {
                    0 => rs1 == rs2,
                    1 => rs1 != rs2,
                    4 => (rs1 as i32) < (rs2 as i32),
                    5 => (rs1 as i32) >= (rs2 as i32),
                    6 => rs1 < rs2,
                    7 => rs1 >= rs2,
                    _ => return Err(format!("bad branch at 0x{:08x}", self.pc)),
                };
                if taken {
                    next = self.pc.wrapping_add(imm);
                }
            }
            0x6F => {
                write = Some(next);
                next = self.pc.wrapping_add(imm);
            }
            0x67 => {
                write = Some(next);
                next = rs1.wrapping_add(imm) & !1;
            }
            0x37 => write = Some(imm),
            0x17 => write = Some(self.pc.wrapping_add(imm)),
            0x73 => self.halted = true,
            0x0F => {}
            _ => return Err(format!("unknown instruction 0x{instr:08x} at 0x{:08x}", self.pc)),
        }
        if let (Some(v), true) = (write, rd != 0) {
            self.regs[rd] = v;
        }
        if !self.halted {
            self.pc = next;
        }
        self.steps += 1;
        Ok(())
    }

    /// Runs until ECALL/EBREAK, or `max_steps` instructions.
    pub fn run(&mut self, max_steps: u64) -> Result<(), String> {
        while !self.halted {
            if self.steps >= max_steps {
                return Err(format!("still running after {max_steps} instructions"));
            }
            self.step()?;
        }
        Ok(())
    }
}

fn circuiter_imm(instr: u32) -> u32 {
    let i = instr as i32;
    let bits = |hi: u32, lo: u32| (instr >> lo) & ((1 << (hi - lo + 1)) - 1);
    match instr & 0x7F {
        0x03 | 0x13 | 0x67 | 0x73 => (i >> 20) as u32,
        0x23 => (((i >> 25) << 5) as u32) | bits(11, 7),
        0x63 => {
            (((i >> 31) << 12) as u32)
                | (bits(7, 7) << 11)
                | (bits(30, 25) << 5)
                | (bits(11, 8) << 1)
        }
        0x37 | 0x17 => instr & 0xFFFF_F000,
        0x6F => {
            (((i >> 31) << 20) as u32)
                | (bits(19, 12) << 12)
                | (bits(20, 20) << 11)
                | (bits(30, 21) << 1)
        }
        _ => 0,
    }
}

fn alu(op: u32, a: u32, b: u32) -> u32 {
    let sh = b & 31;
    match op {
        0b0000 => a.wrapping_add(b),
        0b1000 => a.wrapping_sub(b),
        0b0001 => a << sh,
        0b0010 => ((a as i32) < (b as i32)) as u32,
        0b0011 => (a < b) as u32,
        0b0100 => a ^ b,
        0b0101 => a >> sh,
        0b1101 => ((a as i32) >> sh) as u32,
        0b0110 => a | b,
        0b0111 => a & b,
        _ => a.wrapping_add(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asm::assemble;

    #[test]
    fn runs_a_program() {
        let p = assemble(
            "
            # Sum 1..10 into a0, store it, load it back into a1.
                li t0, 10
                li a0, 0
            loop:
                add a0, a0, t0
                addi t0, t0, -1
                bnez t0, loop
                la t1, result
                sw a0, 0(t1)
                lw a1, 0(t1)
                lb a2, 0(t1)
                ecall
            .data
            result: .word 0
            ",
        )
        .unwrap();
        let mut cpu = Cpu::new(&p, 256);
        cpu.run(1000).unwrap();
        assert_eq!(cpu.regs[10], 55);
        assert_eq!(cpu.regs[11], 55);
        assert_eq!(cpu.regs[12], 55);
        assert_eq!(cpu.data[0], 55);
    }

    #[test]
    fn signed_and_unsigned_comparisons_and_shifts() {
        let p = assemble(
            "li a0, -8\nsrai a1, a0, 1\nsrli a2, a0, 28\nslt a3, a0, zero\nsltu a4, a0, zero\necall",
        )
        .unwrap();
        let mut cpu = Cpu::new(&p, 16);
        cpu.run(100).unwrap();
        assert_eq!(cpu.regs[11] as i32, -4);
        assert_eq!(cpu.regs[12], 0xF);
        assert_eq!((cpu.regs[13], cpu.regs[14]), (1, 0));
    }
}
