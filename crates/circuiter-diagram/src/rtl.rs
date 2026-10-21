//! The RTL view of a Verilog or SystemVerilog module, as synthesis tools show one: each
//! instance of another module, each register (`always_ff`), each block of logic
//! (`always_comb`) and each `assign` as a box, joined by the signals between them and
//! laid out by the layout engine.

use crate::sheet::arrange_sheet;
use circuiter_hdl::parse::{self, Dir, Expr, Item, LValue, Module, Stmt};
use circuiter_model::builder;
use circuiter_model::cell::{Cell, CellDef, CellRef, Port, PortKind};
use circuiter_model::geom::{Length, Point, Transform};
use circuiter_model::library::Library;
use circuiter_model::schematic::{Instance, NetLabel, Schematic, SchematicPort};
use circuiter_model::{CellId, Design};
use std::collections::{BTreeMap, BTreeSet};

/// A module as boxes and the signals joining them.
#[derive(Debug, Clone, PartialEq)]
pub struct Rtl {
    pub module: String,
    pub ports: Vec<(String, Dir)>,
    pub blocks: Vec<Block>,
    /// Each signal's width in bits, where the module could be compiled to know it.
    pub widths: BTreeMap<String, u32>,
}

/// One box: an instance, a register, a block of logic or an operator.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub name: String,
    /// What it is: the module instantiated, `register`, `logic`, `mux`, `add`…
    pub kind: String,
    /// (pin, signal) for what it reads and what it drives.
    pub inputs: Vec<(String, String)>,
    pub outputs: Vec<(String, String)>,
}

/// The diagram of a module, ready to draw: the parts it shows are cells of the design
/// made for it.
#[derive(Debug, Clone)]
pub struct RtlDiagram {
    pub design: Design,
    pub id: CellId,
    pub sheet: Schematic,
    /// The modules in the file, for going inside an instance of one.
    pub modules: Vec<String>,
    /// What each box is (`register`, `mux`, a module's name), by its name.
    pub kinds: BTreeMap<String, String>,
}

/// The library the diagram's cells are in.
pub const LIBRARY: &str = "rtl";

/// The RTL diagram of `module` in `src` (the file's last module when none is named).
pub fn diagram(src: &str, module: Option<&str>) -> Result<RtlDiagram, String> {
    let modules = parse::modules(src).map_err(|e| e.to_string())?;
    let rtl = elaborate(&modules, module)?;
    let (design, id, sheet) = to_sheet(&rtl)?;
    let def = design.cell(&id).expect("made");
    let sheet = arrange_sheet(&design, LIBRARY, &def.cell, &sheet);
    let kinds = rtl.blocks.iter().map(|b| (b.name.clone(), b.kind.clone())).collect();
    Ok(RtlDiagram {
        design,
        id,
        sheet,
        modules: modules.into_iter().map(|m| m.name).collect(),
        kinds,
    })
}

/// `module` (or the last) of `modules` as boxes and signals.
pub fn elaborate(modules: &[Module], module: Option<&str>) -> Result<Rtl, String> {
    let m = match module {
        Some(name) => {
            modules.iter().find(|m| m.name == name).ok_or_else(|| format!("no module `{name}`"))?
        }
        None => modules.last().ok_or("no module in the file")?,
    };
    // Names that aren't signals: parameters, loop variables and functions.
    let mut skip: BTreeSet<String> = m.params.iter().map(|(n, _)| n.clone()).collect();
    collect_constants(&m.items, &mut skip);
    let mut blocks = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    add_items(&m.items, modules, &skip, &mut blocks, &mut counts);
    // Names unique, as parts on a sheet must be.
    let mut seen = BTreeSet::new();
    for b in &mut blocks {
        let mut name = b.name.clone();
        let mut k = 1;
        while !seen.insert(name.clone()) {
            k += 1;
            name = format!("{}_{k}", b.name);
        }
        b.name = name;
    }
    // Widths are only shown, so a module that won't compile is drawn without them.
    let widths = circuiter_hdl::sim::widths(modules, Some(&m.name)).unwrap_or_default();
    Ok(Rtl {
        module: m.name.clone(),
        ports: m.ports.iter().map(|p| (p.name.clone(), p.dir)).collect(),
        blocks,
        widths,
    })
}

fn collect_constants(items: &[Item], skip: &mut BTreeSet<String>) {
    for item in items {
        match item {
            Item::Param(name, _) => {
                skip.insert(name.clone());
            }
            Item::Function(f) => {
                skip.insert(f.name.clone());
            }
            Item::GenFor { var, items, .. } => {
                skip.insert(var.clone());
                collect_constants(items, skip);
            }
            Item::GenIf(_, a, b) => {
                collect_constants(a, skip);
                collect_constants(b, skip);
            }
            _ => {}
        }
    }
}

fn add_items(
    items: &[Item],
    modules: &[Module],
    skip: &BTreeSet<String>,
    blocks: &mut Vec<Block>,
    counts: &mut BTreeMap<String, usize>,
) {
    fn numbered(kind: &str, counts: &mut BTreeMap<String, usize>) -> String {
        let n = counts.entry(kind.to_owned()).or_default();
        *n += 1;
        format!("{kind}{n}")
    }
    for item in items {
        match item {
            Item::Assign(lhs, rhs, _) => {
                let outs = writes(lhs, skip);
                let mut ins = Vec::new();
                reads(rhs, skip, &mut ins);
                lvalue_reads(lhs, skip, &mut ins);
                let kind = operator(rhs);
                let name = outs.first().cloned().unwrap_or_else(|| numbered(&kind, counts));
                blocks.push(by_signal(name, kind, ins, outs));
            }
            Item::Comb(stmt) => {
                let (ins, outs) = statement(stmt, skip);
                let name = numbered("comb", counts);
                blocks.push(by_signal(name, "logic".into(), ins, outs));
            }
            Item::Seq(edges, stmt) => {
                let (body, outs) = statement(stmt, skip);
                // Clock and reset first.
                let mut ins: Vec<String> = edges.iter().map(|(_, s)| s.clone()).collect();
                ins.extend(body);
                let name = match outs.as_slice() {
                    [one] => format!("{one}_reg"),
                    _ => numbered("reg", counts),
                };
                blocks.push(by_signal(name, "register".into(), ins, outs));
            }
            Item::Instance { module, name, conns, .. } => {
                let def = modules.iter().find(|m| m.name == *module);
                let mut inputs = Vec::new();
                let mut outputs = Vec::new();
                for (k, (port, expr)) in conns.iter().enumerate() {
                    let decl = match port {
                        Some(p) => def.and_then(|d| d.ports.iter().find(|q| q.name == *p)),
                        None => def.and_then(|d| d.ports.get(k)),
                    };
                    let pin = port
                        .clone()
                        .or_else(|| decl.map(|d| d.name.clone()))
                        .unwrap_or_else(|| format!("p{k}"));
                    let Some(expr) = expr else { continue };
                    let mut signals = Vec::new();
                    reads(expr, skip, &mut signals);
                    let several = signals.len() > 1;
                    let output = decl.is_some_and(|d| d.dir == Dir::Output);
                    for s in signals {
                        let pin = if several { format!("{pin}:{s}") } else { pin.clone() };
                        if output { outputs.push((pin, s)) } else { inputs.push((pin, s)) }
                    }
                }
                blocks.push(Block { name: name.clone(), kind: module.clone(), inputs, outputs });
            }
            Item::GenFor { items, .. } => add_items(items, modules, skip, blocks, counts),
            Item::GenIf(_, a, b) => {
                add_items(a, modules, skip, blocks, counts);
                add_items(b, modules, skip, blocks, counts);
            }
            Item::Decl { .. } | Item::Param(..) | Item::Function(_) => {}
        }
    }
}

/// A box whose pins are named after the signals on them.
fn by_signal(name: String, kind: String, ins: Vec<String>, outs: Vec<String>) -> Block {
    // A signal both read and written (a register holding its value) is an output.
    let ins = ins.into_iter().filter(|s| !outs.contains(s));
    Block {
        name,
        kind,
        inputs: dedup(ins).into_iter().map(|s| (s.clone(), s)).collect(),
        outputs: dedup(outs).into_iter().map(|s| (s.clone(), s)).collect(),
    }
}

fn dedup(signals: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    signals.into_iter().filter(|s| seen.insert(s.clone())).collect()
}

/// What an `assign` does, by its expression's outermost operation.
fn operator(e: &Expr) -> String {
    match e {
        Expr::Ternary(..) => "mux",
        Expr::Binary(op, ..) => match *op {
            "+" => "add",
            "-" => "sub",
            "*" => "mul",
            "/" | "%" => "div",
            "&" => "and",
            "|" => "or",
            "^" | "~^" | "^~" => "xor",
            "==" | "!=" | "===" | "!==" | "<" | "<=" | ">" | ">=" => "compare",
            "<<" | ">>" | "<<<" | ">>>" => "shift",
            "&&" | "||" => "logic",
            _ => "op",
        },
        Expr::Unary("~" | "!", _) => "not",
        Expr::Unary(..) => "reduce",
        Expr::Concat(_) | Expr::Repeat(..) => "concat",
        Expr::Index(..) | Expr::Slice(..) | Expr::IndexedPart(..) => "slice",
        Expr::Ident(..) => "buf",
        Expr::Num { .. } | Expr::Fill(_) => "const",
        Expr::Call(name, ..) | Expr::Apply(name, ..) => {
            return name.trim_start_matches('$').to_owned();
        }
    }
    .to_owned()
}

/// The signals an expression reads, in order.
fn reads(e: &Expr, skip: &BTreeSet<String>, out: &mut Vec<String>) {
    match e {
        Expr::Ident(name, _) => {
            if !skip.contains(name) && !out.contains(name) {
                out.push(name.clone());
            }
        }
        Expr::Index(a, b) => {
            reads(a, skip, out);
            reads(b, skip, out);
        }
        Expr::Slice(a, b, c) | Expr::IndexedPart(a, b, c) | Expr::Ternary(a, b, c) => {
            reads(a, skip, out);
            reads(b, skip, out);
            reads(c, skip, out);
        }
        Expr::Binary(_, a, b) => {
            reads(a, skip, out);
            reads(b, skip, out);
        }
        Expr::Unary(_, a) => reads(a, skip, out),
        Expr::Concat(v) | Expr::Call(_, v, _) | Expr::Apply(_, v, _) => {
            for x in v {
                reads(x, skip, out);
            }
        }
        Expr::Repeat(n, v) => {
            reads(n, skip, out);
            for x in v {
                reads(x, skip, out);
            }
        }
        Expr::Num { .. } | Expr::Fill(_) => {}
    }
}

/// The signals an assignment's target writes.
fn writes(lhs: &LValue, skip: &BTreeSet<String>) -> Vec<String> {
    match lhs {
        LValue::Var(n, _) | LValue::Bit(n, ..) | LValue::Part(n, ..) => {
            if skip.contains(n) {
                Vec::new()
            } else {
                vec![n.clone()]
            }
        }
        LValue::Concat(v) => v.iter().flat_map(|l| writes(l, skip)).collect(),
    }
}

/// Signals a target reads to choose its bits: `mem[addr] = …` reads `addr`.
fn lvalue_reads(lhs: &LValue, skip: &BTreeSet<String>, out: &mut Vec<String>) {
    match lhs {
        LValue::Var(..) => {}
        LValue::Bit(_, e, _) => reads(e, skip, out),
        LValue::Part(_, a, b, _) => {
            reads(a, skip, out);
            reads(b, skip, out);
        }
        LValue::Concat(v) => {
            for l in v {
                lvalue_reads(l, skip, out);
            }
        }
    }
}

/// What a procedural block reads and what it writes.
fn statement(stmt: &Stmt, skip: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
    fn walk(s: &Stmt, skip: &BTreeSet<String>, ins: &mut Vec<String>, outs: &mut Vec<String>) {
        match s {
            Stmt::Block(v) => {
                for x in v {
                    walk(x, skip, ins, outs);
                }
            }
            Stmt::Assign { lhs, rhs, .. } => {
                reads(rhs, skip, ins);
                lvalue_reads(lhs, skip, ins);
                for w in writes(lhs, skip) {
                    if !outs.contains(&w) {
                        outs.push(w);
                    }
                }
            }
            Stmt::If(c, a, b) => {
                reads(c, skip, ins);
                walk(a, skip, ins, outs);
                if let Some(b) = b {
                    walk(b, skip, ins, outs);
                }
            }
            Stmt::Case(subject, arms, default, _) => {
                reads(subject, skip, ins);
                for (labels, body) in arms {
                    for l in labels {
                        reads(l, skip, ins);
                    }
                    walk(body, skip, ins, outs);
                }
                if let Some(d) = default {
                    walk(d, skip, ins, outs);
                }
            }
            Stmt::For { var, body, .. } => {
                let mut inner = skip.clone();
                inner.insert(var.clone());
                walk(body, &inner, ins, outs);
            }
            Stmt::Return(e, _) => reads(e, skip, ins),
            Stmt::Null => {}
        }
    }
    let (mut ins, mut outs) = (Vec::new(), Vec::new());
    walk(stmt, skip, &mut ins, &mut outs);
    (ins, outs)
}

/// The module as a sheet whose parts are joined only by their signals' names: one cell
/// per box, and labels on every pin, for the layout engine to wire.
fn to_sheet(rtl: &Rtl) -> Result<(Design, CellId, Schematic), String> {
    let mut lib = Library::new(LIBRARY);
    let mut sheet = Schematic::default();
    let mut used: BTreeSet<String> = BTreeSet::from([rtl.module.clone()]);
    // Far apart, so nothing touches until the layout puts it where it goes.
    let spot = |k: usize| Point::new(Length::from_mm(1000.0 * (k as f64 + 1.0)), Length::ZERO);
    for (k, block) in rtl.blocks.iter().enumerate() {
        let mut name = block.kind.clone();
        let mut n = 1;
        while !used.insert(name.clone()) {
            n += 1;
            name = format!("{}_{n}", block.kind);
        }
        let width = |signal: &str| rtl.widths.get(signal).copied().unwrap_or(1);
        let port = |(pin, signal): &(String, String), kind| Port {
            width: width(signal),
            ..Port::new(pin, kind)
        };
        let mut ports: Vec<Port> = block.inputs.iter().map(|p| port(p, PortKind::Input)).collect();
        ports.extend(block.outputs.iter().map(|p| port(p, PortKind::Output)));
        let cell = Cell {
            description: format!("{} in module {}", block.kind, rtl.module),
            prefix: "U".into(),
            ports,
            ..Cell::default()
        };
        let symbol = builder::generate(&block.kind, &cell);
        let at = spot(k);
        for pin in &symbol.pins {
            let signal = block
                .inputs
                .iter()
                .chain(&block.outputs)
                .find(|(p, _)| *p == pin.port)
                .map(|(_, s)| s.clone())
                .expect("a pin per port");
            sheet.labels.push(NetLabel { net: signal, at: at + pin.at, global: false });
        }
        sheet.instances.push(Instance {
            name: block.name.clone(),
            cell: CellRef::new(Some(LIBRARY), &name),
            placement: Transform::at(at),
            params: BTreeMap::new(),
            view: None,
            fields: BTreeMap::new(),
        });
        lib.insert(CellDef { name, cell, symbol: Some(symbol), ..CellDef::default() });
    }
    let mut ports = Vec::new();
    for (k, (name, dir)) in rtl.ports.iter().enumerate() {
        let kind = match dir {
            Dir::Input => PortKind::Input,
            Dir::Output => PortKind::Output,
            Dir::Inout => PortKind::Bidirectional,
        };
        let width = rtl.widths.get(name).copied().unwrap_or(1);
        ports.push(Port { width, ..Port::new(name, kind) });
        let at = Point::new(Length::ZERO, Length::from_mm(1000.0 * (k as f64 + 1.0)));
        sheet.ports.push(SchematicPort { port: name.clone(), at });
        sheet.labels.push(NetLabel { net: name.clone(), at, global: false });
    }
    let top = Cell { description: format!("module {}", rtl.module), ports, ..Cell::default() };
    lib.insert(CellDef { name: rtl.module.clone(), cell: top, ..CellDef::default() });
    let mut design = Design::new();
    design.add_library(lib).map_err(|e| e.to_string())?;
    Ok((design, CellId::new(LIBRARY, &rtl.module), sheet))
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::connectivity::{Connectivity, Item as NetItem};

    const COUNTER: &str = "
module adder #(parameter W = 8) (input logic [W-1:0] a, b, output logic [W-1:0] y);
    assign y = a + b;
endmodule

module counter (input logic clk, rst, en, input logic [7:0] step, output logic [7:0] count, output logic wrap);
    logic [7:0] next, sum;
    adder #(.W(8)) add (.a(count), .b(step), .y(sum));
    assign next = en ? sum : count;
    assign wrap = count == 8'hff;
    always_ff @(posedge clk or posedge rst)
        if (rst) count <= 0; else count <= next;
endmodule
";

    /// Saves the diagram as a picture when `CIRCUITER_DIAGRAM_SVG` names a folder.
    fn picture(d: &RtlDiagram, name: &str) {
        if let Some(dir) = std::env::var_os("CIRCUITER_DIAGRAM_SVG") {
            let def = d.design.cell(&d.id).unwrap();
            let scene = circuiter_scene::Scene::build_from(&d.design, &d.id, &def.cell, &d.sheet);
            let svg = circuiter_scene::to_svg(&scene, &circuiter_scene::Theme::PAPER);
            std::fs::write(std::path::Path::new(&dir).join(format!("{name}.svg")), svg).unwrap();
        }
    }

    /// Each signal and the pins (`block.pin`) and ports on it, from a sheet.
    fn nets(d: &RtlDiagram) -> BTreeSet<BTreeSet<String>> {
        let conn = Connectivity::analyze(&d.design, LIBRARY, &d.sheet);
        conn.nets
            .iter()
            .map(|net| {
                net.items
                    .iter()
                    .filter_map(|item| match *item {
                        NetItem::Pin(p) => {
                            let pin = &conn.pins[p];
                            Some(format!("{}.{}", d.sheet.instances[pin.instance].name, pin.port))
                        }
                        NetItem::Port(i) => Some(format!("port {}", d.sheet.ports[i].port)),
                        _ => None,
                    })
                    .collect::<BTreeSet<String>>()
            })
            .filter(|s| !s.is_empty())
            .collect()
    }

    #[test]
    fn a_module_becomes_boxes_joined_by_its_signals() {
        let modules = parse::modules(COUNTER).unwrap();
        let rtl = elaborate(&modules, None).unwrap();
        assert_eq!(rtl.module, "counter");
        let kinds: Vec<(&str, &str)> =
            rtl.blocks.iter().map(|b| (b.name.as_str(), b.kind.as_str())).collect();
        assert_eq!(
            kinds,
            [("add", "adder"), ("next", "mux"), ("wrap", "compare"), ("count_reg", "register")]
        );
        let reg = &rtl.blocks[3];
        let ins: Vec<&str> = reg.inputs.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(ins, ["clk", "rst", "next"], "edges first, and not its own output");
        assert_eq!(rtl.blocks[0].outputs, [("y".to_owned(), "sum".to_owned())]);
        assert_eq!(rtl.widths["count"], 8);
        assert_eq!(rtl.widths["en"], 1);
    }

    #[test]
    fn buses_carry_their_widths() {
        let d = diagram(COUNTER, None).unwrap();
        let top = d.design.cell(&d.id).unwrap();
        assert_eq!(top.cell.port("count").unwrap().width, 8);
        assert_eq!(top.cell.port("clk").unwrap().width, 1);
        let scene = circuiter_scene::Scene::build_from(&d.design, &d.id, &top.cell, &d.sheet);
        let bus = |role| scene.elements.iter().filter(|e| e.role == role).count();
        assert!(bus(circuiter_scene::Role::Bus) > 0, "count and sum are drawn as buses");
        assert!(bus(circuiter_scene::Role::Wire) > 0, "clk and en as wires");
        let marks: Vec<&str> = scene
            .elements
            .iter()
            .filter_map(|e| match &e.shape {
                circuiter_scene::Shape::Text { text, .. }
                    if e.role == circuiter_scene::Role::Bus =>
                {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect();
        assert!(marks.iter().all(|m| *m == "8"), "{marks:?}");
    }

    #[test]
    fn the_diagram_wires_each_signal_to_its_pins() {
        let d = diagram(COUNTER, None).unwrap();
        picture(&d, "rtl_counter");
        let want: BTreeSet<BTreeSet<String>> = [
            &["port clk", "count_reg.clk"][..],
            &["port rst", "count_reg.rst"],
            &["port en", "next.en"],
            &["port step", "add.b"],
            &["port count", "add.a", "next.count", "wrap.count", "count_reg.count"],
            &["port wrap", "wrap.wrap"],
            &["add.y", "next.sum"],
            &["next.next", "count_reg.next"],
        ]
        .iter()
        .map(|s| s.iter().map(|x| x.to_string()).collect())
        .collect();
        assert_eq!(nets(&d), want);
        // Wired, not just labelled: the layout drew wires between them.
        assert!(d.sheet.wires.len() > 8);
        let conn = Connectivity::analyze(&d.design, LIBRARY, &d.sheet);
        assert!(conn.passes_over.is_empty());
        assert!(conn.dangling_wire_ends.is_empty());
    }

    #[test]
    fn the_example_control_unit() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/riscv/programs/control.sv"),
        )
        .unwrap();
        let d = diagram(&src, None).unwrap();
        picture(&d, "rtl_control");
        let names: Vec<&str> = d.sheet.instances.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["opcode", "funct3", "f7b5", "comb1"]);
        let n = nets(&d);
        assert!(
            n.contains(&BTreeSet::from(["opcode.opcode".to_owned(), "comb1.opcode".to_owned()]))
        );
        assert!(n.contains(&BTreeSet::from(["port halt".to_owned(), "comb1.halt".to_owned()])));
    }
}
