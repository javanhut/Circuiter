//! What a board house needs: a Gerber file per layer (RS-274X with X2 attributes, in
//! millimetres with six decimals, Y up as Gerber has it), Excellon drill files for
//! plated and unplated holes, a placement file for the assembly machine, and a bill of
//! materials. File names follow KiCad's (`board-F_Cu.gbr`, `board-PTH.drl`…).

use crate::drc;
use crate::font;
use crate::geometry::{self, Contour};
use crate::layout::{Layout, P, PadAt, p};
use crate::zones::ZoneFill;
use circuiter_model::board::{Graphic, Layer, PadKind, PadShape};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

const SOFTWARE: &str = concat!("Circuiter,Circuiter,", env!("CARGO_PKG_VERSION"));

/// The width of the line drawing the board's edge.
const EDGE_WIDTH: f64 = 0.1;

/// A file to hand over: its name and contents.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub name: String,
    pub contents: String,
}

/// Gerber's fields can't hold these.
fn field(s: &str) -> String {
    s.chars().map(|c| if matches!(c, ',' | '*' | '%' | '\n' | '\r') { '_' } else { c }).collect()
}

fn coord(v: f64) -> i64 {
    (v * 1e6).round() as i64
}

/// A Gerber file being written: apertures collected as they're needed, then the
/// drawing that uses them.
struct Gerber {
    apertures: Vec<(String, Option<&'static str>)>,
    codes: BTreeMap<(String, Option<&'static str>), usize>,
    body: String,
    current: Option<usize>,
}

impl Gerber {
    fn new() -> Gerber {
        Gerber { apertures: Vec::new(), codes: BTreeMap::new(), body: String::new(), current: None }
    }

    /// The D code of an aperture (`C,0.800000`), defining it on first use.
    fn aperture(&mut self, def: String, function: Option<&'static str>) -> usize {
        let key = (def, function);
        if let Some(&d) = self.codes.get(&key) {
            return d;
        }
        let d = 10 + self.apertures.len();
        self.apertures.push(key.clone());
        self.codes.insert(key, d);
        d
    }

    fn select(&mut self, d: usize) {
        if self.current != Some(d) {
            let _ = writeln!(self.body, "D{d}*");
            self.current = Some(d);
        }
    }

    fn xy(&mut self, v: P, op: &str) {
        let _ = writeln!(self.body, "X{}Y{}{op}*", coord(v[0]), coord(-v[1]));
    }

    fn attributes(&mut self, attrs: &[(&str, String)]) {
        for (k, v) in attrs {
            let _ = writeln!(self.body, "%TO.{k},{}*%", v);
        }
    }

    fn clear_attributes(&mut self, attrs: &[(&str, String)]) {
        if !attrs.is_empty() {
            self.body.push_str("%TD*%\n");
        }
    }

    fn flash(&mut self, d: usize, at: P) {
        self.select(d);
        self.xy(at, "D03");
    }

    /// A line through `points` with a round pen `width` wide.
    fn stroke(&mut self, width: f64, points: &[P], function: Option<&'static str>) {
        if points.len() < 2 {
            return;
        }
        let d = self.aperture(format!("C,{width:.6}"), function);
        self.select(d);
        self.xy(points[0], "D02");
        for &q in &points[1..] {
            self.xy(q, "D01");
        }
    }

    fn circle(&mut self, center: P, r: f64, width: f64) {
        let d = self.aperture(format!("C,{width:.6}"), None);
        self.select(d);
        let start = [center[0] + r, center[1]];
        self.xy(start, "D02");
        let _ = writeln!(
            self.body,
            "G75*\nG03*\nX{}Y{}I{}J0D01*\nG01*",
            coord(start[0]),
            coord(-start[1]),
            coord(-r)
        );
    }

    /// A filled outline, dark or (for a hole in one) clear.
    fn region(&mut self, contour: &Contour, dark: bool) {
        if contour.len() < 3 {
            return;
        }
        if !dark {
            self.body.push_str("%LPC*%\n");
        }
        self.body.push_str("G36*\n");
        self.xy(contour[0], "D02");
        for &q in &contour[1..] {
            self.xy(q, "D01");
        }
        self.xy(contour[0], "D01");
        self.body.push_str("G37*\n");
        if !dark {
            self.body.push_str("%LPD*%\n");
        }
    }

    fn finish(self, function: &str, negative: bool) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "%TF.GenerationSoftware,{SOFTWARE}*%");
        out.push_str("%TF.SameCoordinates,Original*%\n");
        let _ = writeln!(out, "%TF.FileFunction,{function}*%");
        let _ =
            writeln!(out, "%TF.FilePolarity,{}*%", if negative { "Negative" } else { "Positive" });
        out.push_str("%FSLAX46Y46*%\n");
        out.push_str("G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*\n");
        out.push_str("%MOMM*%\n%LPD*%\nG01*\nG04 APERTURE LIST*\n");
        for (k, (def, function)) in self.apertures.iter().enumerate() {
            if let Some(f) = function {
                let _ = writeln!(out, "%TA.AperFunction,{f}*%");
            }
            let (shape, size) = def.split_once(',').unwrap_or((def, ""));
            let _ = writeln!(out, "%ADD{}{shape},{size}*%", 10 + k);
            if function.is_some() {
                out.push_str("%TD*%\n");
            }
        }
        out.push_str("G04 APERTURE END LIST*\n");
        out.push_str(&self.body);
        out.push_str("M02*\n");
        out
    }
}

/// A pad, flashed with an aperture when its shape allows one, else as a region.
fn pad(
    g: &mut Gerber,
    layout: &Layout,
    pad: &PadAt,
    grow: f64,
    function: Option<&'static str>,
    attributes: bool,
) {
    let attrs: Vec<(&str, String)> = if attributes {
        let part = &layout.parts[pad.part];
        let mut a = vec![("P", format!("{},{}", field(&part.reference), field(&pad.number)))];
        if let Some(net) = &pad.net {
            a.push(("N", field(net)));
        }
        a.push(("C", field(&part.reference)));
        a
    } else {
        Vec::new()
    };
    g.attributes(&attrs);
    let (w, h) = (pad.size.0 + 2.0 * grow, pad.size.1 + 2.0 * grow);
    let quarter = (pad.rotation / 90.0).round();
    let square = (pad.rotation - quarter * 90.0).abs() < 1e-6;
    let (w, h) = if square && quarter.rem_euclid(2.0) == 1.0 { (h, w) } else { (w, h) };
    let def = match pad.shape {
        PadShape::Circle => Some(format!("C,{:.6}", w.max(h))),
        PadShape::Rect if square => Some(format!("R,{w:.6}X{h:.6}")),
        PadShape::Oval if square => Some(format!("O,{w:.6}X{h:.6}")),
        _ => None,
    };
    match def {
        Some(def) => {
            let d = g.aperture(def, function);
            g.flash(d, pad.center);
        }
        None => {
            g.region(&geometry::pad(pad, grow), true);
        }
    }
    g.clear_attributes(&attrs);
}

/// A drawing on a non-copper layer.
fn graphic(g: &mut Gerber, item: &Graphic, function: Option<&'static str>) {
    match item {
        Graphic::Line { a, b, width, .. } => g.stroke(width.mm(), &[p(*a), p(*b)], function),
        Graphic::Circle { center, radius, width, .. } => {
            g.circle(p(*center), radius.mm(), width.mm())
        }
        Graphic::Poly { points, width, filled, .. } => {
            let pts: Vec<P> = points.iter().map(|q| p(*q)).collect();
            if *filled {
                g.region(&pts, true);
            } else if let Some(&first) = pts.first() {
                let mut closed = pts.clone();
                closed.push(first);
                g.stroke(width.mm(), &closed, function);
            }
        }
        Graphic::Text { layer, at, text, size } => {
            let mirror = matches!(layer, Layer::BSilk | Layer::BFab);
            for line in font::strokes(text, p(*at), size.mm(), 0.0, mirror) {
                g.stroke(font::stroke(size.mm()), &line, function);
            }
        }
    }
}

fn graphic_layer(item: &Graphic) -> Layer {
    match item {
        Graphic::Line { layer, .. }
        | Graphic::Circle { layer, .. }
        | Graphic::Poly { layer, .. }
        | Graphic::Text { layer, .. } => *layer,
    }
}

/// What a panel adds to its copies' files: fiducials (copper dots in wider mask
/// openings), unplated holes (mouse bites, tooling) and V-score lines.
#[derive(Debug, Clone, Default)]
struct Extras {
    fiducials: Vec<P>,
    holes: Vec<(P, f64)>,
    vscores: Vec<(P, P)>,
}

/// A fiducial's copper dot and the opening round it.
const FIDUCIAL: f64 = 1.0;
const FIDUCIAL_MASK: f64 = 2.0;

fn copper(
    layout: &Layout,
    fills: &[ZoneFill],
    layer: Layer,
    index: usize,
    extras: &Extras,
) -> Output {
    let mut g = Gerber::new();
    if layer == Layer::FCu && !extras.fiducials.is_empty() {
        let d = g.aperture(format!("C,{FIDUCIAL:.6}"), Some("FiducialPad,Global"));
        for f in &extras.fiducials {
            g.flash(d, *f);
        }
    }
    // Pours first, largest first so that islands inside another's holes come after it.
    let mut pieces: Vec<&geometry::Shape> =
        fills.iter().filter(|f| f.layer == layer).flat_map(|f| f.shapes.iter()).collect();
    pieces.sort_by(|a, b| {
        geometry::area(&vec![(*b).clone()]).total_cmp(&geometry::area(&vec![(*a).clone()]))
    });
    for piece in pieces {
        if let Some((outer, holes)) = piece.split_first() {
            g.region(outer, true);
            for hole in holes {
                g.region(hole, false);
            }
        }
    }
    for t in crate::teardrops::shapes(layout).iter().filter(|t| t.layer == layer) {
        g.region(&t.outline, true);
    }
    for t in layout.board.tracks.iter().filter(|t| t.layer == layer) {
        let attrs = [("N", field(&t.net))];
        g.attributes(&attrs);
        g.stroke(t.width.mm(), &[p(t.a), p(t.b)], Some("Conductor"));
        g.clear_attributes(&attrs);
    }
    for v in &layout.board.vias {
        let attrs = [("N", field(&v.net))];
        g.attributes(&attrs);
        let d = g.aperture(format!("C,{:.6}", v.diameter.mm()), Some("ViaPad"));
        g.flash(d, p(v.at));
        g.clear_attributes(&attrs);
    }
    for pd in layout.pads.iter().filter(|pd| pd.layers.contains(&layer)) {
        let function =
            if matches!(pd.kind, PadKind::Smd) { "SMDPad,CuDef" } else { "ComponentPad" };
        pad(&mut g, layout, pd, 0.0, Some(function), true);
    }
    let side = match layer {
        Layer::FCu => "Top",
        Layer::BCu => "Bot",
        _ => "Inr",
    };
    Output {
        name: layer.name().replace('.', "_"),
        contents: g.finish(&format!("Copper,L{},{side}", index + 1), false),
    }
}

/// Every file for the board house, named after `project`.
pub fn outputs(layout: &Layout, fills: &[ZoneFill], project: &str) -> Vec<Output> {
    let mut out = outputs_with(layout, fills, project, &Extras::default());
    out.push(Output { name: format!("{project}-bom.csv"), contents: bom(layout) });
    out.push(Output {
        name: format!("{project}-fab.svg"),
        contents: crate::drawing::fabrication(layout, project),
    });
    out.push(Output {
        name: format!("{project}-assembly.svg"),
        contents: crate::drawing::assembly(layout, project),
    });
    out
}

/// The files for a panel, named `<project>-panel-…`: the copies' copper, mask, paste,
/// legend and drills, the panel's edge with what's routed out, its V-scores, and every
/// copy's parts in the placement file. The bill of materials stays the board's.
pub fn panel_outputs(panel: &crate::panel::Panelized, project: &str) -> Vec<Output> {
    let extras = Extras {
        fiducials: panel.fiducials.clone(),
        holes: panel.holes.clone(),
        vscores: panel.vscores.clone(),
    };
    outputs_with(&panel.layout, &panel.fills, &format!("{project}-panel"), &extras)
}

fn outputs_with(
    layout: &Layout,
    fills: &[ZoneFill],
    project: &str,
    extras: &Extras,
) -> Vec<Output> {
    let board = &layout.board;
    let mut out = Vec::new();
    for (k, layer) in board.copper().into_iter().enumerate() {
        out.push(copper(layout, fills, layer, k, extras));
    }
    let graphics: Vec<Graphic> = (0..layout.parts.len())
        .flat_map(|k| layout.graphics(k))
        .chain(board.graphics.iter().cloned())
        .collect();
    for (front, side) in [(true, "Top"), (false, "Bot")] {
        let cu = if front { Layer::FCu } else { Layer::BCu };
        // Openings in the mask: every pad on this side; vias stay covered.
        let mut mask = Gerber::new();
        for pd in layout
            .pads
            .iter()
            .filter(|pd| pd.layers.contains(&cu) || matches!(pd.kind, PadKind::Hole { .. }))
        {
            pad(&mut mask, layout, pd, 0.0, None, false);
        }
        if front && !extras.fiducials.is_empty() {
            let d = mask.aperture(format!("C,{FIDUCIAL_MASK:.6}"), None);
            for f in &extras.fiducials {
                mask.flash(d, *f);
            }
        }
        let mask_layer = if front { Layer::FMask } else { Layer::BMask };
        out.push(Output {
            name: mask_layer.name().replace('.', "_"),
            contents: mask.finish(&format!("Soldermask,{side}"), true),
        });
        let mut paste = Gerber::new();
        for pd in layout
            .pads
            .iter()
            .filter(|pd| matches!(pd.kind, PadKind::Smd) && pd.layers.contains(&cu))
        {
            pad(&mut paste, layout, pd, 0.0, None, false);
        }
        let paste_layer = if front { Layer::FPaste } else { Layer::BPaste };
        out.push(Output {
            name: paste_layer.name().replace('.', "_"),
            contents: paste.finish(&format!("Paste,{side}"), false),
        });
        let silk_layer = if front { Layer::FSilk } else { Layer::BSilk };
        let mut silk = Gerber::new();
        for item in graphics.iter().filter(|g| graphic_layer(g) == silk_layer) {
            graphic(&mut silk, item, None);
        }
        out.push(Output {
            name: silk_layer.name().replace('.', "_"),
            contents: silk.finish(&format!("Legend,{side}"), false),
        });
    }
    let mut edge = Gerber::new();
    let mut outline: Vec<P> = board.edge().iter().map(|q| p(*q)).collect();
    if let Some(&first) = outline.first() {
        outline.push(first);
    }
    edge.stroke(EDGE_WIDTH, &outline, Some("Profile"));
    for item in board.graphics.iter().filter(|g| graphic_layer(g) == Layer::EdgeCuts) {
        graphic(&mut edge, item, Some("Profile"));
    }
    out.push(Output { name: "Edge_Cuts".into(), contents: edge.finish("Profile,NP", false) });
    if !extras.vscores.is_empty() {
        let mut v = Gerber::new();
        for (a, b) in &extras.vscores {
            v.stroke(EDGE_WIDTH, &[*a, *b], None);
        }
        out.push(Output { name: "V_Score".into(), contents: v.finish("Vcut", false) });
    }
    for o in &mut out {
        o.name = format!("{project}-{}.gbr", o.name);
    }
    out.push(Output {
        name: format!("{project}-PTH.drl"),
        contents: drill_with(layout, true, &[]),
    });
    out.push(Output {
        name: format!("{project}-NPTH.drl"),
        contents: drill_with(layout, false, &extras.holes),
    });
    out.push(Output { name: format!("{project}-pos.csv"), contents: positions(layout) });
    out.push(Output { name: format!("{project}-ipc356.ipc"), contents: ipc356(layout, project) });
    out
}

/// A drill coordinate: millimetres, as short as exact.
fn num(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

/// An Excellon drill file: the plated holes (pads and vias) or the unplated ones.
pub fn drill(layout: &Layout, plated: bool) -> String {
    drill_with(layout, plated, &[])
}

/// [`drill`] with more holes (a panel's).
fn drill_with(layout: &Layout, plated: bool, more: &[(P, f64)]) -> String {
    let copper = layout.board.copper().len();
    // Holes by tool: (diameter in µm, is a via) → positions.
    let mut tools: BTreeMap<(i64, bool), Vec<P>> = BTreeMap::new();
    for pd in &layout.pads {
        match pd.kind {
            PadKind::Through { drill } if plated => {
                tools.entry(((drill.mm() * 1e3).round() as i64, false)).or_default().push(pd.center)
            }
            PadKind::Hole { drill } if !plated => {
                tools.entry(((drill.mm() * 1e3).round() as i64, false)).or_default().push(pd.center)
            }
            _ => {}
        }
    }
    for (at, d) in more {
        tools.entry(((d * 1e3).round() as i64, false)).or_default().push(*at);
    }
    if plated {
        for v in &layout.board.vias {
            tools.entry(((v.drill.mm() * 1e3).round() as i64, true)).or_default().push(p(v.at));
        }
    }
    let (kind, short) = if plated { ("Plated", "PTH") } else { ("NonPlated", "NPTH") };
    let mut out = String::new();
    out.push_str("M48\n");
    let _ = writeln!(out, "; DRILL file {{Circuiter {}}}", env!("CARGO_PKG_VERSION"));
    out.push_str("; FORMAT={-:-/ absolute / metric / decimal}\n");
    let _ = writeln!(out, "; #@! TF.GenerationSoftware,{SOFTWARE}");
    let _ = writeln!(out, "; #@! TF.FileFunction,{kind},1,{copper},{short}");
    out.push_str("FMAT,2\nMETRIC\n");
    for (k, ((um, via), _)) in tools.iter().enumerate() {
        let function = if *via { "ViaDrill" } else { "ComponentDrill" };
        let _ = writeln!(out, "; #@! TA.AperFunction,{kind},{short},{function}");
        let _ = writeln!(out, "T{}C{:.3}", k + 1, *um as f64 / 1e3);
    }
    out.push_str("%\nG90\nG05\n");
    for (k, (_, holes)) in tools.iter().enumerate() {
        let _ = writeln!(out, "T{}", k + 1);
        for h in holes {
            let _ = writeln!(out, "X{}Y{}", num(h[0]), num(-h[1]));
        }
    }
    out.push_str("T0\nM30\n");
    out
}

fn csv(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Reference designators in human order: R2 before R10.
pub fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let split = |s: &str| {
        let digits = s.trim_end_matches(|c: char| c.is_ascii_digit());
        (digits.to_owned(), s[digits.len()..].parse::<u64>().unwrap_or(0))
    };
    split(a).cmp(&split(b)).then_with(|| a.cmp(b))
}

/// Where the assembly machine puts each part: KiCad's CSV, Y up. Parts marked DNP
/// (do not populate) are left out.
pub fn positions(layout: &Layout) -> String {
    let mut out = String::from("Ref,Val,Package,PosX,PosY,Rot,Side\n");
    let mut placed: Vec<usize> = (0..layout.parts.len())
        .filter(|&k| layout.parts[k].place.is_some() && !layout.parts[k].dnp())
        .collect();
    placed.sort_by(|&a, &b| natural(&layout.parts[a].reference, &layout.parts[b].reference));
    for k in placed {
        let part = &layout.parts[k];
        let place = part.place.unwrap();
        let _ = writeln!(
            out,
            "{},{},{},{:.4},{:.4},{:.4},{}",
            csv(&part.reference),
            csv(&part.value),
            csv(&part.footprint.name),
            place.at[0],
            -place.at[1],
            place.rotation,
            if place.back { "bottom" } else { "top" }
        );
    }
    out
}

/// The bill of materials: one line per kind of part (same part, value, footprint and
/// manufacturer's number), with its references; DNP parts on lines of their own.
pub fn bom(layout: &Layout) -> String {
    type Key = (String, String, String, String, String, bool);
    let mut groups: BTreeMap<Key, Vec<String>> = BTreeMap::new();
    for part in &layout.parts {
        let field = |k: &str| part.fields.get(k).cloned().unwrap_or_default();
        let key = (
            part.cell.to_string(),
            part.value.clone(),
            part.footprint.name.clone(),
            field("MPN"),
            field("Manufacturer"),
            part.dnp(),
        );
        groups.entry(key).or_default().push(part.reference.clone());
    }
    let mut rows: Vec<_> = groups.into_iter().collect();
    for (_, refs) in &mut rows {
        refs.sort_by(|a, b| natural(a, b));
    }
    rows.sort_by(|a, b| natural(&a.1[0], &b.1[0]));
    let mut out = String::from("Item,Qty,Reference(s),Value,Footprint,Part,MPN,Manufacturer,DNP\n");
    for (n, ((cell, value, footprint, mpn, maker, dnp), refs)) in rows.iter().enumerate() {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{}",
            n + 1,
            refs.len(),
            csv(&refs.join(", ")),
            csv(value),
            csv(footprint),
            csv(cell),
            csv(mpn),
            csv(maker),
            if *dnp { "DNP" } else { "" }
        );
    }
    out
}

/// The netlist for the bare-board electrical test (IPC-D-356A, metric): every pad and
/// via, where it is, its hole and size, and which net it's on, so the tester can check
/// each net is joined and no two are. Fixed columns; nets with names too long for
/// their 14 are given aliases first.
pub fn ipc356(layout: &Layout, project: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "C  IPC-D-356A netlist from Circuiter {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "C  Board: {project}");
    out.push_str("P  JOB   ");
    out.push_str(&project.chars().take(60).collect::<String>());
    out.push('\n');
    out.push_str(
        "P  CODE  00\nP  UNITS CUST 1\nP  DIM   N\nP  VER   IPC-D-356A\nP  IMAGE PRIMARY\n",
    );
    // Long names get aliases.
    let mut alias: BTreeMap<String, String> = BTreeMap::new();
    let mut names: Vec<&String> = layout.nets.iter().collect();
    names.sort();
    for n in names {
        if n.len() > 14 || n.contains(' ') {
            let a = format!("NNAME{}", alias.len() + 1);
            let _ = writeln!(out, "P  {a:<8}{n}");
            alias.insert(n.clone(), a);
        }
    }
    out.push_str("C\n");
    let net = |n: Option<&str>| -> String {
        match n {
            Some(n) => alias.get(n).cloned().unwrap_or_else(|| n.to_owned()),
            None => "N/C".into(),
        }
    };
    let xy = |v: P| {
        let (x, y) = ((v[0] * 1e3).round() as i64, (-v[1] * 1e3).round() as i64);
        let sign = |v: i64| if v < 0 { '-' } else { '+' };
        format!("X{}{:06}Y{}{:06}", sign(x), x.abs().min(999_999), sign(y), y.abs().min(999_999))
    };
    let size = |v: f64| ((v * 1e3).round() as i64).clamp(0, 9999);
    for pd in &layout.pads {
        let part = &layout.parts[pd.part];
        let (code, drill, access) = match pd.kind {
            PadKind::Through { drill } => {
                ("317", format!("D{:04}P", size(drill.mm())), "A00".to_owned())
            }
            PadKind::Hole { drill } => {
                ("367", format!("D{:04}U", size(drill.mm())), "A00".to_owned())
            }
            PadKind::Smd => {
                let back = pd.layers.contains(&Layer::BCu) && !pd.layers.contains(&Layer::FCu);
                ("327", "      ".to_owned(), if back { "A02".to_owned() } else { "A01".to_owned() })
            }
        };
        let rot = (pd.rotation.rem_euclid(360.0).round() as i64) % 360;
        let _ = writeln!(
            out,
            "{code}{:<14.14}   {:<6.6}-{:<4.4} {drill}{access}{}X{:04}Y{:04}R{rot:03} S0",
            net(pd.net.as_deref()),
            part.reference,
            pd.number,
            xy(pd.center),
            size(pd.size.0),
            size(pd.size.1),
        );
    }
    for v in &layout.board.vias {
        let _ = writeln!(
            out,
            "317{:<14.14}   {:<6.6}-{:<4.4}MD{:04}PA00{}X{:04}Y0000R000 S3",
            net(Some(&v.net)),
            "VIA",
            "",
            size(v.drill.mm()),
            xy(p(v.at)),
            size(v.diameter.mm()),
        );
    }
    out.push_str("999\n");
    out
}

/// Writes `outputs` into `dir`.
pub fn write(dir: &Path, outputs: &[Output]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for o in outputs {
        std::fs::write(dir.join(&o.name), &o.contents)?;
    }
    Ok(())
}

/// Checks the board, then makes its files: refused while it has errors, unless `force`.
pub fn plot(
    layout: &Layout,
    project: &str,
    force: bool,
) -> Result<Vec<Output>, Vec<drc::Violation>> {
    let checked = drc::check(layout);
    let errors: Vec<_> = checked
        .violations
        .iter()
        .filter(|v| v.severity() == drc::Severity::Error)
        .cloned()
        .collect();
    if !errors.is_empty() && !force {
        return Err(errors);
    }
    Ok(outputs(layout, &checked.fills, project))
}
