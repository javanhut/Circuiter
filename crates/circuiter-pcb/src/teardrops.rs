//! Teardrops and via stitching, as Altium's Tools menu has them.
//!
//! A teardrop widens a track where it meets a pad or a via, so a drill that wanders or
//! an etch that undercuts doesn't break the joint. Each is worked out from the routing
//! whenever it's needed (like a pour), so it follows the tracks as they're moved; one
//! that would come too close to another net's copper is left out.
//!
//! Stitching vias join a net's pours on every layer through the board on a grid,
//! wherever one fits clear of everything else.

use crate::geometry::{self, Contour};
use crate::layout::{Layout, P, p, point};
use crate::zones::ZoneFill;
use circuiter_model::board::{Layer, Via};
use circuiter_model::geom::Length;

/// A teardrop: its layer, net and outline.
#[derive(Debug, Clone, PartialEq)]
pub struct Teardrop {
    pub layer: Layer,
    pub net: String,
    pub outline: Contour,
}

/// The board's teardrops, if it asks for them.
pub fn shapes(layout: &Layout) -> Vec<Teardrop> {
    let board = &layout.board;
    let Some(t) = &board.teardrops else { return Vec::new() };
    let rules = &board.rules;
    // What each joint may widen onto: pads and vias, by centre, size, layers and net.
    let mut lands: Vec<(P, f64, Vec<Layer>, Option<String>)> = layout
        .pads
        .iter()
        .map(|pad| (pad.center, pad.size.0.min(pad.size.1), pad.layers.clone(), pad.net.clone()))
        .collect();
    lands.extend(
        board.vias.iter().map(|v| (p(v.at), v.diameter.mm(), board.copper(), Some(v.net.clone()))),
    );
    let mut out = Vec::new();
    for tr in &board.tracks {
        let (a, b) = (p(tr.a), p(tr.b));
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if len < 1e-6 {
            continue;
        }
        let w = tr.width.mm();
        for (end, other) in [(a, b), (b, a)] {
            let Some((c, size, _, _)) = lands.iter().find(|(c, size, layers, net)| {
                net.as_deref() == Some(tr.net.as_str())
                    && layers.contains(&tr.layer)
                    && (c[0] - end[0]).hypot(c[1] - end[1]) < size / 2.0
            }) else {
                continue;
            };
            // Wider than the track only where the land is wider.
            let half = (size * t.width / 2.0).max(w / 2.0);
            if half <= w / 2.0 + 1e-6 {
                continue;
            }
            let dir = [(other[0] - c[0]) / len.max(1e-9), (other[1] - c[1]) / len.max(1e-9)];
            let along = (other[0] - c[0]) * dir[0] + (other[1] - c[1]) * dir[1];
            let reach = (size / 2.0 + size * t.length).min(along);
            if reach <= size / 2.0 {
                continue;
            }
            let n = [-dir[1], dir[0]];
            let base = [c[0] + dir[0] * size * 0.25, c[1] + dir[1] * size * 0.25];
            let tip = [c[0] + dir[0] * reach, c[1] + dir[1] * reach];
            let hw = w / 2.0;
            let outline = geometry::ccw(vec![
                [base[0] + n[0] * half, base[1] + n[1] * half],
                [tip[0] + n[0] * hw, tip[1] + n[1] * hw],
                [tip[0] - n[0] * hw, tip[1] - n[1] * hw],
                [base[0] - n[0] * half, base[1] - n[1] * half],
            ]);
            // Left out where it would come too close to another net.
            let shape = vec![vec![outline.clone()]];
            let need = rules.clearance_for(&tr.net).mm();
            let grown = geometry::offset(&shape, need);
            let clash = layout.pads.iter().any(|pad| {
                pad.net.as_deref() != Some(tr.net.as_str())
                    && pad.layers.contains(&tr.layer)
                    && geometry::overlaps(&grown, &vec![vec![geometry::pad(pad, 0.0)]])
            }) || board.tracks.iter().any(|o| {
                o.net != tr.net
                    && o.layer == tr.layer
                    && geometry::overlaps(&grown, &vec![vec![geometry::track(o, 0.0)]])
            }) || board.vias.iter().any(|v| {
                v.net != tr.net && geometry::overlaps(&grown, &vec![vec![geometry::via(v, 0.0)]])
            });
            if !clash {
                out.push(Teardrop { layer: tr.layer, net: tr.net.clone(), outline });
            }
        }
    }
    out
}

/// Stitching vias for `net` every `spacing` mm: wherever a via fits inside the net's
/// pours, clear of other nets' copper, holes and the edge.
pub fn stitch(layout: &Layout, fills: &[ZoneFill], net: &str, spacing: f64) -> Vec<Via> {
    let board = &layout.board;
    let rules = &board.rules;
    let (dia, drill) = rules.via_for(net);
    let (r, hole) = (dia.mm() / 2.0, drill.mm() / 2.0);
    let need = rules.clearance_for(net).mm();
    let outline: Vec<P> = board.edge().iter().map(|q| p(*q)).collect();
    let Some((lo, hi)) = geometry::bounds(&vec![vec![outline.clone()]]) else { return Vec::new() };
    let pours: Vec<&ZoneFill> = fills.iter().filter(|f| f.net == net).collect();
    if pours.is_empty() || spacing <= 0.0 {
        return Vec::new();
    }
    let mut holes: Vec<(P, f64)> =
        layout.pads.iter().filter_map(|pad| Some((pad.center, pad.drill()? / 2.0))).collect();
    holes.extend(board.vias.iter().map(|v| (p(v.at), v.drill.mm() / 2.0)));
    let mut out: Vec<Via> = Vec::new();
    let mut y = lo[1] + spacing / 2.0;
    while y < hi[1] {
        let mut x = lo[0] + spacing / 2.0;
        while x < hi[0] {
            let q = [x, y];
            let disc = vec![vec![geometry::circle(q, r)]];
            // Wholly inside the net's pour on some layer.
            let poured = pours
                .iter()
                .any(|f| geometry::area(&geometry::difference(&disc, &f.shapes)) < 1e-6);
            let grown = vec![vec![geometry::circle(q, r + need)]];
            let clear = poured
                && !layout.pads.iter().any(|pad| pad.net.as_deref() != Some(net) && geometry::overlaps(&grown, &vec![vec![geometry::pad(pad, 0.0)]]))
                && !board.tracks.iter().any(|t| t.net != net && geometry::overlaps(&grown, &vec![vec![geometry::track(t, 0.0)]]))
                && !board.vias.iter().any(|v| v.net != net && geometry::overlaps(&grown, &vec![vec![geometry::via(v, 0.0)]]))
                && holes.iter().chain(out.iter().map(|v| (p(v.at), hole)).collect::<Vec<_>>().iter()).all(|(c, h)| {
                    (c[0] - q[0]).hypot(c[1] - q[1]) - h - hole >= rules.hole_to_hole.mm()
                })
                // Its own net's pads too: a via in a pad is another matter.
                && !layout.pads.iter().any(|pad| pad.net.as_deref() == Some(net) && geometry::overlaps(&disc, &vec![vec![geometry::pad(pad, 0.0)]]));
            if clear {
                out.push(Via {
                    at: point(q),
                    diameter: dia,
                    drill: Length::from_mm(drill.mm()),
                    net: net.to_owned(),
                });
            }
            x += spacing;
        }
        y += spacing;
    }
    out
}
