//! Schematic view: instances of other cells connected by wires, net labels and the
//! cell's own ports. A schematic defines a cell's behaviour hierarchically.

use crate::cell::{CellRef, ViewKind};
use crate::geom::{Point, Transform};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Schematic {
    #[serde(default)]
    pub instances: Vec<Instance>,
    #[serde(default)]
    pub wires: Vec<Wire>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<NetLabel>,
    /// Where each of the cell's ports attaches inside this schematic.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<SchematicPort>,
    /// No-connect flags: pins here are meant to be left unconnected.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub no_connects: Vec<Point>,
    /// Boxes and text that explain the drawing: a dashed box around the power supply,
    /// a note on a part's value. They don't connect to anything.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// The border and title block it is printed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<Frame>,
}

impl Schematic {
    pub fn instance(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name == name)
    }
}

/// A placed component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    /// Reference designator, unique within the schematic: `R1`, `M3`, `U12`.
    pub name: String,
    pub cell: CellRef,
    pub placement: Transform,
    /// Parameter overrides. Parameters not listed take the cell's defaults.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// Forces a particular view for this instance (e.g. simulate one gate at transistor
    /// level while the rest use fast models). `None` uses the design's view order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewKind>,
    /// Fields for the bill of materials and the board house, as KiCad has them:
    /// `MPN`, `Manufacturer`, `Datasheet`, and `DNP` (do not populate) when set.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

impl Instance {
    /// Whether the part is left off the assembled board (its `DNP` field is set).
    pub fn dnp(&self) -> bool {
        self.fields
            .get("DNP")
            .is_some_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false"))
    }
}

/// A wire drawn as a polyline. Connects to pins, ports, labels and other wires at its
/// vertices, and to other wires' end points that touch its segments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wire {
    pub points: Vec<Point>,
}

/// Names the net at a point. Labels with the same name in one schematic are connected;
/// a global label joins its net everywhere in the design, across the hierarchy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetLabel {
    pub net: String,
    pub at: Point,
    #[serde(default, skip_serializing_if = "is_false")]
    pub global: bool,
}

fn is_false(v: &bool) -> bool {
    !*v
}

/// The attachment point of one of the cell's ports inside its own schematic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchematicPort {
    pub port: String,
    pub at: Point,
}

/// A note on a sheet. Notes are only drawings: they never join nets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Note {
    /// A dashed box around a group of parts, titled at its top-left corner.
    Box {
        a: Point,
        b: Point,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        title: String,
    },
    /// Free text, read from its top-left corner. Lines break at `\n`.
    Text { at: Point, text: String },
}

impl Note {
    /// Moves the note by `(dx, dy)`.
    pub fn translate(&mut self, d: Point) {
        match self {
            Note::Box { a, b, .. } => {
                *a = *a + d;
                *b = *b + d;
            }
            Note::Text { at, .. } => *at = *at + d,
        }
    }
}

/// A drawing frame: the paper's border with grid references along its edges and the
/// title block in its bottom-right corner, as on a printed schematic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub paper: Paper,
    #[serde(default, skip_serializing_if = "is_false")]
    pub portrait: bool,
    /// The paper's top-left corner on the sheet.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub origin: Point,
    /// The title; empty shows the cell's name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub revision: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub date: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub company: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub comment: String,
}

fn is_origin(p: &Point) -> bool {
    *p == Point::ORIGIN
}

impl Frame {
    pub fn new(paper: Paper) -> Frame {
        Frame {
            paper,
            portrait: false,
            origin: Point::ORIGIN,
            title: String::new(),
            revision: String::new(),
            date: String::new(),
            company: String::new(),
            comment: String::new(),
        }
    }

    /// The paper's width and height in millimetres, as it is turned.
    pub fn size(&self) -> (f64, f64) {
        let (w, h) = self.paper.landscape();
        if self.portrait { (h, w) } else { (w, h) }
    }
}

/// Paper sizes, as KiCad offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Paper {
    A4,
    A3,
    A2,
    A1,
    A0,
    /// US Letter, 8.5 × 11 in.
    Letter,
    /// US Tabloid (ANSI B), 11 × 17 in.
    Tabloid,
}

impl Paper {
    pub const ALL: [Paper; 7] =
        [Paper::A4, Paper::A3, Paper::A2, Paper::A1, Paper::A0, Paper::Letter, Paper::Tabloid];

    /// Width and height in millimetres, lying on its long side.
    pub fn landscape(self) -> (f64, f64) {
        match self {
            Paper::A4 => (297.0, 210.0),
            Paper::A3 => (420.0, 297.0),
            Paper::A2 => (594.0, 420.0),
            Paper::A1 => (841.0, 594.0),
            Paper::A0 => (1189.0, 841.0),
            Paper::Letter => (279.4, 215.9),
            Paper::Tabloid => (431.8, 279.4),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Paper::A4 => "A4",
            Paper::A3 => "A3",
            Paper::A2 => "A2",
            Paper::A1 => "A1",
            Paper::A0 => "A0",
            Paper::Letter => "Letter",
            Paper::Tabloid => "Tabloid",
        }
    }

    /// The smallest paper, landscape or portrait, that holds a `w` × `h` mm drawing
    /// inside the border and clear of the title block; A0 if none does.
    pub fn fitting(w: f64, h: f64) -> (Paper, bool) {
        for paper in [Paper::A4, Paper::A3, Paper::A2, Paper::A1, Paper::A0] {
            let (pw, ph) = paper.landscape();
            let usable = |pw: f64, ph: f64| {
                let (iw, ih) = (pw - 2.0 * FRAME_MARGIN, ph - 2.0 * FRAME_MARGIN);
                // Beside the title block, or above it.
                (w <= iw && h <= ih - TITLE_BLOCK.1) || (w <= iw - TITLE_BLOCK.0 && h <= ih)
            };
            if usable(pw, ph) {
                return (paper, false);
            }
            if usable(ph, pw) {
                return (paper, true);
            }
        }
        (Paper::A0, false)
    }
}

/// From the paper's edge to the border, in millimetres.
pub const FRAME_MARGIN: f64 = 10.0;
/// The title block's width and height, in millimetres.
pub const TITLE_BLOCK: (f64, f64) = (110.0, 32.0);
