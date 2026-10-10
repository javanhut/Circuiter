//! Running simulations on the design being edited, and mapping results back onto the
//! sheets: net voltages, device currents and power, waveforms.

use circuiter_model::check::{Diagnostic, Severity};
use circuiter_model::units::format_quantity;
use circuiter_model::{CellId, Design};
use circuiter_netlist::Netlist;
use circuiter_physics::{AMBIENT, Assessment, Event, Monitor};
use circuiter_sim::{
    AcResult, Circuit, Complex, Control, OpResult, Options, Problem, Spacing, TranResult,
};
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
    Tran { result: TranResult, tstop: f64, tstep: f64 },
    Ac { result: AcResult, fstart: f64, fstop: f64, per_decade: usize },
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
}

impl Simulation {
    /// Flattens `top` and builds the circuit. Fails only if there is nothing to
    /// simulate; circuit problems are reported in [`Simulation::diagnostics`].
    pub fn prepare(design: &Design, top: &CellId) -> Result<Simulation, String> {
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
        })
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
        let outcome = execute(&self.circuit, &self.netlist, analysis, Options::default());
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
        std::thread::spawn(move || {
            let _ = tx.send(execute(&circuit, &netlist, analysis, options));
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
