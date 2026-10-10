//! Reading Verilog and SystemVerilog source: a lexer and a recursive-descent parser for
//! the synthesizable subset Circuiter runs.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    /// A literal: width if sized, value bits, unknown bits, signedness.
    Num {
        width: Option<u32>,
        bits: u64,
        x: u64,
        signed: bool,
    },
    Sym(&'static str),
    /// `$signed`, `$clog2`…
    System(String),
    /// `'0`, `'1`, `'x`, `'z`: every bit, at whatever width the context needs.
    Fill(char),
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
}

/// A problem in the source, with its line.
#[derive(Debug, Clone, PartialEq)]
pub struct HdlError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for HdlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

pub fn err<T>(line: usize, message: impl Into<String>) -> Result<T, HdlError> {
    Err(HdlError { line, message: message.into() })
}

const SYMBOLS: [&str; 46] = [
    "<<<", ">>>", "===", "!==", "<=", ">=", "==", "!=", "&&", "||", "<<", ">>", "~&", "~|", "~^",
    "^~", "+:", "-:", "::", "(", ")", "[", "]", "{", "}", ";", ",", ":", "?", "=", "+", "-", "*",
    "/", "%", "&", "|", "^", "~", "!", "<", ">", "@", "#", ".", "'",
];

pub fn lex(src: &str) -> Result<Vec<Token>, HdlError> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut line = 1;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if src[i..].starts_with("//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if src[i..].starts_with("/*") {
            let end = src[i + 2..]
                .find("*/")
                .ok_or(HdlError { line, message: "unclosed comment".into() })?;
            line += src[i..i + 2 + end].matches('\n').count();
            i += end + 4;
            continue;
        }
        if c == '`' {
            // Compiler directives (`timescale, `default_nettype): skip the line.
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == '"' {
            return err(line, "strings aren't supported here");
        }
        if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let start = i;
            i += 1;
            while i < b.len()
                && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$')
            {
                i += 1;
            }
            let word = &src[start..i];
            out.push(Token {
                tok: if let Some(sys) = word.strip_prefix('$') {
                    Tok::System(sys.to_owned())
                } else {
                    Tok::Ident(word.to_owned())
                },
                line,
            });
            continue;
        }
        if c == '\'' && i + 1 < b.len() && "01xXzZ".contains(b[i + 1] as char) {
            out.push(Token { tok: Tok::Fill((b[i + 1] as char).to_ascii_lowercase()), line });
            i += 2;
            continue;
        }
        if c.is_ascii_digit()
            || (c == '\'' && i + 1 < b.len() && "bBoOdDhHsS".contains(b[i + 1] as char))
        {
            let (tok, len) = number(&src[i..]).map_err(|m| HdlError { line, message: m })?;
            out.push(Token { tok, line });
            i += len;
            continue;
        }
        match SYMBOLS.iter().find(|s| src[i..].starts_with(**s)) {
            Some(s) => {
                out.push(Token { tok: Tok::Sym(s), line });
                i += s.len();
            }
            None => return err(line, format!("unexpected `{c}`")),
        }
    }
    Ok(out)
}

/// A number: `42`, `8'hFF`, `4'b10x1`, `'d5`, `32'sd7`, `1_000`.
fn number(s: &str) -> Result<(Tok, usize), String> {
    let digits = |s: &str| s.chars().take_while(|c| c.is_ascii_digit() || *c == '_').count();
    let lead = digits(s);
    let rest = &s[lead..];
    if !rest.starts_with('\'') {
        let v: u64 = s[..lead].replace('_', "").parse().map_err(|_| "number too large")?;
        return Ok((Tok::Num { width: None, bits: v, x: 0, signed: true }, lead));
    }
    let width = if lead == 0 {
        None
    } else {
        Some(s[..lead].replace('_', "").parse::<u32>().map_err(|_| "bad width")?)
    };
    let mut k = lead + 1;
    let signed = matches!(s.as_bytes().get(k), Some(b's' | b'S'));
    if signed {
        k += 1;
    }
    let base = match s.as_bytes().get(k).map(|b| b.to_ascii_lowercase()) {
        Some(b'b') => 2,
        Some(b'o') => 8,
        Some(b'd') => 10,
        Some(b'h') => 16,
        _ => return Err("expected b, o, d or h after '".into()),
    };
    k += 1;
    let body_len = s[k..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() || matches!(c, '_' | 'x' | 'X' | 'z' | 'Z' | '?'))
        .count();
    let body = &s[k..k + body_len];
    let (mut bits, mut x) = (0u64, 0u64);
    if base == 10 {
        bits = body.replace('_', "").parse().map_err(|_| format!("bad decimal `{body}`"))?;
    } else {
        let per = match base {
            2 => 1,
            8 => 3,
            _ => 4,
        };
        for ch in body.chars().filter(|c| *c != '_') {
            bits <<= per;
            x <<= per;
            match ch {
                'x' | 'X' | 'z' | 'Z' | '?' => x |= (1 << per) - 1,
                d => {
                    bits |= u64::from(
                        d.to_digit(base)
                            .ok_or_else(|| format!("`{d}` isn't a base-{base} digit"))?,
                    )
                }
            }
        }
    }
    if let Some(w) = width {
        if w == 0 || w > 64 {
            return Err(format!("widths of 1 to 64 bits are supported, not {w}"));
        }
        let m = if w == 64 { u64::MAX } else { (1 << w) - 1 };
        bits &= m;
        x &= m;
    }
    Ok((Tok::Num { width, bits, x, signed }, k + body_len))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dir {
    Input,
    Output,
    Inout,
}

/// `[msb:lsb]`, as expressions until parameters are known.
pub type Range = Option<(Expr, Expr)>;

#[derive(Debug, Clone)]
pub struct PortDecl {
    pub name: String,
    pub dir: Dir,
    pub range: Range,
    pub signed: bool,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub name: String,
    pub params: Vec<(String, Expr)>,
    pub ports: Vec<PortDecl>,
    pub items: Vec<Item>,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub enum Item {
    Decl {
        names: Vec<String>,
        range: Range,
        signed: bool,
        init: Option<Expr>,
        line: usize,
    },
    Param(String, Expr),
    Assign(LValue, Expr, usize),
    Comb(Stmt),
    /// Edges: (rising, signal).
    Seq(Vec<(bool, String)>, Stmt),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Block(Vec<Stmt>),
    Assign {
        lhs: LValue,
        rhs: Expr,
        nonblocking: bool,
        line: usize,
    },
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    /// Subject, arms (labels, body), default, casez/casex (X and Z bits in labels match anything).
    Case(Expr, Vec<(Vec<Expr>, Stmt)>, Option<Box<Stmt>>, bool),
    Null,
}

#[derive(Debug, Clone)]
pub enum LValue {
    Var(String, usize),
    Bit(String, Expr, usize),
    Part(String, Expr, Expr, usize),
    Concat(Vec<LValue>),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Num {
        width: Option<u32>,
        bits: u64,
        x: u64,
        signed: bool,
    },
    Ident(String, usize),
    Index(Box<Expr>, Box<Expr>),
    Slice(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `a[base +: width]`.
    IndexedPart(Box<Expr>, Box<Expr>, Box<Expr>),
    Concat(Vec<Expr>),
    Repeat(Box<Expr>, Vec<Expr>),
    Unary(&'static str, Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>, usize),
    Fill(char),
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.tok)
    }

    fn line(&self) -> usize {
        self.toks.get(self.pos).or(self.toks.last()).map_or(1, |t| t.line)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).map(|t| t.tok.clone());
        self.pos += 1;
        t
    }

    fn is(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if *x == s)
            || matches!(self.peek(), Some(Tok::Ident(x)) if x == s)
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.is(s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, s: &str) -> Result<(), HdlError> {
        if self.eat(s) {
            Ok(())
        } else {
            let found = match self.peek() {
                Some(Tok::Ident(w)) => format!("`{w}`"),
                Some(Tok::Sym(w)) => format!("`{w}`"),
                Some(Tok::Num { .. }) => "a number".into(),
                Some(Tok::System(w)) => format!("`${w}`"),
                Some(Tok::Fill(c)) => format!("`'{c}`"),
                None => "the end of the file".into(),
            };
            err(self.line(), format!("expected `{s}`, found {found}"))
        }
    }

    fn ident(&mut self) -> Result<String, HdlError> {
        match self.next() {
            Some(Tok::Ident(w)) if !is_keyword(&w) => Ok(w),
            _ => {
                self.pos -= 1;
                err(self.line(), "expected a name")
            }
        }
    }

    fn range(&mut self) -> Result<Range, HdlError> {
        if !self.eat("[") {
            return Ok(None);
        }
        let msb = self.expr()?;
        self.expect(":")?;
        let lsb = self.expr()?;
        self.expect("]")?;
        Ok(Some((msb, lsb)))
    }

    /// `wire`, `reg`, `logic`, `signed`, in any order; returns signedness.
    fn net_type(&mut self) -> bool {
        let mut signed = false;
        loop {
            if self.eat("wire") || self.eat("reg") || self.eat("logic") || self.eat("bit") {
                continue;
            }
            if self.eat("signed") {
                signed = true;
                continue;
            }
            if self.eat("unsigned") {
                continue;
            }
            return signed;
        }
    }

    fn module(&mut self) -> Result<Module, HdlError> {
        let line = self.line();
        self.expect("module")?;
        let name = self.ident()?;
        let mut params = Vec::new();
        if self.eat("#") {
            self.expect("(")?;
            while !self.eat(")") {
                self.eat("parameter");
                self.eat("localparam");
                self.net_type();
                let _ = self.range()?;
                let p = self.ident()?;
                self.expect("=")?;
                params.push((p, self.expr()?));
                if !self.eat(",") {
                    self.expect(")")?;
                    break;
                }
            }
        }
        let mut ports = Vec::new();
        if self.eat("(") {
            let mut current: Option<(Dir, Range, bool)> = None;
            while !self.eat(")") {
                let pline = self.line();
                let dir = if self.eat("input") {
                    Some(Dir::Input)
                } else if self.eat("output") {
                    Some(Dir::Output)
                } else if self.eat("inout") {
                    Some(Dir::Inout)
                } else {
                    None
                };
                if let Some(d) = dir {
                    let signed = self.net_type();
                    let range = self.range()?;
                    current = Some((d, range, signed));
                }
                let pname = self.ident()?;
                let Some((d, range, signed)) = current.clone() else {
                    return err(
                        pline,
                        "declare ports with their direction in the header: `input wire a`",
                    );
                };
                ports.push(PortDecl { name: pname, dir: d, range, signed, line: pline });
                if !self.eat(",") {
                    self.expect(")")?;
                    break;
                }
            }
        }
        self.expect(";")?;
        let mut items = Vec::new();
        while !self.eat("endmodule") {
            if self.peek().is_none() {
                return err(self.line(), "missing `endmodule`");
            }
            self.item(&mut items)?;
        }
        Ok(Module { name, params, ports, items, line })
    }

    fn item(&mut self, items: &mut Vec<Item>) -> Result<(), HdlError> {
        let line = self.line();
        if self.eat("parameter") || self.eat("localparam") {
            self.net_type();
            let _ = self.range()?;
            loop {
                let n = self.ident()?;
                self.expect("=")?;
                items.push(Item::Param(n, self.expr()?));
                if !self.eat(",") {
                    break;
                }
            }
            return self.expect(";");
        }
        if self.is("wire")
            || self.is("reg")
            || self.is("logic")
            || self.is("bit")
            || self.is("integer")
        {
            let integer = self.eat("integer");
            let signed = self.net_type() || integer;
            let range = if integer {
                Some((
                    Expr::Num { width: None, bits: 31, x: 0, signed: true },
                    Expr::Num { width: None, bits: 0, x: 0, signed: true },
                ))
            } else {
                self.range()?
            };
            let mut names = Vec::new();
            let mut init = None;
            loop {
                names.push(self.ident()?);
                if self.eat("=") {
                    // `wire y = a & b;` is a declaration and an assignment.
                    init = Some(self.expr()?);
                }
                if !self.eat(",") {
                    break;
                }
            }
            self.expect(";")?;
            items.push(Item::Decl { names, range, signed, init, line });
            return Ok(());
        }
        if self.eat("assign") {
            loop {
                let lhs = self.lvalue()?;
                self.expect("=")?;
                items.push(Item::Assign(lhs, self.expr()?, line));
                if !self.eat(",") {
                    break;
                }
            }
            return self.expect(";");
        }
        if self.eat("always_comb") || self.eat("always_latch") {
            items.push(Item::Comb(self.stmt()?));
            return Ok(());
        }
        if self.eat("always_ff") || self.eat("always") {
            self.expect("@")?;
            if self.eat("*") {
                items.push(Item::Comb(self.stmt()?));
                return Ok(());
            }
            self.expect("(")?;
            if self.eat("*") {
                self.expect(")")?;
                items.push(Item::Comb(self.stmt()?));
                return Ok(());
            }
            let mut edges = Vec::new();
            let mut level = false;
            loop {
                if self.eat("posedge") {
                    edges.push((true, self.ident()?));
                } else if self.eat("negedge") {
                    edges.push((false, self.ident()?));
                } else {
                    let _ = self.ident()?;
                    level = true;
                }
                if !(self.eat("or") || self.eat(",")) {
                    break;
                }
            }
            self.expect(")")?;
            let body = self.stmt()?;
            if level || edges.is_empty() {
                items.push(Item::Comb(body));
            } else {
                items.push(Item::Seq(edges, body));
            }
            return Ok(());
        }
        if self.eat("initial") {
            return err(line, "`initial` blocks aren't simulated: give registers a reset instead");
        }
        match self.peek() {
            Some(Tok::Ident(w)) => err(
                line,
                format!("`{w}` isn't supported here (instances of other modules aren't yet)"),
            ),
            _ => err(line, "expected a declaration, `assign` or `always`"),
        }
    }

    fn stmt(&mut self) -> Result<Stmt, HdlError> {
        let line = self.line();
        if self.eat("begin") {
            if self.eat(":") {
                self.ident()?;
            }
            let mut body = Vec::new();
            while !self.eat("end") {
                if self.peek().is_none() {
                    return err(line, "missing `end`");
                }
                body.push(self.stmt()?);
            }
            if self.eat(":") {
                self.ident()?;
            }
            return Ok(Stmt::Block(body));
        }
        if self.eat(";") {
            return Ok(Stmt::Null);
        }
        // `unique case` and `priority case` decode like plain `case`.
        let _ = self.eat("unique") || self.eat("priority");
        if self.eat("if") {
            self.expect("(")?;
            let c = self.expr()?;
            self.expect(")")?;
            let then = self.stmt()?;
            let other = if self.eat("else") { Some(Box::new(self.stmt()?)) } else { None };
            return Ok(Stmt::If(c, Box::new(then), other));
        }
        if self.is("case") || self.is("casez") || self.is("casex") {
            let wild = !self.eat("case");
            if wild {
                self.pos += 1;
            }
            self.expect("(")?;
            let subject = self.expr()?;
            self.expect(")")?;
            let mut arms = Vec::new();
            let mut default = None;
            while !self.eat("endcase") {
                if self.peek().is_none() {
                    return err(line, "missing `endcase`");
                }
                if self.eat("default") {
                    self.eat(":");
                    default = Some(Box::new(self.stmt()?));
                    continue;
                }
                let mut labels = vec![self.expr()?];
                while self.eat(",") {
                    labels.push(self.expr()?);
                }
                self.expect(":")?;
                arms.push((labels, self.stmt()?));
            }
            return Ok(Stmt::Case(subject, arms, default, wild));
        }
        if self.is("for") || self.is("while") || self.is("repeat") {
            return err(line, "loops aren't supported yet");
        }
        let lhs = self.lvalue()?;
        let nonblocking = if self.eat("<=") {
            true
        } else {
            self.expect("=")?;
            false
        };
        let rhs = self.expr()?;
        self.expect(";")?;
        Ok(Stmt::Assign { lhs, rhs, nonblocking, line })
    }

    fn lvalue(&mut self) -> Result<LValue, HdlError> {
        let line = self.line();
        if self.eat("{") {
            let mut parts = vec![self.lvalue()?];
            while self.eat(",") {
                parts.push(self.lvalue()?);
            }
            self.expect("}")?;
            return Ok(LValue::Concat(parts));
        }
        let name = self.ident()?;
        if self.eat("[") {
            let a = self.expr()?;
            if self.eat(":") {
                let b = self.expr()?;
                self.expect("]")?;
                return Ok(LValue::Part(name, a, b, line));
            }
            self.expect("]")?;
            return Ok(LValue::Bit(name, a, line));
        }
        Ok(LValue::Var(name, line))
    }

    pub fn expr(&mut self) -> Result<Expr, HdlError> {
        let c = self.binary(0)?;
        if self.eat("?") {
            let a = self.expr()?;
            self.expect(":")?;
            let b = self.expr()?;
            return Ok(Expr::Ternary(Box::new(c), Box::new(a), Box::new(b)));
        }
        Ok(c)
    }

    fn binary(&mut self, min: u8) -> Result<Expr, HdlError> {
        let mut lhs = self.unary()?;
        while let Some(Tok::Sym(s)) = self.peek() {
            let op = *s;
            let prec = match op {
                "||" => 1,
                "&&" => 2,
                "|" => 3,
                "^" | "~^" | "^~" => 4,
                "&" => 5,
                "==" | "!=" | "===" | "!==" => 6,
                "<" | "<=" | ">" | ">=" => 7,
                "<<" | ">>" | "<<<" | ">>>" => 8,
                "+" | "-" => 9,
                "*" | "/" | "%" => 10,
                _ => break,
            };
            if prec <= min {
                break;
            }
            self.pos += 1;
            let rhs = self.binary(prec)?;
            lhs = Expr::Binary(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr, HdlError> {
        for op in ["~&", "~|", "~^", "!", "~", "-", "+", "&", "|", "^"] {
            if self.eat(op) {
                let e = self.unary()?;
                return Ok(if op == "+" { e } else { Expr::Unary(op, Box::new(e)) });
            }
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, HdlError> {
        let mut e = self.primary()?;
        while self.eat("[") {
            let a = self.expr()?;
            if self.eat(":") {
                let b = self.expr()?;
                self.expect("]")?;
                e = Expr::Slice(Box::new(e), Box::new(a), Box::new(b));
            } else if self.eat("+:") {
                let w = self.expr()?;
                self.expect("]")?;
                e = Expr::IndexedPart(Box::new(e), Box::new(a), Box::new(w));
            } else {
                self.expect("]")?;
                e = Expr::Index(Box::new(e), Box::new(a));
            }
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, HdlError> {
        let line = self.line();
        match self.next() {
            Some(Tok::Num { width, bits, x, signed }) => Ok(Expr::Num { width, bits, x, signed }),
            Some(Tok::Fill(c)) => Ok(Expr::Fill(c)),
            Some(Tok::Ident(w)) if !is_keyword(&w) => Ok(Expr::Ident(w, line)),
            Some(Tok::System(name)) => {
                self.expect("(")?;
                let mut args = Vec::new();
                while !self.eat(")") {
                    args.push(self.expr()?);
                    if !self.eat(",") {
                        self.expect(")")?;
                        break;
                    }
                }
                Ok(Expr::Call(name, args, line))
            }
            Some(Tok::Sym("(")) => {
                let e = self.expr()?;
                self.expect(")")?;
                Ok(e)
            }
            Some(Tok::Sym("{")) => {
                let first = self.expr()?;
                if self.eat("{") {
                    // Replication: {n{a, b}}.
                    let mut parts = vec![self.expr()?];
                    while self.eat(",") {
                        parts.push(self.expr()?);
                    }
                    self.expect("}")?;
                    self.expect("}")?;
                    return Ok(Expr::Repeat(Box::new(first), parts));
                }
                let mut parts = vec![first];
                while self.eat(",") {
                    parts.push(self.expr()?);
                }
                self.expect("}")?;
                Ok(Expr::Concat(parts))
            }
            _ => {
                self.pos -= 1;
                err(line, "expected a value")
            }
        }
    }
}

fn is_keyword(w: &str) -> bool {
    matches!(
        w,
        "module"
            | "endmodule"
            | "input"
            | "output"
            | "inout"
            | "wire"
            | "reg"
            | "logic"
            | "assign"
            | "always"
            | "always_ff"
            | "always_comb"
            | "begin"
            | "end"
            | "if"
            | "else"
            | "case"
            | "casez"
            | "casex"
            | "endcase"
            | "default"
            | "posedge"
            | "negedge"
            | "or"
            | "parameter"
            | "localparam"
            | "signed"
            | "unsigned"
            | "integer"
    )
}

/// Every module in `src`.
pub fn modules(src: &str) -> Result<Vec<Module>, HdlError> {
    let toks = lex(src)?;
    let mut p = Parser { toks, pos: 0 };
    let mut out = Vec::new();
    while p.peek().is_some() {
        out.push(p.module()?);
    }
    Ok(out)
}
