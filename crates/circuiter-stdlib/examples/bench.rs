//! Writes the example bench project: a circuit to run live, with a supply to turn, a
//! switch, a logic switch and a button, LEDs, a counter, multimeters and an
//! oscilloscope.
//!
//! ```text
//! cargo run -p circuiter-stdlib --example bench -- examples/bench
//! ```

use circuiter_model::cell::CellDef;
use circuiter_model::{Design, Library, Project};
use std::path::PathBuf;

const PROJECT: &str = "bench";

#[path = "common/sheet.rs"]
mod sheet;
use sheet::Sheet;

fn bench(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    // An LED on a supply you can turn, behind a switch; its voltage on a meter.
    let vs = sh.part("std:dial", "VS", 0, 0, &[("value", "5"), ("max", "12")]);
    let sw = sh.part("std:switch", "SW1", 14, 0, &[]);
    let r1 = sh.part("std:resistor", "R1", 30, 0, &[("R", "330")]);
    let d1 = sh.part("std:led", "D1", 46, 0, &[]);
    let m1 = sh.part("std:multimeter", "DMM1", 62, 0, &[]);
    sh.nets(vs, &[("p", "supply"), ("n", "GND")]);
    sh.nets(sw, &[("a", "supply"), ("b", "switched")]);
    sh.nets(r1, &[("a", "switched"), ("b", "led1")]);
    sh.nets(d1, &[("a", "led1"), ("k", "GND")]);
    sh.nets(m1, &[("p", "led1"), ("com", "GND")]);
    // A potentiometer across the supply, its wiper on a second meter.
    let pot = sh.part("std:potentiometer", "RV1", 0, 22, &[]);
    let m2 = sh.part("std:multimeter", "DMM2", 20, 22, &[]);
    sh.nets(pot, &[("a", "supply"), ("w", "wiper"), ("b", "GND")]);
    sh.nets(m2, &[("p", "wiper"), ("com", "GND")]);
    // A 4 Hz clock counted while the logic switch enables the counter; bit 0 lights an
    // LED. The button clears the count.
    let clk = sh.part("std:clock", "CLK", 0, 44, &[("frequency", "4")]);
    let en = sh.part("std:toggle", "EN", 14, 44, &[]);
    let rst = sh.part("std:reset", "RST", 0, 60, &[("length", "300m")]);
    let btn = sh.part("std:button", "BTN", 14, 60, &[]);
    let g2 = sh.part("std:or", "G2", 28, 60, &[]);
    let cnt = sh.part("std:counter", "CNT", 44, 44, &[("width", "4")]);
    let b0 = sh.part("std:slice", "B0", 64, 44, &[("from", "4"), ("lo", "0"), ("width", "1")]);
    let r2 = sh.part("std:resistor", "R2", 82, 44, &[("R", "330")]);
    let d2 = sh.part("std:led", "D2", 98, 44, &[]);
    sh.nets(clk, &[("p", "clk"), ("n", "GND")]);
    sh.nets(en, &[("y", "enable")]);
    sh.nets(rst, &[("rst", "power_on")]);
    sh.nets(btn, &[("y", "button")]);
    sh.nets(g2, &[("a", "power_on"), ("b", "button"), ("y", "clear")]);
    sh.nets(cnt, &[("clk", "clk"), ("rst", "clear"), ("en", "enable"), ("q", "count")]);
    sh.nets(b0, &[("a", "count"), ("y", "bit0")]);
    sh.nets(r2, &[("a", "bit0"), ("b", "led2")]);
    sh.nets(d2, &[("a", "led2"), ("k", "GND")]);
    // A 1 Hz square wave into an RC filter (τ = 0.1 s), on the oscilloscope.
    let sq = sh.part(
        "std:vpulse",
        "VSQ",
        0,
        82,
        &[
            ("low", "0"),
            ("high", "5"),
            ("rise", "1m"),
            ("fall", "1m"),
            ("width", "0.5"),
            ("period", "1"),
        ],
    );
    let r3 = sh.part("std:resistor", "R3", 16, 82, &[("R", "100k")]);
    let c1 = sh.part("std:capacitor", "C1", 32, 82, &[("C", "1u")]);
    let scope = sh.part("std:oscilloscope", "SCOPE1", 46, 82, &[]);
    sh.nets(sq, &[("p", "square"), ("n", "GND")]);
    sh.nets(r3, &[("a", "square"), ("b", "filtered")]);
    sh.nets(c1, &[("a", "filtered"), ("b", "GND")]);
    sh.nets(scope, &[("ch1", "square"), ("ch2", "filtered"), ("com", "GND")]);
    CellDef {
        name: "bench".into(),
        cell: circuiter_model::cell::Cell {
            description:
                "Run it live: turn VS and RV1, flip SW1 and EN, hold BTN, and watch the meters, \
                          the LEDs and the oscilloscope"
                    .into(),
            ..Default::default()
        },
        schematic: Some(sh.s),
        ..CellDef::default()
    }
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "examples/bench".into()));
    std::fs::create_dir_all(&dir).expect("project directory");
    let mut design = Design::new();
    design.add_library(circuiter_stdlib::library()).expect("std");
    design.add_library(Library::new(PROJECT)).expect("project");
    let cell = bench(&design);
    let mut library = Library::new(PROJECT);
    library.insert(cell);
    let manifest = dir.join(circuiter_model::project::MANIFEST_FILE);
    let project = if manifest.is_file() {
        let mut p = Project::load(&dir).expect("existing project");
        p.library = library;
        p
    } else {
        let mut p = Project::create(&dir, PROJECT).expect("new project");
        p.manifest.project.top = "bench".into();
        p.manifest.project.description =
            "An electronics bench: run it live and operate switches, knobs and buttons while \
             meters and an oscilloscope watch"
                .into();
        p.library = library;
        p
    };
    project.save().expect("saved");
    let stray = dir.join("cells/top");
    if stray.join("cell.ron").is_file() {
        std::fs::remove_dir_all(stray).expect("removing the empty top cell");
    }
    println!("wrote {}", dir.display());
}
