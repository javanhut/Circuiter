//! Running a circuit live: the simulation goes on until stopped, at a chosen speed
//! (simulated time per real second), while switches are flipped, buttons held and
//! knobs turned. It runs on its own thread; the window reads what it shares (the
//! latest value on every net, recent samples of the nets being watched, instrument
//! readings) and sends it commands.

use crate::cosim::{self, Cosim};
use crate::digital;
use circuiter_logic::{Simulator, Time, Value};
use circuiter_model::{CellDef, CellId, Design, STD_LIBRARY};
use circuiter_netlist::Netlist;
use circuiter_physics::{AMBIENT, Monitor};
use circuiter_sim::{Circuit, Inputs, NoObserver, Options, Transient};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Parts you operate while a circuit runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    /// Click: open or closed contacts.
    Switch,
    /// Held: closed contacts.
    Pushbutton,
    /// Click: a logic 0 or 1.
    Toggle,
    /// Held: a logic 1.
    Button,
    /// A voltage between its limits.
    Dial,
    /// A wiper position, 0 to 1.
    Potentiometer,
    /// Click: a changeover switch thrown or at rest.
    Changeover,
    /// Light falling on a sensor, in lux.
    Light,
    /// Sunlight on a solar panel, W/m².
    Sunlight,
    /// A sensor's temperature, °C.
    Temperature,
}

impl ControlKind {
    pub fn of(cell: &CellId) -> Option<ControlKind> {
        if cell.library != STD_LIBRARY {
            return None;
        }
        Some(match cell.cell.as_str() {
            "switch" => ControlKind::Switch,
            "pushbutton" => ControlKind::Pushbutton,
            "toggle" => ControlKind::Toggle,
            "button" => ControlKind::Button,
            "dial" => ControlKind::Dial,
            "potentiometer" => ControlKind::Potentiometer,
            "switch_spdt" => ControlKind::Changeover,
            "ldr" | "photodiode" | "phototransistor" => ControlKind::Light,
            "solar_cell" => ControlKind::Sunlight,
            "thermistor" | "lm35" => ControlKind::Temperature,
            _ => return None,
        })
    }

    /// The parameter that holds its state.
    pub fn param(self) -> &'static str {
        match self {
            ControlKind::Switch => "closed",
            ControlKind::Pushbutton | ControlKind::Button => "pressed",
            ControlKind::Toggle => "on",
            ControlKind::Dial => "value",
            ControlKind::Potentiometer => "position",
            ControlKind::Changeover => "thrown",
            ControlKind::Light => "lux",
            ControlKind::Sunlight => "light",
            ControlKind::Temperature => "temp",
        }
    }

    /// Operated by holding the mouse down, rather than clicking.
    pub fn momentary(self) -> bool {
        matches!(self, ControlKind::Pushbutton | ControlKind::Button)
    }

    /// Flipped by a click.
    pub fn clicks(self) -> bool {
        matches!(self, ControlKind::Switch | ControlKind::Toggle | ControlKind::Changeover)
    }
}

/// What a multimeter reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    Volts(f64),
    Amps(f64),
    Ohms(f64),
}

impl Reading {
    pub fn show(self) -> String {
        match self {
            Reading::Volts(v) => crate::simulation::show(v, "V"),
            Reading::Amps(a) => crate::simulation::show(a, "A"),
            Reading::Ohms(r) if !(r.is_finite() && r.abs() < 1e8) => "OL Ω".to_owned(),
            Reading::Ohms(r) => crate::simulation::show(r, "Ω"),
        }
    }
}

/// A multimeter in the netlist: where its leads are and how it measures.
#[derive(Debug, Clone)]
pub struct Meter {
    pub path: String,
    pub p: usize,
    pub com: usize,
    /// Its device: a resistor (volts), a 0 V source (amps) or a current source (ohms).
    pub device: String,
}

impl Meter {
    /// Every multimeter in a netlist.
    pub fn all(netlist: &Netlist) -> Vec<Meter> {
        netlist
            .elements
            .iter()
            .filter(|e| e.cell == CellId::new(STD_LIBRARY, "multimeter"))
            .filter_map(|e| {
                let net = |port: &str| e.ports.iter().find(|(p, _)| p == port).map(|(_, n)| *n);
                Some(Meter {
                    path: e.path.clone(),
                    p: net("p")?,
                    com: net("com")?,
                    device: e.devices().next()?.to_owned(),
                })
            })
            .collect()
    }

    /// The reading, from a voltage lookup and a branch-current lookup.
    pub fn read(
        &self,
        volts: &dyn Fn(usize) -> Option<f64>,
        amps: &dyn Fn(&str) -> Option<f64>,
    ) -> Option<Reading> {
        let across = volts(self.p)? - volts(self.com).unwrap_or(0.0);
        Some(match self.device.chars().next()?.to_ascii_uppercase() {
            'V' => Reading::Amps(amps(&self.device)?),
            'I' => Reading::Ohms(across / 1e-3),
            _ => Reading::Volts(across),
        })
    }
}

/// An oscilloscope in the netlist: its channels' nets and its ground clip.
#[derive(Debug, Clone)]
pub struct Scope {
    pub path: String,
    /// Each channel's net, if its probe is on something.
    pub channels: [Option<usize>; 4],
    pub com: Option<usize>,
}

impl Scope {
    pub fn all(netlist: &Netlist) -> Vec<Scope> {
        netlist
            .elements
            .iter()
            .filter(|e| e.cell == CellId::new(STD_LIBRARY, "oscilloscope"))
            .map(|e| {
                let net = |port: &str| {
                    let n = e.ports.iter().find(|(p, _)| p == port).map(|(_, n)| *n)?;
                    // A probe on nothing is a net with only itself on it.
                    (netlist.nets[n].pins.len() > 1 || n == 0).then_some(n)
                };
                Scope {
                    path: e.path.clone(),
                    channels: [net("ch1"), net("ch2"), net("ch3"), net("ch4")],
                    com: net("com"),
                }
            })
            .collect()
    }
}

/// A part operated live, as found in the design.
#[derive(Debug, Clone)]
struct Control {
    path: String,
    kind: ControlKind,
    /// Its SPICE devices (the switch's resistor, the dial's source, the pot's halves).
    devices: Vec<String>,
    /// A potentiometer's end-to-end resistance.
    ohms: f64,
    /// Its cell, and its numeric parameters: what a sensor's reading is worked out from.
    cell: String,
    params: BTreeMap<String, f64>,
}

/// Commands from the window.
#[derive(Debug, Clone)]
pub enum Command {
    /// Sets a part's state: `"true"`/`"false"`, a voltage, a position.
    Set(String, String),
    /// Simulated seconds per real second.
    Speed(f64),
    Pause(bool),
    /// The nets to keep recent samples of (besides scope channels).
    Watch(Vec<usize>),
}

/// Recent samples of one net.
#[derive(Debug, Clone, Default)]
pub struct Trace {
    pub analog: VecDeque<(f64, f64)>,
    pub logic: Vec<(Time, Value)>,
}

/// What the running session shares with the window.
#[derive(Debug, Default)]
pub struct Shared {
    /// Simulated time reached.
    pub time: f64,
    pub speed: f64,
    pub paused: bool,
    /// How much of real time the simulation needs to keep up (over 1: falling behind).
    pub load: f64,
    pub error: Option<String>,
    /// The latest voltage on each flat net (analog nets).
    pub voltages: Vec<Option<f64>>,
    /// The latest logic value on each flat net (digital nets).
    pub logic: Vec<Option<Value>>,
    /// Recent samples, by flat net.
    pub traces: BTreeMap<usize, Trace>,
    /// How much history the traces keep, in simulated seconds.
    pub window: f64,
    pub readings: BTreeMap<String, Reading>,
    /// How brightly each LED and lamp shines (0 dark, 1 fully lit) and its colour.
    pub glow: BTreeMap<String, (f32, [u8; 3])>,
    /// Each operated part's state, by instance path.
    pub states: BTreeMap<String, String>,
    /// Parts destroyed so far.
    pub events: Vec<circuiter_physics::Event>,
    /// Bumped on every update, so the window knows to repaint.
    pub frame: u64,
}

/// A circuit running live on its own thread.
#[derive(Debug)]
pub struct Session {
    pub shared: Arc<Mutex<Shared>>,
    pub scopes: Vec<Scope>,
    /// Operated parts: instance path and kind.
    pub controls: Vec<(String, ControlKind)>,
    pub netlist: Netlist,
    tx: Sender<Command>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The parameters of the instance at `path` (from `top`) and its cell.
fn instance(
    design: &Design,
    top: &CellId,
    path: &str,
) -> Option<(BTreeMap<String, String>, CellDef)> {
    let mut cell = top.clone();
    let mut found = None;
    for name in path.split('.') {
        let def = design.cell(&cell)?;
        let inst = def.schematic.as_ref()?.instances.iter().find(|i| i.name == name)?;
        let (child, cdef) = design.resolve(&inst.cell, &cell.library)?;
        found = Some((inst.params.clone(), cdef.clone()));
        cell = child;
    }
    found
}

/// Every parameter of the instance at `path`: its own values, then its cell's defaults.
pub(crate) fn instance_params(
    design: &Design,
    top: &CellId,
    path: &str,
) -> BTreeMap<String, String> {
    let Some((params, def)) = instance(design, top, path) else { return BTreeMap::new() };
    let mut out: BTreeMap<String, String> =
        def.cell.params.iter().filter_map(|p| Some((p.name.clone(), p.default.clone()?))).collect();
    out.extend(params);
    out
}

fn param_of(params: &BTreeMap<String, String>, def: &CellDef, name: &str) -> String {
    params.get(name).cloned().or_else(|| def.cell.param(name)?.default.clone()).unwrap_or_default()
}

fn number(s: &str) -> f64 {
    circuiter_model::units::parse_quantity(s, None).unwrap_or(0.0)
}

/// What each operated part sets on the analog side for state `value`: (device, value).
fn analog_settings(c: &Control, value: &str) -> Vec<(String, f64)> {
    let on = matches!(value.trim(), "true" | "1");
    match c.kind {
        ControlKind::Switch | ControlKind::Pushbutton => {
            c.devices.iter().map(|d| (d.clone(), if on { 0.05 } else { 1e9 })).collect()
        }
        ControlKind::Dial => c.devices.iter().map(|d| (d.clone(), number(value))).collect(),
        ControlKind::Potentiometer => {
            let pos = number(value).clamp(0.0, 1.0);
            let mut out = Vec::new();
            if let Some(a) = c.devices.first() {
                out.push((a.clone(), c.ohms * pos + 1e-3));
            }
            if let Some(b) = c.devices.get(1) {
                out.push((b.clone(), c.ohms * (1.0 - pos) + 1e-3));
            }
            out
        }
        ControlKind::Changeover => {
            let (rest, thrown) = if on { (1e9, 0.05) } else { (0.05, 1e9) };
            c.devices.iter().cloned().zip([rest, thrown]).collect()
        }
        ControlKind::Light | ControlKind::Sunlight | ControlKind::Temperature => {
            let x = number(value);
            let p = |k: &str| c.params.get(k).copied().unwrap_or(0.0);
            // The device the reading sets, by its kind letter, and what to set it to.
            let (letter, set) = match c.cell.as_str() {
                "ldr" => ('R', p("rdark").min(p("r10") * (x.max(1e-3) / 10.0).powf(-p("gamma")))),
                "photodiode" | "phototransistor" => ('I', x * p("sens")),
                "solar_cell" => ('I', p("isc") * x / 1000.0),
                "thermistor" => {
                    ('R', p("r25") * (p("beta") * (1.0 / (x + 273.15) - 1.0 / 298.15)).exp())
                }
                "lm35" => ('V', x * 0.01),
                _ => return Vec::new(),
            };
            let device = c.devices.iter().find(|d| d.starts_with(letter));
            device.map(|d| vec![(d.clone(), set)]).unwrap_or_default()
        }
        ControlKind::Toggle | ControlKind::Button => Vec::new(),
    }
}

enum Engine {
    Analog { run: Transient, monitor: Option<Monitor> },
    Digital { sim: Simulator },
    Mixed { run: Transient, cosim: Box<Cosim> },
}

impl Session {
    /// Starts `top` running at `speed` simulated seconds per real second.
    pub fn start(
        design: &Design,
        top: &CellId,
        root: &Path,
        speed: f64,
    ) -> Result<Session, String> {
        let digital_only = Netlist::is_digital(design, top);
        let mixed = !digital_only && Netlist::is_mixed(design, top);
        let netlist = if digital_only {
            Netlist::build_digital(design, top)?
        } else if mixed {
            Netlist::build_mixed(design, top)?
        } else {
            Netlist::build(design, top)?
        };
        if let Some(e) = netlist
            .diagnostics
            .iter()
            .find(|d| d.severity == circuiter_model::check::Severity::Error)
        {
            return Err(e.message.clone());
        }
        // The parts to operate, with what they start at.
        let mut controls = Vec::new();
        let mut states = BTreeMap::new();
        let mut elements: Vec<(&str, &CellId, Vec<String>)> = netlist
            .elements
            .iter()
            .map(|e| (e.path.as_str(), &e.cell, e.devices().map(str::to_owned).collect()))
            .collect();
        elements.extend(netlist.digital.iter().map(|e| (e.path.as_str(), &e.cell, Vec::new())));
        for (path, cell, devices) in elements {
            let Some(kind) = ControlKind::of(cell) else { continue };
            let Some((params, def)) = instance(design, top, path) else { continue };
            let ohms = number(&param_of(&params, &def, "R"));
            states.insert(path.to_owned(), param_of(&params, &def, kind.param()));
            let values = def
                .cell
                .params
                .iter()
                .map(|p| (p.name.clone(), number(&param_of(&params, &def, &p.name))))
                .collect();
            controls.push(Control {
                path: path.to_owned(),
                kind,
                devices,
                ohms,
                cell: cell.cell.clone(),
                params: values,
            });
        }
        let initial: Vec<(String, f64)> =
            controls.iter().flat_map(|c| analog_settings(c, &states[&c.path])).collect();
        let tstep = (speed / 500.0).max(1e-12);
        let o = |inputs: Arc<Inputs>| Options {
            power_on: true,
            inputs: Some(inputs),
            ..Options::default()
        };
        let mut digital_handles = Vec::new();
        let (engine, inputs, circuit) = if digital_only {
            let (c, handles) = digital::build_with_inputs(&netlist, root)?;
            digital_handles = handles;
            (Engine::Digital { sim: Simulator::new(c) }, Arc::new(Inputs::default()), None)
        } else if mixed {
            let prepared = cosim::prepare(&netlist, root, &initial)?;
            digital_handles = prepared.digital_inputs.clone();
            let cosim::Prepared { circuit, inputs, mut cosim, .. } = prepared;
            cosim.monitor = Monitor::new(&netlist, &circuit, AMBIENT).ok();
            let mut cosim = Box::new(cosim);
            let run = Transient::start(
                &circuit,
                tstep,
                f64::INFINITY,
                &o(inputs.clone()),
                cosim.as_mut(),
            )
            .map_err(|e| e.message)?;
            (Engine::Mixed { run, cosim }, inputs, Some(circuit))
        } else {
            let circuit =
                Circuit::parse(&netlist.to_spice()).map_err(|e| format!("netlist: {e}"))?;
            let names: Vec<(&str, f64)> = initial.iter().map(|(n, v)| (n.as_str(), *v)).collect();
            let inputs = Arc::new(Inputs::new(&names));
            let mut monitor = Monitor::new(&netlist, &circuit, AMBIENT).ok();
            let run = match &mut monitor {
                Some(m) => Transient::start(&circuit, tstep, f64::INFINITY, &o(inputs.clone()), m),
                None => Transient::start(
                    &circuit,
                    tstep,
                    f64::INFINITY,
                    &o(inputs.clone()),
                    &mut NoObserver,
                ),
            }
            .map_err(|e| e.message)?;
            (Engine::Analog { run, monitor }, inputs, Some(circuit))
        };
        let meters = Meter::all(&netlist);
        let emitters = circuit
            .as_ref()
            .map(|c| crate::glow::Emitter::all(design, top, &netlist, c))
            .unwrap_or_default();
        let scopes = Scope::all(&netlist);
        let shared = Arc::new(Mutex::new(Shared {
            speed,
            states,
            window: speed * 2.0,
            voltages: vec![None; netlist.nets.len()],
            logic: vec![None; netlist.nets.len()],
            ..Shared::default()
        }));
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut worker = Worker {
            engine,
            inputs,
            circuit,
            node_of: netlist.nets.iter().map(|n| n.node.clone()).collect(),
            controls: controls.clone(),
            digital_handles,
            meters: meters.clone(),
            emitters,
            scopes: scopes.clone(),
            watch: Vec::new(),
            speed,
            paused: false,
            shared: shared.clone(),
            rx,
            stop: stop.clone(),
        };
        let thread = std::thread::spawn(move || worker.run());
        Ok(Session {
            shared,
            scopes,
            controls: controls.iter().map(|c| (c.path.clone(), c.kind)).collect(),
            netlist,
            tx,
            stop,
            thread: Some(thread),
        })
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    /// The kind of operated part at `path`, if it is one.
    pub fn control(&self, path: &str) -> Option<ControlKind> {
        self.controls.iter().find(|(p, _)| p == path).map(|(_, k)| *k)
    }
}

struct Worker {
    engine: Engine,
    inputs: Arc<Inputs>,
    circuit: Option<Circuit>,
    node_of: Vec<String>,
    controls: Vec<Control>,
    digital_handles: Vec<digital::InputHandle>,
    meters: Vec<Meter>,
    emitters: Vec<crate::glow::Emitter>,
    scopes: Vec<Scope>,
    watch: Vec<usize>,
    speed: f64,
    paused: bool,
    shared: Arc<Mutex<Shared>>,
    rx: Receiver<Command>,
    stop: Arc<AtomicBool>,
}

impl Worker {
    fn run(&mut self) {
        let mut last = Instant::now();
        let mut traces: BTreeMap<usize, Trace> = BTreeMap::new();
        let mut load = 0.0;
        while !self.stop.load(Ordering::Relaxed) {
            while let Ok(cmd) = self.rx.try_recv() {
                self.command(cmd);
            }
            let frame = Instant::now();
            let real = frame.duration_since(last).min(Duration::from_millis(100)).as_secs_f64();
            last = frame;
            let result = if self.paused { Ok(()) } else { self.advance(self.speed * real, frame) };
            let busy = frame.elapsed().as_secs_f64();
            if real > 0.0 {
                load = 0.8 * load + 0.2 * busy / real.max(1e-3);
            }
            self.publish(&mut traces, load, result.err());
            let spent = frame.elapsed();
            if let Some(rest) = Duration::from_millis(16).checked_sub(spent) {
                std::thread::sleep(rest);
            }
        }
    }

    fn command(&mut self, cmd: Command) {
        match cmd {
            Command::Speed(s) => {
                self.speed = s.max(1e-12);
                if let Ok(mut sh) = self.shared.lock() {
                    sh.speed = self.speed;
                    sh.window = self.speed * 2.0;
                }
            }
            Command::Pause(p) => self.paused = p,
            Command::Watch(nets) => self.watch = nets,
            Command::Set(path, value) => {
                let Some(c) = self.controls.iter().find(|c| c.path == path).cloned() else {
                    return;
                };
                for (device, v) in analog_settings(&c, &value) {
                    if let Some(k) = self.inputs.index(&device) {
                        self.inputs.set(k, v);
                    }
                }
                let on = matches!(value.trim(), "true" | "1");
                for (p, element, handle) in &self.digital_handles {
                    if *p == path {
                        handle.store(u64::from(on), Ordering::Relaxed);
                        if let Engine::Digital { sim } = &mut self.engine {
                            sim.poke(*element);
                        }
                    }
                }
                match &mut self.engine {
                    Engine::Analog { run, .. } | Engine::Mixed { run, .. } => run.discontinuity(),
                    Engine::Digital { .. } => {}
                }
                if let Ok(mut sh) = self.shared.lock() {
                    sh.states.insert(path, value);
                }
            }
        }
    }

    /// Runs `dt` simulated seconds on, giving up after about 30 ms of real time (the
    /// rest waits for the next frame: the run falls behind instead of freezing).
    fn advance(&mut self, dt: f64, started: Instant) -> Result<(), String> {
        let budget = Duration::from_millis(30);
        match &mut self.engine {
            Engine::Digital { sim } => {
                let target = sim.now() + circuiter_logic::from_seconds(dt);
                // Nothing scheduled: time still moves on (inputs land at the right time).
                let slice = circuiter_logic::from_seconds(dt / 8.0).max(1);
                let mut t = sim.now();
                while t < target && started.elapsed() < budget {
                    t = (t + slice).min(target);
                    sim.run_until(t)?;
                    sim.skip_to(t);
                }
                Ok(())
            }
            Engine::Analog { run, monitor } => {
                let target = run.time() + dt;
                let mut t = run.time();
                while t < target - 1e-15 && started.elapsed() < budget {
                    t = (t + dt / 8.0).min(target);
                    match monitor {
                        Some(m) => run.advance(t, m),
                        None => run.advance(t, &mut NoObserver),
                    }
                    .map_err(|e| e.message)?;
                }
                Ok(())
            }
            Engine::Mixed { run, cosim } => {
                let target = run.time() + dt;
                let mut t = run.time();
                while t < target - 1e-15 && started.elapsed() < budget {
                    t = (t + dt / 8.0).min(target);
                    run.advance(t, cosim.as_mut()).map_err(|e| e.message)?;
                }
                Ok(())
            }
        }
    }

    fn publish(&mut self, traces: &mut BTreeMap<usize, Trace>, load: f64, error: Option<String>) {
        let window = self.speed * 2.0;
        // Which nets to sample: the watched ones and every scope channel.
        let mut nets: Vec<usize> = self.watch.clone();
        for s in &self.scopes {
            nets.extend(s.channels.iter().flatten());
            nets.extend(s.com);
        }
        nets.sort_unstable();
        nets.dedup();
        traces.retain(|k, _| nets.contains(k));
        let n = self.node_of.len();
        let mut voltages = vec![None; n];
        let mut logic = vec![None; n];
        let mut readings = BTreeMap::new();
        let mut glow = BTreeMap::new();
        let mut events = Vec::new();
        let time;
        // Analog side: new points, latest voltages, meter readings.
        let analog = match &mut self.engine {
            Engine::Analog { run, monitor } => {
                events = monitor.as_ref().map(|m| m.events.clone()).unwrap_or_default();
                Some(run)
            }
            Engine::Mixed { run, cosim } => {
                events = cosim.monitor.as_ref().map(|m| m.events.clone()).unwrap_or_default();
                Some(run)
            }
            Engine::Digital { .. } => None,
        };
        if let (Some(run), Some(circuit)) = (analog, &self.circuit) {
            time = run.time();
            let points = run.take_points();
            // Lights: averaged over the points since the last frame, or the latest.
            for e in &self.emitters {
                let level = if points.values.is_empty() {
                    e.level(&|k| run.voltage(k))
                } else {
                    let sum: f32 = points
                        .values
                        .iter()
                        .map(|x| e.level(&|k| if k == 0 { 0.0 } else { x[k - 1] }))
                        .sum();
                    sum / points.values.len() as f32
                };
                glow.insert(e.path.clone(), (level, e.color));
            }
            let node = |net: usize| circuit.node(&self.node_of[net]);
            for &net in &nets {
                let Some(k) = node(net) else { continue };
                let tr = traces.entry(net).or_default();
                let step = window / 4000.0;
                for (t, x) in points.time.iter().zip(&points.values) {
                    if tr.analog.back().is_none_or(|(lt, _)| t - lt >= step) {
                        tr.analog.push_back((*t, if k == 0 { 0.0 } else { x[k - 1] }));
                    }
                }
                while tr.analog.front().is_some_and(|(t, _)| *t < time - window) {
                    tr.analog.pop_front();
                }
            }
            for (net, v) in voltages.iter_mut().enumerate() {
                *v = node(net).map(|k| run.voltage(k));
            }
            let x = run.solution();
            let nodes = circuit.nodes.len();
            let amps = |name: &str| circuit.branch_of(name).map(|j| x[nodes - 1 + j]);
            let volts = |net: usize| node(net).map(|k| run.voltage(k));
            for m in &self.meters {
                if let Some(r) = m.read(&volts, &amps) {
                    readings.insert(m.path.clone(), r);
                }
            }
        } else {
            time = match &self.engine {
                Engine::Digital { sim } => circuiter_logic::seconds(sim.now()),
                _ => 0.0,
            };
        }
        // Digital side: values and recent changes.
        let sim = match &mut self.engine {
            Engine::Digital { sim } => Some(sim),
            Engine::Mixed { cosim, .. } => Some(&mut cosim.digital),
            Engine::Analog { .. } => None,
        };
        if let Some(sim) = sim {
            let count = sim.circuit().nets.len().min(n);
            for (net, v) in logic.iter_mut().enumerate().take(count) {
                *v = Some(sim.value(net));
            }
            for &net in &nets {
                if net >= count || voltages[net].is_some() {
                    continue;
                }
                let tr = traces.entry(net).or_default();
                let from = tr.logic.last().map(|(t, _)| *t);
                tr.logic.extend(
                    sim.history(net).iter().filter(|(t, _)| from.is_none_or(|f| *t > f)).copied(),
                );
                let cutoff = circuiter_logic::from_seconds((time - window).max(0.0));
                let keep = tr.logic.partition_point(|(t, _)| *t < cutoff).saturating_sub(1);
                tr.logic.drain(..keep);
            }
            sim.forget_before(circuiter_logic::from_seconds((time - 2.0 * window).max(0.0)));
        }
        if let Ok(mut sh) = self.shared.lock() {
            sh.time = time;
            sh.load = load;
            sh.voltages = voltages;
            sh.logic = logic;
            sh.traces = traces.clone();
            sh.window = window;
            sh.readings = readings;
            sh.glow = glow;
            sh.events = events;
            sh.frame += 1;
            if let Some(e) = error {
                sh.error = Some(e);
                self.stop.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Speeds to choose from: simulated time per real second.
pub const SPEEDS: [f64; 13] =
    [1e-9, 1e-8, 1e-7, 1e-6, 1e-5, 1e-4, 1e-3, 1e-2, 0.1, 0.25, 0.5, 1.0, 2.0];

/// A speed that shows a design's activity: its run time over about four seconds,
/// rounded to the nearest preset.
pub fn default_speed(tstop: f64) -> f64 {
    let want = tstop / 4.0;
    SPEEDS
        .iter()
        .copied()
        .min_by(|a, b| (a.ln() - want.ln()).abs().total_cmp(&(b.ln() - want.ln()).abs()))
        .unwrap_or(1e-3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::sheet_builder::design;

    /// Waits up to three seconds for `ok` to hold on the shared state.
    fn until(s: &Session, what: &str, ok: impl Fn(&Shared) -> bool) {
        let start = Instant::now();
        loop {
            {
                let sh = s.shared.lock().unwrap();
                assert_eq!(sh.error, None);
                if ok(&sh) {
                    return;
                }
                if start.elapsed() > Duration::from_secs(3) {
                    panic!(
                        "{what}: time {} voltages {:?} logic {:?} readings {:?}",
                        sh.time, sh.voltages, sh.logic, sh.readings
                    );
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn sensors_and_a_changeover_switch_respond_while_running() {
        let (d, top) = design(&[
            ("V1", "std:vdc", &[], &[("p", "in"), ("n", "GND")]),
            ("R1", "std:resistor", &[("R", "10k")], &[("a", "in"), ("b", "l")]),
            ("L1", "std:ldr", &[], &[("a", "l"), ("b", "GND")]),
            ("SW1", "std:switch_spdt", &[], &[("com", "in"), ("a", "sa"), ("b", "sb")]),
            ("RA", "std:resistor", &[], &[("a", "sa"), ("b", "GND")]),
            ("RB", "std:resistor", &[], &[("a", "sb"), ("b", "GND")]),
            ("U1", "std:lm35", &[], &[("vs", "in"), ("out", "t"), ("gnd", "GND")]),
        ]);
        let s = Session::start(&d, &top, Path::new("."), 1e-3).unwrap();
        assert_eq!(s.control("L1"), Some(ControlKind::Light));
        assert_eq!(s.control("SW1"), Some(ControlKind::Changeover));
        assert_eq!(s.control("U1"), Some(ControlKind::Temperature));
        let (l, sa, sb, t) = (
            s.netlist.net("l").unwrap(),
            s.netlist.net("sa").unwrap(),
            s.netlist.net("sb").unwrap(),
            s.netlist.net("t").unwrap(),
        );
        // 10 lux: the LDR is 10 kΩ, half the supply.
        until(&s, "dim", |sh| sh.voltages[l].is_some_and(|v| (v - 2.5).abs() < 0.05));
        until(&s, "at rest", |sh| sh.voltages[sa].is_some_and(|v| v > 4.9));
        until(&s, "25 °C", |sh| sh.voltages[t].is_some_and(|v| (v - 0.25).abs() < 0.002));
        s.send(Command::Set("L1".into(), "1000".into()));
        s.send(Command::Set("SW1".into(), "true".into()));
        s.send(Command::Set("U1".into(), "80".into()));
        // 1000 lux: about 400 Ω.
        until(&s, "bright", |sh| sh.voltages[l].is_some_and(|v| v < 0.25));
        until(&s, "thrown", |sh| {
            sh.voltages[sb].is_some_and(|v| v > 4.9) && sh.voltages[sa].is_some_and(|v| v < 0.01)
        });
        until(&s, "80 °C", |sh| sh.voltages[t].is_some_and(|v| (v - 0.8).abs() < 0.002));
    }

    #[test]
    fn an_led_glows_while_its_switch_is_closed() {
        let (d, top) = design(&[
            ("V1", "std:vdc", &[], &[("p", "in"), ("n", "GND")]),
            ("SW1", "std:switch", &[], &[("a", "in"), ("b", "sw")]),
            ("R1", "std:resistor", &[("R", "150")], &[("a", "sw"), ("b", "a")]),
            ("D1", "std:led", &[("color", "green")], &[("a", "a"), ("k", "GND")]),
        ]);
        let s = Session::start(&d, &top, Path::new("."), 1e-3).unwrap();
        until(&s, "dark", |sh| sh.time > 1e-4 && sh.glow.get("D1").is_some_and(|g| g.0 < 0.01));
        s.send(Command::Set("SW1".into(), "true".into()));
        until(&s, "lit", |sh| sh.glow.get("D1").is_some_and(|g| g.0 > 0.8));
        assert_eq!(s.shared.lock().unwrap().glow["D1"].1, crate::glow::color("green"));
    }

    #[test]
    fn a_switch_flipped_while_running_lights_a_load() {
        let (d, top) = design(&[
            ("V1", "std:vdc", &[], &[("p", "in"), ("n", "GND")]),
            ("SW1", "std:switch", &[], &[("a", "in"), ("b", "out")]),
            ("R1", "std:resistor", &[], &[("a", "out"), ("b", "GND")]),
            ("DMM1", "std:multimeter", &[], &[("p", "out"), ("com", "GND")]),
        ]);
        let s = Session::start(&d, &top, Path::new("."), 1e-3).unwrap();
        assert_eq!(s.control("SW1"), Some(ControlKind::Switch));
        let out = s.netlist.net("out").unwrap();
        until(&s, "running, open", |sh| {
            sh.time > 1e-4 && sh.voltages[out].is_some_and(|v| v.abs() < 0.01)
        });
        until(
            &s,
            "the meter reads 0 V",
            |sh| matches!(sh.readings.get("DMM1"), Some(Reading::Volts(v)) if v.abs() < 0.01),
        );
        s.send(Command::Set("SW1".into(), "true".into()));
        until(&s, "closed", |sh| sh.voltages[out].is_some_and(|v| (v - 5.0).abs() < 0.01));
        until(
            &s,
            "the meter reads 5 V",
            |sh| matches!(sh.readings.get("DMM1"), Some(Reading::Volts(v)) if (v - 5.0).abs() < 0.01),
        );
        assert_eq!(s.shared.lock().unwrap().states["SW1"], "true");
    }

    #[test]
    fn a_logic_switch_drives_gates() {
        let (d, top) = design(&[
            ("SW1", "std:toggle", &[], &[("y", "a")]),
            ("U1", "std:not", &[], &[("a", "a"), ("y", "y")]),
        ]);
        let s = Session::start(&d, &top, Path::new("."), 1e-6).unwrap();
        let y = s.netlist.net("y").unwrap();
        until(&s, "off: y = 1", |sh| sh.logic[y] == Some(Value::bit(true)));
        s.send(Command::Set("SW1".into(), "true".into()));
        until(&s, "on: y = 0", |sh| sh.logic[y] == Some(Value::bit(false)));
    }

    #[test]
    fn a_logic_switch_lights_an_led_and_a_meter_reads_its_current() {
        let (d, top) = design(&[
            ("SW1", "std:toggle", &[], &[("y", "drive")]),
            ("R1", "std:resistor", &[("R", "1k")], &[("a", "drive"), ("b", "m")]),
            ("DMM1", "std:multimeter", &[("mode", "A")], &[("p", "m"), ("com", "anode")]),
            ("D1", "std:led", &[], &[("a", "anode"), ("k", "GND")]),
        ]);
        let s = Session::start(&d, &top, Path::new("."), 1e-3).unwrap();
        let anode = s.netlist.net("anode").unwrap();
        until(&s, "off", |sh| sh.time > 1e-4 && sh.voltages[anode].is_some_and(|v| v < 0.2));
        s.send(Command::Set("SW1".into(), "true".into()));
        // 5 V through 1 kΩ and a red LED (about 1.6 V at 3 mA): about 3 mA.
        until(&s, "on", |sh| sh.voltages[anode].is_some_and(|v| v > 1.4));
        until(
            &s,
            "the meter reads the LED's current",
            |sh| matches!(sh.readings.get("DMM1"), Some(Reading::Amps(a)) if (2.5e-3..4e-3).contains(a)),
        );
    }
}

#[cfg(test)]
mod bench_example {
    use super::*;

    /// The bench example runs live: the switch lights its LED, the meters read, the
    /// counter counts when enabled, and the scope sees the filter charging.
    #[test]
    fn the_bench_example_runs_live() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/bench");
        let project = circuiter_model::Project::load(&root).unwrap();
        let design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
        let top = CellId::new("bench", "bench");
        assert!(Netlist::is_mixed(&design, &top));
        let s = Session::start(&design, &top, &root, 2.0).unwrap();
        let wait = |what: &str, ok: &dyn Fn(&Shared) -> bool| {
            let start = Instant::now();
            loop {
                {
                    let sh = s.shared.lock().unwrap();
                    assert_eq!(sh.error, None, "{what}");
                    if ok(&sh) {
                        return;
                    }
                    assert!(
                        start.elapsed() < Duration::from_secs(5),
                        "{what}: t = {}, readings {:?}",
                        sh.time,
                        sh.readings
                    );
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        let net = |n: &str| s.netlist.net(n).unwrap();
        let (led1, count) = (net("led1"), net("count"));
        // The pot's wiper at half of 5 V.
        wait(
            "wiper",
            &|sh| matches!(sh.readings.get("DMM2"), Some(Reading::Volts(v)) if (v - 2.5).abs() < 0.05),
        );
        wait("LED off", &|sh| sh.voltages[led1].is_some_and(|v| v < 0.1));
        s.send(Command::Set("SW1".into(), "true".into()));
        wait(
            "LED on",
            &|sh| matches!(sh.readings.get("DMM1"), Some(Reading::Volts(v)) if (1.4..2.2).contains(v)),
        );
        // Counting only once enabled.
        wait("reset", &|sh| sh.time > 0.4 && sh.logic[count].and_then(|v| v.known()) == Some(0));
        s.send(Command::Set("EN".into(), "true".into()));
        wait("counting", &|sh| sh.logic[count].and_then(|v| v.known()).is_some_and(|c| c >= 2));
        s.send(Command::Set("BTN".into(), "true".into()));
        wait("cleared", &|sh| sh.logic[count].and_then(|v| v.known()) == Some(0));
        // The scope's channels have data.
        let ch2 = s.scopes[0].channels[1].unwrap();
        wait("scope", &|sh| sh.traces.get(&ch2).is_some_and(|t| t.analog.len() > 20));
    }
}
