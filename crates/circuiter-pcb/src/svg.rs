//! The board drawn as an SVG picture, layers in KiCad's colours: for documentation, and
//! for a look at the board without a Gerber viewer.

use crate::connect::Connectivity;
use crate::font;
use crate::geometry::{self, Contour};
use crate::layout::{Layout, P, p};
use crate::zones::ZoneFill;
use circuiter_model::board::{Graphic, Layer};
use std::fmt::Write as _;

/// A layer's colour, as KiCad shows it.
pub fn colour(layer: Layer) -> &'static str {
    match layer {
        Layer::FCu => "#c83434",
        Layer::BCu => "#4d7fc4",
        Layer::In(1) => "#7fc87f",
        Layer::In(_) => "#ce7d2c",
        Layer::FSilk => "#f2eda1",
        Layer::BSilk => "#e8b2a7",
        Layer::FMask | Layer::BMask => "#d864ff",
        Layer::FPaste | Layer::BPaste => "#b4a0a0",
        Layer::FCrtYd | Layer::BCrtYd => "#ff26e2",
        Layer::FFab | Layer::BFab => "#afafaf",
        Layer::EdgeCuts => "#d0d2cd",
    }
}

fn path(contours: &[Contour]) -> String {
    let mut d = String::new();
    for c in contours {
        for (k, q) in c.iter().enumerate() {
            let _ = write!(d, "{}{:.4},{:.4} ", if k == 0 { "M" } else { "L" }, q[0], q[1]);
        }
        d.push_str("Z ");
    }
    d
}

fn polyline(points: &[P]) -> String {
    points.iter().map(|q| format!("{:.4},{:.4}", q[0], q[1])).collect::<Vec<_>>().join(" ")
}

/// Draws the board: back copper under front, pours, pads, holes, silkscreen, the edge,
/// and the ratsnest of what's still to route.
pub fn plot(layout: &Layout, fills: &[ZoneFill], connectivity: Option<&Connectivity>) -> String {
    let board = &layout.board;
    let outline: Vec<P> = board.outline.iter().map(|q| p(*q)).collect();
    let (lo, hi) =
        geometry::bounds(&vec![vec![outline.clone()]]).unwrap_or(([0.0, 0.0], [10.0, 10.0]));
    let m = 2.0;
    let mut s = String::new();
    let _ = writeln!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" width="{}mm" height="{}mm">"#,
        lo[0] - m,
        lo[1] - m,
        hi[0] - lo[0] + 2.0 * m,
        hi[1] - lo[1] + 2.0 * m,
        hi[0] - lo[0] + 2.0 * m,
        hi[1] - lo[1] + 2.0 * m
    );
    let _ = writeln!(
        s,
        r##"<rect x="{}" y="{}" width="{}" height="{}" fill="#001023"/>"##,
        lo[0] - m,
        lo[1] - m,
        hi[0] - lo[0] + 2.0 * m,
        hi[1] - lo[1] + 2.0 * m
    );
    let mut layers = board.copper();
    layers.reverse();
    for layer in layers {
        let c = colour(layer);
        let _ = writeln!(s, r#"<g fill="{c}" stroke="none" opacity="0.85">"#);
        for f in fills.iter().filter(|f| f.layer == layer) {
            for piece in &f.shapes {
                let _ = writeln!(s, r#"<path fill-rule="evenodd" d="{}"/>"#, path(piece));
            }
        }
        for t in board.tracks.iter().filter(|t| t.layer == layer) {
            let _ = writeln!(s, r#"<path d="{}"/>"#, path(&[geometry::track(t, 0.0)]));
        }
        for pad in layout.pads.iter().filter(|pd| pd.layers.contains(&layer)) {
            let _ = writeln!(s, r#"<path d="{}"/>"#, path(&[geometry::pad(pad, 0.0)]));
        }
        s.push_str("</g>\n");
    }
    for v in &board.vias {
        let _ = writeln!(
            s,
            r##"<circle cx="{}" cy="{}" r="{}" fill="#c2c2c2"/>"##,
            v.at.x.mm(),
            v.at.y.mm(),
            v.diameter.mm() / 2.0
        );
    }
    for pad in &layout.pads {
        if let Some(d) = pad.drill() {
            let _ = writeln!(
                s,
                r##"<circle cx="{}" cy="{}" r="{}" fill="#001023"/>"##,
                pad.center[0],
                pad.center[1],
                d / 2.0
            );
        }
    }
    for v in &board.vias {
        let _ = writeln!(
            s,
            r##"<circle cx="{}" cy="{}" r="{}" fill="#001023"/>"##,
            v.at.x.mm(),
            v.at.y.mm(),
            v.drill.mm() / 2.0
        );
    }
    let graphics: Vec<Graphic> = (0..layout.parts.len())
        .flat_map(|k| layout.graphics(k))
        .chain(board.graphics.iter().cloned())
        .collect();
    for want in [Layer::BSilk, Layer::FSilk] {
        let c = colour(want);
        let _ = writeln!(
            s,
            r#"<g stroke="{c}" fill="none" stroke-linecap="round" stroke-linejoin="round">"#
        );
        for g in &graphics {
            match g {
                Graphic::Line { layer, a, b, width } if *layer == want => {
                    let _ = writeln!(
                        s,
                        r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke-width="{}"/>"#,
                        a.x.mm(),
                        a.y.mm(),
                        b.x.mm(),
                        b.y.mm(),
                        width.mm()
                    );
                }
                Graphic::Circle { layer, center, radius, width } if *layer == want => {
                    let _ = writeln!(
                        s,
                        r#"<circle cx="{}" cy="{}" r="{}" stroke-width="{}"/>"#,
                        center.x.mm(),
                        center.y.mm(),
                        radius.mm(),
                        width.mm()
                    );
                }
                Graphic::Poly { layer, points, width, filled } if *layer == want => {
                    let fill = if *filled { c } else { "none" };
                    let pts: Vec<P> = points.iter().map(|q| p(*q)).collect();
                    let _ = writeln!(
                        s,
                        r#"<polygon points="{}" stroke-width="{}" fill="{fill}"/>"#,
                        polyline(&pts),
                        width.mm()
                    );
                }
                Graphic::Text { layer, at, text, size } if *layer == want => {
                    for line in font::strokes(text, p(*at), size.mm(), 0.0, want == Layer::BSilk) {
                        let _ = writeln!(
                            s,
                            r#"<polyline points="{}" stroke-width="{}"/>"#,
                            polyline(&line),
                            font::stroke(size.mm())
                        );
                    }
                }
                _ => {}
            }
        }
        s.push_str("</g>\n");
    }
    let _ = writeln!(
        s,
        r#"<polygon points="{}" fill="none" stroke="{}" stroke-width="0.15"/>"#,
        polyline(&outline),
        colour(Layer::EdgeCuts)
    );
    if let Some(conn) = connectivity {
        for (_, a, b) in &conn.ratsnest {
            let _ = writeln!(
                s,
                r##"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="#ffffff" stroke-width="0.1"/>"##,
                a[0], a[1], b[0], b[1]
            );
        }
    }
    s.push_str("</svg>\n");
    s
}
