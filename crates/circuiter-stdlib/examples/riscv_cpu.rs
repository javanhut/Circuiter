//! Writes the example RISC-V processor project: a single-cycle RV32I CPU built from the
//! standard library's digital parts, with a branch unit and a gate-level adder as
//! components of its own; and the same processor with every block made of logic gates
//! (`cpu_gates`, see `common/gate_level.rs`).
//!
//! ```text
//! cargo run -p circuiter-stdlib --example riscv_cpu -- examples/riscv
//! ```
//!
//! Each pin gets a short wire ending in a net label; same-named labels are connected.

use circuiter_model::builder;
use circuiter_model::cell::{Cell, CellDef, Port, PortKind};
use circuiter_model::geom::Point;
use circuiter_model::schematic::{NetLabel, Schematic, SchematicPort, Wire};
use circuiter_model::{Design, Library, Project};
use std::path::PathBuf;

const PROJECT: &str = "riscv";

#[path = "common/sheet.rs"]
mod sheet;
use sheet::{GRID, Sheet};

#[path = "common/gate_level.rs"]
mod gate_level;

/// The cell's ports in a column at the left of its sheet, each on a short wire ending
/// in a label for the net inside: `(port, net)`.
fn io(sh: &mut Sheet<'_>, entries: &[(&str, &str)]) {
    for (k, (name, net)) in entries.iter().enumerate() {
        let y = GRID * (4 * k) as f64;
        let at = Point::mm(-24.0 * GRID, y);
        let end = Point::mm(-18.0 * GRID, y);
        sh.s.wires.push(Wire { points: vec![at, end] });
        sh.s.ports.push(SchematicPort { port: name.to_string(), at });
        sh.s.labels.push(NetLabel { global: false, net: net.to_string(), at: end });
    }
}

fn port(name: &str, kind: PortKind, width: u32, description: &str) -> Port {
    Port { width, description: description.into(), ..Port::new(name, kind) }
}

fn component(
    name: &str,
    description: &str,
    prefix: &str,
    ports: Vec<Port>,
    s: Schematic,
) -> CellDef {
    let cell =
        Cell { description: description.into(), prefix: prefix.into(), ports, ..Cell::default() };
    let symbol = builder::generate(name, &cell);
    CellDef {
        name: name.into(),
        cell,
        symbol: Some(symbol),
        schematic: Some(s),
        ..CellDef::default()
    }
}

fn add(design: &mut Design, def: CellDef) {
    design.library_mut(PROJECT).expect("project library").insert(def);
}

fn full_adder(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let x1 = sh.part("std:xor", "X1", 0, 0, &[]);
    let x2 = sh.part("std:xor", "X2", 18, 0, &[]);
    let a1 = sh.part("std:and", "A1", 0, 12, &[]);
    let a2 = sh.part("std:and", "A2", 18, 12, &[]);
    let o1 = sh.part("std:or", "O1", 36, 12, &[]);
    sh.nets(x1, &[("a", "a"), ("b", "b"), ("y", "p")]);
    sh.nets(x2, &[("a", "p"), ("b", "cin"), ("y", "s")]);
    sh.nets(a1, &[("a", "a"), ("b", "b"), ("y", "g")]);
    sh.nets(a2, &[("a", "p"), ("b", "cin"), ("y", "c")]);
    sh.nets(o1, &[("a", "g"), ("b", "c"), ("y", "cout")]);
    io(&mut sh, &[("a", "a"), ("b", "b"), ("cin", "cin"), ("s", "s"), ("cout", "cout")]);
    component(
        "full_adder",
        "Full adder from gates: s = a xor b xor cin, cout = majority(a, b, cin)",
        "FA",
        vec![
            port("a", PortKind::Input, 1, ""),
            port("b", PortKind::Input, 1, ""),
            port("cin", PortKind::Input, 1, "Carry in"),
            port("s", PortKind::Output, 1, "Sum"),
            port("cout", PortKind::Output, 1, "Carry out"),
        ],
        sh.s,
    )
}

fn adder4(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let mut col = 0;
    for bit in 0..4 {
        let sa = sh.part(
            "std:slice",
            &format!("SA{bit}"),
            col,
            0,
            &[("from", "4"), ("lo", &bit.to_string()), ("width", "1")],
        );
        let sb = sh.part(
            "std:slice",
            &format!("SB{bit}"),
            col,
            10,
            &[("from", "4"), ("lo", &bit.to_string()), ("width", "1")],
        );
        let fa = sh.part("full_adder", &format!("FA{bit}"), col, 22, &[]);
        sh.nets(sa, &[("a", "A"), ("y", &format!("a{bit}"))]);
        sh.nets(sb, &[("a", "B"), ("y", &format!("b{bit}"))]);
        let cin = if bit == 0 { "CIN".to_string() } else { format!("c{bit}") };
        let cout = if bit == 3 { "COUT".to_string() } else { format!("c{}", bit + 1) };
        sh.nets(
            fa,
            &[
                ("a", &format!("a{bit}")),
                ("b", &format!("b{bit}")),
                ("cin", &cin),
                ("s", &format!("s{bit}")),
                ("cout", &cout),
            ],
        );
        col += 24;
    }
    let j1 = sh.part("std:concat", "J1", 0, 40, &[]);
    let j2 = sh.part("std:concat", "J2", 24, 40, &[]);
    let j3 = sh.part("std:concat", "J3", 48, 40, &[("wa", "2"), ("wb", "2")]);
    sh.nets(j1, &[("a", "s3"), ("b", "s2"), ("y", "hi")]);
    sh.nets(j2, &[("a", "s1"), ("b", "s0"), ("y", "lo")]);
    sh.nets(j3, &[("a", "hi"), ("b", "lo"), ("y", "SUM")]);
    io(&mut sh, &[("a", "A"), ("b", "B"), ("cin", "CIN"), ("sum", "SUM"), ("cout", "COUT")]);
    let ports = vec![
        port("a", PortKind::Input, 4, ""),
        port("b", PortKind::Input, 4, ""),
        port("cin", PortKind::Input, 1, "Carry in"),
        port("sum", PortKind::Output, 4, ""),
        port("cout", PortKind::Output, 1, "Carry out"),
    ];
    component("adder4", "4-bit ripple-carry adder from four full adders", "ADD", ports, sh.s)
}

fn adder4_test(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let ka = sh.part("std:const", "KA", 0, 0, &[("width", "4"), ("value", "5")]);
    let kb = sh.part("std:const", "KB", 0, 8, &[("width", "4"), ("value", "6")]);
    let kc = sh.part("std:const", "KC", 0, 16, &[("width", "1"), ("value", "0")]);
    let u = sh.part("adder4", "U1", 24, 0, &[]);
    sh.nets(ka, &[("y", "A")]);
    sh.nets(kb, &[("y", "B")]);
    sh.nets(kc, &[("y", "CIN")]);
    sh.nets(u, &[("a", "A"), ("b", "B"), ("cin", "CIN"), ("sum", "SUM"), ("cout", "COUT")]);
    let mut def = component(
        "adder4_test",
        "5 + 6 through the gate-level adder: Run, and SUM shows 0xb",
        "T",
        vec![],
        sh.s,
    );
    def.symbol = None;
    def
}

fn branch_unit(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let cmp = sh.part("std:compare", "CMP", 0, 0, &[("width", "32")]);
    let n1 = sh.part("std:not", "N1", 28, 0, &[]);
    let n2 = sh.part("std:not", "N2", 28, 8, &[]);
    let n3 = sh.part("std:not", "N3", 28, 16, &[]);
    let k0 = sh.part("std:const", "K0", 28, 26, &[("width", "1"), ("value", "0")]);
    let sel = sh.part("std:mux8", "SEL", 50, 0, &[("width", "1")]);
    let a1 = sh.part("std:and", "A1", 76, 0, &[]);
    let o1 = sh.part("std:or", "O1", 96, 0, &[]);
    sh.nets(cmp, &[("a", "a"), ("b", "b"), ("eq", "eq"), ("lt", "lt"), ("ltu", "ltu")]);
    sh.nets(n1, &[("a", "eq"), ("y", "ne")]);
    sh.nets(n2, &[("a", "lt"), ("y", "ge")]);
    sh.nets(n3, &[("a", "ltu"), ("y", "geu")]);
    sh.nets(k0, &[("y", "never")]);
    sh.nets(
        sel,
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
    sh.nets(a1, &[("a", "branch"), ("b", "cond"), ("y", "taken")]);
    sh.nets(o1, &[("a", "taken"), ("b", "jump"), ("y", "take")]);
    io(
        &mut sh,
        &[
            ("a", "a"),
            ("b", "b"),
            ("funct3", "funct3"),
            ("branch", "branch"),
            ("jump", "jump"),
            ("take", "take"),
        ],
    );
    component(
        "branch_unit",
        "Branch unit: take = (branch and the funct3 condition holds) or jump. \
         BEQ BNE BLT BGE BLTU BGEU, from a comparator, a multiplexer and gates",
        "BU",
        vec![
            port("a", PortKind::Input, 32, "rs1"),
            port("b", PortKind::Input, 32, "rs2"),
            port("funct3", PortKind::Input, 3, "Which comparison"),
            port("branch", PortKind::Input, 1, "A branch instruction"),
            port("jump", PortKind::Input, 1, "JAL: always taken"),
            port("take", PortKind::Output, 1, "Go to pc + imm"),
        ],
        sh.s,
    )
}

fn cpu(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let prog = "programs/fib.s";
    // Column 1: clock, reset, program counter.
    let clk = sh.part("std:clock", "CLK", 0, 0, &[("frequency", "20meg")]);
    let rst = sh.part("std:reset", "RST", 0, 12, &[("length", "60n")]);
    let pc = sh.part("std:pc", "PC", 0, 22, &[]);
    let k4 = sh.part("std:const", "K4", 0, 36, &[("width", "32"), ("value", "4")]);
    let pc4 = sh.part("std:add", "PC4", 0, 46, &[]);
    let pci = sh.part("std:add", "PCIMM", 0, 58, &[]);
    sh.nets(clk, &[("p", "clk"), ("n", "GND")]);
    sh.nets(rst, &[("rst", "rst")]);
    sh.nets(pc, &[("d", "pc_next"), ("clk", "clk"), ("rst", "rst"), ("q", "pc")]);
    sh.nets(k4, &[("y", "four")]);
    sh.nets(pc4, &[("a", "pc"), ("b", "four"), ("y", "pc4")]);
    sh.nets(pci, &[("a", "pc"), ("b", "imm"), ("y", "pc_imm")]);
    // Column 2: fetch and decode.
    let imem = sh.part("std:rom", "IMEM", 30, 0, &[("program", prog)]);
    let rs1 = sh.part("std:slice", "RS1", 30, 10, &[("from", "32"), ("lo", "15"), ("width", "5")]);
    let rs2 = sh.part("std:slice", "RS2", 30, 18, &[("from", "32"), ("lo", "20"), ("width", "5")]);
    let rd = sh.part("std:slice", "RD", 30, 26, &[("from", "32"), ("lo", "7"), ("width", "5")]);
    let f3 = sh.part("std:slice", "F3", 30, 34, &[("from", "32"), ("lo", "12"), ("width", "3")]);
    let imm = sh.part("std:immgen", "IMMGEN", 30, 42, &[]);
    let ctrl = sh.part("std:control", "CTRL", 30, 52, &[]);
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
    // Column 3: registers and the ALU with its input multiplexers.
    let rf = sh.part("std:regfile", "RF", 66, 0, &[]);
    let k0 = sh.part("std:const", "K0", 66, 24, &[("width", "32"), ("value", "0")]);
    let amux = sh.part("std:mux4", "AMUX", 66, 34, &[("width", "32")]);
    let bmux = sh.part("std:mux2", "BMUX", 66, 52, &[("width", "32")]);
    let alu = sh.part("std:alu", "ALU", 66, 64, &[]);
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
    // Column 4: data memory and write-back.
    let dmem = sh.part("std:dmem", "DMEM", 100, 0, &[("program", prog)]);
    let wb = sh.part("std:mux4", "WB", 100, 20, &[("width", "32")]);
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
    // Column 5: where to go next.
    let bu = sh.part("branch_unit", "BU", 132, 0, &[]);
    let npc1 = sh.part("std:mux2", "NPC1", 132, 18, &[("width", "32")]);
    let jt1 = sh.part("std:slice", "JHI", 132, 30, &[("from", "32"), ("lo", "1"), ("width", "31")]);
    let kb = sh.part("std:const", "KB0", 132, 38, &[("width", "1"), ("value", "0")]);
    let jt = sh.part("std:concat", "JT", 132, 46, &[("wa", "31"), ("wb", "1")]);
    let npc2 = sh.part("std:mux2", "NPC2", 132, 56, &[("width", "32")]);
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
        "cpu",
        "Single-cycle RV32I processor: fetch, decode, execute, memory and write-back in one \
         clock cycle. Runs the program in IMEM (set its program parameter) until ECALL",
        "CPU",
        vec![],
        sh.s,
    );
    def.symbol = None;
    def
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "examples/riscv".into()));
    std::fs::create_dir_all(&dir).expect("project directory");
    let mut design = Design::new();
    design.add_library(circuiter_stdlib::library()).expect("std");
    design.add_library(Library::new(PROJECT)).expect("project");
    let fa = full_adder(&design);
    add(&mut design, fa);
    let a4 = adder4(&design);
    add(&mut design, a4);
    let t = adder4_test(&design);
    add(&mut design, t);
    let bu = branch_unit(&design);
    add(&mut design, bu);
    let c = cpu(&design);
    add(&mut design, c);
    let mut folders: Vec<(String, &str)> = [
        ("full_adder", "arithmetic"),
        ("adder4", "arithmetic"),
        ("adder4_test", "arithmetic"),
        ("branch_unit", "cpu"),
        ("cpu", "cpu"),
    ]
    .map(|(n, f)| (n.to_owned(), f))
    .into();
    for (make, folder) in gate_level::cells() {
        let def = make(&design);
        folders.push((def.name.clone(), folder));
        add(&mut design, def);
    }

    let mut library = design.library(PROJECT).expect("project").clone();
    for (name, folder) in folders {
        library.cells.get_mut(&name).expect("made").folder = folder.into();
    }
    let manifest_path = dir.join(circuiter_model::project::MANIFEST_FILE);
    let project = if manifest_path.is_file() {
        let mut p = Project::load(&dir).expect("existing project");
        p.library = library;
        p
    } else {
        let mut p = Project::create(&dir, PROJECT).expect("new project");
        p.manifest.project.top = "cpu".into();
        p.manifest.project.description = "A single-cycle RISC-V (RV32I) processor built from \
            digital parts, running programs in assembly, C or Rust"
            .into();
        p.library = library;
        p
    };
    project.save().expect("saved");
    // A new project starts with an empty `top` cell; this one's top is the CPU.
    let stray = dir.join("cells/top");
    if stray.join("cell.ron").is_file() && project.library.cell("top").is_none() {
        std::fs::remove_dir_all(stray).expect("removing the empty top cell");
    }
    println!("wrote {}", dir.display());
}
