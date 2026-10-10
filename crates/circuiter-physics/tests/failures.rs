//! Parts that are overstressed fail when and as a real one would.

use circuiter_model::cell::{Effect, FailureMode};
use circuiter_model::connectivity::placed_pins;
use circuiter_model::geom::{Point, Transform};
use circuiter_model::schematic::{Instance, NetLabel, Schematic};
use circuiter_model::{Cell, CellDef, CellId, Design, Library};
use circuiter_netlist::Netlist;
use circuiter_physics::{AMBIENT, Monitor};
use circuiter_sim::{Circuit, Options, op, tran_observed};

/// Instance name, standard cell, parameter overrides, net on each port.
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

#[test]
fn an_overloaded_resistor_burns_open_when_its_heat_says() {
    // 12 V across 100 Ω is 1.44 W in a quarter-watt resistor.
    let (netlist, c) = build(&[
        ("V1", "vdc", &[("V", "12")], &["a", "GND"]),
        ("R1", "resistor", &[("R", "100")], &["a", "GND"]),
    ]);
    let mut m = Monitor::new(&netlist, &c, AMBIENT).unwrap();

    // The operating point predicts it.
    let at_op = m.assess(&c, &op(&c, &Options::default()).unwrap());
    assert_eq!(at_op.len(), 1);
    let a = &at_op[0];
    assert!((a.power - 1.44).abs() < 1e-6);
    assert!(a.reason.contains(&format!("{:.0} °C", 25.0 + 1.44 * 520.0)), "{}", a.reason);
    // τ = 520 °C/W × 0.01 J/°C = 5.2 s; 250 °C is reached at −τ ln(1 − 225/748.8).
    let expected = -5.2 * (1.0 - 225.0 / 748.8_f64).ln();
    assert!((a.time_to_failure.unwrap() - expected).abs() < 1e-6);

    // The transient run makes it happen, then carries on with the resistor open.
    let (r, _) = tran_observed(&c, 3.0, 10e-3, &Options::default(), &mut m).unwrap();
    assert_eq!(m.events.len(), 1, "{:?}", m.events);
    let e = &m.events[0];
    assert_eq!((e.path.as_str(), &e.mode, e.effect), ("R1", &FailureMode::Open, Effect::Smoke));
    assert!((e.time - expected).abs() < 0.02, "burnt at {} s, expected {expected} s", e.time);
    let i = r.current(&c, "VV1").unwrap();
    assert!(i.last().unwrap().abs() < 1e-9, "no current once open: {}", i.last().unwrap());
    let part = m.part("R1").unwrap();
    assert!(
        part.peak >= 250.0 && part.temperature < part.peak,
        "cools once open: {}",
        part.temperature
    );
    eprintln!("{}", e.describe());
}

#[test]
fn a_fuse_blows_after_its_melting_energy() {
    // 5 V into 1 Ω through a 1 A fuse: about 5 A, so (25 − 1) A² × t = 0.2 A²s.
    let (netlist, c) = build(&[
        ("V1", "vdc", &[("V", "5")], &["a", "GND"]),
        ("F1", "fuse", &[], &["a", "b"]),
        ("RL", "resistor", &[("R", "1"), ("P", "100")], &["b", "GND"]),
    ]);
    let mut m = Monitor::new(&netlist, &c, AMBIENT).unwrap();
    tran_observed(&c, 20e-3, 50e-6, &Options::default(), &mut m).unwrap();
    assert_eq!(m.events.len(), 1, "{:?}", m.events);
    let i = 5.0 / 1.1;
    let expected = 0.2 / (i * i - 1.0);
    let e = &m.events[0];
    assert_eq!((e.path.as_str(), e.effect), ("F1", Effect::Flash));
    assert!((e.time - expected).abs() < 1e-4, "{} vs {expected}", e.time);
}

#[test]
fn an_led_without_a_resistor_dies_at_once_and_with_one_lives() {
    let (netlist, c) =
        build(&[("V1", "vdc", &[("V", "5")], &["a", "GND"]), ("D1", "led", &[], &["a", "GND"])]);
    let mut m = Monitor::new(&netlist, &c, AMBIENT).unwrap();
    tran_observed(&c, 1e-3, 10e-6, &Options::default(), &mut m).unwrap();
    assert_eq!(m.events.len(), 1);
    assert_eq!(m.events[0].time, 0.0, "already over its limit at power-on");
    assert!(m.events[0].cause.contains("beyond its 150mA rating"), "{}", m.events[0].cause);

    let (netlist, c) = build(&[
        ("V1", "vdc", &[("V", "5")], &["a", "GND"]),
        ("R1", "resistor", &[("R", "150")], &["a", "k"]),
        ("D1", "led", &[], &["k", "GND"]),
    ]);
    let mut m = Monitor::new(&netlist, &c, AMBIENT).unwrap();
    tran_observed(&c, 0.1, 1e-3, &Options::default(), &mut m).unwrap();
    assert!(m.events.is_empty(), "{:?}", m.events);
    let led = m.part("D1").unwrap();
    assert!(
        led.temperature > 40.0 && led.temperature < 60.0,
        "warm, not hot: {} °C",
        led.temperature
    );
}

#[test]
fn an_overvolted_capacitor_cracks_shorted() {
    let (netlist, c) = build(&[
        ("V1", "vdc", &[("V", "24")], &["a", "GND"]),
        ("R1", "resistor", &[("R", "1k"), ("P", "10")], &["a", "b"]),
        ("C1", "capacitor", &[("Vmax", "16")], &["b", "GND"]),
    ]);
    let mut m = Monitor::new(&netlist, &c, AMBIENT).unwrap();
    let (r, _) = tran_observed(&c, 1e-3, 10e-6, &Options::default(), &mut m).unwrap();
    assert_eq!(m.events.len(), 1);
    assert_eq!(m.events[0].mode, FailureMode::Short("a".into(), "b".into()));
    let vb = r.voltage(&c, "b").unwrap();
    assert!(vb.last().unwrap().abs() < 0.5, "shorted: {}", vb.last().unwrap());
}
