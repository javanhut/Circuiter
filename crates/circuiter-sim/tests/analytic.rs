//! Results checked against closed-form solutions. These need nothing installed.

use circuiter_sim::{Circuit, Options, Problem, op, tran};

fn opts() -> Options {
    Options { leakage: 0.0, ..Options::default() }
}

#[test]
fn divider_and_current_source() {
    let c = Circuit::parse("V1 in 0 DC 12\nR1 in mid 3k\nR2 mid 0 1k\nI1 0 x DC 2m\nR3 x 0 500")
        .unwrap();
    let r = op(&c, &opts()).unwrap();
    assert!((r.voltage(&c, "mid").unwrap() - 3.0).abs() < 1e-9);
    // SPICE: current flows from + through the source to -, so into node x.
    assert!((r.voltage(&c, "x").unwrap() - 1.0).abs() < 1e-9);
    let v1 = r.device("V1").unwrap();
    assert!((v1.power + 12.0 * 3e-3).abs() < 1e-9, "the source delivers 36 mW");
    let r1 = r.device("R1").unwrap();
    assert!((r1.power - 27e-3).abs() < 1e-9 && (r1.currents[0] - 3e-3).abs() < 1e-12);
}

#[test]
fn diode_solution_satisfies_shockley() {
    let c = Circuit::parse("V1 in 0 DC 5\nR1 in a 1k\nD1 a 0 dx\n.model dx D(IS=1e-14)").unwrap();
    let r = op(&c, &opts()).unwrap();
    let vd = r.voltage(&c, "a").unwrap();
    let id = (5.0 - vd) / 1e3;
    let shockley = 1e-14 * ((vd / circuiter_sim::device::VT).exp() - 1.0);
    assert!((id - shockley).abs() / id < 1e-3, "{id} vs {shockley}");
}

#[test]
fn rc_charges_exponentially() {
    let c = Circuit::parse("V1 in 0 PULSE(0 1 0 1p 1p 1 2)\nR1 in out 1k\nC1 out 0 1u").unwrap();
    let r = tran(&c, 5e-3, 10e-6, &opts()).unwrap();
    let v = r.voltage(&c, "out").unwrap();
    for t in [0.5e-3, 1e-3, 2e-3, 4e-3] {
        let expected = 1.0 - (-t / 1e-3_f64).exp();
        let got = r.sample(&v, t);
        assert!((got - expected).abs() < 2e-3, "t = {t}: {got} vs {expected}");
    }
}

#[test]
fn lc_tank_rings_at_its_resonant_frequency() {
    // 1 mA through the inductor at t = 0 (from the DC operating point), then the
    // source is switched off and the tank rings: f = 1 / (2π √(LC)).
    let c = Circuit::parse("I1 0 a PULSE(1m 0 0 1n 1n 1 2)\nL1 a 0 1m\nC1 a 0 1u\nR1 a 0 1meg")
        .unwrap();
    let r = tran(&c, 1e-3, 1e-6, &opts()).unwrap();
    let v = r.voltage(&c, "a").unwrap();
    let crossings: Vec<f64> = r
        .time
        .windows(2)
        .zip(v.windows(2))
        .filter(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
        .map(|(t, w)| t[0] + (t[1] - t[0]) * (-w[0] / (w[1] - w[0])))
        .collect();
    let period = (crossings.last().unwrap() - crossings[0]) / (crossings.len() - 1) as f64;
    let expected = std::f64::consts::TAU * (1e-3_f64 * 1e-6).sqrt();
    assert!((period - expected).abs() / expected < 2e-3, "{period} vs {expected}");
    // Peak voltage: I √(L/C) = 31.6 mV, nearly undamped.
    let peak = v.iter().fold(0.0_f64, |m, x| m.max(x.abs()));
    assert!((peak - 0.0316).abs() / 0.0316 < 0.02, "{peak}");
}

#[test]
fn a_missing_ground_is_diagnosed_and_still_solves_with_leakage() {
    let deck = "V1 a b DC 5\nR1 a b 1k";
    let c = Circuit::parse(deck).unwrap();
    assert!(
        matches!(&c.topology_problems()[..], [Problem::Floating { nodes }] if nodes.len() == 2)
    );
    assert!(op(&c, &opts()).is_err(), "singular without leakage");
    let r = op(&c, &Options::default()).unwrap();
    let (a, b) = (r.voltage(&c, "a").unwrap(), r.voltage(&c, "b").unwrap());
    assert!((a - b - 5.0).abs() < 1e-9, "the source still sets the difference");
}

#[test]
fn parallel_voltage_sources_are_named() {
    let c = Circuit::parse("V1 a 0 DC 5\nV2 a 0 DC 3\nR1 a 0 1k").unwrap();
    assert_eq!(c.topology_problems(), [Problem::SourceLoop { device: "V2".into() }]);
    let e = op(&c, &Options::default()).unwrap_err();
    assert!(e.message.contains("V2"), "{}", e.message);
}

#[test]
fn hard_operating_points_converge() {
    // A latch: two cross-coupled inverters have three operating points; any is fine,
    // but the solver must find one.
    let deck = "\
VDD vdd 0 DC 1.8
MP1 q qb vdd vdd pch W=2u L=180n
MN1 q qb 0 0 nch W=1u L=180n
MP2 qb q vdd vdd pch W=2u L=180n
MN2 qb q 0 0 nch W=1u L=180n
.model nch NMOS(LEVEL=1 VTO=0.7 KP=110u LAMBDA=0.04)
.model pch PMOS(LEVEL=1 VTO=-0.7 KP=50u LAMBDA=0.04)";
    let c = Circuit::parse(deck).unwrap();
    let r = op(&c, &Options::default()).unwrap();
    let (q, qb) = (r.voltage(&c, "q").unwrap(), r.voltage(&c, "qb").unwrap());
    assert!((0.0..=1.8).contains(&q) && (0.0..=1.8).contains(&qb));
}

#[test]
fn power_on_starts_with_everything_off() {
    // A DC source charging a capacitor: from the operating point nothing happens (the
    // capacitor is already charged); from power-on it charges with τ = RC.
    let c = Circuit::parse("V1 a 0 DC 5\nR1 a b 1k\nC1 b 0 1u").unwrap();
    let steady = tran(&c, 5e-3, 10e-6, &opts()).unwrap();
    let v = steady.voltage(&c, "b").unwrap();
    assert!((v[0] - 5.0).abs() < 1e-6 && (v.last().unwrap() - 5.0).abs() < 1e-6);

    let on = tran(&c, 5e-3, 10e-6, &Options { power_on: true, ..opts() }).unwrap();
    let v = on.voltage(&c, "b").unwrap();
    assert_eq!(v[0], 0.0);
    for t in [0.5e-3, 1e-3, 3e-3] {
        let expected = 5.0 * (1.0 - (-t / 1e-3_f64).exp());
        let got = on.sample(&v, t);
        assert!((got - expected).abs() < 0.02, "t = {t}: {got} vs {expected}");
    }
}
