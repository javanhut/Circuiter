//! Writes the example analog lab: everyday circuits built from the standard library's
//! op amps, timers, regulators, sensors, power transistors, relays and motors, each
//! ready to run (or run live).
//!
//! ```text
//! cargo run -p circuiter-stdlib --example analog_lab -- examples/analog_lab
//! ```

use circuiter_model::cell::{Cell, CellDef};
use circuiter_model::schematic::Schematic;
use circuiter_model::{Design, Library, Project};
use std::path::PathBuf;

const PROJECT: &str = "analog_lab";

#[path = "common/sheet.rs"]
mod sheet;
use sheet::Sheet;

fn cell(name: &str, description: &str, s: Schematic) -> CellDef {
    CellDef {
        name: name.into(),
        cell: Cell { description: description.into(), ..Cell::default() },
        schematic: Some(s),
        ..CellDef::default()
    }
}

/// A non-inverting amplifier with a gain of 11, watched by an oscilloscope.
fn amplifier(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let vp = sh.part("std:vdc", "VP", 0, 0, &[("V", "12")]);
    let vn = sh.part("std:vdc", "VN", 0, 16, &[("V", "12")]);
    let sig = sh.part("std:vsine", "SIG", 16, 0, &[("amplitude", "0.5"), ("frequency", "1k")]);
    let u1 = sh.part("std:tl072", "U1", 36, 4, &[]);
    let rf = sh.part("std:resistor", "RF", 56, 0, &[("R", "10k")]);
    let rg = sh.part("std:resistor", "RG", 56, 12, &[("R", "1k")]);
    let rl = sh.part("std:resistor", "RL", 76, 4, &[("R", "10k")]);
    let scope = sh.part("std:oscilloscope", "SCOPE", 92, 0, &[]);
    sh.nets(vp, &[("p", "vp"), ("n", "GND")]);
    sh.nets(vn, &[("p", "GND"), ("n", "vn")]);
    sh.nets(sig, &[("p", "in"), ("n", "GND")]);
    sh.nets(u1, &[("inp", "in"), ("inn", "fb"), ("out", "out"), ("vp", "vp"), ("vn", "vn")]);
    sh.nets(rf, &[("a", "out"), ("b", "fb")]);
    sh.nets(rg, &[("a", "fb"), ("b", "GND")]);
    sh.nets(rl, &[("a", "out"), ("b", "GND")]);
    sh.nets(scope, &[("ch1", "in"), ("ch2", "out"), ("com", "GND")]);
    cell(
        "amplifier",
        "Non-inverting amplifier: a TL072 with a gain of 1 + RF/RG = 11 on ±12 V. Turn the \
         input up past 1 V and the output clips short of the rails",
        sh.s,
    )
}

/// A 555 blinking an LED at about 1.5 Hz from a 9 V battery.
fn blinker(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let bat = sh.part("std:battery", "BT1", 0, 0, &[]);
    let u1 = sh.part("std:ne555", "U1", 24, 0, &[]);
    let r1 = sh.part("std:resistor", "R1", 0, 24, &[("R", "1k")]);
    let r2 = sh.part("std:resistor", "R2", 16, 24, &[("R", "47k")]);
    let c1 = sh.part("std:electrolytic", "C1", 32, 24, &[("C", "10u")]);
    let c2 = sh.part("std:capacitor", "C2", 44, 24, &[("C", "10n")]);
    let r3 = sh.part("std:resistor", "R3", 56, 0, &[("R", "470")]);
    let d1 = sh.part("std:led", "D1", 72, 0, &[]);
    sh.nets(bat, &[("p", "vcc"), ("n", "GND")]);
    sh.nets(
        u1,
        &[
            ("gnd", "GND"),
            ("trig", "t"),
            ("out", "out"),
            ("rst", "vcc"),
            ("ctrl", "ctl"),
            ("thr", "t"),
            ("dis", "dis"),
            ("vcc", "vcc"),
        ],
    );
    sh.nets(r1, &[("a", "vcc"), ("b", "dis")]);
    sh.nets(r2, &[("a", "dis"), ("b", "t")]);
    sh.nets(c1, &[("p", "t"), ("n", "GND")]);
    sh.nets(c2, &[("a", "ctl"), ("b", "GND")]);
    sh.nets(r3, &[("a", "out"), ("b", "led")]);
    sh.nets(d1, &[("a", "led"), ("k", "GND")]);
    cell(
        "blinker",
        "555 astable blinking an LED: f ≈ 1.44 / ((R1 + 2 R2) C1), about 1.5 Hz. Run it live",
        sh.s,
    )
}

/// Mains through a centre-tapped transformer, two rectifiers and a reservoir to a 7805.
fn power_supply(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let mains = sh.part("std:vsine", "MAINS", 0, 0, &[("amplitude", "325"), ("frequency", "50")]);
    let t1 = sh.part("std:transformer_ct", "T1", 18, 0, &[("lp", "10"), ("n", "15")]);
    let d1 = sh.part("std:rectifier", "D1", 40, 0, &[]);
    let d2 = sh.part("std:rectifier", "D2", 40, 12, &[]);
    let c1 = sh.part("std:electrolytic", "C1", 56, 0, &[("C", "2200u")]);
    let u1 = sh.part("std:lm7805", "U1", 72, 0, &[]);
    let c2 = sh.part("std:capacitor", "C2", 92, 0, &[("C", "100n")]);
    let rl = sh.part("std:resistor", "RL", 104, 0, &[("R", "50")]);
    let dmm = sh.part("std:multimeter", "DMM1", 120, 0, &[]);
    sh.nets(mains, &[("p", "live"), ("n", "GND")]);
    sh.nets(t1, &[("p1", "live"), ("p2", "GND"), ("s1", "s1"), ("ct", "GND"), ("s2", "s2")]);
    sh.nets(d1, &[("a", "s1"), ("k", "raw")]);
    sh.nets(d2, &[("a", "s2"), ("k", "raw")]);
    sh.nets(c1, &[("p", "raw"), ("n", "GND")]);
    sh.nets(u1, &[("in", "raw"), ("gnd", "GND"), ("out", "5v")]);
    sh.nets(c2, &[("a", "5v"), ("b", "GND")]);
    sh.nets(rl, &[("a", "5v"), ("b", "GND")]);
    sh.nets(dmm, &[("p", "5v"), ("com", "GND")]);
    cell(
        "power_supply",
        "Linear power supply: 230 V mains, a 15:1 centre-tapped transformer, full-wave \
         rectifiers, a 2200 µF reservoir and a 7805 giving 5 V at 100 mA. Watch the ripple on \
         raw and the regulator's temperature",
        sh.s,
    )
}

/// An LDR and a comparator lighting an LED when it gets bright; the pot sets the level.
fn light_switch(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let v1 = sh.part("std:vdc", "V1", 0, 0, &[("V", "5")]);
    let r1 = sh.part("std:resistor", "R1", 14, 0, &[("R", "10k")]);
    let ldr = sh.part("std:ldr", "LDR1", 14, 12, &[("lux", "1")]);
    let pot = sh.part("std:potentiometer", "RV1", 30, 0, &[("position", "0.3")]);
    let u1 = sh.part("std:lm393", "U1", 50, 4, &[]);
    let r2 = sh.part("std:resistor", "R2", 70, 0, &[("R", "330")]);
    let d1 = sh.part("std:led", "D1", 86, 0, &[]);
    sh.nets(v1, &[("p", "vcc"), ("n", "GND")]);
    sh.nets(r1, &[("a", "vcc"), ("b", "sense")]);
    sh.nets(ldr, &[("a", "sense"), ("b", "GND")]);
    sh.nets(pot, &[("a", "vcc"), ("w", "level"), ("b", "GND")]);
    sh.nets(
        u1,
        &[("inp", "sense"), ("inn", "level"), ("out", "out"), ("vp", "vcc"), ("vn", "GND")],
    );
    sh.nets(r2, &[("a", "vcc"), ("b", "led")]);
    sh.nets(d1, &[("a", "led"), ("k", "out")]);
    cell(
        "light_switch",
        "Light switch: as light on the LDR rises its resistance falls, sense drops below the \
         level set by RV1 (3.5 V: about 3 lux), and the comparator pulls the LED on. It starts \
         in the dark: run it live and turn the light up",
        sh.s,
    )
}

/// A logic-level switch turning a power MOSFET on into a motor, with a flyback diode.
fn motor_driver(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let v1 = sh.part("std:vdc", "V1", 0, 0, &[("V", "12")]);
    let sw = sh.part("std:switch", "SW1", 14, 0, &[]);
    let rg = sh.part("std:resistor", "RG", 30, 0, &[("R", "100")]);
    let rpd = sh.part("std:resistor", "RPD", 30, 12, &[("R", "100k")]);
    let q1 = sh.part("std:nmos_power", "Q1", 50, 4, &[]);
    let m1 = sh.part("std:dc_motor", "M1", 66, 0, &[]);
    let d1 = sh.part("std:schottky", "D1", 80, 0, &[]);
    let dmm = sh.part("std:multimeter", "DMM1", 96, 0, &[("mode", "A")]);
    sh.nets(v1, &[("p", "vcc"), ("n", "GND")]);
    sh.nets(sw, &[("a", "vcc"), ("b", "drive")]);
    sh.nets(rg, &[("a", "drive"), ("b", "gate")]);
    sh.nets(rpd, &[("a", "gate"), ("b", "GND")]);
    sh.nets(q1, &[("d", "drain"), ("g", "gate"), ("s", "GND")]);
    sh.nets(m1, &[("a", "motor"), ("b", "drain")]);
    sh.nets(d1, &[("a", "drain"), ("k", "motor")]);
    sh.nets(dmm, &[("p", "vcc"), ("com", "motor")]);
    cell(
        "motor_driver",
        "Motor driver: SW1 turns the MOSFET on and the motor spins up; its current starts high \
         and falls as the back-EMF builds. D1 catches the winding's kick when it switches off",
        sh.s,
    )
}

/// A pushbutton driving a transistor into a relay coil; the contacts switch a lamp.
fn relay_driver(design: &Design) -> CellDef {
    let mut sh = Sheet::new(design, PROJECT);
    let v1 = sh.part("std:vdc", "V1", 0, 0, &[("V", "5")]);
    let pb = sh.part("std:pushbutton", "PB1", 14, 0, &[]);
    let rb = sh.part("std:resistor", "RB", 30, 0, &[("R", "4.7k")]);
    let q1 = sh.part("std:npn", "Q1", 46, 4, &[]);
    let k1 = sh.part("std:relay", "K1", 62, 0, &[]);
    let d1 = sh.part("std:diode", "D1", 62, 20, &[]);
    let bt = sh.part("std:battery", "BT1", 84, 0, &[("V", "12"), ("r", "0.1")]);
    let lp = sh.part("std:lamp", "LP1", 100, 0, &[]);
    let r2 = sh.part("std:resistor", "R2", 100, 16, &[("R", "1k")]);
    let d2 = sh.part("std:led", "D2", 116, 16, &[]);
    sh.nets(v1, &[("p", "vcc"), ("n", "GND")]);
    sh.nets(pb, &[("a", "vcc"), ("b", "press")]);
    sh.nets(rb, &[("a", "press"), ("b", "base")]);
    sh.nets(q1, &[("c", "coil"), ("b", "base"), ("e", "GND")]);
    sh.nets(
        k1,
        &[("c1", "vcc"), ("c2", "coil"), ("com", "12v"), ("no", "lamp"), ("nc", "standby")],
    );
    sh.nets(d1, &[("a", "coil"), ("k", "vcc")]);
    sh.nets(bt, &[("p", "12v"), ("n", "GND")]);
    sh.nets(lp, &[("a", "lamp"), ("b", "GND")]);
    sh.nets(r2, &[("a", "standby"), ("b", "led")]);
    sh.nets(d2, &[("a", "led"), ("k", "GND")]);
    cell(
        "relay_driver",
        "Relay driver: hold PB1 and the transistor energises the relay, whose contacts move \
         from the standby LED to the 12 V lamp. D1 across the coil protects the transistor \
         when it lets go",
        sh.s,
    )
}

fn main() {
    let dir =
        PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "examples/analog_lab".into()));
    std::fs::create_dir_all(&dir).expect("project directory");
    let mut design = Design::new();
    design.add_library(circuiter_stdlib::library()).expect("std");
    design.add_library(Library::new(PROJECT)).expect("project");
    let mut library = Library::new(PROJECT);
    for make in [amplifier, blinker, power_supply, light_switch, motor_driver, relay_driver] {
        library.insert(make(&design));
    }
    let manifest = dir.join(circuiter_model::project::MANIFEST_FILE);
    let project = if manifest.is_file() {
        let mut p = Project::load(&dir).expect("existing project");
        p.library = library;
        p
    } else {
        let mut p = Project::create(&dir, PROJECT).expect("new project");
        p.manifest.project.top = "amplifier".into();
        p.manifest.project.description = "Everyday analog circuits: an op-amp amplifier, a 555 \
             blinker, a mains power supply, a light switch, a motor driver and a relay driver"
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
