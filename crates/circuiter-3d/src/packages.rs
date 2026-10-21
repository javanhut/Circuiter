//! The physical bodies of parts, generated from a package name and parameters: an
//! axial resistor with its value's colour bands, a DIP with any number of pins, a
//! TO-220 with its tab, a toggle switch with its lever where the switch is set…
//!
//! Each body is centred on the origin in X and Y with the board at z = 0. Two-lead
//! parts lie along X, as their schematic symbols do. Dimensions are typical datasheet
//! ones, in millimetres.

use crate::math::{Mat4, V3};
use crate::{Mesh, Rgba, color, cuboid, cylinder, extrude, lathe, rod, rounded_rect, sphere, wire};
use std::f32::consts::{FRAC_PI_2, PI};

/// Every package, for menus and checks.
pub const PACKAGES: &[&str] = &[
    "0603",
    "0805",
    "1206",
    "sod123",
    "sot23",
    "soic",
    "qfp",
    "axial",
    "disc",
    "led",
    "radial_can",
    "radial_inductor",
    "to92",
    "to220",
    "sot223",
    "dip",
    "bridge",
    "hc49",
    "fuse",
    "bead",
    "ldr",
    "relay",
    "motor",
    "speaker",
    "lamp",
    "battery",
    "solar",
    "pot",
    "toggle_switch",
    "tactile",
    "transformer",
    "header",
    "terminal",
];

/// The parameters a package reads, with their values.
pub type Params<'a> = &'a dyn Fn(&str) -> Option<String>;

fn num(p: Params<'_>, key: &str, default: f32) -> f32 {
    p(key)
        .and_then(|t| circuiter_model::units::parse_quantity(&t, None).ok())
        .map_or(default, |v| v as f32)
}

fn text(p: Params<'_>, key: &str, default: &str) -> String {
    p(key).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| default.to_owned())
}

fn flag(p: Params<'_>, key: &str) -> bool {
    p(key).is_some_and(|t| matches!(t.trim(), "true" | "1" | "yes"))
}

const LEAD: f32 = 0.3;
/// How far through-hole leads reach below the board's top surface.
const BELOW: f32 = -3.0;

fn tin() -> Rgba {
    color("tin")
}

/// Turns a body built along Z (from z = 0 to `len`) to lie along X, centred, at
/// height `z`.
fn along_x(m: Mesh, len: f32, z: f32) -> Mesh {
    m.translated([0.0, 0.0, -len / 2.0])
        .transformed(&Mat4::rotate_y(FRAC_PI_2))
        .translated([0.0, 0.0, z])
}

/// Straight leads down through the board at each `(x, y)`, from height `top`.
fn leads(at: &[(f32, f32)], top: f32) -> Mesh {
    let mut m = Mesh::default();
    for &(x, y) in at {
        m.append(rod([x, y, BELOW], [x, y, top], LEAD, 8, tin()));
    }
    m
}

/// Makes the body of `package` from its parameters.
pub fn generate(package: &str, p: Params<'_>) -> Result<Mesh, String> {
    Ok(match package {
        "axial" => axial(p),
        "disc" => disc(p),
        "led" => led(p),
        "radial_can" => radial_can(p),
        "radial_inductor" => radial_inductor(p),
        "to92" => to92(p),
        "to220" => to220(p),
        "sot223" => sot223(),
        "dip" => dip(p)?,
        "0603" | "0805" | "1206" => chip(package, p),
        "sod123" => sod123(),
        "sot23" => sot23(),
        "soic" => soic(p)?,
        "qfp" => qfp(p)?,
        "bridge" => bridge(),
        "hc49" => hc49(),
        "fuse" => fuse(p),
        "bead" => bead(p),
        "ldr" => ldr(),
        "relay" => relay(p),
        "motor" => motor(),
        "speaker" => speaker(),
        "lamp" => lamp(),
        "battery" => battery(p),
        "solar" => solar(p),
        "pot" => pot(p),
        "toggle_switch" => toggle_switch(p),
        "tactile" => tactile(p),
        "transformer" => transformer(p),
        "header" => header(p),
        "terminal" => terminal(p),
        other => return Err(format!("no 3D package called `{other}`")),
    })
}

// ---- Resistor colour code ------------------------------------------------------------

const DIGITS: [&str; 10] =
    ["black", "brown", "red", "orange", "yellow", "green", "blue", "violet", "grey", "white"];

/// The four bands of a ±5 % resistor: two digits, multiplier, tolerance (gold).
pub fn resistor_bands(ohms: f64) -> [&'static str; 4] {
    if !(ohms.is_finite() && ohms > 0.0) {
        return ["black", "black", "black", "gold"];
    }
    // Two significant digits, rounded.
    let mut exp = ohms.log10().floor() as i32 - 1;
    let mut sig = (ohms / 10f64.powi(exp)).round() as i64;
    if sig >= 100 {
        sig /= 10;
        exp += 1;
    }
    let mult = match exp {
        -2 => "silver",
        -1 => "gold",
        e if (0..=9).contains(&e) => DIGITS[e as usize],
        _ => "black",
    };
    [DIGITS[(sig / 10) as usize % 10], DIGITS[(sig % 10) as usize], mult, "gold"]
}

// ---- Two-lead parts ------------------------------------------------------------------

/// An axial part: `style` resistor (with colour bands from `R`), diode (a cathode band
/// at −X, over pad 1 as KiCad numbers it) or plain.
fn axial(p: Params<'_>) -> Mesh {
    let style = text(p, "style", "resistor");
    let l = num(p, "length", 6.3);
    let d = num(p, "diameter", 2.4);
    let pitch = num(p, "pitch", 10.16).max(l + 2.0);
    let body_color = color(&text(p, "color", if style == "resistor" { "beige" } else { "black" }));
    let r = d / 2.0;
    let zc = r + 1.0;
    let profile: Vec<(f32, f32)> = if style == "resistor" {
        vec![
            (0.0, 0.0),
            (r * 0.9, 0.0),
            (r, l * 0.06),
            (r, l * 0.26),
            (r * 0.84, l * 0.32),
            (r * 0.84, l * 0.68),
            (r, l * 0.74),
            (r, l * 0.94),
            (r * 0.9, l),
            (0.0, l),
        ]
    } else {
        vec![(0.0, 0.0), (r * 0.8, 0.0), (r, l * 0.08), (r, l * 0.92), (r * 0.8, l), (0.0, l)]
    };
    let mut body = lathe(&profile, 24, body_color);
    if style == "resistor" {
        let bands = resistor_bands(f64::from(num(p, "R", 1000.0)));
        let at = [(0.17, 1.02), (0.38, 0.86), (0.52, 0.86), (0.82, 1.02)];
        for (name, (pos, k)) in bands.iter().zip(at) {
            body.append(cylinder(r * k, l * pos, l * (pos + 0.07), 24, color(name)));
        }
    } else if style == "diode" {
        let band = color(&text(p, "band", "silver"));
        body.append(cylinder(r * 1.02, l * 0.1, l * 0.22, 24, band));
    }
    let mut m = along_x(body, l, zc);
    for s in [-1.0f32, 1.0] {
        m.append(wire(
            &[[s * l / 2.0, 0.0, zc], [s * pitch / 2.0, 0.0, zc], [s * pitch / 2.0, 0.0, BELOW]],
            LEAD,
            tin(),
        ));
    }
    m
}

/// A disc standing on two leads: a ceramic capacitor or varistor.
fn disc(p: Params<'_>) -> Mesh {
    let dia = num(p, "diameter", 5.0);
    let t = num(p, "thickness", 2.5);
    let pitch = num(p, "pitch", 5.08);
    let zc = 2.0 + dia / 2.0;
    let face = cylinder(dia / 2.0, -t / 2.0, t / 2.0, 32, color(&text(p, "color", "orange")));
    let mut m = face.transformed(&Mat4::rotate_x(FRAC_PI_2)).translated([0.0, 0.0, zc]);
    let inner = (dia * 0.3).min(pitch / 2.0);
    for s in [-1.0f32, 1.0] {
        m.append(wire(
            &[
                [s * inner, 0.0, zc - dia * 0.3],
                [s * inner, 0.0, 2.0],
                [s * pitch / 2.0, 0.0, 1.0],
                [s * pitch / 2.0, 0.0, BELOW],
            ],
            LEAD,
            tin(),
        ));
    }
    m
}

/// A 5 mm LED (or photodiode, phototransistor): a domed lens on a flange.
fn led(p: Params<'_>) -> Mesh {
    let d = num(p, "diameter", 5.0);
    let lens = color(&text(p, "color", "red"));
    let r = d / 2.0;
    let h = 8.6;
    let mut profile = vec![(0.0, 1.0), (r + 0.4, 1.0), (r + 0.4, 2.0), (r, 2.0), (r, h - r)];
    for k in 1..=8 {
        let a = FRAC_PI_2 * k as f32 / 8.0;
        profile.push((r * a.cos(), h - r + r * a.sin()));
    }
    let mut m = lathe(&profile, 32, lens);
    // The anode (+X) lead is the longer one; the cathode is on the left, as the
    // footprint's pad 1 is.
    m.append(leads(&[(1.27, 0.0)], 1.0));
    m.append(rod([-1.27, 0.0, BELOW + 1.0], [-1.27, 0.0, 1.0], LEAD, 8, tin()));
    m
}

/// An electrolytic capacitor: a sleeved can with a stripe on its negative side.
fn radial_can(p: Params<'_>) -> Mesh {
    let d = num(p, "diameter", 6.3);
    let h = num(p, "height", 11.0);
    let pitch = num(p, "pitch", 2.5);
    let r = d / 2.0;
    let sleeve = color(&text(p, "color", "darkblue"));
    let profile = [
        (0.0, 0.4),
        (r - 0.3, 0.4),
        (r, 0.7),
        (r, 1.6),
        (r - 0.25, 1.9),
        (r, 2.2),
        (r, h - 0.3),
        (r - 0.3, h),
        (0.0, h),
    ];
    let mut m = lathe(&profile, 40, sleeve);
    m.append(cylinder(r - 0.35, h - 0.02, h + 0.03, 40, color("aluminium")));
    m.append(cuboid([r * 0.94, -r * 0.3, 2.4], [r + 0.03, r * 0.3, h - 0.5], color("lightblue")));
    m.append(leads(&[(-pitch / 2.0, 0.0), (pitch / 2.0, 0.0)], 0.5));
    m
}

/// A radial inductor: copper wound on a ferrite drum.
fn radial_inductor(p: Params<'_>) -> Mesh {
    let d = num(p, "diameter", 8.0);
    let r = d / 2.0;
    let ferrite = color("black");
    let mut m = cylinder(r, 0.5, 1.5, 32, ferrite);
    m.append(cylinder(r * 0.6, 1.5, 7.0, 24, ferrite));
    m.append(cylinder(r * 0.92, 1.6, 6.9, 32, color("copper")));
    m.append(cylinder(r, 7.0, 8.0, 32, ferrite));
    m.append(leads(&[(-2.5, 0.0), (2.5, 0.0)], 0.5));
    m
}

/// A glass cartridge fuse in clips' place: a tube with metal caps and its wire.
fn fuse(p: Params<'_>) -> Mesh {
    let l = num(p, "length", 20.0);
    let d = num(p, "diameter", 5.0);
    let r = d / 2.0;
    let mut body = cylinder(r * 0.92, 4.0, l - 4.0, 24, color("clear"));
    body.append(cylinder(r, 0.0, 4.0, 24, color("silver")));
    body.append(cylinder(r, l - 4.0, l, 24, color("silver")));
    body.append(cylinder(0.08, 3.9, l - 3.9, 6, color("grey")));
    let zc = r + 1.0;
    let mut m = along_x(body, l, zc);
    for s in [-1.0f32, 1.0] {
        m.append(wire(
            &[
                [s * l / 2.0, 0.0, zc],
                [s * (l / 2.0 + 1.3), 0.0, zc],
                [s * (l / 2.0 + 1.3), 0.0, BELOW],
            ],
            LEAD,
            tin(),
        ));
    }
    m
}

/// An epoxy bead (thermistor) on two converging leads.
fn bead(p: Params<'_>) -> Mesh {
    let d = num(p, "diameter", 3.0);
    let z = 6.0;
    let mut m = sphere([0.0, 0.0, z], d / 2.0, 20, color(&text(p, "color", "black")));
    for s in [-1.0f32, 1.0] {
        m.append(wire(
            &[[s * 0.5, 0.0, z], [s * 1.27, 0.0, 2.0], [s * 1.27, 0.0, BELOW]],
            LEAD * 0.7,
            tin(),
        ));
    }
    m
}

/// A photoresistor: a ceramic disc with its orange cadmium-sulphide track on top.
fn ldr() -> Mesh {
    let mut m = cylinder(2.6, 1.5, 3.5, 32, color("white"));
    let track = color("orange");
    for k in 0..5 {
        let y = -1.6 + 0.8 * k as f32;
        m.append(cuboid([-1.6, y - 0.12, 3.5], [1.6, y + 0.12, 3.56], track));
    }
    m.append(leads(&[(-1.7, 0.0), (1.7, 0.0)], 1.5));
    m
}

fn hc49() -> Mesh {
    let mut m = extrude(&rounded_rect(11.0, 4.7, 2.3, 8), 0.5, 4.0, color("silver"));
    m.append(leads(&[(-2.44, 0.0), (2.44, 0.0)], 0.5));
    m
}

// ---- Semiconductors ------------------------------------------------------------------

/// TO-92: a black (or clear) half-round body on three leads.
fn to92(p: Params<'_>) -> Mesh {
    let body = color(&text(p, "color", "black"));
    let r = 2.4;
    // The flat face points towards −Y (the viewer of a schematic): an arc round the
    // back from the right end of the flat to its left end.
    let a0 = (-1.3f32 / r).asin();
    let a1 = PI - a0;
    let outline: Vec<(f32, f32)> = (0..=24)
        .map(|k| {
            let a = a0 + (a1 - a0) * k as f32 / 24.0;
            (r * a.cos(), r * a.sin())
        })
        .collect();
    let mut m = extrude(&outline, 2.5, 7.0, body);
    m.append(leads(&[(-1.27, 0.0), (0.0, 0.0), (1.27, 0.0)], 2.5));
    m
}

/// TO-220: a black body, its metal tab with the mounting hole, three leads.
fn to220(p: Params<'_>) -> Mesh {
    let _ = p;
    let mut m = cuboid([-5.0, -2.25, 3.0], [5.0, 2.25, 12.2], color("black"));
    m.append(cuboid([-5.0, 0.95, 12.2], [5.0, 2.25, 18.7], color("silver")));
    let hole = cylinder(1.8, 0.9, 2.3, 24, color("black")).transformed(&Mat4::rotate_x(-FRAC_PI_2));
    m.append(hole.transformed(&Mat4::rotate_z(PI)).translated([0.0, 2.2, 15.6]));
    for x in [-2.54f32, 0.0, 2.54] {
        m.append(cuboid([x - 0.7, -0.25, 0.8], [x + 0.7, 0.25, 3.0], tin()));
        m.append(cuboid([x - 0.4, -0.25, BELOW], [x + 0.4, 0.25, 0.8], tin()));
    }
    m
}

/// SOT-223, surface mount: body, three gull-wing leads and the tab.
/// A surface-mount chip lying along X: a resistor (black, white-printed top), a
/// capacitor (tan) or an LED (its colour), with tinned ends over the pads.
fn chip(package: &str, p: Params<'_>) -> Mesh {
    let (l, w, h) = match package {
        "0603" => (1.6, 0.8, 0.45),
        "0805" => (2.0, 1.25, 0.5),
        _ => (3.2, 1.6, 0.55),
    };
    let end = l * 0.18;
    let style = text(p, "style", "resistor");
    let body = match style.as_str() {
        "capacitor" => [196, 160, 110, 255],
        "led" => color(&text(p, "color", "red")),
        _ => color("black"),
    };
    let mut m = cuboid([-l / 2.0 + end, -w / 2.0, 0.0], [l / 2.0 - end, w / 2.0, h], body);
    for s in [-1.0f32, 1.0] {
        let (a, b) = if s < 0.0 { (-l / 2.0, -l / 2.0 + end) } else { (l / 2.0 - end, l / 2.0) };
        m.append(cuboid([a, -w / 2.0, 0.0], [b, w / 2.0, h], tin()));
    }
    if style == "led" {
        // The cathode mark: a green dot at the −X end, over pad 1.
        m.append(cuboid(
            [-l / 2.0 + end, -w / 4.0, h],
            [-l / 2.0 + end * 1.6, w / 4.0, h + 0.01],
            color("green"),
        ));
    }
    m
}

/// SOD-123: a small moulded diode with a band at its cathode (−X) and bent leads.
fn sod123() -> Mesh {
    let mut m = cuboid([-1.35, -0.8, 0.05], [1.35, 0.8, 1.1], color("black"));
    m.append(cuboid([-1.1, -0.8, 1.1], [-0.7, 0.8, 1.11], color("grey")));
    m.append(cuboid([-1.9, -0.35, 0.0], [-1.35, 0.35, 0.12], tin()));
    m.append(cuboid([1.35, -0.35, 0.0], [1.9, 0.35, 0.12], tin()));
    m
}

/// A gull-wing lead from the body's edge at height `z` out to `x_out`, at `y`.
fn gull(x_edge: f32, x_out: f32, y: f32, half: f32, z: f32) -> Mesh {
    let (a, b) = (x_edge.min(x_out), x_edge.max(x_out));
    let mut m = cuboid([a, y - half, 0.0], [b, y + half, 0.12], tin());
    let foot = if x_out > x_edge {
        x_edge + (x_out - x_edge) * 0.35
    } else {
        x_edge - (x_edge - x_out) * 0.35
    };
    let (c, d) = (x_edge.min(foot), x_edge.max(foot));
    m.append(cuboid([c, y - half, 0.0], [d, y + half, z], tin()));
    m
}

/// SOT-23: a black body, pins 1 and 2 on the left (at +Y and −Y on the board), 3 on
/// the right.
fn sot23() -> Mesh {
    let mut m = cuboid([-0.65, -1.45, 0.1], [0.65, 1.45, 1.0], color("black"));
    for (x, y) in [(-1.2f32, 0.95f32), (-1.2, -0.95), (1.2, 0.0)] {
        let edge = if x < 0.0 { -0.65 } else { 0.65 };
        m.append(gull(edge, x, y, 0.2, 0.5));
    }
    m
}

/// SOIC with `pins` pins, 1.27 mm apart, numbered as the footprint has them: pin 1 at
/// the top left.
fn soic(p: Params<'_>) -> Result<Mesh, String> {
    let n = num(p, "pins", 8.0).round() as i32;
    if !(4..=32).contains(&n) || n % 2 != 0 {
        return Err(format!("a SOIC needs an even number of pins, 4 to 32, not {n}"));
    }
    let per = n / 2;
    let span = (per - 1) as f32 * 1.27;
    let half = span / 2.0 + 0.6;
    let mut m = cuboid([-1.95, -half, 0.1], [1.95, half, 1.55], color("black"));
    // Pin 1's dimple.
    m.append(cylinder(0.3, 1.55, 1.57, 16, color("grey")).translated([-1.2, half - 0.7, 0.0]));
    for k in 0..per {
        let y = -span / 2.0 + 1.27 * k as f32;
        m.append(gull(-1.95, -3.0, y, 0.2, 0.75));
        m.append(gull(1.95, 3.0, y, 0.2, 0.75));
    }
    Ok(m)
}

/// A quad flat pack: a square body `body` wide, 1.4 mm thick on a 0.1 mm standoff,
/// with `pins` gull-wing leads a quarter to a side, `pitch` apart, reaching 1 mm out;
/// pin 1 at the top of the left side, as the footprint numbers them.
fn qfp(p: Params<'_>) -> Result<Mesh, String> {
    let n = num(p, "pins", 48.0).round() as i32;
    if !(8..=256).contains(&n) || n % 4 != 0 {
        return Err(format!("a QFP needs a multiple of 4 pins, 8 to 256, not {n}"));
    }
    let pitch = num(p, "pitch", 0.5);
    let per = n / 4;
    let row = (per - 1) as f32 * pitch;
    let h = num(p, "body", 7.0).max(row + 2.0 * pitch) / 2.0;
    let mut m = cuboid([-h, -h, 0.1], [h, h, 1.5], color("black"));
    // Pin 1's dimple, in the corner by it.
    let d = (h * 0.12).clamp(0.25, 0.6);
    m.append(cylinder(d, 1.5, 1.52, 20, color("grey")).translated([
        -h + 2.0 * d,
        h - 2.0 * d,
        0.0,
    ]));
    let half = (pitch * 0.22).min(0.25);
    let mut side = Mesh::default();
    for k in 0..per {
        side.append(gull(-h, -h - 1.0, -row / 2.0 + pitch * k as f32, half, 0.75));
    }
    for turn in 0..4 {
        m.append(side.clone().transformed(&Mat4::rotate_z(turn as f32 * FRAC_PI_2)));
    }
    Ok(m)
}

fn sot223() -> Mesh {
    let mut m = cuboid([-3.25, -1.75, 0.1], [3.25, 1.75, 1.7], color("black"));
    for x in [-2.3f32, 0.0, 2.3] {
        m.append(cuboid([x - 0.35, -3.5, 0.0], [x + 0.35, -1.75, 0.1], tin()));
        m.append(cuboid([x - 0.35, -2.4, 0.1], [x + 0.35, -1.75, 0.8], tin()));
    }
    m.append(cuboid([-1.5, 1.75, 0.0], [1.5, 3.5, 0.1], tin()));
    m.append(cuboid([-1.5, 1.75, 0.1], [1.5, 2.2, 0.8], tin()));
    m
}

/// A dual in-line package with `pins` pins (even), 7.62 mm between its rows.
fn dip(p: Params<'_>) -> Result<Mesh, String> {
    let pins = num(p, "pins", 8.0).round() as i32;
    if pins < 4 || pins % 2 != 0 || pins > 64 {
        return Err(format!("a DIP needs an even number of pins, 4 to 64, not {pins}"));
    }
    let per_side = pins / 2;
    let span = (per_side - 1) as f32 * 2.54;
    let len = span + 2.3;
    let mut m = cuboid([-len / 2.0, -3.2, 0.8], [len / 2.0, 3.2, 4.2], color("black"));
    // Pin 1's notch and dot.
    m.append(cylinder(0.9, 4.18, 4.22, 20, color("grey")).translated([-len / 2.0, 0.0, 0.0]));
    m.append(cylinder(0.45, 4.18, 4.22, 16, color("grey")).translated([
        -len / 2.0 + 1.3,
        -2.0,
        0.0,
    ]));
    for k in 0..per_side {
        let x = -span / 2.0 + 2.54 * k as f32;
        for side in [-1.0f32, 1.0] {
            let y0 = side * 3.2;
            let y1 = side * 3.81;
            m.append(cuboid([x - 0.75, y0.min(y1), 1.8], [x + 0.75, y0.max(y1), 2.3], tin()));
            m.append(cuboid([x - 0.75, y1 - 0.13, 0.5], [x + 0.75, y1 + 0.13, 2.3], tin()));
            m.append(cuboid([x - 0.25, y1 - 0.13, BELOW], [x + 0.25, y1 + 0.13, 0.5], tin()));
        }
    }
    // Stood with its rows running up the board, pin 1 at the top left, as KiCad's
    // footprints have it.
    Ok(m.transformed(&Mat4::rotate_z(-FRAC_PI_2)))
}

fn bridge() -> Mesh {
    let mut m = cuboid([-5.0, -5.0, 0.5], [5.0, 5.0, 5.5], color("black"));
    m.append(leads(&[(-2.54, -2.54), (2.54, -2.54), (-2.54, 2.54), (2.54, 2.54)], 0.5));
    m
}

// ---- Electromechanical ---------------------------------------------------------------

/// A sugar-cube relay (like an SRD-05) on five pins.
fn relay(p: Params<'_>) -> Mesh {
    let mut m = cuboid([-9.5, -7.75, 0.3], [9.5, 7.75, 15.3], color(&text(p, "color", "blue")));
    m.append(leads(&[(-8.0, -6.0), (-8.0, 6.0), (6.0, 0.0), (8.0, -6.0), (8.0, 6.0)], 0.3));
    m
}

/// A small brushed DC motor lying along X: can, end cap, shaft and terminals.
fn motor() -> Mesh {
    let r = 10.0;
    let mut body = cylinder(r, 0.0, 25.0, 40, color("silver"));
    body.append(cylinder(r * 0.98, -3.0, 0.0, 40, color("black")));
    body.append(cylinder(3.0, 25.0, 26.5, 24, color("silver")));
    body.append(cylinder(1.0, 26.5, 34.0, 16, color("silver")));
    let mut m = along_x(body, 25.0, r + 0.5);
    for y in [-4.0f32, 4.0] {
        m.append(cuboid([-16.0, y - 1.0, r - 1.0], [-15.6, y + 1.0, r + 3.0], color("copper")));
    }
    m
}

/// A loudspeaker facing up: magnet, cone, rim and dust cap.
fn speaker() -> Mesh {
    let mut m = cylinder(10.0, 0.0, 6.0, 32, color("black"));
    m.append(lathe(&[(6.0, 6.0), (24.0, 14.0), (24.0, 14.6), (5.0, 7.0)], 48, color("grey")));
    m.append(lathe(
        &[(23.0, 14.0), (26.0, 14.0), (26.0, 15.0), (23.0, 15.0), (23.0, 14.0)],
        48,
        color("silver"),
    ));
    let cap: Vec<(f32, f32)> = (0..=6)
        .map(|k| {
            let a = FRAC_PI_2 * k as f32 / 6.0;
            (5.0 * a.cos(), 8.0 + 2.5 * a.sin())
        })
        .collect();
    m.append(lathe(&cap, 32, color("black")));
    m
}

/// A miniature incandescent bulb on its screw base.
fn lamp() -> Mesh {
    let mut m = cylinder(4.0, 0.0, 7.0, 24, color("silver"));
    for k in 0..3 {
        let z = 1.5 + 2.0 * k as f32;
        m.append(cylinder(4.3, z, z + 0.8, 24, color("silver")));
    }
    let r = 9.0;
    let mut profile = vec![(4.0, 7.0), (4.5, 10.0)];
    for k in 0..=16 {
        let a = -1.2 + (FRAC_PI_2 + 1.2) * k as f32 / 16.0;
        profile.push((r * a.cos(), 19.0 + r * a.sin()));
    }
    m.append(lathe(&profile, 40, [250, 236, 180, 255]));
    m
}

/// A battery: a 9 V block, or an AA cell for 1.5 V and below.
fn battery(p: Params<'_>) -> Mesh {
    let v = num(p, "V", 9.0);
    if v <= 1.6 {
        let mut body = cylinder(7.25, 0.0, 49.5, 40, color("black"));
        body.append(cylinder(7.3, 30.0, 49.5, 40, color("gold")));
        body.append(cylinder(2.7, 49.5, 50.5, 20, color("silver")));
        return along_x(body, 50.5, 7.5);
    }
    let mut m = cuboid([-24.25, -13.25, 0.0], [24.25, 13.25, 17.5], color("black"));
    m.append(cuboid([-24.3, -13.3, 4.0], [10.0, 13.3, 13.5], color("gold")));
    let snap = |x: f32, r: f32| {
        cylinder(r, 0.0, 3.0, 6, color("silver"))
            .transformed(&Mat4::rotate_y(FRAC_PI_2))
            .translated([24.25, x, 8.75])
    };
    m.append(snap(-6.35, 3.0));
    m.append(snap(6.35, 4.0));
    m
}

/// A solar panel: framed blue cells with white busbars.
fn solar(p: Params<'_>) -> Mesh {
    let cells = num(p, "cells", 36.0).max(1.0) as i32;
    let cols = 4;
    let rows = (cells + cols - 1) / cols;
    let (cw, gap) = (15.0f32, 1.0f32);
    let w = cols as f32 * (cw + gap) + gap;
    let h = rows as f32 * (cw + gap) + gap;
    let mut m = cuboid(
        [-w / 2.0 - 2.0, -h / 2.0 - 2.0, 0.0],
        [w / 2.0 + 2.0, h / 2.0 + 2.0, 3.0],
        color("aluminium"),
    );
    for r in 0..rows {
        for c in 0..cols {
            if r * cols + c >= cells {
                continue;
            }
            let x = -w / 2.0 + gap + c as f32 * (cw + gap);
            let y = -h / 2.0 + gap + r as f32 * (cw + gap);
            m.append(cuboid([x, y, 3.0], [x + cw, y + cw, 3.2], color("darkblue")));
            for k in 1..3 {
                let bx = x + cw * k as f32 / 3.0;
                m.append(cuboid([bx - 0.2, y, 3.2], [bx + 0.2, y + cw, 3.25], color("white")));
            }
        }
    }
    m
}

/// A rotary potentiometer: body, shaft and knob with its pointer.
fn pot(p: Params<'_>) -> Mesh {
    let position = num(p, "position", 0.5).clamp(0.0, 1.0);
    let mut m = cuboid([-5.0, -5.0, 0.5], [5.0, 5.0, 7.0], color("darkblue"));
    m.append(cylinder(3.5, 7.0, 9.0, 24, color("silver")));
    m.append(cylinder(1.5, 9.0, 12.0, 16, color("silver")));
    m.append(cylinder(6.0, 12.0, 19.0, 32, color("black")));
    // The pointer turns through 270° with the wiper.
    let angle = (-135.0 + 270.0 * position).to_radians();
    let pointer = cuboid([-0.4, 0.0, 19.0], [0.4, 5.2, 19.2], color("white"));
    m.append(pointer.transformed(&Mat4::rotate_z(-angle)));
    m.append(leads(&[(-2.54, 6.0), (0.0, 6.0), (2.54, 6.0)], 0.5));
    m.append(cuboid([-3.0, 5.0, 0.4], [3.0, 6.2, 0.6], tin()));
    m
}

/// A toggle switch whose lever leans the way the switch is set (`on`).
fn toggle_switch(p: Params<'_>) -> Mesh {
    let on = flag(p, "on");
    let mut m = cuboid([-6.5, -3.5, 0.3], [6.5, 3.5, 9.0], color("black"));
    m.append(cylinder(3.0, 9.0, 13.0, 24, color("silver")));
    let lean: f32 = if on { 0.35 } else { -0.35 };
    let tip: V3 = [10.0 * lean.sin(), 0.0, 13.0 + 10.0 * lean.cos()];
    m.append(rod([0.0, 0.0, 12.5], tip, 1.0, 16, color("silver")));
    m.append(sphere(tip, 1.4, 16, color("silver")));
    m.append(leads(&[(-4.7, 0.0), (0.0, 0.0), (4.7, 0.0)], 0.3));
    m
}

/// A 6 mm tactile pushbutton; its cap sits lower while `pressed`.
fn tactile(p: Params<'_>) -> Mesh {
    let down = if flag(p, "pressed") { 0.4 } else { 0.0 };
    let mut m = cuboid([-3.0, -3.0, 0.0], [3.0, 3.0, 3.4], color("black"));
    m.append(cuboid([-3.05, -3.05, 3.4], [3.05, 3.05, 3.6], color("silver")));
    m.append(cylinder(1.75, 3.6, 5.0 - down, 24, color("black")));
    m.append(leads(&[(-3.25, -2.25), (3.25, -2.25), (-3.25, 2.25), (3.25, 2.25)], 0.5));
    m
}

/// A mains transformer: laminated E-I core with the bobbin showing front and back.
fn transformer(p: Params<'_>) -> Mesh {
    let _ = p;
    let mut m = cuboid([-15.0, -8.0, 0.5], [15.0, 8.0, 25.5], color("grey"));
    m.append(cuboid([-9.0, -12.0, 3.0], [9.0, 12.0, 23.0], color("yellow")));
    for k in 0..12 {
        let z = 1.0 + 2.0 * k as f32;
        m.append(cuboid([-15.05, -8.05, z], [15.05, 8.05, z + 0.15], color("black")));
    }
    let pins: Vec<(f32, f32)> =
        [-7.5f32, -2.5, 2.5, 7.5].iter().flat_map(|&x| [(x, -10.0), (x, 10.0)]).collect();
    m.append(leads(&pins, 0.5));
    m
}

/// A 1 × N pin header, 2.54 mm pitch: a black plastic strip with square gold pins.
fn header(p: Params<'_>) -> Mesh {
    let n = num(p, "pins", 2.0).round().max(1.0) as i32;
    let x0 = -(n - 1) as f32 * 1.27;
    let mut m = cuboid([x0 - 1.27, -1.27, 0.0], [-x0 + 1.27, 1.27, 2.5], color("black"));
    for k in 0..n {
        let x = x0 + 2.54 * k as f32;
        m.append(cuboid([x - 0.32, -0.32, BELOW], [x + 0.32, 0.32, 8.5], color("gold")));
    }
    m
}

/// A screw terminal block, 5.08 mm pitch: green, a wire entry and a screw per way.
fn terminal(p: Params<'_>) -> Mesh {
    let n = num(p, "pins", 2.0).round().max(1.0) as i32;
    let x0 = -(n - 1) as f32 * 2.54;
    let mut m = cuboid([x0 - 2.54, -4.0, 0.0], [-x0 + 2.54, 4.0, 10.0], color("green"));
    for k in 0..n {
        let x = x0 + 5.08 * k as f32;
        m.append(cylinder(1.6, 10.0, 10.1, 20, color("silver")).translated([x, 1.0, 0.0]));
        m.append(cuboid([x - 1.2, 0.85, 10.1], [x + 1.2, 1.15, 10.2], color("black")));
        m.append(cuboid([x - 1.6, -4.05, 3.0], [x + 1.6, -3.95, 6.2], color("black")));
        m.append(rod([x, 0.0, BELOW], [x, 0.0, 0.0], 0.5, 8, tin()));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn resistor_bands_read_the_value() {
        assert_eq!(resistor_bands(4700.0), ["yellow", "violet", "red", "gold"]);
        assert_eq!(resistor_bands(330.0), ["orange", "orange", "brown", "gold"]);
        assert_eq!(resistor_bands(10.0), ["brown", "black", "black", "gold"]);
        assert_eq!(resistor_bands(1e6), ["brown", "black", "green", "gold"]);
        assert_eq!(resistor_bands(2.2), ["red", "red", "gold", "gold"]);
        assert_eq!(resistor_bands(999.0), ["brown", "black", "red", "gold"], "rounds to 1 kΩ");
    }

    #[test]
    fn every_package_builds_on_the_board() {
        for name in PACKAGES {
            let p = params(&[]);
            let m = generate(name, &|k| p.get(k).cloned()).unwrap();
            assert!(m.triangles() > 10, "{name}");
            let (lo, hi) = m.bounds().unwrap();
            assert!(lo[2] >= BELOW - 1e-3, "{name} reaches below the leads: {lo:?}");
            assert!(hi[2] > 0.3, "{name} rises above the board: {hi:?}");
            assert!(hi[0] - lo[0] < 140.0 && hi[1] - lo[1] < 200.0, "{name} size {lo:?} {hi:?}");
        }
        let p = params(&[("pins", "7")]);
        assert!(generate("dip", &|k| p.get(k).cloned()).is_err());
        assert!(generate("nope", &|_| None).is_err());
    }

    #[test]
    fn a_dip_grows_with_its_pins_and_a_switch_lever_follows_its_state() {
        // Rows run up the board (along Y), as the footprint's do.
        let length = |pins: &str| {
            let p = params(&[("pins", pins)]);
            let (lo, hi) = generate("dip", &|k| p.get(k).cloned()).unwrap().bounds().unwrap();
            hi[1] - lo[1]
        };
        assert!((length("16") - length("8") - 4.0 * 2.54).abs() < 0.01);
        // Where the lever's tip is: the mean X of everything above the bushing.
        let tip = |on: &str| {
            let p = params(&[("on", on)]);
            let m = generate("toggle_switch", &|k| p.get(k).cloned()).unwrap();
            let high: Vec<f32> =
                m.vertices.iter().filter(|v| v.pos[2] > 20.0).map(|v| v.pos[0]).collect();
            high.iter().sum::<f32>() / high.len() as f32
        };
        assert!(tip("true") > tip("false") + 2.0, "the lever leans to the on side");
    }
}
