//! Circuiter's RISC-V tools: an RV32I [`asm`]sembler, an [`elf`] loader for programs
//! built from C or Rust, and a reference instruction-set simulator ([`iss`]) that
//! processors built from parts are checked against.

pub mod asm;
pub mod elf;
pub mod iss;

pub use asm::{AsmError, DATA_BASE, Program, TEXT_BASE, assemble};
