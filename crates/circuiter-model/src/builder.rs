//! The component builder: box symbols generated from a cell's ports, kept in step with
//! the interface as ports are added, removed, renamed or retyped.
//!
//! Layout follows the IEEE 315 block conventions: inputs on the left, outputs on the
//! right, supplies on top and grounds at the bottom. Every pin sits on the 100-mil grid
//! so schematic wires can reach it.

use crate::cell::{Cell, CellDef, Port, PortKind};
use crate::geom::{Length, Point};
use crate::symbol::{Anchor, Graphic, PinDirection, Symbol, SymbolPin};

/// Pin pitch and length, in mils: one schematic grid step.
const PITCH: i64 = 100;
/// Pin-name metrics in mils, matching the renderer: 40-mil text at 0.6 em per character,
/// set 20 mils inside the body edge.
const NAME_CHAR: i64 = 24;
const NAME_GAP: i64 = 20;
/// Clear space between left and right names facing each other across the body.
const NAME_CLEARANCE: i64 = 60;
const MIN_WIDTH: i64 = 400;
/// Reference and cell-name text.
const TEXT_SIZE: i64 = 50;

/// The side of a box symbol a pin is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    /// The direction pins on this side point, from the connection point to the body.
    pub fn direction(self) -> PinDirection {
        match self {
            Side::Left => PinDirection::Right,
            Side::Right => PinDirection::Left,
            Side::Top => PinDirection::Down,
            Side::Bottom => PinDirection::Up,
        }
    }
}

fn is_ground_like(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["gnd", "vss", "vee", "ground", "vneg"].contains(&n.as_str())
        || n.starts_with("gnd")
        || n.ends_with("gnd")
        || n.starts_with("vss")
        || n.starts_with("vee")
}

fn is_supply_like(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("vdd") || n.starts_with("vcc") || n == "vpos"
}

/// The side the generator puts each port on, in interface order. Passive ports without
/// a telling name are shared between left and right to keep the box compact.
pub fn sides(cell: &Cell) -> Vec<Side> {
    let fixed: Vec<Option<Side>> = cell
        .ports
        .iter()
        .map(|p| match p.kind {
            PortKind::Input => Some(Side::Left),
            PortKind::Output | PortKind::Bidirectional | PortKind::PowerOut => Some(Side::Right),
            PortKind::PowerIn | PortKind::Passive if is_ground_like(&p.name) => Some(Side::Bottom),
            PortKind::PowerIn => Some(Side::Top),
            PortKind::Passive if is_supply_like(&p.name) => Some(Side::Top),
            PortKind::Passive => None,
        })
        .collect();
    let count = |s| fixed.iter().filter(|f| **f == Some(s)).count();
    let (mut left, mut right) = (count(Side::Left), count(Side::Right));
    fixed
        .into_iter()
        .map(|f| {
            f.unwrap_or_else(|| {
                if left <= right {
                    left += 1;
                    Side::Left
                } else {
                    right += 1;
                    Side::Right
                }
            })
        })
        .collect()
}

fn chars(port: &Port) -> i64 {
    port.name.chars().count() as i64
}

fn round_up(v: i64, step: i64) -> i64 {
    (v + step - 1).div_euclid(step) * step
}

/// Generates a box symbol for `cell`, which is named `name`. Hidden (global) ports get
/// no pin.
pub fn generate(name: &str, cell: &Cell) -> Symbol {
    let visible = Cell {
        ports: cell.ports.iter().filter(|p| p.global.is_none()).cloned().collect(),
        ..cell.clone()
    };
    let cell = &visible;
    let sides = sides(cell);
    let on = |side: Side| -> Vec<usize> {
        (0..cell.ports.len()).filter(|&i| sides[i] == side).collect()
    };
    let (left, right, top, bottom) =
        (on(Side::Left), on(Side::Right), on(Side::Top), on(Side::Bottom));
    let widest = |ports: &[usize]| ports.iter().map(|&i| chars(&cell.ports[i])).max();

    // Left and right names must fit side by side; top and bottom pins need room for
    // their names between neighbours and a pitch clear of each corner.
    let name_width = |ports: &[usize]| widest(ports).map_or(0, |n| NAME_GAP + n * NAME_CHAR);
    let step = |ports: &[usize]| {
        round_up(widest(ports).map_or(PITCH, |n| (n * NAME_CHAR + 2 * NAME_GAP).max(PITCH)), PITCH)
    };
    let (top_step, bottom_step) = (step(&top), step(&bottom));
    let span = |ports: &[usize], step: i64| ports.len().saturating_sub(1) as i64 * step;
    // Sections of side pins, in the order their groups first appear.
    let mut groups: Vec<&str> = Vec::new();
    for (i, port) in cell.ports.iter().enumerate() {
        if matches!(sides[i], Side::Left | Side::Right) && !groups.contains(&port.group.as_str()) {
            groups.push(&port.group);
        }
    }
    let sectioned = groups.len() > 1 || groups.first().is_some_and(|g| !g.is_empty());
    let title_width = |g: &&str| g.chars().count() as i64 * TEXT_SIZE * 3 / 5 + 2 * NAME_GAP;
    let width = round_up(
        (name_width(&left) + name_width(&right) + NAME_CLEARANCE)
            .max(span(&top, top_step) + 2 * PITCH)
            .max(span(&bottom, bottom_step) + 2 * PITCH)
            .max(if sectioned { groups.iter().map(title_width).max().unwrap_or(0) } else { 0 })
            .max(MIN_WIDTH),
        PITCH,
    );

    // Each side pin's distance below the top edge. Unsectioned, a shorter side is
    // centred against the longer one; sectioned, each section's pins start below its
    // title, and a rule closes it off from the next.
    let mut depth = vec![0; cell.ports.len()];
    let mut rules = Vec::new();
    let mut titles = Vec::new();
    let height = if sectioned {
        let mut cursor = 0;
        for (k, g) in groups.iter().enumerate() {
            if k > 0 {
                rules.push(cursor);
            }
            if !g.is_empty() {
                titles.push((cursor + PITCH, *g));
                cursor += PITCH;
            }
            let mut rows = 0;
            for side in [&left, &right] {
                let mine: Vec<usize> =
                    side.iter().copied().filter(|&i| cell.ports[i].group == *g).collect();
                for (k, &i) in mine.iter().enumerate() {
                    depth[i] = cursor + (k as i64 + 1) * PITCH;
                }
                rows = rows.max(mine.len() as i64);
            }
            cursor += (rows + 1) * PITCH;
        }
        cursor
    } else {
        let rows = left.len().max(right.len()).max(1) as i64;
        for side in [&left, &right] {
            let offset = (rows - side.len() as i64) / 2;
            for (k, &i) in side.iter().enumerate() {
                depth[i] = (offset + k as i64 + 1) * PITCH;
            }
        }
        (rows + 1) * PITCH
    };
    let (x0, y0) = (-(width / PITCH / 2) * PITCH, -(height / PITCH / 2) * PITCH);
    let (x1, y1) = (x0 + width, y0 + height);

    let mut at = vec![Point::ORIGIN; cell.ports.len()];
    for &i in &left {
        at[i] = Point::mils(x0 - PITCH, y0 + depth[i]);
    }
    for &i in &right {
        at[i] = Point::mils(x1 + PITCH, y0 + depth[i]);
    }
    let row = |ports: &[usize], step: i64, y: i64, at: &mut Vec<Point>| {
        let start = x0 + (width - span(ports, step)) / 2 / PITCH * PITCH;
        for (k, &i) in ports.iter().enumerate() {
            at[i] = Point::mils(start + k as i64 * step, y);
        }
    };
    row(&top, top_step, y0 - PITCH, &mut at);
    row(&bottom, bottom_step, y1 + PITCH, &mut at);

    let pins = cell
        .ports
        .iter()
        .zip(&sides)
        .zip(&at)
        .map(|((port, side), &at)| SymbolPin {
            port: port.name.clone(),
            at,
            direction: side.direction(),
            length: Length::from_mils(PITCH),
            show_name: true,
        })
        .collect();

    // The reference goes above the top-left corner and the cell name below the
    // bottom-left one, moving out to the right when pins are in the way.
    let text = |at: Point, text: &str| Graphic::Text {
        at,
        text: text.to_owned(),
        size: Length::from_mils(TEXT_SIZE),
        anchor: Anchor::Left,
    };
    // Text is 0.6 em per character; a reference is budgeted at five (`U123`).
    let fits_left = |pins: &[usize], chars: i64| {
        let first = pins.iter().map(|&i| at[i].x.nm() / Length::MIL.nm()).min();
        first.is_none_or(|x| x - x0 >= chars * TEXT_SIZE * 3 / 5 + NAME_GAP)
    };
    let place = |pins: &[usize], chars: i64, y: i64| {
        if fits_left(pins, chars) { Point::mils(x0, y) } else { Point::mils(x1 + NAME_GAP, y) }
    };
    let mut graphics = vec![
        Graphic::Rect { a: Point::mils(x0, y0), b: Point::mils(x1, y1), fill: false },
        text(place(&top, 5, y0 - TEXT_SIZE), "${name}"),
        text(place(&bottom, name.chars().count() as i64, y1 + TEXT_SIZE), name),
    ];
    for y in rules {
        graphics.push(Graphic::Polyline {
            points: vec![Point::mils(x0, y0 + y), Point::mils(x1, y0 + y)],
            fill: false,
        });
    }
    for (y, title) in titles {
        graphics.push(Graphic::Text {
            at: Point::mils((x0 + x1) / 2, y0 + y),
            text: title.to_owned(),
            size: Length::from_mils(TEXT_SIZE),
            anchor: Anchor::Center,
        });
    }
    Symbol { generated: true, graphics, pins, states: Vec::new() }
}

/// Whether `def`'s symbol was made by [`generate`] and not edited since.
pub fn is_generated(def: &CellDef) -> bool {
    def.symbol.as_ref().is_some_and(|s| s.generated)
}

/// Brings `after`'s symbol in line with its ports after an edit that started from
/// `before`.
///
/// Editing a generated drawing by hand makes it a custom one. A symbol that was
/// generated from the old ports is regenerated, so generated symbols follow the
/// interface. A custom symbol keeps its drawing: pins of removed ports go, and new ports
/// get pins on their usual side, past the pins already there. A cell without a symbol
/// gets one as soon as it has ports, which makes a new cell placeable the moment it has
/// an interface.
pub fn sync(before: &CellDef, after: &mut CellDef) {
    let fresh = generate(&after.name, &after.cell);
    if let Some(s) = after.symbol.as_mut() {
        if s.generated && Some(&*s) != before.symbol.as_ref() && *s != fresh {
            s.generated = false;
        }
    }
    if before.cell.ports == after.cell.ports || after.cell.global_net.is_some() {
        return;
    }
    if before.symbol.is_none() || is_generated(before) {
        after.symbol = (!after.cell.ports.is_empty()).then_some(fresh);
        return;
    }
    let Some(symbol) = after.symbol.as_mut() else { return };
    symbol.pins.retain(|p| after.cell.port(&p.port).is_some());
    for (port, side) in after.cell.ports.iter().zip(sides(&after.cell)) {
        if port.global.is_none() && symbol.pin(&port.name).is_none() {
            add_pin(symbol, &port.name, side);
        }
    }
}

/// Adds a pin for `port` on `side` of a drawn symbol, one pitch past the last pin on that
/// side, or centred beside the drawing if the side has none.
pub fn add_pin(symbol: &mut Symbol, port: &str, side: Side) {
    let pitch = Length::from_mils(PITCH);
    let direction = side.direction();
    let same = symbol.pins.iter().filter(|p| p.direction == direction).map(|p| p.at);
    let vertical = matches!(side, Side::Left | Side::Right);
    let last = if vertical { same.max_by_key(|p| p.y) } else { same.max_by_key(|p| p.x) };
    let g = pitch.nm();
    let floor = |v: Length| Length(v.nm().div_euclid(g) * g);
    let ceil = |v: Length| Length(round_up(v.nm(), g));
    let mut at = match (last, symbol.bounds()) {
        (Some(p), _) if vertical => Point::new(p.x, p.y + pitch),
        (Some(p), _) => Point::new(p.x + pitch, p.y),
        (None, None) => Point::ORIGIN,
        (None, Some((min, max))) => {
            let mid = |a: Length, b: Length| floor(Length((a.nm() + b.nm()) / 2));
            match side {
                Side::Left => Point::new(floor(min.x) - pitch, mid(min.y, max.y)),
                Side::Right => Point::new(ceil(max.x) + pitch, mid(min.y, max.y)),
                Side::Top => Point::new(mid(min.x, max.x), floor(min.y) - pitch),
                Side::Bottom => Point::new(mid(min.x, max.x), ceil(max.y) + pitch),
            }
        }
    };
    let step =
        if vertical { Point::new(Length::ZERO, pitch) } else { Point::new(pitch, Length::ZERO) };
    while symbol.pins.iter().any(|p| p.at == at) {
        at = at + step;
    }
    symbol.pins.push(SymbolPin {
        port: port.to_owned(),
        at,
        direction,
        length: pitch,
        show_name: true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Port;

    fn port(name: &str, kind: PortKind) -> Port {
        Port::new(name, kind)
    }

    fn cell(ports: &[(&str, PortKind)]) -> Cell {
        Cell { ports: ports.iter().map(|(n, k)| port(n, *k)).collect(), ..Cell::default() }
    }

    fn on_grid(p: Point) -> bool {
        let g = Length::from_mils(PITCH).nm();
        p.x.nm() % g == 0 && p.y.nm() % g == 0
    }

    #[test]
    fn ports_go_to_conventional_sides() {
        use PortKind::*;
        let c = cell(&[
            ("d", Input),
            ("clk", Input),
            ("q", Output),
            ("vdd", PowerIn),
            ("vss", PowerIn),
            ("a", Passive),
            ("b", Passive),
        ]);
        assert_eq!(
            sides(&c),
            [Side::Left, Side::Left, Side::Right, Side::Top, Side::Bottom, Side::Right, Side::Left]
        );
    }

    #[test]
    fn generated_pins_are_on_grid_and_outside_the_body() {
        use PortKind::*;
        let c = cell(&[
            ("data_in", Input),
            ("clk", Input),
            ("enable", Input),
            ("q", Output),
            ("vdd", PowerIn),
            ("vcc_io", PowerIn),
            ("gnd", PowerIn),
        ]);
        let s = generate("dff", &c);
        assert_eq!(s.pins.len(), 7);
        let Some(Graphic::Rect { a, b, .. }) = s.graphics.first() else { panic!("no body") };
        for pin in &s.pins {
            assert!(on_grid(pin.at), "{} at {}", pin.port, pin.at);
            let inside = pin.at.x > a.x && pin.at.x < b.x && pin.at.y > a.y && pin.at.y < b.y;
            assert!(!inside, "{} is inside the body", pin.port);
        }
        assert!(on_grid(*a) && on_grid(*b));
        // Left names and the right name fit side by side.
        let width = (b.x - a.x).nm() / Length::MIL.nm();
        assert!(width >= NAME_GAP * 2 + 7 * NAME_CHAR + NAME_CHAR + NAME_CLEARANCE);
        // No two pins share a connection point.
        let mut points: Vec<Point> = s.pins.iter().map(|p| p.at).collect();
        points.sort();
        points.dedup();
        assert_eq!(points.len(), 7);
    }

    #[test]
    fn a_lone_output_is_centred_against_the_inputs() {
        use PortKind::*;
        let s = generate("and3", &cell(&[("a", Input), ("b", Input), ("c", Input), ("y", Output)]));
        assert_eq!(s.pin("b").unwrap().at.y, s.pin("y").unwrap().at.y);
    }

    #[test]
    fn grouped_ports_get_titled_sections() {
        use PortKind::*;
        let mut c = cell(&[
            ("pa0", Bidirectional),
            ("pa1", Bidirectional),
            ("pb0", Bidirectional),
            ("rst", Input),
            ("vdd", PowerIn),
        ]);
        for (port, group) in c.ports.iter_mut().zip(["Bank A", "Bank A", "Bank B", "Bank A", ""]) {
            port.group = group.into();
        }
        let s = generate("mcu", &c);
        let Some(Graphic::Rect { a, b, .. }) = s.graphics.first() else { panic!("no body") };
        let titles: Vec<(&str, Point)> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Graphic::Text { text, at, anchor: Anchor::Center, .. } => {
                    Some((text.as_str(), *at))
                }
                _ => None,
            })
            .collect();
        assert_eq!(titles.iter().map(|t| t.0).collect::<Vec<_>>(), ["Bank A", "Bank B"]);
        let rules: Vec<Length> = s
            .graphics
            .iter()
            .filter_map(|g| match g {
                Graphic::Polyline { points, .. } => Some(points[0].y),
                _ => None,
            })
            .collect();
        assert_eq!(rules.len(), 1);
        let y = |p: &str| s.pin(p).unwrap().at.y;
        // Bank A's pins sit between its title and the rule, Bank B's below the rule.
        for p in ["pa0", "pa1", "rst"] {
            assert!(y(p) > titles[0].1.y && y(p) < rules[0], "{p}");
        }
        assert!(y("pb0") > titles[1].1.y && titles[1].1.y > rules[0]);
        assert_eq!(y("pa0"), y("rst"), "left and right start level");
        assert!(a.y < titles[0].1.y && y("pb0") < b.y);
        for pin in &s.pins {
            assert!(on_grid(pin.at), "{} at {}", pin.port, pin.at);
        }
        // The supply stays on top, outside the sections.
        assert!(y("vdd") < a.y);
    }

    #[test]
    fn ungrouped_symbols_are_unchanged() {
        use PortKind::*;
        let c = cell(&[("a", Input), ("b", Input), ("y", Output)]);
        let s = generate("and2", &c);
        assert_eq!(s.graphics.len(), 3);
        assert_eq!(s.pin("a").unwrap().at, Point::mils(-300, 0));
        assert_eq!(s.pin("b").unwrap().at, Point::mils(-300, 100));
        assert_eq!(s.pin("y").unwrap().at, Point::mils(300, 0));
    }

    fn def(ports: &[(&str, PortKind)], symbol: Option<Symbol>) -> CellDef {
        CellDef { name: "blk".into(), cell: cell(ports), symbol, ..CellDef::default() }
    }

    #[test]
    fn generated_symbols_follow_the_ports() {
        use PortKind::*;
        let mut before = def(&[], None);
        let mut after = def(&[("in", Input)], None);
        sync(&before, &mut after);
        assert!(is_generated(&after), "a cell gets a symbol with its first port");

        before = after.clone();
        after.cell.ports.push(port("out", Output));
        sync(&before, &mut after);
        assert!(is_generated(&after));
        assert!(after.symbol.as_ref().unwrap().pin("out").is_some());

        before = after.clone();
        after.cell.ports.clear();
        sync(&before, &mut after);
        assert_eq!(after.symbol, None);
    }

    #[test]
    fn drawn_symbols_keep_their_drawing() {
        use PortKind::*;
        let mut drawn = generate("blk", &cell(&[("in", Input), ("out", Output)]));
        drawn.generated = false;
        drawn.graphics.push(Graphic::Circle {
            center: Point::ORIGIN,
            radius: Length::from_mils(50),
            fill: false,
        });
        let before = def(&[("in", Input), ("out", Output)], Some(drawn.clone()));
        let mut after = def(&[("in", Input), ("en", Input)], Some(drawn.clone()));
        sync(&before, &mut after);
        let s = after.symbol.as_ref().unwrap();
        assert_eq!(s.graphics, drawn.graphics);
        assert!(s.pin("out").is_none());
        let (inp, en) = (s.pin("in").unwrap(), s.pin("en").unwrap());
        assert_eq!(en.direction, PinDirection::Right);
        assert_eq!(en.at, Point::new(inp.at.x, inp.at.y + Length::from_mils(PITCH)));
    }

    #[test]
    fn editing_the_drawing_makes_it_custom() {
        use PortKind::*;
        let before = def(&[("in", Input)], Some(generate("blk", &cell(&[("in", Input)]))));
        let mut after = before.clone();
        after.symbol.as_mut().unwrap().pins[0].show_name = false;
        sync(&before, &mut after);
        assert!(!is_generated(&after));
        // Regenerating by hand is not an edit of the drawing.
        let mut regenerated = after.clone();
        regenerated.symbol = Some(generate("blk", &regenerated.cell));
        sync(&after, &mut regenerated);
        assert!(is_generated(&regenerated));
    }

    #[test]
    fn new_pins_avoid_existing_ones() {
        let mut s = Symbol::default();
        add_pin(&mut s, "a", Side::Left);
        add_pin(&mut s, "b", Side::Right);
        add_pin(&mut s, "c", Side::Left);
        assert_ne!(s.pins[0].at, s.pins[1].at);
        assert_ne!(s.pins[0].at, s.pins[2].at);
    }
}
