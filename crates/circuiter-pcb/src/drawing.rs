//! Drawings for the board house and the assembler, as Altium's Draftsman makes them:
//! sheets with a frame and title block. The fabrication drawing dimensions the board,
//! marks every hole with its tool's symbol, tables the drills and the layer stack, and
//! lists the notes a board house builds to. The assembly drawing shows each side's
//! parts by their outlines and references (those left off dashed), beside the bill of
//! materials.

use crate::layout::{Layout, P, p};
use circuiter_model::board::{Graphic, Layer, PadKind, StackKind};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// An A4 sheet, landscape, in millimetres.
const SHEET: (f64, f64) = (297.0, 210.0);
const MARGIN: f64 = 10.0;
/// Where the board's drawing goes, from the sheet's top left.
const AREA: (f64, f64, f64, f64) = (22.0, 22.0, 170.0, 120.0);
const INK: &str = "#1d2433";
const FAINT: &str = "#8a93a6";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A sheet being drawn.
struct Sheet {
    s: String,
}

impl Sheet {
    fn new() -> Sheet {
        let mut s = String::new();
        let _ = writeln!(
            s,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}mm" height="{h}mm" font-family="DejaVu Sans, Arial, sans-serif">"#,
            w = SHEET.0,
            h = SHEET.1
        );
        let _ = writeln!(
            s,
            r##"<defs><marker id="arrow" viewBox="0 0 10 10" refX="10" refY="5" markerWidth="4" markerHeight="4" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill="{INK}"/></marker></defs>"##
        );
        let _ = writeln!(s, r##"<rect width="{}" height="{}" fill="#ffffff"/>"##, SHEET.0, SHEET.1);
        let _ = writeln!(
            s,
            r#"<rect x="{MARGIN}" y="{MARGIN}" width="{}" height="{}" fill="none" stroke="{INK}" stroke-width="0.5"/>"#,
            SHEET.0 - 2.0 * MARGIN,
            SHEET.1 - 2.0 * MARGIN
        );
        Sheet { s }
    }

    fn text(&mut self, at: P, size: f64, anchor: &str, text: &str) {
        let _ = writeln!(
            self.s,
            r#"<text x="{:.3}" y="{:.3}" font-size="{size}" text-anchor="{anchor}" fill="{INK}">{}</text>"#,
            at[0],
            at[1],
            esc(text)
        );
    }

    fn line(&mut self, a: P, b: P, width: f64, colour: &str) {
        let _ = writeln!(
            self.s,
            r#"<line x1="{:.3}" y1="{:.3}" x2="{:.3}" y2="{:.3}" stroke="{colour}" stroke-width="{width}"/>"#,
            a[0], a[1], b[0], b[1]
        );
    }

    fn poly(&mut self, pts: &[P], width: f64, colour: &str, dashed: bool) {
        let pts: Vec<String> = pts.iter().map(|q| format!("{:.3},{:.3}", q[0], q[1])).collect();
        let dash = if dashed { r#" stroke-dasharray="0.8 0.5""# } else { "" };
        let _ = writeln!(
            self.s,
            r#"<polygon points="{}" fill="none" stroke="{colour}" stroke-width="{width}"{dash}/>"#,
            pts.join(" ")
        );
    }

    /// A dimension from `a` to `b`, drawn `off` to its side, with its length.
    fn dimension(&mut self, a: P, b: P, off: P, label: &str) {
        let (a2, b2) = ([a[0] + off[0], a[1] + off[1]], [b[0] + off[0], b[1] + off[1]]);
        self.line(a, [a2[0] + off[0] * 0.15, a2[1] + off[1] * 0.15], 0.15, FAINT);
        self.line(b, [b2[0] + off[0] * 0.15, b2[1] + off[1] * 0.15], 0.15, FAINT);
        let _ = writeln!(
            self.s,
            r#"<line x1="{:.3}" y1="{:.3}" x2="{:.3}" y2="{:.3}" stroke="{INK}" stroke-width="0.2" marker-start="url(#arrow)" marker-end="url(#arrow)"/>"#,
            a2[0], a2[1], b2[0], b2[1]
        );
        let mid = [(a2[0] + b2[0]) / 2.0, (a2[1] + b2[1]) / 2.0];
        if (b[0] - a[0]).abs() >= (b[1] - a[1]).abs() {
            self.text([mid[0], mid[1] - 1.0], 2.5, "middle", label);
        } else {
            let _ = writeln!(
                self.s,
                r#"<text x="{:.3}" y="{:.3}" font-size="2.5" text-anchor="middle" fill="{INK}" transform="rotate(-90 {:.3} {:.3})">{}</text>"#,
                mid[0] - 1.0,
                mid[1],
                mid[0] - 1.0,
                mid[1],
                esc(label)
            );
        }
    }

    /// A table at `at`: column widths, a header row and rows, `row` high.
    fn table(&mut self, at: P, widths: &[f64], header: &[&str], rows: &[Vec<String>]) -> f64 {
        let row = 4.5;
        let total: f64 = widths.iter().sum();
        let mut y = at[1];
        for (k, cells) in
            std::iter::once(header.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
                .chain(rows.iter().cloned())
                .enumerate()
        {
            if k == 0 {
                let _ = writeln!(
                    self.s,
                    r##"<rect x="{:.3}" y="{:.3}" width="{total:.3}" height="{row}" fill="#eef1f6"/>"##,
                    at[0], y
                );
            }
            let mut x = at[0];
            for (c, w) in cells.iter().zip(widths) {
                self.text([x + 1.0, y + row - 1.3], 2.4, "start", c);
                x += w;
            }
            self.line([at[0], y + row], [at[0] + total, y + row], 0.15, FAINT);
            y += row;
        }
        self.poly(&[at, [at[0] + total, at[1]], [at[0] + total, y], [at[0], y]], 0.25, INK, false);
        y
    }

    /// The title block in the bottom right corner.
    fn title(&mut self, rows: &[(&str, String)]) {
        let (w, row) = (95.0, 5.5);
        let x = SHEET.0 - MARGIN - w;
        let y0 = SHEET.1 - MARGIN - row * rows.len() as f64;
        self.poly(
            &[[x, y0], [x + w, y0], [x + w, SHEET.1 - MARGIN], [x, SHEET.1 - MARGIN]],
            0.4,
            INK,
            false,
        );
        for (k, (name, value)) in rows.iter().enumerate() {
            let y = y0 + row * k as f64;
            if k > 0 {
                self.line([x, y], [x + w, y], 0.15, INK);
            }
            self.text([x + 1.5, y + 3.9], 2.2, "start", name);
            self.text([x + 25.0, y + 4.0], if k == 0 { 3.2 } else { 2.6 }, "start", value);
        }
        self.line([x + 23.5, y0], [x + 23.5, SHEET.1 - MARGIN], 0.15, INK);
    }

    fn finish(mut self) -> String {
        self.s.push_str("</svg>\n");
        self.s
    }
}

/// Board to sheet: scaled to fit the drawing area, Y down as both are.
#[derive(Debug, Clone, Copy)]
struct View {
    lo: P,
    scale: f64,
    at: P,
}

impl View {
    fn fit(lo: P, hi: P, area: (f64, f64, f64, f64)) -> View {
        let (w, h) = ((hi[0] - lo[0]).max(1.0), (hi[1] - lo[1]).max(1.0));
        // A scale a drawing would print: 10:1, 5:1, 4:1, 2:1, 1:1, 1:2…
        let fit = (area.2 / w).min(area.3 / h);
        let scale = [10.0, 5.0, 4.0, 3.0, 2.0, 1.5, 1.0, 0.5, 0.25, 0.2, 0.1]
            .into_iter()
            .find(|s| *s <= fit)
            .unwrap_or(fit);
        let at = [area.0 + (area.2 - w * scale) / 2.0, area.1 + (area.3 - h * scale) / 2.0];
        View { lo, scale, at }
    }

    fn map(&self, q: P) -> P {
        [
            self.at[0] + (q[0] - self.lo[0]) * self.scale,
            self.at[1] + (q[1] - self.lo[1]) * self.scale,
        ]
    }

    fn label(&self) -> String {
        if self.scale >= 1.0 {
            format!("{}:1", trim(self.scale))
        } else {
            format!("1:{}", trim(1.0 / self.scale))
        }
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn outline(layout: &Layout) -> (Vec<P>, P, P) {
    let pts: Vec<P> = layout.board.edge().iter().map(|q| p(*q)).collect();
    let (lo, hi) =
        crate::geometry::bounds(&vec![vec![pts.clone()]]).unwrap_or(([0.0, 0.0], [10.0, 10.0]));
    (pts, lo, hi)
}

/// A drill's symbol on the drawing, by its tool's number.
fn symbol(s: &mut Sheet, k: usize, at: P, r: f64) {
    let [x, y] = at;
    let w = 0.2;
    match k % 6 {
        0 => {
            let _ = writeln!(
                s.s,
                r#"<circle cx="{x:.3}" cy="{y:.3}" r="{r}" fill="none" stroke="{INK}" stroke-width="{w}"/>"#
            );
            s.line([x - r, y], [x + r, y], w, INK);
            s.line([x, y - r], [x, y + r], w, INK);
        }
        1 => {
            s.line([x - r, y - r], [x + r, y + r], w, INK);
            s.line([x - r, y + r], [x + r, y - r], w, INK);
        }
        2 => {
            s.poly(&[[x - r, y - r], [x + r, y - r], [x + r, y + r], [x - r, y + r]], w, INK, false)
        }
        3 => s.poly(&[[x, y - r], [x + r, y + r * 0.8], [x - r, y + r * 0.8]], w, INK, false),
        4 => s.poly(&[[x, y - r], [x + r, y], [x, y + r], [x - r, y]], w, INK, false),
        _ => {
            let _ = writeln!(
                s.s,
                r#"<circle cx="{x:.3}" cy="{y:.3}" r="{r}" fill="none" stroke="{INK}" stroke-width="{w}"/>"#
            );
            let _ = writeln!(
                s.s,
                r#"<circle cx="{x:.3}" cy="{y:.3}" r="{:.3}" fill="{INK}"/>"#,
                r * 0.35
            );
        }
    }
}

/// Every hole by tool: (diameter, plated) → where.
pub fn holes(layout: &Layout) -> BTreeMap<(i64, bool), Vec<P>> {
    let mut tools: BTreeMap<(i64, bool), Vec<P>> = BTreeMap::new();
    let um = |d: f64| (d * 1e3).round() as i64;
    for pd in &layout.pads {
        match pd.kind {
            PadKind::Through { drill } => {
                tools.entry((um(drill.mm()), true)).or_default().push(pd.center)
            }
            PadKind::Hole { drill } => {
                tools.entry((um(drill.mm()), false)).or_default().push(pd.center)
            }
            PadKind::Smd => {}
        }
    }
    for v in &layout.board.vias {
        tools.entry((um(v.drill.mm()), true)).or_default().push(p(v.at));
    }
    tools
}

/// The fabrication drawing.
pub fn fabrication(layout: &Layout, project: &str) -> String {
    let board = &layout.board;
    let rules = &board.rules;
    let (pts, lo, hi) = outline(layout);
    let view = View::fit(lo, hi, AREA);
    let mut s = Sheet::new();
    let mapped: Vec<P> = pts.iter().map(|q| view.map(*q)).collect();
    s.poly(&mapped, 0.35, INK, false);
    for g in board.graphics.iter() {
        if let Graphic::Poly { layer: Layer::EdgeCuts, points, .. } = g {
            let c: Vec<P> = points.iter().map(|q| view.map(p(*q))).collect();
            s.poly(&c, 0.3, INK, false);
        }
    }
    // Overall size.
    let (a, b) = (view.map(lo), view.map(hi));
    s.dimension([a[0], b[1]], [b[0], b[1]], [0.0, 8.0], &format!("{:.2}", hi[0] - lo[0]));
    s.dimension([a[0], a[1]], [a[0], b[1]], [-8.0, 0.0], &format!("{:.2}", hi[1] - lo[1]));
    // Holes by symbol.
    let tools = holes(layout);
    let r = 0.9;
    for (k, (_, at)) in tools.iter().enumerate() {
        for q in at {
            symbol(&mut s, k, view.map(*q), r);
        }
    }
    let x = AREA.0 + AREA.2 + 8.0;
    s.text([x, 18.0], 3.0, "start", "DRILL TABLE");
    let rows: Vec<Vec<String>> = tools
        .iter()
        .map(|((um, plated), at)| {
            vec![
                String::new(),
                format!("{:.2} mm", *um as f64 / 1e3),
                at.len().to_string(),
                if *plated { "Plated".into() } else { "Unplated".into() },
            ]
        })
        .collect();
    let end = s.table([x, 20.0], &[10.0, 20.0, 12.0, 17.0], &["", "Hole", "Qty", "Finish"], &rows);
    for k in 0..tools.len() {
        symbol(&mut s, k, [x + 5.0, 20.0 + 4.5 * (k as f64 + 1.0) + 2.25], 1.3);
    }
    let total: usize = tools.values().map(Vec::len).sum();
    s.text([x, end + 4.0], 2.4, "start", &format!("{total} holes in all"));
    // The stack.
    let stack = board.stack();
    let y = end + 12.0;
    s.text([x, y - 2.0], 3.0, "start", "LAYER STACK");
    let rows: Vec<Vec<String>> = stack
        .layers
        .iter()
        .map(|l| {
            vec![
                l.name.clone(),
                match l.kind {
                    StackKind::SolderMask => "Solder mask".into(),
                    StackKind::Copper => "Copper".into(),
                    StackKind::Core => "Core".into(),
                    StackKind::Prepreg => "Prepreg".into(),
                },
                format!("{:.3}", l.thickness.mm()),
                if l.er > 0.0 { format!("{:.2}", l.er) } else { String::new() },
            ]
        })
        .collect();
    let end = s.table([x, y], &[27.0, 20.0, 12.0, 10.0], &["Layer", "Material", "mm", "εr"], &rows);
    s.text([x, end + 4.0], 2.4, "start", &format!("Finished {:.2} mm ±10%", board.thickness.mm()));
    // The notes.
    let copper = board.copper().len();
    let mut notes = vec![
        format!(
            "Material FR-4, Tg ≥ 150 °C, {:.2} mm finished, {copper} copper layers.",
            board.thickness.mm()
        ),
        "Solder mask both sides, green; legend white, both sides as supplied.".to_owned(),
        "Surface finish lead-free HASL or ENIG.".to_owned(),
        format!(
            "Narrowest track {:.3} mm, closest spacing {:.3} mm, smallest hole {:.2} mm.",
            rules.min_track_width.mm(),
            rules.min_clearance.mm(),
            rules.min_drill.mm()
        ),
        "Build and inspect to IPC-6012 class 2 and IPC-A-600.".to_owned(),
        "Electrically test to the IPC-D-356 netlist supplied.".to_owned(),
        "Dimensions in millimetres; the Gerber files rule where they differ from this drawing."
            .to_owned(),
    ];
    if board.panel.is_some() {
        notes.push("Deliver in panels as the panel files describe.".to_owned());
    }
    let y0 = AREA.1 + AREA.3 + 16.0;
    s.text([MARGIN + 4.0, y0], 3.0, "start", "NOTES");
    for (k, n) in notes.iter().enumerate() {
        s.text([MARGIN + 4.0, y0 + 4.5 + 3.6 * k as f64], 2.4, "start", &format!("{}. {n}", k + 1));
    }
    s.title(&[
        ("Fabrication", project.to_owned()),
        ("Size", format!("{:.2} × {:.2} mm", hi[0] - lo[0], hi[1] - lo[1])),
        ("Layers", format!("{copper} copper, {:.2} mm", board.thickness.mm())),
        ("Scale", view.label()),
        ("Drawn by", concat!("Circuiter ", env!("CARGO_PKG_VERSION")).to_owned()),
    ]);
    s.finish()
}

/// The assembly drawing: each side with parts on it, and the bill of materials.
pub fn assembly(layout: &Layout, project: &str) -> String {
    let (pts, lo, hi) = outline(layout);
    let placed: Vec<usize> =
        (0..layout.parts.len()).filter(|&k| layout.parts[k].place.is_some()).collect();
    let back = placed.iter().any(|&k| layout.parts[k].place.is_some_and(|pl| pl.back));
    let mut s = Sheet::new();
    // Top, and the bottom (seen from below, so mirrored) beside it if anything's there.
    let sides: Vec<bool> = if back { vec![false, true] } else { vec![false] };
    let width = AREA.2 / sides.len() as f64;
    let mut scale_label = String::new();
    for (i, &bottom) in sides.iter().enumerate() {
        let area = (AREA.0 + width * i as f64, AREA.1, width - 6.0, AREA.3);
        let view = View::fit(lo, hi, area);
        scale_label = view.label();
        let flip = |q: P| if bottom { [lo[0] + hi[0] - q[0], q[1]] } else { q };
        let map = |q: P| view.map(flip(q));
        let mapped: Vec<P> = pts.iter().map(|q| map(*q)).collect();
        s.poly(&mapped, 0.35, INK, false);
        s.text(
            [area.0 + area.2 / 2.0, area.1 - 4.0],
            3.0,
            "middle",
            if bottom { "BOTTOM (seen from below)" } else { "TOP" },
        );
        let (fab, crt) =
            if bottom { (Layer::BFab, Layer::BCrtYd) } else { (Layer::FFab, Layer::FCrtYd) };
        for &k in &placed {
            let part = &layout.parts[k];
            if part.place.is_some_and(|pl| pl.back) != bottom {
                continue;
            }
            let dnp = part.dnp();
            let colour = if dnp { FAINT } else { INK };
            let gs = layout.graphics(k);
            let outline_layer = if gs.iter().any(|g| layer_of(g) == fab) { fab } else { crt };
            for g in gs.iter().filter(|g| layer_of(g) == outline_layer) {
                match g {
                    Graphic::Line { a, b, .. } => {
                        if dnp {
                            s.poly(&[map(p(*a)), map(p(*b))], 0.2, colour, true);
                        } else {
                            s.line(map(p(*a)), map(p(*b)), 0.2, colour);
                        }
                    }
                    Graphic::Poly { points, .. } => {
                        let c: Vec<P> = points.iter().map(|q| map(p(*q))).collect();
                        s.poly(&c, 0.2, colour, dnp);
                    }
                    Graphic::Circle { center, radius, .. } => {
                        let c = map(p(*center));
                        let dash = if dnp { r#" stroke-dasharray="0.8 0.5""# } else { "" };
                        let _ = writeln!(
                            s.s,
                            r#"<circle cx="{:.3}" cy="{:.3}" r="{:.3}" fill="none" stroke="{colour}" stroke-width="0.2"{dash}/>"#,
                            c[0],
                            c[1],
                            radius.mm() * view.scale
                        );
                    }
                    Graphic::Text { .. } => {}
                }
            }
            // Its pads, faintly; pin 1 marked; the reference in the middle.
            for pd in layout.pads_of(k) {
                let c: Vec<P> = crate::geometry::pad(pd, 0.0).into_iter().map(map).collect();
                s.poly(&c, 0.1, FAINT, false);
            }
            if let Some(pin1) = layout.pads_of(k).find(|pd| pd.number == "1") {
                let c = map(pin1.center);
                let _ = writeln!(
                    s.s,
                    r#"<circle cx="{:.3}" cy="{:.3}" r="0.45" fill="{colour}"/>"#,
                    c[0], c[1]
                );
            }
            let at = map(part.place.map_or([0.0, 0.0], |pl| pl.at));
            let size = (1.2 * view.scale).clamp(1.6, 3.0);
            let label =
                if dnp { format!("{} DNP", part.reference) } else { part.reference.clone() };
            let _ = writeln!(
                s.s,
                r#"<text x="{:.3}" y="{:.3}" font-size="{size:.2}" text-anchor="middle" fill="{colour}" font-weight="bold">{}</text>"#,
                at[0],
                at[1] + size * 0.35,
                esc(&label)
            );
        }
    }
    // The bill of materials beside it.
    let x = AREA.0 + AREA.2 + 8.0;
    s.text([x, 18.0], 3.0, "start", "BILL OF MATERIALS");
    let csv = crate::fab::bom(layout);
    let rows: Vec<Vec<String>> = csv
        .lines()
        .skip(1)
        .map(|l| {
            let f = split_csv(l);
            let get = |i: usize| f.get(i).cloned().unwrap_or_default();
            let refs = get(2);
            let refs = if refs.len() > 18 {
                format!("{}…", &refs[..refs.char_indices().nth(17).map_or(refs.len(), |c| c.0)])
            } else {
                refs
            };
            vec![get(1), refs, get(3), if get(8).is_empty() { String::new() } else { "DNP".into() }]
        })
        .collect();
    s.table([x, 20.0], &[9.0, 26.0, 16.0, 9.0], &["Qty", "Reference", "Value", ""], &rows);
    s.text(
        [MARGIN + 4.0, AREA.1 + AREA.3 + 16.0],
        2.4,
        "start",
        "Parts drawn dashed and marked DNP are left off. A dot marks pin 1.",
    );
    s.title(&[
        ("Assembly", project.to_owned()),
        ("Parts", format!("{} placed", placed.iter().filter(|&&k| !layout.parts[k].dnp()).count())),
        ("Sides", if back { "Both".into() } else { "Top only".into() }),
        ("Scale", scale_label),
        ("Drawn by", concat!("Circuiter ", env!("CARGO_PKG_VERSION")).to_owned()),
    ]);
    s.finish()
}

fn layer_of(g: &Graphic) -> Layer {
    match g {
        Graphic::Line { layer, .. }
        | Graphic::Circle { layer, .. }
        | Graphic::Poly { layer, .. }
        | Graphic::Text { layer, .. } => *layer,
    }
}

/// A CSV line's fields, quotes taken off.
fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out
}
