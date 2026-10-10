//! Structural checks on cells and their views: every reference resolves, every view
//! agrees with the cell's interface, and the hierarchy is not recursive.
//!
//! Connectivity and electrical rules (floating nets, missing ground) belong to the
//! netlister, which understands nets.

use crate::cell::{CellDef, ViewKind, validate_name};
use crate::design::{CellId, Design};
use crate::geom::Length;
use crate::spice::{Placeholder, Segment, parse_template};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Where the problem is, e.g. `demo:inverter/schematic`.
    pub location: String,
    pub message: String,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(f, "{level}: {}: {}", self.location, self.message)
    }
}

/// Checks every cell in the named libraries, then the hierarchy for recursion.
pub fn check_libraries(design: &Design, libraries: &[&str]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for &name in libraries {
        let Some(lib) = design.library(name) else {
            out.push(Diagnostic {
                severity: Severity::Error,
                location: name.to_owned(),
                message: "library not loaded".to_owned(),
            });
            continue;
        };
        for def in lib.cells.values() {
            check_cell(design, &CellId::new(name, &def.name), def, &mut out);
        }
    }
    check_recursion(design, libraries, &mut out);
    out
}

/// Checks a single cell and all of its views.
pub fn check_cell(design: &Design, id: &CellId, def: &CellDef, out: &mut Vec<Diagnostic>) {
    let mut report = |severity, view: Option<&str>, message: String| {
        let location = match view {
            Some(v) => format!("{id}/{v}"),
            None => id.to_string(),
        };
        out.push(Diagnostic { severity, location, message });
    };
    use Severity::{Error, Warning};
    let cell = &def.cell;

    // Interface.
    let mut seen = BTreeSet::new();
    for port in &cell.ports {
        if let Err(e) = validate_name(&port.name) {
            report(Error, None, format!("port name: {e}"));
        }
        if !seen.insert(&port.name) {
            report(Error, None, format!("port `{}` is defined twice", port.name));
        }
    }
    let mut seen = BTreeSet::new();
    for param in &cell.params {
        if let Err(e) = validate_name(&param.name) {
            report(Error, None, format!("parameter name: {e}"));
        }
        if !seen.insert(&param.name) {
            report(Error, None, format!("parameter `{}` is defined twice", param.name));
        }
        if let Some(default) = &param.default {
            if let Err(e) = param.kind.validate(default) {
                report(Error, None, format!("default for `{}`: {e}", param.name));
            }
        }
    }
    if cell.global_net.is_some() && cell.ports.len() != 1 {
        report(Error, None, "a global-net cell must have exactly one port".to_owned());
    }
    if def.schematic.is_none() && def.spice.is_none() && cell.global_net.is_none() {
        report(Warning, None, "no schematic or SPICE view, so it cannot be simulated".to_owned());
    }

    // Symbol.
    if let Some(symbol) = &def.symbol {
        for pin in &symbol.pins {
            if cell.port(&pin.port).is_none() {
                report(Error, Some("symbol"), format!("pin for unknown port `{}`", pin.port));
            }
        }
        for port in &cell.ports {
            match symbol.pins.iter().filter(|p| p.port == port.name).count() {
                0 if port.global.is_some() => {}
                0 => report(Error, Some("symbol"), format!("port `{}` has no pin", port.name)),
                1 => {}
                n => report(Error, Some("symbol"), format!("port `{}` has {n} pins", port.name)),
            }
        }
        let grid = Length::from_mils(100).nm();
        for pin in &symbol.pins {
            if pin.at.x.nm() % grid != 0 || pin.at.y.nm() % grid != 0 {
                report(
                    Warning,
                    Some("symbol"),
                    format!("pin `{}` is off the 100-mil grid, so wires can't reach it", pin.port),
                );
            }
        }
    }

    // Physics.
    if let Some(physics) = &cell.physics {
        let mut port = |name: &str| {
            if cell.port(name).is_none() {
                report(Error, Some("physics"), format!("`{name}` is not a port"));
            }
        };
        for limit in &physics.limits {
            match limit {
                crate::cell::Limit::Voltage { ports: (a, b), .. } => {
                    port(a);
                    port(b);
                }
                crate::cell::Limit::Current { port: p, .. }
                | crate::cell::Limit::Energy { port: p, .. } => port(p),
            }
        }
        if let crate::cell::FailureMode::Short(a, b) = &physics.failure.mode {
            port(a);
            port(b);
        }
        let mut texts: Vec<&String> =
            [&physics.thermal_resistance, &physics.heat_capacity, &physics.max_temperature]
                .into_iter()
                .flatten()
                .collect();
        for limit in &physics.limits {
            match limit {
                crate::cell::Limit::Voltage { max, .. }
                | crate::cell::Limit::Current { max, .. } => texts.push(max),
                crate::cell::Limit::Energy { rated, max, .. } => texts.extend([rated, max]),
            }
        }
        for text in texts {
            match parse_template(text) {
                Err(e) => report(Error, Some("physics"), e),
                Ok(segments) => {
                    for seg in segments {
                        match seg {
                            Segment::Placeholder(Placeholder::Param(p))
                                if cell.param(p).is_none() =>
                            {
                                report(
                                    Error,
                                    Some("physics"),
                                    format!("`${{param.{p}}}` is not a parameter"),
                                )
                            }
                            Segment::Placeholder(Placeholder::Param(_)) | Segment::Text(_) => {}
                            Segment::Placeholder(_) => report(
                                Error,
                                Some("physics"),
                                format!("only parameters may appear in `{text}`"),
                            ),
                        }
                    }
                }
            }
        }
    }

    // SPICE.
    if let Some(spice) = &def.spice {
        for text in std::iter::once(&spice.template).chain(&spice.models) {
            match parse_template(text) {
                Err(e) => report(Error, Some("spice"), e),
                Ok(segments) => {
                    for seg in segments {
                        match seg {
                            Segment::Placeholder(Placeholder::Node(p))
                                if cell.port(p).is_none() =>
                            {
                                report(
                                    Error,
                                    Some("spice"),
                                    format!("`${{node.{p}}}` is not a port"),
                                );
                            }
                            Segment::Placeholder(Placeholder::Param(p))
                                if cell.param(p).is_none() =>
                            {
                                report(
                                    Error,
                                    Some("spice"),
                                    format!("`${{param.{p}}}` is not a parameter"),
                                );
                            }
                            Segment::Placeholder(Placeholder::Model) if spice.models.is_empty() => {
                                report(
                                    Error,
                                    Some("spice"),
                                    "`${model}` is used but there are no model cards".to_owned(),
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    // Schematic.
    let Some(schematic) = &def.schematic else { return };
    let view = Some("schematic");
    let mut names = BTreeSet::new();
    for inst in &schematic.instances {
        let who = format!("instance {}", inst.name);
        if let Err(e) = validate_name(&inst.name) {
            report(Error, view, format!("{who}: {e}"));
        }
        if !names.insert(&inst.name) {
            report(Error, view, format!("{who}: name is used more than once"));
        }
        let Some((child_id, child)) = design.resolve(&inst.cell, &id.library) else {
            report(Error, view, format!("{who}: unknown cell `{}`", inst.cell));
            continue;
        };
        if child.symbol.is_none() {
            report(Error, view, format!("{who}: cell {child_id} has no symbol to place"));
        }
        for (param, value) in &inst.params {
            match child.cell.param(param) {
                None => {
                    report(Error, view, format!("{who}: {child_id} has no parameter `{param}`"))
                }
                Some(def) => {
                    if let Err(e) = def.kind.validate(value) {
                        report(Error, view, format!("{who}: parameter `{param}`: {e}"));
                    }
                }
            }
        }
        for param in &child.cell.params {
            if param.default.is_none() && !inst.params.contains_key(&param.name) {
                report(
                    Error,
                    view,
                    format!("{who}: required parameter `{}` is not set", param.name),
                );
            }
        }
        if let Some(forced) = inst.view {
            if !child.has_view(forced) {
                report(Error, view, format!("{who}: {child_id} has no {forced} view"));
            }
        }
    }
    for wire in &schematic.wires {
        if wire.points.len() < 2 {
            report(
                Error,
                view,
                format!(
                    "wire at {} has fewer than two points",
                    wire.points.first().map_or("?".to_owned(), ToString::to_string)
                ),
            );
        }
    }
    for label in &schematic.labels {
        if label.net.trim().is_empty() {
            report(Error, view, format!("net label at {} has no name", label.at));
        }
    }
    let mut placed: BTreeMap<&str, usize> = BTreeMap::new();
    for port in &schematic.ports {
        if cell.port(&port.port).is_none() {
            report(Error, view, format!("schematic port `{}` is not a cell port", port.port));
        }
        *placed.entry(port.port.as_str()).or_default() += 1;
    }
    for port in &cell.ports {
        match placed.get(port.name.as_str()) {
            None => report(Error, view, format!("cell port `{}` is not placed", port.name)),
            Some(n) if *n > 1 => {
                report(Error, view, format!("cell port `{}` is placed {n} times", port.name));
            }
            _ => {}
        }
    }
}

/// Reports cells that (transitively) instantiate themselves through schematic views.
fn check_recursion(design: &Design, libraries: &[&str], out: &mut Vec<Diagnostic>) {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Visiting,
        Done,
    }
    fn visit(
        design: &Design,
        id: &CellId,
        marks: &mut BTreeMap<CellId, Mark>,
        path: &mut Vec<CellId>,
        out: &mut Vec<Diagnostic>,
    ) {
        match marks.get(id) {
            Some(Mark::Done) => return,
            Some(Mark::Visiting) => {
                let start = path.iter().position(|p| p == id).unwrap_or(0);
                let cycle: Vec<_> =
                    path[start..].iter().chain([id]).map(ToString::to_string).collect();
                out.push(Diagnostic {
                    severity: Severity::Error,
                    location: id.to_string(),
                    message: format!("recursive instantiation: {}", cycle.join(" -> ")),
                });
                return;
            }
            None => {}
        }
        marks.insert(id.clone(), Mark::Visiting);
        path.push(id.clone());
        if let Some(schematic) = design.cell(id).and_then(|d| d.schematic.as_ref()) {
            for inst in &schematic.instances {
                if let Some((child, def)) = design.resolve(&inst.cell, &id.library) {
                    // Only schematic views recurse; a SPICE-only cell is a leaf.
                    if def.has_view(ViewKind::Schematic) {
                        visit(design, &child, marks, path, out);
                    }
                }
            }
        }
        path.pop();
        marks.insert(id.clone(), Mark::Done);
    }

    let mut marks = BTreeMap::new();
    for &name in libraries {
        if let Some(lib) = design.library(name) {
            for cell in lib.cells.keys() {
                visit(design, &CellId::new(name, cell), &mut marks, &mut Vec::new(), out);
            }
        }
    }
}
