//! Running designs made of digital parts: from the digital netlist to the logic
//! simulator, with programs loaded into memories, and the history of every net kept
//! for the waveforms.

use circuiter_logic::{Circuit, Simulator, Time, Value, prims};
use circuiter_netlist::Netlist;
use circuiter_riscv::Program;
use std::collections::BTreeMap;
use std::path::Path;

/// What a digital run produced.
#[derive(Debug, Clone, PartialEq)]
pub struct LogicRun {
    /// Every change on each flat net: (time, value).
    pub history: Vec<Vec<(Time, Value)>>,
    /// The time asked for, in seconds.
    pub tstop: f64,
    /// When the run ended: `tstop`, or earlier when the design halted.
    pub end: f64,
    /// A halt part stopped it (the program finished).
    pub halted: bool,
    /// Memories and register files at the end, by instance path.
    pub memories: BTreeMap<String, Vec<Value>>,
}

impl LogicRun {
    /// Flat net `net`'s value at the end.
    pub fn last(&self, net: usize) -> Option<Value> {
        self.history.get(net)?.last().map(|(_, v)| *v)
    }
}

/// Reads a program for a memory: assembly is assembled, executables are loaded.
pub fn load_program(path: &Path) -> Result<Program, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let shown = path.display();
    match ext.as_str() {
        "s" | "asm" => {
            let source =
                std::fs::read_to_string(path).map_err(|e| format!("reading {shown}: {e}"))?;
            circuiter_riscv::assemble(&source).map_err(|errors| {
                let first: Vec<String> =
                    errors.iter().take(5).map(|e| format!("{shown}: {e}")).collect();
                first.join("\n")
            })
        }
        "c" => crate::toolchain::compile_c(path).and_then(|elf| circuiter_riscv::elf::load(&elf)),
        "rs" => {
            crate::toolchain::compile_rust(path).and_then(|elf| circuiter_riscv::elf::load(&elf))
        }
        _ => {
            let bytes = std::fs::read(path).map_err(|e| format!("reading {shown}: {e}"))?;
            circuiter_riscv::elf::load(&bytes).map_err(|e| format!("{shown}: {e}"))
        }
    }
}

/// The logic circuit for a digital netlist: one logic net per flat net, in the same
/// order. Programs named by memories are loaded relative to `root`.
pub fn build(netlist: &Netlist, root: &Path) -> Result<Circuit, String> {
    let mut c = Circuit::new();
    for net in &netlist.nets {
        c.net(&net.name, net.width);
    }
    let mut programs: BTreeMap<String, Program> = BTreeMap::new();
    for e in &netlist.digital {
        if e.model == "verilog" {
            let part = hdl_part(e, root)?;
            let mut nets = Vec::new();
            for spec in part.ports() {
                let net = match e.ports.iter().find(|(p, _)| *p == spec.name) {
                    Some((_, n)) => *n,
                    None => c.net(&format!("{}.{}", e.path, spec.name), spec.width),
                };
                nets.push(net);
            }
            c.add(&e.path, part, &nets)?;
            continue;
        }
        let data = match &e.contents {
            Some(file) => {
                if !programs.contains_key(file) {
                    let p = load_program(&root.join(file))
                        .map_err(|err| format!("{}: {err}", e.path))?;
                    programs.insert(file.clone(), p);
                }
                let p = &programs[file];
                // Instructions go to instruction memories, data to data memories.
                Some(if e.model == "dmem" { p.data.clone() } else { p.text.clone() })
            }
            None => None,
        };
        let part = prims::create(&e.model, &e.params, data.as_deref())
            .map_err(|err| format!("{}: {err}", e.path))?;
        let mut nets = Vec::new();
        for spec in part.ports() {
            let net = match e.ports.iter().find(|(p, _)| *p == spec.name) {
                Some((_, n)) => *n,
                // Not wired: an undriven net of its own.
                None => c.net(&format!("{}.{}", e.path, spec.name), spec.width),
            };
            nets.push(net);
        }
        c.add(&e.path, part, &nets)?;
    }
    Ok(c)
}

/// A part described in Verilog: its module, from the source file it names.
fn hdl_part(
    e: &circuiter_netlist::DigitalElement,
    root: &Path,
) -> Result<Box<dyn circuiter_logic::Primitive>, String> {
    let file =
        e.contents.as_ref().ok_or_else(|| format!("{}: no Verilog source file set", e.path))?;
    let path = root.join(file);
    let src = std::fs::read_to_string(&path)
        .map_err(|err| format!("{}: reading {}: {err}", e.path, path.display()))?;
    let module = e.params.get("module").map(String::as_str).filter(|m| !m.is_empty());
    let params: BTreeMap<String, String> = e
        .params
        .iter()
        .filter(|(k, _)| k.as_str() != "module" && k.as_str() != "delay")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let delay = e
        .params
        .get("delay")
        .and_then(|d| prims::number(d))
        .map_or(circuiter_logic::ns(1.0), circuiter_logic::from_seconds);
    circuiter_hdl::create(&src, module, &params, delay)
        .map_err(|err| format!("{}: {file}: {err}", e.path))
}

/// A cell for module `module` (or the file's only one) in the Verilog file `rel`: its
/// ports and widths from the module, run from the file in the digital simulator.
pub fn cell_from_module(
    root: &Path,
    rel: &Path,
    module: Option<&str>,
) -> Result<circuiter_model::CellDef, String> {
    use circuiter_model::cell::{Cell, CellDef, ParamDef, ParamKind, Port, PortKind};
    let src = std::fs::read_to_string(root.join(rel))
        .map_err(|e| format!("reading {}: {e}", rel.display()))?;
    let modules = circuiter_hdl::parse::modules(&src).map_err(|e| e.to_string())?;
    let m = match module {
        Some(n) => {
            modules.iter().find(|m| m.name == n).ok_or_else(|| format!("no module `{n}`"))?
        }
        None => modules.last().ok_or("no module in the file")?,
    };
    circuiter_model::cell::validate_name(&m.name).map_err(|e| format!("module name: {e}"))?;
    let ports = circuiter_hdl::interface(&src, Some(&m.name), &BTreeMap::new())?;
    let file = rel.to_string_lossy().replace('\\', "/");
    let cell = Cell {
        description: format!("Described in Verilog: module {} in {file}", m.name),
        prefix: "U".into(),
        ports: ports
            .iter()
            .map(|p| Port {
                width: p.width,
                ..Port::new(
                    &p.name,
                    match p.dir {
                        circuiter_hdl::Dir::Input => PortKind::Input,
                        circuiter_hdl::Dir::Output => PortKind::Output,
                        circuiter_hdl::Dir::Inout => PortKind::Bidirectional,
                    },
                )
            })
            .collect(),
        params: vec![ParamDef {
            name: "source".into(),
            kind: ParamKind::Text,
            default: Some(file.clone()),
            description: "The Verilog file the module is in".into(),
        }],
        ..Cell::default()
    };
    let symbol = circuiter_model::builder::generate(&m.name, &cell);
    Ok(CellDef {
        name: m.name.clone(),
        cell,
        symbol: Some(symbol),
        digital: Some(circuiter_model::digital::DigitalView {
            model: "verilog".into(),
            params: BTreeMap::from([("module".to_owned(), m.name.clone())]),
            contents: Some("source".into()),
            ports: BTreeMap::new(),
        }),
        ..CellDef::default()
    })
}

/// Clock periods in the design, in seconds.
pub fn clock_periods(netlist: &Netlist) -> Vec<f64> {
    netlist
        .digital
        .iter()
        .filter(|e| e.model == "clock")
        .filter_map(|e| {
            let f = e.params.get("frequency").and_then(|f| prims::number(f));
            let p = e.params.get("period").and_then(|p| prims::number(p));
            f.map(|f| 1.0 / f).or(p)
        })
        .collect()
}

/// A run long enough to see something: a program until it halts (up to a hundred
/// thousand cycles), otherwise sixty-four cycles of the slowest clock.
pub fn default_stop(netlist: &Netlist) -> f64 {
    let period = clock_periods(netlist)
        .into_iter()
        .fold(None, |a: Option<f64>, p| Some(a.map_or(p, |a| a.max(p))));
    let halts = netlist.digital.iter().any(|e| e.model == "stop");
    match (period, halts) {
        (Some(p), true) => p * 100_000.0,
        (Some(p), false) => p * 64.0,
        (None, _) => 1e-6,
    }
}

/// Runs the circuit to `tstop` seconds, calling `progress` with the fraction done; it
/// returns false to cancel.
pub fn run(
    circuit: Circuit,
    tstop: f64,
    progress: &mut dyn FnMut(f64) -> bool,
) -> Result<LogicRun, String> {
    let end = circuiter_logic::from_seconds(tstop);
    let mut sim = Simulator::new(circuit);
    let mut cancelled = false;
    sim.run_with(end, &mut |t| {
        let go = progress(t as f64 / end.max(1) as f64);
        cancelled |= !go;
        go
    })?;
    if cancelled {
        return Err("cancelled".into());
    }
    let n = sim.circuit().nets.len();
    let history = (0..n).map(|k| sim.history(k).to_vec()).collect();
    let memories = sim
        .circuit()
        .elements
        .iter()
        .filter_map(|e| Some((e.path.clone(), e.primitive.memory()?.to_vec())))
        .collect();
    Ok(LogicRun {
        history,
        tstop,
        end: if sim.halted() { circuiter_logic::seconds(sim.now()) } else { tstop },
        halted: sim.halted(),
        memories,
    })
}
