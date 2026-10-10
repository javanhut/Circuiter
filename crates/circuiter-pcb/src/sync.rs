//! Bringing a board up to date with its schematic, as KiCad's "Update PCB from
//! Schematic" does: parts new to the schematic are put on the board in free space (or
//! beside it when it's full), and footprints whose part was deleted are taken off.

use crate::footprints::Footprint;
use crate::layout::{Layout, P, point};
use circuiter_model::board::{Board, Graphic, Layer, Placed};
use circuiter_model::{CellId, Design};

/// What an update changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Update {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// Room left between parts placed automatically.
const SPACING: f64 = 1.0;
const GRID: f64 = 1.27;

/// The box a footprint needs, in its own frame: its courtyard, or its pads with a margin.
pub fn extent(fp: &Footprint) -> (P, P) {
    let mut points: Vec<P> = Vec::new();
    for g in &fp.graphics {
        match g {
            Graphic::Poly { layer: Layer::FCrtYd, points: outline, .. } => {
                points.extend(outline.iter().map(|q| [q.x.mm(), q.y.mm()]))
            }
            Graphic::Circle { layer: Layer::FCrtYd, center, radius, .. } => {
                let (c, r) = ([center.x.mm(), center.y.mm()], radius.mm());
                points.extend([[c[0] - r, c[1] - r], [c[0] + r, c[1] + r]]);
            }
            _ => {}
        }
    }
    if points.is_empty() {
        for pad in &fp.pads {
            let (c, h) =
                ([pad.at.x.mm(), pad.at.y.mm()], pad.size.0.mm().max(pad.size.1.mm()) / 2.0 + 0.5);
            points.extend([[c[0] - h, c[1] - h], [c[0] + h, c[1] + h]]);
        }
    }
    if points.is_empty() {
        return ([-1.0, -1.0], [1.0, 1.0]);
    }
    points.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), q| {
        ([lo[0].min(q[0]), lo[1].min(q[1])], [hi[0].max(q[0]), hi[1].max(q[1])])
    })
}

/// The box of a footprint placed at `at` (unturned).
fn boxed(ext: (P, P), at: P) -> (P, P) {
    ([at[0] + ext.0[0], at[1] + ext.0[1]], [at[0] + ext.1[0], at[1] + ext.1[1]])
}

fn overlap(a: (P, P), b: (P, P), gap: f64) -> bool {
    a.0[0] < b.1[0] + gap && b.0[0] < a.1[0] + gap && a.0[1] < b.1[1] + gap && b.0[1] < a.1[1] + gap
}

fn inside(poly: &[P], q: P) -> bool {
    let mut odd = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > q[1]) != (b[1] > q[1])
            && q[0] < a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
        {
            odd = !odd;
        }
    }
    odd
}

/// Updates `board` from the schematic of `top`: places parts not on it yet and, if
/// `remove_orphans`, takes off footprints whose part is gone. Tracks stay as they are.
pub fn update(
    design: &Design,
    top: &CellId,
    board: &mut Board,
    remove_orphans: bool,
) -> Result<Update, String> {
    let layout = Layout::new(design, top, board)?;
    let mut changes = Update::default();
    if remove_orphans && !layout.orphans.is_empty() {
        board.footprints.retain(|f| !layout.orphans.contains(&f.path));
        changes.removed = layout.orphans.clone();
    }
    // What's taken already: each placed part's box, turned as it is.
    let mut taken: Vec<(P, P)> = Vec::new();
    for part in &layout.parts {
        let Some(place) = part.place else { continue };
        let (lo, hi) = extent(&part.footprint);
        let corners = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]].map(|c| place.apply(c));
        let xs = corners.map(|c| c[0]);
        let ys = corners.map(|c| c[1]);
        taken.push((
            [
                xs.iter().copied().fold(f64::MAX, f64::min),
                ys.iter().copied().fold(f64::MAX, f64::min),
            ],
            [
                xs.iter().copied().fold(f64::MIN, f64::max),
                ys.iter().copied().fold(f64::MIN, f64::max),
            ],
        ));
    }
    let outline: Vec<P> = board.outline.iter().map(|q| [q.x.mm(), q.y.mm()]).collect();
    let (blo, bhi) = outline.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), q| {
        ([lo[0].min(q[0]), lo[1].min(q[1])], [hi[0].max(q[0]), hi[1].max(q[1])])
    });
    let margin = board.rules.edge_clearance.mm() + SPACING;
    // Beside the board, for what doesn't fit: a column to its right.
    let mut beside = [bhi[0].max(0.0) + 10.0, blo[1].min(0.0)];
    for path in &layout.unplaced {
        let Some(k) = layout.part(path) else { continue };
        let ext = extent(&layout.parts[k].footprint);
        let fits = |at: P| {
            let b = boxed(ext, at);
            b.0[0] >= blo[0] + margin
                && b.1[0] <= bhi[0] - margin
                && b.0[1] >= blo[1] + margin
                && b.1[1] <= bhi[1] - margin
                && [b.0, [b.1[0], b.0[1]], b.1, [b.0[0], b.1[1]]]
                    .iter()
                    .all(|&c| inside(&outline, c))
                && !taken.iter().any(|t| overlap(*t, b, SPACING))
        };
        let mut spot = None;
        if outline.len() >= 3 {
            let snap = |v: f64| (v / GRID).ceil() * GRID;
            let mut y = snap(blo[1] + margin - ext.0[1]);
            'search: while y + ext.1[1] <= bhi[1] - margin {
                let mut x = snap(blo[0] + margin - ext.0[0]);
                while x + ext.1[0] <= bhi[0] - margin {
                    if fits([x, y]) {
                        spot = Some([x, y]);
                        break 'search;
                    }
                    x += GRID;
                }
                y += GRID;
            }
        }
        let at = spot.unwrap_or_else(|| {
            let at = [beside[0] - ext.0[0], beside[1] - ext.0[1]];
            beside[1] += ext.1[1] - ext.0[1] + 2.0 * SPACING;
            at
        });
        taken.push(boxed(ext, at));
        board.footprints.push(Placed {
            path: path.clone(),
            at: point(at),
            rotation: 0.0,
            back: false,
            locked: false,
        });
        changes.added.push(path.clone());
    }
    Ok(changes)
}

/// A new two-layer board for the schematic of `top`, sized for its parts with room to
/// route, every part placed on it.
pub fn new_board(design: &Design, top: &CellId) -> Result<Board, String> {
    let layout = Layout::new(design, top, &Board::new(1.0, 1.0))?;
    let area: f64 = layout
        .parts
        .iter()
        .map(|p| {
            let (lo, hi) = extent(&p.footprint);
            (hi[0] - lo[0] + 2.0 * SPACING) * (hi[1] - lo[1] + 2.0 * SPACING)
        })
        .sum();
    let widest = layout
        .parts
        .iter()
        .map(|p| extent(&p.footprint))
        .map(|(lo, hi)| (hi[0] - lo[0]).max(hi[1] - lo[1]))
        .fold(0.0, f64::max);
    let round = |v: f64| (v / 5.0).ceil() * 5.0;
    let h = round((area * 2.5 / 1.4).sqrt().max(widest + 6.0).max(20.0));
    let mut board = Board::new(round(h * 1.4), h);
    let mut grow = 0;
    loop {
        let mut b = board.clone();
        update(design, top, &mut b, true)?;
        let outside = Layout::new(design, top, &b)?
            .parts
            .iter()
            .filter_map(|p| p.place)
            .any(|pl| pl.at[0] > b.outline[1].x.mm());
        if !outside || grow >= 8 {
            return Ok(b);
        }
        // Too tight to pack: a size larger.
        grow += 1;
        let (w, h) = (board.outline[1].x.mm() + 5.0, board.outline[2].y.mm() + 5.0);
        board = Board::new(w, h);
    }
}
