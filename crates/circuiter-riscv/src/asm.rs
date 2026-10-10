//! An RV32I assembler.
//!
//! The usual GNU-style syntax: labels (`loop:`), comments (`#`, `//`, `;`), ABI
//! register names, `offset(reg)` memory operands, the common pseudo-instructions (`li`,
//! `la`, `mv`, `j`, `call`, `ret`, `beqz`…), `%hi`/`%lo`, and the data directives
//! `.word .half .byte .ascii .asciz .string .space .zero .align .balign .equ .set`.
//! Code goes in `.text` from address 0; `.data` starts at [`DATA_BASE`], the data
//! memory of Circuiter's example processor.

use std::collections::BTreeMap;
use std::fmt;

pub const TEXT_BASE: u32 = 0;
pub const DATA_BASE: u32 = 0x1000_0000;

/// An assembled program.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Program {
    /// Instructions, little-endian, from [`TEXT_BASE`].
    pub text: Vec<u8>,
    /// Initialised data, from [`DATA_BASE`].
    pub data: Vec<u8>,
    pub symbols: BTreeMap<String, u32>,
    /// For each instruction address, the source line (1-based) it came from.
    pub lines: Vec<(u32, usize)>,
}

impl Program {
    /// The instruction words.
    pub fn words(&self) -> Vec<u32> {
        self.text.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
    }

    /// The source line of the instruction at `addr`.
    pub fn line_of(&self, addr: u32) -> Option<usize> {
        self.lines.iter().find(|(a, _)| *a == addr).map(|(_, l)| *l)
    }
}

/// A problem on a line of the source.
#[derive(Debug, Clone, PartialEq)]
pub struct AsmError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for AsmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Section {
    Text,
    Data,
}

/// One statement after the first pass: where it goes and how big it is.
#[derive(Debug)]
struct Stmt {
    line: usize,
    section: Section,
    addr: u32,
    kind: StmtKind,
}

#[derive(Debug)]
enum StmtKind {
    /// Mnemonic and operands; `size` is in instructions.
    Instr {
        op: String,
        args: Vec<String>,
        size: u32,
    },
    /// Data items of `width` bytes each, as expressions.
    Data {
        width: u32,
        items: Vec<String>,
    },
    Bytes(Vec<u8>),
    Fill(u32),
}

/// Assembles `source`. Every problem found is returned, not just the first.
pub fn assemble(source: &str) -> Result<Program, Vec<AsmError>> {
    let mut errors = Vec::new();
    let mut symbols: BTreeMap<String, u32> = BTreeMap::new();
    let mut stmts = Vec::new();
    let mut section = Section::Text;
    let mut pc = [TEXT_BASE, DATA_BASE];
    let at = |s: Section| if s == Section::Text { 0 } else { 1 };

    // Pass 1: lay everything out and collect the labels.
    for (n, raw) in source.lines().enumerate() {
        let line = n + 1;
        let mut text = strip_comment(raw).trim();
        // Any number of labels before the statement.
        while let Some(colon) = label_end(text) {
            let name = text[..colon].trim();
            if !is_symbol(name) {
                errors.push(AsmError { line, message: format!("`{name}` isn't a valid label") });
            } else if symbols.insert(name.to_owned(), pc[at(section)]).is_some() {
                errors.push(AsmError { line, message: format!("label `{name}` defined twice") });
            }
            text = text[colon + 1..].trim();
        }
        if text.is_empty() {
            continue;
        }
        let (op, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let op = op.to_ascii_lowercase();
        let args = split_args(rest.trim());
        let mut push = |kind: StmtKind, size: u32, pc: &mut [u32; 2]| {
            stmts.push(Stmt { line, section, addr: pc[at(section)], kind });
            pc[at(section)] += size;
        };
        match op.as_str() {
            ".text" => section = Section::Text,
            ".data" | ".rodata" | ".bss" => section = Section::Data,
            ".section" => {
                section = if args.first().is_some_and(|a| a.starts_with(".text")) {
                    Section::Text
                } else {
                    Section::Data
                }
            }
            ".globl" | ".global" | ".type" | ".size" | ".file" | ".option" | ".attribute" => {}
            ".equ" | ".set" => match args.as_slice() {
                [name, value] => match eval(value, &symbols, 0) {
                    Ok(v) => {
                        symbols.insert(name.clone(), v as u32);
                    }
                    Err(e) => errors.push(AsmError { line, message: e }),
                },
                _ => errors.push(AsmError { line, message: format!("{op} name, value") }),
            },
            ".word" | ".half" | ".byte" | ".2byte" | ".4byte" => {
                let width = match op.as_str() {
                    ".word" | ".4byte" => 4,
                    ".half" | ".2byte" => 2,
                    _ => 1,
                };
                let size = width * args.len() as u32;
                push(StmtKind::Data { width, items: args }, size, &mut pc);
            }
            ".ascii" | ".asciz" | ".string" => {
                let mut bytes = Vec::new();
                for a in &args {
                    match unquote(a) {
                        Some(s) => {
                            bytes.extend(s);
                            if op != ".ascii" {
                                bytes.push(0);
                            }
                        }
                        None => errors.push(AsmError { line, message: format!("{op} \"text\"") }),
                    }
                }
                let size = bytes.len() as u32;
                push(StmtKind::Bytes(bytes), size, &mut pc);
            }
            ".space" | ".zero" | ".skip" => match args.first().map(|a| eval(a, &symbols, 0)) {
                Some(Ok(n)) if n >= 0 => push(StmtKind::Fill(n as u32), n as u32, &mut pc),
                _ => errors.push(AsmError { line, message: format!("{op} needs a size") }),
            },
            ".align" | ".p2align" | ".balign" => {
                let n = args.first().map(|a| eval(a, &symbols, 0));
                let Some(Ok(n)) = n else {
                    errors.push(AsmError { line, message: format!("{op} needs a number") });
                    continue;
                };
                let align = if op == ".balign" { n as u32 } else { 1 << n };
                let here = pc[at(section)];
                let pad = (align - here % align.max(1)) % align.max(1);
                if pad > 0 {
                    push(StmtKind::Fill(pad), pad, &mut pc);
                }
            }
            d if d.starts_with('.') => {
                errors.push(AsmError { line, message: format!("unknown directive `{d}`") })
            }
            _ => {
                if section != Section::Text {
                    errors.push(AsmError { line, message: "instructions belong in .text".into() });
                    continue;
                }
                let size = match size_of(&op, &args, &symbols) {
                    Ok(s) => s,
                    Err(e) => {
                        errors.push(AsmError { line, message: e });
                        continue;
                    }
                };
                push(StmtKind::Instr { op, args, size }, size * 4, &mut pc);
            }
        }
    }

    // Pass 2: encode.
    let mut program = Program { symbols: symbols.clone(), ..Program::default() };
    for stmt in &stmts {
        let out = match stmt.section {
            Section::Text => &mut program.text,
            Section::Data => &mut program.data,
        };
        let base = if stmt.section == Section::Text { TEXT_BASE } else { DATA_BASE };
        let offset = (stmt.addr - base) as usize;
        if out.len() < offset {
            out.resize(offset, 0);
        }
        match &stmt.kind {
            StmtKind::Instr { op, args, size } => {
                match encode(op, args, stmt.addr, *size, &symbols) {
                    Ok(words) => {
                        debug_assert_eq!(words.len() as u32, *size, "{op}");
                        for (k, w) in words.iter().enumerate() {
                            out.extend(w.to_le_bytes());
                            program.lines.push((stmt.addr + 4 * k as u32, stmt.line));
                        }
                    }
                    Err(message) => {
                        errors.push(AsmError { line: stmt.line, message });
                        out.extend(std::iter::repeat_n(0, 4 * *size as usize));
                    }
                }
            }
            StmtKind::Data { width, items } => {
                for item in items {
                    let v = eval(item, &symbols, stmt.addr).unwrap_or_else(|e| {
                        errors.push(AsmError { line: stmt.line, message: e });
                        0
                    });
                    out.extend(&(v as u32).to_le_bytes()[..*width as usize]);
                }
            }
            StmtKind::Bytes(b) => out.extend(b),
            StmtKind::Fill(n) => out.extend(std::iter::repeat_n(0, *n as usize)),
        }
    }
    if errors.is_empty() {
        Ok(program)
    } else {
        errors.sort_by_key(|e| e.line);
        Err(errors)
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut prev = ' ';
    for (i, c) in line.char_indices() {
        match c {
            '"' if prev != '\\' => in_string = !in_string,
            '#' | ';' if !in_string => return &line[..i],
            '/' if !in_string && line[i..].starts_with("//") => return &line[..i],
            _ => {}
        }
        prev = c;
    }
    line
}

/// The colon ending a label at the start of `text`, if there is one.
fn label_end(text: &str) -> Option<usize> {
    let colon = text.find(':')?;
    let name = &text[..colon];
    (!name.contains(|c: char| c.is_whitespace() || c == '"' || c == '(')).then_some(colon)
}

fn is_symbol(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '.')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$'))
}

fn split_args(rest: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_string = false;
    let mut depth = 0;
    for c in rest.chars() {
        match c {
            '"' => {
                in_string = !in_string;
                cur.push(c);
            }
            '(' if !in_string => {
                depth += 1;
                cur.push(c);
            }
            ')' if !in_string => {
                depth -= 1;
                cur.push(c);
            }
            ',' if !in_string && depth == 0 => out.push(std::mem::take(&mut cur).trim().to_owned()),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_owned());
    }
    out
}

fn unquote(s: &str) -> Option<Vec<u8>> {
    let inner = s.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = Vec::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.push(match chars.next()? {
                'n' => b'\n',
                't' => b'\t',
                'r' => b'\r',
                '0' => 0,
                '\\' => b'\\',
                '"' => b'"',
                other => other as u8,
            });
        } else {
            let mut buf = [0; 4];
            out.extend(c.encode_utf8(&mut buf).bytes());
        }
    }
    Some(out)
}

/// A register by number or ABI name.
pub fn register(name: &str) -> Option<u32> {
    let n = name.trim().to_ascii_lowercase();
    if let Some(num) = n.strip_prefix('x') {
        return num.parse().ok().filter(|r| *r < 32);
    }
    let abi = [
        "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
        "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
        "t5", "t6",
    ];
    if n == "fp" {
        return Some(8);
    }
    abi.iter().position(|r| *r == n).map(|r| r as u32)
}

/// Evaluates an expression: numbers, characters, symbols, `.`, `+`, `-`, and
/// `%hi(...)` / `%lo(...)`. `here` is the address of the statement.
fn eval(expr: &str, symbols: &BTreeMap<String, u32>, here: u32) -> Result<i64, String> {
    let e = expr.trim();
    if e.is_empty() {
        return Err("missing value".into());
    }
    for (prefix, hi) in [("%hi(", true), ("%lo(", false)] {
        if let Some(inner) = e.strip_prefix(prefix).and_then(|r| r.strip_suffix(')')) {
            let v = eval(inner, symbols, here)? as u32;
            let lo = ((v as i32) << 20) >> 20;
            return Ok(if hi { i64::from(v.wrapping_sub(lo as u32) >> 12) } else { i64::from(lo) });
        }
    }
    // Sums and differences, left to right, of terms.
    let mut total: i64 = 0;
    let mut sign = 1;
    let mut term = String::new();
    let mut prev_operand = false;
    let flush = |term: &mut String, sign: i64, total: &mut i64| -> Result<(), String> {
        let t = term.trim();
        if !t.is_empty() {
            *total += sign * atom(t, symbols, here)?;
        }
        term.clear();
        Ok(())
    };
    let mut quoted = false;
    for c in e.chars() {
        if c == '\'' {
            quoted = !quoted;
        }
        if (c == '+' || c == '-') && !quoted && prev_operand {
            flush(&mut term, sign, &mut total)?;
            sign = if c == '-' { -1 } else { 1 };
            prev_operand = false;
            continue;
        }
        if !c.is_whitespace() {
            prev_operand = true;
        }
        term.push(c);
    }
    flush(&mut term, sign, &mut total)?;
    Ok(total)
}

fn atom(t: &str, symbols: &BTreeMap<String, u32>, here: u32) -> Result<i64, String> {
    let (neg, t) = match t.strip_prefix('-') {
        Some(r) => (true, r.trim()),
        None => (false, t),
    };
    let v = if t == "." {
        i64::from(here)
    } else if let Some(c) = t.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        let b = unquote(&format!("\"{c}\"")).filter(|b| b.len() == 1).ok_or("one character")?;
        i64::from(b[0])
    } else if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i64::from_str_radix(&h.replace('_', ""), 16).map_err(|_| format!("bad number `{t}`"))?
    } else if let Some(b) = t.strip_prefix("0b") {
        i64::from_str_radix(&b.replace('_', ""), 2).map_err(|_| format!("bad number `{t}`"))?
    } else if t.starts_with(|c: char| c.is_ascii_digit()) {
        t.replace('_', "").parse().map_err(|_| format!("bad number `{t}`"))?
    } else if let Some(&v) = symbols.get(t) {
        i64::from(v)
    } else {
        return Err(format!("unknown symbol `{t}`"));
    };
    Ok(if neg { -v } else { v })
}

fn fits_i12(v: i64) -> bool {
    (-2048..2048).contains(&v)
}

/// How many instructions a statement becomes.
fn size_of(op: &str, args: &[String], symbols: &BTreeMap<String, u32>) -> Result<u32, String> {
    Ok(match op {
        "li" => {
            // One instruction when the value is known now and small; else two.
            match args.get(1).map(|a| eval(a, symbols, 0)) {
                Some(Ok(v)) if fits_i12(v) => 1,
                Some(Ok(v)) if (v as i32 as i64 == v || v as u32 as i64 == v) && v & 0xFFF == 0 => {
                    1
                }
                _ => 2,
            }
        }
        "la" => 2,
        _ => 1,
    })
}

fn reg(args: &[String], i: usize) -> Result<u32, String> {
    let a = args.get(i).ok_or_else(|| format!("missing operand {}", i + 1))?;
    register(a).ok_or_else(|| format!("`{a}` isn't a register"))
}

fn want(args: &[String], n: usize, form: &str) -> Result<(), String> {
    if args.len() == n { Ok(()) } else { Err(format!("expected {form}")) }
}

/// `offset(reg)`, or a bare `(reg)`.
fn mem_operand(a: &str, symbols: &BTreeMap<String, u32>, here: u32) -> Result<(i64, u32), String> {
    let open = a.rfind('(').ok_or_else(|| format!("`{a}` isn't offset(register)"))?;
    let close = a.rfind(')').ok_or_else(|| format!("`{a}` isn't offset(register)"))?;
    let r = register(&a[open + 1..close]).ok_or_else(|| format!("`{a}`: not a register"))?;
    let off = if a[..open].trim().is_empty() { 0 } else { eval(&a[..open], symbols, here)? };
    Ok((off, r))
}

fn r_type(f7: u32, rs2: u32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 {
    (f7 << 25) | (rs2 << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op
}

fn i_type(imm: i64, rs1: u32, f3: u32, rd: u32, op: u32) -> Result<u32, String> {
    if !fits_i12(imm) {
        return Err(format!("{imm} doesn't fit in 12 bits (-2048..2047)"));
    }
    Ok((((imm as u32) & 0xFFF) << 20) | (rs1 << 15) | (f3 << 12) | (rd << 7) | op)
}

fn s_type(imm: i64, rs2: u32, rs1: u32, f3: u32) -> Result<u32, String> {
    if !fits_i12(imm) {
        return Err(format!("{imm} doesn't fit in 12 bits (-2048..2047)"));
    }
    let i = imm as u32;
    Ok((((i >> 5) & 0x7F) << 25)
        | (rs2 << 20)
        | (rs1 << 15)
        | (f3 << 12)
        | ((i & 0x1F) << 7)
        | 0x23)
}

fn b_type(off: i64, rs2: u32, rs1: u32, f3: u32) -> Result<u32, String> {
    if !(-4096..4096).contains(&off) || off % 2 != 0 {
        return Err(format!("branch target {off} bytes away is out of reach (±4 KiB)"));
    }
    let i = off as u32;
    Ok((((i >> 12) & 1) << 31)
        | (((i >> 5) & 0x3F) << 25)
        | (rs2 << 20)
        | (rs1 << 15)
        | (f3 << 12)
        | (((i >> 1) & 0xF) << 8)
        | (((i >> 11) & 1) << 7)
        | 0x63)
}

fn j_type(off: i64, rd: u32) -> Result<u32, String> {
    if !(-(1 << 20)..(1 << 20)).contains(&off) || off % 2 != 0 {
        return Err(format!("jump target {off} bytes away is out of reach (±1 MiB)"));
    }
    let i = off as u32;
    Ok((((i >> 20) & 1) << 31)
        | (((i >> 1) & 0x3FF) << 21)
        | (((i >> 11) & 1) << 20)
        | (((i >> 12) & 0xFF) << 12)
        | (rd << 7)
        | 0x6F)
}

fn u_type(imm20: u32, rd: u32, op: u32) -> u32 {
    (imm20 << 12) | (rd << 7) | op
}

/// `li`/`la` as LUI (or AUIPC) then ADDI.
fn hi_lo(v: u32) -> (u32, i64) {
    let lo = ((v as i32) << 20) >> 20;
    (v.wrapping_sub(lo as u32) >> 12, i64::from(lo))
}

/// Encodes one statement as `size` instructions (decided in the first pass).
fn encode(
    op: &str,
    a: &[String],
    pc: u32,
    size: u32,
    sy: &BTreeMap<String, u32>,
) -> Result<Vec<u32>, String> {
    let ev = |i: usize| -> Result<i64, String> {
        eval(a.get(i).ok_or_else(|| format!("missing operand {}", i + 1))?, sy, pc)
    };
    let target = |i: usize| -> Result<i64, String> { Ok(ev(i)? - i64::from(pc)) };
    let r3 = |f7: u32, f3: u32| -> Result<Vec<u32>, String> {
        want(a, 3, "rd, rs1, rs2")?;
        Ok(vec![r_type(f7, reg(a, 2)?, reg(a, 1)?, f3, reg(a, 0)?, 0x33)])
    };
    let imm = |f3: u32| -> Result<Vec<u32>, String> {
        want(a, 3, "rd, rs1, imm")?;
        Ok(vec![i_type(ev(2)?, reg(a, 1)?, f3, reg(a, 0)?, 0x13)?])
    };
    let shift = |f7: u32, f3: u32| -> Result<Vec<u32>, String> {
        want(a, 3, "rd, rs1, shamt")?;
        let s = ev(2)?;
        if !(0..32).contains(&s) {
            return Err(format!("shift amount {s} must be 0..31"));
        }
        Ok(vec![r_type(f7, s as u32, reg(a, 1)?, f3, reg(a, 0)?, 0x13)])
    };
    let load = |f3: u32| -> Result<Vec<u32>, String> {
        want(a, 2, "rd, offset(rs1)")?;
        let (off, rs1) = mem_operand(&a[1], sy, pc)?;
        Ok(vec![i_type(off, rs1, f3, reg(a, 0)?, 0x03)?])
    };
    let store = |f3: u32| -> Result<Vec<u32>, String> {
        want(a, 2, "rs2, offset(rs1)")?;
        let (off, rs1) = mem_operand(&a[1], sy, pc)?;
        Ok(vec![s_type(off, reg(a, 0)?, rs1, f3)?])
    };
    let branch = |f3: u32, swap: bool| -> Result<Vec<u32>, String> {
        want(a, 3, "rs1, rs2, label")?;
        let (x, y) = if swap { (reg(a, 1)?, reg(a, 0)?) } else { (reg(a, 0)?, reg(a, 1)?) };
        Ok(vec![b_type(target(2)?, y, x, f3)?])
    };
    let branch_zero = |f3: u32, rs_first: bool| -> Result<Vec<u32>, String> {
        want(a, 2, "rs, label")?;
        let r = reg(a, 0)?;
        let (rs1, rs2) = if rs_first { (r, 0) } else { (0, r) };
        Ok(vec![b_type(target(1)?, rs2, rs1, f3)?])
    };
    Ok(match op {
        "add" => r3(0, 0)?,
        "sub" => r3(0x20, 0)?,
        "sll" => r3(0, 1)?,
        "slt" => r3(0, 2)?,
        "sltu" => r3(0, 3)?,
        "xor" => r3(0, 4)?,
        "srl" => r3(0, 5)?,
        "sra" => r3(0x20, 5)?,
        "or" => r3(0, 6)?,
        "and" => r3(0, 7)?,
        "addi" => imm(0)?,
        "slti" => imm(2)?,
        "sltiu" => imm(3)?,
        "xori" => imm(4)?,
        "ori" => imm(6)?,
        "andi" => imm(7)?,
        "slli" => shift(0, 1)?,
        "srli" => shift(0, 5)?,
        "srai" => shift(0x20, 5)?,
        "lb" => load(0)?,
        "lh" => load(1)?,
        "lw" => load(2)?,
        "lbu" => load(4)?,
        "lhu" => load(5)?,
        "sb" => store(0)?,
        "sh" => store(1)?,
        "sw" => store(2)?,
        "beq" => branch(0, false)?,
        "bne" => branch(1, false)?,
        "blt" => branch(4, false)?,
        "bge" => branch(5, false)?,
        "bltu" => branch(6, false)?,
        "bgeu" => branch(7, false)?,
        "bgt" => branch(4, true)?,
        "ble" => branch(5, true)?,
        "bgtu" => branch(6, true)?,
        "bleu" => branch(7, true)?,
        "beqz" => branch_zero(0, true)?,
        "bnez" => branch_zero(1, true)?,
        "bltz" => branch_zero(4, true)?,
        "bgez" => branch_zero(5, true)?,
        "bgtz" => branch_zero(4, false)?,
        "blez" => branch_zero(5, false)?,
        "lui" | "auipc" => {
            want(a, 2, "rd, imm20")?;
            let v = ev(1)?;
            if !(-(1 << 19)..(1 << 20)).contains(&v) {
                return Err(format!("{v} doesn't fit in 20 bits"));
            }
            u_type((v as u32) & 0xFFFFF, reg(a, 0)?, if op == "lui" { 0x37 } else { 0x17 })
                .pipe(|w| vec![w])
        }
        "jal" => match a.len() {
            1 => vec![j_type(target(0)?, 1)?],
            2 => vec![j_type(target(1)?, reg(a, 0)?)?],
            _ => return Err("expected jal [rd,] label".into()),
        },
        "jalr" => match a.len() {
            1 => vec![i_type(0, reg(a, 0)?, 0, 1, 0x67)?],
            2 if a[1].contains('(') => {
                let (off, rs1) = mem_operand(&a[1], sy, pc)?;
                vec![i_type(off, rs1, 0, reg(a, 0)?, 0x67)?]
            }
            3 => vec![i_type(ev(2)?, reg(a, 1)?, 0, reg(a, 0)?, 0x67)?],
            _ => return Err("expected jalr rd, rs1, imm".into()),
        },
        "ecall" => vec![0x0000_0073],
        "ebreak" => vec![0x0010_0073],
        "fence" | "fence.i" => vec![0x0000_000F],
        // Pseudo-instructions.
        "nop" => vec![0x0000_0013],
        "mv" => {
            want(a, 2, "rd, rs")?;
            vec![i_type(0, reg(a, 1)?, 0, reg(a, 0)?, 0x13)?]
        }
        "not" => {
            want(a, 2, "rd, rs")?;
            vec![i_type(-1, reg(a, 1)?, 4, reg(a, 0)?, 0x13)?]
        }
        "neg" => {
            want(a, 2, "rd, rs")?;
            vec![r_type(0x20, reg(a, 1)?, 0, 0, reg(a, 0)?, 0x33)]
        }
        "seqz" => {
            want(a, 2, "rd, rs")?;
            vec![i_type(1, reg(a, 1)?, 3, reg(a, 0)?, 0x13)?]
        }
        "snez" => {
            want(a, 2, "rd, rs")?;
            vec![r_type(0, reg(a, 1)?, 0, 3, reg(a, 0)?, 0x33)]
        }
        "sltz" => {
            want(a, 2, "rd, rs")?;
            vec![r_type(0, 0, reg(a, 1)?, 2, reg(a, 0)?, 0x33)]
        }
        "sgtz" => {
            want(a, 2, "rd, rs")?;
            vec![r_type(0, reg(a, 1)?, 0, 2, reg(a, 0)?, 0x33)]
        }
        "j" => {
            want(a, 1, "label")?;
            vec![j_type(target(0)?, 0)?]
        }
        "jr" => {
            want(a, 1, "rs")?;
            vec![i_type(0, reg(a, 0)?, 0, 0, 0x67)?]
        }
        "ret" => vec![i_type(0, 1, 0, 0, 0x67)?],
        "call" => {
            want(a, 1, "label")?;
            vec![j_type(target(0)?, 1)?]
        }
        "tail" => {
            want(a, 1, "label")?;
            vec![j_type(target(0)?, 0)?]
        }
        "li" => {
            want(a, 2, "rd, value")?;
            let rd = reg(a, 0)?;
            let v = ev(1)?;
            if !(-(1i64 << 31)..(1i64 << 32)).contains(&v) {
                return Err(format!("{v} doesn't fit in 32 bits"));
            }
            let (hi, lo) = hi_lo(v as u32);
            match size {
                1 if fits_i12(v) => vec![i_type(v, 0, 0, rd, 0x13)?],
                1 => vec![u_type(hi, rd, 0x37)],
                _ => vec![u_type(hi, rd, 0x37), i_type(lo, rd, 0, rd, 0x13)?],
            }
        }
        "la" => {
            want(a, 2, "rd, symbol")?;
            let rd = reg(a, 0)?;
            let off = (ev(1)? - i64::from(pc)) as u32;
            let (hi, lo) = hi_lo(off);
            vec![u_type(hi & 0xFFFFF, rd, 0x17), i_type(lo, rd, 0, rd, 0x13)?]
        }
        other => return Err(format!("unknown instruction `{other}`")),
    })
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(src: &str) -> Vec<u32> {
        assemble(src).unwrap_or_else(|e| panic!("{e:?}")).words()
    }

    #[test]
    fn encodes_like_gnu_as() {
        assert_eq!(words("addi x1, x0, -5"), [0xFFB0_0093]);
        assert_eq!(words("add a0, a1, a2"), [0x00C5_8533]);
        assert_eq!(words("sub t0, t1, t2"), [0x4073_02B3]);
        assert_eq!(words("sw sp, -8(gp)"), [0xFE21_AC23]);
        assert_eq!(words("lw a5, 12(sp)"), [0x00C1_2783]);
        assert_eq!(words("srai a0, a0, 3"), [0x4035_5513]);
        assert_eq!(words("lui t0, 0x12345"), [0x1234_52B7]);
        assert_eq!(words("ecall"), [0x0000_0073]);
        assert_eq!(words("loop: beq x0, x0, loop"), [0x0000_0063]);
        assert_eq!(words("nop\nl: j l"), [0x0000_0013, 0x0000_006F]);
        assert_eq!(words("jal ra, f\nf: ret"), [0x0040_00EF, 0x0000_8067]);
    }

    /// The same source through LLVM's assembler (`llvm-mc -triple=riscv32
    /// -mattr=-c,-relax`) gives exactly these words.
    #[test]
    fn matches_llvm_mc() {
        let src = "addi x1, x0, -5\nadd a0, a1, a2\nsub t0, t1, t2\nsw sp, -8(gp)\n\
            lw a5, 12(sp)\nsrai a0, a0, 3\nlui t0, 0x12345\necall\nloop: beq x0, x0, loop\n\
            nop\nl: j l\njal ra, f\nf: ret\nli a0, 42\nli a0, 0x12345678\nli a0, -2048\n\
            slli a1,a1,31\nsltiu a0,a1,-1\nbltu a0,a1,f\nlhu a3, -2(a4)\nsb a0, 2047(a1)\n\
            auipc ra, 0xfffff\njalr ra, 16(t0)\nebreak";
        let llvm = [
            0xffb00093, 0x00c58533, 0x407302b3, 0xfe21ac23, 0x00c12783, 0x40355513, 0x123452b7,
            0x00000073, 0x00000063, 0x00000013, 0x0000006f, 0x004000ef, 0x00008067, 0x02a00513,
            0x12345537, 0x67850513, 0x80000513, 0x01f59593, 0xfff5b513, 0xfeb562e3, 0xffe75683,
            0x7ea58fa3, 0xfffff097, 0x010280e7, 0x00100073,
        ];
        assert_eq!(words(src), llvm);
    }

    #[test]
    fn pseudo_instructions_and_data() {
        assert_eq!(words("li a0, 42"), [0x02A0_0513]);
        // 0x12345678 = lui 0x12345 + addi 0x678
        assert_eq!(words("li a0, 0x12345678"), [0x1234_5537, 0x6785_0513]);
        // 0xFFFFF800 needs the high part rounded up for the negative low part.
        assert_eq!(words("li a0, -2048"), [0x8000_0513]);
        let p =
            assemble(".data\nmsg: .asciz \"hi\"\nn: .word 7, msg\n.text\nla a0, n\nlw a1, 0(a0)")
                .unwrap();
        assert_eq!(p.symbols["n"], DATA_BASE + 3);
        assert_eq!(&p.data[..3], b"hi\0");
        assert_eq!(&p.data[3..7], &7u32.to_le_bytes());
        assert_eq!(&p.data[7..11], &DATA_BASE.to_le_bytes());
        assert_eq!(p.words().len(), 3);
    }

    #[test]
    fn errors_name_their_lines() {
        let e = assemble("addi x1, x0, 5000\nfoo x1\nbeq x0, x0, nowhere").unwrap_err();
        let lines: Vec<usize> = e.iter().map(|e| e.line).collect();
        assert_eq!(lines, [1, 2, 3]);
        assert!(e.iter().any(|e| e.message.contains("12 bits")));
        assert!(e.iter().any(|e| e.message.contains("unknown instruction")));
        assert!(e.iter().any(|e| e.message.contains("nowhere")));
    }
}
