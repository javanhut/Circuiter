//! Which copper is joined: pads, tracks, vias and pours of one net touch on a shared
//! layer and so connect. What a net still lacks is its ratsnest: the shortest lines
//! joining its separate islands, drawn until they're routed.

use crate::geometry::{self, Shapes};
use crate::layout::{Layout, P, p};
use crate::zones::ZoneFill;
use circuiter_model::board::Layer;
use std::collections::BTreeMap;

/// A piece of copper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Pad(usize),
    Track(usize),
    Via(usize),
    /// A zone's fill: zone index, piece.
    Fill(usize, usize),
    /// A teardrop, by its place in [`crate::teardrops::shapes`].
    Teardrop(usize),
}

#[derive(Debug, Clone)]
pub struct Copper {
    pub item: Item,
    pub net: Option<String>,
    pub layers: Vec<Layer>,
    pub shape: Shapes,
    /// Where a ratsnest line may end: a pad's centre, a track's ends, a via.
    pub anchors: Vec<P>,
    pub lo: P,
    pub hi: P,
}

/// Every piece of copper on the board, with its shape.
pub fn copper(layout: &Layout, fills: &[ZoneFill]) -> Vec<Copper> {
    let mut out = Vec::new();
    let mut push =
        |item, net: Option<String>, layers: Vec<Layer>, shape: Shapes, anchors: Vec<P>| {
            if let Some((lo, hi)) = geometry::bounds(&shape) {
                out.push(Copper { item, net, layers, shape, anchors, lo, hi });
            }
        };
    for (k, pad) in layout.pads.iter().enumerate() {
        if !pad.layers.is_empty() {
            push(
                Item::Pad(k),
                pad.net.clone(),
                pad.layers.clone(),
                vec![vec![geometry::pad(pad, 0.0)]],
                vec![pad.center],
            );
        }
    }
    let all = layout.board.copper();
    for (k, t) in layout.board.tracks.iter().enumerate() {
        push(
            Item::Track(k),
            Some(t.net.clone()),
            vec![t.layer],
            vec![vec![geometry::track(t, 0.0)]],
            vec![p(t.a), p(t.b)],
        );
    }
    for (k, v) in layout.board.vias.iter().enumerate() {
        push(
            Item::Via(k),
            Some(v.net.clone()),
            all.clone(),
            vec![vec![geometry::via(v, 0.0)]],
            vec![p(v.at)],
        );
    }
    for (k, t) in crate::teardrops::shapes(layout).into_iter().enumerate() {
        let anchors = vec![t.outline[0]];
        push(Item::Teardrop(k), Some(t.net), vec![t.layer], vec![vec![t.outline]], anchors);
    }
    for f in fills {
        for (j, piece) in f.shapes.iter().enumerate() {
            let anchors = piece
                .first()
                .map(|c| c.iter().step_by((c.len() / 8).max(1)).copied().collect())
                .unwrap_or_default();
            push(
                Item::Fill(f.zone, j),
                Some(f.net.clone()),
                vec![f.layer],
                vec![piece.clone()],
                anchors,
            );
        }
    }
    out
}

fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

/// Whether two pieces of copper touch: they share a layer and their shapes meet.
pub fn touch(a: &Copper, b: &Copper) -> bool {
    const EPS: f64 = 1e-4;
    if a.hi[0] + EPS < b.lo[0]
        || b.hi[0] + EPS < a.lo[0]
        || a.hi[1] + EPS < b.lo[1]
        || b.hi[1] + EPS < a.lo[1]
    {
        return false;
    }
    if !a.layers.iter().any(|l| b.layers.contains(l)) {
        return false;
    }
    let grown = geometry::offset(&a.shape, EPS);
    geometry::overlaps(&grown, &b.shape)
}

/// Pairs of copper pieces whose boxes come within `margin` of each other, found by
/// sweeping across X: the candidates for touching and clearance checks.
pub fn near_pairs(items: &[Copper], margin: f64) -> Vec<(usize, usize)> {
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by(|&a, &b| items[a].lo[0].total_cmp(&items[b].lo[0]));
    let mut out = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        for &j in &order[k + 1..] {
            if items[j].lo[0] > items[i].hi[0] + margin {
                break;
            }
            let (a, b) = (&items[i], &items[j]);
            if a.lo[1] <= b.hi[1] + margin && b.lo[1] <= a.hi[1] + margin {
                out.push((i.min(j), i.max(j)));
            }
        }
    }
    out
}

/// What's joined, and what still isn't.
#[derive(Debug, Clone, Default)]
pub struct Connectivity {
    /// For each copper piece, the island it belongs to.
    pub island: Vec<usize>,
    /// Lines still to route: net, from, to.
    pub ratsnest: Vec<(String, P, P)>,
}

impl Connectivity {
    pub fn unrouted(&self) -> usize {
        self.ratsnest.len()
    }
}

/// Joins touching copper of each net and works out the ratsnest.
pub fn analyze(items: &[Copper]) -> Connectivity {
    let n = items.len();
    let mut parent: Vec<usize> = (0..n).collect();
    for (i, j) in near_pairs(items, 1e-3) {
        let (a, b) = (&items[i], &items[j]);
        if a.net.is_none() || a.net != b.net {
            continue;
        }
        let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
        if ri != rj && touch(a, b) {
            parent[ri] = rj;
        }
    }
    let island: Vec<usize> = (0..n).map(|i| find(&mut parent, i)).collect();
    // Each net's islands, by their anchors.
    let mut nets: BTreeMap<&str, BTreeMap<usize, Vec<P>>> = BTreeMap::new();
    for (k, c) in items.iter().enumerate() {
        if let Some(net) = &c.net {
            nets.entry(net)
                .or_default()
                .entry(island[k])
                .or_default()
                .extend(c.anchors.iter().copied());
        }
    }
    let mut ratsnest = Vec::new();
    for (net, islands) in nets {
        let groups: Vec<&Vec<P>> = islands.values().filter(|a| !a.is_empty()).collect();
        if groups.len() < 2 {
            continue;
        }
        // Prim's algorithm over the islands, each edge the closest pair of anchors.
        let closest = |a: &[P], b: &[P]| {
            let mut best = (f64::INFINITY, [0.0; 2], [0.0; 2]);
            for &x in a {
                for &y in b {
                    let d = (x[0] - y[0]).hypot(x[1] - y[1]);
                    if d < best.0 {
                        best = (d, x, y);
                    }
                }
            }
            best
        };
        let mut joined = vec![false; groups.len()];
        joined[0] = true;
        for _ in 1..groups.len() {
            let mut best: Option<(f64, usize, P, P)> = None;
            for (i, g) in groups.iter().enumerate().filter(|(i, _)| joined[*i]) {
                let _ = i;
                for (j, h) in groups.iter().enumerate().filter(|(j, _)| !joined[*j]) {
                    let (d, a, b) = closest(g, h);
                    if best.is_none_or(|x| d < x.0) {
                        best = Some((d, j, a, b));
                    }
                }
            }
            let Some((_, j, a, b)) = best else { break };
            joined[j] = true;
            ratsnest.push((net.to_owned(), a, b));
        }
    }
    Connectivity { island, ratsnest }
}
