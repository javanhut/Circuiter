//! Connectivity of a single schematic sheet: which pins, wires, labels and ports are
//! electrically joined, where junction dots belong, and what is left unconnected.
//!
//! Rules, matching common schematic conventions:
//!
//! - Anything placed at the same point is connected: pin ends, wire vertices, labels and
//!   ports.
//! - A wire vertex, label or port lying on another wire's segment connects to that wire
//!   (a "T" connection).
//! - A pin connects to a wire only at one of the wire's ends or corners. A wire merely
//!   passing over a pin's end does not connect to it (that is reported, since it looks
//!   connected), so routing a wire past a part never shorts it by accident.
//! - Two wires that merely cross, with no vertex at the crossing, are *not* connected.
//! - Net labels with the same name are connected, and so are global-net symbols (ground)
//!   with the same net name. A label named like a global net joins that global net.
//!
//! A junction dot is drawn wherever three or more connections meet at a point.

use crate::cell::CellDef;
use crate::design::Design;
use crate::geom::Point;
use crate::schematic::Schematic;
use std::collections::{BTreeMap, BTreeSet};

/// Something on a sheet that can belong to a net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Item {
    /// Index into [`Connectivity::pins`].
    Pin(usize),
    /// Index into [`Schematic::wires`].
    Wire(usize),
    /// Index into [`Schematic::labels`].
    Label(usize),
    /// Index into [`Schematic::ports`].
    Port(usize),
}

/// A symbol pin placed on the sheet, in sheet coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedPin {
    /// Index into [`Schematic::instances`].
    pub instance: usize,
    pub port: String,
    pub at: Point,
}

/// Where a net's name came from, in order of precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NetNameSource {
    Global,
    Port,
    Label,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Net {
    pub name: String,
    pub source: NetNameSource,
    /// Every distinct name attached to this net. More than one means conflicting labels.
    pub names: Vec<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Default)]
pub struct Connectivity {
    pub pins: Vec<PlacedPin>,
    pub nets: Vec<Net>,
    item_net: BTreeMap<Item, usize>,
    pub junctions: Vec<Point>,
    /// Indices into `pins` of pins connected to nothing.
    pub dangling_pins: Vec<usize>,
    /// Wire end points connected to nothing.
    pub dangling_wire_ends: Vec<Point>,
    /// Pins a wire passes over without connecting: (index into `pins`, wire).
    pub passes_over: Vec<(usize, usize)>,
}

impl Connectivity {
    pub fn net_of(&self, item: Item) -> Option<usize> {
        self.item_net.get(&item).copied()
    }

    /// Analyses `schematic`, a view of a cell in `library`.
    ///
    /// Instances whose cell or symbol can't be resolved contribute no pins; the checker
    /// reports those separately.
    pub fn analyze(design: &Design, library: &str, schematic: &Schematic) -> Self {
        let mut pins = Vec::new();
        let mut global_of_pin: Vec<Option<String>> = Vec::new();
        let mut probe_pin: Vec<bool> = Vec::new();
        for (index, inst) in schematic.instances.iter().enumerate() {
            let Some((_, def)) = design.resolve(&inst.cell, library) else { continue };
            for (at, port) in placed_pins(def, inst) {
                pins.push(PlacedPin { instance: index, port, at });
                global_of_pin.push(def.cell.global_net.clone());
                probe_pin.push(def.cell.probe);
            }
        }

        let wires = &schematic.wires;
        let n_pins = pins.len();
        let n_wires = wires.len();
        let n_labels = schematic.labels.len();
        let id = |item: Item| match item {
            Item::Pin(i) => i,
            Item::Wire(i) => n_pins + i,
            Item::Label(i) => n_pins + n_wires + i,
            Item::Port(i) => n_pins + n_wires + n_labels + i,
        };
        let n_items = n_pins + n_wires + n_labels + schematic.ports.len();

        // Virtual nodes join same-named labels and global nets.
        let mut named: BTreeMap<String, usize> = BTreeMap::new();
        let names: Vec<&str> = global_of_pin
            .iter()
            .flatten()
            .map(String::as_str)
            .chain(schematic.labels.iter().map(|l| l.net.as_str()))
            .collect();
        for name in names {
            let next = n_items + named.len();
            named.entry(name.to_owned()).or_insert(next);
        }
        let mut uf = UnionFind::new(n_items + named.len());

        // Anchor points: everything that connects by position.
        let mut anchors: Vec<(Point, Item)> = Vec::new();
        anchors.extend(pins.iter().enumerate().map(|(i, p)| (p.at, Item::Pin(i))));
        for (i, w) in wires.iter().enumerate() {
            anchors.extend(w.points.iter().map(|&p| (p, Item::Wire(i))));
        }
        anchors.extend(schematic.labels.iter().enumerate().map(|(i, l)| (l.at, Item::Label(i))));
        anchors.extend(schematic.ports.iter().enumerate().map(|(i, p)| (p.at, Item::Port(i))));

        let mut at_point: BTreeMap<Point, Vec<Item>> = BTreeMap::new();
        for &(p, item) in &anchors {
            at_point.entry(p).or_default().push(item);
        }
        for items in at_point.values() {
            for pair in items.windows(2) {
                uf.union(id(pair[0]), id(pair[1]));
            }
        }

        // T connections: an anchor on the interior of another wire's segment.
        // Pins only join wires at their vertices, except instruments' probe tips.
        let mut on_interior: BTreeMap<Point, usize> = BTreeMap::new();
        let mut passes_over = Vec::new();
        for (&p, items) in &at_point {
            for (w, wire) in wires.iter().enumerate() {
                let hits =
                    wire.points.windows(2).filter(|s| strictly_inside(p, s[0], s[1])).count();
                if hits == 0 {
                    continue;
                }
                match items.iter().find(|it| !matches!(it, Item::Pin(k) if !probe_pin[*k])) {
                    Some(&joining) => {
                        *on_interior.entry(p).or_default() += hits;
                        uf.union(id(joining), id(Item::Wire(w)));
                    }
                    None => passes_over.extend(items.iter().filter_map(|it| match it {
                        Item::Pin(k) => Some((*k, w)),
                        _ => None,
                    })),
                }
            }
        }

        for (i, global) in global_of_pin.iter().enumerate() {
            if let Some(name) = global {
                uf.union(id(Item::Pin(i)), named[name]);
            }
        }
        for (i, label) in schematic.labels.iter().enumerate() {
            uf.union(id(Item::Label(i)), named[&label.net]);
        }

        // Junctions and dangling ends, from the number of connections at each point.
        let mut degree: BTreeMap<Point, usize> = BTreeMap::new();
        for p in &pins {
            *degree.entry(p.at).or_default() += 1;
        }
        for wire in wires {
            for seg in wire.points.windows(2) {
                if seg[0] != seg[1] {
                    *degree.entry(seg[0]).or_default() += 1;
                    *degree.entry(seg[1]).or_default() += 1;
                }
            }
        }
        for (&p, &hits) in &on_interior {
            *degree.entry(p).or_default() += 2 * hits;
        }
        let junctions = degree.iter().filter(|(_, d)| **d >= 3).map(|(p, _)| *p).collect();

        let attached =
            |p: &Point| at_point[p].iter().any(|i| matches!(i, Item::Label(_) | Item::Port(_)));
        let mut dangling_wire_ends = Vec::new();
        for wire in wires {
            let ends = [wire.points.first(), wire.points.last()];
            for p in ends.into_iter().flatten() {
                if degree.get(p).copied().unwrap_or(0) <= 1 && !attached(p) {
                    dangling_wire_ends.push(*p);
                }
            }
        }

        // Group items into nets.
        let mut groups: BTreeMap<usize, Vec<Item>> = BTreeMap::new();
        let all_items = (0..n_pins)
            .map(Item::Pin)
            .chain((0..n_wires).map(Item::Wire))
            .chain((0..n_labels).map(Item::Label))
            .chain((0..schematic.ports.len()).map(Item::Port));
        for item in all_items {
            groups.entry(uf.find(id(item))).or_default().push(item);
        }
        let dangling_pins =
            (0..n_pins).filter(|&i| degree[&pins[i].at] <= 1 && !attached(&pins[i].at)).collect();

        let mut nets: Vec<Net> =
            groups.into_values().map(|items| name_net(items, &global_of_pin, schematic)).collect();
        nets.sort_by(|a, b| a.items.cmp(&b.items));
        let mut auto = 0;
        for net in &mut nets {
            if net.source == NetNameSource::Auto {
                auto += 1;
                net.name = format!("N${auto}");
            }
        }
        let item_net = nets
            .iter()
            .enumerate()
            .flat_map(|(n, net)| net.items.iter().map(move |&i| (i, n)))
            .collect();

        Connectivity {
            pins,
            nets,
            item_net,
            junctions,
            dangling_pins,
            dangling_wire_ends,
            passes_over,
        }
    }
}

/// The world positions and ports of an instance's symbol pins.
pub fn placed_pins(def: &CellDef, inst: &crate::schematic::Instance) -> Vec<(Point, String)> {
    def.symbol
        .iter()
        .flat_map(|s| &s.pins)
        .map(|pin| (inst.placement.apply(pin.at), pin.port.clone()))
        .collect()
}

fn name_net(items: Vec<Item>, global_of_pin: &[Option<String>], schematic: &Schematic) -> Net {
    let mut candidates: BTreeSet<(NetNameSource, String)> = BTreeSet::new();
    for item in &items {
        match *item {
            Item::Pin(i) => {
                if let Some(g) = &global_of_pin[i] {
                    candidates.insert((NetNameSource::Global, g.clone()));
                }
            }
            Item::Port(i) => {
                candidates.insert((NetNameSource::Port, schematic.ports[i].port.clone()));
            }
            Item::Label(i) => {
                candidates.insert((NetNameSource::Label, schematic.labels[i].net.clone()));
            }
            Item::Wire(_) => {}
        }
    }
    let (source, name) =
        candidates.iter().next().cloned().unwrap_or((NetNameSource::Auto, String::new()));
    let names: BTreeSet<String> = candidates.into_iter().map(|(_, n)| n).collect();
    Net { name, source, names: names.into_iter().collect(), items }
}

/// Whether `p` lies on segment `a`–`b`, excluding the end points.
fn strictly_inside(p: Point, a: Point, b: Point) -> bool {
    if p == a || p == b || a == b {
        return false;
    }
    let (ax, ay) = (a.x.0 as i128, a.y.0 as i128);
    let (dx, dy) = (b.x.0 as i128 - ax, b.y.0 as i128 - ay);
    let (px, py) = (p.x.0 as i128 - ax, p.y.0 as i128 - ay);
    if dx * py - dy * px != 0 {
        return false;
    }
    let dot = dx * px + dy * py;
    dot > 0 && dot < dx * dx + dy * dy
}

#[derive(Debug)]
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        UnionFind { parent: (0..n).collect() }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            // Keep the smaller root so results don't depend on union order.
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent[hi] = lo;
        }
    }
}
