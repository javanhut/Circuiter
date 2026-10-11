//! Filling copper pours: each zone's outline, within the board less its edge
//! clearance, minus everything of other nets grown by the clearance; pads of its own
//! net joined by thermal spokes, solidly, or not at all; slivers thinner than the
//! minimum width removed, and islands that join nothing of the net dropped.

use crate::geometry::{self, Shapes};
use crate::layout::{Layout, PadAt, p};
use circuiter_model::board::{Layer, PadConnection};

/// Extra room pours keep beyond the clearance, so that curves drawn as chords never
/// come closer than the rule.
const MARGIN: f64 = 0.01;

/// A zone's computed copper.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneFill {
    pub zone: usize,
    pub layer: Layer,
    pub net: String,
    pub shapes: Shapes,
}

/// The board's outline as a shape.
pub fn board_shape(layout: &Layout) -> Shapes {
    let outline: Vec<_> = layout.board.outline.iter().map(|q| p(*q)).collect();
    if outline.len() < 3 {
        return Vec::new();
    }
    vec![vec![geometry::ccw(outline)]]
}

/// Four spokes from a pad's centre, along its axes, `width` wide, reaching `reach`
/// beyond its edge.
fn spokes(pad: &PadAt, width: f64, reach: f64) -> Shapes {
    let (s, c) = pad.rotation.to_radians().sin_cos();
    let mut out = Vec::new();
    for (dx, dy, half) in [
        (1.0, 0.0, pad.size.0 / 2.0),
        (-1.0, 0.0, pad.size.0 / 2.0),
        (0.0, 1.0, pad.size.1 / 2.0),
        (0.0, -1.0, pad.size.1 / 2.0),
    ] {
        let len = half + reach;
        // The spoke's direction on the board (the pad turned counter-clockwise on screen).
        let (ux, uy) = (dx * c + dy * s, -dx * s + dy * c);
        let end = [pad.center[0] + ux * len, pad.center[1] + uy * len];
        out.push(vec![geometry::capsule(pad.center, end, width)]);
    }
    geometry::merge(out)
}

/// Fills every zone, highest priority first.
pub fn fill(layout: &Layout) -> Vec<ZoneFill> {
    let board = &layout.board;
    let rules = &board.rules;
    let inside = geometry::offset(&board_shape(layout), -rules.edge_clearance.mm() - MARGIN);
    let mut order: Vec<usize> = (0..board.zones.len()).collect();
    order.sort_by_key(|&k| std::cmp::Reverse(board.zones[k].priority));
    let mut done: Vec<ZoneFill> = Vec::new();
    for k in order {
        let zone = &board.zones[k];
        let outline: Vec<_> = zone.outline.iter().map(|q| p(*q)).collect();
        if outline.len() < 3 {
            continue;
        }
        let mut area = geometry::intersection(&vec![vec![geometry::ccw(outline)]], &inside);
        // Keepouts that bar pours.
        for k in
            board.keepouts.iter().filter(|k| k.pours && k.on(zone.layer) && k.outline.len() >= 3)
        {
            let shape = vec![vec![geometry::ccw(k.outline.iter().map(|q| p(*q)).collect())]];
            area = geometry::difference(&area, &geometry::offset(&shape, rules.clearance.mm()));
        }
        let own = zone.clearance.map_or(0.0, |c| c.mm());
        // The clearance to keep from an item of net `net`.
        let gap = |net: Option<&str>| {
            own.max(net.map_or(rules.clearance.mm(), |n| rules.clearance_for(n).mm()))
                .max(rules.clearance_for(&zone.net).mm())
                + MARGIN
        };
        let mut keep_out: Shapes = Vec::new();
        let mut thermal: Vec<&PadAt> = Vec::new();
        for pad in &layout.pads {
            let on_layer = pad.layers.contains(&zone.layer);
            let same = pad.net.as_deref() == Some(zone.net.as_str());
            if same && on_layer {
                match zone.pads {
                    PadConnection::Solid => {}
                    PadConnection::Thermal => {
                        keep_out.push(vec![geometry::pad(pad, zone.thermal_gap.mm())]);
                        thermal.push(pad);
                    }
                    PadConnection::None => {
                        keep_out.push(vec![geometry::pad(pad, gap(Some(&zone.net)))])
                    }
                }
            } else if on_layer {
                keep_out.push(vec![geometry::pad(pad, gap(pad.net.as_deref()))]);
            }
            // Holes keep clear on every layer.
            if let Some(d) = pad.drill().filter(|_| !same || !pad.plated()) {
                keep_out
                    .push(vec![geometry::circle(pad.center, d / 2.0 + gap(pad.net.as_deref()))]);
            }
        }
        for t in board.tracks.iter().filter(|t| t.layer == zone.layer && t.net != zone.net) {
            keep_out.push(vec![geometry::track(t, gap(Some(&t.net)))]);
        }
        for v in board.vias.iter().filter(|v| v.net != zone.net) {
            keep_out.push(vec![geometry::via(v, gap(Some(&v.net)))]);
        }
        for t in crate::teardrops::shapes(layout)
            .iter()
            .filter(|t| t.layer == zone.layer && t.net != zone.net)
        {
            keep_out.push(vec![
                geometry::offset(&vec![vec![t.outline.clone()]], gap(Some(&t.net)))[0][0].clone(),
            ]);
        }
        for f in done.iter().filter(|f| f.layer == zone.layer && f.net != zone.net) {
            keep_out.extend(geometry::offset(&f.shapes, gap(Some(&f.net))));
        }
        let keep_out = geometry::merge(keep_out);
        let mut copper = geometry::difference(&area, &keep_out);
        if !thermal.is_empty() {
            // Spokes bridge the relief gap, but never come near anything else.
            let reach = zone.thermal_gap.mm() + zone.spoke_width.mm();
            let mut all: Shapes = Vec::new();
            for pad in &thermal {
                all.extend(spokes(pad, zone.spoke_width.mm(), reach));
            }
            let mut others = keep_out.clone();
            // The spokes' own pads aren't obstacles for them.
            let relief: Shapes =
                thermal.iter().map(|pad| vec![geometry::pad(pad, zone.thermal_gap.mm())]).collect();
            others = geometry::difference(&others, &relief);
            let bridges = geometry::difference(
                &geometry::intersection(&geometry::merge(all), &area),
                &others,
            );
            copper = geometry::union(&copper, &bridges);
        }
        // Too thin to make: an opening by half the minimum width.
        let half = zone.min_width.mm() / 2.0;
        copper = geometry::offset(&geometry::offset(&copper, -half), half);
        // Keep only islands that join copper of the net.
        let net_copper: Shapes = layout
            .pads
            .iter()
            .filter(|pad| {
                pad.net.as_deref() == Some(zone.net.as_str()) && pad.layers.contains(&zone.layer)
            })
            .map(|pad| vec![geometry::pad(pad, 0.0)])
            .chain(
                board
                    .tracks
                    .iter()
                    .filter(|t| t.layer == zone.layer && t.net == zone.net)
                    .map(|t| vec![geometry::track(t, 0.0)]),
            )
            .chain(
                board
                    .vias
                    .iter()
                    .filter(|v| v.net == zone.net)
                    .map(|v| vec![geometry::via(v, 0.0)]),
            )
            .collect();
        let net_copper = geometry::offset(&net_copper, 1e-3);
        copper.retain(|piece| geometry::overlaps(&vec![piece.clone()], &net_copper));
        done.push(ZoneFill { zone: k, layer: zone.layer, net: zone.net.clone(), shapes: copper });
    }
    done.sort_by_key(|f| f.zone);
    done
}
