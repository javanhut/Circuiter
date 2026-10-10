//! The physics and failure engine.
//!
//! Every part with physics data is watched while a transient runs:
//!
//! 1. its dissipated power drives a first-order thermal model (thermal resistance to
//!    ambient and heat capacity), integrated exactly over each time step;
//! 2. its limits are checked at every point: voltage across two ports, current into a
//!    port, and melting energy (`∫(i² − rated²) dt`) for fuses;
//! 3. when a part reaches its destruction temperature or breaks a limit, it fails: its
//!    elements are replaced by its failure mode (open, or shorted between two ports),
//!    an [`Event`] records what happened and why, and the simulation carries on with
//!    the damaged circuit.
//!
//! At an operating point, [`Monitor::assess`] reports which parts would overheat if
//! left running, and how soon.
//!
//! A product in a case keeps its heat in: with a [`Case`], every part's power warms the
//! air inside, which loses it through the case to the room, and each part's ambient is
//! that air rather than the room.

use circuiter_model::cell::{Effect, FailureMode, Limit};
use circuiter_model::units::format_quantity;
use circuiter_netlist::Netlist;
use circuiter_sim::number::eval;
use circuiter_sim::{Circuit, Element, Observer, OpResult, Step};

/// Ambient temperature, °C, unless set otherwise.
pub const AMBIENT: f64 = 25.0;

/// Conductance of a failed short (20 mΩ): a fused, welded or punched-through junction.
const SHORT: f64 = 50.0;

/// A limit with values and the circuit nodes it concerns.
#[derive(Debug, Clone, PartialEq)]
enum Check {
    Voltage { ports: (String, String), nodes: (usize, usize), max: f64 },
    Current { port: String, node: usize, max: f64 },
    Energy { port: String, node: usize, rated: f64, max: f64, accumulated: f64 },
}

/// A part being watched.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// Instance path: `R1`, `U1.MN`.
    pub path: String,
    /// The circuit devices this part is made of.
    devices: Vec<usize>,
    /// Port name to circuit node.
    ports: Vec<(String, usize)>,
    thermal_resistance: Option<f64>,
    heat_capacity: Option<f64>,
    max_temperature: Option<f64>,
    checks: Vec<Check>,
    mode: FailureMode,
    effect: Effect,
    /// Hot-spot temperature, °C.
    pub temperature: f64,
    /// Dissipated power at the latest point, W.
    pub power: f64,
    /// Highest temperature reached.
    pub peak: f64,
    pub failed: bool,
}

/// A part's destruction.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub time: f64,
    pub path: String,
    pub effect: Effect,
    pub mode: FailureMode,
    /// What did it, in words: `reached 251 °C (destroyed at 250 °C) dissipating 1.1 W`.
    pub cause: String,
}

impl Event {
    /// One line for a log: `R1 burnt open at 3.21 s: reached 251 °C ...`.
    pub fn describe(&self) -> String {
        let what = match (&self.effect, &self.mode) {
            (Effect::Smoke, FailureMode::Open) => "burnt open".to_owned(),
            (Effect::Smoke, FailureMode::Short(a, b)) => format!("burnt, shorting {a} to {b}"),
            (Effect::Pop, FailureMode::Open) => "cracked open".to_owned(),
            (Effect::Pop, FailureMode::Short(a, b)) => format!("popped, shorting {a} to {b}"),
            (Effect::Flash, FailureMode::Open) => "blew open with a flash".to_owned(),
            (Effect::Flash, FailureMode::Short(a, b)) => {
                format!("flashed over, shorting {a} to {b}")
            }
            (Effect::Explode, FailureMode::Open) => "exploded".to_owned(),
            (Effect::Explode, FailureMode::Short(a, b)) => format!("exploded, shorting {a} to {b}"),
        };
        format!("{} {what} at {}: {}", self.path, format_quantity(self.time, "s"), self.cause)
    }
}

/// A part an operating point would destroy, if it held.
#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    pub path: String,
    pub power: f64,
    /// How long it would last from cold: zero if a limit is already broken, `None` if
    /// it overheats but its heat capacity isn't known.
    pub time_to_failure: Option<f64>,
    /// Why, in words.
    pub reason: String,
}

/// The enclosure a circuit runs in, thermally: the air inside and the case around it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Case {
    /// From the air inside to the room, through the case's walls, K/W.
    pub resistance: f64,
    /// Of the air and the case together, J/K.
    pub capacity: f64,
    /// The air's temperature now, and the hottest it got, °C.
    pub air: f64,
    pub peak: f64,
}

impl Case {
    pub fn new(resistance: f64, capacity: f64, room: f64) -> Case {
        Case { resistance, capacity, air: room, peak: room }
    }
}

/// Watches a transient run: an [`Observer`] for [`circuiter_sim::tran_observed`].
#[derive(Debug, Clone)]
pub struct Monitor {
    pub parts: Vec<Part>,
    pub events: Vec<Event>,
    /// The room's temperature.
    pub ambient: f64,
    /// Each part's temperature at every accepted time point.
    pub temperatures: Vec<Vec<f32>>,
    /// The case the circuit is in, if it is in one.
    pub case: Option<Case>,
    /// The air in the case at every accepted time point.
    pub air: Vec<f32>,
}

fn value(text: &Option<String>) -> Result<Option<f64>, String> {
    text.as_deref()
        .map(|t| {
            let t = t.trim();
            let inner = t.strip_prefix('{').and_then(|t| t.strip_suffix('}')).unwrap_or(t);
            eval(inner)
        })
        .transpose()
}

impl Part {
    /// The temperature that destroys it, if it has one.
    pub fn max_temperature(&self) -> Option<f64> {
        self.max_temperature
    }
}

impl Monitor {
    /// Finds every part with physics in `netlist` and the circuit devices it was built
    /// into (`circuit` must be parsed from the same netlist).
    pub fn new(netlist: &Netlist, circuit: &Circuit, ambient: f64) -> Result<Monitor, String> {
        let mut parts = Vec::new();
        for e in &netlist.elements {
            let Some(physics) = &e.physics else { continue };
            let at = |m: String| format!("{}: {m}", e.path);
            let devices: Vec<usize> = e.devices().filter_map(|d| circuit.device(d)).collect();
            let node = |net: usize| circuit.node(&netlist.nets[net].node).unwrap_or(0);
            let ports: Vec<(String, usize)> =
                e.ports.iter().map(|(p, n)| (p.clone(), node(*n))).collect();
            let port_node = |p: &str| {
                ports
                    .iter()
                    .find(|(q, _)| q == p)
                    .map(|(_, n)| *n)
                    .ok_or_else(|| at(format!("no port `{p}`")))
            };
            let num = |t: &str| value(&Some(t.to_owned())).map(|v| v.unwrap_or(0.0)).map_err(at);
            let mut checks = Vec::new();
            for l in &physics.limits {
                checks.push(match l {
                    Limit::Voltage { ports: (a, b), max } => Check::Voltage {
                        ports: (a.clone(), b.clone()),
                        nodes: (port_node(a)?, port_node(b)?),
                        max: num(max)?,
                    },
                    Limit::Current { port, max } => Check::Current {
                        port: port.clone(),
                        node: port_node(port)?,
                        max: num(max)?,
                    },
                    Limit::Energy { port, rated, max } => Check::Energy {
                        port: port.clone(),
                        node: port_node(port)?,
                        rated: num(rated)?,
                        max: num(max)?,
                        accumulated: 0.0,
                    },
                });
            }
            parts.push(Part {
                path: e.path.clone(),
                devices,
                ports,
                thermal_resistance: value(&physics.thermal_resistance).map_err(at)?,
                heat_capacity: value(&physics.heat_capacity).map_err(at)?,
                max_temperature: value(&physics.max_temperature).map_err(at)?,
                checks,
                mode: physics.failure.mode.clone(),
                effect: physics.failure.effect,
                temperature: ambient,
                power: 0.0,
                peak: ambient,
                failed: false,
            });
        }
        Ok(Monitor {
            parts,
            events: Vec::new(),
            ambient,
            temperatures: Vec::new(),
            case: None,
            air: Vec::new(),
        })
    }

    /// The same, for a circuit inside `case`.
    pub fn in_case(mut self, case: Case) -> Monitor {
        self.case = Some(case);
        self
    }

    /// What surrounds the parts: the air in the case, or the room.
    pub fn surroundings(&self) -> f64 {
        self.case.map_or(self.ambient, |c| c.air)
    }

    pub fn part(&self, path: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.path == path)
    }

    /// Parts the operating point `op` (of `circuit`) would destroy if it held: those
    /// already beyond a limit, and those that would overheat.
    pub fn assess(&self, circuit: &Circuit, op: &OpResult) -> Vec<Assessment> {
        let mut out = Vec::new();
        let voltage = |node: usize| op.voltages[node];
        // In a case, the air settles where the whole circuit's power holds it.
        let power_of = |p: &Part| -> f64 {
            p.devices
                .iter()
                .filter_map(|&d| op.device(&circuit.devices[d].name))
                .map(|o| o.power)
                .sum()
        };
        let surroundings = match self.case {
            Some(c) => {
                self.ambient + self.parts.iter().map(power_of).sum::<f64>().max(0.0) * c.resistance
            }
            None => self.ambient,
        };
        for p in &self.parts {
            let ops: Vec<(usize, circuiter_sim::DeviceOp)> = p
                .devices
                .iter()
                .filter_map(|&d| op.device(&circuit.devices[d].name).map(|o| (d, o.clone())))
                .collect();
            let power: f64 = ops.iter().map(|(_, o)| o.power).sum();
            let current_into = |node: usize| current_into(circuit, &ops, node);
            if let Some(reason) = p.checks.iter().find_map(|c| instant(c, &voltage, &current_into))
            {
                out.push(Assessment {
                    path: p.path.clone(),
                    power,
                    time_to_failure: Some(0.0),
                    reason,
                });
                continue;
            }
            let (Some(rth), Some(tmax)) = (p.thermal_resistance, p.max_temperature) else {
                continue;
            };
            let settle = surroundings + power * rth;
            if settle <= tmax {
                continue;
            }
            // T(t) = ambient + P·Rth·(1 − e^(−t/τ)) reaches tmax at t = −τ ln(1 − ΔT/(P·Rth)).
            let time_to_failure = p.heat_capacity.map(|cth| {
                let tau = rth * cth;
                -tau * (1.0 - (tmax - surroundings) / (power * rth)).ln()
            });
            out.push(Assessment {
                path: p.path.clone(),
                power,
                time_to_failure,
                reason: format!(
                    "{} in it would take it to {settle:.0} °C (destroyed at {tmax:.0} °C)",
                    format_quantity(power, "W")
                ),
            });
        }
        out
    }

    /// Where part `k` would stand after `step`, without changing anything: its power,
    /// temperature, accumulated melting energies and, if it would fail, why.
    fn evaluate(&self, k: usize, step: &Step<'_>) -> (f64, f64, Vec<f64>, Option<String>) {
        let part = &self.parts[k];
        let ops: Vec<_> = part.devices.iter().map(|&d| (d, step.device(d))).collect();
        let power: f64 = ops.iter().map(|(_, o)| o.power).sum();
        let current_into = |node: usize| current_into(step.circuit(), &ops, node);
        // Exact for constant power over the step: T → Tss with time constant τ.
        let settle = self.surroundings() + power * part.thermal_resistance.unwrap_or(0.0);
        let temperature = match (part.thermal_resistance, part.heat_capacity) {
            (Some(rth), Some(cth)) if step.dt > 0.0 => {
                settle + (part.temperature - settle) * (-step.dt / (rth * cth)).exp()
            }
            (Some(_), None) => settle,
            _ => part.temperature,
        };
        let mut energies = Vec::new();
        let mut cause = part.max_temperature.filter(|max| temperature > *max).map(|max| {
            format!(
                "reached {temperature:.0} °C (destroyed at {max:.0} °C) dissipating {}",
                format_quantity(power, "W")
            )
        });
        for check in &part.checks {
            if let Check::Energy { port, node, rated, max, accumulated } = check {
                let i = current_into(*node);
                let e = accumulated
                    + if i.abs() > *rated { (i * i - rated * rated) * step.dt } else { 0.0 };
                energies.push(e);
                if e > *max && cause.is_none() {
                    cause = Some(format!(
                        "melted carrying {} through {port} (rated {})",
                        format_quantity(i.abs(), "A"),
                        format_quantity(*rated, "A")
                    ));
                }
            } else if cause.is_none() {
                cause = instant(check, &|n| step.voltage(n), &current_into);
            }
        }
        (power, temperature, energies, cause)
    }

    /// What the part becomes once destroyed.
    fn failure(&self, part: &Part, circuit: &Circuit) -> Vec<(usize, Element)> {
        let mut out: Vec<(usize, Element)> = part
            .devices
            .iter()
            .map(|&d| {
                (d, Element::Failed { branch: circuit.devices[d].element.branch(), short: None })
            })
            .collect();
        if let (FailureMode::Short(a, b), Some(first)) = (&part.mode, out.first_mut()) {
            let node = |p: &str| part.ports.iter().find(|(q, _)| q == p).map(|(_, n)| *n);
            if let (Some(na), Some(nb)) = (node(a), node(b)) {
                if let Element::Failed { short, .. } = &mut first.1 {
                    *short = Some((na, nb, SHORT));
                }
            }
        }
        out
    }
}

/// The current into the part at `node`, summed over its devices' terminals there.
fn current_into(circuit: &Circuit, ops: &[(usize, circuiter_sim::DeviceOp)], node: usize) -> f64 {
    ops.iter()
        .map(|(d, o)| {
            circuit.devices[*d]
                .terminals()
                .iter()
                .zip(&o.currents)
                .filter(|(t, _)| **t == node)
                .map(|(_, i)| *i)
                .sum::<f64>()
        })
        .sum()
}

/// Whether an instant limit is broken, and how.
fn instant(
    check: &Check,
    voltage: &dyn Fn(usize) -> f64,
    current: &dyn Fn(usize) -> f64,
) -> Option<String> {
    match check {
        Check::Voltage { ports, nodes, max } => {
            let v = voltage(nodes.0) - voltage(nodes.1);
            (v.abs() > *max).then(|| {
                format!(
                    "{} across {}–{}, beyond its {} rating",
                    format_quantity(v.abs(), "V"),
                    ports.0,
                    ports.1,
                    format_quantity(*max, "V")
                )
            })
        }
        Check::Current { port, node, max } => {
            let i = current(*node);
            (i.abs() > *max).then(|| {
                format!(
                    "{} through {port}, beyond its {} rating",
                    format_quantity(i.abs(), "A"),
                    format_quantity(*max, "A")
                )
            })
        }
        Check::Energy { .. } => None,
    }
}

impl Observer for Monitor {
    fn fails(&self, step: &Step<'_>) -> bool {
        (0..self.parts.len()).any(|k| !self.parts[k].failed && self.evaluate(k, step).3.is_some())
    }

    fn step(&mut self, step: &Step<'_>) -> Vec<(usize, Element)> {
        let mut replace = Vec::new();
        for k in 0..self.parts.len() {
            let (power, temperature, energies, cause) = self.evaluate(k, step);
            let part = &mut self.parts[k];
            // A destroyed part keeps its thermal life: with nothing flowing, it cools.
            part.temperature = temperature;
            part.peak = part.peak.max(temperature);
            if part.failed {
                continue;
            }
            part.power = power;
            let mut e = energies.into_iter();
            for check in &mut part.checks {
                if let Check::Energy { accumulated, .. } = check {
                    *accumulated = e.next().unwrap_or(*accumulated);
                }
            }
            if let Some(cause) = cause {
                part.failed = true;
                self.events.push(Event {
                    time: step.time,
                    path: part.path.clone(),
                    effect: part.effect,
                    mode: part.mode.clone(),
                    cause,
                });
                replace.extend(self.failure(&self.parts[k], step.circuit()));
            }
        }
        // The case's air: warmed by every part, cooled through the case to the room.
        if let Some(case) = &mut self.case {
            let power: f64 =
                self.parts.iter().filter(|p| !p.failed).map(|p| p.power.max(0.0)).sum();
            let settle = self.ambient + power * case.resistance;
            let tau = case.resistance * case.capacity;
            // No time passing (the starting point), no change.
            case.air = if tau > 0.0 {
                settle + (case.air - settle) * (-step.dt.max(0.0) / tau).exp()
            } else {
                settle
            };
            case.peak = case.peak.max(case.air);
            self.air.push(case.air as f32);
        }
        self.temperatures.push(self.parts.iter().map(|p| p.temperature as f32).collect());
        replace
    }
}
