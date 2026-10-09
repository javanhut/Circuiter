//! Geometry primitives shared by every 2D view (symbols, schematics, and later PCB).
//!
//! Coordinates are stored as integer nanometres so that snapping, connectivity and
//! equality are exact. On disk they are written as millimetres (e.g. `2.54`) so
//! files stay readable and diff cleanly.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::ops::{Add, Neg, Sub};

/// A length in integer nanometres. Serialized as a decimal number of millimetres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Length(pub i64);

impl Length {
    pub const ZERO: Length = Length(0);
    pub const NM_PER_MM: i64 = 1_000_000;
    /// One mil (a thousandth of an inch). Schematic grids are traditionally 50 or 100 mil.
    pub const MIL: Length = Length(25_400);

    pub const fn from_nm(nm: i64) -> Self {
        Length(nm)
    }

    pub fn from_mm(mm: f64) -> Self {
        Length((mm * Self::NM_PER_MM as f64).round() as i64)
    }

    pub const fn from_mils(mils: i64) -> Self {
        Length(mils * Self::MIL.0)
    }

    pub const fn nm(self) -> i64 {
        self.0
    }

    pub fn mm(self) -> f64 {
        self.0 as f64 / Self::NM_PER_MM as f64
    }
}

impl Add for Length {
    type Output = Length;
    fn add(self, rhs: Length) -> Length {
        Length(self.0 + rhs.0)
    }
}

impl Sub for Length {
    type Output = Length;
    fn sub(self, rhs: Length) -> Length {
        Length(self.0 - rhs.0)
    }
}

impl Neg for Length {
    type Output = Length;
    fn neg(self) -> Length {
        Length(-self.0)
    }
}

impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}mm", self.mm())
    }
}

impl Serialize for Length {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64(self.mm())
    }
}

impl<'de> Deserialize<'de> for Length {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        f64::deserialize(d).map(Length::from_mm)
    }
}

/// A 2D point. Serialized as an `(x, y)` tuple in millimetres. +Y points down, as on screen.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(from = "(Length, Length)", into = "(Length, Length)")]
pub struct Point {
    pub x: Length,
    pub y: Length,
}

impl Point {
    pub const ORIGIN: Point = Point { x: Length::ZERO, y: Length::ZERO };

    pub const fn new(x: Length, y: Length) -> Self {
        Point { x, y }
    }

    pub fn mm(x: f64, y: f64) -> Self {
        Point::new(Length::from_mm(x), Length::from_mm(y))
    }

    pub const fn mils(x: i64, y: i64) -> Self {
        Point::new(Length::from_mils(x), Length::from_mils(y))
    }
}

impl From<(Length, Length)> for Point {
    fn from((x, y): (Length, Length)) -> Self {
        Point { x, y }
    }
}

impl From<Point> for (Length, Length) {
    fn from(p: Point) -> Self {
        (p.x, p.y)
    }
}

impl Add for Point {
    type Output = Point;
    fn add(self, rhs: Point) -> Point {
        Point::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Point {
    type Output = Point;
    fn sub(self, rhs: Point) -> Point {
        Point::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl fmt::Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x.mm(), self.y.mm())
    }
}

/// Rotation in 90° steps, counter-clockwise as seen on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    R0,
    R90,
    R180,
    R270,
}

/// Placement of a child (symbol or instance) in its parent's coordinate space.
///
/// Applied in the order: mirror about the Y axis, then rotate, then translate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Transform {
    pub position: Point,
    #[serde(default, skip_serializing_if = "is_default")]
    pub rotation: Rotation,
    #[serde(default, skip_serializing_if = "is_default")]
    pub mirror: bool,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

impl Transform {
    pub const fn at(position: Point) -> Self {
        Transform { position, rotation: Rotation::R0, mirror: false }
    }

    /// Maps a point from child-local coordinates into parent coordinates.
    pub fn apply(&self, p: Point) -> Point {
        let (mut x, y) = (p.x.0, p.y.0);
        if self.mirror {
            x = -x;
        }
        // Screen space has +Y down, so a visually counter-clockwise turn maps
        // (x, y) to (y, -x).
        let (rx, ry) = match self.rotation {
            Rotation::R0 => (x, y),
            Rotation::R90 => (y, -x),
            Rotation::R180 => (-x, -y),
            Rotation::R270 => (-y, x),
        };
        Point::new(Length(rx), Length(ry)) + self.position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_mm_round_trip_is_exact_on_grid() {
        assert_eq!(Length::from_mm(2.54), Length::from_mils(100));
        assert_eq!(Length::from_mils(50).mm(), 1.27);
    }

    #[test]
    fn rotation_is_counter_clockwise_on_screen() {
        // A pin pointing right (+x) should point up (-y on screen) after a 90° CCW turn.
        let t = Transform { rotation: Rotation::R90, ..Transform::default() };
        assert_eq!(t.apply(Point::mils(100, 0)), Point::mils(0, -100));
    }

    #[test]
    fn transform_mirrors_then_rotates_then_translates() {
        let t =
            Transform { position: Point::mils(1000, 1000), rotation: Rotation::R180, mirror: true };
        // mirror: (100, 50) -> (-100, 50); rotate 180: (100, -50); translate.
        assert_eq!(t.apply(Point::mils(100, 50)), Point::mils(1100, 950));
    }

    #[test]
    fn point_serializes_as_mm_tuple() {
        let s = ron::to_string(&Point::mils(100, -50)).unwrap();
        assert_eq!(s, "(2.54,-1.27)");
        let p: Point = ron::from_str("(2.54, -1.27)").unwrap();
        assert_eq!(p, Point::mils(100, -50));
    }
}
