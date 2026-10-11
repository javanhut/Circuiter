//! Each net at a glance, as Altium's PCB panel shows them: its class, how many pads it
//! joins, how long its routing is (tracks, plus the board's thickness for each via),
//! how many connections are still to route, and whether its length is within its
//! class's limits.

use crate::drc::Checked;
use crate::layout::{Layout, p};
use circuiter_model::board::Board;
use std::cmp::Ordering;

/// One net.
#[derive(Debug, Clone, PartialEq)]
pub struct NetInfo {
    pub name: String,
    /// Its net class, if it's in one.
    pub class: Option<String>,
    pub pads: usize,
    /// Routed length, mm.
    pub routed: f64,
    pub vias: usize,
    /// Connections still to route.
    pub unrouted: usize,
    /// Against its class's length limits: `Less` too short, `Greater` too long.
    pub length: Option<Ordering>,
}

/// The routed length of `net`: its tracks, and the board's thickness for each via.
pub fn routed_length(board: &Board, net: &str) -> f64 {
    let tracks: f64 = board
        .tracks
        .iter()
        .filter(|t| t.net == net)
        .map(|t| (p(t.b)[0] - p(t.a)[0]).hypot(p(t.b)[1] - p(t.a)[1]))
        .sum();
    let vias = board.vias.iter().filter(|v| v.net == net).count() as f64;
    tracks + vias * board.thickness.mm()
}

/// How `length` stands against `net`'s class limits.
pub fn against_limits(board: &Board, net: &str, length: f64) -> Option<Ordering> {
    let class = board.rules.class(net)?;
    if class.min_length.is_some_and(|m| length < m.mm()) {
        Some(Ordering::Less)
    } else if class.max_length.is_some_and(|m| length > m.mm()) {
        Some(Ordering::Greater)
    } else {
        None
    }
}

/// Every net on the board.
pub fn nets(layout: &Layout, checked: &Checked) -> Vec<NetInfo> {
    let board = &layout.board;
    layout
        .nets
        .iter()
        .map(|name| {
            let routed = routed_length(board, name);
            let unrouted = checked.connectivity.ratsnest.iter().filter(|(n, ..)| n == name).count();
            NetInfo {
                name: name.clone(),
                class: board.rules.class(name).map(|c| c.name.clone()),
                pads: layout
                    .pads
                    .iter()
                    .filter(|pad| pad.net.as_deref() == Some(name.as_str()))
                    .count(),
                routed,
                vias: board.vias.iter().filter(|v| v.net == *name).count(),
                unrouted,
                length: if unrouted == 0 { against_limits(board, name, routed) } else { None },
            }
        })
        .collect()
}
