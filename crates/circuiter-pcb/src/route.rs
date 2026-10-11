//! The autorouter: turns ratsnest lines into tracks. The board is laid out as a fine
//! grid on every copper layer, each cell marked with the net whose copper (grown by the
//! clearance and half a track) covers it; an A* search then finds each connection's
//! cheapest path in steps of 0°, 45° and 90°, paying for length, for bends, for going
//! against a layer's preferred direction (the front runs across, the back up and down)
//! and for vias. Each found path is drawn back as few straight tracks as it can be
//! (pulled taut where nothing is in the way), then marked on the grid so later
//! connections keep clear of it. Shortest connections go first; those that can't be
//! found are left on the ratsnest and reported.

use crate::connect;
use crate::geometry;
use crate::layout::{Layout, P, PadAt, p, point};
use crate::zones;
use circuiter_model::board::{Layer, PadShape, Track, Via};
use circuiter_model::geom::Length;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

/// How to route.
#[derive(Debug, Clone)]
pub struct Options {
    /// Grid step, mm.
    pub pitch: f64,
    /// The cost of a via, in grid steps of track.
    pub via_cost: f64,
    /// The cost of a 45° and a 90° bend, in steps.
    pub bend: (f64, f64),
    /// Extra cost per step against a layer's preferred direction.
    pub against: f64,
    /// Only these nets (all when `None`).
    pub nets: Option<BTreeSet<String>>,
    /// Give up on a connection after this many search steps.
    pub limit: usize,
    /// Only horizontal and vertical steps (90° corners).
    pub orthogonal: bool,
    /// Corners at any angle: paths pulled straight from corner to corner.
    pub any_angle: bool,
    /// Whether a path may change layer.
    pub vias: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            pitch: 0.15,
            via_cost: 25.0,
            bend: (0.6, 2.5),
            against: 0.4,
            nets: None,
            limit: 3_000_000,
            orthogonal: false,
            any_angle: false,
            vias: true,
        }
    }
}

/// What routing made.
#[derive(Debug, Clone, Default)]
pub struct Routed {
    pub tracks: Vec<Track>,
    pub vias: Vec<Via>,
    /// Connections routed.
    pub done: usize,
    /// Connections it couldn't find a way for: net, from, to.
    pub failed: Vec<(String, P, P)>,
}

/// Grid cell contents: free, blocked for everyone, or a net's.
const FREE: i32 = -1;
const BLOCKED: i32 = -2;

/// Something on the board, for the grid: its shape and how far it reaches.
#[derive(Debug, Clone)]
enum Prim {
    Disc(P, f64),
    /// A segment and its half width.
    Seg(P, P, f64),
    /// A rectangle centred at a point, half sizes, turned (degrees).
    Rect(P, (f64, f64), f64),
}

impl Prim {
    /// Distance from `q` to its copper, 0 inside.
    fn dist(&self, q: P) -> f64 {
        match self {
            Prim::Disc(c, r) => ((q[0] - c[0]).hypot(q[1] - c[1]) - r).max(0.0),
            Prim::Seg(a, b, h) => (geometry::segment_distance(*a, *b, q, q) - h).max(0.0),
            Prim::Rect(c, (hw, hh), deg) => {
                let (s, co) = deg.to_radians().sin_cos();
                let (dx, dy) = (q[0] - c[0], q[1] - c[1]);
                // Into the pad's frame (turned counter-clockwise on screen, Y down).
                let (x, y) = (dx * co - dy * s, dx * s + dy * co);
                let (ox, oy) = ((x.abs() - hw).max(0.0), (y.abs() - hh).max(0.0));
                ox.hypot(oy)
            }
        }
    }

    fn bounds(&self) -> (P, P) {
        match self {
            Prim::Disc(c, r) => ([c[0] - r, c[1] - r], [c[0] + r, c[1] + r]),
            Prim::Seg(a, b, h) => {
                ([a[0].min(b[0]) - h, a[1].min(b[1]) - h], [a[0].max(b[0]) + h, a[1].max(b[1]) + h])
            }
            Prim::Rect(c, (hw, hh), _) => {
                let r = hw.hypot(*hh);
                ([c[0] - r, c[1] - r], [c[0] + r, c[1] + r])
            }
        }
    }
}

/// A pad's copper as a grid shape.
fn pad_prim(pad: &PadAt) -> Prim {
    let (w, h) = pad.size;
    match pad.shape {
        PadShape::Circle => Prim::Disc(pad.center, w.max(h) / 2.0),
        PadShape::Oval => {
            // A stadium: a segment along the longer side.
            let r = w.min(h) / 2.0;
            let half = (w.max(h) / 2.0 - r).max(0.0);
            let (s, c) = pad.rotation.to_radians().sin_cos();
            let (ux, uy) = if w >= h { (c, -s) } else { (s, c) };
            let a = [pad.center[0] - ux * half, pad.center[1] - uy * half];
            let b = [pad.center[0] + ux * half, pad.center[1] + uy * half];
            Prim::Seg(a, b, r)
        }
        PadShape::Rect | PadShape::RoundRect => {
            Prim::Rect(pad.center, (w / 2.0, h / 2.0), pad.rotation)
        }
    }
}

/// The routing grid: for each layer, which net (if any) may use each cell, once for a
/// track's centre and once for a via's.
struct Grid {
    x0: f64,
    y0: f64,
    pitch: f64,
    nx: usize,
    ny: usize,
    layers: usize,
    track: Vec<i32>,
    via: Vec<i32>,
}

impl Grid {
    fn index(&self, l: usize, x: usize, y: usize) -> usize {
        (l * self.ny + y) * self.nx + x
    }

    fn centre(&self, x: usize, y: usize) -> P {
        [self.x0 + x as f64 * self.pitch, self.y0 + y as f64 * self.pitch]
    }

    fn cell(&self, q: P) -> (usize, usize) {
        let x = ((q[0] - self.x0) / self.pitch).round().clamp(0.0, (self.nx - 1) as f64) as usize;
        let y = ((q[1] - self.y0) / self.pitch).round().clamp(0.0, (self.ny - 1) as f64) as usize;
        (x, y)
    }

    /// Marks the cells within `reach` of `prim` on layer `l` as `net`'s (or blocked,
    /// where another net is already there).
    fn mark(&mut self, via_map: bool, l: usize, prim: &Prim, reach: f64, net: i32) {
        let (lo, hi) = prim.bounds();
        let (x0, y0) = self.cell([lo[0] - reach, lo[1] - reach]);
        let (x1, y1) = self.cell([hi[0] + reach, hi[1] + reach]);
        for y in y0..=y1 {
            for x in x0..=x1 {
                if prim.dist(self.centre(x, y)) < reach {
                    let i = self.index(l, x, y);
                    let map = if via_map { &mut self.via } else { &mut self.track };
                    map[i] = match map[i] {
                        FREE => net,
                        v if v == net => net,
                        _ => BLOCKED,
                    };
                }
            }
        }
    }

    fn passable(&self, via_map: bool, i: usize, net: i32) -> bool {
        let v = if via_map { self.via[i] } else { self.track[i] };
        v == FREE || v == net
    }
}

/// Directions: right, down-right, down, down-left, left, up-left, up, up-right.
const DIRS: [(i32, i32); 8] =
    [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

#[derive(Debug, Clone, Copy, PartialEq)]
struct Open {
    f: f64,
    state: usize,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.total_cmp(&self.f).then_with(|| other.state.cmp(&self.state))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The search's memory, kept between connections.
struct Search {
    cost: Vec<f64>,
    from: Vec<u32>,
    stamp: Vec<u32>,
    generation: u32,
}

impl Search {
    fn new(states: usize) -> Search {
        Search {
            cost: vec![0.0; states],
            from: vec![0; states],
            stamp: vec![0; states],
            generation: 0,
        }
    }
}

/// The copper layers there is copper of `net` at `q` on: a pad's layers, a via's
/// (all), a track's, a pour's.
fn layers_at(
    layout: &Layout,
    fills: &[zones::ZoneFill],
    q: P,
    net: &str,
    all: &[Layer],
) -> Vec<Layer> {
    let near = |a: P| (a[0] - q[0]).hypot(a[1] - q[1]) < 1e-3;
    let mut out = BTreeSet::new();
    for pad in &layout.pads {
        if pad.net.as_deref() == Some(net) && near(pad.center) {
            out.extend(pad.layers.iter().copied());
        }
    }
    for v in &layout.board.vias {
        if v.net == net && near(p(v.at)) {
            out.extend(all.iter().copied());
        }
    }
    for t in &layout.board.tracks {
        if t.net == net && (near(p(t.a)) || near(p(t.b))) {
            out.insert(t.layer);
        }
    }
    if out.is_empty() {
        for f in fills.iter().filter(|f| f.net == net) {
            out.insert(f.layer);
        }
    }
    if out.is_empty() {
        out.extend(all.iter().copied());
    }
    out.into_iter().collect()
}

/// A board ready to route on: its grid built once, then connection after connection
/// found and added to it. The interactive router keeps one while the board is
/// unchanged, so a track can walk around obstacles as the pointer moves.
pub struct Router {
    grid: Grid,
    search: Search,
    ids: BTreeMap<String, i32>,
    layers: Vec<Layer>,
    track_reach: f64,
    via_reach: f64,
    via_hole_reach: f64,
    rules: circuiter_model::board::Rules,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Router({}×{}×{} at {} mm)",
            self.grid.nx, self.grid.ny, self.grid.layers, self.grid.pitch
        )
    }
}

impl Router {
    /// The grid for the board of `layout`, at `pitch`.
    pub fn new(layout: &Layout, pitch: f64) -> Option<Router> {
        let board = &layout.board;
        let rules = &board.rules;
        let layers = board.copper();
        let outline: Vec<P> = board.outline.iter().map(|q| p(*q)).collect();
        let (lo, hi) = geometry::bounds(&vec![vec![outline.clone()]])?;
        let pitch = pitch.max(0.02);
        let slack = pitch * 0.71;
        let clearance = std::iter::once(rules.clearance.mm())
            .chain(rules.classes.iter().map(|c| c.clearance.mm()))
            .fold(0.0, f64::max);
        let widest = std::iter::once(rules.track_width.mm())
            .chain(rules.classes.iter().map(|c| c.track_width.mm()))
            .fold(0.0, f64::max);
        let via_r = std::iter::once(rules.via_diameter.mm())
            .chain(rules.classes.iter().map(|c| c.via_diameter.mm()))
            .fold(0.0, f64::max)
            / 2.0;
        let track_reach = clearance + widest / 2.0 + slack;
        let via_reach = clearance + via_r + slack;
        let via_hole_reach = via_reach.max(via_r + rules.hole_to_hole.mm() + slack);
        let nx = ((hi[0] - lo[0]) / pitch).ceil() as usize + 1;
        let ny = ((hi[1] - lo[1]) / pitch).ceil() as usize + 1;
        let nl = layers.len();
        let mut grid = Grid {
            x0: lo[0],
            y0: lo[1],
            pitch,
            nx,
            ny,
            layers: nl,
            track: vec![FREE; nl * nx * ny],
            via: vec![FREE; nl * nx * ny],
        };
        // The edge: nothing outside the board or within its clearance of the edge.
        let edge = rules.edge_clearance.mm();
        let n = outline.len();
        for y in 0..ny {
            for x in 0..nx {
                let c = grid.centre(x, y);
                let mut inside = false;
                for k in 0..n {
                    let (a, b) = (outline[k], outline[(k + 1) % n]);
                    if (a[1] > c[1]) != (b[1] > c[1])
                        && c[0] < a[0] + (c[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
                    {
                        inside = !inside;
                    }
                }
                let to_edge = (0..n)
                    .map(|k| geometry::segment_distance(outline[k], outline[(k + 1) % n], c, c))
                    .fold(f64::MAX, f64::min);
                for l in 0..nl {
                    let i = grid.index(l, x, y);
                    if !inside || to_edge < edge + widest / 2.0 + slack {
                        grid.track[i] = BLOCKED;
                    }
                    if !inside || to_edge < edge + via_r + slack {
                        grid.via[i] = BLOCKED;
                    }
                }
            }
        }
        let mut r = Router {
            grid,
            search: Search::new(nl * nx * ny * 9),
            ids: BTreeMap::new(),
            layers: layers.clone(),
            track_reach,
            via_reach,
            via_hole_reach,
            rules: rules.clone(),
        };
        // Everything already on the board.
        for pad in &layout.pads {
            let net = pad.net.as_deref().map_or(BLOCKED, |n| r.id(n));
            let prim = pad_prim(pad);
            let on: Vec<usize> = pad.layers.iter().filter_map(|l| r.layer_index(*l)).collect();
            for l in on {
                r.grid.mark(false, l, &prim, track_reach, net);
                r.grid.mark(true, l, &prim, via_reach, net);
            }
            // A hole keeps everything clear on every layer.
            if let Some(d) = pad.drill() {
                let hole = Prim::Disc(pad.center, d / 2.0);
                for l in 0..nl {
                    r.grid.mark(false, l, &hole, track_reach, net);
                    r.grid.mark(true, l, &hole, via_hole_reach, net);
                }
            }
        }
        r.add(&board.tracks, &board.vias);
        Some(r)
    }

    fn id(&mut self, net: &str) -> i32 {
        let next = self.ids.len() as i32;
        *self.ids.entry(net.to_owned()).or_insert(next)
    }

    fn layer_index(&self, l: Layer) -> Option<usize> {
        self.layers.iter().position(|x| *x == l)
    }

    /// Marks tracks and vias on the grid, so later routes keep clear of them.
    pub fn add(&mut self, tracks: &[Track], vias: &[Via]) {
        for t in tracks {
            let net = self.id(&t.net);
            if let Some(l) = self.layer_index(t.layer) {
                let prim = Prim::Seg(p(t.a), p(t.b), t.width.mm() / 2.0);
                self.grid.mark(false, l, &prim, self.track_reach, net);
                self.grid.mark(true, l, &prim, self.via_reach, net);
            }
        }
        for v in vias {
            let net = self.id(&v.net);
            let prim = Prim::Disc(p(v.at), v.diameter.mm() / 2.0);
            for l in 0..self.grid.layers {
                self.grid.mark(false, l, &prim, self.track_reach, net);
                self.grid.mark(true, l, &prim, self.via_hole_reach, net);
            }
        }
    }

    /// Whether a track of `net` on `layer` from `a` to `b` stays clear of other nets'
    /// copper (as the grid sees it).
    pub fn clear(&mut self, net: &str, layer: Layer, a: P, b: P) -> bool {
        let nid = self.id(net);
        self.layer_index(layer).is_some_and(|l| clear_line(&self.grid, l, a, b, nid))
    }

    /// How far along `path` (on `layer`) a track of `net` gets before it would run into
    /// something: the path cut short there (KiCad and Altium's "stop at first obstacle").
    pub fn until_blocked(&mut self, net: &str, layer: Layer, path: &[P]) -> Vec<P> {
        let nid = self.id(net);
        let Some(l) = self.layer_index(layer) else { return path.to_vec() };
        let mut out = vec![path[0]];
        for w in path.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            let n = (len / (self.grid.pitch / 3.0)).ceil().max(1.0) as usize;
            let mut last = a;
            for k in 1..=n {
                let t = k as f64 / n as f64;
                let q = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
                let (x, y) = self.grid.cell(q);
                if !self.grid.passable(false, self.grid.index(l, x, y), nid) {
                    if (last[0] - a[0]).hypot(last[1] - a[1]) > 1e-9 {
                        out.push(last);
                    }
                    return out;
                }
                last = q;
            }
            out.push(b);
        }
        out
    }

    /// A path for `net` from `a` (on layers `la`) to `b` (on `lb`): tracks and vias, or
    /// `None` if there's no way through within the search's limit.
    #[allow(clippy::too_many_arguments)]
    pub fn connect(
        &mut self,
        net: &str,
        a: P,
        la: &[Layer],
        b: P,
        lb: &[Layer],
        width: f64,
        opts: &Options,
    ) -> Option<(Vec<Track>, Vec<Via>)> {
        let nid = self.id(net);
        let la: Vec<usize> = la.iter().filter_map(|l| self.layer_index(*l)).collect();
        let lb: Vec<usize> = lb.iter().filter_map(|l| self.layer_index(*l)).collect();
        let (via_d, via_drill) = self.rules.via_for(net);
        let path = find(&self.grid, &mut self.search, opts, nid, (a, &la), (b, &lb))?;
        Some(draw(
            &self.grid,
            &path,
            a,
            b,
            nid,
            net,
            width,
            &self.layers,
            (via_d, via_drill),
            (opts.orthogonal, opts.any_angle),
        ))
    }
}

/// Routes the board's ratsnest (or the nets `opts` names).
pub fn autoroute(layout: &Layout, opts: &Options) -> Routed {
    let mut out = Routed::default();
    let fills = zones::fill(layout);
    let copper = connect::copper(layout, &fills);
    let conn = connect::analyze(&copper);
    let mut todo: Vec<(String, P, P)> = conn
        .ratsnest
        .iter()
        .filter(|(n, _, _)| opts.nets.as_ref().is_none_or(|s| s.contains(n)))
        .cloned()
        .collect();
    if todo.is_empty() {
        return out;
    }
    todo.sort_by(|a, b| {
        let d = |c: &(String, P, P)| (c.1[0] - c.2[0]).hypot(c.1[1] - c.2[1]);
        d(a).total_cmp(&d(b))
    });
    let Some(mut router) = Router::new(layout, opts.pitch) else { return out };
    let layers = layout.board.copper();
    let mut failed = Vec::new();
    for pass in 0..2 {
        let list = if pass == 0 { std::mem::take(&mut todo) } else { std::mem::take(&mut failed) };
        // The second pass, for what failed: vias cheaper, bends cheaper.
        let mut o = opts.clone();
        if pass == 1 {
            o.via_cost *= 0.4;
            o.against *= 0.25;
        }
        for (net, a, b) in list {
            let width = layout.board.rules.track_width_for(&net).mm();
            let la = layers_at(layout, &fills, a, &net, &layers);
            let lb = layers_at(layout, &fills, b, &net, &layers);
            match router.connect(&net, a, &la, b, &lb, width, &o) {
                Some((tracks, vias)) => {
                    router.add(&tracks, &vias);
                    out.tracks.extend(tracks);
                    out.vias.extend(vias);
                    out.done += 1;
                }
                None if pass == 0 => failed.push((net, a, b)),
                None => out.failed.push((net, a, b)),
            }
        }
    }
    out
}

/// The copper layers there's copper of `net` at `q` on, for a route to start or end.
pub fn layers_of(layout: &Layout, q: P, net: &str) -> Vec<Layer> {
    layers_at(layout, &[], q, net, &layout.board.copper())
}

/// One cell on one layer.
type Node = (usize, usize, usize);

/// The cheapest path from `a` (on layers `la`) to `b` (on `lb`) for net `nid`.
fn find(
    grid: &Grid,
    s: &mut Search,
    o: &Options,
    nid: i32,
    (a, la): (P, &[usize]),
    (b, lb): (P, &[usize]),
) -> Option<Vec<Node>> {
    let (nx, ny) = (grid.nx, grid.ny);
    s.generation += 1;
    let gen_ = s.generation;
    let (ax, ay) = grid.cell(a);
    let (bx, by) = grid.cell(b);
    let goal: BTreeSet<usize> = lb.iter().map(|&l| grid.index(l, bx, by)).collect();
    let h = |x: usize, y: usize| {
        let (dx, dy) = ((x as f64 - bx as f64).abs(), (y as f64 - by as f64).abs());
        dx.max(dy) + (std::f64::consts::SQRT_2 - 1.0) * dx.min(dy)
    };
    let mut open = BinaryHeap::new();
    for &l in la {
        let st = grid.index(l, ax, ay) * 9 + 8;
        s.cost[st] = 0.0;
        s.from[st] = u32::MAX;
        s.stamp[st] = gen_;
        open.push(Open { f: h(ax, ay), state: st });
    }
    let mut steps = 0;
    while let Some(Open { f: _, state }) = open.pop() {
        steps += 1;
        if steps > o.limit {
            return None;
        }
        let cell = state / 9;
        let dir = state % 9;
        let g = s.cost[state];
        let l = cell / (nx * ny);
        let rem = cell % (nx * ny);
        let (y, x) = (rem / nx, rem % nx);
        if goal.contains(&cell) {
            // Walk back.
            let mut path = Vec::new();
            let mut at = state;
            loop {
                let c = at / 9;
                let l = c / (nx * ny);
                let r = c % (nx * ny);
                path.push((l, r % nx, r / nx));
                let prev = s.from[at];
                if prev == u32::MAX {
                    break;
                }
                at = prev as usize;
            }
            path.reverse();
            return Some(path);
        }
        let relax =
            |next: usize, cost: f64, open: &mut BinaryHeap<Open>, s: &mut Search, hx: f64| {
                if s.stamp[next] != gen_ || cost < s.cost[next] - 1e-9 {
                    s.stamp[next] = gen_;
                    s.cost[next] = cost;
                    s.from[next] = state as u32;
                    open.push(Open { f: cost + hx, state: next });
                }
            };
        // Along the layer.
        for (d, (dx, dy)) in DIRS.iter().enumerate() {
            if o.orthogonal && d % 2 == 1 {
                continue;
            }
            let (x2, y2) = (x as i32 + dx, y as i32 + dy);
            if x2 < 0 || y2 < 0 || x2 >= nx as i32 || y2 >= ny as i32 {
                continue;
            }
            let (x2, y2) = (x2 as usize, y2 as usize);
            let c2 = grid.index(l, x2, y2);
            if !goal.contains(&c2) && !grid.passable(false, c2, nid) {
                continue;
            }
            let bend = if dir == 8 {
                0.0
            } else {
                match (d as i32 - dir as i32)
                    .rem_euclid(8)
                    .min((dir as i32 - d as i32).rem_euclid(8))
                {
                    0 => 0.0,
                    1 => o.bend.0,
                    2 => o.bend.1,
                    _ => continue,
                }
            };
            let step = if dx != &0 && dy != &0 { std::f64::consts::SQRT_2 } else { 1.0 };
            // The front prefers to run across, the next layer down up and down, and so on.
            let against = match (l % 2, dx, dy) {
                (0, 0, _) | (1, _, 0) => o.against,
                _ => 0.0,
            };
            relax(c2 * 9 + d, g + step + bend + against, &mut open, s, h(x2, y2));
        }
        // Through a via to another layer.
        if grid.layers > 1 && o.vias {
            let clear = (0..grid.layers).all(|k| grid.passable(true, grid.index(k, x, y), nid));
            if clear {
                for l2 in (0..grid.layers).filter(|&k| k != l) {
                    let c2 = grid.index(l2, x, y);
                    relax(c2 * 9 + 8, g + o.via_cost, &mut open, s, h(x, y));
                }
            }
        }
    }
    None
}

/// A found path drawn as tracks (corners only where it bends, pulled taut where it can
/// be at 0°, 45° or 90°) and vias where it changes layer.
#[allow(clippy::too_many_arguments)]
fn draw(
    grid: &Grid,
    path: &[Node],
    a: P,
    b: P,
    nid: i32,
    net: &str,
    width: f64,
    layers: &[Layer],
    (via_d, via_drill): (Length, Length),
    (orthogonal, any_angle): (bool, bool),
) -> (Vec<Track>, Vec<Via>) {
    let mut tracks = Vec::new();
    let mut vias = Vec::new();
    // Runs on one layer.
    let mut runs: Vec<(usize, Vec<P>)> = Vec::new();
    for &(l, x, y) in path {
        let c = grid.centre(x, y);
        match runs.last_mut() {
            Some((rl, pts)) if *rl == l => pts.push(c),
            _ => {
                if let Some((_, pts)) = runs.last() {
                    let at = *pts.last().expect("a point");
                    vias.push(Via {
                        at: point(at),
                        diameter: via_d,
                        drill: via_drill,
                        net: net.to_owned(),
                    });
                }
                let start = runs.last().and_then(|(_, pts)| pts.last().copied());
                runs.push((l, start.into_iter().chain(std::iter::once(c)).collect()));
            }
        }
    }
    let last = runs.len().saturating_sub(1);
    for (k, (l, mut pts)) in runs.into_iter().enumerate() {
        pts.dedup();
        let mut pts = taut(grid, &pts, l, nid, (orthogonal, any_angle));
        // Join the exact pad (or track end) to the grid at each end.
        if k == 0 {
            pts.insert(0, a);
        }
        if k == last {
            pts.push(b);
        }
        pts.dedup_by(|x, y| (x[0] - y[0]).hypot(x[1] - y[1]) < 1e-6);
        for w in pts.windows(2) {
            tracks.push(Track {
                layer: layers[l],
                a: point(w[0]),
                b: point(w[1]),
                width: Length::from_mm(width),
                net: net.to_owned(),
            });
        }
    }
    (tracks, vias)
}

/// Whether a line on layer `l` crosses only cells net `nid` may use.
fn clear_line(grid: &Grid, l: usize, a: P, b: P, nid: i32) -> bool {
    let len = (b[0] - a[0]).hypot(b[1] - a[1]);
    let n = (len / (grid.pitch / 3.0)).ceil().max(1.0) as usize;
    (0..=n).all(|k| {
        let t = k as f64 / n as f64;
        let (x, y) = grid.cell([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
        grid.passable(false, grid.index(l, x, y), nid)
    })
}

/// Fewest corners: from each corner, straight on to the furthest point along the path
/// that can be reached through clear cells at an allowed angle (multiples of 45°, of
/// 90°, or any).
fn taut(
    grid: &Grid,
    pts: &[P],
    l: usize,
    nid: i32,
    (orthogonal, any_angle): (bool, bool),
) -> Vec<P> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let allowed = |a: P, b: P| {
        let (dx, dy) = ((b[0] - a[0]).abs(), (b[1] - a[1]).abs());
        any_angle || dx < 1e-9 || dy < 1e-9 || (!orthogonal && (dx - dy).abs() < 1e-9)
    };
    let mut out = vec![pts[0]];
    let mut i = 0;
    while i < pts.len() - 1 {
        let mut j = pts.len() - 1;
        while j > i + 1 && !(allowed(pts[i], pts[j]) && clear_line(grid, l, pts[i], pts[j], nid)) {
            j -= 1;
        }
        out.push(pts[j]);
        i = j;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_have_distances() {
        let r = Prim::Rect([0.0, 0.0], (1.0, 0.5), 0.0);
        assert_eq!(r.dist([0.5, 0.2]), 0.0);
        assert!((r.dist([2.0, 0.0]) - 1.0).abs() < 1e-12);
        let turned = Prim::Rect([0.0, 0.0], (1.0, 0.5), 90.0);
        assert!((turned.dist([0.0, 2.0]) - 1.0).abs() < 1e-12, "{}", turned.dist([0.0, 2.0]));
        assert!((Prim::Seg([0.0, 0.0], [2.0, 0.0], 0.5).dist([1.0, 1.0]) - 0.5).abs() < 1e-12);
    }
}
