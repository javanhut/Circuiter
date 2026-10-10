//! Building C and Rust programs for Circuiter's RISC-V processor with the toolchains
//! installed on the system: clang (with LLVM's lld) for C; rustc and cargo for Rust.
//!
//! Programs are linked for the example processor's memory map: code from address 0,
//! read-only data, data and the stack in the 4 KiB data memory at 0x1000_0000. A small
//! start-up routine sets the stack pointer, calls `main` and then `ecall`s, which halts
//! the processor with `main`'s return value in a0.

use std::path::{Path, PathBuf};
use std::process::Command;

const LINK_SCRIPT: &str = r#"
OUTPUT_ARCH(riscv)
ENTRY(_start)
MEMORY {
  ROM (rx) : ORIGIN = 0x00000000, LENGTH = 1M
  RAM (rw) : ORIGIN = 0x10000000, LENGTH = 4K
}
SECTIONS {
  .text : { KEEP(*(.text.start)) *(.text .text.*) } > ROM
  /* The processor reads data only from data memory, so constants live there too. */
  .rodata : { *(.rodata .rodata.* .srodata .srodata.*) } > RAM
  .data : { *(.data .data.* .sdata .sdata.*) } > RAM
  .bss : { *(.bss .bss.* .sbss .sbss.* COMMON) } > RAM
  __global_pointer$ = ORIGIN(RAM) + 0x800;
  __stack_top = ORIGIN(RAM) + LENGTH(RAM);
  /DISCARD/ : { *(.comment) *(.riscv.attributes) *(.eh_frame*) }
}
"#;

const START: &str = r#"
  .section .text.start
  .globl _start
_start:
  .option push
  .option norelax
  la gp, __global_pointer$
  .option pop
  la sp, __stack_top
  call main
  ecall
1:
  j 1b
"#;

/// RV32I has no multiply or divide instructions; C's `*`, `/` and `%` call these.
const RUNTIME_C: &str = r#"
unsigned __mulsi3(unsigned a, unsigned b) {
  unsigned r = 0;
  while (b) { if (b & 1) r += a; a <<= 1; b >>= 1; }
  return r;
}
unsigned __udivsi3(unsigned n, unsigned d) {
  unsigned q = 0, r = 0;
  if (d == 0) return 0xffffffff;
  for (int i = 31; i >= 0; i--) {
    r = (r << 1) | ((n >> i) & 1);
    if (r >= d) { r -= d; q |= 1u << i; }
  }
  return q;
}
unsigned __umodsi3(unsigned n, unsigned d) { return d ? n - __udivsi3(n, d) * d : n; }
int __divsi3(int n, int d) {
  int neg = (n < 0) != (d < 0);
  unsigned q = __udivsi3(n < 0 ? -(unsigned)n : n, d < 0 ? -(unsigned)d : d);
  return neg ? -(int)q : (int)q;
}
int __modsi3(int n, int d) { return n - __divsi3(n, d) * d; }
"#;

/// Where builds happen: a cache directory that survives between runs, so Rust's
/// `core` is built only once.
fn work_dir(kind: &str) -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("circuiter").join(kind);
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    Ok(dir)
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if std::fs::read_to_string(path).ok().as_deref() == Some(text) {
        return Ok(());
    }
    std::fs::write(path, text).map_err(|e| format!("writing {}: {e}", path.display()))
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{what} isn't installed"),
        _ => format!("running {what}: {e}"),
    })?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).take(12).collect();
    Err(format!("{what} failed:\n{}", lines.join("\n")))
}

/// A file name for a program's build products, from its path.
fn stem(path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("program");
    format!("{name}-{:08x}", h.finish() as u32)
}

/// Compiles a C program to an RV32I executable with clang and lld.
pub fn compile_c(path: &Path) -> Result<Vec<u8>, String> {
    let dir = work_dir("c")?;
    let (ld, start, rt) = (dir.join("link.ld"), dir.join("start.s"), dir.join("runtime.c"));
    write(&ld, LINK_SCRIPT)?;
    write(&start, START)?;
    write(&rt, RUNTIME_C)?;
    let out = dir.join(format!("{}.elf", stem(path)));
    run(
        Command::new("clang")
            .args([
                "--target=riscv32-unknown-elf",
                "-march=rv32i",
                "-mabi=ilp32",
                "-mno-relax",
                "-O2",
                "-ffreestanding",
                "-fno-builtin",
                "-nostdlib",
                "-fuse-ld=lld",
                "-Wall",
            ])
            .arg("-T")
            .arg(&ld)
            .arg(&start)
            .arg(&rt)
            .arg(path)
            .arg("-o")
            .arg(&out),
        "clang (C compiler for RISC-V)",
    )?;
    std::fs::read(&out).map_err(|e| format!("reading {}: {e}", out.display()))
}

/// Compiles a Rust program to an RV32I executable. The file is a `no_std` program
/// whose entry point is `#[unsafe(no_mangle)] pub extern "C" fn main() -> u32`; the
/// panic handler and start-up code are supplied. Without the installed
/// `riscv32i-unknown-none-elf` target, Rust's `core` is built from source once.
pub fn compile_rust(path: &Path) -> Result<Vec<u8>, String> {
    let source = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = work_dir("rust")?.join(stem(path));
    std::fs::create_dir_all(dir.join("src")).map_err(|e| e.to_string())?;
    write(
        &dir.join("Cargo.toml"),
        "[package]\nname = \"program\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
         [profile.release]\npanic = \"abort\"\nopt-level = \"s\"\n\n\
         [profile.dev]\npanic = \"abort\"\n",
    )?;
    write(&dir.join("link.ld"), LINK_SCRIPT)?;
    let start_asm = START.replace('\n', "\n    ");
    write(
        &dir.join("src/main.rs"),
        &format!(
            "#![no_std]\n#![no_main]\n\
             core::arch::global_asm!(r#\"{start_asm}\"#);\n\
             #[panic_handler]\nfn panic(_: &core::panic::PanicInfo) -> ! {{ loop {{}} }}\n\
             include!({:?});\n",
            source.display().to_string()
        ),
    )?;
    let target = "riscv32i-unknown-none-elf";
    let sysroot = Command::new("rustc").args(["--print", "sysroot"]).output();
    let installed = sysroot.ok().is_some_and(|o| {
        Path::new(String::from_utf8_lossy(&o.stdout).trim())
            .join("lib/rustlib")
            .join(target)
            .is_dir()
    });
    let shared = work_dir("rust")?.join("target");
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&dir)
        .args(["build", "--release", "--quiet", "--target", target])
        .arg("--target-dir")
        .arg(&shared)
        .env("RUSTFLAGS", format!("-C link-arg=-T{}", dir.join("link.ld").display()));
    if !installed {
        // Build `core` from the installed Rust sources.
        cmd.env("RUSTC_BOOTSTRAP", "1").args([
            "-Zbuild-std=core,compiler_builtins",
            "-Zbuild-std-features=compiler-builtins-mem",
        ]);
    }
    run(&mut cmd, "cargo (Rust for RISC-V)")?;
    let out = shared.join(target).join("release/program");
    std::fs::read(&out).map_err(|e| format!("reading {}: {e}", out.display()))
}
