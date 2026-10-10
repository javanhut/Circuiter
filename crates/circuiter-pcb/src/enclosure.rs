//! A case for the board, made to fit it: a base whose walls follow the board's outline
//! at a gap, standoffs under the board's mounting holes (or its corners) tall enough
//! to clear the leads, openings in the walls for connectors at the edge and in the lid
//! for buttons, switches, pots and LEDs, and a lid with a locating lip and, if asked,
//! vent slots. Both are ordinary modelled bodies (features), so they can be taken
//! further in the body editor.
//!
//! Then whether everything fits (no part's body inside the case's walls, nothing
//! through the lid), and how the case holds heat in: the thermal resistance from its
//! air to the room and its heat capacity, for the physics engine.

use crate::board3d::Board3d;
use crate::geometry::{self, Shapes};
use crate::layout::{Layout, P, p};
use circuiter_3d::math::V3;
use circuiter_3d::solid::Solid;
use circuiter_3d::{Mesh, cad};
use circuiter_model::board::{CaseMaterial, Enclosure, PadKind};
use circuiter_model::model3d::{Feature, Model3d, Operation, Shape, Sketch, SketchPoint};

/// Parts reached through the wall when they sit at the board's edge.
const THROUGH_WALL: &[&str] = &["header", "terminal"];
/// Parts reached through the lid, and the hole each needs (radius, mm).
const THROUGH_LID: &[(&str, f64)] =
    &[("tactile", 2.2), ("pot", 6.5), ("toggle_switch", 3.5), ("led", 2.7)];

/// A case made for a board.
#[derive(Debug, Clone)]
pub struct Made {
    pub base: Model3d,
    pub lid: Model3d,
    /// The space inside, world coordinates (board top at z = 0, board Y up).
    pub inner: (V3, V3),
    /// Outside.
    pub outer: (V3, V3),
    /// The lid's underside.
    pub lid_z: f64,
    pub wall: f64,
}

fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

fn at(x: f64, y: f64, z: f64) -> [String; 3] {
    [num(x), num(y), num(z)]
}

/// An outline on the board (Y down) as a sketch in the world (Y up).
fn sketch(shape: &Shapes) -> Option<Sketch> {
    let outer = shape.first()?.first()?;
    Some(Sketch { points: outer.iter().map(|q| SketchPoint::new(q[0], -q[1])).collect() })
}

/// Outlines for the case's bodies: corners in 15° steps.
fn coarse(shapes: &Shapes, d: f64) -> Shapes {
    geometry::offset_by(shapes, d, 0.26)
}

fn feature(name: &str, shape: Shape, op: Operation, place: [String; 3]) -> Feature {
    Feature { operation: op, at: place, ..Feature::new(name, shape) }
}

/// The bounds of every part's body, by part path (world coordinates).
fn part_bounds(built: &Board3d) -> Vec<(&str, V3, V3)> {
    built
        .parts
        .iter()
        .filter_map(|(path, m)| m.bounds().map(|(lo, hi)| (path.as_str(), lo, hi)))
        .collect()
}

/// The case for the board of `layout`, its parts as built in 3D.
pub fn make(layout: &Layout, built: &Board3d, e: &Enclosure) -> Result<Made, String> {
    let board = &layout.board;
    let t = board.thickness.mm();
    let outline: Vec<P> = board.outline.iter().map(|q| p(*q)).collect();
    if outline.len() < 3 {
        return Err("the board needs an outline first".into());
    }
    let shape: Shapes = vec![vec![geometry::ccw(outline.clone())]];
    // Inside the walls: the board's outline at the gap, its corners rounded.
    let mut inner = coarse(&shape, e.gap.max(0.2));
    let round = (e.radius - e.wall).max(0.0);
    if round > e.gap {
        inner = coarse(&coarse(&inner, -round), round);
    }
    let outer = coarse(&inner, e.wall.max(0.5));
    let (Some(inner_sketch), Some(outer_sketch)) = (sketch(&inner), sketch(&outer)) else {
        return Err("the board's outline is too small for a case".into());
    };
    // How high the parts stand and how far their leads reach under the board.
    let bounds = part_bounds(built);
    let top = bounds.iter().map(|b| b.2[2] as f64).fold(5.0, f64::max);
    let bottom = bounds.iter().map(|b| b.1[2] as f64).fold(-t, f64::min);
    let below = if e.below > 0.0 { e.below } else { ((-t - bottom) + 2.0).max(4.0) };
    let above = if e.above > 0.0 { e.above } else { top + 2.0 };
    let floor = -t - below;
    let lid_z = above;
    let wall = e.wall.max(0.5);
    let height = lid_z - (floor - wall);
    let mut base = vec![
        feature(
            "Shell",
            Shape::Extrude { sketch: outer_sketch.clone(), height: num(height) },
            Operation::Add,
            at(0.0, 0.0, floor - wall),
        ),
        feature(
            "Cavity",
            Shape::Extrude { sketch: inner_sketch.clone(), height: num(lid_z - floor + 1.0) },
            Operation::Cut,
            at(0.0, 0.0, floor),
        ),
    ];
    // Standoffs: under the mounting holes, else in from the corners.
    let mut mounts: Vec<P> = layout
        .pads
        .iter()
        .filter(|pad| matches!(pad.kind, PadKind::Hole { .. }))
        .map(|pad| pad.center)
        .collect();
    let (lo, hi) = geometry::bounds(&shape).expect("an outline");
    if mounts.is_empty() && hi[0] - lo[0] > 16.0 && hi[1] - lo[1] > 16.0 {
        let k = 3.5;
        mounts = vec![
            [lo[0] + k, lo[1] + k],
            [hi[0] - k, lo[1] + k],
            [hi[0] - k, hi[1] - k],
            [lo[0] + k, hi[1] - k],
        ];
        // Not under a part's leads.
        mounts.retain(|m| {
            !layout.pads.iter().any(|pad| (pad.center[0] - m[0]).hypot(pad.center[1] - m[1]) < 3.5)
        });
    }
    for (k, m) in mounts.iter().enumerate() {
        let n = k + 1;
        base.push(feature(
            &format!("Standoff {n}"),
            Shape::Cylinder { radius: "2.75".into(), height: num(below) },
            Operation::Add,
            at(m[0], -m[1], floor),
        ));
        base.push(feature(
            &format!("Screw hole {n}"),
            Shape::Cylinder { radius: "1.1".into(), height: num(below) },
            Operation::Cut,
            at(m[0], -m[1], floor + 1.0),
        ));
    }
    let (ilo, ihi) = geometry::bounds(&inner).expect("an inside");
    let mut lid_holes = Vec::new();
    if e.openings {
        for (k, part) in layout.parts.iter().enumerate() {
            let Some((_, plo, phi)) = bounds.iter().find(|b| b.0 == part.path) else { continue };
            let (plo, phi) = (
                [plo[0] as f64, -(phi[1] as f64), plo[2] as f64],
                [phi[0] as f64, -(plo[1] as f64), phi[2] as f64],
            );
            if THROUGH_WALL.contains(&part.package.as_str()) {
                // The nearest wall, if it's close.
                let sides = [
                    (plo[0] - ilo[0], 0),
                    (ihi[0] - phi[0], 1),
                    (plo[1] - ilo[1], 2),
                    (ihi[1] - phi[1], 3),
                ];
                let (d, side) =
                    sides.iter().copied().fold((f64::MAX, 0), |a, b| if b.0 < a.0 { b } else { a });
                if d < 4.0 {
                    // From just outside the wall in to the part, as wide and tall as it.
                    let (z0, z1) = (plo[2].max(0.0) - 0.5, phi[2] + 0.5);
                    let out = wall + 1.0;
                    let ((x0, x1), (y0, y1)) = match side {
                        0 => ((ilo[0] - out, plo[0] + 0.5), (plo[1] - 0.5, phi[1] + 0.5)),
                        1 => ((phi[0] - 0.5, ihi[0] + out), (plo[1] - 0.5, phi[1] + 0.5)),
                        2 => ((plo[0] - 0.5, phi[0] + 0.5), (ilo[1] - out, plo[1] + 0.5)),
                        _ => ((plo[0] - 0.5, phi[0] + 0.5), (phi[1] - 0.5, ihi[1] + out)),
                    };
                    base.push(feature(
                        &format!("Opening for {}", part.reference),
                        Shape::Box {
                            size: [num(x1 - x0), num(y1 - y0), num(z1 - z0)],
                            radius: String::new(),
                        },
                        Operation::Cut,
                        at((x0 + x1) / 2.0, -(y0 + y1) / 2.0, z0),
                    ));
                }
            }
            if let Some((_, r)) = THROUGH_LID.iter().find(|(pkg, _)| *pkg == part.package) {
                let c = part
                    .place
                    .map(|pl| pl.at)
                    .unwrap_or([(plo[0] + phi[0]) / 2.0, (plo[1] + phi[1]) / 2.0]);
                lid_holes.push((format!("Hole for {}", layout.parts[k].reference), c, *r));
            }
        }
    }
    // The lid: a plate, a lip that locates it inside the walls, holes and vents.
    let lip_out = coarse(&inner, -0.3);
    let lip_in = coarse(&inner, -1.8);
    let mut lid = vec![feature(
        "Lid",
        Shape::Extrude { sketch: outer_sketch, height: num(wall) },
        Operation::Add,
        at(0.0, 0.0, lid_z),
    )];
    if let (Some(a), Some(b)) = (sketch(&lip_out), sketch(&lip_in)) {
        lid.push(feature(
            "Lip",
            Shape::Extrude { sketch: a, height: "2".into() },
            Operation::Add,
            at(0.0, 0.0, lid_z - 2.0),
        ));
        lid.push(feature(
            "Lip inside",
            Shape::Extrude { sketch: b, height: "2.01".into() },
            Operation::Cut,
            at(0.0, 0.0, lid_z - 2.005),
        ));
    }
    for (name, c, r) in lid_holes {
        lid.push(feature(
            &name,
            Shape::Cylinder { radius: num(r), height: num(wall + 3.0) },
            Operation::Cut,
            at(c[0], -c[1], lid_z - 2.5),
        ));
    }
    if e.vents {
        let (w, d) = (ihi[0] - ilo[0], ihi[1] - ilo[1]);
        let len = (w * 0.5).clamp(6.0, 30.0);
        let rows = (((d * 0.4) / 4.0).floor() as usize).clamp(1, 8);
        let (cx, cy) = ((ilo[0] + ihi[0]) / 2.0, (ilo[1] + ihi[1]) / 2.0);
        let y0 = cy - (rows as f64 - 1.0) * 2.0;
        lid.push(feature(
            "Vent",
            Shape::Extrude { sketch: Sketch::slot(len, 2.0), height: num(wall + 3.0) },
            Operation::Cut,
            at(cx, -y0, lid_z - 2.5),
        ));
        if rows > 1 {
            lid.push(feature(
                "Vents",
                Shape::Copies {
                    of: "Vent".into(),
                    count: rows.to_string(),
                    step: ["0".into(), "-4".into(), "0".into()],
                },
                Operation::Cut,
                at(0.0, 0.0, 0.0),
            ));
        }
    }
    let colour = match e.material {
        CaseMaterial::Plastic => "grey",
        CaseMaterial::Aluminium => "aluminium",
    };
    for f in base.iter_mut().chain(lid.iter_mut()) {
        f.color = colour.into();
    }
    let (olo, ohi) = geometry::bounds(&outer).expect("an outside");
    Ok(Made {
        base: Model3d { features: base, ..Model3d::default() },
        lid: Model3d { features: lid, ..Model3d::default() },
        inner: (
            [ilo[0] as f32, -ihi[1] as f32, floor as f32],
            [ihi[0] as f32, -ilo[1] as f32, lid_z as f32],
        ),
        outer: (
            [olo[0] as f32, -ohi[1] as f32, (floor - wall) as f32],
            [ohi[0] as f32, -olo[1] as f32, (lid_z + wall) as f32],
        ),
        lid_z,
        wall,
    })
}

/// The bodies of a made case: base and lid.
pub fn bodies(made: &Made) -> (Mesh, Mesh) {
    let none = |_: &str| None;
    (cad::evaluate(&made.base.features, &none).mesh, cad::evaluate(&made.lid.features, &none).mesh)
}

/// What doesn't fit: parts inside the case's walls, floor or lid, or standing out
/// above it, and a board too big for it. Each finding names the part.
pub fn fit(layout: &Layout, built: &Board3d, case: &[&Mesh], lid_top: Option<f32>) -> Vec<String> {
    let solids: Vec<Solid> = case.iter().map(|m| Solid::new(m)).collect();
    let inside = |v: V3| solids.iter().any(|s| s.contains(v));
    let mut out = Vec::new();
    for (path, mesh) in &built.parts {
        let hits = mesh.vertices.iter().filter(|v| inside(v.pos)).count();
        let reference =
            layout.part(path).map_or(path.as_str(), |k| layout.parts[k].reference.as_str());
        if hits > 0 {
            out.push(format!(
                "{reference} runs into the case ({hits} points of it inside the walls)"
            ));
        } else if let (Some(top), Some((_, hi))) = (lid_top, mesh.bounds()) {
            if hi[2] > top {
                out.push(format!("{reference} stands {:.1} mm above the lid", hi[2] - top));
            }
        }
    }
    let t = layout.board.thickness.mm() as f32;
    let corners = layout.board.outline.iter().flat_map(|q| {
        let (x, y) = (q.x.mm() as f32, -(q.y.mm() as f32));
        [[x, y, -0.01], [x, y, -t + 0.01]]
    });
    let stuck = corners.filter(|&c| inside(c)).count();
    if stuck > 0 {
        out.push(format!("the board's edge runs into the case at {} corner(s)", stuck / 2));
    }
    out
}

/// How the case passes heat to the room: thermal resistance (K/W) from the air inside,
/// and heat capacity (J/K) of the air and the case.
pub fn thermal(made: &Made, e: &Enclosure) -> (f64, f64) {
    thermal_of(made.outer, made.inner, made.wall, e)
}

/// [`thermal`] for a case of any shape, from its outside and inside bounds (mm).
pub fn thermal_of(outer: (V3, V3), inner: (V3, V3), wall: f64, e: &Enclosure) -> (f64, f64) {
    let size = |b: &(V3, V3)| [0, 1, 2].map(|k| f64::from((b.1[k] - b.0[k]).max(0.0)) / 1000.0);
    let [l, w, h] = size(&outer);
    let [li, wi, hi] = size(&inner);
    let area = 2.0 * (l * w + l * h + w * h);
    let area_in = (2.0 * (li * wi + li * hi + wi * hi)).max(1e-6);
    let wall = wall / 1000.0;
    // Natural convection and radiation outside (≈8 W/m²K), still air inside (≈4).
    let (h_out, h_in) = (8.0, 4.0);
    let (k, density, heat) = match e.material {
        CaseMaterial::Plastic => (0.2, 1050.0, 1400.0),
        CaseMaterial::Aluminium => (200.0, 2700.0, 900.0),
    };
    let mut r =
        1.0 / (h_out * area.max(1e-6)) + wall / (k * area.max(1e-6)) + 1.0 / (h_in * area_in);
    if e.vents {
        // Air through the slots carries heat out: about halves it.
        r *= 0.55;
    }
    let air = li * wi * hi;
    let shell = (l * w * h - air).max(0.0);
    let capacity = 1.2 * 1005.0 * air + density * heat * shell;
    (r, capacity)
}
