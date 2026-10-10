//! A KiCad netlist (`.net`, the "E" s-expression format Eeschema exports), so a design
//! drawn here can be laid out in KiCad's Pcbnew: every part with a footprint, under
//! KiCad's footprint name where one has the same pads, with its fields, and every net
//! with the pads on it and what kind of pin each is.

use crate::footprints;
use crate::layout::value_of;
use circuiter_model::cell::PortKind;
use circuiter_model::{CellId, Design};
use circuiter_netlist::Netlist;
use std::collections::BTreeMap;
use std::fmt::Write as _;

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A stable timestamp for a part, made from its path (KiCad links parts by these).
fn tstamp(path: &str) -> String {
    // FNV-1a, twice with different seeds, for 128 bits.
    let hash = |seed: u64| {
        path.bytes().fold(seed, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
    };
    let (a, b) = (hash(0xcbf2_9ce4_8422_2325), hash(0x8422_2325_cbf2_9ce4));
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xffff,
        b >> 48,
        b & 0xffff_ffff_ffff
    )
}

fn pin_type(kind: PortKind) -> &'static str {
    match kind {
        PortKind::Input => "input",
        PortKind::Output => "output",
        PortKind::Bidirectional => "bidirectional",
        PortKind::Passive => "passive",
        PortKind::PowerIn => "power_in",
        PortKind::PowerOut => "power_out",
    }
}

/// The netlist of the design under `top`, from project `source`.
pub fn netlist(design: &Design, top: &CellId, source: &str) -> Result<String, String> {
    let n = Netlist::build_physical(design, top)?;
    let mut out = String::new();
    out.push_str("(export (version \"E\")\n");
    let _ = writeln!(
        out,
        "  (design (source {}) (tool {}))",
        q(source),
        q(concat!("Circuiter ", env!("CARGO_PKG_VERSION")))
    );
    out.push_str("  (components");
    // Each pad's (reference, number, kind), by net.
    let mut nodes: BTreeMap<usize, Vec<(String, String, PortKind)>> = BTreeMap::new();
    let mut parts: Vec<_> = n.physical.iter().collect();
    parts.sort_by(|a, b| crate::fab::natural(&a.path, &b.path));
    for e in parts {
        let Some(def) = design.cell(&e.cell) else { continue };
        let Some(view) = &def.footprint else { continue };
        let param = |name: &str| {
            e.params.get(name).cloned().or_else(|| def.cell.param(name)?.default.clone())
        };
        let ports: Vec<String> = def.cell.ports.iter().map(|p| p.name.clone()).collect();
        let fp = footprints::resolve(view, &ports, &param)
            .map_err(|err| format!("{}: {err}", e.path))?;
        let value = value_of(def, &e.params);
        let _ = write!(
            out,
            "\n    (comp (ref {})\n      (value {})\n      (footprint {})",
            q(&e.path),
            q(&value),
            q(&fp.name)
        );
        if !e.fields.is_empty() {
            out.push_str("\n      (fields");
            for (k, v) in &e.fields {
                let _ = write!(out, " (field (name {}) {})", q(k), q(v));
            }
            out.push(')');
        }
        let _ = write!(
            out,
            "\n      (libsource (lib {}) (part {}) (description {}))\n      (sheetpath (names \"/\") (tstamps \"/\"))\n      (tstamps {}))",
            q(&e.cell.library),
            q(&e.cell.cell),
            q(&def.cell.description),
            q(&tstamp(&e.path))
        );
        for (port, net) in &e.ports {
            let kind = def.cell.port(port).map_or(PortKind::Passive, |p| p.kind);
            for pad in fp.pads_of(port) {
                nodes.entry(*net).or_default().push((e.path.clone(), pad.number.clone(), kind));
            }
        }
    }
    out.push_str(")\n  (nets");
    for (code, (net, pins)) in nodes.iter().enumerate() {
        let name = &n.nets[*net].name;
        let _ = write!(out, "\n    (net (code {}) (name {})", q(&(code + 1).to_string()), q(name));
        for (r, pin, kind) in pins {
            let _ = write!(
                out,
                "\n      (node (ref {}) (pin {}) (pintype {}))",
                q(r),
                q(pin),
                q(pin_type(*kind))
            );
        }
        out.push(')');
    }
    out.push_str("))\n");
    Ok(out)
}
