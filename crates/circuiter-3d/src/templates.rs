//! Ready-made bodies to start a part from: chips with bent or gull-wing leads, ball
//! grids, headers, card and memory slots, connectors, capacitors, a heatsink. Each is
//! an ordinary feature history (a body, one lead drawn as its side profile, and copies
//! and mirrors of it), so every dimension stays editable: the pin count is a copy
//! count, the pitch its step, the lead's shape its sketch.
//!
//! Bodies are centred on the origin with rows of pins running along Y and leads
//! reaching out along X, standing on the board's top (z = 0), through-hole pins below
//! it, as the package generators and KiCad's footprints have them.

use circuiter_model::model3d::{Axis, Ends, Feature, Operation, Shape, Sketch, SketchPoint};

/// The templates on offer: name and what it is.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("DIP-8", "Through-hole chip: two rows of bent leads, 2.54 mm pitch"),
    ("SOIC-8", "Surface-mount chip with gull-wing leads, 1.27 mm pitch"),
    ("QFP-44", "Square chip with gull-wing leads on all four sides, 0.8 mm pitch"),
    ("BGA-121", "Ball grid array: 11 × 11 solder balls under a substrate"),
    ("Pin header 1×8", "Single row of square pins, 2.54 mm pitch"),
    ("Pin header 2×8", "Double row of square pins, 2.54 mm pitch"),
    ("PCIe x16 slot", "Card edge slot with 82 contacts a side and its key"),
    ("DDR4 DIMM slot", "Memory slot with 144 contacts a side, latches and key"),
    ("USB-A receptacle", "Metal shell, tongue and mounting legs"),
    ("TO-220", "Power transistor: body, tab with its hole, three leads"),
    ("Electrolytic capacitor", "Radial can on two leads"),
    ("Heatsink", "Base and a row of fins"),
];

/// A number as a feature dimension: short, without float noise.
fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn n3(v: [f64; 3]) -> [String; 3] {
    v.map(num)
}

fn feature(name: &str, shape: Shape, at: [f64; 3], color: &str) -> Feature {
    let mut f = Feature::new(name, shape);
    f.at = n3(at);
    f.color = color.into();
    f
}

fn cuboid(name: &str, size: [f64; 3], at: [f64; 3], color: &str) -> Feature {
    feature(name, Shape::Box { size: n3(size), radius: String::new() }, at, color)
}

fn cut(mut f: Feature) -> Feature {
    f.operation = Operation::Cut;
    f
}

fn copies(name: &str, of: &str, count: usize, step: [f64; 3], color: &str) -> Feature {
    let shape = Shape::Copies { of: of.into(), count: count.to_string(), step: n3(step) };
    feature(name, shape, [0.0; 3], color)
}

fn mirror(name: &str, of: &str, axis: Axis, color: &str) -> Feature {
    feature(name, Shape::Mirror { of: of.into(), axis }, [0.0; 3], color)
}

fn ring(name: &str, of: &str, count: usize, color: &str) -> Feature {
    let shape = Shape::Ring { of: of.into(), count: count.to_string(), angle: "360".into() };
    feature(name, shape, [0.0; 3], color)
}

/// A lead drawn as its side profile (x out from the body, y up) and extruded to its
/// width `w`, standing square to Y at `y` (its middle).
fn lead(name: &str, profile: &[(f64, f64)], w: f64, y: f64) -> Feature {
    let sketch = Sketch { points: profile.iter().map(|&(x, z)| SketchPoint::new(x, z)).collect() };
    let mut f = feature(
        name,
        Shape::Extrude { sketch, height: num(w) },
        // The profile's plane turned upright: its y up the Z axis, the width along −Y.
        [0.0, y + w / 2.0, 0.0],
        "silver",
    );
    f.turn = n3([90.0, 0.0, 0.0]);
    f
}

/// A through-hole lead: out from the body's side at `inner` along its shoulder at
/// height `top`, then bent straight down at `outer` to `bottom`, `t` thick.
fn bent(inner: f64, outer: f64, top: f64, bottom: f64, t: f64) -> Vec<(f64, f64)> {
    vec![
        (inner, top),
        (outer, top),
        (outer, bottom),
        (outer - t, bottom),
        (outer - t, top - t),
        (inner, top - t),
    ]
}

/// A gull-wing lead: out from the body at `inner` at height `top`, down a slope to a
/// foot on the board ending at `end`, `t` thick.
fn gull_wing(inner: f64, end: f64, top: f64, t: f64) -> Vec<(f64, f64)> {
    let len = end - inner;
    let (knee, ankle) = (inner + 0.3 * len, inner + 0.55 * len);
    let d = t * 0.6;
    vec![
        (inner, top),
        (knee, top),
        (ankle, t),
        (end, t),
        (end, 0.0),
        (ankle - d, 0.0),
        (knee - d, top - t),
        (inner, top - t),
    ]
}

/// Two rows of leads along Y either side of a body: one lead, its copies down the row,
/// and both mirrored across to the other side.
fn two_rows(profile: &[(f64, f64)], w: f64, per_side: usize, pitch: f64) -> Vec<Feature> {
    let first = (per_side - 1) as f64 * pitch / 2.0;
    vec![
        lead("Lead", profile, w, first),
        copies("Leads", "Lead", per_side, [0.0, -pitch, 0.0], "silver"),
        mirror("Lead, other side", "Lead", Axis::X, "silver"),
        mirror("Leads, other side", "Leads", Axis::X, "silver"),
    ]
}

/// The features of template `name`.
pub fn template(name: &str) -> Option<Vec<Feature>> {
    let mut f = match name {
        "DIP-8" => {
            let mut f = vec![cuboid("Body", [6.35, 9.4, 3.3], [0.0, 0.0, 0.5], "black")];
            f.push(cut(feature(
                "Notch",
                Shape::Cylinder { radius: "0.8".into(), height: "1".into() },
                [0.0, 4.7, 3.3],
                "",
            )));
            f.extend(two_rows(&bent(3.17, 3.94, 1.75, -3.3, 0.25), 0.5, 4, 2.54));
            f
        }
        "SOIC-8" => {
            let mut f = vec![cuboid("Body", [3.9, 4.9, 1.45], [0.0, 0.0, 0.1], "black")];
            f.push(cut(feature(
                "Pin 1 dot",
                Shape::Cylinder { radius: "0.3".into(), height: "0.1".into() },
                [-1.2, 1.7, 1.5],
                "",
            )));
            f.extend(two_rows(&gull_wing(1.95, 3.0, 0.75, 0.2), 0.42, 4, 1.27));
            f
        }
        "QFP-44" => {
            let (per_side, pitch) = (11, 0.8);
            let first = (per_side - 1) as f64 * pitch / 2.0;
            vec![
                cuboid("Body", [10.0, 10.0, 1.4], [0.0, 0.0, 0.1], "black"),
                cut(feature(
                    "Pin 1 dot",
                    Shape::Cylinder { radius: "0.5".into(), height: "0.1".into() },
                    [-4.0, 4.0, 1.45],
                    "",
                )),
                lead("Lead", &gull_wing(5.0, 6.0, 0.75, 0.15), 0.37, first),
                copies("Leads", "Lead", per_side, [0.0, -pitch, 0.0], "silver"),
                ring("Lead, other sides", "Lead", 4, "silver"),
                ring("Leads, other sides", "Leads", 4, "silver"),
            ]
        }
        "BGA-121" => {
            let (n, pitch) = (11, 1.27);
            let first = -((n - 1) as f64) * pitch / 2.0;
            vec![
                cuboid("Substrate", [15.0, 15.0, 0.4], [0.0, 0.0, 0.61], "green"),
                cuboid("Mould", [13.0, 13.0, 0.9], [0.0, 0.0, 1.01], "black"),
                feature(
                    "Ball",
                    Shape::Sphere { radius: "0.3".into() },
                    [first, first, 0.3],
                    "silver",
                ),
                copies("Ball row", "Ball", n, [pitch, 0.0, 0.0], "silver"),
                copies("Ball column", "Ball", n, [0.0, pitch, 0.0], "silver"),
                copies("Ball grid", "Ball row", n, [0.0, pitch, 0.0], "silver"),
            ]
        }
        "Pin header 1×8" => {
            let n = 8;
            vec![
                cuboid("Base", [2.54 * n as f64, 2.54, 2.5], [0.0, 0.0, 0.0], "black"),
                cuboid(
                    "Pin",
                    [0.64, 0.64, 11.5],
                    [-2.54 * (n - 1) as f64 / 2.0, 0.0, -3.0],
                    "gold",
                ),
                copies("Pins", "Pin", n, [2.54, 0.0, 0.0], "gold"),
            ]
        }
        "Pin header 2×8" => {
            let n = 8;
            vec![
                cuboid("Base", [2.54 * n as f64, 5.08, 2.5], [0.0, 0.0, 0.0], "black"),
                cuboid(
                    "Pin",
                    [0.64, 0.64, 11.5],
                    [-2.54 * (n - 1) as f64 / 2.0, 1.27, -3.0],
                    "gold",
                ),
                copies("Pins", "Pin", n, [2.54, 0.0, 0.0], "gold"),
                mirror("Pin, second row", "Pin", Axis::Y, "gold"),
                mirror("Pins, second row", "Pins", Axis::Y, "gold"),
            ]
        }
        "PCIe x16 slot" => {
            let (n, pitch) = (82, 1.0);
            vec![
                cuboid("Body", [89.0, 7.5, 11.0], [0.0, 0.0, 0.0], "black"),
                cut(cuboid("Slot", [85.0, 1.8, 8.0], [0.0, 0.0, 3.5], "")),
                cuboid("Key", [1.2, 1.8, 8.0], [-32.85, 0.0, 3.0], "black"),
                cuboid(
                    "Contact",
                    [0.5, 0.3, 3.3],
                    [-((n - 1) as f64) * pitch / 2.0, 1.5, -3.0],
                    "gold",
                ),
                copies("Contacts", "Contact", n, [pitch, 0.0, 0.0], "gold"),
                mirror("Contact, other side", "Contact", Axis::Y, "gold"),
                mirror("Contacts, other side", "Contacts", Axis::Y, "gold"),
            ]
        }
        "DDR4 DIMM slot" => {
            let (n, pitch) = (144, 0.85);
            vec![
                cuboid("Body", [142.0, 7.0, 9.5], [0.0, 0.0, 0.0], "black"),
                cut(cuboid("Slot", [133.5, 1.6, 7.0], [0.0, 0.0, 3.0], "")),
                cuboid("Key", [1.2, 1.6, 7.0], [4.5, 0.0, 2.5], "black"),
                cuboid("Latch", [3.0, 7.0, 13.0], [69.5, 0.0, 0.0], "white"),
                mirror("Latch, other end", "Latch", Axis::X, "white"),
                cuboid(
                    "Contact",
                    [0.4, 0.3, 3.3],
                    [-((n - 1) as f64) * pitch / 2.0, 1.3, -3.0],
                    "gold",
                ),
                copies("Contacts", "Contact", n, [pitch, 0.0, 0.0], "gold"),
                mirror("Contact, other side", "Contact", Axis::Y, "gold"),
                mirror("Contacts, other side", "Contacts", Axis::Y, "gold"),
            ]
        }
        "USB-A receptacle" => vec![
            cuboid("Shell", [13.1, 14.0, 5.7], [0.0, 0.0, 0.0], "silver"),
            cut(cuboid("Opening", [12.5, 13.6, 5.1], [0.0, -0.5, 0.3], "")),
            cuboid("Tongue", [11.0, 9.0, 1.8], [0.0, -2.0, 2.0], "white"),
            cuboid("Leg", [0.9, 0.3, 3.0], [6.25, 0.0, -3.0], "silver"),
            mirror("Leg, other side", "Leg", Axis::X, "silver"),
        ],
        "TO-220" => {
            let mut hole = feature(
                "Hole",
                Shape::Cylinder { radius: "1.8".into(), height: "2".into() },
                [0.0, 2.5, 15.6],
                "",
            );
            hole.turn = n3([90.0, 0.0, 0.0]);
            vec![
                cuboid("Body", [10.0, 4.5, 9.0], [0.0, 0.0, 3.0], "black"),
                cuboid("Tab", [10.0, 1.3, 6.5], [0.0, 1.6, 12.0], "silver"),
                cut(hole),
                cuboid("Lead", [0.8, 0.5, 6.5], [-2.54, 0.0, -3.5], "silver"),
                copies("Leads", "Lead", 3, [2.54, 0.0, 0.0], "silver"),
            ]
        }
        "Electrolytic capacitor" => {
            let mut can = feature(
                "Can",
                Shape::Cylinder { radius: "2.5".into(), height: "11".into() },
                [0.0, 0.0, 0.5],
                "darkblue",
            );
            can.fillet = "0.4".into();
            can.edges = Ends::Top;
            vec![
                can,
                feature(
                    "Lead",
                    Shape::Cylinder { radius: "0.25".into(), height: "4".into() },
                    [1.25, 0.0, -3.5],
                    "silver",
                ),
                mirror("Lead, other side", "Lead", Axis::X, "silver"),
            ]
        }
        "Heatsink" => vec![
            cuboid("Base", [30.0, 30.0, 3.0], [0.0, 0.0, 0.0], "aluminium"),
            cuboid("Fin", [1.2, 30.0, 17.0], [-14.4, 0.0, 0.0], "aluminium"),
            copies("Fins", "Fin", 12, [2.6, 0.0, 0.0], "aluminium"),
        ],
        _ => return None,
    };
    // The body's colour is the first feature's; cuts take it.
    for x in f.iter_mut().filter(|x| x.operation == Operation::Cut) {
        x.color.clear();
    }
    Some(std::mem::take(&mut f))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cad::evaluate;

    #[test]
    fn every_template_builds_whole() {
        for (name, _) in TEMPLATES {
            let f = template(name).unwrap();
            let built = evaluate(&f, &|_| None);
            assert!(built.errors.is_empty(), "{name}: {:?}", built.errors);
            assert!(built.mesh.triangles() > 12, "{name}");
        }
        assert!(template("nothing").is_none());
    }

    #[test]
    fn leads_reach_their_rows_and_through_the_board() {
        // DIP: the leads' bends at ±3.81 (7.62 mm rows), pins 2.54 apart, through the board.
        let built = evaluate(&template("DIP-8").unwrap(), &|_| None);
        let (lo, hi) = built.mesh.bounds().unwrap();
        assert!((hi[0] - 3.94).abs() < 1e-3 && (lo[0] + 3.94).abs() < 1e-3, "{lo:?} {hi:?}");
        assert!(lo[2] < -3.2, "through the board");
        let y_span = 3.0 * 2.54 + 0.5;
        let leads: Vec<f32> =
            built.mesh.vertices.iter().filter(|v| v.pos[2] < -1.0).map(|v| v.pos[1]).collect();
        let (ylo, yhi) =
            leads.iter().fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(*y), b.max(*y)));
        assert!(((yhi - ylo) - y_span as f32).abs() < 1e-3, "{ylo} {yhi}");
        // SOIC: the feet sit on the board, out to 3 mm either side.
        let built = evaluate(&template("SOIC-8").unwrap(), &|_| None);
        let (lo, hi) = built.mesh.bounds().unwrap();
        assert!(lo[2].abs() < 1e-4 && (hi[0] - 3.0).abs() < 1e-3 && (lo[0] + 3.0).abs() < 1e-3);
    }
}
