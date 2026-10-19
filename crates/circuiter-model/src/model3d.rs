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
    /// The profile drawn with lines, arcs and circles under constraints; when there is
    /// one, an extrusion, revolve, sweep or loft uses it in place of its `sketch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drawing: Option<Drawing>,
    /// Rounds the top and bottom edges of a box, cylinder or extrusion by this radius.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fillet: String,
    /// Bevels those edges by this distance (when there is no fillet).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub chamfer: String,
    /// Which of those edges are rounded or bevelled.
    #[serde(default, skip_serializing_if = "is_default")]
    pub edges: Ends,
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
            drawing: None,
            fillet: String::new(),
            chamfer: String::new(),
            edges: Ends::Both,
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
    /// The sketch carried along `path`, drawn in the XZ plane (its x across, its y up):
    /// the sketch stands square to the path, its origin on it, its y along Y.
    Sweep {
        sketch: Sketch,
        path: Drawing,
    },
    /// From the sketch at z = 0 to `to` at `height`, the two outlines joined round.
    Loft {
        sketch: Sketch,
        to: Drawing,
        height: String,
    },
    /// The inside of feature `of` (a box, cylinder, cone, sphere, extrusion or
    /// revolve), leaving walls `thickness` thick, open at an end or closed; cut, it
    /// hollows `of` out.
    Shell {
        of: String,
        thickness: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        open: Option<Ends>,
    },
}

/// The ends of a shape standing on z = 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Ends {
    #[default]
    Both,
    Top,
    Bottom,
}

/// A sketch drawn as in a 2D CAD program: points, the lines, arcs and circles between
/// them, and the constraints and dimensions that hold them. Its closed outlines make the
/// profile (inner ones are holes); lines that lead nowhere, and guides, are left out.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Drawing {
    /// Each point's x and y, expressions in millimetres. Constraints move the points
    /// given as plain numbers; the others stay where they are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<[String; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub curves: Vec<Curve>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Constraint>,
}

/// One curve of a drawing, between its points (by index).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Curve {
    Line(usize, usize),
    /// A construction line: it guides mirrors and constraints but isn't drawn into the
    /// outline.
    Guide(usize, usize),
    /// Counter-clockwise about `center` from `from` to `to`, at `from`'s distance (a
    /// whole circle when they are the same point).
    Arc {
        center: usize,
        from: usize,
        to: usize,
    },
    Circle {
        center: usize,
        radius: String,
    },
}

/// What holds a drawing's points and curves (by index). Dimensions are expressions, in
/// millimetres or degrees, that may name the cell's parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Constraint {
    /// Two points in one place.
    Coincident(usize, usize),
    /// A point on a curve.
    OnCurve(usize, usize),
    Horizontal(usize),
    Vertical(usize),
    Parallel(usize, usize),
    Perpendicular(usize, usize),
    /// A line and an arc or circle, or two arcs or circles, touching without crossing.
    Tangent(usize, usize),
    /// Two lines of one length, or two arcs or circles of one radius.
    Equal(usize, usize),
    /// A point that stays where it is.
    Fixed(usize),
    /// A line's length.
    Length(usize, String),
    /// Between two points.
    Distance(usize, usize, String),
    /// Across from one point to another.
    DistanceX(usize, usize, String),
    /// Up from one point to another.
    DistanceY(usize, usize, String),
    /// An arc's or circle's.
    Radius(usize, String),
    /// From one line to another, counter-clockwise, in degrees.
    Angle(usize, usize, String),
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

/// A number as a field holds it: up to four decimals, no trailing zeros.
pub fn fmt(v: f64) -> String {
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
        for word in ["drawing", "fillet", "chamfer", "edges"] {
            assert!(!text.contains(word), "{text}");
        }
        // A drawn profile with constraints, rounded edges, and a shell.
        let mut plate =
            Feature::new("Plate", Shape::Extrude { sketch: Sketch::default(), height: "2".into() });
        plate.drawing = Some(Drawing {
            points: vec![["0".into(), "0".into()], ["w".into(), "0".into()]],
            curves: vec![Curve::Line(0, 1)],
            constraints: vec![Constraint::Horizontal(0), Constraint::Length(0, "w".into())],
        });
        plate.fillet = "1".into();
        plate.edges = Ends::Top;
        let shell = Feature::new(
            "Shell",
            Shape::Shell { of: "Plate".into(), thickness: "1".into(), open: Some(Ends::Top) },
        );
        let m = Model3d { features: vec![plate, shell], ..Model3d::default() };
        let text = ron::to_string(&m).unwrap();
        assert!(text.contains("Line(0,1)") && text.contains("edges:Top"), "{text}");
        assert_eq!(ron::from_str::<Model3d>(&text).unwrap(), m);
    }
}
