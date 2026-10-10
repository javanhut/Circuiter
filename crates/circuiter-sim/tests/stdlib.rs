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

// ---- The wider library: amplifiers, regulators, timers, semiconductors, sensors and
// electromechanical parts, each in the circuit it is usually found in.

/// Switched on at time zero, as the app's Run does, instead of from the steady state.
fn on() -> Options {
    Options { power_on: true, ..opts() }
}

fn v(r: &circuiter_sim::OpResult, c: &Circuit, node: &str) -> f64 {
    r.voltage(c, node).unwrap_or_else(|| panic!("no node {node}"))
}

/// The supply current drawn from voltage source `name` (positive out of its + pin).
fn supply_current(r: &circuiter_sim::OpResult, name: &str) -> f64 {
    -r.device(name).unwrap_or_else(|| panic!("no {name}")).currents[0]
}

#[test]
fn op_amps_amplify_clip_at_their_rails_and_draw_from_their_supplies() {
    // The µA741 is the published Boyle macromodel, which swings within 0.4 V of the
    // rails (a real one stops nearer 2 V short).
    for (part, rail_gap) in
        [("opamp", 1.5), ("lm358", 1.5), ("tl072", 1.5), ("ne5532", 2.0), ("ua741", 0.4)]
    {
        let amp = |vin: &str| {
            build(&[
                ("VP", "vdc", &[("V", "15")], &["vp", "GND"]),
                ("VN", "vdc", &[("V", "15")], &["GND", "vn"]),
                ("V1", "vdc", &[("V", vin)], &["in", "GND"]),
                ("RI", "resistor", &[("R", "1k")], &["in", "n"]),
                ("RF", "resistor", &[("R", "10k")], &["n", "out"]),
                ("U1", part, &[], &["GND", "n", "out", "vp", "vn"]),
                ("RL", "resistor", &[("R", "2k")], &["out", "GND"]),
            ])
            .1
        };
        // Inverting, gain −10.
        let c = amp("0.5");
        let r = op(&c, &opts()).unwrap();
        let out = v(&r, &c, "out");
        assert!((out + 5.0).abs() < 0.02, "{part}: out {out} for −10 × 0.5 V");
        // Its load current comes from the negative supply (it sinks 2.5 mA + 0.5 mA).
        if part != "ua741" {
            let ivn = supply_current(&r, "VVN").abs();
            assert!(ivn > 2.9e-3, "{part}: V− supplies the sunk current, {ivn}");
        }
        // Clipped short of the rail.
        let c = amp("-3");
        let r = op(&c, &opts()).unwrap();
        let out = v(&r, &c, "out");
        assert!(out > 13.0 - rail_gap && out < 15.0 - rail_gap + 0.6, "{part}: clips at {out}");
    }
}

#[test]
fn an_op_amps_bandwidth_is_its_gain_bandwidth_product() {
    use circuiter_sim::{Spacing, ac};
    let (_, c) = build(&[
        ("VP", "vdc", &[("V", "15")], &["vp", "GND"]),
        ("VN", "vdc", &[("V", "15")], &["GND", "vn"]),
        ("V1", "vsine", &[("amplitude", "0"), ("ac", "1")], &["in", "GND"]),
        ("U1", "opamp", &[], &["in", "n", "out", "vp", "vn"]),
        ("RF", "resistor", &[("R", "99k")], &["out", "n"]),
        ("RG", "resistor", &[("R", "1k")], &["n", "GND"]),
    ]);
    let r = ac(&c, Spacing::Decade(10), 100.0, 1e6, &opts()).unwrap();
    let out = r.voltage(&c, "out").unwrap();
    let at = |f: f64| {
        let k = r.frequency.iter().position(|x| (*x - f).abs() < f * 1e-6).unwrap();
        out[k].abs()
    };
    assert!((at(100.0) - 100.0).abs() < 1.0, "gain 100 at 100 Hz: {}", at(100.0));
    // 1 MHz / 100 = 10 kHz bandwidth: at 100 kHz the gain is down to about 10.
    assert!((8.0..12.0).contains(&at(1e5)), "gain at 100 kHz: {}", at(1e5));
}

#[test]
fn a_comparator_pulls_its_open_collector_low() {
    let cmp = |a: &str, b: &str| {
        let (_, c) = build(&[
            ("V5", "vdc", &[("V", "5")], &["vcc", "GND"]),
            ("VA", "vdc", &[("V", a)], &["a", "GND"]),
            ("VB", "vdc", &[("V", b)], &["b", "GND"]),
            ("U1", "lm393", &[], &["a", "b", "out", "vcc", "GND"]),
            ("RP", "resistor", &[("R", "10k")], &["vcc", "out"]),
        ]);
        let r = op(&c, &opts()).unwrap();
        v(&r, &c, "out")
    };
    assert!(cmp("2", "1") > 4.9, "+ above −: released, pulled up");
    assert!(cmp("1", "1.01") < 0.2, "− above +: pulled low");
}

#[test]
fn regulators_hold_their_outputs() {
    let reg = |part: &str, vin: &str, load: &str| {
        let (_, c) = build(&[
            ("V1", "vdc", &[("V", vin)], &["in", "GND"]),
            ("U1", part, &[], &["in", "GND", "out"]),
            ("RL", "resistor", &[("R", load)], &["out", "GND"]),
        ]);
        let r = op(&c, &opts()).unwrap();
        (v(&r, &c, "out"), supply_current(&r, "VV1"))
    };
    let (out, iin) = reg("lm7805", "12", "100");
    assert!((out - 5.0).abs() < 0.02, "7805: {out}");
    assert!((iin - 0.055).abs() < 0.002, "7805 input current {iin}: load plus quiescent");
    let (out, _) = reg("lm7805", "6", "100");
    assert!((3.8..4.1).contains(&out), "7805 in dropout at 6 V in: {out}");
    let (out, _) = reg("ams1117", "5", "33");
    assert!((out - 3.3).abs() < 0.02, "AMS1117: {out}");
    // Current limit: a short-ish load holds the current near 1.5 A.
    let (out, _) = reg("lm7805", "12", "1");
    assert!(out < 1.6 && out > 1.3, "7805 current-limited into 1 Ω: {out} V");

    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "12")], &["in", "GND"]),
        ("U1", "lm317", &[], &["in", "adj", "out"]),
        ("R1", "resistor", &[("R", "240")], &["out", "adj"]),
        ("R2", "resistor", &[("R", "720")], &["adj", "GND"]),
        ("RL", "resistor", &[("R", "100")], &["out", "GND"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let out = v(&r, &c, "out");
    assert!((out - 5.04).abs() < 0.03, "LM317 with 240/720: {out}");

    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "12")], &["in", "GND"]),
        ("R1", "resistor", &[("R", "1k")], &["in", "k"]),
        ("U1", "tl431", &[], &["k", "GND", "k"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let k = v(&r, &c, "k");
    assert!((k - 2.495).abs() < 0.01, "TL431 as a 2.5 V reference: {k}");
}

#[test]
fn a_555_astable_runs_at_its_formula_frequency() {
    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "9")], &["vcc", "GND"]),
        ("U1", "ne555", &[], &["GND", "t", "out", "vcc", "ctl", "t", "dis", "vcc"]),
        ("R1", "resistor", &[("R", "1k")], &["vcc", "dis"]),
        ("R2", "resistor", &[("R", "10k")], &["dis", "t"]),
        ("C1", "capacitor", &[("C", "100n")], &["t", "GND"]),
        ("C2", "capacitor", &[("C", "10n")], &["ctl", "GND"]),
        ("RL", "resistor", &[("R", "1k")], &["out", "GND"]),
    ]);
    let r = tran(&c, 12e-3, 2e-6, &on()).unwrap();
    let out = r.voltage(&c, "out").unwrap();
    let rises: Vec<f64> = r
        .time
        .windows(2)
        .zip(out.windows(2))
        .filter(|(_, v)| v[0] < 4.0 && v[1] >= 4.0)
        .map(|(t, _)| t[1])
        .filter(|t| *t > 2e-3)
        .collect();
    assert!(rises.len() >= 5, "{} rising edges", rises.len());
    let period = (rises[rises.len() - 1] - rises[0]) / (rises.len() - 1) as f64;
    let expected = 0.693 * (1e3 + 2.0 * 10e3) * 100e-9;
    eprintln!("555 period {:.3} ms, formula {:.3} ms", period * 1e3, expected * 1e3);
    assert!((period / expected - 1.0).abs() < 0.1, "period {period:e}, expected {expected:e}");
    let high = out.iter().copied().fold(f64::MIN, f64::max);
    assert!((6.9..7.5).contains(&high), "output high {high} V, 1.7 V below 9 V");
}

#[test]
fn diodes_zener_schottky_rectifier_and_bridge() {
    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "12")], &["in", "GND"]),
        ("R1", "resistor", &[("R", "1k")], &["in", "z"]),
        ("D1", "zener", &[], &["GND", "z"]),
        ("R2", "resistor", &[("R", "47")], &["in", "s"]),
        ("D2", "schottky", &[], &["s", "GND"]),
        ("R3", "resistor", &[("R", "47")], &["in", "f"]),
        ("D3", "rectifier", &[], &["f", "GND"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let (z, s, f) = (v(&r, &c, "z"), v(&r, &c, "s"), v(&r, &c, "f"));
    assert!((4.9..5.15).contains(&z), "zener holds {z} V");
    assert!((0.25..0.4).contains(&s), "Schottky drop at 250 mA: {s}");
    assert!((0.75..0.95).contains(&f), "rectifier drop at 240 mA: {f}");
    assert!(f - s > 0.35, "a Schottky drops less than a rectifier");

    let (_, c) = build(&[
        ("V1", "vsine", &[("amplitude", "10"), ("frequency", "50")], &["ac1", "ac2"]),
        ("RG", "resistor", &[("R", "1meg")], &["ac2", "GND"]),
        ("B1", "bridge", &[], &["ac1", "ac2", "plus", "minus"]),
        ("C1", "electrolytic", &[("C", "1000u")], &["plus", "minus"]),
        ("RL", "resistor", &[("R", "1k")], &["plus", "minus"]),
    ]);
    let r = tran(&c, 0.2, 50e-6, &opts()).unwrap();
    let p = r.voltage(&c, "plus").unwrap();
    let m = r.voltage(&c, "minus").unwrap();
    let dc = p.last().unwrap() - m.last().unwrap();
    assert!((8.0..8.9).contains(&dc), "bridge and reservoir: {dc} V DC from 10 V peak");
}

#[test]
fn light_sensors_respond_to_light() {
    let (_, c) = build(&[
        ("D1", "photodiode", &[("lux", "1000")], &["a", "k"]),
        ("R1", "resistor", &[("R", "1k")], &["a", "k"]),
        ("RG", "resistor", &[("R", "1")], &["k", "GND"]),
        ("V1", "vdc", &[("V", "5")], &["vcc", "GND"]),
        ("RC", "resistor", &[("R", "10k")], &["vcc", "c"]),
        ("Q1", "phototransistor", &[("lux", "1000")], &["c", "GND"]),
        ("R2", "resistor", &[("R", "10k")], &["vcc", "l"]),
        ("L1", "ldr", &[("lux", "1000")], &["l", "GND"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let vpd = v(&r, &c, "a") - v(&r, &c, "k");
    assert!((0.065..0.071).contains(&vpd), "70 µA photocurrent through 1 kΩ: {vpd}");
    let vc = v(&r, &c, "c");
    assert!((2.5..3.5).contains(&vc), "phototransistor draws about 0.2 mA: {vc}");
    let vl = v(&r, &c, "l");
    let rl = 10e3 * vl / (5.0 - vl);
    assert!((350.0..450.0).contains(&rl), "LDR at 1000 lux: {rl} Ω");

    let (_, c) = build(&[
        ("PV", "solar_cell", &[], &["p", "GND"]),
        ("RL", "resistor", &[("R", "1meg")], &["p", "GND"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let voc = v(&r, &c, "p");
    assert!((20.0..23.0).contains(&voc), "36-cell panel open circuit: {voc}");
}

#[test]
fn temperature_sensors() {
    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "5")], &["vcc", "GND"]),
        ("R1", "resistor", &[("R", "10k")], &["vcc", "t"]),
        ("TH", "thermistor", &[("temp", "50")], &["t", "GND"]),
        ("U1", "lm35", &[("temp", "30")], &["vcc", "o", "GND"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let vt = v(&r, &c, "t");
    let rt = 10e3 * vt / (5.0 - vt);
    assert!((3500.0..3700.0).contains(&rt), "NTC at 50 °C: {rt} Ω");
    let o = v(&r, &c, "o");
    assert!((o - 0.3).abs() < 0.002, "LM35 at 30 °C: {o}");
}

#[test]
fn transistors_power_mosfets_jfet_darlington() {
    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "12")], &["vcc", "GND"]),
        ("VG", "vdc", &[("V", "10")], &["g", "GND"]),
        ("RL", "resistor", &[("R", "1")], &["vcc", "d"]),
        ("Q1", "nmos_power", &[], &["d", "g", "GND"]),
        ("VG2", "vdc", &[("V", "2")], &["g2", "GND"]),
        ("Q2", "pmos_power", &[], &["d2", "g2", "vcc"]),
        ("RL2", "resistor", &[("R", "10")], &["d2", "GND"]),
        ("RB", "resistor", &[("R", "10k")], &["g", "b"]),
        ("RC", "resistor", &[("R", "10")], &["vcc", "c"]),
        ("Q3", "darlington", &[], &["c", "b", "GND"]),
        ("RD", "resistor", &[("R", "2.2k")], &["vcc", "jd"]),
        ("J1", "njf", &[], &["jd", "GND", "GND"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let vds = v(&r, &c, "d");
    assert!(vds < 0.6, "IRF540-like switch on at 10 V gate: Vds {vds} at 11 A");
    let vd2 = v(&r, &c, "d2");
    assert!(vd2 > 11.0, "high-side P-channel on: {vd2}");
    let vce = v(&r, &c, "c");
    assert!((0.6..1.3).contains(&vce), "Darlington saturates at about 1 V: {vce}");
    let idss = (12.0 - v(&r, &c, "jd")) / 2.2e3;
    assert!((3e-3..5.5e-3).contains(&idss), "2N3819 Idss: {idss}");
}

#[test]
fn a_thyristor_latches_and_a_triac_conducts_both_ways() {
    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "12")], &["vcc", "GND"]),
        ("RL", "resistor", &[("R", "100")], &["vcc", "a"]),
        ("S1", "scr", &[], &["a", "GND", "g"]),
        (
            "VG",
            "vpulse",
            &[("low", "0"), ("high", "5"), ("delay", "1m"), ("width", "1m"), ("period", "1")],
            &["vg", "GND"],
        ),
        ("RG", "resistor", &[("R", "1k")], &["vg", "g"]),
    ]);
    let r = tran(&c, 5e-3, 10e-6, &opts()).unwrap();
    let a = r.voltage(&c, "a").unwrap();
    let at = |t: f64| r.sample(&a, t);
    assert!(at(0.5e-3) > 11.0, "blocks before the gate pulse: {}", at(0.5e-3));
    assert!(at(1.8e-3) < 2.0, "on during the pulse: {}", at(1.8e-3));
    assert!(at(4.5e-3) < 2.0, "still on after the gate is released: {}", at(4.5e-3));

    for (vmt, vg) in [("12", "5"), ("-12", "-5")] {
        let (_, c) = build(&[
            ("V1", "vdc", &[("V", vmt)], &["vcc", "GND"]),
            ("RL", "resistor", &[("R", "100")], &["vcc", "mt2"]),
            ("T1", "triac", &[], &["GND", "mt2", "g"]),
            ("VG", "vdc", &[("V", vg)], &["vg", "GND"]),
            ("RG", "resistor", &[("R", "1k")], &["vg", "g"]),
        ]);
        let r = op(&c, &opts()).unwrap();
        let m = v(&r, &c, "mt2");
        assert!(m.abs() < 2.0, "triac on with MT2 {vmt} V, gate {vg} V: {m}");
    }
}

#[test]
fn an_optocoupler_transfers_current_across_its_isolation() {
    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "5")], &["vcc", "GND"]),
        ("RF", "resistor", &[("R", "1k")], &["vcc", "a"]),
        ("U1", "optocoupler", &[], &["a", "GND", "c", "e"]),
        ("V2", "vdc", &[("V", "12")], &["v2", "e"]),
        ("RE", "resistor", &[("R", "1meg")], &["e", "GND"]),
        ("RC", "resistor", &[("R", "10k")], &["v2", "c"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let vf = v(&r, &c, "a");
    assert!((1.1..1.35).contains(&vf), "IR LED forward voltage {vf}");
    let vce = v(&r, &c, "c") - v(&r, &c, "e");
    assert!(vce < 0.4, "3.8 mA through the LED saturates the transistor: {vce}");
}

#[test]
fn a_relay_switches_its_contacts() {
    let relay = |coil: &str| {
        let (_, c) = build(&[
            ("V1", "vdc", &[("V", coil)], &["coil", "GND"]),
            ("K1", "relay", &[], &["coil", "GND", "com", "no", "nc"]),
            ("V2", "vdc", &[("V", "12")], &["com", "GND"]),
            ("R1", "resistor", &[("R", "1k")], &["no", "GND"]),
            ("R2", "resistor", &[("R", "1k")], &["nc", "GND"]),
        ]);
        let r = op(&c, &opts()).unwrap();
        (v(&r, &c, "no"), v(&r, &c, "nc"))
    };
    let (no, nc) = relay("0");
    assert!(no < 0.1 && nc > 11.9, "at rest: no {no}, nc {nc}");
    let (no, nc) = relay("5");
    assert!(no > 11.9 && nc < 0.1, "energised: no {no}, nc {nc}");
}

#[test]
fn a_motor_spins_up_and_its_back_emf_cuts_the_current() {
    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "12")], &["vcc", "GND"]),
        ("M1", "dc_motor", &[], &["vcc", "GND"]),
    ]);
    let r = tran(&c, 0.2, 100e-6, &on()).unwrap();
    let i = r.current(&c, "VM1s").expect("motor current");
    let peak = i.iter().copied().fold(0.0_f64, f64::max);
    let last = *i.last().unwrap();
    assert!(peak > 4.0, "starting current {peak}");
    assert!((0.05..0.3).contains(&last), "running current {last}");
}

#[test]
fn a_lamp_draws_a_surge_then_its_rated_current() {
    let (_, c) = build(&[
        ("V1", "vdc", &[("V", "12")], &["vcc", "GND"]),
        ("L1", "lamp", &[], &["vcc", "GND"]),
    ]);
    let r = tran(&c, 0.5, 100e-6, &on()).unwrap();
    let i: Vec<f64> = r.current(&c, "VV1").unwrap().iter().map(|x| -x).collect();
    let peak = i.iter().copied().fold(0.0_f64, f64::max);
    let last = *i.last().unwrap();
    assert!((last - 5.0 / 12.0).abs() < 0.02, "hot: {last} A for 5 W at 12 V");
    assert!(peak > 5.0 * last, "cold surge {peak} A");
}

#[test]
fn passives_sources_and_controlled_sources() {
    let (_, c) = build(&[
        ("B1", "battery", &[], &["bp", "GND"]),
        ("RL", "resistor", &[("R", "1")], &["bp", "GND"]),
        ("V1", "vdc", &[("V", "0.1")], &["x", "GND"]),
        ("E1", "vcvs", &[], &["e", "GND", "x", "GND"]),
        ("RE", "resistor", &[("R", "1k")], &["e", "GND"]),
        ("V2", "vdc", &[("V", "1")], &["y", "GND"]),
        ("RS", "resistor", &[("R", "1k")], &["y", "s"]),
        ("H1", "ccvs", &[], &["h", "GND", "s", "GND"]),
        ("RH", "resistor", &[("R", "1k")], &["h", "GND"]),
        ("V3", "vdc", &[("V", "5")], &["w", "GND"]),
        ("SW", "switch_spdt", &[("thrown", "true")], &["w", "sa", "sb"]),
        ("RA", "resistor", &[("R", "1k")], &["sa", "GND"]),
        ("RB", "resistor", &[("R", "1k")], &["sb", "GND"]),
        ("V4", "vdc", &[("V", "400")], &["hv", "GND"]),
        ("RV", "resistor", &[("R", "100k")], &["hv", "mov"]),
        ("MOV", "varistor", &[], &["mov", "GND"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    assert!((v(&r, &c, "bp") - 4.5).abs() < 1e-3, "9 V battery with 1 Ω inside into 1 Ω");
    assert!((v(&r, &c, "e") - 1.0).abs() < 1e-6, "VCVS gain 10");
    assert!((v(&r, &c, "h") - 1.0).abs() < 1e-6, "CCVS: 1 mA × 1 kΩ");
    assert!(v(&r, &c, "sb") > 4.9 && v(&r, &c, "sa") < 0.01, "SPDT thrown to b");
    assert!(v(&r, &c, "mov") > 399.0, "a 470 V varistor barely conducts at 400 V");

    let (_, c) = build(&[
        ("V4", "vdc", &[("V", "600")], &["hv", "GND"]),
        ("RV", "resistor", &[("R", "100")], &["hv", "mov"]),
        ("MOV", "varistor", &[], &["mov", "GND"]),
    ]);
    let r = op(&c, &opts()).unwrap();
    let clamp = v(&r, &c, "mov");
    // 0.3 A through a 470 V part: about 570 V, as a datasheet's clamping curve shows.
    assert!((540.0..600.0).contains(&clamp), "varistor clamps a 600 V surge at {clamp} V");
}

#[test]
fn a_crystal_resonates_and_a_centre_tapped_transformer_splits() {
    use circuiter_sim::{Spacing, ac};
    let (_, c) = build(&[
        ("V1", "vsine", &[("amplitude", "0"), ("ac", "1")], &["in", "GND"]),
        ("R1", "resistor", &[("R", "1k")], &["in", "x"]),
        ("Y1", "crystal", &[], &["x", "GND"]),
    ]);
    let r = ac(&c, Spacing::Linear(201), 15.99e6, 16.01e6, &opts()).unwrap();
    let x = r.voltage(&c, "x").unwrap();
    let (k, min) = x
        .iter()
        .map(|z| z.abs())
        .enumerate()
        .fold((0, f64::MAX), |(bk, bm), (k, m)| if m < bm { (k, m) } else { (bk, bm) });
    let f = r.frequency[k];
    assert!((f - 16e6).abs() < 2e3, "series resonance at {f}");
    assert!(min < 0.05, "only the motional resistance at resonance: {min}");

    let (_, c) = build(&[
        ("V1", "vsine", &[("amplitude", "100"), ("frequency", "50")], &["in", "GND"]),
        ("T1", "transformer_ct", &[], &["in", "GND", "s1", "GND", "s2"]),
        ("R1", "resistor", &[("R", "1k")], &["s1", "GND"]),
        ("R2", "resistor", &[("R", "1k")], &["s2", "GND"]),
    ]);
    let r = tran(&c, 60e-3, 20e-6, &opts()).unwrap();
    let peak = |n: &str| {
        let x = r.voltage(&c, n).unwrap();
        x.iter().skip(x.len() / 2).fold(0.0_f64, |m, y| m.max(y.abs()))
    };
    let (a, b) = (peak("s1"), peak("s2"));
    assert!((4.7..5.1).contains(&a) && (4.7..5.1).contains(&b), "each half 5 V peak: {a}, {b}");
}

#[test]
fn a_function_generator_makes_squares_and_triangles() {
    for (wave, check) in [("square", 0.99), ("triangle", 0.5)] {
        let (_, c) = build(&[
            ("FG", "function_generator", &[("wave", wave), ("amplitude", "2")], &["out", "GND"]),
            ("RL", "resistor", &[("R", "1meg")], &["out", "GND"]),
        ]);
        let r = tran(&c, 3e-3, 1e-6, &opts()).unwrap();
        let o = r.voltage(&c, "out").unwrap();
        let hi = o.iter().copied().fold(f64::MIN, f64::max);
        let lo = o.iter().copied().fold(f64::MAX, f64::min);
        assert!((hi - 2.0).abs() < 0.01 && (lo + 2.0).abs() < 0.01, "{wave}: {lo}..{hi}");
        // The share of time spent above half the amplitude tells the shapes apart.
        let above = o.iter().filter(|x| x.abs() > 1.0).count() as f64 / o.len() as f64;
        assert!((above - check).abs() < 0.12, "{wave}: {above} of the time beyond half");
    }
}
