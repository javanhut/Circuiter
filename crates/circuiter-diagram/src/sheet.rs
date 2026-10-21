//! A sheet laid out afresh: the same parts, ports and nets, placed and wired by the
//! layout engine.

use crate::layout::{self, Block, Column, Net, P, Pin, Side, UNIT};
use circuiter_model::cell::{Cell, CellRef, PortKind};
use circuiter_model::connectivity::{Connectivity, Item, NetNameSource};
use circuiter_model::geom::{Length, Point, Rotation, Transform};
use circuiter_model::schematic::{Instance, NetLabel, Schematic, SchematicPort, Wire};
use circuiter_model::symbol::PinDirection;
use circuiter_model::{CellId, Design};
use circuiter_scene::{CHAR_WIDTH, Element, Rect, instance_elements};
use std::collections::BTreeMap;

/// `id`'s schematic laid out as a diagram. Power symbols aren't wired up: each pin on
/// a power net gets its own, as on a hand-drawn sheet.
pub fn arrange(design: &Design, id: &CellId) -> Result<Schematic, String> {
    let def = design.cell(id).ok_or_else(|| format!("{id}: no such cell"))?;
    let sheet = def.schematic.as_ref().ok_or_else(|| format!("{id} has no schematic"))?;
    Ok(arrange_sheet(design, &id.library, &def.cell, sheet))
}

/// [`arrange`] for `sheet`, a schematic of `cell` in `library`.
pub fn arrange_sheet(design: &Design, library: &str, cell: &Cell, sheet: &Schematic) -> Schematic {
    let conn = Connectivity::analyze(design, library, sheet);
    let mut blocks: Vec<Block> = Vec::new();
    // The block of each instance (none for a power symbol), and the (block, pin) of each
    // of the sheet's pins.
    let mut inst_block: Vec<Option<usize>> = vec![None; sheet.instances.len()];
    let mut pin_at: Vec<Option<(usize, usize)>> = vec![None; conn.pins.len()];
    // A power symbol for each global net, to draw at each of its pins.
    let mut power: Vec<(String, Instance)> = Vec::new();
    for (i, inst) in sheet.instances.iter().enumerate() {
        let def = design.resolve(&inst.cell, library).map(|(_, d)| d);
        if let Some(net) = def.and_then(|d| d.cell.global_net_for(&inst.params)) {
            if !power.iter().any(|(n, _)| *n == net) {
                power.push((net, inst.clone()));
            }
            continue;
        }
        let (min, max) = bounds(&upright_elements(design, library, inst));
        let symbol_pins = def.and_then(|d| d.symbol.as_ref()).map_or(&[][..], |s| &s.pins[..]);
        let pins = symbol_pins
            .iter()
            .map(|pin| {
                let kind = def.and_then(|d| d.cell.port(&pin.port)).map(|p| p.kind);
                Pin {
                    at: (pin.at.x.nm(), pin.at.y.nm()),
                    side: match pin.direction {
                        PinDirection::Right => Side::Left,
                        PinDirection::Left => Side::Right,
                        PinDirection::Down => Side::Top,
                        PinDirection::Up => Side::Bottom,
                    },
                    drives: matches!(kind, Some(PortKind::Output | PortKind::PowerOut)),
                }
            })
            .collect();
        inst_block[i] = Some(blocks.len());
        blocks.push(Block { min, max, pins, column: Column::Any });
    }
    // The sheet's pins come instance by instance, in each symbol's order.
    let mut seen = vec![0usize; sheet.instances.len()];
    for (k, pin) in conn.pins.iter().enumerate() {
        if let Some(b) = inst_block[pin.instance] {
            pin_at[k] = Some((b, seen[pin.instance]));
        }
        seen[pin.instance] += 1;
    }
    // Ports: inputs down the left, outputs down the right, as flags.
    let mut port_block = Vec::with_capacity(sheet.ports.len());
    for port in &sheet.ports {
        let kind = cell.port(&port.port).map_or(PortKind::Passive, |p| p.kind);
        let input = matches!(kind, PortKind::Input | PortKind::PowerIn);
        let output = matches!(kind, PortKind::Output | PortKind::PowerOut);
        let h = 1.27 * 1.6;
        let len = port.port.chars().count() as f64 * 1.27 * CHAR_WIDTH + h * 1.5;
        let (len, half) = (snap_up(nm(len)), snap_up(nm(h / 2.0)));
        // The flag points away from its wire.
        let (side, min, max) = if output {
            (Side::Left, (0, -half), (len, half))
        } else {
            (Side::Right, (-len, -half), (0, half))
        };
        let column = if input {
            Column::First
        } else if output {
            Column::Last
        } else {
            Column::Any
        };
        port_block.push(blocks.len());
        blocks.push(Block {
            min,
            max,
            pins: vec![Pin { at: (0, 0), side, drives: input }],
            column,
        });
    }
    // Global nets named only by labels get the library's symbol for them, and so do
    // nets named like one (a `GND` label).
    for net in &conn.nets {
        let global = net.source == NetNameSource::Global;
        if !power.iter().any(|(n, _)| *n == net.name) {
            if let Some(inst) = power_symbol(design, &net.name, global) {
                power.push((net.name.clone(), inst));
            }
        }
    }
    let nets: Vec<Net> = conn
        .nets
        .iter()
        .map(|net| {
            let pins = net
                .items
                .iter()
                .filter_map(|item| match *item {
                    Item::Pin(p) => pin_at[p],
                    Item::Port(i) => Some((port_block[i], 0)),
                    _ => None,
                })
                .collect();
            let global =
                net.source == NetNameSource::Global || power.iter().any(|(n, _)| *n == net.name);
            let named = net.source == NetNameSource::Label;
            // A power symbol or the name's text, and how far it reaches.
            let symbol = power.iter().find(|(n, _)| *n == net.name).map(|(_, inst)| inst);
            let mark = match symbol {
                Some(inst) if global => {
                    let (min, max) = bounds(&upright_elements(design, library, inst));
                    let reach = [-min.0, -min.1, max.0, max.1].into_iter().max().unwrap_or(0);
                    (reach, reach)
                }
                _ if global || named => {
                    let w = net.name.chars().count() as f64 * 1.27 * CHAR_WIDTH + 0.254;
                    (snap_up(nm(w)), snap_up(nm(1.6)))
                }
                _ => (0, 0),
            };
            Net { pins, wired: !global, named, mark }
        })
        .collect();
    let laid = layout::layout(&blocks, &nets);

    let point = |(x, y): P| Point::new(Length::from_nm(x), Length::from_nm(y));
    let mut out = Schematic::default();
    for (i, inst) in sheet.instances.iter().enumerate() {
        if let Some(b) = inst_block[i] {
            let mut placed = inst.clone();
            placed.placement = Transform::at(point(laid.origins[b]));
            out.instances.push(placed);
        }
    }
    for (port, &b) in sheet.ports.iter().zip(&port_block) {
        out.ports.push(SchematicPort { port: port.port.clone(), at: point(laid.origins[b]) });
    }
    out.wires =
        laid.wires.iter().map(|(_, w)| Wire { points: vec![point(w[0]), point(w[1])] }).collect();
    // What a power symbol must keep clear of: the blocks, and the wires.
    let mut taken: Vec<(P, P)> = blocks
        .iter()
        .zip(&laid.origins)
        .map(|(b, o)| ((o.0 + b.min.0, o.1 + b.min.1), (o.0 + b.max.0, o.1 + b.max.1)))
        .collect();
    let mut copies = 0;
    for &(k, at, side) in &laid.labels {
        let name = &conn.nets[k].name;
        let symbol = power.iter().find(|(n, _)| n == name).map(|(_, inst)| inst);
        match symbol.filter(|_| laid.labelled.contains(&k)) {
            Some(inst) => {
                copies += 1;
                let mut copy = inst.clone();
                copy.name = format!("{}_{copies}", inst.name);
                // On a stub out of a pin's side, a symbol stands upright a little further
                // out, where the wire turns down (or up) to it, if there's room for it
                // there; otherwise it lies along the stub.
                match upright_beside(design, library, inst, at, side, &laid.wires, &taken) {
                    Some((placement, stub, body)) => {
                        copy.placement = placement;
                        out.wires.push(Wire { points: vec![point(at), point(stub)] });
                        taken.push(body);
                    }
                    None => copy.placement = facing(design, library, inst, point(at), side),
                }
                out.instances.push(copy);
            }
            None => out.labels.push(NetLabel { net: name.clone(), at: point(at), global: false }),
        }
    }
    // No-connect flags stay on their pins.
    for &nc in &sheet.no_connects {
        let pin = conn.pins.iter().position(|p| p.at == nc).and_then(|k| pin_at[k]);
        if let Some((b, p)) = pin {
            let at = blocks[b].pins[p].at;
            out.no_connects.push(point((laid.origins[b].0 + at.0, laid.origins[b].1 + at.1)));
        }
    }
    out
}

/// A power symbol for net `net`: the one made for it (`GND`), else (when the net is
/// global) a rail named after it.
fn power_symbol(design: &Design, net: &str, global: bool) -> Option<Instance> {
    let mut rail = None;
    for lib in design.libraries() {
        for (name, def) in &lib.cells {
            let made = def.cell.global_net.as_deref();
            let symbol = |params: BTreeMap<String, String>| Instance {
                name: format!("#{net}"),
                cell: CellRef::new(Some(&lib.name), name),
                placement: Transform::default(),
                params,
                view: None,
                fields: BTreeMap::new(),
            };
            if made == Some(net) {
                return Some(symbol(BTreeMap::new()));
            }
            if let Some(param) = made.and_then(|g| g.strip_prefix("${param.")?.strip_suffix('}')) {
                if rail.is_none() && def.symbol.is_some() {
                    rail = Some(symbol(BTreeMap::from([(param.to_owned(), net.to_owned())])));
                }
            }
        }
    }
    rail.filter(|_| global)
}

/// The drawing of `inst` placed upright at the origin.
fn upright_elements(design: &Design, library: &str, inst: &Instance) -> Vec<Element> {
    let mut upright = inst.clone();
    upright.placement = Transform::at(Point::ORIGIN);
    instance_elements(design, library, &upright)
}

/// Where a power symbol goes so its pin is at `at` and its body points away from the
/// pin it marks, whose stub heads out on `side`: upright if that works, else turned.
fn facing(design: &Design, library: &str, inst: &Instance, at: Point, side: Side) -> Transform {
    let pin = design
        .resolve(&inst.cell, library)
        .and_then(|(_, d)| d.symbol.as_ref()?.pins.first().map(|p| p.at))
        .unwrap_or(Point::ORIGIN);
    let (min, max) = bounds(&upright_elements(design, library, inst));
    let body =
        Point::new(Length::from_nm((min.0 + max.0) / 2), Length::from_nm((min.1 + max.1) / 2));
    let away = match side {
        Side::Left => (-1, 0),
        Side::Right => (1, 0),
        Side::Top => (0, -1),
        Side::Bottom => (0, 1),
    };
    let turns = [Rotation::R0, Rotation::R90, Rotation::R270, Rotation::R180];
    let rotation = turns
        .into_iter()
        .max_by_key(|&rotation| {
            let t = Transform { position: Point::ORIGIN, rotation, mirror: false };
            let (b, p) = (t.apply(body), t.apply(pin));
            let (dx, dy) = ((b.x - p.x).nm(), (b.y - p.y).nm());
            // Pointing the right way first, then the least turned.
            (dx.signum() * away.0 + dy.signum() * away.1, rotation == Rotation::R0)
        })
        .unwrap_or_default();
    let t = Transform { position: Point::ORIGIN, rotation, mirror: false };
    let p = t.apply(pin);
    Transform { position: Point::new(at.x - p.x, at.y - p.y), rotation, mirror: false }
}

/// For a power symbol at the end of a stub heading out sideways from `at`: where it
/// stands upright one step on, with the stub carried on to its pin, if its body there
/// keeps clear of the wires and of what's `taken`. Gives the placement, the stub's new
/// end and the body's box.
fn upright_beside(
    design: &Design,
    library: &str,
    inst: &Instance,
    at: P,
    side: Side,
    wires: &[(usize, [P; 2])],
    taken: &[(P, P)],
) -> Option<(Transform, P, (P, P))> {
    let along = match side {
        Side::Left => -1,
        Side::Right => 1,
        Side::Top | Side::Bottom => return None,
    };
    let pin = design.resolve(&inst.cell, library)?.1.symbol.as_ref()?.pins.first()?.at;
    let (min, max) = bounds(&upright_elements(design, library, inst));
    let pin = (pin.x.nm(), pin.y.nm());
    // Far enough on that the body clears the stub's start.
    let half = snap_up((pin.0 - min.0).max(max.0 - pin.0));
    let stub = (at.0 + along * half, at.1);
    let origin = (stub.0 - pin.0, stub.1 - pin.1);
    let body = ((origin.0 + min.0, origin.1 + min.1), (origin.0 + max.0, origin.1 + max.1));
    // Touching at the edge is fine: the stub itself meets the body's edge at its pin.
    let inside = |p: P| p.0 > body.0.0 && p.0 < body.1.0 && p.1 > body.0.1 && p.1 < body.1.1;
    let crosses = |w: &[P; 2]| {
        let (lo, hi) =
            ((w[0].0.min(w[1].0), w[0].1.min(w[1].1)), (w[0].0.max(w[1].0), w[0].1.max(w[1].1)));
        lo.0 < body.1.0 && hi.0 > body.0.0 && lo.1 < body.1.1 && hi.1 > body.0.1
            || inside(w[0])
            || inside(w[1])
    };
    let overlaps =
        |r: &(P, P)| r.0.0 < body.1.0 && r.1.0 > body.0.0 && r.0.1 < body.1.1 && r.1.1 > body.0.1;
    if wires.iter().any(|(_, w)| crosses(w)) || taken.iter().any(overlaps) {
        return None;
    }
    let placement = Transform::at(Point::new(Length::from_nm(origin.0), Length::from_nm(origin.1)));
    Some((placement, stub, body))
}

/// The box around `elements`, in nanometres, out to the layout's step.
fn bounds(elements: &[Element]) -> (P, P) {
    let mut r = Rect::EMPTY;
    for e in elements {
        r.union(&e.shape.bounds());
    }
    if r.is_empty() {
        return ((0, 0), (0, 0));
    }
    let down = |v: f64| -snap_up(-nm(v));
    ((down(r.min.x), down(r.min.y)), (snap_up(nm(r.max.x)), snap_up(nm(r.max.y))))
}

fn nm(mm: f64) -> i64 {
    (mm * 1e6).round() as i64
}

/// Up to the next multiple of the layout's step.
fn snap_up(v: i64) -> i64 {
    v.div_euclid(UNIT) * UNIT + if v.rem_euclid(UNIT) == 0 { 0 } else { UNIT }
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::project::Project;
    use std::collections::BTreeSet;
    use std::path::Path;

    fn design(example: &str) -> Design {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(example);
        let project = Project::load(&root).unwrap();
        Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap()
    }

    /// Each net as the set of what's on it: instance pins by (instance, port, which of
    /// the port's pins) and ports by name. Power symbols and lone pins are left out.
    fn nets(design: &Design, library: &str, sheet: &Schematic) -> BTreeSet<BTreeSet<String>> {
        let conn = Connectivity::analyze(design, library, sheet);
        let power = |i: usize| {
            let inst = &sheet.instances[i];
            design
                .resolve(&inst.cell, library)
                .is_some_and(|(_, d)| d.cell.global_net_for(&inst.params).is_some())
        };
        let mut count: BTreeMap<(usize, String), usize> = BTreeMap::new();
        let mut names = vec![String::new(); conn.pins.len()];
        for (k, pin) in conn.pins.iter().enumerate() {
            let n = count.entry((pin.instance, pin.port.clone())).or_default();
            names[k] = format!("{}.{}#{n}", sheet.instances[pin.instance].name, pin.port);
            *n += 1;
        }
        conn.nets
            .iter()
            .map(|net| {
                net.items
                    .iter()
                    .filter_map(|item| match *item {
                        Item::Pin(p) if !power(conn.pins[p].instance) => Some(names[p].clone()),
                        Item::Port(i) => Some(format!("port {}", sheet.ports[i].port)),
                        _ => None,
                    })
                    .collect::<BTreeSet<String>>()
            })
            .filter(|s| s.len() > 1)
            .collect()
    }

    /// Every sheet in every example and in the standard library, laid out: the same
    /// nets as before, no wire over a pin, and nothing left hanging that wasn't.
    #[test]
    fn every_sheet_keeps_its_nets() {
        let out = std::env::var_os("CIRCUITER_DIAGRAM_SVG");
        let mut checked = BTreeSet::new();
        for example in [
            "analog_lab",
            "bench",
            "ce_amplifier",
            "cmos_inverter",
            "devboard",
            "logic",
            "overstress",
            "riscv",
        ] {
            let design = design(example);
            for lib in design.libraries() {
                for (name, def) in &lib.cells {
                    let Some(sheet) = &def.schematic else { continue };
                    let id = CellId::new(&lib.name, name);
                    let arranged = arrange(&design, &id).unwrap();
                    let before = nets(&design, &lib.name, sheet);
                    let after = nets(&design, &lib.name, &arranged);
                    assert_eq!(before, after, "{id}: nets changed");
                    let conn = Connectivity::analyze(&design, &lib.name, &arranged);
                    assert!(conn.passes_over.is_empty(), "{id}: a wire passes over a pin");
                    assert!(
                        conn.dangling_wire_ends.is_empty(),
                        "{id}: wire ends hanging: {:?}",
                        conn.dangling_wire_ends
                    );
                    if let Some(dir) = &out {
                        let scene =
                            circuiter_scene::Scene::build_from(&design, &id, &def.cell, &arranged);
                        let svg = circuiter_scene::to_svg(&scene, &circuiter_scene::Theme::PAPER);
                        std::fs::write(Path::new(dir).join(format!("{name}.svg")), svg).unwrap();
                    }
                    checked.insert(id);
                }
            }
        }
        assert!(checked.len() >= 40, "only {} sheets", checked.len());
    }
}
