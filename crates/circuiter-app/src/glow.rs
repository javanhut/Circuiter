//! Parts that give light (LEDs, lamps) and how brightly they shine, from the node
//! voltages of a solution: an LED by the current through its series resistance, a lamp
//! by its filament's temperature. Running live, brightness is averaged over the time
//! points since the last frame, so a dimmed (PWM) LED looks dim rather than flickering.

use circuiter_model::{CellId, Design, STD_LIBRARY};
use circuiter_netlist::Netlist;
use circuiter_sim::Circuit;

#[derive(Debug, Clone)]
enum Source {
    /// Current = (anode − junction) / rs, against `rated` for full brightness.
    Led { anode: usize, junction: usize, rs: f64, rated: f64 },
    /// The filament's temperature rise: 9 when hot.
    Lamp { temp: usize },
}

/// A light-giving part in a circuit.
#[derive(Debug, Clone)]
pub struct Emitter {
    pub path: String,
    pub color: [u8; 3],
    source: Source,
}

/// The colour a part glows.
pub fn color(name: &str) -> [u8; 3] {
    match name.trim() {
        "orange" => [255, 150, 40],
        "yellow" => [255, 225, 60],
        "green" => [70, 255, 90],
        "blue" => [70, 130, 255],
        "white" => [235, 240, 255],
        "lamp" => [255, 214, 140],
        _ => [255, 50, 40],
    }
}

impl Emitter {
    /// Every LED and lamp of `netlist`, as built into `circuit`.
    pub fn all(
        design: &Design,
        top: &CellId,
        netlist: &Netlist,
        circuit: &Circuit,
    ) -> Vec<Emitter> {
        let mut out = Vec::new();
        for e in &netlist.elements {
            if e.cell.library != STD_LIBRARY {
                continue;
            }
            let Some(device) = e.devices().next() else { continue };
            let params = crate::live::instance_params(design, top, &e.path);
            let param = |name: &str, default: &str| {
                params.get(name).cloned().unwrap_or_else(|| default.to_owned())
            };
            let number = |name: &str, default: f64| {
                params
                    .get(name)
                    .and_then(|v| circuiter_model::units::parse_quantity(v, None).ok())
                    .unwrap_or(default)
            };
            let node_of = |port: &str| {
                let net = e.ports.iter().find(|(p, _)| p == port)?.1;
                circuit.node(&netlist.nets[net].node)
            };
            match e.cell.cell.as_str() {
                "led" => {
                    // The series resistance puts the junction on a node of its own.
                    let (Some(anode), Some(junction)) =
                        (node_of("a"), circuit.node(&format!("{device}#a")))
                    else {
                        continue;
                    };
                    out.push(Emitter {
                        path: e.path.clone(),
                        color: color(&param("color", "red")),
                        source: Source::Led { anode, junction, rs: number("rs", 2.0), rated: 0.02 },
                    });
                }
                "lamp" => {
                    // The template names its filament node after the instance: B<inst>.
                    let inst = device.get(1..).unwrap_or_default();
                    let Some(temp) = circuit.node(&format!("{inst}_t")) else { continue };
                    out.push(Emitter {
                        path: e.path.clone(),
                        color: color("lamp"),
                        source: Source::Lamp { temp },
                    });
                }
                _ => {}
            }
        }
        out
    }

    /// How brightly it shines at a solution whose node voltages `v` gives: 0 (dark) to
    /// 1 (fully lit at its rated current), more when overdriven.
    pub fn level(&self, v: &dyn Fn(usize) -> f64) -> f32 {
        match self.source {
            Source::Led { anode, junction, rs, rated } => {
                let i = (v(anode) - v(junction)) / rs.max(1e-6);
                // Barely visible below a tenth of a milliamp; brightness looks roughly
                // like the square root of the current.
                if i < 1e-4 { 0.0 } else { ((i / rated).sqrt() as f32).min(1.5) }
            }
            Source::Lamp { temp } => {
                let t = (v(temp) / 9.0).max(0.0);
                // A filament only glows when hot.
                ((t * t) as f32).min(1.5)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::sheet_builder::design;
    use circuiter_sim::{Options, op};

    #[test]
    fn leds_glow_by_their_current_and_lamps_by_their_heat() {
        let (d, top) = design(&[
            ("V1", "std:vdc", &[("V", "5")], &[("p", "vcc"), ("n", "GND")]),
            ("R1", "std:resistor", &[("R", "150")], &[("a", "vcc"), ("b", "a1")]),
            ("D1", "std:led", &[], &[("a", "a1"), ("k", "GND")]),
            ("R2", "std:resistor", &[("R", "10k")], &[("a", "vcc"), ("b", "a2")]),
            ("D2", "std:led", &[("color", "blue")], &[("a", "a2"), ("k", "GND")]),
            ("D3", "std:led", &[], &[("a", "GND"), ("k", "vcc")]),
            ("V2", "std:vdc", &[("V", "12")], &[("p", "v12"), ("n", "GND")]),
            ("L1", "std:lamp", &[], &[("a", "v12"), ("b", "GND")]),
        ]);
        let netlist = Netlist::build(&d, &top).unwrap();
        let c = Circuit::parse(&netlist.to_spice()).unwrap();
        let r = op(&c, &Options::default()).unwrap();
        let v = |k: usize| r.voltages[k];
        let lit: std::collections::BTreeMap<String, f32> = Emitter::all(&d, &top, &netlist, &c)
            .iter()
            .map(|e| (e.path.clone(), e.level(&v)))
            .collect();
        assert!((0.9..1.1).contains(&lit["D1"]), "20 mA: fully lit, {}", lit["D1"]);
        assert!((0.1..0.3).contains(&lit["D2"]), "a blue LED on 10 kΩ: dim, {}", lit["D2"]);
        assert_eq!(lit["D3"], 0.0, "reversed: dark");
        assert!(lit["L1"] > 0.9, "a lamp at its rated voltage: {}", lit["L1"]);
        let blue =
            Emitter::all(&d, &top, &netlist, &c).into_iter().find(|e| e.path == "D2").unwrap();
        assert_eq!(blue.color, color("blue"));
    }
}
