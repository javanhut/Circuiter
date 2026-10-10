//! Modelled bodies: a list of features evaluated in order into one solid, as a CAD
//! program's history is. Each feature's shape is made in its own frame, turned and
//! placed, then added to, cut from or intersected with the body so far. Dimensions are
//! expressions that may name the cell's parameters.

use crate::math::{Mat4, V3};
use crate::{Mesh, Rgba, color, csg};
use circuiter_model::model3d::{Axis, Feature, Operation, Shape, Sketch};
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

fn segments(r: f32) -> usize {
    ((r * 8.0) as usize).clamp(48, 128)
}

/// A feature's own shape, before it is placed.
fn shape(
    f: &Feature,
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
            if r > 0.0 {
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
            crate::cylinder(r, 0.0, h, segments(r), c)
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
            crate::extrude(&outline(sketch, vars)?, h.min(0.0), h.max(0.0), c)
        }
        Shape::Revolve { sketch, angle } => {
            let a = v(angle)?.clamp(-360.0, 360.0);
            if a.abs() < 1e-3 {
                return Err("a revolve needs an angle".into());
            }
            let pts = outline(sketch, vars)?;
            if pts.iter().any(|p| p.0 < -1e-4) {
                return Err("a revolve's sketch must stay on one side of the axis (x ≥ 0)".into());
            }
            let r = pts.iter().map(|p| p.0).fold(0.0, f32::max);
            let m = crate::revolve(&pts, a.abs().to_radians(), segments(r), c);
            if a < 0.0 { m.transformed(&Mat4::scale([1.0, -1.0, 1.0])) } else { m }
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
    matches!(f.shape, Shape::Copies { .. } | Shape::Ring { .. } | Shape::Mirror { .. })
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
        let placed = shape(f, vars, &made, c).and_then(|m| {
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
}
