//! A board resolved against its schematic: every placed part's footprint, every pad
//! where it sits on the board with its layers and net, and what the schematic and the
//! board disagree about (parts not placed yet, footprints whose part is gone).

use crate::footprints::{self, Footprint};
use circuiter_model::board::{Board, Graphic, Layer, Pad, PadKind, Placed};
use circuiter_model::geom::{Length, Point};
use circuiter_model::{CellId, Design};
use circuiter_netlist::Netlist;
use std::collections::{BTreeMap, BTreeSet};

/// A point on the board in millimetres, Y down as on the sheet.
pub type P = [f64; 2];

pub fn p(pt: Point) -> P {
    [pt.x.mm(), pt.y.mm()]
}

pub fn point(v: P) -> Point {
    Point::new(Length::from_mm(v[0]), Length::from_mm(v[1]))
}

/// Where a footprint's own coordinates go on the board: mirrored for the back, turned
/// counter-clockwise on screen (Y down), moved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub at: P,
    pub rotation: f64,
    pub back: bool,
}

impl Place {
    pub fn of(placed: &Placed) -> Place {
        Place { at: p(placed.at), rotation: placed.rotation, back: placed.back }
    }

    pub fn apply(&self, v: P) -> P {
        let x = if self.back { -v[0] } else { v[0] };
        let (s, c) = self.rotation.to_radians().sin_cos();
        // Counter-clockwise on a screen whose Y points down.
        [self.at[0] + x * c + v[1] * s, self.at[1] - x * s + v[1] * c]
    }

    /// A point on the board in the footprint's own frame: `apply` undone.
    pub fn unapply(&self, v: P) -> P {
        let (dx, dy) = (v[0] - self.at[0], v[1] - self.at[1]);
        let (s, c) = self.rotation.to_radians().sin_cos();
        let (x, y) = (dx * c - dy * s, dx * s + dy * c);
        [if self.back { -x } else { x }, y]
    }

    /// A rotation in the footprint's frame, as one on the board.
    pub fn angle(&self, a: f64) -> f64 {
        let a = if self.back { -a } else { a };
        (a + self.rotation).rem_euclid(360.0)
    }

    pub fn layer(&self, l: Layer) -> Layer {
        if self.back { l.flipped() } else { l }
    }
}

/// A part on the board.
#[derive(Debug, Clone)]
pub struct Part {
    pub path: String,
    pub cell: CellId,
    /// Its reference designator (the instance path, until annotation).
    pub reference: String,
    /// Its value for the BOM: the main parameter (`10k`, `1u`) or the cell's name.
    pub value: String,
    pub package: String,
    /// The instance's parameter values (missing ones take the cell's defaults).
    pub params: BTreeMap<String, String>,
    pub footprint: Footprint,
    /// Where it is, if placed.
    pub place: Option<Place>,
    /// Each port's net.
    pub nets: BTreeMap<String, String>,
    pub locked: bool,
    /// Its fields for the bill of materials (`MPN`, `Manufacturer`, `DNP`…).
    pub fields: BTreeMap<String, String>,
}

impl Part {
    /// Whether it's left off the assembled board (its `DNP` field is set).
    pub fn dnp(&self) -> bool {
        self.fields
            .get("DNP")
            .is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
    }
}

/// A pad where it is on the board.
#[derive(Debug, Clone, PartialEq)]
pub struct PadAt {
    pub part: usize,
    pub number: String,
    pub center: P,
    pub size: (f64, f64),
    pub shape: circuiter_model::board::PadShape,
    /// Degrees on the board.
    pub rotation: f64,
    pub kind: PadKind,
    /// Copper layers it is on.
    pub layers: Vec<Layer>,
    pub net: Option<String>,
}

impl PadAt {
    pub fn drill(&self) -> Option<f64> {
        match self.kind {
            PadKind::Through { drill } | PadKind::Hole { drill } => Some(drill.mm()),
            PadKind::Smd => None,
        }
    }

    pub fn plated(&self) -> bool {
        matches!(self.kind, PadKind::Through { .. })
    }
}

/// A board with everything on it resolved.
#[derive(Debug, Clone)]
pub struct Layout {
    pub board: Board,
    pub parts: Vec<Part>,
    pub pads: Vec<PadAt>,
    /// Every net with copper on the board.
    pub nets: BTreeSet<String>,
    /// Parts in the schematic not yet on the board.
    pub unplaced: Vec<String>,
    /// Footprints on the board whose part is no longer in the schematic.
    pub orphans: Vec<String>,
    /// Parts without footprints (sources, meters), so never on the board.
    pub off_board: Vec<String>,
    /// Parts whose footprint couldn't be made, and why.
    pub errors: Vec<String>,
}

/// The value a BOM shows for a part: its main parameter, or its cell's name.
pub fn value_of(def: &circuiter_model::CellDef, params: &BTreeMap<String, String>) -> String {
    let main = ["R", "C", "L", "V", "value", "vz", "f", "vout"];
    for k in main {
        if let Some(v) = params.get(k).cloned().or_else(|| def.cell.param(k)?.default.clone()) {
            return v;
        }
    }
    def.name.clone()
}

impl Layout {
    /// Resolves `board`, the board of cell `top`, against `top`'s schematic.
    pub fn new(design: &Design, top: &CellId, board: &Board) -> Result<Layout, String> {
        let netlist = Netlist::build_physical(design, top)?;
        let mut out = Layout {
            board: board.clone(),
            parts: Vec::new(),
            pads: Vec::new(),
            nets: BTreeSet::new(),
            unplaced: Vec::new(),
            orphans: Vec::new(),
            off_board: netlist.off_board.clone(),
            errors: Vec::new(),
        };
        let in_schematic: BTreeSet<&str> =
            netlist.physical.iter().map(|e| e.path.as_str()).collect();
        out.orphans = board
            .footprints
            .iter()
            .filter(|f| !in_schematic.contains(f.path.as_str()))
            .map(|f| f.path.clone())
            .collect();
        for e in &netlist.physical {
            let Some(def) = design.cell(&e.cell) else { continue };
            let Some(view) = &def.footprint else { continue };
            let param = |name: &str| {
                e.params.get(name).cloned().or_else(|| def.cell.param(name)?.default.clone())
            };
            let ports: Vec<String> = def.cell.ports.iter().map(|p| p.name.clone()).collect();
            let footprint = match footprints::resolve(view, &ports, &param) {
                Ok(f) => f,
                Err(err) => {
                    out.errors.push(format!("{}: {err}", e.path));
                    continue;
                }
            };
            let nets: BTreeMap<String, String> = e
                .ports
                .iter()
                .map(|(port, n)| (port.clone(), netlist.nets[*n].name.clone()))
                .collect();
            let placed = board.placed(&e.path);
            if placed.is_none() {
                out.unplaced.push(e.path.clone());
            }
            out.parts.push(Part {
                path: e.path.clone(),
                cell: e.cell.clone(),
                reference: e.path.clone(),
                value: value_of(def, &e.params),
                package: view.package.clone(),
                params: e.params.clone(),
                footprint,
                place: placed.map(Place::of),
                nets,
                locked: placed.is_some_and(|f| f.locked),
                fields: e.fields.clone(),
            });
        }
        let copper = board.copper();
        for (k, part) in out.parts.iter().enumerate() {
            let Some(place) = part.place else { continue };
            // Which port each pad belongs to.
            let mut net_of_pad: BTreeMap<&str, &str> = BTreeMap::new();
            for (port, numbers) in &part.footprint.pins {
                for n in numbers.split(',').map(str::trim) {
                    let Some(net) = part.nets.get(port) else { continue };
                    // Ports sharing a pad (a MOSFET's body and source) must share a net.
                    if let Some(other) = net_of_pad.insert(n, net).filter(|o| o != net) {
                        out.errors
                            .push(format!("{}: pad {n} joins nets {other} and {net}", part.path));
                    }
                }
            }
            for pad in &part.footprint.pads {
                out.pads.push(pad_at(
                    k,
                    pad,
                    &place,
                    &copper,
                    net_of_pad.get(pad.number.as_str()).map(|s| s.to_string()),
                ));
            }
        }
        for pad in &out.pads {
            if let Some(n) = &pad.net {
                out.nets.insert(n.clone());
            }
        }
        out.nets.extend(board.tracks.iter().map(|t| t.net.clone()));
        out.nets.extend(board.vias.iter().map(|v| v.net.clone()));
        Ok(out)
    }

    /// A part's drawings, placed on the board.
    pub fn graphics(&self, part: usize) -> Vec<Graphic> {
        let part = &self.parts[part];
        let Some(place) = part.place else { return Vec::new() };
        part.footprint
            .graphics
            .iter()
            .map(|g| {
                let pt = |v: Point| point(place.apply(p(v)));
                match g {
                    Graphic::Line { layer, a, b, width } => Graphic::Line {
                        layer: place.layer(*layer),
                        a: pt(*a),
                        b: pt(*b),
                        width: *width,
                    },
                    Graphic::Circle { layer, center, radius, width } => Graphic::Circle {
                        layer: place.layer(*layer),
                        center: pt(*center),
                        radius: *radius,
                        width: *width,
                    },
                    Graphic::Poly { layer, points, width, filled } => Graphic::Poly {
                        layer: place.layer(*layer),
                        points: points.iter().map(|q| pt(*q)).collect(),
                        width: *width,
                        filled: *filled,
                    },
                    Graphic::Text { layer, at, text, size } => Graphic::Text {
                        layer: place.layer(*layer),
                        at: pt(*at),
                        text: text
                            .replace("${REFERENCE}", &part.reference)
                            .replace("${VALUE}", &part.value),
                        size: *size,
                    },
                }
            })
            .collect()
    }

    /// The pads of `part`.
    pub fn pads_of(&self, part: usize) -> impl Iterator<Item = &PadAt> {
        self.pads.iter().filter(move |p| p.part == part)
    }

    pub fn part(&self, path: &str) -> Option<usize> {
        self.parts.iter().position(|p| p.path == path)
    }
}

fn pad_at(part: usize, pad: &Pad, place: &Place, copper: &[Layer], net: Option<String>) -> PadAt {
    let layers = match pad.kind {
        PadKind::Through { .. } => copper.to_vec(),
        PadKind::Smd => vec![place.layer(Layer::FCu)],
        PadKind::Hole { .. } => Vec::new(),
    };
    PadAt {
        part,
        number: pad.number.clone(),
        center: place.apply(p(pad.at)),
        size: (pad.size.0.mm(), pad.size.1.mm()),
        shape: pad.shape,
        rotation: place.angle(pad.rotation),
        kind: pad.kind,
        layers,
        net,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placements_turn_counter_clockwise_and_mirror_on_the_back() {
        let close = |a: P, b: P| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9;
        let place = Place { at: [10.0, 20.0], rotation: 90.0, back: false };
        // Right (+X) turns to up (−Y on screen).
        assert!(close(place.apply([1.0, 0.0]), [10.0, 19.0]));
        let back = Place { at: [0.0, 0.0], rotation: 0.0, back: true };
        assert!(close(back.apply([1.0, 2.0]), [-1.0, 2.0]));
        assert_eq!(back.layer(Layer::FSilk), Layer::BSilk);
        assert_eq!(back.angle(30.0), 330.0);
        let turned = Place { at: [3.0, -2.0], rotation: 30.0, back: true };
        assert!(close(turned.unapply(turned.apply([1.5, -0.5])), [1.5, -0.5]));
    }
}
