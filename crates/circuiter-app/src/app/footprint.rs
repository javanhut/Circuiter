//! Choosing a cell's footprint in the inspector: a package, its parameters, which pad
//! each port goes to, and a drawing of the result with every pad's number and port.
//! And the footprint editor, for pads laid out one by one: placed, dragged, numbered,
//! sized and shaped, in arrays of rows, columns or ball grids, with silkscreen lines and
//! a courtyard round them.

use super::CircuiterApp;
use crate::style::{self, icon};
use crate::workspace::Workspace;
use circuiter_model::CellId;
use circuiter_model::board::{FootprintView, Graphic, Layer, Pad, PadKind, PadShape};
use circuiter_model::geom::{Length, Point};
use circuiter_pcb::footprints::{self, Numbering, PACKAGES};
use eframe::egui::{self, Color32, RichText, Stroke};
use std::collections::BTreeSet;

/// A text field that keeps what's typed until it's left, then hands it over if it
/// changed. While not being typed in, it shows `current`.
fn field(ui: &mut egui::Ui, id: egui::Id, current: &str, hint: &str, width: f32) -> Option<String> {
    let mut text = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| current.to_owned());
    let r =
        ui.add(egui::TextEdit::singleline(&mut text).id(id).hint_text(hint).desired_width(width));
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
        return None;
    }
    ui.data_mut(|d| d.remove::<String>(id));
    (r.lost_focus() && text != current).then_some(text)
}

/// The cell's footprint section: chosen, changed, checked and drawn. Returns the
/// footprint to open in the editor, when asked to.
pub fn section(ui: &mut egui::Ui, ws: &mut Workspace) -> Option<FootprintView> {
    let mut edit = None;
    let p = style::palette(ui);
    let def = ws.current_def().cloned()?;
    let editable = ws.editable();
    let ports: Vec<String> = def.cell.ports.iter().map(|q| q.name.clone()).collect();
    let mut change: Option<Option<FootprintView>> = None;
    let id = egui::Id::new(("footprint", ws.current().cell.to_string()));
    ui.add_enabled_ui(editable, |ui| {
        let Some(view) = &def.footprint else {
            ui.label(
                RichText::new("None yet: pick a package and its pads are made for it.").weak(),
            );
            egui::ComboBox::from_id_salt(id.with("new"))
                .selected_text("Choose a package…")
                .show_ui(ui, |ui| {
                    if ui.selectable_label(false, "Draw my own…").clicked() {
                        edit = Some(FootprintView::default());
                    }
                    ui.separator();
                    for name in PACKAGES {
                        if ui.selectable_label(false, *name).clicked() {
                            change = Some(Some(FootprintView {
                                package: (*name).to_owned(),
                                ..FootprintView::default()
                            }));
                        }
                    }
                });
            return;
        };
        let mut view = view.clone();
        style::properties(ui, "footprint-props", |ui| {
            style::name(ui, "Package");
            let custom = view.package.is_empty() || view.package.contains("${");
            let shown = if view.package.is_empty() { "pads as drawn" } else { &view.package };
            egui::ComboBox::from_id_salt(id.with("package")).selected_text(shown).show_ui(
                ui,
                |ui| {
                    for name in PACKAGES {
                        if ui.selectable_label(view.package == *name, *name).clicked() {
                            view.package = (*name).to_owned();
                            // The old package's parameters mean nothing to the new one.
                            view.params.retain(|k, _| {
                                footprints::package_params(name).iter().any(|(n, _)| n == k)
                            });
                            change = Some(Some(view.clone()));
                        }
                    }
                },
            );
            ui.end_row();
            if custom {
                style::name(ui, "");
                ui.label(
                    RichText::new("chosen by the part's parameters or drawn pad by pad").weak(),
                );
                ui.end_row();
            }
            for (key, default) in footprints::package_params(&view.package) {
                style::name(ui, key);
                let now = view.params.get(*key).cloned().unwrap_or_default();
                if let Some(new) = field(ui, id.with(("param", *key)), &now, default, 90.0) {
                    if new.trim().is_empty() {
                        view.params.remove(*key);
                    } else {
                        view.params.insert((*key).to_owned(), new.trim().to_owned());
                    }
                    change = Some(Some(view.clone()));
                }
                ui.end_row();
            }
        });
        let param = |n: &str| def.cell.param(n).and_then(|q| q.default.clone());
        let resolved = footprints::resolve(&view, &ports, &param);
        let fp = match resolved {
            Ok(fp) => fp,
            Err(e) => {
                ui.label(RichText::new(e).color(p.error));
                return;
            }
        };
        ui.label(RichText::new(&fp.name).small().weak())
            .on_hover_text("Its name in KiCad's libraries, where it's the same");
        // Each port's pads: as listed, else numbered in order.
        ui.add_space(4.0);
        ui.label(RichText::new("Pads for each port").strong());
        let numbers: Vec<&str> = fp.pads.iter().map(|q| q.number.as_str()).collect();
        egui::Grid::new(id.with("pins")).num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
            for (k, port) in ports.iter().enumerate() {
                ui.label(RichText::new(port).monospace());
                let listed = view.pins.get(port).cloned().unwrap_or_default();
                let order = (k + 1).to_string();
                if let Some(new) = field(ui, id.with(("pin", port)), &listed, &order, 70.0) {
                    let new = new.replace(' ', "");
                    if new.is_empty() || new == order {
                        view.pins.remove(port);
                    } else {
                        view.pins.insert(port.clone(), new);
                    }
                    change = Some(Some(view.clone()));
                }
                let missing: Vec<&str> = fp.pins[port]
                    .split(',')
                    .map(str::trim)
                    .filter(|n| !numbers.contains(n))
                    .collect();
                if missing.is_empty() {
                    ui.label("");
                } else {
                    ui.label(
                        RichText::new(format!("{}  no pad {}", icon::WARNING, missing.join(", ")))
                            .color(p.error),
                    );
                }
                ui.end_row();
            }
        });
        let used: std::collections::BTreeSet<&str> =
            fp.pins.values().flat_map(|n| n.split(',').map(str::trim)).collect();
        let spare: Vec<&str> = numbers.iter().copied().filter(|n| !used.contains(n)).collect();
        if !spare.is_empty() {
            ui.label(
                RichText::new(format!("Pads joined to no port: {}", spare.join(", ")))
                    .weak()
                    .small(),
            );
        }
        if ports.is_empty() {
            ui.label(RichText::new("Add ports to join them to pads.").weak().small());
        }
        preview(ui, &fp);
        let label = if view.package.is_empty() { "Edit Pads…" } else { "Edit Pads One by One…" };
        if ui
            .button(format!("{}  {label}", icon::PENCIL_SIMPLE))
            .on_hover_text("The footprint editor: place, move, number and size pads, in rows, columns or grids, with silkscreen; a package becomes its pads to change")
            .clicked()
        {
            let param = |n: &str| def.cell.param(n).and_then(|q| q.default.clone());
            match footprints::editable(&view, &ports, &param) {
                Ok(v) => edit = Some(v),
                Err(e) => {
                    ui.label(RichText::new(e).color(p.error));
                }
            }
        }
        if ui
            .button(format!("{}  Remove Footprint", icon::TRASH))
            .on_hover_text("The part then stays off the board")
            .clicked()
        {
            change = Some(None);
        }
    });
    if let Some(new) = change {
        if let Err(e) = ws.edit(|_, _, d| {
            d.footprint = new;
            Ok(())
        }) {
            ui.label(RichText::new(e).color(p.error));
        }
    }
    edit
}

/// The footprint drawn to fit the panel's width: silkscreen, pads with their holes,
/// each pad's number on it and its port beside it.
fn preview(ui: &mut egui::Ui, fp: &footprints::Footprint) {
    let mut lo = [f64::MAX; 2];
    let mut hi = [f64::MIN; 2];
    let mut grow = |x: f64, y: f64| {
        lo = [lo[0].min(x), lo[1].min(y)];
        hi = [hi[0].max(x), hi[1].max(y)];
    };
    for pad in &fp.pads {
        let (w, h) = (pad.size.0.mm(), pad.size.1.mm());
        let r = w.hypot(h) / 2.0;
        grow(pad.at.x.mm() - r, pad.at.y.mm() - r);
        grow(pad.at.x.mm() + r, pad.at.y.mm() + r);
    }
    for g in &fp.graphics {
        match g {
            Graphic::Line { layer: Layer::FSilk, a, b, .. } => {
                grow(a.x.mm(), a.y.mm());
                grow(b.x.mm(), b.y.mm());
            }
            Graphic::Poly { layer: Layer::FSilk, points, .. } => {
                points.iter().for_each(|q| grow(q.x.mm(), q.y.mm()));
            }
            Graphic::Circle { layer: Layer::FSilk, center, radius, .. } => {
                let (c, r) = ([center.x.mm(), center.y.mm()], radius.mm());
                grow(c[0] - r, c[1] - r);
                grow(c[0] + r, c[1] + r);
            }
            _ => {}
        }
    }
    if lo[0] > hi[0] {
        return;
    }
    let width = ui.available_width().max(120.0);
    let span = [(hi[0] - lo[0]).max(1.0), (hi[1] - lo[1]).max(1.0)];
    let height = (width as f64 * span[1] / span[0]).clamp(100.0, 260.0) as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, Color32::from_rgb(0, 16, 35));
    let k = ((rect.width() - 30.0) as f64 / span[0]).min((rect.height() - 30.0) as f64 / span[1]);
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let at = |x: f64, y: f64| {
        rect.center() + egui::vec2(((x - mid[0]) * k) as f32, ((y - mid[1]) * k) as f32)
    };
    let silk = Stroke::new(1.2, super::pcb::layer_colour(Layer::FSilk));
    for g in &fp.graphics {
        match g {
            Graphic::Line { layer: Layer::FSilk, a, b, .. } => {
                painter.line_segment([at(a.x.mm(), a.y.mm()), at(b.x.mm(), b.y.mm())], silk);
            }
            Graphic::Poly { layer: Layer::FSilk, points, .. } => {
                let pts: Vec<egui::Pos2> = points.iter().map(|q| at(q.x.mm(), q.y.mm())).collect();
                painter.add(egui::Shape::line(pts, silk));
            }
            Graphic::Circle { layer: Layer::FSilk, center, radius, .. } => {
                painter.circle_stroke(
                    at(center.x.mm(), center.y.mm()),
                    (radius.mm() * k) as f32,
                    silk,
                );
            }
            _ => {}
        }
    }
    let ports_of = |number: &str| -> Vec<&str> {
        fp.pins
            .iter()
            .filter(|(_, n)| n.split(',').any(|x| x.trim() == number))
            .map(|(port, _)| port.as_str())
            .collect()
    };
    let copper = super::pcb::layer_colour(Layer::FCu);
    for pad in &fp.pads {
        let c = at(pad.at.x.mm(), pad.at.y.mm());
        let (w, h) = ((pad.size.0.mm() * k) as f32, (pad.size.1.mm() * k) as f32);
        // Turned pads are drawn as their corners; square-on ones with rounded ends.
        let (s, co) = (-pad.rotation.to_radians() as f32).sin_cos();
        let corner = |x: f32, y: f32| c + egui::vec2(x * co - y * s, x * s + y * co);
        match pad.shape {
            PadShape::Circle => {
                painter.circle_filled(c, w.max(h) / 2.0, copper);
            }
            _ if pad.rotation.abs() > 1e-9 => {
                let pts = vec![
                    corner(-w / 2.0, -h / 2.0),
                    corner(w / 2.0, -h / 2.0),
                    corner(w / 2.0, h / 2.0),
                    corner(-w / 2.0, h / 2.0),
                ];
                painter.add(egui::Shape::convex_polygon(pts, copper, Stroke::NONE));
            }
            shape => {
                let round = match shape {
                    PadShape::Oval => w.min(h) / 2.0,
                    PadShape::RoundRect => w.min(h) / 4.0,
                    _ => 0.0,
                };
                let r = egui::Rect::from_center_size(c, egui::vec2(w, h));
                painter.rect_filled(r, round, copper);
            }
        }
        if let PadKind::Through { drill } | PadKind::Hole { drill } = pad.kind {
            painter.circle_filled(c, (drill.mm() * k / 2.0) as f32, Color32::from_rgb(0, 16, 35));
        }
        painter.text(
            c,
            egui::Align2::CENTER_CENTER,
            &pad.number,
            egui::FontId::monospace(11.0),
            Color32::WHITE,
        );
        let joined = ports_of(&pad.number);
        if !joined.is_empty() {
            painter.text(
                c - egui::vec2(0.0, h / 2.0 + 2.0),
                egui::Align2::CENTER_BOTTOM,
                joined.join(","),
                egui::FontId::proportional(10.0),
                Color32::from_rgb(140, 200, 255),
            );
        }
    }
}

// ---- The footprint editor ------------------------------------------------------------

/// What the pointer does in the footprint editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Pad,
    Line,
    Rect,
}

/// Grid steps on offer, in millimetres.
const GRIDS: &[f64] = &[0.05, 0.1, 0.25, 0.5, 0.635, 1.0, 1.27, 2.54];

/// The footprint editor's state: the footprint being drawn, apart from the cell's until
/// saved.
#[derive(Debug)]
pub struct Editor {
    /// The cell whose footprint is open, and the footprint as drawn so far.
    open: Option<(CellId, FootprintView)>,
    undo: Vec<FootprintView>,
    redo: Vec<FootprintView>,
    selected: BTreeSet<usize>,
    graphic: Option<usize>,
    tool: Tool,
    /// A line or rectangle's first corner.
    first: Option<[f64; 2]>,
    /// Dragging the selection (from, to), or a selection box.
    drag: Option<([f64; 2], [f64; 2])>,
    band: Option<([f64; 2], [f64; 2])>,
    /// The view: the point in the middle and pixels per millimetre.
    view: Option<([f64; 2], f64)>,
    grid: f64,
    /// What new pads are like.
    pad: Pad,
    /// Arrays: rows, columns, pitch across and down, numbering, first number.
    rows: usize,
    cols: usize,
    pitch: (f64, f64),
    numbering: Numbering,
    first_number: usize,
    error: Option<String>,
    /// Closing without saving, once this frame is drawn.
    cancel: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Editor {
            open: None,
            undo: Vec::new(),
            redo: Vec::new(),
            selected: BTreeSet::new(),
            graphic: None,
            tool: Tool::Select,
            first: None,
            drag: None,
            band: None,
            view: None,
            grid: 0.05,
            pad: Pad {
                number: String::new(),
                at: Point::ORIGIN,
                size: (Length::from_mm(1.7), Length::from_mm(1.7)),
                shape: PadShape::Circle,
                kind: PadKind::Through { drill: Length::from_mm(1.0) },
                rotation: 0.0,
            },
            rows: 2,
            cols: 4,
            pitch: (2.54, 2.54),
            numbering: Numbering::Columns,
            first_number: 1,
            error: None,
            cancel: false,
        }
    }
}

impl Editor {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Opens `view` of `cell` for editing.
    pub fn start(&mut self, cell: CellId, view: FootprintView) {
        *self = Editor { open: Some((cell, view)), ..std::mem::take(self) };
        self.undo.clear();
        self.redo.clear();
        self.selected.clear();
        self.graphic = None;
        self.view = None;
        self.first = None;
        self.error = None;
        self.cancel = false;
    }

    fn draft(&mut self) -> &mut FootprintView {
        &mut self.open.as_mut().expect("open").1
    }

    /// Keeps the footprint as it is, to come back to with undo.
    fn checkpoint(&mut self) {
        let now = self.draft().clone();
        self.undo.push(now);
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn step(&mut self, back: bool) {
        let (from, to) =
            if back { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        if let Some(v) = from.pop() {
            let now = std::mem::replace(&mut self.open.as_mut().expect("open").1, v);
            to.push(now);
            self.selected.clear();
            self.graphic = None;
        }
    }

    fn snap(&self, q: [f64; 2]) -> [f64; 2] {
        let g = self.grid;
        [(q[0] / g).round() * g, (q[1] / g).round() * g]
    }

    fn delete(&mut self) {
        if self.selected.is_empty() && self.graphic.is_none() {
            return;
        }
        self.checkpoint();
        let (pads, graphic) = (std::mem::take(&mut self.selected), self.graphic.take());
        let d = self.draft();
        let mut k = 0;
        d.pads.retain(|_| {
            k += 1;
            !pads.contains(&(k - 1))
        });
        if let Some(g) = graphic.filter(|g| *g < d.graphics.len()) {
            d.graphics.remove(g);
        }
    }
}

fn mm(v: f64) -> Length {
    Length::from_mm(v)
}

fn xy(q: Point) -> [f64; 2] {
    [q.x.mm(), q.y.mm()]
}

/// The pad under `at`, last drawn first.
fn pad_at(pads: &[Pad], at: [f64; 2]) -> Option<usize> {
    (0..pads.len()).rev().find(|&k| {
        let c = footprints::pad_corners(&pads[k]);
        let mut odd = false;
        for i in 0..4 {
            let (a, b) = (c[i], c[(i + 1) % 4]);
            if (a.1 > at[1]) != (b.1 > at[1])
                && at[0] < a.0 + (at[1] - a.1) * (b.0 - a.0) / (b.1 - a.1)
            {
                odd = !odd;
            }
        }
        odd
    })
}

/// The drawing within `reach` millimetres of `at`.
fn graphic_at(graphics: &[Graphic], at: [f64; 2], reach: f64) -> Option<usize> {
    let near =
        |a: [f64; 2], b: [f64; 2]| circuiter_pcb::geometry::segment_distance(a, b, at, at) <= reach;
    (0..graphics.len()).rev().find(|&k| match &graphics[k] {
        Graphic::Line { a, b, .. } => near(xy(*a), xy(*b)),
        Graphic::Poly { points, .. } => {
            (0..points.len()).any(|i| near(xy(points[i]), xy(points[(i + 1) % points.len()])))
        }
        Graphic::Circle { center, radius, .. } => {
            let c = xy(*center);
            ((c[0] - at[0]).hypot(c[1] - at[1]) - radius.mm()).abs() <= reach
        }
        Graphic::Text { .. } => false,
    })
}

/// The footprint editor, in a window of its own over the app while a footprint is open.
pub fn window(app: &mut CircuiterApp, ctx: &egui::Context) {
    if !app.footprint_editor.is_open() {
        return;
    }
    let mut keep_open = true;
    let mut save = false;
    let name = app.footprint_editor.open.as_ref().map(|(c, _)| c.cell.clone()).unwrap_or_default();
    egui::Window::new(format!("{}  Footprint of {name}", icon::CPU))
        .id(egui::Id::new("footprint-editor"))
        .open(&mut keep_open)
        .default_size([1100.0, 700.0])
        .resizable(true)
        .collapsible(false)
        .show(ctx, |ui| {
            let ed = &mut app.footprint_editor;
            egui::Panel::left("footprint-editor-side")
                .resizable(false)
                .exact_size(290.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| save = side(ui, ed));
                });
            canvas(ui, ed);
        });
    let ed = &mut app.footprint_editor;
    if save {
        if let (Some((cell, mut view)), Some(ws)) = (ed.open.clone(), app.workspace.as_mut()) {
            // A courtyard round it, when none was drawn: the rules check needs one.
            if !view.graphics.iter().any(|g| {
                matches!(
                    g,
                    Graphic::Poly { layer: Layer::FCrtYd, .. }
                        | Graphic::Circle { layer: Layer::FCrtYd, .. }
                )
            }) {
                view.graphics.extend(footprints::courtyard(&view.pads, &view.graphics, 0.25));
            }
            let saved = if ws.current().cell == cell {
                ws.edit(|_, _, d| {
                    d.footprint = Some(view);
                    Ok(())
                })
            } else {
                Err("open the part again to save its footprint".into())
            };
            match saved {
                Ok(()) => ed.open = None,
                Err(e) => ed.error = Some(e),
            }
        }
    } else if !keep_open || ed.cancel {
        ed.open = None;
    }
}

/// The tools, the settings for new pads and arrays, the selection's settings, and
/// saving. True when asked to save.
fn side(ui: &mut egui::Ui, ed: &mut Editor) -> bool {
    let p = style::palette(ui);
    let mut save = false;
    // Saving and undo first, always in view.
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!ed.undo.is_empty(), egui::Button::new(icon::ARROW_COUNTER_CLOCKWISE))
            .on_hover_text("Undo  (Ctrl+Z)")
            .clicked()
        {
            ed.step(true);
        }
        if ui
            .add_enabled(!ed.redo.is_empty(), egui::Button::new(icon::ARROW_CLOCKWISE))
            .on_hover_text("Redo  (Ctrl+Y)")
            .clicked()
        {
            ed.step(false);
        }
        if ui
            .button("Cancel")
            .on_hover_text("Close, leaving the part's footprint as it was")
            .clicked()
        {
            ed.cancel = true;
        }
        let ok =
            egui::Button::new(RichText::new(format!("{}  Save", icon::CHECK)).strong().color(p.go))
                .fill(p.go_soft);
        if ui
            .add(ok)
            .on_hover_text("Becomes the part's footprint (undoable like any edit)")
            .clicked()
        {
            save = true;
        }
    });
    if let Some(e) = &ed.error {
        ui.label(RichText::new(e).color(p.error));
    }
    ui.separator();
    ui.label(RichText::new("Tool").strong());
    let tools = [("Select", true), ("Pad", true), ("Line", true), ("Rectangle", true)];
    let now = match ed.tool {
        Tool::Select => 0,
        Tool::Pad => 1,
        Tool::Line => 2,
        Tool::Rect => 3,
    };
    if let Some(k) = style::segmented(ui, &tools, now) {
        ed.tool = [Tool::Select, Tool::Pad, Tool::Line, Tool::Rect][k];
        ed.first = None;
    }
    ui.label(RichText::new(match ed.tool {
        Tool::Select => "Click a pad or line; Shift adds; drag to move; drag on empty space to box-select. R turns, Del deletes",
        Tool::Pad => "Click to place a pad like the one set below, numbered next",
        Tool::Line => "Click where a silkscreen line starts and ends",
        Tool::Rect => "Click two opposite corners of a silkscreen rectangle",
    }).small().weak());
    ui.horizontal(|ui| {
        ui.label("Grid");
        egui::ComboBox::from_id_salt("fp-grid").selected_text(format!("{} mm", ed.grid)).show_ui(
            ui,
            |ui| {
                for &g in GRIDS {
                    ui.selectable_value(&mut ed.grid, g, format!("{g} mm"));
                }
            },
        );
    });
    ui.separator();
    ui.label(RichText::new("New pads").strong());
    let mut template = ed.pad.clone();
    pad_settings(ui, "fp-new", &mut template);
    ed.pad = template;
    ui.separator();
    ui.label(RichText::new("Array").strong());
    egui::Grid::new("fp-array").num_columns(2).show(ui, |ui| {
        ui.label("Rows × columns");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut ed.rows).range(1..=100));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut ed.cols).range(1..=100));
        });
        ui.end_row();
        ui.label("Pitch across, down");
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut ed.pitch.0).range(0.1..=50.0).speed(0.01).suffix(" mm"),
            );
            ui.add(
                egui::DragValue::new(&mut ed.pitch.1).range(0.1..=50.0).speed(0.01).suffix(" mm"),
            );
        });
        ui.end_row();
        ui.label("Numbered");
        egui::ComboBox::from_id_salt("fp-numbering")
            .selected_text(match ed.numbering {
                Numbering::Rows => "along rows",
                Numbering::Columns => "down columns",
                Numbering::Grid => "A1, A2… (grid)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut ed.numbering, Numbering::Rows, "along rows")
                    .on_hover_text("1 2 3 / 4 5 6");
                ui.selectable_value(&mut ed.numbering, Numbering::Columns, "down columns")
                    .on_hover_text("1 3 5 / 2 4 6, as a double-row header");
                ui.selectable_value(&mut ed.numbering, Numbering::Grid, "A1, A2… (grid)")
                    .on_hover_text(
                        "A letter per row, a number per column, as a BGA (no I, O, Q, S, X, Z)",
                    );
            });
        ui.end_row();
        if ed.numbering != Numbering::Grid {
            ui.label("From");
            ui.add(egui::DragValue::new(&mut ed.first_number).range(1..=9999));
            ui.end_row();
        }
    });
    if ui.button(format!("{}  Place Array", icon::SQUARES_FOUR)).on_hover_text("Rows and columns of pads like the one set above, centred on the origin, selected to move").clicked() {
        ed.checkpoint();
        let mut first = ed.pad.clone();
        first.at = Point::mm(
            -((ed.cols - 1) as f64) * ed.pitch.0 / 2.0,
            -((ed.rows - 1) as f64) * ed.pitch.1 / 2.0,
        );
        let start = if ed.numbering == Numbering::Grid { 1 } else { ed.first_number };
        let made = footprints::pad_array(&first, ed.rows, ed.cols, ed.pitch, start, ed.numbering);
        let d = ed.draft();
        let from = d.pads.len();
        d.pads.extend(made);
        ed.selected = (from..ed.draft().pads.len()).collect();
        ed.graphic = None;
    }
    ui.separator();
    // The selection.
    let pads = ed.draft().pads.len();
    ed.selected.retain(|k| *k < pads);
    if !ed.selected.is_empty() {
        let sel: Vec<usize> = ed.selected.iter().copied().collect();
        ui.label(
            RichText::new(if sel.len() == 1 {
                "Selected pad".to_owned()
            } else {
                format!("{} pads selected", sel.len())
            })
            .strong(),
        );
        let before = ed.draft().clone();
        let mut first = before.pads[sel[0]].clone();
        if sel.len() == 1 {
            egui::Grid::new("fp-sel-one").num_columns(2).show(ui, |ui| {
                ui.label("Number");
                if let Some(n) = field(ui, egui::Id::new("fp-number"), &first.number, "", 80.0) {
                    first.number = n.trim().to_owned();
                }
                ui.end_row();
                let mut at = xy(first.at);
                ui.label("At X, Y");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut at[0]).speed(0.01).suffix(" mm"));
                    ui.add(egui::DragValue::new(&mut at[1]).speed(0.01).suffix(" mm"));
                });
                first.at = Point::mm(at[0], at[1]);
                ui.end_row();
            });
        }
        pad_settings(ui, "fp-sel", &mut first);
        // What changed on the first goes to every pad selected.
        let base = &before.pads[sel[0]];
        let changed = first != *base;
        if changed {
            let d = ed.draft();
            for &k in &sel {
                let pd = &mut d.pads[k];
                if sel.len() == 1 {
                    *pd = first.clone();
                } else {
                    pd.size = first.size;
                    pd.shape = first.shape;
                    pd.kind = first.kind;
                    pd.rotation = first.rotation;
                }
            }
            // One step of undo for each change made.
            let now = ed.draft().clone();
            if ed.undo.last() != Some(&before) {
                ed.undo.push(before);
                ed.redo.clear();
            }
            *ed.draft() = now;
        }
        if ui.button(format!("{}  Delete", icon::TRASH)).clicked() {
            ed.delete();
        }
        ui.separator();
    } else if let Some(g) = ed.graphic.filter(|g| *g < ed.draft().graphics.len()) {
        ui.label(RichText::new("Selected drawing").strong());
        let before = ed.draft().clone();
        let mut graphic = before.graphics[g].clone();
        let (layer, width) = match &mut graphic {
            Graphic::Line { layer, width, .. }
            | Graphic::Poly { layer, width, .. }
            | Graphic::Circle { layer, width, .. } => (layer, width),
            Graphic::Text { layer, size, .. } => (layer, size),
        };
        ui.horizontal(|ui| {
            for (l, name) in
                [(Layer::FSilk, "Silkscreen"), (Layer::FFab, "Fab"), (Layer::FCrtYd, "Courtyard")]
            {
                ui.selectable_value(layer, l, name);
            }
        });
        let mut w = width.mm();
        ui.horizontal(|ui| {
            ui.label("Width");
            ui.add(egui::DragValue::new(&mut w).range(0.01..=2.0).speed(0.01).suffix(" mm"));
        });
        *width = mm(w);
        if graphic != before.graphics[g] {
            ed.checkpoint();
            ed.draft().graphics[g] = graphic;
        }
        if ui.button(format!("{}  Delete", icon::TRASH)).clicked() {
            ed.delete();
        }
        ui.separator();
    }
    if ui.button("Courtyard Round the Pads").on_hover_text("Replaces any courtyard with a rectangle 0.25 mm clear of the pads and the drawing (one is added on saving when there's none)").clicked() {
        ed.checkpoint();
        let d = ed.draft();
        d.graphics.retain(|g| !matches!(g, Graphic::Poly { layer: Layer::FCrtYd, .. } | Graphic::Circle { layer: Layer::FCrtYd, .. } | Graphic::Line { layer: Layer::FCrtYd, .. }));
        let c = footprints::courtyard(&d.pads, &d.graphics, 0.25);
        d.graphics.extend(c);
    }
    let d = ed.draft();
    ui.label(
        RichText::new(format!("{} pads · {} drawings", d.pads.len(), d.graphics.len())).weak(),
    );
    let mut numbers = BTreeSet::new();
    let twice: Vec<&str> =
        d.pads.iter().map(|q| q.number.as_str()).filter(|n| !numbers.insert(*n)).collect();
    if !twice.is_empty() {
        ui.label(
            RichText::new(format!("{}  Numbered twice: {}", icon::WARNING, twice.join(", ")))
                .color(p.warning),
        )
        .on_hover_text(
            "Pads of one number are joined: fine for a tab and its pin, a mistake otherwise",
        );
    }
    ui.label(
        RichText::new("Ports are joined to pads by number in the Footprint section.")
            .small()
            .weak(),
    );
    save
}

/// A pad's size, shape, kind (through-hole, surface mount or a plain hole), drill and
/// turn.
fn pad_settings(ui: &mut egui::Ui, id: &str, pad: &mut Pad) {
    egui::Grid::new(id).num_columns(2).show(ui, |ui| {
        let (mut w, mut h) = (pad.size.0.mm(), pad.size.1.mm());
        ui.label("Size");
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut w).range(0.05..=50.0).speed(0.01).suffix(" mm"));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut h).range(0.05..=50.0).speed(0.01).suffix(" mm"));
        });
        pad.size = (mm(w), mm(h));
        ui.end_row();
        ui.label("Shape");
        egui::ComboBox::from_id_salt((id, "shape"))
            .selected_text(format!("{:?}", pad.shape))
            .show_ui(ui, |ui| {
                for (s, name) in [
                    (PadShape::Circle, "Circle"),
                    (PadShape::Rect, "Rect"),
                    (PadShape::Oval, "Oval"),
                    (PadShape::RoundRect, "RoundRect"),
                ] {
                    ui.selectable_value(&mut pad.shape, s, name);
                }
            });
        ui.end_row();
        ui.label("Kind");
        let drill = match pad.kind {
            PadKind::Through { drill } | PadKind::Hole { drill } => drill,
            PadKind::Smd => mm(1.0),
        };
        let mut kind = match pad.kind {
            PadKind::Through { .. } => 0,
            PadKind::Smd => 1,
            PadKind::Hole { .. } => 2,
        };
        egui::ComboBox::from_id_salt((id, "kind"))
            .selected_text(["Through-hole", "Surface mount", "Hole (unplated)"][kind])
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut kind, 0, "Through-hole");
                ui.selectable_value(&mut kind, 1, "Surface mount");
                ui.selectable_value(&mut kind, 2, "Hole (unplated)");
            });
        ui.end_row();
        let mut d = drill.mm();
        if kind != 1 {
            ui.label("Drill");
            ui.add(egui::DragValue::new(&mut d).range(0.1..=20.0).speed(0.01).suffix(" mm"));
            ui.end_row();
        }
        pad.kind = match kind {
            0 => PadKind::Through { drill: mm(d) },
            1 => PadKind::Smd,
            _ => PadKind::Hole { drill: mm(d) },
        };
        ui.label("Turned");
        ui.add(
            egui::DragValue::new(&mut pad.rotation).range(-360.0..=360.0).speed(1.0).suffix("°"),
        );
        ui.end_row();
    });
}

/// The drawing: grid, origin, silkscreen and other drawings, pads with their numbers;
/// and the pointer's work on it.
fn canvas(ui: &mut egui::Ui, ed: &mut Editor) {
    let rect = ui.available_rect_before_wrap();
    let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(0, 16, 35));
    // Fitted to what's drawn the first time.
    let (mid, scale) = *ed.view.get_or_insert_with(|| {
        let d = &ed.open.as_ref().expect("open").1;
        let mut lo = [-3.0f64, -3.0];
        let mut hi = [3.0f64, 3.0];
        for p in &d.pads {
            for (x, y) in footprints::pad_corners(p) {
                lo = [lo[0].min(x), lo[1].min(y)];
                hi = [hi[0].max(x), hi[1].max(y)];
            }
        }
        let span = (hi[0] - lo[0]).max(hi[1] - lo[1]) * 1.3;
        (
            [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0],
            f64::from(rect.width().min(rect.height())) / span,
        )
    });
    let to_screen = |q: [f64; 2]| {
        rect.center()
            + egui::vec2(((q[0] - mid[0]) * scale) as f32, ((q[1] - mid[1]) * scale) as f32)
    };
    let to_world = |s: egui::Pos2| {
        [
            mid[0] + f64::from(s.x - rect.center().x) / scale,
            mid[1] + f64::from(s.y - rect.center().y) / scale,
        ]
    };
    // Navigation: wheel zooms about the pointer; right or middle drag pans.
    if let Some(at) = response.hover_pos() {
        let wheel = ui.input(|i| i.smooth_scroll_delta.y);
        if wheel != 0.0 {
            let k = f64::from((wheel * 0.002).exp());
            let w = to_world(at);
            let s = scale * k;
            let m = [w[0] - (w[0] - mid[0]) / k, w[1] - (w[1] - mid[1]) / k];
            ed.view = Some((m, s.clamp(2.0, 2000.0)));
        }
    }
    if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        let d = response.drag_delta();
        let (m, s) = ed.view.expect("set");
        ed.view = Some(([m[0] - f64::from(d.x) / s, m[1] - f64::from(d.y) / s], s));
    }
    // The grid, where its dots are far enough apart to see, and the origin.
    let mut g = ed.grid;
    while g * scale < 8.0 {
        g *= 10.0;
    }
    let (lo, hi) = (to_world(rect.left_top()), to_world(rect.right_bottom()));
    let dot = Color32::from_rgba_unmultiplied(160, 170, 190, 45);
    let mut y = (lo[1] / g).floor() * g;
    while y <= hi[1] {
        let mut x = (lo[0] / g).floor() * g;
        while x <= hi[0] {
            painter.circle_filled(to_screen([x, y]), 0.8, dot);
            x += g;
        }
        y += g;
    }
    let o = to_screen([0.0, 0.0]);
    let axis = Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 200, 255, 90));
    painter.line_segment([o - egui::vec2(12.0, 0.0), o + egui::vec2(12.0, 0.0)], axis);
    painter.line_segment([o - egui::vec2(0.0, 12.0), o + egui::vec2(0.0, 12.0)], axis);
    let pointer = response.hover_pos().map(|s| ed.snap(to_world(s)));
    // The pointer's work.
    let shift = ui.input(|i| i.modifiers.shift);
    let typing = ui.ctx().egui_wants_keyboard_input();
    let key = |k: egui::Key| !typing && ui.input(|i| i.key_pressed(k));
    let cmd = ui.input(|i| i.modifiers.command);
    if cmd && key(egui::Key::Z) {
        ed.step(!shift);
    } else if cmd && key(egui::Key::Y) {
        ed.step(false);
    }
    if key(egui::Key::Delete) || key(egui::Key::Backspace) {
        ed.delete();
    }
    if key(egui::Key::Escape) {
        ed.first = None;
        ed.tool = Tool::Select;
    }
    if key(egui::Key::R) && !ed.selected.is_empty() {
        ed.checkpoint();
        let sel = ed.selected.clone();
        for k in sel {
            let pd = &mut ed.draft().pads[k];
            pd.rotation = (pd.rotation + 90.0).rem_euclid(360.0);
        }
    }
    let reach = 5.0 / scale;
    match ed.tool {
        Tool::Select => {
            if response.drag_started_by(egui::PointerButton::Primary) {
                let from = ui.input(|i| i.pointer.press_origin()).map(to_world);
                if let Some(from) = from {
                    let d = &ed.open.as_ref().expect("open").1;
                    match pad_at(&d.pads, from) {
                        Some(k) => {
                            if !ed.selected.contains(&k) {
                                if !shift {
                                    ed.selected.clear();
                                }
                                ed.selected.insert(k);
                            }
                            ed.graphic = None;
                            ed.drag = Some((from, from));
                        }
                        None => ed.band = Some((from, from)),
                    }
                }
            }
            if let Some(at) = response.interact_pointer_pos().map(to_world) {
                if let Some(d) = &mut ed.drag {
                    d.1 = at;
                }
                if let Some(b) = &mut ed.band {
                    b.1 = at;
                }
            }
            if response.drag_stopped() {
                if let Some((from, to)) = ed.drag.take() {
                    let delta = ed.snap([to[0] - from[0], to[1] - from[1]]);
                    if delta != [0.0, 0.0] {
                        ed.checkpoint();
                        let sel = ed.selected.clone();
                        for k in sel {
                            let pd = &mut ed.draft().pads[k];
                            pd.at = Point::mm(pd.at.x.mm() + delta[0], pd.at.y.mm() + delta[1]);
                        }
                    }
                }
                if let Some((a, b)) = ed.band.take() {
                    let (lo, hi) =
                        ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
                    if !shift {
                        ed.selected.clear();
                    }
                    let d = &ed.open.as_ref().expect("open").1;
                    let inside: Vec<usize> = (0..d.pads.len())
                        .filter(|&k| {
                            let c = xy(d.pads[k].at);
                            c[0] >= lo[0] && c[0] <= hi[0] && c[1] >= lo[1] && c[1] <= hi[1]
                        })
                        .collect();
                    ed.selected.extend(inside);
                    ed.graphic = None;
                }
            }
            if response.clicked_by(egui::PointerButton::Primary) {
                if let Some(at) = response.interact_pointer_pos().map(to_world) {
                    let d = &ed.open.as_ref().expect("open").1;
                    let pad = pad_at(&d.pads, at);
                    let graphic = graphic_at(&d.graphics, at, reach);
                    if !shift {
                        ed.selected.clear();
                    }
                    ed.graphic = None;
                    match (pad, graphic) {
                        (Some(k), _) => {
                            if !ed.selected.remove(&k) {
                                ed.selected.insert(k);
                            }
                        }
                        (None, Some(g)) => {
                            ed.selected.clear();
                            ed.graphic = Some(g);
                        }
                        _ => {}
                    }
                }
            }
        }
        Tool::Pad => {
            if let (true, Some(at)) = (response.clicked_by(egui::PointerButton::Primary), pointer) {
                ed.checkpoint();
                let number = footprints::next_number(&ed.draft().pads);
                let pad = Pad { number, at: Point::mm(at[0], at[1]), ..ed.pad.clone() };
                ed.draft().pads.push(pad);
                ed.selected = [ed.draft().pads.len() - 1].into();
                ed.graphic = None;
            }
        }
        Tool::Line | Tool::Rect => {
            if let (true, Some(at)) = (response.clicked_by(egui::PointerButton::Primary), pointer) {
                match ed.first.take() {
                    None => ed.first = Some(at),
                    Some(a) if a != at => {
                        ed.checkpoint();
                        let (pa, pb) = (Point::mm(a[0], a[1]), Point::mm(at[0], at[1]));
                        let w = mm(0.12);
                        let g = if ed.tool == Tool::Line {
                            Graphic::Line { layer: Layer::FSilk, a: pa, b: pb, width: w }
                        } else {
                            Graphic::Poly {
                                layer: Layer::FSilk,
                                points: vec![
                                    pa,
                                    Point::mm(at[0], a[1]),
                                    pb,
                                    Point::mm(a[0], at[1]),
                                ],
                                width: w,
                                filled: false,
                            }
                        };
                        ed.draft().graphics.push(g);
                        ed.graphic = Some(ed.draft().graphics.len() - 1);
                        ed.selected.clear();
                    }
                    Some(a) => ed.first = Some(a),
                }
            }
        }
    }
    // Drawings, then pads (as dragged), then what's being drawn.
    let d = &ed.open.as_ref().expect("open").1;
    for (k, g) in d.graphics.iter().enumerate() {
        let colour = if ed.graphic == Some(k) {
            Color32::WHITE
        } else {
            super::pcb::layer_colour(match g {
                Graphic::Line { layer, .. }
                | Graphic::Poly { layer, .. }
                | Graphic::Circle { layer, .. }
                | Graphic::Text { layer, .. } => *layer,
            })
        };
        let stroke = |w: Length| Stroke::new(((w.mm() * scale) as f32).max(1.0), colour);
        match g {
            Graphic::Line { a, b, width, .. } => {
                painter.line_segment([to_screen(xy(*a)), to_screen(xy(*b))], stroke(*width));
            }
            Graphic::Poly { points, width, .. } => {
                let mut pts: Vec<egui::Pos2> = points.iter().map(|q| to_screen(xy(*q))).collect();
                pts.extend(pts.first().copied());
                painter.add(egui::Shape::line(pts, stroke(*width)));
            }
            Graphic::Circle { center, radius, width, .. } => {
                painter.circle_stroke(
                    to_screen(xy(*center)),
                    (radius.mm() * scale) as f32,
                    stroke(*width),
                );
            }
            Graphic::Text { at, text, .. } => {
                painter.text(
                    to_screen(xy(*at)),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(11.0),
                    colour,
                );
            }
        }
    }
    let shift_by = match ed.drag {
        Some((from, to)) => ed.snap([to[0] - from[0], to[1] - from[1]]),
        None => [0.0, 0.0],
    };
    let copper = super::pcb::layer_colour(Layer::FCu);
    let hole = Color32::from_rgb(0, 16, 35);
    for (k, pad) in d.pads.iter().enumerate() {
        let on = ed.selected.contains(&k);
        let off = if on { shift_by } else { [0.0, 0.0] };
        let corners: Vec<egui::Pos2> = footprints::pad_corners(pad)
            .iter()
            .map(|&(x, y)| to_screen([x + off[0], y + off[1]]))
            .collect();
        let c = to_screen([pad.at.x.mm() + off[0], pad.at.y.mm() + off[1]]);
        let fill = if matches!(pad.kind, PadKind::Hole { .. }) {
            Color32::from_rgb(90, 90, 90)
        } else {
            copper
        };
        let (w, h) = ((pad.size.0.mm() * scale) as f32, (pad.size.1.mm() * scale) as f32);
        match pad.shape {
            PadShape::Circle => {
                painter.circle_filled(c, w.max(h) / 2.0, fill);
            }
            PadShape::Oval | PadShape::RoundRect if pad.rotation.rem_euclid(90.0).abs() < 1e-9 => {
                let turned = pad.rotation.rem_euclid(180.0) > 45.0;
                let (w, h) = if turned { (h, w) } else { (w, h) };
                let round =
                    if pad.shape == PadShape::Oval { w.min(h) / 2.0 } else { w.min(h) / 4.0 };
                painter.rect_filled(egui::Rect::from_center_size(c, egui::vec2(w, h)), round, fill);
            }
            _ => {
                painter.add(egui::Shape::convex_polygon(corners.clone(), fill, Stroke::NONE));
            }
        }
        if let PadKind::Through { drill } | PadKind::Hole { drill } = pad.kind {
            painter.circle_filled(c, (drill.mm() * scale / 2.0) as f32, hole);
        }
        if on {
            let mut ring = corners.clone();
            ring.extend(corners.first().copied());
            painter.add(egui::Shape::line(ring, Stroke::new(2.0, Color32::WHITE)));
        }
        let size = (w.min(h) * 0.45).clamp(7.0, 16.0);
        painter.text(
            c,
            egui::Align2::CENTER_CENTER,
            &pad.number,
            egui::FontId::monospace(size),
            Color32::WHITE,
        );
    }
    if let Some((a, b)) = ed.band {
        let r = egui::Rect::from_two_pos(to_screen(a), to_screen(b));
        painter.rect_stroke(
            r,
            0.0,
            Stroke::new(1.0, Color32::from_rgb(120, 200, 255)),
            egui::StrokeKind::Middle,
        );
    }
    if let (Some(a), Some(b)) = (ed.first, pointer) {
        let ghost = Stroke::new(1.5, Color32::from_rgba_unmultiplied(255, 255, 255, 160));
        if ed.tool == Tool::Line {
            painter.line_segment([to_screen(a), to_screen(b)], ghost);
        } else {
            painter.rect_stroke(
                egui::Rect::from_two_pos(to_screen(a), to_screen(b)),
                0.0,
                ghost,
                egui::StrokeKind::Middle,
            );
        }
    }
    if ed.tool == Tool::Pad {
        if let Some(at) = pointer {
            let mut ghost = ed.pad.clone();
            ghost.at = Point::mm(at[0], at[1]);
            let pts: Vec<egui::Pos2> =
                footprints::pad_corners(&ghost).iter().map(|&(x, y)| to_screen([x, y])).collect();
            painter.add(egui::Shape::convex_polygon(pts, copper.gamma_multiply(0.4), Stroke::NONE));
        }
    }
    // Where the pointer is, on the grid.
    if let Some(at) = pointer {
        painter.text(
            rect.left_bottom() + egui::vec2(10.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            format!("X {:.3}  Y {:.3} mm · wheel zooms · right-drag pans", at[0], at[1]),
            egui::FontId::monospace(11.0),
            Color32::from_rgb(160, 170, 190),
        );
    }
}
