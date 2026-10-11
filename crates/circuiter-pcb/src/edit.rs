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
