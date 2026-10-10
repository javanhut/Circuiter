//! Standard library parts, wired into schematics, flattened by the netlister and
//! simulated: the whole path a user's design takes.

use circuiter_model::connectivity::placed_pins;
use circuiter_model::geom::{Point, Transform};
use circuiter_model::schematic::{Instance, NetLabel, Schematic};
use circuiter_model::{Cell, CellDef, CellId, Design, Library};
use circuiter_netlist::Netlist;
use circuiter_sim::{Circuit, Options, op, tran};

/// A circuit from `(name, cell, params, nets)`: each part is placed apart from the
/// others and its pins are joined to the named nets by labels. `GND` is ground.
/// A part: instance name, standard cell, parameter overrides, and the net on each
/// port in the cell's port order.
type Part<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], &'a [&'a str]);

fn build(parts: &[Part<'_>]) -> (Netlist, Circuit) {
    let mut design = Design::new();
    design.add_library(circuiter_stdlib::load().unwrap()).unwrap();
    let mut schematic = Schematic::default();
    for (k, (name, cell, params, nets)) in parts.iter().enumerate() {
        let inst = Instance {
            name: (*name).into(),
            cell: format!("std:{cell}").parse().unwrap(),
            placement: Transform::at(Point::mils(2000 * k as i64, 0)),
            params: params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            view: None,
        };
        let def = design.cell(&CellId::new("std", cell)).unwrap();
        let ports: Vec<String> = def.cell.ports.iter().map(|p| p.name.clone()).collect();
        for (at, port) in placed_pins(def, &inst) {
            let i = ports.iter().position(|p| *p == port).unwrap();
            schematic.labels.push(NetLabel { net: nets[i].into(), at });
        }
        schematic.instances.push(inst);
    }
    let mut lib = Library::new("t");
    lib.insert(CellDef {
        name: "top".into(),
        cell: Cell::default(),
        schematic: Some(schematic),
        ..CellDef::default()
    });
    design.add_library(lib).unwrap();
    let netlist = Netlist::build(&design, &CellId::new("t", "top")).unwrap();
    let circuit = Circuit::parse(&netlist.to_spice()).unwrap();
    (netlist, circuit)
}

fn opts() -> Options {
    Options { leakage: 0.0, ..Options::default() }
}

#[test]
fn red_led_drops_about_two_volts_at_twenty_milliamps() {
    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "5")], &["vcc", "GND"]),
        ("R1", "resistor", &[("R", "150")], &["vcc", "a"]),
        ("D1", "led", &[], &["a", "GND"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let vf = r.voltage(&c, "a").unwrap();
    let i = (5.0 - vf) / 150.0;
    assert!((1.9..2.3).contains(&vf), "forward voltage {vf}");
    assert!((0.017..0.022).contains(&i), "current {i}");
}

#[test]
fn transformer_steps_down_by_its_turns_ratio() {
    let (_, c) = build(&[
        ("V1", "vsine", &[("amplitude", "100"), ("frequency", "50")], &["in", "GND"]),
        ("T1", "transformer", &[], &["in", "GND", "out", "GND"]),
        ("RL", "resistor", &[("R", "1k")], &["out", "GND"]),
    ]);
    let r = tran(&c, 60e-3, 20e-6, &opts()).unwrap();
    let v = r.voltage(&c, "out").unwrap();
    let peak = v.iter().skip(v.len() / 2).fold(0.0_f64, |m, x| m.max(x.abs()));
    assert!((9.5..10.1).contains(&peak), "secondary peak {peak} V for a 10:1 ratio");
}

#[test]
fn npn_switch_saturates_and_pnp_sources_current() {
    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "5")], &["vcc", "GND"]),
        ("RB", "resistor", &[("R", "10k")], &["vcc", "b"]),
        ("RC", "resistor", &[("R", "1k")], &["vcc", "c"]),
        ("Q1", "npn", &[], &["c", "b", "GND"]),
        ("RB2", "resistor", &[("R", "10k")], &["b2", "GND"]),
        ("Q2", "pnp", &[], &["c2", "b2", "vcc"]),
        ("RL", "resistor", &[("R", "1k")], &["c2", "GND"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let vce = r.voltage(&c, "c").unwrap();
    assert!(vce < 0.2, "saturated NPN: Vce = {vce}");
    let vc2 = r.voltage(&c, "c2").unwrap();
    assert!(vc2 > 4.7, "saturated PNP pulls its load up: {vc2}");
}

#[test]
fn mosfet_inverter_delay_comes_from_gate_capacitance() {
    // Two inverters in a row with no load capacitor: only device capacitance slows
    // the edges, so the second output must lag the input by tens of picoseconds.
    let parts: Vec<Part<'_>> = vec![
        ("VDD", "vdc", &[("V", "1.8")], &["vdd", "GND"]),
        (
            "VIN",
            "vpulse",
            &[
                ("low", "0"),
                ("high", "1.8"),
                ("delay", "100p"),
                ("rise", "10p"),
                ("fall", "10p"),
                ("width", "1n"),
                ("period", "2n"),
            ],
            &["a", "GND"],
        ),
        ("MP1", "pmos", &[("w", "2u")], &["b", "a", "vdd", "vdd"]),
        ("MN1", "nmos", &[], &["b", "a", "GND", "GND"]),
        ("MP2", "pmos", &[("w", "2u")], &["c", "b", "vdd", "vdd"]),
        ("MN2", "nmos", &[], &["c", "b", "GND", "GND"]),
    ];
    let (_, c) = build(&parts);
    let r = tran(&c, 1e-9, 1e-12, &opts()).unwrap();
    let v = r.voltage(&c, "c").unwrap();
    let crossing = r
        .time
        .iter()
        .zip(&v)
        .find(|(t, x)| **t > 100e-12 && **x > 0.9)
        .map(|(t, _)| *t)
        .expect("output rises");
    let delay = crossing - 105e-12;
    eprintln!("two-stage delay: {:.1} ps", delay * 1e12);
    assert!((5e-12..200e-12).contains(&delay), "two-stage delay {delay:e} s");
}

#[test]
fn every_gate_follows_its_truth_table() {
    type Truth = fn(bool, bool) -> bool;
    let gates: [(&str, Truth); 6] = [
        ("and", |a, b| a && b),
        ("or", |a, b| a || b),
        ("nand", |a, b| !(a && b)),
        ("nor", |a, b| !(a || b)),
        ("xor", |a, b| a != b),
        ("xnor", |a, b| a == b),
    ];
    let level = |x: bool| if x { "5" } else { "0" };
    for (gate, f) in gates {
        for (a, b) in [(false, false), (false, true), (true, false), (true, true)] {
            let (n, c) = build(&[
                ("VS", "vdc", &[("V", "5")], &["VDD", "GND"]),
                ("VA", "vdc", &[("V", level(a))], &["a", "GND"]),
                ("VB", "vdc", &[("V", level(b))], &["b", "GND"]),
                ("U1", gate, &[], &["a", "b", "y"]),
            ]);
            // Only the unloaded output may be remarked on.
            assert!(
                n.diagnostics.iter().all(|d| d.message.contains("only one pin")),
                "{gate}: {:?}",
                n.diagnostics
            );
            let y = op(&c, &opts()).unwrap().voltage(&c, "y").unwrap();
            let want = f(a, b);
            assert!(
                if want { y > 4.95 } else { y < 0.05 },
                "{gate}({a}, {b}) = {y} V, expected {want}"
            );
        }
    }
    for a in [false, true] {
        let (_, c) = build(&[
            ("VS", "vdc", &[("V", "5")], &["VDD", "GND"]),
            ("VA", "vdc", &[("V", level(a))], &["a", "GND"]),
            ("U1", "not", &[], &["a", "y"]),
        ]);
        let y = op(&c, &opts()).unwrap().voltage(&c, "y").unwrap();
        assert!(if a { y < 0.05 } else { y > 4.95 }, "not({a}) = {y}");
    }
}

#[test]
fn a_gate_without_a_supply_is_reported() {
    let (n, _) =
        build(&[("VA", "vdc", &[("V", "5")], &["a", "GND"]), ("U1", "not", &[], &["a", "y"])]);
    assert!(
        n.diagnostics
            .iter()
            .any(|d| d.message.contains("VDD") && d.message.contains("nothing drives it")),
        "{:?}",
        n.diagnostics
    );
}

#[test]
fn gates_switch_in_nanoseconds_like_74hc_logic() {
    // A NAND driving a 15 pF load, as a 74HC00 output into a few inputs and a trace.
    let (_, c) = build(&[
        ("VS", "vdc", &[("V", "5")], &["VDD", "GND"]),
        (
            "VA",
            "vpulse",
            &[
                ("low", "0"),
                ("high", "5"),
                ("delay", "10n"),
                ("rise", "2n"),
                ("fall", "2n"),
                ("width", "50n"),
                ("period", "100n"),
            ],
            &["a", "GND"],
        ),
        ("VB", "vdc", &[("V", "5")], &["b", "GND"]),
        ("U1", "nand", &[], &["a", "b", "y"]),
        ("CL", "capacitor", &[("C", "15p")], &["y", "GND"]),
    ]);
    let r = tran(&c, 100e-9, 0.2e-9, &opts()).unwrap();
    let y = r.voltage(&c, "y").unwrap();
    let fall =
        r.time.iter().zip(&y).find(|(t, v)| **t > 10e-9 && **v < 2.5).map(|(t, _)| *t).unwrap();
    let delay = fall - 11e-9; // input crosses 2.5 V at 11 ns
    eprintln!("NAND into 15 pF: falls in {:.1} ns", delay * 1e9);
    assert!((1e-9..20e-9).contains(&delay), "{delay:e}");
}
