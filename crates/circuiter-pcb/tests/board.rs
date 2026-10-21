//! Boards built from standard parts: resolved against their schematics, poured,
//! checked and routed.

use circuiter_model::board::{Board, Layer, Placed, Track, Via, Zone};
use circuiter_model::cell::{Cell, CellDef};
use circuiter_model::geom::{Length, Point};
use circuiter_model::{CellId, Design, Library};
use circuiter_pcb::drc::{self, Rule};
use circuiter_pcb::layout::Layout;

#[path = "../../circuiter-stdlib/examples/common/sheet.rs"]
mod sheet;
use sheet::Sheet;

const LIB: &str = "test";

fn pt(x: f64, y: f64) -> Point {
    Point::new(Length::from_mm(x), Length::from_mm(y))
}

/// A divider: J1 brings in `vin` and ground, R1 and R2 halve it onto `mid`, a source
/// (off the board) drives it in simulation.
fn design() -> (Design, CellId) {
    let mut design = Design::new();
    design.add_library(circuiter_stdlib::library()).unwrap();
    design.add_library(Library::new(LIB)).unwrap();
    let mut sh = Sheet::new(&design, LIB);
    let v = sh.part("std:vdc", "V1", 0, 0, &[("V", "5")]);
    let j = sh.part("std:header_2", "J1", 0, 16, &[]);
    let r1 = sh.part("std:resistor", "R1", 20, 0, &[("R", "10k")]);
    let r2 = sh.part("std:resistor", "R2", 20, 16, &[("R", "10k")]);
    sh.nets(v, &[("p", "vin"), ("n", "GND")]);
    sh.nets(j, &[("p1", "vin"), ("p2", "GND")]);
    sh.nets(r1, &[("a", "vin"), ("b", "mid")]);
    sh.nets(r2, &[("a", "mid"), ("b", "GND")]);
    let def = CellDef {
        name: "divider".into(),
        cell: Cell::default(),
        schematic: Some(sh.s),
        ..CellDef::default()
    };
    design.library_mut(LIB).unwrap().insert(def);
    (design, CellId::new(LIB, "divider"))
}

fn place(board: &mut Board, path: &str, x: f64, y: f64, rotation: f64) {
    board.footprints.push(Placed {
        path: path.into(),
        at: pt(x, y),
        rotation,
        back: false,
        locked: false,
    });
}

/// R1 and R2 side by side, J1 to their left.
fn board() -> Board {
    let mut b = Board::new(40.0, 30.0);
    place(&mut b, "R1", 22.0, 10.0, 0.0);
    place(&mut b, "R2", 22.0, 20.0, 0.0);
    place(&mut b, "J1", 6.0, 15.0, 90.0);
    b
}

fn rules(checked: &drc::Checked) -> Vec<Rule> {
    checked.violations.iter().map(|v| v.rule).collect()
}

fn count(checked: &drc::Checked, rule: Rule) -> usize {
    checked.violations.iter().filter(|v| v.rule == rule).count()
}

fn pad<'a>(layout: &'a Layout, part: &str, number: &str) -> &'a circuiter_pcb::layout::PadAt {
    let k = layout.part(part).unwrap();
    layout.pads_of(k).find(|p| p.number == number).unwrap()
}

fn track(layout: &Layout, net: &str, a: [f64; 2], b: [f64; 2], layer: Layer) -> Track {
    let _ = layout;
    Track {
        layer,
        a: pt(a[0], a[1]),
        b: pt(b[0], b[1]),
        width: Length::from_mm(0.25),
        net: net.into(),
    }
}

#[test]
fn boards_resolve_against_their_schematics() {
    let (design, top) = design();
    let layout = Layout::new(&design, &top, &board()).unwrap();
    assert!(layout.unplaced.is_empty(), "{:?}", layout.unplaced);
    assert!(layout.errors.is_empty(), "{:?}", layout.errors);
    assert_eq!(layout.off_board, vec!["V1".to_string()]);
    assert_eq!(layout.pads.len(), 6);
    let r1a = pad(&layout, "R1", "1");
    assert_eq!(r1a.net.as_deref(), Some("vin"));
    assert!((r1a.center[0] - (22.0 - 5.08)).abs() < 1e-9 && (r1a.center[1] - 10.0).abs() < 1e-9);
    assert_eq!(pad(&layout, "R2", "2").net.as_deref(), Some("GND"));
    // J1 turned to stand upright: pin 2 above pin 1.
    let (j1, j2) = (pad(&layout, "J1", "1"), pad(&layout, "J1", "2"));
    assert!(
        (j1.center[0] - j2.center[0]).abs() < 1e-9
            && (j1.center[1] - j2.center[1] - 2.54).abs() < 1e-9
    );
    assert_eq!(j1.net.as_deref(), Some("vin"));
    assert_eq!(layout.nets.len(), 3);

    let mut missing = board();
    missing.footprints.retain(|f| f.path != "R2");
    missing.footprints.push(Placed {
        path: "R9".into(),
        at: pt(1.0, 1.0),
        rotation: 0.0,
        back: false,
        locked: false,
    });
    let layout = Layout::new(&design, &top, &missing).unwrap();
    assert_eq!(layout.unplaced, vec!["R2".to_string()]);
    assert_eq!(layout.orphans, vec!["R9".to_string()]);
}

#[test]
fn unrouted_nets_show_in_the_ratsnest_until_tracks_join_them() {
    let (design, top) = design();
    let mut b = board();
    let layout = Layout::new(&design, &top, &b).unwrap();
    let checked = drc::check(&layout);
    assert_eq!(rules(&checked), vec![Rule::Unconnected; 3], "{:#?}", checked.violations);

    // mid: R1 pad 2 straight down to R2 pad 1 would cross; go round on the back.
    let (m1, m2) = (pad(&layout, "R1", "2").center, pad(&layout, "R2", "1").center);
    b.tracks.push(track(&layout, "mid", m1, [m1[0], 15.0], Layer::BCu));
    b.tracks.push(track(&layout, "mid", [m1[0], 15.0], [m2[0], 15.0], Layer::BCu));
    b.tracks.push(track(&layout, "mid", [m2[0], 15.0], m2, Layer::BCu));
    let layout = Layout::new(&design, &top, &b).unwrap();
    let checked = drc::check(&layout);
    assert_eq!(rules(&checked), vec![Rule::Unconnected; 2], "{:#?}", checked.violations);

    // vin: J1 pad 1 to R1 pad 1, on the front then through a via to the back.
    let (j, r) = (pad(&layout, "J1", "1").center, pad(&layout, "R1", "1").center);
    // Round J1's pin 2, which stands above pin 1.
    b.tracks.push(track(&layout, "vin", j, [3.0, j[1]], Layer::FCu));
    b.tracks.push(track(&layout, "vin", [3.0, j[1]], [3.0, r[1]], Layer::FCu));
    b.tracks.push(track(&layout, "vin", [3.0, r[1]], [10.0, r[1]], Layer::FCu));
    b.vias.push(Via {
        at: pt(10.0, r[1]),
        diameter: Length::from_mm(0.6),
        drill: Length::from_mm(0.3),
        net: "vin".into(),
    });
    b.tracks.push(track(&layout, "vin", [10.0, r[1]], r, Layer::BCu));
    let layout = Layout::new(&design, &top, &b).unwrap();
    let checked = drc::check(&layout);
    assert_eq!(rules(&checked), vec![Rule::Unconnected], "{:#?}", checked.violations);
    assert!(checked.violations[0].message.starts_with("GND"));

    // GND: a pour over the whole front joins J1 pad 2 and R2 pad 2 through thermal spokes.
    b.zones.push(Zone::new(
        Layer::FCu,
        "GND",
        vec![pt(0.0, 0.0), pt(40.0, 0.0), pt(40.0, 30.0), pt(0.0, 30.0)],
    ));
    let layout = Layout::new(&design, &top, &b).unwrap();
    let checked = drc::check(&layout);
    assert!(checked.violations.is_empty(), "{:#?}", checked.violations);
    assert_eq!(checked.fills.len(), 1);
    let fill = &checked.fills[0];
    assert_eq!(fill.shapes.len(), 1, "one connected pour");
    // The pour is less than the board: edge clearance, other nets, reliefs.
    let area = circuiter_pcb::geometry::area(&fill.shapes);
    assert!(area > 900.0 && area < 1200.0, "{area}");
}

#[test]
fn rule_breaks_are_found() {
    let (design, top) = design();
    let mut b = board();
    let layout = Layout::new(&design, &top, &b).unwrap();
    // A vin track through R2's GND pad: a short.
    let g = pad(&layout, "R2", "2").center;
    b.tracks.push(track(&layout, "vin", [g[0], g[1] - 3.0], [g[0], g[1] + 3.0], Layer::FCu));
    // A thin track with a mid track passing too close.
    let mut thin = track(&layout, "mid", [28.0, 26.0], [34.0, 26.0], Layer::FCu);
    thin.width = Length::from_mm(0.1);
    b.tracks.push(thin);
    b.tracks.push(track(&layout, "vin", [28.0, 26.35], [34.0, 26.35], Layer::FCu));
    // A tiny via near the edge with a ring too narrow and a hole right by J1's.
    let j = pad(&layout, "J1", "1").center;
    b.vias.push(Via {
        at: pt(j[0] + 1.0, j[1]),
        diameter: Length::from_mm(0.35),
        drill: Length::from_mm(0.2),
        net: "vin".into(),
    });
    // Two parts on top of each other.
    place(&mut b, "R2", 22.0, 11.0, 0.0);
    b.footprints.remove(1);
    let layout = Layout::new(&design, &top, &b).unwrap();
    let checked = drc::check(&layout);
    for rule in [
        Rule::Short,
        Rule::Clearance,
        Rule::TrackWidth,
        Rule::Drill,
        Rule::AnnularRing,
        Rule::HoleToHole,
        Rule::CourtyardOverlap,
    ] {
        assert!(count(&checked, rule) > 0, "no {rule:?} in {:#?}", checked.violations);
    }

    // A track off the board, and one hugging the edge.
    let mut b = board();
    b.tracks.push(track(&layout, "mid", [38.0, 2.0], [45.0, 2.0], Layer::FCu));
    b.tracks.push(track(&layout, "mid", [10.0, 0.3], [20.0, 0.3], Layer::FCu));
    b.outline = vec![pt(0.0, 0.0), pt(40.0, 0.0), pt(0.0, 30.0), pt(40.0, 30.0)];
    let checked = drc::check(&Layout::new(&design, &top, &b).unwrap());
    assert!(count(&checked, Rule::Outline) == 1, "{:#?}", checked.violations);
    let mut b = board();
    b.tracks.push(track(&layout, "mid", [38.0, 2.0], [45.0, 2.0], Layer::FCu));
    b.tracks.push(track(&layout, "mid", [10.0, 0.3], [20.0, 0.3], Layer::FCu));
    let checked = drc::check(&Layout::new(&design, &top, &b).unwrap());
    assert_eq!(count(&checked, Rule::OutsideBoard), 1, "{:#?}", checked.violations);
    assert_eq!(count(&checked, Rule::EdgeClearance), 1, "{:#?}", checked.violations);
}

#[test]
fn every_standard_footprint_resolves_with_its_ports_on_pads() {
    let lib = circuiter_stdlib::library();
    let mut seen = 0;
    let mut bad = Vec::new();
    for def in lib.cells.values() {
        let Some(view) = &def.footprint else { continue };
        seen += 1;
        let ports: Vec<String> = def.cell.ports.iter().map(|p| p.name.clone()).collect();
        let param = |name: &str| def.cell.param(name)?.default.clone();
        let fp = circuiter_pcb::footprints::resolve(view, &ports, &param)
            .unwrap_or_else(|e| panic!("{}: {e}", def.name));
        let mut used = std::collections::BTreeSet::new();
        for port in &ports {
            let pads: Vec<_> = fp.pads_of(port).collect();
            if pads.is_empty() {
                bad.push(format!(
                    "{}: port {port} ({:?}) is on no pad",
                    def.name,
                    fp.pins.get(port)
                ));
            }
            for pad in pads {
                // Only where the part says so: a MOSFET's body is its source inside.
                if !used.insert(pad.number.clone()) && !view.pins.contains_key(port) {
                    bad.push(format!("{}: pad {} is on two ports", def.name, pad.number));
                }
            }
        }
        // Each choice of a part (an LED's colour…) resolves too.
        for p in &def.cell.params {
            if let circuiter_model::cell::ParamKind::Choice(options) = &p.kind {
                for option in options {
                    let param = |name: &str| {
                        if name == p.name {
                            Some(option.clone())
                        } else {
                            def.cell.param(name)?.default.clone()
                        }
                    };
                    circuiter_pcb::footprints::resolve(view, &ports, &param)
                        .unwrap_or_else(|e| panic!("{} {}={option}: {e}", def.name, p.name));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
    assert!(seen >= 60, "only {seen} parts have footprints");
}

/// The routed divider of the ratsnest test, with its ground poured.
fn routed() -> (Design, CellId, Board) {
    let (design, top) = design();
    let mut b = board();
    let layout = Layout::new(&design, &top, &b).unwrap();
    let (m1, m2) = (pad(&layout, "R1", "2").center, pad(&layout, "R2", "1").center);
    for (a, c) in [(m1, [m1[0], 15.0]), ([m1[0], 15.0], [m2[0], 15.0]), ([m2[0], 15.0], m2)] {
        b.tracks.push(track(&layout, "mid", a, c, Layer::BCu));
    }
    let (j, r) = (pad(&layout, "J1", "1").center, pad(&layout, "R1", "1").center);
    for (a, c) in [(j, [3.0, j[1]]), ([3.0, j[1]], [3.0, r[1]]), ([3.0, r[1]], [10.0, r[1]])] {
        b.tracks.push(track(&layout, "vin", a, c, Layer::FCu));
    }
    b.vias.push(Via {
        at: pt(10.0, r[1]),
        diameter: Length::from_mm(0.6),
        drill: Length::from_mm(0.3),
        net: "vin".into(),
    });
    b.tracks.push(track(&layout, "vin", [10.0, r[1]], r, Layer::BCu));
    b.zones.push(Zone::new(
        Layer::FCu,
        "GND",
        vec![pt(0.0, 0.0), pt(40.0, 0.0), pt(40.0, 30.0), pt(0.0, 30.0)],
    ));
    (design, top, b)
}

#[test]
fn fabrication_files_describe_the_board() {
    let (design, top, b) = routed();
    let layout = Layout::new(&design, &top, &b).unwrap();
    let files = circuiter_pcb::fab::plot(&layout, "divider", false).unwrap();
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    for want in [
        "divider-F_Cu.gbr",
        "divider-B_Cu.gbr",
        "divider-F_Mask.gbr",
        "divider-B_Mask.gbr",
        "divider-F_Paste.gbr",
        "divider-F_Silkscreen.gbr",
        "divider-Edge_Cuts.gbr",
        "divider-PTH.drl",
        "divider-NPTH.drl",
        "divider-pos.csv",
        "divider-bom.csv",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    let file = |n: &str| &files.iter().find(|f| f.name == format!("divider-{n}")).unwrap().contents;
    let front = file("F_Cu.gbr");
    assert!(front.starts_with("%TF.GenerationSoftware,Circuiter"));
    assert!(front.contains("%TF.FileFunction,Copper,L1,Top*%") && front.ends_with("M02*\n"));
    assert!(front.contains("%FSLAX46Y46*%") && front.contains("%MOMM*%"));
    // The pour is a region; pads are flashed with their nets attached.
    assert!(front.contains("G36*") && front.contains("%LPC*%"));
    assert!(front.contains("%TO.P,R1,1*%\n%TO.N,vin*%"));
    assert_eq!(front.matches("D03*").count(), 6 + 1, "six pads and a via");
    // R1 pad 1 at (16.92, 10) is at Y −10 in Gerber, in nanometre-ish integers.
    assert!(front.contains("X16920000Y-10000000D03*"), "{front}");
    assert!(file("F_Mask.gbr").contains("%TF.FilePolarity,Negative*%"));
    assert!(file("F_Silkscreen.gbr").contains("D01*"), "references are drawn");
    assert!(file("Edge_Cuts.gbr").contains("%TA.AperFunction,Profile*%"));
    let pth = file("PTH.drl");
    assert!(pth.contains("T1C0.300") || pth.contains("C0.300"), "{pth}");
    assert!(pth.contains("; #@! TF.FileFunction,Plated,1,2,PTH"));
    assert_eq!(pth.lines().filter(|l| l.starts_with('X')).count(), 7, "{pth}");
    assert_eq!(file("NPTH.drl").lines().filter(|l| l.starts_with('X')).count(), 0);
    let pos = file("pos.csv");
    assert!(
        pos.contains("\"R1\",\"10k\",\"Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P10.16mm_Horizontal\",22.0000,-10.0000,0.0000,top"),
        "{pos}"
    );
    let bom = file("bom.csv");
    assert!(bom.contains("1,1,\"J1\"") && bom.contains("2,2,\"R1, R2\",\"10k\""), "{bom}");

    // A board with errors isn't plotted.
    let mut broken = b.clone();
    broken.tracks.clear();
    let layout = Layout::new(&design, &top, &broken).unwrap();
    let errors = circuiter_pcb::fab::plot(&layout, "divider", false).unwrap_err();
    assert!(errors.iter().all(|e| e.rule == Rule::Unconnected), "{errors:#?}");
}

#[test]
fn boards_plot_as_svg() {
    let (design, top, b) = routed();
    let layout = Layout::new(&design, &top, &b).unwrap();
    let checked = drc::check(&layout);
    let svg = circuiter_pcb::svg::plot(&layout, &checked.fills, Some(&checked.connectivity));
    assert!(svg.starts_with("<svg") && svg.contains("#c83434") && svg.contains("#4d7fc4"));
    if let Ok(path) = std::env::var("CIRCUITER_BOARD_SVG") {
        std::fs::write(path, &svg).unwrap();
    }
}

/// Every through-hole part's leads go down through its own pads' holes: the 3D bodies
/// and the footprints agree.
#[test]
fn bodies_stand_with_their_leads_in_their_holes() {
    use circuiter_pcb::board3d::{WIRED, footprint_point};
    let lib = circuiter_stdlib::library();
    let mut bad = Vec::new();
    for def in lib.cells.values() {
        let Some(view) = &def.footprint else { continue };
        if WIRED.contains(&view.package.as_str()) {
            continue;
        }
        let Ok(Some(mesh)) = circuiter_3d::assembly::body(def, &Default::default()) else {
            continue;
        };
        let ports: Vec<String> = def.cell.ports.iter().map(|p| p.name.clone()).collect();
        let param = |name: &str| def.cell.param(name)?.default.clone();
        let fp = circuiter_pcb::footprints::resolve(view, &ports, &param).unwrap();
        // The lead ends, through the board and out below it.
        let below: Vec<[f64; 2]> = mesh
            .vertices
            .iter()
            .filter(|v| v.pos[2] < -1.0)
            .map(|v| footprint_point(v.pos))
            .collect();
        let holes: Vec<([f64; 2], f64)> = fp
            .pads
            .iter()
            .filter_map(|pad| match pad.kind {
                circuiter_model::board::PadKind::Through { drill } => {
                    Some(([pad.at.x.mm(), pad.at.y.mm()], drill.mm() / 2.0))
                }
                _ => None,
            })
            .collect();
        if holes.is_empty() {
            continue;
        }
        if below.is_empty() {
            bad.push(format!("{} ({}): no leads go into the board", def.name, view.package));
            continue;
        }
        let stray = below
            .iter()
            .find(|q| !holes.iter().any(|(c, r)| (q[0] - c[0]).hypot(q[1] - c[1]) <= r + 0.05));
        if let Some(q) = stray {
            bad.push(format!(
                "{} ({}): a lead at ({:.2}, {:.2}) misses every hole {:?}",
                def.name,
                view.package,
                q[0],
                q[1],
                holes.iter().map(|h| h.0).collect::<Vec<_>>()
            ));
        }
        for (c, r) in &holes {
            if !below.iter().any(|q| (q[0] - c[0]).hypot(q[1] - c[1]) <= r + 0.05) {
                bad.push(format!(
                    "{} ({}): nothing goes into the hole at ({:.2}, {:.2})",
                    def.name, view.package, c[0], c[1]
                ));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn boards_are_built_in_3d() {
    let (design, top, mut b) = routed();
    b.footprints.push(Placed {
        path: "X".into(),
        at: pt(1.0, 1.0),
        rotation: 0.0,
        back: false,
        locked: false,
    });
    let layout = Layout::new(&design, &top, &b).unwrap();
    let checked = drc::check(&layout);
    let built = circuiter_pcb::board3d::build(&design, &layout, &checked.fills);
    assert!(built.errors.is_empty(), "{:?}", built.errors);
    assert_eq!(built.parts.len(), 3);
    let (lo, hi) = built.board.bounds().unwrap();
    assert!((lo[0] - 0.0).abs() < 1e-3 && (hi[0] - 40.0).abs() < 1e-3);
    assert!(
        (lo[1] + 30.0).abs() < 1e-3 && (hi[1] - 0.0).abs() < 1e-3,
        "board Y runs down the sheet: {lo:?} {hi:?}"
    );
    assert!(lo[2] < -1.6 && hi[2] > 0.0 && hi[2] < 0.1);
    // R1's body sits over its footprint at (22, 10), above the board.
    let r1 = &built.parts.iter().find(|(n, _)| n == "R1").unwrap().1;
    let (lo, hi) = r1.bounds().unwrap();
    assert!(
        ((lo[0] + hi[0]) / 2.0 - 22.0).abs() < 0.5 && ((lo[1] + hi[1]) / 2.0 + 10.0).abs() < 0.5,
        "{lo:?} {hi:?}"
    );
    assert!(hi[2] > 1.0);
    // On the back it hangs under the board, mirrored.
    let mut back = b.clone();
    back.footprints.iter_mut().find(|f| f.path == "R1").unwrap().back = true;
    let layout = Layout::new(&design, &top, &back).unwrap();
    let built = circuiter_pcb::board3d::build(&design, &layout, &[]);
    let r1 = &built.parts.iter().find(|(n, _)| n == "R1").unwrap().1;
    let (lo, _) = r1.bounds().unwrap();
    assert!(lo[2] < -2.6, "{lo:?}");
}

#[test]
fn updating_from_the_schematic_places_new_parts_and_drops_old_ones() {
    let (design, top) = design();
    let mut b = board();
    b.footprints.retain(|f| f.path != "R2");
    b.footprints.push(Placed {
        path: "R9".into(),
        at: pt(30.0, 25.0),
        rotation: 0.0,
        back: false,
        locked: false,
    });
    let changes = circuiter_pcb::sync::update(&design, &top, &mut b, true).unwrap();
    assert_eq!(changes.added, vec!["R2".to_string()]);
    assert_eq!(changes.removed, vec!["R9".to_string()]);
    let layout = Layout::new(&design, &top, &b).unwrap();
    assert!(layout.unplaced.is_empty() && layout.orphans.is_empty());
    let checked = drc::check(&layout);
    let problems: Vec<_> =
        checked.violations.iter().filter(|v| v.rule != Rule::Unconnected).collect();
    assert!(problems.is_empty(), "placed clear of everything: {problems:#?}");

    // A board made for the schematic holds every part, clear of each other.
    let fresh = circuiter_pcb::sync::new_board(&design, &top).unwrap();
    let layout = Layout::new(&design, &top, &fresh).unwrap();
    assert!(layout.unplaced.is_empty());
    let checked = drc::check(&layout);
    let problems: Vec<_> =
        checked.violations.iter().filter(|v| v.rule != Rule::Unconnected).collect();
    assert!(problems.is_empty(), "{problems:#?}");
    assert_eq!(checked.connectivity.unrouted(), 3);
}

/// The analog lab's circuits all lay out on boards of their own.
#[test]
fn example_circuits_get_boards() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/analog_lab");
    let project = circuiter_model::Project::load(&dir).unwrap();
    let design = Design::for_project(&project, circuiter_stdlib::library()).unwrap();
    for cell in
        ["amplifier", "blinker", "power_supply", "light_switch", "motor_driver", "relay_driver"]
    {
        let top = CellId::new("analog_lab", cell);
        let b =
            circuiter_pcb::sync::new_board(&design, &top).unwrap_or_else(|e| panic!("{cell}: {e}"));
        let layout = Layout::new(&design, &top, &b).unwrap();
        assert!(layout.errors.is_empty(), "{cell}: {:?}", layout.errors);
        let checked = drc::check(&layout);
        let problems: Vec<_> =
            checked.violations.iter().filter(|v| v.rule != Rule::Unconnected).collect();
        assert!(problems.is_empty(), "{cell}: {problems:#?}");
        assert!(checked.connectivity.unrouted() > 0);
    }
}

/// The netlist KiCad reads: each part with KiCad's footprint, each net with its pads.
#[test]
fn kicad_netlists_list_parts_and_nets() {
    let (mut design, top) = design();
    // An MPN on R1 goes into its fields.
    let def = design.cell_mut(&top).unwrap();
    let r1 = def.schematic.as_mut().unwrap().instances.iter_mut().find(|i| i.name == "R1").unwrap();
    r1.fields.insert("MPN".into(), "MFR-25FBF52-10K".into());
    let net = circuiter_pcb::kicad::netlist(&design, &top, "divider").unwrap();
    assert!(net.starts_with("(export (version \"E\")"));
    assert!(net.contains("(comp (ref \"R1\")\n      (value \"10k\")\n      (footprint \"Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P10.16mm_Horizontal\")"), "{net}");
    assert!(net.contains("(field (name \"MPN\") \"MFR-25FBF52-10K\")"));
    assert!(
        net.contains("(footprint \"Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm_Vertical\")")
    );
    assert!(!net.contains("\"V1\""), "the source isn't on the board");
    // `mid` joins R1 pad 2 and R2 pad 1.
    let mid = net.split("(net ").find(|n| n.contains("(name \"mid\")")).expect("mid");
    assert!(
        mid.contains("(node (ref \"R1\") (pin \"2\") (pintype \"passive\"))")
            && mid.contains("(node (ref \"R2\") (pin \"1\")"),
        "{mid}"
    );
    // Balanced parentheses, outside strings.
    let mut depth = 0i32;
    let mut in_str = false;
    for c in net.chars() {
        match c {
            '"' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => depth -= 1,
            _ => {}
        }
        assert!(depth >= 0);
    }
    assert_eq!(depth, 0);
}

/// A case made for the divider: everything fits, the header at the edge gets an
/// opening, a lid set too low runs into the parts, and the case holds heat in about as
/// a small plastic box does.
#[test]
fn a_case_is_made_to_fit_the_board() {
    use circuiter_model::board::Enclosure;
    use circuiter_pcb::enclosure;
    let (design, top, mut b) = routed();
    // J1 at the left edge, so its pins face the wall.
    b.footprints.iter_mut().find(|f| f.path == "J1").unwrap().at = pt(2.0, 15.0);
    b.tracks.retain(|t| t.net != "vin");
    let layout = Layout::new(&design, &top, &b).unwrap();
    let built = circuiter_pcb::board3d::build(&design, &layout, &[]);
    let e = Enclosure::default();
    let made = enclosure::make(&layout, &built, &e).unwrap();
    let names: Vec<&str> = made.base.features.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"Opening for J1"), "{names:?}");
    assert_eq!(names.iter().filter(|n| n.starts_with("Standoff")).count(), 4, "{names:?}");
    let (base, lid) = enclosure::bodies(&made);
    assert!(base.triangles() > 100 && lid.triangles() > 50);
    let (lo, hi) = base.bounds().unwrap();
    // The board (0..40 × 0..30) with a 1 mm gap and 2 mm walls.
    assert!((lo[0] + 3.0).abs() < 0.05 && (hi[0] - 43.0).abs() < 0.05, "{lo:?} {hi:?}");
    let clashes =
        enclosure::fit(&layout, &built, &[&base, &lid], Some(made.lid_z as f32 + made.wall as f32));
    assert!(clashes.is_empty(), "{clashes:#?}");

    // A lid only 3 mm over the board: the header's pins run into it.
    let low = Enclosure { above: 3.0, ..Enclosure::default() };
    let made_low = enclosure::make(&layout, &built, &low).unwrap();
    let (base_low, lid_low) = enclosure::bodies(&made_low);
    let clashes =
        enclosure::fit(&layout, &built, &[&base_low, &lid_low], Some(made_low.lid_z as f32 + 2.0));
    assert!(clashes.iter().any(|c| c.starts_with("J1")), "{clashes:#?}");

    // About 46 × 36 × 18 mm of plastic: some tens of kelvin per watt.
    let (r, c) = enclosure::thermal(&made, &e);
    assert!(r > 20.0 && r < 150.0, "{r} K/W");
    assert!(c > 5.0 && c < 100.0, "{c} J/K");
    let vented = enclosure::thermal(&made, &Enclosure { vents: true, ..e.clone() }).0;
    assert!(vented < r);
}

/// The autorouter routes what's unrouted without breaking a rule.
#[test]
fn the_autorouter_routes_cleanly() {
    use circuiter_pcb::route::{Options, autoroute};
    let (design, top) = design();
    let mut b = board();
    let layout = Layout::new(&design, &top, &b).unwrap();
    let routed = autoroute(&layout, &Options { pitch: 0.2, ..Options::default() });
    assert!(routed.failed.is_empty(), "{:?}", routed.failed);
    assert_eq!(routed.done, 3);
    b.tracks.extend(routed.tracks.clone());
    b.vias.extend(routed.vias.clone());
    let checked = drc::check(&Layout::new(&design, &top, &b).unwrap());
    assert!(checked.violations.is_empty(), "{:#?}", checked.violations);
    // Every track is at 0°, 45° or 90° except the short joins onto pads.
    for t in &routed.tracks {
        let (dx, dy) = ((t.b.x.mm() - t.a.x.mm()).abs(), (t.b.y.mm() - t.a.y.mm()).abs());
        let square = dx < 1e-6 || dy < 1e-6 || (dx - dy).abs() < 1e-6;
        assert!(square || dx.hypot(dy) < 0.3, "{t:?}");
    }
}

/// The analog lab's circuits all route on boards of their own.
#[test]
fn example_circuits_route() {
    use circuiter_pcb::route::{Options, autoroute};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/analog_lab");
    let project = circuiter_model::Project::load(&dir).unwrap();
    let design = Design::for_project(&project, circuiter_stdlib::library()).unwrap();
    for cell in ["blinker", "amplifier", "light_switch"] {
        let top = CellId::new("analog_lab", cell);
        let mut b = circuiter_pcb::sync::new_board(&design, &top).unwrap();
        let layout = Layout::new(&design, &top, &b).unwrap();
        let routed = autoroute(&layout, &Options { pitch: 0.25, ..Options::default() });
        b.tracks.extend(routed.tracks);
        b.vias.extend(routed.vias);
        let checked = drc::check(&Layout::new(&design, &top, &b).unwrap());
        let errors: Vec<_> = checked
            .violations
            .iter()
            .filter(|v| v.severity() == drc::Severity::Error && v.rule != Rule::Unconnected)
            .collect();
        assert!(errors.is_empty(), "{cell}: {errors:#?}");
        assert_eq!(checked.connectivity.unrouted(), routed.failed.len(), "{cell}");
        assert!(routed.failed.is_empty(), "{cell}: {:?}", routed.failed);
    }
}

#[test]
#[ignore]
fn plot_routed_blinker() {
    use circuiter_pcb::route::{Options, autoroute};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/analog_lab");
    let project = circuiter_model::Project::load(&dir).unwrap();
    let design = Design::for_project(&project, circuiter_stdlib::library()).unwrap();
    let cell = std::env::var("CELL").unwrap_or("blinker".into());
    let top = CellId::new("analog_lab", &cell);
    let mut b = circuiter_pcb::sync::new_board(&design, &top).unwrap();
    let layout = Layout::new(&design, &top, &b).unwrap();
    let t = std::time::Instant::now();
    let routed = autoroute(&layout, &Options::default());
    eprintln!("{:?} done {} failed {}", t.elapsed(), routed.done, routed.failed.len());
    b.tracks.extend(routed.tracks);
    b.vias.extend(routed.vias);
    let layout = Layout::new(&design, &top, &b).unwrap();
    let checked = drc::check(&layout);
    std::fs::write(
        std::env::var("OUT").unwrap(),
        circuiter_pcb::svg::plot(&layout, &checked.fills, Some(&checked.connectivity)),
    )
    .unwrap();
}

/// Tracks stay on a part's pads when it moves or turns, and clean-up takes off
/// leftovers without unrouting anything.
#[test]
fn tracks_follow_parts_and_tidy_up() {
    use circuiter_pcb::edit;
    use circuiter_pcb::route::{Options, autoroute};
    let (design, top) = design();
    let mut b = board();
    let layout = Layout::new(&design, &top, &b).unwrap();
    let routed = autoroute(&layout, &Options { pitch: 0.2, ..Options::default() });
    b.tracks.extend(routed.tracks);
    b.vias.extend(routed.vias);
    let layout = Layout::new(&design, &top, &b).unwrap();
    let mut r2 = b.placed("R2").unwrap().clone();
    r2.at = pt(22.0, 24.0);
    r2.rotation = 180.0;
    edit::move_footprint(&mut b, &layout, "R2", r2);
    let checked = drc::check(&Layout::new(&design, &top, &b).unwrap());
    assert_eq!(checked.connectivity.unrouted(), 0, "still connected: {:#?}", checked.violations);

    // A duplicate, a stub and a track split in two are tidied.
    let layout = Layout::new(&design, &top, &b).unwrap();
    let n = b.tracks.len();
    b.tracks.push(b.tracks[0].clone());
    b.tracks.push(track(&layout, "mid", [30.0, 28.0], [35.0, 28.0], Layer::FCu));
    let removed = edit::clean_up(&mut b, &layout);
    assert!(removed >= 2, "{removed}");
    assert!(b.tracks.len() <= n);
    let checked = drc::check(&Layout::new(&design, &top, &b).unwrap());
    assert_eq!(checked.connectivity.unrouted(), 0);
}

/// A four-layer board routes on its inner layers too, and each gets a Gerber.
#[test]
fn inner_layers_route_and_plot() {
    use circuiter_pcb::route::{Options, autoroute};
    let (design, top) = design();
    let mut b = board();
    b.copper_layers = 4;
    let layout = Layout::new(&design, &top, &b).unwrap();
    let routed = autoroute(
        &layout,
        &Options { pitch: 0.2, via_cost: 2.0, against: 3.0, ..Options::default() },
    );
    assert!(routed.failed.is_empty());
    b.tracks.extend(routed.tracks.clone());
    b.vias.extend(routed.vias.clone());
    let layout = Layout::new(&design, &top, &b).unwrap();
    let checked = drc::check(&layout);
    assert!(checked.violations.is_empty(), "{:#?}", checked.violations);
    let files = circuiter_pcb::fab::outputs(&layout, &checked.fills, "four");
    for name in ["four-F_Cu.gbr", "four-In1_Cu.gbr", "four-In2_Cu.gbr", "four-B_Cu.gbr"] {
        assert!(files.iter().any(|f| f.name == name), "{name}");
    }
    let inner = files.iter().find(|f| f.name == "four-In1_Cu.gbr").unwrap();
    assert!(inner.contents.contains("%TF.FileFunction,Copper,L2,Inr*%"));
}

/// Teardrops widen the routed joints without breaking a rule; stitching vias join the
/// ground pours through the board wherever one fits.
#[test]
fn teardrops_and_stitching() {
    use circuiter_model::board::Teardrops;
    let (design, top, mut b) = routed();
    b.teardrops = Some(Teardrops::default());
    let layout = Layout::new(&design, &top, &b).unwrap();
    let drops = circuiter_pcb::teardrops::shapes(&layout);
    assert!(drops.len() >= 4, "{}", drops.len());
    let checked = drc::check(&layout);
    assert!(checked.violations.is_empty(), "{:#?}", checked.violations);
    // Ground poured on the back too, then stitched.
    b.zones.push(Zone::new(
        Layer::BCu,
        "GND",
        vec![pt(0.0, 0.0), pt(40.0, 0.0), pt(40.0, 30.0), pt(0.0, 30.0)],
    ));
    let layout = Layout::new(&design, &top, &b).unwrap();
    let fills = circuiter_pcb::zones::fill(&layout);
    let vias = circuiter_pcb::teardrops::stitch(&layout, &fills, "GND", 4.0);
    assert!(vias.len() >= 10, "{}", vias.len());
    b.vias.extend(vias);
    let checked = drc::check(&Layout::new(&design, &top, &b).unwrap());
    assert!(checked.violations.is_empty(), "{:#?}", checked.violations);
}

/// Optimising placement shortens the connections and keeps the parts apart and on the
/// board.
#[test]
fn placement_is_optimised() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/analog_lab");
    let project = circuiter_model::Project::load(&dir).unwrap();
    let design = Design::for_project(&project, circuiter_stdlib::library()).unwrap();
    let top = CellId::new("analog_lab", "blinker");
    let mut b = circuiter_pcb::sync::new_board(&design, &top).unwrap();
    let (before, after) = circuiter_pcb::sync::optimize(&design, &top, &mut b).unwrap();
    assert!(after < before * 0.95, "{before} → {after}");
    let checked = drc::check(&Layout::new(&design, &top, &b).unwrap());
    let problems: Vec<_> =
        checked.violations.iter().filter(|v| v.rule != Rule::Unconnected).collect();
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The Engineering Change Order lists what the schematic asks, and only what's chosen
/// is done.
#[test]
fn engineering_change_orders() {
    use circuiter_pcb::sync::{Change, apply, changes};
    let (design, top) = design();
    let mut b = board();
    b.footprints.retain(|f| f.path != "R2");
    b.footprints.push(Placed {
        path: "R9".into(),
        at: pt(30.0, 25.0),
        rotation: 0.0,
        back: false,
        locked: false,
    });
    let layout = Layout::new(&design, &top, &b).unwrap();
    b.tracks.push(track(&layout, "gone", [5.0, 25.0], [10.0, 25.0], Layer::FCu));
    let found = changes(&design, &top, &b).unwrap();
    assert_eq!(
        found,
        vec![Change::Add("R2".into()), Change::Remove("R9".into()), Change::Unroute("gone".into())]
    );
    apply(&design, &top, &mut b, &[Change::Add("R2".into())]).unwrap();
    assert!(b.placed("R2").is_some() && b.placed("R9").is_some() && !b.tracks.is_empty());
    apply(&design, &top, &mut b, &[Change::Remove("R9".into()), Change::Unroute("gone".into())])
        .unwrap();
    assert!(b.placed("R9").is_none() && b.tracks.is_empty());
    assert!(changes(&design, &top, &b).unwrap().is_empty());
}

/// Every surface-mount body sits on its own pads: the bottoms of its leads and
/// terminations are over copper, in every package a part offers.
#[test]
fn surface_mount_bodies_sit_on_their_pads() {
    use circuiter_pcb::board3d::footprint_point;
    let lib = circuiter_stdlib::library();
    let mut checked = 0;
    let mut bad = Vec::new();
    for def in lib.cells.values() {
        let (Some(view), Some(choice)) = (&def.footprint, def.cell.param("package")) else {
            continue;
        };
        let circuiter_model::cell::ParamKind::Choice(options) = &choice.kind else { continue };
        for package in options {
            let params =
                std::collections::BTreeMap::from([("package".to_owned(), package.clone())]);
            let ports: Vec<String> = def.cell.ports.iter().map(|p| p.name.clone()).collect();
            let param = |name: &str| {
                params.get(name).cloned().or_else(|| def.cell.param(name)?.default.clone())
            };
            let fp = circuiter_pcb::footprints::resolve(view, &ports, &param).unwrap();
            if fp.pads.iter().any(|p| !matches!(p.kind, circuiter_model::board::PadKind::Smd)) {
                continue;
            }
            checked += 1;
            let mesh = circuiter_3d::assembly::body(def, &params).unwrap().unwrap();
            let tin = circuiter_3d::color("tin");
            let feet: Vec<[f64; 2]> = mesh
                .vertices
                .iter()
                .filter(|v| v.pos[2] < 0.01 && v.color == tin)
                .map(|v| footprint_point(v.pos))
                .collect();
            if feet.is_empty() {
                bad.push(format!("{} {package}: nothing touches the board", def.name));
            }
            let on_pad = |q: [f64; 2]| {
                fp.pads.iter().any(|p| {
                    (q[0] - p.at.x.mm()).abs() <= p.size.0.mm() / 2.0 + 0.3
                        && (q[1] - p.at.y.mm()).abs() <= p.size.1.mm() / 2.0 + 0.3
                })
            };
            if let Some(q) = feet.iter().find(|q| !on_pad(**q)) {
                bad.push(format!(
                    "{} {package}: a foot at ({:.2}, {:.2}) is off its pads",
                    def.name, q[0], q[1]
                ));
            }
            for pad in &fp.pads {
                let c = [pad.at.x.mm(), pad.at.y.mm()];
                if !feet.iter().any(|q| {
                    (q[0] - c[0]).abs() <= pad.size.0.mm() / 2.0 + 0.3
                        && (q[1] - c[1]).abs() <= pad.size.1.mm() / 2.0 + 0.3
                }) {
                    bad.push(format!("{} {package}: nothing sits on pad {}", def.name, pad.number));
                }
            }
        }
    }
    assert!(checked >= 20, "{checked}");
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Keepouts: the router goes round, pours stay out, and anything inside is flagged.
#[test]
fn keepouts_are_kept() {
    use circuiter_model::board::Keepout;
    use circuiter_pcb::route::{Options, autoroute};
    let (design, top) = design();
    let mut b = board();
    // A strip across the middle between R1 and R2, from the edge to x = 30.
    let strip = Keepout::new(vec![pt(0.0, 14.0), pt(30.0, 14.0), pt(30.0, 16.0), pt(0.0, 16.0)]);
    b.keepouts.push(strip);
    b.zones.push(Zone::new(
        Layer::BCu,
        "GND",
        vec![pt(0.0, 0.0), pt(40.0, 0.0), pt(40.0, 30.0), pt(0.0, 30.0)],
    ));
    let layout = Layout::new(&design, &top, &b).unwrap();
    let routed = autoroute(&layout, &Options { pitch: 0.2, ..Options::default() });
    assert!(routed.failed.is_empty(), "{:?}", routed.failed);
    b.tracks.extend(routed.tracks);
    b.vias.extend(routed.vias);
    let layout = Layout::new(&design, &top, &b).unwrap();
    let checked = drc::check(&layout);
    assert!(checked.violations.is_empty(), "{:#?}", checked.violations);
    // The pour has a hole where the keepout is.
    let fill = checked.fills.iter().find(|f| f.layer == Layer::BCu).unwrap();
    let probe = vec![vec![circuiter_pcb::geometry::circle([15.0, 15.0], 0.3)]];
    assert!(!circuiter_pcb::geometry::overlaps(&fill.shapes, &probe));
    // A track straight through it is flagged.
    b.tracks.push(track(&layout, "mid", [10.0, 12.0], [10.0, 18.0], Layer::FCu));
    let checked = drc::check(&Layout::new(&design, &top, &b).unwrap());
    assert!(
        checked.violations.iter().any(|v| v.rule == Rule::Keepout),
        "{:#?}",
        checked.violations
    );
}

/// Parts line up and spread out as asked, their boxes' edges and centres meeting.
#[test]
fn parts_align_and_distribute() {
    use circuiter_pcb::edit::{Align, align, part_box};
    let (design, top) = design();
    let mut b = board();
    // Three parts at odd places.
    b.footprints.iter_mut().find(|f| f.path == "R2").unwrap().at = pt(25.0, 21.0);
    let paths: Vec<String> = ["R1", "R2", "J1"].map(String::from).to_vec();
    let boxes = |b: &Board| {
        let l = Layout::new(&design, &top, b).unwrap();
        paths.iter().map(|p| part_box(&l, l.part(p).unwrap()).unwrap()).collect::<Vec<_>>()
    };
    let layout = Layout::new(&design, &top, &b).unwrap();
    align(&mut b, &layout, &paths, Align::Left);
    let bx = boxes(&b);
    assert!(bx.iter().all(|x| (x.0[0] - bx[0].0[0]).abs() < 1e-6), "{bx:?}");
    let layout = Layout::new(&design, &top, &b).unwrap();
    align(&mut b, &layout, &paths, Align::SpreadY);
    let bx = boxes(&b);
    let mut mids: Vec<f64> = bx.iter().map(|x| (x.0[1] + x.1[1]) / 2.0).collect();
    mids.sort_by(f64::total_cmp);
    assert!(((mids[1] - mids[0]) - (mids[2] - mids[1])).abs() < 1e-6, "{mids:?}");
}

/// A differential pair between two connectors, found by its nets' names, routed side
/// by side at its gap, without breaking a rule and with its halves the same length.
#[test]
fn differential_pairs_route_together() {
    use circuiter_model::board::DiffPair;
    use circuiter_pcb::route::{Options, route_pair};
    let mut d = Design::new();
    d.add_library(circuiter_stdlib::library()).unwrap();
    d.add_library(Library::new(LIB)).unwrap();
    let mut sh = Sheet::new(&d, LIB);
    let j1 = sh.part("std:header_2", "J1", 0, 0, &[]);
    let j2 = sh.part("std:header_2", "J2", 20, 0, &[]);
    sh.nets(j1, &[("p1", "D_P"), ("p2", "D_N")]);
    sh.nets(j2, &[("p1", "D_P"), ("p2", "D_N")]);
    let s = sh.s;
    d.library_mut(LIB).unwrap().insert(CellDef {
        name: "link".into(),
        cell: Cell::default(),
        schematic: Some(s),
        ..CellDef::default()
    });
    let top = CellId::new(LIB, "link");
    let mut b = Board::new(40.0, 20.0);
    place(&mut b, "J1", 6.0, 10.0, 90.0);
    place(&mut b, "J2", 34.0, 12.0, 90.0);
    let layout = Layout::new(&d, &top, &b).unwrap();
    let pairs = DiffPair::from_names(&layout.nets);
    assert_eq!(pairs.len(), 1);
    assert_eq!(
        (pairs[0].positive.as_str(), pairs[0].negative.as_str(), pairs[0].name.as_str()),
        ("D_P", "D_N", "D")
    );
    let mut pair = pairs[0].clone();
    pair.gap = Length::from_mm(0.3);
    let tracks = route_pair(&layout, &pair, &Options { pitch: 0.1, ..Options::default() }).unwrap();
    b.tracks.extend(tracks);
    b.pairs.push(pair.clone());
    let layout = Layout::new(&d, &top, &b).unwrap();
    let checked = drc::check(&layout);
    assert!(checked.violations.is_empty(), "{:#?}", checked.violations);
    // Side by side: the two halves' middle runs are the gap plus a width apart.
    let longest = |net: &str| {
        b.tracks
            .iter()
            .filter(|t| t.net == net)
            .max_by(|x, y| {
                let l = |t: &Track| (t.b.x.mm() - t.a.x.mm()).hypot(t.b.y.mm() - t.a.y.mm());
                l(x).total_cmp(&l(y))
            })
            .unwrap()
            .clone()
    };
    let (tp, tn) = (longest("D_P"), longest("D_N"));
    let mid = [(tp.a.x.mm() + tp.b.x.mm()) / 2.0, (tp.a.y.mm() + tp.b.y.mm()) / 2.0];
    let apart = circuiter_pcb::geometry::segment_distance(
        [tn.a.x.mm(), tn.a.y.mm()],
        [tn.b.x.mm(), tn.b.y.mm()],
        mid,
        mid,
    );
    assert!((apart - (0.3 + pair.width.mm())).abs() < 0.02, "{apart}");
    let skew = (circuiter_pcb::nets::routed_length(&b, "D_P")
        - circuiter_pcb::nets::routed_length(&b, "D_N"))
    .abs();
    assert!(skew < 1.0, "{skew}");
}

/// The power network: the routed board's `vin` carries an amp from J1 to R1 through
/// its tracks and via, losing what their resistance says, and `GND` through its pour
/// loses next to nothing.
#[test]
fn ir_drop_and_current_capacity() {
    use circuiter_pcb::power;
    let (design, top, b) = routed();
    let layout = Layout::new(&design, &top, &b).unwrap();
    let fills = circuiter_pcb::zones::fill(&layout);
    let index = |part: &str, number: &str| {
        let k = layout.part(part).unwrap();
        (0..layout.pads.len())
            .find(|&i| layout.pads[i].part == k && layout.pads[i].number == number)
            .unwrap()
    };
    let (feed, load) = (index("J1", "1"), index("R1", "1"));
    let vin = power::ir_drop(&layout, &fills, "vin", &[(feed, 1.0), (load, -1.0)], None).unwrap();
    let length: f64 = b
        .tracks
        .iter()
        .filter(|t| t.net == "vin")
        .map(|t| (t.b.x.mm() - t.a.x.mm()).hypot(t.b.y.mm() - t.a.y.mm()))
        .sum();
    let t = power::copper_thickness(&layout);
    let tracks = 1.72e-5 * length / (0.25 * t);
    assert_eq!(vin.worst_pad, Some(load));
    assert!(
        vin.worst_drop > tracks && vin.worst_drop < tracks * 1.1,
        "{} vs {tracks}",
        vin.worst_drop
    );
    assert!(vin.tracks.iter().all(|(_, i)| (i - 1.0).abs() < 1e-3), "{:?}", vin.tracks);
    // An amp is too much for a quarter-millimetre track; a tenth isn't.
    assert_eq!(vin.overloaded.len(), 4);
    let light = power::ir_drop(&layout, &fills, "vin", &[(feed, 0.1), (load, -0.1)], None).unwrap();
    assert!(light.overloaded.is_empty());
    assert!((light.worst_drop * 10.0 - vin.worst_drop).abs() < 1e-9);
    // Ground through the pour.
    let (gf, gl) = (index("J1", "2"), index("R2", "2"));
    let gnd = power::ir_drop(&layout, &fills, "GND", &[(gf, 1.0), (gl, -1.0)], Some(gf)).unwrap();
    assert!(gnd.worst_drop < vin.worst_drop / 5.0, "{} vs {}", gnd.worst_drop, vin.worst_drop);
    assert!(gnd.worst_drop > 0.0);
}

/// Pin currents from a simulation land on the pads their ports use, and the feed is
/// found when the supply is off the board.
#[test]
fn simulated_currents_reach_the_pads() {
    use circuiter_pcb::power;
    let (design, top, b) = routed();
    let layout = Layout::new(&design, &top, &b).unwrap();
    // R1 draws 0.25 mA in at `a` and out at `b`.
    let currents = power::pad_currents(&layout, |path| {
        (path == "R1").then(|| vec![("a".to_owned(), 0.25e-3), ("b".to_owned(), -0.25e-3)])
    });
    assert_eq!(currents.len(), 2);
    let r1a = currents.iter().find(|(k, _)| layout.pads[*k].net.as_deref() == Some("vin")).unwrap();
    assert!((r1a.1 + 0.25e-3).abs() < 1e-12, "into the part is out of the copper");
    let feed = power::feed(&layout, "vin", &currents).unwrap();
    assert_eq!(layout.parts[layout.pads[feed].part].reference, "J1");
    let fills = circuiter_pcb::zones::fill(&layout);
    let all = power::analyse(&layout, &fills, &currents);
    assert_eq!(all.iter().map(|n| n.net.as_str()).collect::<Vec<_>>().len(), 2);
    let vin = all.iter().find(|n| n.net == "vin").unwrap();
    assert_eq!(vin.worst_pad, Some(r1a.0));
}

/// Parts whose bodies run into each other are found in 3D, though their footprints
/// may be told to overlap; apart, they're not.
#[test]
fn bodies_that_collide_are_found() {
    let (design, top) = design();
    let b = board();
    let layout = Layout::new(&design, &top, &b).unwrap();
    assert!(drc::bodies(&design, &layout).is_empty(), "{:?}", drc::bodies(&design, &layout));
    // R2 laid over R1.
    let mut b = board();
    b.footprints.iter_mut().find(|f| f.path == "R2").unwrap().at = pt(23.0, 10.5);
    let layout = Layout::new(&design, &top, &b).unwrap();
    let found = drc::bodies(&design, &layout);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].rule, Rule::Bodies);
    assert!(
        found[0].message.contains("R1") && found[0].message.contains("R2"),
        "{}",
        found[0].message
    );
    let at = found[0].at.unwrap();
    assert!((at[0] - 22.5).abs() < 4.0 && (at[1] - 10.25).abs() < 2.0, "{at:?}");
}

/// Panels: V-scored copies butted together between rails, and mouse-bitten copies
/// routed apart and held by tabs; every copy's copper and parts in the panel's files.
#[test]
fn boards_are_panelized() {
    use circuiter_model::board::{Panel, Separation};
    use circuiter_pcb::panel::panelize;
    let (design, top, b) = routed();
    let layout = Layout::new(&design, &top, &b).unwrap();
    let fills = circuiter_pcb::zones::fill(&layout);
    let v = Panel { rows: 2, columns: 3, separation: Separation::VScore, ..Panel::default() };
    let p = panelize(&layout, &fills, &v).unwrap();
    assert_eq!(p.copies.len(), 6);
    assert!((p.size.0 - 120.0).abs() < 1e-9 && (p.size.1 - 70.0).abs() < 1e-9, "{:?}", p.size);
    // Two lines between the columns, three across (rail, middle, rail).
    assert_eq!(p.vscores.len(), 5);
    assert_eq!(p.layout.parts.len(), layout.parts.len() * 6);
    assert_eq!(p.layout.board.tracks.len(), b.tracks.len() * 6);
    assert_eq!(p.fiducials.len(), 3);
    assert_eq!(p.holes.len(), 4, "tooling only");
    let files = circuiter_pcb::fab::panel_outputs(&p, "divider");
    let names: Vec<&str> = files.iter().map(|o| o.name.as_str()).collect();
    assert!(names.contains(&"divider-panel-V_Score.gbr"), "{names:?}");
    assert!(!names.iter().any(|n| n.contains("bom")));
    let pos = files.iter().find(|o| o.name.ends_with("pos.csv")).unwrap();
    assert_eq!(
        pos.contents.lines().filter(|l| l.starts_with("\"R1\",")).count(),
        6,
        "{}",
        pos.contents
    );
    let cu = files.iter().find(|o| o.name.ends_with("F_Cu.gbr")).unwrap();
    assert!(cu.contents.contains("FiducialPad"));
    // Mouse bites.
    let m = Panel { rows: 2, columns: 2, ..Panel::default() };
    let p = panelize(&layout, &fills, &m).unwrap();
    if let Ok(out) = std::env::var("PANEL_SVG") {
        std::fs::write(out, circuiter_pcb::svg::plot_panel(&p)).unwrap();
    }
    assert!((p.size.0 - 82.0).abs() < 1e-9 && (p.size.1 - 76.0).abs() < 1e-9, "{:?}", p.size);
    assert!(p.vscores.is_empty());
    // Each copy is a slot of its own in the substrate, held by tabs.
    assert_eq!(p.substrate.len(), 1);
    assert!(p.substrate[0].len() >= 2, "{}", p.substrate[0].len());
    let bites = p.holes.iter().filter(|h| h.1 < 1.0).count();
    assert!(bites >= 8 * 3, "{bites}");
    let npth = circuiter_pcb::fab::panel_outputs(&p, "divider")
        .into_iter()
        .find(|o| o.name.ends_with("NPTH.drl"))
        .unwrap();
    assert!(npth.contents.contains("C0.500") && npth.contents.contains("C2.000"));
    // Without rails a single column of copies has nothing holding it but each other.
    let bare = Panel { rows: 2, columns: 1, rails: Length::from_mm(0.0), ..Panel::default() };
    assert!(panelize(&layout, &fills, &bare).is_ok());
    // An odd-shaped board can't be V-scored.
    let mut odd = b.clone();
    odd.outline = vec![pt(0.0, 0.0), pt(40.0, 0.0), pt(40.0, 30.0), pt(10.0, 30.0), pt(0.0, 20.0)];
    let odd = Layout::new(&design, &top, &odd).unwrap();
    assert!(panelize(&odd, &[], &v).is_err());
}

/// The drawings and the test netlist: the fabrication drawing dimensions the board and
/// tables its holes and stack, the assembly drawing names each part, and IPC-D-356
/// lists every pad and via on its net in fixed columns.
#[test]
fn drawings_and_test_netlist() {
    let (design, top, mut b) = routed();
    b.footprints.iter_mut().find(|f| f.path == "R2").unwrap().back = false;
    let layout = Layout::new(&design, &top, &b).unwrap();
    let fab = circuiter_pcb::drawing::fabrication(&layout, "divider");
    assert!(fab.contains(">40.00<") && fab.contains(">30.00<"), "overall size dimensioned");
    assert!(fab.contains("DRILL TABLE") && fab.contains("0.30 mm") && fab.contains("LAYER STACK"));
    assert!(fab.contains("IPC-6012"));
    let asm = circuiter_pcb::drawing::assembly(&layout, "divider");
    for r in ["R1", "R2", "J1"] {
        assert!(asm.contains(&format!(">{r}<")), "{r}");
    }
    assert!(asm.contains("BILL OF MATERIALS") && asm.contains("R1, R2"));
    if let Ok(dir) = std::env::var("DRAWINGS") {
        std::fs::write(format!("{dir}/fab.svg"), &fab).unwrap();
        std::fs::write(format!("{dir}/asm.svg"), &asm).unwrap();
    }
    let ipc = circuiter_pcb::fab::ipc356(&layout, "divider");
    assert!(ipc.contains("P  UNITS CUST 1") && ipc.ends_with("999\n"));
    let records: Vec<&str> =
        ipc.lines().filter(|l| l.starts_with("317") || l.starts_with("327")).collect();
    assert_eq!(records.len(), layout.pads.len() + b.vias.len());
    // R1 pin 1 on vin, through-hole 0.8ish mm drill, at (16.92, −10).
    let r1 = records.iter().find(|l| &l[20..26] == "R1    " && &l[27..31] == "1   ").unwrap();
    assert_eq!(&r1[3..17], "vin           ");
    assert!(r1.contains("X+016920Y-010000"), "{r1}");
    assert!(records.iter().all(|l| l.len() == records[0].len()), "fixed columns");
    let files = circuiter_pcb::fab::outputs(&layout, &[], "divider");
    for n in ["divider-ipc356.ipc", "divider-fab.svg", "divider-assembly.svg"] {
        assert!(files.iter().any(|f| f.name == n), "{n}");
    }
}

/// Assembly variants: the active one leaves parts off and changes values, in the bill
/// of materials, the placement file and 3D; the design as drawn is untouched.
#[test]
fn assembly_variants() {
    use circuiter_model::board::{Variant, VariantPart};
    let (design, top, mut b) = routed();
    let mut lite = Variant { name: "Lite".into(), ..Variant::default() };
    lite.parts.insert("R2".into(), VariantPart { not_fitted: true, ..VariantPart::default() });
    lite.parts.insert(
        "R1".into(),
        VariantPart {
            value: Some("4k7".into()),
            fields: [("MPN".to_owned(), "RC0603-4K7".to_owned())].into(),
            ..VariantPart::default()
        },
    );
    b.variants.push(lite);
    let base = Layout::new(&design, &top, &b).unwrap();
    assert!(circuiter_pcb::fab::bom(&base).contains("\"R1, R2\",\"10k\""));
    b.variant = Some("Lite".into());
    let layout = Layout::new(&design, &top, &b).unwrap();
    let bom = circuiter_pcb::fab::bom(&layout);
    assert!(bom.contains("\"R1\",\"4k7\"") && bom.contains("RC0603-4K7"), "{bom}");
    assert!(bom.lines().any(|l| l.contains("\"R2\"") && l.ends_with("DNP")), "{bom}");
    let pos = circuiter_pcb::fab::positions(&layout);
    assert!(!pos.contains("\"R2\""));
    let built = circuiter_pcb::board3d::build(&design, &layout, &[]);
    assert!(!built.parts.iter().any(|(p, _)| p == "R2"));
    assert!(circuiter_pcb::drawing::assembly(&layout, "divider").contains("R2 DNP"));
    // A variant that's gone leaves the design as drawn.
    b.variant = Some("Gone".into());
    let layout = Layout::new(&design, &top, &b).unwrap();
    assert!(circuiter_pcb::fab::bom(&layout).contains("\"R1, R2\",\"10k\""));
}

/// A shaped board: a round corner and a mounting hole. The pour keeps clear of both,
/// copper crossing the hole is off the board, the router keeps out of it, and the 3D
/// board has the hole through it.
#[test]
fn arcs_and_cutouts_shape_the_board() {
    use circuiter_model::board::Graphic;
    let (design, top) = design();
    let mut b = board();
    // The right edge bows out 5 mm; a 4 mm hole in the bottom right.
    b.bulges = vec![0.0, 2.0 * 5.0 / 30.0];
    b.graphics.push(Graphic::Circle {
        layer: Layer::EdgeCuts,
        center: pt(35.0, 25.0),
        radius: Length::from_mm(2.0),
        width: Length::from_mm(0.1),
    });
    b.zones.push(Zone::new(Layer::BCu, "GND", b.outline.clone()));
    let layout = Layout::new(&design, &top, &b).unwrap();
    let shape = circuiter_pcb::zones::board_shape(&layout);
    let area = circuiter_pcb::geometry::area(&shape);
    // 40 × 30, plus the bow's circular segment (radius 25 mm), less the hole.
    let angle = 2.0 * (15.0f64 / 25.0).asin();
    let expect = 1200.0 + 625.0 / 2.0 * (angle - angle.sin()) - std::f64::consts::PI * 4.0;
    assert!((area - expect).abs() < 0.5, "{area} vs {expect}");
    let fills = circuiter_pcb::zones::fill(&layout);
    let hole = vec![vec![circuiter_pcb::geometry::circle([35.0, 25.0], 2.0)]];
    for f in &fills {
        let inside = circuiter_pcb::geometry::difference(&f.shapes, &hole);
        let lost =
            circuiter_pcb::geometry::area(&f.shapes) - circuiter_pcb::geometry::area(&inside);
        assert!(lost < 1e-6, "the pour stays out of the hole: {lost}");
    }
    // A track over the hole is off the board.
    b.tracks.push(track(&layout, "mid", [30.0, 25.0], [39.0, 25.0], Layer::FCu));
    let crossing = Layout::new(&design, &top, &b).unwrap();
    assert!(count(&drc::check(&crossing), Rule::OutsideBoard) >= 1);
    // The 3D board has the hole: fewer triangles' worth of board over it.
    let built = circuiter_pcb::board3d::build(&design, &layout, &[]);
    let solid = Board::new(40.0, 30.0);
    let plain =
        circuiter_pcb::board3d::build(&design, &Layout::new(&design, &top, &solid).unwrap(), &[]);
    assert!(built.board.triangles() > plain.board.triangles());
}

/// Push and shove: a new track beside another net's pushes it aside just far enough,
/// the track it then runs into goes too, and a new track crossing one can't shove it.
#[test]
fn new_tracks_shove_others_aside() {
    use circuiter_pcb::edit::{margin, shove};
    let (design, top) = design();
    let mut b = board();
    let layout = Layout::new(&design, &top, &b).unwrap();
    // `mid` runs across at y = 15, `GND` 0.5 mm under it.
    b.tracks.push(track(&layout, "mid", [10.0, 15.0], [30.0, 15.0], Layer::FCu));
    b.tracks.push(track(&layout, "GND", [10.0, 15.5], [30.0, 15.5], Layer::FCu));
    let layout = Layout::new(&design, &top, &b).unwrap();
    // `vin` along just above `mid`.
    let path = [[12.0, 14.8], [28.0, 14.8]];
    assert!(margin(&layout, "vin", &path, 0.25, Layer::FCu) < 0.0);
    let pushed = shove(&layout, "vin", &path, 0.25, Layer::FCu).expect("room to push");
    let (mid, gnd) = (&pushed.tracks[0], &pushed.tracks[1]);
    assert!(mid.a.y.mm() > 15.0 && (mid.a.y.mm() - mid.b.y.mm()).abs() < 1e-9, "{mid:?}");
    assert!(gnd.a.y.mm() > 15.5, "GND pushed on by mid: {gnd:?}");
    let mut after = layout.clone();
    after.board = pushed;
    assert!(margin(&after, "vin", &path, 0.25, Layer::FCu) >= -1e-6);
    let seg = [[10.0, after.board.tracks[0].a.y.mm()], [30.0, after.board.tracks[0].b.y.mm()]];
    assert!(margin(&after, "mid", &seg, 0.25, Layer::FCu) >= -1e-6);
    // Straight across `mid`: nothing to shove it to.
    assert!(shove(&layout, "vin", &[[20.0, 12.0], [20.0, 18.0]], 0.25, Layer::FCu).is_none());
}

/// A quad flat pack's body stands on its lands: a foot on every pad and none off them,
/// pin 1's corner where the footprint's pad 1 is.
#[test]
fn a_qfp_body_sits_on_its_lands() {
    use circuiter_pcb::board3d::footprint_point;
    for (pins, pitch, body) in [("48", "0.5", "7"), ("44", "0.8", "10"), ("64", "0.5", "10")] {
        let p: std::collections::BTreeMap<&str, &str> =
            [("pins", pins), ("pitch", pitch), ("body", body)].into();
        let param = |k: &str| p.get(k).map(|v| v.to_string());
        let (pads, _) = circuiter_pcb::footprints::generate("qfp", &param).unwrap();
        let mesh = circuiter_3d::packages::generate("qfp", &param).unwrap();
        let tin = circuiter_3d::color("tin");
        let feet: Vec<[f64; 2]> = mesh
            .vertices
            .iter()
            .filter(|v| v.pos[2] < 0.01 && v.color == tin)
            .map(|v| footprint_point(v.pos))
            .collect();
        let on = |pad: &circuiter_model::board::Pad, q: [f64; 2]| {
            (q[0] - pad.at.x.mm()).abs() <= pad.size.0.mm() / 2.0 + 0.05
                && (q[1] - pad.at.y.mm()).abs() <= pad.size.1.mm() / 2.0 + 0.05
        };
        for q in &feet {
            assert!(pads.iter().any(|pad| on(pad, *q)), "QFP-{pins}: a foot at {q:?} is off");
        }
        for pad in &pads {
            assert!(feet.iter().any(|q| on(pad, *q)), "QFP-{pins}: nothing on pad {}", pad.number);
        }
        assert_eq!(pads.len(), pins.parse::<usize>().unwrap());
    }
}

/// Pours round a fine-pitch part: a ground pin's thermal spokes stop short of the
/// neighbours 0.2 mm away (they once ran through them), and in 3D no pour covers a
/// drilled hole, not even its own net's.
#[test]
fn pours_keep_clear_of_fine_pitch_neighbours_and_holes() {
    use circuiter_model::board::FootprintView;
    let mut d = Design::new();
    d.add_library(circuiter_stdlib::library()).unwrap();
    d.add_library(Library::new(LIB)).unwrap();
    let ports: Vec<String> = (1..=8).map(|k| format!("p{k}")).collect();
    let mut cell = Cell::default();
    for p in &ports {
        let port: circuiter_model::cell::Port = ron::from_str(&format!("(name: \"{p}\")")).unwrap();
        cell.ports.push(port);
    }
    let params = [("pins", "8"), ("pitch", "0.5"), ("body", "3")];
    d.library_mut(LIB).unwrap().insert(CellDef {
        name: "chip".into(),
        symbol: Some(circuiter_model::builder::generate("chip", &cell)),
        cell,
        footprint: Some(FootprintView {
            package: "qfp".into(),
            params: params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            ..FootprintView::default()
        }),
        ..CellDef::default()
    });
    let mut sh = Sheet::new(&d, LIB);
    let u = sh.part("chip", "U1", 0, 0, &[]);
    let j = sh.part("std:header_2", "J1", 20, 0, &[]);
    let nets: Vec<(String, String)> = ports
        .iter()
        .enumerate()
        .map(|(k, p)| (p.clone(), if k == 0 { "GND".into() } else { format!("s{k}") }))
        .collect();
    let pairs: Vec<(&str, &str)> = nets.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    sh.nets(u, &pairs);
    sh.nets(j, &[("p1", "GND"), ("p2", "s1")]);
    let s = sh.s;
    d.library_mut(LIB).unwrap().insert(CellDef {
        name: "fine".into(),
        cell: Cell::default(),
        schematic: Some(s),
        ..CellDef::default()
    });
    let top = CellId::new(LIB, "fine");
    let mut b = Board::new(20.0, 20.0);
    place(&mut b, "U1", 10.0, 8.0, 0.0);
    place(&mut b, "J1", 10.0, 15.0, 0.0);
    let o = b.outline.clone();
    b.zones.push(Zone::new(Layer::FCu, "GND", o.clone()));
    let mut solid = Zone::new(Layer::BCu, "GND", o);
    solid.pads = circuiter_model::board::PadConnection::Solid;
    b.zones.push(solid);
    let layout = Layout::new(&d, &top, &b).unwrap();
    assert!(layout.errors.is_empty(), "{:?}", layout.errors);
    let checked = drc::check(&layout);
    let broken: Vec<_> = checked
        .violations
        .iter()
        .filter(|v| matches!(v.rule, Rule::Short | Rule::Clearance))
        .collect();
    assert!(broken.is_empty(), "{broken:#?}");
    // Pad 1 is joined to the pour by its spokes, so ground needs no routing.
    assert!(
        !checked.connectivity.ratsnest.iter().any(|(n, ..)| n == "GND"),
        "{:?}",
        checked.connectivity.ratsnest
    );
    for f in &checked.fills {
        assert!(!f.shapes.is_empty(), "{:?} poured", f.layer);
    }
    // J1's ground pin is plated and on the pours' net; in 3D its hole is still open
    // through both pours (a drill takes their copper out).
    let hole = layout.pads.iter().find(|p| p.net.as_deref() == Some("GND") && p.drill().is_some());
    let c = hole.unwrap().center;
    let built = circuiter_pcb::board3d::build(&d, &layout, &checked.fills);
    let down = built.board.ray_hit([c[0] as f32, -c[1] as f32, 5.0], [0.0, 0.0, -1.0]);
    assert!(down.is_none(), "something covers the hole: {down:?}");
}

/// The development board example: four layers, every connection routed (the USB
/// pair as a pair), ground pours and planes, and nothing the rules check or the 3D
/// body check objects to.
#[test]
fn the_devboard_example_is_routed_and_clean() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/devboard");
    let project = circuiter_model::Project::load(&dir).unwrap();
    let design = Design::for_project(&project, circuiter_stdlib::library()).unwrap();
    let top = CellId::new("devboard", "devboard");
    // The schematic passes the electrical rules check without a word.
    let erc = circuiter_netlist::erc::check(&design, &top);
    assert!(erc.is_empty(), "{erc:#?}");
    let b = design.cell(&top).and_then(|d| d.board.clone()).expect("a board");
    assert_eq!(b.copper_layers, 4);
    let layout = Layout::new(&design, &top, &b).unwrap();
    assert!(layout.errors.is_empty(), "{:?}", layout.errors);
    assert!(layout.unplaced.is_empty() && layout.orphans.is_empty());
    assert!(layout.parts.len() >= 40, "{} parts", layout.parts.len());
    let checked = drc::check(&layout);
    assert!(checked.violations.is_empty(), "{:#?}", checked.violations);
    assert_eq!(checked.connectivity.unrouted(), 0);
    // Tracks on both outer layers, vias between them, ground and 3.3 V planes inside.
    for l in [Layer::FCu, Layer::BCu] {
        assert!(b.tracks.iter().filter(|t| t.layer == l).count() > 50, "{l:?}");
    }
    assert!(b.vias.len() > 50);
    for (l, net) in [(Layer::In(1), "GND"), (Layer::In(2), "+3V3"), (Layer::FCu, "GND")] {
        let fill = checked.fills.iter().find(|f| f.layer == l && f.net == net).expect("a pour");
        assert!(circuiter_pcb::geometry::area(&fill.shapes) > 1000.0, "{l:?} {net}");
    }
    // USB D+ and D− side by side.
    let pair = &b.pairs[0];
    let skew = (circuiter_pcb::nets::routed_length(&b, &pair.positive)
        - circuiter_pcb::nets::routed_length(&b, &pair.negative))
    .abs();
    assert!(skew < 0.5, "{skew}");
    // Every part but the mounting holes has a body, and none collide.
    let built = circuiter_pcb::board3d::build(&design, &layout, &checked.fills);
    assert!(built.errors.is_empty(), "{:?}", built.errors);
    assert!(built.without.iter().all(|p| p.starts_with('H')), "{:?}", built.without);
    assert!(drc::bodies(&design, &layout).is_empty(), "{:?}", drc::bodies(&design, &layout));
}
