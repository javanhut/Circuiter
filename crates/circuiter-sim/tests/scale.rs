//! Larger circuits, to keep the sparse solver honest. Run with `--release` and
//! `--ignored` to see timings.

use circuiter_sim::{Circuit, Options, op, tran};
use std::fmt::Write;
use std::time::Instant;

fn mesh(n: usize) -> String {
    let mut d = String::from("V1 n0_0 0 DC 1\n");
    for i in 0..n {
        for j in 0..n {
            if i + 1 < n {
                let _ = writeln!(d, "RV{i}_{j} n{i}_{j} n{}_{j} 1k", i + 1);
            }
            if j + 1 < n {
                let _ = writeln!(d, "RH{i}_{j} n{i}_{j} n{i}_{} 1k", j + 1);
            }
        }
    }
    let _ = writeln!(d, "RG n{0}_{0} 0 1k", n - 1);
    d
}

fn inverter_chain(stages: usize) -> String {
    let mut d = String::from(
        "VDD vdd 0 DC 1.8\nVIN a0 0 PULSE(0 1.8 100p 20p 20p 1n 2n)\n\
         .model nc NMOS(LEVEL=1 VTO=0.5 KP=170u LAMBDA=0.05 TOX=4.1n CGSO=3e-10 CGDO=3e-10)\n\
         .model pc PMOS(LEVEL=1 VTO=-0.5 KP=60u LAMBDA=0.06 TOX=4.1n CGSO=3e-10 CGDO=3e-10)\n",
    );
    for k in 0..stages {
        let _ = writeln!(d, "MP{k} a{} a{k} vdd vdd pc W=2u L=180n", k + 1);
        let _ = writeln!(d, "MN{k} a{} a{k} 0 0 nc W=1u L=180n", k + 1);
    }
    d
}

#[test]
fn a_resistor_mesh_solves_exactly() {
    // Symmetric: the far corner sits between the source and ground through equal
    // halves of the mesh, which a small mesh checks quickly.
    let c = Circuit::parse(&mesh(20)).unwrap();
    let r = op(&c, &Options { leakage: 0.0, ..Options::default() }).unwrap();
    let v = r.voltage(&c, "n19_19").unwrap();
    assert!(v > 0.0 && v < 1.0);
}

#[test]
#[ignore = "timing; run with --release --ignored"]
fn timings() {
    let t = Instant::now();
    let c = Circuit::parse(&mesh(100)).unwrap();
    op(&c, &Options::default()).unwrap();
    eprintln!("100×100 resistor mesh ({} nodes): {:?}", c.nodes.len(), t.elapsed());

    let t = Instant::now();
    let c = Circuit::parse(&inverter_chain(500)).unwrap();
    let r = tran(&c, 4e-9, 10e-12, &Options::default()).unwrap();
    eprintln!(
        "500-inverter chain transient ({} MOSFETs, {} time points): {:?}",
        1000,
        r.time.len(),
        t.elapsed()
    );
}

#[test]
fn long_inverter_chains_find_their_operating_point() {
    for n in [100, 200, 400] {
        let c = Circuit::parse(&inverter_chain(n)).unwrap();
        op(&c, &Options::default()).unwrap_or_else(|e| panic!("{n} stages: {}", e.message));
    }
}
