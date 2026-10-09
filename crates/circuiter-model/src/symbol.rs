//! Symbol view: the graphic drawn on a schematic and the pins that wires attach to.

use crate::geom::{Length, Point};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Symbol {
    #[serde(default)]
    pub graphics: Vec<Graphic>,
    #[serde(default)]
    pub pins: Vec<SymbolPin>,
}

impl Symbol {
    pub fn pin(&self, port: &str) -> Option<&SymbolPin> {
        self.pins.iter().find(|p| p.port == port)
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
