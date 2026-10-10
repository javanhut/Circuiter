//! Behavioural sources (`B` elements): a voltage or current given by an expression of
//! node voltages, branch currents and time, as ngspice writes them:
//!
//! ```text
//! B1 out 0 V={tanh(1e5*(V(inp)-V(inn)))*12}
//! B2 a b I=1m*V(ctl)^2
//! ```
//!
//! The expression is parsed once into a tree; each Newton iteration evaluates it with
//! its exact partial derivatives (forward-mode differentiation), so the source is
//! linearised like any other nonlinear device. Comparisons, `u()`, `if()` and `?:`
//! have zero derivative: smooth functions (`tanh`, `limit` with a gentle slope)
//! converge better where a design allows them.

use crate::number::parse_number;

/// An expression tree. Inputs are numbered: the voltages first, then the currents.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Input(usize),
    Time,
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    Call(Func, Vec<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Func {
    Sqrt,
    Exp,
    Ln,
    Log10,
    Abs,
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Pow,
    Pwrs,
    Min,
    Max,
    Limit,
    Step,
    Ramp,
    Sgn,
    If,
    Floor,
    Ceil,
}

impl Func {
    fn by_name(name: &str) -> Option<(Func, usize)> {
        Some(match name {
            "sqrt" => (Func::Sqrt, 1),
            "exp" => (Func::Exp, 1),
            "ln" | "log" => (Func::Ln, 1),
            "log10" => (Func::Log10, 1),
            "abs" => (Func::Abs, 1),
            "sin" => (Func::Sin, 1),
            "cos" => (Func::Cos, 1),
            "tan" => (Func::Tan, 1),
            "asin" => (Func::Asin, 1),
            "acos" => (Func::Acos, 1),
            "atan" => (Func::Atan, 1),
            "sinh" => (Func::Sinh, 1),
            "cosh" => (Func::Cosh, 1),
            "tanh" => (Func::Tanh, 1),
            "pow" | "pwr" => (Func::Pow, 2),
            "pwrs" => (Func::Pwrs, 2),
            "min" => (Func::Min, 2),
            "max" => (Func::Max, 2),
            "limit" => (Func::Limit, 3),
            "u" | "stp" => (Func::Step, 1),
            "uramp" => (Func::Ramp, 1),
            "sgn" | "sign" => (Func::Sgn, 1),
            "if" => (Func::If, 3),
            "floor" => (Func::Floor, 1),
            "ceil" => (Func::Ceil, 1),
            _ => return None,
        })
    }
}

/// A behavioural source as read from its card: what it sets, its expression, and the
/// voltages (node pairs, `0` for ground) and currents (voltage sources or inductors)
/// it reads, in the order the expression numbers them.
#[derive(Debug, Clone, PartialEq)]
pub struct Behavior {
    /// `I=`: a current from the first node through the source to the second.
    /// Otherwise `V=`: the voltage between them.
    pub current: bool,
    pub expr: Expr,
    pub voltages: Vec<(String, String)>,
    pub currents: Vec<String>,
}

impl Behavior {
    /// Parses the text after `B name n+ n-`: `V=expr` or `I=expr`, the expression
    /// optionally in braces. `param` gives the values of names that aren't functions
    /// or `time`; `node` and `element` rename what the expression refers to (inside a
    /// sub-circuit, its local names to the flattened ones).
    pub fn parse(
        text: &str,
        param: &dyn Fn(&str) -> Option<f64>,
        node: &dyn Fn(&str) -> String,
        element: &dyn Fn(&str) -> String,
    ) -> Result<Behavior, String> {
        let t = text.trim();
        let (kind, rest) = t.split_once('=').ok_or("expected `V=` or `I=` and an expression")?;
        let current = match kind.trim().to_ascii_lowercase().as_str() {
            "v" => false,
            "i" => true,
            other => return Err(format!("expected `V=` or `I=`, found `{other}=`")),
        };
        let mut body = rest.trim();
        if let Some(inner) = body.strip_prefix('{').and_then(|b| b.strip_suffix('}')) {
            body = inner;
        }
        let mut p = Parser {
            s: body.as_bytes(),
            text: body,
            i: 0,
            param,
            node,
            element,
            voltages: Vec::new(),
            currents: Vec::new(),
        };
        let expr = p.ternary()?;
        p.skip();
        if p.i < p.s.len() {
            return Err(format!("unexpected `{}` in `{body}`", &body[p.i..]));
        }
        // Currents come after the voltages.
        let nv = p.voltages.len();
        let expr = renumber(expr, &|k| if k >= CURRENT { nv + (k - CURRENT) } else { k });
        Ok(Behavior { current, expr, voltages: p.voltages, currents: p.currents })
    }
}

/// Current inputs are numbered from here while parsing.
const CURRENT: usize = 1 << 20;

fn renumber(e: Expr, f: &dyn Fn(usize) -> usize) -> Expr {
    let b = |e: Box<Expr>| Box::new(renumber(*e, f));
    match e {
        Expr::Input(k) => Expr::Input(f(k)),
        Expr::Neg(a) => Expr::Neg(b(a)),
        Expr::Not(a) => Expr::Not(b(a)),
        Expr::Bin(op, x, y) => Expr::Bin(op, b(x), b(y)),
        Expr::Call(func, args) => Expr::Call(func, args.into_iter().map(|a| renumber(a, f)).collect()),
        Expr::Cond(c, x, y) => Expr::Cond(b(c), b(x), b(y)),
        other => other,
    }
}

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
    param: &'a dyn Fn(&str) -> Option<f64>,
    node: &'a dyn Fn(&str) -> String,
    element: &'a dyn Fn(&str) -> String,
    voltages: Vec<(String, String)>,
    currents: Vec<String>,
}

impl Parser<'_> {
    fn skip(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&mut self) -> Option<u8> {
        self.skip();
        self.s.get(self.i).copied()
    }

    /// Consumes `tok` if it comes next.
    fn eat(&mut self, tok: &str) -> bool {
        self.skip();
        if self.text[self.i..].starts_with(tok) {
            self.i += tok.len();
            true
        } else {
            false
        }
    }

    fn ternary(&mut self) -> Result<Expr, String> {
        let c = self.or()?;
        if self.eat("?") {
            let a = self.ternary()?;
            if !self.eat(":") {
                return Err(format!("`?` without `:` in `{}`", self.text));
            }
            let b = self.ternary()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut a = self.and()?;
        while self.eat("||") {
            a = Expr::Bin(Op::Or, Box::new(a), Box::new(self.and()?));
        }
        Ok(a)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut a = self.cmp()?;
        while self.eat("&&") {
            a = Expr::Bin(Op::And, Box::new(a), Box::new(self.cmp()?));
        }
        Ok(a)
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let a = self.add()?;
        for (tok, op) in [
            ("<=", Op::Le),
            (">=", Op::Ge),
            ("==", Op::Eq),
            ("!=", Op::Ne),
            ("<", Op::Lt),
            (">", Op::Gt),
        ] {
            if self.eat(tok) {
                return Ok(Expr::Bin(op, Box::new(a), Box::new(self.add()?)));
            }
        }
        Ok(a)
    }

    fn add(&mut self) -> Result<Expr, String> {
        let mut a = self.mul()?;
        loop {
            let op = match self.peek() {
                Some(b'+') => Op::Add,
                Some(b'-') => Op::Sub,
                _ => return Ok(a),
            };
            self.i += 1;
            a = Expr::Bin(op, Box::new(a), Box::new(self.mul()?));
        }
    }

    fn mul(&mut self) -> Result<Expr, String> {
        let mut a = self.unary()?;
        loop {
            self.skip();
            let op = if self.text[self.i..].starts_with("**") {
                return Ok(a);
            } else {
                match self.peek() {
                    Some(b'*') => Op::Mul,
                    Some(b'/') => Op::Div,
                    _ => return Ok(a),
                }
            };
            self.i += 1;
            a = Expr::Bin(op, Box::new(a), Box::new(self.unary()?));
        }
    }

    fn unary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            Some(b'-') => {
                self.i += 1;
                Ok(Expr::Neg(Box::new(self.unary()?)))
            }
            Some(b'+') => {
                self.i += 1;
                self.unary()
            }
            Some(b'!') if !self.text[self.i..].starts_with("!=") => {
                self.i += 1;
                Ok(Expr::Not(Box::new(self.unary()?)))
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Result<Expr, String> {
        let base = self.primary()?;
        if self.eat("**") || self.eat("^") {
            // Right-associative, and binding tighter than a unary minus on its right.
            let exp = self.unary()?;
            return Ok(Expr::Bin(Op::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        match self.peek() {
            None => Err(format!("`{}` ends too soon", self.text)),
            Some(b'(') => {
                self.i += 1;
                let e = self.ternary()?;
                if !self.eat(")") {
                    return Err(format!("missing `)` in `{}`", self.text));
                }
                Ok(e)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => {
                let start = self.i;
                while self.i < self.s.len() {
                    let c = self.s[self.i];
                    let exponent_sign = (c == b'+' || c == b'-')
                        && matches!(self.s[self.i - 1], b'e' | b'E')
                        && self.s[start..self.i - 1].iter().all(|b| b.is_ascii_digit() || *b == b'.');
                    if c.is_ascii_alphanumeric() || c == b'.' || exponent_sign {
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                let word = &self.text[start..self.i];
                parse_number(word).map(Expr::Num).ok_or(format!("`{word}` is not a number"))
            }
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                let start = self.i;
                while self.i < self.s.len()
                    && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_')
                {
                    self.i += 1;
                }
                let name = self.text[start..self.i].to_ascii_lowercase();
                if self.peek() == Some(b'(') {
                    self.i += 1;
                    return self.call(&name);
                }
                match name.as_str() {
                    "time" => Ok(Expr::Time),
                    "pi" => Ok(Expr::Num(std::f64::consts::PI)),
                    "temper" => Ok(Expr::Num(27.0)),
                    _ => (self.param)(&name)
                        .map(Expr::Num)
                        .ok_or(format!("unknown name `{name}` in `{}`", self.text)),
                }
            }
            Some(c) => Err(format!("unexpected `{}` in `{}`", c as char, self.text)),
        }
    }

    /// A name as written inside `V(...)` or `I(...)`: up to `,` or `)`.
    fn raw_name(&mut self) -> Result<String, String> {
        self.skip();
        let start = self.i;
        while self.i < self.s.len() && !matches!(self.s[self.i], b',' | b')') {
            self.i += 1;
        }
        let name = self.text[start..self.i].trim();
        if name.is_empty() {
            return Err(format!("empty name in `{}`", self.text));
        }
        Ok(name.to_owned())
    }

    fn call(&mut self, name: &str) -> Result<Expr, String> {
        if name == "v" {
            let a = (self.node)(&self.raw_name()?);
            let b = if self.eat(",") { (self.node)(&self.raw_name()?) } else { "0".to_owned() };
            if !self.eat(")") {
                return Err(format!("missing `)` after V( in `{}`", self.text));
            }
            let key = (a, b);
            let k = match self.voltages.iter().position(|v| *v == key) {
                Some(k) => k,
                None => {
                    self.voltages.push(key);
                    self.voltages.len() - 1
                }
            };
            return Ok(Expr::Input(k));
        }
        if name == "i" {
            let src = (self.element)(&self.raw_name()?);
            if !self.eat(")") {
                return Err(format!("missing `)` after I( in `{}`", self.text));
            }
            let k = match self.currents.iter().position(|c| c.eq_ignore_ascii_case(&src)) {
                Some(k) => k,
                None => {
                    self.currents.push(src);
                    self.currents.len() - 1
                }
            };
            return Ok(Expr::Input(CURRENT + k));
        }
        let (func, arity) =
            Func::by_name(name).ok_or(format!("unknown function `{name}` in `{}`", self.text))?;
        let mut args = Vec::new();
        if !self.eat(")") {
            loop {
                args.push(self.ternary()?);
                if self.eat(")") {
                    break;
                }
                if !self.eat(",") {
                    return Err(format!("missing `)` after the arguments of `{name}`"));
                }
            }
        }
        if args.len() != arity {
            return Err(format!("`{name}` takes {arity} argument(s), not {}", args.len()));
        }
        Ok(Expr::Call(func, args))
    }
}

/// A value and its partial derivatives with respect to each input.
#[derive(Debug, Clone)]
struct Dual {
    v: f64,
    d: Vec<f64>,
}

impl Dual {
    fn constant(v: f64, n: usize) -> Dual {
        Dual { v, d: vec![0.0; n] }
    }

    /// `f(self)` with derivative `df` at `self`.
    fn chain(mut self, v: f64, df: f64) -> Dual {
        self.v = v;
        let df = if df.is_finite() { df } else { 0.0 };
        self.d.iter_mut().for_each(|x| *x *= df);
        self
    }

    fn combine(self, o: Dual, v: f64, da: f64, db: f64) -> Dual {
        let d = self.d.iter().zip(&o.d).map(|(a, b)| a * da + b * db).collect();
        Dual { v, d }
    }

    fn flat(self, v: f64) -> Dual {
        let n = self.d.len();
        Dual::constant(v, n)
    }
}

const EXP_LIMIT: f64 = 80.0;

impl Expr {
    /// The value at inputs `x` and time `t`, with its partial derivatives.
    pub fn eval(&self, x: &[f64], t: f64) -> (f64, Vec<f64>) {
        let d = self.dual(x, t);
        let v = if d.v.is_finite() { d.v } else { 0.0 };
        (v, d.d)
    }

    fn dual(&self, x: &[f64], t: f64) -> Dual {
        let n = x.len();
        match self {
            Expr::Num(v) => Dual::constant(*v, n),
            Expr::Time => Dual::constant(t, n),
            Expr::Input(k) => {
                let mut d = Dual::constant(x.get(*k).copied().unwrap_or(0.0), n);
                if *k < n {
                    d.d[*k] = 1.0;
                }
                d
            }
            Expr::Neg(a) => {
                let a = a.dual(x, t);
                let v = -a.v;
                a.chain(v, -1.0)
            }
            Expr::Not(a) => {
                let a = a.dual(x, t);
                let v = f64::from(a.v == 0.0);
                a.flat(v)
            }
            Expr::Cond(c, a, b) => {
                if c.dual(x, t).v != 0.0 {
                    a.dual(x, t)
                } else {
                    b.dual(x, t)
                }
            }
            Expr::Bin(op, a, b) => {
                let (a, b) = (a.dual(x, t), b.dual(x, t));
                let (p, q) = (a.v, b.v);
                let truth = |c: bool| f64::from(c);
                match op {
                    Op::Add => a.combine(b, p + q, 1.0, 1.0),
                    Op::Sub => a.combine(b, p - q, 1.0, -1.0),
                    Op::Mul => a.combine(b, p * q, q, p),
                    Op::Div => {
                        let q = if q.abs() < 1e-300 { 1e-300f64.copysign(q) } else { q };
                        a.combine(b, p / q, 1.0 / q, -p / (q * q))
                    }
                    Op::Pow => pow(a, b),
                    Op::Lt => a.flat(truth(p < q)),
                    Op::Gt => a.flat(truth(p > q)),
                    Op::Le => a.flat(truth(p <= q)),
                    Op::Ge => a.flat(truth(p >= q)),
                    Op::Eq => a.flat(truth(p == q)),
                    Op::Ne => a.flat(truth(p != q)),
                    Op::And => a.flat(truth(p != 0.0 && q != 0.0)),
                    Op::Or => a.flat(truth(p != 0.0 || q != 0.0)),
                }
            }
            Expr::Call(f, args) => {
                let mut a: Vec<Dual> = args.iter().map(|e| e.dual(x, t)).collect();
                let first = a.remove(0);
                let u = first.v;
                match f {
                    Func::Sqrt => {
                        let s = u.max(0.0).sqrt();
                        first.chain(s, if s > 1e-150 { 0.5 / s } else { 0.0 })
                    }
                    Func::Exp => {
                        if u > EXP_LIMIT {
                            // Linear beyond the limit, so a Newton step can come back.
                            let e = EXP_LIMIT.exp();
                            first.chain(e * (1.0 + u - EXP_LIMIT), e)
                        } else {
                            let e = u.exp();
                            first.chain(e, e)
                        }
                    }
                    Func::Ln => {
                        let w = u.max(1e-300);
                        first.chain(w.ln(), 1.0 / w)
                    }
                    Func::Log10 => {
                        let w = u.max(1e-300);
                        first.chain(w.log10(), 1.0 / (w * std::f64::consts::LN_10))
                    }
                    Func::Abs => first.chain(u.abs(), if u < 0.0 { -1.0 } else { 1.0 }),
                    Func::Sin => first.chain(u.sin(), u.cos()),
                    Func::Cos => first.chain(u.cos(), -u.sin()),
                    Func::Tan => {
                        let c = u.cos();
                        first.chain(u.tan(), 1.0 / (c * c))
                    }
                    Func::Asin => {
                        let w = u.clamp(-1.0, 1.0);
                        first.chain(w.asin(), 1.0 / (1.0 - w * w).max(1e-30).sqrt())
                    }
                    Func::Acos => {
                        let w = u.clamp(-1.0, 1.0);
                        first.chain(w.acos(), -1.0 / (1.0 - w * w).max(1e-30).sqrt())
                    }
                    Func::Atan => first.chain(u.atan(), 1.0 / (1.0 + u * u)),
                    Func::Sinh => {
                        let w = u.clamp(-EXP_LIMIT, EXP_LIMIT);
                        first.chain(w.sinh(), w.cosh())
                    }
                    Func::Cosh => {
                        let w = u.clamp(-EXP_LIMIT, EXP_LIMIT);
                        first.chain(w.cosh(), w.sinh())
                    }
                    Func::Tanh => {
                        let th = u.tanh();
                        first.chain(th, 1.0 - th * th)
                    }
                    Func::Pow => pow(first, a.remove(0)),
                    Func::Pwrs => {
                        let b = a.remove(0);
                        let (y, m) = (b.v, u.abs());
                        if m < 1e-300 {
                            first.flat(0.0)
                        } else {
                            let r = m.powf(y);
                            let v = r.copysign(u);
                            let da = y * m.powf(y - 1.0);
                            let db = v * m.ln();
                            first.combine(b, v, da, db)
                        }
                    }
                    Func::Min => {
                        let b = a.remove(0);
                        if u <= b.v { first } else { b }
                    }
                    Func::Max => {
                        let b = a.remove(0);
                        if u >= b.v { first } else { b }
                    }
                    Func::Limit => {
                        let (lo, hi) = (a.remove(0), a.remove(0));
                        let (l, h) = (lo.v.min(hi.v), lo.v.max(hi.v));
                        if u < l {
                            if lo.v <= hi.v { lo } else { hi }
                        } else if u > h {
                            if lo.v <= hi.v { hi } else { lo }
                        } else {
                            first
                        }
                    }
                    Func::Step => first.flat(f64::from(u > 0.0)),
                    Func::Ramp => {
                        if u > 0.0 {
                            first
                        } else {
                            first.flat(0.0)
                        }
                    }
                    Func::Sgn => first.flat(if u > 0.0 {
                        1.0
                    } else if u < 0.0 {
                        -1.0
                    } else {
                        0.0
                    }),
                    Func::If => {
                        let (then, other) = (a.remove(0), a.remove(0));
                        if u != 0.0 { then } else { other }
                    }
                    Func::Floor => first.flat(u.floor()),
                    Func::Ceil => first.flat(u.ceil()),
                }
            }
        }
    }
}

fn pow(a: Dual, b: Dual) -> Dual {
    let (p, q) = (a.v, b.v);
    if p > 0.0 {
        let v = p.powf(q);
        a.combine(b, v, q * p.powf(q - 1.0), v * p.ln())
    } else if q.fract() == 0.0 {
        // A whole power of a negative base (or zero).
        let v = p.powf(q);
        let da = if p == 0.0 && q < 1.0 { 0.0 } else { q * p.powf(q - 1.0) };
        a.combine(b, v, da, 0.0)
    } else {
        a.flat(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Behavior {
        Behavior::parse(
            text,
            &|n| (n == "gain").then_some(3.0),
            &|n| n.to_ascii_lowercase(),
            &|n| n.to_owned(),
        )
        .unwrap()
    }

    #[test]
    fn parses_voltages_currents_and_functions() {
        let b = parse("V={gain*tanh(V(a,b)) + 2k*I(Vs) - V(a,b)^2}");
        assert!(!b.current);
        assert_eq!(b.voltages, [("a".to_owned(), "b".to_owned())]);
        assert_eq!(b.currents, ["Vs"]);
        let (v, d) = b.expr.eval(&[0.5, 1e-3], 0.0);
        let want = 3.0 * 0.5f64.tanh() + 2.0 - 0.25;
        assert!((v - want).abs() < 1e-12, "{v} vs {want}");
        let th = 0.5f64.tanh();
        assert!((d[0] - (3.0 * (1.0 - th * th) - 1.0)).abs() < 1e-12);
        assert!((d[1] - 2000.0).abs() < 1e-9);
    }

    #[test]
    fn conditions_limits_and_time() {
        let b = parse("I=V(x) > 1 ? limit(V(x), 0, 2) : -1m*time");
        assert!(b.current);
        assert_eq!(b.expr.eval(&[5.0], 0.0).0, 2.0);
        assert_eq!(b.expr.eval(&[1.5], 0.0), (1.5, vec![1.0]));
        assert!((b.expr.eval(&[0.0], 2.0).0 + 2e-3).abs() < 1e-15);
        let b = parse("V=-2**2 + u(V(n)) + if(V(n)>=0, 1, 0)");
        assert_eq!(b.expr.eval(&[0.0], 0.0).0, -4.0 + 0.0 + 1.0);
    }

    #[test]
    fn derivatives_match_finite_differences() {
        let b = parse("V=exp(V(a)/0.1)*sqrt(V(b)) + pwrs(V(a),1.5)/(1+V(b)) + log10(V(b))");
        let x = [0.3, 2.0];
        let (v, d) = b.expr.eval(&x, 0.0);
        for k in 0..2 {
            let mut y = x;
            y[k] += 1e-7;
            let fd = (b.expr.eval(&y, 0.0).0 - v) / 1e-7;
            assert!((fd - d[k]).abs() < 1e-4 * fd.abs().max(1.0), "input {k}: {fd} vs {}", d[k]);
        }
    }

    #[test]
    fn errors_say_what_is_wrong() {
        let e = |t: &str| {
            Behavior::parse(t, &|_| None, &|n| n.to_owned(), &|n| n.to_owned()).unwrap_err()
        };
        assert!(e("V=V(a)*nope").contains("nope"));
        assert!(e("V=frob(1)").contains("frob"));
        assert!(e("X=1").contains("V="));
        assert!(e("V=min(1)").contains("2 argument"));
    }
}
