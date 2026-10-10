//! Circuiter's digital simulator.
//!
//! Event-driven, four-state (0, 1, X, Z) simulation of nets up to 64 bits wide, with
//! propagation delays: fast enough to run processors, where transistor-level analog
//! simulation would take hours. Each part is a [`Primitive`]: a gate, a flip-flop, a
//! register file, a memory. Nets resolve several drivers the way wires do (Z gives
//! way, a conflict is X).
//!
//! ```
//! use circuiter_logic::{Circuit, Simulator, Value, prims, ns};
//! let mut c = Circuit::new();
//! let (a, b, y) = (c.net("a", 1), c.net("b", 1), c.net("y", 1));
//! let p = |model, extra: &[(&str, &str)]| prims::create(model, &prims::params(extra), None).unwrap();
//! c.add("A", p("const", &[("value", "1")]), &[a]).unwrap();
//! c.add("B", p("const", &[("value", "0")]), &[b]).unwrap();
//! c.add("U1", p("nand", &[]), &[a, b, y]).unwrap();
//! let mut sim = Simulator::new(c);
//! sim.run_until(ns(10.0)).unwrap();
//! assert_eq!(sim.value(y), Value::bit(true));
//! ```

pub mod prims;
mod value;

pub use value::{Bit, MAX_WIDTH, Value};

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fmt;

/// Simulated time, in picoseconds.
pub type Time = u64;

pub fn ns(t: f64) -> Time {
    (t * 1e3).round() as Time
}

/// Seconds to [`Time`].
pub fn from_seconds(t: f64) -> Time {
    (t * 1e12).round().max(0.0) as Time
}

pub fn seconds(t: Time) -> f64 {
    t as f64 * 1e-12
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    In,
    Out,
    /// Read and driven, as a tri-state bus pin.
    InOut,
}

/// A primitive's terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortSpec {
    pub name: String,
    pub dir: Dir,
    pub width: u32,
}

impl PortSpec {
    pub fn input(name: &str, width: u32) -> PortSpec {
        PortSpec { name: name.to_owned(), dir: Dir::In, width }
    }

    pub fn output(name: &str, width: u32) -> PortSpec {
        PortSpec { name: name.to_owned(), dir: Dir::Out, width }
    }
}

/// What a part does: looks at its inputs and drives its outputs, after a delay.
pub trait Primitive: fmt::Debug + Send {
    fn ports(&self) -> &[PortSpec];

    /// Called once at time 0, whenever an input changes, and at times asked for with
    /// [`Eval::wake_at`].
    fn eval(&mut self, cx: &mut Eval<'_>);

    /// Contents worth inspecting: a memory's words, a register file's registers.
    fn memory(&self) -> Option<&[Value]> {
        None
    }

    /// Asks the simulation to stop (the `stop` part does, when its input rises).
    fn halted(&self) -> bool {
        false
    }
}

/// A primitive's view of the moment it is evaluated.
#[derive(Debug)]
pub struct Eval<'a> {
    pub now: Time,
    inputs: &'a [Value],
    drives: &'a mut Vec<(usize, Value, Time)>,
    wake: &'a mut Option<Time>,
}

impl Eval<'_> {
    /// The value on port `port`'s net, Z bits read as X.
    pub fn input(&self, port: usize) -> Value {
        self.inputs[port].as_input()
    }

    /// The value on port `port`'s net as driven: Z stays Z.
    pub fn raw(&self, port: usize) -> Value {
        self.inputs[port]
    }

    /// Drives `value` onto port `port` after `delay`.
    pub fn drive(&mut self, port: usize, value: Value, delay: Time) {
        self.drives.push((port, value, delay));
    }

    /// Evaluates the part again at `time` (for clocks and other timed sources).
    pub fn wake_at(&mut self, time: Time) {
        *self.wake = Some(time);
    }
}

#[derive(Debug, Clone)]
pub struct NetDef {
    pub name: String,
    pub width: u32,
}

#[derive(Debug)]
pub struct Element {
    pub path: String,
    pub primitive: Box<dyn Primitive>,
    /// The net on each port.
    pub nets: Vec<usize>,
}

/// Nets and the parts between them.
#[derive(Debug, Default)]
pub struct Circuit {
    pub nets: Vec<NetDef>,
    pub elements: Vec<Element>,
}

impl Circuit {
    pub fn new() -> Circuit {
        Circuit::default()
    }

    pub fn net(&mut self, name: &str, width: u32) -> usize {
        self.nets.push(NetDef { name: name.to_owned(), width });
        self.nets.len() - 1
    }

    /// Adds a part with its ports on `nets`, in the order of its [`Primitive::ports`].
    pub fn add(
        &mut self,
        path: &str,
        primitive: Box<dyn Primitive>,
        nets: &[usize],
    ) -> Result<usize, String> {
        let ports = primitive.ports();
        if ports.len() != nets.len() {
            return Err(format!("{path}: {} ports but {} nets", ports.len(), nets.len()));
        }
        for (port, &net) in ports.iter().zip(nets) {
            let n = self.nets.get(net).ok_or_else(|| format!("{path}: no net {net}"))?;
            if n.width != port.width {
                return Err(format!(
                    "{path}.{} is {} bits wide but net {} is {} bits",
                    port.name, port.width, n.name, n.width
                ));
            }
        }
        self.elements.push(Element { path: path.to_owned(), primitive, nets: nets.to_vec() });
        Ok(self.elements.len() - 1)
    }
}

#[derive(Debug, Clone, Copy)]
enum Event {
    /// Element, port, value.
    Drive(usize, usize, Value),
    Wake(usize),
}

/// An event in the queue, in time order, then in the order it was scheduled.
#[derive(Debug, Clone, Copy)]
struct Scheduled {
    time: Time,
    seq: u64,
    event: Event,
}

impl PartialEq for Scheduled {
    fn eq(&self, other: &Self) -> bool {
        (self.time, self.seq) == (other.time, other.seq)
    }
}

impl Eq for Scheduled {}

impl PartialOrd for Scheduled {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Scheduled {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.time, self.seq).cmp(&(other.time, other.seq))
    }
}

/// How many evaluation rounds one instant may take before the circuit is called
/// oscillating (a loop of gates with no delay).
const DELTA_LIMIT: usize = 10_000;

/// Runs a [`Circuit`] and keeps the history of every net.
#[derive(Debug)]
pub struct Simulator {
    circuit: Circuit,
    now: Time,
    seq: u64,
    queue: BinaryHeap<Reverse<Scheduled>>,
    /// Per net: each driver (element, port) and what it drives.
    drivers: Vec<Vec<(usize, usize, Value)>>,
    /// Per net: the elements that read it.
    loads: Vec<Vec<usize>>,
    values: Vec<Value>,
    history: Vec<Vec<(Time, Value)>>,
    wakes: Vec<Option<Time>>,
    started: bool,
    halted: bool,
    pub events: u64,
}

impl Simulator {
    pub fn new(circuit: Circuit) -> Simulator {
        let n = circuit.nets.len();
        let mut drivers = vec![Vec::new(); n];
        let mut loads = vec![Vec::new(); n];
        for (e, el) in circuit.elements.iter().enumerate() {
            for (p, (spec, &net)) in el.primitive.ports().iter().zip(&el.nets).enumerate() {
                if spec.dir != Dir::In {
                    drivers[net].push((e, p, Value::floating(spec.width)));
                }
                if spec.dir != Dir::Out && !loads[net].contains(&e) {
                    loads[net].push(e);
                }
            }
        }
        let values: Vec<Value> = circuit.nets.iter().map(|n| Value::floating(n.width)).collect();
        let history = values.iter().map(|&v| vec![(0, v)]).collect();
        let wakes = vec![None; circuit.elements.len()];
        Simulator {
            circuit,
            now: 0,
            seq: 0,
            queue: BinaryHeap::new(),
            drivers,
            loads,
            values,
            history,
            wakes,
            started: false,
            halted: false,
            events: 0,
        }
    }

    pub fn circuit(&self) -> &Circuit {
        &self.circuit
    }

    pub fn now(&self) -> Time {
        self.now
    }

    pub fn value(&self, net: usize) -> Value {
        self.values[net]
    }

    /// Every change on `net`: (time, new value), starting at time 0.
    pub fn history(&self, net: usize) -> &[(Time, Value)] {
        &self.history[net]
    }

    /// The value `net` had at `time`.
    pub fn value_at(&self, net: usize, time: Time) -> Value {
        let h = &self.history[net];
        let i = h.partition_point(|(t, _)| *t <= time);
        h[i.saturating_sub(1)].1
    }

    pub fn element(&self, path: &str) -> Option<&Element> {
        self.circuit.elements.iter().find(|e| e.path == path)
    }

    /// Whether a part asked to stop.
    pub fn halted(&self) -> bool {
        self.halted
    }

    fn push(&mut self, time: Time, event: Event) {
        self.seq += 1;
        self.queue.push(Reverse(Scheduled { time, seq: self.seq, event }));
    }

    fn evaluate(&mut self, e: usize) {
        let el = &mut self.circuit.elements[e];
        let inputs: Vec<Value> = el.nets.iter().map(|&n| self.values[n]).collect();
        let mut drives = Vec::new();
        let mut wake = None;
        let mut cx = Eval { now: self.now, inputs: &inputs, drives: &mut drives, wake: &mut wake };
        el.primitive.eval(&mut cx);
        if el.primitive.halted() {
            self.halted = true;
        }
        let widths: Vec<u32> = el.primitive.ports().iter().map(|p| p.width).collect();
        for (port, value, delay) in drives {
            let at = self.now + delay;
            self.push(at, Event::Drive(e, port, value.resize(widths[port])));
        }
        if let Some(t) = wake {
            let t = t.max(self.now);
            self.wakes[e] = Some(t);
            self.push(t, Event::Wake(e));
        }
    }

    /// Runs until `end`, or until a part halts the simulation, the circuit settles
    /// with nothing left to happen, or it oscillates (an error).
    pub fn run_until(&mut self, end: Time) -> Result<(), String> {
        self.run_with(end, &mut |_| true)
    }

    /// As [`Simulator::run_until`], calling `keep_going(now)` now and then: return
    /// false to stop early (to cancel, or report progress).
    pub fn run_with(
        &mut self,
        end: Time,
        keep_going: &mut dyn FnMut(Time) -> bool,
    ) -> Result<(), String> {
        if !self.started {
            self.started = true;
            for e in 0..self.circuit.elements.len() {
                self.evaluate(e);
            }
        }
        let mut steps = 0u64;
        while let Some(&Reverse(Scheduled { time: t, .. })) = self.queue.peek() {
            if t > end || self.halted {
                break;
            }
            self.now = t;
            steps += 1;
            if steps % 4096 == 0 && !keep_going(t) {
                return Ok(());
            }
            // Delta cycles: changes at this instant until nothing more happens now.
            let mut rounds = 0;
            while self.queue.peek().is_some_and(|Reverse(s)| s.time == t) {
                rounds += 1;
                if rounds > DELTA_LIMIT {
                    return Err(format!(
                        "the circuit oscillates at {} ns: a loop of logic with no delay",
                        t as f64 / 1e3
                    ));
                }
                let mut dirty = Vec::new();
                let mut woken = Vec::new();
                while self.queue.peek().is_some_and(|Reverse(s)| s.time == t) {
                    let Reverse(Scheduled { event: ev, .. }) = self.queue.pop().expect("peeked");
                    self.events += 1;
                    match ev {
                        Event::Drive(e, port, value) => {
                            let net = self.circuit.elements[e].nets[port];
                            if let Some(slot) = self.drivers[net]
                                .iter_mut()
                                .find(|(de, dp, _)| *de == e && *dp == port)
                            {
                                if slot.2 != value {
                                    slot.2 = value;
                                    dirty.push(net);
                                }
                            }
                        }
                        Event::Wake(e) => {
                            if self.wakes[e] == Some(t) {
                                self.wakes[e] = None;
                                woken.push(e);
                            }
                        }
                    }
                }
                let mut to_eval = woken;
                dirty.sort_unstable();
                dirty.dedup();
                for net in dirty {
                    let width = self.circuit.nets[net].width;
                    let resolved = self.drivers[net]
                        .iter()
                        .fold(Value::floating(width), |acc, (_, _, v)| acc.resolve(*v));
                    if resolved != self.values[net] {
                        self.values[net] = resolved;
                        let h = &mut self.history[net];
                        match h.last_mut() {
                            Some(last) if last.0 == t => last.1 = resolved,
                            _ => h.push((t, resolved)),
                        }
                        to_eval.extend(self.loads[net].iter().copied());
                    }
                }
                to_eval.sort_unstable();
                to_eval.dedup();
                for e in to_eval {
                    self.evaluate(e);
                }
            }
        }
        if self.queue.is_empty() || self.halted {
            // Nothing more will happen (or a part halted): time stands where it is.
        } else {
            self.now = end;
        }
        Ok(())
    }
}
