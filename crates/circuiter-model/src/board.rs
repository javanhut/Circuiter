//! Printed circuit boards: a part's footprint (its pads and markings) and a sheet's
//! board (outline, placed footprints, tracks, vias, copper zones, design rules).
//!
//! A footprint is either made by one of Circuiter's package generators from parameters
//! (`axial` with a pitch, `dip` with a pin count…), matching the part's 3D body, or
//! listed pad by pad. Its `pins` map the cell's ports to pad numbers (an op amp's `out`
//! to pad 1); without one, ports are numbered 1, 2, 3… in order.
//!
//! A board belongs to a cell with a schematic. Footprints on it are keyed by the
//! instance path of their part (`R1`, `X1.R2`); nets are the schematic's.

use crate::geom::{Length, Point};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Board layers, named as KiCad names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Layer {
    /// Front copper.
    FCu,
    /// Inner copper layer 1, 2…
    In(u8),
    /// Back copper.
    BCu,
    FSilk,
    BSilk,
    FMask,
    BMask,
    FPaste,
    BPaste,
    /// Courtyards: the space a part needs, for overlap checks.
    FCrtYd,
    BCrtYd,
    /// Fabrication drawings: the part's outline and value.
    FFab,
    BFab,
    /// The board's edge.
    EdgeCuts,
}

impl Layer {
    pub fn name(self) -> String {
        match self {
            Layer::FCu => "F.Cu".into(),
            Layer::In(k) => format!("In{k}.Cu"),
            Layer::BCu => "B.Cu".into(),
            Layer::FSilk => "F.Silkscreen".into(),
            Layer::BSilk => "B.Silkscreen".into(),
            Layer::FMask => "F.Mask".into(),
            Layer::BMask => "B.Mask".into(),
            Layer::FPaste => "F.Paste".into(),
            Layer::BPaste => "B.Paste".into(),
            Layer::FCrtYd => "F.Courtyard".into(),
            Layer::BCrtYd => "B.Courtyard".into(),
            Layer::FFab => "F.Fab".into(),
            Layer::BFab => "B.Fab".into(),
            Layer::EdgeCuts => "Edge.Cuts".into(),
        }
    }

    pub fn is_copper(self) -> bool {
        matches!(self, Layer::FCu | Layer::BCu | Layer::In(_))
    }

    /// The same layer on the other side of the board (for parts on the back).
    pub fn flipped(self) -> Layer {
        match self {
            Layer::FCu => Layer::BCu,
            Layer::BCu => Layer::FCu,
            Layer::FSilk => Layer::BSilk,
            Layer::BSilk => Layer::FSilk,
            Layer::FMask => Layer::BMask,
            Layer::BMask => Layer::FMask,
            Layer::FPaste => Layer::BPaste,
            Layer::BPaste => Layer::FPaste,
            Layer::FCrtYd => Layer::BCrtYd,
            Layer::BCrtYd => Layer::FCrtYd,
            Layer::FFab => Layer::BFab,
            Layer::BFab => Layer::FFab,
            other => other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PadShape {
    #[default]
    Circle,
    Rect,
    /// A stadium: a rectangle with fully rounded ends.
    Oval,
    /// A rectangle with rounded corners, a quarter of its smaller side.
    RoundRect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PadKind {
    /// Plated through the board, on every copper layer.
    Through { drill: Length },
    /// Surface mount, on the footprint's side only.
    Smd,
    /// A non-plated hole (a mounting hole), with no copper.
    Hole { drill: Length },
}

/// A pad, in the footprint's frame (millimetres, Y down as on the sheet).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pad {
    /// Matched to a port through the footprint's `pins` (`1`, `2`, `A1`…).
    pub number: String,
    pub at: Point,
    pub size: (Length, Length),
    #[serde(default)]
    pub shape: PadShape,
    pub kind: PadKind,
    /// Degrees counter-clockwise.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
}

/// A drawing on a footprint or a board: silkscreen, courtyard, fab outline, or the edge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Graphic {
    Line {
        layer: Layer,
        a: Point,
        b: Point,
        width: Length,
    },
    Circle {
        layer: Layer,
        center: Point,
        radius: Length,
        width: Length,
    },
    /// A closed outline; `filled` for solid shapes.
    Poly {
        layer: Layer,
        points: Vec<Point>,
        width: Length,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        filled: bool,
    },
    Text {
        layer: Layer,
        at: Point,
        text: String,
        size: Length,
    },
}

/// A part's footprint.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FootprintView {
    /// A package generator (`axial`, `dip`, `to220`…), or empty for `pads` as listed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub package: String,
    /// The generator's parameters; values may use `${param.NAME}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pads: Vec<Pad>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphics: Vec<Graphic>,
    /// Cell port → pad number; ports not listed are numbered in order from 1.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pins: BTreeMap<String, String>,
    /// Pin maps for particular packages, where `package` is chosen by a parameter
    /// (`${param.package}`) and they number their pins differently: a BJT's SOT-23 isn't
    /// its TO-92.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub package_pins: BTreeMap<String, BTreeMap<String, String>>,
}

/// A footprint placed on a board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    /// The part's instance path in the schematic: `R1`, `X1.R2`.
    pub path: String,
    pub at: Point,
    /// Degrees counter-clockwise.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// On the back of the board (mirrored).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub back: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
}

/// A straight piece of track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub layer: Layer,
    pub a: Point,
    pub b: Point,
    pub width: Length,
    pub net: String,
}

/// A plated hole joining copper layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Via {
    pub at: Point,
    pub diameter: Length,
    pub drill: Length,
    pub net: String,
}

/// How a zone joins pads of its own net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PadConnection {
    /// Through spokes, so the pad can be soldered without the pour soaking up heat.
    #[default]
    Thermal,
    Solid,
    None,
}

/// A copper pour: an outline filled with copper of `net`, kept clear of other nets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Zone {
    pub layer: Layer,
    pub net: String,
    pub outline: Vec<Point>,
    /// Higher priority zones fill first; lower ones keep clear of them.
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub pads: PadConnection,
    /// Overrides the board's clearance for this pour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance: Option<Length>,
    #[serde(default = "Zone::default_min_width")]
    pub min_width: Length,
    #[serde(default = "Zone::default_thermal_gap")]
    pub thermal_gap: Length,
    #[serde(default = "Zone::default_spoke_width")]
    pub spoke_width: Length,
}

impl Zone {
    fn default_min_width() -> Length {
        Length::from_mm(0.25)
    }
    fn default_thermal_gap() -> Length {
        Length::from_mm(0.5)
    }
    fn default_spoke_width() -> Length {
        Length::from_mm(0.5)
    }

    pub fn new(layer: Layer, net: &str, outline: Vec<Point>) -> Zone {
        Zone {
            layer,
            net: net.to_owned(),
            outline,
            priority: 0,
            pads: PadConnection::Thermal,
            clearance: None,
            min_width: Zone::default_min_width(),
            thermal_gap: Zone::default_thermal_gap(),
            spoke_width: Zone::default_spoke_width(),
        }
    }
}

/// Track widths, via sizes and clearances for a group of nets (power nets wider…).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetClass {
    pub name: String,
    /// The nets in it, by name.
    #[serde(default)]
    pub nets: Vec<String>,
    pub clearance: Length,
    pub track_width: Length,
    pub via_diameter: Length,
    pub via_drill: Length,
    /// Routed length limits (Altium's Length rule): too short or too long is flagged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<Length>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<Length>,
    /// The impedance profile its tracks are sized by, if any (from the stack-up).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impedance: Option<String>,
}

impl NetClass {
    /// A class like the board's defaults, named `name`.
    pub fn new(name: &str, rules: &Rules) -> NetClass {
        NetClass {
            name: name.to_owned(),
            nets: Vec::new(),
            clearance: rules.clearance,
            track_width: rules.track_width,
            via_diameter: rules.via_diameter,
            via_drill: rules.via_drill,
            min_length: None,
            max_length: None,
            impedance: None,
        }
    }
}

/// Design rules: what the board house can make, and what the design asks for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    /// The Default net class.
    pub clearance: Length,
    pub track_width: Length,
    pub via_diameter: Length,
    pub via_drill: Length,
    /// Manufacturing minimums.
    pub min_track_width: Length,
    pub min_clearance: Length,
    pub min_drill: Length,
    /// Copper ring around a plated hole.
    pub min_annular_ring: Length,
    /// Copper to the board edge.
    pub edge_clearance: Length,
    /// Hole to hole, edge to edge.
    pub hole_to_hole: Length,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<NetClass>,
}

impl Default for Rules {
    fn default() -> Self {
        let mm = Length::from_mm;
        Rules {
            clearance: mm(0.2),
            track_width: mm(0.2),
            via_diameter: mm(0.6),
            via_drill: mm(0.3),
            min_track_width: mm(0.15),
            min_clearance: mm(0.15),
            min_drill: mm(0.3),
            min_annular_ring: mm(0.1),
            edge_clearance: mm(0.5),
            hole_to_hole: mm(0.25),
            classes: Vec::new(),
        }
    }
}

impl Rules {
    /// The class `net` is in, if any.
    pub fn class(&self, net: &str) -> Option<&NetClass> {
        self.classes.iter().find(|c| c.nets.iter().any(|n| n == net))
    }

    pub fn track_width_for(&self, net: &str) -> Length {
        self.class(net).map_or(self.track_width, |c| c.track_width)
    }

    pub fn clearance_for(&self, net: &str) -> Length {
        self.class(net).map_or(self.clearance, |c| c.clearance)
    }

    pub fn via_for(&self, net: &str) -> (Length, Length) {
        self.class(net)
            .map_or((self.via_diameter, self.via_drill), |c| (c.via_diameter, c.via_drill))
    }
}

/// A board: the physical form of a cell's schematic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Board {
    /// Copper layers: 2, 4, 6…
    pub copper_layers: u8,
    pub thickness: Length,
    /// The board's edge, a closed outline.
    pub outline: Vec<Point>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub footprints: Vec<Placed>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vias: Vec<Via>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<Zone>,
    /// Text and drawings on the board itself (not on a footprint).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphics: Vec<Graphic>,
    #[serde(default)]
    pub rules: Rules,
    /// The product's case, if it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enclosure: Option<Enclosure>,
    /// Colours given to nets, to show on their routing.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub net_colours: BTreeMap<String, [u8; 3]>,
    /// The layer stack, top to bottom, and its impedance profiles; a standard FR-4
    /// stack for the layer count and thickness when not given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stackup: Option<Stackup>,
    /// Teardrops where tracks meet pads and vias, when wanted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teardrops: Option<Teardrops>,
    /// Areas kept clear of tracks, vias, pours or parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keepouts: Vec<Keepout>,
    /// Differential pairs: two nets routed side by side.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<DiffPair>,
    /// How copies of the board are made up into a panel for manufacture, if they are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<Panel>,
    /// Assembly variants: the same board built differently.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
    /// The variant being worked on and made, by name; the design as drawn when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}

/// An assembly variant (Altium's): the same bare board built with some parts left off
/// and some given other values: a cheaper model, a region's version, a test build.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Variant {
    pub name: String,
    /// How parts differ from the design, by instance path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parts: BTreeMap<String, VariantPart>,
}

/// How a part differs in a variant.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct VariantPart {
    /// Left off the board.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub not_fitted: bool,
    /// Another value for the bill of materials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Other fields (`MPN`, `Manufacturer`…).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

/// A panel (an array, in Altium's words) of copies of the board, made and assembled
/// together then broken apart: rows and columns of it, rails along the top and bottom
/// for the assembly line to hold (with fiducials for its camera and tooling holes),
/// and how the copies separate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    pub rows: u32,
    pub columns: u32,
    /// The rails' width (none when 0).
    pub rails: Length,
    pub separation: Separation,
    /// Three fiducials on the rails.
    #[serde(default = "yes")]
    pub fiducials: bool,
    /// A tooling hole in each corner of the rails.
    #[serde(default = "yes")]
    pub tooling: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Panel {
            rows: 2,
            columns: 2,
            rails: Length::from_mm(5.0),
            separation: Separation::default(),
            fiducials: true,
            tooling: true,
        }
    }
}

/// How the copies on a panel come apart.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Separation {
    /// Butted together, with V-grooves cut a third deep from each side to snap along:
    /// for rectangular boards.
    VScore,
    /// Routed round with a cutter `gap` wide, held by tabs `tab` wide drilled with a
    /// row of small holes to snap at: any shape of board.
    MouseBites { gap: Length, tab: Length },
}

impl Default for Separation {
    fn default() -> Self {
        Separation::MouseBites { gap: Length::from_mm(2.0), tab: Length::from_mm(3.0) }
    }
}

/// A differential pair (Altium's and KiCad's): two nets carrying one signal as a
/// difference, routed side by side, `width` wide and `gap` apart, kept to within
/// `max_skew` of the same length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffPair {
    pub name: String,
    pub positive: String,
    pub negative: String,
    pub width: Length,
    pub gap: Length,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_skew: Option<Length>,
}

impl DiffPair {
    /// Pairs to suggest from net names: `X_P`/`X_N`, `X+`/`X-`, `XP`/`XN`.
    pub fn from_names<'a>(nets: impl IntoIterator<Item = &'a String>) -> Vec<DiffPair> {
        let nets: Vec<&String> = nets.into_iter().collect();
        let mut out = Vec::new();
        for n in &nets {
            for (pos, neg) in [("_P", "_N"), ("+", "-"), ("P", "N"), ("_p", "_n")] {
                let Some(base) = n.strip_suffix(pos) else { continue };
                if base.is_empty() {
                    continue;
                }
                let other = format!("{base}{neg}");
                if nets.iter().any(|m| **m == other) {
                    out.push(DiffPair {
                        name: base.trim_end_matches('_').to_owned(),
                        positive: (*n).clone(),
                        negative: other,
                        width: Length::from_mm(0.2),
                        gap: Length::from_mm(0.2),
                        max_skew: Some(Length::from_mm(0.5)),
                    });
                    break;
                }
            }
        }
        out
    }
}

/// An area kept clear (Altium's keepouts, KiCad's rule areas): of tracks, vias,
/// copper pours and parts, on the copper layers listed (every one when none are).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keepout {
    pub outline: Vec<Point>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<Layer>,
    #[serde(default = "yes")]
    pub tracks: bool,
    #[serde(default = "yes")]
    pub vias: bool,
    #[serde(default = "yes")]
    pub pours: bool,
    #[serde(default)]
    pub parts: bool,
}

impl Keepout {
    pub fn new(outline: Vec<Point>) -> Keepout {
        Keepout { outline, layers: Vec::new(), tracks: true, vias: true, pours: true, parts: false }
    }

    /// Whether it applies on copper layer `l`.
    pub fn on(&self, l: Layer) -> bool {
        self.layers.is_empty() || self.layers.contains(&l)
    }
}

/// How big teardrops are, as fractions of the pad or via they widen onto: how far
/// they reach past its edge, and how wide they are where they meet it (Altium's
/// length and width percentages).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Teardrops {
    pub length: f64,
    pub width: f64,
}

impl Default for Teardrops {
    fn default() -> Self {
        Teardrops { length: 0.5, width: 0.7 }
    }
}

/// What a layer of the stack is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StackKind {
    SolderMask,
    Copper,
    /// A cured laminate.
    Core,
    /// The bonding layers pressed between cores.
    Prepreg,
}

/// One layer of the stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackLayer {
    pub name: String,
    pub kind: StackKind,
    pub thickness: Length,
    /// Dielectric constant (εr), for dielectrics.
    #[serde(default)]
    pub er: f64,
}

/// A target impedance for tracks on a layer, as Altium's impedance profiles: single
/// tracks, or differential pairs `gap` apart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpedanceProfile {
    pub name: String,
    pub ohms: f64,
    pub layer: Layer,
    #[serde(default)]
    pub differential: bool,
    #[serde(default = "default_gap")]
    pub gap: Length,
}

fn default_gap() -> Length {
    Length::from_mm(0.2)
}

/// The board's layers through its thickness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Stackup {
    pub layers: Vec<StackLayer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<ImpedanceProfile>,
}

impl Stackup {
    /// A standard stack: 1 oz (35 µm) copper, FR-4 (εr 4.5) between, solder mask over
    /// both sides, cores and prepregs sharing `thickness` (mm) as boards are pressed.
    pub fn standard(copper_layers: u8, thickness: f64) -> Stackup {
        let mm = Length::from_mm;
        let cu = 0.035;
        let n = copper_layers.max(2) as usize;
        let mask = |name: &str| StackLayer {
            name: name.into(),
            kind: StackKind::SolderMask,
            thickness: mm(0.01),
            er: 3.8,
        };
        let dielectric_total = (thickness - n as f64 * cu - 0.02).max(0.1);
        let gaps = n - 1;
        // Thin prepregs under the outer layers, the rest in the cores.
        let prepreg = if gaps > 1 { (dielectric_total * 0.12).min(0.2) } else { 0.0 };
        let cores = gaps - if gaps > 1 { 2 } else { 0 };
        let core =
            (dielectric_total - prepreg * if gaps > 1 { 2.0 } else { 0.0 }) / cores.max(1) as f64;
        let mut layers = vec![mask("Top Solder Mask")];
        for k in 0..n {
            let name = match k {
                0 => "F.Cu".to_owned(),
                _ if k == n - 1 => "B.Cu".to_owned(),
                _ => format!("In{k}.Cu"),
            };
            layers.push(StackLayer { name, kind: StackKind::Copper, thickness: mm(cu), er: 0.0 });
            if k + 1 < n {
                let outer = gaps > 1 && (k == 0 || k + 2 == n);
                layers.push(if outer {
                    StackLayer {
                        name: "Prepreg".into(),
                        kind: StackKind::Prepreg,
                        thickness: mm(prepreg),
                        er: 4.2,
                    }
                } else {
                    StackLayer {
                        name: "Core".into(),
                        kind: StackKind::Core,
                        thickness: mm(core),
                        er: 4.5,
                    }
                });
            }
        }
        layers.push(mask("Bottom Solder Mask"));
        Stackup { layers, profiles: Vec::new() }
    }

    /// The whole stack's thickness, mm.
    pub fn thickness(&self) -> f64 {
        self.layers.iter().map(|l| l.thickness.mm()).sum()
    }
}

impl Board {
    /// The stack-up, as given or the standard one for this board.
    pub fn stack(&self) -> Stackup {
        self.stackup
            .clone()
            .unwrap_or_else(|| Stackup::standard(self.copper_layers, self.thickness.mm()))
    }
}

/// What a case is made of, for its strength and how it passes heat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CaseMaterial {
    /// Moulded or printed plastic (ABS): a poor conductor of heat.
    #[default]
    Plastic,
    /// Aluminium: spreads heat through its walls.
    Aluminium,
}

/// The case a board goes in: one made to fit it, a base and a lid with standoffs under
/// the board and openings for its connectors and controls, or a 3D part of your own
/// with the board set in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Enclosure {
    /// A 3D part to use as the case instead of one made to fit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<crate::cell::CellRef>,
    /// Its lid, another 3D part, if it has one apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lid: Option<crate::cell::CellRef>,
    /// For a part of your own: where the board's origin goes in the part's frame, mm.
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub board_at: [f64; 3],
    /// Wall thickness, mm.
    pub wall: f64,
    /// Room between the board's edge and the walls, mm.
    pub gap: f64,
    /// Standoff height under the board, mm; 0 to clear the parts' leads.
    #[serde(default)]
    pub below: f64,
    /// Headroom over the board, mm; 0 to clear the tallest part.
    #[serde(default)]
    pub above: f64,
    /// Rounding of the case's corners, mm.
    pub radius: f64,
    #[serde(default)]
    pub material: CaseMaterial,
    /// Slots in the lid to let heat out.
    #[serde(default)]
    pub vents: bool,
    /// Openings in the walls and lid for connectors, switches, buttons, pots and LEDs.
    #[serde(default = "yes")]
    pub openings: bool,
    /// Simulations run with the circuit in the case: its air warms with the parts.
    #[serde(default = "yes")]
    pub simulate: bool,
    /// The room's temperature, °C.
    #[serde(default = "room")]
    pub room: f64,
}

fn yes() -> bool {
    true
}

fn room() -> f64 {
    25.0
}

fn is_zero3(v: &[f64; 3]) -> bool {
    v.iter().all(|x| *x == 0.0)
}

impl Default for Enclosure {
    fn default() -> Self {
        Enclosure {
            part: None,
            lid: None,
            board_at: [0.0; 3],
            wall: 2.0,
            gap: 1.0,
            below: 0.0,
            above: 0.0,
            radius: 2.0,
            material: CaseMaterial::Plastic,
            vents: false,
            openings: true,
            simulate: true,
            room: 25.0,
        }
    }
}

impl Board {
    /// A two-layer board, `w` × `h` millimetres, its top-left corner at the origin.
    pub fn new(w: f64, h: f64) -> Board {
        let p = |x: f64, y: f64| Point::new(Length::from_mm(x), Length::from_mm(y));
        Board {
            copper_layers: 2,
            thickness: Length::from_mm(1.6),
            outline: vec![p(0.0, 0.0), p(w, 0.0), p(w, h), p(0.0, h)],
            footprints: Vec::new(),
            tracks: Vec::new(),
            vias: Vec::new(),
            zones: Vec::new(),
            graphics: Vec::new(),
            rules: Rules::default(),
            enclosure: None,
            net_colours: BTreeMap::new(),
            stackup: None,
            teardrops: None,
            keepouts: Vec::new(),
            pairs: Vec::new(),
            panel: None,
            variants: Vec::new(),
            variant: None,
        }
    }

    /// The copper layers, front first.
    pub fn copper(&self) -> Vec<Layer> {
        let inner = self.copper_layers.saturating_sub(2);
        std::iter::once(Layer::FCu)
            .chain((1..=inner).map(Layer::In))
            .chain(std::iter::once(Layer::BCu))
            .collect()
    }

    pub fn placed(&self, path: &str) -> Option<&Placed> {
        self.footprints.iter().find(|f| f.path == path)
    }

    /// The variant being worked on, if one is.
    pub fn active_variant(&self) -> Option<&Variant> {
        let name = self.variant.as_ref()?;
        self.variants.iter().find(|v| &v.name == name)
    }
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boards_round_trip_and_name_their_layers() {
        let mut b = Board::new(50.0, 30.0);
        b.footprints.push(Placed {
            path: "R1".into(),
            at: Point::mm(10.0, 10.0),
            rotation: 90.0,
            back: false,
            locked: false,
        });
        b.tracks.push(Track {
            layer: Layer::FCu,
            a: Point::mm(10.0, 10.0),
            b: Point::mm(20.0, 10.0),
            width: Length::from_mm(0.25),
            net: "out".into(),
        });
        b.zones.push(Zone::new(Layer::BCu, "GND", b.outline.clone()));
        let text = ron::to_string(&b).unwrap();
        assert_eq!(ron::from_str::<Board>(&text).unwrap(), b);
        assert_eq!(b.copper(), [Layer::FCu, Layer::BCu]);
        b.copper_layers = 4;
        assert_eq!(b.copper(), [Layer::FCu, Layer::In(1), Layer::In(2), Layer::BCu]);
        assert_eq!(Layer::In(2).name(), "In2.Cu");
        assert_eq!(Layer::FSilk.flipped(), Layer::BSilk);
        assert_eq!(b.rules.track_width_for("anything"), Length::from_mm(0.2));
    }
}
