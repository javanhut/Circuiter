//! Rearranging what's routed, as pcbnew does: drag a track's segment sideways and its
//! neighbours stretch to meet it at the same angles; drag a corner or a via and the
//! tracks meeting there follow; move, turn or flip a part and the tracks ending on its
//! pads come with it. And tidying up: segments in a line joined into one, stubs and
//! duplicates taken off.

use crate::layout::{Layout, P, Place, p, point};
use circuiter_model::board::{Board, Layer, Placed, Track};

const EPS: f64 = 1e-6;

fn near(a: P, b: P) -> bool {
    (a[0] - b[0]).abs() < EPS && (a[1] - b[1]).abs() < EPS
}

/// Where the lines through `a`–`b` and `c`–`d` cross, if they aren't parallel.
fn cross(a: P, b: P, c: P, d: P) -> Option<P> {
    let (r, s) = ([b[0] - a[0], b[1] - a[1]], [d[0] - c[0], d[1] - c[1]]);
    let den = r[0] * s[1] - r[1] * s[0];
    if den.abs() < 1e-12 {
        return None;
    }
    let t = ((c[0] - a[0]) * s[1] - (c[1] - a[1]) * s[0]) / den;
    Some([a[0] + t * r[0], a[1] + t * r[1]])
}

/// The other end of the one track (other than `except`) on `layer` ending at `at`.
fn neighbour(board: &Board, except: usize, layer: Layer, at: P) -> Option<(usize, bool)> {
    let found: Vec<(usize, bool)> = board
        .tracks
        .iter()
        .enumerate()
        .filter(|(k, t)| *k != except && t.layer == layer)
        .filter_map(|(k, t)| {
            if near(p(t.a), at) {
                Some((k, true))
            } else if near(p(t.b), at) {
                Some((k, false))
            } else {
                None
            }
        })
        .collect();
    (found.len() == 1).then(|| found[0])
}

/// Drags segment `k` by `delta`, keeping its direction: it moves square to itself, and
/// each neighbour meeting it at a corner keeps its own direction, lengthening or
/// shortening to reach the new corner. An end with no neighbour to slide along (on a
/// pad, a via or a junction) gets a new leg to stay joined.
pub fn drag_segment(board: &mut Board, k: usize, delta: P) {
    let Some(t) = board.tracks.get(k).cloned() else { return };
    let (a, b) = (p(t.a), p(t.b));
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = dx.hypot(dy);
    if len < EPS {
        return;
    }
    // Only the part of the move square to the segment.
    let n = [-dy / len, dx / len];
    let along = delta[0] * n[0] + delta[1] * n[1];
    let shift = [n[0] * along, n[1] * along];
    if along.abs() < EPS {
        return;
    }
    let (a2, b2) = ([a[0] + shift[0], a[1] + shift[1]], [b[0] + shift[0], b[1] + shift[1]]);
    let mut new_a = a2;
    let mut new_b = b2;
    let mut legs = Vec::new();
    for (end, moved, out) in [(a, a2, &mut new_a), (b, b2, &mut new_b)] {
        match neighbour(board, k, t.layer, end) {
            Some((j, at_a)) => {
                let o = &board.tracks[j];
                let far = if at_a { p(o.b) } else { p(o.a) };
                // Where the neighbour's line meets the moved segment's.
                match cross(far, end, a2, b2) {
                    Some(c) => {
                        *out = c;
                        let o = &mut board.tracks[j];
                        if at_a {
                            o.a = point(c);
                        } else {
                            o.b = point(c);
                        }
                    }
                    None => legs.push((end, moved)),
                }
            }
            None => legs.push((end, moved)),
        }
    }
    let t = &mut board.tracks[k];
    t.a = point(new_a);
    t.b = point(new_b);
    let template = t.clone();
    for (from, to) in legs {
        board.tracks.push(Track { a: point(from), b: point(to), ..template.clone() });
    }
}

/// Moves every track end on `layer` at `from` (a corner) to `to`.
pub fn move_corner(board: &mut Board, layer: Layer, from: P, to: P) {
    for t in board.tracks.iter_mut().filter(|t| t.layer == layer) {
        if near(p(t.a), from) {
            t.a = point(to);
        }
        if near(p(t.b), from) {
            t.b = point(to);
        }
    }
}

/// Moves via `k` to `to`, and the track ends on it with it.
pub fn move_via(board: &mut Board, k: usize, to: P) {
    let Some(v) = board.vias.get_mut(k) else { return };
    let from = p(v.at);
    v.at = point(to);
    for t in &mut board.tracks {
        if near(p(t.a), from) {
            t.a = point(to);
        }
        if near(p(t.b), from) {
            t.b = point(to);
        }
    }
}

/// Puts footprint `path` at `placed`, and brings the track ends on its pads with it:
/// each end on a pad's centre goes where that pad goes.
pub fn move_footprint(board: &mut Board, layout: &Layout, path: &str, placed: Placed) {
    let Some(k) = layout.part(path) else { return };
    let part = &layout.parts[k];
    if let Some(old) = part.place {
        let new = Place::of(&placed);
        for pad in &part.footprint.pads {
            let local = p(pad.at);
            let (from, to) = (old.apply(local), new.apply(local));
            for t in &mut board.tracks {
                if near(p(t.a), from) {
                    t.a = point(to);
                }
                if near(p(t.b), from) {
                    t.b = point(to);
                }
            }
        }
    }
    if let Some(f) = board.footprints.iter_mut().find(|f| f.path == path) {
        *f = placed;
    }
}

/// Tidies the tracks: drops ones of no length and duplicates, joins segments that
/// carry straight on through a corner nothing else meets, and takes off stubs that end
/// on nothing. Returns how many tracks fewer there are.
pub fn clean_up(board: &mut Board, layout: &Layout) -> usize {
    let before = board.tracks.len();
    board.tracks.retain(|t| !near(p(t.a), p(t.b)));
    // Duplicates, either way round.
    let mut seen: Vec<Track> = Vec::new();
    board.tracks.retain(|t| {
        let same = |o: &Track| {
            o.layer == t.layer
                && o.net == t.net
                && ((near(p(o.a), p(t.a)) && near(p(o.b), p(t.b)))
                    || (near(p(o.a), p(t.b)) && near(p(o.b), p(t.a))))
        };
        if seen.iter().any(same) {
            false
        } else {
            seen.push(t.clone());
            true
        }
    });
    // What holds a track end: a pad, a via, or another track's end or side.
    let held = |board: &Board, k: usize, q: P| -> bool {
        let t = &board.tracks[k];
        layout.pads.iter().any(|pad| {
            pad.layers.contains(&t.layer)
                && (pad.center[0] - q[0]).hypot(pad.center[1] - q[1])
                    < pad.size.0.min(pad.size.1) / 2.0
        }) || board
            .vias
            .iter()
            .any(|v| (p(v.at)[0] - q[0]).hypot(p(v.at)[1] - q[1]) < v.diameter.mm() / 2.0)
            || board.tracks.iter().enumerate().any(|(j, o)| {
                j != k
                    && o.layer == t.layer
                    && o.net == t.net
                    && crate::geometry::segment_distance(p(o.a), p(o.b), q, q) < o.width.mm() / 2.0
            })
    };
    // Stubs, until there are none (taking one may leave another).
    loop {
        let stub = (0..board.tracks.len()).find(|&k| {
            let t = &board.tracks[k];
            !held(board, k, p(t.a)) || !held(board, k, p(t.b))
        });
        match stub {
            Some(k) => drop(board.tracks.remove(k)),
            None => break,
        }
    }
    // Straight on through a corner where only the two meet and nothing else is.
    loop {
        let mut joined = false;
        'outer: for i in 0..board.tracks.len() {
            for j in 0..board.tracks.len() {
                if i == j {
                    continue;
                }
                let (s, t) = (&board.tracks[i], &board.tracks[j]);
                if s.layer != t.layer || s.net != t.net || (s.width.mm() - t.width.mm()).abs() > EPS
                {
                    continue;
                }
                // s ends where t starts (either way round).
                for (sa, sb) in [(p(s.a), p(s.b)), (p(s.b), p(s.a))] {
                    for (ta, tb) in [(p(t.a), p(t.b)), (p(t.b), p(t.a))] {
                        if !near(sb, ta) {
                            continue;
                        }
                        let straight = ((sb[0] - sa[0]) * (tb[1] - ta[1])
                            - (sb[1] - sa[1]) * (tb[0] - ta[0]))
                            .abs()
                            < 1e-9
                            && (sb[0] - sa[0]) * (tb[0] - ta[0])
                                + (sb[1] - sa[1]) * (tb[1] - ta[1])
                                > 0.0;
                        let others = board
                            .tracks
                            .iter()
                            .enumerate()
                            .filter(|(k, o)| {
                                *k != i
                                    && *k != j
                                    && o.layer == s.layer
                                    && (near(p(o.a), sb) || near(p(o.b), sb))
                            })
                            .count();
                        let on_pad = layout.pads.iter().any(|pad| near(pad.center, sb))
                            || board.vias.iter().any(|v| near(p(v.at), sb));
                        if straight && others == 0 && !on_pad {
                            let merged = Track { a: point(sa), b: point(tb), ..s.clone() };
                            let (hi, lo) = (i.max(j), i.min(j));
                            board.tracks.remove(hi);
                            board.tracks.remove(lo);
                            board.tracks.push(merged);
                            joined = true;
                            break 'outer;
                        }
                    }
                }
            }
        }
        if !joined {
            break;
        }
    }
    before - board.tracks.len()
}

/// Loop removal, as Altium's router does it: when a new route (tracks `fresh`) makes
/// another way between two points of `net`, the old tracks it duplicates come off. A
/// track is redundant when both its ends are joined to the rest of the net's copper by
/// other means; tracks in `fresh` are kept. Returns how many were taken off.
pub fn remove_loops(board: &mut Board, layout: &Layout, net: &str, fresh: &[Track]) -> usize {
    let mut removed = 0;
    loop {
        let candidates: Vec<usize> = (0..board.tracks.len())
            .filter(|&k| board.tracks[k].net == net && !fresh.contains(&board.tracks[k]))
            .collect();
        let mut gone = None;
        for k in candidates {
            if redundant(board, layout, k) {
                gone = Some(k);
                break;
            }
        }
        match gone {
            Some(k) => {
                let t = board.tracks.remove(k);
                removed += 1;
                // The rest of the old way, left hanging, comes off with it.
                let mut freed = vec![(t.layer, p(t.a)), (t.layer, p(t.b))];
                while let Some((layer, q)) = freed.pop() {
                    let hanging = (0..board.tracks.len()).find(|&j| {
                        let o = &board.tracks[j];
                        o.net == net
                            && o.layer == layer
                            && !fresh.contains(o)
                            && [p(o.a), p(o.b)].iter().any(|e| near(*e, q))
                            && !attached(board, layout, j, q)
                    });
                    if let Some(j) = hanging {
                        let o = board.tracks.remove(j);
                        removed += 1;
                        let other = if near(p(o.a), q) { p(o.b) } else { p(o.a) };
                        freed.push((o.layer, other));
                    }
                }
            }
            None => return removed,
        }
    }
}

/// Whether the end of track `k` at `q` touches other copper of its net: a pad, a via
/// or another track.
fn attached(board: &Board, layout: &Layout, k: usize, q: P) -> bool {
    let t = &board.tracks[k];
    layout.pads.iter().any(|pad| {
        pad.net.as_deref() == Some(t.net.as_str())
            && pad.layers.contains(&t.layer)
            && crate::geometry::overlaps(
                &vec![vec![crate::geometry::pad(pad, 0.0)]],
                &vec![vec![crate::geometry::circle(q, 0.01)]],
            )
    }) || board.vias.iter().any(|v| {
        v.net == t.net && (p(v.at)[0] - q[0]).hypot(p(v.at)[1] - q[1]) <= v.diameter.mm() / 2.0
    }) || board.tracks.iter().enumerate().any(|(j, o)| {
        j != k
            && o.net == t.net
            && o.layer == t.layer
            && crate::geometry::segment_distance(p(o.a), p(o.b), q, q) <= o.width.mm() / 2.0
    })
}

/// Whether track `k`'s two ends are joined through the rest of its net's copper.
fn redundant(board: &Board, layout: &Layout, k: usize) -> bool {
    let t = &board.tracks[k];
    // The net's copper other than this track: pads, vias, tracks, as nodes.
    #[derive(Clone, Copy)]
    enum Node {
        Pad(usize),
        Via(usize),
        Track(usize),
    }
    let mut nodes = Vec::new();
    for (j, pad) in layout.pads.iter().enumerate() {
        if pad.net.as_deref() == Some(t.net.as_str()) {
            nodes.push(Node::Pad(j));
        }
    }
    for (j, v) in board.vias.iter().enumerate() {
        if v.net == t.net {
            nodes.push(Node::Via(j));
        }
    }
    for (j, o) in board.tracks.iter().enumerate() {
        if j != k && o.net == t.net {
            nodes.push(Node::Track(j));
        }
    }
    // Whether a node's copper covers point `q` on `layer`.
    let covers = |n: Node, q: P, layer: Layer| -> bool {
        match n {
            Node::Pad(j) => {
                let pad = &layout.pads[j];
                pad.layers.contains(&layer)
                    && crate::geometry::overlaps(
                        &vec![vec![crate::geometry::pad(pad, 0.0)]],
                        &vec![vec![crate::geometry::circle(q, 0.01)]],
                    )
            }
            Node::Via(j) => {
                let v = &board.vias[j];
                (p(v.at)[0] - q[0]).hypot(p(v.at)[1] - q[1]) <= v.diameter.mm() / 2.0
            }
            Node::Track(j) => {
                let o = &board.tracks[j];
                o.layer == layer
                    && crate::geometry::segment_distance(p(o.a), p(o.b), q, q) <= o.width.mm() / 2.0
            }
        }
    };
    // Whether two nodes touch.
    let touch = |a: Node, b: Node| -> bool {
        match (a, b) {
            (Node::Track(i), other) | (other, Node::Track(i)) => {
                let o = &board.tracks[i];
                [p(o.a), p(o.b)].iter().any(|&q| covers(other, q, o.layer))
                    || match other {
                        Node::Track(j) => {
                            let w = &board.tracks[j];
                            w.layer == o.layer
                                && [p(w.a), p(w.b)]
                                    .iter()
                                    .any(|&q| covers(Node::Track(i), q, o.layer))
                        }
                        _ => false,
                    }
            }
            (Node::Via(i), Node::Pad(j)) | (Node::Pad(j), Node::Via(i)) => {
                let v = &board.vias[i];
                layout.pads[j].layers.iter().any(|l| covers(Node::Pad(j), p(v.at), *l))
            }
            (Node::Via(i), Node::Via(j)) => {
                let (a, b) = (&board.vias[i], &board.vias[j]);
                (p(a.at)[0] - p(b.at)[0]).hypot(p(a.at)[1] - p(b.at)[1])
                    < (a.diameter.mm() + b.diameter.mm()) / 2.0
            }
            (Node::Pad(_), Node::Pad(_)) => false,
        }
    };
    let n = nodes.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for i in 0..n {
        for j in i + 1..n {
            if touch(nodes[i], nodes[j]) {
                let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                parent[a] = b;
            }
        }
    }
    let ends = |q: P| -> Vec<usize> { (0..n).filter(|&i| covers(nodes[i], q, t.layer)).collect() };
    let (ea, eb) = (ends(p(t.a)), ends(p(t.b)));
    ea.iter()
        .any(|&i| eb.iter().any(|&j| find(&mut parent.clone(), i) == find(&mut parent.clone(), j)))
}

/// The points of an accordion along the x axis from 0 to `len`: `bumps` square bumps
/// `amplitude` high and `space` wide, starting and ending `space` in, every corner
/// mitered (Altium's mitered-lines style).
fn accordion(len: f64, bumps: usize, amplitude: f64, space: f64) -> Vec<P> {
    let m = space.min(amplitude) * 0.25;
    let start = (len - (2 * bumps) as f64 * space) / 2.0;
    let mut pts = vec![[0.0, 0.0]];
    for k in 0..bumps {
        let x0 = start + (2 * k) as f64 * space;
        pts.extend([
            [x0 - m, 0.0],
            [x0, m],
            [x0, amplitude - m],
            [x0 + m, amplitude],
            [x0 + space - m, amplitude],
            [x0 + space, amplitude - m],
            [x0 + space, m],
            [x0 + space + m, 0.0],
        ]);
    }
    pts.push([len, 0.0]);
    pts.dedup_by(|a, b| near(*a, *b));
    pts
}

fn polyline_length(pts: &[P]) -> f64 {
    pts.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum()
}

/// Lengthens track `k` by `extra` mm with an accordion along it, as Altium's
/// interactive length tuning does: bumps `space` apart, no taller than
/// `max_amplitude`, on whichever side `clear` (asked of each new segment's points)
/// finds room. Returns the length added.
pub fn tune_length(
    board: &mut Board,
    k: usize,
    extra: f64,
    max_amplitude: f64,
    space: f64,
    clear: &mut dyn FnMut(&[P]) -> bool,
) -> Result<f64, String> {
    let t = board.tracks.get(k).cloned().ok_or("no such track")?;
    if extra <= 1e-6 {
        return Err("it's already long enough".into());
    }
    let (a, b) = (p(t.a), p(t.b));
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let space = space.max(t.width.mm() * 2.0);
    let bumps = ((len - 2.0 * space) / (2.0 * space)).floor();
    if bumps < 1.0 {
        return Err(format!(
            "the segment is too short to tune: it needs at least {:.2} mm",
            4.0 * space
        ));
    }
    let bumps = bumps as usize;
    // The amplitude that adds `extra`, by bisection.
    let added = |amp: f64| polyline_length(&accordion(len, bumps, amp, space)) - len;
    if added(max_amplitude) < extra {
        return Err(format!(
            "{} mm more needs bumps taller than {:.2} mm here: tune a longer segment, or allow more",
            (extra * 100.0).round() / 100.0,
            max_amplitude
        ));
    }
    let (mut lo, mut hi) = (0.0, max_amplitude);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if added(mid) < extra {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let local = accordion(len, bumps, hi, space);
    let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
    for side in [1.0, -1.0] {
        let n = [-u[1] * side, u[0] * side];
        let pts: Vec<P> = local
            .iter()
            .map(|q| [a[0] + u[0] * q[0] + n[0] * q[1], a[1] + u[1] * q[0] + n[1] * q[1]])
            .collect();
        if clear(&pts) {
            board.tracks.remove(k);
            for w in pts.windows(2) {
                board.tracks.push(Track { a: point(w[0]), b: point(w[1]), ..t.clone() });
            }
            return Ok(polyline_length(&pts) - len);
        }
    }
    Err("there's no room either side of the segment for the bumps".into())
}

/// How to line parts up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Top,
    Bottom,
    /// Centres on one vertical line.
    CentreX,
    /// Centres on one horizontal line.
    CentreY,
    /// Evenly spaced across, the outermost staying put.
    SpreadX,
    /// Evenly spaced down.
    SpreadY,
}

/// The box a placed part takes up on the board.
pub fn part_box(layout: &Layout, k: usize) -> Option<(P, P)> {
    let part = &layout.parts[k];
    let place = part.place?;
    let (lo, hi) = crate::sync::extent(&part.footprint);
    let c = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]].map(|q| place.apply(q));
    Some((
        [
            c.iter().map(|q| q[0]).fold(f64::MAX, f64::min),
            c.iter().map(|q| q[1]).fold(f64::MAX, f64::min),
        ],
        [
            c.iter().map(|q| q[0]).fold(f64::MIN, f64::max),
            c.iter().map(|q| q[1]).fold(f64::MIN, f64::max),
        ],
    ))
}

/// Lines up the parts `paths` by their boxes, as Altium's Align commands do: edges to
/// the outermost one's, centres to their middle, or spread evenly between the two
/// outermost. Their tracks come with them; locked parts stay.
pub fn align(board: &mut Board, layout: &Layout, paths: &[String], how: Align) {
    let items: Vec<(String, (P, P))> = paths
        .iter()
        .filter(|p| !board.placed(p).is_some_and(|f| f.locked))
        .filter_map(|p| Some((p.clone(), part_box(layout, layout.part(p)?)?)))
        .collect();
    if items.len() < 2 {
        return;
    }
    let lo = |i: usize| items.iter().map(|(_, b)| b.0[i]).fold(f64::MAX, f64::min);
    let hi = |i: usize| items.iter().map(|(_, b)| b.1[i]).fold(f64::MIN, f64::max);
    let mid = |b: &(P, P), i: usize| (b.0[i] + b.1[i]) / 2.0;
    let mut moves: Vec<(String, P)> = Vec::new();
    match how {
        Align::Left => {
            moves = items.iter().map(|(p, b)| (p.clone(), [lo(0) - b.0[0], 0.0])).collect()
        }
        Align::Right => {
            moves = items.iter().map(|(p, b)| (p.clone(), [hi(0) - b.1[0], 0.0])).collect()
        }
        Align::Top => {
            moves = items.iter().map(|(p, b)| (p.clone(), [0.0, lo(1) - b.0[1]])).collect()
        }
        Align::Bottom => {
            moves = items.iter().map(|(p, b)| (p.clone(), [0.0, hi(1) - b.1[1]])).collect()
        }
        Align::CentreX => {
            let c = items.iter().map(|(_, b)| mid(b, 0)).sum::<f64>() / items.len() as f64;
            moves = items.iter().map(|(p, b)| (p.clone(), [c - mid(b, 0), 0.0])).collect();
        }
        Align::CentreY => {
            let c = items.iter().map(|(_, b)| mid(b, 1)).sum::<f64>() / items.len() as f64;
            moves = items.iter().map(|(p, b)| (p.clone(), [0.0, c - mid(b, 1)])).collect();
        }
        Align::SpreadX | Align::SpreadY => {
            let i = usize::from(how == Align::SpreadY);
            let mut sorted = items.clone();
            sorted.sort_by(|a, b| mid(&a.1, i).total_cmp(&mid(&b.1, i)));
            let (first, last) = (mid(&sorted[0].1, i), mid(&sorted[sorted.len() - 1].1, i));
            let step = (last - first) / (sorted.len() - 1) as f64;
            for (k, (p, b)) in sorted.iter().enumerate() {
                let mut d = [0.0, 0.0];
                d[i] = first + step * k as f64 - mid(b, i);
                moves.push((p.clone(), d));
            }
        }
    }
    for (path, d) in moves {
        if d[0].abs() < 1e-9 && d[1].abs() < 1e-9 {
            continue;
        }
        if let Some(f) = board.placed(&path).cloned() {
            let mut moved = f.clone();
            moved.at = point([p(f.at)[0] + d[0], p(f.at)[1] + d[1]]);
            move_footprint(board, layout, &path, moved);
        }
    }
}

/// Takes off every track and via of `net` (all nets when `None`).
pub fn unroute(board: &mut Board, net: Option<&str>) {
    board.tracks.retain(|t| net.is_some_and(|n| t.net != n));
    board.vias.retain(|v| net.is_some_and(|n| v.net != n));
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::geom::Length;

    fn track(a: P, b: P) -> Track {
        Track {
            layer: Layer::FCu,
            a: point(a),
            b: point(b),
            width: Length::from_mm(0.25),
            net: "n".into(),
        }
    }

    fn ends(b: &Board) -> Vec<(P, P)> {
        b.tracks.iter().map(|t| (p(t.a), p(t.b))).collect()
    }

    #[test]
    fn a_dragged_segment_keeps_its_neighbours_angles() {
        // A U: up, across, down. Drag the top 3 mm higher.
        let mut b = Board::new(50.0, 50.0);
        b.tracks = vec![
            track([0.0, 10.0], [0.0, 5.0]),
            track([0.0, 5.0], [10.0, 5.0]),
            track([10.0, 5.0], [10.0, 10.0]),
        ];
        drag_segment(&mut b, 1, [4.0, -3.0]);
        assert_eq!(
            ends(&b),
            vec![([0.0, 10.0], [0.0, 2.0]), ([0.0, 2.0], [10.0, 2.0]), ([10.0, 2.0], [10.0, 10.0])]
        );
        // A diagonal neighbour stays diagonal.
        let mut b = Board::new(50.0, 50.0);
        b.tracks = vec![track([0.0, 10.0], [5.0, 5.0]), track([5.0, 5.0], [15.0, 5.0])];
        drag_segment(&mut b, 1, [0.0, -2.0]);
        let e = ends(&b);
        assert!((e[0].1[0] - 7.0).abs() < 1e-9 && (e[0].1[1] - 3.0).abs() < 1e-9, "{e:?}");
        // The free end, with nothing to slide along, gets a leg.
        assert_eq!(b.tracks.len(), 3);
        assert_eq!(e[2], ([15.0, 5.0], [15.0, 3.0]));
    }

    #[test]
    fn loops_lose_their_old_way() {
        // Two ways from (0,0) to (10,0): straight, and up and over. The new straight one
        // stays; the old way round comes off; a stub that goes nowhere stays.
        let mut b = Board::new(50.0, 50.0);
        let old = vec![
            track([0.0, 0.0], [0.0, -5.0]),
            track([0.0, -5.0], [10.0, -5.0]),
            track([10.0, -5.0], [10.0, 0.0]),
        ];
        let fresh = vec![track([0.0, 0.0], [10.0, 0.0])];
        let stub = track([10.0, 0.0], [10.0, 8.0]);
        b.tracks = old.into_iter().chain(fresh.clone()).chain([stub.clone()]).collect();
        let layout = Layout::new(
            &circuiter_model::Design::new(),
            &circuiter_model::CellId::new("x", "y"),
            &b,
        );
        // No schematic here: a layout of the bare board.
        let layout = layout.unwrap_or_else(|_| Layout {
            board: b.clone(),
            parts: Vec::new(),
            pads: Vec::new(),
            nets: Default::default(),
            unplaced: Vec::new(),
            orphans: Vec::new(),
            off_board: Vec::new(),
            errors: Vec::new(),
        });
        let removed = remove_loops(&mut b, &layout, "n", &fresh);
        assert_eq!(removed, 3, "{:?}", ends(&b));
        assert!(b.tracks.contains(&fresh[0]) && b.tracks.contains(&stub));
    }

    #[test]
    fn an_accordion_adds_the_length_asked() {
        let mut b = Board::new(50.0, 50.0);
        b.tracks = vec![track([0.0, 10.0], [20.0, 10.0])];
        let added = tune_length(&mut b, 0, 6.0, 3.0, 1.0, &mut |_| true).unwrap();
        assert!((added - 6.0).abs() < 1e-6, "{added}");
        // Lengths are kept to the nanometre, so the tracks add up to within a few.
        let total: f64 =
            b.tracks.iter().map(|t| (p(t.b)[0] - p(t.a)[0]).hypot(p(t.b)[1] - p(t.a)[1])).sum();
        assert!((total - 26.0).abs() < 1e-4, "{total}");
        // Joined end to end, starting and ending where the track did.
        assert!(
            b.tracks.iter().any(|t| near(p(t.a), [0.0, 10.0]))
                && b.tracks.iter().any(|t| near(p(t.b), [20.0, 10.0]))
        );
        // Every corner mitered: no square turns.
        for w in b.tracks.windows(2) {
            let d = |t: &Track| [p(t.b)[0] - p(t.a)[0], p(t.b)[1] - p(t.a)[1]];
            let (u, v) = (d(&w[0]), d(&w[1]));
            assert!(u[0] * v[0] + u[1] * v[1] > -1e-9, "no turn sharper than 90°");
        }
        // Too much for the room: refused.
        let mut b = Board::new(50.0, 50.0);
        b.tracks = vec![track([0.0, 10.0], [8.0, 10.0])];
        assert!(tune_length(&mut b, 0, 50.0, 2.0, 1.0, &mut |_| true).is_err());
    }

    #[test]
    fn corners_and_vias_carry_their_tracks() {
        let mut b = Board::new(50.0, 50.0);
        b.tracks = vec![track([0.0, 0.0], [5.0, 5.0]), track([5.0, 5.0], [10.0, 5.0])];
        move_corner(&mut b, Layer::FCu, [5.0, 5.0], [6.0, 4.0]);
        assert_eq!(ends(&b), vec![([0.0, 0.0], [6.0, 4.0]), ([6.0, 4.0], [10.0, 5.0])]);
        b.vias.push(circuiter_model::board::Via {
            at: point([10.0, 5.0]),
            diameter: Length::from_mm(0.6),
            drill: Length::from_mm(0.3),
            net: "n".into(),
        });
        move_via(&mut b, 0, [12.0, 6.0]);
        assert_eq!(ends(&b)[1].1, [12.0, 6.0]);
        unroute(&mut b, Some("other"));
        assert_eq!(b.tracks.len(), 2);
        unroute(&mut b, Some("n"));
        assert!(b.tracks.is_empty() && b.vias.is_empty());
    }
}
