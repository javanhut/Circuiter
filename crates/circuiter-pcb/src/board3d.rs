//! The board in 3D: its outline as a slab with every hole drilled, copper under the
//! green mask, tinned pads, white silkscreen, and each placed part's body standing in
//! its footprint (turned with it, and upside down under the board for parts on the
//! back). Board X runs along world X and board Y (down the sheet) along world −Y, with
//! the board's top face at z = 0.

use crate::font;
use crate::geometry::{self, Contour};
use crate::layout::{Layout, P, Place, p};
use crate::zones::ZoneFill;
use circuiter_3d::math::{Mat4, V3};
use circuiter_3d::{Mesh, Rgba, assembly, color, extrude, extrude_holes};
use circuiter_model::Design;
use circuiter_model::board::{Graphic, Layer};

/// Parts whose leads are wired to the board rather than mounted on it: only their
/// solder pads are shown.
pub const WIRED: &[&str] = &["battery", "motor", "speaker", "lamp", "solar"];

const COPPER: f32 = 0.035;
const SILK: f32 = 0.015;

const MASK_OVER_COPPER: Rgba = [44, 132, 74, 255];

/// A board in 3D.
#[derive(Debug, Clone, Default)]
pub struct Board3d {
    /// The board with its copper and markings.
    pub board: Mesh,
    /// Each part's body, by instance path.
    pub parts: Vec<(String, Mesh)>,
    /// Placed parts with no body.
    pub without: Vec<String>,
    pub errors: Vec<String>,
}

impl Board3d {
    pub fn mesh(&self) -> Mesh {
        let mut m = self.board.clone();
        for (_, part) in &self.parts {
            m.append(part.clone());
        }
        m
    }
}

fn world(v: P) -> (f32, f32) {
    (v[0] as f32, -(v[1] as f32))
}

fn ring(c: &Contour) -> Vec<(f32, f32)> {
    c.iter().map(|&q| world(q)).collect()
}

/// Where a part's body goes: its own X and Y (Y up, as bodies are drawn) onto the
/// board, turned as the footprint is; flipped under the board on the back.
pub fn part_transform(place: &Place, thickness: f32) -> Mat4 {
    let at = world(place.at);
    let turn = Mat4::rotate_z((place.rotation as f32).to_radians());
    let m = Mat4::translate([at.0, at.1, 0.0]).mul(&turn);
    if place.back {
        // Mirror across X (as the footprint is) and turn upside down: a half turn about Y.
        m.mul(&Mat4::translate([0.0, 0.0, -thickness])).mul(&Mat4::rotate_y(std::f32::consts::PI))
    } else {
        m
    }
}

/// A stroke of `width` along `points`, raised from `z0` to `z1`.
fn strokes(points: &[P], width: f64, z0: f32, z1: f32, colour: Rgba, out: &mut Mesh) {
    for seg in points.windows(2) {
        out.append(extrude(&ring(&geometry::capsule(seg[0], seg[1], width)), z0, z1, colour));
    }
}

/// The copper, pads and markings on one side: `top` for the front.
fn side(layout: &Layout, fills: &[ZoneFill], top: bool, t: f32, out: &mut Mesh) {
    let cu = if top { Layer::FCu } else { Layer::BCu };
    let silk = if top { Layer::FSilk } else { Layer::BSilk };
    // Heights going outwards from this face.
    let z = |h: f32| if top { h } else { -t - h };
    let span = |a: f32, b: f32| if top { (z(a), z(b)) } else { (z(b), z(a)) };
    let (c0, c1) = span(0.0, COPPER);
    for f in fills.iter().filter(|f| f.layer == cu) {
        for piece in &f.shapes {
            if let Some((outer, holes)) = piece.split_first() {
                let holes: Vec<_> = holes.iter().map(ring).collect();
                out.append(extrude_holes(&ring(outer), &holes, c0, c1 - 0.005, MASK_OVER_COPPER));
            }
        }
    }
    for t in crate::teardrops::shapes(layout).iter().filter(|t| t.layer == cu) {
        out.append(extrude(&ring(&t.outline), c0, c1 - 0.005, MASK_OVER_COPPER));
    }
    for tr in layout.board.tracks.iter().filter(|tr| tr.layer == cu) {
        out.append(extrude(&ring(&geometry::track(tr, 0.0)), c0, c1 - 0.005, MASK_OVER_COPPER));
    }
    let tin = color("silver");
    for pad in layout.pads.iter().filter(|pd| pd.layers.contains(&cu)) {
        let outline = ring(&geometry::pad(pad, 0.0));
        match pad.drill() {
            Some(d) => {
                let hole = ring(&geometry::circle(pad.center, d / 2.0));
                out.append(extrude_holes(&outline, &[hole], c0, c1, tin));
            }
            None => out.append(extrude(&outline, c0, c1, tin)),
        }
    }
    for v in &layout.board.vias {
        let outline = ring(&geometry::via(v, 0.0));
        let hole = ring(&geometry::circle(p(v.at), v.drill.mm() / 2.0));
        out.append(extrude_holes(&outline, &[hole], c0, c1 - 0.005, MASK_OVER_COPPER));
    }
    let (s0, s1) = span(COPPER, COPPER + SILK);
    let white = color("white");
    let graphics = (0..layout.parts.len())
        .flat_map(|k| layout.graphics(k))
        .chain(layout.board.graphics.iter().cloned());
    for g in graphics {
        match g {
            Graphic::Line { layer, a, b, width } if layer == silk => {
                strokes(&[p(a), p(b)], width.mm(), s0, s1, white, out)
            }
            Graphic::Circle { layer, center, radius, width } if layer == silk => {
                let (r, w) = (radius.mm(), width.mm());
                let outer = ring(&geometry::circle(p(center), r + w / 2.0));
                let inner = ring(&geometry::circle(p(center), (r - w / 2.0).max(0.01)));
                out.append(extrude_holes(&outer, &[inner], s0, s1, white));
            }
            Graphic::Poly { layer, points, width, filled } if layer == silk => {
                let pts: Vec<P> = points.iter().map(|q| p(*q)).collect();
                if filled {
                    out.append(extrude(&ring(&pts), s0, s1, white));
                } else {
                    let mut closed = pts.clone();
                    closed.extend(pts.first().copied());
                    strokes(&closed, width.mm(), s0, s1, white, out);
                }
            }
            Graphic::Text { layer, at, text, size } if layer == silk => {
                for line in font::strokes(&text, p(at), size.mm(), 0.0, !top) {
                    strokes(&line, font::stroke(size.mm()), s0, s1, white, out);
                }
            }
            _ => {}
        }
    }
}

/// The board of `layout`, with its parts.
pub fn build(design: &Design, layout: &Layout, fills: &[ZoneFill]) -> Board3d {
    build_live(design, layout, fills, &Live::default())
}

/// A circuit running: parts' parameters as they're set now (a switch thrown, a pot
/// turned), and how brightly each light shines (0 to 1).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    pub params: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    pub glow: std::collections::BTreeMap<String, f32>,
}

/// As [`build`], with the parts as they stand in a running circuit: levers where their
/// switches are, lights lit.
pub fn build_live(design: &Design, layout: &Layout, fills: &[ZoneFill], live: &Live) -> Board3d {
    let board = &layout.board;
    let t = board.thickness.mm() as f32;
    let mut out = Board3d::default();
    let outline: Vec<P> = board.outline.iter().map(|q| p(*q)).collect();
    if outline.len() >= 3 {
        let holes: Vec<Vec<(f32, f32)>> = layout
            .pads
            .iter()
            .filter_map(|pd| Some(ring(&geometry::circle(pd.center, pd.drill()? / 2.0))))
            .chain(board.vias.iter().map(|v| ring(&geometry::circle(p(v.at), v.drill.mm() / 2.0))))
            .collect();
        out.board = extrude_holes(&ring(&geometry::ccw(outline)), &holes, -t, 0.0, color("board"));
    }
    side(layout, fills, true, t, &mut out.board);
    side(layout, fills, false, t, &mut out.board);
    for part in &layout.parts {
        let Some(place) = part.place else { continue };
        if WIRED.contains(&part.package.as_str()) {
            out.without.push(part.path.clone());
            continue;
        }
        let Some(def) = design.cell(&part.cell) else { continue };
        let mut params = part.params.clone();
        if let Some(now) = live.params.get(&part.path) {
            params.extend(now.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        match assembly::body(def, &params) {
            Ok(Some(mut mesh)) => {
                // A lit light glows: its colours washed towards white.
                if let Some(&g) = live.glow.get(&part.path).filter(|g| **g > 0.02) {
                    let tin = color("tin");
                    let k = g.clamp(0.0, 1.0) * 0.75;
                    for v in &mut mesh.vertices {
                        if v.color != tin {
                            for c in 0..3 {
                                v.color[c] = (f32::from(v.color[c])
                                    + (255.0 - f32::from(v.color[c])) * k)
                                    as u8;
                            }
                        }
                    }
                }
                out.parts.push((part.path.clone(), mesh.transformed(&part_transform(&place, t))))
            }
            Ok(None) => out.without.push(part.path.clone()),
            Err(e) => out.errors.push(format!("{}: {e}", part.path)),
        }
    }
    out
}

/// A point of a part's body in the footprint's frame (Y down), for checks.
pub fn footprint_point(v: V3) -> P {
    [v[0] as f64, -(v[1] as f64)]
}
