//! SVG export. Coordinates are millimetres, so the output prints at true scale.

use crate::{DASH, GAP, HAlign, LINE_WIDTH, Scene, Shape, Theme, VAlign, dashed, line_width};
use std::fmt::Write;

/// Renders a scene as a standalone SVG document.
pub fn to_svg(scene: &Scene, theme: &Theme) -> String {
    let margin = 5.08;
    let b = if scene.bounds.is_empty() {
        crate::Rect { min: crate::Pt::new(0.0, 0.0), max: crate::Pt::new(50.0, 50.0) }
    } else {
        scene.bounds.expand(margin)
    };
    let (w, h) = (b.width(), b.height());
    let mut out = String::new();
    let _ = writeln!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.3}mm" height="{h:.3}mm" viewBox="{:.3} {:.3} {w:.3} {h:.3}">"#,
        b.min.x, b.min.y
    );
    let _ = writeln!(out, "<title>{}</title>", escape(&scene.cell.to_string()));
    let _ = writeln!(
        out,
        r#"<rect x="{:.3}" y="{:.3}" width="{w:.3}" height="{h:.3}" fill="{}"/>"#,
        b.min.x,
        b.min.y,
        theme.background.css()
    );
    let _ = writeln!(
        out,
        r#"<g stroke-width="{LINE_WIDTH}" stroke-linecap="round" stroke-linejoin="round" font-family="DejaVu Sans Mono, monospace">"#
    );
    for e in &scene.elements {
        let color = theme.color(e.role).css();
        match &e.shape {
            Shape::Polyline { points, closed, filled } => {
                let pts: Vec<String> =
                    points.iter().map(|p| format!("{:.4},{:.4}", p.x, p.y)).collect();
                let tag = if *closed { "polygon" } else { "polyline" };
                let fill = if *filled { color.as_str() } else { "none" };
                let width = line_width(e.role);
                let dash = if dashed(e.role) {
                    format!(r#" stroke-dasharray="{DASH} {GAP}""#)
                } else {
                    String::new()
                };
                let _ = writeln!(
                    out,
                    r#"<{tag} points="{}" fill="{fill}" stroke="{color}" stroke-width="{width:.4}"{dash}/>"#,
                    pts.join(" ")
                );
            }
            Shape::Circle { center, radius, filled } => {
                let fill = if *filled { color.as_str() } else { "none" };
                let _ = writeln!(
                    out,
                    r#"<circle cx="{:.4}" cy="{:.4}" r="{radius:.4}" fill="{fill}" stroke="{color}"/>"#,
                    center.x, center.y
                );
            }
            Shape::Text { at, text, size, halign, valign, vertical } => {
                let anchor = match halign {
                    HAlign::Left => "start",
                    HAlign::Center => "middle",
                    HAlign::Right => "end",
                };
                // Place the baseline explicitly; `dominant-baseline` support varies.
                let shift = match valign {
                    VAlign::Top => size * 0.75,
                    VAlign::Center => size * 0.36,
                    VAlign::Bottom => 0.0,
                };
                let (x, y, rotate) = if *vertical {
                    (
                        at.x + shift,
                        at.y,
                        format!(r#" transform="rotate(-90 {:.4} {:.4})""#, at.x + shift, at.y),
                    )
                } else {
                    (at.x, at.y + shift, String::new())
                };
                let _ = writeln!(
                    out,
                    r#"<text x="{x:.4}" y="{y:.4}" font-size="{size:.3}" text-anchor="{anchor}" fill="{color}" stroke="none"{rotate}>{}</text>"#,
                    escape(text)
                );
            }
        }
    }
    out.push_str("</g>\n</svg>\n");
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
