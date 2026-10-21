//! Footprints made from a package name and parameters, matching the parts' 3D bodies:
//! pads (pad 1 square on through-hole parts), silkscreen, courtyard and fabrication
//! outlines. Coordinates are millimetres in the footprint's frame, Y down as on the
//! sheet, with the origin at the part's centre.

use circuiter_model::board::{FootprintView, Graphic, Layer, Pad, PadKind, PadShape};
use circuiter_model::geom::{Length, Point};
use std::collections::BTreeMap;

/// Every footprint package, for menus and checks.
pub const PACKAGES: &[&str] = &[
    "0603",
    "0805",
    "1206",
    "sod123",
    "sot23",
    "soic",
    "qfp",
    "axial",
    "radial",
    "disc",
    "led",
    "to92",
    "to220",
    "sot223",
    "dip",
    "bridge",
    "hc49",
    "relay",
    "header",
    "terminal",
    "pot",
    "toggle_switch",
    "tactile",
    "transformer",
    "battery",
    "motor",
    "speaker",
    "lamp",
    "solar",
    "fuse",
];

/// The parameters `package` reads, with their defaults, for forms to offer.
pub fn package_params(package: &str) -> &'static [(&'static str, &'static str)] {
    match package {
        "axial" | "fuse" => &[
            ("pitch", "10.16"),
            ("length", "6.3"),
            ("diameter", "2.5"),
            ("drill", "0.8"),
            ("style", ""),
        ],
        "radial" => &[("pitch", "2.5"), ("diameter", "6.3")],
        "disc" => &[("pitch", "5.08"), ("diameter", "5"), ("thickness", "2.5")],
        "soic" | "dip" => &[("pins", "8")],
        "qfp" => &[("pins", "48"), ("pitch", "0.5"), ("body", "7")],
        "header" | "terminal" => &[("pins", "2")],
        _ => &[],
    }
}

/// A footprint ready to place: its pads and drawings, with ports joined to pads.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Footprint {
    pub pads: Vec<Pad>,
    pub graphics: Vec<Graphic>,
    /// Cell port → pad number.
    pub pins: BTreeMap<String, String>,
    /// Its name in KiCad's libraries (`Package_DIP:DIP-8_W7.62mm`) where one is the same,
    /// else `Circuiter:<package>`.
    pub name: String,
}

impl Footprint {
    /// The pads joined to `port` (a port may have several: a regulator's tab and pin).
    pub fn pads_of<'a>(&'a self, port: &'a str) -> impl Iterator<Item = &'a Pad> + 'a {
        let numbers: Vec<&str> =
            self.pins.get(port).map(|n| n.split(',').map(str::trim).collect()).unwrap_or_default();
        self.pads.iter().filter(move |p| numbers.contains(&p.number.as_str()))
    }
}

fn mm(v: f64) -> Length {
    Length::from_mm(v)
}

fn pt(x: f64, y: f64) -> Point {
    Point::mm(x, y)
}

const SILK: f64 = 0.12;
const CRT: f64 = 0.05;
const FAB: f64 = 0.1;

/// The round (pad 1 square) through-hole pad of an ordinary part.
fn tht(number: usize, x: f64, y: f64, size: f64, drill: f64) -> Pad {
    Pad {
        number: number.to_string(),
        at: pt(x, y),
        size: (mm(size), mm(size)),
        shape: if number == 1 { PadShape::Rect } else { PadShape::Circle },
        kind: PadKind::Through { drill: mm(drill) },
        rotation: 0.0,
    }
}

fn smd(number: usize, x: f64, y: f64, w: f64, h: f64) -> Pad {
    Pad {
        number: number.to_string(),
        at: pt(x, y),
        size: (mm(w), mm(h)),
        shape: PadShape::RoundRect,
        kind: PadKind::Smd,
        rotation: 0.0,
    }
}

fn rect(layer: Layer, x0: f64, y0: f64, x1: f64, y1: f64, width: f64) -> Graphic {
    Graphic::Poly {
        layer,
        points: vec![pt(x0, y0), pt(x1, y0), pt(x1, y1), pt(x0, y1)],
        width: mm(width),
        filled: false,
    }
}

fn line(layer: Layer, a: (f64, f64), b: (f64, f64), width: f64) -> Graphic {
    Graphic::Line { layer, a: pt(a.0, a.1), b: pt(b.0, b.1), width: mm(width) }
}

fn circle(layer: Layer, x: f64, y: f64, r: f64, width: f64) -> Graphic {
    Graphic::Circle { layer, center: pt(x, y), radius: mm(r), width: mm(width) }
}

fn reference(y: f64) -> Graphic {
    Graphic::Text {
        layer: Layer::FSilk,
        at: pt(0.0, y),
        text: "${REFERENCE}".into(),
        size: mm(1.0),
    }
}

/// A body outline on silk and fab, and a courtyard 0.25 mm around everything.
fn outlines(g: &mut Vec<Graphic>, pads: &[Pad], body: (f64, f64, f64, f64)) {
    let (x0, y0, x1, y1) = body;
    g.push(rect(Layer::FFab, x0, y0, x1, y1, FAB));
    g.push(rect(Layer::FSilk, x0 - 0.12, y0 - 0.12, x1 + 0.12, y1 + 0.12, SILK));
    let (mut lo, mut hi) = ((x0, y0), (x1, y1));
    for p in pads {
        let (w, h) = (p.size.0.mm() / 2.0, p.size.1.mm() / 2.0);
        lo = (lo.0.min(p.at.x.mm() - w), lo.1.min(p.at.y.mm() - h));
        hi = (hi.0.max(p.at.x.mm() + w), hi.1.max(p.at.y.mm() + h));
    }
    g.push(rect(Layer::FCrtYd, lo.0 - 0.25, lo.1 - 0.25, hi.0 + 0.25, hi.1 + 0.25, CRT));
    g.push(reference(lo.1 - 1.2));
}

/// The parameters a package reads.
pub type Params<'a> = &'a dyn Fn(&str) -> Option<String>;

fn num(p: Params<'_>, key: &str, default: f64) -> f64 {
    p(key).and_then(|t| circuiter_model::units::parse_quantity(&t, None).ok()).unwrap_or(default)
}

/// Pads and drawings for `package`.
pub fn generate(package: &str, p: Params<'_>) -> Result<(Vec<Pad>, Vec<Graphic>), String> {
    let mut pads = Vec::new();
    let mut g = Vec::new();
    match package {
        // Two leads `pitch` apart along X, a body between them.
        "axial" | "fuse" => {
            let pitch = num(p, "pitch", 10.16);
            let (len, dia) = (num(p, "length", 6.3), num(p, "diameter", 2.5));
            let drill = num(p, "drill", 0.8);
            pads.push(tht(1, -pitch / 2.0, 0.0, 1.6, drill));
            pads.push(tht(2, pitch / 2.0, 0.0, 1.6, drill));
            outlines(&mut g, &pads, (-len / 2.0, -dia / 2.0, len / 2.0, dia / 2.0));
            // Leads, on silk where they leave the body.
            for s in [-1.0, 1.0] {
                g.push(line(
                    Layer::FSilk,
                    (s * (len / 2.0 + 0.12), 0.0),
                    (s * (pitch / 2.0 - 1.0), 0.0),
                    SILK,
                ));
            }
            if p("style").as_deref() == Some("diode") {
                // The cathode band, at pad 1's end (KiCad numbers the cathode 1).
                let x = -(len / 2.0 - len * 0.18);
                g.push(line(Layer::FSilk, (x, -dia / 2.0), (x, dia / 2.0), SILK * 2.0));
            }
        }
        "radial" | "disc" => {
            let pitch = num(p, "pitch", if package == "disc" { 5.08 } else { 2.5 });
            let d = num(p, "diameter", if package == "disc" { 5.0 } else { 6.3 });
            pads.push(tht(1, -pitch / 2.0, 0.0, 1.6, 0.8));
            pads.push(tht(2, pitch / 2.0, 0.0, 1.6, 0.8));
            if package == "radial" {
                g.push(circle(Layer::FFab, 0.0, 0.0, d / 2.0, FAB));
                g.push(circle(Layer::FSilk, 0.0, 0.0, d / 2.0 + 0.12, SILK));
                g.push(Graphic::Text {
                    layer: Layer::FSilk,
                    at: pt(-d / 2.0 - 0.8, -d / 4.0),
                    text: "+".into(),
                    size: mm(1.0),
                });
                g.push(circle(Layer::FCrtYd, 0.0, 0.0, d / 2.0 + 0.4, CRT));
                g.push(reference(-d / 2.0 - 1.2));
            } else {
                let t = num(p, "thickness", 2.5);
                outlines(&mut g, &pads, (-d / 2.0, -t / 2.0, d / 2.0, t / 2.0));
            }
        }
        // A 5 mm LED: cathode (pad 1, square) and anode 2.54 apart.
        "led" => {
            pads.push(tht(1, -1.27, 0.0, 1.8, 0.9));
            pads.push(tht(2, 1.27, 0.0, 1.8, 0.9));
            g.push(circle(Layer::FFab, 0.0, 0.0, 2.5, FAB));
            g.push(Graphic::Poly {
                layer: Layer::FSilk,
                points: (0..=24)
                    .map(|k| {
                        let a = (-160.0 + 320.0 * k as f64 / 24.0f64).to_radians();
                        pt(2.9 * a.cos(), 2.9 * a.sin())
                    })
                    .collect(),
                width: mm(SILK),
                filled: false,
            });
            // The flat marks the cathode, pad 1.
            g.push(line(Layer::FSilk, (-2.72, -1.0), (-2.72, 1.0), SILK));
            g.push(circle(Layer::FCrtYd, 0.0, 0.0, 3.25, CRT));
            g.push(reference(-4.0));
        }
        "to92" => {
            for k in 0..3 {
                pads.push(Pad {
                    shape: if k == 0 { PadShape::Rect } else { PadShape::Oval },
                    ..tht(k + 1, -1.27 + 1.27 * k as f64, 0.0, 1.05, 0.75)
                });
                pads[k].size = (mm(1.05), mm(1.5));
            }
            g.push(line(Layer::FSilk, (-2.0, 1.6), (2.0, 1.6), SILK));
            g.push(Graphic::Poly {
                layer: Layer::FSilk,
                points: (0..=20)
                    .map(|k| {
                        // Round the back of the body (−Y), the flat face along +Y.
                        let a = (200.0 + 140.0 * k as f64 / 20.0f64).to_radians();
                        pt(2.55 * a.cos(), 2.55 * a.sin())
                    })
                    .collect(),
                width: mm(SILK),
                filled: false,
            });
            g.push(rect(Layer::FCrtYd, -2.7, -2.7, 2.7, 1.9, CRT));
            g.push(rect(Layer::FFab, -2.4, -2.4, 2.4, 1.6, FAB));
            g.push(reference(-3.6));
        }
        "to220" => {
            for k in 0..3 {
                let mut pad = tht(k + 1, -2.54 + 2.54 * k as f64, 0.0, 1.9, 1.1);
                pad.size = (mm(1.9), mm(2.4));
                pad.shape = if k == 0 { PadShape::Rect } else { PadShape::Oval };
                pads.push(pad);
            }
            outlines(&mut g, &pads, (-5.0, -3.4, 5.0, 1.3));
            g.push(line(Layer::FSilk, (-5.12, -1.9), (5.12, -1.9), SILK));
        }
        // Surface-mount chips (resistors, capacitors, LEDs): two pads along X, pad 1
        // on the left; IPC nominal land patterns.
        "0603" | "0805" | "1206" => {
            let (x, w, h, body) = match package {
                "0603" => (0.775, 0.8, 0.95, (0.8, 0.4)),
                "0805" => (0.9125, 1.025, 1.4, (1.0, 0.625)),
                _ => (1.4875, 1.125, 1.75, (1.6, 0.8)),
            };
            pads.push(smd(1, -x, 0.0, w, h));
            pads.push(smd(2, x, 0.0, w, h));
            g.push(rect(Layer::FFab, -body.0, -body.1, body.0, body.1, FAB));
            // Silk only above and below the body, clear of the pads.
            let y = h / 2.0 + 0.2;
            g.push(line(Layer::FSilk, (-x + w / 2.0 - 0.1, -y), (x - w / 2.0 + 0.1, -y), SILK));
            g.push(line(Layer::FSilk, (-x + w / 2.0 - 0.1, y), (x - w / 2.0 + 0.1, y), SILK));
            let (cx, cy) = (x + w / 2.0 + 0.25, h / 2.0 + 0.25);
            g.push(rect(Layer::FCrtYd, -cx, -cy, cx, cy, CRT));
            g.push(reference(-cy - 0.8));
        }
        // SOD-123: a diode, pad 1 (the cathode) on the left.
        "sod123" => {
            pads.push(smd(1, -1.635, 0.0, 0.91, 1.22));
            pads.push(smd(2, 1.635, 0.0, 0.91, 1.22));
            g.push(rect(Layer::FFab, -1.35, -0.8, 1.35, 0.8, FAB));
            g.push(line(Layer::FSilk, (-2.3, -0.95), (1.2, -0.95), SILK));
            g.push(line(Layer::FSilk, (-2.3, 0.95), (1.2, 0.95), SILK));
            g.push(line(Layer::FSilk, (-2.3, -0.95), (-2.3, 0.95), SILK));
            g.push(rect(Layer::FCrtYd, -2.35, -1.15, 2.35, 1.15, CRT));
            g.push(reference(-1.9));
        }
        // SOT-23: pins 1 and 2 on the left, 3 on the right.
        "sot23" => {
            for (k, (x, y)) in
                [(-1.1375, -0.95), (-1.1375, 0.95), (1.1375, 0.0)].into_iter().enumerate()
            {
                pads.push(smd(k + 1, x, y, 1.325, 0.6));
            }
            g.push(rect(Layer::FFab, -0.65, -1.45, 0.65, 1.45, FAB));
            g.push(line(Layer::FSilk, (-0.76, -1.56), (0.76, -1.56), SILK));
            g.push(line(Layer::FSilk, (-0.76, 1.56), (0.76, 1.56), SILK));
            g.push(line(Layer::FSilk, (0.76, -1.56), (0.76, -0.65), SILK));
            g.push(line(Layer::FSilk, (0.76, 1.56), (0.76, 0.65), SILK));
            g.push(rect(Layer::FCrtYd, -2.05, -1.7, 2.05, 1.7, CRT));
            g.push(reference(-2.5));
        }
        // SOIC with `pins` pins, 1.27 mm pitch, 3.9 mm body: numbered as a DIP is.
        "soic" => {
            let n = num(p, "pins", 8.0).round() as usize;
            if !(4..=32).contains(&n) || n % 2 != 0 {
                return Err(format!("a SOIC needs an even number of pins, 4 to 32, not {n}"));
            }
            let per = n / 2;
            let span = (per - 1) as f64 * 1.27;
            for k in 0..per {
                pads.push(smd(k + 1, -2.475, -span / 2.0 + 1.27 * k as f64, 1.95, 0.6));
            }
            for k in 0..per {
                pads.push(smd(per + k + 1, 2.475, span / 2.0 - 1.27 * k as f64, 1.95, 0.6));
            }
            let half = span / 2.0 + 0.6;
            g.push(rect(Layer::FFab, -1.95, -half, 1.95, half, FAB));
            g.push(line(Layer::FSilk, (-1.95, -half - 0.1), (1.95, -half - 0.1), SILK));
            g.push(line(Layer::FSilk, (-1.95, half + 0.1), (1.95, half + 0.1), SILK));
            // Pin 1's mark.
            g.push(line(Layer::FSilk, (-3.45, -half - 0.1), (-1.95, -half - 0.1), SILK));
            g.push(rect(Layer::FCrtYd, -3.7, -half - 0.35, 3.7, half + 0.35, CRT));
            g.push(reference(-half - 1.2));
        }
        // A quad flat pack (LQFP, TQFP): `pins` gull-wing leads, a quarter on each
        // side of a square body `body` wide, `pitch` apart, reaching 1 mm beyond it.
        // Pin 1 is at the top of the left side and they count anticlockwise, as on
        // the package's top.
        "qfp" => {
            let n = num(p, "pins", 48.0).round() as usize;
            if !(8..=256).contains(&n) || n % 4 != 0 {
                return Err(format!("a QFP needs a multiple of 4 pins, 8 to 256, not {n}"));
            }
            let pitch = num(p, "pitch", 0.5);
            if !(0.3..=10.0).contains(&pitch) {
                return Err(format!("a QFP's pitch must be 0.3 to 10 mm, not {pitch}"));
            }
            let per = n / 4;
            let row = (per - 1) as f64 * pitch;
            // The body is at least as long as its rows of leads.
            let body = num(p, "body", 7.0).max(row + 2.0 * pitch);
            // IPC nominal lands: 1.475 mm long, 0.6 of the pitch wide, centred on the
            // feet of leads reaching 1 mm out from the body.
            let (len, w) = (1.475, (pitch * 0.6).min(0.6));
            let x = body / 2.0 + 1.0 - 0.3375;
            for k in 0..per {
                let along = -row / 2.0 + pitch * k as f64;
                pads.push(smd(k + 1, -x, along, len, w));
                pads.push(smd(per + k + 1, along, x, w, len));
                pads.push(smd(2 * per + k + 1, x, -along, len, w));
                pads.push(smd(3 * per + k + 1, -along, -x, w, len));
            }
            pads.sort_by_key(|pad| pad.number.parse::<usize>().unwrap_or(0));
            let h = body / 2.0;
            // The body on fab, its pin 1 corner cut.
            let c = 1.0f64.min(h / 2.0);
            g.push(Graphic::Poly {
                layer: Layer::FFab,
                points: vec![pt(-h + c, -h), pt(h, -h), pt(h, h), pt(-h, h), pt(-h, -h + c)],
                width: mm(FAB),
                filled: false,
            });
            // Silk at the corners, clear of the pads, and a tick out along pin 1.
            let (s, end) = (h + 0.11, row / 2.0 + w / 2.0 + 0.32);
            if end < s {
                for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                    g.push(line(Layer::FSilk, (sx * s, sy * s), (sx * end, sy * s), SILK));
                    g.push(line(Layer::FSilk, (sx * s, sy * s), (sx * s, sy * end), SILK));
                }
            }
            g.push(line(Layer::FSilk, (-s, -end), (-x - len / 2.0, -end), SILK));
            let o = x + len / 2.0 + 0.25;
            g.push(rect(Layer::FCrtYd, -o, -o, o, o, CRT));
            g.push(reference(-o - 0.8));
        }
        "sot223" => {
            for k in 0..3 {
                pads.push(smd(k + 1, -2.3 + 2.3 * k as f64, 3.15, 0.95, 2.0));
            }
            pads.push(smd(4, 0.0, -3.15, 3.25, 2.0));
            outlines(&mut g, &pads, (-3.25, -1.75, 3.25, 1.75));
        }
        // A DIP with `pins` pins, pin 1 at the top left, numbered anticlockwise.
        "dip" => {
            let n = num(p, "pins", 8.0).round() as usize;
            if !(4..=64).contains(&n) || n % 2 != 0 {
                return Err(format!("a DIP needs an even number of pins, 4 to 64, not {n}"));
            }
            let per = n / 2;
            let span = (per - 1) as f64 * 2.54;
            for k in 0..per {
                let mut pad = tht(k + 1, -3.81, -span / 2.0 + 2.54 * k as f64, 1.6, 0.8);
                pad.size = (mm(2.4), mm(1.6));
                pad.shape = if k == 0 { PadShape::Rect } else { PadShape::Oval };
                pads.push(pad);
            }
            for k in 0..per {
                let mut pad = tht(per + k + 1, 3.81, span / 2.0 - 2.54 * k as f64, 1.6, 0.8);
                pad.size = (mm(2.4), mm(1.6));
                pad.shape = PadShape::Oval;
                pads.push(pad);
            }
            let half = span / 2.0 + 1.27;
            g.push(rect(Layer::FFab, -2.54, -half, 2.54, half, FAB));
            g.push(Graphic::Poly {
                layer: Layer::FSilk,
                points: vec![
                    pt(-1.0, -half - 0.1),
                    pt(-2.66, -half - 0.1),
                    pt(-2.66, half + 0.1),
                    pt(2.66, half + 0.1),
                    pt(2.66, -half - 0.1),
                    pt(1.0, -half - 0.1),
                ],
                width: mm(SILK),
                filled: false,
            });
            // The notch at pin 1's end.
            g.push(Graphic::Poly {
                layer: Layer::FSilk,
                points: (0..=12)
                    .map(|k| {
                        let a = (180.0 * k as f64 / 12.0f64).to_radians();
                        pt(-a.cos(), -half - 0.1 + a.sin())
                    })
                    .collect(),
                width: mm(SILK),
                filled: false,
            });
            g.push(rect(Layer::FCrtYd, -5.3, -half - 0.3, 5.3, half + 0.3, CRT));
            g.push(reference(-half - 1.3));
        }
        "bridge" => {
            for (k, (x, y)) in
                [(-2.54, -2.54), (2.54, -2.54), (-2.54, 2.54), (2.54, 2.54)].into_iter().enumerate()
            {
                pads.push(tht(k + 1, x, y, 1.8, 1.0));
            }
            outlines(&mut g, &pads, (-5.0, -5.0, 5.0, 5.0));
        }
        "hc49" => {
            pads.push(tht(1, -2.44, 0.0, 1.5, 0.8));
            pads.push(tht(2, 2.44, 0.0, 1.5, 0.8));
            outlines(&mut g, &pads, (-5.5, -2.35, 5.5, 2.35));
        }
        "relay" => {
            for (k, (x, y)) in [(-8.0, -6.0), (-8.0, 6.0), (6.0, 0.0), (8.0, -6.0), (8.0, 6.0)]
                .into_iter()
                .enumerate()
            {
                pads.push(tht(k + 1, x, y, 2.5, 1.3));
            }
            outlines(&mut g, &pads, (-9.5, -7.75, 9.5, 7.75));
        }
        // A 1 × N pin header (2.54 mm), or a screw terminal (5.08 mm).
        "header" | "terminal" => {
            let n = num(p, "pins", 2.0).round().max(1.0) as usize;
            let (pitch, size, drill) =
                if package == "header" { (2.54, 1.7, 1.0) } else { (5.08, 2.6, 1.3) };
            let x0 = -((n - 1) as f64) * pitch / 2.0;
            for k in 0..n {
                pads.push(tht(k + 1, x0 + pitch * k as f64, 0.0, size, drill));
            }
            let h = if package == "header" { 1.27 } else { 4.0 };
            outlines(&mut g, &pads, (x0 - pitch / 2.0, -h, -x0 + pitch / 2.0, h));
        }
        // A trimmer's pins along its front; a miniature toggle's 4.7 mm apart.
        "pot" | "toggle_switch" => {
            let (pitch, y) = if package == "pot" { (2.54, -6.0) } else { (4.7, 0.0) };
            for k in 0..3 {
                pads.push(tht(k + 1, pitch * (k as f64 - 1.0), y, 1.8, 1.0));
            }
            let body =
                if package == "pot" { (-5.0, -5.0, 5.0, 5.0) } else { (-6.5, -3.5, 6.5, 3.5) };
            outlines(&mut g, &pads, body);
        }
        "tactile" => {
            for (k, (x, y)) in
                [(-3.25, -2.25), (3.25, -2.25), (-3.25, 2.25), (3.25, 2.25)].into_iter().enumerate()
            {
                pads.push(tht(k + 1, x, y, 1.8, 1.0));
            }
            outlines(&mut g, &pads, (-3.0, -3.0, 3.0, 3.0));
        }
        "transformer" => {
            // Pins 6 to 8 hold the bobbin and connect nothing.
            let at = [
                (-7.5, -10.0),
                (-7.5, 10.0),
                (7.5, -10.0),
                (2.5, 10.0),
                (7.5, 10.0),
                (-2.5, -10.0),
                (-2.5, 10.0),
                (2.5, -10.0),
            ];
            for (k, (x, y)) in at.into_iter().enumerate() {
                pads.push(tht(k + 1, x, y, 2.5, 1.2));
            }
            outlines(&mut g, &pads, (-15.0, -12.0, 15.0, 12.0));
        }
        // Parts usually wired off the board: two solder pads for their leads.
        "battery" | "motor" | "speaker" | "lamp" | "solar" => {
            pads.push(tht(1, -2.54, 0.0, 2.0, 1.0));
            pads.push(tht(2, 2.54, 0.0, 2.0, 1.0));
            outlines(&mut g, &pads, (-4.0, -1.5, 4.0, 1.5));
        }
        other => return Err(format!("no footprint package called `{other}`")),
    }
    Ok((pads, g))
}

/// The KiCad library footprint with the same pads as `package` with these parameters,
/// where there is one; else `Circuiter:<package>_<pads>`.
pub fn kicad_name(package: &str, p: Params<'_>, pads: usize) -> String {
    let n = |k: &str, d: f64| num(p, k, d);
    match package {
        "axial" if p("style").as_deref() == Some("diode") => {
            format!("Diode_THT:D_DO-41_SOD81_P{:.2}mm_Horizontal", n("pitch", 10.16))
        }
        "axial" => format!(
            "Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P{:.2}mm_Horizontal",
            n("pitch", 10.16)
        ),
        "radial" => format!(
            "Capacitor_THT:CP_Radial_D{:.1}mm_P{:.2}mm",
            n("diameter", 6.3),
            n("pitch", 2.5)
        ),
        "disc" => format!(
            "Capacitor_THT:C_Disc_D{:.1}mm_W{:.1}mm_P{:.2}mm",
            n("diameter", 5.0),
            n("thickness", 2.5),
            n("pitch", 5.08)
        ),
        "led" => "LED_THT:LED_D5.0mm".into(),
        "to92" => "Package_TO_SOT_THT:TO-92_Inline".into(),
        "to220" => "Package_TO_SOT_THT:TO-220-3_Vertical".into(),
        "sot223" => "Package_TO_SOT_SMD:SOT-223-3_TabPin2".into(),
        "dip" => format!("Package_DIP:DIP-{pads}_W7.62mm"),
        "soic" => match pads {
            8 => "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm".into(),
            14 => "Package_SO:SOIC-14_3.9x8.7mm_P1.27mm".into(),
            16 => "Package_SO:SOIC-16_3.9x9.9mm_P1.27mm".into(),
            n => format!("Package_SO:SOIC-{n}_3.9mm_P1.27mm"),
        },
        "qfp" => {
            let (pitch, body) = (n("pitch", 0.5), n("body", 7.0));
            let kind = if pitch >= 0.8 { "TQFP" } else { "LQFP" };
            format!("Package_QFP:{kind}-{pads}_{body}x{body}mm_P{pitch}mm")
        }
        "0603" | "0805" | "1206" => {
            let metric = match package {
                "0603" => "1608",
                "0805" => "2012",
                _ => "3216",
            };
            match p("style").as_deref() {
                Some("capacitor") => format!("Capacitor_SMD:C_{package}_{metric}Metric"),
                Some("led") => format!("LED_SMD:LED_{package}_{metric}Metric"),
                _ => format!("Resistor_SMD:R_{package}_{metric}Metric"),
            }
        }
        "sod123" => "Diode_SMD:D_SOD-123".into(),
        "sot23" => "Package_TO_SOT_SMD:SOT-23".into(),
        "hc49" => "Crystal:Crystal_HC49-U_Vertical".into(),
        "header" => format!("Connector_PinHeader_2.54mm:PinHeader_1x{pads:02}_P2.54mm_Vertical"),
        "terminal" => format!("TerminalBlock:TerminalBlock_bornier-{pads}_P5.08mm"),
        "tactile" => "Button_Switch_THT:SW_PUSH_6mm".into(),
        "relay" => "Relay_THT:Relay_SPDT_SANYOU_SRD_Series_Form_C".into(),
        other => format!("Circuiter:{other}_{pads}"),
    }
}

/// The footprint of a cell with a footprint view, for an instance with these parameter
/// values: pads and drawings, and its ports joined to pads.
pub fn resolve(
    view: &FootprintView,
    ports: &[String],
    param: &dyn Fn(&str) -> Option<String>,
) -> Result<Footprint, String> {
    let mut name = "Circuiter:custom".to_owned();
    // The package may be the part's choice: `${param.package}`.
    let package = circuiter_model::digital::expand(&view.package, param)
        .map_err(|e| format!("footprint package: {e}"))?;
    let (pads, graphics) = if package.is_empty() {
        (view.pads.clone(), view.graphics.clone())
    } else {
        let mut values = BTreeMap::new();
        for (k, v) in &view.params {
            let value = circuiter_model::digital::expand(v, param)
                .map_err(|e| format!("footprint parameter {k}: {e}"))?;
            values.insert(k.clone(), value);
        }
        let (pads, mut graphics) = generate(&package, &|k| values.get(k).cloned())?;
        name = kicad_name(&package, &|k| values.get(k).cloned(), pads.len());
        graphics.extend(view.graphics.iter().cloned());
        (pads, graphics)
    };
    let mut pins = BTreeMap::new();
    let map = view.package_pins.get(&package).unwrap_or(&view.pins);
    for (k, port) in ports.iter().enumerate() {
        let number = map.get(port).cloned().unwrap_or_else(|| (k + 1).to_string());
        pins.insert(port.clone(), number);
    }
    Ok(Footprint { pads, graphics, pins, name })
}

// ---- Editing footprints pad by pad ----------------------------------------------------

/// How a pad array numbers its pads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Numbering {
    /// Along each row, then the next row: 1 2 3 / 4 5 6.
    Rows,
    /// Down each column, then the next: 1 3 5 / 2 4 6, as a double-row header.
    Columns,
    /// A letter for the row and a number for the column: A1 A2 / B1 B2, as a BGA.
    Grid,
}

/// JEDEC's row letters for ball grids: I, O, Q, S, X and Z are left out.
const ROW_LETTERS: &[u8] = b"ABCDEFGHJKLMNPRTUVWY";

/// The name of grid row `k` (from 0): A … Y, then AA, AB …
pub fn row_name(k: usize) -> String {
    let n = ROW_LETTERS.len();
    if k < n {
        (ROW_LETTERS[k] as char).to_string()
    } else {
        format!("{}{}", row_name(k / n - 1), ROW_LETTERS[k % n] as char)
    }
}

/// `rows` × `cols` copies of `pad`, `pitch` apart (across, down), the first where
/// `pad` is, numbered from `first` (a number; the grid's columns always count from 1).
pub fn pad_array(
    pad: &Pad,
    rows: usize,
    cols: usize,
    pitch: (f64, f64),
    first: usize,
    numbering: Numbering,
) -> Vec<Pad> {
    let (x0, y0) = (pad.at.x.mm(), pad.at.y.mm());
    let mut out = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            let number = match numbering {
                Numbering::Rows => (first + r * cols + c).to_string(),
                Numbering::Columns => (first + c * rows + r).to_string(),
                Numbering::Grid => format!("{}{}", row_name(r), c + 1),
            };
            out.push(Pad {
                number,
                at: pt(x0 + c as f64 * pitch.0, y0 + r as f64 * pitch.1),
                ..pad.clone()
            });
        }
    }
    out
}

/// The number after the highest plain number among `pads`: what a new pad gets.
pub fn next_number(pads: &[Pad]) -> String {
    let top = pads.iter().filter_map(|p| p.number.parse::<usize>().ok()).max().unwrap_or(0);
    (top + 1).to_string()
}

/// A pad's corners, turned as it is.
pub fn pad_corners(p: &Pad) -> [(f64, f64); 4] {
    let (w, h) = (p.size.0.mm() / 2.0, p.size.1.mm() / 2.0);
    let (s, c) = p.rotation.to_radians().sin_cos();
    [(-w, -h), (w, -h), (w, h), (-w, h)].map(|(x, y)| {
        // Counter-clockwise as seen, with Y down.
        (p.at.x.mm() + x * c + y * s, p.at.y.mm() - x * s + y * c)
    })
}

/// A courtyard round the pads and the body drawn on the fab and silkscreen layers,
/// `margin` clear of them (IPC-7351's 0.25 mm for most parts).
pub fn courtyard(pads: &[Pad], graphics: &[Graphic], margin: f64) -> Option<Graphic> {
    let mut lo = [f64::MAX; 2];
    let mut hi = [f64::MIN; 2];
    let mut grow = |x: f64, y: f64| {
        lo = [lo[0].min(x), lo[1].min(y)];
        hi = [hi[0].max(x), hi[1].max(y)];
    };
    for p in pads {
        pad_corners(p).iter().for_each(|&(x, y)| grow(x, y));
    }
    for g in graphics {
        match g {
            Graphic::Line { layer: Layer::FSilk | Layer::FFab, a, b, .. } => {
                grow(a.x.mm(), a.y.mm());
                grow(b.x.mm(), b.y.mm());
            }
            Graphic::Poly { layer: Layer::FSilk | Layer::FFab, points, .. } => {
                points.iter().for_each(|q| grow(q.x.mm(), q.y.mm()));
            }
            Graphic::Circle { layer: Layer::FSilk | Layer::FFab, center, radius, .. } => {
                let (x, y, r) = (center.x.mm(), center.y.mm(), radius.mm());
                grow(x - r, y - r);
                grow(x + r, y + r);
            }
            _ => {}
        }
    }
    (lo[0] <= hi[0]).then(|| {
        // On a 0.01 mm grid, as KiCad keeps courtyards.
        let snap = |v: f64, up: bool| {
            let k = v * 100.0;
            (if up { k.ceil() } else { k.floor() }) / 100.0
        };
        rect(
            Layer::FCrtYd,
            snap(lo[0] - margin, false),
            snap(lo[1] - margin, false),
            snap(hi[0] + margin, true),
            snap(hi[1] + margin, true),
            CRT,
        )
    })
}

/// `view` as pads and drawings to edit one by one: a package's are generated (with
/// the part's parameter values) and the package let go; the ports keep their pads.
pub fn editable(
    view: &FootprintView,
    ports: &[String],
    param: &dyn Fn(&str) -> Option<String>,
) -> Result<FootprintView, String> {
    if view.package.is_empty() {
        return Ok(view.clone());
    }
    let fp = resolve(view, ports, param)?;
    Ok(FootprintView { pads: fp.pads, graphics: fp.graphics, pins: fp.pins, ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(package: &str, params: &[(&str, &str)]) -> (Vec<Pad>, Vec<Graphic>) {
        let p: BTreeMap<String, String> =
            params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        generate(package, &|k| p.get(k).cloned()).unwrap()
    }

    #[test]
    fn every_package_has_numbered_pads_and_a_courtyard() {
        for name in PACKAGES {
            let (pads, g) = make(name, &[]);
            assert!(!pads.is_empty(), "{name}");
            let numbers: std::collections::BTreeSet<&str> =
                pads.iter().map(|p| p.number.as_str()).collect();
            assert_eq!(numbers.len(), pads.len(), "{name}: pad numbers are unique");
            assert!(numbers.contains("1"), "{name}");
            assert!(
                g.iter().any(|x| matches!(
                    x,
                    Graphic::Poly { layer: Layer::FCrtYd, .. }
                        | Graphic::Circle { layer: Layer::FCrtYd, .. }
                )),
                "{name} has a courtyard"
            );
        }
        assert!(generate("dip", &|k| (k == "pins").then(|| "7".into())).is_err());
    }

    #[test]
    fn arrays_number_by_rows_columns_or_grid() {
        let pad = smd(1, -1.0, -2.0, 0.5, 0.5);
        let rows = pad_array(&pad, 2, 3, (1.27, 2.54), 1, Numbering::Rows);
        let names: Vec<&str> = rows.iter().map(|p| p.number.as_str()).collect();
        assert_eq!(names, ["1", "2", "3", "4", "5", "6"]);
        assert_eq!(rows[5].at, pt(-1.0 + 2.0 * 1.27, -2.0 + 2.54));
        let cols = pad_array(&pad, 2, 3, (2.54, 2.54), 1, Numbering::Columns);
        let at = |n: &str| cols.iter().find(|p| p.number == n).unwrap().at;
        assert_eq!(at("2"), pt(-1.0, 0.54), "a double-row header: 2 under 1");
        assert_eq!(at("3"), pt(1.54, -2.0));
        let grid = pad_array(&pad, 22, 2, (1.0, 1.0), 1, Numbering::Grid);
        let names: Vec<&str> = grid.iter().map(|p| p.number.as_str()).collect();
        assert_eq!(&names[..4], ["A1", "A2", "B1", "B2"]);
        assert_eq!(names[16], "J1", "no I");
        assert_eq!(names[40], "AA1", "after Y");
        assert_eq!(next_number(&rows), "7");
        assert_eq!(next_number(&grid), "1");
    }

    #[test]
    fn a_courtyard_clears_turned_pads_and_packages_become_pads() {
        let mut pad = smd(1, 2.0, 0.0, 2.0, 1.0);
        pad.rotation = 90.0;
        let Some(Graphic::Poly { points, layer, .. }) = courtyard(&[pad], &[], 0.25) else {
            panic!("a courtyard")
        };
        assert_eq!(layer, Layer::FCrtYd);
        // Turned, the pad is 1 wide and 2 tall.
        assert_eq!(points[0], pt(1.25, -1.25));
        assert_eq!(points[2], pt(2.75, 1.25));
        let view = FootprintView {
            package: "dip".into(),
            pins: [("vcc".to_owned(), "8".to_owned())].into(),
            ..FootprintView::default()
        };
        let ports = ["gnd".to_owned(), "vcc".to_owned()];
        let pads = editable(&view, &ports, &|_| None).unwrap();
        assert!(pads.package.is_empty() && pads.pads.len() == 8);
        let a = resolve(&view, &ports, &|_| None).unwrap();
        let b = resolve(&pads, &ports, &|_| None).unwrap();
        assert_eq!((a.pads, a.pins), (b.pads, b.pins), "the same footprint, now editable");
    }

    #[test]
    fn the_parameters_offered_are_the_ones_read() {
        // Each one offered changes the footprint when changed (style only marks diodes).
        for name in PACKAGES {
            for (key, default) in package_params(name) {
                let other = match *key {
                    "style" => "diode",
                    "pins" => "16",
                    _ => "7.62",
                };
                assert_ne!(*default, other);
                assert_ne!(make(name, &[]), make(name, &[(key, other)]), "{name} reads {key}");
            }
        }
    }

    #[test]
    fn a_dip_counts_anticlockwise_on_a_300_mil_body() {
        let (pads, _) = make("dip", &[("pins", "8")]);
        let at = |n: &str| pads.iter().find(|p| p.number == n).unwrap().at;
        assert_eq!(at("1"), Point::mm(-3.81, -3.81));
        assert_eq!(at("4"), Point::mm(-3.81, 3.81));
        assert_eq!(at("5"), Point::mm(3.81, 3.81));
        assert_eq!(at("8"), Point::mm(3.81, -3.81));
        assert_eq!(pads[0].shape, PadShape::Rect, "pin 1 is square");
    }

    #[test]
    fn a_qfp_counts_anticlockwise_from_the_top_of_its_left_side() {
        // KiCad's LQFP-48_7x7mm_P0.5mm: lands 1.475 × 0.3 centred 4.1625 out.
        let (pads, g) = make("qfp", &[]);
        assert_eq!(pads.len(), 48);
        let at = |n: &str| {
            let p = pads.iter().find(|p| p.number == n).unwrap();
            ((p.at.x.mm() * 1e4).round() / 1e4, (p.at.y.mm() * 1e4).round() / 1e4)
        };
        assert_eq!(at("1"), (-4.1625, -2.75));
        assert_eq!(at("12"), (-4.1625, 2.75));
        assert_eq!(at("13"), (-2.75, 4.1625), "then along the bottom");
        assert_eq!(at("25"), (4.1625, 2.75), "up the right");
        assert_eq!(at("48"), (-2.75, -4.1625), "and back along the top");
        let p1 = &pads[0];
        assert_eq!((p1.size.0.mm(), p1.size.1.mm()), (1.475, 0.3));
        assert_eq!((pads[12].size.0.mm(), pads[12].size.1.mm()), (0.3, 1.475));
        // Silk keeps off the lands.
        for x in &g {
            let Graphic::Line { layer: Layer::FSilk, a, b, .. } = x else { continue };
            for p in &pads {
                let (w, h) = (p.size.0.mm() / 2.0 + 0.1, p.size.1.mm() / 2.0 + 0.1);
                let inside = |q: &Point| {
                    (q.x.mm() - p.at.x.mm()).abs() < w && (q.y.mm() - p.at.y.mm()).abs() < h
                };
                let mid = Point::mm((a.x.mm() + b.x.mm()) / 2.0, (a.y.mm() + b.y.mm()) / 2.0);
                assert!(!inside(a) && !inside(b) && !inside(&mid), "silk over pad {}", p.number);
            }
        }
        let p = |k: &str| match k {
            "pins" => Some("64".to_owned()),
            "body" => Some("10".to_owned()),
            _ => None,
        };
        assert_eq!(kicad_name("qfp", &p, 64), "Package_QFP:LQFP-64_10x10mm_P0.5mm");
        assert!(generate("qfp", &|k| (k == "pins").then(|| "50".into())).is_err());
    }

    #[test]
    fn ports_map_to_pads_in_order_or_as_listed() {
        let view = FootprintView {
            package: "axial".into(),
            params: [("pitch".to_owned(), "${param.pitch}".to_owned())].into(),
            pins: [("k".to_owned(), "2".to_owned())].into(),
            ..FootprintView::default()
        };
        let fp =
            resolve(&view, &["a".into(), "k".into()], &|n| (n == "pitch").then(|| "7.62".into()))
                .unwrap();
        assert_eq!(fp.pins["a"], "1");
        assert_eq!(fp.pins["k"], "2");
        assert_eq!(fp.pads_of("k").next().unwrap().at, Point::mm(3.81, 0.0));
    }
}
