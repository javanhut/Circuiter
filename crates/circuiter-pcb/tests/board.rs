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
