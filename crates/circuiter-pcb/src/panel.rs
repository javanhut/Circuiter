//! Panels: copies of the board made and assembled together, then broken apart (KiKit's
//! and Altium's embedded board arrays). Rows and columns of copies, rails along the
//! top and bottom for the assembly line to hold, fiducials for its camera, tooling
//! holes for its pins, and either V-scores to snap along (rectangular boards, butted
//! together) or a routed gap with tabs drilled with mouse bites (any shape). The panel
//! is itself a layout, so every fabrication file is written for it as for a board.

use crate::geometry::{self, Contour, Shapes};
use crate::layout::{Layout, P, p, point};
use crate::zones::ZoneFill;
use circuiter_model::board::{Graphic, Layer, Panel, Separation};
use circuiter_model::geom::{Length, Point};

/// Mouse bites: hole size and spacing, and how far into the board their row sits so
/// the break leaves the edge clean.
const BITE: f64 = 0.5;
const BITE_PITCH: f64 = 0.8;
const BITE_INSET: f64 = 0.25;
const TOOLING: f64 = 2.0;
/// The longest stretch of edge one tab holds.
const TAB_EVERY: f64 = 40.0;

/// A board made up into a panel.
#[derive(Debug, Clone)]
pub struct Panelized {
    /// Every copy's parts, pads, copper and drawings, with the panel's edge as the
    /// outline and what's routed out drawn on Edge.Cuts.
    pub layout: Layout,
    pub fills: Vec<ZoneFill>,
    pub size: (f64, f64),
    /// Where each copy's origin went.
    pub copies: Vec<P>,
    /// The panel's substrate: frame, copies and tabs.
    pub substrate: Shapes,
    /// V-score lines, across the whole panel.
    pub vscores: Vec<(P, P)>,
    /// Unplated holes: mouse bites and tooling (centre, diameter).
    pub holes: Vec<(P, f64)>,
    pub fiducials: Vec<P>,
}

fn shift(q: Point, o: P) -> Point {
    let v = p(q);
    point([v[0] + o[0], v[1] + o[1]])
}

fn shift_graphic(g: &Graphic, o: P) -> Graphic {
    match g.clone() {
        Graphic::Line { layer, a, b, width } => {
            Graphic::Line { layer, a: shift(a, o), b: shift(b, o), width }
        }
        Graphic::Circle { layer, center, radius, width } => {
            Graphic::Circle { layer, center: shift(center, o), radius, width }
        }
        Graphic::Poly { layer, points, width, filled } => Graphic::Poly {
            layer,
            points: points.into_iter().map(|q| shift(q, o)).collect(),
            width,
            filled,
        },
        Graphic::Text { layer, at, text, size } => {
            Graphic::Text { layer, at: shift(at, o), text, size }
        }
    }
}

fn rect(lo: P, hi: P) -> Contour {
    vec![lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
}

/// Whether `q` is inside the closed polygon `c`.
fn inside(c: &[P], q: P) -> bool {
    let mut odd = false;
    for k in 0..c.len() {
        let (a, b) = (c[k], c[(k + 1) % c.len()]);
        if (a[1] > q[1]) != (b[1] > q[1])
            && q[0] < a[0] + (q[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
        {
            odd = !odd;
        }
    }
    odd
}

/// Makes `layout` up into `panel`.
pub fn panelize(layout: &Layout, fills: &[ZoneFill], panel: &Panel) -> Result<Panelized, String> {
    let board = &layout.board;
    let outline: Contour = board.outline.iter().map(|q| p(*q)).collect();
    if outline.len() < 3 {
        return Err("the board needs an outline".into());
    }
    if panel.rows == 0 || panel.columns == 0 {
        return Err("a panel needs at least one row and column".into());
    }
    let (lo, hi) = geometry::bounds(&vec![vec![outline.clone()]]).expect("an outline");
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    let rails = panel.rails.mm().max(0.0);
    let (rows, cols) = (panel.rows as usize, panel.columns as usize);
    let gap = match panel.separation {
        Separation::VScore => {
            let area = geometry::area2(&outline).abs() / 2.0;
            if (area - w * h).abs() > 1e-3 * w * h {
                return Err(
                    "V-scores cut straight across: the board must be rectangular (use mouse bites)"
                        .into(),
                );
            }
            0.0
        }
        Separation::MouseBites { gap, .. } => gap.mm().max(0.3),
    };
    // Copies left to right, top to bottom, inside the rails (and a routed gap from them).
    let top = if rails > 0.0 { rails + gap } else { 0.0 };
    let copies: Vec<P> = (0..rows)
        .flat_map(|r| (0..cols).map(move |c| (r, c)))
        .map(|(r, c)| [c as f64 * (w + gap) - lo[0], top + r as f64 * (h + gap) - lo[1]])
        .collect();
    let width = cols as f64 * w + (cols - 1) as f64 * gap;
    let height = rows as f64 * h + (rows - 1) as f64 * gap + 2.0 * top;
    let rail_shapes: Vec<Contour> = if rails > 0.0 {
        vec![rect([0.0, 0.0], [width, rails]), rect([0.0, height - rails], [width, height])]
    } else {
        Vec::new()
    };
    let placed: Vec<Contour> = copies
        .iter()
        .map(|o| outline.iter().map(|q| [q[0] + o[0], q[1] + o[1]]).collect())
        .collect();
    let mut vscores = Vec::new();
    let mut holes = Vec::new();
    let mut tabs: Vec<Contour> = Vec::new();
    match panel.separation {
        Separation::VScore => {
            for r in 0..=rows {
                if (r > 0 && r < rows) || rails > 0.0 {
                    let y = top + r as f64 * h;
                    vscores.push(([0.0, y], [width, y]));
                }
            }
            for c in 1..cols {
                let x = c as f64 * w;
                vscores.push(([x, 0.0], [x, height]));
            }
        }
        Separation::MouseBites { tab, .. } => {
            let tab = tab.mm().max(1.0);
            let held = |k: usize, q: P| -> Option<bool> {
                // What's across the gap: another copy (true), a rail (false), or nothing.
                if placed.iter().enumerate().any(|(j, c)| j != k && inside(c, q)) {
                    Some(true)
                } else if rail_shapes.iter().any(|c| inside(c, q)) {
                    Some(false)
                } else {
                    None
                }
            };
            for (k, c) in placed.iter().enumerate() {
                for e in 0..c.len() {
                    let (a, b) = (c[e], c[(e + 1) % c.len()]);
                    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
                    if len < tab * 2.0 {
                        continue;
                    }
                    let d = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
                    // Outward: whichever side of the edge isn't the board.
                    let n = {
                        let n = [d[1], -d[0]];
                        let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                        if inside(c, [mid[0] + n[0] * 0.01, mid[1] + n[1] * 0.01]) {
                            [-n[0], -n[1]]
                        } else {
                            n
                        }
                    };
                    let count = (len / TAB_EVERY).ceil().max(1.0) as usize;
                    for t in 0..count {
                        let s = len * (t as f64 + 0.5) / count as f64;
                        let m = [a[0] + d[0] * s, a[1] + d[1] * s];
                        let across = [m[0] + n[0] * (gap + 0.05), m[1] + n[1] * (gap + 0.05)];
                        let Some(to_copy) = held(k, across) else { continue };
                        // Between copies, one tab each: from the left or upper one.
                        if to_copy && (n[0] < -0.5 || n[1] < -0.5) {
                            continue;
                        }
                        let half = tab / 2.0;
                        let at = |u: f64, v: f64| {
                            [m[0] + d[0] * u + n[0] * v, m[1] + d[1] * u + n[1] * v]
                        };
                        tabs.push(vec![
                            at(-half, -0.1),
                            at(half, -0.1),
                            at(half, gap + 0.1),
                            at(-half, gap + 0.1),
                        ]);
                        // A row of holes where the tab meets each copy.
                        let bites = ((tab - BITE) / BITE_PITCH).floor() as usize + 1;
                        let span = (bites - 1) as f64 * BITE_PITCH;
                        let mut rows_at = vec![-BITE_INSET];
                        if to_copy {
                            rows_at.push(gap + BITE_INSET);
                        }
                        for v in rows_at {
                            for i in 0..bites {
                                holes.push((at(-span / 2.0 + i as f64 * BITE_PITCH, v), BITE));
                            }
                        }
                    }
                }
            }
        }
    }
    let mut pieces: Shapes = rail_shapes.iter().map(|c| vec![c.clone()]).collect();
    pieces.extend(placed.iter().map(|c| vec![geometry::ccw(c.clone())]));
    pieces.extend(tabs.into_iter().map(|c| vec![geometry::ccw(c)]));
    let substrate = geometry::merge(pieces);
    if substrate.len() != 1 {
        return Err(format!(
            "the panel falls apart into {} pieces: the copies need rails or neighbours to hold them",
            substrate.len()
        ));
    }
    // Tooling holes in the rails' corners, fiducials beside them.
    let mut fiducials = Vec::new();
    if rails >= 4.0 && width >= 30.0 {
        let ys = [rails / 2.0, height - rails / 2.0];
        if panel.tooling {
            for y in ys {
                holes.push(([4.0, y], TOOLING));
                holes.push(([width - 4.0, y], TOOLING));
            }
        }
        if panel.fiducials {
            fiducials = vec![[10.0, ys[0]], [width - 10.0, ys[0]], [10.0, ys[1]]];
        }
    }
    // The panel as a layout: every copy of everything.
    let mut out = layout.clone();
    let b = &mut out.board;
    b.outline = substrate[0][0].iter().map(|q| point(*q)).collect();
    b.footprints.clear();
    b.tracks.clear();
    b.vias.clear();
    b.zones.clear();
    b.graphics.clear();
    b.keepouts.clear();
    b.pairs.clear();
    b.enclosure = None;
    b.panel = None;
    out.parts.clear();
    out.pads.clear();
    let mut out_fills = Vec::new();
    for o in &copies {
        let base = out.parts.len();
        let b = &mut out.board;
        b.footprints.extend(board.footprints.iter().cloned().map(|mut f| {
            f.at = shift(f.at, *o);
            f
        }));
        b.tracks.extend(board.tracks.iter().cloned().map(|mut t| {
            t.a = shift(t.a, *o);
            t.b = shift(t.b, *o);
            t
        }));
        b.vias.extend(board.vias.iter().cloned().map(|mut v| {
            v.at = shift(v.at, *o);
            v
        }));
        b.zones.extend(board.zones.iter().cloned().map(|mut z| {
            z.outline = z.outline.iter().map(|q| shift(*q, *o)).collect();
            z
        }));
        b.graphics.extend(board.graphics.iter().map(|g| shift_graphic(g, *o)));
        out.parts.extend(layout.parts.iter().cloned().map(|mut part| {
            if let Some(pl) = &mut part.place {
                pl.at = [pl.at[0] + o[0], pl.at[1] + o[1]];
            }
            part
        }));
        out.pads.extend(layout.pads.iter().cloned().map(|mut pd| {
            pd.part += base;
            pd.center = [pd.center[0] + o[0], pd.center[1] + o[1]];
            pd
        }));
        out_fills.extend(fills.iter().cloned().map(|mut f| {
            for shape in &mut f.shapes {
                for c in shape.iter_mut() {
                    for q in c.iter_mut() {
                        *q = [q[0] + o[0], q[1] + o[1]];
                    }
                }
            }
            f
        }));
    }
    // What's routed out: the substrate's holes, cut round.
    for slot in substrate[0].iter().skip(1) {
        out.board.graphics.push(Graphic::Poly {
            layer: Layer::EdgeCuts,
            points: slot.iter().map(|q| point(*q)).collect(),
            width: Length::from_mm(0.1),
            filled: false,
        });
    }
    Ok(Panelized {
        layout: out,
        fills: out_fills,
        size: (width, height),
        copies,
        substrate,
        vscores,
        holes,
        fiducials,
    })
}
