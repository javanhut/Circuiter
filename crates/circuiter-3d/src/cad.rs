//! Modelled bodies: a list of features evaluated in order into one solid, as a CAD
//! program's history is. Each feature's shape is made in its own frame, turned and
//! placed, then added to, cut from or intersected with the body so far. Dimensions are
//! expressions that may name the cell's parameters.

use crate::math::{Mat4, V3};
use crate::{Edge, Mesh, Rgba, color, csg};
use circuiter_model::model3d::{Axis, Drawing, Ends, Feature, Operation, Shape, Sketch};
use std::collections::BTreeMap;

/// The colour of a body whose features don't say.
pub const BODY_COLOR: &str = "grey";

/// The result of evaluating features: the body, and what went wrong with which feature
/// (those are left out).
#[derive(Debug, Clone, Default)]
pub struct Built {
    pub mesh: Mesh,
    pub errors: Vec<(usize, String)>,
}

/// Evaluates `text` as an expression in millimetres; names are looked up in `vars`.
pub fn value(text: &str, vars: &dyn Fn(&str) -> Option<f64>) -> Result<f32, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err("a value is missing".into());
    }
    circuiter_sim::number::eval_with(t, vars).map(|v| v as f32).map_err(|e| format!("`{t}`: {e}"))
}

/// A sketch's outline as points, its arcs broken into short straight segments.
pub fn outline(
    sketch: &Sketch,
    vars: &dyn Fn(&str) -> Option<f64>,
) -> Result<Vec<(f32, f32)>, String> {
    let pts: Vec<(f32, f32, f32)> = sketch
        .points
        .iter()
        .map(|p| {
            let bulge = if p.bulge.trim().is_empty() { 0.0 } else { value(&p.bulge, vars)? };
            Ok((value(&p.x, vars)?, value(&p.y, vars)?, bulge))
        })
        .collect::<Result<_, String>>()?;
    if pts.len() < 2 || (pts.len() < 3 && pts.iter().all(|p| p.2 == 0.0)) {
        return Err("a sketch needs at least three corners (or two joined by arcs)".into());
    }
    let mut out = Vec::new();
    for k in 0..pts.len() {
        let (x0, y0, b) = pts[k];
        let (x1, y1, _) = pts[(k + 1) % pts.len()];
        out.push((x0, y0));
        if b.abs() < 1e-6 {
            continue;
        }
        // An arc: sweep 4·atan(bulge), counter-clockwise for a positive bulge.
        let theta = 4.0 * b.atan();
        let (dx, dy) = (x1 - x0, y1 - y0);
        let chord = (dx * dx + dy * dy).sqrt();
        if chord < 1e-9 {
            continue;
        }
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        // The centre lies off the chord's middle, to its left for a positive bulge.
        let d = chord / 2.0 * (1.0 - b * b) / (2.0 * b);
        let (nx, ny) = (-dy / chord, dx / chord);
        let (cx, cy) = (mx + nx * d, my + ny * d);
        let r = ((x0 - cx).powi(2) + (y0 - cy).powi(2)).sqrt();
        let a0 = (y0 - cy).atan2(x0 - cx);
        let steps = ((theta.abs() * 24.0 / std::f32::consts::PI).ceil() as usize).max(4);
        for s in 1..steps {
            let a = a0 + theta * s as f32 / steps as f32;
            out.push((cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    Ok(out)
}

/// A feature's profile: its drawing's outlines when it has one, else its sketch's.
/// Largest first; those inside others are holes.
pub fn profile(
    f: &Feature,
    sketch: &Sketch,
    vars: &dyn Fn(&str) -> Option<f64>,
) -> Result<Vec<Vec<(f32, f32)>>, String> {
    match &f.drawing {
        Some(d) => crate::sketch::profile(d, vars),
        None => Ok(vec![outline(sketch, vars)?]),
    }
}

/// A drawing's outlines, for a loft's far end.
fn drawn(d: &Drawing, vars: &dyn Fn(&str) -> Option<f64>) -> Result<Vec<(f32, f32)>, String> {
    Ok(crate::sketch::profile(d, vars)?.swap_remove(0))
}

/// How a feature's bottom and top edges are finished.
fn edges(f: &Feature, vars: &dyn Fn(&str) -> Option<f64>) -> Result<(Edge, Edge), String> {
    let e = if !f.fillet.trim().is_empty() {
        Edge::Round(value(&f.fillet, vars)?)
    } else if !f.chamfer.trim().is_empty() {
        Edge::Bevel(value(&f.chamfer, vars)?)
    } else {
        Edge::Sharp
    };
    Ok(match f.edges {
        Ends::Both => (e, e),
        Ends::Top => (Edge::Sharp, e),
        Ends::Bottom => (e, Edge::Sharp),
    })
}

/// A circle as an outline.
fn circle(r: f32) -> Vec<(f32, f32)> {
    let n = segments(r);
    (0..n)
        .map(|k| {
            let a = std::f32::consts::TAU * k as f32 / n as f32;
            (r * a.cos(), r * a.sin())
        })
        .collect()
}

/// Outlines revolved: those inside an odd number of others are taken away.
fn revolve_all(rings: &[Vec<(f32, f32)>], a: f32, c: Rgba) -> Mesh {
    let r = rings.iter().flatten().map(|p| p.0).fold(0.0, f32::max);
    let mut m = Mesh::default();
    for (k, ring) in rings.iter().enumerate() {
        let depth =
            rings.iter().enumerate().filter(|(j, o)| *j != k && crate::inside(o, ring[0])).count();
        let part = crate::revolve(ring, a.abs().to_radians(), segments(r), c);
        m = if depth % 2 == 0 { csg::union(&m, &part) } else { csg::difference(&m, &part, None) };
    }
    if a < 0.0 { m.transformed(&Mat4::scale([1.0, -1.0, 1.0])) } else { m }
}

/// The inside of `src` that leaves walls `t` thick, open at the ends asked for, in
/// `src`'s own frame.
fn hollow(
    src: &Feature,
    t: f32,
    open: Option<Ends>,
    vars: &dyn Fn(&str) -> Option<f64>,
    c: Rgba,
) -> Result<Mesh, String> {
    let v = |s: &str| value(s, vars);
    let (lo_open, hi_open) = match open {
        None => (false, false),
        Some(Ends::Both) => (true, true),
        Some(Ends::Top) => (false, true),
        Some(Ends::Bottom) => (true, false),
    };
    // Through an open end, short of a closed one.
    let span = |z0: f32, z1: f32| {
        let (a, b) =
            (if lo_open { z0 - 1.0 } else { z0 + t }, if hi_open { z1 + 1.0 } else { z1 - t });
        if b > a { Ok((a, b)) } else { Err("the walls are too thick for it".to_owned()) }
    };
    let thin = || Err("the walls are too thick for it".to_owned());
    Ok(match &src.shape {
        Shape::Box { size, radius } => {
            let (w, d, h) = (v(&size[0])? - 2.0 * t, v(&size[1])? - 2.0 * t, v(&size[2])?);
            if w <= 0.0 || d <= 0.0 {
                return thin();
            }
            let (a, b) = span(0.0, h)?;
            let r = if radius.trim().is_empty() { 0.0 } else { v(radius)? - t };
            if r > 0.0 {
                crate::extrude(&crate::rounded_rect(w, d, r, 8), a, b, c)
            } else {
                crate::cuboid([-w / 2.0, -d / 2.0, a], [w / 2.0, d / 2.0, b], c)
            }
        }
        Shape::Cylinder { radius, height } => {
            let r = v(radius)? - t;
            if r <= 0.0 {
                return thin();
            }
            let (a, b) = span(0.0, v(height)?)?;
            crate::cylinder(r, a, b, segments(r), c)
        }
        Shape::Cone { bottom, top, height } => {
            let (rb, rt, h) = (v(bottom)?, v(top)?, v(height)?);
            let (a, b) = span(0.0, h)?;
            let at = |z: f32| (rb + (rt - rb) * z / h - t).max(0.0);
            if at(a) <= 0.0 && at(b) <= 0.0 {
                return thin();
            }
            crate::lathe(&[(0.0, a), (at(a), a), (at(b), b), (0.0, b)], segments(rb.max(rt)), c)
        }
        Shape::Sphere { radius } => {
            let r = v(radius)? - t;
            if r <= 0.0 {
                return thin();
            }
            crate::sphere([0.0; 3], r, segments(r), c)
        }
        Shape::Extrude { sketch, height } => {
            let h = v(height)?;
            let rings: Vec<_> = crate::orient_nested(&profile(src, sketch, vars)?)
                .iter()
                .map(|r| crate::inset(r, t))
                .collect();
            let (a, b) = span(h.min(0.0), h.max(0.0))?;
            crate::extrude_holes(&rings[0], &rings[1..], a, b, c)
        }
        Shape::Revolve { sketch, angle } => {
            let rings: Vec<_> = crate::orient_nested(&profile(src, sketch, vars)?)
                .iter()
                .map(|r| crate::inset(r, t))
                .collect();
            revolve_all(&rings, v(angle)?.clamp(-360.0, 360.0), c)
        }
        _ => {
            return Err(
                "only boxes, cylinders, cones, spheres, extrusions and revolves can be hollowed"
                    .into(),
            );
        }
    })
}

fn segments(r: f32) -> usize {
    ((r * 8.0) as usize).clamp(48, 128)
}

/// A feature's own shape, before it is placed; `earlier` are the features before it.
fn shape(
    f: &Feature,
    earlier: &[Feature],
    vars: &dyn Fn(&str) -> Option<f64>,
    made: &BTreeMap<String, Mesh>,
    c: Rgba,
) -> Result<Mesh, String> {
    let v = |t: &str| value(t, vars);
    Ok(match &f.shape {
        Shape::Box { size, radius } => {
            let (w, d, h) = (v(&size[0])?, v(&size[1])?, v(&size[2])?);
            if w <= 0.0 || d <= 0.0 || h <= 0.0 {
                return Err("a box needs a positive size".into());
            }
            let r = if radius.trim().is_empty() { 0.0 } else { v(radius)? };
            let (lo, hi) = edges(f, vars)?;
            if lo != Edge::Sharp || hi != Edge::Sharp {
                let ring = if r > 0.0 {
                    crate::rounded_rect(w, d, r, 8)
                } else {
                    vec![
                        (-w / 2.0, -d / 2.0),
                        (w / 2.0, -d / 2.0),
                        (w / 2.0, d / 2.0),
                        (-w / 2.0, d / 2.0),
                    ]
                };
                crate::extrude_edged(&[ring], 0.0, h, lo, hi, c)
            } else if r > 0.0 {
                crate::extrude(&crate::rounded_rect(w, d, r, 8), 0.0, h, c)
            } else {
                crate::cuboid([-w / 2.0, -d / 2.0, 0.0], [w / 2.0, d / 2.0, h], c)
            }
        }
        Shape::Cylinder { radius, height } => {
            let (r, h) = (v(radius)?, v(height)?);
            if r <= 0.0 || h <= 0.0 {
                return Err("a cylinder needs a positive radius and height".into());
            }
            match edges(f, vars)? {
                (Edge::Sharp, Edge::Sharp) => crate::cylinder(r, 0.0, h, segments(r), c),
                (lo, hi) => crate::extrude_edged(&[circle(r)], 0.0, h, lo, hi, c),
            }
        }
        Shape::Cone { bottom, top, height } => {
            let (b, t, h) = (v(bottom)?.max(0.0), v(top)?.max(0.0), v(height)?);
            if h <= 0.0 || (b == 0.0 && t == 0.0) {
                return Err("a cone needs a height and a radius".into());
            }
            crate::lathe(&[(0.0, 0.0), (b, 0.0), (t, h), (0.0, h)], segments(b.max(t)), c)
        }
        Shape::Sphere { radius } => {
            let r = v(radius)?;
            if r <= 0.0 {
                return Err("a sphere needs a positive radius".into());
            }
            crate::sphere([0.0; 3], r, segments(r), c)
        }
        Shape::Extrude { sketch, height } => {
            let h = v(height)?;
            if h == 0.0 {
                return Err("an extrusion needs a height".into());
            }
            let rings = profile(f, sketch, vars)?;
            let (z0, z1) = (h.min(0.0), h.max(0.0));
            match edges(f, vars)? {
                (Edge::Sharp, Edge::Sharp) => {
                    crate::extrude_holes(&rings[0], &rings[1..], z0, z1, c)
                }
                (lo, hi) => crate::extrude_edged(&rings, z0, z1, lo, hi, c),
            }
        }
        Shape::Revolve { sketch, angle } => {
            let a = v(angle)?.clamp(-360.0, 360.0);
            if a.abs() < 1e-3 {
                return Err("a revolve needs an angle".into());
            }
            let rings = profile(f, sketch, vars)?;
            if rings.iter().flatten().any(|p| p.0 < -1e-4) {
                return Err("a revolve's sketch must stay on one side of the axis (x ≥ 0)".into());
            }
            revolve_all(&rings, a, c)
        }
        Shape::Sweep { sketch, path } => {
            let rings = profile(f, sketch, vars)?;
            let (g, miss) = crate::sketch::solve(path, vars, &[])?;
            if miss > 1e-4 {
                return Err("the path's constraints can't all be met".into());
            }
            let pts: Vec<V3> = crate::sketch::path(path, &g)
                .iter()
                .map(|p| [p[0] as f32, 0.0, p[1] as f32])
                .collect();
            if pts.len() < 2 {
                return Err(
                    "a sweep needs a path: draw one or more lines or arcs end to end".into()
                );
            }
            crate::sweep(&rings, &pts, [0.0, 1.0, 0.0], c)
        }
        Shape::Loft { sketch, to, height } => {
            let h = v(height)?;
            if h.abs() < 1e-6 {
                return Err("a loft needs a height".into());
            }
            let a = profile(f, sketch, vars)?.swap_remove(0);
            let b = drawn(to, vars).map_err(|e| format!("its far end: {e}"))?;
            crate::loft(&a, &b, h, c)
        }
        Shape::Shell { of, thickness, open } => {
            let src = earlier
                .iter()
                .rev()
                .find(|e| e.name == *of)
                .ok_or_else(|| format!("no earlier feature called `{of}`"))?;
            let t = v(thickness)?;
            if t <= 0.0 {
                return Err("a shell needs a wall thickness".into());
            }
            recolor(hollow(src, t, *open, vars, c)?.transformed(&placement(src, vars)?), c)
        }
        Shape::Copies { of, count, step } => {
            let base = made.get(of).ok_or_else(|| format!("no earlier feature called `{of}`"))?;
            let n = v(count)?.round() as i32;
            let s: V3 = [v(&step[0])?, v(&step[1])?, v(&step[2])?];
            let mut m = Mesh::default();
            for k in 1..n.max(1) {
                m.append(base.clone().translated(crate::math::scale(s, k as f32)));
            }
            recolor(m, c)
        }
        Shape::Ring { of, count, angle } => {
            let base = made.get(of).ok_or_else(|| format!("no earlier feature called `{of}`"))?;
            let n = v(count)?.round() as i32;
            let total = v(angle)?;
            // A full turn shares its span among all; a partial one ends on the last.
            let full = (total.abs() - 360.0).abs() < 1e-3;
            let step = if full { total / n.max(1) as f32 } else { total / (n - 1).max(1) as f32 };
            let mut m = Mesh::default();
            for k in 1..n.max(1) {
                m.append(base.clone().transformed(&Mat4::rotate_z((step * k as f32).to_radians())));
            }
            recolor(m, c)
        }
        Shape::Mirror { of, axis } => {
            let base = made.get(of).ok_or_else(|| format!("no earlier feature called `{of}`"))?;
            let s = match axis {
                Axis::X => [-1.0, 1.0, 1.0],
                Axis::Y => [1.0, -1.0, 1.0],
                Axis::Z => [1.0, 1.0, -1.0],
            };
            recolor(base.clone().transformed(&Mat4::scale(s)), c)
        }
    })
}

fn recolor(mut m: Mesh, c: Rgba) -> Mesh {
    m.vertices.iter_mut().for_each(|v| v.color = c);
    m
}

/// Where a feature goes: turned about X, then Y, then Z, then moved.
fn placement(f: &Feature, vars: &dyn Fn(&str) -> Option<f64>) -> Result<Mat4, String> {
    let n = |t: &str| if t.trim().is_empty() { Ok(0.0) } else { value(t, vars) };
    let at = [n(&f.at[0])?, n(&f.at[1])?, n(&f.at[2])?];
    let turn = [n(&f.turn[0])?, n(&f.turn[1])?, n(&f.turn[2])?].map(f32::to_radians);
    Ok(Mat4::translate(at)
        .mul(&Mat4::rotate_z(turn[2]))
        .mul(&Mat4::rotate_y(turn[1]))
        .mul(&Mat4::rotate_x(turn[0])))
}

/// Whether a feature places itself: patterns and mirrors work on another feature as it
/// was placed.
fn placed_by_others(f: &Feature) -> bool {
    matches!(
        f.shape,
        Shape::Copies { .. } | Shape::Ring { .. } | Shape::Mirror { .. } | Shape::Shell { .. }
    )
}

/// Evaluates `features` into one body. A feature that fails is left out, and its error
/// reported by its index.
pub fn evaluate(features: &[Feature], vars: &dyn Fn(&str) -> Option<f64>) -> Built {
    let mut out = Built::default();
    let body_color = features
        .iter()
        .find(|f| !f.hidden && f.operation == Operation::Add && !f.color.trim().is_empty())
        .map_or(color(BODY_COLOR), |f| color(&f.color));
    // Each feature's placed shape, by name, for patterns and mirrors.
    let mut made: BTreeMap<String, Mesh> = BTreeMap::new();
    for (k, f) in features.iter().enumerate() {
        if f.hidden {
            continue;
        }
        let c = if f.color.trim().is_empty() { body_color } else { color(&f.color) };
        let placed = shape(f, &features[..k], vars, &made, c).and_then(|m| {
            if placed_by_others(f) { Ok(m) } else { Ok(m.transformed(&placement(f, vars)?)) }
        });
        let m = match placed {
            Ok(m) => m,
            Err(e) => {
                out.errors.push((k, e));
                continue;
            }
        };
        out.mesh = match f.operation {
            Operation::Add => csg::union(&out.mesh, &m),
            Operation::Cut => csg::difference(&out.mesh, &m, Some(body_color)),
            Operation::Intersect => csg::intersection(&out.mesh, &m),
        };
        made.insert(f.name.clone(), m);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::model3d::SketchPoint;

    fn s3(a: &str, b: &str, c: &str) -> [String; 3] {
        [a.into(), b.into(), c.into()]
    }

    fn volume(m: &Mesh) -> f32 {
        m.indices
            .chunks(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize].pos);
                crate::math::dot(a, crate::math::cross(b, c)) / 6.0
            })
            .sum()
    }

    #[test]
    fn an_enclosure_from_features() {
        let vars = |n: &str| match n {
            "w" => Some(60.0),
            "wall" => Some(2.0),
            _ => None,
        };
        let mut cavity = Feature::new(
            "Cavity",
            Shape::Box { size: s3("w - 2*wall", "36", "30"), radius: String::new() },
        );
        cavity.operation = Operation::Cut;
        cavity.at = s3("0", "0", "wall");
        let mut hole =
            Feature::new("Hole", Shape::Cylinder { radius: "2".into(), height: "10".into() });
        hole.operation = Operation::Cut;
        hole.at = s3("-20", "0", "-1");
        let mut more = Feature::new(
            "Holes",
            Shape::Copies { of: "Hole".into(), count: "3".into(), step: s3("20", "0", "0") },
        );
        more.operation = Operation::Cut;
        let features = vec![
            Feature::new("Case", Shape::Box { size: s3("w", "40", "25"), radius: String::new() }),
            cavity,
            hole,
            more,
        ];
        let b = evaluate(&features, &vars);
        assert!(b.errors.is_empty(), "{:?}", b.errors);
        let holes = 3.0 * std::f32::consts::PI * 4.0 * 2.0;
        let want = 60.0 * 40.0 * 25.0 - 56.0 * 36.0 * 23.0 - holes;
        let got = volume(&b.mesh);
        assert!((got - want).abs() < want * 0.002, "{got} vs {want}");
    }

    #[test]
    fn sketches_with_arcs_extrude_and_revolve() {
        let none = |_: &str| None;
        // A circle of radius 5 made of two half arcs.
        let disc = Feature::new(
            "Disc",
            Shape::Extrude { sketch: Sketch::circle(5.0), height: "2".into() },
        );
        let v = volume(&evaluate(&[disc], &none).mesh);
        // 48 straight segments round the circle: 0.3 % short of a true one.
        assert!((v - std::f32::consts::PI * 25.0 * 2.0).abs() < 0.6, "{v}");
        // A ring of square section, half way round.
        let section = Sketch::rectangle(1.0, 1.0);
        let mut moved = section.clone();
        for p in &mut moved.points {
            p.x = format!("{} + 1.5", p.x);
        }
        let ring = Feature::new("Ring", Shape::Revolve { sketch: moved, angle: "180".into() });
        let v = volume(&evaluate(&[ring], &none).mesh);
        let want = std::f32::consts::PI * (4.0 - 1.0) / 2.0;
        assert!((v - want).abs() < 0.05, "{v} vs {want}");
        // An arc bulging outwards (counter-clockwise) adds area; inwards removes it.
        let mut d = Sketch::rectangle(2.0, 2.0);
        d.points[1].bulge = "1".into();
        let pts = outline(&d, &none).unwrap();
        assert!(pts.iter().any(|p| p.0 > 1.9), "the right side bulges out");
    }

    #[test]
    fn patterns_mirrors_and_errors() {
        let none = |_: &str| None;
        let mut post =
            Feature::new("Post", Shape::Cylinder { radius: "1".into(), height: "5".into() });
        post.at = s3("10", "0", "0");
        let ring = Feature::new(
            "Posts",
            Shape::Ring { of: "Post".into(), count: "4".into(), angle: "360".into() },
        );
        let mirror = Feature::new("Mirrored", Shape::Mirror { of: "Post".into(), axis: Axis::Z });
        let bad = Feature::new(
            "Bad",
            Shape::Copies { of: "Nothing".into(), count: "2".into(), step: s3("1", "0", "0") },
        );
        let typo = Feature::new("Typo", Shape::Sphere { radius: "r*2".into() });
        let b = evaluate(&[post, ring, mirror, bad, typo], &none);
        assert_eq!(b.errors.iter().map(|e| e.0).collect::<Vec<_>>(), [3, 4], "{:?}", b.errors);
        let want = 5.0 * std::f32::consts::PI * 5.0;
        assert!(
            (volume(&b.mesh) - want).abs() < 1.0,
            "four posts up and one down: {}",
            volume(&b.mesh)
        );
        let (lo, _) = b.mesh.bounds().unwrap();
        assert!(lo[2] < -4.9 && lo[0] < -10.5, "{lo:?}");
        let _ = SketchPoint::new(0.0, 0.0);
    }

    #[test]
    fn drawn_profiles_edges_shells_lofts_and_sweeps() {
        use circuiter_model::model3d::{Curve, Ends};
        let none = |_: &str| None;
        let pi = std::f32::consts::PI;
        let near = |got: f32, want: f32, tol: f32| {
            assert!((got - want).abs() < want.abs() * tol, "{got} vs {want}");
        };
        // A plate drawn with a hole: a rectangle held by constraints, a circle inside.
        let mut d = Drawing::default();
        crate::sketch::rectangle(&mut d, [-10.0, -5.0], [10.0, 5.0]);
        let c = crate::sketch::add_point(&mut d, [0.0, 0.0]);
        d.curves.push(Curve::Circle { center: c, radius: "r".into() });
        let mut plate =
            Feature::new("Plate", Shape::Extrude { sketch: Sketch::default(), height: "2".into() });
        plate.drawing = Some(d.clone());
        let vars = |n: &str| (n == "r").then_some(2.0);
        let b = evaluate(std::slice::from_ref(&plate), &vars);
        assert!(b.errors.is_empty(), "{:?}", b.errors);
        near(volume(&b.mesh), (200.0 - pi * 4.0 * 0.997) * 2.0, 0.003);

        // A cylinder bevelled 1 mm at both ends.
        let mut can =
            Feature::new("Can", Shape::Cylinder { radius: "5".into(), height: "10".into() });
        can.chamfer = "1".into();
        let want = pi * (200.0 + 2.0 * (25.0 + 20.0 + 16.0) / 3.0) * 0.997;
        near(volume(&evaluate(&[can], &none).mesh), want, 0.004);

        // A box rounded along its top edges only.
        let mut cube =
            Feature::new("Cube", Shape::Box { size: s3("10", "10", "10"), radius: String::new() });
        cube.fillet = "2".into();
        cube.edges = Ends::Top;
        let band: f32 = (0..1000)
            .map(|k| {
                let u = (k as f32 + 0.5) / 500.0;
                (10.0 - 2.0 * (2.0 - (4.0 - u * u).sqrt())).powi(2) / 500.0
            })
            .sum();
        let rounded = evaluate(std::slice::from_ref(&cube), &none).mesh;
        near(volume(&rounded), 800.0 + band, 0.004);
        let (_, hi) = rounded.bounds().unwrap();
        assert!((hi[2] - 10.0).abs() < 1e-4);

        // That box hollowed, open at the top.
        cube.fillet.clear();
        let mut shell = Feature::new(
            "Shell",
            Shape::Shell { of: "Cube".into(), thickness: "1".into(), open: Some(Ends::Top) },
        );
        shell.operation = Operation::Cut;
        let b = evaluate(&[cube, shell], &none);
        assert!(b.errors.is_empty(), "{:?}", b.errors);
        near(volume(&b.mesh), 1000.0 - 64.0 * 9.0, 0.002);

        // A square loft from 10 wide to 4 wide: a frustum.
        let mut top = Drawing::default();
        crate::sketch::rectangle(&mut top, [-2.0, -2.0], [2.0, 2.0]);
        let loft = Feature::new(
            "Loft",
            Shape::Loft { sketch: Sketch::rectangle(10.0, 10.0), to: top, height: "6".into() },
        );
        near(volume(&evaluate(&[loft], &none).mesh), 2.0 * (100.0 + 16.0 + 40.0), 0.002);

        // A square bar swept up and over: its section times its length.
        let mut path = Drawing::default();
        let p =
            [[0.0, 0.0], [0.0, 10.0], [10.0, 10.0]].map(|q| crate::sketch::add_point(&mut path, q));
        path.curves.push(Curve::Line(p[0], p[1]));
        path.curves.push(Curve::Line(p[1], p[2]));
        let bar = Feature::new("Bar", Shape::Sweep { sketch: Sketch::rectangle(2.0, 2.0), path });
        let b = evaluate(&[bar], &none);
        assert!(b.errors.is_empty(), "{:?}", b.errors);
        near(volume(&b.mesh), 80.0, 0.002);
        let (lo, hi) = b.mesh.bounds().unwrap();
        assert!(
            (hi[0] - 10.0).abs() < 1e-4 && (lo[2]).abs() < 1e-4 && (hi[2] - 11.0).abs() < 1e-4,
            "{lo:?} {hi:?}"
        );

        // A drawing with nothing closed says so.
        let mut open = Drawing::default();
        let (a, b2) = (
            crate::sketch::add_point(&mut open, [0.0, 0.0]),
            crate::sketch::add_point(&mut open, [5.0, 0.0]),
        );
        open.curves.push(Curve::Line(a, b2));
        plate.drawing = Some(open);
        assert_eq!(evaluate(&[plate], &none).errors.len(), 1);
    }
}
