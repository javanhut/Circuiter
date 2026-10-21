//! Running simulations on the design being edited, and mapping results back onto the
//! sheets: net voltages, device currents and power, waveforms.

use crate::digital::{self, LogicRun};
use circuiter_model::check::{Diagnostic, Severity};
use circuiter_model::units::format_quantity;
use circuiter_model::{CellId, Design};
use circuiter_netlist::Netlist;
use circuiter_physics::{AMBIENT, Assessment, Case, Event, Monitor};
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
    /// Both simulators together: the analog side's waveforms and the digital side's
    /// history.
    Mixed {
        result: TranResult,
        logic: LogicRun,
        tstop: f64,
        tstep: f64,
    },
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
    /// The case the circuit runs in, if its board has one: its air warms with the parts.
    pub case: Option<Case>,
    /// After an operating point: parts that would overheat if it held.
    pub assessments: Vec<Assessment>,
    /// Every part is digital: this runs in the digital simulator.
    pub digital: bool,
    /// Analog and digital parts together: both simulators, joined.
    pub mixed: bool,
    /// The project directory, where programs for memories are found.
    pub root: PathBuf,
    /// Register files made of gates, found once: see [`register_nets`].
    gate_registers: Vec<(String, [Option<usize>; 32])>,
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
            let gate_registers = register_nets(&netlist);
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
                case: case_of(design, top),
                assessments: Vec::new(),
                digital: true,
                mixed: false,
                root: root.to_path_buf(),
                gate_registers,
            });
        }
        let mixed = Netlist::is_mixed(design, top);
        let netlist =
            if mixed { Netlist::build_mixed(design, top)? } else { Netlist::build(design, top)? };
        let mut diagnostics = netlist.diagnostics.clone();
        let location = format!("{top}/simulation");
        let mut circuit = None;
        if mixed && !diagnostics.iter().any(|d| d.severity == Severity::Error) {
            // The analog half with its converters, built now so problems show early.
            match crate::cosim::prepare(&netlist, root, &[]) {
                Ok(p) => circuit = Some(p.circuit),
                Err(message) => diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    location: location.clone(),
                    message,
                }),
            }
        }
        let circuit = match circuit {
            Some(c) => c,
            None => Circuit::parse(&netlist.to_spice()).map_err(|e| format!("netlist: {e}"))?,
        };
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
        let gate_registers = register_nets(&netlist);
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
            case: case_of(design, top),
            assessments: Vec::new(),
            digital: false,
            mixed,
            root: root.to_path_buf(),
            gate_registers,
        })
    }

    /// What multimeter `m` reads at the operating point, or at the end of a run.
    pub fn meter(&self, m: &crate::live::Meter) -> Option<crate::live::Reading> {
        let volts = |net: usize| match &self.results {
            Some(Results::Op(_)) => self.voltage(net),
            _ => self.waveform(net).and_then(|(_, v)| v.last().copied()),
        };
        let amps = |name: &str| match &self.results {
            Some(Results::Op(op)) => op.device(name).and_then(|d| d.currents.first().copied()),
            Some(Results::Tran { result, .. } | Results::Mixed { result, .. }) => {
                result.current(&self.circuit, name).and_then(|c| c.last().copied())
            }
            _ => None,
        };
        m.read(&volts, &amps)
    }

    /// The digital side of a digital or mixed run.
    pub fn logic_run(&self) -> Option<&LogicRun> {
        match &self.results {
            Some(Results::Logic(run) | Results::Mixed { logic: run, .. }) => Some(run),
            _ => None,
        }
    }

    /// The digital history of flat net `net`, after a digital or mixed run.
    pub fn logic(&self, net: usize) -> Option<&[(circuiter_logic::Time, circuiter_logic::Value)]> {
        self.logic_run()?.history.get(net).map(Vec::as_slice)
    }

    /// The simulator's part at instance path `path` (`regfile`, `rom`…).
    pub fn model(&self, path: &str) -> Option<&str> {
        self.netlist.digital.iter().find(|e| e.path == path).map(|e| e.model.as_str())
    }

    /// The design has memories or register files whose contents a run leaves behind.
    pub fn has_memories(&self) -> bool {
        !self.gate_registers.is_empty()
            || self
                .netlist
                .digital
                .iter()
                .any(|e| matches!(e.model.as_str(), "regfile" | "rom" | "dmem"))
    }

    /// The register files at the end of a digital or mixed run, by instance path: a
    /// register file part's contents, or the values on the nets of one made of gates.
    pub fn registers(&self) -> Vec<(String, Vec<circuiter_logic::Value>)> {
        use circuiter_logic::Value;
        let Some(run) = self.logic_run() else { return Vec::new() };
        let parts = run
            .memories
            .iter()
            .filter(|(path, _)| self.model(path) == Some("regfile"))
            .map(|(path, regs)| (path.clone(), regs.clone()));
        let gates = self.gate_registers.iter().map(|(path, nets)| {
            let regs = nets
                .iter()
                .map(|n| match n {
                    // Flip-flops never loaded hold nothing known.
                    Some(n) => run.last(*n).unwrap_or(Value::unknown(32)),
                    // Only x0 can be missing: it's wired to zero.
                    None => Value::new(32, 0),
                })
                .collect();
            (path.clone(), regs)
        });
        parts.chain(gates).collect()
    }

    /// What a program left in a0 (x10), the register main's return value goes to: from
    /// the first 32-register file, after a digital or mixed run.
    pub fn program_result(&self) -> Option<circuiter_logic::Value> {
        self.registers().into_iter().find(|(_, regs)| regs.len() == 32).map(|(_, regs)| regs[10])
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
        } else if self.mixed {
            execute_mixed(&self.netlist, &self.root, analysis, Options::default(), self.case)
        } else {
            execute(&self.circuit, &self.netlist, analysis, Options::default(), self.case)
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
        let (is_digital, is_mixed, root, c, case) =
            (self.digital, self.mixed, self.root.clone(), control.clone(), self.case);
        std::thread::spawn(move || {
            let outcome = if is_digital {
                execute_digital(&netlist, &root, analysis, &c)
            } else if is_mixed {
                execute_mixed(&netlist, &root, analysis, options, case)
            } else {
                execute(&circuit, &netlist, analysis, options, case)
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
                    if let Ok(m) = self::monitor(&self.netlist, &self.circuit, self.case) {
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
            Some(Results::Tran { result, .. } | Results::Mixed { result, .. }) => {
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

/// What Run will do: how long, how finely, and whether the design ends the run itself
/// (a processor's ECALL) before then.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunPlan {
    pub tstop: f64,
    pub tstep: f64,
    pub until_halt: bool,
}

/// The run plan for `top`, from its netlist alone: nothing is built or compiled, so this
/// is cheap enough to show in the toolbar.
pub fn plan(design: &Design, top: &CellId) -> Option<RunPlan> {
    if Netlist::is_digital(design, top) {
        let n = Netlist::build_digital(design, top).ok()?;
        let tstop = digital::default_stop(&n);
        let until_halt = n.digital.iter().any(|e| e.model == "stop");
        return Some(RunPlan { tstop, tstep: tstop / 1000.0, until_halt });
    }
    let mixed = Netlist::is_mixed(design, top);
    let n = if mixed {
        Netlist::build_mixed(design, top).ok()?
    } else {
        Netlist::build(design, top).ok()?
    };
    let circuit = Circuit::parse(&n.to_spice()).ok()?;
    let (tstop, tstep) = if mixed { mixed_run(&circuit, &n) } else { default_tran(&circuit) };
    Some(RunPlan { tstop, tstep, until_halt: false })
}

/// The physics engine's watch over a run, in the case if there is one.
pub fn monitor(
    netlist: &Netlist,
    circuit: &Circuit,
    case: Option<Case>,
) -> Result<Monitor, String> {
    let m = Monitor::new(netlist, circuit, case.map_or(AMBIENT, |c| c.air))?;
    Ok(match case {
        Some(c) => m.in_case(c),
        None => m,
    })
}

/// The case `top`'s board is in, thermally, when simulations are to run inside it.
pub fn case_of(design: &Design, top: &CellId) -> Option<Case> {
    let board = design.cell(top)?.board.as_ref()?;
    let e = board.enclosure.as_ref().filter(|e| e.simulate)?;
    let layout = circuiter_pcb::layout::Layout::new(design, top, board).ok()?;
    let built = circuiter_pcb::board3d::build(design, &layout, &[]);
    let (r, c) = match &e.part {
        // A case of your own: its size from its body.
        Some(part) => {
            let (_, def) = design.resolve(part, &top.library)?;
            let body = circuiter_3d::assembly::body(def, &Default::default()).ok()??;
            let (lo, hi) = body.bounds()?;
            let w = e.wall as f32;
            let inner = ([lo[0] + w, lo[1] + w, lo[2] + w], [hi[0] - w, hi[1] - w, hi[2] - w]);
            circuiter_pcb::enclosure::thermal_of((lo, hi), inner, e.wall, e)
        }
        None => {
            let made = circuiter_pcb::enclosure::make(&layout, &built, e).ok()?;
            circuiter_pcb::enclosure::thermal(&made, e)
        }
    };
    Some(Case::new(r, c, e.room))
}

/// How long and how finely to run a mixed design: three periods of the slowest analog
/// source, or 64 cycles of the slowest clock, whichever is longer.
fn mixed_run(circuit: &Circuit, netlist: &Netlist) -> (f64, f64) {
    let (ta, sa) = default_tran(circuit);
    let clocks = digital::clock_periods(netlist);
    match clocks.iter().copied().reduce(f64::max) {
        Some(slow) => {
            let fast = clocks.iter().copied().fold(f64::INFINITY, f64::min);
            let periodic = circuit.devices.iter().any(|d| {
                matches!(&d.element, circuiter_sim::Element::Vsource { source, .. } if source.spec.tran.is_some())
            });
            let tstop = if periodic { ta.max(64.0 * slow) } else { 64.0 * slow };
            (tstop, sa.min(fast / 20.0).min(tstop / 500.0))
        }
        None => (ta, sa),
    }
}

/// A mixed run: the analog transient with the digital simulator joined to it.
fn execute_mixed(
    netlist: &Netlist,
    root: &Path,
    analysis: Analysis,
    o: Options,
    case: Option<Case>,
) -> Outcome {
    let prepared = crate::cosim::prepare(netlist, root, &[])?;
    let (tstop, tstep, power_on) = match analysis {
        Analysis::Tran { tstop, tstep, power_on } => (tstop, tstep, power_on),
        _ => {
            let (a, b) = mixed_run(&prepared.circuit, netlist);
            (a, b, true)
        }
    };
    let crate::cosim::Prepared { circuit, inputs, mut cosim, .. } = prepared;
    cosim.monitor = monitor(netlist, &circuit, case).ok();
    let o = Options { power_on, inputs: Some(inputs), ..o };
    let mut run = circuiter_sim::Transient::start(&circuit, tstep, tstop, &o, &mut cosim)
        .map_err(|e| e.message)?;
    run.advance(tstop, &mut cosim).map_err(|e| e.message)?;
    let end = run.time();
    let logic = cosim.logic_run(tstop, end);
    let monitor = cosim.monitor.take();
    Ok((Results::Mixed { result: run.result, logic, tstop, tstep }, monitor))
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

fn execute(
    c: &Circuit,
    netlist: &Netlist,
    analysis: Analysis,
    o: Options,
    case: Option<Case>,
) -> Outcome {
    let r = match analysis {
        Analysis::Op => circuiter_sim::op(c, &o).map(|op| (Results::Op(op), None)),
        Analysis::Tran { tstop, tstep, power_on } => {
            // The physics engine watches every transient run: parts heat, and fail.
            let mut m = monitor(netlist, c, case)?;
            let o = Options { power_on, ..o };
            circuiter_sim::tran_observed(c, tstop, tstep, &o, &mut m)
                .map(|(result, _)| (Results::Tran { result, tstop, tstep }, Some(m)))
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

/// Register files made of gates, which no part holds as an array: an instance with
/// 32-bit nets `x1` to `x31` (and `x0`, if it has one), by its path, with each
/// register's flat net.
fn register_nets(netlist: &Netlist) -> Vec<(String, [Option<usize>; 32])> {
    let mut found: std::collections::BTreeMap<&str, [Option<usize>; 32]> = Default::default();
    for (i, n) in netlist.nets.iter().enumerate() {
        let Some((path, reg)) = n.name.rsplit_once('.') else { continue };
        let Some(k) = reg.strip_prefix('x').and_then(|k| k.parse::<usize>().ok()) else {
            continue;
        };
        if k < 32 && n.width == 32 && reg == format!("x{k}") {
            found.entry(path).or_default()[k] = Some(i);
        }
    }
    found
        .into_iter()
        .filter(|(_, regs)| regs[1..].iter().all(Option::is_some))
        .map(|(path, regs)| (path.to_owned(), regs))
        .collect()
}

/// Named top-level nets, which are usually the ones worth plotting first.
pub fn default_probes(netlist: &Netlist) -> Vec<usize> {
    // Constants are flat lines: not worth a lane.
    let constant: std::collections::BTreeSet<usize> = netlist
        .digital
        .iter()
        .filter(|e| e.model == "const")
        .flat_map(|e| e.ports.iter().map(|(_, n)| *n))
        .collect();
    // In an all-digital design VDD is only a constant 1.
    let all_digital = netlist.elements.is_empty() && !netlist.digital.is_empty();
    netlist
        .nets
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, n)| !n.name.contains('$') && !n.name.contains('.'))
        .filter(|(i, n)| !constant.contains(i) && !(all_digital && n.name == "VDD"))
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

    /// A port named as an inverted output (`Q'`) works through every view: the
    /// inverter's output renamed so still drives the testbench's OUT.
    #[test]
    fn a_primed_port_simulates() {
        let (mut design, top) = example();
        let inv = CellId::new("cmos_inverter", "inverter");
        crate::interface::rename_port(design.cell_mut(&inv).unwrap(), "out", "Q'").unwrap();
        let mut sim = Simulation::prepare(&design, &top).unwrap();
        assert!(sim.diagnostics.is_empty(), "{:?}", sim.diagnostics);
        sim.run(Analysis::Op);
        assert_eq!(sim.error, None);
        let out = sim.netlist.net("OUT").unwrap();
        assert!(sim.voltage(out).unwrap() > 1.79, "the clock starts low, so Q' is high");
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

    /// The power supply in a small sealed case: the regulator's watt or so warms the air
    /// inside, which the run tracks; out of the case the air is the room's.
    #[test]
    fn a_circuit_in_its_case_warms_the_air() {
        let (mut design, top) = load("analog_lab", "power_supply");
        let mut board = circuiter_pcb::sync::new_board(&design, &top).unwrap();
        board.enclosure = Some(Default::default());
        design.cell_mut(&top).unwrap().board = Some(board);
        let mut sim = Simulation::prepare(&design, &top).unwrap();
        let case = sim.case.expect("the board's case");
        assert!(case.resistance > 10.0 && (case.air - 25.0).abs() < 1e-9);
        sim.run(Analysis::Tran { tstop: 2.0, tstep: 1e-3, power_on: true });
        assert_eq!(sim.error, None);
        let m = sim.monitor.as_ref().unwrap();
        let air = m.case.unwrap();
        assert!(air.peak > 25.05, "the air warms: {}", air.peak);
        assert_eq!(m.air.len(), m.temperatures.len());
        // Told not to simulate in it, the room is the parts' air.
        design
            .cell_mut(&top)
            .unwrap()
            .board
            .as_mut()
            .unwrap()
            .enclosure
            .as_mut()
            .unwrap()
            .simulate = false;
        assert!(Simulation::prepare(&design, &top).unwrap().case.is_none());
    }

    /// Every circuit of the analog lab example runs from power-on and does its job.
    #[test]
    fn the_analog_lab_circuits_work() {
        let run = |cell: &str, tstop: f64, tstep: f64| {
            let (design, top) = load("analog_lab", cell);
            let mut sim = Simulation::prepare(&design, &top).unwrap();
            assert!(sim.diagnostics.is_empty(), "{cell}: {:?}", sim.diagnostics);
            sim.run(Analysis::Tran { tstop, tstep, power_on: true });
            assert_eq!(sim.error, None, "{cell}");
            let Some(Results::Tran { result, .. }) = sim.results.clone() else { panic!("{cell}") };
            (sim, result)
        };
        let wave = |sim: &Simulation, r: &circuiter_sim::TranResult, net: &str| {
            let node = &sim.netlist.nets[sim.netlist.net(net).unwrap()].node;
            r.voltage(&sim.circuit, node).unwrap()
        };
        let span = |x: &[f64]| {
            let half = &x[x.len() / 2..];
            let hi = half.iter().copied().fold(f64::MIN, f64::max);
            let lo = half.iter().copied().fold(f64::MAX, f64::min);
            (lo, hi)
        };

        let (sim, r) = run("amplifier", 3e-3, 2e-6);
        let (lo, hi) = span(&wave(&sim, &r, "out"));
        assert!((hi - 5.5).abs() < 0.15 && (lo + 5.5).abs() < 0.15, "gain 11: {lo}..{hi}");

        let (sim, r) = run("blinker", 3.0, 1e-3);
        let out = wave(&sim, &r, "out");
        let rises = out.windows(2).filter(|w| w[0] < 4.0 && w[1] >= 4.0).count();
        assert!((3..=6).contains(&rises), "about 1.5 Hz for 3 s: {rises} rising edges");
        let (lo, hi) = span(&wave(&sim, &r, "led"));
        assert!(hi > 1.7 && lo < 0.5, "the LED lights and goes out: {lo}..{hi}");

        let (sim, r) = run("power_supply", 0.3, 50e-6);
        let (lo, hi) = span(&wave(&sim, &r, "5v"));
        assert!((lo - 5.0).abs() < 0.05 && (hi - 5.0).abs() < 0.05, "5 V: {lo}..{hi}");
        let (lo, _) = span(&wave(&sim, &r, "raw"));
        assert!(lo > 7.0, "the reservoir stays above the dropout: {lo}");

        let (sim, r) = run("light_switch", 1e-3, 1e-6);
        let out = wave(&sim, &r, "out");
        assert!(*out.last().unwrap() > 4.5, "1 lux is dark: the comparator lets go");

        let (sim, r) = run("motor_driver", 0.2, 100e-6);
        let drain = wave(&sim, &r, "drain");
        assert!(*drain.last().unwrap() > 11.9, "switch open: the motor is off");

        let (sim, r) = run("relay_driver", 20e-3, 20e-6);
        let lamp = wave(&sim, &r, "lamp");
        assert!(lamp.last().unwrap().abs() < 0.01, "button released: the lamp is off");
        let standby = wave(&sim, &r, "led");
        assert!(*standby.last().unwrap() > 1.7, "and the standby LED on");
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
    use std::collections::BTreeSet;

    fn riscv() -> (Design, PathBuf) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/riscv");
        let project = Project::load(&root).unwrap();
        let design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
        (design, root)
    }

    /// Runs the example processor on `program` and returns its run.
    fn run_cpu(program: &str) -> (LogicRun, Simulation) {
        run_processor("cpu", program)
    }

    /// Runs processor `cell` of the example on `program`.
    fn run_processor(cell: &str, program: &str) -> (LogicRun, Simulation) {
        let (mut design, root) = riscv();
        let cpu = CellId::new("riscv", cell);
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

    /// The processor made of gates: no part in it is more than a gate, a flip-flop's
    /// NAND latch, wiring or a memory array, and it runs the program as the reference does.
    #[test]
    fn the_gate_level_cpu_runs_like_the_reference() {
        let (run, sim) = run_processor("cpu_gates", "programs/fib.s");
        assert!(sim.diagnostics.is_empty(), "{:#?}", sim.diagnostics);
        let models: BTreeSet<&str> = sim.netlist.digital.iter().map(|e| e.model.as_str()).collect();
        let allowed = [
            "not", "and", "or", "nand", "xor", // gates
            "slice", "concat", "extend", "const", // wiring
            "rom", "dmem", // memories
            "clock", "pulse", "stop", // clock, reset and the end of the program
        ];
        assert!(models.iter().all(|m| allowed.contains(m)), "{models:?}");
        let gates = sim.netlist.digital.iter().filter(|e| e.model == "nand").count();
        assert_eq!(gates, 8 * (31 * 32 + 32), "NAND latches of every flip-flop");
        assert!(run.halted, "the program ran to its ECALL");
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/riscv");
        let p = crate::digital::load_program(&root.join("programs/fib.s")).unwrap();
        let mut golden = iss::Cpu::new(&p, 1024);
        golden.run(1_000_000).unwrap();
        for k in 1..32 {
            let net = sim.netlist.net(&format!("RF.x{k}")).unwrap_or_else(|| panic!("x{k}"));
            match run.last(net).and_then(|v| v.known()) {
                Some(v) => assert_eq!(v as u32, golden.regs[k], "x{k}"),
                None => assert_eq!(golden.regs[k], 0, "x{k} unwritten on the gates"),
            }
        }
        // With no register file part, the registers shown are read from its nets.
        let regs = sim.registers();
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].0, "RF");
        assert_eq!(sim.program_result().and_then(|v| v.known()), Some(u64::from(golden.regs[10])));
        let fib: Vec<u64> = run.memories["DMEM"][..12].iter().map(|v| v.known().unwrap()).collect();
        assert_eq!(fib, [0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89]);
    }

    #[test]
    fn the_cpu_runs_c() {
        if std::process::Command::new("clang").arg("--version").output().is_err() {
            eprintln!("clang isn't installed; skipping");
            return;
        }
        check_against_iss("programs/sum.c", 285);
    }

    /// The first build compiles Rust's `core` for RV32I (several seconds); later ones
    /// reuse it.
    #[test]
    fn the_cpu_runs_rust() {
        let sysroot = std::process::Command::new("rustc").args(["--print", "sysroot"]).output();
        let Ok(out) = sysroot else {
            eprintln!("rustc isn't installed; skipping");
            return;
        };
        let lib = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()).join("lib/rustlib");
        let usable = lib.join("riscv32i-unknown-none-elf").is_dir()
            || lib.join("src/rust/library/core").is_dir();
        if !usable {
            eprintln!("neither the riscv32i target nor Rust's sources are installed; skipping");
            return;
        }
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

/// Building sheets in tests: parts placed apart, each pin on a short wire ending in a
/// label for its net.
#[cfg(test)]
pub mod sheet_builder {
    use circuiter_model::geom::{Length, Point, Rotation, Transform};
    use circuiter_model::schematic::{Instance, NetLabel, Schematic, Wire};
    use circuiter_model::symbol::PinDirection;
    use circuiter_model::{CellDef, CellId, Design, Library};

    /// `(name, cell, params, [(port, net)])` for each part.
    pub type Part<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], &'a [(&'a str, &'a str)]);

    /// A design with the standard library and a project `t` whose cell `top` holds
    /// `parts`.
    pub fn design(parts: &[Part<'_>]) -> (Design, CellId) {
        let mut design = Design::new();
        design.add_library(circuiter_stdlib::library()).unwrap();
        design.add_library(Library::new("t")).unwrap();
        let mut s = Schematic::default();
        for (k, (name, cell, params, nets)) in parts.iter().enumerate() {
            let r: circuiter_model::CellRef = cell.parse().unwrap();
            let def = design.resolve(&r, "t").unwrap().1.clone();
            let at = Point::mm(100.0 * k as f64, 0.0);
            let placement = Transform { position: at, rotation: Rotation::R0, mirror: false };
            let inst = Instance {
                fields: Default::default(),
                name: name.to_string(),
                cell: r,
                placement,
                params: params.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
                view: None,
            };
            let sym = def.symbol.as_ref().unwrap();
            for (port, net) in nets.iter() {
                let pin = sym.pins.iter().find(|p| p.port == *port).unwrap();
                let (dx, dy) = match pin.direction {
                    PinDirection::Left => (1, 0),
                    PinDirection::Right => (-1, 0),
                    PinDirection::Up => (0, 1),
                    PinDirection::Down => (0, -1),
                };
                let p = placement.apply(pin.at);
                let g = Length::from_mm(5.08).nm();
                let end = Point::new(Length(p.x.nm() + dx * g), Length(p.y.nm() + dy * g));
                s.wires.push(Wire { points: vec![p, end] });
                s.labels.push(NetLabel { global: false, net: net.to_string(), at: end });
            }
            s.instances.push(inst);
        }
        let top = CellDef { name: "top".into(), schematic: Some(s), ..CellDef::default() };
        design.library_mut("t").unwrap().insert(top);
        (design, CellId::new("t", "top"))
    }
}

#[cfg(test)]
mod mixed_tests {
    use super::sheet_builder::design;
    use super::*;

    fn run(design: &Design, top: &CellId, tstop: f64, tstep: f64) -> Simulation {
        let mut sim = Simulation::prepare_in(design, top, Path::new(".")).unwrap();
        assert!(sim.mixed, "both simulators");
        assert!(!sim.has_errors(), "{:#?}", sim.diagnostics);
        sim.run(Analysis::Tran { tstop, tstep, power_on: true });
        assert_eq!(sim.error, None);
        sim
    }

    /// A digital counter lights an LED through a resistor: digital into analog.
    #[test]
    fn a_counter_blinks_an_led() {
        let (d, top) = design(&[
            ("CLK", "std:clock", &[("frequency", "1k")], &[("p", "clk"), ("n", "GND")]),
            ("K1", "std:const", &[("value", "1")], &[("y", "one")]),
            // Reset synchronously: across the first clock edge, at 0.5 ms.
            ("RST", "std:reset", &[("length", "600u")], &[("rst", "rst")]),
            (
                "CNT",
                "std:counter",
                &[("width", "1")],
                &[("clk", "clk"), ("rst", "rst"), ("en", "one"), ("q", "drive")],
            ),
            ("R1", "std:resistor", &[("R", "220")], &[("a", "drive"), ("b", "anode")]),
            ("D1", "std:led", &[], &[("a", "anode"), ("k", "GND")]),
        ]);
        let sim = run(&d, &top, 6e-3, 10e-6);
        let Some(Results::Mixed { result, logic, .. }) = &sim.results else { panic!() };
        // The counter starts unknown: it never resets here, so give it a clean start.
        let _ = logic;
        let anode = sim.netlist.net("anode").unwrap();
        let (t, v) = sim.waveform(anode).unwrap();
        let drive = sim.netlist.net("drive").unwrap();
        let q = |time: f64| {
            let h = sim.logic(drive).unwrap();
            let i = h.partition_point(|(ht, _)| circuiter_logic::seconds(*ht) <= time);
            h[i.saturating_sub(1)].1.known()
        };
        // Wherever the counter's output is known and has settled, the LED follows it.
        let mut lit = 0;
        let mut dark = 0;
        for (time, volts) in t.iter().zip(&v) {
            match q(*time - 50e-6).zip(q(*time)) {
                Some((1, 1)) => {
                    assert!(*volts > 1.6, "{time}: LED on, {volts} V");
                    lit += 1;
                }
                Some((0, 0)) => {
                    assert!(*volts < 0.5, "{time}: LED off, {volts} V");
                    dark += 1;
                }
                _ => {}
            }
        }
        let _ = result;
        assert!(lit > 10 && dark > 10, "it blinked: {lit} lit, {dark} dark");
    }

    /// A sine wave clocks a counter through a gate: analog into digital.
    #[test]
    fn a_sine_wave_clocks_a_counter() {
        let (d, top) = design(&[
            (
                "V1",
                "std:vsine",
                &[("offset", "2.5"), ("amplitude", "2.5"), ("frequency", "1k")],
                &[("p", "sine"), ("n", "GND")],
            ),
            ("U1", "std:not", &[], &[("a", "sine"), ("y", "edge")]),
            ("RST", "std:reset", &[("length", "700u")], &[("rst", "rst")]),
            ("K1", "std:const", &[("value", "1")], &[("y", "one")]),
            (
                "CNT",
                "std:counter",
                &[("width", "8")],
                &[("clk", "edge"), ("rst", "rst"), ("en", "one"), ("q", "count")],
            ),
        ]);
        let sim = run(&d, &top, 5.2e-3, 5e-6);
        let Some(Results::Mixed { logic, .. }) = &sim.results else { panic!() };
        let count = sim.netlist.net("count").unwrap();
        // The inverter rises as the sine falls through 30 % of 5 V (1.5 V), once a
        // cycle: at 0.566, 1.566, … 4.566 ms. The first edge resets the counter (the
        // reset lasts 0.7 ms); the next four count.
        assert_eq!(logic.last(count).and_then(|v| v.known()), Some(4));
        let edge = sim.netlist.net("edge").unwrap();
        let h = &logic.history[edge];
        let first_rise = h
            .windows(2)
            .find(|w| w[0].1.known() == Some(0) && w[1].1.known() == Some(1))
            .map(|w| w[1].0)
            .unwrap();
        let pi = std::f64::consts::PI;
        let expect = (pi + 0.4f64.asin()) / (2.0 * pi) * 1e-3 + 1e-9;
        let got = circuiter_logic::seconds(first_rise);
        assert!((got - expect).abs() < 5e-6, "rose at {got}, expected {expect}");
    }
}
