//! The netlister: flattens a design's hierarchy into one list of nets and leaf
//! elements, and writes it as a SPICE deck.
//!
//! Sub-circuits are expanded in place. A child's port joins the parent net wired to the
//! instance's pin, global nets (ground, supply symbols) are shared by every level, and
//! the ground net `GND` becomes SPICE node `0`. Leaf instances are written from their
//! cell's SPICE template, with parameters as plain numbers and identical model cards
//! merged. Electrical rule checks that need the whole flattened circuit (a missing
//! ground, nets reaching a single pin) are reported here; checks that need device
//! physics, such as DC paths to ground, belong to the simulator.

use circuiter_model::cell::{CellDef, Limit, ParamKind, Physics, ViewKind};
use circuiter_model::check::{Diagnostic, Severity};
use circuiter_model::connectivity::{Connectivity, Item, NetNameSource};
use circuiter_model::spice::{Placeholder, Segment, parse_template};
use circuiter_model::units::parse_quantity;
use circuiter_model::{CellId, Design};
use std::collections::{BTreeMap, BTreeSet};

/// The global net that is the circuit's reference, SPICE node `0`.
pub const GROUND: &str = "GND";

/// A net of the flattened circuit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatNet {
    /// Hierarchical display name: `OUT`, `U1.mid`, `N$3`.
    pub name: String,
    /// The SPICE node name: `0` for ground, otherwise unique ignoring case.
    pub node: String,
    /// The element terminals on this net, as (element path, port).
    pub pins: Vec<(String, String)>,
}

/// A leaf element of the flattened circuit, such as one transistor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatElement {
    /// Instance path from the top cell: `U1.MN`.
    pub path: String,
    pub cell: CellId,
    /// The expanded SPICE line (several lines for a part made of several elements).
    pub line: String,
    /// Each port of the cell and the net it is on.
    pub ports: Vec<(String, usize)>,
    /// The cell's physics with this instance's parameters filled in, if it has any.
    pub physics: Option<Physics>,
}

impl FlatElement {
    /// The names of the SPICE elements this part expands to.
    pub fn devices(&self) -> impl Iterator<Item = &str> {
        self.line.lines().filter_map(|l| l.split_whitespace().next())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Netlist {
    pub top: CellId,
    /// Index 0 is always ground.
    pub nets: Vec<FlatNet>,
    pub elements: Vec<FlatElement>,
    /// Expanded `.model` and `.subckt` cards, each written once.
    pub models: Vec<String>,
    /// For each expanded sheet, by instance path (`""` for the top), the flat net of
    /// each of its local nets, in the order of [`Connectivity::nets`].
    pub sheets: BTreeMap<String, Vec<usize>>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Netlist {
    /// Flattens the hierarchy under `top`. Problems that make an element impossible to
    /// write leave it out and are reported in [`Netlist::diagnostics`].
    pub fn build(design: &Design, top: &CellId) -> Result<Netlist, String> {
        let def = design.cell(top).ok_or_else(|| format!("cell {top} not found"))?;
        if design.select_view(def, None) != Some(ViewKind::Schematic) {
            return Err(format!("{top} has no schematic to simulate"));
        }
        let mut b = Builder::new(design, top);
        b.sheet(top, def, "", 0, &BTreeMap::new(), &mut vec![top.clone()]);
        Ok(b.finish())
    }

    /// The flat net with this display name or SPICE node name (ignoring case).
    pub fn net(&self, name: &str) -> Option<usize> {
        self.nets
            .iter()
            .position(|n| n.name == name)
            .or_else(|| self.nets.iter().position(|n| n.node.eq_ignore_ascii_case(name)))
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }

    /// The circuit as a SPICE deck, without analysis commands or `.end`, so a
    /// simulator front end can append its own.
    pub fn to_spice(&self) -> String {
        let mut out = format!("* Circuiter netlist of {}\n", self.top);
        for e in &self.elements {
            out.push_str(&e.line);
            out.push('\n');
        }
        for m in &self.models {
            out.push_str(m);
            out.push('\n');
        }
        out
    }
}

/// A net name candidate. The flat net is named after its best candidate: global names
/// first, then names given by a label or port at the shallowest level, then automatic
/// names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Candidate {
    class: u8,
    depth: usize,
    source: u8,
    name: String,
}

enum Part {
    Text(String),
    Net(usize),
}

struct PendingElement {
    path: String,
    cell: CellId,
    parts: Vec<Part>,
    ports: Vec<(String, usize)>,
    physics: Option<Physics>,
}

struct Builder<'a> {
    design: &'a Design,
    top: CellId,
    parent: Vec<usize>,
    candidates: Vec<Candidate>,
    globals: BTreeMap<String, usize>,
    elements: Vec<PendingElement>,
    models: Vec<String>,
    model_names: BTreeMap<(CellId, String), String>,
    model_counts: BTreeMap<String, usize>,
    sheets: BTreeMap<String, Vec<usize>>,
    diagnostics: Vec<Diagnostic>,
    /// Names of every global net in the design: labels with these names join them.
    global_names: BTreeSet<String>,
    /// Element terminals connected through hidden (global) ports.
    hidden: BTreeSet<(String, String)>,
}

const MODEL_SENTINEL: &str = "\u{0}model\u{0}";

impl<'a> Builder<'a> {
    fn new(design: &'a Design, top: &CellId) -> Self {
        let mut b = Builder {
            design,
            top: top.clone(),
            parent: Vec::new(),
            candidates: Vec::new(),
            globals: BTreeMap::new(),
            elements: Vec::new(),
            models: Vec::new(),
            model_names: BTreeMap::new(),
            model_counts: BTreeMap::new(),
            sheets: BTreeMap::new(),
            diagnostics: Vec::new(),
            global_names: design
                .libraries()
                .flat_map(|l| l.cells.values())
                .flat_map(|d| {
                    d.cell
                        .global_net
                        .iter()
                        .cloned()
                        .chain(d.cell.ports.iter().filter_map(|p| p.global.clone()))
                })
                .collect(),
            hidden: BTreeSet::new(),
        };
        let ground = b.new_net(Candidate { class: 0, depth: 0, source: 0, name: GROUND.into() });
        b.globals.insert(GROUND.to_owned(), ground);
        b
    }

    fn report(&mut self, severity: Severity, message: String) {
        self.diagnostics.push(Diagnostic {
            severity,
            location: format!("{}/simulation", self.top),
            message,
        });
    }

    fn new_net(&mut self, candidate: Candidate) -> usize {
        self.parent.push(self.parent.len());
        self.candidates.push(candidate);
        self.parent.len() - 1
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            // Ground stays the root of its set so it remains net 0.
            let (root, child) = if b == 0 { (b, a) } else { (a, b) };
            self.parent[child] = root;
        }
    }

    fn global(&mut self, name: &str) -> usize {
        if let Some(&id) = self.globals.get(name) {
            return id;
        }
        let id = self.new_net(Candidate { class: 0, depth: 0, source: 0, name: name.into() });
        self.globals.insert(name.to_owned(), id);
        id
    }

    fn sheet(
        &mut self,
        id: &CellId,
        def: &CellDef,
        path: &str,
        depth: usize,
        ports: &BTreeMap<String, usize>,
        stack: &mut Vec<CellId>,
    ) {
        let schematic = def.schematic.as_ref().expect("caller checked the view");
        let conn = Connectivity::analyze(self.design, &id.library, schematic);
        let join = |a: &str, b: &str| if a.is_empty() { b.to_owned() } else { format!("{a}.{b}") };

        let mut local = Vec::with_capacity(conn.nets.len());
        for net in &conn.nets {
            let (class, source) = match net.source {
                NetNameSource::Global => (0, 0),
                NetNameSource::Port => (1, 1),
                NetNameSource::Label => (1, 0),
                NetNameSource::Auto => (2, 0),
            };
            let name = match net.source {
                NetNameSource::Global => net.name.clone(),
                _ => join(path, &net.name),
            };
            let flat = self.new_net(Candidate { class, depth, source, name });
            // Global symbols join their net everywhere; so does a label named like one.
            if net.source == NetNameSource::Global
                || (net.source == NetNameSource::Label && self.global_names.contains(&net.name))
            {
                let g = self.global(&net.name);
                self.union(flat, g);
            }
            for item in &net.items {
                if let Item::Port(p) = item {
                    if let Some(&outer) = ports.get(&schematic.ports[*p].port) {
                        self.union(flat, outer);
                    }
                }
            }
            local.push(flat);
        }
        self.sheets.insert(path.to_owned(), local.clone());

        for (i, inst) in schematic.instances.iter().enumerate() {
            let inst_path = join(path, &inst.name);
            let Some((child_id, child)) = self.design.resolve(&inst.cell, &id.library) else {
                self.report(Severity::Error, format!("{inst_path}: unknown cell `{}`", inst.cell));
                continue;
            };
            let mut pins: BTreeMap<String, usize> = conn
                .pins
                .iter()
                .enumerate()
                .filter(|(_, p)| p.instance == i)
                .filter_map(|(j, p)| Some((p.port.clone(), local[conn.net_of(Item::Pin(j))?])))
                .collect();
            // Hidden ports join their global net.
            for port in &child.cell.ports {
                if let Some(g) = &port.global {
                    let net = self.global(g);
                    pins.entry(port.name.clone()).or_insert(net);
                    self.hidden.insert((inst_path.clone(), port.name.clone()));
                }
            }
            match self.design.select_view(child, inst.view) {
                Some(ViewKind::Schematic) => {
                    if stack.contains(&child_id) {
                        self.report(
                            Severity::Error,
                            format!("{inst_path}: {child_id} contains itself"),
                        );
                        continue;
                    }
                    stack.push(child_id.clone());
                    self.sheet(&child_id, child, &inst_path, depth + 1, &pins, stack);
                    stack.pop();
                }
                Some(ViewKind::Spice) => self.element(&inst_path, &child_id, child, inst, &pins),
                None if child.cell.global_net.is_some() => {}
                None => self.report(
                    Severity::Error,
                    format!("{inst_path}: {child_id} has no schematic or SPICE model to simulate"),
                ),
            }
        }
    }

    fn element(
        &mut self,
        path: &str,
        cell: &CellId,
        def: &CellDef,
        inst: &circuiter_model::schematic::Instance,
        pins: &BTreeMap<String, usize>,
    ) {
        let spice = def.spice.as_ref().expect("caller checked the view");
        let inst_name = spice_name(path);
        // A pin with no symbol pin (or none at all) still needs a node; give it its own.
        let mut ports = Vec::new();
        for port in &def.cell.ports {
            let net = match pins.get(&port.name) {
                Some(&n) => n,
                None => self.new_net(Candidate {
                    class: 2,
                    depth: usize::MAX,
                    source: 0,
                    name: format!("{path}.{}", port.name),
                }),
            };
            ports.push((port.name.clone(), net));
        }

        let mut param = |name: &str| -> Result<String, String> {
            let p = def.cell.param(name).ok_or_else(|| format!("no parameter `{name}`"))?;
            let raw = inst
                .params
                .get(name)
                .or(p.default.as_ref())
                .ok_or_else(|| format!("parameter `{name}` is not set"))?;
            match &p.kind {
                ParamKind::Real { unit } => {
                    let unit = (!unit.is_empty()).then_some(unit.as_str());
                    parse_quantity(raw, unit)
                        .map(spice_number)
                        .map_err(|e| format!("parameter `{name}`: {e}"))
                }
                _ => Ok(raw.trim().to_owned()),
            }
        };

        // Model cards first, so the element can use the merged model's name.
        let mut model_name = None;
        if !spice.models.is_empty() {
            let mut cards = Vec::new();
            for card in &spice.models {
                match expand(card, &inst_name, &mut param, &|_| None) {
                    Ok(parts) => cards.push(parts),
                    Err(e) => {
                        self.report(Severity::Error, format!("{path}: {e}"));
                        return;
                    }
                }
            }
            let text = cards.join("\n");
            let key = (cell.clone(), text.clone());
            let name = match self.model_names.get(&key) {
                Some(n) => n.clone(),
                None => {
                    let count = self.model_counts.entry(spice_name(&cell.cell)).or_default();
                    *count += 1;
                    let name = format!("{}_{count}", spice_name(&cell.cell));
                    self.models.extend(cards.iter().map(|c| c.replace(MODEL_SENTINEL, &name)));
                    self.model_names.insert(key, name.clone());
                    name
                }
            };
            model_name = Some(name);
        }

        let node_of = |port: &str| ports.iter().find(|(p, _)| p == port).map(|(_, n)| *n);
        let mut parts = Vec::new();
        let segments = match parse_template(&spice.template) {
            Ok(s) => s,
            Err(e) => {
                self.report(Severity::Error, format!("{path}: {e}"));
                return;
            }
        };
        for seg in segments {
            let part = match seg {
                Segment::Text(t) => Part::Text(t.to_owned()),
                Segment::Placeholder(Placeholder::Instance) => Part::Text(inst_name.clone()),
                Segment::Placeholder(Placeholder::Model) => match &model_name {
                    Some(m) => Part::Text(m.clone()),
                    None => {
                        self.report(
                            Severity::Error,
                            format!("{path}: `${{model}}` without models"),
                        );
                        return;
                    }
                },
                Segment::Placeholder(Placeholder::Node(p)) => match node_of(p) {
                    Some(n) => Part::Net(n),
                    None => {
                        self.report(Severity::Error, format!("{path}: `{p}` is not a port"));
                        return;
                    }
                },
                Segment::Placeholder(Placeholder::Param(p)) => match param(p) {
                    Ok(v) => Part::Text(v),
                    Err(e) => {
                        self.report(Severity::Error, format!("{path}: {e}"));
                        return;
                    }
                },
            };
            parts.push(part);
        }
        let physics = match def.cell.physics.as_ref().map(|p| resolve_physics(p, &mut param)) {
            Some(Ok(p)) => Some(p),
            Some(Err(e)) => {
                self.report(Severity::Error, format!("{path}: physics: {e}"));
                None
            }
            None => None,
        };
        self.elements.push(PendingElement {
            path: path.to_owned(),
            cell: cell.clone(),
            parts,
            ports,
            physics,
        });
    }

    fn finish(mut self) -> Netlist {
        // Number the root nets, ground first, naming each after its best candidate.
        let n = self.parent.len();
        let mut best: BTreeMap<usize, Candidate> = BTreeMap::new();
        for i in 0..n {
            let root = self.find(i);
            let c = self.candidates[i].clone();
            match best.get(&root) {
                Some(b) if *b <= c => {}
                _ => {
                    best.insert(root, c);
                }
            }
        }
        let mut index: BTreeMap<usize, usize> = BTreeMap::new();
        let mut nets = Vec::new();
        let mut taken: BTreeSet<String> = BTreeSet::from(["0".to_owned()]);
        let ground_root = self.find(0);
        for (&root, candidate) in std::iter::once((&ground_root, &best[&ground_root]))
            .chain(best.iter().filter(|(r, _)| **r != ground_root))
        {
            let node = if root == ground_root {
                "0".to_owned()
            } else {
                let base = spice_name(&candidate.name);
                let mut node = base.clone();
                let mut k = 2;
                while taken.contains(&node.to_ascii_lowercase()) {
                    node = format!("{base}_{k}");
                    k += 1;
                }
                taken.insert(node.to_ascii_lowercase());
                node
            };
            index.insert(root, nets.len());
            let name = if root == ground_root { GROUND.to_owned() } else { candidate.name.clone() };
            nets.push(FlatNet { name, node, pins: Vec::new() });
        }
        let flat = |b: &mut Self, id: usize| index[&b.find(id)];

        let pending = std::mem::take(&mut self.elements);
        let mut elements = Vec::with_capacity(pending.len());
        for e in pending {
            let ports: Vec<(String, usize)> =
                e.ports.iter().map(|(p, id)| (p.clone(), flat(&mut self, *id))).collect();
            let mut line = String::new();
            for part in &e.parts {
                match part {
                    Part::Text(t) => line.push_str(t),
                    Part::Net(id) => {
                        let net = flat(&mut self, *id);
                        line.push_str(&nets[net].node);
                    }
                }
            }
            for (port, net) in &ports {
                nets[*net].pins.push((e.path.clone(), port.clone()));
            }
            elements.push(FlatElement {
                path: e.path,
                cell: e.cell,
                line,
                ports,
                physics: e.physics,
            });
        }
        let sheets = std::mem::take(&mut self.sheets)
            .into_iter()
            .map(|(path, ids)| (path, ids.into_iter().map(|id| flat(&mut self, id)).collect()))
            .collect();

        // Rules that need the whole circuit. A global supply that only hidden power pins
        // reach has nothing driving it.
        for net in nets.iter().skip(1) {
            let hidden = net.pins.iter().filter(|p| self.hidden.contains(p)).count();
            if hidden > 0 && hidden == net.pins.len() {
                self.report(
                    Severity::Warning,
                    format!(
                        "{} powers {} part(s) through their hidden supply pins, but nothing \
                         drives it: connect a voltage source to a {} symbol",
                        net.name,
                        net.pins.iter().map(|(p, _)| p.as_str()).collect::<BTreeSet<_>>().len(),
                        net.name
                    ),
                );
            }
        }

        if nets[0].pins.is_empty() {
            // Physically a circuit works without an earth reference (a battery-powered
            // one does), so this is a warning: voltages are then measured against board
            // leakage and their absolute values are arbitrary.
            self.report(
                Severity::Warning,
                "no ground: nothing connects to GND, so voltages have no 0 V reference \
                 and only differences between nets are meaningful. Place a ground symbol \
                 (std:gnd) on the circuit's 0 V net"
                    .into(),
            );
        }
        for net in nets.iter().skip(1) {
            if let [(path, port)] = net.pins.as_slice() {
                let msg = format!("net {} reaches only one pin, {path} {port}", net.name);
                self.report(Severity::Warning, msg);
            }
        }

        Netlist {
            top: self.top,
            nets,
            elements,
            models: self.models,
            sheets,
            diagnostics: self.diagnostics,
        }
    }
}

/// Fills a cell's physics expressions with an instance's parameter values.
fn resolve_physics(
    p: &Physics,
    param: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Physics, String> {
    let mut fill = |text: &str| -> Result<String, String> {
        let mut out = String::new();
        for seg in parse_template(text)? {
            match seg {
                Segment::Text(t) => out.push_str(t),
                Segment::Placeholder(Placeholder::Param(name)) => out.push_str(&param(name)?),
                Segment::Placeholder(_) => {
                    return Err(format!("only `${{param.X}}` may appear in `{text}`"));
                }
            }
        }
        Ok(out)
    };
    let mut opt = |v: &Option<String>| v.as_deref().map(&mut fill).transpose();
    let thermal_resistance = opt(&p.thermal_resistance)?;
    let heat_capacity = opt(&p.heat_capacity)?;
    let max_temperature = opt(&p.max_temperature)?;
    let limits = p
        .limits
        .iter()
        .map(|l| {
            Ok(match l {
                Limit::Voltage { ports, max } => {
                    Limit::Voltage { ports: ports.clone(), max: fill(max)? }
                }
                Limit::Current { port, max } => {
                    Limit::Current { port: port.clone(), max: fill(max)? }
                }
                Limit::Energy { port, rated, max } => {
                    Limit::Energy { port: port.clone(), rated: fill(rated)?, max: fill(max)? }
                }
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Physics {
        thermal_resistance,
        heat_capacity,
        max_temperature,
        limits,
        failure: p.failure.clone(),
    })
}

/// Expands a model card's placeholders. `${model}` becomes a sentinel replaced once the
/// merged name is known.
fn expand(
    text: &str,
    inst: &str,
    param: &mut dyn FnMut(&str) -> Result<String, String>,
    node: &dyn Fn(&str) -> Option<String>,
) -> Result<String, String> {
    let mut out = String::new();
    for seg in parse_template(text)? {
        match seg {
            Segment::Text(t) => out.push_str(t),
            Segment::Placeholder(Placeholder::Instance) => out.push_str(inst),
            Segment::Placeholder(Placeholder::Model) => out.push_str(MODEL_SENTINEL),
            Segment::Placeholder(Placeholder::Param(p)) => out.push_str(&param(p)?),
            Segment::Placeholder(Placeholder::Node(p)) => {
                out.push_str(&node(p).ok_or_else(|| format!("`${{node.{p}}}` in a model card"))?)
            }
        }
    }
    Ok(out)
}

/// A name SPICE accepts: letters, digits, `_` and `.`; anything else becomes `_`.
pub fn spice_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

/// A number as SPICE reads it: plain digits, no SI prefix (SPICE takes `M` as milli),
/// rounded to 12 significant digits so `180n` is written `1.8e-7`, not
/// `1.8000000000000002e-7`.
pub fn spice_number(v: f64) -> String {
    let rounded: f64 = format!("{v:.11e}").parse().unwrap_or(v);
    let plain = format!("{rounded}");
    let sci = format!("{rounded:e}");
    if sci.len() < plain.len() { sci } else { plain }
}
