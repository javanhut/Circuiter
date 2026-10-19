//! Bringing a board up to date with its schematic, as KiCad's "Update PCB from
//! Schematic" does: parts new to the schematic are put on the board in free space (or
//! beside it when it's full), and footprints whose part was deleted are taken off.

use crate::footprints::Footprint;
use crate::layout::{Layout, P, Place, point};
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
    update_with(design, top, board, &|_| true, &|_| remove_orphans)
}

/// A change the schematic asks of the board, for the Engineering Change Order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Change {
    /// A part to place.
    Add(String),
    /// A footprint whose part is gone.
    Remove(String),
    /// Routing of a net the schematic no longer has.
    Unroute(String),
}

impl Change {
    pub fn describe(&self) -> String {
        match self {
            Change::Add(p) => format!("Add footprint {p}"),
            Change::Remove(p) => format!("Remove footprint {p} (no longer in the schematic)"),
            Change::Unroute(n) => {
                format!("Remove the tracks and vias of net {n} (no longer in the schematic)")
            }
        }
    }
}

/// What updating the board from the schematic would change, as Altium's Engineering
/// Change Order lists it before anything is done.
pub fn changes(design: &Design, top: &CellId, board: &Board) -> Result<Vec<Change>, String> {
    use std::collections::BTreeSet;
    let layout = Layout::new(design, top, board)?;
    let mut out: Vec<Change> = layout.unplaced.iter().cloned().map(Change::Add).collect();
    out.extend(layout.orphans.iter().cloned().map(Change::Remove));
    // The schematic's nets: those of every part, placed or not.
    let nets: BTreeSet<&str> =
        layout.parts.iter().flat_map(|p| p.nets.values().map(String::as_str)).collect();
    let routed: BTreeSet<&str> = board
        .tracks
        .iter()
        .map(|t| t.net.as_str())
        .chain(board.vias.iter().map(|v| v.net.as_str()))
        .collect();
    out.extend(
        routed.into_iter().filter(|n| !nets.contains(n)).map(|n| Change::Unroute(n.to_owned())),
    );
    Ok(out)
}

/// Makes the `chosen` changes.
pub fn apply(
    design: &Design,
    top: &CellId,
    board: &mut Board,
    chosen: &[Change],
) -> Result<Update, String> {
    for c in chosen {
        if let Change::Unroute(n) = c {
            board.tracks.retain(|t| t.net != *n);
            board.vias.retain(|v| v.net != *n);
        }
    }
    let pick = |want: fn(&Change) -> Option<&String>| -> Vec<String> {
        chosen.iter().filter_map(want).cloned().collect()
    };
    let add = pick(|c| if let Change::Add(p) = c { Some(p) } else { None });
    let remove = pick(|c| if let Change::Remove(p) = c { Some(p) } else { None });
    update_with(design, top, board, &|p| add.iter().any(|a| a == p), &|p| {
        remove.iter().any(|r| r == p)
    })
}

/// [`update`], placing only the parts `place` allows and removing only the orphans
/// `remove` allows.
fn update_with(
    design: &Design,
    top: &CellId,
    board: &mut Board,
    place: &dyn Fn(&str) -> bool,
    remove: &dyn Fn(&str) -> bool,
) -> Result<Update, String> {
    let layout = Layout::new(design, top, board)?;
    let mut changes = Update::default();
    let gone: Vec<String> = layout.orphans.iter().filter(|p| remove(p)).cloned().collect();
    if !gone.is_empty() {
        board.footprints.retain(|f| !gone.contains(&f.path));
        changes.removed = gone;
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
    let outline: Vec<P> = board.edge().iter().map(|q| [q.x.mm(), q.y.mm()]).collect();
    let (blo, bhi) = outline.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), q| {
        ([lo[0].min(q[0]), lo[1].min(q[1])], [hi[0].max(q[0]), hi[1].max(q[1])])
    });
    let margin = board.rules.edge_clearance.mm() + SPACING;
    // Beside the board, for what doesn't fit: a column to its right.
    let mut beside = [bhi[0].max(0.0) + 10.0, blo[1].min(0.0)];
    for path in layout.unplaced.iter().filter(|p| place(p)) {
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

/// The total half-perimeter of each net's pads: the usual measure of how much routing
/// a placement asks for.
pub fn wire_length(layout: &Layout) -> f64 {
    let mut boxes: std::collections::BTreeMap<&str, (P, P)> = std::collections::BTreeMap::new();
    for pad in &layout.pads {
        let Some(net) = pad.net.as_deref() else { continue };
        let b = boxes.entry(net).or_insert((pad.center, pad.center));
        b.0 = [b.0[0].min(pad.center[0]), b.0[1].min(pad.center[1])];
        b.1 = [b.1[0].max(pad.center[0]), b.1[1].max(pad.center[1])];
    }
    boxes.values().map(|(lo, hi)| (hi[0] - lo[0]) + (hi[1] - lo[1])).sum()
}

/// Rearranges the board's unlocked parts to shorten their connections: each part is
/// tried at the middle of what it connects to and around it, turned four ways, and
/// pairs of parts swapped, keeping every move that shortens the total half-perimeter
/// wire length without overlapping another part or leaving the board. Returns the
/// wire length before and after.
pub fn optimize(design: &Design, top: &CellId, board: &mut Board) -> Result<(f64, f64), String> {
    let layout = Layout::new(design, top, board)?;
    let before = wire_length(&layout);
    let outline: Vec<P> = board.edge().iter().map(|q| [q.x.mm(), q.y.mm()]).collect();
    let (blo, bhi) = outline.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), q| {
        ([lo[0].min(q[0]), lo[1].min(q[1])], [hi[0].max(q[0]), hi[1].max(q[1])])
    });
    let margin = board.rules.edge_clearance.mm() + SPACING;
    // Each placed part: its box and its pads (local position, net).
    struct Item {
        path: String,
        ext: (P, P),
        pads: Vec<(P, Option<String>)>,
        place: Place,
        locked: bool,
    }
    let mut items: Vec<Item> = layout
        .parts
        .iter()
        .enumerate()
        .filter_map(|(k, part)| {
            let place = part.place?;
            let pads =
                layout.pads_of(k).map(|pad| (place.unapply(pad.center), pad.net.clone())).collect();
            Some(Item {
                path: part.path.clone(),
                ext: extent(&part.footprint),
                pads,
                place,
                locked: part.locked,
            })
        })
        .collect();
    let world_box = |it: &Item, place: &Place| -> (P, P) {
        let (lo, hi) = it.ext;
        let c = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]].map(|q| place.apply(q));
        (
            [
                c.iter().map(|q| q[0]).fold(f64::MAX, f64::min),
                c.iter().map(|q| q[1]).fold(f64::MAX, f64::min),
            ],
            [
                c.iter().map(|q| q[0]).fold(f64::MIN, f64::max),
                c.iter().map(|q| q[1]).fold(f64::MIN, f64::max),
            ],
        )
    };
    let cost = |items: &[Item]| -> f64 {
        let mut boxes: std::collections::BTreeMap<&str, (P, P)> = std::collections::BTreeMap::new();
        for it in items {
            for (q, net) in &it.pads {
                let Some(net) = net.as_deref() else { continue };
                let w = it.place.apply(*q);
                let b = boxes.entry(net).or_insert((w, w));
                b.0 = [b.0[0].min(w[0]), b.0[1].min(w[1])];
                b.1 = [b.1[0].max(w[0]), b.1[1].max(w[1])];
            }
        }
        boxes.values().map(|(lo, hi)| (hi[0] - lo[0]) + (hi[1] - lo[1])).sum()
    };
    let fits = |items: &[Item], k: usize, place: &Place| -> bool {
        let b = world_box(&items[k], place);
        b.0[0] >= blo[0] + margin
            && b.1[0] <= bhi[0] - margin
            && b.0[1] >= blo[1] + margin
            && b.1[1] <= bhi[1] - margin
            && [b.0, [b.1[0], b.0[1]], b.1, [b.0[0], b.1[1]]].iter().all(|&c| inside(&outline, c))
            && items
                .iter()
                .enumerate()
                .all(|(j, o)| j == k || !overlap(world_box(o, &o.place), b, SPACING))
    };
    let snap = |v: f64| (v / GRID).round() * GRID;
    let mut best = cost(&items);
    for _pass in 0..40 {
        let mut improved = false;
        for k in 0..items.len() {
            if items[k].locked {
                continue;
            }
            // Where its connections pull it: the middle of the pads it shares nets with.
            let nets: Vec<&str> = items[k].pads.iter().filter_map(|(_, n)| n.as_deref()).collect();
            let mut pull = [0.0, 0.0];
            let mut count = 0.0;
            for (j, o) in items.iter().enumerate() {
                if j == k {
                    continue;
                }
                for (q, n) in &o.pads {
                    if n.as_deref().is_some_and(|n| nets.contains(&n)) {
                        let w = o.place.apply(*q);
                        pull = [pull[0] + w[0], pull[1] + w[1]];
                        count += 1.0;
                    }
                }
            }
            let at = items[k].place.at;
            let mut spots = vec![at];
            if count > 0.0 {
                let c = [pull[0] / count, pull[1] / count];
                for t in [1.0, 0.75, 0.5, 0.25] {
                    spots
                        .push([snap(at[0] + (c[0] - at[0]) * t), snap(at[1] + (c[1] - at[1]) * t)]);
                }
                // And the neighbourhood of where it's pulled, in case that's taken.
                let step = 2.0 * GRID;
                for i in -6..=6 {
                    for j in -6..=6 {
                        spots.push([
                            snap(c[0] + f64::from(i) * step),
                            snap(c[1] + f64::from(j) * step),
                        ]);
                    }
                }
            }
            for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
                spots.push([snap(at[0] + dx * 2.0 * GRID), snap(at[1] + dy * 2.0 * GRID)]);
            }
            let old = items[k].place;
            for spot in spots {
                for rotation in [0.0, 90.0, 180.0, 270.0] {
                    let place = Place { at: spot, rotation, back: old.back };
                    if place == items[k].place || !fits(&items, k, &place) {
                        continue;
                    }
                    let keep = items[k].place;
                    items[k].place = place;
                    let c = cost(&items);
                    if c < best - 1e-6 {
                        best = c;
                        improved = true;
                    } else {
                        items[k].place = keep;
                    }
                }
            }
            // Swaps with parts of the same size.
            for j in 0..items.len() {
                if j == k || items[j].locked {
                    continue;
                }
                let same = |a: (P, P), b: (P, P)| {
                    ((a.1[0] - a.0[0]) - (b.1[0] - b.0[0])).abs() < 1e-6
                        && ((a.1[1] - a.0[1]) - (b.1[1] - b.0[1])).abs() < 1e-6
                };
                if !same(items[k].ext, items[j].ext) {
                    continue;
                }
                let (pk, pj) = (items[k].place, items[j].place);
                items[k].place = pj;
                items[j].place = pk;
                let ok = fits(&items, k, &pj) && fits(&items, j, &pk);
                let c = cost(&items);
                if ok && c < best - 1e-6 {
                    best = c;
                    improved = true;
                } else {
                    items[k].place = pk;
                    items[j].place = pj;
                }
            }
        }
        if !improved {
            break;
        }
    }
    for it in &items {
        if let Some(f) = board.footprints.iter_mut().find(|f| f.path == it.path) {
            f.at = point(it.place.at);
            f.rotation = it.place.rotation;
        }
    }
    let after = wire_length(&Layout::new(design, top, board)?);
    Ok((before, after))
}
