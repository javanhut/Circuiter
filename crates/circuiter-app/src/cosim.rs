//! Mixed circuits: the analog and digital simulators running together on one timeline.
//!
//! Where a digital part drives a net that analog parts are on, the digital value is
//! applied through a converter: a voltage source (0 V, or the supply for a 1) behind a
//! 50 Ω output resistance, opened to 1 GΩ when the output is undriven (Z). Where a
//! digital part reads a net that only analog parts drive, the voltage is read through
//! thresholds at 30 % and 70 % of the supply, with hysteresis between them; the time
//! it crosses is interpolated between the analog steps. The analog solver lands its
//! steps exactly on scheduled digital changes of the nets it is joined to.

use crate::digital::{self, LogicRun};
use circuiter_logic::{Simulator, Time, Value, prims};
use circuiter_netlist::Netlist;
use circuiter_physics::Monitor;
use circuiter_sim::{Circuit, Element, Inputs, Observer, Step};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The supply when the design has no VDD net to take it from.
pub const DEFAULT_SUPPLY: f64 = 5.0;
/// A driven digital output's resistance.
const OUTPUT_RESISTANCE: f64 = 50.0;
/// An undriven output: open.
const OPEN: f64 = 1e9;

/// A digital output driving an analog net.
#[derive(Debug)]
struct DigitalToAnalog {
    net: usize,
    /// External inputs: the level source and the output resistance.
    level: usize,
    resistance: usize,
    applied: Option<Value>,
}

/// An analog net read by digital parts.
#[derive(Debug)]
struct AnalogToDigital {
    node: usize,
    element: usize,
    handle: Arc<AtomicU64>,
    state: Option<bool>,
    last: (f64, f64),
}

/// The two simulators joined, as an observer of the analog run.
#[derive(Debug)]
pub struct Cosim {
    pub digital: Simulator,
    inputs: Arc<Inputs>,
    to_analog: Vec<DigitalToAnalog>,
    to_digital: Vec<AnalogToDigital>,
    /// Which logic nets drive analog nets, for landing steps on their changes.
    bridged: Vec<bool>,
    vdd: Option<usize>,
    jumped: bool,
    /// The physics engine, watching the analog side.
    pub monitor: Option<Monitor>,
    /// Interactive inputs on the digital side, polled each step.
    pub live: Vec<(usize, Arc<AtomicU64>, u64)>,
}

/// A mixed design ready to run: the analog circuit (with converters added), its
/// inputs, and the co-simulation that drives them.
#[derive(Debug)]
pub struct Prepared {
    pub circuit: Circuit,
    pub inputs: Arc<Inputs>,
    pub cosim: Cosim,
    /// Switches and buttons on the digital side.
    pub digital_inputs: Vec<digital::InputHandle>,
}

/// Builds both halves of a mixed design and the converters between them. `extra`
/// adds analog inputs (switches, knobs) by device name, with their starting values.
pub fn prepare(
    netlist: &Netlist,
    root: &Path,
    extra: &[(String, f64)],
) -> Result<Prepared, String> {
    let (mut logic, digital_inputs) = digital::build_with_inputs(netlist, root)?;
    // Which nets digital parts drive and read.
    let n = netlist.nets.len();
    let mut drives = vec![false; n];
    let mut reads = vec![false; n];
    for e in &logic.elements {
        for (spec, &net) in e.primitive.ports().iter().zip(&e.nets) {
            if net >= n {
                continue;
            }
            match spec.dir {
                circuiter_logic::Dir::In => reads[net] = true,
                circuiter_logic::Dir::Out => drives[net] = true,
                circuiter_logic::Dir::InOut => {
                    reads[net] = true;
                    drives[net] = true;
                }
            }
        }
    }
    let mut analog = vec![false; n];
    for e in &netlist.elements {
        for (_, net) in &e.ports {
            analog[*net] = true;
        }
    }
    let mut deck = netlist.to_spice();
    let mut initial: Vec<(String, f64)> = extra.to_vec();
    let mut pending_da = Vec::new();
    let mut pending_ad = Vec::new();
    for k in 0..n {
        if !analog[k] || !(drives[k] || reads[k]) {
            continue;
        }
        let net = &netlist.nets[k];
        if net.width > 1 {
            return Err(format!(
                "{} is a {}-bit bus and joins analog parts; only single wires can",
                net.name, net.width
            ));
        }
        if drives[k] {
            let (v, r) = (format!("VXDA{k}"), format!("RXDA{k}"));
            deck.push_str(&format!("{v} xda{k} 0 DC 0\n{r} xda{k} {} {OPEN}\n", net.node));
            initial.push((v.clone(), 0.0));
            initial.push((r.clone(), OPEN));
            pending_da.push((k, v, r));
        } else {
            pending_ad.push(k);
        }
    }
    let circuit = Circuit::parse(&deck).map_err(|e| format!("netlist: {e}"))?;
    let names: Vec<(&str, f64)> = initial.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    let inputs = Arc::new(Inputs::new(&names));
    let to_analog: Vec<DigitalToAnalog> = pending_da
        .into_iter()
        .map(|(net, v, r)| DigitalToAnalog {
            net,
            level: inputs.index(&v).expect("added"),
            resistance: inputs.index(&r).expect("added"),
            applied: None,
        })
        .collect();
    let mut to_digital = Vec::new();
    for k in pending_ad {
        let node = circuit.node(&netlist.nets[k].node).unwrap_or(0);
        let (part, handle) = prims::Input::with_handle(1, prims::INPUT_UNKNOWN);
        let element = logic.add(&format!("#adc.{}", netlist.nets[k].name), part, &[k])?;
        to_digital.push(AnalogToDigital { node, element, handle, state: None, last: (0.0, 0.0) });
    }
    let mut bridged = vec![false; logic.nets.len()];
    for d in &to_analog {
        bridged[d.net] = true;
    }
    let vdd = netlist
        .nets
        .iter()
        .find(|net| net.name == "VDD")
        .and_then(|net| circuit.node(&net.node))
        .filter(|&node| node > 0);
    let cosim = Cosim {
        digital: Simulator::new(logic),
        inputs: inputs.clone(),
        to_analog,
        to_digital,
        bridged,
        vdd,
        jumped: false,
        monitor: None,
        live: digital_inputs
            .iter()
            .map(|(_, k, h)| (*k, h.clone(), h.load(Ordering::Relaxed)))
            .collect(),
    };
    Ok(Prepared { circuit, inputs, cosim, digital_inputs })
}

fn seconds_to_time(t: f64) -> Time {
    circuiter_logic::from_seconds(t)
}

impl Cosim {
    /// The supply the logic levels are measured against.
    fn supply(&self, step: &Step<'_>) -> f64 {
        match self.vdd {
            Some(node) => step.voltage(node).max(0.1),
            None => DEFAULT_SUPPLY,
        }
    }

    /// Runs the digital side up to `t`, applying threshold crossings on the way.
    fn catch_up(&mut self, step: &Step<'_>) {
        let t = step.time;
        let vdd = self.supply(step);
        let (low, high) = (0.3 * vdd, 0.7 * vdd);
        let mut crossings: Vec<(f64, usize, bool)> = Vec::new();
        for (k, a) in self.to_digital.iter_mut().enumerate() {
            let v = step.voltage(a.node);
            let next = if v >= high {
                Some(true)
            } else if v <= low {
                Some(false)
            } else {
                a.state
            };
            if let Some(level) = next.filter(|_| next != a.state) {
                // Where it crossed the middle, between the last step and this one.
                let (t0, v0) = a.last;
                let mid = 0.5 * vdd;
                let at = if (v - v0).abs() > 1e-12 && step.dt > 0.0 {
                    (t0 + (mid - v0) / (v - v0) * (t - t0)).clamp(t0, t)
                } else {
                    t
                };
                crossings.push((at, k, level));
                a.state = next;
            }
            a.last = (t, v);
        }
        crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (at, k, high) in crossings {
            let when = seconds_to_time(at);
            let _ = self.digital.run_until(when.saturating_sub(1));
            self.digital.skip_to(when);
            let a = &self.to_digital[k];
            a.handle.store(u64::from(high), Ordering::Relaxed);
            self.digital.poke(a.element);
        }
        // Switches and buttons on the digital side.
        for (element, handle, applied) in &mut self.live {
            let v = handle.load(Ordering::Relaxed);
            if v != *applied {
                *applied = v;
                self.digital.skip_to(seconds_to_time(t));
                self.digital.poke(*element);
            }
        }
        let _ = self.digital.run_until(seconds_to_time(t));
        // Digital outputs onto the analog nets.
        for d in &mut self.to_analog {
            let v = self.digital.value(d.net);
            if d.applied == Some(v) {
                continue;
            }
            d.applied = Some(v);
            let (bits, x, z) = v.planes();
            let (level, r) = if z & 1 == 1 {
                (0.0, OPEN)
            } else if x & 1 == 1 {
                (0.5 * vdd, OUTPUT_RESISTANCE)
            } else {
                (if bits & 1 == 1 { vdd } else { 0.0 }, OUTPUT_RESISTANCE)
            };
            self.inputs.set(d.level, level);
            self.inputs.set(d.resistance, r);
            self.jumped = true;
        }
    }

    /// What the digital side recorded, as a digital run ending at `end`.
    pub fn logic_run(&self, tstop: f64, end: f64) -> LogicRun {
        let n = self.digital.circuit().nets.len();
        LogicRun {
            history: (0..n).map(|k| self.digital.history(k).to_vec()).collect(),
            tstop,
            end,
            halted: self.digital.halted(),
            memories: self
                .digital
                .circuit()
                .elements
                .iter()
                .filter_map(|e| Some((e.path.clone(), e.primitive.memory()?.to_vec())))
                .collect(),
        }
    }
}

impl Observer for Cosim {
    fn fails(&self, step: &Step<'_>) -> bool {
        self.monitor.as_ref().is_some_and(|m| m.fails(step))
    }

    fn step(&mut self, step: &Step<'_>) -> Vec<(usize, Element)> {
        self.catch_up(step);
        match &mut self.monitor {
            Some(m) => m.step(step),
            None => Vec::new(),
        }
    }

    fn next_event(&self, t: f64) -> Option<f64> {
        let next = self.digital.next_drive_on(&self.bridged)?;
        let s = circuiter_logic::seconds(next);
        (s > t).then_some(s)
    }

    fn take_discontinuity(&mut self) -> bool {
        std::mem::take(&mut self.jumped)
    }
}
