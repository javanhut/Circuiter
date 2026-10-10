//! Running simulations on the design being edited, and mapping results back onto the
//! sheets: net voltages, device currents and power, waveforms.

use crate::digital::{self, LogicRun};
use circuiter_model::check::{Diagnostic, Severity};
use circuiter_model::units::format_quantity;
use circuiter_model::{CellId, Design};
use circuiter_netlist::Netlist;
use circuiter_physics::{AMBIENT, Assessment, Event, Monitor};
use circuiter_sim::{
    AcResult, Circuit, Complex, Control, OpResult, Options, Problem, Spacing, TranResult,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// An analysis to run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Analysis {
    Op,
    /// `power_on` starts with everything off, as when the circuit is switched on;
    /// otherwise the run starts from the operating point.
    Tran {
        tstop: f64,
        tstep: f64,
        power_on: bool,
    },
    /// A sweep from `fstart` to `fstop` with `per_decade` points per decade.
    Ac {
        fstart: f64,
        fstop: f64,
        per_decade: usize,
    },
}

/// What a run produced: results and, for a transient run, what the physics engine saw.
type Outcome = Result<(Results, Option<Monitor>), String>;

/// An analysis running on another thread.
#[derive(Debug)]
pub struct Job {
    pub control: Arc<Control>,
    rx: Receiver<Outcome>,
}

/// The analysis whose results are showing.
#[derive(Debug, Clone, PartialEq)]
pub enum Results {
    Op(OpResult),
    Tran {
        result: TranResult,
        tstop: f64,
        tstep: f64,
    },
    Ac {
        result: AcResult,
        fstart: f64,
        fstop: f64,
        per_decade: usize,
    },
    /// A run of the digital simulator.
    Logic(LogicRun),
}

#[derive(Debug)]
pub struct Simulation {
    /// The cell simulated: the top of the hierarchy being viewed.
    pub top: CellId,
    pub netlist: Netlist,
    pub circuit: Circuit,
    pub results: Option<Results>,
    /// Why the last run failed, if it did.
    pub error: Option<String>,
    /// The design changed since this ran; results no longer match the sheets.
    pub stale: bool,
    /// Flat nets shown in the waveform plot.
    pub probes: Vec<usize>,
    /// Problems found in the circuit before or while simulating.
    pub diagnostics: Vec<Diagnostic>,
    /// The analysis in progress, if one is running.
    pub job: Option<Job>,
    /// After a transient run: every rated part's heating and what was destroyed.
    pub monitor: Option<Monitor>,
    /// After an operating point: parts that would overheat if it held.
    pub assessments: Vec<Assessment>,
    /// Every part is digital: this runs in the digital simulator.
    pub digital: bool,
    /// The project directory, where programs for memories are found.
    pub root: PathBuf,
}

impl Simulation {
    /// Flattens `top` and builds the circuit. Fails only if there is nothing to
    /// simulate; circuit problems are reported in [`Simulation::diagnostics`].
    #[cfg(test)]
    pub fn prepare(design: &Design, top: &CellId) -> Result<Simulation, String> {
        Simulation::prepare_in(design, top, Path::new("."))
    }

    /// As [`Simulation::prepare`], with programs found relative to `root`. A design
    /// made only of digital parts runs in the digital simulator.
    pub fn prepare_in(design: &Design, top: &CellId, root: &Path) -> Result<Simulation, String> {
        if Netlist::is_digital(design, top) {
            let netlist = Netlist::build_digital(design, top)?;
            let mut diagnostics = netlist.diagnostics.clone();
            let has_errors = diagnostics.iter().any(|d| d.severity == Severity::Error);
            if !has_errors {
                // Build it once now, so missing programs and mismatched buses show as
                // problems before Run.
                if let Err(message) = digital::build(&netlist, root) {
                    diagnostics.push(Diagnostic {
                        severity: Severity::Error,
                        location: format!("{top}/simulation"),
                        message,
                    });
                }
            }
            let probes = default_probes(&netlist);
            return Ok(Simulation {
                top: top.clone(),
                netlist,
                circuit: Circuit::parse("").map_err(|e| format!("netlist: {e}"))?,
                results: None,
                error: None,
                stale: false,
                probes,
                diagnostics,
                job: None,
                monitor: None,
                assessments: Vec::new(),
                digital: true,
                root: root.to_path_buf(),
            });
        }
        let netlist = Netlist::build(design, top)?;
        let mut diagnostics = netlist.diagnostics.clone();
        let location = format!("{top}/simulation");
        let circuit = Circuit::parse(&netlist.to_spice()).map_err(|e| format!("netlist: {e}"))?;
        for problem in circuit.topology_problems() {
            let message = match problem {
                Problem::Floating { nodes } => {
                    let names: Vec<String> =
                        nodes.iter().filter_map(|n| display_name(&netlist, n)).collect();
                    if names.is_empty() {
                        continue;
                    }
                    format!(
                        "no DC path to ground from {}: only leakage holds {}",
                        names.join(", "),
                        if names.len() == 1 { "its voltage" } else { "their voltages" }
                    )
                }
                Problem::SourceLoop { device } => format!(
                    "{} closes a loop of voltage sources and inductors; nothing limits the current",
                    element_path(&netlist, &device).unwrap_or(device)
                ),
            };
            let severity =
                if message.contains("loop") { Severity::Error } else { Severity::Warning };
            diagnostics.push(Diagnostic { severity, location: location.clone(), message });
        }
        for w in &circuit.warnings {
            diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                location: location.clone(),
                message: w.clone(),
            });
        }
        let probes = default_probes(&netlist);
        Ok(Simulation {
            top: top.clone(),
            netlist,
            circuit,
            results: None,
            error: None,
            stale: false,
            probes,
            diagnostics,
            job: None,
            monitor: None,
            assessments: Vec::new(),
            digital: false,
            root: root.to_path_buf(),
        })
    }

    /// A sensible run: (stop time, step) in seconds.
    pub fn default_run(&self) -> (f64, f64) {
        if self.digital {
            let t = digital::default_stop(&self.netlist);
            (t, t / 1000.0)
        } else {
            default_tran(&self.circuit)
        }
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }

    /// Runs `analysis` to completion on this thread.
    #[cfg(test)]
    pub fn run(&mut self, analysis: Analysis) {
        self.results = None;
        self.error = None;
        if self.has_errors() {
            self.error = Some("fix the errors listed under Problems first".into());
            return;
        }
        let outcome = if self.digital {
            execute_digital(&self.netlist, &self.root, analysis, &Control::default())
        } else {
            execute(&self.circuit, &self.netlist, analysis, Options::default())
        };
        self.finish(outcome);
    }

    /// Starts `analysis` on another thread; [`Simulation::poll`] collects the result.
    pub fn start(&mut self, analysis: Analysis) {
        self.results = None;
        self.error = None;
        if self.has_errors() {
            self.error = Some("fix the errors listed under Problems first".into());
            return;
        }
        let control = Arc::new(Control::default());
        let options = Options { control: Some(control.clone()), ..Options::default() };
        let circuit = self.circuit.clone();
        let netlist = self.netlist.clone();
        let (tx, rx) = channel();
        let (is_digital, root, c) = (self.digital, self.root.clone(), control.clone());
        std::thread::spawn(move || {
            let outcome = if is_digital {
                execute_digital(&netlist, &root, analysis, &c)
            } else {
                execute(&circuit, &netlist, analysis, options)
            };
            let _ = tx.send(outcome);
        });
        self.job = Some(Job { control, rx });
    }

    /// Collects a finished background run. Returns whether one is still running.
    pub fn poll(&mut self) -> bool {
        let Some(job) = &self.job else { return false };
        match job.rx.try_recv() {
            Ok(outcome) => {
                self.job = None;
                self.finish(outcome);
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.error = Some("the simulation stopped unexpectedly".into());
                false
            }
        }
    }

    pub fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.control.cancel();
        }
    }

    fn finish(&mut self, outcome: Outcome) {
        self.monitor = None;
        self.assessments.clear();
        match outcome {
            Ok((r, monitor)) => {
                if let Results::Op(op) = &r {
                    if let Ok(m) = Monitor::new(&self.netlist, &self.circuit, AMBIENT) {
                        self.assessments = m.assess(&self.circuit, op);
                    }
                }
                self.monitor = monitor;
                self.results = Some(r);
            }
            Err(mut message) => {
                for net in &self.netlist.nets {
                    if net.node != net.name && message.contains(&net.node) {
                        message = message.replace(&net.node, &net.name);
                    }
                }
                self.error = Some(message);
            }
        }
    }

    /// What destroyed the part at instance path `path`, if anything did.
    pub fn failure(&self, path: &str) -> Option<&Event> {
        self.monitor.as_ref()?.events.iter().find(|e| e.path == path)
    }

    /// Every part's destruction, in order.
    pub fn events(&self) -> &[Event] {
        self.monitor.as_ref().map_or(&[], |m| &m.events)
    }

    /// The small-signal response of flat net `net`.
    pub fn response(&self, net: usize) -> Option<(&[f64], Vec<Complex>)> {
        match &self.results {
            Some(Results::Ac { result, .. }) => {
                let node = &self.netlist.nets.get(net)?.node;
                Some((&result.frequency, result.voltage(&self.circuit, node)?))
            }
            _ => None,
        }
    }

    /// The circuit node of flat net `net`.
    fn node(&self, net: usize) -> Option<usize> {
        self.circuit.node(&self.netlist.nets.get(net)?.node)
    }

    /// The operating-point voltage of flat net `net`.
    pub fn voltage(&self, net: usize) -> Option<f64> {
        match &self.results {
            Some(Results::Op(op)) => Some(op.voltages[self.node(net)?]),
            _ => None,
        }
    }

    /// The waveform of flat net `net`.
    pub fn waveform(&self, net: usize) -> Option<(&[f64], Vec<f64>)> {
        match &self.results {
            Some(Results::Tran { result, .. }) => {
                let node = &self.netlist.nets.get(net)?.node;
                Some((&result.time, result.voltage(&self.circuit, node)?))
            }
            _ => None,
        }
    }

    /// What the leaf element at instance path `path` is doing at the operating point:
    /// `(terminal, current into it)` pairs and absorbed power.
    pub fn element_op(&self, path: &str) -> Option<(Vec<(String, f64)>, f64)> {
        let Some(Results::Op(op)) = &self.results else { return None };
        let e = self.netlist.elements.iter().find(|e| e.path == path)?;
        let name = e.line.split_whitespace().next()?;
        let dev = op.device(name)?;
        let ports: Vec<String> = e.ports.iter().map(|(p, _)| p.clone()).collect();
        // The card's terminal order is the template's; map by node where possible.
        let terminals = self.circuit.devices[self.circuit.device(name)?].terminals();
        let mut out = Vec::new();
        for (k, &node) in terminals.iter().enumerate() {
            let port = e
                .ports
                .iter()
                .find(|(_, net)| self.node(*net) == Some(node))
                .map(|(p, _)| p.clone())
                .unwrap_or_else(|| ports.get(k).cloned().unwrap_or_default());
            if !out.iter().any(|(p, _): &(String, f64)| *p == port) {
                out.push((port, dev.currents[k]));
            }
        }
        Some((out, dev.power))
    }

    /// Total power absorbed by every element under instance path `path`, for a
    /// sub-circuit; `None` without an operating point.
    pub fn power_under(&self, path: &str) -> Option<f64> {
        let Some(Results::Op(op)) = &self.results else { return None };
        let prefix = format!("{path}.");
        let mut total = 0.0;
        for e in self.netlist.elements.iter().filter(|e| e.path.starts_with(&prefix)) {
            let name = e.line.split_whitespace().next()?;
            total += op.device(name)?.power;
        }
        Some(total)
    }
}

/// A digital run: any analysis means "run for a while" (a transient run's stop time,
/// or the default).
fn execute_digital(
    netlist: &Netlist,
    root: &Path,
    analysis: Analysis,
    control: &Control,
) -> Outcome {
    let tstop = match analysis {
        Analysis::Tran { tstop, .. } => tstop,
        _ => digital::default_stop(netlist),
    };
    let circuit = digital::build(netlist, root)?;
    let run = digital::run(circuit, tstop, &mut |f| {
        control.set_progress(f);
        !control.is_cancelled()
    })?;
    Ok((Results::Logic(run), None))
}

fn execute(c: &Circuit, netlist: &Netlist, analysis: Analysis, o: Options) -> Outcome {
    let r = match analysis {
        Analysis::Op => circuiter_sim::op(c, &o).map(|op| (Results::Op(op), None)),
        Analysis::Tran { tstop, tstep, power_on } => {
            // The physics engine watches every transient run: parts heat, and fail.
            let mut monitor = Monitor::new(netlist, c, AMBIENT)?;
            let o = Options { power_on, ..o };
            circuiter_sim::tran_observed(c, tstop, tstep, &o, &mut monitor)
                .map(|(result, _)| (Results::Tran { result, tstop, tstep }, Some(monitor)))
        }
        Analysis::Ac { fstart, fstop, per_decade } => {
            circuiter_sim::ac(c, Spacing::Decade(per_decade), fstart, fstop, &o)
                .map(|result| (Results::Ac { result, fstart, fstop, per_decade }, None))
        }
    };
    r.map_err(|e| e.message)
}

/// A short word for how a part died: `burnt`, `blown`, `exploded`.
pub fn effect_word(e: &Event) -> &'static str {
    use circuiter_model::cell::Effect;
    match e.effect {
        Effect::Smoke => "burnt",
        Effect::Pop => "cracked",
        Effect::Flash => "blown",
        Effect::Explode => "exploded",
    }
}

/// The netlist's display name for a circuit node (`OUT`, `U1.mid`).
fn display_name(netlist: &Netlist, node: &str) -> Option<String> {
    let base = node.split('#').next().unwrap_or(node);
    netlist.nets.iter().find(|n| n.node.eq_ignore_ascii_case(base)).map(|n| n.name.clone())
}

fn element_path(netlist: &Netlist, element: &str) -> Option<String> {
    netlist
        .elements
        .iter()
        .find(|e| e.line.split_whitespace().next().is_some_and(|n| n.eq_ignore_ascii_case(element)))
        .map(|e| e.path.clone())
}

/// Named top-level nets, which are usually the ones worth plotting first.
fn default_probes(netlist: &Netlist) -> Vec<usize> {
    netlist
        .nets
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, n)| !n.name.contains('$') && !n.name.contains('.'))
        .map(|(i, _)| i)
        .take(6)
        .collect()
}

/// A transient run long enough to see a few cycles of the slowest periodic source,
/// sampled finely enough to show its edges. Without one, ten milliseconds.
pub fn default_tran(circuit: &Circuit) -> (f64, f64) {
    use circuiter_sim::parse::Waveform;
    let mut period: Option<f64> = None;
    for d in &circuit.devices {
        let (circuiter_sim::Element::Vsource { source, .. }
        | circuiter_sim::Element::Isource { source, .. }) = &d.element
        else {
            continue;
        };
        let p = match &source.spec.tran {
            Some(Waveform::Pulse { per: Some(p), .. }) => Some(*p),
            Some(Waveform::Sin { freq: Some(f), .. }) if *f > 0.0 => Some(1.0 / f),
            _ => None,
        };
        if let Some(p) = p {
            period = Some(period.map_or(p, |q: f64| q.max(p)));
        }
    }
    match period {
        Some(p) => (3.0 * p, p / 500.0),
        None => (10e-3, 10e-6),
    }
}

/// A value for display: `1.800 V`, `−12.50 mA`.
pub fn show(value: f64, unit: &str) -> String {
    let v = if value.abs() < 1e-15 { 0.0 } else { value };
    format_quantity(v, unit).replace('-', "−")
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::Project;
    use std::path::Path;

    fn example() -> (Design, CellId) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cmos_inverter");
        let project = Project::load(&root).unwrap();
        let design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
        (design, CellId::new("cmos_inverter", "testbench"))
    }

    #[test]
    fn the_example_simulates_end_to_end() {
        let (design, top) = example();
        let mut sim = Simulation::prepare(&design, &top).unwrap();
        assert!(sim.diagnostics.is_empty(), "{:?}", sim.diagnostics);
        sim.run(Analysis::Op);
        assert_eq!(sim.error, None);
        let vdd = sim.netlist.net("VDD").unwrap();
        let out = sim.netlist.net("OUT").unwrap();
        assert!((sim.voltage(vdd).unwrap() - 1.8).abs() < 1e-9);
        // The clock starts low, so the inverter's output is high.
        assert!(sim.voltage(out).unwrap() > 1.79);
        let (currents, power) = sim.element_op("CL").unwrap();
        assert_eq!(currents.len(), 2);
        assert!(power.abs() < 1e-15, "a capacitor at DC draws nothing");
        assert!(sim.power_under("U1").unwrap() >= 0.0);

        let (tstop, tstep) = default_tran(&sim.circuit);
        assert!((tstop - 30e-9).abs() < 1e-15, "three 100 MHz clock periods");
        sim.run(Analysis::Tran { tstop, tstep, power_on: false });
        assert_eq!(sim.error, None);
        let (time, v) = sim.waveform(out).unwrap();
        // The output falls once the clock rises (at 0) and rises again after 5 ns.
        let at = |t: f64| v[time.partition_point(|&x| x < t)];
        assert!(at(3e-9) < 0.05 && at(8e-9) > 1.75, "{} {}", at(3e-9), at(8e-9));
        assert_eq!(sim.probes.len(), 3, "IN, OUT and VDD are plotted first");
    }

    #[test]
    fn the_amplifier_example_is_biased_and_amplifies() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ce_amplifier");
        let project = Project::load(&root).unwrap();
        let design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
        let top = CellId::new("ce_amplifier", "amplifier");
        let mut sim = Simulation::prepare(&design, &top).unwrap();
        assert!(sim.diagnostics.is_empty(), "{:?}", sim.diagnostics);
        sim.run(Analysis::Op);
        let out = sim.netlist.net("OUT").unwrap();
        let vc = sim.voltage(out).unwrap();
        assert!(vc.abs() < 1e-6, "the output capacitor blocks DC: {vc}");
        let (_, power) = sim.element_op("D1").unwrap();
        assert!((0.015..0.025).contains(&power), "LED at about 10 mA, 2 V: {power} W");
        let (currents, _) = sim.element_op("Q1").unwrap();
        let ic = currents.iter().find(|(p, _)| p == "c").unwrap().1;
        assert!((1.0e-3..2.0e-3).contains(&ic), "collector current {ic}");

        sim.run(Analysis::Ac { fstart: 10.0, fstop: 10e6, per_decade: 10 });
        let (f, v) = sim.response(out).unwrap();
        let k = f.iter().position(|&x| (x - 1e3).abs() < 1.0).unwrap();
        // AC analysis drives the source with its `ac` value, 1 V, so this is the gain.
        let gain = v[k].db();
        eprintln!("gain at 1 kHz: {gain:.1} dB, phase {:.0}°", v[k].phase());
        assert!((38.0..48.0).contains(&gain), "gain {gain} dB");
        assert!((v[k].phase().abs() - 180.0).abs() < 15.0, "inverting: {}°", v[k].phase());
    }

    #[test]
    fn the_overstress_example_destroys_its_parts_in_order() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/overstress");
        let project = Project::load(&root).unwrap();
        let design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
        let mut sim = Simulation::prepare(&design, &CellId::new("overstress", "lab")).unwrap();
        assert!(sim.diagnostics.is_empty(), "{:?}", sim.diagnostics);

        // The operating point warns about what will burn.
        sim.run(Analysis::Op);
        let doomed: Vec<&str> = sim.assessments.iter().map(|a| a.path.as_str()).collect();
        assert!(doomed.contains(&"R3"), "{doomed:?}");

        sim.run(Analysis::Tran { tstop: 3.0, tstep: 5e-3, power_on: true });
        assert_eq!(sim.error, None);
        let events: Vec<(&str, f64)> =
            sim.events().iter().map(|e| (e.path.as_str(), e.time)).collect();
        for e in sim.events() {
            eprintln!("{}", e.describe());
        }
        assert_eq!(events.iter().map(|e| e.0).collect::<Vec<_>>(), ["D1", "F1", "R3"]);
        assert!(events[0].1 < 1e-3, "the LED dies at power-on: {}", events[0].1);
        assert!((0.02..0.05).contains(&events[1].1), "fuse at {}", events[1].1);
        assert!((1.5..2.5).contains(&events[2].1), "resistor at {}", events[2].1);
        assert!(sim.failure("R3").is_some());
    }

    fn load(name: &str, top: &str) -> (Design, CellId) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name);
        let project = Project::load(&root).unwrap();
        let design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
        (design, CellId::new(name, top))
    }

    #[test]
    fn the_half_adder_adds() {
        let (design, top) = load("logic", "half_adder");
        let mut sim = Simulation::prepare(&design, &top).unwrap();
        assert!(sim.diagnostics.is_empty(), "{:?}", sim.diagnostics);
        let (tstop, tstep) = default_tran(&sim.circuit);
        sim.run(Analysis::Tran { tstop, tstep, power_on: true });
        assert_eq!(sim.error, None);
        let wave = |name: &str| sim.waveform(sim.netlist.net(name).unwrap()).unwrap();
        let (t, a) = wave("A");
        let (_, b) = wave("B");
        let (_, sum) = wave("SUM");
        let (_, carry) = wave("CARRY");
        let at = |v: &[f64], when: f64| v[t.partition_point(|&x| x < when).min(v.len() - 1)];
        // Sample mid-way through each quarter of B's period, after things settle.
        for k in 0..8 {
            let when = 2e-6 + 0.25e-6 + k as f64 * 0.5e-6;
            let (x, y) = (at(&a, when) > 2.5, at(&b, when) > 2.5);
            assert_eq!(at(&sum, when) > 2.5, x != y, "SUM at {when:e}");
            assert_eq!(at(&carry, when) > 2.5, x && y, "CARRY at {when:e}");
        }
    }

    #[test]
    fn every_example_runs_from_power_on() {
        for (name, top) in [
            ("cmos_inverter", "testbench"),
            ("ce_amplifier", "amplifier"),
            ("overstress", "lab"),
            ("logic", "half_adder"),
        ] {
            let (design, id) = load(name, top);
            let mut sim = Simulation::prepare(&design, &id).unwrap();
            let (tstop, tstep) = default_tran(&sim.circuit);
            sim.run(Analysis::Tran { tstop, tstep, power_on: true });
            assert_eq!(sim.error, None, "{name}");
        }
    }

    #[test]
    fn background_runs_finish_and_can_be_cancelled() {
        let (design, top) = example();
        let mut sim = Simulation::prepare(&design, &top).unwrap();
        sim.start(Analysis::Tran { tstop: 30e-9, tstep: 10e-12, power_on: true });
        while sim.poll() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(matches!(sim.results, Some(Results::Tran { .. })), "{:?}", sim.error);

        sim.start(Analysis::Tran { tstop: 1e-3, tstep: 1e-12, power_on: true });
        sim.cancel();
        while sim.poll() {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(sim.error.as_deref(), Some("cancelled"));
    }

    #[test]
    fn ac_sweeps_report_complex_responses() {
        let (design, top) = example();
        let mut sim = Simulation::prepare(&design, &top).unwrap();
        sim.run(Analysis::Ac { fstart: 1e3, fstop: 1e9, per_decade: 5 });
        assert_eq!(sim.error, None);
        let out = sim.netlist.net("OUT").unwrap();
        let (f, v) = sim.response(out).unwrap();
        assert_eq!(f.len(), 31);
        // The clock source has no AC value, so nothing is driven.
        assert!(v.iter().all(|x| x.abs() < 1e-9));
    }

    #[test]
    fn without_ground_the_circuit_still_simulates_against_leakage() {
        let (mut design, top) = example();
        let s = design.cell_mut(&top).unwrap().schematic.as_mut().unwrap();
        s.instances.retain(|i| i.cell.cell != "gnd");
        let mut sim = Simulation::prepare(&design, &top).unwrap();
        assert!(!sim.has_errors());
        assert!(sim.diagnostics.iter().any(|d| d.message.contains("no ground")));
        sim.run(Analysis::Op);
        assert_eq!(sim.error, None);
        // Only differences mean anything now, and the supply still sets them.
        let supply = sim.netlist.elements.iter().find(|e| e.path == "VDD").unwrap();
        let net = |port: &str| supply.ports.iter().find(|(p, _)| p == port).unwrap().1;
        let (p, n) = (sim.voltage(net("p")).unwrap(), sim.voltage(net("n")).unwrap());
        assert!((p - n - 1.8).abs() < 1e-6, "{p} {n}");
    }
}

#[cfg(test)]
mod digital_tests {
    use super::*;
    use circuiter_model::Project;
    use circuiter_riscv::iss;

    fn riscv() -> (Design, PathBuf) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/riscv");
        let project = Project::load(&root).unwrap();
        let design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
        (design, root)
    }

    /// Runs the example processor on `program` and returns its run.
    fn run_cpu(program: &str) -> (LogicRun, Simulation) {
        let (mut design, root) = riscv();
        let cpu = CellId::new("riscv", "cpu");
        let def = design.cell_mut(&cpu).unwrap();
        for inst in def.schematic.as_mut().unwrap().instances.iter_mut() {
            if inst.name == "IMEM" || inst.name == "DMEM" {
                inst.params.insert("program".into(), program.into());
            }
        }
        let mut sim = Simulation::prepare_in(&design, &cpu, &root).unwrap();
        assert!(sim.digital, "the CPU is all digital");
        assert!(!sim.has_errors(), "{:#?}", sim.diagnostics);
        sim.run(Analysis::Op);
        assert_eq!(sim.error, None);
        let Some(Results::Logic(run)) = sim.results.clone() else { panic!("no digital run") };
        (run, sim)
    }

    /// The registers and data memory at the end must match the reference simulator.
    fn check_against_iss(program: &str, a0: u32) {
        let (run, _) = run_cpu(program);
        assert!(run.halted, "{program}: the program ran to its ECALL");
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/riscv");
        let p = crate::digital::load_program(&root.join(program)).unwrap();
        let mut golden = iss::Cpu::new(&p, 1024);
        golden.run(1_000_000).unwrap();
        assert_eq!(golden.regs[10], a0, "{program}: the reference result");
        let regs = &run.memories["RF"];
        for (k, (hw, sw)) in regs.iter().zip(golden.regs).enumerate().skip(1) {
            // Registers the program never wrote stay unknown on the hardware.
            if let Some(v) = hw.known() {
                assert_eq!(v as u32, sw, "{program}: x{k}");
            } else {
                assert_eq!(sw, 0, "{program}: x{k} unwritten on the CPU but {sw} in the reference");
            }
        }
        let mem = &run.memories["DMEM"];
        for (k, (hw, sw)) in mem.iter().zip(&golden.data).enumerate() {
            assert_eq!(hw.known().map(|v| v as u32), Some(*sw), "{program}: data word {k}");
        }
    }

    #[test]
    fn the_cpu_runs_assembly_like_the_reference() {
        check_against_iss("programs/fib.s", 89);
        let (run, sim) = run_cpu("programs/fib.s");
        let fib: Vec<u64> = run.memories["DMEM"][..12].iter().map(|v| v.known().unwrap()).collect();
        assert_eq!(fib, [0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89]);
        // The program counter's history is there to plot.
        let pc = sim.netlist.net("pc").unwrap();
        assert!(run.history[pc].len() > 50);
    }

    #[test]
    fn the_cpu_runs_c() {
        if std::process::Command::new("clang").arg("--version").output().is_err() {
            eprintln!("clang isn't installed; skipping");
            return;
        }
        check_against_iss("programs/sum.c", 285);
    }

    #[test]
    #[ignore = "builds Rust's core for RV32I the first time (slow)"]
    fn the_cpu_runs_rust() {
        check_against_iss("programs/collatz.rs", 111);
    }
}

#[cfg(test)]
mod hdl_tests {
    use super::*;
    use crate::workspace::Workspace;

    fn riscv_copy() -> tempfile::TempDir {
        fn copy(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let p = entry.unwrap().path();
                let dest = to.join(p.file_name().unwrap());
                if p.is_dir() {
                    copy(&p, &dest)
                } else {
                    std::fs::copy(&p, &dest).map(drop).unwrap()
                }
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        copy(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/riscv"), tmp.path());
        tmp
    }

    /// The control unit written in SystemVerilog, made into a part and swapped into the
    /// processor, runs the program exactly as the built-in one does.
    #[test]
    fn a_control_unit_written_in_systemverilog_runs_the_cpu() {
        let tmp = riscv_copy();
        let mut ws = Workspace::open(tmp.path()).unwrap();
        let def =
            crate::digital::cell_from_module(tmp.path(), Path::new("programs/control.sv"), None)
                .unwrap();
        let ports: Vec<(&str, u32)> =
            def.cell.ports.iter().map(|p| (p.name.as_str(), p.width)).collect();
        assert_eq!(ports[..4], [("instr", 32), ("reg_write", 1), ("alu_a", 2), ("alu_b", 1)]);
        let id = ws.add_hdl_part(def).unwrap();
        assert_eq!(ws.design.cell(&id).unwrap().folder, "hdl");
        // Swap CTRL's cell for the Verilog one.
        let cpu = CellId::new("riscv", "cpu");
        ws.open_cell(cpu.clone());
        let ctrl = ws.schematic().unwrap().instances.iter().position(|i| i.name == "CTRL").unwrap();
        ws.edit(|_, _, d| {
            d.schematic.as_mut().unwrap().instances[ctrl].cell =
                circuiter_model::CellRef::new(None, "control");
            Ok(())
        })
        .unwrap();
        assert!(ws.problems().iter().all(|(s, _)| *s != Severity::Error), "{:?}", ws.problems());
        let mut sim = Simulation::prepare_in(&ws.design, &cpu, tmp.path()).unwrap();
        assert!(!sim.has_errors(), "{:#?}", sim.diagnostics);
        sim.run(Analysis::Op);
        assert_eq!(sim.error, None);
        let Some(Results::Logic(run)) = &sim.results else { panic!("no run") };
        assert!(run.halted, "the program ran to its ECALL");
        assert_eq!(run.memories["RF"][10].known(), Some(89));
        let fib: Vec<u64> = run.memories["DMEM"][..12].iter().map(|v| v.known().unwrap()).collect();
        assert_eq!(fib, [0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89]);
    }
}
