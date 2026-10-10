use circuiter_model::check::Severity;
use circuiter_model::geom::{Point, Transform};
use circuiter_model::schematic::{Instance, Schematic, Wire};
use circuiter_model::{CellId, Design, Project};
use circuiter_netlist::{Netlist, spice_number};
use std::collections::BTreeMap;
use std::path::Path;

fn example() -> (Project, Design) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cmos_inverter");
    let project = Project::load(&root).unwrap();
    let design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
    (project, design)
}

fn testbench() -> CellId {
    CellId::new("cmos_inverter", "testbench")
}

fn pins(n: &Netlist, net: &str) -> Vec<String> {
    let i = n.net(net).unwrap_or_else(|| panic!("no net {net}"));
    let mut p: Vec<String> = n.nets[i].pins.iter().map(|(e, port)| format!("{e}.{port}")).collect();
    p.sort();
    p
}

#[test]
fn flattens_the_inverter_into_the_testbench() {
    let (_, design) = example();
    let n = Netlist::build(&design, &testbench()).unwrap();
    assert!(n.diagnostics.is_empty(), "{:?}", n.diagnostics);
    assert_eq!(n.nets[0].node, "0");
    assert_eq!(pins(&n, "OUT"), ["CL.a", "U1.MN.d", "U1.MP.d"]);
    assert_eq!(pins(&n, "IN"), ["U1.MN.g", "U1.MP.g", "VCLK.p"]);
    // The inverter's vss port joins ground through the testbench wiring.
    assert!(pins(&n, "GND").contains(&"U1.MN.s".to_owned()));
    let deck = n.to_spice();
    assert!(deck.contains("MU1.MN OUT IN 0 0 nmos_1 W=1e-6 L=1.8e-7"), "{deck}");
    assert!(deck.contains("CCL OUT 0 1e-14"), "{deck}");
    assert!(deck.contains(".model nmos_1 NMOS(LEVEL=1 VTO=0.7 KP=1.1e-4"), "{deck}");
    // Each sheet maps its local nets onto flat ones, for annotating what's on screen.
    assert_eq!(n.sheets.keys().collect::<Vec<_>>(), ["", "U1"]);
}

#[test]
fn identical_models_are_written_once() {
    let (_, mut design) = example();
    let lib = "cmos_inverter";
    let tb = design.cell_mut(&testbench()).unwrap();
    let s = tb.schematic.as_mut().unwrap();
    let mut second = s.instances.iter().find(|i| i.name == "U1").unwrap().clone();
    second.name = "U2".into();
    second.placement = Transform::at(Point::mm(200.0, 200.0));
    s.instances.push(second);
    let n = Netlist::build(&design, &CellId::new(lib, "testbench")).unwrap();
    assert_eq!(n.models.len(), 2, "one PMOS and one NMOS card: {:?}", n.models);
    // U2 is wired to nothing, so its nets are its own, named inside it.
    assert!(n.net("U2.in").is_some(), "{:?}", n.nets.iter().map(|x| &x.name).collect::<Vec<_>>());
}

#[test]
fn reports_a_missing_ground() {
    let (_, mut design) = example();
    let tb = design.cell_mut(&testbench()).unwrap();
    let s = tb.schematic.as_mut().unwrap();
    s.instances.retain(|i| !i.cell.cell.starts_with("gnd"));
    let n = Netlist::build(&design, &testbench()).unwrap();
    assert!(
        n.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning && d.message.contains("no ground")),
        "{:?}",
        n.diagnostics
    );
}

#[test]
fn single_pin_nets_are_warned_about() {
    let (_, mut design) = example();
    let tb = design.cell_mut(&testbench()).unwrap();
    *tb.schematic.as_mut().unwrap() = Schematic {
        instances: vec![
            Instance {
                fields: Default::default(),
                name: "R1".into(),
                cell: "std:resistor".parse().unwrap(),
                placement: Transform::default(),
                params: BTreeMap::from([("R".into(), "4k7".into())]),
                view: None,
            },
            Instance {
                fields: Default::default(),
                name: "G".into(),
                cell: "std:gnd".parse().unwrap(),
                placement: Transform::at(Point::mm(0.0, 20.0)),
                params: BTreeMap::new(),
                view: None,
            },
        ],
        wires: vec![Wire { points: vec![Point::mm(0.0, 5.08), Point::mm(0.0, 20.0)] }],
        ..Schematic::default()
    };
    let n = Netlist::build(&design, &testbench()).unwrap();
    assert!(n.to_spice().contains("RR1 "), "{}", n.to_spice());
    assert!(n.to_spice().contains(" 4700"), "{}", n.to_spice());
    assert!(
        n.diagnostics.iter().any(|d| d.message.contains("only one pin")),
        "{:?}",
        n.diagnostics
    );
}

#[test]
fn numbers_are_plain_and_short() {
    assert_eq!(spice_number(180e-9), "1.8e-7");
    assert_eq!(spice_number(1.8), "1.8");
    assert_eq!(spice_number(4700.0), "4700");
    assert_eq!(spice_number(1e6), "1e6");
    assert_eq!(spice_number(0.0), "0");
    assert_eq!(spice_number(-0.7), "-0.7");
}
