//! Symbol editing operations, independent of the GUI. Like [`crate::edit`], each one
//! works on a working copy of a cell, so the workspace's snapshots give undo and redo.

use crate::edit::{GRID, snap};
use circuiter_model::CellDef;
use circuiter_model::builder;
use circuiter_model::geom::{Length, Point};
use circuiter_model::symbol::{Anchor, Graphic, PinDirection, Symbol, SymbolPin};
use circuiter_scene::Owner;
use std::collections::BTreeSet;

/// The drawing grid: 50 mil, half the pin grid, for finer outlines. Pins stay on the
/// 100-mil [`GRID`] so schematic wires can reach them.
pub const FINE_GRID: Length = Length::from_mils(50);

/// Adds a drawing primitive and returns its index.
pub fn add_graphic(def: &mut CellDef, graphic: Graphic) -> usize {
    let symbol = def.symbol.get_or_insert_with(Symbol::default);
    symbol.graphics.push(graphic);
    symbol.graphics.len() - 1
}

/// Ports of the cell that have no pin on the symbol yet.
pub fn unpinned_ports(def: &CellDef) -> Vec<String> {
    def.cell
        .ports
        .iter()
        .filter(|p| def.symbol.as_ref().is_none_or(|s| s.pin(&p.name).is_none()))
        .map(|p| p.name.clone())
        .collect()
}

/// Adds the pin for `port`, which must be a port of the cell without a pin.
pub fn add_pin(
    def: &mut CellDef,
    port: &str,
    at: Point,
    direction: PinDirection,
) -> Result<usize, String> {
    if def.cell.port(port).is_none() {
        return Err(format!("`{port}` is not a port of this cell"));
    }
    let symbol = def.symbol.get_or_insert_with(Symbol::default);
    if symbol.pin(port).is_some() {
        return Err(format!("port `{port}` already has a pin"));
    }
    if symbol.pins.iter().any(|p| p.at == at) {
        return Err("another pin already connects there".into());
    }
    symbol.pins.push(SymbolPin {
        port: port.to_owned(),
        at,
        direction,
        length: GRID,
        show_name: true,
    });
    Ok(symbol.pins.len() - 1)
}

/// Replaces the symbol with a box generated from the ports.
pub fn generate(def: &mut CellDef) {
    def.symbol = Some(builder::generate(&def.name, &def.cell));
}

/// Deletes the given graphics and pins.
pub fn delete(def: &mut CellDef, targets: &BTreeSet<Owner>) {
    let Some(symbol) = def.symbol.as_mut() else { return };
    // Remove from the back so earlier indices stay valid.
    for target in targets.iter().rev() {
        match *target {
            Owner::Graphic(i) if i < symbol.graphics.len() => {
                symbol.graphics.remove(i);
            }
            Owner::Pin(i) if i < symbol.pins.len() => {
                symbol.pins.remove(i);
            }
            _ => {}
        }
    }
}

/// Moves the given graphics and pins by `delta`.
pub fn translate(def: &mut CellDef, targets: &BTreeSet<Owner>, delta: Point) {
    map_targets(def, targets, &|p| p + delta, &|d| d, &|a| a, false);
}

/// Turns the given graphics and pins 90° counter-clockwise about the middle of their
/// extent, taken on the pin grid so pins stay on it.
pub fn rotate(def: &mut CellDef, targets: &BTreeSet<Owner>) {
    let Some(c) = pivot(def, targets) else { return };
    let turn = |p: Point| Point::new(c.x + (p.y - c.y), c.y - (p.x - c.x));
    let dir = |d| match d {
        PinDirection::Right => PinDirection::Up,
        PinDirection::Up => PinDirection::Left,
        PinDirection::Left => PinDirection::Down,
        PinDirection::Down => PinDirection::Right,
    };
    map_targets(def, targets, &turn, &dir, &|a| (a.0 + 90.0, a.1 + 90.0), false);
}

/// Mirrors the given graphics and pins left to right about the middle of their extent.
pub fn mirror(def: &mut CellDef, targets: &BTreeSet<Owner>) {
    let Some(c) = pivot(def, targets) else { return };
    let flip = |p: Point| Point::new(c.x + c.x - p.x, p.y);
    let dir = |d| match d {
        PinDirection::Right => PinDirection::Left,
        PinDirection::Left => PinDirection::Right,
        other => other,
    };
    // Mirroring reverses the sweep, so the old end becomes the new start.
    map_targets(def, targets, &flip, &dir, &|(s, e)| (180.0 - e, 180.0 - s), true);
}

/// The grid point at the middle of the targets' extent.
fn pivot(def: &CellDef, targets: &BTreeSet<Owner>) -> Option<Point> {
    let symbol = def.symbol.as_ref()?;
    let picked = Symbol {
        generated: false,
        states: Vec::new(),
        graphics: targets
            .iter()
            .filter_map(|t| match t {
                Owner::Graphic(i) => symbol.graphics.get(*i).cloned(),
                _ => None,
            })
            .collect(),
        pins: targets
            .iter()
            .filter_map(|t| match t {
                Owner::Pin(i) => symbol.pins.get(*i).cloned(),
                _ => None,
            })
            .collect(),
    };
    let (min, max) = picked.bounds()?;
    let mid =
        Point::new(Length((min.x.nm() + max.x.nm()) / 2), Length((min.y.nm() + max.y.nm()) / 2));
    Some(snap(mid, GRID))
}

fn map_targets(
    def: &mut CellDef,
    targets: &BTreeSet<Owner>,
    point: &dyn Fn(Point) -> Point,
    direction: &dyn Fn(PinDirection) -> PinDirection,
    angles: &dyn Fn((f64, f64)) -> (f64, f64),
    mirrored: bool,
) {
    let Some(symbol) = def.symbol.as_mut() else { return };
    for target in targets {
        match *target {
            Owner::Graphic(i) => {
                let Some(g) = symbol.graphics.get_mut(i) else { continue };
                match g {
                    Graphic::Polyline { points, .. } => {
                        points.iter_mut().for_each(|p| *p = point(*p));
                    }
                    Graphic::Rect { a, b, .. } => {
                        let (p, q) = (point(*a), point(*b));
                        *a = Point::new(p.x.min(q.x), p.y.min(q.y));
                        *b = Point::new(p.x.max(q.x), p.y.max(q.y));
                    }
                    Graphic::Circle { center, .. } => *center = point(*center),
                    Graphic::Arc { center, start, end, .. } => {
                        *center = point(*center);
                        let (s, e) = angles((*start, *end));
                        (*start, *end) = (normalize(s), normalize(e));
                    }
                    Graphic::Text { at, anchor, .. } => {
                        *at = point(*at);
                        if mirrored {
                            *anchor = match anchor {
                                Anchor::Left => Anchor::Right,
                                Anchor::Right => Anchor::Left,
                                Anchor::Center => Anchor::Center,
                            };
                        }
                    }
                }
            }
            Owner::Pin(i) => {
                if let Some(pin) = symbol.pins.get_mut(i) {
                    pin.at = point(pin.at);
                    pin.direction = direction(pin.direction);
                }
            }
            _ => {}
        }
    }
}

/// An angle in degrees in `[0, 360)`, rounded to a tenth of a degree.
pub fn normalize(degrees: f64) -> f64 {
    let a = (degrees * 10.0).round() / 10.0;
    let a = a.rem_euclid(360.0);
    if a == -0.0 { 0.0 } else { a }
}

/// The angle of `p` seen from `center`, counter-clockwise on screen from +X, in degrees.
pub fn angle(center: Point, p: Point) -> f64 {
    normalize((-(p.y - center.y).mm()).atan2((p.x - center.x).mm()).to_degrees())
}

pub fn distance(a: Point, b: Point) -> Length {
    Length::from_mm((a.x - b.x).mm().hypot((a.y - b.y).mm()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::cell::{Port, PortKind};

    fn block() -> CellDef {
        let mut def = CellDef { name: "blk".into(), ..CellDef::default() };
        for (name, kind) in [("a", PortKind::Input), ("y", PortKind::Output)] {
            def.cell.ports.push(Port::new(name, kind));
        }
        generate(&mut def);
        def
    }

    fn on_grid(p: Point) -> bool {
        p.x.nm() % GRID.nm() == 0 && p.y.nm() % GRID.nm() == 0
    }

    #[test]
    fn pins_are_added_once_per_port() {
        let mut def = block();
        assert!(unpinned_ports(&def).is_empty());
        delete(&mut def, &BTreeSet::from([Owner::Pin(0)]));
        assert_eq!(unpinned_ports(&def), ["a"]);
        assert!(add_pin(&mut def, "nope", Point::ORIGIN, PinDirection::Right).is_err());
        add_pin(&mut def, "a", Point::mils(-400, 0), PinDirection::Right).unwrap();
        assert!(add_pin(&mut def, "a", Point::mils(-500, 0), PinDirection::Right).is_err());
        assert!(unpinned_ports(&def).is_empty());
    }

    #[test]
    fn rotating_four_times_is_identity_and_keeps_pins_on_grid() {
        let mut def = block();
        add_graphic(
            &mut def,
            Graphic::Arc { center: Point::mils(50, 0), radius: GRID, start: 0.0, end: 90.0 },
        );
        let original = def.clone();
        let all: BTreeSet<Owner> =
            (0..4).map(Owner::Graphic).chain((0..2).map(Owner::Pin)).collect();
        rotate(&mut def, &all);
        let s = def.symbol.as_ref().unwrap();
        assert!(s.pins.iter().all(|p| on_grid(p.at)));
        assert_eq!(s.pin("a").unwrap().direction, PinDirection::Up, "left pin now at the bottom");
        assert!(s.pin("a").unwrap().at.y > Length::ZERO);
        for _ in 0..3 {
            rotate(&mut def, &all);
        }
        assert_eq!(def, original);
    }

    #[test]
    fn mirroring_swaps_sides_and_reverses_arcs() {
        let mut def = block();
        let arc = add_graphic(
            &mut def,
            Graphic::Arc { center: Point::ORIGIN, radius: GRID, start: 0.0, end: 90.0 },
        );
        let all: BTreeSet<Owner> =
            (0..=arc).map(Owner::Graphic).chain((0..2).map(Owner::Pin)).collect();
        mirror(&mut def, &all);
        let s = def.symbol.as_ref().unwrap();
        assert_eq!(s.pin("a").unwrap().direction, PinDirection::Left);
        let Graphic::Arc { start, end, .. } = s.graphics[arc] else { panic!() };
        assert_eq!((start, end), (90.0, 180.0));
        mirror(&mut def, &all);
        assert_eq!(def, block_with_arc());

        fn block_with_arc() -> CellDef {
            let mut def = block();
            add_graphic(
                &mut def,
                Graphic::Arc { center: Point::ORIGIN, radius: GRID, start: 0.0, end: 90.0 },
            );
            def
        }
    }

    #[test]
    fn angles_are_counter_clockwise_on_screen() {
        assert_eq!(angle(Point::ORIGIN, Point::mils(100, 0)), 0.0);
        assert_eq!(angle(Point::ORIGIN, Point::mils(0, -100)), 90.0);
        assert_eq!(angle(Point::ORIGIN, Point::mils(0, 100)), 270.0);
        assert_eq!(normalize(-90.0), 270.0);
    }
}
