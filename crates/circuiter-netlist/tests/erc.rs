//! The electrical rules check on small circuits built from standard parts.

use circuiter_model::cell::{Cell, CellDef, Port, PortKind};
use circuiter_model::check::Severity;
use circuiter_model::schematic::NetLabel;
use circuiter_model::{CellId, Design, Library};
use circuiter_netlist::erc::{self, Violation};

#[path = "../../circuiter-stdlib/examples/common/sheet.rs"]
mod sheet;
use sheet::Sheet;

const LIB: &str = "test";

fn design() -> Design {
    let mut d = Design::new();
    d.add_library(circuiter_stdlib::library()).unwrap();
    d.add_library(Library::new(LIB)).unwrap();
    d
}

fn add(
    design: &mut Design,
    name: &str,
    cell: Cell,
    s: circuiter_model::schematic::Schematic,
) -> CellId {
    let def = CellDef { name: name.into(), cell, schematic: Some(s), ..CellDef::default() };
    design.library_mut(LIB).unwrap().insert(def);
    CellId::new(LIB, name)
}

fn errors(v: &[Violation]) -> Vec<&str> {
    v.iter().filter(|v| v.severity == Severity::Error).map(|v| v.message.as_str()).collect()
}

fn warnings(v: &[Violation]) -> Vec<&str> {
    v.iter().filter(|v| v.severity == Severity::Warning).map(|v| v.message.as_str()).collect()
}

#[test]
fn outputs_fighting_and_inputs_left_undriven_are_errors() {
    let mut d = design();
    let mut sh = Sheet::new(&d, LIB);
    let v = sh.part("std:vdc", "V1", 0, 0, &[("V", "5")]);
    let a = sh.part("std:not", "U1", 20, 0, &[]);
    let b = sh.part("std:not", "U2", 20, 12, &[]);
    let c = sh.part("std:not", "U3", 40, 0, &[]);
    sh.nets(v, &[("p", "VDD"), ("n", "GND")]);
    // U1 and U2 both drive `x`; U3 reads `x`. U1 and U2's inputs read `floating`.
    sh.nets(a, &[("a", "floating"), ("y", "x")]);
    sh.nets(b, &[("a", "floating"), ("y", "x")]);
    sh.nets(c, &[("a", "x"), ("y", "out")]);
    let s = sh.s;
    let top = add(&mut d, "fight", Cell::default(), s);
    let found = erc::check(&d, &top);
    let errs = errors(&found);
    assert!(
        errs.iter()
            .any(|e| e.contains("U1.y") && e.contains("U2.y") && e.contains("against each other")),
        "{errs:#?}"
    );
    assert!(
        errs.iter().any(|e| e.contains("driven by nothing") && e.contains("floating")),
        "{errs:#?}"
    );
    // VDD is fed by V1, so the gates' supply pins are fine.
    assert!(!errs.iter().any(|e| e.contains("has no supply")), "{errs:#?}");
}

#[test]
fn a_rail_nothing_feeds_needs_a_source_or_a_power_flag() {
    let mut d = design();
    let build = |d: &Design, feed: &str| {
        let mut sh = Sheet::new(d, LIB);
        let u = sh.part("std:lm358", "U1", 20, 0, &[]);
        let r = sh.part("std:rail", "PWR1", 0, 0, &[("net", "+12V")]);
        let g = sh.part("std:gnd", "GND1", 0, 20, &[]);
        let _ = (r, g);
        sh.nets(u, &[("vp", "+12V"), ("vn", "GND"), ("inp", "GND"), ("inn", "o"), ("out", "o")]);
        match feed {
            "source" => {
                let v = sh.part("std:vdc", "V1", 40, 0, &[("V", "12")]);
                sh.nets(v, &[("p", "+12V"), ("n", "GND")]);
            }
            "flag" => {
                let f = sh.part("std:pwr_flag", "FLG1", 40, 0, &[]);
                sh.nets(f, &[("pwr", "+12V")]);
            }
            _ => {}
        }
        sh.s
    };
    let bare = build(&d, "");
    let top = add(&mut d, "bare", Cell::default(), bare);
    let errs: Vec<String> = errors(&erc::check(&d, &top)).into_iter().map(str::to_owned).collect();
    assert!(
        errs.iter()
            .any(|e| e.contains("U1.vp") && e.contains("+12V") && e.contains("has no supply")),
        "{errs:#?}"
    );
    for feed in ["source", "flag"] {
        let s = build(&d, feed);
        let top = add(&mut d, feed, Cell::default(), s);
        let found = erc::check(&d, &top);
        assert!(
            !errors(&found).iter().any(|e| e.contains("has no supply")),
            "{feed}: {:#?}",
            errors(&found)
        );
    }
}

#[test]
fn global_labels_join_sheets_and_lonely_ones_are_reported() {
    let mut d = design();
    // A sub-sheet with a resistor from a global `SENSE` to its port `a`.
    let mut sub = Sheet::new(&d, LIB);
    let r = sub.part("std:resistor", "R1", 0, 0, &[]);
    let (at, _) = sub.pin(r, "a");
    sub.s.labels.push(NetLabel { net: "SENSE".into(), at, global: true });
    let (b, _) = sub.pin(r, "b");
    sub.s.ports.push(circuiter_model::schematic::SchematicPort { port: "a".into(), at: b });
    let cell = Cell { ports: vec![Port::new("a", PortKind::Passive)], ..Cell::default() };
    let mut sub_def =
        CellDef { name: "sub".into(), cell, schematic: Some(sub.s), ..CellDef::default() };
    sub_def.symbol = Some(circuiter_model::builder::generate("sub", &sub_def.cell));
    d.library_mut(LIB).unwrap().insert(sub_def);
    let mut top = Sheet::new(&d, LIB);
    let x = top.part("test:sub", "X1", 0, 0, &[]);
    let r2 = top.part("std:resistor", "R2", 20, 0, &[]);
    let v = top.part("std:vdc", "V1", 40, 0, &[]);
    top.nets(x, &[("a", "GND")]);
    top.nets(v, &[("p", "top_sense"), ("n", "GND")]);
    top.nets(r2, &[("b", "GND"), ("a", "top_sense")]);
    let (at, _) = top.pin(r2, "a");
    top.s.labels.push(NetLabel { net: "SENSE".into(), at, global: true });
    let s = top.s;
    let id = add(&mut d, "top", Cell::default(), s);
    let n = circuiter_netlist::Netlist::build(&d, &id).unwrap();
    let sense = n.net("SENSE").expect("the global label names the net");
    let pins: Vec<String> = n.nets[sense].pins.iter().map(|(e, p)| format!("{e}.{p}")).collect();
    assert!(
        pins.contains(&"X1.R1.a".to_string()) && pins.contains(&"R2.a".to_string()),
        "{pins:?}"
    );
    assert!(!warnings(&erc::check(&d, &id)).iter().any(|w| w.contains("only once")));
    // Take the top's label away: the sub-sheet's is alone.
    let def = d.cell_mut(&id).unwrap();
    def.schematic.as_mut().unwrap().labels.retain(|l| !l.global);
    let w: Vec<String> = warnings(&erc::check(&d, &id)).into_iter().map(str::to_owned).collect();
    assert!(w.iter().any(|w| w.contains("`SENSE` appears only once")), "{w:#?}");
}

#[test]
fn no_connect_flags_quiet_open_pins_and_catch_connected_ones() {
    let mut d = design();
    let mut sh = Sheet::new(&d, LIB);
    let r = sh.part("std:resistor", "R1", 0, 0, &[]);
    let v = sh.part("std:vdc", "V1", 20, 0, &[]);
    sh.nets(v, &[("p", "x"), ("n", "GND")]);
    sh.nets(r, &[("a", "x")]);
    let (open, _) = sh.pin(r, "b");
    let (connected, _) = sh.pin(r, "a");
    let mut base = sh.s;
    base.no_connects = vec![open];
    let id = add(&mut d, "nc", Cell::default(), base.clone());
    let conn = circuiter_model::connectivity::Connectivity::analyze(
        &d,
        LIB,
        d.cell(&id).unwrap().schematic.as_ref().unwrap(),
    );
    assert!(conn.dangling_pins.is_empty(), "the flagged pin isn't dangling");
    assert!(warnings(&erc::check(&d, &id)).is_empty(), "{:#?}", erc::check(&d, &id));
    base.no_connects = vec![open, connected, circuiter_model::geom::Point::mm(50.0, 50.0)];
    let id = add(&mut d, "nc2", Cell::default(), base);
    let w: Vec<String> = warnings(&erc::check(&d, &id)).into_iter().map(str::to_owned).collect();
    assert!(w.iter().any(|w| w.contains("R1.a is flagged not to connect")), "{w:#?}");
    assert!(w.iter().any(|w| w.contains("not on a pin")), "{w:#?}");
}

/// The example circuits pass the check, as a designer would leave them: every cell
/// with a schematic in every example project.
#[test]
fn the_examples_pass() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut checked = 0;
    let mut bad = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let root = entry.unwrap().path();
        let Ok(p) = circuiter_model::Project::load(&root) else { continue };
        let design = Design::for_project(&p, circuiter_stdlib::library()).unwrap();
        let lib = design.library(p.name()).unwrap();
        for (name, def) in &lib.cells {
            if def.schematic.is_none() {
                continue;
            }
            checked += 1;
            let found = erc::check(&design, &CellId::new(p.name(), name));
            for e in errors(&found) {
                bad.push(format!("{}:{name}: {e}", p.name()));
            }
        }
    }
    assert!(checked > 20, "{checked}");
    assert!(bad.is_empty(), "{bad:#?}");
}
