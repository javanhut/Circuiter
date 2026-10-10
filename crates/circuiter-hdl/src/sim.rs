//! Running a module: parameters and widths resolved, names bound to signal slots, and
//! an interpreter that evaluates continuous assignments, combinational blocks and
//! clocked blocks (non-blocking assignments take effect together) as a part of the
//! digital simulator.

use crate::parse::{Dir, Expr, HdlError, Item, LValue, Module, Stmt, err};
use circuiter_logic::{Eval, PortSpec, Primitive, Time, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
struct Signal {
    width: u32,
    signed: bool,
}

/// An expression with names bound and its natural width and signedness worked out.
#[derive(Debug, Clone)]
struct CExpr {
    kind: Kind,
    width: u32,
    signed: bool,
}

#[derive(Debug, Clone)]
enum Kind {
    Const(Value),
    Sig(usize),
    Index(Box<CExpr>, Box<CExpr>),
    /// Value, low bit, width.
    Slice(Box<CExpr>, u32),
    DynPart(Box<CExpr>, Box<CExpr>),
    Concat(Vec<CExpr>),
    Unary(&'static str, Box<CExpr>),
    Binary(&'static str, Box<CExpr>, Box<CExpr>),
    Ternary(Box<CExpr>, Box<CExpr>, Box<CExpr>),
    /// $signed / $unsigned: the operand, reinterpreted.
    Cast(Box<CExpr>),
    /// `'0`, `'1`, `'x`: every bit of the context.
    Fill(char),
}

#[derive(Debug, Clone)]
enum CLValue {
    /// Signal, low bit, width.
    Part(usize, u32, u32),
    /// Signal and a bit number computed at run time.
    Bit(usize, CExpr),
    Concat(Vec<CLValue>),
}

impl CLValue {
    fn width(&self) -> u32 {
        match self {
            CLValue::Part(_, _, w) => *w,
            CLValue::Bit(..) => 1,
            CLValue::Concat(v) => v.iter().map(CLValue::width).sum(),
        }
    }
}

#[derive(Debug, Clone)]
enum CStmt {
    Block(Vec<CStmt>),
    Assign(CLValue, CExpr, bool),
    If(CExpr, Box<CStmt>, Option<Box<CStmt>>),
    Case(CExpr, Vec<(Vec<CExpr>, CStmt)>, Option<Box<CStmt>>, bool),
    Null,
}

fn mask(w: u32) -> u64 {
    if w >= 64 { u64::MAX } else { (1 << w) - 1 }
}

struct Compiler<'m> {
    module: &'m Module,
    params: BTreeMap<String, i64>,
    signals: Vec<Signal>,
    names: BTreeMap<String, usize>,
}

impl Compiler<'_> {
    fn constant(&self, e: &Expr) -> Result<i64, HdlError> {
        let line = expr_line(e).unwrap_or(self.module.line);
        let c = self.expr(e)?;
        let v = eval(&c, &[], c.width)
            .known()
            .ok_or(HdlError { line, message: "must be a constant".into() })?;
        Ok(if c.signed { Value::new(c.width, v).signed().unwrap_or(v as i64) } else { v as i64 })
    }

    fn width(&self, range: &Option<(Expr, Expr)>, line: usize) -> Result<u32, HdlError> {
        match range {
            None => Ok(1),
            Some((msb, lsb)) => {
                let (m, l) = (self.constant(msb)?, self.constant(lsb)?);
                if l != 0 && m != 0 && l > m {
                    return err(line, "write ranges as [msb:lsb]");
                }
                let w = (m - l).unsigned_abs() + 1;
                if w > 64 {
                    return err(line, format!("{w} bits: up to 64 are supported"));
                }
                Ok(w as u32)
            }
        }
    }

    fn declare(
        &mut self,
        name: &str,
        width: u32,
        signed: bool,
        line: usize,
    ) -> Result<usize, HdlError> {
        if let Some(&k) = self.names.get(name) {
            // A port declared again in the body (`output reg q; reg q;`) is the same signal.
            return Ok(k);
        }
        if self.params.contains_key(name) {
            return err(line, format!("`{name}` is already a parameter"));
        }
        self.signals.push(Signal { width, signed });
        self.names.insert(name.to_owned(), self.signals.len() - 1);
        Ok(self.signals.len() - 1)
    }

    fn expr(&self, e: &Expr) -> Result<CExpr, HdlError> {
        Ok(match e {
            Expr::Num { width, bits, x, signed } => {
                let w = width.unwrap_or(32);
                CExpr {
                    kind: Kind::Const(Value::from_planes(w, *bits, *x, 0)),
                    width: w,
                    signed: *signed,
                }
            }
            Expr::Ident(name, line) => {
                if let Some(&p) = self.params.get(name) {
                    CExpr { kind: Kind::Const(Value::new(32, p as u64)), width: 32, signed: true }
                } else if let Some(&k) = self.names.get(name) {
                    let s = &self.signals[k];
                    CExpr { kind: Kind::Sig(k), width: s.width, signed: s.signed }
                } else {
                    return err(*line, format!("`{name}` isn't declared"));
                }
            }
            Expr::Index(a, i) => {
                let a = self.expr(a)?;
                let i = self.expr(i)?;
                CExpr { kind: Kind::Index(Box::new(a), Box::new(i)), width: 1, signed: false }
            }
            Expr::Slice(a, m, l) => {
                let (m, l) = (self.constant(m)?, self.constant(l)?);
                let a = self.expr(a)?;
                let (hi, lo) = (m.max(l), m.min(l));
                let line = expr_line(e).unwrap_or(0);
                if lo < 0 || hi >= i64::from(a.width) {
                    return err(
                        line,
                        format!("bits [{hi}:{lo}] are outside a {}-bit value", a.width),
                    );
                }
                let w = (hi - lo + 1) as u32;
                CExpr { kind: Kind::Slice(Box::new(a), lo as u32), width: w, signed: false }
            }
            Expr::IndexedPart(a, base, w) => {
                let w = self.constant(w)?;
                if !(1..=64).contains(&w) {
                    return err(expr_line(e).unwrap_or(0), "part width must be 1 to 64");
                }
                let a = self.expr(a)?;
                let base = self.expr(base)?;
                CExpr {
                    kind: Kind::DynPart(Box::new(a), Box::new(base)),
                    width: w as u32,
                    signed: false,
                }
            }
            Expr::Concat(parts) => {
                let parts: Vec<CExpr> =
                    parts.iter().map(|p| self.expr(p)).collect::<Result<_, _>>()?;
                let w: u32 = parts.iter().map(|p| p.width).sum();
                if w > 64 {
                    return err(expr_line(e).unwrap_or(0), "concatenation wider than 64 bits");
                }
                CExpr { kind: Kind::Concat(parts), width: w, signed: false }
            }
            Expr::Repeat(n, parts) => {
                let n = self.constant(n)?;
                let parts: Vec<CExpr> =
                    parts.iter().map(|p| self.expr(p)).collect::<Result<_, _>>()?;
                let mut all = Vec::new();
                for _ in 0..n.max(0) {
                    all.extend(parts.iter().cloned());
                }
                let w: u32 = all.iter().map(|p| p.width).sum();
                if w == 0 || w > 64 {
                    return err(expr_line(e).unwrap_or(0), "replication must be 1 to 64 bits wide");
                }
                CExpr { kind: Kind::Concat(all), width: w, signed: false }
            }
            Expr::Unary(op, a) => {
                let a = self.expr(a)?;
                let (w, s) = match *op {
                    "~" | "-" => (a.width, a.signed),
                    _ => (1, false),
                };
                CExpr { kind: Kind::Unary(op, Box::new(a)), width: w, signed: s }
            }
            Expr::Binary(op, a, b) => {
                let (a, b) = (self.expr(a)?, self.expr(b)?);
                let (w, s) = match *op {
                    "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" | "~^" | "^~" => {
                        (a.width.max(b.width), a.signed && b.signed)
                    }
                    "<<" | ">>" | "<<<" | ">>>" => (a.width, a.signed),
                    _ => (1, false),
                };
                CExpr { kind: Kind::Binary(op, Box::new(a), Box::new(b)), width: w, signed: s }
            }
            Expr::Ternary(c, a, b) => {
                let (c, a, b) = (self.expr(c)?, self.expr(a)?, self.expr(b)?);
                let w = a.width.max(b.width);
                let s = a.signed && b.signed;
                CExpr {
                    kind: Kind::Ternary(Box::new(c), Box::new(a), Box::new(b)),
                    width: w,
                    signed: s,
                }
            }
            Expr::Fill(c) => CExpr { kind: Kind::Fill(*c), width: 1, signed: false },
            Expr::Call(name, args, line) => match (name.as_str(), args.as_slice()) {
                ("signed" | "unsigned", [a]) => {
                    let a = self.expr(a)?;
                    let w = a.width;
                    CExpr { kind: Kind::Cast(Box::new(a)), width: w, signed: name == "signed" }
                }
                ("clog2", [a]) => {
                    let v = self.constant(a)?.max(1) as u64;
                    let bits = 64 - (v - 1).leading_zeros();
                    CExpr {
                        kind: Kind::Const(Value::new(32, u64::from(bits))),
                        width: 32,
                        signed: true,
                    }
                }
                _ => return err(*line, format!("`${name}` isn't supported")),
            },
        })
    }

    fn lvalue(&self, l: &LValue) -> Result<CLValue, HdlError> {
        let sig = |name: &str, line: usize| -> Result<usize, HdlError> {
            self.names
                .get(name)
                .copied()
                .ok_or(HdlError { line, message: format!("`{name}` isn't declared") })
        };
        Ok(match l {
            LValue::Var(n, line) => {
                let k = sig(n, *line)?;
                CLValue::Part(k, 0, self.signals[k].width)
            }
            LValue::Bit(n, i, line) => {
                let k = sig(n, *line)?;
                let i = self.expr(i)?;
                CLValue::Bit(k, i)
            }
            LValue::Part(n, m, l, line) => {
                let k = sig(n, *line)?;
                let (m, l) = (self.constant(m)?, self.constant(l)?);
                let (hi, lo) = (m.max(l), m.min(l));
                if lo < 0 || hi >= i64::from(self.signals[k].width) {
                    return err(*line, format!("bits [{hi}:{lo}] are outside `{n}`"));
                }
                CLValue::Part(k, lo as u32, (hi - lo + 1) as u32)
            }
            LValue::Concat(parts) => {
                CLValue::Concat(parts.iter().map(|p| self.lvalue(p)).collect::<Result<_, _>>()?)
            }
        })
    }

    fn stmt(&self, s: &Stmt) -> Result<CStmt, HdlError> {
        Ok(match s {
            Stmt::Block(v) => {
                CStmt::Block(v.iter().map(|s| self.stmt(s)).collect::<Result<_, _>>()?)
            }
            Stmt::Assign { lhs, rhs, nonblocking, .. } => {
                CStmt::Assign(self.lvalue(lhs)?, self.expr(rhs)?, *nonblocking)
            }
            Stmt::If(c, a, b) => CStmt::If(
                self.expr(c)?,
                Box::new(self.stmt(a)?),
                b.as_ref().map(|b| self.stmt(b)).transpose()?.map(Box::new),
            ),
            Stmt::Case(subject, arms, default, wild) => CStmt::Case(
                self.expr(subject)?,
                arms.iter()
                    .map(|(labels, body)| {
                        Ok((
                            labels.iter().map(|l| self.expr(l)).collect::<Result<_, _>>()?,
                            self.stmt(body)?,
                        ))
                    })
                    .collect::<Result<_, HdlError>>()?,
                default.as_ref().map(|d| self.stmt(d)).transpose()?.map(Box::new),
                *wild,
            ),
            Stmt::Null => CStmt::Null,
        })
    }
}

fn expr_line(e: &Expr) -> Option<usize> {
    match e {
        Expr::Ident(_, l) | Expr::Call(_, _, l) => Some(*l),
        Expr::Index(a, _) | Expr::Slice(a, ..) | Expr::IndexedPart(a, ..) | Expr::Unary(_, a) => {
            expr_line(a)
        }
        Expr::Binary(_, a, b) => expr_line(a).or_else(|| expr_line(b)),
        Expr::Ternary(c, ..) => expr_line(c),
        Expr::Concat(v) | Expr::Repeat(_, v) => v.iter().find_map(expr_line),
        Expr::Num { .. } | Expr::Fill(_) => None,
    }
}

/// `v` at width `w`, sign-extended if `signed`.
fn extend(v: Value, w: u32, signed: bool) -> Value {
    if w <= v.width() || !signed {
        return v.resize(w);
    }
    let (bits, x, z) = v.planes();
    let top = v.width() - 1;
    let fill = |plane: u64| {
        if plane >> top & 1 == 1 { plane | (mask(w) & !mask(v.width())) } else { plane }
    };
    Value::from_planes(w, fill(bits), fill(x), fill(z))
}

/// Evaluates `e` in a context `ctx` bits wide (Verilog sizes arithmetic by its context:
/// `{cout, sum} = a + b` keeps the carry).
fn eval(e: &CExpr, values: &[Value], ctx: u32) -> Value {
    let w = e.width.max(ctx).min(64);
    match &e.kind {
        Kind::Const(v) => extend(*v, w, e.signed),
        Kind::Sig(k) => extend(values[*k].as_input(), w, e.signed),
        Kind::Index(a, i) => {
            let v = eval(a, values, a.width);
            match eval(i, values, i.width).known() {
                Some(i) if i < u64::from(v.width()) => v.slice(i as u32, 1).resize(w),
                _ => Value::unknown(w),
            }
        }
        Kind::Slice(a, lo) => eval(a, values, a.width).slice(*lo, e.width).resize(w),
        Kind::DynPart(a, base) => {
            let v = eval(a, values, a.width);
            match eval(base, values, base.width).known() {
                Some(b) if b + u64::from(e.width) <= u64::from(v.width()) => {
                    v.slice(b as u32, e.width).resize(w)
                }
                _ => Value::unknown(w),
            }
        }
        Kind::Concat(parts) => {
            let mut it = parts.iter().map(|p| eval(p, values, p.width));
            let first = it.next().expect("non-empty");
            it.fold(first, Value::concat).resize(w)
        }
        Kind::Cast(a) => extend(eval(a, values, a.width), w, e.signed),
        Kind::Fill(c) => match c {
            '0' => Value::new(w, 0),
            '1' => Value::new(w, u64::MAX),
            'z' => Value::floating(w),
            _ => Value::unknown(w),
        },
        Kind::Unary(op, a) => {
            let v = eval(a, values, if *op == "~" || *op == "-" { w } else { a.width });
            match *op {
                "~" => v.invert(),
                "-" => match v.known() {
                    Some(x) => Value::new(w, x.wrapping_neg()),
                    None => Value::unknown(w),
                },
                "!" => logical(v).map_or(Value::unknown(1), |b| Value::bit(!b)).resize(w),
                "&" | "~&" | "|" | "~|" | "^" | "~^" => reduce(op, v).resize(w),
                _ => Value::unknown(w),
            }
        }
        Kind::Binary(op, a, b) => binary(op, a, b, values, w),
        Kind::Ternary(c, a, b) => {
            let cv = eval(c, values, c.width);
            match logical(cv) {
                Some(true) => eval(a, values, w),
                Some(false) => eval(b, values, w),
                None => {
                    let (x, y) = (eval(a, values, w), eval(b, values, w));
                    if x == y { x } else { Value::unknown(w) }
                }
            }
        }
    }
}

/// True if any bit is 1, false if all are 0, unknown otherwise.
fn logical(v: Value) -> Option<bool> {
    let (bits, x, z) = v.planes();
    if bits != 0 {
        Some(true)
    } else if x | z != 0 {
        None
    } else {
        Some(false)
    }
}

fn reduce(op: &str, v: Value) -> Value {
    let Some(bits) = v.known() else {
        // A known 0 decides AND, a known 1 decides OR.
        let (b, x, z) = v.planes();
        let known_zero = !b & !(x | z) & mask(v.width());
        return match op {
            "&" if known_zero != 0 => Value::bit(false),
            "~&" if known_zero != 0 => Value::bit(true),
            "|" if b != 0 => Value::bit(true),
            "~|" if b != 0 => Value::bit(false),
            _ => Value::unknown(1),
        };
    };
    let w = v.width();
    let r = match op {
        "&" => bits == mask(w),
        "~&" => bits != mask(w),
        "|" => bits != 0,
        "~|" => bits == 0,
        "^" => bits.count_ones() % 2 == 1,
        _ => bits.count_ones() % 2 == 0,
    };
    Value::bit(r)
}

fn binary(op: &str, a: &CExpr, b: &CExpr, values: &[Value], w: u32) -> Value {
    match op {
        "&" | "|" | "^" | "~^" | "^~" => {
            let (x, y) = (eval(a, values, w), eval(b, values, w));
            match op {
                "&" => x.and(y),
                "|" => x.or(y),
                "^" => x.xor(y),
                _ => x.xor(y).invert(),
            }
        }
        "+" | "-" | "*" | "/" | "%" => {
            let signed = a.signed && b.signed;
            let (x, y) = (eval(a, values, w), eval(b, values, w));
            let (Some(p), Some(q)) = (x.known(), y.known()) else { return Value::unknown(w) };
            let r = match op {
                "+" => p.wrapping_add(q),
                "-" => p.wrapping_sub(q),
                "*" => p.wrapping_mul(q),
                _ if q == 0 => return Value::unknown(w),
                _ if signed => {
                    let (sp, sq) = (x.signed().unwrap_or(0), y.signed().unwrap_or(1));
                    (if op == "/" { sp.wrapping_div(sq) } else { sp.wrapping_rem(sq) }) as u64
                }
                "/" => p / q,
                _ => p % q,
            };
            Value::new(w, r)
        }
        "<<" | ">>" | "<<<" | ">>>" => {
            let x = eval(a, values, w);
            let n = eval(b, values, b.width).known();
            let Some(n) = n else { return Value::unknown(w) };
            match (op, x.known()) {
                (_, None) => Value::unknown(w),
                ("<<" | "<<<", Some(v)) => Value::new(w, if n >= 64 { 0 } else { v << n }),
                (">>>", Some(_)) if a.signed => {
                    let s = x.signed().unwrap_or(0);
                    Value::new(w, (s >> n.min(63)) as u64)
                }
                (_, Some(v)) => Value::new(w, if n >= 64 { 0 } else { v >> n }),
            }
        }
        "&&" | "||" => {
            let (x, y) = (logical(eval(a, values, a.width)), logical(eval(b, values, b.width)));
            let r = match (op, x, y) {
                ("&&", Some(false), _) | ("&&", _, Some(false)) => Some(false),
                ("&&", Some(true), Some(true)) => Some(true),
                ("||", Some(true), _) | ("||", _, Some(true)) => Some(true),
                ("||", Some(false), Some(false)) => Some(false),
                _ => None,
            };
            r.map_or(Value::unknown(1), Value::bit).resize(w)
        }
        _ => {
            // Comparisons, at the wider operand's width.
            let cw = a.width.max(b.width);
            let signed = a.signed && b.signed;
            let (x, y) = (eval(a, values, cw), eval(b, values, cw));
            let r = match op {
                "===" => Some(x == y),
                "!==" => Some(x != y),
                _ => match (x.known(), y.known()) {
                    (Some(p), Some(q)) => {
                        let (sp, sq) = (x.signed().unwrap_or(0), y.signed().unwrap_or(0));
                        Some(match op {
                            "==" => p == q,
                            "!=" => p != q,
                            "<" if signed => sp < sq,
                            "<=" if signed => sp <= sq,
                            ">" if signed => sp > sq,
                            ">=" if signed => sp >= sq,
                            "<" => p < q,
                            "<=" => p <= q,
                            ">" => p > q,
                            _ => p >= q,
                        })
                    }
                    _ => None,
                },
            };
            r.map_or(Value::unknown(1), Value::bit).resize(w)
        }
    }
}

/// Writes `v` into `target`: a whole signal, a bit, a part, or several.
fn store(
    target: &CLValue,
    v: Value,
    values: &mut [Value],
    writes: &mut Vec<(usize, u32, Value)>,
    defer: bool,
) {
    match target {
        CLValue::Part(k, lo, w) => {
            let v = v.resize(*w);
            if defer {
                writes.push((*k, *lo, v));
            } else {
                values[*k] = put(values[*k], *lo, v);
            }
        }
        CLValue::Bit(k, i) => {
            let width = values[*k].width();
            match eval(i, values, i.width).known() {
                Some(i) if i < u64::from(width) => {
                    let bit = v.resize(1);
                    if defer {
                        writes.push((*k, i as u32, bit));
                    } else {
                        values[*k] = put(values[*k], i as u32, bit);
                    }
                }
                _ => {}
            }
        }
        CLValue::Concat(parts) => {
            // The last part takes the lowest bits.
            let mut lo = 0;
            for p in parts.iter().rev() {
                let w = p.width();
                store(p, v.slice(lo, w), values, writes, defer);
                lo += w;
            }
        }
    }
}

/// `old` with bits `lo..lo+w` replaced by `v`.
fn put(old: Value, lo: u32, v: Value) -> Value {
    let w = old.width();
    let m = mask(v.width()) << lo;
    let (ob, ox, oz) = old.planes();
    let (vb, vx, vz) = v.planes();
    let sh = |p: u64| if lo >= 64 { 0 } else { p << lo };
    Value::from_planes(w, (ob & !m) | sh(vb), (ox & !m) | sh(vx), (oz & !m) | sh(vz))
}

fn run(s: &CStmt, values: &mut [Value], writes: &mut Vec<(usize, u32, Value)>) {
    match s {
        CStmt::Block(v) => v.iter().for_each(|s| run(s, values, writes)),
        CStmt::Assign(lhs, rhs, nonblocking) => {
            let v = eval(rhs, values, lhs.width());
            store(lhs, v, values, writes, *nonblocking);
        }
        CStmt::If(c, a, b) => {
            // An unknown condition takes the else branch, as Verilog does.
            if logical(eval(c, values, c.width)) == Some(true) {
                run(a, values, writes);
            } else if let Some(b) = b {
                run(b, values, writes);
            }
        }
        CStmt::Case(subject, arms, default, wild) => {
            for (labels, body) in arms {
                for l in labels {
                    let w = subject.width.max(l.width);
                    let (s, lv) = (eval(subject, values, w), eval(l, values, w));
                    let hit = if *wild {
                        let (lb, lx, lz) = lv.planes();
                        let care = !(lx | lz) & mask(w);
                        s.known().is_some_and(|sv| sv & care == lb & care)
                    } else {
                        s.is_known() && s == lv
                    };
                    if hit {
                        run(body, values, writes);
                        return;
                    }
                }
            }
            if let Some(d) = default {
                run(d, values, writes);
            }
        }
        CStmt::Null => {}
    }
}

/// A module running as a part of the digital simulator.
#[derive(Debug)]
pub struct HdlPart {
    ports: Vec<PortSpec>,
    /// The signal slot of each port.
    slots: Vec<usize>,
    values: Vec<Value>,
    assigns: Vec<(CLValue, CExpr)>,
    combs: Vec<CStmt>,
    seqs: Vec<(Vec<(bool, usize)>, CStmt)>,
    last_edges: Vec<Vec<Value>>,
    driven: Vec<Option<Value>>,
    delay: Time,
}

impl HdlPart {
    fn settle(&mut self) {
        let mut scratch = Vec::new();
        for _ in 0..256 {
            let before = self.values.clone();
            for (lhs, rhs) in &self.assigns {
                let v = eval(rhs, &self.values, lhs.width());
                store(lhs, v, &mut self.values, &mut scratch, false);
            }
            for s in &self.combs {
                run(s, &mut self.values, &mut scratch);
            }
            if self.values == before {
                return;
            }
        }
        // Still changing: a combinational loop. What it drives is unknown.
        for (lhs, _) in &self.assigns {
            if let CLValue::Part(k, ..) = lhs {
                self.values[*k] = Value::unknown(self.values[*k].width());
            }
        }
    }
}

impl Primitive for HdlPart {
    fn ports(&self) -> &[PortSpec] {
        &self.ports
    }

    fn eval(&mut self, cx: &mut Eval<'_>) {
        for (p, spec) in self.ports.iter().enumerate() {
            if spec.dir != circuiter_logic::Dir::Out {
                self.values[self.slots[p]] = cx.raw(p);
            }
        }
        // Clocked blocks whose edge just came, all seeing the values from before.
        let mut writes = Vec::new();
        let snapshot = self.values.clone();
        for (k, (edges, body)) in self.seqs.iter().enumerate() {
            let mut fire = false;
            for (e, (rising, slot)) in edges.iter().enumerate() {
                let (was, now) =
                    (self.last_edges[k][e].known(), snapshot[*slot].as_input().known());
                if (*rising && was == Some(0) && now == Some(1))
                    || (!*rising && was == Some(1) && now == Some(0))
                {
                    fire = true;
                }
                self.last_edges[k][e] = snapshot[*slot].as_input();
            }
            if fire {
                let mut values = snapshot.clone();
                run(body, &mut values, &mut writes);
                // Blocking assignments inside clocked blocks take effect too.
                for (slot, (new, old)) in values.iter().zip(&snapshot).enumerate() {
                    if new != old {
                        writes.push((slot, 0, *new));
                    }
                }
            }
        }
        for (k, lo, v) in writes {
            self.values[k] = if lo == 0 && v.width() == self.values[k].width() {
                v
            } else {
                put(self.values[k], lo, v)
            };
        }
        self.settle();
        for (p, spec) in self.ports.iter().enumerate() {
            if spec.dir == circuiter_logic::Dir::In {
                continue;
            }
            let v = self.values[self.slots[p]];
            if self.driven[p] != Some(v) || cx.now == 0 {
                self.driven[p] = Some(v);
                cx.drive(p, v, self.delay);
            }
        }
    }
}

/// A port of a module, for making a cell from it.
#[derive(Debug, Clone, PartialEq)]
pub struct PortInfo {
    pub name: String,
    pub dir: Dir,
    pub width: u32,
}

/// The module called `name` in `modules`, or the only (or last) one.
fn pick<'a>(modules: &'a [Module], name: Option<&str>) -> Result<&'a Module, String> {
    match name {
        Some(n) => modules.iter().find(|m| m.name == n).ok_or_else(|| format!("no module `{n}`")),
        None => modules.last().ok_or_else(|| "no module in the file".to_owned()),
    }
}

/// A compiled module: its names and signals, continuous assignments, combinational
/// blocks, and clocked blocks with their edges.
type Compiled<'m> =
    (Compiler<'m>, Vec<(CLValue, CExpr)>, Vec<CStmt>, Vec<(Vec<(bool, usize)>, CStmt)>);

fn compile<'m>(
    module: &'m Module,
    overrides: &BTreeMap<String, String>,
) -> Result<Compiled<'m>, HdlError> {
    let mut c =
        Compiler { module, params: BTreeMap::new(), signals: Vec::new(), names: BTreeMap::new() };
    for (name, default) in &module.params {
        let v = match overrides.get(name) {
            Some(text) => text
                .trim()
                .parse::<i64>()
                .or_else(|_| i64::from_str_radix(text.trim().trim_start_matches("0x"), 16))
                .map_err(|_| HdlError {
                    line: module.line,
                    message: format!("parameter {name}: `{text}` isn't a number"),
                })?,
            None => c.constant(default)?,
        };
        c.params.insert(name.clone(), v);
    }
    for p in &module.ports {
        let w = c.width(&p.range, p.line)?;
        c.declare(&p.name, w, p.signed, p.line)?;
    }
    let mut assigns = Vec::new();
    let mut combs = Vec::new();
    let mut seqs = Vec::new();
    // Declarations and parameters first, so items may use names declared later.
    for item in &module.items {
        match item {
            Item::Param(n, e) => {
                let v = c.constant(e)?;
                c.params.insert(n.clone(), v);
            }
            Item::Decl { names, range, signed, line, .. } => {
                let w = c.width(range, *line)?;
                for n in names {
                    c.declare(n, w, *signed, *line)?;
                }
            }
            _ => {}
        }
    }
    for item in &module.items {
        match item {
            Item::Decl { names, init: Some(e), line, .. } => {
                for n in names {
                    let lhs = c.lvalue(&LValue::Var(n.clone(), *line))?;
                    assigns.push((lhs, c.expr(e)?));
                }
            }
            Item::Assign(l, e, _) => assigns.push((c.lvalue(l)?, c.expr(e)?)),
            Item::Comb(s) => combs.push(c.stmt(s)?),
            Item::Seq(edges, s) => {
                let mut bound = Vec::new();
                for (rising, name) in edges {
                    let k = c.names.get(name).copied().ok_or(HdlError {
                        line: module.line,
                        message: format!("`{name}` isn't declared"),
                    })?;
                    bound.push((*rising, k));
                }
                seqs.push((bound, c.stmt(s)?));
            }
            _ => {}
        }
    }
    Ok((c, assigns, combs, seqs))
}

/// The ports of module `name` (or the file's only module), with widths for the given
/// parameter values.
pub fn interface(
    src: &str,
    name: Option<&str>,
    params: &BTreeMap<String, String>,
) -> Result<Vec<PortInfo>, String> {
    let modules = crate::parse::modules(src).map_err(|e| e.to_string())?;
    let m = pick(&modules, name)?;
    let (c, ..) = compile(m, params).map_err(|e| e.to_string())?;
    Ok(m.ports
        .iter()
        .map(|p| PortInfo {
            name: p.name.clone(),
            dir: p.dir,
            width: c.signals[c.names[&p.name]].width,
        })
        .collect())
}

/// Makes a part of the digital simulator from module `name` in `src`.
pub fn create(
    src: &str,
    name: Option<&str>,
    params: &BTreeMap<String, String>,
    delay: Time,
) -> Result<Box<dyn Primitive>, String> {
    let modules = crate::parse::modules(src).map_err(|e| e.to_string())?;
    let m = pick(&modules, name)?;
    let (c, assigns, combs, seqs) = compile(m, params).map_err(|e| e.to_string())?;
    let mut ports = Vec::new();
    let mut slots = Vec::new();
    for p in &m.ports {
        let k = c.names[&p.name];
        let width = c.signals[k].width;
        ports.push(PortSpec {
            name: p.name.clone(),
            dir: match p.dir {
                Dir::Input => circuiter_logic::Dir::In,
                Dir::Output => circuiter_logic::Dir::Out,
                Dir::Inout => circuiter_logic::Dir::InOut,
            },
            width,
        });
        slots.push(k);
    }
    let values = c.signals.iter().map(|s| Value::unknown(s.width)).collect();
    let last_edges = seqs.iter().map(|(e, _)| vec![Value::unknown(1); e.len()]).collect();
    let n = ports.len();
    Ok(Box::new(HdlPart {
        ports,
        slots,
        values,
        assigns,
        combs,
        seqs,
        last_edges,
        driven: vec![None; n],
        delay,
    }))
}
