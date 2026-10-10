//! 3D view: the part's physical body. Either one of Circuiter's package generators
//! (`axial`, `dip`, `to220`, `radial_can`…) with parameters, or a body modelled feature
//! by feature, as in a CAD program: shapes and sketches added, cut and intersected in
//! order, each placed and turned, with patterns and mirrors of earlier ones.
//!
//! Package parameters may use `${param.NAME}` to take the instance's own parameters, as
//! SPICE templates do: an axial resistor reads its colour bands from `R`. Feature
//! dimensions are expressions in millimetres that may name the cell's parameters
//! (`width/2 + 3`).
//!
//! ```ron
//! (package: "dip", params: {"pins": "8"})
//! (features: [(name: "Case", shape: Box(size: ("60", "40", "25"), radius: "4"))])
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Model3d {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub package: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// A body modelled feature by feature; when there is one, it is used instead of
    /// the package.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<Feature>,
}

/// One step of a modelled body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    pub name: String,
    pub shape: Shape,
    #[serde(default, skip_serializing_if = "is_default")]
    pub operation: Operation,
    /// Where its origin goes, in millimetres.
    #[serde(default = "origin", skip_serializing_if = "is_origin")]
    pub at: [String; 3],
    /// Turns about X, then Y, then Z, in degrees.
    #[serde(default = "origin", skip_serializing_if = "is_origin")]
    pub turn: [String; 3],
    /// A colour name (`grey`, `black`, `red`…); empty for the body's.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub color: String,
    /// Left out of the body (suppressed), but kept.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

impl Feature {
    pub fn new(name: &str, shape: Shape) -> Feature {
        Feature {
            name: name.to_owned(),
            shape,
            operation: Operation::Add,
            at: origin(),
            turn: origin(),
            color: String::new(),
            hidden: false,
        }
    }
}

/// How a feature combines with the body made so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Operation {
    /// Joined to it.
    #[default]
    Add,
    /// Removed from it: holes, pockets, slots.
    Cut,
    /// Only what both share is kept.
    Intersect,
}

/// The shape a feature makes, in its own frame: centred in X and Y, standing on z = 0.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Shape {
    /// A box; `radius` rounds its four vertical edges.
    Box {
        size: [String; 3],
        #[serde(default, skip_serializing_if = "String::is_empty")]
        radius: String,
    },
    Cylinder {
        radius: String,
        height: String,
    },
    /// A cone or a truncated one: radius at the bottom and at the top.
    Cone {
        bottom: String,
        top: String,
        height: String,
    },
    Sphere {
        radius: String,
    },
    /// A sketch in the XY plane, raised by `height`.
    Extrude {
        sketch: Sketch,
        height: String,
    },
    /// A sketch turned about the Z axis by `angle` degrees: its x is the distance from
    /// the axis, its y the height.
    Revolve {
        sketch: Sketch,
        angle: String,
    },
    /// `count` copies of feature `of`, each moved by `step` from the one before.
    Copies {
        of: String,
        count: String,
        step: [String; 3],
    },
    /// `count` copies of feature `of` around the Z axis, spread over `angle` degrees.
    Ring {
        of: String,
        count: String,
        angle: String,
    },
    /// Feature `of` reflected across the plane through the origin square to `axis`.
    Mirror {
        of: String,
        axis: Axis,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Axis {
    #[default]
    X,
    Y,
    Z,
}

/// A closed outline: corners in order, each joined to the next by a straight line or,
/// with a bulge, an arc (the bulge is the tangent of a quarter of its angle: 1 for a
/// half circle, negative to curve the other way).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Sketch {
    pub points: Vec<SketchPoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SketchPoint {
    pub x: String,
    pub y: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bulge: String,
}

impl SketchPoint {
    pub fn new(x: f64, y: f64) -> SketchPoint {
        SketchPoint { x: fmt(x), y: fmt(y), bulge: String::new() }
    }
}

impl Sketch {
    /// A `w` × `h` rectangle centred on the origin.
    pub fn rectangle(w: f64, h: f64) -> Sketch {
        let (a, b) = (w / 2.0, h / 2.0);
        Sketch {
            points: vec![
                SketchPoint::new(-a, -b),
                SketchPoint::new(a, -b),
                SketchPoint::new(a, b),
                SketchPoint::new(-a, b),
            ],
        }
    }

    /// A circle of radius `r`: two half-circle arcs.
    pub fn circle(r: f64) -> Sketch {
        let half = |x: f64| SketchPoint { bulge: "1".into(), ..SketchPoint::new(x, 0.0) };
        Sketch { points: vec![half(-r), half(r)] }
    }

    /// A slot `len` long (end centres) and `w` wide, along X.
    pub fn slot(len: f64, w: f64) -> Sketch {
        let (a, r) = (len / 2.0, w / 2.0);
        let arc = |x: f64, y: f64| SketchPoint { bulge: "1".into(), ..SketchPoint::new(x, y) };
        Sketch {
            points: vec![SketchPoint::new(-a, -r), arc(a, -r), SketchPoint::new(a, r), arc(-a, r)],
        }
    }

    /// A regular polygon with `n` corners on a circle of radius `r`.
    pub fn polygon(n: usize, r: f64) -> Sketch {
        let n = n.max(3);
        Sketch {
            points: (0..n)
                .map(|k| {
                    let a = std::f64::consts::TAU * k as f64 / n as f64;
                    SketchPoint::new(r * a.cos(), r * a.sin())
                })
                .collect(),
        }
    }
}

fn fmt(v: f64) -> String {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

fn origin() -> [String; 3] {
    ["0".into(), "0".into(), "0".into()]
}

fn is_origin(v: &[String; 3]) -> bool {
    v.iter().all(|s| s.trim().is_empty() || s.trim() == "0")
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_round_trip_compactly() {
        let mut hole =
            Feature::new("Hole", Shape::Cylinder { radius: "1.5".into(), height: "10".into() });
        hole.operation = Operation::Cut;
        hole.at = ["10".into(), "0".into(), "-1".into()];
        let m = Model3d {
            features: vec![
                Feature::new(
                    "Case",
                    Shape::Box {
                        size: ["60".into(), "40".into(), "w/2".into()],
                        radius: "4".into(),
                    },
                ),
                hole,
                Feature::new(
                    "Knob",
                    Shape::Extrude { sketch: Sketch::slot(10.0, 4.0), height: "3".into() },
                ),
            ],
            ..Model3d::default()
        };
        let text = ron::to_string(&m).unwrap();
        assert!(!text.contains("turn") && !text.contains("hidden"), "{text}");
        let back: Model3d = ron::from_str(&text).unwrap();
        assert_eq!(back, m);
        assert_eq!(Sketch::rectangle(10.0, 4.0).points[0], SketchPoint::new(-5.0, -2.0));
        assert_eq!(fmt(-0.00001), "0");
        assert_eq!(fmt(2.54), "2.54");
    }
}
