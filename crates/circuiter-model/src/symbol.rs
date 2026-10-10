//! Symbol view: the graphic drawn on a schematic and the pins that wires attach to.

use crate::geom::{Length, Point};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Symbol {
    /// Made by the component builder from the cell's ports, and regenerated when they
    /// change. Editing the drawing by hand clears it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub generated: bool,
    #[serde(default)]
    pub graphics: Vec<Graphic>,
    #[serde(default)]
    pub pins: Vec<SymbolPin>,
    /// Drawings shown only in some states: a switch's lever, open or closed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub states: Vec<SymbolState>,
}

/// Graphics drawn when a parameter has a value: `when: "closed=true"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolState {
    pub when: String,
    pub graphics: Vec<Graphic>,
}

impl SymbolState {
    /// Whether the state holds, given how to look parameters up.
    pub fn holds(&self, param: &dyn Fn(&str) -> Option<String>) -> bool {
        let Some((name, value)) = self.when.split_once('=') else { return false };
        param(name.trim()).is_some_and(|v| v.trim().eq_ignore_ascii_case(value.trim()))
    }
}

impl Symbol {
    pub fn pin(&self, port: &str) -> Option<&SymbolPin> {
        self.pins.iter().find(|p| p.port == port)
    }

    /// The corners of the box around the drawing and the pins, ignoring text, whose size
    /// depends on how it is rendered. `None` for an empty symbol.
    pub fn bounds(&self) -> Option<(Point, Point)> {
        let mut points = Vec::new();
        for g in &self.graphics {
            match g {
                Graphic::Polyline { points: p, .. } => points.extend(p),
                Graphic::Rect { a, b, .. } => points.extend([*a, *b]),
                Graphic::Circle { center, radius, .. } | Graphic::Arc { center, radius, .. } => {
                    let r = Point::new(*radius, *radius);
                    points.extend([*center - r, *center + r]);
                }
                Graphic::Text { at, .. } => points.push(*at),
            }
        }
        points.extend(self.pins.iter().map(|p| p.at));
        let first = *points.first()?;
        Some(points.iter().fold((first, first), |(min, max), p| {
            (Point::new(min.x.min(p.x), min.y.min(p.y)), Point::new(max.x.max(p.x), max.y.max(p.y)))
        }))
    }
}

/// Drawing primitives in symbol-local coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Graphic {
    Polyline {
        points: Vec<Point>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fill: bool,
    },
    Rect {
        a: Point,
        b: Point,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fill: bool,
    },
    Circle {
        center: Point,
        radius: Length,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fill: bool,
    },
    /// Arc swept counter-clockwise (on screen) from `start` to `end`, in degrees,
    /// where 0° points along +X.
    Arc { center: Point, radius: Length, start: f64, end: f64 },
    /// Text. May contain `${name}` (the instance name) and `${param.X}` (a parameter value).
    Text {
        at: Point,
        text: String,
        #[serde(default = "default_text_size", skip_serializing_if = "is_default_text_size")]
        size: Length,
        #[serde(default, skip_serializing_if = "is_default_anchor")]
        anchor: Anchor,
    },
}

fn default_text_size() -> Length {
    Length::from_mils(50)
}

fn is_default_text_size(size: &Length) -> bool {
    *size == default_text_size()
}

fn is_default_anchor(anchor: &Anchor) -> bool {
    *anchor == Anchor::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Anchor {
    #[default]
    Left,
    Center,
    Right,
}

/// A pin on a symbol. `at` is the connection point (the outer end of the pin, where
/// wires attach); the pin line extends `length` from there toward the body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SymbolPin {
    pub port: String,
    pub at: Point,
    pub direction: PinDirection,
    #[serde(default = "default_pin_length", skip_serializing_if = "is_default_pin_length")]
    pub length: Length,
    /// Whether to print the port name beside the pin. Off for simple passives.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub show_name: bool,
}

fn default_pin_length() -> Length {
    Length::from_mils(100)
}

fn is_default_pin_length(length: &Length) -> bool {
    *length == default_pin_length()
}

/// The direction from a pin's connection point toward the symbol body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PinDirection {
    Left,
    Right,
    Up,
    Down,
}
