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
            editor: Default::default(),
        }
    }
}

impl Viewer3d {
    /// The scene for `cell` of `design`, rebuilt only when either changed.
    pub fn update(&mut self, design: &circuiter_model::Design, cell: &CellId, revision: u64) {
        let mode = self.body_mode;
        if self
            .scene
            .as_ref()
            .is_some_and(|s| s.cell == *cell && s.revision == revision && s.body_mode == mode)
        {
            return;
        }
        let refit = self.scene.as_ref().is_some_and(|s| s.body_mode != mode && s.cell == *cell);
        self.next_id += 1;
        let scene = build(design, cell, revision, self.next_id, mode);
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
        self.viewer3d.update(&ws.design, &cell, ws.revision);
        let Some(scene) = self.viewer3d.scene.clone() else { return };
        // The view's own toolbar.
        let mut export = false;
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
                        let mut mode = usize::from(self.viewer3d.body_mode);
                        if let Some(m) =
                            style::segmented(ui, &[("Sheet", true), ("Body", true)], mode)
                        {
                            mode = m;
                            self.viewer3d.body_mode = mode == 1;
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
            },
        );
        if export {
            self.export_stl(&scene);
        }
        if !scene.sheet {
            egui::Panel::left("body-panel")
                .resizable(true)
                .default_size(380.0)
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
        if scene.sheet {
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
}
