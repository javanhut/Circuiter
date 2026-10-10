//! The parts the digital simulator knows: sources, gates, arithmetic, storage, and
//! the RISC-V specific pieces a processor needs (immediate decoding, control, data
//! memory). Each is made by [`create`] from a model name and parameters, so cells can
//! name them in their digital view.
//!
//! | Model | Ports | Parameters |
//! |---|---|---|
//! | `const` | y | `value`, `width` |
//! | `clock` | clk | `period` or `frequency`, `duty` (0.5), `delay` (first rising edge; half a period) |
//! | `pulse` | y | `length`: high from the start for this long (a power-on reset) |
//! | `buf`, `not` | a → y | `width` |
//! | `and`, `or`, `nand`, `nor`, `xor`, `xnor` | a, b (, c, d…) → y | `inputs` (2), `width` |
//! | `mux` | d0…dN-1, sel → y | `inputs` (2), `width` |
//! | `adder` | a, b, cin → sum, cout | `width` |
//! | `add` | a, b → y | `width`: y = a + b, wrapping |
//! | `alu` | a, b, op → y, zero | `width` (32); `op` is RISC-V's {funct7[5], funct3} |
//! | `compare` | a, b → eq, lt, ltu | `width` |
//! | `dff` | d, clk (, rst, en) → q | `width`, `reset` (value), `has_reset`, `has_enable` |
//! | `regfile` | ra1, ra2, wa, wd, we, clk → rd1, rd2 | `width` (32), `count` (32), `zero` (x0 reads 0) |
//! | `counter` | clk, rst, en → q | `width` |
//! | `decoder` | a → y | `width` (of a; y has 2^width bits) |
//! | `rom` | addr → data | `abits`, `width`, `bytes` (byte-addressed); contents from data |
//! | `dmem` | addr, wd, we, size, clk → rd | `depth` (words); RISC-V loads and stores |
//! | `slice` | a → y | `from` (width of a), `lo`, `width` |
//! | `concat` | a, b (, c, d) → y | `widths`: comma-separated, a (most significant) first |
//! | `extend` | a → y | `from`, `width`, `signed` |
//! | `immgen` | instr → imm | RV32I immediates (I, S, B, U, J) |
//! | `control` | instr → reg_write, alu_a, alu_b, alu_op, mem_write, wb_sel, branch, jump, jalr, halt | RV32I main decoder |
//! | `stop` | a, clk | ends the simulation at a rising clock edge with `a` high |
//!
//! Every part takes `delay`: how long its outputs take to follow its inputs (1 ns by
//! default; 2 ns for memories).

use crate::{Eval, PortSpec, Primitive, Time, Value};
use std::collections::BTreeMap;

pub type Params = BTreeMap<String, String>;

/// Parameters from `(name, value)` pairs.
pub fn params(pairs: &[(&str, &str)]) -> Params {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Reads a number with an optional SI suffix: `10n`, `2.5meg`, `0x1f`, `100`.
pub fn number(text: &str) -> Option<f64> {
    let t = text.trim().to_ascii_lowercase();
    if let Some(hex) = t.strip_prefix("0x") {
        return u64::from_str_radix(&hex.replace('_', ""), 16).ok().map(|v| v as f64);
    }
    if let Some(bin) = t.strip_prefix("0b") {
        return u64::from_str_radix(&bin.replace('_', ""), 2).ok().map(|v| v as f64);
    }
    let end = t.find(|c: char| c.is_ascii_alphabetic() && c != 'e').unwrap_or(t.len());
    // `1e-9` is a number; `2meg` and `5m` have suffixes.
    let (num, suffix) = t.split_at(end);
    let (num, suffix) = if num.ends_with('e') && !suffix.is_empty() {
        (&t[..end - 1], &t[end - 1..])
    } else {
        (num, suffix)
    };
    let v: f64 = num.parse().ok()?;
    let scale = match suffix {
        "" | "s" | "hz" => 1.0,
        s if s.starts_with("meg") => 1e6,
        s if s.starts_with('t') => 1e12,
        s if s.starts_with('g') => 1e9,
        s if s.starts_with('k') => 1e3,
        s if s.starts_with("mil") => 25.4e-6,
        s if s.starts_with('m') => 1e-3,
        s if s.starts_with('u') || s.starts_with('µ') => 1e-6,
        s if s.starts_with('n') => 1e-9,
        s if s.starts_with('p') => 1e-12,
        s if s.starts_with('f') => 1e-15,
        _ => return None,
    };
    Some(v * scale)
}

struct Reader<'a> {
    model: &'a str,
    params: &'a Params,
}

impl Reader<'_> {
    fn raw(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str).filter(|v| !v.trim().is_empty())
    }

    fn num(&self, name: &str, default: f64) -> Result<f64, String> {
        match self.raw(name) {
            None => Ok(default),
            Some(v) => {
                number(v).ok_or_else(|| format!("{}: `{v}` isn't a number ({name})", self.model))
            }
        }
    }

    fn int(&self, name: &str, default: u64) -> Result<u64, String> {
        let v = self.num(name, default as f64)?;
        if v < 0.0 || v.fract() != 0.0 {
            return Err(format!("{}: {name} must be a whole number", self.model));
        }
        Ok(v as u64)
    }

    fn width(&self, name: &str, default: u32) -> Result<u32, String> {
        let w = self.int(name, u64::from(default))?;
        if !(1..=64).contains(&w) {
            return Err(format!("{}: {name} must be 1 to 64 bits", self.model));
        }
        Ok(w as u32)
    }

    fn time(&self, name: &str, default: f64) -> Result<Time, String> {
        Ok(crate::from_seconds(self.num(name, default)?))
    }

    fn flag(&self, name: &str) -> bool {
        matches!(self.raw(name), Some("1" | "true" | "yes"))
    }
}

/// Makes a part. `data` is the contents of a ROM (little-endian bytes).
pub fn create(
    model: &str,
    params: &Params,
    data: Option<&[u8]>,
) -> Result<Box<dyn Primitive>, String> {
    let r = Reader { model, params };
    let gate_delay = r.time("delay", 1e-9)?;
    let mem_delay = r.time("delay", 2e-9)?;
    let p: Box<dyn Primitive> = match model {
        "const" => {
            let width = r.width("width", 1)?;
            let value = r.num("value", 0.0)? as i128 as u64;
            Box::new(Const {
                ports: vec![PortSpec::output("y", width)],
                value: Value::new(width, value),
            })
        }
        "clock" => {
            let period = match r.raw("frequency") {
                Some(_) => crate::from_seconds(1.0 / r.num("frequency", 10e6)?.max(1e-3)),
                None => r.time("period", 100e-9)?,
            };
            if period < 2 {
                return Err("clock: period too short".into());
            }
            let duty = r.num("duty", 0.5)?.clamp(0.01, 0.99);
            let high = ((period as f64 * duty) as Time).clamp(1, period - 1);
            let first = r.time("delay", crate::seconds(period / 2))?;
            Box::new(Clock {
                ports: vec![PortSpec::output("clk", 1)],
                high,
                low: period - high,
                first,
                level: false,
                next: 0,
            })
        }
        "pulse" => Box::new(Pulse {
            ports: vec![PortSpec::output("y", 1)],
            length: r.time("length", 100e-9)?,
            started: false,
        }),
        "buf" | "not" => {
            let width = r.width("width", 1)?;
            Box::new(Gate {
                op: if model == "buf" { GateOp::Buf } else { GateOp::Not },
                ports: vec![PortSpec::input("a", width), PortSpec::output("y", width)],
                delay: gate_delay,
            })
        }
        "and" | "or" | "nand" | "nor" | "xor" | "xnor" => {
            let width = r.width("width", 1)?;
            let n = r.int("inputs", 2)? as usize;
            if !(2..=8).contains(&n) {
                return Err(format!("{model}: 2 to 8 inputs"));
            }
            let op = match model {
                "and" => GateOp::And,
                "or" => GateOp::Or,
                "nand" => GateOp::Nand,
                "nor" => GateOp::Nor,
                "xor" => GateOp::Xor,
                _ => GateOp::Xnor,
            };
            let mut ports: Vec<PortSpec> =
                (0..n).map(|i| PortSpec::input(&letter(i), width)).collect();
            ports.push(PortSpec::output("y", width));
            Box::new(Gate { op, ports, delay: gate_delay })
        }
        "mux" => {
            let width = r.width("width", 1)?;
            let n = r.int("inputs", 2)? as usize;
            if !(2..=16).contains(&n) {
                return Err("mux: 2 to 16 inputs".into());
            }
            let sel = usize::BITS - (n - 1).leading_zeros();
            let mut ports: Vec<PortSpec> =
                (0..n).map(|i| PortSpec::input(&format!("d{i}"), width)).collect();
            ports.push(PortSpec::input("sel", sel));
            ports.push(PortSpec::output("y", width));
            Box::new(Mux { n, ports, delay: gate_delay })
        }
        "adder" => {
            let width = r.width("width", 32)?;
            Box::new(Adder {
                ports: vec![
                    PortSpec::input("a", width),
                    PortSpec::input("b", width),
                    PortSpec::input("cin", 1),
                    PortSpec::output("sum", width),
                    PortSpec::output("cout", 1),
                ],
                delay: gate_delay,
            })
        }
        "add" => {
            let width = r.width("width", 32)?;
            Box::new(Adder {
                ports: vec![
                    PortSpec::input("a", width),
                    PortSpec::input("b", width),
                    PortSpec::output("y", width),
                ],
                delay: gate_delay,
            })
        }
        "alu" => {
            let width = r.width("width", 32)?;
            Box::new(Alu {
                ports: vec![
                    PortSpec::input("a", width),
                    PortSpec::input("b", width),
                    PortSpec::input("op", 4),
                    PortSpec::output("y", width),
                    PortSpec::output("zero", 1),
                ],
                delay: gate_delay,
            })
        }
        "compare" => {
            let width = r.width("width", 32)?;
            Box::new(Compare {
                ports: vec![
                    PortSpec::input("a", width),
                    PortSpec::input("b", width),
                    PortSpec::output("eq", 1),
                    PortSpec::output("lt", 1),
                    PortSpec::output("ltu", 1),
                ],
                delay: gate_delay,
            })
        }
        "dff" => {
            let width = r.width("width", 1)?;
            let reset = Value::new(width, r.num("reset", 0.0)? as u64);
            let (has_reset, has_enable) = (r.flag("has_reset"), r.flag("has_enable"));
            let mut ports = vec![PortSpec::input("d", width), PortSpec::input("clk", 1)];
            if has_reset {
                ports.push(PortSpec::input("rst", 1));
            }
            if has_enable {
                ports.push(PortSpec::input("en", 1));
            }
            ports.push(PortSpec::output("q", width));
            Box::new(Dff {
                ports,
                reset,
                has_reset,
                has_enable,
                last_clk: Value::unknown(1),
                q: Value::unknown(width),
                delay: gate_delay,
            })
        }
        "regfile" => {
            let width = r.width("width", 32)?;
            let count = r.int("count", 32)? as usize;
            if !(2..=1024).contains(&count) || !count.is_power_of_two() {
                return Err("regfile: count must be a power of two, 2 to 1024".into());
            }
            let abits = count.trailing_zeros();
            let zero = r.raw("zero").is_none_or(|v| v != "0" && v != "false");
            let mut regs = vec![Value::unknown(width); count];
            if zero {
                regs[0] = Value::new(width, 0);
            }
            Box::new(RegFile {
                ports: vec![
                    PortSpec::input("ra1", abits),
                    PortSpec::input("ra2", abits),
                    PortSpec::input("wa", abits),
                    PortSpec::input("wd", width),
                    PortSpec::input("we", 1),
                    PortSpec::input("clk", 1),
                    PortSpec::output("rd1", width),
                    PortSpec::output("rd2", width),
                ],
                regs,
                zero,
                last_clk: Value::unknown(1),
                delay: mem_delay,
            })
        }
        "counter" => {
            let width = r.width("width", 8)?;
            Box::new(Counter {
                ports: vec![
                    PortSpec::input("clk", 1),
                    PortSpec::input("rst", 1),
                    PortSpec::input("en", 1),
                    PortSpec::output("q", width),
                ],
                q: Value::unknown(width),
                last_clk: Value::unknown(1),
                delay: gate_delay,
            })
        }
        "decoder" => {
            let width = r.width("width", 2)?;
            if width > 6 {
                return Err("decoder: at most 6 inputs (64 outputs)".into());
            }
            Box::new(Decoder {
                ports: vec![PortSpec::input("a", width), PortSpec::output("y", 1 << width)],
                delay: gate_delay,
            })
        }
        "rom" => {
            let width = r.width("width", 32)?;
            let abits = r.width("abits", 32)?;
            let bytes = r.flag("bytes");
            let word_bytes = width.div_ceil(8) as usize;
            let data = data.unwrap_or(&[]);
            let words = data
                .chunks(word_bytes)
                .map(|c| {
                    let mut v = 0u64;
                    for (i, b) in c.iter().enumerate() {
                        v |= u64::from(*b) << (8 * i);
                    }
                    Value::new(width, v)
                })
                .collect();
            Box::new(Rom {
                ports: vec![PortSpec::input("addr", abits), PortSpec::output("data", width)],
                words,
                shift: if bytes { word_bytes.trailing_zeros() } else { 0 },
                delay: mem_delay,
            })
        }
        "dmem" => {
            let depth = r.int("depth", 1024)? as usize;
            if depth == 0 || depth > 1 << 24 {
                return Err("dmem: depth must be 1 to 16M words".into());
            }
            let mut words = vec![Value::new(32, 0); depth];
            if let Some(data) = data {
                for (i, c) in data.chunks(4).take(depth).enumerate() {
                    let mut v = 0u64;
                    for (k, b) in c.iter().enumerate() {
                        v |= u64::from(*b) << (8 * k);
                    }
                    words[i] = Value::new(32, v);
                }
            }
            Box::new(Dmem {
                ports: vec![
                    PortSpec::input("addr", 32),
                    PortSpec::input("wd", 32),
                    PortSpec::input("we", 1),
                    PortSpec::input("size", 3),
                    PortSpec::input("clk", 1),
                    PortSpec::output("rd", 32),
                ],
                words,
                last_clk: Value::unknown(1),
                delay: mem_delay,
            })
        }
        "slice" => {
            let from = r.width("from", 32)?;
            let lo = r.int("lo", 0)? as u32;
            let width = r.width("width", 1)?;
            if lo + width > from {
                return Err(format!(
                    "slice: bits {lo}..{} are outside a {from}-bit input",
                    lo + width
                ));
            }
            Box::new(Slice {
                ports: vec![PortSpec::input("a", from), PortSpec::output("y", width)],
                lo,
                delay: 0,
            })
        }
        "concat" => {
            let widths: Vec<u32> = r
                .raw("widths")
                .unwrap_or("1,1")
                .split(',')
                .map(|w| {
                    w.trim().parse::<u32>().map_err(|_| "concat: widths like `8,8`".to_string())
                })
                .collect::<Result<_, _>>()?;
            let total: u32 = widths.iter().sum();
            if !(2..=8).contains(&widths.len()) || widths.contains(&0) || total > 64 {
                return Err("concat: 2 to 8 inputs, 64 bits in all".into());
            }
            let mut ports: Vec<PortSpec> =
                widths.iter().enumerate().map(|(i, &w)| PortSpec::input(&letter(i), w)).collect();
            ports.push(PortSpec::output("y", total));
            Box::new(Concat { ports, delay: 0 })
        }
        "extend" => {
            let from = r.width("from", 8)?;
            let width = r.width("width", 32)?;
            Box::new(Extend {
                ports: vec![PortSpec::input("a", from), PortSpec::output("y", width)],
                signed: r.flag("signed"),
                delay: 0,
            })
        }
        "immgen" => Box::new(ImmGen {
            ports: vec![PortSpec::input("instr", 32), PortSpec::output("imm", 32)],
            delay: gate_delay,
        }),
        "control" => Box::new(Control {
            ports: vec![
                PortSpec::input("instr", 32),
                PortSpec::output("reg_write", 1),
                PortSpec::output("alu_a", 2),
                PortSpec::output("alu_b", 1),
                PortSpec::output("alu_op", 4),
                PortSpec::output("mem_write", 1),
                PortSpec::output("wb_sel", 2),
                PortSpec::output("branch", 1),
                PortSpec::output("jump", 1),
                PortSpec::output("jalr", 1),
                PortSpec::output("halt", 1),
            ],
            delay: gate_delay,
        }),
        "stop" => Box::new(Stop {
            ports: vec![PortSpec::input("a", 1), PortSpec::input("clk", 1)],
            last_clk: Value::unknown(1),
            stopped: false,
        }),
        other => return Err(format!("no digital part called `{other}`")),
    };
    Ok(p)
}

fn letter(i: usize) -> String {
    char::from(b'a' + i as u8).to_string()
}

/// Whether the clock went from 0 to 1.
fn rising(last: &mut Value, now: Value) -> bool {
    let rose = last.known() == Some(0) && now.known() == Some(1);
    *last = now;
    rose
}

/// A value set from outside the simulation: a switch, a button, or an analog level
/// read into the digital world. Change it with the handle, then
/// [`crate::Simulator::poke`] the element.
#[derive(Debug)]
pub struct Input {
    ports: Vec<PortSpec>,
    value: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Bit planes are packed as value bits, with all-ones meaning unknown.
    last: Option<u64>,
}

/// The unknown marker for [`Input`] values.
pub const INPUT_UNKNOWN: u64 = u64::MAX;

impl Input {
    /// An input `width` bits wide starting at `initial` ([`INPUT_UNKNOWN`] for X), and
    /// the handle that sets it.
    pub fn with_handle(
        width: u32,
        initial: u64,
    ) -> (Box<dyn Primitive>, std::sync::Arc<std::sync::atomic::AtomicU64>) {
        let value = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(initial));
        let part =
            Input { ports: vec![PortSpec::output("y", width)], value: value.clone(), last: None };
        (Box::new(part), value)
    }
}

impl Primitive for Input {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let v = self.value.load(std::sync::atomic::Ordering::Relaxed);
        if self.last != Some(v) {
            self.last = Some(v);
            let w = self.ports[0].width;
            let value = if v == INPUT_UNKNOWN { Value::unknown(w) } else { Value::new(w, v) };
            cx.drive(0, value, 0);
        }
    }
}

#[derive(Debug)]
struct Const {
    ports: Vec<PortSpec>,
    value: Value,
}

impl Primitive for Const {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        cx.drive(0, self.value, 0);
    }
}

#[derive(Debug)]
struct Clock {
    ports: Vec<PortSpec>,
    high: Time,
    low: Time,
    first: Time,
    level: bool,
    next: Time,
}

impl Primitive for Clock {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        if cx.now == 0 && self.next == 0 {
            cx.drive(0, Value::bit(false), 0);
            self.next = self.first;
        } else if cx.now >= self.next {
            self.level = !self.level;
            cx.drive(0, Value::bit(self.level), 0);
            self.next = cx.now + if self.level { self.high } else { self.low };
        }
        cx.wake_at(self.next);
    }
}

#[derive(Debug)]
struct Pulse {
    ports: Vec<PortSpec>,
    length: Time,
    started: bool,
}

impl Primitive for Pulse {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        if !self.started {
            self.started = true;
            cx.drive(0, Value::bit(true), 0);
            cx.wake_at(self.length);
        } else if cx.now >= self.length {
            cx.drive(0, Value::bit(false), 0);
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum GateOp {
    Buf,
    Not,
    And,
    Or,
    Nand,
    Nor,
    Xor,
    Xnor,
}

#[derive(Debug)]
struct Gate {
    op: GateOp,
    ports: Vec<PortSpec>,
    delay: Time,
}

impl Primitive for Gate {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let n = self.ports.len() - 1;
        let first = cx.input(0);
        let rest = (1..n).map(|i| cx.input(i));
        let y = match self.op {
            GateOp::Buf => first,
            GateOp::Not => first.invert(),
            GateOp::And => rest.fold(first, Value::and),
            GateOp::Nand => rest.fold(first, Value::and).invert(),
            GateOp::Or => rest.fold(first, Value::or),
            GateOp::Nor => rest.fold(first, Value::or).invert(),
            GateOp::Xor => rest.fold(first, Value::xor),
            GateOp::Xnor => rest.fold(first, Value::xor).invert(),
        };
        cx.drive(n, y, self.delay);
    }
}

#[derive(Debug)]
struct Mux {
    n: usize,
    ports: Vec<PortSpec>,
    delay: Time,
}

impl Primitive for Mux {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let width = self.ports[0].width;
        let y = match cx.input(self.n).known() {
            Some(s) if (s as usize) < self.n => cx.input(s as usize),
            Some(_) => Value::unknown(width),
            // An unknown select still gives a known output if every input agrees.
            None => {
                let first = cx.input(0);
                if (1..self.n).all(|i| cx.input(i) == first) {
                    first
                } else {
                    Value::unknown(width)
                }
            }
        };
        cx.drive(self.n + 1, y, self.delay);
    }
}

#[derive(Debug)]
struct Adder {
    ports: Vec<PortSpec>,
    delay: Time,
}

impl Primitive for Adder {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let w = self.ports[0].width;
        if self.ports.len() == 3 {
            // `add`: no carries.
            let y = match (cx.input(0).known(), cx.input(1).known()) {
                (Some(a), Some(b)) => Value::new(w, a.wrapping_add(b)),
                _ => Value::unknown(w),
            };
            cx.drive(2, y, self.delay);
            return;
        }
        match (cx.input(0).known(), cx.input(1).known(), cx.input(2).known()) {
            (Some(a), Some(b), Some(c)) => {
                let sum = u128::from(a) + u128::from(b) + u128::from(c);
                cx.drive(3, Value::new(w, sum as u64), self.delay);
                cx.drive(4, Value::bit((sum >> w) & 1 == 1), self.delay);
            }
            _ => {
                cx.drive(3, Value::unknown(w), self.delay);
                cx.drive(4, Value::unknown(1), self.delay);
            }
        }
    }
}

/// RISC-V's ALU operations, by {funct7[5], funct3}.
pub fn alu(op: u64, a: u64, b: u64, width: u32) -> u64 {
    let mask = if width >= 64 { u64::MAX } else { (1 << width) - 1 };
    let sa = Value::new(width, a).signed().unwrap_or(0);
    let sb = Value::new(width, b).signed().unwrap_or(0);
    let shamt = (b & u64::from(width.next_power_of_two() - 1)) as u32;
    let y = match op {
        0b0000 => a.wrapping_add(b),
        0b1000 => a.wrapping_sub(b),
        0b0001 => a.checked_shl(shamt).unwrap_or(0),
        0b0010 => (sa < sb) as u64,
        0b0011 => (a < b) as u64,
        0b0100 => a ^ b,
        0b0101 => a.checked_shr(shamt).unwrap_or(0),
        0b1101 => (sa >> shamt.min(63)) as u64,
        0b0110 => a | b,
        0b0111 => a & b,
        _ => a.wrapping_add(b),
    };
    y & mask
}

#[derive(Debug)]
struct Alu {
    ports: Vec<PortSpec>,
    delay: Time,
}

impl Primitive for Alu {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let w = self.ports[0].width;
        match (cx.input(0).known(), cx.input(1).known(), cx.input(2).known()) {
            (Some(a), Some(b), Some(op)) => {
                let y = alu(op, a, b, w);
                cx.drive(3, Value::new(w, y), self.delay);
                cx.drive(4, Value::bit(y == 0), self.delay);
            }
            _ => {
                cx.drive(3, Value::unknown(w), self.delay);
                cx.drive(4, Value::unknown(1), self.delay);
            }
        }
    }
}

#[derive(Debug)]
struct Compare {
    ports: Vec<PortSpec>,
    delay: Time,
}

impl Primitive for Compare {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let (a, b) = (cx.input(0), cx.input(1));
        match (a.known(), b.known()) {
            (Some(x), Some(y)) => {
                cx.drive(2, Value::bit(x == y), self.delay);
                cx.drive(3, Value::bit(a.signed() < b.signed()), self.delay);
                cx.drive(4, Value::bit(x < y), self.delay);
            }
            _ => {
                for p in 2..5 {
                    cx.drive(p, Value::unknown(1), self.delay);
                }
            }
        }
    }
}

#[derive(Debug)]
struct Dff {
    ports: Vec<PortSpec>,
    reset: Value,
    has_reset: bool,
    has_enable: bool,
    last_clk: Value,
    q: Value,
    delay: Time,
}

impl Primitive for Dff {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let mut k = 2;
        let rst = self.has_reset.then(|| {
            k += 1;
            cx.input(k - 1)
        });
        let en = self.has_enable.then(|| {
            k += 1;
            cx.input(k - 1)
        });
        if rising(&mut self.last_clk, cx.input(1)) {
            self.q = match (rst.map(Value::known), en.map(Value::known)) {
                (Some(Some(1)), _) => self.reset,
                (Some(None), _) => Value::unknown(self.reset.width()),
                (_, Some(Some(0))) => self.q,
                (_, Some(None)) => Value::unknown(self.reset.width()),
                _ => cx.input(0),
            };
            cx.drive(self.ports.len() - 1, self.q, self.delay);
        } else if cx.now == 0 {
            cx.drive(self.ports.len() - 1, self.q, 0);
        }
    }
}

#[derive(Debug)]
struct RegFile {
    ports: Vec<PortSpec>,
    regs: Vec<Value>,
    zero: bool,
    last_clk: Value,
    delay: Time,
}

impl Primitive for RegFile {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let width = self.ports[3].width;
        if rising(&mut self.last_clk, cx.input(5)) && cx.input(4).known() == Some(1) {
            match cx.input(2).known() {
                Some(a) if a == 0 && self.zero => {}
                Some(a) => self.regs[a as usize] = cx.input(3),
                // A write to an unknown register spoils them all.
                None => self
                    .regs
                    .iter_mut()
                    .skip(self.zero as usize)
                    .for_each(|r| *r = Value::unknown(width)),
            }
        }
        for (port, out) in [(0, 6), (1, 7)] {
            let v = match cx.input(port).known() {
                Some(a) => self.regs[a as usize],
                None => Value::unknown(width),
            };
            cx.drive(out, v, self.delay);
        }
    }

    fn memory(&self) -> Option<&[Value]> {
        Some(&self.regs)
    }
}

#[derive(Debug)]
struct Counter {
    ports: Vec<PortSpec>,
    q: Value,
    last_clk: Value,
    delay: Time,
}

impl Primitive for Counter {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let w = self.q.width();
        if rising(&mut self.last_clk, cx.input(0)) {
            self.q = match (cx.input(1).known(), cx.input(2).known(), self.q.known()) {
                (Some(1), ..) => Value::new(w, 0),
                (Some(0), Some(0), _) => self.q,
                (Some(0), Some(1), Some(q)) => Value::new(w, q.wrapping_add(1)),
                _ => Value::unknown(w),
            };
            cx.drive(3, self.q, self.delay);
        } else if cx.now == 0 {
            cx.drive(3, self.q, 0);
        }
    }
}

#[derive(Debug)]
struct Decoder {
    ports: Vec<PortSpec>,
    delay: Time,
}

impl Primitive for Decoder {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let w = self.ports[1].width;
        let y = match cx.input(0).known() {
            Some(a) => Value::new(w, 1 << a),
            None => Value::unknown(w),
        };
        cx.drive(1, y, self.delay);
    }
}

#[derive(Debug)]
struct Rom {
    ports: Vec<PortSpec>,
    words: Vec<Value>,
    shift: u32,
    delay: Time,
}

impl Primitive for Rom {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let w = self.ports[1].width;
        let v = match cx.input(0).known() {
            // Past the end of the program: zeros, as erased memory.
            Some(a) => {
                self.words.get((a >> self.shift) as usize).copied().unwrap_or(Value::new(w, 0))
            }
            None => Value::unknown(w),
        };
        cx.drive(1, v, self.delay);
    }

    fn memory(&self) -> Option<&[Value]> {
        Some(&self.words)
    }
}

/// RISC-V data memory: byte, halfword and word loads and stores, little-endian.
/// `size` is the instruction's funct3: 0 LB/SB, 1 LH/SH, 2 LW/SW, 4 LBU, 5 LHU.
#[derive(Debug)]
struct Dmem {
    ports: Vec<PortSpec>,
    words: Vec<Value>,
    last_clk: Value,
    delay: Time,
}

impl Dmem {
    fn index(&self, addr: u64) -> usize {
        (addr >> 2) as usize % self.words.len()
    }
}

impl Primitive for Dmem {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let (addr, size) = (cx.input(0).known(), cx.input(3).known());
        if rising(&mut self.last_clk, cx.input(4)) && cx.input(2).known() == Some(1) {
            if let (Some(a), Some(s)) = (addr, size) {
                let i = self.index(a);
                let shift = (a & 3) * 8;
                let (bits, mask) = match s & 3 {
                    0 => (8, 0xFF),
                    1 => (16, 0xFFFF),
                    _ => (32, 0xFFFF_FFFF),
                };
                let _ = bits;
                let wd = cx.input(1);
                self.words[i] = match (self.words[i].known(), wd.known()) {
                    (Some(old), Some(d)) => {
                        let m = (mask << shift) & 0xFFFF_FFFF;
                        Value::new(32, (old & !m) | ((d << shift) & m))
                    }
                    _ => Value::unknown(32),
                };
            }
        }
        let rd = match (addr, size) {
            (Some(a), Some(s)) => {
                let word = self.words[self.index(a)];
                match word.known() {
                    Some(w) => {
                        let v = w >> ((a & 3) * 8);
                        let value = match s {
                            0 => Value::new(8, v).signed().map(|x| x as u64),
                            1 => Value::new(16, v).signed().map(|x| x as u64),
                            4 => Some(v & 0xFF),
                            5 => Some(v & 0xFFFF),
                            _ => Some(w),
                        };
                        value.map_or(Value::unknown(32), |x| Value::new(32, x))
                    }
                    None => Value::unknown(32),
                }
            }
            _ => Value::unknown(32),
        };
        cx.drive(5, rd, self.delay);
    }

    fn memory(&self) -> Option<&[Value]> {
        Some(&self.words)
    }
}

#[derive(Debug)]
struct Slice {
    ports: Vec<PortSpec>,
    lo: u32,
    delay: Time,
}

impl Primitive for Slice {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let y = cx.raw(0).slice(self.lo, self.ports[1].width);
        cx.drive(1, y, self.delay);
    }
}

#[derive(Debug)]
struct Concat {
    ports: Vec<PortSpec>,
    delay: Time,
}

impl Primitive for Concat {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let n = self.ports.len() - 1;
        let y = (1..n).fold(cx.raw(0), |acc, i| acc.concat(cx.raw(i)));
        cx.drive(n, y, self.delay);
    }
}

#[derive(Debug)]
struct Extend {
    ports: Vec<PortSpec>,
    signed: bool,
    delay: Time,
}

impl Primitive for Extend {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let a = cx.input(0);
        let w = self.ports[1].width;
        let y = if self.signed {
            a.signed().map_or(Value::unknown(w), |v| Value::new(w, v as u64))
        } else {
            a.resize(w)
        };
        cx.drive(1, y, self.delay);
    }
}

/// The immediate of an RV32I instruction, sign-extended.
pub fn immediate(instr: u32) -> u32 {
    let i = instr as i32;
    let bits = |hi: u32, lo: u32| (instr >> lo) & ((1 << (hi - lo + 1)) - 1);
    match instr & 0x7F {
        // I-type: loads, arithmetic with an immediate, JALR.
        0x03 | 0x13 | 0x67 | 0x73 => (i >> 20) as u32,
        // S-type: stores.
        0x23 => (((i >> 25) << 5) as u32) | bits(11, 7),
        // B-type: branches.
        0x63 => {
            (((i >> 31) << 12) as u32)
                | (bits(7, 7) << 11)
                | (bits(30, 25) << 5)
                | (bits(11, 8) << 1)
        }
        // U-type: LUI, AUIPC.
        0x37 | 0x17 => instr & 0xFFFF_F000,
        // J-type: JAL.
        0x6F => {
            (((i >> 31) << 20) as u32)
                | (bits(19, 12) << 12)
                | (bits(20, 20) << 11)
                | (bits(30, 21) << 1)
        }
        _ => 0,
    }
}

#[derive(Debug)]
struct ImmGen {
    ports: Vec<PortSpec>,
    delay: Time,
}

impl Primitive for ImmGen {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        let y = match cx.input(0).known() {
            Some(instr) => Value::new(32, u64::from(immediate(instr as u32))),
            None => Value::unknown(32),
        };
        cx.drive(1, y, self.delay);
    }
}

/// The control signals for one RV32I instruction, in [`create`]'s `control` port
/// order: reg_write, alu_a (0 rs1, 1 pc, 2 zero), alu_b (0 rs2, 1 imm), alu_op,
/// mem_write, wb_sel (0 ALU, 1 memory, 2 pc + 4), branch, jump, jalr, halt.
pub fn control(instr: u32) -> [u64; 10] {
    let f3 = u64::from((instr >> 12) & 7);
    let f7b5 = u64::from((instr >> 30) & 1);
    match instr & 0x7F {
        0x33 => [1, 0, 0, (f7b5 << 3) | f3, 0, 0, 0, 0, 0, 0],
        0x13 => {
            // Only shifts right use funct7 (SRAI); ADDI's immediate may set that bit.
            let op = if f3 == 5 { (f7b5 << 3) | f3 } else { f3 };
            [1, 0, 1, op, 0, 0, 0, 0, 0, 0]
        }
        0x03 => [1, 0, 1, 0, 0, 1, 0, 0, 0, 0],
        0x23 => [0, 0, 1, 0, 1, 0, 0, 0, 0, 0],
        0x63 => [0, 0, 0, 0b1000, 0, 0, 1, 0, 0, 0],
        0x6F => [1, 0, 0, 0, 0, 2, 0, 1, 0, 0],
        0x67 => [1, 0, 1, 0, 0, 2, 0, 0, 1, 0],
        0x37 => [1, 2, 1, 0, 0, 0, 0, 0, 0, 0],
        0x17 => [1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
        0x73 => [0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
        // FENCE and anything unknown: do nothing.
        _ => [0; 10],
    }
}

#[derive(Debug)]
struct Control {
    ports: Vec<PortSpec>,
    delay: Time,
}

impl Primitive for Control {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        match cx.input(0).known() {
            Some(instr) => {
                for (k, v) in control(instr as u32).into_iter().enumerate() {
                    let w = self.ports[k + 1].width;
                    cx.drive(k + 1, Value::new(w, v), self.delay);
                }
            }
            None => {
                for k in 1..self.ports.len() {
                    let w = self.ports[k].width;
                    cx.drive(k, Value::unknown(w), self.delay);
                }
            }
        }
    }
}

#[derive(Debug)]
struct Stop {
    ports: Vec<PortSpec>,
    last_clk: Value,
    stopped: bool,
}

impl Primitive for Stop {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        if rising(&mut self.last_clk, cx.input(1)) && cx.input(0).known() == Some(1) {
            self.stopped = true;
        }
    }

    fn halted(&self) -> bool {
        self.stopped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_with_suffixes() {
        assert_eq!(number("10n"), Some(10e-9));
        assert_eq!(number("2meg"), Some(2e6));
        assert_eq!(number("0x1F"), Some(31.0));
        assert_eq!(number("1e-9"), Some(1e-9));
        assert_eq!(number("5m"), Some(5e-3));
        assert_eq!(number("fish"), None);
    }

    #[test]
    fn rv32i_immediates() {
        // addi x1, x0, -5
        assert_eq!(immediate(0xFFB0_0093) as i32, -5);
        // sw x2, -8(x3)
        assert_eq!(immediate(0xFE21_AC23) as i32, -8);
        // beq x0, x0, -4
        assert_eq!(immediate(0xFE00_0EE3) as i32, -4);
        // lui x5, 0x12345
        assert_eq!(immediate(0x1234_52B7), 0x1234_5000);
        // jal x1, 2048
        assert_eq!(immediate(0x0010_00EF) as i32, 2048);
    }

    #[test]
    fn alu_operations() {
        assert_eq!(alu(0b1000, 3, 5, 32), (-2i32) as u32 as u64);
        assert_eq!(alu(0b1101, 0x8000_0000, 4, 32), 0xF800_0000);
        assert_eq!(alu(0b0101, 0x8000_0000, 4, 32), 0x0800_0000);
        assert_eq!(alu(0b0010, (-1i32) as u32 as u64, 1, 32), 1, "slt is signed");
        assert_eq!(alu(0b0011, (-1i32) as u32 as u64, 1, 32), 0, "sltu isn't");
    }
}
