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
    Unconnected,
    Outline,
    Unplaced,
    Orphan,
    Footprint,
}

impl Rule {
    pub fn severity(self) -> Severity {
        match self {
            Rule::CourtyardOverlap | Rule::Unplaced | Rule::Orphan => Severity::Warning,
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
    if geometry::overlaps(a, b) {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    for ca in a.iter().flatten() {
        for cb in b.iter().flatten() {
            for i in 0..ca.len() {
                let (a0, a1) = (ca[i], ca[(i + 1) % ca.len()]);
                for j in 0..cb.len() {
                    best =
                        best.min(geometry::segment_distance(a0, a1, cb[j], cb[(j + 1) % cb.len()]));
                }
            }
        }
    }
    best
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
pub fn check(layout: &Layout) -> Checked {
    let board = &layout.board;
    let rules = &board.rules;
    let mm = |v: f64| format!("{v:.3} mm");
    let mut out = Vec::new();
    let mut flag =
        |rule: Rule, message: String, at: Option<P>| out.push(Violation { rule, message, at });

    // The outline.
    let outline: Vec<P> = board.outline.iter().map(|q| p(*q)).collect();
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
    let edges: Vec<(P, P)> =
        (0..outline.len()).map(|k| (outline[k], outline[(k + 1) % outline.len()])).collect();
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

    // Routing.
    let connectivity = connect::analyze(&copper);
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
