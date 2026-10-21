//! Display lists for schematics.
//!
//! [`Scene::build`] turns a cell's schematic into flat drawing primitives in sheet
//! coordinates (millimetres, +Y down), tagged with what they depict ([`Role`]) and which
//! schematic object they belong to ([`Owner`]). Renderers (the GUI canvas, SVG export,
//! printing) only map roles to colours and primitives to their drawing API.

mod svg;
mod theme;

pub use svg::to_svg;
pub use theme::{Rgba, Theme};

use circuiter_model::cell::{Cell, ParamKind, PortKind};
use circuiter_model::connectivity::{Connectivity, Item};
use circuiter_model::geom::{Length, Point, Transform};
use circuiter_model::schematic::{FRAME_MARGIN, Frame, Instance, Note, Schematic, TITLE_BLOCK};
use circuiter_model::symbol::{Anchor, Graphic, PinDirection, Symbol};
use circuiter_model::units::{format_quantity, parse_quantity};
use circuiter_model::{CellDef, CellId, Design};
use std::collections::BTreeMap;

/// Standard line width: 6 mil, as on a printed schematic.
pub const LINE_WIDTH: f64 = 0.1524;
/// Junction dot radius (36 mil diameter).
pub const JUNCTION_RADIUS: f64 = 0.4572;
/// Approximate advance width of a character, as a fraction of text size (monospace).
pub const CHAR_WIDTH: f64 = 0.6;

/// How wide renderers stroke lines of a role, in millimetres: wires a little heavier
/// than drawings, buses three times as heavy.
pub fn line_width(role: Role) -> f64 {
    match role {
        Role::Wire => LINE_WIDTH * 1.2,
        Role::Bus => LINE_WIDTH * 3.0,
        _ => LINE_WIDTH,
    }
}

/// Whether lines of a role are drawn dashed (`DASH` on, `GAP` off, in millimetres).
pub fn dashed(role: Role) -> bool {
    role == Role::Note
}

pub const DASH: f64 = 1.27;
pub const GAP: f64 = 0.762;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub fn dist(self, other: Pt) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    pub const fn new(x: f64, y: f64) -> Self {
        Pt { x, y }
    }
}

impl From<Point> for Pt {
    fn from(p: Point) -> Self {
        Pt::new(p.x.mm(), p.y.mm())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: Pt,
    pub max: Pt,
}

impl Rect {
    pub const EMPTY: Rect = Rect {
        min: Pt::new(f64::INFINITY, f64::INFINITY),
        max: Pt::new(f64::NEG_INFINITY, f64::NEG_INFINITY),
    };

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }

    pub fn include(&mut self, p: Pt) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
    }

    pub fn union(&mut self, other: &Rect) {
        if !other.is_empty() {
            self.include(other.min);
            self.include(other.max);
        }
    }

    pub fn expand(&self, by: f64) -> Rect {
        Rect {
            min: Pt::new(self.min.x - by, self.min.y - by),
            max: Pt::new(self.max.x + by, self.max.y + by),
        }
    }

    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    pub fn area(&self) -> f64 {
        if self.is_empty() { 0.0 } else { (self.max.x - self.min.x) * (self.max.y - self.min.y) }
    }

    pub fn width(&self) -> f64 {
        self.max.x - self.min.x
    }

    pub fn height(&self) -> f64 {
        self.max.y - self.min.y
    }

    pub fn center(&self) -> Pt {
        Pt::new((self.min.x + self.max.x) / 2.0, (self.min.y + self.max.y) / 2.0)
    }
}

/// What a primitive depicts. Renderers colour by role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Wire,
    Junction,
    /// Symbol outline.
    Body,
    /// Symbol pin lines.
    Pin,
    PinName,
    /// The pad a pin lands on, printed over the pin line.
    PinNumber,
    /// Reference designator (`R1`).
    Reference,
    /// Parameter values (`4.7kΩ`).
    Value,
    NetLabel,
    /// Hierarchical port flags and their names.
    Port,
    /// Unconnected pins and wire ends.
    Unconnected,
    /// A wire carrying several bits, and the slash and count that give its width.
    Bus,
    /// Notes: dashed group boxes and free text.
    Note,
    /// The sheet's border, grid references and title block.
    Frame,
    /// Editing aids that are not part of the design, such as the symbol origin.
    Guide,
}

/// The schematic object a primitive belongs to, for selection and highlighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Owner {
    Instance(usize),
    Wire(usize),
    Label(usize),
    Port(usize),
    /// A drawing primitive of the symbol being edited.
    Graphic(usize),
    /// A pin of the symbol being edited.
    Pin(usize),
    /// Index into [`Schematic::notes`].
    Note(usize),
    /// Junction dots and markers: derived from connectivity, not selectable themselves.
    Derived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VAlign {
    Top,
    Center,
    Bottom,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    Polyline {
        points: Vec<Pt>,
        closed: bool,
        filled: bool,
    },
    Circle {
        center: Pt,
        radius: f64,
        filled: bool,
    },
    /// Text. `vertical` text is turned 90° counter-clockwise, reading bottom to top.
    Text {
        at: Pt,
        text: String,
        size: f64,
        halign: HAlign,
        valign: VAlign,
        vertical: bool,
    },
}

impl Shape {
    pub fn bounds(&self) -> Rect {
        let mut r = Rect::EMPTY;
        match self {
            Shape::Polyline { points, .. } => points.iter().for_each(|p| r.include(*p)),
            Shape::Circle { center, radius, .. } => {
                r.include(Pt::new(center.x - radius, center.y - radius));
                r.include(Pt::new(center.x + radius, center.y + radius));
            }
            Shape::Text { at, text, size, halign, valign, vertical } => {
                let w = text.chars().count() as f64 * size * CHAR_WIDTH;
                // Extent along the reading direction (u) and across it (v, down the glyphs).
                let u0 = match halign {
                    HAlign::Left => 0.0,
                    HAlign::Center => -w / 2.0,
                    HAlign::Right => -w,
                };
                let v0 = match valign {
                    VAlign::Top => 0.0,
                    VAlign::Center => -size / 2.0,
                    VAlign::Bottom => -size,
                };
                if *vertical {
                    // Reading up the page: u runs toward -Y, v toward +X.
                    r.include(Pt::new(at.x + v0, at.y - u0 - w));
                    r.include(Pt::new(at.x + v0 + size, at.y - u0));
                } else {
                    r.include(Pt::new(at.x + u0, at.y + v0));
                    r.include(Pt::new(at.x + u0 + w, at.y + v0 + size));
                }
            }
        }
        r
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub role: Role,
    pub owner: Owner,
    pub shape: Shape,
}

/// A schematic ready to draw.
#[derive(Debug, Clone)]
pub struct Scene {
    pub cell: CellId,
    pub elements: Vec<Element>,
    /// Extent of everything drawn.
    pub bounds: Rect,
    /// Per instance: the symbol body and pins, used for selection.
    pub instance_bounds: Vec<Rect>,
    /// Per instance: the symbol's drawing alone, without pins or text. A click inside
    /// it picks the part even with a wire close by.
    pub instance_bodies: Vec<Rect>,
    /// Per instance: whether its cell has a schematic to descend into.
    pub descendable: Vec<bool>,
    pub connectivity: Connectivity,
}

impl Scene {
    /// Builds the scene for a cell's schematic. Returns `None` if it has no schematic.
    pub fn build(design: &Design, id: &CellId) -> Option<Scene> {
        let def = design.cell(id)?;
        let schematic = def.schematic.as_ref()?;
        Some(Self::build_from(design, id, &def.cell, schematic))
    }

    /// A scene showing one instance of the cell's symbol at the origin, for previewing
    /// library parts. Returns `None` if the cell has no symbol.
    pub fn symbol_preview(design: &Design, id: &CellId) -> Option<Scene> {
        let def = design.cell(id)?;
        def.symbol.as_ref()?;
        let instance = Instance {
            fields: Default::default(),
            name: format!("{}?", def.cell.prefix),
            cell: circuiter_model::CellRef::new(Some(&id.library), &id.cell),
            placement: Transform::default(),
            params: Default::default(),
            view: None,
        };
        let schematic = Schematic { instances: vec![instance], ..Schematic::default() };
        let mut scene = Self::build_from(design, id, &Cell::default(), &schematic);
        // A lone symbol has nothing attached; don't flag its pins as unconnected.
        scene.elements.retain(|e| e.role != Role::Unconnected);
        Some(scene)
    }

    /// The symbol being edited: like [`Scene::symbol_preview`], but each graphic and pin
    /// is its own object ([`Owner::Graphic`], [`Owner::Pin`]), pin connection points are
    /// marked and the origin is shown. A cell without a symbol gives an empty sheet.
    pub fn symbol_sheet(design: &Design, id: &CellId) -> Option<Scene> {
        let def = design.cell(id)?;
        let mut b = Builder::default();
        let s = 0.635;
        for (from, to) in [((-s, 0.0), (s, 0.0)), ((0.0, -s), (0.0, s))] {
            b.push(
                Role::Guide,
                Owner::Derived,
                Shape::Polyline {
                    points: vec![Pt::new(from.0, from.1), Pt::new(to.0, to.1)],
                    closed: false,
                    filled: false,
                },
            );
        }
        if let Some(symbol) = &def.symbol {
            let instance = Instance {
                fields: Default::default(),
                name: format!("{}?", def.cell.prefix),
                cell: circuiter_model::CellRef::new(Some(&id.library), &id.cell),
                placement: Transform::default(),
                params: Default::default(),
                view: None,
            };
            b.symbol(None, &instance, def, symbol);
        }
        let mut bounds = Rect::EMPTY;
        for e in &b.elements {
            bounds.union(&e.shape.bounds());
        }
        Some(Scene {
            cell: id.clone(),
            elements: b.elements,
            bounds,
            instance_bounds: Vec::new(),
            instance_bodies: Vec::new(),
            descendable: Vec::new(),
            connectivity: Connectivity::default(),
        })
    }

    /// The scene for `schematic` as a view of cell `id` (which may differ from the stored
    /// one: a running circuit shows its switches as they are now).
    pub fn build_from(design: &Design, id: &CellId, cell: &Cell, schematic: &Schematic) -> Scene {
        let connectivity = Connectivity::analyze(design, &id.library, schematic);
        let mut b = Builder::default();

        let mut instance_bounds = Vec::with_capacity(schematic.instances.len());
        let mut instance_bodies = Vec::with_capacity(schematic.instances.len());
        let mut descendable = Vec::with_capacity(schematic.instances.len());
        for (i, inst) in schematic.instances.iter().enumerate() {
            let resolved = design.resolve(&inst.cell, &id.library).map(|(_, d)| d);
            descendable.push(resolved.is_some_and(|d| d.schematic.is_some()));
            let start = b.elements.len();
            match resolved.and_then(|d| d.symbol.as_ref().map(|s| (d, s))) {
                Some((def, symbol)) => b.symbol(Some(i), inst, def, symbol),
                None => b.missing_symbol(i, inst),
            }
            let mut r = Rect::EMPTY;
            let mut body = Rect::EMPTY;
            for e in &b.elements[start..] {
                if !matches!(e.role, Role::Reference | Role::Value) {
                    r.union(&e.shape.bounds());
                }
                if e.role == Role::Body {
                    body.union(&e.shape.bounds());
                }
            }
            instance_bounds.push(r);
            instance_bodies.push(body);
        }

        let widths = net_widths(design, &id.library, cell, schematic, &connectivity);
        // Each bus is marked with its width once, on its longest straight run.
        let mut longest: BTreeMap<usize, (f64, usize, [Pt; 2])> = BTreeMap::new();
        for (i, wire) in schematic.wires.iter().enumerate() {
            let points: Vec<Pt> = wire.points.iter().map(|&p| p.into()).collect();
            let net = connectivity.net_of(Item::Wire(i));
            let width = net.map_or(1, |n| widths[n]);
            let role = if width > 1 { Role::Bus } else { Role::Wire };
            if let Some(n) = net.filter(|_| width > 1) {
                for seg in points.windows(2) {
                    let len = seg[0].dist(seg[1]);
                    if longest.get(&n).is_none_or(|l| len > l.0) {
                        longest.insert(n, (len, i, [seg[0], seg[1]]));
                    }
                }
            }
            b.push(role, Owner::Wire(i), Shape::Polyline { points, closed: false, filled: false });
        }
        for (n, (len, wire, seg)) in longest {
            if len >= 2.54 {
                b.bus_width(wire, seg, widths[n]);
            }
        }
        for (i, note) in schematic.notes.iter().enumerate() {
            b.note(i, note);
        }
        if let Some(frame) = &schematic.frame {
            b.frame(frame, id);
        }
        for (i, label) in schematic.labels.iter().enumerate() {
            let at = Pt::from(label.at);
            // A global label is a flag, as KiCad draws them.
            if label.global {
                let dir = port_direction(schematic, label.at);
                b.flag(
                    Owner::Label(i),
                    Role::NetLabel,
                    at,
                    dir,
                    &label.net,
                    PortKind::Bidirectional,
                );
                continue;
            }
            // The text sits above its wire, running away from a wire that ends here.
            let (text_at, halign, valign) = match wire_away(schematic, label.at) {
                Some(d) if d.x < -0.5 => {
                    (Pt::new(at.x - 0.254, at.y - 0.254), HAlign::Right, VAlign::Bottom)
                }
                Some(d) if d.y > 0.5 => {
                    (Pt::new(at.x + 0.254, at.y + 0.254), HAlign::Left, VAlign::Top)
                }
                _ => (Pt::new(at.x + 0.254, at.y - 0.254), HAlign::Left, VAlign::Bottom),
            };
            b.push(
                Role::NetLabel,
                Owner::Label(i),
                Shape::Text {
                    at: text_at,
                    text: label.net.clone(),
                    size: 1.27,
                    halign,
                    valign,
                    vertical: false,
                },
            );
        }
        for (i, port) in schematic.ports.iter().enumerate() {
            let kind = cell.port(&port.port).map_or(PortKind::Passive, |p| p.kind);
            let dir = port_direction(schematic, port.at);
            b.port_flag(i, port.at.into(), dir, &port.port, kind);
        }

        // No-connect flags: a cross on the pin.
        for &p in &schematic.no_connects {
            let c = Pt::from(p);
            let h = 0.635;
            for (a, z) in [((-h, -h), (h, h)), ((-h, h), (h, -h))] {
                b.push(
                    Role::Wire,
                    Owner::Derived,
                    Shape::Polyline {
                        points: vec![Pt::new(c.x + a.0, c.y + a.1), Pt::new(c.x + z.0, c.y + z.1)],
                        closed: false,
                        filled: false,
                    },
                );
            }
        }
        for &p in &connectivity.junctions {
            b.push(
                Role::Junction,
                Owner::Derived,
                Shape::Circle { center: p.into(), radius: JUNCTION_RADIUS, filled: true },
            );
        }
        for &i in &connectivity.dangling_pins {
            let at = connectivity.pins[i].at.into();
            b.push(
                Role::Unconnected,
                Owner::Derived,
                Shape::Circle { center: at, radius: 0.381, filled: false },
            );
        }
        for &p in &connectivity.dangling_wire_ends {
            let c = Pt::from(p);
            let h = 0.381;
            let points = vec![
                Pt::new(c.x - h, c.y - h),
                Pt::new(c.x + h, c.y - h),
                Pt::new(c.x + h, c.y + h),
                Pt::new(c.x - h, c.y + h),
            ];
            b.push(
                Role::Unconnected,
                Owner::Derived,
                Shape::Polyline { points, closed: true, filled: false },
            );
        }

        let mut bounds = Rect::EMPTY;
        for e in &b.elements {
            bounds.union(&e.shape.bounds());
        }
        Scene {
            cell: id.clone(),
            elements: b.elements,
            bounds,
            instance_bounds,
            instance_bodies,
            descendable,
            connectivity,
        }
    }

    /// Objects entirely inside `rect`, for rubber-band selection.
    pub fn objects_in(&self, rect: &Rect) -> Vec<Owner> {
        let mut out: Vec<Owner> = self
            .instance_bounds
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.is_empty() && rect.contains(b.min) && rect.contains(b.max))
            .map(|(i, _)| Owner::Instance(i))
            .collect();
        for e in &self.elements {
            if matches!(
                e.owner,
                Owner::Wire(_)
                    | Owner::Label(_)
                    | Owner::Port(_)
                    | Owner::Graphic(_)
                    | Owner::Pin(_)
                    | Owner::Note(_)
            ) {
                let b = e.shape.bounds();
                if rect.contains(b.min) && rect.contains(b.max) && !out.contains(&e.owner) {
                    out.push(e.owner);
                }
            }
        }
        out
    }

    /// Finds the object under `p`. `tolerance` is in millimetres (a few pixels' worth).
    /// Wires and labels win over instances, and smaller instances over larger ones, so
    /// overlapping objects stay pickable.
    pub fn hit_test(&self, p: Pt, tolerance: f64) -> Option<Owner> {
        // Inside a part's drawing, the part wins: wires ending at its pins sit close by.
        if let Some((i, _)) = self
            .instance_bodies
            .iter()
            .enumerate()
            .filter(|(_, r)| !r.is_empty() && r.expand(tolerance * 0.5).contains(p))
            .min_by(|(_, a), (_, b)| a.area().total_cmp(&b.area()))
        {
            return Some(Owner::Instance(i));
        }
        let mut best_wire: Option<(f64, Owner)> = None;
        for e in &self.elements {
            let d = match (&e.owner, &e.shape) {
                (Owner::Wire(_), Shape::Polyline { points, .. }) => points
                    .windows(2)
                    .map(|s| segment_distance(p, s[0], s[1]))
                    .fold(f64::INFINITY, f64::min),
                (Owner::Label(_) | Owner::Port(_), shape) => {
                    if shape.bounds().expand(tolerance).contains(p) {
                        0.0
                    } else {
                        continue;
                    }
                }
                // Pins win ties with the graphics they overlap.
                (Owner::Pin(_), shape) => shape_distance(shape, p) - tolerance / 2.0,
                (Owner::Graphic(_) | Owner::Note(_), shape) => shape_distance(shape, p),
                _ => continue,
            };
            if d <= tolerance && best_wire.is_none_or(|(bd, _)| d < bd) {
                best_wire = Some((d, e.owner));
            }
        }
        if let Some((_, owner)) = best_wire {
            return Some(owner);
        }
        self.instance_bounds
            .iter()
            .enumerate()
            .filter(|(_, r)| r.expand(tolerance).contains(p))
            .min_by(|(_, a), (_, b)| a.area().total_cmp(&b.area()))
            .map(|(i, _)| Owner::Instance(i))
    }
}

/// The drawing of one instance, as it would appear on a sheet in `library`. Used for
/// previews such as the part following the cursor while placing.
pub fn instance_elements(design: &Design, library: &str, inst: &Instance) -> Vec<Element> {
    let mut b = Builder::default();
    match design.resolve(&inst.cell, library).and_then(|(_, d)| d.symbol.as_ref().map(|s| (d, s))) {
        Some((def, symbol)) => b.symbol(Some(0), inst, def, symbol),
        None => b.missing_symbol(0, inst),
    }
    b.elements
}

/// The direction a port flag should extend: away from the wire attached at `at`.
fn port_direction(schematic: &Schematic, at: Point) -> Pt {
    wire_away(schematic, at).unwrap_or(Pt::new(-1.0, 0.0))
}

/// The direction away from a wire that ends at `at`, if one does.
fn wire_away(schematic: &Schematic, at: Point) -> Option<Pt> {
    for wire in &schematic.wires {
        for seg in wire.points.windows(2) {
            let other = if seg[0] == at {
                seg[1]
            } else if seg[1] == at {
                seg[0]
            } else {
                continue;
            };
            let (dx, dy) = ((at.x - other.x).mm(), (at.y - other.y).mm());
            let len = dx.hypot(dy);
            if len > 0.0 {
                return Some(Pt::new(dx / len, dy / len));
            }
        }
    }
    None
}

/// Distance from `p` to the drawn outline of a shape, or zero inside a filled shape or
/// the box around a text.
fn shape_distance(shape: &Shape, p: Pt) -> f64 {
    match shape {
        Shape::Polyline { points, closed, filled } => {
            let n = points.len();
            let edges = if *closed { n } else { n.saturating_sub(1) };
            let outline = (0..edges)
                .map(|i| segment_distance(p, points[i], points[(i + 1) % n]))
                .fold(f64::INFINITY, f64::min);
            if *filled && *closed && polygon_contains(points, p) { 0.0 } else { outline }
        }
        Shape::Circle { center, radius, filled } => {
            let d = (p.x - center.x).hypot(p.y - center.y);
            if *filled { (d - radius).max(0.0) } else { (d - radius).abs() }
        }
        Shape::Text { .. } => {
            if shape.bounds().contains(p) {
                0.0
            } else {
                let b = shape.bounds();
                let dx = (b.min.x - p.x).max(p.x - b.max.x).max(0.0);
                let dy = (b.min.y - p.y).max(p.y - b.max.y).max(0.0);
                dx.hypot(dy)
            }
        }
    }
}

fn polygon_contains(points: &[Pt], p: Pt) -> bool {
    let mut inside = false;
    let n = points.len();
    for i in 0..n {
        let (a, b) = (points[i], points[(i + n - 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

fn segment_distance(p: Pt, a: Pt, b: Pt) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    };
    (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
}

#[derive(Default)]
struct Builder {
    elements: Vec<Element>,
}

impl Builder {
    fn push(&mut self, role: Role, owner: Owner, shape: Shape) {
        self.elements.push(Element { role, owner, shape });
    }

    /// Draws a symbol as instance `index` of a sheet, or for editing when `index` is
    /// `None`, with each graphic and pin a separate object.
    fn graphic(&mut self, g: &Graphic, owner: Owner, inst: &Instance, def: &CellDef) {
        let t = &inst.placement;
        let map = |p: Point| Pt::from(t.apply(p));
        match g {
            Graphic::Polyline { points, fill } => {
                let closed = points.len() > 2 && points.first() == points.last();
                let mut points: Vec<Pt> = points.iter().map(|&p| map(p)).collect();
                if closed {
                    points.pop();
                }
                self.push(Role::Body, owner, Shape::Polyline { points, closed, filled: *fill });
            }
            Graphic::Rect { a, b, fill } => {
                let corners = [*a, Point::new(b.x, a.y), *b, Point::new(a.x, b.y)];
                let points = corners.iter().map(|&p| map(p)).collect();
                self.push(
                    Role::Body,
                    owner,
                    Shape::Polyline { points, closed: true, filled: *fill },
                );
            }
            Graphic::Circle { center, radius, fill } => self.push(
                Role::Body,
                owner,
                Shape::Circle { center: map(*center), radius: radius.mm(), filled: *fill },
            ),
            Graphic::Arc { center, radius, start, end } => {
                let points =
                    arc_points(*center, *radius, *start, *end).into_iter().map(&map).collect();
                self.push(
                    Role::Body,
                    owner,
                    Shape::Polyline { points, closed: false, filled: false },
                );
            }
            Graphic::Text { at, text, size, anchor } => {
                let role = if text.contains("${name}") { Role::Reference } else { Role::Value };
                let halign = match (anchor, t.mirror) {
                    (Anchor::Center, _) => HAlign::Center,
                    (Anchor::Left, false) | (Anchor::Right, true) => HAlign::Left,
                    (Anchor::Left, true) | (Anchor::Right, false) => HAlign::Right,
                };
                self.push(
                    role,
                    owner,
                    Shape::Text {
                        at: map(*at),
                        text: substitute(text, inst, def),
                        size: size.mm(),
                        halign,
                        valign: VAlign::Center,
                        vertical: false,
                    },
                );
            }
        }
    }

    fn symbol(&mut self, index: Option<usize>, inst: &Instance, def: &CellDef, symbol: &Symbol) {
        for (i, g) in symbol.graphics.iter().enumerate() {
            let owner = index.map_or(Owner::Graphic(i), Owner::Instance);
            self.graphic(g, owner, inst, def);
        }
        // Drawings for the state the part is in: a switch's lever, open or closed.
        let param = |name: &str| {
            inst.params.get(name).cloned().or_else(|| def.cell.param(name)?.default.clone())
        };
        for state in symbol.states.iter().filter(|s| s.holds(&param)) {
            let owner = index.map_or(Owner::Derived, Owner::Instance);
            for g in &state.graphics {
                self.graphic(g, owner, inst, def);
            }
        }
        let t = &inst.placement;
        let map = |p: Point| Pt::from(t.apply(p));
        // Pins named on the drawing are numbered too, when the part has pads to number.
        let ports: Vec<&str> = def.cell.ports.iter().map(|p| p.name.as_str()).collect();
        let number = |port: &str| def.footprint.as_ref()?.pin_number(&ports, port, &param);
        for (i, pin) in symbol.pins.iter().enumerate() {
            let owner = index.map_or(Owner::Pin(i), Owner::Instance);
            let (dx, dy) = match pin.direction {
                PinDirection::Left => (-1, 0),
                PinDirection::Right => (1, 0),
                PinDirection::Up => (0, -1),
                PinDirection::Down => (0, 1),
            };
            let len = pin.length.nm();
            let inner = Point::new(pin.at.x + Length(dx * len), pin.at.y + Length(dy * len));
            self.push(
                Role::Pin,
                owner,
                Shape::Polyline {
                    points: vec![map(pin.at), map(inner)],
                    closed: false,
                    filled: false,
                },
            );
            if index.is_none() {
                self.push(
                    Role::Pin,
                    owner,
                    Shape::Circle { center: map(pin.at), radius: 0.254, filled: false },
                );
            }
            if let Some(n) = number(&pin.port).filter(|_| pin.show_name) {
                let mid = map(Point::new(
                    Length((pin.at.x.nm() + inner.x.nm()) / 2),
                    Length((pin.at.y.nm() + inner.y.nm()) / 2),
                ));
                let vertical = transform_dir(t, dx, dy).0 == 0;
                let off = 0.254;
                self.push(
                    Role::PinNumber,
                    owner,
                    Shape::Text {
                        at: if vertical {
                            Pt::new(mid.x - off, mid.y)
                        } else {
                            Pt::new(mid.x, mid.y - off)
                        },
                        text: n,
                        size: 0.889,
                        halign: HAlign::Center,
                        valign: VAlign::Bottom,
                        vertical,
                    },
                );
            }
            if pin.show_name {
                let gap = Length::from_mils(20).nm();
                let at = Point::new(inner.x + Length(dx * gap), inner.y + Length(dy * gap));
                let world_dir = transform_dir(t, dx, dy);
                let halign = if world_dir.0 < 0 { HAlign::Right } else { HAlign::Left };
                self.push(
                    Role::PinName,
                    owner,
                    Shape::Text {
                        at: map(at),
                        text: pin.port.clone(),
                        size: 1.016,
                        halign: if world_dir.0 == 0 { HAlign::Center } else { halign },
                        // Names of vertical pins sit just inside the body edge.
                        valign: match world_dir.1.signum() {
                            1 => VAlign::Top,
                            -1 => VAlign::Bottom,
                            _ => VAlign::Center,
                        },
                        vertical: false,
                    },
                );
            }
        }
    }

    /// A placeholder for instances whose cell or symbol can't be found.
    fn missing_symbol(&mut self, index: usize, inst: &Instance) {
        let c = Pt::from(inst.placement.position);
        let h = 2.54;
        let points = vec![
            Pt::new(c.x - h, c.y - h),
            Pt::new(c.x + h, c.y - h),
            Pt::new(c.x + h, c.y + h),
            Pt::new(c.x - h, c.y + h),
        ];
        let owner = Owner::Instance(index);
        self.push(
            Role::Unconnected,
            owner,
            Shape::Polyline { points, closed: true, filled: false },
        );
        self.push(
            Role::Unconnected,
            owner,
            Shape::Text {
                at: c,
                text: format!("{}?", inst.name),
                size: 1.27,
                halign: HAlign::Center,
                valign: VAlign::Center,
                vertical: false,
            },
        );
    }

    fn port_flag(&mut self, index: usize, at: Pt, dir: Pt, name: &str, kind: PortKind) {
        self.flag(Owner::Port(index), Role::Port, at, dir, name, kind);
    }

    /// A named flag pointing along `dir` from `at`, shaped by the kind of pin it is.
    fn flag(&mut self, owner: Owner, role: Role, at: Pt, dir: Pt, name: &str, kind: PortKind) {
        let size = 1.27;
        let h = size * 1.6;
        let text_w = name.chars().count() as f64 * size * CHAR_WIDTH;
        let len = text_w + h * 1.5;
        // Local coordinates: u runs from the connection point along `dir`, v across it.
        let local: Vec<(f64, f64)> = match kind {
            PortKind::Input => vec![
                (0.0, 0.0),
                (h / 2.0, -h / 2.0),
                (len, -h / 2.0),
                (len, h / 2.0),
                (h / 2.0, h / 2.0),
            ],
            PortKind::Output => vec![
                (0.0, -h / 2.0),
                (len - h / 2.0, -h / 2.0),
                (len, 0.0),
                (len - h / 2.0, h / 2.0),
                (0.0, h / 2.0),
            ],
            PortKind::Bidirectional => vec![
                (0.0, 0.0),
                (h / 2.0, -h / 2.0),
                (len - h / 2.0, -h / 2.0),
                (len, 0.0),
                (len - h / 2.0, h / 2.0),
                (h / 2.0, h / 2.0),
            ],
            PortKind::Passive | PortKind::PowerIn | PortKind::PowerOut => {
                vec![(0.0, -h / 2.0), (len, -h / 2.0), (len, h / 2.0), (0.0, h / 2.0)]
            }
        };
        let to_world = |(u, v): (f64, f64)| {
            Pt::new(at.x + u * dir.x - v * dir.y, at.y + u * dir.y + v * dir.x)
        };
        self.push(
            role,
            owner,
            Shape::Polyline {
                points: local.into_iter().map(to_world).collect(),
                closed: true,
                filled: false,
            },
        );
        self.push(
            role,
            owner,
            Shape::Text {
                at: to_world((len / 2.0, 0.0)),
                text: name.to_owned(),
                size,
                halign: HAlign::Center,
                valign: VAlign::Center,
                vertical: dir.y.abs() > dir.x.abs(),
            },
        );
    }
}

impl Builder {
    /// A slash across a straight run `seg` of a bus wire, with the number of bits.
    fn bus_width(&mut self, wire: usize, seg: [Pt; 2], width: u32) {
        let mid = Pt::new((seg[0].x + seg[1].x) / 2.0, (seg[0].y + seg[1].y) / 2.0);
        let h = 0.635;
        let horizontal = (seg[1].x - seg[0].x).abs() >= (seg[1].y - seg[0].y).abs();
        self.push(
            Role::Bus,
            Owner::Wire(wire),
            Shape::Polyline {
                points: vec![Pt::new(mid.x - h, mid.y + h), Pt::new(mid.x + h, mid.y - h)],
                closed: false,
                filled: false,
            },
        );
        let (at, halign, valign) = if horizontal {
            (Pt::new(mid.x, mid.y - h - 0.254), HAlign::Center, VAlign::Bottom)
        } else {
            (Pt::new(mid.x + h + 0.254, mid.y), HAlign::Left, VAlign::Center)
        };
        self.push(
            Role::Bus,
            Owner::Wire(wire),
            Shape::Text {
                at,
                text: width.to_string(),
                size: 1.016,
                halign,
                valign,
                vertical: false,
            },
        );
    }

    fn note(&mut self, index: usize, note: &Note) {
        let owner = Owner::Note(index);
        match note {
            Note::Box { a, b, title } => {
                let (a, b) = (Pt::from(*a), Pt::from(*b));
                let (min, max) =
                    (Pt::new(a.x.min(b.x), a.y.min(b.y)), Pt::new(a.x.max(b.x), a.y.max(b.y)));
                let points = vec![min, Pt::new(max.x, min.y), max, Pt::new(min.x, max.y)];
                self.push(
                    Role::Note,
                    owner,
                    Shape::Polyline { points, closed: true, filled: false },
                );
                if !title.is_empty() {
                    self.push(
                        Role::Note,
                        owner,
                        Shape::Text {
                            at: Pt::new(min.x + 1.27, min.y + 1.27),
                            text: title.clone(),
                            size: 1.524,
                            halign: HAlign::Left,
                            valign: VAlign::Top,
                            vertical: false,
                        },
                    );
                }
            }
            Note::Text { at, text } => {
                let at = Pt::from(*at);
                for (k, line) in text.lines().enumerate() {
                    self.push(
                        Role::Note,
                        owner,
                        Shape::Text {
                            at: Pt::new(at.x, at.y + k as f64 * NOTE_TEXT * 1.5),
                            text: line.to_owned(),
                            size: NOTE_TEXT,
                            halign: HAlign::Left,
                            valign: VAlign::Top,
                            vertical: false,
                        },
                    );
                }
            }
        }
    }

    fn frame_line(&mut self, points: Vec<Pt>, closed: bool) {
        self.push(Role::Frame, Owner::Derived, Shape::Polyline { points, closed, filled: false });
    }

    fn frame_text(&mut self, at: Pt, text: &str, size: f64, halign: HAlign) {
        self.push(
            Role::Frame,
            Owner::Derived,
            Shape::Text {
                at,
                text: text.to_owned(),
                size,
                halign,
                valign: VAlign::Center,
                vertical: false,
            },
        );
    }

    /// The paper's border with grid references, and the title block, as KiCad prints them.
    fn frame(&mut self, frame: &Frame, id: &CellId) {
        let o = Pt::from(frame.origin);
        let (w, h) = frame.size();
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| {
            vec![Pt::new(x0, y0), Pt::new(x1, y0), Pt::new(x1, y1), Pt::new(x0, y1)]
        };
        let m = FRAME_MARGIN;
        let band = 2.0;
        let (ox0, oy0, ox1, oy1) = (o.x + m, o.y + m, o.x + w - m, o.y + h - m);
        let (ix0, iy0, ix1, iy1) = (ox0 + band, oy0 + band, ox1 - band, oy1 - band);
        self.frame_line(rect(ox0, oy0, ox1, oy1), true);
        self.frame_line(rect(ix0, iy0, ix1, iy1), true);
        // Grid references: numbered columns, lettered rows, about 50 mm apart.
        let cols = ((ix1 - ix0) / 50.0).round().max(1.0) as usize;
        let rows = ((iy1 - iy0) / 50.0).round().max(1.0) as usize;
        let (cw, rh) = ((ix1 - ix0) / cols as f64, (iy1 - iy0) / rows as f64);
        for k in 0..cols {
            let x = ix0 + k as f64 * cw;
            if k > 0 {
                self.frame_line(vec![Pt::new(x, oy0), Pt::new(x, iy0)], false);
                self.frame_line(vec![Pt::new(x, iy1), Pt::new(x, oy1)], false);
            }
            let label = (k + 1).to_string();
            let cx = x + cw / 2.0;
            self.frame_text(Pt::new(cx, oy0 + band / 2.0), &label, 1.27, HAlign::Center);
            self.frame_text(Pt::new(cx, iy1 + band / 2.0), &label, 1.27, HAlign::Center);
        }
        for k in 0..rows {
            let y = iy0 + k as f64 * rh;
            if k > 0 {
                self.frame_line(vec![Pt::new(ox0, y), Pt::new(ix0, y)], false);
                self.frame_line(vec![Pt::new(ix1, y), Pt::new(ox1, y)], false);
            }
            let label = ((b'A' + (k % 26) as u8) as char).to_string();
            let cy = y + rh / 2.0;
            self.frame_text(Pt::new(ox0 + band / 2.0, cy), &label, 1.27, HAlign::Center);
            self.frame_text(Pt::new(ix1 + band / 2.0, cy), &label, 1.27, HAlign::Center);
        }

        // The title block, in four rows: company and comment, title, size, date and
        // revision, and the sheet and the program.
        let (bw, bh) = TITLE_BLOCK;
        let (bx0, by0) = (ix1 - bw, iy1 - bh);
        self.frame_line(rect(bx0, by0, ix1, iy1), true);
        let row = bh / 4.0;
        for k in 1..4 {
            let y = by0 + k as f64 * row;
            self.frame_line(vec![Pt::new(bx0, y), Pt::new(ix1, y)], false);
        }
        let pad = 1.5;
        let mid = |k: usize| by0 + (k as f64 + 0.5) * row;
        let small = 1.27;
        if !frame.company.is_empty() {
            self.frame_text(Pt::new(bx0 + pad, mid(0)), &frame.company, 1.524, HAlign::Left);
        }
        if !frame.comment.is_empty() {
            self.frame_text(Pt::new(ix1 - pad, mid(0)), &frame.comment, small, HAlign::Right);
        }
        let title = if frame.title.is_empty() { &id.cell } else { &frame.title };
        self.frame_text(Pt::new(bx0 + pad, mid(1)), "Title:", small, HAlign::Left);
        self.frame_text(Pt::new(bx0 + pad + 10.0, mid(1)), title, 2.54, HAlign::Left);
        // Size, date and revision share the third row.
        let (c1, c2) = (bx0 + 28.0, ix1 - 26.0);
        for x in [c1, c2] {
            self.frame_line(
                vec![Pt::new(x, mid(2) - row / 2.0), Pt::new(x, mid(2) + row / 2.0)],
                false,
            );
        }
        let paper = format!("Size: {}", frame.paper.name());
        self.frame_text(Pt::new(bx0 + pad, mid(2)), &paper, small, HAlign::Left);
        self.frame_text(
            Pt::new(c1 + pad, mid(2)),
            &format!("Date: {}", frame.date),
            small,
            HAlign::Left,
        );
        self.frame_text(
            Pt::new(c2 + pad, mid(2)),
            &format!("Rev: {}", frame.revision),
            small,
            HAlign::Left,
        );
        self.frame_text(Pt::new(bx0 + pad, mid(3)), &format!("Sheet: {id}"), small, HAlign::Left);
        self.frame_text(Pt::new(ix1 - pad, mid(3)), "Circuiter", small, HAlign::Right);
    }
}

/// Size of note text, in millimetres (60 mil).
const NOTE_TEXT: f64 = 1.524;

/// The number of bits each net carries: the widest pin or port on it, at the widths
/// its instances' parameters give.
fn net_widths(
    design: &Design,
    library: &str,
    cell: &Cell,
    schematic: &Schematic,
    conn: &Connectivity,
) -> Vec<u32> {
    let mut widths = vec![1; conn.nets.len()];
    for (k, pin) in conn.pins.iter().enumerate() {
        let Some(net) = conn.net_of(Item::Pin(k)) else { continue };
        let inst = &schematic.instances[pin.instance];
        let Some((_, def)) = design.resolve(&inst.cell, library) else { continue };
        let Some(port) = def.cell.port(&pin.port) else { continue };
        widths[net] = widths[net].max(def.cell.port_width(port, &inst.params).unwrap_or(1));
    }
    for (k, p) in schematic.ports.iter().enumerate() {
        let Some(net) = conn.net_of(Item::Port(k)) else { continue };
        if let Some(port) = cell.port(&p.port) {
            widths[net] = widths[net].max(port.width);
        }
    }
    widths
}

fn transform_dir(t: &Transform, dx: i64, dy: i64) -> (i64, i64) {
    let rotated_only = Transform { position: Point::ORIGIN, ..*t };
    let p = rotated_only.apply(Point::new(Length(dx), Length(dy)));
    (p.x.nm(), p.y.nm())
}

/// Points along an arc, counter-clockwise on screen from `start` to `end` degrees.
fn arc_points(center: Point, radius: Length, start: f64, end: f64) -> Vec<Point> {
    let mut sweep = end - start;
    if sweep <= 0.0 {
        sweep += 360.0;
    }
    let steps = ((sweep / 10.0).ceil() as usize).max(2);
    (0..=steps)
        .map(|i| {
            let a = (start + sweep * i as f64 / steps as f64).to_radians();
            // Counter-clockwise on screen means -Y for positive angles.
            Point::new(
                center.x + Length::from_mm(radius.mm() * a.cos()),
                center.y - Length::from_mm(radius.mm() * a.sin()),
            )
        })
        .collect()
}

/// Expands `${name}` and `${param.X}` in symbol text, formatting quantities with SI
/// prefixes and units (`4.7kΩ`).
pub fn substitute(text: &str, inst: &Instance, def: &CellDef) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let key = &after[..end];
        if key == "name" {
            out.push_str(&inst.name);
        } else if let Some(name) = key.strip_prefix("param.") {
            out.push_str(&param_display(inst, def, name));
        } else {
            out.push_str(&rest[start..start + 3 + end]);
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The display form of an instance's parameter value.
pub fn param_display(inst: &Instance, def: &CellDef, name: &str) -> String {
    let param = def.cell.param(name);
    let raw = inst
        .params
        .get(name)
        .map(String::as_str)
        .or_else(|| param.and_then(|p| p.default.as_deref()))
        .unwrap_or("?");
    match param.map(|p| &p.kind) {
        Some(ParamKind::Real { unit }) => {
            let unit_opt = (!unit.is_empty()).then_some(unit.as_str());
            match parse_quantity(raw, unit_opt) {
                Ok(v) => format_quantity(v, unit),
                Err(_) => raw.to_owned(),
            }
        }
        _ => raw.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn design() -> Design {
        let mut d = Design::new();
        d.add_library(circuiter_stdlib::library()).unwrap();
        d
    }

    fn instance(cell: &str, params: &[(&str, &str)]) -> Instance {
        Instance {
            fields: Default::default(),
            name: "R7".into(),
            cell: cell.parse().unwrap(),
            placement: Transform::default(),
            params: params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            view: None,
        }
    }

    #[test]
    fn svg_export_draws_symbols_and_text() {
        let d = design();
        let scene = Scene::symbol_preview(&d, &CellId::new("std", "nmos")).unwrap();
        let svg = to_svg(&scene, &Theme::PAPER);
        assert!(svg.starts_with("<svg") || svg.starts_with("<?xml"), "{svg}");
        assert!(svg.contains(">M?<"), "reference text is drawn");
        assert!(svg.contains(&Theme::PAPER.body.css()), "drawn in theme colours");
    }

    fn texts(scene: &Scene, role: Role) -> Vec<&str> {
        scene
            .elements
            .iter()
            .filter(|e| e.role == role)
            .filter_map(|e| match &e.shape {
                Shape::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn named_pins_of_parts_with_pads_are_numbered() {
        let d = design();
        let timer = Scene::symbol_preview(&d, &CellId::new("std", "ne555")).unwrap();
        let mut numbers = texts(&timer, Role::PinNumber);
        numbers.sort();
        assert_eq!(numbers, ["1", "2", "3", "4", "5", "6", "7", "8"]);
        // A resistor's leads aren't named, so they aren't numbered either.
        let r = Scene::symbol_preview(&d, &CellId::new("std", "resistor")).unwrap();
        assert!(texts(&r, Role::PinNumber).is_empty());
    }

    #[test]
    fn a_frame_borders_the_paper_with_its_title_block() {
        use circuiter_model::schematic::{Frame, Paper};
        let d = design();
        let mut frame = Frame::new(Paper::A4);
        frame.title = "Blinker".into();
        frame.revision = "B".into();
        frame.origin = Point::mm(-50.8, -25.4);
        let schematic = Schematic {
            instances: vec![instance("std:resistor", &[])],
            notes: vec![
                Note::Box {
                    a: Point::mm(-10.16, -10.16),
                    b: Point::mm(10.16, 10.16),
                    title: "Load".into(),
                },
                Note::Text { at: Point::mm(0.0, 20.32), text: "one\ntwo".into() },
            ],
            frame: Some(frame),
            ..Schematic::default()
        };
        let id = CellId::new("std", "top");
        let scene = Scene::build_from(&d, &id, &Cell::default(), &schematic);
        let framed = texts(&scene, Role::Frame);
        for want in ["Blinker", "Rev: B", "Size: A4", "Sheet: std:top", "1", "5", "A", "D"] {
            assert!(framed.contains(&want), "{want} in {framed:?}");
        }
        assert!(!framed.contains(&""), "no empty fields");
        // The border sits 10 mm inside the paper.
        assert!((scene.bounds.min.x - (-50.8 + 10.0)).abs() < 0.01, "{:?}", scene.bounds);
        assert!((scene.bounds.max.y - (-25.4 + 210.0 - 10.0)).abs() < 0.01, "{:?}", scene.bounds);
        assert_eq!(texts(&scene, Role::Note), ["Load", "one", "two"]);
        // Notes are picked by their outline, and drawn dashed.
        assert_eq!(scene.hit_test(Pt::new(-10.16, 5.0), 0.5), Some(Owner::Note(0)));
        let svg = to_svg(&scene, &Theme::PAPER);
        assert!(svg.contains("stroke-dasharray"));
        assert!(svg.contains(&Theme::PAPER.frame.css()));
    }

    #[test]
    fn symbol_text_shows_formatted_values() {
        let d = design();
        let def = d.cell(&CellId::new("std", "resistor")).unwrap();
        let inst = instance("std:resistor", &[("R", "4k7")]);
        assert_eq!(substitute("${name} = ${param.R}", &inst, def), "R7 = 4.7kΩ");
        let inst = instance("std:resistor", &[]);
        assert_eq!(substitute("${param.R}", &inst, def), "1kΩ", "falls back to the default");
    }

    #[test]
    fn arcs_sweep_counter_clockwise_on_screen() {
        let pts = arc_points(Point::ORIGIN, Length::from_mm(1.0), 0.0, 90.0);
        let last = pts.last().unwrap();
        assert_eq!((last.x.mm().round(), last.y.mm().round()), (0.0, -1.0), "90° is up");
    }

    #[test]
    fn hit_test_prefers_wires_and_finds_instances() {
        let d = design();
        let mut schematic = Schematic::default();
        schematic.instances.push(instance("std:resistor", &[]));
        schematic.wires.push(circuiter_model::schematic::Wire {
            points: vec![Point::mm(5.08, 0.0), Point::mm(10.0, 0.0)],
        });
        let id = CellId::new("std", "resistor");
        let cell = Cell::default();
        let scene = Scene::build_from(&d, &id, &cell, &schematic);
        assert_eq!(scene.hit_test(Pt::new(0.0, 0.5), 0.3), Some(Owner::Instance(0)));
        assert_eq!(scene.hit_test(Pt::new(8.0, 0.1), 0.3), Some(Owner::Wire(0)));
        assert_eq!(scene.hit_test(Pt::new(50.0, 50.0), 0.3), None);
        let all = Rect { min: Pt::new(-20.0, -20.0), max: Pt::new(20.0, 20.0) };
        assert_eq!(scene.objects_in(&all), vec![Owner::Instance(0), Owner::Wire(0)]);
        let left = Rect { min: Pt::new(-20.0, -20.0), max: Pt::new(6.0, 20.0) };
        assert_eq!(scene.objects_in(&left), vec![Owner::Instance(0)], "wire only partly inside");
    }
}
