//! The board editor, after KiCad's: a cell's printed circuit board drawn layer by layer;
//! parts placed, moved, turned, flipped and locked; tracks routed at 45° from pad to
//! pad with vias to change layer; copper pours; the ratsnest of what's left to route;
//! design rules checked as you go; and the files for the board house.

use super::CircuiterApp;
use crate::canvas::Camera;
use crate::style::{self, icon};
use circuiter_model::board::{Board, Layer, PadConnection, Placed, Track, Via, Zone};
use circuiter_model::geom::Length;
use circuiter_model::{CellId, Design};
use circuiter_pcb::drc::{self, Checked, Severity};
use circuiter_pcb::geometry::{self, Contour};
use circuiter_pcb::layout::{Layout, P, PadAt, p, point};
use circuiter_pcb::{fab, font, sync};
use circuiter_scene::Pt;
use eframe::egui::{self, Color32, Pos2, RichText, Sense, Stroke};

/// Grid steps on offer, in millimetres.
pub const GRIDS: &[f64] = &[0.1, 0.25, 0.5, 0.635, 1.0, 1.27, 2.54];

const BACKGROUND: Color32 = Color32::from_rgb(0, 16, 35);
const BOARD_FILL: Color32 = Color32::from_rgb(10, 34, 40);
const EDGE: Color32 = Color32::from_rgb(208, 210, 205);
const SILK_FRONT: Color32 = Color32::from_rgb(242, 237, 161);
const SILK_BACK: Color32 = Color32::from_rgb(232, 178, 167);
const RATSNEST: Color32 = Color32::from_rgb(230, 230, 230);
const HOLE: Color32 = Color32::from_rgb(0, 16, 35);
const VIA: Color32 = Color32::from_rgb(194, 194, 194);
const SELECTED: Color32 = Color32::WHITE;

/// A copper layer's colour, as KiCad shows it.
pub fn layer_colour(layer: Layer) -> Color32 {
    match layer {
        Layer::FCu => Color32::from_rgb(200, 52, 52),
        Layer::BCu => Color32::from_rgb(77, 127, 196),
        Layer::In(1) => Color32::from_rgb(127, 200, 127),
        Layer::In(_) => Color32::from_rgb(206, 125, 44),
        _ => Color32::GRAY,
    }
}

/// A change to the board, made once the frame's input is read.
type BoardEdit = Box<dyn FnOnce(&mut Board) -> Result<(), String>>;

/// What the editor is doing with the pointer.
#[derive(Debug, Clone, PartialEq)]
pub enum PcbTool {
    Select,
    /// Routing: nothing yet (`net` empty), or a track of `net` running from `points`'
    /// last point.
    Route {
        net: String,
        width: f64,
        points: Vec<P>,
    },
    /// A copper pour being outlined.
    Zone {
        points: Vec<P>,
    },
    /// The board's edge being drawn.
    Outline {
        points: Vec<P>,
    },
}

/// What to do with the tool in hand, for the status bar.
pub fn hint(tool: &PcbTool) -> &'static str {
    match tool {
        PcbTool::Select => {
            "Click to select · drag a part to move it · R turn · F flip to the other side · L lock · Del delete · X route · wheel zoom · right-drag pan"
        }
        PcbTool::Route { points, .. } if points.is_empty() => {
            "Click a pad, track or via to start a track of its net · Esc to stop"
        }
        PcbTool::Route { .. } => {
            "Click to lay a corner · click a pad of the net to finish · V via and change layer · / corner style · Backspace undo a leg · Esc stop"
        }
        PcbTool::Zone { .. } => {
            "Click the pour's corners · double-click or Enter to finish · Backspace removes a corner · Esc cancels"
        }
        PcbTool::Outline { .. } => {
            "Click the board's corners · double-click or Enter to finish · Esc cancels"
        }
    }
}

impl PcbTool {
    pub fn name(&self) -> &'static str {
        match self {
            PcbTool::Select => "Select",
            PcbTool::Route { .. } => "Route",
            PcbTool::Zone { .. } => "Zone",
            PcbTool::Outline { .. } => "Outline",
        }
    }
}

/// Something on the board.
#[derive(Debug, Clone, PartialEq)]
pub enum Pick {
    Footprint(String),
    Track(usize),
    Via(usize),
    Zone(usize),
}

/// A board worked out from the design: resolved, poured, checked.
pub struct Analysis {
    pub cell: CellId,
    pub revision: u64,
    pub layout: Result<Layout, String>,
    pub checked: Option<Checked>,
    /// Each pour piece as triangles in board millimetres, with its layer.
    fills: Vec<(Layer, Vec<P>, Vec<u32>)>,
    /// The board's face, as triangles.
    face: (Vec<P>, Vec<u32>),
}

impl Analysis {
    pub fn new(design: &Design, cell: &CellId, revision: u64) -> Analysis {
        let board = design.cell(cell).and_then(|d| d.board.clone());
        let layout = match &board {
            Some(b) => Layout::new(design, cell, b),
            None => Err("no board yet".into()),
        };
        let checked = layout.as_ref().ok().map(drc::check);
        let tris = |outer: &Contour, holes: &[Contour]| {
            let f = |c: &Contour| c.iter().map(|q| (q[0] as f32, q[1] as f32)).collect::<Vec<_>>();
            let holes: Vec<_> = holes.iter().map(f).collect();
            let (pts, tris) = circuiter_3d::triangulate_holes(&f(outer), &holes);
            let pts: Vec<P> = pts.iter().map(|q| [f64::from(q.0), f64::from(q.1)]).collect();
            (pts, tris.into_iter().flatten().collect::<Vec<u32>>())
        };
        let mut fills = Vec::new();
        if let Some(c) = &checked {
            for f in &c.fills {
                for piece in &f.shapes {
                    if let Some((outer, holes)) = piece.split_first() {
                        let (pts, idx) = tris(outer, holes);
                        fills.push((f.layer, pts, idx));
                    }
                }
            }
        }
        let face = match &board {
            Some(b) if b.outline.len() >= 3 => {
                tris(&b.outline.iter().map(|q| p(*q)).collect(), &[])
            }
            _ => (Vec::new(), Vec::new()),
        };
        Analysis { cell: cell.clone(), revision, layout, checked, fills, face }
    }
}

/// Which layers and markings show.
#[derive(Debug, Clone, Copy)]
pub struct Visible {
    pub front: bool,
    pub back: bool,
    pub zones: bool,
    pub silk: bool,
    pub ratsnest: bool,
    pub drc: bool,
}

impl Default for Visible {
    fn default() -> Self {
        Visible { front: true, back: true, zones: true, silk: true, ratsnest: true, drc: true }
    }
}

/// The board editor's state.
pub struct PcbView {
    pub camera: Option<Camera>,
    pub analysis: Option<Analysis>,
    pub tool: PcbTool,
    /// The copper layer being worked on.
    pub layer: Layer,
    pub selected: Option<Pick>,
    /// A footprint being dragged: where from and to.
    drag: Option<(String, P, P)>,
    pub visible: Visible,
    pub grid: f64,
    /// The net new pours are of.
    pub zone_net: String,
    /// Route corners: diagonal first, then straight (`/` swaps).
    pub diagonal_first: bool,
    /// A note for the status line (what was exported, why a route didn't start).
    pub note: Option<String>,
    cursor: Option<P>,
    /// Where the board was drawn last, on screen.
    pub rect: Option<egui::Rect>,
}

impl Default for PcbView {
    fn default() -> Self {
        PcbView {
            camera: None,
            analysis: None,
            tool: PcbTool::Select,
            layer: Layer::FCu,
            selected: None,
            drag: None,
            visible: Visible::default(),
            grid: 0.635,
            zone_net: String::new(),
            diagonal_first: false,
            note: None,
            cursor: None,
            rect: None,
        }
    }
}

impl PcbView {
    /// The analysis of `cell`'s board, recomputed when the design changed.
    pub fn update(&mut self, design: &Design, cell: &CellId, revision: u64) {
        let fresh =
            self.analysis.as_ref().is_some_and(|a| a.cell == *cell && a.revision == revision);
        if !fresh {
            if self.analysis.as_ref().is_some_and(|a| a.cell != *cell) {
                self.camera = None;
                self.selected = None;
                self.tool = PcbTool::Select;
            }
            self.analysis = Some(Analysis::new(design, cell, revision));
        }
    }

    pub fn layout(&self) -> Option<&Layout> {
        self.analysis.as_ref()?.layout.as_ref().ok()
    }

    pub fn snap(&self, v: P) -> P {
        let g = self.grid;
        [(v[0] / g).round() * g, (v[1] / g).round() * g]
    }
}

/// Two legs at 45°: straight then diagonal, or diagonal then straight.
pub fn posture(a: P, b: P, diagonal_first: bool) -> Vec<P> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let d = dx.abs().min(dy.abs());
    let diag = [d * dx.signum(), d * dy.signum()];
    let corner = if diagonal_first {
        [a[0] + diag[0], a[1] + diag[1]]
    } else {
        [b[0] - diag[0], b[1] - diag[1]]
    };
    let mut out = vec![a];
    let near = |u: P, v: P| (u[0] - v[0]).abs() < 1e-9 && (u[1] - v[1]).abs() < 1e-9;
    if !near(corner, a) && !near(corner, b) {
        out.push(corner);
    }
    if !near(b, a) {
        out.push(b);
    }
    out
}

fn inside(poly: &[P], q: P) -> bool {
    let mut odd = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > q[1]) != (b[1] > q[1])
            && q[0] < a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
        {
            odd = !odd;
        }
    }
    odd
}

/// The pad under `at`, if any: on `layer` when one is given.
pub fn pad_at(layout: &Layout, at: P, layer: Option<Layer>) -> Option<&PadAt> {
    layout.pads.iter().find(|pad| {
        layer.is_none_or(|l| pad.layers.contains(&l)) && inside(&geometry::pad(pad, 0.0), at)
    })
}

/// What's under `at`: tracks and vias first, then parts, then pours.
pub fn pick(layout: &Layout, at: P, tolerance: f64, layer: Layer) -> Option<Pick> {
    let board = &layout.board;
    let near_track = |t: &Track| {
        geometry::segment_distance(p(t.a), p(t.b), at, at) <= t.width.mm() / 2.0 + tolerance
    };
    for want in [true, false] {
        if let Some(k) =
            board.tracks.iter().position(|t| (t.layer == layer) == want && near_track(t))
        {
            return Some(Pick::Track(k));
        }
    }
    let via = board.vias.iter().position(|v| {
        let c = p(v.at);
        (c[0] - at[0]).hypot(c[1] - at[1]) <= v.diameter.mm() / 2.0 + tolerance
    });
    if let Some(k) = via {
        return Some(Pick::Via(k));
    }
    // The smallest part whose box holds the point.
    let mut best: Option<(f64, &str)> = None;
    for part in &layout.parts {
        let Some(place) = part.place else { continue };
        let (lo, hi) = sync::extent(&part.footprint);
        let q = place.unapply(at);
        if q[0] >= lo[0] && q[0] <= hi[0] && q[1] >= lo[1] && q[1] <= hi[1] {
            let area = (hi[0] - lo[0]) * (hi[1] - lo[1]);
            if best.is_none_or(|(a, _)| area < a) {
                best = Some((area, &part.path));
            }
        }
    }
    if let Some((_, path)) = best {
        return Some(Pick::Footprint(path.to_owned()));
    }
    board
        .zones
        .iter()
        .position(|z| inside(&z.outline.iter().map(|q| p(*q)).collect::<Vec<_>>(), at))
        .map(Pick::Zone)
}

/// The net of the copper at `at` on `layer` (a pad, track or via) and the point a route
/// should start from there.
fn copper_at(layout: &Layout, at: P, tolerance: f64, layer: Layer) -> Option<(Option<String>, P)> {
    if let Some(pad) = pad_at(layout, at, Some(layer)) {
        return Some((pad.net.clone(), pad.center));
    }
    let board = &layout.board;
    for v in &board.vias {
        let c = p(v.at);
        if (c[0] - at[0]).hypot(c[1] - at[1]) <= v.diameter.mm() / 2.0 + tolerance {
            return Some((Some(v.net.clone()), c));
        }
    }
    for t in board.tracks.iter().filter(|t| t.layer == layer) {
        let (a, b) = (p(t.a), p(t.b));
        if geometry::segment_distance(a, b, at, at) <= t.width.mm() / 2.0 + tolerance {
            // An end if near one, else the nearest point along it.
            let end =
                [a, b].into_iter().find(|e| (e[0] - at[0]).hypot(e[1] - at[1]) <= t.width.mm());
            let on = end.unwrap_or_else(|| {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l2 = (dx * dx + dy * dy).max(1e-12);
                let s = (((at[0] - a[0]) * dx + (at[1] - a[1]) * dy) / l2).clamp(0.0, 1.0);
                [a[0] + s * dx, a[1] + s * dy]
            });
            return Some((Some(t.net.clone()), on));
        }
    }
    None
}

/// The closest a route of `net`, `width` wide along `path` on `layer`, comes to copper
/// of other nets, less the clearance it needs: negative where it breaks the rule.
fn route_margin(layout: &Layout, net: &str, path: &[P], width: f64, layer: Layer) -> f64 {
    let rules = &layout.board.rules;
    let need = |other: &str| rules.clearance_for(net).mm().max(rules.clearance_for(other).mm());
    let mut worst = f64::INFINITY;
    for seg in path.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let lo = [a[0].min(b[0]) - 5.0, a[1].min(b[1]) - 5.0];
        let hi = [a[0].max(b[0]) + 5.0, a[1].max(b[1]) + 5.0];
        let near = |c: P| c[0] >= lo[0] && c[0] <= hi[0] && c[1] >= lo[1] && c[1] <= hi[1];
        for pad in layout.pads.iter().filter(|pd| {
            pd.layers.contains(&layer) && pd.net.as_deref() != Some(net) && near(pd.center)
        }) {
            let c = geometry::pad(pad, 0.0);
            let d = (0..c.len())
                .map(|k| geometry::segment_distance(a, b, c[k], c[(k + 1) % c.len()]))
                .fold(f64::INFINITY, f64::min);
            let d = if inside(&c, a) || inside(&c, b) { 0.0 } else { d };
            worst = worst.min(d - width / 2.0 - need(pad.net.as_deref().unwrap_or("")));
        }
        for t in layout.board.tracks.iter().filter(|t| t.layer == layer && t.net != net) {
            let d =
                geometry::segment_distance(a, b, p(t.a), p(t.b)) - width / 2.0 - t.width.mm() / 2.0;
            worst = worst.min(d - need(&t.net));
        }
        for v in layout.board.vias.iter().filter(|v| v.net != net) {
            let d = geometry::segment_distance(a, b, p(v.at), p(v.at))
                - width / 2.0
                - v.diameter.mm() / 2.0;
            worst = worst.min(d - need(&v.net));
        }
    }
    worst
}

/// Draws board millimetres on screen.
struct Draw<'a> {
    painter: &'a egui::Painter,
    cam: Camera,
    view: egui::Rect,
}

impl Draw<'_> {
    fn at(&self, v: P) -> Pos2 {
        self.cam.to_screen(self.view, Pt::new(v[0], v[1]))
    }

    fn px(&self, mm: f64) -> f32 {
        (mm as f32 * self.cam.scale).max(1.0)
    }

    fn convex(&self, c: &[P], fill: Color32) {
        let pts: Vec<Pos2> = c.iter().map(|&q| self.at(q)).collect();
        self.painter.add(egui::Shape::convex_polygon(pts, fill, Stroke::NONE));
    }

    fn outline(&self, c: &[P], stroke: Stroke) {
        let pts: Vec<Pos2> = c.iter().map(|&q| self.at(q)).collect();
        self.painter.add(egui::Shape::closed_line(pts, stroke));
    }

    fn mesh(&self, pts: &[P], idx: &[u32], fill: Color32) {
        let mut m = egui::Mesh::default();
        for &q in pts {
            m.colored_vertex(self.at(q), fill);
        }
        m.indices = idx.to_vec();
        self.painter.add(egui::Shape::mesh(m));
    }

    /// A round-ended stroke `width` millimetres wide.
    fn stroke(&self, pts: &[P], width: f64, colour: Color32) {
        let w = self.px(width);
        for seg in pts.windows(2) {
            self.painter.line_segment([self.at(seg[0]), self.at(seg[1])], Stroke::new(w, colour));
        }
        if w > 2.0 {
            for &q in pts {
                self.painter.circle_filled(self.at(q), w / 2.0, colour);
            }
        }
    }

    fn disc(&self, c: P, r: f64, fill: Color32) {
        self.painter.circle_filled(self.at(c), (r as f32 * self.cam.scale).max(0.5), fill);
    }
}

impl CircuiterApp {
    /// The board view: toolbar, then the board.
    pub(super) fn view_pcb(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &self.workspace else { return };
        let cell = ws.current().cell.clone();
        self.pcb.update(&ws.design, &cell, ws.revision);
        let has_schematic = ws.has_schematic();
        let editable = ws.editable();
        self.pcb_toolbar(ui);
        let p = style::palette(ui);
        let rect = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let has_board = self.pcb.analysis.as_ref().is_some_and(|a| a.layout.is_ok());
        if !has_board {
            ui.painter().rect_filled(rect, 0.0, BACKGROUND);
            let reason = self
                .pcb
                .analysis
                .as_ref()
                .and_then(|a| a.layout.as_ref().err().cloned())
                .unwrap_or_default();
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink(40.0))
                    .layout(egui::Layout::top_down(egui::Align::Center)),
            );
            child.add_space(rect.height() * 0.3);
            if reason == "no board yet" {
                child.label(
                    RichText::new(format!("{} has no board yet", cell.cell))
                        .size(18.0)
                        .color(p.text),
                );
                child.add_space(6.0);
                if has_schematic {
                    child.label(RichText::new("A board is made from the schematic: every part with a footprint is placed on it, joined by the ratsnest, ready to route.").color(p.weak));
                    child.add_space(10.0);
                    let make = egui::Button::new(
                        RichText::new(format!("{}  Create Board from Schematic", icon::SPARKLE))
                            .strong(),
                    );
                    if child.add_enabled(editable, make).clicked() {
                        self.pcb_create_board();
                    }
                } else {
                    child.label(
                        RichText::new("Boards are made from schematics: open a cell with one.")
                            .color(p.weak),
                    );
                }
            } else {
                child.label(
                    RichText::new(format!("Can't lay out the board: {reason}")).color(p.error),
                );
            }
            return;
        }
        self.pcb_interact(ui, rect, &response);
        self.pcb_paint(ui, rect);
    }

    /// Makes the cell a board from its schematic, every part placed.
    pub(super) fn pcb_create_board(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let made = ws.edit_board(|design, id, board| {
            *board = Some(sync::new_board(design, id)?);
            Ok(())
        });
        match made {
            Ok(()) => {
                self.pcb.camera = None;
                self.pcb.note =
                    Some("Board made: parts placed, ratsnest showing. Route with X.".into());
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Turns the case made to fit into two 3D parts of the project, base and lid, and
    /// uses them: they can then be changed in the body editor.
    pub(super) fn pcb_case_parts(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let cell = ws.current().cell.clone();
        let made = (|| -> Result<_, String> {
            let board = ws.current_def().and_then(|d| d.board.clone()).ok_or("no board")?;
            let e = board.enclosure.clone().ok_or("no case")?;
            let layout = Layout::new(&ws.design, &cell, &board)?;
            let built = circuiter_pcb::board3d::build(&ws.design, &layout, &[]);
            circuiter_pcb::enclosure::make(&layout, &built, &e)
        })();
        let made = match made {
            Ok(m) => m,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        let result = (|| -> Result<(CellId, CellId), String> {
            let base =
                ws.add_body_part(&format!("{}_case", cell.cell), "enclosures", made.base.clone())?;
            let lid =
                ws.add_body_part(&format!("{}_lid", cell.cell), "enclosures", made.lid.clone())?;
            ws.edit_board(|_, _, b| {
                let e = b.as_mut().and_then(|b| b.enclosure.as_mut()).ok_or("no case")?;
                e.part = Some(circuiter_model::CellRef::new(None, &base.cell));
                e.lid = Some(circuiter_model::CellRef::new(None, &lid.cell));
                e.board_at = [0.0; 3];
                Ok(())
            })?;
            Ok((base, lid))
        })();
        match result {
            Ok((base, lid)) => {
                self.pcb.note = Some(format!(
                    "The case is now {} and {}: open them to model",
                    base.cell, lid.cell
                ));
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Brings the board up to date with the schematic.
    pub(super) fn pcb_update_from_schematic(&mut self) {
        let Some(ws) = &mut self.workspace else { return };
        let done = ws.edit_board(|design, id, board| match board {
            Some(b) => sync::update(design, id, b, true),
            None => {
                *board = Some(sync::new_board(design, id)?);
                Ok(sync::Update::default())
            }
        });
        match done {
            Ok(u) if u.added.is_empty() && u.removed.is_empty() => {
                self.pcb.note = Some("The board matches the schematic".into())
            }
            Ok(u) => {
                let mut parts = Vec::new();
                if !u.added.is_empty() {
                    parts.push(format!("placed {}", u.added.join(", ")));
                }
                if !u.removed.is_empty() {
                    parts.push(format!("removed {}", u.removed.join(", ")));
                }
                self.pcb.note = Some(format!("Updated from the schematic: {}", parts.join("; ")));
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn pcb_toolbar(&mut self, ui: &mut egui::Ui) {
        let p = style::palette(ui);
        let mut action: Option<&'static str> = None;
        let editable = self.workspace.as_ref().is_some_and(|ws| ws.editable());
        let has_board = self.pcb.analysis.as_ref().is_some_and(|a| a.layout.is_ok());
        let nets: Vec<String> =
            self.pcb.layout().map(|l| l.nets.iter().cloned().collect()).unwrap_or_default();
        egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let view = &mut self.pcb;
                if ui.button(format!("{}  Fit", icon::CORNERS_OUT)).on_hover_text("Home, or double-click").clicked() {
                    view.camera = None;
                }
                ui.separator();
                let current = view.tool.name();
                let tool = |ui: &mut egui::Ui, glyph: &str, name: &str, tip: &str| {
                    style::icon_button(ui, glyph, tip, editable && has_board, current == name).clicked()
                };
                if tool(ui, icon::CURSOR, "Select", "Select and move  (Esc)\nR turn · F flip · L lock · Del delete") {
                    view.tool = PcbTool::Select;
                }
                if tool(ui, icon::PATH, "Route", "Route tracks  (X)\nClick a pad to start, click to add corners, click a pad of the same net to finish.\nV: via and change layer · /: corner style · Backspace: last leg · Esc: stop") {
                    view.tool = PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() };
                }
                if tool(ui, icon::POLYGON, "Zone", "Copper pour: click its corners, double-click to finish") {
                    view.tool = PcbTool::Zone { points: Vec::new() };
                }
                if tool(ui, icon::BOUNDING_BOX, "Outline", "Board edge: click its corners, double-click to finish") {
                    view.tool = PcbTool::Outline { points: Vec::new() };
                }
                ui.separator();
                let mut layer = usize::from(view.layer == Layer::BCu);
                if let Some(l) = style::segmented(ui, &[("F.Cu", true), ("B.Cu", true)], layer) {
                    layer = l;
                    view.layer = if layer == 0 { Layer::FCu } else { Layer::BCu };
                }
                if matches!(view.tool, PcbTool::Zone { .. }) {
                    if view.zone_net.is_empty() || !nets.contains(&view.zone_net) {
                        view.zone_net = nets.iter().find(|n| n.eq_ignore_ascii_case("gnd") || *n == "0").or(nets.first()).cloned().unwrap_or_default();
                    }
                    egui::ComboBox::from_id_salt("zone-net").selected_text(format!("Net: {}", view.zone_net)).show_ui(ui, |ui| {
                        for n in &nets {
                            ui.selectable_value(&mut view.zone_net, n.clone(), n);
                        }
                    });
                }
                egui::ComboBox::from_id_salt("pcb-grid").selected_text(format!("Grid {} mm", view.grid)).show_ui(ui, |ui| {
                    for &g in GRIDS {
                        ui.selectable_value(&mut view.grid, g, format!("{g} mm"));
                    }
                });
                ui.separator();
                let v = &mut view.visible;
                ui.menu_button(format!("{}  Layers", icon::STACK), |ui| {
                    ui.checkbox(&mut v.front, "Front copper (F.Cu)");
                    ui.checkbox(&mut v.back, "Back copper (B.Cu)");
                    ui.checkbox(&mut v.zones, "Copper pours");
                    ui.checkbox(&mut v.silk, "Silkscreen");
                    ui.checkbox(&mut v.ratsnest, "Ratsnest");
                    ui.checkbox(&mut v.drc, "Design rule markers");
                });
                ui.separator();
                if ui.add_enabled(editable && has_board, egui::Button::new(format!("{}  Update from Schematic", icon::ARROWS_CLOCKWISE)))
                    .on_hover_text("Place parts new to the schematic and take off those deleted from it")
                    .clicked()
                {
                    action = Some("update");
                }
                if ui.add_enabled(has_board, egui::Button::new(format!("{}  Fabrication Files…", icon::EXPORT)))
                    .on_hover_text("Gerbers, drill files, positions and bill of materials, for the board house")
                    .clicked()
                {
                    action = Some("fab");
                }
                if ui.add_enabled(has_board, egui::Button::new(format!("{}  3D", icon::CUBE))).on_hover_text("See the board with its parts in 3D").clicked() {
                    action = Some("3d");
                }
            });
        });
        match action {
            Some("update") => self.pcb_update_from_schematic(),
            Some("fab") => self.pcb_export(),
            Some("3d") => {
                self.show_pcb = false;
                self.show_3d = true;
                self.viewer3d.body_mode = false;
                self.viewer3d.board = true;
            }
            _ => {}
        }
    }

    /// Writes the fabrication files into a folder the user picks.
    pub(super) fn pcb_export(&mut self) {
        let Some(ws) = &self.workspace else { return };
        let Some(layout) = self.pcb.layout() else { return };
        let Some(checked) = self.pcb.analysis.as_ref().and_then(|a| a.checked.as_ref()) else {
            return;
        };
        let name = ws.current().cell.cell.clone();
        let default = ws.project.root.join("fab").join(&name);
        let dir = if cfg!(test) {
            Some(default)
        } else {
            rfd::FileDialog::new().set_directory(&ws.project.root).pick_folder()
        };
        let Some(dir) = dir else { return };
        let files = fab::outputs(layout, &checked.fills, &name);
        let mut svg_path = dir.join(format!("{name}.svg"));
        let svg = circuiter_pcb::svg::plot(layout, &checked.fills, Some(&checked.connectivity));
        let written = fab::write(&dir, &files).and_then(|()| std::fs::write(&svg_path, svg));
        if written.is_err() {
            svg_path = dir.clone();
        }
        match written {
            Ok(()) => {
                let errors = checked.errors();
                let warn = if errors > 0 {
                    format!(" — the board still breaks {errors} design rule(s)")
                } else {
                    String::new()
                };
                self.pcb.note =
                    Some(format!("Wrote {} files to {}{warn}", files.len() + 1, dir.display()));
            }
            Err(e) => self.error = Some(format!("writing {}: {e}", svg_path.display())),
        }
    }

    /// Pointer and keys over the board.
    fn pcb_interact(&mut self, ui: &mut egui::Ui, rect: egui::Rect, response: &egui::Response) {
        let Some(layout) = self.pcb.layout() else { return };
        // Fit to the board the first time and whenever asked.
        if self.pcb.camera.is_none() {
            let outline: Vec<P> = layout.board.outline.iter().map(|q| p(*q)).collect();
            let mut pts = outline.clone();
            for part in &layout.parts {
                if let Some(place) = part.place {
                    pts.push(place.at);
                }
            }
            let (lo, hi) = pts.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), q| {
                ([lo[0].min(q[0]), lo[1].min(q[1])], [hi[0].max(q[0]), hi[1].max(q[1])])
            });
            let bounds = if lo[0] <= hi[0] {
                circuiter_scene::Rect { min: Pt::new(lo[0], lo[1]), max: Pt::new(hi[0], hi[1]) }
            } else {
                circuiter_scene::Rect { min: Pt::new(0.0, 0.0), max: Pt::new(50.0, 40.0) }
            };
            let mut cam = Camera::fit(&bounds, rect);
            cam.fitted = None;
            self.pcb.camera = Some(cam);
        }
        let mut cam = self.pcb.camera.expect("fitted");
        // Navigation: wheel zooms about the pointer; middle or right drag pans.
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if let (true, Some(at)) = (scroll != 0.0, response.hover_pos()) {
                cam.zoom_about(rect, at, (scroll * 0.002).exp());
            }
        }
        let space = ui.input(|i| i.key_down(egui::Key::Space));
        if response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
            || (space && response.dragged_by(egui::PointerButton::Primary))
        {
            let d = response.drag_delta() / cam.scale;
            cam.center = Pt::new(cam.center.x - f64::from(d.x), cam.center.y - f64::from(d.y));
        }
        self.pcb.camera = Some(cam);
        self.pcb.rect = Some(rect);
        let world = |pos: Pos2| {
            let w = cam.to_world(rect, pos);
            [w.x, w.y]
        };
        let tolerance = f64::from(4.0 / cam.scale);
        let hover = response.hover_pos().map(world);
        self.pcb.cursor = hover;
        let editable = self.workspace.as_ref().is_some_and(|ws| ws.editable());
        let typing = ui.ctx().egui_wants_keyboard_input();
        let keys = |k: egui::Key| !typing && ui.input(|i| i.key_pressed(k));
        let (shift, cmd) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
        if keys(egui::Key::Home)
            || (response.double_clicked()
                && self.pcb.tool == PcbTool::Select
                && self.pcb.selected.is_none())
        {
            self.pcb.camera = None;
        }
        if !editable {
            return;
        }
        let layer = self.pcb.layer;
        let mut edit: Option<BoardEdit> = None;
        let layout = self.pcb.layout().expect("board").clone();
        match self.pcb.tool.clone() {
            PcbTool::Select => {
                if response.drag_started_by(egui::PointerButton::Primary) && !space {
                    // From where the button went down, not where the drag was noticed.
                    let origin =
                        ui.input(|i| i.pointer.press_origin()).or(response.interact_pointer_pos());
                    if let Some(from) = origin.map(world) {
                        let mut grabbed = None;
                        if let Some(Pick::Footprint(path)) = pick(&layout, from, tolerance, layer) {
                            if !layout.board.placed(&path).is_some_and(|f| f.locked) {
                                grabbed = Some(path.clone());
                            }
                            self.pcb.selected = Some(Pick::Footprint(path));
                        }
                        self.pcb.drag = grabbed.map(|g| (g, from, from));
                    }
                }
                if let (Some(drag), Some(to)) =
                    (&mut self.pcb.drag, response.interact_pointer_pos().map(world))
                {
                    drag.2 = to;
                }
                if response.drag_stopped() {
                    if let Some((path, from, to)) = self.pcb.drag.take() {
                        let grid = self.pcb.grid;
                        if (to[0] - from[0]).hypot(to[1] - from[1]) > 1e-6 {
                            edit = Some(Box::new(move |b: &mut Board| {
                                let f = b
                                    .footprints
                                    .iter_mut()
                                    .find(|f| f.path == path)
                                    .ok_or("gone")?;
                                let at = p(f.at);
                                let moved = [at[0] + to[0] - from[0], at[1] + to[1] - from[1]];
                                f.at = point([
                                    (moved[0] / grid).round() * grid,
                                    (moved[1] / grid).round() * grid,
                                ]);
                                Ok(())
                            }));
                        }
                    }
                }
                if response.clicked_by(egui::PointerButton::Primary) {
                    self.pcb.selected = hover.and_then(|at| pick(&layout, at, tolerance, layer));
                }
                // Keys act on the selection.
                match self.pcb.selected.clone() {
                    Some(Pick::Footprint(path)) => {
                        let toggle = |k| keys(k) && !cmd;
                        let (r, f, l) =
                            (toggle(egui::Key::R), toggle(egui::Key::F), toggle(egui::Key::L));
                        let del = keys(egui::Key::Delete);
                        if r || f || l || del {
                            edit = Some(Box::new(move |b: &mut Board| {
                                if del {
                                    b.footprints.retain(|x| x.path != path);
                                    return Ok(());
                                }
                                let fp = b
                                    .footprints
                                    .iter_mut()
                                    .find(|x| x.path == path)
                                    .ok_or("gone")?;
                                if l {
                                    fp.locked = !fp.locked;
                                } else if fp.locked {
                                    return Err(format!("{path} is locked (L to unlock)"));
                                }
                                if r {
                                    fp.rotation = (fp.rotation + if shift { -90.0 } else { 90.0 })
                                        .rem_euclid(360.0);
                                }
                                if f {
                                    fp.back = !fp.back;
                                }
                                Ok(())
                            }));
                            if del {
                                self.pcb.selected = None;
                            }
                        }
                    }
                    Some(pick @ (Pick::Track(_) | Pick::Via(_) | Pick::Zone(_)))
                        if keys(egui::Key::Delete) || keys(egui::Key::Backspace) =>
                    {
                        self.pcb.selected = None;
                        edit = Some(Box::new(move |b: &mut Board| {
                            match pick {
                                Pick::Track(k) if k < b.tracks.len() => drop(b.tracks.remove(k)),
                                Pick::Via(k) if k < b.vias.len() => drop(b.vias.remove(k)),
                                Pick::Zone(k) if k < b.zones.len() => drop(b.zones.remove(k)),
                                _ => {}
                            }
                            Ok(())
                        }));
                    }
                    _ => {}
                }
                if keys(egui::Key::X) && !cmd {
                    self.pcb.tool =
                        PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() };
                }
                if keys(egui::Key::Escape) {
                    self.pcb.selected = None;
                }
            }
            PcbTool::Route { net, width, points } => {
                let at = hover.map(|h| match copper_at(&layout, h, tolerance, layer) {
                    Some((Some(n), q)) if n == net => q,
                    _ => self.pcb.snap(h),
                });
                if keys(egui::Key::Slash) {
                    self.pcb.diagonal_first = !self.pcb.diagonal_first;
                }
                if keys(egui::Key::Escape) {
                    self.pcb.tool = if points.is_empty() {
                        PcbTool::Select
                    } else {
                        PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() }
                    };
                } else if points.is_empty() {
                    if response.clicked_by(egui::PointerButton::Primary) {
                        match hover.and_then(|h| copper_at(&layout, h, tolerance, layer)) {
                            Some((Some(n), start)) => {
                                let w = layout.board.rules.track_width_for(&n).mm();
                                self.pcb.note = Some(format!("Routing {n} on {}", layer.name()));
                                self.pcb.tool =
                                    PcbTool::Route { net: n, width: w, points: vec![start] };
                            }
                            Some((None, _)) => {
                                self.pcb.note = Some(
                                    "That pad isn't connected to anything in the schematic".into(),
                                )
                            }
                            None => {
                                self.pcb.note = Some(format!(
                                    "Start a route on a pad, track or via on {}",
                                    layer.name()
                                ))
                            }
                        }
                    }
                } else if let Some(to) = at {
                    let from = *points.last().expect("started");
                    let legs = posture(from, to, self.pcb.diagonal_first);
                    let via = keys(egui::Key::V);
                    let back = keys(egui::Key::Backspace);
                    if back {
                        // Take off the last leg laid.
                        let mut pts = points.clone();
                        pts.pop();
                        if pts.is_empty() {
                            self.pcb.tool = PcbTool::Route {
                                net: String::new(),
                                width: 0.0,
                                points: Vec::new(),
                            };
                        } else {
                            let undo = (pts.last().copied().expect("one"), from);
                            self.pcb.tool = PcbTool::Route { net: net.clone(), width, points: pts };
                            edit = Some(Box::new(move |b: &mut Board| {
                                if let Some(k) = b
                                    .tracks
                                    .iter()
                                    .rposition(|t| p(t.a) == undo.0 && p(t.b) == undo.1)
                                {
                                    b.tracks.remove(k);
                                }
                                Ok(())
                            }));
                        }
                    } else if response.clicked_by(egui::PointerButton::Primary)
                        || response.double_clicked()
                        || via
                    {
                        // Lay the legs; finish on copper of the net, or with a double-click.
                        let ends = copper_at(&layout, to, tolerance, layer)
                            .is_some_and(|(n, _)| n.as_deref() == Some(net.as_str()));
                        let finish = (ends && !via) || response.double_clicked();
                        let tracks: Vec<Track> = legs
                            .windows(2)
                            .map(|w| Track {
                                layer,
                                a: point(w[0]),
                                b: point(w[1]),
                                width: Length::from_mm(width),
                                net: net.clone(),
                            })
                            .collect();
                        let (dia, drill) = layout.board.rules.via_for(&net);
                        let new_via = via.then(|| Via {
                            at: point(to),
                            diameter: dia,
                            drill,
                            net: net.clone(),
                        });
                        let mut pts = points.clone();
                        pts.extend(legs.iter().skip(1));
                        if via {
                            let other = if layer == Layer::FCu { Layer::BCu } else { Layer::FCu };
                            self.pcb.layer = other;
                            self.pcb.note = Some(format!("Via: routing {net} on {}", other.name()));
                        }
                        self.pcb.tool = if finish {
                            self.pcb.note = Some(format!("Routed {net}"));
                            PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() }
                        } else {
                            PcbTool::Route { net: net.clone(), width, points: pts }
                        };
                        edit = Some(Box::new(move |b: &mut Board| {
                            b.tracks.extend(tracks);
                            b.vias.extend(new_via);
                            Ok(())
                        }));
                    }
                }
            }
            PcbTool::Zone { mut points } | PcbTool::Outline { mut points } => {
                let zone = matches!(self.pcb.tool, PcbTool::Zone { .. });
                let make = |points: Vec<P>| {
                    if zone { PcbTool::Zone { points } } else { PcbTool::Outline { points } }
                };
                if keys(egui::Key::Escape) {
                    self.pcb.tool =
                        if points.is_empty() { PcbTool::Select } else { make(Vec::new()) };
                } else if keys(egui::Key::Backspace) {
                    points.pop();
                    self.pcb.tool = make(points);
                } else if response.double_clicked() || keys(egui::Key::Enter) {
                    if points.len() >= 3 {
                        let outline: Vec<_> = points.iter().map(|&q| point(q)).collect();
                        let (net, layer) = (self.pcb.zone_net.clone(), self.pcb.layer);
                        edit = Some(if zone {
                            Box::new(move |b: &mut Board| {
                                if net.is_empty() {
                                    return Err("choose the pour's net first".into());
                                }
                                b.zones.push(Zone::new(layer, &net, outline));
                                Ok(())
                            })
                        } else {
                            Box::new(move |b: &mut Board| {
                                b.outline = outline;
                                Ok(())
                            })
                        });
                    }
                    self.pcb.tool = make(Vec::new());
                } else if response.clicked_by(egui::PointerButton::Primary) {
                    if let Some(h) = hover {
                        let q = self.pcb.snap(h);
                        if points.last() != Some(&q) {
                            points.push(q);
                        }
                        self.pcb.tool = make(points);
                    }
                }
            }
        }
        if keys(egui::Key::X)
            && !cmd
            && matches!(self.pcb.tool, PcbTool::Zone { .. } | PcbTool::Outline { .. })
        {
            self.pcb.tool = PcbTool::Route { net: String::new(), width: 0.0, points: Vec::new() };
        }
        // Page Up / Down change layer, as in KiCad.
        if keys(egui::Key::PageUp) {
            self.pcb.layer = Layer::FCu;
        }
        if keys(egui::Key::PageDown) {
            self.pcb.layer = Layer::BCu;
        }
        if let Some(f) = edit {
            let Some(ws) = &mut self.workspace else { return };
            let done = ws.edit_board(|_, _, board| match board {
                Some(b) => f(b),
                None => Err("no board".into()),
            });
            if let Err(e) = done {
                self.error = Some(e);
            }
        }
    }

    /// Draws the board, back layer first.
    fn pcb_paint(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let view = &self.pcb;
        let Some(analysis) = &view.analysis else { return };
        let Ok(layout) = &analysis.layout else { return };
        let Some(cam) = view.camera else { return };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, BACKGROUND);
        let d = Draw { painter: &painter, cam, view: rect };
        let board = &layout.board;
        d.mesh(&analysis.face.0, &analysis.face.1, BOARD_FILL);
        // Grid dots, when they're far enough apart to see.
        let step_px = view.grid as f32 * cam.scale;
        if step_px >= 8.0 {
            let lo = cam.to_world(rect, rect.left_top());
            let hi = cam.to_world(rect, rect.right_bottom());
            let g = view.grid;
            let dot = Color32::from_rgba_unmultiplied(160, 170, 190, 50);
            let mut y = (lo.y / g).floor() * g;
            while y <= hi.y {
                let mut x = (lo.x / g).floor() * g;
                while x <= hi.x {
                    painter.circle_filled(d.at([x, y]), 0.8, dot);
                    x += g;
                }
                y += g;
            }
        }
        let dragged = view
            .drag
            .as_ref()
            .map(|(path, from, to)| (layout.part(path), [to[0] - from[0], to[1] - from[1]]));
        let shift = |part: usize, q: P| match dragged {
            Some((Some(k), off)) if k == part => [q[0] + off[0], q[1] + off[1]],
            _ => q,
        };
        let net_hi = match &view.selected {
            Some(Pick::Track(k)) => board.tracks.get(*k).map(|t| t.net.clone()),
            Some(Pick::Via(k)) => board.vias.get(*k).map(|v| v.net.clone()),
            _ => None,
        };
        let mut layers = vec![Layer::BCu, Layer::FCu];
        if view.layer == Layer::BCu {
            layers.reverse();
        }
        for layer in layers {
            let shown = if layer == Layer::FCu { view.visible.front } else { view.visible.back };
            if !shown {
                continue;
            }
            let active = layer == view.layer;
            let base = layer_colour(layer);
            let colour = if active { base } else { base.gamma_multiply(0.45) };
            if view.visible.zones {
                for (l, pts, idx) in &analysis.fills {
                    if *l == layer {
                        d.mesh(pts, idx, colour.gamma_multiply(0.7));
                    }
                }
            }
            for (k, t) in board.tracks.iter().enumerate().filter(|(_, t)| t.layer == layer) {
                let hi = view.selected == Some(Pick::Track(k))
                    || net_hi.as_deref() == Some(t.net.as_str());
                d.stroke(
                    &[p(t.a), p(t.b)],
                    t.width.mm(),
                    if hi { colour.gamma_multiply(1.4).to_opaque() } else { colour },
                );
            }
            for pad in layout.pads.iter().filter(|pd| pd.layers.contains(&layer)) {
                let mut c = geometry::pad(pad, 0.0);
                for q in &mut c {
                    *q = shift(pad.part, *q);
                }
                let hi = net_hi.is_some() && pad.net == net_hi;
                d.convex(&c, if hi { colour.gamma_multiply(1.4).to_opaque() } else { colour });
            }
        }
        for (k, v) in board.vias.iter().enumerate() {
            d.disc(
                p(v.at),
                v.diameter.mm() / 2.0,
                if view.selected == Some(Pick::Via(k)) { SELECTED } else { VIA },
            );
            d.disc(p(v.at), v.drill.mm() / 2.0, HOLE);
        }
        for pad in &layout.pads {
            if let Some(dr) = pad.drill() {
                d.disc(shift(pad.part, pad.center), dr / 2.0, HOLE);
            }
        }
        // Zone outlines, hatched at their edge as KiCad draws them.
        for (k, z) in board.zones.iter().enumerate() {
            let pts: Vec<P> = z.outline.iter().map(|q| p(*q)).collect();
            let c =
                if view.selected == Some(Pick::Zone(k)) { SELECTED } else { layer_colour(z.layer) };
            d.outline(&pts, Stroke::new(1.0, c));
        }
        // Silkscreen.
        if view.visible.silk {
            for (k, part) in layout.parts.iter().enumerate() {
                if part.place.is_none() {
                    continue;
                }
                for g in layout.graphics(k) {
                    paint_graphic(&d, &g, &|q| shift(k, q));
                }
            }
            for g in &board.graphics {
                paint_graphic(&d, g, &|q| q);
            }
        }
        // The edge.
        let edge: Vec<P> = board.outline.iter().map(|q| p(*q)).collect();
        if edge.len() >= 2 {
            d.outline(&edge, Stroke::new(1.5, EDGE));
        }
        // The selected part's box.
        if let Some(Pick::Footprint(path)) = &view.selected {
            if let Some(k) = layout.part(path) {
                let part = &layout.parts[k];
                if let Some(place) = part.place {
                    let (lo, hi) = sync::extent(&part.footprint);
                    let c: Vec<P> = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
                        .iter()
                        .map(|&q| shift(k, place.apply(q)))
                        .collect();
                    let locked = board.placed(path).is_some_and(|f| f.locked);
                    d.outline(
                        &c,
                        Stroke::new(
                            1.5,
                            if locked { Color32::from_rgb(255, 170, 60) } else { SELECTED },
                        ),
                    );
                }
            }
        }
        if let Some(checked) = &analysis.checked {
            if view.visible.ratsnest {
                for (_, a, b) in &checked.connectivity.ratsnest {
                    painter.line_segment([d.at(*a), d.at(*b)], Stroke::new(1.0, RATSNEST));
                }
            }
            if view.visible.drc {
                for v in &checked.violations {
                    let Some(at) = v.at else { continue };
                    if v.rule == drc::Rule::Unconnected {
                        continue;
                    }
                    let c = if v.severity() == Severity::Error {
                        Color32::from_rgb(255, 60, 60)
                    } else {
                        Color32::from_rgb(255, 190, 40)
                    };
                    let s = d.at(at);
                    painter.add(egui::Shape::convex_polygon(
                        vec![s, s + egui::vec2(10.0, -4.0), s + egui::vec2(4.0, -10.0)],
                        c,
                        Stroke::NONE,
                    ));
                }
            }
        }
        // What's being drawn.
        let ghost = Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 255, 255, 180));
        match &view.tool {
            PcbTool::Route { net, width, points } if !points.is_empty() => {
                if let Some(c) = view.cursor {
                    let to = match copper_at(layout, c, f64::from(4.0 / cam.scale), view.layer) {
                        Some((Some(n), q)) if n == *net => q,
                        _ => view.snap(c),
                    };
                    let legs = posture(*points.last().expect("started"), to, view.diagonal_first);
                    let margin = route_margin(layout, net, &legs, *width, view.layer);
                    let colour = if margin < -1e-6 {
                        Color32::from_rgb(255, 80, 80)
                    } else {
                        layer_colour(view.layer).gamma_multiply(1.3).to_opaque()
                    };
                    d.stroke(&legs, *width, colour.gamma_multiply(0.8));
                    // The clearance it must keep, faintly.
                    let clearance = layout.board.rules.clearance_for(net).mm();
                    for seg in legs.windows(2) {
                        d.outline(
                            &geometry::capsule(seg[0], seg[1], width + 2.0 * clearance),
                            Stroke::new(1.0, colour.gamma_multiply(0.35)),
                        );
                    }
                }
            }
            PcbTool::Zone { points } | PcbTool::Outline { points } if !points.is_empty() => {
                let mut pts = points.clone();
                if let Some(c) = view.cursor {
                    pts.push(view.snap(c));
                }
                let screen: Vec<Pos2> = pts.iter().map(|&q| d.at(q)).collect();
                painter.add(egui::Shape::line(screen, ghost));
            }
            _ => {}
        }
        if let Some(c) = view.cursor {
            if view.tool != PcbTool::Select {
                let s = d.at(view.snap(c));
                painter.line_segment([s - egui::vec2(6.0, 0.0), s + egui::vec2(6.0, 0.0)], ghost);
                painter.line_segment([s - egui::vec2(0.0, 6.0), s + egui::vec2(0.0, 6.0)], ghost);
            }
        }
        // Status, faintly, in the corner.
        let mut status = String::new();
        if let Some(c) = view.cursor {
            status = format!("X {:.3}  Y {:.3} mm   ", c[0], c[1]);
        }
        if let Some(checked) = &analysis.checked {
            let un = checked.connectivity.unrouted();
            status.push_str(&format!(
                "{} unrouted · {} error(s) · {} warning(s)",
                un,
                checked.errors(),
                checked.warnings()
            ));
        }
        if let Some(n) = &view.note {
            status.push_str(&format!("   {n}"));
        }
        painter.text(
            rect.left_bottom() + egui::vec2(10.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            status,
            egui::FontId::monospace(11.0),
            Color32::from_gray(170),
        );
    }
}

/// A silkscreen drawing.
fn paint_graphic(d: &Draw<'_>, g: &circuiter_model::board::Graphic, shift: &dyn Fn(P) -> P) {
    use circuiter_model::board::Graphic as G;
    let colour = |l: Layer| match l {
        Layer::FSilk => Some(SILK_FRONT),
        Layer::BSilk => Some(SILK_BACK),
        _ => None,
    };
    match g {
        G::Line { layer, a, b, width } => {
            if let Some(c) = colour(*layer) {
                d.stroke(&[shift(p(*a)), shift(p(*b))], width.mm(), c);
            }
        }
        G::Circle { layer, center, radius, width } => {
            if let Some(c) = colour(*layer) {
                d.painter.circle_stroke(
                    d.at(shift(p(*center))),
                    radius.mm() as f32 * d.cam.scale,
                    Stroke::new(d.px(width.mm()), c),
                );
            }
        }
        G::Poly { layer, points, width, filled } => {
            if let Some(c) = colour(*layer) {
                let pts: Vec<P> = points.iter().map(|q| shift(p(*q))).collect();
                if *filled {
                    d.convex(&pts, c);
                } else {
                    let mut closed = pts.clone();
                    closed.extend(pts.first().copied());
                    d.stroke(&closed, width.mm(), c);
                }
            }
        }
        G::Text { layer, at, text, size } => {
            if let Some(c) = colour(*layer) {
                for line in
                    font::strokes(text, shift(p(*at)), size.mm(), 0.0, *layer == Layer::BSilk)
                {
                    d.stroke(&line, font::stroke(size.mm()), c);
                }
            }
        }
    }
}

/// The board's settings and the selected item, for the inspector.
pub fn board_inspector(app: &mut CircuiterApp, ui: &mut egui::Ui) {
    let pal = style::palette(ui);
    let Some(ws) = &app.workspace else { return };
    let editable = ws.editable();
    let Some(board) = ws.current_def().and_then(|d| d.board.clone()) else {
        ui.label(
            RichText::new("No board yet: create one from the PCB view's button.").color(pal.weak),
        );
        return;
    };
    let mut changed = board.clone();
    let selected = app.pcb.selected.clone();
    // 3D parts of the project, for a case of your own.
    let bodies: Vec<String> = ws
        .design
        .library(ws.project.name())
        .map(|l| {
            l.cells
                .values()
                .filter(|d| d.model3d.is_some() && d.schematic.is_none())
                .map(|d| d.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut make_parts = false;
    let mut import_error = None;
    let layout = app.pcb.layout().cloned();
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        ui.add_enabled_ui(editable, |ui| {
            if let Some(pick) = &selected {
                style::section(ui, "Selected", |ui| {
                    selected_ui(ui, &mut changed, layout.as_ref(), pick)
                });
            }
            style::section(ui, "Board", |ui| {
                let outline: Vec<P> = changed.outline.iter().map(|q| p(*q)).collect();
                let rect = outline.len() == 4 && {
                    let (xs, ys): (Vec<f64>, Vec<f64>) =
                        outline.iter().map(|q| (q[0], q[1])).unzip();
                    xs.iter().all(|&x| {
                        (x - xs[0]).abs() < 1e-9
                            || (x - xs[1]).abs() < 1e-9
                            || (x - xs[2]).abs() < 1e-9
                    }) && ys.iter().filter(|&&y| (y - ys[0]).abs() < 1e-9).count() == 2
                };
                style::properties(ui, "board-props", |ui| {
                    if rect {
                        let (lo, hi) =
                            outline.iter().fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), q| {
                                (
                                    [lo[0].min(q[0]), lo[1].min(q[1])],
                                    [hi[0].max(q[0]), hi[1].max(q[1])],
                                )
                            });
                        let (mut w, mut h) = (hi[0] - lo[0], hi[1] - lo[1]);
                        style::name(ui, "Width");
                        let a = ui
                            .add(
                                egui::DragValue::new(&mut w)
                                    .range(5.0..=1000.0)
                                    .speed(0.5)
                                    .suffix(" mm"),
                            )
                            .changed();
                        ui.end_row();
                        style::name(ui, "Height");
                        let b = ui
                            .add(
                                egui::DragValue::new(&mut h)
                                    .range(5.0..=1000.0)
                                    .speed(0.5)
                                    .suffix(" mm"),
                            )
                            .changed();
                        ui.end_row();
                        if a || b {
                            let pt = |x: f64, y: f64| point([x, y]);
                            changed.outline = vec![
                                pt(lo[0], lo[1]),
                                pt(lo[0] + w, lo[1]),
                                pt(lo[0] + w, lo[1] + h),
                                pt(lo[0], lo[1] + h),
                            ];
                        }
                    } else {
                        style::name(ui, "Outline");
                        ui.label(format!("{} corners", outline.len()));
                        ui.end_row();
                    }
                    style::name(ui, "");
                    if ui
                        .button(format!("{}  Import Outline…", icon::POLYGON))
                        .on_hover_text(
                            "The board's edge from a DXF or SVG drawing: its largest outline",
                        )
                        .clicked()
                    {
                        match import_outline() {
                            Ok(Some(points)) => changed.outline = points,
                            Ok(None) => {}
                            Err(e) => import_error = Some(e),
                        }
                    }
                    ui.end_row();
                    style::name(ui, "Copper layers");
                    egui::ComboBox::from_id_salt("copper-layers")
                        .selected_text(changed.copper_layers.to_string())
                        .show_ui(ui, |ui| {
                            for n in [2u8, 4, 6] {
                                ui.selectable_value(&mut changed.copper_layers, n, n.to_string());
                            }
                        });
                    ui.end_row();
                    length_row(ui, "Thickness", &mut changed.thickness, 0.4..=3.2);
                });
            });
            style::section(ui, "Design rules", |ui| {
                style::properties(ui, "rules-props", |ui| {
                    let r = &mut changed.rules;
                    length_row(ui, "Clearance", &mut r.clearance, 0.05..=5.0);
                    length_row(ui, "Track width", &mut r.track_width, 0.05..=10.0);
                    length_row(ui, "Via diameter", &mut r.via_diameter, 0.2..=5.0);
                    length_row(ui, "Via drill", &mut r.via_drill, 0.1..=4.0);
                    length_row(ui, "Min. track", &mut r.min_track_width, 0.05..=5.0);
                    length_row(ui, "Min. clearance", &mut r.min_clearance, 0.05..=5.0);
                    length_row(ui, "Min. drill", &mut r.min_drill, 0.1..=5.0);
                    length_row(ui, "Min. ring", &mut r.min_annular_ring, 0.05..=2.0);
                    length_row(ui, "To edge", &mut r.edge_clearance, 0.0..=5.0);
                    length_row(ui, "Hole to hole", &mut r.hole_to_hole, 0.0..=5.0);
                });
                ui.label(
                    RichText::new("The defaults suit most board houses' standard service.")
                        .color(pal.weak)
                        .small(),
                );
            });
            style::section(ui, "Case", |ui| {
                case_ui(ui, &mut changed, &bodies, &mut make_parts, pal)
            });
        });
    });
    if make_parts {
        app.pcb_case_parts();
        return;
    }
    if let Some(e) = import_error {
        app.error = Some(e);
    }
    if changed != board {
        let Some(ws) = &mut app.workspace else { return };
        if let Err(e) = ws.edit_board(|_, _, b| {
            *b = Some(changed);
            Ok(())
        }) {
            app.error = Some(e);
        }
    }
}

/// A board outline from a drawing the user picks: its largest outline, arcs drawn as
/// chords, moved so its top-left corner is the board's origin. `None` if they cancel.
fn import_outline() -> Result<Option<Vec<circuiter_model::geom::Point>>, String> {
    let Some(path) = rfd::FileDialog::new().add_filter("Drawings", &["dxf", "svg"]).pick_file()
    else {
        return Ok(None);
    };
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let loops = circuiter_3d::import2d::outlines(&path.to_string_lossy(), &text)?;
    Ok(Some(board_outline(&loops[0])))
}

/// A drawing's outline (Y up) as a board's (Y down), its top-left corner at the origin.
pub fn board_outline(l: &circuiter_3d::import2d::Loop) -> Vec<circuiter_model::geom::Point> {
    let pts = circuiter_3d::import2d::flatten(l);
    let x0 = pts.iter().map(|q| q.0).fold(f64::MAX, f64::min);
    let y1 = pts.iter().map(|q| q.1).fold(f64::MIN, f64::max);
    pts.iter().map(|&(x, y)| point([x - x0, y1 - y])).collect()
}

/// The board's case: made to fit, or a part of your own; its size, material and use.
fn case_ui(
    ui: &mut egui::Ui,
    board: &mut Board,
    bodies: &[String],
    make_parts: &mut bool,
    pal: &style::Palette,
) {
    let Some(e) = &mut board.enclosure else {
        ui.label(RichText::new("A base and lid made to fit the board, with standoffs and openings for its connectors and controls; or a 3D part of your own.").color(pal.weak));
        if ui.button(format!("{}  Make a Case", icon::CUBE)).clicked() {
            board.enclosure = Some(Default::default());
        }
        return;
    };
    let mut remove = false;
    style::properties(ui, "case-props", |ui| {
        style::name(ui, "Case");
        let current = e.part.as_ref().map_or("Made to fit".to_owned(), |p| p.to_string());
        egui::ComboBox::from_id_salt("case-part").selected_text(current).show_ui(ui, |ui| {
            if ui.selectable_label(e.part.is_none(), "Made to fit").clicked() {
                e.part = None;
                e.lid = None;
            }
            for name in bodies {
                let r = circuiter_model::CellRef::new(None, name);
                if ui.selectable_label(e.part.as_ref() == Some(&r), name).clicked() {
                    e.part = Some(r);
                }
            }
        });
        ui.end_row();
        if e.part.is_some() {
            style::name(ui, "Lid");
            let current = e.lid.as_ref().map_or("None".to_owned(), |p| p.to_string());
            egui::ComboBox::from_id_salt("case-lid").selected_text(current).show_ui(ui, |ui| {
                if ui.selectable_label(e.lid.is_none(), "None").clicked() {
                    e.lid = None;
                }
                for name in bodies {
                    let r = circuiter_model::CellRef::new(None, name);
                    if ui.selectable_label(e.lid.as_ref() == Some(&r), name).clicked() {
                        e.lid = Some(r);
                    }
                }
            });
            ui.end_row();
            style::name(ui, "Board at");
            ui.horizontal(|ui| {
                for v in &mut e.board_at {
                    ui.add(egui::DragValue::new(v).speed(0.1).suffix(" mm"));
                }
            });
            ui.end_row();
        }
        let mm = |ui: &mut egui::Ui, name: &str, v: &mut f64, lo: f64, hi: f64, tip: &str| {
            style::name(ui, name);
            ui.add(egui::DragValue::new(v).range(lo..=hi).speed(0.1).suffix(" mm"))
                .on_hover_text(tip);
            ui.end_row();
        };
        mm(ui, "Wall", &mut e.wall, 0.5, 10.0, "Wall thickness");
        if e.part.is_none() {
            mm(ui, "Gap", &mut e.gap, 0.2, 20.0, "Room between the board and the walls");
            mm(
                ui,
                "Under the board",
                &mut e.below,
                0.0,
                100.0,
                "Standoff height; 0 clears the parts' leads",
            );
            mm(
                ui,
                "Over the board",
                &mut e.above,
                0.0,
                200.0,
                "Headroom; 0 clears the tallest part",
            );
            mm(ui, "Corner radius", &mut e.radius, 0.0, 30.0, "Rounding of the outside corners");
        }
        style::name(ui, "Material");
        let mut m = usize::from(e.material == circuiter_model::board::CaseMaterial::Aluminium);
        if let Some(k) = style::segmented(ui, &[("Plastic", true), ("Aluminium", true)], m) {
            m = k;
            e.material = if m == 0 {
                circuiter_model::board::CaseMaterial::Plastic
            } else {
                circuiter_model::board::CaseMaterial::Aluminium
            };
        }
        ui.end_row();
        style::name(ui, "Vents");
        ui.checkbox(&mut e.vents, "Slots in the lid");
        ui.end_row();
        if e.part.is_none() {
            style::name(ui, "Openings");
            ui.checkbox(&mut e.openings, "For connectors and controls");
            ui.end_row();
        }
        style::name(ui, "Simulate in it");
        ui.checkbox(&mut e.simulate, "The air inside warms with the parts");
        ui.end_row();
        style::name(ui, "Room");
        ui.add(egui::DragValue::new(&mut e.room).range(-40.0..=85.0).speed(0.5).suffix(" °C"));
        ui.end_row();
    });
    ui.horizontal(|ui| {
        if e.part.is_none()
            && ui
                .button(format!("{}  Make Editable 3D Parts", icon::CUBE))
                .on_hover_text(
                    "Turns the case into two 3D parts, base and lid, to change in the body editor",
                )
                .clicked()
        {
            *make_parts = true;
        }
        if ui.button(format!("{}  Remove Case", icon::TRASH)).clicked() {
            remove = true;
        }
    });
    if remove {
        board.enclosure = None;
    }
}

fn length_row(
    ui: &mut egui::Ui,
    name: &str,
    value: &mut Length,
    range: std::ops::RangeInclusive<f64>,
) {
    style::name(ui, name);
    let mut mm = value.mm();
    if ui
        .add(egui::DragValue::new(&mut mm).range(range).speed(0.01).max_decimals(3).suffix(" mm"))
        .changed()
    {
        *value = Length::from_mm(mm);
    }
    ui.end_row();
}

fn selected_ui(ui: &mut egui::Ui, board: &mut Board, layout: Option<&Layout>, pick: &Pick) {
    let p = style::palette(ui);
    match pick {
        Pick::Footprint(path) => {
            let part = layout.and_then(|l| l.part(path).map(|k| &l.parts[k]));
            let Some(fp) = board.footprints.iter_mut().find(|f| f.path == *path) else { return };
            if let Some(part) = part {
                ui.label(
                    RichText::new(format!(
                        "{}  {}  ·  {}",
                        part.reference, part.value, part.package
                    ))
                    .strong()
                    .color(p.text),
                );
            }
            style::properties(ui, "fp-props", |ui| {
                let mut at = p_of(fp);
                style::name(ui, "X");
                let a = ui.add(egui::DragValue::new(&mut at[0]).speed(0.1).suffix(" mm")).changed();
                ui.end_row();
                style::name(ui, "Y");
                let b = ui.add(egui::DragValue::new(&mut at[1]).speed(0.1).suffix(" mm")).changed();
                ui.end_row();
                if a || b {
                    fp.at = point(at);
                }
                style::name(ui, "Rotation");
                ui.add(
                    egui::DragValue::new(&mut fp.rotation)
                        .range(0.0..=359.9)
                        .speed(1.0)
                        .suffix("°"),
                );
                ui.end_row();
                style::name(ui, "Side");
                let mut side = usize::from(fp.back);
                if let Some(s) = style::segmented(ui, &[("Front", true), ("Back", true)], side) {
                    side = s;
                    fp.back = side == 1;
                }
                ui.end_row();
                style::name(ui, "Locked");
                ui.checkbox(&mut fp.locked, "");
                ui.end_row();
            });
        }
        Pick::Track(k) => {
            let Some(t) = board.tracks.get_mut(*k) else { return };
            ui.label(RichText::new(format!("Track · {}", t.net)).strong().color(p.text));
            style::properties(ui, "track-props", |ui| {
                length_row(ui, "Width", &mut t.width, 0.05..=10.0);
                style::name(ui, "Layer");
                let mut side = usize::from(t.layer == Layer::BCu);
                if let Some(s) = style::segmented(ui, &[("F.Cu", true), ("B.Cu", true)], side) {
                    side = s;
                    t.layer = if side == 0 { Layer::FCu } else { Layer::BCu };
                }
                ui.end_row();
            });
            let len = (t.b.x.mm() - t.a.x.mm()).hypot(t.b.y.mm() - t.a.y.mm());
            ui.label(RichText::new(format!("{len:.2} mm long")).color(p.weak));
        }
        Pick::Via(k) => {
            let Some(v) = board.vias.get_mut(*k) else { return };
            ui.label(RichText::new(format!("Via · {}", v.net)).strong().color(p.text));
            style::properties(ui, "via-props", |ui| {
                length_row(ui, "Diameter", &mut v.diameter, 0.2..=5.0);
                length_row(ui, "Drill", &mut v.drill, 0.1..=4.0);
            });
        }
        Pick::Zone(k) => {
            let nets: Vec<String> =
                layout.map(|l| l.nets.iter().cloned().collect()).unwrap_or_default();
            let Some(z) = board.zones.get_mut(*k) else { return };
            ui.label(
                RichText::new(format!("Copper pour on {}", z.layer.name())).strong().color(p.text),
            );
            style::properties(ui, "zone-props", |ui| {
                style::name(ui, "Net");
                egui::ComboBox::from_id_salt("zone-net-sel").selected_text(z.net.clone()).show_ui(
                    ui,
                    |ui| {
                        for n in &nets {
                            ui.selectable_value(&mut z.net, n.clone(), n);
                        }
                    },
                );
                ui.end_row();
                style::name(ui, "Layer");
                let mut side = usize::from(z.layer == Layer::BCu);
                if let Some(s) = style::segmented(ui, &[("F.Cu", true), ("B.Cu", true)], side) {
                    side = s;
                    z.layer = if side == 0 { Layer::FCu } else { Layer::BCu };
                }
                ui.end_row();
                style::name(ui, "Priority");
                ui.add(egui::DragValue::new(&mut z.priority).range(0..=99));
                ui.end_row();
                style::name(ui, "Pads");
                egui::ComboBox::from_id_salt("zone-pads")
                    .selected_text(format!("{:?}", z.pads))
                    .show_ui(ui, |ui| {
                        for c in [PadConnection::Thermal, PadConnection::Solid, PadConnection::None]
                        {
                            ui.selectable_value(&mut z.pads, c, format!("{c:?}"));
                        }
                    });
                ui.end_row();
                length_row(ui, "Min. width", &mut z.min_width, 0.05..=5.0);
                length_row(ui, "Relief gap", &mut z.thermal_gap, 0.1..=5.0);
                length_row(ui, "Spoke width", &mut z.spoke_width, 0.1..=5.0);
            });
        }
    }
}

fn p_of(f: &Placed) -> P {
    p(f.at)
}

/// The design rule check's findings, for the results panel.
pub fn drc_panel(app: &mut CircuiterApp, ui: &mut egui::Ui) {
    let p = style::palette(ui);
    let Some(checked) = app.pcb.analysis.as_ref().and_then(|a| a.checked.as_ref()) else {
        ui.label(RichText::new("No board to check.").color(p.weak));
        return;
    };
    let mut focus = None;
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        if checked.violations.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{}  Every design rule is met and every net is routed.",
                    icon::CHECK_CIRCLE
                ))
                .color(p.go),
            );
        }
        for v in &checked.violations {
            let (glyph, c) = match v.severity() {
                Severity::Error => (icon::X_CIRCLE, p.error),
                Severity::Warning => (icon::WARNING, p.warning),
            };
            let row = ui.horizontal(|ui| {
                ui.label(RichText::new(glyph).color(c));
                ui.label(RichText::new(v.rule.title()).strong().color(p.text));
                ui.label(RichText::new(&v.message).color(p.weak));
            });
            let r = row.response.interact(Sense::click());
            if r.clicked() {
                focus = v.at;
            }
            if v.at.is_some() {
                r.on_hover_text("Click to show it on the board");
            }
        }
    });
    if let (Some(at), Some(cam)) = (focus, &mut app.pcb.camera) {
        cam.center = Pt::new(at[0], at[1]);
        cam.scale = cam.scale.max(20.0);
    }
}
