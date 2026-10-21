//! The layout engine, on abstract blocks and nets: the method of RTL viewers and
//! layered graph drawing (Sugiyama). Blocks go in columns so signals flow left to
//! right; a net that spans columns passes between the blocks of the columns in between;
//! and in the channel between two columns each net gets a vertical track, joined by
//! horizontal stubs to its pins.
//!
//! A pin on a block's top or bottom rises out of it and runs along a lane of its own
//! above (or below) the block to the channel it joins.
//!
//! Wires of different nets may cross, which doesn't connect them, but never touch: no
//! end or corner of one lies on another, no two overlap, and no wire passes over a pin.
//! A net that can't be wired that way (two nets would each need their track left of
//! the other's) is drawn with labels instead, which connect it just as well.
//!
//! Coordinates are integer nanometres, as in the design model, with y growing down.

use std::collections::{BTreeMap, BTreeSet};

/// 50 mil, the layout's step: half the schematic grid.
pub const UNIT: i64 = 1_270_000;
/// Room between blocks (and the wires passing between them) in a column.
const GAP: i64 = 2 * UNIT;
/// Between neighbouring tracks in a channel.
const PITCH: i64 = 2 * UNIT;
/// How far a labelled pin's stub reaches out of a block's top or bottom.
const RISER: i64 = 2 * UNIT;

pub type P = (i64, i64);

/// Which side of its block a pin is on: wires leave it in that direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// Where a block may go: the first column (the sheet's inputs), the last (its
/// outputs), or wherever the signals put it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Column {
    #[default]
    Any,
    First,
    Last,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    /// The connection point, from the block's origin.
    pub at: P,
    pub side: Side,
    /// It drives its net (an output), so the net flows away from it.
    pub drives: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// The box around everything drawn for the block, text included, from its origin.
    pub min: P,
    pub max: P,
    pub pins: Vec<Pin>,
    pub column: Column,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Net {
    /// (block, pin) of each pin on the net.
    pub pins: Vec<(usize, usize)>,
    /// Draw it with wires if it can be; otherwise (power and other global nets) with a
    /// label at each pin, even a lone one.
    pub wired: bool,
    /// When it's wired, name it with a label too.
    pub named: bool,
    /// The size of what marks the net at a labelled pin, or beside its wire when it's
    /// named (its label's text, or a power symbol): along the wire, then across it.
    pub mark: P,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    /// Where each block's origin goes.
    pub origins: Vec<P>,
    /// Straight wires, each with its net.
    pub wires: Vec<(usize, [P; 2])>,
    /// Labels: their net, where they go, and the side of the pin whose stub they're on
    /// (the way the stub heads out).
    pub labels: Vec<(usize, P, Side)>,
    /// Nets drawn with labels rather than wires.
    pub labelled: BTreeSet<usize>,
}

/// Lays out `blocks` joined by `nets`.
pub fn layout(blocks: &[Block], nets: &[Net]) -> Layout {
    let mut wired: Vec<bool> = nets.iter().map(|net| net.wired && net.pins.len() >= 2).collect();
    let layer = layers(blocks, nets);
    // Each pass either routes every channel or gives up wiring the nets that couldn't
    // be, and lays out again without them.
    loop {
        let mut plan = Plan::new(blocks, nets, &layer, &wired);
        plan.order();
        plan.place_vertically();
        match plan.route() {
            Ok(routes) => return plan.finish(&routes, &wired),
            Err(stuck) => {
                for n in stuck {
                    wired[n] = false;
                }
            }
        }
    }
}

/// The pin whose wire carries a named net's label: a driving output if there is one.
fn name_pin(blocks: &[Block], net: &Net) -> Option<(usize, usize)> {
    if !net.named {
        return None;
    }
    let pin = |&&(b, p): &&(usize, usize)| &blocks[b].pins[p];
    net.pins
        .iter()
        .find(|bp| pin(bp).drives && pin(bp).side == Side::Right)
        .or_else(|| net.pins.iter().find(|bp| pin(bp).side == Side::Right))
        .or_else(|| net.pins.first())
        .copied()
}

/// Whether `net` is drawn with labels: it isn't wired, and it either joins pins or is
/// meant to be labelled (a global net joins others by its name, even at one pin).
fn labelled(net: &Net, wired: bool) -> bool {
    !wired && (net.pins.len() >= 2 || !net.wired)
}

/// Each block's column: longest path from the sources once feedback is set aside,
/// then each block moved up to just before the first one it feeds.
fn layers(blocks: &[Block], nets: &[Net]) -> Vec<usize> {
    let n = blocks.len();
    let mut succ: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    for net in nets.iter().filter(|net| net.wired && net.pins.len() >= 2) {
        let side = |&&(b, p): &&(usize, usize)| blocks[b].pins[p].side;
        let mut drivers: Vec<&(usize, usize)> =
            net.pins.iter().filter(|&&(b, p)| blocks[b].pins[p].drives).collect();
        if drivers.is_empty() {
            drivers = net.pins.iter().filter(|pin| side(pin) == Side::Right).collect();
        }
        if drivers.is_empty() {
            drivers = net.pins.iter().take(1).collect();
        }
        for &&(d, dp) in &drivers {
            for &(s, sp) in &net.pins {
                if s != d && !(blocks[s].pins[sp].drives && blocks[d].pins[dp].drives) {
                    succ[d].insert(s);
                }
            }
        }
    }
    // Loops are cut where they cost least: blocks in the order of Eades, Lin and Smyth's
    // greedy heuristic (sinks to the end, sources to the front, otherwise the block
    // that most feeds others than is fed), and every connection running backwards
    // in that order set aside. Several nets between two blocks weigh more.
    let mut weight: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for (v, s) in succ.iter().enumerate() {
        for &t in s {
            *weight.entry((v, t)).or_default() += 1;
        }
    }
    for net in nets.iter().filter(|net| net.wired && net.pins.len() >= 2) {
        let blocks_on: BTreeSet<usize> = net.pins.iter().map(|&(b, _)| b).collect();
        for &a in &blocks_on {
            for &b in &blocks_on {
                if let Some(w) = weight.get_mut(&(a, b)) {
                    *w += 1;
                }
            }
        }
    }
    let mut out_w = vec![0usize; n];
    let mut in_w = vec![0usize; n];
    for (&(a, b), &w) in &weight {
        out_w[a] += w;
        in_w[b] += w;
    }
    let mut pred: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); n];
    for (v, s) in succ.iter().enumerate() {
        for &t in s {
            pred[t].insert(v);
        }
    }
    let mut left: BTreeSet<usize> = (0..n).collect();
    let mut front: Vec<usize> = Vec::new();
    let mut back: Vec<usize> = Vec::new();
    let remove =
        |v: usize, left: &mut BTreeSet<usize>, out_w: &mut Vec<usize>, in_w: &mut Vec<usize>| {
            left.remove(&v);
            for &t in &succ[v] {
                in_w[t] -= weight[&(v, t)];
            }
            for &u in &pred[v] {
                out_w[u] -= weight[&(u, v)];
            }
        };
    // Inputs lead and outputs trail whatever their connections.
    for v in (0..n).filter(|&v| blocks[v].column == Column::First) {
        front.push(v);
        remove(v, &mut left, &mut out_w, &mut in_w);
    }
    for v in (0..n).filter(|&v| blocks[v].column == Column::Last) {
        back.push(v);
        remove(v, &mut left, &mut out_w, &mut in_w);
    }
    while !left.is_empty() {
        let mut changed = true;
        while changed {
            changed = false;
            let sinks: Vec<usize> = left.iter().copied().filter(|&v| out_w[v] == 0).collect();
            for v in sinks {
                back.push(v);
                remove(v, &mut left, &mut out_w, &mut in_w);
                changed = true;
            }
            let sources: Vec<usize> = left.iter().copied().filter(|&v| in_w[v] == 0).collect();
            for v in sources {
                front.push(v);
                remove(v, &mut left, &mut out_w, &mut in_w);
                changed = true;
            }
        }
        let best = left
            .iter()
            .copied()
            .max_by_key(|&v| (out_w[v] as i64 - in_w[v] as i64, std::cmp::Reverse(v)));
        if let Some(v) = best {
            front.push(v);
            remove(v, &mut left, &mut out_w, &mut in_w);
        }
    }
    front.extend(back.into_iter().rev());
    let mut position = vec![0usize; n];
    for (k, &v) in front.iter().enumerate() {
        position[v] = k;
    }
    let forward: Vec<Vec<usize>> = (0..n)
        .map(|v| succ[v].iter().copied().filter(|&t| position[t] > position[v]).collect())
        .collect();
    // Longest path over the remaining edges, in topological order.
    let mut indeg = vec![0usize; n];
    for f in &forward {
        for &t in f {
            indeg[t] += 1;
        }
    }
    let has_first = blocks.iter().any(|b| b.column == Column::First);
    let mut layer: Vec<usize> =
        blocks.iter().map(|b| usize::from(has_first && b.column != Column::First)).collect();
    let mut queue: Vec<usize> = (0..n).filter(|&b| indeg[b] == 0).collect();
    let mut topo = Vec::with_capacity(n);
    while let Some(v) = queue.pop() {
        topo.push(v);
        for &t in &forward[v] {
            layer[t] = layer[t].max(layer[v] + 1);
            indeg[t] -= 1;
            if indeg[t] == 0 {
                queue.push(t);
            }
        }
    }
    // Blocks feeding only later columns move up to meet them.
    for &v in topo.iter().rev() {
        if blocks[v].column != Column::Any {
            continue;
        }
        let ahead = forward[v]
            .iter()
            .filter(|&&t| blocks[t].column == Column::Any)
            .map(|&t| layer[t])
            .min();
        if let Some(a) = ahead {
            layer[v] = layer[v].max(a.saturating_sub(1));
        }
    }
    // Outputs last, together.
    let inner = (0..n).filter(|&b| blocks[b].column != Column::Last).map(|b| layer[b]).max();
    let last = inner.map_or(0, |m| m + 1);
    for (b, l) in layer.iter_mut().enumerate() {
        match blocks[b].column {
            Column::First => *l = 0,
            Column::Last => *l = last,
            Column::Any => {}
        }
    }
    layer
}

/// A block, or a net passing through a column between blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Block(usize),
    Pass(usize),
}

/// One end of a net in a channel: an item, its pin's offset from the item's
/// reference, and whether the item is the column left of the channel.
#[derive(Debug, Clone, Copy)]
struct Term {
    item: usize,
    pin: Option<usize>,
    offset: i64,
    left: bool,
}

/// A net's vertical wire in a channel, or none when its ends line up.
#[derive(Debug, Clone, Copy)]
struct Route {
    track: Option<usize>,
}

struct Plan<'a> {
    blocks: &'a [Block],
    nets: &'a [Net],
    items: Vec<(usize, Item)>,
    columns: Vec<Vec<usize>>,
    /// Channel `c + 1` lies right of column `c` (channel 0 is left of everything).
    channels: Vec<BTreeMap<usize, Vec<Term>>>,
    /// Each item's ends in each channel: (channel, net, end).
    ends: Vec<Vec<(usize, usize, Term)>>,
    /// Each item's reference y, and its extent above and below it.
    y: Vec<i64>,
    extent: Vec<(i64, i64)>,
    /// For each wired pin on a block's top or bottom: the height of its lane from the
    /// block's origin, and whether it heads right.
    lanes: BTreeMap<(usize, usize), (i64, bool)>,
}

impl<'a> Plan<'a> {
    fn new(blocks: &'a [Block], nets: &'a [Net], layer: &[usize], wired: &[bool]) -> Plan<'a> {
        let width = layer.iter().max().map_or(0, |m| m + 1);
        let mut items: Vec<(usize, Item)> = Vec::new();
        let mut columns = vec![Vec::new(); width];
        for (b, &l) in layer.iter().enumerate() {
            columns[l].push(items.len());
            items.push((l, Item::Block(b)));
        }
        let mut channels: Vec<BTreeMap<usize, Vec<Term>>> = vec![BTreeMap::new(); width + 1];
        // Which way each wired top or bottom pin heads: toward most of its net.
        let mut heads: BTreeMap<(usize, usize), bool> = BTreeMap::new();
        for (k, net) in nets.iter().enumerate() {
            if !wired[k] {
                continue;
            }
            for &(b, p) in &net.pins {
                if matches!(blocks[b].pins[p].side, Side::Top | Side::Bottom) {
                    let ahead = net.pins.iter().filter(|&&(o, _)| layer[o] > layer[b]).count();
                    let behind = net.pins.iter().filter(|&&(o, _)| layer[o] < layer[b]).count();
                    heads.insert((b, p), ahead >= behind);
                }
            }
        }
        // A labelled pin's stub and label reach out of the block's top or bottom, and the
        // lanes run beyond them, the ones nearest the side they head for innermost.
        let mut extent: Vec<(i64, i64)> = Vec::new();
        let mut lanes: BTreeMap<(usize, usize), (i64, bool)> = BTreeMap::new();
        for (b, block) in blocks.iter().enumerate() {
            let mut up = -block.min.1;
            let mut down = block.max.1;
            let mut top = i64::MAX;
            let mut bottom = i64::MIN;
            for (k, net) in nets.iter().enumerate() {
                for &(nb, p) in net.pins.iter().filter(|(nb, _)| *nb == b) {
                    let pin = &blocks[nb].pins[p];
                    let reach =
                        if labelled(net, wired[k]) { RISER + net.mark.0.max(UNIT) } else { 0 };
                    match pin.side {
                        Side::Top => {
                            up = up.max(-pin.at.1 + reach);
                            top = top.min(pin.at.1 - reach.max(RISER - PITCH) - PITCH);
                        }
                        Side::Bottom => {
                            down = down.max(pin.at.1 + reach);
                            bottom = bottom.max(pin.at.1 + reach.max(RISER - PITCH) + PITCH);
                        }
                        _ => {}
                    }
                }
            }
            for (side, base, step) in [(Side::Top, top, -PITCH), (Side::Bottom, bottom, PITCH)] {
                let mut mine: Vec<(bool, i64, usize)> = heads
                    .iter()
                    .filter(|&(&(hb, p), _)| hb == b && blocks[b].pins[p].side == side)
                    .map(|(&(_, p), &right)| {
                        let x = blocks[b].pins[p].at.0;
                        (!right, if right { -x } else { x }, p)
                    })
                    .collect();
                mine.sort();
                for (level, &(left, _, p)) in mine.iter().enumerate() {
                    let y = base + level as i64 * step;
                    lanes.insert((b, p), (y, !left));
                    match side {
                        Side::Top => up = up.max(-y),
                        _ => down = down.max(y),
                    }
                }
            }
            extent.push((up, down));
        }
        for (k, net) in nets.iter().enumerate() {
            if !wired[k] {
                continue;
            }
            let channel = |&(b, p): &(usize, usize)| match blocks[b].pins[p].side {
                Side::Left => layer[b],
                Side::Right => layer[b] + 1,
                _ if lanes[&(b, p)].1 => layer[b] + 1,
                _ => layer[b],
            };
            let lo = net.pins.iter().map(channel).min().expect("pins");
            let hi = net.pins.iter().map(channel).max().expect("pins");
            for &(b, p) in &net.pins {
                let pin = &blocks[b].pins[p];
                let (offset, left) = match lanes.get(&(b, p)) {
                    Some(&(y, right)) => (y, right),
                    None => (pin.at.1, pin.side == Side::Right),
                };
                channels[channel(&(b, p))].entry(k).or_default().push(Term {
                    item: b,
                    pin: Some(p),
                    offset,
                    left,
                });
            }
            // Through each column between, beside its blocks.
            for l in lo..hi {
                let item = items.len();
                columns[l].push(item);
                items.push((l, Item::Pass(k)));
                extent.push((0, 0));
                let term = |left| Term { item, pin: None, offset: 0, left };
                channels[l].entry(k).or_default().push(term(false));
                channels[l + 1].entry(k).or_default().push(term(true));
            }
        }
        let mut ends = vec![Vec::new(); items.len()];
        for (c, channel) in channels.iter().enumerate() {
            for (&k, terms) in channel {
                for t in terms {
                    ends[t.item].push((c, k, *t));
                }
            }
        }
        let y = vec![0; items.len()];
        Plan { blocks, nets, items, columns, channels, ends, y, extent, lanes }
    }

    /// Orders each column to keep wires short and crossings few: each item goes
    /// to the average place of what it connects to in the column before (or after).
    fn order(&mut self) {
        let mut rank = vec![0.0f64; self.items.len()];
        let set_ranks = |columns: &Vec<Vec<usize>>, rank: &mut Vec<f64>| {
            for col in columns {
                for (r, &i) in col.iter().enumerate() {
                    rank[i] = r as f64;
                }
            }
        };
        set_ranks(&self.columns, &mut rank);
        for sweep in 0..8 {
            let forward = sweep % 2 == 0;
            let order: Vec<usize> = if forward {
                (1..self.columns.len()).collect()
            } else {
                (0..self.columns.len().saturating_sub(1)).rev().collect()
            };
            for l in order {
                // The channel facing the neighbour column, and which side of it we're on.
                let (c, ours_left) = if forward { (l, false) } else { (l + 1, true) };
                let mut keyed: Vec<(f64, usize)> = self.columns[l]
                    .iter()
                    .map(|&i| {
                        let mut sum = 0.0;
                        let mut count = 0usize;
                        for &(ec, k, t) in &self.ends[i] {
                            if ec != c || t.left != ours_left {
                                continue;
                            }
                            for o in self.channels[c][&k].iter().filter(|o| o.left != ours_left) {
                                sum += rank[o.item] + o.offset as f64 * 1e-12;
                                count += 1;
                            }
                        }
                        let key = if count == 0 { rank[i] } else { sum / count as f64 };
                        (key, i)
                    })
                    .collect();
                keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
                self.columns[l] = keyed.into_iter().map(|(_, i)| i).collect();
                for (r, &i) in self.columns[l].iter().enumerate() {
                    rank[i] = r as f64;
                }
            }
        }
    }

    /// Heights: each item as near as it can get to lining its pins up with what they
    /// connect to, keeping the column's order and the gaps between items.
    fn place_vertically(&mut self) {
        for l in 0..self.columns.len() {
            self.stack(l, None);
        }
        let n = self.columns.len();
        for _ in 0..4 {
            for l in 1..n {
                self.stack(l, Some(false));
            }
            for l in (0..n.saturating_sub(1)).rev() {
                self.stack(l, Some(true));
            }
        }
        for l in 0..n {
            self.stack(l, None);
        }
        // Everything below y = 0.
        let top = (0..self.items.len()).map(|i| self.y[i] - self.extent[i].0).min().unwrap_or(0);
        for y in &mut self.y {
            *y -= top;
        }
    }

    /// Places column `l`'s items, pulled toward their neighbours on one side
    /// (`Some(left)`: the side where our ends are `left`), or both.
    fn stack(&mut self, l: usize, toward: Option<bool>) {
        let col = &self.columns[l];
        let mut wants: Vec<Vec<(i64, u32)>> = Vec::with_capacity(col.len());
        for &i in col {
            let mut w = vec![(self.y[i], 1)];
            for &(c, k, t) in &self.ends[i] {
                if toward.is_some_and(|left| t.left != left) {
                    continue;
                }
                for o in self.channels[c][&k].iter().filter(|o| o.left != t.left) {
                    w.push((self.y[o.item] + o.offset - t.offset, 2));
                }
            }
            wants.push(w);
        }
        let sep: Vec<i64> =
            col.windows(2).map(|w| self.extent[w[0]].1 + GAP + self.extent[w[1]].0).collect();
        let placed = settle(&wants, &sep);
        for (&i, y) in col.iter().zip(placed) {
            self.y[i] = y;
        }
    }

    fn x_of(&self, cols: &Columns, t: &Term) -> i64 {
        let (l, item) = self.items[t.item];
        match (item, t.pin) {
            (Item::Block(b), Some(p)) if !self.lanes.contains_key(&(b, p)) => {
                cols.origin_x(self.blocks, l, b) + self.blocks[b].pins[p].at.0
            }
            _ if t.left => cols.x[l] + cols.width[l],
            _ => cols.x[l],
        }
    }

    /// Chooses each net's track in each channel. Fails with the nets to give up on when
    /// two nets each need to be left of the other.
    #[allow(clippy::type_complexity)]
    fn route(&self) -> Result<Vec<BTreeMap<usize, Route>>, Vec<usize>> {
        let mut out = Vec::with_capacity(self.channels.len());
        let mut stuck = Vec::new();
        for channel in &self.channels {
            let abs = |t: &Term| self.y[t.item] + t.offset;
            let mut spans: BTreeMap<usize, (i64, i64)> = BTreeMap::new();
            let mut at: BTreeMap<i64, (Option<usize>, Option<usize>)> = BTreeMap::new();
            for (&k, terms) in channel {
                let lo = terms.iter().map(abs).min().expect("terms");
                let hi = terms.iter().map(abs).max().expect("terms");
                if lo != hi {
                    spans.insert(k, (lo, hi));
                }
                for t in terms {
                    let slot = at.entry(abs(t)).or_default();
                    if t.left { slot.0 = Some(k) } else { slot.1 = Some(k) }
                }
            }
            // A stub from the left and one from the right at the same height must not
            // meet: the left one's track goes first.
            let mut before: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
            for (a, b) in at.values().filter_map(|&(a, b)| Some((a?, b?))) {
                if a != b && spans.contains_key(&a) && spans.contains_key(&b) {
                    before.entry(b).or_default().insert(a);
                }
            }
            let mut track: BTreeMap<usize, usize> = BTreeMap::new();
            let mut t = 0;
            while track.len() < spans.len() {
                let mut ready: Vec<(i64, i64, usize)> = spans
                    .iter()
                    .filter(|(k, _)| !track.contains_key(k))
                    .filter(|(k, _)| {
                        before.get(k).is_none_or(|b| {
                            b.iter().all(|a| track.get(a).is_some_and(|&ta| ta < t))
                        })
                    })
                    .map(|(&k, &(lo, hi))| (lo, hi, k))
                    .collect();
                if ready.is_empty() {
                    // A loop of constraints: give up on its smallest net.
                    let k = spans
                        .keys()
                        .filter(|k| !track.contains_key(k))
                        .min_by_key(|&&k| (self.nets[k].pins.len(), k))
                        .copied()
                        .expect("unplaced");
                    stuck.push(k);
                    break;
                }
                ready.sort();
                let mut end = i64::MIN;
                for (lo, hi, k) in ready {
                    if lo > end {
                        track.insert(k, t);
                        end = hi;
                    }
                }
                t += 1;
            }
            // Packing settles which nets share a track, not the tracks' order: put them
            // in the order with the fewest crossings that keeps the constraints.
            let ends = |k: usize| -> Vec<(i64, bool)> {
                channel[&k].iter().map(|t| (abs(t), t.left)).collect()
            };
            let order = order_tracks(t, &track, &spans, &before, ends);
            for v in track.values_mut() {
                *v = order[*v];
            }
            untangle(t, &mut track, &spans, &before, ends);
            out.push(
                channel.keys().map(|&k| (k, Route { track: track.get(&k).copied() })).collect(),
            );
        }
        if stuck.is_empty() { Ok(out) } else { Err(stuck) }
    }

    fn finish(&self, routes: &[BTreeMap<usize, Route>], wired: &[bool]) -> Layout {
        let tracks: Vec<usize> = routes
            .iter()
            .map(|r| r.values().filter_map(|r| r.track).max().map_or(0, |t| t + 1))
            .collect();
        // Room in each channel, before its first track and after its last, for what
        // marks the nets at the pins either side.
        let mut before = vec![0i64; self.channels.len()];
        let mut after = vec![0i64; self.channels.len()];
        let column: BTreeMap<usize, usize> = self
            .items
            .iter()
            .filter_map(|&(l, item)| if let Item::Block(b) = item { Some((b, l)) } else { None })
            .collect();
        for (k, net) in self.nets.iter().enumerate() {
            let marked: Vec<(usize, usize)> = if labelled(net, wired[k]) {
                net.pins.clone()
            } else if wired[k] {
                name_pin(self.blocks, net).into_iter().collect()
            } else {
                Vec::new()
            };
            for (b, p) in marked {
                let room = UNIT + net.mark.0 + UNIT;
                match self.blocks[b].pins[p].side {
                    Side::Right => before[column[&b] + 1] = before[column[&b] + 1].max(room),
                    Side::Left => after[column[&b]] = after[column[&b]].max(room),
                    _ => {}
                }
            }
        }
        let snap = |v: i64| (v + UNIT - 1).div_euclid(UNIT) * UNIT;
        let before: Vec<i64> = before.into_iter().map(|v| snap(v).max(PITCH)).collect();
        let after: Vec<i64> = after.into_iter().map(|v| snap(v).max(PITCH)).collect();
        let cols = Columns::new(self, &tracks, &before, &after);
        let mut out = Layout { origins: vec![(0, 0); self.blocks.len()], ..Layout::default() };
        for (i, &(l, item)) in self.items.iter().enumerate() {
            if let Item::Block(b) = item {
                out.origins[b] = (cols.origin_x(self.blocks, l, b), self.y[i]);
            }
        }
        // Pins on blocks' tops and bottoms: up (or down) to their lanes and along them.
        for (&(b, p), &(lane, right)) in &self.lanes {
            let k = self.nets.iter().position(|n| n.pins.contains(&(b, p))).expect("on a net");
            let l = self.items.iter().find(|&&(_, i)| i == Item::Block(b)).expect("placed").0;
            let at = self.blocks[b].pins[p].at;
            let (x, y0) = (out.origins[b].0 + at.0, out.origins[b].1 + at.1);
            let y = out.origins[b].1 + lane;
            let edge = if right { cols.x[l] + cols.width[l] } else { cols.x[l] };
            out.wires.push((k, [(x, y0), (x, y)]));
            if edge != x {
                out.wires.push((k, [(x, y), (edge, y)]));
            }
        }
        // Nets passing through columns.
        for (i, &(l, item)) in self.items.iter().enumerate() {
            if let Item::Pass(k) = item {
                if cols.width[l] > 0 {
                    out.wires.push((
                        k,
                        [(cols.x[l], self.y[i]), (cols.x[l] + cols.width[l], self.y[i])],
                    ));
                }
            }
        }
        // Tracks and stubs.
        for (c, channel) in self.channels.iter().enumerate() {
            let edge = cols.channel_x(c);
            for (&k, terms) in channel {
                let abs = |t: &Term| self.y[t.item] + t.offset;
                match routes[c][&k].track {
                    None => {
                        let y = abs(&terms[0]);
                        let xs: Vec<i64> = terms.iter().map(|t| self.x_of(&cols, t)).collect();
                        let (lo, hi) = (*xs.iter().min().expect("x"), *xs.iter().max().expect("x"));
                        if lo != hi {
                            out.wires.push((k, [(lo, y), (hi, y)]));
                        }
                    }
                    Some(t) => {
                        let x = edge + cols.before[c] + t as i64 * PITCH;
                        let lo = terms.iter().map(abs).min().expect("y");
                        let hi = terms.iter().map(abs).max().expect("y");
                        out.wires.push((k, [(x, lo), (x, hi)]));
                        for term in terms {
                            out.wires
                                .push((k, [(self.x_of(&cols, term), abs(term)), (x, abs(term))]));
                        }
                    }
                }
            }
        }
        // Names on wired nets, just out from one of their pins.
        for (k, net) in self.nets.iter().enumerate() {
            if !wired[k] {
                continue;
            }
            if let Some((b, p)) = name_pin(self.blocks, net) {
                let pin = &self.blocks[b].pins[p];
                let (x, y) = (out.origins[b].0 + pin.at.0, out.origins[b].1 + pin.at.1);
                let at = match pin.side {
                    Side::Left => (x - UNIT, y),
                    Side::Right => (x + UNIT, y),
                    Side::Top => (x, y - UNIT),
                    Side::Bottom => (x, y + UNIT),
                };
                out.labels.push((k, at, pin.side));
            }
        }
        // Everything else by label: a short stub out of each pin, and its name there.
        for (k, net) in self.nets.iter().enumerate() {
            if !labelled(net, wired[k]) {
                continue;
            }
            out.labelled.insert(k);
            for &(b, p) in &net.pins {
                let pin = &self.blocks[b].pins[p];
                let from = (out.origins[b].0 + pin.at.0, out.origins[b].1 + pin.at.1);
                let to = match pin.side {
                    Side::Left => (from.0 - UNIT, from.1),
                    Side::Right => (from.0 + UNIT, from.1),
                    Side::Top => (from.0, from.1 - RISER),
                    Side::Bottom => (from.0, from.1 + RISER),
                };
                out.wires.push((k, [from, to]));
                out.labels.push((k, to, pin.side));
            }
        }
        out
    }
}

/// Where the columns and channels go across the sheet.
struct Columns {
    /// Each column's left edge and width.
    x: Vec<i64>,
    width: Vec<i64>,
    /// Where each channel's first track is, from its left edge.
    before: Vec<i64>,
    /// The inputs' column lines up on the right; the others on the left.
    right_aligned: Vec<bool>,
}

impl Columns {
    fn new(plan: &Plan<'_>, tracks: &[usize], before: &[i64], after: &[i64]) -> Columns {
        let n = plan.columns.len();
        let mut width = vec![0; n];
        let mut right_aligned = vec![false; n];
        for &(l, item) in &plan.items {
            if let Item::Block(b) = item {
                let block = &plan.blocks[b];
                width[l] = width[l].max(block.max.0 - block.min.0);
                right_aligned[l] |= block.column == Column::First;
            }
        }
        // Room for the tracks, and for what marks the nets at each side.
        let channel: Vec<i64> = (0..tracks.len())
            .map(|c| before[c] + tracks[c].saturating_sub(1) as i64 * PITCH + after[c])
            .collect();
        let mut x = vec![0; n];
        let mut at = channel[0];
        for l in 0..n {
            x[l] = at;
            at += width[l] + channel[l + 1];
        }
        Columns { x, width, before: before.to_vec(), right_aligned }
    }

    /// The left edge of channel `c`.
    fn channel_x(&self, c: usize) -> i64 {
        if c == 0 { 0 } else { self.x[c - 1] + self.width[c - 1] }
    }

    fn origin_x(&self, blocks: &[Block], l: usize, b: usize) -> i64 {
        let block = &blocks[b];
        if self.right_aligned[l] {
            self.x[l] + self.width[l] - block.max.0
        } else {
            self.x[l] - block.min.0
        }
    }
}

/// Positions for items stacked in order, each as near as it can get to the places it
/// wants to be (weighted by how much it wants each), where item `i + 1` must be at
/// least `sep[i]` below item `i`. Isotonic regression, by pooling adjacent violators,
/// with the weighted median as each pool's best place.
fn settle(wants: &[Vec<(i64, u32)>], sep: &[i64]) -> Vec<i64> {
    let mut offset = vec![0i64; wants.len()];
    for i in 1..wants.len() {
        offset[i] = offset[i - 1] + sep[i - 1];
    }
    // Pools of consecutive items: how many, their shifted wishes, and their best
    // shifted place.
    type Pool = (usize, Vec<(i64, u32)>, i64);
    let mut pools: Vec<Pool> = Vec::new();
    for (i, w) in wants.iter().enumerate() {
        let mut values: Vec<(i64, u32)> = w.iter().map(|&(v, wt)| (v - offset[i], wt)).collect();
        let mut count = 1;
        let mut best = median(&mut values);
        while let Some((c, v, b)) = pools.last_mut() {
            if *b <= best {
                break;
            }
            values.append(v);
            count += *c;
            best = median(&mut values);
            pools.pop();
        }
        pools.push((count, values, best));
    }
    let mut out = Vec::with_capacity(wants.len());
    for (count, _, best) in pools {
        for _ in 0..count {
            out.push(best + offset[out.len()]);
        }
    }
    out
}

/// The weighted lower median.
fn median(values: &mut [(i64, u32)]) -> i64 {
    values.sort_unstable();
    let total: u64 = values.iter().map(|&(_, w)| u64::from(w)).sum();
    let mut seen = 0u64;
    for &(v, w) in values.iter() {
        seen += u64::from(w);
        if 2 * seen >= total {
            return v;
        }
    }
    values.last().map_or(0, |&(v, _)| v)
}

/// Where each of `count` tracks goes, left to right, given which track each net with a
/// vertical span is on: adjacent tracks are swapped while that crosses fewer wires and
/// keeps every net after those it must come `before`. A net's stubs (`ends`: height,
/// and whether it comes from the left) cross the vertical of each net on the far side
/// of it whose span they fall inside.
fn order_tracks(
    count: usize,
    track: &BTreeMap<usize, usize>,
    spans: &BTreeMap<usize, (i64, i64)>,
    before: &BTreeMap<usize, BTreeSet<usize>>,
    ends: impl Fn(usize) -> Vec<(i64, bool)>,
) -> Vec<usize> {
    let mut on: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (&k, &t) in track {
        on[t].push(k);
    }
    let ends: BTreeMap<usize, Vec<(i64, bool)>> = track.keys().map(|&k| (k, ends(k))).collect();
    let within = |y: i64, k: usize| spans.get(&k).is_some_and(|&(lo, hi)| lo < y && y < hi);
    // Crossings between nets `a` and `b` with `a`'s track left of `b`'s: `a`'s stubs to
    // the right edge cross `b`'s vertical, `b`'s from the left edge cross `a`'s.
    let pair = |a: usize, b: usize| {
        let right = ends[&a].iter().filter(|&&(y, left)| !left && within(y, b)).count();
        let left = ends[&b].iter().filter(|&&(y, left)| left && within(y, a)).count();
        right + left
    };
    let cost = |u: usize, v: usize| -> usize {
        on[u].iter().map(|&a| on[v].iter().map(|&b| pair(a, b)).sum::<usize>()).sum()
    };
    let must_precede = |u: usize, v: usize| {
        on[v].iter().any(|b| before.get(b).is_some_and(|set| on[u].iter().any(|a| set.contains(a))))
    };
    let mut seq: Vec<usize> = (0..count).collect();
    for _ in 0..count {
        let mut swapped = false;
        for i in 0..count.saturating_sub(1) {
            let (u, v) = (seq[i], seq[i + 1]);
            if !must_precede(u, v) && cost(v, u) < cost(u, v) {
                seq.swap(i, i + 1);
                swapped = true;
            }
        }
        if !swapped {
            break;
        }
    }
    let mut place = vec![0; count];
    for (i, &t) in seq.iter().enumerate() {
        place[t] = i;
    }
    place
}

/// Moves single nets to other tracks, or exchanges two nets' tracks, while that crosses
/// fewer wires, keeping nets on a track apart and every net after those it must come
/// `before`.
fn untangle(
    count: usize,
    track: &mut BTreeMap<usize, usize>,
    spans: &BTreeMap<usize, (i64, i64)>,
    before: &BTreeMap<usize, BTreeSet<usize>>,
    ends: impl Fn(usize) -> Vec<(i64, bool)>,
) {
    let nets: Vec<usize> = track.keys().copied().collect();
    let n = nets.len();
    let index: BTreeMap<usize, usize> = nets.iter().enumerate().map(|(i, &k)| (k, i)).collect();
    let span: Vec<(i64, i64)> = nets.iter().map(|k| spans[k]).collect();
    let ends: Vec<Vec<(i64, bool)>> = nets.iter().map(|&k| ends(k)).collect();
    let within = |y: i64, i: usize| span[i].0 < y && y < span[i].1;
    // cross[i][j]: the crossings between nets i and j with i's track left of j's.
    let cross: Vec<Vec<u32>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| {
                    let right = ends[i].iter().filter(|&&(y, left)| !left && within(y, j)).count();
                    let left = ends[j].iter().filter(|&&(y, left)| left && within(y, i)).count();
                    (right + left) as u32
                })
                .collect()
        })
        .collect();
    let mut pos: Vec<usize> = nets.iter().map(|k| track[k]).collect();
    // Which nets must come before which, by index.
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (b, set) in before {
        let Some(&bi) = index.get(b) else { continue };
        for a in set {
            if let Some(&ai) = index.get(a) {
                preds[bi].push(ai);
                succs[ai].push(bi);
            }
        }
    }
    // The crossings net i has with the others (but `skip`) were it on track t.
    let crossings = |pos: &[usize], i: usize, t: usize, skip: usize| -> u32 {
        (0..n)
            .filter(|&o| o != i && o != skip)
            .map(|o| match pos[o].cmp(&t) {
                std::cmp::Ordering::Less => cross[o][i],
                std::cmp::Ordering::Greater => cross[i][o],
                std::cmp::Ordering::Equal => 0,
            })
            .sum()
    };
    // Whether net i fits on track t, with the others where `pos` has them.
    let fits = |pos: &[usize], i: usize, t: usize| {
        let (lo, hi) = span[i];
        (0..n).all(|o| o == i || pos[o] != t || span[o].1 < lo || hi < span[o].0)
            && preds[i].iter().all(|&a| pos[a] < t)
            && succs[i].iter().all(|&b| t < pos[b])
    };
    for _ in 0..4 {
        let mut better = false;
        for a in 0..n {
            let ta = pos[a];
            let here = crossings(&pos, a, ta, a);
            if here == 0 {
                continue;
            }
            // Somewhere else on its own.
            if let Some(t) = (0..count)
                .filter(|&t| t != ta && fits(&pos, a, t))
                .min_by_key(|&t| crossings(&pos, a, t, a))
                .filter(|&t| crossings(&pos, a, t, a) < here)
            {
                pos[a] = t;
                better = true;
                continue;
            }
            // Or trading places with another net.
            for b in 0..n {
                let tb = pos[b];
                if tb == ta {
                    continue;
                }
                let between =
                    |ta: usize, tb: usize| if ta < tb { cross[a][b] } else { cross[b][a] };
                let now = crossings(&pos, a, ta, b) + crossings(&pos, b, tb, a) + between(ta, tb);
                let then = {
                    pos.swap(a, b);
                    let then =
                        crossings(&pos, a, tb, b) + crossings(&pos, b, ta, a) + between(tb, ta);
                    let ok = fits(&pos, a, tb) && fits(&pos, b, ta);
                    pos.swap(a, b);
                    if ok { then } else { u32::MAX }
                };
                if then < now {
                    pos.swap(a, b);
                    better = true;
                    break;
                }
            }
        }
        if !better {
            break;
        }
    }
    for (i, k) in nets.iter().enumerate() {
        track.insert(*k, pos[i]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box `w` by `h` units with inputs on the left and outputs on the right, one
    /// unit apart from the top.
    fn block(ins: usize, outs: usize) -> Block {
        let rows = ins.max(outs).max(1) as i64;
        let h = (rows + 1) * 2 * UNIT;
        let pin = |side, k: usize, drives| Pin {
            at: (if side == Side::Left { 0 } else { 8 * UNIT }, (k as i64 + 1) * 2 * UNIT),
            side,
            drives,
        };
        let mut pins: Vec<Pin> = (0..ins).map(|k| pin(Side::Left, k, false)).collect();
        pins.extend((0..outs).map(|k| pin(Side::Right, k, true)));
        Block { min: (0, 0), max: (8 * UNIT, h), pins, column: Column::Any }
    }

    fn net(pins: &[(usize, usize)]) -> Net {
        Net { pins: pins.to_vec(), wired: true, named: false, mark: (0, 0) }
    }

    /// Checks the promise: wires of different nets only ever cross, wires meet pins only
    /// at their ends and only their own net's pins, and labels sit on their own net.
    fn check(blocks: &[Block], nets: &[Net], l: &Layout) {
        let mut pin_net: BTreeMap<P, usize> = BTreeMap::new();
        for (b, block) in blocks.iter().enumerate() {
            for pin in &block.pins {
                pin_net.insert((l.origins[b].0 + pin.at.0, l.origins[b].1 + pin.at.1), usize::MAX);
            }
        }
        for (k, net) in nets.iter().enumerate() {
            for &(b, p) in &net.pins {
                let at = blocks[b].pins[p].at;
                pin_net.insert((l.origins[b].0 + at.0, l.origins[b].1 + at.1), k);
            }
        }
        let on = |p: P, s: &[P; 2]| {
            let (a, b) = (s[0], s[1]);
            (a.0 == b.0 && p.0 == a.0 && p.1 >= a.1.min(b.1) && p.1 <= a.1.max(b.1))
                || (a.1 == b.1 && p.1 == a.1 && p.0 >= a.0.min(b.0) && p.0 <= a.0.max(b.0))
        };
        for (i, (ka, a)) in l.wires.iter().enumerate() {
            assert!(a[0].0 == a[1].0 || a[0].1 == a[1].1, "straight: {a:?}");
            for (kb, b) in &l.wires[i + 1..] {
                if ka == kb {
                    continue;
                }
                for p in [a[0], a[1]] {
                    assert!(!on(p, b), "net {ka}'s end {p:?} touches net {kb}'s wire {b:?}");
                }
                for p in [b[0], b[1]] {
                    assert!(!on(p, a), "net {kb}'s end {p:?} touches net {ka}'s wire {a:?}");
                }
                let parallel = (a[0].0 == a[1].0) == (b[0].0 == b[1].0);
                if parallel {
                    let overlap = on(b[0], a) || on(b[1], a) || on(a[0], b) || on(a[1], b);
                    assert!(!overlap, "nets {ka} and {kb} overlap");
                }
            }
            for (&p, &k) in &pin_net {
                if on(p, a) {
                    assert_eq!(k, *ka, "net {ka}'s wire {a:?} touches net {k}'s pin");
                    assert!(p == a[0] || p == a[1], "wire {a:?} passes over a pin at {p:?}");
                }
            }
        }
        for &(k, p, _) in &l.labels {
            for (kw, w) in &l.wires {
                assert!(*kw == k || !on(p, w), "net {k}'s label touches net {kw}'s wire");
            }
        }
    }

    /// Whether `p` lies on segment `w`.
    fn on(p: P, w: &[P; 2]) -> bool {
        (w[0].0 == w[1].0
            && p.0 == w[0].0
            && p.1 >= w[0].1.min(w[1].1)
            && p.1 <= w[0].1.max(w[1].1))
            || (w[0].1 == w[1].1
                && p.1 == w[0].1
                && p.0 >= w[0].0.min(w[1].0)
                && p.0 <= w[0].0.max(w[1].0))
    }

    /// Whether every pin of every wired net is joined to the others by its wires: wires
    /// join where an end of one lies on the other.
    fn joined(blocks: &[Block], nets: &[Net], l: &Layout) {
        for (k, net) in nets.iter().enumerate() {
            if l.labelled.contains(&k) || net.pins.len() < 2 {
                continue;
            }
            let wires: Vec<[P; 2]> =
                l.wires.iter().filter(|(n, _)| *n == k).map(|(_, w)| *w).collect();
            let mut group: Vec<usize> = (0..wires.len()).collect();
            fn root(g: &mut [usize], i: usize) -> usize {
                if g[i] == i {
                    i
                } else {
                    let r = root(g, g[i]);
                    g[i] = r;
                    r
                }
            }
            for i in 0..wires.len() {
                for j in 0..wires.len() {
                    if on(wires[j][0], &wires[i]) || on(wires[j][1], &wires[i]) {
                        let (a, b) = (root(&mut group, i), root(&mut group, j));
                        group[a] = b;
                    }
                }
            }
            let mut groups = BTreeSet::new();
            for &(b, p) in &net.pins {
                let a = blocks[b].pins[p].at;
                let at = (l.origins[b].0 + a.0, l.origins[b].1 + a.1);
                let w = wires.iter().position(|w| w[0] == at || w[1] == at);
                let w = w.unwrap_or_else(|| panic!("net {k}: no wire ends at pin ({b}, {p})"));
                groups.insert(root(&mut group, w));
            }
            assert_eq!(groups.len(), 1, "net {k} is in pieces");
        }
    }

    #[test]
    fn a_chain_lines_up_in_straight_wires() {
        let blocks = vec![block(0, 1), block(1, 1), block(1, 0)];
        let nets = vec![net(&[(0, 0), (1, 0)]), net(&[(1, 1), (2, 0)])];
        let l = layout(&blocks, &nets);
        check(&blocks, &nets, &l);
        joined(&blocks, &nets, &l);
        assert!(l.origins[0].0 < l.origins[1].0 && l.origins[1].0 < l.origins[2].0);
        // Each net a single straight wire.
        assert_eq!(l.wires.len(), 2);
        assert!(l.wires.iter().all(|(_, w)| w[0].1 == w[1].1));
    }

    #[test]
    fn feedback_runs_back_between_the_blocks() {
        // A register feeding an adder feeding the register: a loop.
        let blocks = vec![block(1, 1), block(1, 1)];
        let nets = vec![net(&[(0, 1), (1, 0)]), net(&[(1, 1), (0, 0)])];
        let l = layout(&blocks, &nets);
        check(&blocks, &nets, &l);
        joined(&blocks, &nets, &l);
        assert!(l.labelled.is_empty());
    }

    #[test]
    fn fanout_shares_one_track() {
        let blocks = vec![block(0, 1), block(1, 0), block(1, 0), block(1, 0)];
        let nets = vec![net(&[(0, 0), (1, 0), (2, 0), (3, 0)])];
        let l = layout(&blocks, &nets);
        check(&blocks, &nets, &l);
        joined(&blocks, &nets, &l);
        let verticals = l.wires.iter().filter(|(_, w)| w[0].0 == w[1].0).count();
        assert_eq!(verticals, 1);
    }

    #[test]
    fn nets_not_to_be_wired_get_a_label_at_every_pin() {
        let mut b = block(1, 1);
        b.pins.push(Pin { at: (4 * UNIT, 0), side: Side::Top, drives: false });
        let blocks = vec![b.clone(), b];
        let mut power = net(&[(0, 2), (1, 2)]);
        power.wired = false;
        // Even alone: a global net joins others by its name.
        let mut lone = net(&[(0, 0)]);
        lone.wired = false;
        let nets = vec![net(&[(0, 1), (1, 0)]), power, lone];
        let l = layout(&blocks, &nets);
        check(&blocks, &nets, &l);
        joined(&blocks, &nets, &l);
        assert_eq!(l.labelled, BTreeSet::from([1, 2]));
        assert_eq!(l.labels.iter().filter(|(k, _, _)| *k == 1).count(), 2);
        assert_eq!(l.labels.iter().filter(|(k, _, _)| *k == 2).count(), 1);
    }

    #[test]
    fn top_and_bottom_pins_are_wired_along_lanes() {
        // Two parts with supply pins on top and bottom, joined, and a labelled ground.
        let mut b = block(1, 1);
        b.pins.push(Pin { at: (2 * UNIT, -2 * UNIT), side: Side::Top, drives: false });
        b.pins.push(Pin { at: (6 * UNIT, -2 * UNIT), side: Side::Top, drives: false });
        b.pins.push(Pin { at: (4 * UNIT, 8 * UNIT), side: Side::Bottom, drives: false });
        let blocks = vec![b.clone(), b.clone(), b];
        let mut gnd = net(&[(0, 4), (1, 4), (2, 4)]);
        gnd.wired = false;
        gnd.mark = (3 * UNIT, 2 * UNIT);
        let nets = vec![
            net(&[(0, 1), (1, 0)]),
            net(&[(1, 1), (2, 0)]),
            net(&[(0, 2), (1, 3), (2, 2)]),
            net(&[(0, 3), (2, 3)]),
            gnd,
        ];
        let l = layout(&blocks, &nets);
        check(&blocks, &nets, &l);
        joined(&blocks, &nets, &l);
        assert_eq!(l.labelled, BTreeSet::from([4]));
    }

    #[test]
    fn settling_keeps_order_and_gaps() {
        let wants = vec![vec![(10, 1)], vec![(0, 1)], vec![(100, 1)]];
        let y = settle(&wants, &[5, 5]);
        assert!(y[1] >= y[0] + 5 && y[2] >= y[1] + 5);
        assert_eq!(y[2], 100);
    }

    /// Random circuits: whatever the shape, the promise holds and every net is joined.
    #[test]
    fn random_circuits_route_cleanly() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rand = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..200 {
            let count = 2 + rand(14);
            let mut blocks: Vec<Block> =
                (0..count).map(|_| block(1 + rand(4), 1 + rand(3))).collect();
            // Some with pins on top and bottom too, joined up at random.
            let mut extra: Vec<(usize, usize)> = Vec::new();
            for (b, block) in blocks.iter_mut().enumerate() {
                for k in 0..rand(3) {
                    let side = if rand(2) == 0 { Side::Top } else { Side::Bottom };
                    let y = if side == Side::Top { -2 * UNIT } else { block.max.1 + 2 * UNIT };
                    let x = (2 + 2 * k as i64) * UNIT;
                    block.pins.push(Pin { at: (x, y), side, drives: false });
                    extra.push((b, block.pins.len() - 1));
                }
            }
            // Each input fed by some output (or left alone), outputs shared freely.
            let mut by_output: BTreeMap<(usize, usize), Vec<(usize, usize)>> = BTreeMap::new();
            for (b, block) in blocks.iter().enumerate() {
                for (p, pin) in block.pins.iter().enumerate() {
                    if pin.side == Side::Left && rand(5) > 0 {
                        let src = rand(count);
                        let outs: Vec<usize> = blocks[src]
                            .pins
                            .iter()
                            .enumerate()
                            .filter(|(_, q)| q.side == Side::Right)
                            .map(|(k, _)| k)
                            .collect();
                        by_output.entry((src, outs[rand(outs.len())])).or_default().push((b, p));
                    }
                }
            }
            let mut nets: Vec<Net> = by_output
                .into_iter()
                .map(|(o, mut sinks)| {
                    sinks.insert(0, o);
                    net(&sinks)
                })
                .collect();
            for pair in extra.chunks(2) {
                let mut n = net(pair);
                n.wired = rand(4) > 0;
                nets.push(n);
            }
            let l = layout(&blocks, &nets);
            check(&blocks, &nets, &l);
            joined(&blocks, &nets, &l);
        }
    }
}
