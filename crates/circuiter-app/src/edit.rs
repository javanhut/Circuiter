//! Schematic editing operations, independent of the GUI.
//!
//! Each operation edits a working copy of a cell (`CellDef`). The workspace snapshots the
//! cell before an operation, which gives undo and redo for free. `design` is used only to
//! look up the symbols of placed parts.

use circuiter_model::cell::{Port, PortKind, validate_name};
use circuiter_model::connectivity::placed_pins;
use circuiter_model::geom::{Length, Point, Rotation, Transform};
use circuiter_model::schematic::{Instance, NetLabel, Schematic, SchematicPort, Wire};
use circuiter_model::{CellDef, CellRef, Design};
use circuiter_scene::Owner;
use std::collections::{BTreeMap, BTreeSet};

/// The schematic grid: 100 mil, which every standard symbol's pins sit on.
pub const GRID: Length = Length::from_mils(100);

pub fn snap(p: Point, grid: Length) -> Point {
    let g = grid.nm();
    let round = |v: i64| (v as f64 / g as f64).round() as i64 * g;
    Point::new(Length(round(p.x.nm())), Length(round(p.y.nm())))
}

/// The first free reference designator with this prefix: `R1`, `R2`, ...
pub fn next_instance_name(s: &Schematic, prefix: &str) -> String {
    let prefix = if prefix.is_empty() { "X" } else { prefix };
    let used: BTreeSet<&str> = s.instances.iter().map(|i| i.name.as_str()).collect();
    (1..).map(|n| format!("{prefix}{n}")).find(|name| !used.contains(name.as_str())).unwrap()
}

/// Places a part and returns its index.
pub fn place(
    design: &Design,
    library: &str,
    def: &mut CellDef,
    cell: CellRef,
    placement: Transform,
) -> usize {
    let prefix =
        design.resolve(&cell, library).map(|(_, d)| d.cell.prefix.clone()).unwrap_or_default();
    let s = def.schematic.get_or_insert_with(Schematic::default);
    let name = next_instance_name(s, &prefix);
    s.instances.push(Instance { name, cell, placement, params: BTreeMap::new(), view: None });
    s.instances.len() - 1
}

/// A right-angle route from `from` to `to`, turning once.
pub fn route(from: Point, to: Point, horizontal_first: bool) -> Vec<Point> {
    let corner = if horizontal_first { Point::new(to.x, from.y) } else { Point::new(from.x, to.y) };
    let mut points = vec![from, corner, to];
    simplify(&mut points);
    points
}

/// Adds a wire through `points`, dropping repeated and redundant collinear points.
/// Returns its index, or `None` if nothing is left to draw.
pub fn add_wire(def: &mut CellDef, mut points: Vec<Point>) -> Option<usize> {
    simplify(&mut points);
    if points.len() < 2 {
        return None;
    }
    let s = def.schematic.get_or_insert_with(Schematic::default);
    s.wires.push(Wire { points });
    Some(s.wires.len() - 1)
}

/// Removes consecutive duplicates and vertices in the middle of straight runs.
fn simplify(points: &mut Vec<Point>) {
    points.dedup();
    let mut i = 1;
    while i + 1 < points.len() {
        let (a, b, c) = (points[i - 1], points[i], points[i + 1]);
        let cross = (b.x.nm() - a.x.nm()) as i128 * (c.y.nm() - b.y.nm()) as i128
            - (b.y.nm() - a.y.nm()) as i128 * (c.x.nm() - b.x.nm()) as i128;
        let forward = (b.x.nm() - a.x.nm()) as i128 * (c.x.nm() - b.x.nm()) as i128
            + (b.y.nm() - a.y.nm()) as i128 * (c.y.nm() - b.y.nm()) as i128;
        if cross == 0 && forward > 0 {
            points.remove(i);
        } else {
            i += 1;
        }
    }
}

pub fn add_label(def: &mut CellDef, net: &str, at: Point) -> Result<usize, String> {
    let net = net.trim();
    if net.is_empty() {
        return Err("a net label needs a name".into());
    }
    let s = def.schematic.get_or_insert_with(Schematic::default);
    s.labels.push(NetLabel { net: net.to_owned(), at });
    Ok(s.labels.len() - 1)
}

/// Places a port of the cell on its schematic, adding it to the cell's interface if it's
/// new. A port can only be placed once.
pub fn add_port(def: &mut CellDef, name: &str, kind: PortKind, at: Point) -> Result<usize, String> {
    validate_name(name)?;
    let s = def.schematic.get_or_insert_with(Schematic::default);
    if s.ports.iter().any(|p| p.port == name) {
        return Err(format!("port `{name}` is already placed on this sheet"));
    }
    if def.cell.port(name).is_none() {
        def.cell.ports.push(Port::new(name, kind));
    }
    let s = def.schematic.as_mut().expect("created above");
    s.ports.push(SchematicPort { port: name.to_owned(), at });
    Ok(s.ports.len() - 1)
}

/// Deletes the given objects. Deleting a port's flag removes the port from the cell's
/// interface and symbol too, since a port only exists through its flag.
pub fn delete(def: &mut CellDef, targets: &BTreeSet<Owner>) {
    let Some(s) = def.schematic.as_mut() else { return };
    let mut removed_ports = Vec::new();
    // Remove from the back so earlier indices stay valid.
    for target in targets.iter().rev() {
        match *target {
            Owner::Instance(i) if i < s.instances.len() => {
                s.instances.remove(i);
            }
            Owner::Wire(i) if i < s.wires.len() => {
                s.wires.remove(i);
            }
            Owner::Label(i) if i < s.labels.len() => {
                s.labels.remove(i);
            }
            Owner::Port(i) if i < s.ports.len() => {
                removed_ports.push(s.ports.remove(i).port);
            }
            _ => {}
        }
    }
    for name in removed_ports {
        if !s.ports.iter().any(|p| p.port == name) {
            def.cell.ports.retain(|p| p.name != name);
            if let Some(symbol) = def.symbol.as_mut() {
                symbol.pins.retain(|p| p.port != name);
            }
        }
    }
}

/// Moves objects by `delta`. Wires attached to moved pins stretch to follow them,
/// keeping right angles, so connections survive the move.
pub fn translate(
    design: &Design,
    library: &str,
    def: &mut CellDef,
    targets: &BTreeSet<Owner>,
    delta: Point,
) {
    if delta == Point::ORIGIN {
        return;
    }
    let instances: Vec<usize> = targets
        .iter()
        .filter_map(|t| match t {
            Owner::Instance(i) => Some(*i),
            _ => None,
        })
        .collect();
    let moved_wires: BTreeSet<usize> = targets
        .iter()
        .filter_map(|t| match t {
            Owner::Wire(i) => Some(*i),
            _ => None,
        })
        .collect();
    // Riders: other wires ending on a dragged wire, and labels on it, move with it.
    let on = |wire: &[Point], p: Point| {
        wire.contains(&p) || wire.windows(2).any(|seg| inside(p, seg[0], seg[1]))
    };
    let mut riders: BTreeMap<Point, Moved> = BTreeMap::new();
    let mut ends: Vec<(usize, bool, bool)> = Vec::new();
    if let Some(s) = def.schematic.as_ref() {
        let pins_staying: Vec<Point> = s
            .instances
            .iter()
            .enumerate()
            .filter(|(k, _)| !instances.contains(k))
            .filter_map(|(_, inst)| {
                design.resolve(&inst.cell, library).map(|(_, d)| placed_pins(d, inst))
            })
            .flatten()
            .map(|(p, _)| p)
            .collect();
        for &w in &moved_wires {
            let Some(wire) = s.wires.get(w) else { continue };
            for (k, other) in s.wires.iter().enumerate() {
                if moved_wires.contains(&k) {
                    continue;
                }
                for p in [other.points[0], *other.points.last().expect("two points")] {
                    if on(&wire.points, p) && !pins_staying.contains(&p) {
                        riders.insert(p, Moved { to: p + delta, body: None });
                    }
                }
            }
            // An end held by something that stays put (a pin, another wire, a label)
            // keeps a jog back to it.
            let held = |p: Point| {
                pins_staying.contains(&p)
                    || s.labels
                        .iter()
                        .enumerate()
                        .any(|(k, l)| l.at == p && !targets.contains(&Owner::Label(k)))
                    || s.wires.iter().enumerate().any(|(k, other)| {
                        !moved_wires.contains(&k)
                            && k != w
                            && on(&other.points, p)
                            && other.points.first() != Some(&p)
                            && other.points.last() != Some(&p)
                    })
            };
            let (first, last) = (wire.points[0], *wire.points.last().expect("two points"));
            ends.push((w, held(first), held(last)));
        }
    }
    let label_riders: Vec<usize> = def.schematic.as_ref().map_or(Vec::new(), |s| {
        s.labels
            .iter()
            .enumerate()
            .filter(|(k, l)| {
                !targets.contains(&Owner::Label(*k))
                    && moved_wires
                        .iter()
                        .any(|&w| s.wires.get(w).is_some_and(|x| on(&x.points, l.at)))
            })
            .map(|(k, _)| k)
            .collect()
    });
    transform_instances(design, library, def, &instances, &moved_wires, false, |t| {
        t.position = t.position + delta;
    });
    let Some(s) = def.schematic.as_mut() else { return };
    for (w, keep_first, keep_last) in ends {
        let Some(wire) = s.wires.get_mut(w) else { continue };
        let (first, last) = (wire.points[0], *wire.points.last().expect("two points"));
        wire.points.iter_mut().for_each(|p| *p = *p + delta);
        if keep_last {
            wire.points.extend(route(last + delta, last, true).into_iter().skip(1));
        }
        if keep_first {
            let mut jog = route(first, first + delta, false);
            jog.pop();
            jog.append(&mut wire.points);
            wire.points = jog;
        }
        simplify(&mut wire.points);
    }
    for k in label_riders {
        s.labels[k].at = s.labels[k].at + delta;
    }
    let pins = all_pins(design, library, s);
    follow(s, &riders, &moved_wires, &pins);
    for target in targets {
        match *target {
            Owner::Label(i) => {
                if let Some(l) = s.labels.get_mut(i) {
                    l.at = l.at + delta;
                }
            }
            Owner::Port(i) => {
                if let Some(p) = s.ports.get_mut(i) {
                    p.at = p.at + delta;
                }
            }
            _ => {}
        }
    }
}

/// Rotates instances 90° counter-clockwise about their origins.
pub fn rotate(design: &Design, library: &str, def: &mut CellDef, instances: &[usize]) {
    transform_instances(design, library, def, instances, &BTreeSet::new(), true, |t| {
        t.rotation = match t.rotation {
            Rotation::R0 => Rotation::R90,
            Rotation::R90 => Rotation::R180,
            Rotation::R180 => Rotation::R270,
            Rotation::R270 => Rotation::R0,
        };
    });
}

/// Mirrors instances left-to-right about their origins.
pub fn mirror(design: &Design, library: &str, def: &mut CellDef, instances: &[usize]) {
    transform_instances(design, library, def, instances, &BTreeSet::new(), true, |t| {
        // Mirroring is applied before rotation, so a part turned on its side mirrors
        // vertically on screen. Flip the rotation too so X always mirrors left-right.
        t.mirror = !t.mirror;
        if matches!(t.rotation, Rotation::R90 | Rotation::R270) {
            t.rotation = match t.rotation {
                Rotation::R90 => Rotation::R270,
                _ => Rotation::R90,
            };
        }
    });
}

/// Applies `change` to the placement of each instance, then drags attached wire ends
/// (except those of `skip_wires`, which are moving as a whole) along with the pins.
fn transform_instances(
    design: &Design,
    library: &str,
    def: &mut CellDef,
    instances: &[usize],
    skip_wires: &BTreeSet<usize>,
    turning: bool,
    change: impl Fn(&mut Transform),
) {
    let Some(s) = def.schematic.as_mut() else { return };
    let mut moved: BTreeMap<Point, Moved> = BTreeMap::new();
    let mut spots: Vec<Point> = Vec::new();
    for &i in instances {
        let Some(inst) = s.instances.get_mut(i) else { continue };
        let Some((_, child)) = design.resolve(&inst.cell, library) else {
            change(&mut inst.placement);
            continue;
        };
        let before = placed_pins(child, inst);
        change(&mut inst.placement);
        let after = placed_pins(child, inst);
        let body = child.symbol.as_ref().and_then(|sym| body_bounds(sym, &inst.placement));
        // A part being turned keeps wires off every spot its pins can turn to, so a
        // later turn doesn't land a pin on a wire routed around it now.
        if turning {
            for rotation in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
                for mirror in [false, true] {
                    let t = Transform { rotation, mirror, ..inst.placement };
                    let mut probe = inst.clone();
                    probe.placement = t;
                    spots.extend(placed_pins(child, &probe).into_iter().map(|(p, _)| p));
                }
            }
        }
        for ((from, _), (to, _)) in before.into_iter().zip(after) {
            if from != to {
                moved.insert(from, Moved { to, body });
            }
        }
    }
    let mut pins = all_pins(design, library, s);
    pins.extend(spots);
    follow(s, &moved, skip_wires, &pins);
}

/// Every pin's connection point on the sheet.
fn all_pins(design: &Design, library: &str, s: &Schematic) -> Vec<Point> {
    s.instances
        .iter()
        .filter_map(|inst| design.resolve(&inst.cell, library).map(|(_, d)| placed_pins(d, inst)))
        .flatten()
        .map(|(p, _)| p)
        .collect()
}

/// Where a pin moved to, and the body of its part after the move (to route around).
#[derive(Debug, Clone, Copy)]
struct Moved {
    to: Point,
    body: Option<(Point, Point)>,
}

/// The bounding box of a symbol's graphics (not its pins), placed by `t`.
fn body_bounds(symbol: &circuiter_model::symbol::Symbol, t: &Transform) -> Option<(Point, Point)> {
    use circuiter_model::symbol::Graphic;
    let mut pts = Vec::new();
    for g in &symbol.graphics {
        match g {
            Graphic::Polyline { points, .. } => pts.extend(points.iter().copied()),
            Graphic::Rect { a, b, .. } => pts.extend([*a, *b]),
            Graphic::Circle { center, radius, .. } | Graphic::Arc { center, radius, .. } => {
                pts.push(Point::new(center.x - *radius, center.y - *radius));
                pts.push(Point::new(center.x + *radius, center.y + *radius));
            }
            Graphic::Text { .. } => {}
        }
    }
    let placed: Vec<Point> = pts.into_iter().map(|p| t.apply(p)).collect();
    let min = Point::new(placed.iter().map(|p| p.x).min()?, placed.iter().map(|p| p.y).min()?);
    let max = Point::new(placed.iter().map(|p| p.x).max()?, placed.iter().map(|p| p.y).max()?);
    Some((min, max))
}

/// Whether the axis-aligned segment `a`–`b` passes through the inside of `rect`.
fn crosses(a: Point, b: Point, (min, max): (Point, Point)) -> bool {
    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
    x0 < max.x && x1 > min.x && y0 < max.y && y1 > min.y
}

/// Whether `p` lies strictly inside the axis-aligned segment `a`–`b`.
fn inside(p: Point, a: Point, b: Point) -> bool {
    let cross = (b.x.nm() - a.x.nm()) as i128 * (p.y.nm() - a.y.nm()) as i128
        - (b.y.nm() - a.y.nm()) as i128 * (p.x.nm() - a.x.nm()) as i128;
    cross == 0
        && p != a
        && p != b
        && p.x >= a.x.min(b.x)
        && p.x <= a.x.max(b.x)
        && p.y >= a.y.min(b.y)
        && p.y <= a.y.max(b.y)
}

/// Moves wire ends that sat on moved points (pins, or the riders of a dragged wire) so
/// every connection survives, and nothing new is connected.
///
/// A wire whose two ends move by the same amount is translated whole. Otherwise each
/// moving end is re-routed from the last thing attached to its wire before it (the
/// previous corner, or a T or label part-way along the last segment, which stays put),
/// along the simplest path that keeps right angles, avoids the moved part's body, and
/// neither touches nor runs over other wires and pins. Labels sitting on a moved point
/// move with it.
fn follow(
    s: &mut Schematic,
    moved: &BTreeMap<Point, Moved>,
    skip_wires: &BTreeSet<usize>,
    pins: &[Point],
) {
    for i in 0..s.wires.len() {
        if skip_wires.contains(&i) || s.wires[i].points.len() < 2 {
            continue;
        }
        let first = s.wires[i].points[0];
        let last = *s.wires[i].points.last().expect("two points");
        match (moved.get(&first), moved.get(&last)) {
            (Some(a), Some(b)) if a.to - first == b.to - last => {
                let d = a.to - first;
                s.wires[i].points.iter_mut().for_each(|p| *p = *p + d);
            }
            (a, b) => {
                if let Some(m) = b {
                    reroute_end(s, i, *m, pins);
                }
                if let Some(m) = a {
                    s.wires[i].points.reverse();
                    reroute_end(s, i, *m, pins);
                    s.wires[i].points.reverse();
                }
                simplify(&mut s.wires[i].points);
            }
        }
    }
    for label in &mut s.labels {
        if let Some(m) = moved.get(&label.at) {
            label.at = m.to;
        }
    }
    // A wire whose ends met has nothing left to draw.
    s.wires.retain(|w| w.points.len() >= 2);
}

/// Re-routes the last point of wire `w` to `m.to`.
fn reroute_end(s: &mut Schematic, w: usize, m: Moved, pins: &[Point]) {
    let points = &s.wires[w].points;
    let e = points.len() - 1;
    let end = points[e];
    // Keep whatever joins the last segment part-way: route from the attachment nearest
    // the end instead of from the previous corner.
    let mut neighbour = points[e - 1];
    let mut split = None;
    let attachments = s
        .wires
        .iter()
        .enumerate()
        .filter(|(k, _)| *k != w)
        .flat_map(|(_, wire)| wire.points.iter().copied())
        .chain(s.labels.iter().map(|l| l.at))
        .chain(s.ports.iter().map(|p| p.at));
    for a in attachments {
        if inside(a, neighbour, end)
            && split.is_none_or(|b: Point| distance(a, end) < distance(b, end))
        {
            split = Some(a);
        }
    }
    if let Some(a) = split {
        neighbour = a;
    }
    let route = best_route(s, w, neighbour, m.to, m.body, pins, end);
    // Vertices something else is attached to must stay.
    let held: BTreeSet<Point> = s
        .wires
        .iter()
        .enumerate()
        .filter(|(k, _)| *k != w)
        .flat_map(|(_, wire)| wire.points.iter().copied())
        .chain(s.labels.iter().map(|l| l.at))
        .chain(s.ports.iter().map(|p| p.at))
        .chain(pins.iter().copied())
        .collect();
    let points = &mut s.wires[w].points;
    points.pop();
    if let Some(a) = split {
        points.push(a);
    }
    points.extend(route.into_iter().skip(1));
    remove_spurs(points, &held);
}

/// Removes places where a wire doubles back on itself (`a → b → c` with `c` back
/// towards `a` on the same line), unless something is attached at the turn `b`.
fn remove_spurs(points: &mut Vec<Point>, held: &BTreeSet<Point>) {
    let mut changed = true;
    while changed {
        changed = false;
        points.dedup();
        let mut i = 1;
        while i + 1 < points.len() {
            let (a, b, c) = (points[i - 1], points[i], points[i + 1]);
            let cross = (b.x.nm() - a.x.nm()) as i128 * (c.y.nm() - b.y.nm()) as i128
                - (b.y.nm() - a.y.nm()) as i128 * (c.x.nm() - b.x.nm()) as i128;
            let forward = (b.x.nm() - a.x.nm()) as i128 * (c.x.nm() - b.x.nm()) as i128
                + (b.y.nm() - a.y.nm()) as i128 * (c.y.nm() - b.y.nm()) as i128;
            if cross == 0 && forward < 0 && !held.contains(&b) {
                points.remove(i);
                changed = true;
            } else {
                i += 1;
            }
        }
    }
}

fn distance(a: Point, b: Point) -> i64 {
    (a.x - b.x).nm().abs() + (a.y - b.y).nm().abs()
}

/// The simplest right-angled route from `from` to `to` that keeps clear of `body`,
/// other wires (except where it starts and ends) and pins (except at `to`): straight or
/// one corner first, then detours around the body. `old` is where the end was, used to
/// keep the segment at the pin in its original direction when possible.
fn best_route(
    s: &Schematic,
    w: usize,
    from: Point,
    to: Point,
    body: Option<(Point, Point)>,
    pins: &[Point],
    old: Point,
) -> Vec<Point> {
    // Keep the segment at the pin as it was (horizontal stays horizontal) if possible.
    let pin_side_horizontal = from.y == old.y;
    let mut candidates =
        vec![route(from, to, !pin_side_horizontal), route(from, to, pin_side_horizontal)];
    let g = GRID.nm();
    let down = |v: Length, k: i64| Length((v.nm() - 1).div_euclid(g) * g - k * g);
    let up = |v: Length, k: i64| Length(((v.nm() + 1) + g - 1).div_euclid(g) * g + k * g);
    // Around the body (or, with none, around the two ends), one to four steps out.
    let (min, max) = body.unwrap_or((
        Point::new(from.x.min(to.x), from.y.min(to.y)),
        Point::new(from.x.max(to.x), from.y.max(to.y)),
    ));
    for k in 0..4 {
        let mut detours = vec![
            vec![from, Point::new(from.x, down(min.y, k)), Point::new(to.x, down(min.y, k)), to],
            vec![from, Point::new(from.x, up(max.y, k)), Point::new(to.x, up(max.y, k)), to],
            vec![from, Point::new(down(min.x, k), from.y), Point::new(down(min.x, k), to.y), to],
            vec![from, Point::new(up(max.x, k), from.y), Point::new(up(max.x, k), to.y), to],
        ];
        detours.sort_by_key(|r| r.windows(2).map(|seg| distance(seg[0], seg[1])).sum::<i64>());
        for mut d in detours {
            simplify(&mut d);
            candidates.push(d);
        }
    }
    let clear_of_body =
        |r: &[Point]| body.is_none_or(|b| r.windows(2).all(|seg| !crosses(seg[0], seg[1], b)));
    let clear_of_wires = |r: &[Point]| {
        // Other wires' geometry, as it stands now.
        let others: Vec<&Vec<Point>> =
            s.wires.iter().enumerate().filter(|(k, _)| *k != w).map(|(_, x)| &x.points).collect();
        let on_wire = |p: Point| {
            others
                .iter()
                .any(|o| o.contains(&p) || o.windows(2).any(|seg| inside(p, seg[0], seg[1])))
        };
        // Corners must not land on anything; the ends may (that is what they join).
        let corners = r.get(1..r.len().saturating_sub(1)).unwrap_or_default();
        let corners_free = corners.iter().all(|&p| !on_wire(p) && !pins.contains(&p));
        // Nothing else may lie along the route: other wires' vertices, or pins.
        let segments_free = r.windows(2).all(|seg| {
            others.iter().all(|o| o.iter().all(|&p| !inside(p, seg[0], seg[1])))
                && pins.iter().all(|&p| !inside(p, seg[0], seg[1]))
        });
        corners_free && segments_free
    };
    candidates
        .iter()
        .find(|r| clear_of_body(r) && clear_of_wires(r))
        .or_else(|| candidates.iter().find(|r| clear_of_body(r)))
        .unwrap_or(&candidates[0])
        .clone()
}

/// Sets (or with `None`, clears back to the default) a parameter on an instance.
pub fn set_param(def: &mut CellDef, instance: usize, name: &str, value: Option<String>) {
    let Some(inst) = def.schematic.as_mut().and_then(|s| s.instances.get_mut(instance)) else {
        return;
    };
    match value {
        Some(v) => {
            inst.params.insert(name.to_owned(), v.trim().to_owned());
        }
        None => {
            inst.params.remove(name);
        }
    }
}

pub fn rename(def: &mut CellDef, instance: usize, name: &str) -> Result<(), String> {
    validate_name(name)?;
    let s = def.schematic.as_mut().ok_or("no schematic")?;
    if s.instances.iter().enumerate().any(|(i, inst)| i != instance && inst.name == name) {
        return Err(format!("`{name}` is already used on this sheet"));
    }
    let inst = s.instances.get_mut(instance).ok_or("no such instance")?;
    inst.name = name.to_owned();
    Ok(())
}

/// Parts, wires and labels copied from a sheet, placed relative to their centre so
/// they can be pasted anywhere. Part references name their library, so a clip pastes
/// into any sheet.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Clip {
    pub instances: Vec<Instance>,
    pub wires: Vec<Wire>,
    pub labels: Vec<NetLabel>,
}

/// What the system clipboard holds for a clip: this line, then the clip in RON.
const CLIP_HEADER: &str = "circuiter clip 1";

impl Clip {
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty() && self.wires.is_empty() && self.labels.is_empty()
    }

    pub fn to_text(&self) -> String {
        let ron = ron::ser::to_string(self).expect("a clip serializes");
        format!("{CLIP_HEADER}\n{ron}")
    }

    /// The clip in clipboard text, if it is one.
    pub fn from_text(text: &str) -> Option<Clip> {
        let ron = text.strip_prefix(CLIP_HEADER)?.trim_start();
        ron::from_str(ron).ok()
    }
}

/// Copies the selected parts, wires and labels of `def`'s sheet, with the wires
/// running between the selected parts even if they weren't selected themselves.
/// Ports are left out: a sheet's ports are its interface, named once. Also returns
/// where the clip's centre was.
pub fn copy(
    design: &Design,
    library: &str,
    def: &CellDef,
    targets: &BTreeSet<Owner>,
) -> (Clip, Point) {
    let origin = Point::new(Length(0), Length(0));
    let empty = Clip { instances: Vec::new(), wires: Vec::new(), labels: Vec::new() };
    let Some(s) = def.schematic.as_ref() else { return (empty, origin) };
    let mut clip = empty;
    let mut ends: BTreeSet<Point> = BTreeSet::new();
    for t in targets {
        if let Owner::Instance(i) = *t {
            let Some(inst) = s.instances.get(i) else { continue };
            let mut inst = inst.clone();
            if let Some((id, d)) = design.resolve(&inst.cell, library) {
                ends.extend(placed_pins(d, &inst).into_iter().map(|(p, _)| p));
                inst.cell = CellRef::new(Some(&id.library), &id.cell);
            }
            clip.instances.push(inst);
        }
    }
    let selected_wire = |i: usize| targets.contains(&Owner::Wire(i));
    for (i, w) in s.wires.iter().enumerate() {
        if selected_wire(i) {
            ends.extend(w.points.iter().copied());
        }
    }
    for (i, w) in s.wires.iter().enumerate() {
        let between = w.points.first().is_some_and(|p| ends.contains(p))
            && w.points.last().is_some_and(|p| ends.contains(p));
        if selected_wire(i) || between {
            clip.wires.push(w.clone());
        }
    }
    for t in targets {
        if let Owner::Label(i) = *t {
            clip.labels.extend(s.labels.get(i).cloned());
        }
    }
    // Centre it on the grid point nearest the middle of what was copied.
    let mut bounds: Option<(Point, Point)> = None;
    let points = clip
        .instances
        .iter()
        .map(|i| i.placement.position)
        .chain(clip.wires.iter().flat_map(|w| w.points.iter().copied()))
        .chain(clip.labels.iter().map(|l| l.at));
    for p in points {
        bounds = Some(match bounds {
            None => (p, p),
            Some((lo, hi)) => {
                (Point::new(lo.x.min(p.x), lo.y.min(p.y)), Point::new(hi.x.max(p.x), hi.y.max(p.y)))
            }
        });
    }
    let Some((lo, hi)) = bounds else { return (clip, origin) };
    let mid = Point::new(Length((lo.x.nm() + hi.x.nm()) / 2), Length((lo.y.nm() + hi.y.nm()) / 2));
    let centre = snap(mid, GRID);
    clip.translate(origin - centre);
    (clip, centre)
}

impl Clip {
    fn translate(&mut self, by: Point) {
        for i in &mut self.instances {
            i.placement.position = i.placement.position + by;
        }
        for w in &mut self.wires {
            for p in &mut w.points {
                *p = *p + by;
            }
        }
        for l in &mut self.labels {
            l.at = l.at + by;
        }
    }
}

/// Pastes `clip` centred on `at`. Parts get the next free names (R3, R4…). Returns
/// what was added, to select.
pub fn paste(
    design: &Design,
    library: &str,
    def: &mut CellDef,
    clip: &Clip,
    at: Point,
) -> BTreeSet<Owner> {
    let mut clip = clip.clone();
    clip.translate(snap(at, GRID));
    let s = def.schematic.get_or_insert_with(Schematic::default);
    let mut added = BTreeSet::new();
    for mut inst in clip.instances {
        let prefix = design.resolve(&inst.cell, library).map(|(id, d)| {
            // Same-library parts are referred to without the library, as when placed.
            if id.library == library {
                inst.cell = CellRef::new(None, &id.cell);
            }
            d.cell.prefix.clone()
        });
        inst.name = next_instance_name(s, &prefix.unwrap_or_default());
        s.instances.push(inst);
        added.insert(Owner::Instance(s.instances.len() - 1));
    }
    for w in clip.wires {
        s.wires.push(w);
        added.insert(Owner::Wire(s.wires.len() - 1));
    }
    for l in clip.labels {
        s.labels.push(l);
        added.insert(Owner::Label(s.labels.len() - 1));
    }
    added
}

/// A new component made from part of a sheet: the selected parts with the wires
/// between them, and for each net that also reaches something left behind (another
/// part, a port, a label), a port on a short wire out of the pin. Global nets (GND,
/// VDD) get a label instead, since they connect everywhere anyway. Returns the cell
/// (with no symbol yet) and its ports' names.
pub fn component_from(
    design: &Design,
    library: &str,
    def: &CellDef,
    targets: &BTreeSet<Owner>,
    conn: &circuiter_model::connectivity::Connectivity,
) -> Result<(CellDef, Vec<String>), String> {
    use circuiter_model::connectivity::{Item, NetNameSource};
    let s = def.schematic.as_ref().ok_or("make components from a schematic")?;
    let chosen: BTreeSet<usize> = targets
        .iter()
        .filter_map(|t| match t {
            Owner::Instance(i) if *i < s.instances.len() => Some(*i),
            _ => None,
        })
        .collect();
    if chosen.is_empty() {
        return Err("select the parts the component is made of".into());
    }
    let (clip, centre) = copy(design, library, def, targets);
    let origin = Point::new(Length(0), Length(0));
    let mut new = CellDef { schematic: Some(Schematic::default()), ..CellDef::default() };
    paste(design, library, &mut new, &clip, origin);
    let copied: Vec<&Wire> = clip.wires.iter().collect();
    let shift = origin - centre;

    let mut ports: Vec<String> = Vec::new();
    for net in &conn.nets {
        // A selected pin on this net, and whether anything outside the selection is.
        let mut inside = None;
        let mut outside = false;
        for item in &net.items {
            match *item {
                Item::Pin(k) => {
                    let pin = &conn.pins[k];
                    if chosen.contains(&pin.instance) {
                        inside = inside.or(Some(k));
                    } else {
                        outside = true;
                    }
                }
                Item::Port(_) | Item::Label(_) => outside = true,
                Item::Wire(w) => {
                    let moved =
                        Wire { points: s.wires[w].points.iter().map(|&p| p + shift).collect() };
                    outside |= !copied.contains(&&moved);
                }
            }
        }
        let (Some(k), true) = (inside, outside) else { continue };
        let pin = &conn.pins[k];
        let inst = &s.instances[pin.instance];
        let Some((_, child)) = design.resolve(&inst.cell, library) else { continue };
        let Some(sym_pin) =
            child.symbol.as_ref().and_then(|sym| sym.pins.iter().find(|p| p.port == pin.port))
        else {
            continue;
        };
        // Out of the pin, away from the part's body, two grid steps.
        let step = GRID.nm() * 2;
        let (dx, dy) = match sym_pin.direction {
            circuiter_model::symbol::PinDirection::Left => (step, 0),
            circuiter_model::symbol::PinDirection::Right => (-step, 0),
            circuiter_model::symbol::PinDirection::Up => (0, step),
            circuiter_model::symbol::PinDirection::Down => (0, -step),
        };
        let away = inst.placement.apply(sym_pin.at + Point::new(Length(dx), Length(dy)));
        let start = pin.at + shift;
        let end = away + shift;
        add_wire(&mut new, vec![start, end]);
        if net.source == NetNameSource::Global {
            add_label(&mut new, &net.name, end)?;
            continue;
        }
        let named = matches!(net.source, NetNameSource::Label | NetNameSource::Port)
            && validate_name(&net.name).is_ok();
        let base = if named { net.name.clone() } else { format!("p{}", ports.len() + 1) };
        let mut name = base.clone();
        let mut n = 2;
        while ports.iter().any(|p| p.eq_ignore_ascii_case(&name)) {
            name = format!("{base}_{n}");
            n += 1;
        }
        let kind = child.cell.port(&pin.port).map_or(PortKind::Passive, |p| p.kind);
        add_port(&mut new, &name, kind, end)?;
        ports.push(name);
    }
    Ok((new, ports))
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::CellId;
    use circuiter_model::connectivity::Connectivity;

    fn design() -> Design {
        let mut d = Design::new();
        d.add_library(circuiter_stdlib::library()).unwrap();
        d
    }

    fn mm(x: f64, y: f64) -> Point {
        Point::mm(x, y)
    }

    fn resistor_at(design: &Design, def: &mut CellDef, x: f64, y: f64) -> usize {
        place(design, "std", def, "std:resistor".parse().unwrap(), Transform::at(mm(x, y)))
    }

    fn sheet() -> CellDef {
        CellDef { name: "top".into(), schematic: Some(Schematic::default()), ..CellDef::default() }
    }

    #[test]
    fn snapping_and_names() {
        assert_eq!(snap(mm(1.3, -1.2), GRID), mm(2.54, 0.0));
        let d = design();
        let mut def = sheet();
        resistor_at(&d, &mut def, 0.0, 0.0);
        resistor_at(&d, &mut def, 0.0, 10.16);
        let names: Vec<_> =
            def.schematic.as_ref().unwrap().instances.iter().map(|i| &i.name).collect();
        assert_eq!(names, ["R1", "R2"]);
    }

    #[test]
    fn wires_are_simplified_and_routed_orthogonally() {
        assert_eq!(
            route(mm(0.0, 0.0), mm(5.08, 2.54), true),
            vec![mm(0.0, 0.0), mm(5.08, 0.0), mm(5.08, 2.54)]
        );
        assert_eq!(route(mm(0.0, 0.0), mm(5.08, 0.0), false), vec![mm(0.0, 0.0), mm(5.08, 0.0)]);
        let mut def = sheet();
        let w = add_wire(&mut def, vec![mm(0.0, 0.0), mm(0.0, 0.0), mm(2.54, 0.0), mm(5.08, 0.0)])
            .unwrap();
        assert_eq!(
            def.schematic.as_ref().unwrap().wires[w].points,
            vec![mm(0.0, 0.0), mm(5.08, 0.0)]
        );
        assert_eq!(add_wire(&mut def, vec![mm(1.0, 1.0), mm(1.0, 1.0)]), None);
    }

    #[test]
    fn moving_a_part_drags_its_wires_and_keeps_them_connected() {
        let d = design();
        let mut def = sheet();
        let r1 = resistor_at(&d, &mut def, 0.0, 0.0); // pins at x = ±5.08
        resistor_at(&d, &mut def, 20.32, 0.0); // pins at 15.24 and 25.4
        add_wire(&mut def, vec![mm(5.08, 0.0), mm(15.24, 0.0)]);

        let targets = BTreeSet::from([Owner::Instance(r1)]);
        translate(&d, "std", &mut def, &targets, mm(0.0, 5.08));

        let s = def.schematic.as_ref().unwrap();
        assert_eq!(s.instances[r1].placement.position, mm(0.0, 5.08));
        assert_eq!(
            s.wires[0].points,
            vec![mm(5.08, 5.08), mm(15.24, 5.08), mm(15.24, 0.0)],
            "the pin-side segment stays horizontal"
        );
        let conn = Connectivity::analyze(&d, "std", s);
        assert_eq!(conn.dangling_pins.len(), 2, "only the two outer pins are open");
    }

    #[test]
    fn rotating_a_part_keeps_wires_attached() {
        let d = design();
        let mut def = sheet();
        let r1 = resistor_at(&d, &mut def, 0.0, 0.0);
        add_wire(&mut def, vec![mm(5.08, 0.0), mm(10.16, 0.0)]);
        rotate(&d, "std", &mut def, &[r1]);
        let s = def.schematic.as_ref().unwrap();
        assert_eq!(s.instances[r1].placement.rotation, Rotation::R90);
        // Pin b moved from (5.08, 0) to (0, -5.08); the wire follows with a corner that
        // keeps clear of the resistor body.
        assert_eq!(s.wires[0].points, vec![mm(0.0, -5.08), mm(10.16, -5.08), mm(10.16, 0.0)]);
        let conn = Connectivity::analyze(&d, "std", s);
        assert_eq!(conn.dangling_pins.len(), 1);
    }

    #[test]
    fn stretched_wires_route_around_the_moved_part() {
        let d = design();
        let mut def = sheet();
        let r1 = resistor_at(&d, &mut def, 0.0, 0.0);
        rotate(&d, "std", &mut def, &[r1]); // vertical: pins at (0, ±5.08)
        add_wire(&mut def, vec![mm(0.0, 5.08), mm(0.0, 10.16)]);
        // Move the part down past the wire's far end. Keeping the pin-side segment
        // vertical would run the wire through the resistor body, so the other corner is used.
        let targets = BTreeSet::from([Owner::Instance(r1)]);
        translate(&d, "std", &mut def, &targets, mm(5.08, 10.16));
        let w = &def.schematic.as_ref().unwrap().wires[0].points;
        assert_eq!(w, &vec![mm(5.08, 15.24), mm(0.0, 15.24), mm(0.0, 10.16)]);
    }

    #[test]
    fn mirror_always_flips_left_to_right() {
        let d = design();
        let mut def = sheet();
        let r1 = resistor_at(&d, &mut def, 0.0, 0.0);
        rotate(&d, "std", &mut def, &[r1]);
        mirror(&d, "std", &mut def, &[r1]);
        let inst = &def.schematic.as_ref().unwrap().instances[r1];
        // A vertical resistor mirrored left-right looks the same: pins stay put.
        let child = d.cell(&CellId::new("std", "resistor")).unwrap();
        let pins: Vec<Point> = placed_pins(child, inst).into_iter().map(|(p, _)| p).collect();
        assert!(pins.contains(&mm(0.0, -5.08)) && pins.contains(&mm(0.0, 5.08)), "{pins:?}");
    }

    #[test]
    fn deleting_ports_updates_the_interface() {
        let mut def = sheet();
        add_port(&mut def, "in", PortKind::Input, mm(0.0, 0.0)).unwrap();
        add_port(&mut def, "out", PortKind::Output, mm(10.0, 0.0)).unwrap();
        assert!(add_port(&mut def, "in", PortKind::Input, mm(5.0, 0.0)).is_err());
        assert_eq!(def.cell.ports.len(), 2);
        delete(&mut def, &BTreeSet::from([Owner::Port(0)]));
        let names: Vec<_> = def.cell.ports.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["out"]);
    }

    #[test]
    fn delete_handles_mixed_selections() {
        let d = design();
        let mut def = sheet();
        for x in [0.0, 20.0, 40.0] {
            resistor_at(&d, &mut def, x, 0.0);
        }
        add_wire(&mut def, vec![mm(5.08, 0.0), mm(14.92, 0.0)]);
        add_label(&mut def, "A", mm(5.08, 0.0)).unwrap();
        delete(&mut def, &BTreeSet::from([Owner::Instance(0), Owner::Instance(2), Owner::Wire(0)]));
        let s = def.schematic.as_ref().unwrap();
        assert_eq!(s.instances.len(), 1);
        assert_eq!(s.instances[0].name, "R2");
        assert!(s.wires.is_empty());
        assert_eq!(s.labels.len(), 1);
    }

    #[test]
    fn params_and_names() {
        let d = design();
        let mut def = sheet();
        let r = resistor_at(&d, &mut def, 0.0, 0.0);
        resistor_at(&d, &mut def, 10.0, 0.0);
        set_param(&mut def, r, "R", Some(" 4k7 ".into()));
        assert_eq!(def.schematic.as_ref().unwrap().instances[r].params["R"], "4k7");
        set_param(&mut def, r, "R", None);
        assert!(def.schematic.as_ref().unwrap().instances[r].params.is_empty());
        assert!(rename(&mut def, r, "R2").is_err(), "duplicate");
        assert!(rename(&mut def, r, "bad name").is_err());
        rename(&mut def, r, "RLOAD").unwrap();
    }

    /// Which pins and labels are joined: one set per net, in a comparable form.
    fn partition(d: &Design, def: &CellDef) -> BTreeSet<BTreeSet<String>> {
        let s = def.schematic.as_ref().unwrap();
        let c = Connectivity::analyze(d, "std", s);
        let mut nets: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
        for (k, pin) in c.pins.iter().enumerate() {
            if let Some(n) = c.net_of(circuiter_model::connectivity::Item::Pin(k)) {
                nets.entry(n)
                    .or_default()
                    .insert(format!("{}.{}", s.instances[pin.instance].name, pin.port));
            }
        }
        for (k, label) in s.labels.iter().enumerate() {
            if let Some(n) = c.net_of(circuiter_model::connectivity::Item::Label(k)) {
                nets.entry(n).or_default().insert(format!("label {}", label.net));
            }
        }
        nets.into_values().filter(|n| n.len() > 1).collect()
    }

    fn orthogonal(def: &CellDef) -> bool {
        def.schematic
            .as_ref()
            .unwrap()
            .wires
            .iter()
            .all(|w| w.points.windows(2).all(|s| s[0].x == s[1].x || s[0].y == s[1].y))
    }

    /// A small circuit with every kind of attachment a move can disturb: two wires sharing
    /// a pin, a T joining a pin's wire part-way, a label on a wire, and a straight run.
    fn busy_sheet(d: &Design) -> CellDef {
        let mut def = sheet();
        let r1 = resistor_at(d, &mut def, 0.0, 0.0); // pins (-5.08, 0), (5.08, 0)
        resistor_at(d, &mut def, 30.48, 0.0); // pins (25.4, 0), (35.56, 0)
        resistor_at(d, &mut def, 15.24, 20.32); // pins (10.16, 20.32), (20.32, 20.32)
        let _ = r1;
        add_wire(&mut def, vec![mm(5.08, 0.0), mm(25.4, 0.0)]);
        // A second wire from R1.b down to R3: shares the pin with the first.
        add_wire(&mut def, vec![mm(5.08, 0.0), mm(5.08, 20.32), mm(10.16, 20.32)]);
        // A T into the middle of the first wire, from R3.b.
        add_wire(&mut def, vec![mm(20.32, 20.32), mm(15.24, 20.32), mm(15.24, 0.0)]);
        add_label(&mut def, "MID", mm(10.16, 0.0)).unwrap();
        // R1.a to nothing but a label.
        add_wire(&mut def, vec![mm(-5.08, 0.0), mm(-10.16, 0.0)]);
        add_label(&mut def, "IN", mm(-10.16, 0.0)).unwrap();
        def
    }

    #[test]
    fn moving_and_rotating_parts_never_changes_what_is_connected() {
        let d = design();
        let base = busy_sheet(&d);
        let nets = partition(&d, &base);
        assert_eq!(nets.len(), 2, "{nets:?}");
        let steps = [-2, -1, 1, 3];
        for inst in 0..3 {
            for &dx in &steps {
                for &dy in &steps {
                    let mut def = base.clone();
                    let delta = Point::mils(100 * dx, 100 * dy);
                    translate(&d, "std", &mut def, &BTreeSet::from([Owner::Instance(inst)]), delta);
                    assert_eq!(partition(&d, &def), nets, "R{} moved by ({dx}, {dy})", inst + 1);
                    assert!(orthogonal(&def), "R{} moved by ({dx}, {dy})", inst + 1);
                }
            }
            let mut def = base.clone();
            for turn in 1..=4 {
                rotate(&d, "std", &mut def, &[inst]);
                assert_eq!(partition(&d, &def), nets, "R{} turned {turn} times", inst + 1);
                assert!(orthogonal(&def));
            }
            let mut def = base.clone();
            mirror(&d, "std", &mut def, &[inst]);
            assert_eq!(partition(&d, &def), nets, "R{} mirrored", inst + 1);
        }
        // Moving everything together changes nothing at all but position.
        let mut def = base.clone();
        let all: BTreeSet<Owner> = (0..3)
            .map(Owner::Instance)
            .chain((0..4).map(Owner::Wire))
            .chain((0..2).map(Owner::Label))
            .collect();
        translate(&d, "std", &mut def, &all, Point::mils(200, -100));
        assert_eq!(partition(&d, &def), nets);
    }

    #[test]
    fn dragging_a_wire_keeps_its_ends_attached() {
        let d = design();
        let base = busy_sheet(&d);
        let nets = partition(&d, &base);
        for w in 0..4 {
            for (dx, dy) in [(0, 2), (0, -1), (1, 0), (2, 3)] {
                let mut def = base.clone();
                let delta = Point::mils(100 * dx, 100 * dy);
                translate(&d, "std", &mut def, &BTreeSet::from([Owner::Wire(w)]), delta);
                assert_eq!(partition(&d, &def), nets, "wire {w} dragged by ({dx}, {dy})");
                assert!(orthogonal(&def), "wire {w} dragged by ({dx}, {dy})");
            }
        }
    }
}
#[cfg(test)]
mod tests_support {
    use super::*;
    pub fn busy(d: &Design) -> CellDef {
        let mm = Point::mm;
        let mut def = CellDef {
            name: "top".into(),
            schematic: Some(Schematic::default()),
            ..CellDef::default()
        };
        for (x, y) in [(0.0, 0.0), (30.48, 0.0), (15.24, 20.32)] {
            place(d, "std", &mut def, "std:resistor".parse().unwrap(), Transform::at(mm(x, y)));
        }
        add_wire(&mut def, vec![mm(5.08, 0.0), mm(25.4, 0.0)]);
        add_wire(&mut def, vec![mm(5.08, 0.0), mm(5.08, 20.32), mm(10.16, 20.32)]);
        add_wire(&mut def, vec![mm(20.32, 20.32), mm(15.24, 20.32), mm(15.24, 0.0)]);
        add_label(&mut def, "MID", mm(10.16, 0.0)).unwrap();
        add_wire(&mut def, vec![mm(-5.08, 0.0), mm(-10.16, 0.0)]);
        add_label(&mut def, "IN", mm(-10.16, 0.0)).unwrap();
        def
    }
}

#[cfg(test)]
mod random_edits {
    use super::*;
    #[test]
    /// Random moves, rotations and mirrors of wired parts: connections change only when
    /// a pin lands on wiring that was already there, almost never because of routing.
    fn random_edits_rarely_change_connections() {
        let mut d = Design::new();
        d.add_library(circuiter_stdlib::library()).unwrap();
        let base = super::tests_support::busy(&d);
        let nets = |def: &CellDef| {
            let s = def.schematic.as_ref().unwrap();
            let c = circuiter_model::connectivity::Connectivity::analyze(&d, "std", s);
            let mut out: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
            for (k, pin) in c.pins.iter().enumerate() {
                if let Some(n) = c.net_of(circuiter_model::connectivity::Item::Pin(k)) {
                    out.entry(n)
                        .or_default()
                        .insert(format!("{}.{}", s.instances[pin.instance].name, pin.port));
                }
            }
            out.into_values().collect::<BTreeSet<_>>()
        };
        let want = nets(&base);
        let mut seed: u64 = 0x9E3779B97F4A7C15;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let (mut ok, mut total, mut landed, mut router) = (0, 0, 0, 0);
        for _ in 0..300 {
            let mut def = base.clone();
            let mut fine = true;
            for _ in 0..6 {
                let inst = rnd(3) as usize;
                let before = def.clone();
                match rnd(3) {
                    0 => {
                        let delta =
                            Point::mils(100 * (rnd(7) as i64 - 3), 100 * (rnd(7) as i64 - 3));
                        translate(
                            &d,
                            "std",
                            &mut def,
                            &BTreeSet::from([Owner::Instance(inst)]),
                            delta,
                        );
                    }
                    1 => rotate(&d, "std", &mut def, &[inst]),
                    _ => mirror(&d, "std", &mut def, &[inst]),
                }
                if nets(&def) != want {
                    // Did a pin land on something that was already there?
                    let bs = before.schematic.as_ref().unwrap();
                    let new_pins: Vec<Point> = {
                        let s = def.schematic.as_ref().unwrap();
                        let inst_def =
                            d.cell(&circuiter_model::CellId::new("std", "resistor")).unwrap();
                        placed_pins(inst_def, &s.instances[inst])
                            .into_iter()
                            .map(|(p, _)| p)
                            .collect()
                    };
                    let old_pins: Vec<Point> = {
                        let inst_def =
                            d.cell(&circuiter_model::CellId::new("std", "resistor")).unwrap();
                        placed_pins(inst_def, &bs.instances[inst])
                            .into_iter()
                            .map(|(p, _)| p)
                            .collect()
                    };
                    let others: Vec<Point> = bs
                        .instances
                        .iter()
                        .enumerate()
                        .filter(|(k, _)| *k != inst)
                        .flat_map(|(_, i)| {
                            placed_pins(
                                d.cell(&circuiter_model::CellId::new("std", "resistor")).unwrap(),
                                i,
                            )
                            .into_iter()
                            .map(|(p, _)| p)
                        })
                        .collect();
                    let lands = new_pins.iter().any(|&p| {
                        !old_pins.contains(&p)
                            && (others.contains(&p)
                                || bs.wires.iter().any(|w| {
                                    (w.points.contains(&p)
                                        && !old_pins.iter().any(|o| w.points.contains(o)))
                                        || w.points.windows(2).any(|seg| inside(p, seg[0], seg[1]))
                                }))
                    });
                    if lands {
                        landed += 1
                    } else {
                        router += 1
                    }
                    fine = false;
                    break;
                }
            }
            total += 1;
            ok += usize::from(fine);
        }
        eprintln!(
            "{ok}/{total} random six-step sequences kept every connection; failures: {landed} pin landed on existing wiring, {router} routing"
        );
        assert!(ok >= 255, "{ok}/{total}");
        assert!(router <= 10, "{router} routing failures");
    }
}
