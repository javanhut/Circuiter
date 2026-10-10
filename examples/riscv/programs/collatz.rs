// A Rust program for the processor on the sheet. Set IMEM's program to
// programs/collatz.rs: Circuiter builds it with cargo when you press Run (the first
// build compiles Rust's `core` for RV32I, which takes a little while). main's return
// value ends up in register a0 (x10) when the program finishes.

/// Steps for 27 to reach 1 in the Collatz sequence.
#[unsafe(no_mangle)]
pub extern "C" fn main() -> u32 {
    let mut n: u32 = 27;
    let mut steps = 0;
    while n != 1 {
        n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 };
        steps += 1;
    }
    steps // 111
}
