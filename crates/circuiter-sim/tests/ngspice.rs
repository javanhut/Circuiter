//! Results compared with ngspice on the same decks. ngspice is a reference for these
//! tests only; they are skipped when it isn't installed (set `CIRCUITER_NGSPICE` to
//! its path to use one outside `PATH`).

use circuiter_sim::{Circuit, Options, dc_sweep, op, tran};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

fn ngspice() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CIRCUITER_NGSPICE") {
        return Some(p.into());
    }
    let ok = Command::new("ngspice").arg("--version").output().is_ok_and(|o| o.status.success());
    ok.then(|| "ngspice".into())
}

macro_rules! require_ngspice {
    () => {
        match ngspice() {
            Some(p) => p,
            None => {
                eprintln!("ngspice not found; skipping comparison");
                return;
            }
        }
    };
}

fn run(bin: &PathBuf, deck: &str, control: &str) -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("deck.cir");
    let text = format!(
        "* circuiter comparison\n{deck}\n.control\noption numdgt=12\n{control}\n.endc\n.end\n"
    );
    std::fs::write(&file, text).unwrap();
    let out = Command::new(bin).arg("-b").arg(&file).current_dir(dir.path()).output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), dir)
}

/// Node voltages from `op` + `print all`.
fn ngspice_op(bin: &PathBuf, deck: &str) -> BTreeMap<String, f64> {
    let (out, _dir) = run(bin, deck, "op\nprint all");
    out.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(" = ")?;
            Some((k.trim().to_ascii_lowercase(), v.trim().parse().ok()?))
        })
        .collect()
}

/// Columns written by `wrdata` after `analysis`: the sweep variable, then each vector.
fn ngspice_table(bin: &PathBuf, deck: &str, analysis: &str, vectors: &[&str]) -> Vec<Vec<f64>> {
    let control = format!("{analysis}\nset wr_singlescale\nwrdata out.txt {}", vectors.join(" "));
    let (_, dir) = run(bin, deck, &control);
    let text = std::fs::read_to_string(dir.path().join("out.txt")).expect("ngspice wrote data");
    text.lines()
        .filter_map(|l| l.split_whitespace().map(|w| w.parse().ok()).collect::<Option<Vec<f64>>>())
        .filter(|row| !row.is_empty())
        .collect()
}

fn close(a: f64, b: f64, rel: f64, abs: f64) -> bool {
    (a - b).abs() <= abs + rel * a.abs().max(b.abs())
}

/// Every node voltage of our operating point agrees with ngspice's.
fn compare_op(deck: &str) {
    let bin = require_ngspice!();
    let reference = ngspice_op(&bin, deck);
    let c = Circuit::parse(deck).unwrap();
    let ours = op(&c, &Options { leakage: 0.0, ..Options::default() }).unwrap();
    let mut compared = 0;
    for (k, name) in c.nodes.iter().enumerate().skip(1) {
        if name.contains('#') {
            continue;
        }
        let r = reference[&name.to_ascii_lowercase()];
        let v = ours.voltages[k];
        assert!(close(v, r, 1e-4, 1e-6), "{name}: ours {v}, ngspice {r}\n{deck}");
        compared += 1;
    }
    assert!(compared > 0);
}

/// A waveform agrees with ngspice's within `tol` of the signal's swing at each of
/// ngspice's time points, allowing half a print step of timing difference: on a fast
/// edge, a tiny shift in time is a large difference in value.
fn compare_tran(deck: &str, tstop: f64, tstep: f64, node: &str, tol: f64) {
    let bin = require_ngspice!();
    let table =
        ngspice_table(&bin, deck, &format!("tran {tstep} {tstop}"), &[&format!("v({node})")]);
    let c = Circuit::parse(deck).unwrap();
    let r = tran(&c, tstop, tstep, &Options { leakage: 0.0, ..Options::default() }).unwrap();
    let ours = r.voltage(&c, node).unwrap();
    let (lo, hi) =
        table.iter().fold((f64::MAX, f64::MIN), |(lo, hi), row| (lo.min(row[1]), hi.max(row[1])));
    let swing = (hi - lo).max(1e-9);
    let mut worst = (0.0, 0.0);
    for row in &table {
        let err = (0..=10)
            .map(|k| row[0] + tstep * (k as f64 / 10.0 - 0.5))
            .map(|t| (r.sample(&ours, t.clamp(0.0, tstop)) - row[1]).abs() / swing)
            .fold(f64::INFINITY, f64::min);
        if err > worst.0 {
            worst = (err, row[0]);
        }
    }
    eprintln!(
        "v({node}): worst {:.3}% of swing; {} points, {} rejected",
        worst.0 * 100.0,
        r.time.len(),
        r.rejected
    );
    assert!(
        worst.0 <= tol,
        "v({node}) differs from ngspice by {:.3}% of its swing at t = {:e} \
         ({} of our points, {} rejected)",
        worst.0 * 100.0,
        worst.1,
        r.time.len(),
        r.rejected
    );
}

const N4148: &str = ".model d4148 D(IS=2.52n N=1.752 RS=0.568 BV=100 IBV=100u CJO=4p TT=20n)";
const CMOS: &str = "\
.model nch NMOS(LEVEL=1 VTO=0.7 KP=110u LAMBDA=0.04 GAMMA=0.4 PHI=0.65)
.model pch PMOS(LEVEL=1 VTO=-0.7 KP=50u LAMBDA=0.04 GAMMA=0.4 PHI=0.65)";

#[test]
fn op_diode_resistor() {
    compare_op(&format!("V1 in 0 DC 5\nR1 in a 1k\nD1 a 0 d4148\n{N4148}"));
}

#[test]
fn op_diode_in_reverse_breakdown() {
    compare_op(&format!("V1 0 in DC 150\nR1 in a 10k\nD1 a 0 d4148\n{N4148}"));
}

#[test]
fn op_cmos_inverter_across_its_transfer_curve() {
    for vin in ["0", "0.6", "0.8", "0.9", "1.0", "1.2", "1.8"] {
        compare_op(&format!(
            "VDD vdd 0 DC 1.8\nVIN in 0 DC {vin}\n\
             MP out in vdd vdd pch W=2u L=180n\nMN out in 0 0 nch W=1u L=180n\n{CMOS}"
        ));
    }
}

#[test]
fn op_source_follower_with_body_effect() {
    compare_op(&format!(
        "VDD vdd 0 DC 3.3\nVG g 0 DC 2.5\nMN vdd g s 0 nch W=10u L=1u\nRS s 0 10k\n{CMOS}"
    ));
}

#[test]
fn op_current_mirror() {
    compare_op(&format!(
        "VDD vdd 0 DC 3\nIREF vdd d1 DC 50u\nM1 d1 d1 0 0 nch W=5u L=1u\n\
         M2 out d1 0 0 nch W=5u L=1u\nRL vdd out 20k\n{CMOS}"
    ));
}

#[test]
fn dc_sweep_traces_the_inverter_transfer_curve() {
    let bin = require_ngspice!();
    let deck = format!(
        "VDD vdd 0 DC 1.8\nVIN in 0 DC 0\n\
         MP out in vdd vdd pch W=2u L=180n\nMN out in 0 0 nch W=1u L=180n\n{CMOS}"
    );
    let table = ngspice_table(&bin, &deck, "dc VIN 0 1.8 0.05", &["v(out)"]);
    let c = Circuit::parse(&deck).unwrap();
    let values: Vec<f64> = table.iter().map(|r| r[0]).collect();
    let ours =
        dc_sweep(&c, "VIN", &values, &Options { leakage: 0.0, ..Options::default() }).unwrap();
    for (row, point) in table.iter().zip(&ours) {
        let v = point.voltage(&c, "out").unwrap();
        assert!(close(v, row[1], 1e-3, 1e-5), "at {} V: ours {v}, ngspice {}", row[0], row[1]);
    }
}

#[test]
fn tran_rc_step() {
    compare_tran(
        "V1 in 0 PULSE(0 1 1u 1n 1n 1 2)\nR1 in out 1k\nC1 out 0 1n",
        10e-6,
        10e-9,
        "out",
        0.005,
    );
}

#[test]
fn tran_rlc_ringing() {
    compare_tran(
        "V1 in 0 PULSE(0 1 0 1n 1n 1 2)\nR1 in a 10\nL1 a out 10u\nC1 out 0 100n",
        30e-6,
        20e-9,
        "out",
        0.01,
    );
}

#[test]
fn tran_half_wave_rectifier() {
    compare_tran(
        &format!("V1 in 0 SIN(0 5 1k)\nD1 in out d4148\nC1 out 0 10u\nR1 out 0 1k\n{N4148}"),
        3e-3,
        2e-6,
        "out",
        0.01,
    );
}

#[test]
fn tran_cmos_inverter_driving_a_load() {
    compare_tran(
        &format!(
            "VDD vdd 0 DC 1.8\nVIN in 0 PULSE(0 1.8 0 50p 50p 4.95n 10n)\n\
             MP out in vdd vdd pch W=2u L=180n\nMN out in 0 0 nch W=1u L=180n\n\
             CL out 0 10f\n{CMOS}"
        ),
        30e-9,
        10e-12,
        "out",
        0.01,
    );
}

#[test]
fn tran_diode_reverse_recovery() {
    compare_tran(
        &format!("V1 in 0 PULSE(1 -1 20n 1n 1n 1 2)\nR1 in a 100\nD1 a 0 d4148\n{N4148}"),
        100e-9,
        0.1e-9,
        "a",
        0.01,
    );
}

const CMOS_CAPS: &str = "\
.model nc NMOS(LEVEL=1 VTO=0.5 KP=170u LAMBDA=0.05 GAMMA=0.45 PHI=0.8 TOX=4.1n CGSO=3e-10 CGDO=3e-10 CJ=1e-3 CJSW=2.5e-10 MJ=0.45 MJSW=0.33 PB=0.8)
.model pc PMOS(LEVEL=1 VTO=-0.5 KP=60u LAMBDA=0.06 GAMMA=0.5 PHI=0.8 TOX=4.1n CGSO=3e-10 CGDO=3e-10 CJ=1.2e-3 CJSW=2.8e-10 MJ=0.45 MJSW=0.33 PB=0.8)";

fn inverter(name: &str, a: &str, y: &str) -> String {
    format!(
        "MP{name} {y} {a} vdd vdd pc W=2u L=180n AD=1p AS=1p PD=4u PS=4u\n\
         MN{name} {y} {a} 0 0 nc W=1u L=180n AD=0.5p AS=0.5p PD=3u PS=3u\n"
    )
}

#[test]
fn tran_inverter_chain_with_gate_and_junction_capacitance() {
    let deck = format!(
        "VDD vdd 0 DC 1.8\nVIN a 0 PULSE(0 1.8 100p 20p 20p 1n 2n)\n{}{}{}{CMOS_CAPS}",
        inverter("1", "a", "b"),
        inverter("2", "b", "c"),
        inverter("3", "c", "d")
    );
    compare_tran(&deck, 4e-9, 2e-12, "d", 0.02);
    compare_tran(&deck, 4e-9, 2e-12, "b", 0.02);
}

#[test]
fn tran_mosfet_through_every_region_with_meyer_capacitance() {
    compare_tran(
        &format!(
            "VD d 0 DC 1.8\nVG g 0 PULSE(-0.5 1.8 0 1n 1n 1n 4n)\nRG g gi 10k\n\
             RD d di 2k\nMN di gi 0 0 nc W=4u L=180n AD=2p AS=2p PD=6u PS=6u\n{CMOS_CAPS}"
        ),
        8e-9,
        5e-12,
        "di",
        0.02,
    );
}

/// The example project, flattened by the netlister, simulated by both engines.
#[test]
fn tran_example_project_from_the_netlister() {
    use circuiter_model::{CellId, Design, Project};
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cmos_inverter");
    let project = Project::load(&root).unwrap();
    let design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
    let netlist =
        circuiter_netlist::Netlist::build(&design, &CellId::new("cmos_inverter", "testbench"))
            .unwrap();
    // Drop the title comment; `run` adds its own.
    let deck: String = netlist.to_spice().lines().skip(1).collect::<Vec<_>>().join("\n");
    compare_tran(&deck, 30e-9, 20e-12, "OUT", 0.02);
}

#[test]
fn tran_transformer_step_down_with_rectified_load() {
    // A 10:1 transformer (inductance ratio 100) feeding a bridge-less rectifier.
    compare_tran(
        &format!(
            "V1 in 0 SIN(0 100 50)\nRP in p 1\nLP p 0 1\nLS s 0 10m\nK1 LP LS 0.98\n\
             D1 s out d4148\nC1 out 0 100u\nRL out 0 1k\n{N4148}"
        ),
        60e-3,
        20e-6,
        "out",
        0.01,
    );
    compare_tran(
        "V1 in 0 SIN(0 100 50)\nRP in p 1\nLP p 0 1\nLS s 0 10m\nK1 LP LS 0.98\nRL s 0 100",
        40e-3,
        20e-6,
        "s",
        0.01,
    );
}

const BJTS: &str = "\
.model q3904 NPN(IS=6.734f XTI=3 EG=1.11 VAF=74.03 BF=416.4 NE=1.259 ISE=6.734f IKF=66.78m XTB=1.5 BR=.7371 NC=2 ISC=0 IKR=0 RC=1 CJC=3.638p MJC=.3085 VJC=.75 FC=.5 CJE=4.493p MJE=.2593 VJE=.75 TR=239.5n TF=301.2p ITF=.4 VTF=4 XTF=2 RB=10)
.model q3906 PNP(IS=1.41f XTI=3 EG=1.11 VAF=18.7 BF=180.7 NE=1.5 ISE=0 IKF=80m XTB=1.5 BR=4.977 NC=2 ISC=0 IKR=0 RC=2.5 CJC=9.728p MJC=.5776 VJC=.75 FC=.5 CJE=8.063p MJE=.3677 VJE=.75 TR=33.42n TF=179.3p ITF=.4 VTF=4 XTF=6 RB=10)";

#[test]
fn op_bjt_divider_biased_amplifier() {
    compare_op(&format!(
        "VCC vcc 0 DC 12\nR1 vcc b 47k\nR2 b 0 10k\nRC vcc c 4.7k\nRE e 0 1k\nQ1 c b e q3904\n{BJTS}"
    ));
}

#[test]
fn op_bjt_in_saturation_and_pnp_mirror() {
    compare_op(&format!("VCC vcc 0 DC 5\nRB vcc b 10k\nRC vcc c 1k\nQ1 c b 0 q3904\n{BJTS}"));
    compare_op(&format!(
        "VCC vcc 0 DC 9\nQ1 d d vcc q3906\nQ2 out d vcc q3906\nRREF d 0 8.2k\nRL out 0 2k\n{BJTS}"
    ));
}

#[test]
fn tran_bjt_switch_with_storage_time() {
    compare_tran(
        &format!(
            "VCC vcc 0 DC 5\nVIN in 0 PULSE(0 5 100n 5n 5n 1u 2u)\nRB in b 4.7k\nRC vcc c 1k\n\
             Q1 c b 0 q3904\n{BJTS}"
        ),
        3e-6,
        2e-9,
        "c",
        0.02,
    );
}

#[test]
fn tran_bjt_common_emitter_amplifier() {
    compare_tran(
        &format!(
            "VCC vcc 0 DC 12\nVS s 0 SIN(0 10m 10k)\nCIN s b 10u\nR1 vcc b 47k\nR2 b 0 10k\n\
             RC vcc c 4.7k\nRE e 0 1k\nCE e 0 100u\nQ1 c b e q3904\n{BJTS}"
        ),
        300e-6,
        200e-9,
        "c",
        0.02,
    );
}

/// The AC response of `node` agrees with ngspice's at every frequency, in magnitude
/// (relative) and phase.
fn compare_ac(deck: &str, node: &str, fstart: f64, fstop: f64) {
    use circuiter_sim::{Spacing, ac};
    let bin = require_ngspice!();
    let table =
        ngspice_table(&bin, deck, &format!("ac dec 10 {fstart} {fstop}"), &[&format!("v({node})")]);
    let c = Circuit::parse(deck).unwrap();
    let r =
        ac(&c, Spacing::Decade(10), fstart, fstop, &Options { leakage: 0.0, ..Options::default() })
            .unwrap();
    let ours = r.voltage(&c, node).unwrap();
    assert_eq!(r.frequency.len(), table.len());
    let mut worst = (0.0_f64, 0.0_f64);
    for ((row, v), f) in table.iter().zip(&ours).zip(&r.frequency) {
        assert!(close(*f, row[0], 1e-9, 0.0), "frequency {f} vs {}", row[0]);
        let reference = circuiter_sim::Complex::new(row[1], row[2]);
        let mag = (v.abs() - reference.abs()).abs() / reference.abs().max(1e-12);
        let mut phase = (v.phase() - reference.phase()).abs();
        if phase > 180.0 {
            phase = 360.0 - phase;
        }
        worst = (worst.0.max(mag), worst.1.max(phase));
    }
    eprintln!("v({node}): worst magnitude {:.4}%, phase {:.4}°", worst.0 * 100.0, worst.1);
    assert!(worst.0 < 1e-3 && worst.1 < 0.1, "magnitude {:e}, phase {}°", worst.0, worst.1);
}

#[test]
fn ac_rc_and_rlc_filters() {
    compare_ac("V1 in 0 DC 0 AC 1\nR1 in out 1k\nC1 out 0 1u", "out", 1.0, 1e6);
    compare_ac(
        "V1 in 0 DC 0 AC 1\nR1 in a 50\nL1 a out 1m\nC1 out 0 1u\nR2 out 0 1k\nL2 b 0 1m\nK1 L1 L2 0.5\nR3 b 0 10",
        "out",
        10.0,
        1e6,
    );
}

#[test]
fn ac_bjt_common_emitter_gain() {
    compare_ac(
        &format!(
            "VCC vcc 0 DC 12\nVS s 0 DC 0 AC 1\nCIN s b 10u\nR1 vcc b 47k\nR2 b 0 10k\n\
             RC vcc c 4.7k\nRE e 0 1k\nCE e 0 100u\nQ1 c b e q3904\nCL c 0 100p\n{BJTS}"
        ),
        "c",
        1.0,
        1e9,
    );
}

#[test]
fn ac_cmos_common_source_with_meyer_capacitance() {
    compare_ac(
        &format!(
            "VDD vdd 0 DC 1.8\nVG g 0 DC 0.8 AC 1\nRL vdd d 10k\n\
             MN d g 0 0 nc W=2u L=180n AD=1p AS=1p PD=4u PS=4u\nCL d 0 20f\n{CMOS_CAPS}"
        ),
        "d",
        1e3,
        1e11,
    );
}

#[test]
fn op_controlled_sources() {
    compare_op(
        "V1 a 0 DC 1\nR1 a b 1k\nVS b 0 DC 0\nF1 0 c VS 10\nR2 c 0 100\n\
         H1 d 0 VS 1k\nR3 d 0 1k\nG1 0 e a 0 2m\nR4 e 0 1k\nE1 f 0 e 0 -3\nR5 f 0 1k",
    );
}

const SUBCKTS: &str = "\
.param rval=2k
.subckt divider in out gnd params: r1=1k r2={r1*2}
R1 in out {r1}
R2 out gnd {r2}
.ends
.subckt opamp inp inn out
.model dclamp D(IS=1e-15)
Rin inp inn 1meg
G1 0 int inp inn 1m
Rint int 0 100meg
Cc int 0 15.9u
D1 int clampp dclamp
VCP clampp 0 DC 12
E1 buf 0 int 0 1
Rout buf out 75
.ends
.subckt buffered in out
X1 in mid 0 divider r1=1k r2={rval}
X2 mid out out opamp
.ends";

#[test]
fn op_nested_subcircuits_with_parameters() {
    compare_op(&format!(
        "V1 a 0 DC 3\nX1 a b buffered\nRL b 0 10k\nX2 a c 0 divider\nRL2 c 0 1meg\n{SUBCKTS}"
    ));
}

#[test]
fn ac_opamp_subcircuit_frequency_response() {
    compare_ac(
        &format!("V1 a 0 DC 0 AC 1\nX1 a b buffered\nRL b 0 10k\n{SUBCKTS}"),
        "b",
        0.01,
        1e6,
    );
}

#[test]
fn tran_opamp_inverting_amplifier_from_a_subcircuit() {
    compare_tran(
        &format!(
            "V1 in 0 SIN(0 0.1 1k)\nRI in n 1k\nRF n out 10k\nX1 0 n out opamp\nRL out 0 2k\n{SUBCKTS}"
        ),
        3e-3,
        2e-6,
        "out",
        0.01,
    );
}

/// The classic µA741 macromodel (Boyle et al.), as published by the manufacturers:
/// POLY sources with voltage and current inputs, a current-controlled limiter, BJTs
/// and clamp diodes. Pins: non-inverting, inverting, V+, V−, output.
const UA741: &str = "\
.subckt uA741 1 2 3 4 5
c1   11 12 8.661E-12
c2    6  7 30.00E-12
dc    5 53 dx
de   54  5 dx
dlp  90 91 dx
dln  92 90 dx
dp    4  3 dx
egnd 99  0 poly(2) (3,0) (4,0) 0 .5 .5
fb    7 99 poly(5) vb vc ve vlp vln 0 10.61E6 -10E6 10E6 10E6 -10E6
ga    6  0 11 12 188.5E-6
gcm   0  6 10 99 5.961E-9
iee  10  4 dc 15.16E-6
hlim 90  0 vlim 1K
q1   11  2 13 qx
q2   12  1 14 qx
r2    6  9 100.0E3
rc1   3 11 5.305E3
rc2   3 12 5.305E3
re1  13 10 1.836E3
re2  14 10 1.836E3
ree  10 99 13.19E6
ro1   8  5 50
ro2   7 99 100
rp    3  4 18.16E3
vb    9  0 dc 0
vc    3 53 dc 1
ve   54  4 dc 1
vlim  7  8 dc 0
vlp  91  0 dc 40
vln   0 92 dc 40
.model dx D(Is=800.0E-18 Rs=1)
.model qx NPN(Is=800.0E-18 Bf=93.75)
.ends";

const UA741_AMP: &str = "VP vp 0 DC 15\nVN vn 0 DC -15\nRI in n 10k\nRF n out 100k\nXU 0 n vp vn out uA741\nRL out 0 2k";

#[test]
fn op_ua741_macromodel() {
    compare_op(&format!("V1 in 0 DC 0.5\n{UA741_AMP}\n{UA741}"));
}

#[test]
fn tran_ua741_amplifier_clipping_and_slewing() {
    // 2 V peak in, gain -10: the output clips at the rails and slews on the way.
    compare_tran(&format!("V1 in 0 SIN(0 2 2k)\n{UA741_AMP}\n{UA741}"), 1e-3, 1e-6, "out", 0.01);
}

#[test]
fn ac_ua741_open_loop_gain_and_bandwidth() {
    compare_ac(
        &format!(
            "VP vp 0 DC 15\nVN vn 0 DC -15\nV1 in 0 DC 0 AC 1\nXU in 0 vp vn out uA741\nRL out 0 2k\n{UA741}"
        ),
        "out",
        0.1,
        10e6,
    );
}

#[test]
fn op_and_tran_switches() {
    compare_op(
        "V1 a 0 DC 5\nR1 a b 1k\nS1 b 0 a 0 sw1\nVC c 0 DC 1\nRC c 0 1k\nVS c2 0 DC 0\n\
         W1 a d VS csw1\nR2 d 0 1k\nRS c c2 100\n.model sw1 SW(VT=2 VH=0.5 RON=10 ROFF=1meg)\n\
         .model csw1 CSW(IT=1m IH=0.2m RON=5 ROFF=1meg)",
    );
    // A slowly rising and falling control crosses the hysteresis band both ways.
    compare_tran(
        "V1 a 0 DC 5\nR1 a b 1k\nS1 b 0 c 0 sw1\nVC c 0 PWL(0 0 1m 4 2m 0)\n\
         .model sw1 SW(VT=2 VH=0.5 RON=10 ROFF=1meg)",
        2e-3,
        2e-6,
        "b",
        0.02,
    );
}

#[test]
fn op_behavioral_sources() {
    compare_op(
        "V1 in 0 DC 0.3\nB1 out 0 V={10*tanh(5*V(in))}\nR1 out 0 1k\n\
         B2 0 x I=1m*V(in,0)^2+V(out)*1u\nR2 x 0 1k\n\
         VS out s DC 0\nR3 s 0 2k\nB3 y 0 V=I(VS)*1k + (V(in) > 0.2 ? 1 : 0)\nR4 y 0 1k",
    );
}

#[test]
fn tran_behavioral_source_in_a_subcircuit() {
    // A cube-law amplifier driving an RC, inside a sub-circuit with a parameter.
    compare_tran(
        "V1 in 0 SIN(0 1 1k)\nX1 in o cube k=2\nR1 o o2 1k\nC1 o2 0 100n\n\
         .subckt cube a y k=1\nB1 y 0 V={k*V(a)^3}\n.ends",
        2e-3,
        2e-6,
        "o2",
        0.01,
    );
}

const JFETS: &str = "\
.model jn NJF(VTO=-3 BETA=1.304m LAMBDA=2.25m RD=1 RS=1 IS=33.57f CGS=2.414p CGD=1.6p PB=1)
.model jp PJF(VTO=-2.5 BETA=0.5m LAMBDA=5m IS=10f CGS=4p CGD=2p)";

/// A self-biased common-source stage and a p-channel follower, in all their regions.
const JFET_AMP: &str = "\
VDD vdd 0 DC 15
RD vdd d 2.2k
J1 d g s jn
RS s 0 1k
RG g 0 1meg
CI in g 1u
CS s 0 100u
J2 0 d s2 jp
RS2 vdd s2 4.7k";

#[test]
fn op_jfets() {
    compare_op(&format!("V1 in 0 DC 0\n{JFET_AMP}\n{JFETS}"));
    // Forward-biased gate, and the reverse (drain below source) region.
    compare_op(&format!("V1 g 0 DC 0.8\nRG g g2 1k\nVD d 0 DC -0.5\nJ1 d g2 0 jn\n{JFETS}"));
}

#[test]
fn tran_jfet_amplifier() {
    compare_tran(&format!("V1 in 0 SIN(0 0.2 10k)\n{JFET_AMP}\n{JFETS}"), 3e-4, 2e-7, "d", 0.01);
}

#[test]
fn ac_jfet_amplifier() {
    compare_ac(&format!("V1 in 0 DC 0 AC 1\n{JFET_AMP}\n{JFETS}"), "d", 10.0, 100e6);
}
