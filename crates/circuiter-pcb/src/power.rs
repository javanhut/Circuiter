//! Power on the board, as Altium's PDN Analyzer looks at it: how much current each
//! track can carry (IPC-2221's chart, as a formula), and how much voltage a net's
//! copper loses carrying the currents the circuit actually draws: the net becomes a
//! network of resistors (tracks by their length, width and copper thickness, vias by
//! their plated barrels, pours as near-solid joins), the simulated pin currents go in
//! at the pads, and solving it gives every track's current and every pad's voltage.

use crate::geometry;
use crate::layout::{Layout, P, p};
use crate::zones::ZoneFill;
use circuiter_model::board::{Layer, StackKind};

/// Copper's resistivity, Ω·mm (1.72 µΩ·cm).
const RHO: f64 = 1.72e-5;
/// Plating in a via's barrel, mm.
const PLATING: f64 = 0.025;
const MIL: f64 = 0.0254;

/// The current a track `width` mm wide of copper `thickness` mm can carry rising
/// `rise` °C, by IPC-2221 (I = k·ΔT^0.44·A^0.725, A in square mils; k 0.048 on an
/// outer layer, 0.024 inside).
pub fn current_for_width(width: f64, thickness: f64, rise: f64, outer: bool) -> f64 {
    let k = if outer { 0.048 } else { 0.024 };
    let area = (width / MIL) * (thickness / MIL);
    k * rise.powf(0.44) * area.powf(0.725)
}

/// The width a track needs to carry `amps` rising `rise` °C.
pub fn width_for_current(amps: f64, thickness: f64, rise: f64, outer: bool) -> f64 {
    let k = if outer { 0.048 } else { 0.024 };
    let area = (amps / (k * rise.powf(0.44))).powf(1.0 / 0.725);
    area / (thickness / MIL) * MIL
}

/// The copper's thickness on the board's layers, mm (the stack's first copper layer).
pub fn copper_thickness(layout: &Layout) -> f64 {
    layout
        .board
        .stack()
        .layers
        .iter()
        .find(|l| l.kind == StackKind::Copper)
        .map_or(0.035, |l| l.thickness.mm())
}

/// What one net's copper does with its current.
#[derive(Debug, Clone, PartialEq)]
pub struct NetPower {
    pub net: String,
    /// Each track's current (A), by its index on the board.
    pub tracks: Vec<(usize, f64)>,
    /// Each pad's voltage relative to where the net is fed (V), by its index in the
    /// layout.
    pub pads: Vec<(usize, f64)>,
    /// The most voltage lost between the feed and any pad (V), and where.
    pub worst_drop: f64,
    pub worst_pad: Option<usize>,
    /// The most current in any track (A), and the tracks carrying more than IPC-2221
    /// allows for a 10 °C rise.
    pub peak: f64,
    pub overloaded: Vec<usize>,
}

/// Solves `net` with these pad currents (A, into the copper from the part: a supply's
/// output is positive, a load's input negative). The pad `feed` (or, if `None`, the
/// one [`feed`] picks) is where the net is held; it takes up any imbalance, so a
/// supply off the board needn't be listed.
pub fn ir_drop(
    layout: &Layout,
    fills: &[ZoneFill],
    net: &str,
    currents: &[(usize, f64)],
    feed: Option<usize>,
) -> Result<NetPower, String> {
    let board = &layout.board;
    let t = copper_thickness(layout);
    let outer = [Layer::FCu, Layer::BCu];
    // Nodes: points of copper joined into one, with the layers they're on.
    let mut nodes: Vec<P> = Vec::new();
    let mut on: Vec<Vec<Layer>> = Vec::new();
    let node_at = |q: P, layers: &[Layer], nodes: &mut Vec<P>, on: &mut Vec<Vec<Layer>>| -> usize {
        // Only copper on the same layers is one node: a track ending on a pad or a via
        // reaches it through the joins below.
        if let Some(k) = (0..nodes.len()).find(|&k| {
            (nodes[k][0] - q[0]).abs() < 1e-3
                && (nodes[k][1] - q[1]).abs() < 1e-3
                && on[k] == layers
        }) {
            return k;
        }
        nodes.push(q);
        on.push(layers.to_vec());
        nodes.len() - 1
    };
    let pads: Vec<usize> =
        (0..layout.pads.len()).filter(|&k| layout.pads[k].net.as_deref() == Some(net)).collect();
    if pads.is_empty() {
        return Err(format!("{net} has no pads"));
    }
    let pad_node: Vec<(usize, usize)> = pads
        .iter()
        .map(|&k| (k, node_at(layout.pads[k].center, &layout.pads[k].layers, &mut nodes, &mut on)))
        .collect();
    let mut edges: Vec<(usize, usize, f64, Option<usize>)> = Vec::new();
    let tracks: Vec<usize> =
        (0..board.tracks.len()).filter(|&k| board.tracks[k].net == net).collect();
    let layers = board.copper();
    let via_nodes: Vec<usize> = board
        .vias
        .iter()
        .filter(|v| v.net == net)
        .map(|v| node_at(p(v.at), &layers, &mut nodes, &mut on))
        .collect();
    let mut track_ends = Vec::new();
    for &k in &tracks {
        let tr = &board.tracks[k];
        let (a, b) = (
            node_at(p(tr.a), &[tr.layer], &mut nodes, &mut on),
            node_at(p(tr.b), &[tr.layer], &mut nodes, &mut on),
        );
        let len = (p(tr.b)[0] - p(tr.a)[0]).hypot(p(tr.b)[1] - p(tr.a)[1]);
        let r = RHO * len.max(1e-6) / (tr.width.mm() * t);
        edges.push((a, b, r, Some(k)));
        track_ends.push((k, a, p(tr.a)));
        track_ends.push((k, b, p(tr.b)));
    }
    // Track ends on pads and vias, and on other tracks' sides.
    let tiny = 1e-6;
    for &(k, n, q) in &track_ends {
        let tr = &board.tracks[k];
        for &(pk, pn) in &pad_node {
            let pad = &layout.pads[pk];
            if pn != n
                && pad.layers.contains(&tr.layer)
                && geometry::overlaps(
                    &vec![vec![geometry::pad(pad, 0.0)]],
                    &vec![vec![geometry::circle(q, 0.005)]],
                )
            {
                edges.push((n, pn, tiny, None));
            }
        }
        for (v, &vn) in board.vias.iter().filter(|v| v.net == net).zip(&via_nodes) {
            // A track meeting a via goes through about half its barrel to whatever
            // meets it on another layer.
            if (p(v.at)[0] - q[0]).hypot(p(v.at)[1] - q[1]) <= v.diameter.mm() / 2.0 {
                edges.push((n, vn, barrel(v.drill.mm(), board.thickness.mm()) / 2.0, None));
            }
        }
        for &j in &tracks {
            let o = &board.tracks[j];
            if j == k || o.layer != tr.layer {
                continue;
            }
            let (oa, ob) = (p(o.a), p(o.b));
            if geometry::segment_distance(oa, ob, q, q) <= o.width.mm() / 2.0 {
                // A tee: joined to the nearer end through the copper between.
                let (near, d) =
                    if (oa[0] - q[0]).hypot(oa[1] - q[1]) < (ob[0] - q[0]).hypot(ob[1] - q[1]) {
                        (oa, (oa[0] - q[0]).hypot(oa[1] - q[1]))
                    } else {
                        (ob, (ob[0] - q[0]).hypot(ob[1] - q[1]))
                    };
                let m = node_at(near, &[tr.layer], &mut nodes, &mut on);
                if m != n {
                    edges.push((n, m, RHO * d.max(1e-6) / (o.width.mm() * t), None));
                }
            }
        }
    }
    // Pours: whatever of the net touches a piece is joined through it, at about a
    // square's sheet resistance.
    let sheet = RHO / t;
    for f in fills.iter().filter(|f| f.net == net) {
        for piece in &f.shapes {
            let shape = vec![piece.clone()];
            let centre = nodes.len();
            nodes.push(
                geometry::bounds(&shape)
                    .map_or([0.0, 0.0], |(lo, hi)| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]),
            );
            on.push(vec![f.layer]);
            for n in 0..centre {
                // Pads are reached by the pour's spokes, not at their middles.
                let probe = match pad_node.iter().find(|(_, pn)| *pn == n) {
                    Some(&(pk, _)) => geometry::pad(&layout.pads[pk], 0.01),
                    None => geometry::circle(nodes[n], 0.05),
                };
                if on[n].contains(&f.layer) && geometry::overlaps(&shape, &vec![vec![probe]]) {
                    edges.push((n, centre, sheet, None));
                }
            }
        }
    }
    // The feed: given, or the likeliest.
    let feed = feed.or_else(|| self::feed(layout, net, currents)).unwrap_or(pads[0]);
    let feed_node = pad_node
        .iter()
        .find(|(k, _)| *k == feed)
        .map(|(_, n)| *n)
        .ok_or("the feed pad isn't on the net")?;
    // Nodal analysis: G·V = I, the feed held at 0.
    let n = nodes.len();
    let mut g = vec![vec![0.0; n]; n];
    let mut inj = vec![0.0; n];
    for &(a, b, r, _) in &edges {
        if a == b {
            continue;
        }
        let c = 1.0 / r;
        g[a][a] += c;
        g[b][b] += c;
        g[a][b] -= c;
        g[b][a] -= c;
    }
    for &(pad, amps) in currents {
        if let Some((_, nn)) = pad_node.iter().find(|(k, _)| *k == pad) {
            inj[*nn] += amps;
        }
    }
    // Copper joined to nothing still needs a voltage: a whisper of leakage.
    for (k, row) in g.iter_mut().enumerate() {
        row[k] += 1e-9;
    }
    for row in g.iter_mut() {
        row[feed_node] = 0.0;
    }
    g[feed_node] = vec![0.0; n];
    g[feed_node][feed_node] = 1.0;
    inj[feed_node] = 0.0;
    let v = solve(g, inj).ok_or("the net's copper can't be solved")?;
    let mut out = NetPower {
        net: net.to_owned(),
        tracks: Vec::new(),
        pads: Vec::new(),
        worst_drop: 0.0,
        worst_pad: None,
        peak: 0.0,
        overloaded: Vec::new(),
    };
    for &(a, b, r, track) in &edges {
        if let Some(k) = track {
            let amps = (v[a] - v[b]).abs() / r;
            out.tracks.push((k, amps));
            out.peak = out.peak.max(amps);
            let tr = &board.tracks[k];
            if amps > current_for_width(tr.width.mm(), t, 10.0, outer.contains(&tr.layer)) {
                out.overloaded.push(k);
            }
        }
    }
    for &(k, nn) in &pad_node {
        let drop = v[nn].abs();
        out.pads.push((k, v[nn]));
        if drop > out.worst_drop {
            out.worst_drop = drop;
            out.worst_pad = Some(k);
        }
    }
    Ok(out)
}

/// The current each pad puts into the board's copper (A), from what `element` says the
/// part at an instance path draws through each port (its operating point: current into
/// the part, as a simulator reports it). A port on several pads shares its current
/// evenly between them.
pub fn pad_currents(
    layout: &Layout,
    element: impl Fn(&str) -> Option<Vec<(String, f64)>>,
) -> Vec<(usize, f64)> {
    let mut out = Vec::new();
    for (k, part) in layout.parts.iter().enumerate() {
        if part.place.is_none() || part.dnp() {
            continue;
        }
        let Some(ports) = element(&part.path) else { continue };
        for (port, into) in ports {
            let Some(numbers) = part.footprint.pins.get(&port) else { continue };
            let numbers: Vec<&str> = numbers.split(',').map(str::trim).collect();
            let pads: Vec<usize> = (0..layout.pads.len())
                .filter(|&i| {
                    layout.pads[i].part == k && numbers.contains(&layout.pads[i].number.as_str())
                })
                .collect();
            for &i in &pads {
                out.push((i, -into / pads.len() as f64));
            }
        }
    }
    out
}

/// Where a net is fed from: the pad putting the most current in, or, when every part
/// feeding it is off the board (a bench supply), a connector's pad, else its first.
pub fn feed(layout: &Layout, net: &str, currents: &[(usize, f64)]) -> Option<usize> {
    let on: Vec<usize> =
        (0..layout.pads.len()).filter(|&k| layout.pads[k].net.as_deref() == Some(net)).collect();
    let most = currents
        .iter()
        .filter(|(k, i)| on.contains(k) && *i > 1e-12)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|c| c.0);
    most.or_else(|| {
        on.iter().copied().find(|&k| {
            let r = &layout.parts[layout.pads[k].part].reference;
            r.starts_with('J') || r.starts_with('P') || r.starts_with("CN") || r.starts_with("BT")
        })
    })
    .or_else(|| on.first().copied())
}

/// Every net carrying current, worst drop first.
pub fn analyse(layout: &Layout, fills: &[ZoneFill], currents: &[(usize, f64)]) -> Vec<NetPower> {
    let mut nets: Vec<&String> = layout
        .nets
        .iter()
        .filter(|n| {
            currents.iter().any(|(k, i)| i.abs() > 1e-9 && layout.pads[*k].net.as_ref() == Some(*n))
        })
        .collect();
    nets.dedup();
    let mut out: Vec<NetPower> = nets
        .into_iter()
        .filter_map(|n| {
            let mine: Vec<(usize, f64)> = currents
                .iter()
                .copied()
                .filter(|(k, _)| layout.pads[*k].net.as_ref() == Some(n))
                .collect();
            ir_drop(layout, fills, n, &mine, feed(layout, n, &mine)).ok()
        })
        .collect();
    out.sort_by(|a, b| b.worst_drop.total_cmp(&a.worst_drop));
    out
}

/// A via barrel's resistance through a board `length` mm thick.
fn barrel(drill: f64, length: f64) -> f64 {
    let r = drill / 2.0;
    RHO * length / (std::f64::consts::PI * ((r + PLATING).powi(2) - r * r))
}

/// Gaussian elimination with partial pivoting.
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..n {
            let f = a[row][col] / a[col][col];
            if f == 0.0 {
                continue;
            }
            let pivot = a[col].clone();
            for (x, y) in a[row][col..].iter_mut().zip(&pivot[col..]) {
                *x -= f * y;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_2221_widths() {
        // About 1 A in a 0.3 mm (12 mil), 1 oz outer track for a 10 °C rise.
        let i = current_for_width(0.3, 0.035, 10.0, true);
        assert!((0.8..1.3).contains(&i), "{i}");
        let w = width_for_current(i, 0.035, 10.0, true);
        assert!((w - 0.3).abs() < 1e-9, "{w}");
        // Inner layers carry less.
        assert!(current_for_width(0.3, 0.035, 10.0, false) < i);
        // 3 A wants something over a millimetre.
        assert!(width_for_current(3.0, 0.035, 10.0, true) > 1.0);
    }
}
