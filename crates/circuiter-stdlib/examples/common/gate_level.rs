//! The processor again, with every block opened down to logic gates: multiplexers from
//! AND, OR and NOT; flip-flops from NAND latches; the register file from 992 of those
//! flip-flops; the ALU from adders, a barrel shifter and bitwise gates. Only the two
//! memories (program ROM and data RAM) stay arrays, as they would be on a chip.
//!
//! Bus parts (slice, concat, extend, const) are wiring: they route bits, with no logic.

use super::sheet::{GRID, Sheet};
use super::{PROJECT, component, io, port};
use circuiter_model::Design;
use circuiter_model::cell::{CellDef, PortKind};
use circuiter_model::geom::Length;

const IN: PortKind = PortKind::Input;
const OUT: PortKind = PortKind::Output;

/// A port: name, direction, width, description.
type PortSpec<'a> = (&'a str, PortKind, u32, &'a str);

/// A sheet being filled column by column, with wiring helpers.
struct Gb<'a> {
    sh: Sheet<'a>,
    x: i64,
    y: i64,
    col_w: i64,
    /// How far down a column goes before the next one starts, in grid steps.
    limit: i64,
    n: usize,
}

impl<'a> Gb<'a> {
    fn new(design: &'a Design) -> Self {
        Gb { sh: Sheet::new(design, PROJECT), x: 0, y: 0, col_w: 0, limit: 64, n: 0 }
    }

    fn next(&mut self, prefix: &str) -> String {
        self.n += 1;
        format!("{prefix}{}", self.n)
    }

    /// The span of a cell's pins, in grid steps.
    fn size(&self, cell: &str) -> (i64, i64) {
        let (_, def) = self.sh.cell(cell);
        let pins = &def.symbol.as_ref().expect("symbol").pins;
        let g = Length::from_mm(GRID).nm();
        let span = |v: Vec<i64>| {
            v.iter().max().copied().unwrap_or(0) - v.iter().min().copied().unwrap_or(0)
        };
        (
            span(pins.iter().map(|p| p.at.x.nm()).collect()) / g,
            span(pins.iter().map(|p| p.at.y.nm()).collect()) / g,
        )
    }

    /// Places a part below the last one (or atop a new column) and labels its pins.
    fn part(&mut self, cell: &str, name: &str, params: &[(&str, &str)], nets: &[(&str, &str)]) {
        let (w, h) = self.size(cell);
        if self.y > 0 && self.y + h > self.limit {
            self.x += self.col_w + 24;
            self.y = 0;
            self.col_w = 0;
        }
        let i = self.sh.part(cell, name, self.x, self.y, params);
        self.sh.nets(i, nets);
        self.y += h + 6;
        self.col_w = self.col_w.max(w);
    }

    fn part_owned(
        &mut self,
        cell: &str,
        name: &str,
        params: &[(&str, &str)],
        nets: &[(String, String)],
    ) {
        let nets: Vec<(&str, &str)> = nets.iter().map(|(p, n)| (p.as_str(), n.as_str())).collect();
        self.part(cell, name, params, &nets);
    }

    /// A two-input gate (`and`, `or`, `xor`, `nand`…), named automatically.
    fn gate(&mut self, kind: &str, a: &str, b: &str, y: &str) {
        let name = self.next("U");
        self.part(&format!("std:{kind}"), &name, &[], &[("a", a), ("b", b), ("y", y)]);
    }

    fn inv(&mut self, a: &str, y: &str) {
        let name = self.next("U");
        self.part("std:not", &name, &[], &[("a", a), ("y", y)]);
    }

    /// All of `inputs` through a tree of two-input `kind` gates onto `out`.
    fn tree(&mut self, kind: &str, inputs: &[&str], out: &str) {
        assert!(inputs.len() >= 2, "a tree needs two inputs");
        let mut level: Vec<String> = inputs.iter().map(|s| s.to_string()).collect();
        while level.len() > 2 {
            let mut next = Vec::new();
            for pair in level.chunks(2) {
                if let [a, b] = pair {
                    let t = self.next("jn");
                    self.gate(kind, a, b, &t);
                    next.push(t);
                } else {
                    next.push(pair[0].clone());
                }
            }
            level = next;
        }
        self.gate(kind, &level[0], &level[1], out);
    }

    /// Bits `lo..lo+width` of the `from`-bit bus `bus` onto `out`.
    fn slice(&mut self, bus: &str, from: u32, lo: u32, width: u32, out: &str) {
        let name = self.next("BS");
        let (f, l, w) = (from.to_string(), lo.to_string(), width.to_string());
        self.part(
            "std:slice",
            &name,
            &[("from", &f), ("lo", &l), ("width", &w)],
            &[("a", bus), ("y", out)],
        );
    }

    /// Joins `parts` (net, width), highest first, onto `out`.
    fn cat(&mut self, parts: &[(&str, u32)], out: &str) {
        assert!(parts.len() >= 2, "a join needs two parts");
        let (hi, lo) = parts.split_at(parts.len() / 2);
        let a = self.cat_part(hi);
        let b = self.cat_part(lo);
        let name = self.next("BJ");
        let (wa, wb) = (a.1.to_string(), b.1.to_string());
        self.part(
            "std:concat",
            &name,
            &[("wa", &wa), ("wb", &wb)],
            &[("a", &a.0), ("b", &b.0), ("y", out)],
        );
    }

    fn cat_part(&mut self, parts: &[(&str, u32)]) -> (String, u32) {
        if let [(net, w)] = parts {
            return (net.to_string(), *w);
        }
        let net = self.next("jn");
        self.cat(parts, &net);
        (net, parts.iter().map(|p| p.1).sum())
    }

    /// `a` (`from` bits) made `width` bits wide; `signed` copies its top bit up.
    fn extend(&mut self, a: &str, from: u32, width: u32, signed: bool, out: &str) {
        let name = self.next("BX");
        let (f, w) = (from.to_string(), width.to_string());
        let s = if signed { "true" } else { "false" };
        self.part(
            "std:extend",
            &name,
            &[("from", &f), ("width", &w), ("signed", s)],
            &[("a", a), ("y", out)],
        );
    }

    fn konst(&mut self, width: u32, value: u64, out: &str) {
        let name = self.next("K");
        let (w, v) = (width.to_string(), value.to_string());
        self.part("std:const", &name, &[("width", &w), ("value", &v)], &[("y", out)]);
    }

    /// Finishes the cell: its ports at the left, each joined to the net of its name.
    fn finish(
        mut self,
        name: &str,
        description: &str,
        prefix: &str,
        ports: &[PortSpec<'_>],
    ) -> CellDef {
        let entries: Vec<(&str, &str)> = ports.iter().map(|p| (p.0, p.0)).collect();
        io(&mut self.sh, &entries);
        component(
            name,
            description,
            prefix,
            ports.iter().map(|&(n, k, w, d)| port(n, k, w, d)).collect(),
            self.sh.s,
        )
    }

    /// Like [`Gb::finish`], with ports joined to nets named differently.
    fn finish_mapped(
        mut self,
        name: &str,
        description: &str,
        prefix: &str,
        ports: &[(PortSpec<'_>, &str)],
    ) -> CellDef {
        let entries: Vec<(&str, &str)> = ports.iter().map(|(p, net)| (p.0, *net)).collect();
        io(&mut self.sh, &entries);
        component(
            name,
            description,
            prefix,
            ports.iter().map(|&((n, k, w, d), _)| port(n, k, w, d)).collect(),
            self.sh.s,
        )
    }
}

fn bit_ports(prefix: &str, kind: PortKind) -> Vec<(String, PortKind)> {
    (0..32).map(|i| (format!("{prefix}{i}"), kind)).collect()
}

// ---- One bit -------------------------------------------------------------------------

fn mux2_1(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    b.part("std:not", "N1", &[], &[("a", "sel"), ("y", "nsel")]);
    b.part("std:and", "A0", &[], &[("a", "d0"), ("b", "nsel"), ("y", "t0")]);
    b.part("std:and", "A1", &[], &[("a", "d1"), ("b", "sel"), ("y", "t1")]);
    b.part("std:or", "O1", &[], &[("a", "t0"), ("b", "t1"), ("y", "y")]);
    b.finish(
        "mux2_1",
        "One-bit multiplexer from gates: y = sel ? d1 : d0",
        "MUX",
        &[("d0", IN, 1, ""), ("d1", IN, 1, ""), ("sel", IN, 1, ""), ("y", OUT, 1, "")],
    )
}

fn mux8_1(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    for k in 0..3 {
        b.slice("sel", 3, k, 1, &format!("s{k}"));
    }
    for k in 0..4 {
        let (d0, d1, y) = (format!("d{}", 2 * k), format!("d{}", 2 * k + 1), format!("m{k}"));
        b.part(
            "mux2_1",
            &format!("L{k}"),
            &[],
            &[("d0", &d0), ("d1", &d1), ("sel", "s0"), ("y", &y)],
        );
    }
    b.part("mux2_1", "L4", &[], &[("d0", "m0"), ("d1", "m1"), ("sel", "s1"), ("y", "n0")]);
    b.part("mux2_1", "L5", &[], &[("d0", "m2"), ("d1", "m3"), ("sel", "s1"), ("y", "n1")]);
    b.part("mux2_1", "L6", &[], &[("d0", "n0"), ("d1", "n1"), ("sel", "s2"), ("y", "y")]);
    let names: Vec<String> = (0..8).map(|k| format!("d{k}")).collect();
    let mut ports: Vec<PortSpec<'_>> = names.iter().map(|n| (n.as_str(), IN, 1, "")).collect();
    ports.push(("sel", IN, 3, "Which input"));
    ports.push(("y", OUT, 1, ""));
    b.finish("mux8_1", "One-bit eight-way multiplexer: a tree of seven two-way ones", "MUX", &ports)
}

fn dff_nand(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    b.part("std:not", "NC", &[], &[("a", "clk"), ("y", "nclk")]);
    b.part("std:not", "ND", &[], &[("a", "d"), ("y", "nd")]);
    // The master latch is open while the clock is low…
    b.part("std:nand", "M1", &[], &[("a", "d"), ("b", "nclk"), ("y", "ms")]);
    b.part("std:nand", "M2", &[], &[("a", "nd"), ("b", "nclk"), ("y", "mr")]);
    b.part("std:nand", "M3", &[], &[("a", "ms"), ("b", "mqb"), ("y", "mq")]);
    b.part("std:nand", "M4", &[], &[("a", "mr"), ("b", "mq"), ("y", "mqb")]);
    // …and the slave while it's high, so q changes only as the clock rises.
    b.part("std:nand", "S1", &[], &[("a", "mq"), ("b", "clk"), ("y", "ss")]);
    b.part("std:nand", "S2", &[], &[("a", "mqb"), ("b", "clk"), ("y", "sr")]);
    b.part("std:nand", "S3", &[], &[("a", "ss"), ("b", "qb"), ("y", "q")]);
    b.part("std:nand", "S4", &[], &[("a", "sr"), ("b", "q"), ("y", "qb")]);
    b.finish(
        "dff_nand",
        "D flip-flop from NAND gates: a master latch open while the clock is low and a slave \
         latch open while it's high, so q takes d as the clock rises",
        "FF",
        &[("d", IN, 1, "Data"), ("clk", IN, 1, "Clock"), ("q", OUT, 1, "")],
    )
}

fn dff_r(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    b.part("std:not", "NR", &[], &[("a", "rst"), ("y", "nrst")]);
    b.part("std:and", "A1", &[], &[("a", "d"), ("b", "nrst"), ("y", "dd")]);
    b.part("dff_nand", "FF", &[], &[("d", "dd"), ("clk", "clk"), ("q", "q")]);
    b.finish(
        "dff_r",
        "D flip-flop with synchronous reset: q becomes 0 at a clock edge while rst is high",
        "FF",
        &[("d", IN, 1, ""), ("clk", IN, 1, ""), ("rst", IN, 1, "Reset"), ("q", OUT, 1, "")],
    )
}

fn dff_en(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    b.part("mux2_1", "M", &[], &[("d0", "q"), ("d1", "d"), ("sel", "en"), ("y", "dd")]);
    b.part("dff_nand", "FF", &[], &[("d", "dd"), ("clk", "clk"), ("q", "q")]);
    b.finish(
        "dff_en",
        "D flip-flop with enable: takes d at a clock edge only while en is high, else keeps q",
        "FF",
        &[("d", IN, 1, ""), ("clk", IN, 1, ""), ("en", IN, 1, "Enable"), ("q", OUT, 1, "")],
    )
}

// ---- Buses ---------------------------------------------------------------------------

fn split32(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    for i in 0..32 {
        b.slice("a", 32, i, 1, &format!("b{i}"));
    }
    let bits = bit_ports("b", OUT);
    let mut ports: Vec<PortSpec<'_>> = vec![("a", IN, 32, "The bus")];
    ports.extend(bits.iter().map(|(n, k)| (n.as_str(), *k, 1, "")));
    b.finish("split32", "Wiring: a 32-bit bus into its bits", "BS", &ports)
}

fn join32(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    let names: Vec<String> = (0..32).rev().map(|i| format!("b{i}")).collect();
    let parts: Vec<(&str, u32)> = names.iter().map(|n| (n.as_str(), 1)).collect();
    b.cat(&parts, "y");
    let bits = bit_ports("b", IN);
    let mut ports: Vec<PortSpec<'_>> = bits.iter().map(|(n, k)| (n.as_str(), *k, 1, "")).collect();
    ports.push(("y", OUT, 32, "The bus"));
    b.finish("join32", "Wiring: 32 bits into a bus (b0 lowest)", "BJ", &ports)
}

/// Splits `bus` into nets `{prefix}{i}` with a split32.
fn split_into(b: &mut Gb<'_>, name: &str, bus: &str, prefix: &str) {
    let mut nets = vec![("a".to_string(), bus.to_string())];
    nets.extend((0..32).map(|i| (format!("b{i}"), format!("{prefix}{i}"))));
    b.part_owned("split32", name, &[], &nets);
}

/// Joins nets `{prefix}{i}` into `bus` with a join32.
fn join_from(b: &mut Gb<'_>, name: &str, prefix: &str, bus: &str) {
    let mut nets: Vec<(String, String)> =
        (0..32).map(|i| (format!("b{i}"), format!("{prefix}{i}"))).collect();
    nets.push(("y".into(), bus.into()));
    b.part_owned("join32", name, &[], &nets);
}

fn mux2_32(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    split_into(&mut b, "S0", "d0", "p");
    split_into(&mut b, "S1", "d1", "q");
    for i in 0..32 {
        let (p, q, r) = (format!("p{i}"), format!("q{i}"), format!("r{i}"));
        b.part(
            "mux2_1",
            &format!("M{i}"),
            &[],
            &[("d0", &p), ("d1", &q), ("sel", "sel"), ("y", &r)],
        );
    }
    join_from(&mut b, "J", "r", "y");
    b.finish(
        "mux2_32",
        "32-bit multiplexer: 32 one-bit multiplexers sharing sel",
        "MUX",
        &[("d0", IN, 32, ""), ("d1", IN, 32, ""), ("sel", IN, 1, ""), ("y", OUT, 32, "")],
    )
}

fn mux4_32(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    b.slice("sel", 2, 0, 1, "s0");
    b.slice("sel", 2, 1, 1, "s1");
    b.part("mux2_32", "M0", &[], &[("d0", "d0"), ("d1", "d1"), ("sel", "s0"), ("y", "lo")]);
    b.part("mux2_32", "M1", &[], &[("d0", "d2"), ("d1", "d3"), ("sel", "s0"), ("y", "hi")]);
    b.part("mux2_32", "M2", &[], &[("d0", "lo"), ("d1", "hi"), ("sel", "s1"), ("y", "y")]);
    b.finish(
        "mux4_32",
        "32-bit four-way multiplexer from three two-way ones",
        "MUX",
        &[
            ("d0", IN, 32, ""),
            ("d1", IN, 32, ""),
            ("d2", IN, 32, ""),
            ("d3", IN, 32, ""),
            ("sel", IN, 2, ""),
            ("y", OUT, 32, ""),
        ],
    )
}

/// A ripple-carry adder of 32 full adders; with `carry`, its carry out is a port.
fn add32(d: &Design, carry: bool) -> CellDef {
    let mut b = Gb::new(d);
    split_into(&mut b, "SA", "a", "p");
    split_into(&mut b, "SB", "b", "q");
    for i in 0..32 {
        let cin = if i == 0 { "cin".to_string() } else { format!("c{i}") };
        let (p, q, r) = (format!("p{i}"), format!("q{i}"), format!("r{i}"));
        if i == 31 && !carry {
            // The top bit's carry goes nowhere: just its sum.
            b.part("std:xor", "X31", &[], &[("a", &p), ("b", &q), ("y", "h31")]);
            b.part("std:xor", "Y31", &[], &[("a", "h31"), ("b", &cin), ("y", &r)]);
            continue;
        }
        let cout = if i == 31 { "cout".to_string() } else { format!("c{}", i + 1) };
        b.part(
            "full_adder",
            &format!("FA{i}"),
            &[],
            &[("a", &p), ("b", &q), ("cin", &cin), ("s", &r), ("cout", &cout)],
        );
    }
    join_from(&mut b, "J", "r", "y");
    let mut ports = vec![("a", IN, 32, ""), ("b", IN, 32, ""), ("cin", IN, 1, "Carry in")];
    ports.push(("y", OUT, 32, "a + b + cin"));
    if carry {
        ports.push(("cout", OUT, 1, "Carry out"));
    }
    let (name, desc) = if carry {
        ("add32c", "32-bit ripple-carry adder of full adders, with its carry out")
    } else {
        ("add32", "32-bit ripple-carry adder of full adders")
    };
    b.finish(name, desc, "ADD", &ports)
}

/// 32 copies of a gate: `kind` is `not`, `and`, `or` or `xor`.
fn bitwise(d: &Design, kind: &str) -> CellDef {
    let mut b = Gb::new(d);
    split_into(&mut b, "SA", "a", "p");
    if kind != "not" {
        split_into(&mut b, "SB", "b", "q");
    }
    for i in 0..32 {
        let (p, q, r) = (format!("p{i}"), format!("q{i}"), format!("r{i}"));
        if kind == "not" {
            b.part("std:not", &format!("G{i}"), &[], &[("a", &p), ("y", &r)]);
        } else {
            b.part(
                &format!("std:{kind}"),
                &format!("G{i}"),
                &[],
                &[("a", &p), ("b", &q), ("y", &r)],
            );
        }
    }
    join_from(&mut b, "J", "r", "y");
    let mut ports = vec![("a", IN, 32, "")];
    if kind != "not" {
        ports.push(("b", IN, 32, ""));
    }
    ports.push(("y", OUT, 32, ""));
    let name = format!("{kind}32");
    b.finish(&name, &format!("32 {} gates side by side", kind.to_uppercase()), "G", &ports)
}

/// A 32-bit register of flip-flops: with reset (`dff_r`) or with enable (`dff_en`).
fn reg32(d: &Design, reset: bool) -> CellDef {
    let mut b = Gb::new(d);
    split_into(&mut b, "SD", "d", "p");
    let (ff, ctl) = if reset { ("dff_r", "rst") } else { ("dff_en", "en") };
    for i in 0..32 {
        let (p, r) = (format!("p{i}"), format!("r{i}"));
        b.part(ff, &format!("F{i}"), &[], &[("d", &p), ("clk", "clk"), (ctl, ctl), ("q", &r)]);
    }
    join_from(&mut b, "J", "r", "q");
    let (name, desc, ctl_desc) = if reset {
        ("reg32_r", "32-bit register of flip-flops, cleared by rst at a clock edge", "Reset")
    } else {
        (
            "reg32_e",
            "32-bit register of flip-flops, loaded at a clock edge while en is high",
            "Enable",
        )
    };
    b.finish(
        name,
        desc,
        "REG",
        &[("d", IN, 32, ""), ("clk", IN, 1, "Clock"), (ctl, IN, 1, ctl_desc), ("q", OUT, 32, "")],
    )
}

// ---- Blocks --------------------------------------------------------------------------

fn decode5(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    for k in 0..5 {
        b.slice("a", 5, k, 1, &format!("a{k}"));
        b.inv(&format!("a{k}"), &format!("na{k}"));
    }
    // The low two bits decoded…
    for v in 0..4 {
        let l0 = if v & 1 == 1 { "a0" } else { "na0" };
        let l1 = if v & 2 == 2 { "a1" } else { "na1" };
        b.gate("and", l1, l0, &format!("lo{v}"));
    }
    // …the high three with the enable…
    for v in 0..8 {
        let lit = |k: u32| if v >> (k - 2) & 1 == 1 { format!("a{k}") } else { format!("na{k}") };
        let (l2, l3, l4) = (lit(2), lit(3), lit(4));
        b.tree("and", &[&l4, &l3, &l2, "en"], &format!("hi{v}"));
    }
    // …and each output from one of each.
    for i in 0..32 {
        b.gate("and", &format!("hi{}", i >> 2), &format!("lo{}", i & 3), &format!("o{i}"));
    }
    join_from(&mut b, "J", "o", "y");
    b.finish(
        "decode5",
        "Five-to-32 decoder from gates: bit a of y is high while en is",
        "DEC",
        &[("a", IN, 5, "Which output"), ("en", IN, 1, "Enable"), ("y", OUT, 32, "One-hot")],
    )
}

fn read_port(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    for k in 0..5 {
        b.slice("ra", 5, k, 1, &format!("s{k}"));
    }
    let mut prev: Vec<String> = (0..32).map(|i| format!("r{i}")).collect();
    for level in 0..5 {
        let mut next = Vec::new();
        for (k, pair) in prev.chunks(2).enumerate() {
            let y = if prev.len() == 2 { "rd".to_string() } else { format!("l{level}_{k}") };
            let sel = format!("s{level}");
            b.part(
                "mux2_32",
                &format!("M{level}_{k}"),
                &[],
                &[("d0", &pair[0]), ("d1", &pair[1]), ("sel", &sel), ("y", &y)],
            );
            next.push(y);
        }
        prev = next;
    }
    let regs: Vec<String> = (0..32).map(|i| format!("r{i}")).collect();
    let mut ports: Vec<PortSpec<'_>> = regs.iter().map(|n| (n.as_str(), IN, 32, "")).collect();
    ports.push(("ra", IN, 5, "Which register"));
    ports.push(("rd", OUT, 32, "Its value"));
    b.finish(
        "read_port",
        "Register file read port: a five-level tree of 32-bit multiplexers picks register ra",
        "RP",
        &ports,
    )
}

fn regfile_gates(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    b.limit = 120;
    b.konst(32, 0, "x0");
    b.part("decode5", "DEC", &[], &[("a", "wa"), ("en", "we"), ("y", "wen")]);
    for k in 1..32 {
        let (e, x) = (format!("e{k}"), format!("x{k}"));
        b.slice("wen", 32, k, 1, &e);
        b.part(
            "reg32_e",
            &format!("X{k}"),
            &[],
            &[("d", "wd"), ("clk", "clk"), ("en", &e), ("q", &x)],
        );
    }
    for (name, ra, rd) in [("RP1", "ra1", "rd1"), ("RP2", "ra2", "rd2")] {
        let mut nets: Vec<(String, String)> =
            (0..32).map(|i| (format!("r{i}"), format!("x{i}"))).collect();
        nets.push(("ra".into(), ra.into()));
        nets.push(("rd".into(), rd.into()));
        b.part_owned("read_port", name, &[], &nets);
    }
    b.finish(
        "regfile_gates",
        "Register file from gates: x0 wired to zero, x1 to x31 each 32 enabled flip-flops \
         (992 in all), a decoder choosing which one a write loads, and two read ports",
        "RF",
        &[
            ("ra1", IN, 5, "Read address 1"),
            ("ra2", IN, 5, "Read address 2"),
            ("wa", IN, 5, "Write address"),
            ("wd", IN, 32, "Write data"),
            ("we", IN, 1, "Write enable"),
            ("clk", IN, 1, "Clock"),
            ("rd1", OUT, 32, "x[ra1]"),
            ("rd2", OUT, 32, "x[ra2]"),
        ],
    )
}

/// A barrel shifter: five stages, each shifting by 1, 2, 4, 8 or 16 places or not.
fn shifter(d: &Design, left: bool) -> CellDef {
    let mut b = Gb::new(d);
    for k in 0..5 {
        b.slice("sh", 5, k, 1, &format!("s{k}"));
    }
    if !left {
        b.slice("a", 32, 31, 1, "top");
        b.gate("and", "arith", "top", "fill");
    }
    let mut x = "a".to_string();
    for s in 0..5u32 {
        let k = 1u32 << s;
        let (shifted, y) =
            (format!("v{s}"), if s == 4 { "y".to_string() } else { format!("x{s}") });
        if left {
            let (lo, z) = (format!("lo{s}"), format!("z{s}"));
            b.slice(&x, 32, 0, 32 - k, &lo);
            b.konst(k, 0, &z);
            b.cat(&[(&lo, 32 - k), (&z, k)], &shifted);
        } else {
            let (hi, f) = (format!("hi{s}"), format!("f{s}"));
            b.slice(&x, 32, k, 32 - k, &hi);
            b.extend("fill", 1, k, true, &f);
            b.cat(&[(&f, k), (&hi, 32 - k)], &shifted);
        }
        let sel = format!("s{s}");
        b.part(
            "mux2_32",
            &format!("ST{s}"),
            &[],
            &[("d0", &x), ("d1", &shifted), ("sel", &sel), ("y", &y)],
        );
        x = y;
    }
    if left {
        b.finish(
            "shl32",
            "Barrel shifter, left: five stages of multiplexers shift by 1, 2, 4, 8 and 16",
            "SH",
            &[("a", IN, 32, ""), ("sh", IN, 5, "Places"), ("y", OUT, 32, "a << sh")],
        )
    } else {
        b.finish(
            "shr32",
            "Barrel shifter, right: fills with zeros, or with the sign bit when arith is high",
            "SH",
            &[
                ("a", IN, 32, ""),
                ("sh", IN, 5, "Places"),
                ("arith", IN, 1, "Copy the sign bit in (SRA)"),
                ("y", OUT, 32, "a >> sh"),
            ],
        )
    }
}

fn alu_gates(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    for k in 1..4 {
        b.slice("op", 4, k, 1, &format!("op{k}"));
    }
    // Subtract for SUB, SLT and SLTU: b inverted, plus one.
    b.inv("op2", "nop2");
    b.gate("and", "op1", "nop2", "slt_op");
    b.gate("or", "op3", "slt_op", "sub");
    b.extend("sub", 1, 32, true, "subs");
    b.part("xor32", "XB", &[], &[("a", "b"), ("b", "subs"), ("y", "bx")]);
    b.part(
        "add32c",
        "ADD",
        &[],
        &[("a", "a"), ("b", "bx"), ("cin", "sub"), ("y", "sum"), ("cout", "carry")],
    );
    // Less than: signed from the signs and the difference, unsigned from the carry.
    b.slice("a", 32, 31, 1, "a31");
    b.slice("b", 32, 31, 1, "b31");
    b.slice("sum", 32, 31, 1, "s31");
    b.gate("xor", "a31", "b31", "dsign");
    b.part("mux2_1", "LT", &[], &[("d0", "s31"), ("d1", "a31"), ("sel", "dsign"), ("y", "lt")]);
    b.inv("carry", "ltu");
    b.extend("lt", 1, 32, false, "lt32");
    b.extend("ltu", 1, 32, false, "ltu32");
    // Shifts by b's low five bits.
    b.slice("b", 32, 0, 5, "shamt");
    b.part("shl32", "SHL", &[], &[("a", "a"), ("sh", "shamt"), ("y", "sll")]);
    b.part("shr32", "SHR", &[], &[("a", "a"), ("sh", "shamt"), ("arith", "op3"), ("y", "srl")]);
    b.part("xor32", "XOR", &[], &[("a", "a"), ("b", "b"), ("y", "xr")]);
    b.part("or32", "OR", &[], &[("a", "a"), ("b", "b"), ("y", "orv")]);
    b.part("and32", "AND", &[], &[("a", "a"), ("b", "b"), ("y", "andv")]);
    // The result by funct3.
    b.slice("op", 4, 0, 2, "op_lo");
    b.part(
        "mux4_32",
        "RLO",
        &[],
        &[
            ("d0", "sum"),
            ("d1", "sll"),
            ("d2", "lt32"),
            ("d3", "ltu32"),
            ("sel", "op_lo"),
            ("y", "mlo"),
        ],
    );
    b.part(
        "mux4_32",
        "RHI",
        &[],
        &[
            ("d0", "xr"),
            ("d1", "srl"),
            ("d2", "orv"),
            ("d3", "andv"),
            ("sel", "op_lo"),
            ("y", "mhi"),
        ],
    );
    b.part("mux2_32", "RES", &[], &[("d0", "mlo"), ("d1", "mhi"), ("sel", "op2"), ("y", "y")]);
    b.finish(
        "alu_gates",
        "ALU from gates, op = {funct7[5], funct3}: an adder that also subtracts, comparisons \
         from its result, two barrel shifters, AND/OR/XOR, and multiplexers picking the result",
        "ALU",
        &[("a", IN, 32, ""), ("b", IN, 32, ""), ("op", IN, 4, "Operation"), ("y", OUT, 32, "")],
    )
}

fn compare32(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    b.part("not32", "NB", &[], &[("a", "b"), ("y", "nb")]);
    b.konst(1, 1, "one");
    b.part(
        "add32c",
        "SUB",
        &[],
        &[("a", "a"), ("b", "nb"), ("cin", "one"), ("y", "diff"), ("cout", "carry")],
    );
    split_into(&mut b, "SD", "diff", "d");
    let bits: Vec<String> = (0..32).map(|i| format!("d{i}")).collect();
    let refs: Vec<&str> = bits.iter().map(String::as_str).collect();
    b.tree("or", &refs, "any");
    b.inv("any", "eq");
    b.slice("a", 32, 31, 1, "a31");
    b.slice("b", 32, 31, 1, "b31");
    b.gate("xor", "a31", "b31", "dsign");
    b.part("mux2_1", "LT", &[], &[("d0", "d31"), ("d1", "a31"), ("sel", "dsign"), ("y", "lt")]);
    b.inv("carry", "ltu");
    b.finish(
        "compare32",
        "Comparator from gates: a - b by an adder, equal when every bit of it is 0",
        "CMP",
        &[
            ("a", IN, 32, ""),
            ("b", IN, 32, ""),
            ("eq", OUT, 1, "a = b"),
            ("lt", OUT, 1, "a < b, signed"),
            ("ltu", OUT, 1, "a < b, unsigned"),
        ],
    )
}

fn branch_unit_gates(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    b.part(
        "compare32",
        "CMP",
        &[],
        &[("a", "a"), ("b", "b"), ("eq", "eq"), ("lt", "lt"), ("ltu", "ltu")],
    );
    b.part("std:not", "N1", &[], &[("a", "eq"), ("y", "ne")]);
    b.part("std:not", "N2", &[], &[("a", "lt"), ("y", "ge")]);
    b.part("std:not", "N3", &[], &[("a", "ltu"), ("y", "geu")]);
    b.konst(1, 0, "never");
    b.part(
        "mux8_1",
        "SEL",
        &[],
        &[
            ("d0", "eq"),
            ("d1", "ne"),
            ("d2", "never"),
            ("d3", "never"),
            ("d4", "lt"),
            ("d5", "ge"),
            ("d6", "ltu"),
            ("d7", "geu"),
            ("sel", "funct3"),
            ("y", "cond"),
        ],
    );
    b.part("std:and", "A1", &[], &[("a", "branch"), ("b", "cond"), ("y", "taken")]);
    b.part("std:or", "O1", &[], &[("a", "taken"), ("b", "jump"), ("y", "take")]);
    b.finish(
        "branch_unit_gates",
        "Branch unit from gates: take = (branch and the funct3 condition holds) or jump",
        "BU",
        &[
            ("a", IN, 32, "rs1"),
            ("b", IN, 32, "rs2"),
            ("funct3", IN, 3, "Which comparison"),
            ("branch", IN, 1, "A branch instruction"),
            ("jump", IN, 1, "JAL: always taken"),
            ("take", OUT, 1, "Go to pc + imm"),
        ],
    )
}

/// Opcodes (bits 6 to 2) of the instruction kinds the decoders need.
const KINDS: [(&str, u32); 10] = [
    ("R", 0b01100),
    ("I", 0b00100),
    ("LD", 0b00000),
    ("ST", 0b01000),
    ("BR", 0b11000),
    ("JAL", 0b11011),
    ("JALR", 0b11001),
    ("LUI", 0b01101),
    ("AUIPC", 0b00101),
    ("SYS", 0b11100),
];

/// Decodes the opcode of `instr` into one net per kind in `which`, each high for its
/// kind of instruction (and bits 1 and 0 must be 11).
fn decode_kinds(b: &mut Gb<'_>, which: &[&str]) {
    for k in 0..7 {
        b.slice("instr", 32, k, 1, &format!("o{k}"));
    }
    for k in 2..7 {
        b.inv(&format!("o{k}"), &format!("no{k}"));
    }
    b.gate("and", "o0", "o1", "valid");
    for (name, code) in KINDS.iter().filter(|(n, _)| which.contains(n)) {
        let lits: Vec<String> = (2..7)
            .rev()
            .map(|k| if code >> (k - 2) & 1 == 1 { format!("o{k}") } else { format!("no{k}") })
            .collect();
        let mut ins: Vec<&str> = lits.iter().map(String::as_str).collect();
        ins.push("valid");
        b.tree("and", &ins, name);
    }
}

fn control_gates(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    decode_kinds(&mut b, &KINDS.map(|k| k.0));
    b.tree("or", &["R", "I", "LD", "JAL", "JALR", "LUI", "AUIPC"], "reg_write");
    b.tree("or", &["I", "LD", "ST", "JALR", "LUI", "AUIPC"], "alu_b");
    b.cat(&[("LUI", 1), ("AUIPC", 1)], "alu_a");
    // The ALU operation: funct3 for arithmetic, with funct7[5] for SUB and SRA (and SRAI);
    // subtract for branches.
    b.slice("instr", 32, 12, 1, "f0");
    b.slice("instr", 32, 13, 1, "f1");
    b.slice("instr", 32, 14, 1, "f2");
    b.slice("instr", 32, 30, 1, "f7");
    b.gate("or", "R", "I", "ri");
    for k in 0..3 {
        b.gate("and", &format!("f{k}"), "ri", &format!("aop{k}"));
    }
    b.inv("f1", "nf1");
    b.tree("and", &["f2", "nf1", "f0"], "srx");
    b.gate("and", "R", "f7", "t1");
    b.tree("and", &["I", "f7", "srx"], "t2");
    b.tree("or", &["t1", "t2", "BR"], "aop3");
    b.cat(&[("aop3", 1), ("aop2", 1), ("aop1", 1), ("aop0", 1)], "alu_op");
    b.gate("or", "JAL", "JALR", "link");
    b.cat(&[("link", 1), ("LD", 1)], "wb_sel");
    b.finish_mapped(
        "control_gates",
        "Control unit from gates: the opcode decoded by AND gates into the instruction's \
         kind, and the control signals ORed from the kinds",
        "CTRL",
        &[
            (("instr", IN, 32, ""), "instr"),
            (("reg_write", OUT, 1, ""), "reg_write"),
            (("alu_a", OUT, 2, "0 rs1, 1 pc, 2 zero"), "alu_a"),
            (("alu_b", OUT, 1, "0 rs2, 1 imm"), "alu_b"),
            (("alu_op", OUT, 4, ""), "alu_op"),
            (("mem_write", OUT, 1, ""), "ST"),
            (("wb_sel", OUT, 2, "0 ALU, 1 memory, 2 pc + 4"), "wb_sel"),
            (("branch", OUT, 1, ""), "BR"),
            (("jump", OUT, 1, ""), "JAL"),
            (("jalr", OUT, 1, ""), "JALR"),
            (("halt", OUT, 1, "ECALL"), "SYS"),
        ],
    )
}

fn immgen_gates(d: &Design) -> CellDef {
    let mut b = Gb::new(d);
    decode_kinds(&mut b, &["I", "LD", "JALR", "SYS", "ST", "BR", "LUI", "AUIPC", "JAL"]);
    b.tree("or", &["I", "LD", "JALR", "SYS"], "is_i");
    b.gate("or", "LUI", "AUIPC", "is_u");
    // Each format's immediate is only wiring: bits moved and the sign copied.
    b.slice("instr", 32, 31, 1, "sign");
    b.extend("sign", 1, 21, true, "sx21");
    b.extend("sign", 1, 20, true, "sx20");
    b.extend("sign", 1, 12, true, "sx12");
    b.slice("instr", 32, 20, 11, "i30_20");
    b.slice("instr", 32, 25, 6, "i30_25");
    b.slice("instr", 32, 7, 5, "i11_7");
    b.slice("instr", 32, 7, 1, "i7");
    b.slice("instr", 32, 8, 4, "i11_8");
    b.slice("instr", 32, 12, 20, "i31_12");
    b.slice("instr", 32, 12, 8, "i19_12");
    b.slice("instr", 32, 20, 1, "i20");
    b.slice("instr", 32, 21, 10, "i30_21");
    b.konst(1, 0, "z1");
    b.konst(12, 0, "z12");
    b.konst(32, 0, "zero");
    b.cat(&[("sx21", 21), ("i30_20", 11)], "imm_i");
    b.cat(&[("sx21", 21), ("i30_25", 6), ("i11_7", 5)], "imm_s");
    b.cat(&[("sx20", 20), ("i7", 1), ("i30_25", 6), ("i11_8", 4), ("z1", 1)], "imm_b");
    b.cat(&[("i31_12", 20), ("z12", 12)], "imm_u");
    b.cat(&[("sx12", 12), ("i19_12", 8), ("i20", 1), ("i30_21", 10), ("z1", 1)], "imm_j");
    // The one for this instruction's kind (none: zero).
    b.part("mux2_32", "MI", &[], &[("d0", "zero"), ("d1", "imm_i"), ("sel", "is_i"), ("y", "m1")]);
    b.part("mux2_32", "MS", &[], &[("d0", "m1"), ("d1", "imm_s"), ("sel", "ST"), ("y", "m2")]);
    b.part("mux2_32", "MB", &[], &[("d0", "m2"), ("d1", "imm_b"), ("sel", "BR"), ("y", "m3")]);
    b.part("mux2_32", "MU", &[], &[("d0", "m3"), ("d1", "imm_u"), ("sel", "is_u"), ("y", "m4")]);
    b.part("mux2_32", "MJ", &[], &[("d0", "m4"), ("d1", "imm_j"), ("sel", "JAL"), ("y", "imm")]);
    b.finish(
        "immgen_gates",
        "Immediate generator from gates: the five formats wired from the instruction's bits, \
         and multiplexers picking the one for its kind",
        "IMM",
        &[("instr", IN, 32, ""), ("imm", OUT, 32, "")],
    )
}

fn cpu_gates(d: &Design) -> CellDef {
    let mut sh = Sheet::new(d, PROJECT);
    let prog = "programs/fib.s";
    let clk = sh.part("std:clock", "CLK", 0, 0, &[("frequency", "2meg")]);
    let rst = sh.part("std:reset", "RST", 0, 12, &[("length", "600n")]);
    let pc = sh.part("reg32_r", "PC", 0, 22, &[]);
    let k4 = sh.part("std:const", "K4", 0, 36, &[("width", "32"), ("value", "4")]);
    let kc = sh.part("std:const", "KC", 0, 42, &[("width", "1"), ("value", "0")]);
    let pc4 = sh.part("add32", "PC4", 0, 50, &[]);
    let pci = sh.part("add32", "PCIMM", 0, 64, &[]);
    sh.nets(clk, &[("p", "clk"), ("n", "GND")]);
    sh.nets(rst, &[("rst", "rst")]);
    sh.nets(pc, &[("d", "pc_next"), ("clk", "clk"), ("rst", "rst"), ("q", "pc")]);
    sh.nets(k4, &[("y", "four")]);
    sh.nets(kc, &[("y", "no_carry")]);
    sh.nets(pc4, &[("a", "pc"), ("b", "four"), ("cin", "no_carry"), ("y", "pc4")]);
    sh.nets(pci, &[("a", "pc"), ("b", "imm"), ("cin", "no_carry"), ("y", "pc_imm")]);
    let imem = sh.part("std:rom", "IMEM", 30, 0, &[("program", prog)]);
    let rs1 = sh.part("std:slice", "RS1", 30, 10, &[("from", "32"), ("lo", "15"), ("width", "5")]);
    let rs2 = sh.part("std:slice", "RS2", 30, 18, &[("from", "32"), ("lo", "20"), ("width", "5")]);
    let rd = sh.part("std:slice", "RD", 30, 26, &[("from", "32"), ("lo", "7"), ("width", "5")]);
    let f3 = sh.part("std:slice", "F3", 30, 34, &[("from", "32"), ("lo", "12"), ("width", "3")]);
    let imm = sh.part("immgen_gates", "IMMGEN", 30, 42, &[]);
    let ctrl = sh.part("control_gates", "CTRL", 30, 52, &[]);
    sh.nets(imem, &[("addr", "pc"), ("data", "instr")]);
    sh.nets(rs1, &[("a", "instr"), ("y", "rs1")]);
    sh.nets(rs2, &[("a", "instr"), ("y", "rs2")]);
    sh.nets(rd, &[("a", "instr"), ("y", "rd")]);
    sh.nets(f3, &[("a", "instr"), ("y", "funct3")]);
    sh.nets(imm, &[("instr", "instr"), ("imm", "imm")]);
    sh.nets(
        ctrl,
        &[
            ("instr", "instr"),
            ("reg_write", "reg_write"),
            ("alu_a", "alu_a"),
            ("alu_b", "alu_b"),
            ("alu_op", "alu_op"),
            ("mem_write", "mem_write"),
            ("wb_sel", "wb_sel"),
            ("branch", "branch"),
            ("jump", "jump"),
            ("jalr", "jalr"),
            ("halt", "halt"),
        ],
    );
    let rf = sh.part("regfile_gates", "RF", 66, 0, &[]);
    let k0 = sh.part("std:const", "K0", 66, 24, &[("width", "32"), ("value", "0")]);
    let amux = sh.part("mux4_32", "AMUX", 66, 34, &[]);
    let bmux = sh.part("mux2_32", "BMUX", 66, 52, &[]);
    let alu = sh.part("alu_gates", "ALU", 66, 64, &[]);
    sh.nets(
        rf,
        &[
            ("ra1", "rs1"),
            ("ra2", "rs2"),
            ("wa", "rd"),
            ("wd", "wb"),
            ("we", "reg_write"),
            ("clk", "clk"),
            ("rd1", "a_reg"),
            ("rd2", "b_reg"),
        ],
    );
    sh.nets(k0, &[("y", "zero32")]);
    sh.nets(
        amux,
        &[
            ("d0", "a_reg"),
            ("d1", "pc"),
            ("d2", "zero32"),
            ("d3", "a_reg"),
            ("sel", "alu_a"),
            ("y", "alu_in_a"),
        ],
    );
    sh.nets(bmux, &[("d0", "b_reg"), ("d1", "imm"), ("sel", "alu_b"), ("y", "alu_in_b")]);
    sh.nets(alu, &[("a", "alu_in_a"), ("b", "alu_in_b"), ("op", "alu_op"), ("y", "alu_y")]);
    let dmem = sh.part("std:dmem", "DMEM", 100, 0, &[("program", prog)]);
    let wb = sh.part("mux4_32", "WB", 100, 20, &[]);
    sh.nets(
        dmem,
        &[
            ("addr", "alu_y"),
            ("wd", "b_reg"),
            ("we", "mem_write"),
            ("size", "funct3"),
            ("clk", "clk"),
            ("rd", "mem_rd"),
        ],
    );
    sh.nets(
        wb,
        &[
            ("d0", "alu_y"),
            ("d1", "mem_rd"),
            ("d2", "pc4"),
            ("d3", "alu_y"),
            ("sel", "wb_sel"),
            ("y", "wb"),
        ],
    );
    let bu = sh.part("branch_unit_gates", "BU", 132, 0, &[]);
    let npc1 = sh.part("mux2_32", "NPC1", 132, 18, &[]);
    let jt1 = sh.part("std:slice", "JHI", 132, 30, &[("from", "32"), ("lo", "1"), ("width", "31")]);
    let kb = sh.part("std:const", "KB0", 132, 38, &[("width", "1"), ("value", "0")]);
    let jt = sh.part("std:concat", "JT", 132, 46, &[("wa", "31"), ("wb", "1")]);
    let npc2 = sh.part("mux2_32", "NPC2", 132, 56, &[]);
    let halt = sh.part("std:halt", "HALT", 132, 68, &[]);
    sh.nets(
        bu,
        &[
            ("a", "a_reg"),
            ("b", "b_reg"),
            ("funct3", "funct3"),
            ("branch", "branch"),
            ("jump", "jump"),
            ("take", "take"),
        ],
    );
    sh.nets(npc1, &[("d0", "pc4"), ("d1", "pc_imm"), ("sel", "take"), ("y", "pc_seq")]);
    sh.nets(jt1, &[("a", "alu_y"), ("y", "jt_hi")]);
    sh.nets(kb, &[("y", "bit0")]);
    sh.nets(jt, &[("a", "jt_hi"), ("b", "bit0"), ("y", "jalr_target")]);
    sh.nets(npc2, &[("d0", "pc_seq"), ("d1", "jalr_target"), ("sel", "jalr"), ("y", "pc_next")]);
    sh.nets(halt, &[("a", "halt"), ("clk", "clk")]);
    let mut def = component(
        "cpu_gates",
        "The single-cycle RV32I processor with every block made of logic gates (about 40,000 \
         of them, flip-flops included); only the program ROM and data RAM are arrays. Slower \
         to simulate than cpu, and its clock is 2 MHz: a cycle must cover the ripple-carry \
         adders' gate delays",
        "CPU",
        vec![],
        sh.s,
    );
    def.symbol = None;
    def
}

type MakeCell = fn(&Design) -> CellDef;

/// Every gate-level cell, in the order they must be made (each uses ones before it),
/// with its library folder.
pub fn cells() -> Vec<(MakeCell, &'static str)> {
    vec![
        (mux2_1, "gate_level/bit"),
        (mux8_1, "gate_level/bit"),
        (dff_nand, "gate_level/bit"),
        (dff_r, "gate_level/bit"),
        (dff_en, "gate_level/bit"),
        (split32, "gate_level/bus"),
        (join32, "gate_level/bus"),
        (mux2_32, "gate_level/bus"),
        (mux4_32, "gate_level/bus"),
        (|d| add32(d, false), "gate_level/bus"),
        (|d| add32(d, true), "gate_level/bus"),
        (|d| bitwise(d, "not"), "gate_level/bus"),
        (|d| bitwise(d, "and"), "gate_level/bus"),
        (|d| bitwise(d, "or"), "gate_level/bus"),
        (|d| bitwise(d, "xor"), "gate_level/bus"),
        (|d| reg32(d, true), "gate_level/bus"),
        (|d| reg32(d, false), "gate_level/bus"),
        (decode5, "gate_level/blocks"),
        (read_port, "gate_level/blocks"),
        (regfile_gates, "gate_level/blocks"),
        (|d| shifter(d, true), "gate_level/blocks"),
        (|d| shifter(d, false), "gate_level/blocks"),
        (alu_gates, "gate_level/blocks"),
        (compare32, "gate_level/blocks"),
        (branch_unit_gates, "gate_level/blocks"),
        (control_gates, "gate_level/blocks"),
        (immgen_gates, "gate_level/blocks"),
        (cpu_gates, "cpu"),
    ]
}
