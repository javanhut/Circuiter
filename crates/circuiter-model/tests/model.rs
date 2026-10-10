use circuiter_model::check::{Severity, check_libraries};
use circuiter_model::{CellId, Design, Library, Project};
use std::path::Path;

fn library(name: &str, files: &[(&str, &str)]) -> Library {
    Library::from_files(name, files.iter().map(|(p, t)| (Path::new(*p), *t))).unwrap()
}

fn errors(design: &Design, lib: &str) -> Vec<String> {
    check_libraries(design, &[lib])
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.to_string())
        .collect()
}

fn assert_reported(errors: &[String], needle: &str) {
    assert!(errors.iter().any(|e| e.contains(needle)), "expected `{needle}` in {errors:#?}");
}

const RES_CELL: &str = r#"(prefix: "R", ports: [(name: "a"), (name: "b")], params: [(name: "R", kind: Real(unit: "Ω"))])"#;
const RES_SYMBOL: &str = r#"(pins: [(port: "a", at: (-5.08, 0), direction: Right), (port: "b", at: (5.08, 0), direction: Left)])"#;

#[test]
fn checker_reports_broken_references_and_interfaces() {
    let lib = library(
        "lib",
        &[
            ("cells/res/cell.ron", RES_CELL),
            ("cells/res/symbol.ron", RES_SYMBOL),
            ("cells/res/spice.ron", r#"(template: "R${inst} ${node.a} ${node.c} ${param.R}")"#),
            ("cells/top/cell.ron", r#"(ports: [(name: "p")])"#),
            (
                "cells/top/schematic.ron",
                r#"(
                    instances: [
                        (name: "R1", cell: "res", placement: (position: (0, 0))),
                        (name: "R1", cell: "res", placement: (position: (0, 0)), params: {"R": "lots"}),
                        (name: "X1", cell: "std:nosuch", placement: (position: (0, 0))),
                    ],
                    wires: [(points: [(0, 0)])],
                )"#,
            ),
        ],
    );
    let mut design = Design::new();
    design.add_library(lib).unwrap();
    let errors = errors(&design, "lib");

    assert_reported(&errors, "lib:res/spice: `${node.c}` is not a port");
    assert_reported(&errors, "instance R1: required parameter `R` is not set");
    assert_reported(&errors, "instance R1: name is used more than once");
    assert_reported(&errors, "instance R1: parameter `R`: invalid quantity `lots`");
    assert_reported(&errors, "instance X1: unknown cell `std:nosuch`");
    assert_reported(&errors, "fewer than two points");
    assert_reported(&errors, "cell port `p` is not placed");
}

#[test]
fn checker_and_hierarchy_reject_recursive_instantiation() {
    let schematic = |child: &str| {
        format!(r#"(instances: [(name: "U1", cell: "{child}", placement: (position: (0, 0)))])"#)
    };
    let (a, b) = (schematic("b"), schematic("a"));
    let lib = library(
        "lib",
        &[
            ("cells/a/cell.ron", "()"),
            ("cells/a/symbol.ron", "()"),
            ("cells/a/schematic.ron", &a),
            ("cells/b/cell.ron", "()"),
            ("cells/b/symbol.ron", "()"),
            ("cells/b/schematic.ron", &b),
        ],
    );
    let mut design = Design::new();
    design.add_library(lib).unwrap();

    assert_reported(&errors(&design, "lib"), "recursive instantiation: lib:a -> lib:b -> lib:a");
    let err = design.hierarchy(&CellId::new("lib", "a")).unwrap_err();
    assert!(err.to_string().contains("recursive instantiation"), "{err}");
}

#[test]
fn unknown_view_files_are_rejected() {
    let files = [("cells/res/cell.ron", RES_CELL), ("cells/res/symbl.ron", RES_SYMBOL)];
    let err =
        Library::from_files("lib", files.iter().map(|(p, t)| (Path::new(*p), *t))).unwrap_err();
    assert!(err.to_string().contains("unknown view file `symbl.ron`"), "{err}");
}

#[test]
fn project_round_trips_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    let created = Project::create(dir.path(), "demo").unwrap();
    let loaded = Project::load(dir.path()).unwrap();
    assert_eq!(created, loaded);

    let before = snapshot(dir.path());
    loaded.save().unwrap();
    assert_eq!(before, snapshot(dir.path()));

    assert!(Project::create(dir.path(), "demo").is_err(), "must not overwrite a project");
    assert_eq!(Project::find_root(&dir.path().join("cells/top")), Some(dir.path().to_path_buf()));
}

#[test]
fn example_files_are_canonical() {
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for entry in std::fs::read_dir(&examples).unwrap() {
        let example = entry.unwrap().path();
        let mut project = Project::load(&example).unwrap();
        // CIRCUITER_BLESS=1 rewrites the examples in canonical form.
        if std::env::var_os("CIRCUITER_BLESS").is_some() {
            project.save().unwrap();
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        project.root = dir.path().to_path_buf();
        project.save().unwrap();
        // Saving rewrites every file in canonical form; the example must already be in it.
        assert_eq!(snapshot(dir.path()), snapshot(&example), "{}", example.display());
    }
}

#[test]
fn reserved_and_invalid_project_names_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Project::create(dir.path(), "std").is_err());
    assert!(Project::create(dir.path(), "my-project").is_err());
}

fn snapshot(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(root).unwrap().display().to_string();
                out.push((rel, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out.sort();
    out
}

mod connectivity {
    use super::*;
    use circuiter_model::connectivity::{Connectivity, Item, NetNameSource};
    use circuiter_model::schematic::Schematic;

    /// A library with a two-pin `res` cell (pins at x = ±5.08) and a ground symbol.
    fn design() -> Design {
        let lib = library(
            "lib",
            &[
                ("cells/res/cell.ron", RES_CELL),
                ("cells/res/symbol.ron", RES_SYMBOL),
                ("cells/gnd/cell.ron", r#"(ports: [(name: "g")], global_net: "GND")"#),
                ("cells/gnd/symbol.ron", r#"(pins: [(port: "g", at: (0, 0), direction: Down)])"#),
            ],
        );
        let mut design = Design::new();
        design.add_library(lib).unwrap();
        design
    }

    fn analyze(schematic: &str) -> (Schematic, Connectivity) {
        let s: Schematic = ron::from_str(schematic).unwrap();
        let c = Connectivity::analyze(&design(), "lib", &s);
        (s, c)
    }

    fn net_of_pin(c: &Connectivity, s: &Schematic, inst: &str, port: &str) -> usize {
        let i = c
            .pins
            .iter()
            .position(|p| s.instances[p.instance].name == inst && p.port == port)
            .unwrap();
        c.net_of(Item::Pin(i)).unwrap()
    }

    #[test]
    fn crossing_wires_do_not_connect_but_t_junctions_do() {
        let (s, c) = analyze(
            r#"(
                instances: [
                    (name: "R1", cell: "res", placement: (position: (0, 0))),
                    (name: "R2", cell: "res", placement: (position: (20, 0))),
                    (name: "R3", cell: "res", placement: (position: (40, 0))),
                ],
                wires: [
                    // R1.b to R2.a, then a T from the middle of that wire up to (10, -10).
                    (points: [(5.08, 0), (14.92, 0)]),
                    (points: [(10, 0), (10, -10)]),
                    // A wire crossing the first one with no vertex at the crossing.
                    (points: [(12, -5), (12, 5)]),
                ],
            )"#,
        );
        assert_eq!(net_of_pin(&c, &s, "R1", "b"), net_of_pin(&c, &s, "R2", "a"));
        assert_eq!(c.net_of(Item::Wire(1)), c.net_of(Item::Wire(0)), "T joins the wire");
        assert_ne!(c.net_of(Item::Wire(2)), c.net_of(Item::Wire(0)), "crossing must not join");
        assert_eq!(c.junctions, vec![circuiter_model::Point::mm(10.0, 0.0)]);
        // R1.a, R2.b and both R3 pins are open, plus the free ends of wires 1 and 2.
        assert_eq!(c.dangling_pins.len(), 4);
        assert_eq!(c.dangling_wire_ends.len(), 3);
    }

    #[test]
    fn a_wire_passing_over_a_pin_does_not_connect_to_it() {
        let (s, c) = analyze(
            r#"(
                instances: [
                    (name: "R1", cell: "res", placement: (position: (0, 0))),
                    (name: "R2", cell: "res", placement: (position: (20, 0))),
                    (name: "R3", cell: "res", placement: (position: (10.16, 10.16), rotation: R90)),
                ],
                wires: [
                    // R3 stands upright with pins at (10.16, 5.08) and (10.16, 15.24); this
                    // wire runs straight over the upper one.
                    (points: [(0, 5.08), (20, 5.08)]),
                    // And one that properly ends on R1.b.
                    (points: [(5.08, 0), (5.08, -5)]),
                ],
            )"#,
        );
        let pin = |inst: &str, port: &str| {
            c.pins
                .iter()
                .position(|p| s.instances[p.instance].name == inst && p.port == port)
                .unwrap()
        };
        assert_ne!(c.net_of(Item::Pin(pin("R3", "b"))), c.net_of(Item::Wire(0)));
        assert_eq!(c.passes_over, vec![(pin("R3", "b"), 0)]);
        assert_eq!(c.net_of(Item::Pin(pin("R1", "b"))), c.net_of(Item::Wire(1)));
    }

    #[test]
    fn labels_and_global_nets_join_and_name_nets() {
        let (s, c) = analyze(
            r#"(
                instances: [
                    (name: "R1", cell: "res", placement: (position: (0, 0))),
                    (name: "R2", cell: "res", placement: (position: (30, 0))),
                    (name: "G1", cell: "gnd", placement: (position: (-5.08, 0))),
                    (name: "G2", cell: "gnd", placement: (position: (35.08, 0))),
                ],
                labels: [(net: "MID", at: (5.08, 0)), (net: "MID", at: (24.92, 0))],
            )"#,
        );
        let mid = net_of_pin(&c, &s, "R1", "b");
        assert_eq!(mid, net_of_pin(&c, &s, "R2", "a"));
        assert_eq!(c.nets[mid].name, "MID");
        assert_eq!(c.nets[mid].source, NetNameSource::Label);
        let gnd = net_of_pin(&c, &s, "R1", "a");
        assert_eq!(gnd, net_of_pin(&c, &s, "R2", "b"));
        assert_eq!(c.nets[gnd].name, "GND");
        assert!(c.dangling_pins.is_empty(), "{:?}", c.dangling_pins);
        assert!(c.junctions.is_empty());
    }

    #[test]
    fn unlabelled_nets_get_stable_auto_names() {
        let (_, c) = analyze(
            r#"(
                instances: [(name: "R1", cell: "res", placement: (position: (0, 0)))],
                wires: [(points: [(5.08, 0), (10, 0)])],
            )"#,
        );
        let names: Vec<&str> = c.nets.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["N$1", "N$2"]);
    }
}

#[test]
fn cells_can_be_sorted_into_folders() {
    let lib = library(
        "lib",
        &[
            ("cells/passives/res/cell.ron", RES_CELL),
            ("cells/passives/res/symbol.ron", RES_SYMBOL),
            ("cells/analog/filters/rc/cell.ron", RES_CELL),
            ("cells/top/cell.ron", RES_CELL),
        ],
    );
    assert_eq!(lib.cell("res").unwrap().folder, "passives");
    assert_eq!(lib.cell("rc").unwrap().folder, "analog/filters");
    assert_eq!(lib.cell("top").unwrap().folder, "");
    assert_eq!(lib.all_folders(), ["analog", "analog/filters", "passives"]);

    // Names are unique across folders: references don't mention folders.
    let clash = Library::from_files(
        "lib",
        [
            (Path::new("cells/a/res/cell.ron"), RES_CELL),
            (Path::new("cells/b/res/cell.ron"), RES_CELL),
        ],
    );
    assert!(clash.unwrap_err().to_string().contains("two folders"));
    let bad = Library::from_files("lib", [(Path::new("cells/9x/res/cell.ron"), RES_CELL)]);
    assert!(bad.unwrap_err().to_string().contains("must start with a letter"));
}

#[test]
fn moving_a_cell_to_another_folder_moves_its_files() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = library(
        "lib",
        &[
            ("cells/res/cell.ron", RES_CELL),
            ("cells/res/symbol.ron", RES_SYMBOL),
            ("cells/keep/cell.ron", RES_CELL),
        ],
    );
    lib.save_dir(tmp.path()).unwrap();
    std::fs::write(tmp.path().join("cells/keep/notes.txt"), "mine").unwrap();
    // Into a nested folder, then back out of it.
    lib.cells.get_mut("res").unwrap().folder = "parts/passive".into();
    lib.cells.get_mut("keep").unwrap().folder = "parts".into();
    lib.save_dir(tmp.path()).unwrap();
    assert!(!tmp.path().join("cells/res").exists(), "old place removed");
    assert!(tmp.path().join("cells/parts/passive/res/symbol.ron").is_file());
    assert!(
        tmp.path().join("cells/keep/notes.txt").is_file(),
        "a directory with someone's own files in it stays"
    );
    assert!(!tmp.path().join("cells/keep/cell.ron").exists());
    let loaded = Library::load_dir("lib", tmp.path()).unwrap();
    assert_eq!(loaded.cells, lib.cells);
    assert_eq!(loaded.all_folders(), ["keep", "parts", "parts/passive"]);

    // A folder emptied by a move stays until deleted, like any folder.
    let mut lib = loaded;
    lib.cells.get_mut("res").unwrap().folder = String::new();
    lib.save_dir(tmp.path()).unwrap();
    assert!(tmp.path().join("cells/parts/passive").is_dir());
    assert!(tmp.path().join("cells/parts/keep").is_dir());
    lib.folders.remove("parts/passive");
    lib.save_dir(tmp.path()).unwrap();
    assert!(!tmp.path().join("cells/parts/passive").exists(), "deleted");
    assert_eq!(Library::load_dir("lib", tmp.path()).unwrap().cells, lib.cells);
}

#[test]
fn empty_folders_are_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let mut lib = library("lib", &[("cells/res/cell.ron", RES_CELL)]);
    lib.folders.insert("cpu/alu/adders".into());
    lib.folders.insert("cpu/regs".into());
    lib.save_dir(tmp.path()).unwrap();
    std::fs::create_dir_all(tmp.path().join("cells/my notes")).unwrap();
    let loaded = Library::load_dir("lib", tmp.path()).unwrap();
    assert_eq!(loaded.all_folders(), ["cpu", "cpu/alu", "cpu/alu/adders", "cpu/regs"]);
    assert!(!loaded.folder_has_cells("cpu"));
    assert!(loaded.folder_has_cells(""));
}
