//! Running a module: parameters and widths resolved, names bound to signal slots, and
//! an interpreter that evaluates continuous assignments, combinational blocks and
//! clocked blocks (non-blocking assignments take effect together) as a part of the
//! digital simulator.

use crate::parse::{Connection, Dir, Expr, Function, HdlError, Item, LValue, Module, Stmt, err};
use circuiter_logic::{Eval, PortSpec, Primitive, Time, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// How deep instances may nest (a module containing itself would never end).
const MAX_DEPTH: usize = 32;
/// How many times a loop may run when it's unrolled.
const MAX_ITERATIONS: i64 = 4096;

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
    /// A call of one of the module's functions, with its arguments.
    Func(Arc<CFunc>, Vec<CExpr>),
}

/// A compiled function. It runs on values of its own: its result, inputs and locals.
#[derive(Debug)]
struct CFunc {
    widths: Vec<u32>,
    /// Slot, width.
    inputs: Vec<(usize, u32)>,
    result: usize,
    body: CStmt,
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
    /// A function's result, and the end of it.
    Return(CLValue, CExpr),
    Null,
}

fn mask(w: u32) -> u64 {
    if w >= 64 { u64::MAX } else { (1 << w) - 1 }
}

/// What a module (with everything inside it) compiles to: continuous assignments,
/// combinational blocks, and clocked blocks with their edges.
#[derive(Default)]
struct Body {
    assigns: Vec<(CLValue, CExpr)>,
    combs: Vec<CStmt>,
    seqs: Vec<(Vec<(bool, usize)>, CStmt)>,
}

struct Compiler<'m> {
    /// Every module in the file, for instances.
    modules: &'m [Module],
    module: &'m Module,
    params: BTreeMap<String, i64>,
    /// Signals of the whole design: instances add theirs to their parent's.
    signals: Vec<Signal>,
    names: BTreeMap<String, usize>,
    /// Names declared in the current scope (each pass of a generate loop has one).
    scope: BTreeSet<String>,
    functions: BTreeMap<String, Arc<CFunc>>,
    /// Inside a function: where `return` puts its result.
    result: Option<usize>,
    depth: usize,
}

impl<'m> Compiler<'m> {
    fn new(modules: &'m [Module], module: &'m Module, signals: Vec<Signal>, depth: usize) -> Self {
        Compiler {
            modules,
            module,
            params: BTreeMap::new(),
            signals,
            names: BTreeMap::new(),
            scope: BTreeSet::new(),
            functions: BTreeMap::new(),
            result: None,
            depth,
        }
    }

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
        if let Some(&k) = self.names.get(name).filter(|_| self.scope.contains(name)) {
            // A port declared again in the body (`output reg q; reg q;`) is the same signal.
            return Ok(k);
        }
        if self.params.contains_key(name) && self.scope.contains(name) {
            return err(line, format!("`{name}` is already a parameter"));
        }
        self.signals.push(Signal { width, signed });
        self.names.insert(name.to_owned(), self.signals.len() - 1);
        self.scope.insert(name.to_owned());
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
            Expr::Apply(name, args, line) => {
                let Some(f) = self.functions.get(name) else {
                    return err(*line, format!("there's no function `{name}`"));
                };
                if args.len() != f.inputs.len() {
                    return err(
                        *line,
                        format!(
                            "`{name}` takes {} argument(s), not {}",
                            f.inputs.len(),
                            args.len()
                        ),
                    );
                }
                let args = args.iter().map(|a| self.expr(a)).collect::<Result<_, _>>()?;
                let width = f.widths[f.result];
                let signed = self.function_signed(name);
                CExpr { kind: Kind::Func(f.clone(), args), width, signed }
            }
        })
    }

    /// Whether function `name` returns a signed value.
    fn function_signed(&self, name: &str) -> bool {
        self.module
            .items
            .iter()
            .any(|i| matches!(i, Item::Function(f) if f.name == name && f.signed))
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

    fn stmt(&mut self, s: &Stmt) -> Result<CStmt, HdlError> {
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
            Stmt::For { var, init, cond, step, body, line } => {
                let mut out = Vec::new();
                self.unroll(var, init, cond, step, *line, |c| {
                    out.push(c.stmt(body)?);
                    Ok(())
                })?;
                CStmt::Block(out)
            }
            Stmt::Return(e, line) => {
                let Some(r) = self.result else {
                    return err(*line, "`return` is only allowed inside a function");
                };
                let w = self.signals[r].width;
                CStmt::Return(CLValue::Part(r, 0, w), self.expr(e)?)
            }
            Stmt::Null => CStmt::Null,
        })
    }

    /// Runs `each` once for every value of a loop variable, bound as a constant.
    fn unroll(
        &mut self,
        var: &str,
        init: &Expr,
        cond: &Expr,
        step: &Expr,
        line: usize,
        mut each: impl FnMut(&mut Self) -> Result<(), HdlError>,
    ) -> Result<(), HdlError> {
        let constant = |c: &Self, e: &Expr| {
            c.constant(e).map_err(|_| HdlError {
                line,
                message: "a loop's start, end and step must be constants".into(),
            })
        };
        let saved = self.params.get(var).copied();
        let mut v = constant(self, init)?;
        let mut n = 0;
        let result = loop {
            self.params.insert(var.to_owned(), v);
            match constant(self, cond) {
                Ok(0) => break Ok(()),
                Ok(_) => {}
                Err(e) => break Err(e),
            }
            if let Err(e) = each(self) {
                break Err(e);
            }
            match constant(self, step) {
                Ok(next) => v = next,
                Err(e) => break Err(e),
            }
            n += 1;
            if n > MAX_ITERATIONS {
                break err(line, format!("the loop runs more than {MAX_ITERATIONS} times"));
            }
        };
        match saved {
            Some(s) => self.params.insert(var.to_owned(), s),
            None => self.params.remove(var),
        };
        result
    }

    /// The module's items: parameters, declarations and functions first, so items may
    /// use names declared after them; then everything else.
    fn items(&mut self, items: &[Item], body: &mut Body) -> Result<(), HdlError> {
        for item in items {
            match item {
                Item::Param(n, e) => {
                    let v = self.constant(e)?;
                    self.params.insert(n.clone(), v);
                }
                Item::Decl { names, range, signed, line, .. } => {
                    let w = self.width(range, *line)?;
                    for n in names {
                        self.declare(n, w, *signed, *line)?;
                    }
                }
                Item::Function(f) => self.function(f)?,
                _ => {}
            }
        }
        for item in items {
            match item {
                Item::Decl { names, init: Some(e), line, .. } => {
                    for n in names {
                        let lhs = self.lvalue(&LValue::Var(n.clone(), *line))?;
                        body.assigns.push((lhs, self.expr(e)?));
                    }
                }
                Item::Assign(l, e, _) => body.assigns.push((self.lvalue(l)?, self.expr(e)?)),
                Item::Comb(s) => body.combs.push(self.stmt(s)?),
                Item::Seq(edges, s) => {
                    let mut bound = Vec::new();
                    for (rising, name) in edges {
                        let k = self.names.get(name).copied().ok_or(HdlError {
                            line: self.module.line,
                            message: format!("`{name}` isn't declared"),
                        })?;
                        bound.push((*rising, k));
                    }
                    body.seqs.push((bound, self.stmt(s)?));
                }
                Item::Instance { module, params, name, conns, line } => {
                    self.instance(module, params, name, conns, *line, body)?;
                }
                Item::GenFor { var, init, cond, step, items, line } => {
                    self.unroll(var, init, cond, step, *line, |c| c.scoped(items, body))?;
                }
                Item::GenIf(cond, then, other) => {
                    let pick = if self.constant(cond)? != 0 { then } else { other };
                    self.scoped(pick, body)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Items in a scope of their own: what they declare is gone after them.
    fn scoped(&mut self, items: &[Item], body: &mut Body) -> Result<(), HdlError> {
        let names = self.names.clone();
        let scope = std::mem::take(&mut self.scope);
        let functions = self.functions.clone();
        let result = self.items(items, body);
        self.names = names;
        self.scope = scope;
        self.functions = functions;
        result
    }

    /// Compiles a function. It sees the module's parameters and the functions declared
    /// before it, and runs on values of its own.
    fn function(&mut self, f: &Function) -> Result<(), HdlError> {
        let mut c = Compiler::new(self.modules, self.module, Vec::new(), self.depth);
        c.params = self.params.clone();
        c.functions = self.functions.clone();
        let w = c.width(&f.range, f.line)?;
        let result = c.declare(&f.name, w, f.signed, f.line)?;
        let mut inputs = Vec::new();
        for (n, r, s) in &f.inputs {
            let w = c.width(r, f.line)?;
            inputs.push((c.declare(n, w, *s, f.line)?, w));
        }
        for (n, r, s) in &f.locals {
            let w = c.width(r, f.line)?;
            c.declare(n, w, *s, f.line)?;
        }
        c.result = Some(result);
        let body = c.stmt(&f.body)?;
        let widths = c.signals.iter().map(|s| s.width).collect();
        self.functions.insert(f.name.clone(), Arc::new(CFunc { widths, inputs, result, body }));
        Ok(())
    }

    /// An instance of another module: compiled into this one, its ports joined to the
    /// connections by continuous assignments.
    fn instance(
        &mut self,
        module: &str,
        params: &[(Option<String>, Expr)],
        name: &str,
        conns: &[Connection],
        line: usize,
        body: &mut Body,
    ) -> Result<(), HdlError> {
        if self.depth >= MAX_DEPTH {
            return err(
                line,
                format!(
                    "instances nest more than {MAX_DEPTH} deep: does `{module}` contain itself?"
                ),
            );
        }
        let Some(m) = self.modules.iter().find(|m| m.name == module) else {
            return err(line, format!("there's no module `{module}` in this file"));
        };
        let mut overrides = BTreeMap::new();
        for (k, (n, e)) in params.iter().enumerate() {
            let p = match n {
                Some(n) if m.params.iter().any(|(p, _)| p == n) => n.clone(),
                Some(n) => return err(line, format!("`{module}` has no parameter `{n}`")),
                None => match m.params.get(k) {
                    Some((p, _)) => p.clone(),
                    None => return err(line, format!("`{module}` has fewer parameters")),
                },
            };
            overrides.insert(p, self.constant(e)?);
        }
        let signals = std::mem::take(&mut self.signals);
        let child = compile(self.modules, m, &overrides, signals, self.depth + 1, body)?;
        self.signals = child.signals;
        for (k, (n, e)) in conns.iter().enumerate() {
            let port = match n {
                Some(n) => m.ports.iter().find(|p| p.name == *n),
                None => m.ports.get(k),
            };
            let Some(port) = port else {
                return match n {
                    Some(n) => err(line, format!("`{module}` has no port `{n}`")),
                    None => err(
                        line,
                        format!("`{name}` has more connections than `{module}` has ports"),
                    ),
                };
            };
            let Some(e) = e else { continue };
            let slot = child.names[&port.name];
            let s = &self.signals[slot];
            let inner = CExpr { kind: Kind::Sig(slot), width: s.width, signed: s.signed };
            match port.dir {
                Dir::Input => body.assigns.push((CLValue::Part(slot, 0, s.width), self.expr(e)?)),
                Dir::Output => {
                    let Some(l) = to_lvalue(e) else {
                        return err(
                            line,
                            format!(
                                "output `{}` of `{name}` must go to a signal, a bit, a part \
                                 or a concatenation of them",
                                port.name
                            ),
                        );
                    };
                    body.assigns.push((self.lvalue(&l)?, inner));
                }
                Dir::Inout => {
                    return err(
                        line,
                        format!(
                            "`{}` of `{name}`: inout ports of instances aren't supported",
                            port.name
                        ),
                    );
                }
            }
        }
        Ok(())
    }
}

/// What an output port connects to, as something it can be assigned to.
fn to_lvalue(e: &Expr) -> Option<LValue> {
    Some(match e {
        Expr::Ident(n, line) => LValue::Var(n.clone(), *line),
        Expr::Index(a, i) => match &**a {
            Expr::Ident(n, line) => LValue::Bit(n.clone(), (**i).clone(), *line),
            _ => return None,
        },
        Expr::Slice(a, m, l) => match &**a {
            Expr::Ident(n, line) => LValue::Part(n.clone(), (**m).clone(), (**l).clone(), *line),
            _ => return None,
        },
        Expr::Concat(parts) => LValue::Concat(parts.iter().map(to_lvalue).collect::<Option<_>>()?),
        _ => return None,
    })
}

fn expr_line(e: &Expr) -> Option<usize> {
    match e {
        Expr::Ident(_, l) | Expr::Call(_, _, l) | Expr::Apply(_, _, l) => Some(*l),
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
        // No values: working out a constant, which a signal isn't.
        Kind::Sig(k) => {
            values.get(*k).map_or(Value::unknown(w), |v| extend(v.as_input(), w, e.signed))
        }
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
        Kind::Func(f, args) => {
            let mut local: Vec<Value> = f.widths.iter().map(|&w| Value::unknown(w)).collect();
            for (&(slot, iw), a) in f.inputs.iter().zip(args) {
                local[slot] = eval(a, values, iw).resize(iw);
            }
            run(&f.body, &mut local, &mut Vec::new());
            extend(local[f.result], w, e.signed)
        }
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

/// Runs a statement; true if it reached a `return`.
fn run(s: &CStmt, values: &mut [Value], writes: &mut Vec<(usize, u32, Value)>) -> bool {
    match s {
        CStmt::Block(v) => v.iter().any(|s| run(s, values, writes)),
        CStmt::Assign(lhs, rhs, nonblocking) => {
            let v = eval(rhs, values, lhs.width());
            store(lhs, v, values, writes, *nonblocking);
            false
        }
        CStmt::Return(lhs, rhs) => {
            let v = eval(rhs, values, lhs.width());
            store(lhs, v, values, writes, false);
            true
        }
        CStmt::If(c, a, b) => {
            // An unknown condition takes the else branch, as Verilog does.
            if logical(eval(c, values, c.width)) == Some(true) {
                run(a, values, writes)
            } else if let Some(b) = b {
                run(b, values, writes)
            } else {
                false
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
                        return run(body, values, writes);
                    }
                }
            }
            default.as_ref().is_some_and(|d| run(d, values, writes))
        }
        CStmt::Null => false,
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
        // The inputs through to everything they feed: a clock reaches the flip-flops of
        // instances inside through their ports.
        self.settle();
        // Clocked blocks whose edge just came, all seeing the values from before. A
        // clock made inside (a divider) can bring more edges: those come next round.
        for _ in 0..64 {
            let mut writes = Vec::new();
            let snapshot = self.values.clone();
            let mut any = false;
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
                    any = true;
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
            if !any {
                break;
            }
            for (k, lo, v) in writes {
                self.values[k] = if lo == 0 && v.width() == self.values[k].width() {
                    v
                } else {
                    put(self.values[k], lo, v)
                };
            }
            self.settle();
        }
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

/// Compiles `module` with the given parameter values into `body`, adding its signals
/// to `signals`; returns the compiler, which holds its names.
fn compile<'m>(
    modules: &'m [Module],
    module: &'m Module,
    overrides: &BTreeMap<String, i64>,
    signals: Vec<Signal>,
    depth: usize,
    body: &mut Body,
) -> Result<Compiler<'m>, HdlError> {
    let mut c = Compiler::new(modules, module, signals, depth);
    for (name, default) in &module.params {
        let v = match overrides.get(name) {
            Some(&v) => v,
            None => c.constant(default)?,
        };
        c.params.insert(name.clone(), v);
    }
    for p in &module.ports {
        let w = c.width(&p.range, p.line)?;
        c.declare(&p.name, w, p.signed, p.line)?;
    }
    c.items(&module.items, body)?;
    Ok(c)
}

/// Parameter values given as text, for the module's own parameters.
fn parse_overrides(
    module: &Module,
    given: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, i64>, String> {
    let mut out = BTreeMap::new();
    for (name, _) in &module.params {
        if let Some(text) = given.get(name) {
            let t = text.trim();
            let v = t
                .parse::<i64>()
                .or_else(|_| i64::from_str_radix(t.trim_start_matches("0x"), 16))
                .map_err(|_| {
                    format!("line {}: parameter {name}: `{text}` isn't a number", module.line)
                })?;
            out.insert(name.clone(), v);
        }
    }
    Ok(out)
}

/// Compiles the top module `name` (or the file's last one).
fn top<'m>(
    modules: &'m [Module],
    name: Option<&str>,
    params: &BTreeMap<String, String>,
) -> Result<(Compiler<'m>, Body), String> {
    let m = pick(modules, name)?;
    let overrides = parse_overrides(m, params)?;
    let mut body = Body::default();
    let c = compile(modules, m, &overrides, Vec::new(), 0, &mut body).map_err(|e| e.to_string())?;
    Ok((c, body))
}

/// The width of each signal of module `name` (or the last of `modules`), its ports
/// among them, at its parameters' defaults.
pub fn widths(modules: &[Module], name: Option<&str>) -> Result<BTreeMap<String, u32>, String> {
    let (c, _) = top(modules, name, &BTreeMap::new())?;
    Ok(c.names.iter().map(|(n, &k)| (n.clone(), c.signals[k].width)).collect())
}

/// The ports of module `name` (or the file's only module), with widths for the given
/// parameter values.
pub fn interface(
    src: &str,
    name: Option<&str>,
    params: &BTreeMap<String, String>,
) -> Result<Vec<PortInfo>, String> {
    let modules = crate::parse::modules(src).map_err(|e| e.to_string())?;
    let (c, _) = top(&modules, name, params)?;
    let m = c.module;
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
    let (c, Body { assigns, combs, seqs }) = top(&modules, name, params)?;
    let m = c.module;
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
