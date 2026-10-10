//! The electrical rules check, after KiCad's ERC: what each pin is (an input, an output,
//! a supply…) says what may share a net with it. Two outputs fighting, an output
//! shorted to a supply, an input nothing drives, a chip's supply pin on a rail no
//! source feeds: each is reported, as are labels that join nothing, global labels used
//! once, and no-connect flags on pins that are connected after all.
//!
//! A power flag (`std:pwr_flag`) on a net says it is fed from somewhere the checker
//! can't see (through a fuse, a switch, a connector), as KiCad's PWR_FLAG does.

use crate::{GROUND, Netlist};
use circuiter_model::cell::PortKind;
use circuiter_model::check::Severity;
use circuiter_model::connectivity::{Connectivity, Item};
use circuiter_model::geom::Point;
use circuiter_model::{CellId, Design, ViewKind};
use std::collections::BTreeMap;

/// One finding.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub severity: Severity,
    /// The sheet it's on, by instance path (`""` for the top), when it belongs to one.
    pub sheet: Option<String>,
    /// Where on that sheet.
    pub at: Option<Point>,
    pub message: String,
}

/// Checks the design under `top`.
pub fn check(design: &Design, top: &CellId) -> Vec<Violation> {
    let mut out = Vec::new();
    sheets(design, top, "", 0, &mut std::collections::BTreeSet::new(), &mut out);
    out.extend(lonely_global_labels(design, top));
    // Flattened as it will run: logic designs digitally, where the supply rails are
    // logic levels rather than nets a source must feed.
    let logic = Netlist::is_digital(design, top) || Netlist::is_mixed(design, top);
    let netlist = if Netlist::is_digital(design, top) {
        Netlist::build_digital(design, top)
    } else if logic {
        Netlist::build_mixed(design, top)
    } else {
        Netlist::build(design, top)
    };
    if let Ok(netlist) = netlist {
        // The top's own ports are fed from outside it.
        let mut outside = std::collections::BTreeSet::new();
        if let (Some(s), Some(locals)) =
            (design.cell(top).and_then(|d| d.schematic.as_ref()), netlist.sheets.get(""))
        {
            let conn = Connectivity::analyze(design, &top.library, s);
            for (k, net) in conn.nets.iter().enumerate() {
                if net.items.iter().any(|i| matches!(i, Item::Port(_))) {
                    outside.extend(locals.get(k).copied());
                }
            }
        }
        nets(design, &netlist, logic, &outside, &mut out);
    }
    out
}

/// The checks of each sheet on its own: labels, flags. A cell used many times is
/// checked once, at the first place it's used.
fn sheets(
    design: &Design,
    id: &CellId,
    path: &str,
    depth: usize,
    seen: &mut std::collections::BTreeSet<CellId>,
    out: &mut Vec<Violation>,
) {
    let Some(def) = design.cell(id) else { return };
    let Some(s) = &def.schematic else { return };
    if depth > 32 || !seen.insert(id.clone()) {
        return;
    }
    let conn = Connectivity::analyze(design, &id.library, s);
    let sheet = Some(path.to_owned());
    let mut flag = |severity, at: Option<Point>, message: String| {
        out.push(Violation { severity, sheet: sheet.clone(), at, message })
    };
    for net in &conn.nets {
        let pins = net.items.iter().filter(|i| matches!(i, Item::Pin(_))).count();
        let ports = net.items.iter().filter(|i| matches!(i, Item::Port(_))).count();
        let labels: Vec<usize> = net
            .items
            .iter()
            .filter_map(|i| if let Item::Label(k) = i { Some(*k) } else { None })
            .collect();
        // A label on a net with nothing else on this sheet joins it to nothing here; a
        // local label the only one of its name joins nothing anywhere.
        if pins == 0 && ports == 0 {
            for &k in &labels {
                let label = &s.labels[k];
                let others = s.labels.iter().filter(|l| l.net == label.net).count();
                if !label.global && others < 2 {
                    flag(
                        Severity::Warning,
                        Some(label.at),
                        format!("label `{}` is connected to nothing", label.net),
                    );
                }
            }
        }
    }
    // No-connect flags: on a pin left open, nowhere else.
    for &p in &s.no_connects {
        let on_pins: Vec<usize> = (0..conn.pins.len()).filter(|&k| conn.pins[k].at == p).collect();
        match on_pins.first() {
            None => {
                flag(Severity::Warning, Some(p), format!("no-connect flag at {p} is not on a pin"))
            }
            Some(&k) => {
                let net = conn.net_of(Item::Pin(k)).map(|n| &conn.nets[n]);
                let joined = net.is_some_and(|n| n.items.len() > 1);
                if joined {
                    let pin = &conn.pins[k];
                    flag(
                        Severity::Warning,
                        Some(p),
                        format!(
                            "pin {}.{} is flagged not to connect but is connected",
                            s.instances[pin.instance].name, pin.port
                        ),
                    );
                }
            }
        }
    }
    for inst in &s.instances {
        let Some((child_id, child)) = design.resolve(&inst.cell, &id.library) else { continue };
        let opens = child.schematic.is_some()
            && design.select_view(child, inst.view) == Some(ViewKind::Schematic);
        if opens {
            let p =
                if path.is_empty() { inst.name.clone() } else { format!("{path}.{}", inst.name) };
            sheets(design, &child_id, &p, depth + 1, seen, out);
        }
    }
}

/// Global labels, by name, over the whole design under `id`: how often each appears
/// (a sheet used twice counts twice) and where one is. Each cell is counted once and
/// remembered.
fn global_labels(
    design: &Design,
    id: &CellId,
    depth: usize,
    memo: &mut BTreeMap<CellId, BTreeMap<String, (usize, Point)>>,
) -> BTreeMap<String, (usize, Point)> {
    if let Some(m) = memo.get(id) {
        return m.clone();
    }
    let mut counts: BTreeMap<String, (usize, Point)> = BTreeMap::new();
    let Some(s) = design.cell(id).and_then(|d| d.schematic.as_ref()) else { return counts };
    if depth > 32 {
        return counts;
    }
    for l in s.labels.iter().filter(|l| l.global) {
        counts.entry(l.net.clone()).or_insert((0, l.at)).0 += 1;
    }
    for inst in &s.instances {
        if let Some((child, _)) = design.resolve(&inst.cell, &id.library) {
            for (name, (n, at)) in global_labels(design, &child, depth + 1, memo) {
                counts.entry(name).or_insert((0, at)).0 += n;
            }
        }
    }
    memo.insert(id.clone(), counts.clone());
    counts
}

/// The checks of the flattened circuit: what drives each net.
fn nets(
    design: &Design,
    netlist: &Netlist,
    logic: bool,
    outside: &std::collections::BTreeSet<usize>,
    out: &mut Vec<Violation>,
) {
    let cells: BTreeMap<&str, &CellId> = netlist
        .elements
        .iter()
        .map(|e| (e.path.as_str(), &e.cell))
        .chain(netlist.digital.iter().map(|e| (e.path.as_str(), &e.cell)))
        .collect();
    // A port's kind, and whether its part is a power flag.
    let kind_of = |path: &str, port: &str| -> Option<(PortKind, bool)> {
        let cell = cells.get(path)?;
        let def = design.cell(cell)?;
        // A digital model may name the cell's ports its own way.
        let renamed = def.digital.as_ref().and_then(|d| {
            d.ports
                .iter()
                .find(|(_, theirs)| theirs.as_str() == port)
                .map(|(ours, _)| ours.as_str())
        });
        let p = def.cell.port(port).or_else(|| def.cell.port(renamed?))?;
        Some((p.kind, def.cell.power_flag))
    };
    let mut flag = |severity, message: String| {
        out.push(Violation { severity, sheet: None, at: None, message })
    };
    for (index, net) in netlist.nets.iter().enumerate() {
        let mut by: BTreeMap<u8, Vec<String>> = BTreeMap::new();
        let mut flagged = outside.contains(&index);
        for (path, port) in &net.pins {
            let Some((kind, power_flag)) = kind_of(path, port) else { continue };
            flagged |= power_flag;
            // The logic simulator's constant rails (`#VDD`) are supplies.
            let rank = match kind {
                _ if path.starts_with('#') => 1,
                PortKind::Output => 0,
                PortKind::PowerOut => 1,
                PortKind::Bidirectional => 2,
                PortKind::Passive => 3,
                PortKind::Input => 4,
                PortKind::PowerIn => 5,
            };
            if !power_flag {
                by.entry(rank).or_default().push(format!("{path}.{port}"));
            }
        }
        let list = |v: &Vec<String>| {
            let mut s: Vec<&str> = v.iter().take(4).map(String::as_str).collect();
            if v.len() > 4 {
                s.push("…");
            }
            s.join(", ")
        };
        let get = |r: u8| by.get(&r).cloned().unwrap_or_default();
        let (outputs, supplies, bidir, passive, inputs, power_in) =
            (get(0), get(1), get(2), get(3), get(4), get(5));
        if outputs.len() > 1 {
            flag(
                Severity::Error,
                format!(
                    "outputs {} drive the same net {} against each other",
                    list(&outputs),
                    net.name
                ),
            );
        }
        if !outputs.is_empty() && !supplies.is_empty() {
            flag(
                Severity::Error,
                format!(
                    "output {} is shorted to supply {} on {}",
                    list(&outputs),
                    list(&supplies),
                    net.name
                ),
            );
        }
        let driven = !outputs.is_empty()
            || !supplies.is_empty()
            || !bidir.is_empty()
            || !passive.is_empty()
            || flagged;
        if !inputs.is_empty() && !driven {
            flag(
                Severity::Error,
                format!("input {} is driven by nothing (net {})", list(&inputs), net.name),
            );
        }
        let ground = net.name == GROUND || net.node == "0";
        // In logic, VDD is the level 1 whatever feeds it.
        let rail = logic && net.name == "VDD";
        if !power_in.is_empty() && supplies.is_empty() && !flagged && !ground && !rail {
            flag(
                Severity::Error,
                format!(
                    "power input {} on {} has no supply: connect a source or a regulator to it, or a power flag if it is fed through a switch, fuse or connector",
                    list(&power_in),
                    net.name
                ),
            );
        }
    }
}

/// Global labels used only once: they join nothing.
pub fn lonely_global_labels(design: &Design, top: &CellId) -> Vec<Violation> {
    global_labels(design, top, 0, &mut BTreeMap::new())
        .into_iter()
        .filter(|(_, (n, _))| *n == 1)
        .map(|(name, (_, at))| Violation {
            severity: Severity::Warning,
            sheet: None,
            at: Some(at),
            message: format!("global label `{name}` appears only once in the design"),
        })
        .collect()
}
