//! The 3D view: the cell's body, or every part of a sheet on a board, to orbit, zoom
//! and export.

use super::CircuiterApp;
use crate::render3d::{Camera, Frame3d, Paint3d};
use crate::style::{self, icon};
use circuiter_3d::Mesh;
use circuiter_3d::assembly::{Assembly, body};
use circuiter_model::CellId;
use eframe::egui::{self, RichText};
use eframe::egui_wgpu;
use std::sync::Arc;

/// What the 3D view shows, worked out once per change of the design.
#[derive(Debug, Clone)]
pub struct Scene3d {
    pub cell: CellId,
    pub revision: u64,
    pub mesh: Arc<Mesh>,
    pub id: u64,
    /// Instances shown, and those with no 3D body.
    pub shown: usize,
    pub without: Vec<String>,
    pub errors: Vec<String>,
    /// A sheet (parts on a board) rather than one part's body.
    pub sheet: bool,
    /// What went wrong with which modelled feature.
    pub feature_errors: Vec<(usize, String)>,
    /// Whether it was built for the body rather than the sheet.
    pub body_mode: bool,
    /// Whether it shows the cell's board (laid out) rather than the sheet's preview.
    pub board: bool,
    /// The board's case, when it has one: what doesn't fit, and how it holds heat.
    pub case: Option<CaseReport>,
    /// The case as shown: how, and how far the lid is lifted (0 to 1).
    pub case_view: (CaseView, f32),
}

/// How the board's case is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseView {
    /// Shut, as it'll be used.
    #[default]
    Closed,
    /// With its lid lifted off.
    Open,
    /// Cut through the middle, front half away.
    Section,
    /// Left out: just the board.
    Hidden,
}

/// What the case check found.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseReport {
    /// Parts that don't fit, and why.
    pub clashes: Vec<String>,
    /// From the air inside to the room, K/W, and heat capacity, J/K.
    pub resistance: f64,
    pub capacity: f64,
    /// Inside, mm.
    pub inside: [f32; 3],
}

/// A board and its case, built once per change of the design; views of it are put
/// together from these.
#[derive(Debug, Clone)]
pub struct BoardBodies {
    cell: CellId,
    revision: u64,
    /// The running circuit's state it shows.
    live: circuiter_pcb::board3d::Live,
    board: Mesh,
    base: Option<Mesh>,
    lid: Option<Mesh>,
    /// How far up the lid lifts when opened.
    lift: f32,
    /// Where to cut a section (the middle, across Y).
    middle: f32,
    report: Option<CaseReport>,
    shown: usize,
    without: Vec<String>,
    errors: Vec<String>,
}

/// The 3D view's state.
#[derive(Debug)]
pub struct Viewer3d {
    pub camera: Camera,
    pub scene: Option<Scene3d>,
    /// The scene the camera was last fitted to.
    fitted: Option<CellId>,
    next_id: u64,
    /// Whether the window renders with the GPU (headless tests don't).
    pub gpu: bool,
    /// The view's width over its height, as last drawn.
    pub aspect: f32,
    /// Showing a sheet cell's own body (to model it) rather than its parts.
    pub body_mode: bool,
    /// Showing the cell's laid-out board, when it has one, rather than the sheet's
    /// preview.
    pub board: bool,
    /// How the case is shown, and how far its lid is lifted (0 to 1).
    pub case_view: CaseView,
    pub lid_lift: f32,
    bodies: Option<BoardBodies>,
    pub editor: super::body_editor::BodyEditor,
}

impl Default for Viewer3d {
    fn default() -> Self {
        Viewer3d {
            camera: Camera::default(),
            scene: None,
            fitted: None,
            next_id: 0,
            gpu: false,
            aspect: 1.6,
            body_mode: false,
            board: false,
            case_view: CaseView::Closed,
            lid_lift: 1.0,
            bodies: None,
            editor: Default::default(),
        }
    }
}

impl Viewer3d {
    /// The scene for `cell` of `design`, rebuilt only when either changed.
    pub fn update(
        &mut self,
        design: &circuiter_model::Design,
        cell: &CellId,
        revision: u64,
        live: &circuiter_pcb::board3d::Live,
    ) {
        let mode = self.body_mode;
        let board = self.board;
        let view = (self.case_view, self.lid_lift);
        let live_now = self.bodies.as_ref().is_none_or(|b| b.live == *live);
        if live_now
            && self.scene.as_ref().is_some_and(|s| {
                s.cell == *cell
                    && s.revision == revision
                    && s.body_mode == mode
                    && s.board == board
                    && (!board || s.case_view == view)
            })
        {
            return;
        }
        let refit = self
            .scene
            .as_ref()
            .is_some_and(|s| (s.body_mode != mode || s.board != board) && s.cell == *cell);
        self.next_id += 1;
        let mut scene = None;
        if board && !mode {
            let same = |b: &BoardBodies| b.cell == *cell && b.revision == revision;
            match &mut self.bodies {
                // Only the running circuit moved: rebuild the board, keep the case.
                Some(b) if same(b) && b.live != *live => b.relive(design, live),
                Some(b) if same(b) => {}
                _ => self.bodies = board_bodies(design, cell, revision, live),
            }
            if let Some(b) = &self.bodies {
                scene = Some(compose(b, view, self.next_id));
            }
        }
        let scene = scene.unwrap_or_else(|| build(design, cell, revision, self.next_id, mode));
        if self.fitted.as_ref() != Some(cell) || refit {
            if let Some(b) = scene.mesh.bounds() {
                self.camera.fit(b, self.aspect);
            }
            self.fitted = Some(cell.clone());
        }
        self.scene = Some(scene);
    }

    pub fn fit(&mut self) {
        if let Some(b) = self.scene.as_ref().and_then(|s| s.mesh.bounds()) {
            self.camera.fit(b, self.aspect);
        }
    }
}

/// Builds what the 3D view shows for `cell`: its sheet's parts on a board, or its own
/// body.
pub fn build(
    design: &circuiter_model::Design,
    cell: &CellId,
    revision: u64,
    id: u64,
    body_mode: bool,
) -> Scene3d {
    let mut scene = Scene3d {
        cell: cell.clone(),
        revision,
        mesh: Arc::new(Mesh::default()),
        id,
        shown: 0,
        without: Vec::new(),
        errors: Vec::new(),
        sheet: false,
        feature_errors: Vec::new(),
        body_mode,
        board: false,
        case: None,
        case_view: (CaseView::Closed, 0.0),
    };
    let Some(def) = design.cell(cell) else { return scene };
    if !body_mode && has_sheet(design, cell) {
        let a = Assembly::sheet(design, cell);
        scene.shown = a.parts.len();
        scene.mesh = Arc::new(a.mesh());
        scene.without = a.without;
        scene.errors = a.errors;
        scene.sheet = true;
    } else if let Some(view) = def.model3d.as_ref().filter(|v| !v.features.is_empty()) {
        // Evaluated here rather than through `body`, to keep each feature's errors.
        let vars = |name: &str| -> Option<f64> {
            let text = def.cell.param(name)?.default.clone()?;
            circuiter_model::units::parse_quantity(&text, None).ok()
        };
        let built = circuiter_3d::cad::evaluate(&view.features, &vars);
        scene.shown = usize::from(!built.mesh.vertices.is_empty());
        scene.mesh = Arc::new(built.mesh);
        scene.feature_errors = built.errors;
    } else {
        match body(def, &Default::default()) {
            Ok(Some(m)) => {
                scene.shown = 1;
                scene.mesh = Arc::new(m);
            }
            Ok(None) => {}
            Err(e) => scene.errors.push(e),
        }
    }
    scene
}

/// The cell's board in 3D with its parts, and its case if it has one, checked for fit.
pub fn board_bodies(
    design: &circuiter_model::Design,
    cell: &CellId,
    revision: u64,
    live: &circuiter_pcb::board3d::Live,
) -> Option<BoardBodies> {
    use circuiter_pcb::enclosure;
    let board = design.cell(cell)?.board.as_ref()?;
    let layout = circuiter_pcb::layout::Layout::new(design, cell, board).ok()?;
    let fills = circuiter_pcb::zones::fill(&layout);
    let built = circuiter_pcb::board3d::build_live(design, &layout, &fills, live);
    let mut out = BoardBodies {
        cell: cell.clone(),
        revision,
        live: live.clone(),
        board: built.mesh(),
        base: None,
        lid: None,
        lift: 0.0,
        middle: 0.0,
        report: None,
        shown: built.parts.len().max(1),
        without: built.without.clone(),
        errors: built.errors.clone(),
    };
    let Some(e) = &board.enclosure else { return Some(out) };
    let (base, lid, lid_top) = match &e.part {
        // A case of your own, with the board set in it.
        Some(part) => {
            let place = |r: &circuiter_model::CellRef| -> Option<Mesh> {
                let (_, def) = design.resolve(r, &cell.library)?;
                let m = circuiter_3d::assembly::body(def, &Default::default()).ok()??;
                let at = e.board_at.map(|v| -(v as f32));
                Some(m.translated(at))
            };
            let Some(base) = place(part) else {
                out.errors.push(format!("the case part {part} has no body"));
                return Some(out);
            };
            let lid = e.lid.as_ref().and_then(place);
            (base, lid, None)
        }
        None => match enclosure::make(&layout, &built, e) {
            Ok(made) => {
                let (base, lid) = enclosure::bodies(&made);
                let (r, c) = enclosure::thermal(&made, e);
                let inside = [0, 1, 2].map(|k| made.inner.1[k] - made.inner.0[k]);
                out.report =
                    Some(CaseReport { clashes: Vec::new(), resistance: r, capacity: c, inside });
                (base, Some(lid), Some((made.lid_z + made.wall) as f32))
            }
            Err(err) => {
                out.errors.push(err);
                return Some(out);
            }
        },
    };
    let meshes: Vec<&Mesh> = std::iter::once(&base).chain(lid.as_ref()).collect();
    let clashes = enclosure::fit(&layout, &built, &meshes, lid_top);
    let bounds = base.bounds().unwrap_or(([0.0; 3], [0.0; 3]));
    if out.report.is_none() {
        let (lo, hi) = bounds;
        let w = e.wall as f32;
        let inner = ([lo[0] + w, lo[1] + w, lo[2] + w], [hi[0] - w, hi[1] - w, hi[2] - w]);
        let (r, c) = enclosure::thermal_of((lo, hi), inner, e.wall, e);
        out.report = Some(CaseReport {
            clashes: Vec::new(),
            resistance: r,
            capacity: c,
            inside: [0, 1, 2].map(|k| inner.1[k] - inner.0[k]),
        });
    }
    if let Some(r) = &mut out.report {
        r.clashes = clashes;
    }
    out.lift = (bounds.1[2] - bounds.0[2]) + 15.0;
    out.middle = (bounds.0[1] + bounds.1[1]) / 2.0;
    out.base = Some(base);
    out.lid = lid;
    Some(out)
}

impl BoardBodies {
    /// The board again with the running circuit's parts as they are now.
    fn relive(&mut self, design: &circuiter_model::Design, live: &circuiter_pcb::board3d::Live) {
        let Some(board) = design.cell(&self.cell).and_then(|d| d.board.as_ref()) else { return };
        let Ok(layout) = circuiter_pcb::layout::Layout::new(design, &self.cell, board) else {
            return;
        };
        let fills = circuiter_pcb::zones::fill(&layout);
        self.board = circuiter_pcb::board3d::build_live(design, &layout, &fills, live).mesh();
        self.live = live.clone();
    }
}

/// A view of the board and its case.
fn compose(b: &BoardBodies, (view, lift): (CaseView, f32), id: u64) -> Scene3d {
    let mut mesh = b.board.clone();
    let cut =
        |m: &Mesh| if view == CaseView::Section { m.clipped(1, b.middle, true) } else { m.clone() };
    if view != CaseView::Hidden {
        if let Some(base) = &b.base {
            mesh.append(cut(base));
        }
        if let Some(lid) = &b.lid {
            let up = if view == CaseView::Open { lift * b.lift } else { 0.0 };
            mesh.append(cut(lid).translated([0.0, 0.0, up]));
        }
    }
    Scene3d {
        cell: b.cell.clone(),
        revision: b.revision,
        mesh: Arc::new(mesh),
        id,
        shown: b.shown,
        without: b.without.clone(),
        errors: b.errors.clone(),
        sheet: true,
        feature_errors: Vec::new(),
        body_mode: false,
        board: true,
        case: b.report.clone(),
        case_view: (view, lift),
    }
}

/// Whether `cell` has parts on its sheet (so its 3D view shows them).
pub fn has_sheet(design: &circuiter_model::Design, cell: &CellId) -> bool {
    design.cell(cell).and_then(|d| d.schematic.as_ref()).is_some_and(|s| !s.instances.is_empty())
}

impl CircuiterApp {
    pub(super) fn view_3d(&mut self, ui: &mut egui::Ui) {
        let p = style::palette(ui);
        let Some(ws) = &self.workspace else { return };
        let cell = ws.current().cell.clone();
        let has_sheet_cell = has_sheet(&ws.design, &cell);
        if !has_sheet_cell {
            self.viewer3d.body_mode = false;
        }
        let live = ws.live_board();
        self.viewer3d.update(&ws.design, &cell, ws.revision, &live);
        if ws.live.is_some() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        let Some(scene) = self.viewer3d.scene.clone() else { return };
        // The view's own toolbar.
        let mut export = false;
        let mut make_case = false;
        let editable = ws.editable();
        egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(10, 4)).show(
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .button(format!("{}  Fit", icon::CORNERS_OUT))
                        .on_hover_text("F, or double-click")
                        .clicked()
                    {
                        self.viewer3d.fit();
                    }
                    let cam = &mut self.viewer3d.camera;
                    for (name, yaw, pitch) in [
                        ("Top", -std::f32::consts::FRAC_PI_2, 1.55),
                        ("Front", -std::f32::consts::FRAC_PI_2, 0.0),
                        ("Side", 0.0, 0.0),
                        ("Iso", -2.2, 0.6),
                    ] {
                        if ui.button(name).clicked() {
                            cam.yaw = yaw;
                            cam.pitch = pitch;
                        }
                    }
                    ui.separator();
                    if has_sheet_cell {
                        // Sheet: parts where their symbols are; Board: as laid out.
                        let has_board = ws.current_def().is_some_and(|d| d.board.is_some());
                        let mode = if self.viewer3d.body_mode {
                            2
                        } else {
                            usize::from(self.viewer3d.board && has_board)
                        };
                        let modes = [("Sheet", true), ("Board", has_board), ("Body", true)];
                        if let Some(m) = style::segmented(ui, &modes, mode) {
                            self.viewer3d.body_mode = m == 2;
                            self.viewer3d.board = m == 1;
                        }
                        ui.separator();
                    }
                    let summary = if scene.sheet {
                        let mut s = format!("{} part(s) in 3D", scene.shown);
                        if !scene.without.is_empty() {
                            s.push_str(&format!(" · without a body: {}", scene.without.join(", ")));
                        }
                        s
                    } else if scene.shown == 1 {
                        format!("{} body", cell.cell)
                    } else {
                        String::new()
                    };
                    ui.label(RichText::new(summary).color(p.weak));
                    style::trailing(ui, |ui| {
                        if ui
                            .add_enabled(
                                scene.shown > 0,
                                egui::Button::new(format!("{}  Export STL…", icon::EXPORT)),
                            )
                            .clicked()
                        {
                            export = true;
                        }
                    });
                });
                // The board's case: how it's shown, whether everything fits, its heat.
                if scene.board {
                    ui.horizontal_wrapped(|ui| {
                        match &scene.case {
                            None => {
                                ui.label(RichText::new("No case yet.").color(p.weak));
                                if ui
                                    .add_enabled(editable, egui::Button::new(format!("{}  Make a Case", icon::CUBE)))
                                    .on_hover_text("A base and lid made to fit the board: standoffs, openings for its connectors and controls")
                                    .clicked()
                                {
                                    make_case = true;
                                }
                            }
                            Some(report) => {
                                let views = [("Closed", true), ("Open", true), ("Section", true), ("No case", true)];
                                let now = match self.viewer3d.case_view {
                                    CaseView::Closed => 0,
                                    CaseView::Open => 1,
                                    CaseView::Section => 2,
                                    CaseView::Hidden => 3,
                                };
                                if let Some(k) = style::segmented(ui, &views, now) {
                                    self.viewer3d.case_view =
                                        [CaseView::Closed, CaseView::Open, CaseView::Section, CaseView::Hidden][k];
                                }
                                if self.viewer3d.case_view == CaseView::Open {
                                    ui.add(egui::Slider::new(&mut self.viewer3d.lid_lift, 0.0..=1.0).text("lid").show_value(false));
                                }
                                ui.separator();
                                if report.clashes.is_empty() {
                                    ui.label(RichText::new(format!("{}  Everything fits", icon::CHECK_CIRCLE)).color(p.go));
                                } else {
                                    ui.label(
                                        RichText::new(format!("{}  {} part(s) don't fit", icon::WARNING, report.clashes.len()))
                                            .color(p.error),
                                    )
                                    .on_hover_text(report.clashes.join("\n"));
                                }
                                ui.separator();
                                let [w, d, h] = report.inside;
                                ui.label(
                                    RichText::new(format!(
                                        "Inside {w:.0} × {d:.0} × {h:.0} mm · air warms {:.0} °C per watt",
                                        report.resistance
                                    ))
                                    .color(p.weak),
                                )
                                .on_hover_text(format!(
                                    "Thermal resistance from the air inside to the room: {:.1} K/W; heat capacity {:.0} J/K (a time constant of {:.0} s). Simulations run with the circuit inside it.",
                                    report.resistance,
                                    report.capacity,
                                    report.resistance * report.capacity
                                ));
                            }
                        }
                    });
                }
            },
        );
        if make_case {
            if let Some(ws) = &mut self.workspace {
                let made = ws.edit_board(|_, _, b| {
                    let b = b.as_mut().ok_or("no board")?;
                    b.enclosure.get_or_insert_with(Default::default);
                    Ok(())
                });
                if let Err(e) = made {
                    self.error = Some(e);
                }
            }
        }
        if export {
            self.export_stl(&scene);
        }
        if !scene.sheet {
            // Sketching, the drawing gets most of the room, the model beside it; each
            // width is kept for next time.
            let sketching = self.viewer3d.editor.sketching();
            let (id, width) = if sketching {
                ("body-panel-sketch", ui.available_width() * 0.62)
            } else {
                ("body-panel", 380.0)
            };
            egui::Panel::left(id)
                .resizable(true)
                .default_size(width)
                .frame(egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::same(10)))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| self.body_panel(ui));
                });
        }
        let rect = ui.available_rect_before_wrap();
        self.viewer3d.aspect = rect.width() / rect.height().max(1.0);
        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        if scene.shown == 0 {
            ui.painter().rect_filled(rect, 0.0, p.base);
            let text = match scene.errors.first() {
                Some(e) => format!("Can't build the 3D view: {e}"),
                None if scene.sheet => "Nothing on this sheet has a 3D body yet (sources, instruments and logic blocks have none)".to_owned(),
                None => format!(
                    "{} has no 3D body: it's a model rather than a physical part, or it hasn't been given one",
                    cell.cell
                ),
            };
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(14.0),
                p.weak,
            );
            return;
        }
        // Mouse: drag to orbit; right, middle or Shift-drag to pan; wheel to zoom.
        let cam = &mut self.viewer3d.camera;
        let shift = ui.input(|i| i.modifiers.shift);
        if response.dragged_by(egui::PointerButton::Primary) && !shift {
            let d = response.drag_delta();
            cam.orbit(d.x, d.y);
        } else if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
            || (response.dragged_by(egui::PointerButton::Primary) && shift)
        {
            let d = response.drag_delta();
            cam.pan(d.x, d.y, rect.height());
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                cam.zoom((scroll * 0.002).exp());
            }
        }
        if response.double_clicked()
            || (response.hovered() && ui.input(|i| i.key_pressed(egui::Key::F)))
        {
            self.viewer3d.fit();
        }
        let cam = self.viewer3d.camera;
        if !self.viewer3d.gpu {
            ui.painter().rect_filled(rect, 0.0, p.base);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "3D needs the GPU renderer",
                egui::FontId::proportional(14.0),
                p.weak,
            );
            return;
        }
        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32];
        let bg = p.base;
        let to_f = |c: u8| f64::from(c) / 255.0;
        let frame = Frame3d {
            mesh: scene.mesh.clone(),
            mesh_id: scene.id,
            view_proj: cam.view_proj(rect.width() / rect.height().max(1.0)),
            background: [to_f(bg.r()), to_f(bg.g()), to_f(bg.b()), 1.0],
            size,
        };
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, Paint3d(frame)));
        // How to drive it, faintly, in the corner.
        ui.painter().text(
            rect.left_bottom() + egui::vec2(10.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            "Drag to orbit · right-drag or Shift-drag to pan · wheel to zoom · double-click to fit",
            egui::FontId::proportional(11.0),
            p.weak,
        );
        if scene.sheet && !scene.board {
            ui.painter().text(
                rect.left_top() + egui::vec2(10.0, 8.0),
                egui::Align2::LEFT_TOP,
                "Preview: parts sit where their symbols are, until the board is laid out",
                egui::FontId::proportional(11.0),
                p.weak,
            );
        }
    }

    fn export_stl(&mut self, scene: &Scene3d) {
        let name = format!("{}.stl", scene.cell.cell);
        if let Some(path) =
            rfd::FileDialog::new().set_file_name(name).add_filter("STL", &["stl"]).save_file()
        {
            if let Err(e) = std::fs::write(&path, scene.mesh.to_stl(&scene.cell.cell)) {
                self.error = Some(format!("writing {}: {e}", path.display()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::{Design, Project};
    use std::path::Path;

    #[test]
    fn a_sheet_and_a_part_build_their_3d_scenes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/analog_lab");
        let project = Project::load(&root).unwrap();
        let design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
        let s = build(&design, &CellId::new("analog_lab", "blinker"), 0, 1, false);
        assert!(s.sheet && s.errors.is_empty(), "{:?}", s.errors);
        // Battery, 555, three resistors, two capacitors, LED.
        assert_eq!(s.shown, 8, "without: {:?}", s.without);
        assert!(s.mesh.triangles() > 1000);
        let part = build(&design, &CellId::new("std", "ne555"), 0, 2, false);
        assert!(!part.sheet && part.shown == 1);
        let none = build(&design, &CellId::new("std", "vdc"), 0, 3, false);
        assert_eq!(none.shown, 0);
    }

    /// The blinker on a board in a case made for it: the case is checked and reported,
    /// opening it lifts the lid, a section cuts it, and a lit LED shows lit.
    #[test]
    fn a_board_in_its_case() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/analog_lab");
        let project = Project::load(&root).unwrap();
        let mut design = Design::for_project(&project, circuiter_stdlib::load().unwrap()).unwrap();
        let id = CellId::new("analog_lab", "blinker");
        let mut board = circuiter_pcb::sync::new_board(&design, &id).unwrap();
        board.enclosure = Some(Default::default());
        design.cell_mut(&id).unwrap().board = Some(board);
        let live = circuiter_pcb::board3d::Live::default();
        let mut b = board_bodies(&design, &id, 1, &live).unwrap();
        let report = b.report.clone().expect("a case");
        assert!(report.clashes.is_empty(), "{:?}", report.clashes);
        assert!(report.resistance > 10.0 && report.capacity > 1.0);
        let closed = compose(&b, (CaseView::Closed, 1.0), 1).mesh.bounds().unwrap();
        let open = compose(&b, (CaseView::Open, 1.0), 2).mesh.bounds().unwrap();
        assert!(open.1[2] > closed.1[2] + 15.0, "the lid lifts off");
        let section = compose(&b, (CaseView::Section, 1.0), 3).mesh;
        assert!(section.triangles() < compose(&b, (CaseView::Closed, 1.0), 4).mesh.triangles());
        // D1 lit: its body's colours are lighter.
        let brightest = |b: &BoardBodies| {
            b.board
                .vertices
                .iter()
                .map(|v| u32::from(v.color[0]) + u32::from(v.color[1]) + u32::from(v.color[2]))
                .max()
                .unwrap()
        };
        let before =
            b.board.vertices.iter().filter(|v| v.color == circuiter_3d::color("red")).count();
        let lit = circuiter_pcb::board3d::Live {
            glow: [("D1".to_owned(), 1.0)].into(),
            ..Default::default()
        };
        b.relive(&design, &lit);
        let after =
            b.board.vertices.iter().filter(|v| v.color == circuiter_3d::color("red")).count();
        assert!(before > 0 && after < before, "the red lens is washed lighter: {before} → {after}");
        assert!(brightest(&b) > 0);
    }
}
