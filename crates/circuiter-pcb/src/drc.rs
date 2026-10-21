//! Design rule checks, as KiCad's DRC runs them: copper too close to copper of another
//! net (or touching it, a short), tracks too thin, holes too small, rings too narrow,
//! copper too near the board's edge, holes too near each other, courtyards that
//! overlap, nets not yet routed, a broken outline, and a board that disagrees with its
//! schematic.

use crate::connect::{self, Copper, Item};
use crate::geometry::{self, Shapes};
use crate::layout::{Layout, P, p};
use crate::zones::{self, ZoneFill};
use circuiter_model::board::{Graphic, Layer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    Short,
    Clearance,
    TrackWidth,
    Drill,
    AnnularRing,
    EdgeClearance,
    OutsideBoard,
    HoleToHole,
    CourtyardOverlap,
    /// A net's routed length outside its class's limits.
    Length,
    /// Silkscreen printed over a pad, where it can't stick and spoils soldering.
    SilkOverPad,
    /// Something in a keepout that bars it.
    Keepout,
    /// A differential pair's two halves routed to different lengths.
    Skew,
    /// Parts' 3D bodies running into each other.
    Bodies,
    Unconnected,
    Outline,
    Unplaced,
    Orphan,
    Footprint,
}

impl Rule {
    pub fn severity(self) -> Severity {
        match self {
            Rule::CourtyardOverlap | Rule::Unplaced | Rule::Orphan | Rule::SilkOverPad => {
                Severity::Warning
            }
            _ => Severity::Error,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Rule::Short => "Short between nets",
            Rule::Clearance => "Clearance violation",
            Rule::TrackWidth => "Track too narrow",
            Rule::Drill => "Hole too small",
            Rule::AnnularRing => "Annular ring too narrow",
            Rule::EdgeClearance => "Copper too close to the board edge",
            Rule::OutsideBoard => "Outside the board",
            Rule::HoleToHole => "Holes too close",
            Rule::CourtyardOverlap => "Courtyards overlap",
            Rule::Length => "Net length",
            Rule::SilkOverPad => "Silkscreen over a pad",
            Rule::Keepout => "In a keepout",
            Rule::Skew => "Differential pair skew",
            Rule::Bodies => "Component bodies collide",
            Rule::Unconnected => "Unconnected items",
            Rule::Outline => "Board outline",
            Rule::Unplaced => "Part not on the board",
            Rule::Orphan => "Footprint without a part",
            Rule::Footprint => "Footprint error",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub rule: Rule,
    pub message: String,
    /// Where to look.
    pub at: Option<P>,
}

impl Violation {
    pub fn severity(&self) -> Severity {
        self.rule.severity()
    }
}

/// Everything a check needs: the board resolved, its pours filled, its copper.
#[derive(Debug)]
pub struct Checked {
    pub fills: Vec<ZoneFill>,
    pub copper: Vec<Copper>,
    pub connectivity: connect::Connectivity,
    pub violations: Vec<Violation>,
}

impl Checked {
    /// Rules broken, not counting connections still to route.
    pub fn errors(&self) -> usize {
        self.violations
            .iter()
            .filter(|v| v.severity() == Severity::Error && v.rule != Rule::Unconnected)
            .count()
    }

    pub fn warnings(&self) -> usize {
        self.violations.iter().filter(|v| v.severity() == Severity::Warning).count()
    }
}

/// The shortest distance between two shapes: 0 where they meet.
pub fn distance(a: &Shapes, b: &Shapes) -> f64 {
    // The larger first, its edges passed over by the smaller one's box.
    let size = |s: &Shapes| s.iter().flatten().map(Vec::len).sum::<usize>();
    let (a, b) = if size(a) < size(b) { (b, a) } else { (a, b) };
    let (Some(box_a), Some(box_b)) = (geometry::bounds(a), geometry::bounds(b)) else {
        return f64::INFINITY;
    };
    // Edge to edge, passing over segments further from the other shape's box than the
    // best found so far: most of a pour's thousands of edges, for a pad or a track. A
    // corner of `b` to `a`'s edges makes a first best, so the passing over starts at once.
    let mut best = f64::INFINITY;
    if let Some(q) = b.iter().flatten().flatten().next().copied() {
        for ca in a.iter().flatten() {
            for i in 0..ca.len() {
                best = best.min(geometry::segment_distance(ca[i], ca[(i + 1) % ca.len()], q, q));
            }
        }
    }
    for ca in a.iter().flatten() {
        for i in 0..ca.len() {
            let (a0, a1) = (ca[i], ca[(i + 1) % ca.len()]);
            if box_gap(segment_box(a0, a1), box_b) > best {
                continue;
            }
            for cb in b.iter().flatten() {
                for j in 0..cb.len() {
                    let (b0, b1) = (cb[j], cb[(j + 1) % cb.len()]);
                    if box_gap(segment_box(b0, b1), box_a) <= best {
                        best = best.min(geometry::segment_distance(a0, a1, b0, b1));
                    }
                }
            }
        }
    }
    // Edges that never meet still overlap if one shape lies inside the other.
    let first = |s: &Shapes| s.iter().flatten().flatten().next().copied();
    let within = |q: Option<P>, s: &Shapes| q.is_some_and(|q| s.iter().any(|x| contains(x, q)));
    if best > 0.0 && (within(first(b), a) || within(first(a), b)) {
        return 0.0;
    }
    best
}

fn segment_box(p: P, q: P) -> (P, P) {
    ([p[0].min(q[0]), p[1].min(q[1])], [p[0].max(q[0]), p[1].max(q[1])])
}

/// How far apart two boxes are: 0 where they meet.
fn box_gap((lo, hi): (P, P), (lo2, hi2): (P, P)) -> f64 {
    let dx = (lo2[0] - hi[0]).max(lo[0] - hi2[0]).max(0.0);
    let dy = (lo2[1] - hi[1]).max(lo[1] - hi2[1]).max(0.0);
    dx.hypot(dy)
}

/// Whether `q` is inside a shape (its outline less its holes): crossings of a ray
/// counted over every contour, odd inside.
fn contains(shape: &[geometry::Contour], q: P) -> bool {
    let mut odd = false;
    for c in shape {
        for k in 0..c.len() {
            let (a, b) = (c[k], c[(k + 1) % c.len()]);
            if (a[1] > q[1]) != (b[1] > q[1])
                && q[0] < a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
            {
                odd = !odd;
            }
        }
    }
    odd
}

fn center(c: &Copper) -> P {
    [(c.lo[0] + c.hi[0]) / 2.0, (c.lo[1] + c.hi[1]) / 2.0]
}

/// What a piece of copper is, for messages.
pub fn describe(layout: &Layout, item: Item) -> String {
    match item {
        Item::Pad(k) => {
            let pad = &layout.pads[k];
            format!("pad {} of {}", pad.number, layout.parts[pad.part].reference)
        }
        Item::Track(k) => format!("track on {}", layout.board.tracks[k].layer.name()),
        Item::Via(_) => "via".into(),
        Item::Teardrop(_) => "teardrop".into(),
        Item::Fill(z, _) => {
            format!("zone {} on {}", layout.board.zones[z].net, layout.board.zones[z].layer.name())
        }
    }
}

/// The net a piece of copper carries, for clearance purposes: a pad of no net is a net
/// of its own.
fn net_key(c: &Copper, k: usize) -> String {
    c.net.clone().unwrap_or_else(|| format!("\u{0}unconnected-{k}"))
}

/// Whether two segments `ab` and `cd` cross, other than at shared ends.
fn crosses(a: P, b: P, c: P, d: P) -> bool {
    let cross = |o: P, p: P, q: P| (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]);
    let (d1, d2, d3, d4) = (cross(c, d, a), cross(c, d, b), cross(a, b, c), cross(a, b, d));
    d1 * d2 < -1e-12 && d3 * d4 < -1e-12
}

/// Every courtyard, by part, as a shape.
fn courtyards(layout: &Layout) -> Vec<(usize, bool, Shapes)> {
    let mut out = Vec::new();
    for (k, _) in layout.parts.iter().enumerate() {
        let mut shapes: Shapes = Vec::new();
        let mut back = false;
        for g in layout.graphics(k) {
            match g {
                Graphic::Poly { layer, points, .. }
                    if matches!(layer, Layer::FCrtYd | Layer::BCrtYd) =>
                {
                    back = layer == Layer::BCrtYd;
                    shapes.push(vec![geometry::ccw(points.iter().map(|q| p(*q)).collect())]);
                }
                Graphic::Circle { layer, center, radius, .. }
                    if matches!(layer, Layer::FCrtYd | Layer::BCrtYd) =>
                {
                    back = layer == Layer::BCrtYd;
                    shapes.push(vec![geometry::circle(p(center), radius.mm())]);
                }
                _ => {}
            }
        }
        if !shapes.is_empty() {
            out.push((k, back, geometry::merge(shapes)));
        }
    }
    out
}

/// Fills the pours, traces connectivity and checks every rule.
/// Parts whose 3D bodies run into each other: a tall capacitor leaning over its
/// neighbour, two connectors' shells, parts whose footprints keep clear but whose
/// bodies don't. Builds the parts' bodies, so it needs the design they come from.
pub fn bodies(design: &circuiter_model::Design, layout: &Layout) -> Vec<Violation> {
    let built = crate::board3d::build(design, layout, &[]);
    let name = |k: usize| {
        let path = &built.parts[k].0;
        layout.part(path).map_or(path.clone(), |i| layout.parts[i].reference.clone())
    };
    crate::board3d::clashes(&built, layout.board.thickness.mm() as f32)
        .into_iter()
        .map(|(a, b, depth, at)| Violation {
            rule: Rule::Bodies,
            message: format!("{} and {} run {depth:.2} mm into each other", name(a), name(b)),
            at: Some(at),
        })
        .collect()
}

pub fn check(layout: &Layout) -> Checked {
    let board = &layout.board;
    let rules = &board.rules;
    let mm = |v: f64| format!("{v:.3} mm");
    let mut out = Vec::new();
    let mut flag =
        |rule: Rule, message: String, at: Option<P>| out.push(Violation { rule, message, at });

    // The outline.
    let outline: Vec<P> = board.edge().iter().map(|q| p(*q)).collect();
    let edge = zones::board_shape(layout);
    if outline.len() < 3 || geometry::area(&edge) < 1e-6 {
        flag(Rule::Outline, "the board has no closed outline on Edge.Cuts".into(), None);
    } else {
        let n = outline.len();
        'cross: for i in 0..n {
            for j in i + 2..n {
                if i == 0 && j == n - 1 {
                    continue;
                }
                if crosses(outline[i], outline[(i + 1) % n], outline[j], outline[(j + 1) % n]) {
                    flag(
                        Rule::Outline,
                        "the board outline crosses itself".into(),
                        Some(outline[i]),
                    );
                    break 'cross;
                }
            }
        }
    }

    // The schematic.
    for path in &layout.unplaced {
        flag(Rule::Unplaced, format!("{path} is in the schematic but not on the board"), None);
    }
    for path in &layout.orphans {
        let at = board.placed(path).map(|f| p(f.at));
        flag(Rule::Orphan, format!("{path} is on the board but no longer in the schematic"), at);
    }
    for err in &layout.errors {
        flag(Rule::Footprint, err.clone(), None);
    }

    // Sizes.
    for t in &board.tracks {
        let w = t.width.mm();
        if w + 1e-9 < rules.min_track_width.mm() {
            flag(
                Rule::TrackWidth,
                format!(
                    "track of {} is {} wide (minimum {})",
                    t.net,
                    mm(w),
                    mm(rules.min_track_width.mm())
                ),
                Some(p(t.a)),
            );
        }
    }
    let ring = |size: f64, drill: f64| (size - drill) / 2.0;
    for v in &board.vias {
        let (d, dia) = (v.drill.mm(), v.diameter.mm());
        if d + 1e-9 < rules.min_drill.mm() {
            flag(
                Rule::Drill,
                format!("via drill {} (minimum {})", mm(d), mm(rules.min_drill.mm())),
                Some(p(v.at)),
            );
        }
        if ring(dia, d) + 1e-9 < rules.min_annular_ring.mm() {
            flag(
                Rule::AnnularRing,
                format!(
                    "via ring {} (minimum {})",
                    mm(ring(dia, d)),
                    mm(rules.min_annular_ring.mm())
                ),
                Some(p(v.at)),
            );
        }
    }
    for pad in &layout.pads {
        let Some(d) = pad.drill() else { continue };
        let who = format!("pad {} of {}", pad.number, layout.parts[pad.part].reference);
        if d + 1e-9 < rules.min_drill.mm() {
            flag(
                Rule::Drill,
                format!("{who}: drill {} (minimum {})", mm(d), mm(rules.min_drill.mm())),
                Some(pad.center),
            );
        }
        let r = ring(pad.size.0.min(pad.size.1), d);
        if pad.plated() && r + 1e-9 < rules.min_annular_ring.mm() {
            flag(
                Rule::AnnularRing,
                format!("{who}: ring {} (minimum {})", mm(r), mm(rules.min_annular_ring.mm())),
                Some(pad.center),
            );
        }
    }

    // Holes.
    let mut holes: Vec<(P, f64, String)> = layout
        .pads
        .iter()
        .filter_map(|pad| {
            Some((
                pad.center,
                pad.drill()?,
                format!("pad {} of {}", pad.number, layout.parts[pad.part].reference),
            ))
        })
        .collect();
    holes.extend(board.vias.iter().map(|v| (p(v.at), v.drill.mm(), "via".to_owned())));
    for i in 0..holes.len() {
        for j in i + 1..holes.len() {
            let (a, b) = (&holes[i], &holes[j]);
            let gap = (a.0[0] - b.0[0]).hypot(a.0[1] - b.0[1]) - (a.1 + b.1) / 2.0;
            if gap + 1e-9 < rules.hole_to_hole.mm() {
                flag(
                    Rule::HoleToHole,
                    format!(
                        "{} and {}: {} apart (minimum {})",
                        a.2,
                        b.2,
                        mm(gap.max(0.0)),
                        mm(rules.hole_to_hole.mm())
                    ),
                    Some(a.0),
                );
            }
        }
    }

    // Copper.
    let fills = zones::fill(layout);
    let copper = connect::copper(layout, &fills);
    let widest = std::iter::once(rules.clearance.mm())
        .chain(rules.classes.iter().map(|c| c.clearance.mm()))
        .chain(board.zones.iter().filter_map(|z| z.clearance.map(|c| c.mm())))
        .fold(0.0, f64::max);
    for (i, j) in connect::near_pairs(&copper, widest) {
        let (a, b) = (&copper[i], &copper[j]);
        if net_key(a, i) == net_key(b, j) || !a.layers.iter().any(|l| b.layers.contains(l)) {
            continue;
        }
        // Pads of one footprint with no net are the part's business.
        if let (Item::Pad(x), Item::Pad(y)) = (a.item, b.item)
            && layout.pads[x].part == layout.pads[y].part
            && (a.net.is_none() || b.net.is_none())
        {
            continue;
        }
        let zone_gap = |c: &Copper| match c.item {
            Item::Fill(z, _) => board.zones[z].clearance.map_or(0.0, |c| c.mm()),
            _ => 0.0,
        };
        let need = [a, b]
            .iter()
            .map(|c| c.net.as_deref().map_or(rules.clearance.mm(), |n| rules.clearance_for(n).mm()))
            .fold(zone_gap(a).max(zone_gap(b)), f64::max);
        let d = distance(&a.shape, &b.shape);
        let what = format!(
            "{} ({}) and {} ({})",
            describe(layout, a.item),
            a.net.as_deref().unwrap_or("no net"),
            describe(layout, b.item),
            b.net.as_deref().unwrap_or("no net")
        );
        if d < 1e-6 {
            flag(Rule::Short, format!("{what} touch"), Some(center(a)));
        } else if d + 1e-6 < need {
            flag(
                Rule::Clearance,
                format!("{what}: {} apart (needs {})", mm(d), mm(need)),
                Some(center(a)),
            );
        }
    }
    let edges: Vec<(P, P)> = std::iter::once(outline.clone())
        .chain(zones::cutouts(board))
        .flat_map(|c| (0..c.len()).map(move |k| (c[k], c[(k + 1) % c.len()])))
        .collect();
    if !edge.is_empty() {
        for c in &copper {
            if matches!(c.item, Item::Fill(..)) {
                continue;
            }
            let outside = geometry::area(&geometry::difference(&c.shape, &edge));
            if outside > 1e-6 {
                flag(
                    Rule::OutsideBoard,
                    format!("{} is off the board", describe(layout, c.item)),
                    Some(center(c)),
                );
                continue;
            }
            let near = c
                .shape
                .iter()
                .flatten()
                .flat_map(|ring| {
                    (0..ring.len()).map(move |k| (ring[k], ring[(k + 1) % ring.len()]))
                })
                .flat_map(|(a, b)| {
                    edges.iter().map(move |&(c, d)| geometry::segment_distance(a, b, c, d))
                })
                .fold(f64::INFINITY, f64::min);
            if near + 1e-6 < rules.edge_clearance.mm() {
                flag(
                    Rule::EdgeClearance,
                    format!(
                        "{}: {} from the edge (needs {})",
                        describe(layout, c.item),
                        mm(near),
                        mm(rules.edge_clearance.mm())
                    ),
                    Some(center(c)),
                );
            }
        }
    }

    // Courtyards.
    let yards = courtyards(layout);
    for i in 0..yards.len() {
        for j in i + 1..yards.len() {
            let (a, b) = (&yards[i], &yards[j]);
            if a.1 == b.1 && geometry::overlaps(&a.2, &b.2) {
                let at = geometry::bounds(&geometry::intersection(&a.2, &b.2))
                    .map(|(lo, hi)| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]);
                flag(
                    Rule::CourtyardOverlap,
                    format!("{} and {}", layout.parts[a.0].reference, layout.parts[b.0].reference),
                    at,
                );
            }
        }
        if !edge.is_empty() && geometry::area(&geometry::difference(&yards[i].2, &edge)) > 1e-3 {
            let at = geometry::bounds(&yards[i].2)
                .map(|(lo, hi)| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]);
            flag(
                Rule::OutsideBoard,
                format!("{} reaches past the board's edge", layout.parts[yards[i].0].reference),
                at,
            );
        }
    }

    // Differential pairs, once routed: their two halves the same length.
    for pair in &board.pairs {
        let routed = |n: &str| board.tracks.iter().any(|t| t.net == n);
        let (Some(max), true, true) =
            (pair.max_skew, routed(&pair.positive), routed(&pair.negative))
        else {
            continue;
        };
        let (a, b) = (
            crate::nets::routed_length(board, &pair.positive),
            crate::nets::routed_length(board, &pair.negative),
        );
        if (a - b).abs() > max.mm() + 1e-6 {
            flag(
                Rule::Skew,
                format!(
                    "{}: {} is {} and {} is {} ({} apart, {} allowed)",
                    pair.name,
                    pair.positive,
                    mm(a),
                    pair.negative,
                    mm(b),
                    mm((a - b).abs()),
                    mm(max.mm())
                ),
                None,
            );
        }
    }

    // Keepouts.
    for (n, k) in board.keepouts.iter().enumerate().filter(|(_, k)| k.outline.len() >= 3) {
        let area = vec![vec![geometry::ccw(k.outline.iter().map(|q| p(*q)).collect())]];
        let name = format!("keepout {}", n + 1);
        for t in board.tracks.iter().filter(|t| k.tracks && k.on(t.layer)) {
            if geometry::overlaps(&area, &vec![vec![geometry::track(t, 0.0)]]) {
                flag(
                    Rule::Keepout,
                    format!("track of {} on {} crosses {name}", t.net, t.layer.name()),
                    Some(p(t.a)),
                );
            }
        }
        for v in board.vias.iter().filter(|_| k.vias) {
            if geometry::overlaps(&area, &vec![vec![geometry::via(v, 0.0)]]) {
                flag(Rule::Keepout, format!("via of {} in {name}", v.net), Some(p(v.at)));
            }
        }
        if k.parts {
            for (j, part) in layout.parts.iter().enumerate() {
                let Some(place) = part.place else { continue };
                let (lo, hi) = crate::sync::extent(&part.footprint);
                let c: Vec<P> = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
                    .iter()
                    .map(|q| place.apply(*q))
                    .collect();
                if geometry::overlaps(&area, &vec![vec![geometry::ccw(c)]]) {
                    flag(
                        Rule::Keepout,
                        format!("{} is placed in {name}", layout.parts[j].reference),
                        Some(place.at),
                    );
                }
            }
        }
    }

    // Silkscreen over pads: each silk line, by part, against every pad on its side.
    for k in 0..layout.parts.len() {
        for g in layout.graphics(k) {
            let (layer, a, b, w) = match &g {
                Graphic::Line { layer, a, b, width } => (*layer, p(*a), p(*b), width.mm()),
                _ => continue,
            };
            let cu = match layer {
                Layer::FSilk => Layer::FCu,
                Layer::BSilk => Layer::BCu,
                _ => continue,
            };
            let line = vec![vec![geometry::capsule(a, b, w)]];
            if let Some(pad) = layout.pads.iter().find(|pad| {
                pad.layers.contains(&cu)
                    && geometry::overlaps(&line, &vec![vec![geometry::pad(pad, 0.0)]])
            }) {
                flag(
                    Rule::SilkOverPad,
                    format!(
                        "{}'s silkscreen crosses pad {} of {}",
                        layout.parts[k].reference, pad.number, layout.parts[pad.part].reference
                    ),
                    Some([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]),
                );
            }
        }
    }

    // Routing.
    let connectivity = connect::analyze(&copper);
    for net in &layout.nets {
        if connectivity.ratsnest.iter().any(|(n, ..)| n == net) {
            continue;
        }
        let length = crate::nets::routed_length(board, net);
        if let (Some(order), Some(class)) =
            (crate::nets::against_limits(board, net, length), rules.class(net))
        {
            let limit = match order {
                std::cmp::Ordering::Less => format!(
                    "shorter than its {} minimum",
                    mm(class.min_length.map_or(0.0, |l| l.mm()))
                ),
                _ => format!(
                    "longer than its {} maximum",
                    mm(class.max_length.map_or(0.0, |l| l.mm()))
                ),
            };
            flag(
                Rule::Length,
                format!("{net} is routed {}, {limit} (class {})", mm(length), class.name),
                None,
            );
        }
    }
    for (net, a, b) in &connectivity.ratsnest {
        flag(
            Rule::Unconnected,
            format!("{net}: ({:.2}, {:.2}) to ({:.2}, {:.2})", a[0], a[1], b[0], b[1]),
            Some(*a),
        );
    }
    out.sort_by_key(|v| v.rule);
    Checked { fills, copper, connectivity, violations: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> geometry::Contour {
        vec![[x, y], [x + s, y], [x + s, y + s], [x, y + s]]
    }

    #[test]
    fn distances_between_shapes() {
        // A pour 10 mm square with a 4 mm hole in its middle, and pads about it.
        let pour: Shapes = vec![vec![square(0.0, 0.0, 10.0), square(3.0, 3.0, 4.0)]];
        let pad = |x, y| -> Shapes { vec![vec![square(x, y, 1.0)]] };
        let close = |d: f64, want: f64| (d - want).abs() < 1e-9;
        // Outside, either way round.
        assert!(close(distance(&pour, &pad(12.0, 4.0)), 2.0));
        assert!(close(distance(&pad(12.0, 4.0), &pour), 2.0));
        // In the hole, clear of its edges.
        assert!(close(distance(&pad(4.5, 4.5), &pour), 1.5));
        // Wholly inside the copper: no edges meet, but they overlap.
        assert_eq!(distance(&pad(1.0, 1.0), &pour), 0.0);
        assert_eq!(distance(&pour, &pad(1.0, 1.0)), 0.0);
        // Across an edge, and just touching.
        assert_eq!(distance(&pad(9.5, 4.0), &pour), 0.0);
        assert_eq!(distance(&pad(10.0, 4.0), &pour), 0.0);
        // A pad round the whole pour.
        let around: Shapes = vec![vec![square(-1.0, -1.0, 12.0)]];
        assert_eq!(distance(&around, &pour), 0.0);
    }
}
