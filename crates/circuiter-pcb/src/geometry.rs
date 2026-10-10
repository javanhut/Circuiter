//! Copper as polygons: pads, tracks and vias as outlines (grown by a margin when a
//! clearance is to be kept), and the booleans and offsets that pours and checks need.
//! Polygons are in millimetres; a shape is an outer contour and its holes, contours
//! counter-clockwise by their coordinates (holes clockwise).

use crate::layout::{P, PadAt};
use circuiter_model::board::{PadShape, Track, Via};
use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::style::{LineJoin, OutlineStyle};

pub type Contour = Vec<P>;
/// An outer contour, then its holes.
pub type Shape = Vec<Contour>;
pub type Shapes = Vec<Shape>;

/// Segments per full circle: fine enough that the error stays under 5 µm for pads.
fn segments(r: f64) -> usize {
    ((r * 64.0) as usize).clamp(24, 128)
}

/// Twice the signed area: positive for counter-clockwise.
pub fn area2(c: &[P]) -> f64 {
    let n = c.len();
    (0..n)
        .map(|k| {
            let (a, b) = (c[k], c[(k + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum()
}

/// `c` running counter-clockwise.
pub fn ccw(mut c: Contour) -> Contour {
    if area2(&c) < 0.0 {
        c.reverse();
    }
    c
}

pub fn area(shapes: &Shapes) -> f64 {
    shapes.iter().flatten().map(|c| area2(c) / 2.0).sum()
}

pub fn circle(center: P, r: f64) -> Contour {
    let n = segments(r);
    (0..n)
        .map(|k| {
            let a = std::f64::consts::TAU * k as f64 / n as f64;
            [center[0] + r * a.cos(), center[1] + r * a.sin()]
        })
        .collect()
}

/// A `w` × `h` rectangle with corners rounded by `r`, centred on `center` and turned
/// `deg` degrees counter-clockwise on screen.
pub fn rounded_rect(center: P, w: f64, h: f64, r: f64, deg: f64) -> Contour {
    let r = r.clamp(0.0, w.min(h) / 2.0);
    let (hw, hh) = (w / 2.0 - r, h / 2.0 - r);
    let mut pts = Vec::new();
    if r <= 1e-9 {
        pts = vec![[hw, hh], [-hw, hh], [-hw, -hh], [hw, -hh]];
    } else {
        let per = (segments(r) / 4).max(2);
        for (cx, cy, start) in
            [(hw, hh, 0.0f64), (-hw, hh, 90.0), (-hw, -hh, 180.0), (hw, -hh, 270.0)]
        {
            for k in 0..=per {
                let a = (start + 90.0 * k as f64 / per as f64).to_radians();
                pts.push([cx + r * a.cos(), cy + r * a.sin()]);
            }
        }
    }
    let (s, c) = deg.to_radians().sin_cos();
    ccw(pts
        .into_iter()
        .map(|[x, y]| [center[0] + x * c + y * s, center[1] - x * s + y * c])
        .collect())
}

/// A stroke from `a` to `b`, `width` wide with round ends: a track.
pub fn capsule(a: P, b: P, width: f64) -> Contour {
    let r = width / 2.0;
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    if len < 1e-9 {
        return ccw(circle(a, r));
    }
    let base = dy.atan2(dx);
    let n = (segments(r) / 2).max(4);
    let mut pts = Vec::new();
    for (c, from) in
        [(b, base - std::f64::consts::FRAC_PI_2), (a, base + std::f64::consts::FRAC_PI_2)]
    {
        for k in 0..=n {
            let t = from + std::f64::consts::PI * k as f64 / n as f64;
            pts.push([c[0] + r * t.cos(), c[1] + r * t.sin()]);
        }
    }
    ccw(pts)
}

/// A pad's copper, grown by `grow` all round.
pub fn pad(pad: &PadAt, grow: f64) -> Contour {
    let (w, h) = (pad.size.0 + 2.0 * grow, pad.size.1 + 2.0 * grow);
    match pad.shape {
        PadShape::Circle => ccw(circle(pad.center, w.max(h) / 2.0)),
        PadShape::Rect => rounded_rect(pad.center, w, h, grow.max(0.0), pad.rotation),
        PadShape::Oval => rounded_rect(pad.center, w, h, w.min(h) / 2.0, pad.rotation),
        PadShape::RoundRect => rounded_rect(
            pad.center,
            w,
            h,
            pad.size.0.min(pad.size.1) * 0.25 + grow.max(0.0),
            pad.rotation,
        ),
    }
}

pub fn track(t: &Track, grow: f64) -> Contour {
    capsule(crate::layout::p(t.a), crate::layout::p(t.b), t.width.mm() + 2.0 * grow)
}

pub fn via(v: &Via, grow: f64) -> Contour {
    ccw(circle(crate::layout::p(v.at), v.diameter.mm() / 2.0 + grow))
}

fn op(a: &Shapes, b: &Shapes, rule: OverlayRule) -> Shapes {
    if a.is_empty() && matches!(rule, OverlayRule::Union) {
        return b.clone();
    }
    if a.is_empty() || (b.is_empty() && matches!(rule, OverlayRule::Intersect)) {
        return Vec::new();
    }
    if b.is_empty() {
        return a.clone();
    }
    a.overlay(b, rule, FillRule::NonZero)
}

pub fn union(a: &Shapes, b: &Shapes) -> Shapes {
    op(a, b, OverlayRule::Union)
}

pub fn difference(a: &Shapes, b: &Shapes) -> Shapes {
    op(a, b, OverlayRule::Difference)
}

pub fn intersection(a: &Shapes, b: &Shapes) -> Shapes {
    op(a, b, OverlayRule::Intersect)
}

/// Every piece of `shapes` merged into a clean set.
pub fn merge(shapes: Shapes) -> Shapes {
    if shapes.is_empty() {
        return shapes;
    }
    let none: Shapes = Vec::new();
    shapes.overlay(&none, OverlayRule::Subject, FillRule::NonZero)
}

/// `shapes` grown (or, for a negative `d`, shrunk) by `d`, with round corners.
pub fn offset(shapes: &Shapes, d: f64) -> Shapes {
    offset_by(shapes, d, 0.05)
}

/// As [`offset`], with corners rounded in steps of `angle` radians: coarser outlines
/// for bodies, where every corner costs.
pub fn offset_by(shapes: &Shapes, d: f64, angle: f64) -> Shapes {
    if d.abs() < 1e-9 || shapes.is_empty() {
        return shapes.clone();
    }
    let style = OutlineStyle::new(d).line_join(LineJoin::Round(angle));
    shapes.outline(&style)
}

/// Whether two sets of shapes share any area.
pub fn overlaps(a: &Shapes, b: &Shapes) -> bool {
    let (Some(ba), Some(bb)) = (bounds(a), bounds(b)) else { return false };
    if ba.1[0] < bb.0[0] || bb.1[0] < ba.0[0] || ba.1[1] < bb.0[1] || bb.1[1] < ba.0[1] {
        return false;
    }
    area(&intersection(a, b)) > 1e-9
}

pub fn bounds(shapes: &Shapes) -> Option<(P, P)> {
    let mut it = shapes.iter().flatten().flatten();
    let first = *it.next()?;
    Some(it.fold((first, first), |(lo, hi), q| {
        ([lo[0].min(q[0]), lo[1].min(q[1])], [hi[0].max(q[0]), hi[1].max(q[1])])
    }))
}

/// Shortest distance between two segments.
pub fn segment_distance(a: P, b: P, c: P, d: P) -> f64 {
    fn point_seg(p: P, a: P, b: P) -> f64 {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = dx * dx + dy * dy;
        let t = if l2 < 1e-18 {
            0.0
        } else {
            (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
        };
        ((a[0] + t * dx - p[0]).powi(2) + (a[1] + t * dy - p[1]).powi(2)).sqrt()
    }
    let cross = |o: P, p: P, q: P| (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]);
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    if ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) {
        return 0.0;
    }
    point_seg(a, c, d).min(point_seg(b, c, d)).min(point_seg(c, a, b)).min(point_seg(d, a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_have_their_areas() {
        let r = vec![vec![rounded_rect([0.0, 0.0], 2.0, 1.0, 0.0, 30.0)]];
        assert!((area(&r) - 2.0).abs() < 1e-9);
        let c = vec![vec![circle([1.0, 1.0], 1.0)]];
        assert!((area(&c) - std::f64::consts::PI).abs() < 0.01);
        let t = vec![vec![capsule([0.0, 0.0], [10.0, 0.0], 0.5)]];
        let want = 10.0 * 0.5 + std::f64::consts::PI * 0.0625;
        assert!((area(&t) - want).abs() < 0.005, "{}", area(&t));
    }

    #[test]
    fn booleans_and_offsets() {
        let a = vec![vec![rounded_rect([0.0, 0.0], 4.0, 4.0, 0.0, 0.0)]];
        let b = vec![vec![rounded_rect([2.0, 2.0], 4.0, 4.0, 0.0, 0.0)]];
        assert!((area(&union(&a, &b)) - 28.0).abs() < 1e-6);
        assert!((area(&difference(&a, &b)) - 12.0).abs() < 1e-6);
        assert!(overlaps(&a, &b));
        let far = vec![vec![rounded_rect([10.0, 0.0], 1.0, 1.0, 0.0, 0.0)]];
        assert!(!overlaps(&a, &far));
        // Grown by 1: a 6 × 6 square with rounded corners.
        let grown = offset(&a, 1.0);
        let want = 16.0 + 4.0 * 4.0 + std::f64::consts::PI;
        assert!((area(&grown) - want).abs() < 0.05, "{}", area(&grown));
        assert!((area(&offset(&a, -1.0)) - 4.0).abs() < 1e-6);
    }

    #[test]
    fn segment_distances() {
        assert_eq!(segment_distance([0.0, 0.0], [10.0, 0.0], [5.0, -1.0], [5.0, 1.0]), 0.0);
        assert!(
            (segment_distance([0.0, 0.0], [10.0, 0.0], [0.0, 2.0], [10.0, 2.0]) - 2.0).abs()
                < 1e-12
        );
        assert!(
            (segment_distance([0.0, 0.0], [1.0, 0.0], [4.0, 4.0], [5.0, 4.0]) - 5.0).abs() < 1e-12
        );
    }
}
