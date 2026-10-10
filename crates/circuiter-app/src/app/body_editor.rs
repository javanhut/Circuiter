//! Modelling a part's body: the feature list beside the 3D view, each feature's
//! dimensions and placement, and a sketch editor for extruded and revolved outlines.
//! Edits are made on a working copy and committed (undoably) when a field is left or a
//! control changes.

use super::CircuiterApp;
use crate::style::{self, icon};
use circuiter_3d::packages::PACKAGES;
use circuiter_model::CellId;
use circuiter_model::model3d::{Axis, Feature, Model3d, Operation, Shape, Sketch, SketchPoint};
use eframe::egui::{self, Color32, RichText, Stroke};

/// The editor's state.
#[derive(Debug, Default)]
pub struct BodyEditor {
    /// The working copy, the cell it belongs to, and the stored model it started from.
    draft: Option<(CellId, Model3d, Model3d)>,
    pub selected: Option<usize>,
    /// The sketch corner being dragged.
    dragging: Option<usize>,
}

const COLORS: &[&str] = &[
    "grey",
    "black",
    "white",
    "red",
    "orange",
    "yellow",
    "green",
    "blue",
    "darkblue",
    "lightblue",
    "violet",
    "brown",
    "beige",
    "silver",
    "aluminium",
    "gold",
    "copper",
    "clear",
];

fn s3(a: &str, b: &str, c: &str) -> [String; 3] {
    [a.into(), b.into(), c.into()]
}

/// A name not yet taken in `features`.
fn unique(features: &[Feature], base: &str) -> String {
    if !features.iter().any(|f| f.name == base) {
        return base.to_owned();
    }
    (2..)
        .map(|k| format!("{base} {k}"))
        .find(|n| !features.iter().any(|f| f.name == *n))
        .expect("a free name")
}

/// What a new 3D part starts as: a box to shape.
pub fn starting_body() -> Model3d {
    let mut body = new_feature("Box", &[], None);
    body.name = "Body".into();
    Model3d { features: vec![body], ..Model3d::default() }
}

/// The features the Add menu makes.
fn new_feature(kind: &str, features: &[Feature], selected: Option<&Feature>) -> Feature {
    let of = selected.map_or_else(
        || features.last().map(|f| f.name.clone()).unwrap_or_default(),
        |f| f.name.clone(),
    );
    let mut f = match kind {
        "Box" => {
            Feature::new("Box", Shape::Box { size: s3("20", "20", "10"), radius: String::new() })
        }
        "Rounded box" => Feature::new(
            "Rounded box",
            Shape::Box { size: s3("30", "20", "10"), radius: "3".into() },
        ),
        "Cylinder" => {
            Feature::new("Cylinder", Shape::Cylinder { radius: "5".into(), height: "10".into() })
        }
        "Cone" => Feature::new(
            "Cone",
            Shape::Cone { bottom: "5".into(), top: "2".into(), height: "10".into() },
        ),
        "Sphere" => Feature::new("Sphere", Shape::Sphere { radius: "5".into() }),
        "Extrude" => Feature::new(
            "Extrude",
            Shape::Extrude { sketch: Sketch::rectangle(20.0, 10.0), height: "5".into() },
        ),
        "Revolve" => {
            let profile = Sketch {
                points: vec![
                    SketchPoint::new(0.0, 0.0),
                    SketchPoint::new(6.0, 0.0),
                    SketchPoint::new(6.0, 2.0),
                    SketchPoint::new(3.0, 8.0),
                    SketchPoint::new(0.0, 8.0),
                ],
            };
            Feature::new("Revolve", Shape::Revolve { sketch: profile, angle: "360".into() })
        }
        "Hole" => {
            let mut h =
                Feature::new("Hole", Shape::Cylinder { radius: "1.5".into(), height: "40".into() });
            h.operation = Operation::Cut;
            h.at = s3("0", "0", "-20");
            h
        }
        "Copies" => Feature::new(
            "Copies",
            Shape::Copies { of: of.clone(), count: "3".into(), step: s3("10", "0", "0") },
        ),
        "Ring" => Feature::new(
            "Ring",
            Shape::Ring { of: of.clone(), count: "4".into(), angle: "360".into() },
        ),
        _ => Feature::new("Mirror", Shape::Mirror { of: of.clone(), axis: Axis::X }),
    };
    // A pattern does what the feature it repeats does: copies of a hole are holes.
    if matches!(f.shape, Shape::Copies { .. } | Shape::Ring { .. } | Shape::Mirror { .. }) {
        if let Some(src) = features.iter().find(|x| x.name == of) {
            f.operation = src.operation;
        }
    }
    f.name = unique(features, &f.name);
    f
}

/// A labelled single-line field; true when an edit is finished (focus left, or Enter).
fn field(ui: &mut egui::Ui, label: &str, value: &mut String, width: f32) -> bool {
    ui.label(RichText::new(label).small());
    let r = ui.add(
        egui::TextEdit::singleline(value).desired_width(width).font(egui::TextStyle::Monospace),
    );
    r.lost_focus()
}

/// Three fields in a row (x, y, z).
fn triple(ui: &mut egui::Ui, label: &str, v: &mut [String; 3]) -> bool {
    let mut done = false;
    ui.label(RichText::new(label).strong());
    ui.horizontal(|ui| {
        for (axis, value) in ["x", "y", "z"].iter().zip(v.iter_mut()) {
            done |= field(ui, axis, value, 78.0);
        }
    });
    done
}

impl CircuiterApp {
    /// The Body panel: the cell's package or modelled features, to edit.
    pub(super) fn body_panel(&mut self, ui: &mut egui::Ui) {
        let p = style::palette(ui);
        let Some(ws) = &mut self.workspace else { return };
        let cell = ws.current().cell.clone();
        let editable = ws.editable();
        let stored = ws.current_def().and_then(|d| d.model3d.clone()).unwrap_or_default();
        let errors: Vec<(usize, String)> =
            self.viewer3d.scene.as_ref().map(|s| s.feature_errors.clone()).unwrap_or_default();
        let ed = &mut self.viewer3d.editor;
        // Follow the stored model when the cell changes or something else changed it
        // (undo, another window).
        let stale = ed.draft.as_ref().is_none_or(|(c, _, base)| *c != cell || *base != stored);
        if stale {
            ed.draft = Some((cell.clone(), stored.clone(), stored.clone()));
            if ed.selected.is_some_and(|k| k >= stored.features.len()) {
                ed.selected = None;
            }
        }
        let (_, draft, _) = ed.draft.as_mut().expect("set");
        let mut commit = false;
        let symbol = ws.current_def().and_then(|d| d.symbol.clone());
        let mut import = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{}  Body", icon::CUBE)).strong().size(15.0));
            if !editable {
                ui.label(RichText::new("read-only").color(p.weak));
            }
            style::trailing(ui, |ui| {
                ui.add_enabled_ui(editable, |ui| {
                    ui.menu_button(format!("{}  From 2D", icon::POLYGON), |ui| {
                        if ui
                            .button("Import Outline (DXF, SVG)…")
                            .on_hover_text(
                                "Extrude a drawing's outline; its inner outlines become holes",
                            )
                            .clicked()
                        {
                            import = true;
                            ui.close();
                        }
                        if ui
                            .add_enabled(symbol.is_some(), egui::Button::new("From the Symbol"))
                            .on_hover_text(
                                "The symbol's outline as a body, its inner drawings raised on top",
                            )
                            .clicked()
                        {
                            if let Some(sym) = &symbol {
                                let loops = symbol_loops(sym);
                                if loops.is_empty() {
                                    self.error =
                                        Some("the symbol has no closed outline to extrude".into());
                                } else {
                                    let added =
                                        from_loops(&loops, &draft.features, "Symbol", "3", true);
                                    ed.selected = Some(draft.features.len());
                                    draft.features.extend(added);
                                    commit = true;
                                }
                            }
                            ui.close();
                        }
                    });
                });
            });
        });
        if import {
            let file = rfd::FileDialog::new().add_filter("Drawings", &["dxf", "svg"]).pick_file();
            if let Some(path) = file {
                let read = std::fs::read_to_string(&path)
                    .map_err(|e| format!("reading {}: {e}", path.display()))
                    .and_then(|text| {
                        circuiter_3d::import2d::outlines(&path.to_string_lossy(), &text)
                    });
                match read {
                    Ok(loops) => {
                        let name = path
                            .file_stem()
                            .map_or("Outline".into(), |s| s.to_string_lossy().into_owned());
                        let added = from_loops(&loops, &draft.features, &name, "5", false);
                        ed.selected = Some(draft.features.len());
                        draft.features.extend(added);
                        commit = true;
                    }
                    Err(e) => self.error = Some(e),
                }
            }
        }
        ui.add_space(4.0);
        if draft.features.is_empty() && draft.package.is_empty() {
            ui.label(RichText::new("No 3D body yet. Model one from features, as in a CAD program, or use one of the ready-made packages.").color(p.weak));
            ui.add_space(6.0);
            ui.add_enabled_ui(editable, |ui| {
                if ui.button(format!("{}  Model a body", icon::CUBE)).clicked() {
                    draft.features.push(new_feature("Box", &[], None));
                    ed.selected = Some(0);
                    commit = true;
                }
                egui::ComboBox::from_id_salt("package-pick")
                    .selected_text("Use a package…")
                    .show_ui(ui, |ui| {
                        for name in PACKAGES {
                            if ui.selectable_label(false, *name).clicked() {
                                draft.package = (*name).to_owned();
                                commit = true;
                            }
                        }
                    });
            });
        } else if draft.features.is_empty() {
            // A package: its name and parameters.
            ui.label(RichText::new("Made by a package generator").color(p.weak));
            ui.add_enabled_ui(editable, |ui| {
                egui::ComboBox::from_id_salt("package").selected_text(draft.package.clone()).show_ui(ui, |ui| {
                    for name in PACKAGES {
                        if ui.selectable_label(draft.package == *name, *name).clicked() {
                            draft.package = (*name).to_owned();
                            commit = true;
                        }
                    }
                });
                let keys: Vec<String> = draft.params.keys().cloned().collect();
                egui::Grid::new("package-params").num_columns(2).show(ui, |ui| {
                    for k in keys {
                        ui.label(&k);
                        let v = draft.params.get_mut(&k).expect("key");
                        let r = ui.add(egui::TextEdit::singleline(v).desired_width(140.0));
                        commit |= r.lost_focus();
                        ui.end_row();
                    }
                });
                ui.add_space(6.0);
                if ui
                    .button("Model it yourself instead")
                    .on_hover_text("Replace the package with features you can shape: holes, cut-outs, your own outline")
                    .clicked()
                {
                    draft.package.clear();
                    draft.params.clear();
                    draft.features.push(new_feature("Box", &[], None));
                    ed.selected = Some(0);
                    commit = true;
                }
            });
        } else {
            // With a feature open, the list scrolls on its own so the editor stays in view.
            let list_height = if ed.selected.is_some() { 230.0 } else { f32::INFINITY };
            egui::ScrollArea::vertical().id_salt("features").max_height(list_height).show(
                ui,
                |ui| {
                    commit |= feature_list(ui, draft, &mut ed.selected, &errors, editable);
                },
            );
            if let Some(k) = ed.selected.filter(|k| *k < draft.features.len()) {
                ui.separator();
                let names: Vec<String> =
                    draft.features[..k].iter().map(|f| f.name.clone()).collect();
                ui.add_enabled_ui(editable, |ui| {
                    commit |= feature_editor(ui, &mut draft.features[k], &names, &mut ed.dragging);
                });
            } else {
                ui.label(RichText::new("Select a feature to change it.").color(p.weak));
            }
        }
        if commit && editable {
            let model = draft.clone();
            let result = ws.edit(|_, _, def| {
                def.model3d =
                    (!model.features.is_empty() || !model.package.is_empty()).then_some(model);
                Ok(())
            });
            if let Err(e) = result {
                self.error = Some(e);
            }
            if let Some((_, d, base)) = &mut self.viewer3d.editor.draft {
                *base = d.clone();
            }
        }
    }
}

/// The features in order, with show/hide, reorder and delete, and the Add menu.
fn feature_list(
    ui: &mut egui::Ui,
    model: &mut Model3d,
    selected: &mut Option<usize>,
    errors: &[(usize, String)],
    editable: bool,
) -> bool {
    let p = style::palette(ui);
    let mut commit = false;
    let mut action: Option<(usize, &str)> = None;
    for (k, f) in model.features.iter().enumerate() {
        ui.horizontal(|ui| {
            let eye = if f.hidden { icon::EYE_SLASH } else { icon::EYE };
            if ui
                .add_enabled(editable, egui::Button::new(eye).frame(false))
                .on_hover_text("Show or leave out")
                .clicked()
            {
                action = Some((k, "hide"));
            }
            let op = match f.operation {
                Operation::Add => "+",
                Operation::Cut => "−",
                Operation::Intersect => "∩",
            };
            let text = RichText::new(format!("{op}  {}", f.name)).color(if f.hidden {
                p.weak
            } else {
                p.text
            });
            if ui.selectable_label(*selected == Some(k), text).clicked() {
                *selected = Some(k);
            }
            style::trailing(ui, |ui| {
                ui.add_enabled_ui(editable, |ui| {
                    if ui.small_button(icon::X).on_hover_text("Delete").clicked() {
                        action = Some((k, "delete"));
                    }
                    if ui.small_button(icon::ARROW_DOWN).on_hover_text("Later").clicked() {
                        action = Some((k, "down"));
                    }
                    if ui.small_button(icon::ARROW_UP).on_hover_text("Earlier").clicked() {
                        action = Some((k, "up"));
                    }
                });
            });
        });
        for (_, e) in errors.iter().filter(|(i, _)| *i == k) {
            ui.label(RichText::new(e).small().color(p.error));
        }
    }
    if let Some((k, what)) = action {
        match what {
            "hide" => model.features[k].hidden = !model.features[k].hidden,
            "delete" => {
                model.features.remove(k);
                *selected = None;
            }
            "up" if k > 0 => {
                model.features.swap(k, k - 1);
                *selected = Some(k - 1);
            }
            "down" if k + 1 < model.features.len() => {
                model.features.swap(k, k + 1);
                *selected = Some(k + 1);
            }
            _ => {}
        }
        commit = true;
    }
    ui.add_space(4.0);
    ui.add_enabled_ui(editable, |ui| {
        ui.menu_button(format!("{}  Add feature", icon::PLUS), |ui| {
            for kind in
                ["Box", "Rounded box", "Cylinder", "Cone", "Sphere", "Extrude", "Revolve", "Hole"]
            {
                if ui.button(kind).clicked() {
                    let f = new_feature(kind, &model.features, None);
                    model.features.push(f);
                    *selected = Some(model.features.len() - 1);
                    commit = true;
                }
            }
            ui.separator();
            let has_any = !model.features.is_empty();
            for kind in ["Copies", "Ring", "Mirror"] {
                if ui.add_enabled(has_any, egui::Button::new(kind)).clicked() {
                    let current = selected.and_then(|k| model.features.get(k)).cloned();
                    let f = new_feature(kind, &model.features, current.as_ref());
                    model.features.push(f);
                    *selected = Some(model.features.len() - 1);
                    commit = true;
                }
            }
        });
    });
    commit
}

/// One feature's settings; true when an edit is finished.
fn feature_editor(
    ui: &mut egui::Ui,
    f: &mut Feature,
    earlier: &[String],
    dragging: &mut Option<usize>,
) -> bool {
    let mut done = false;
    egui::Grid::new("feature-head").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("Name");
        done |= ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(160.0)).lost_focus();
        ui.end_row();
        ui.label("Does");
        egui::ComboBox::from_id_salt("feature-op")
            .selected_text(match f.operation {
                Operation::Add => "Add",
                Operation::Cut => "Cut",
                Operation::Intersect => "Intersect",
            })
            .show_ui(ui, |ui| {
                for (op, label) in [
                    (Operation::Add, "Add"),
                    (Operation::Cut, "Cut"),
                    (Operation::Intersect, "Intersect"),
                ] {
                    if ui.selectable_label(f.operation == op, label).clicked() {
                        f.operation = op;
                        done = true;
                    }
                }
            });
        ui.end_row();
        ui.label("Colour");
        let shown = if f.color.is_empty() { "the body's".to_owned() } else { f.color.clone() };
        egui::ComboBox::from_id_salt("feature-color").selected_text(shown).show_ui(ui, |ui| {
            if ui.selectable_label(f.color.is_empty(), "the body's").clicked() {
                f.color.clear();
                done = true;
            }
            for c in COLORS {
                let [r, g, b, _] = circuiter_3d::color(c);
                let swatch = RichText::new(format!("■  {c}")).color(Color32::from_rgb(r, g, b));
                if ui.selectable_label(f.color == *c, swatch).clicked() {
                    f.color = (*c).to_owned();
                    done = true;
                }
            }
        });
        ui.end_row();
    });
    ui.add_space(4.0);
    let placed_by_others =
        matches!(f.shape, Shape::Copies { .. } | Shape::Ring { .. } | Shape::Mirror { .. });
    let of_picker = |ui: &mut egui::Ui, of: &mut String| -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Of");
            egui::ComboBox::from_id_salt("feature-of").selected_text(of.clone()).show_ui(
                ui,
                |ui| {
                    for n in earlier {
                        if ui.selectable_label(of == n, n).clicked() {
                            *of = n.clone();
                            changed = true;
                        }
                    }
                },
            );
        });
        changed
    };
    match &mut f.shape {
        Shape::Box { size, radius } => {
            ui.horizontal(|ui| {
                done |= field(ui, "width", &mut size[0], 70.0);
                done |= field(ui, "depth", &mut size[1], 70.0);
                done |= field(ui, "height", &mut size[2], 70.0);
            });
            ui.horizontal(|ui| done |= field(ui, "corner radius", radius, 70.0));
        }
        Shape::Cylinder { radius, height } => {
            ui.horizontal(|ui| {
                done |= field(ui, "radius", radius, 120.0);
                done |= field(ui, "height", height, 120.0);
            });
        }
        Shape::Cone { bottom, top, height } => {
            ui.horizontal(|ui| {
                done |= field(ui, "bottom radius", bottom, 62.0);
                done |= field(ui, "top", top, 62.0);
                done |= field(ui, "height", height, 62.0);
            });
        }
        Shape::Sphere { radius } => {
            ui.horizontal(|ui| done |= field(ui, "radius", radius, 120.0));
        }
        Shape::Extrude { sketch, height } => {
            ui.horizontal(|ui| done |= field(ui, "height", height, 120.0));
            done |= sketch_editor(ui, sketch, dragging, false);
        }
        Shape::Revolve { sketch, angle } => {
            ui.horizontal(|ui| done |= field(ui, "angle (°)", angle, 120.0));
            ui.label(RichText::new("The outline turns about the vertical axis on the left: x is the distance from it.").small());
            done |= sketch_editor(ui, sketch, dragging, true);
        }
        Shape::Copies { of, count, step } => {
            done |= of_picker(ui, of);
            ui.horizontal(|ui| done |= field(ui, "count (with the original)", count, 70.0));
            done |= triple(ui, "Each copy moves by", step);
        }
        Shape::Ring { of, count, angle } => {
            done |= of_picker(ui, of);
            ui.horizontal(|ui| {
                done |= field(ui, "count", count, 70.0);
                done |= field(ui, "over (°)", angle, 70.0);
            });
            ui.label(RichText::new("Copies around the vertical axis through the origin.").small());
        }
        Shape::Mirror { of, axis } => {
            done |= of_picker(ui, of);
            ui.horizontal(|ui| {
                ui.label("across the plane square to");
                for (a, label) in [(Axis::X, "X"), (Axis::Y, "Y"), (Axis::Z, "Z")] {
                    if ui.selectable_label(*axis == a, label).clicked() {
                        *axis = a;
                        done = true;
                    }
                }
            });
        }
    }
    if !placed_by_others {
        ui.add_space(4.0);
        done |= triple(ui, "At (mm)", &mut f.at);
        done |= triple(ui, "Turned (° about x, y, z)", &mut f.turn);
    }
    ui.label(
        RichText::new("Values are expressions in millimetres and may use the cell's parameters (width/2 + 3).")
            .small()
            .weak(),
    );
    done
}

/// Features made from outlines: the largest extruded `height` high, centred on the
/// origin; the others cut through it as holes, or, with `raised`, standing half a
/// millimetre proud of its top (a symbol's markings).
pub fn from_loops(
    loops: &[circuiter_3d::import2d::Loop],
    existing: &[Feature],
    name: &str,
    height: &str,
    raised: bool,
) -> Vec<Feature> {
    use circuiter_3d::import2d;
    let Some(outer) = loops.first() else { return Vec::new() };
    let shift = import2d::centre(outer);
    let mut all: Vec<Feature> = existing.to_vec();
    let mut out = Vec::new();
    let base = unique(&all, name);
    let f = Feature::new(
        &base,
        Shape::Extrude { sketch: import2d::sketch(outer, shift), height: height.into() },
    );
    all.push(f.clone());
    out.push(f);
    for (k, l) in loops.iter().enumerate().skip(1) {
        let label = if raised { format!("{base} mark {k}") } else { format!("{base} hole {k}") };
        let shape = if raised {
            Shape::Extrude { sketch: import2d::sketch(l, shift), height: "0.5".into() }
        } else {
            Shape::Extrude { sketch: import2d::sketch(l, shift), height: format!("{height} + 2") }
        };
        let mut f = Feature::new(&unique(&all, &label), shape);
        if raised {
            f.at = s3("0", "0", height);
            f.color = "white".into();
        } else {
            f.operation = Operation::Cut;
            f.at = s3("0", "0", "-1");
        }
        all.push(f.clone());
        out.push(f);
    }
    out
}

/// A symbol's closed outlines (rectangles, circles, closed or filled polylines), in
/// millimetres with Y up.
pub fn symbol_loops(symbol: &circuiter_model::symbol::Symbol) -> Vec<circuiter_3d::import2d::Loop> {
    use circuiter_model::symbol::Graphic;
    let mut loops = Vec::new();
    for g in &symbol.graphics {
        match g {
            Graphic::Rect { a, b, .. } => {
                let (x0, y0, x1, y1) = (a.x.mm(), -a.y.mm(), b.x.mm(), -b.y.mm());
                loops.push(vec![(x0, y0, 0.0), (x1, y0, 0.0), (x1, y1, 0.0), (x0, y1, 0.0)]);
            }
            Graphic::Circle { center, radius, .. } => {
                let (x, y, r) = (center.x.mm(), -center.y.mm(), radius.mm());
                loops.push(vec![(x - r, y, 1.0), (x + r, y, 1.0)]);
            }
            Graphic::Polyline { points, fill }
                if points.len() >= 3 && (*fill || points.first() == points.last()) =>
            {
                let mut l: Vec<(f64, f64, f64)> =
                    points.iter().map(|p| (p.x.mm(), -p.y.mm(), 0.0)).collect();
                if points.first() == points.last() {
                    l.pop();
                }
                loops.push(l);
            }
            _ => {}
        }
    }
    loops.retain(|l| circuiter_3d::import2d::area(l).abs() > 1e-6 || l.len() == 2);
    loops.sort_by(|a, b| {
        let size = |l: &circuiter_3d::import2d::Loop| {
            if l.len() == 2 {
                std::f64::consts::PI * ((l[1].0 - l[0].0) / 2.0).powi(2)
            } else {
                circuiter_3d::import2d::area(l).abs()
            }
        };
        size(b).total_cmp(&size(a))
    });
    loops
}

/// A named sketch to start from.
type Preset = (&'static str, fn() -> Sketch);

/// A sketch's outline to edit: drag corners (snapping to 0.5 mm), click an edge's
/// middle to add a corner, right-click a corner to remove it; presets and a table of
/// the corners below. True when an edit is finished.
fn sketch_editor(
    ui: &mut egui::Ui,
    sketch: &mut Sketch,
    dragging: &mut Option<usize>,
    revolve: bool,
) -> bool {
    let p = style::palette(ui);
    let mut done = false;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Outline").strong());
        let presets: &[Preset] = if revolve {
            &[("Profile", || Sketch {
                points: vec![
                    SketchPoint::new(0.0, 0.0),
                    SketchPoint::new(6.0, 0.0),
                    SketchPoint::new(6.0, 2.0),
                    SketchPoint::new(3.0, 8.0),
                    SketchPoint::new(0.0, 8.0),
                ],
            })]
        } else {
            &[
                ("Rectangle", || Sketch::rectangle(20.0, 10.0)),
                ("Circle", || Sketch::circle(5.0)),
                ("Slot", || Sketch::slot(12.0, 4.0)),
                ("Hexagon", || Sketch::polygon(6, 5.0)),
            ]
        };
        for (name, make) in presets {
            if ui.small_button(*name).clicked() {
                *sketch = make();
                done = true;
            }
        }
    });
    let vars = |_: &str| None;
    let corners: Vec<Option<(f32, f32)>> = sketch
        .points
        .iter()
        .map(|q| {
            Some((
                circuiter_3d::cad::value(&q.x, &vars).ok()?,
                circuiter_3d::cad::value(&q.y, &vars).ok()?,
            ))
        })
        .collect();
    let outline = circuiter_3d::cad::outline(sketch, &vars).unwrap_or_default();
    // Fit the view to the outline (and the axis, for a revolve).
    let mut lo = (f32::MAX, f32::MAX);
    let mut hi = (f32::MIN, f32::MIN);
    for &(x, y) in outline.iter().chain(revolve.then_some((0.0, 0.0)).iter()) {
        lo = (lo.0.min(x), lo.1.min(y));
        hi = (hi.0.max(x), hi.1.max(y));
    }
    if lo.0 > hi.0 {
        lo = (-10.0, -10.0);
        hi = (10.0, 10.0);
    }
    let size = egui::vec2(ui.available_width().min(320.0), 220.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, p.well);
    let span = ((hi.0 - lo.0).max(hi.1 - lo.1)).max(1.0) * 1.3;
    let scale = rect.width().min(rect.height()) / span;
    let mid = ((lo.0 + hi.0) / 2.0, (lo.1 + hi.1) / 2.0);
    let to_screen =
        |x: f32, y: f32| rect.center() + egui::vec2((x - mid.0) * scale, -(y - mid.1) * scale);
    let to_model = |pos: egui::Pos2| {
        (mid.0 + (pos.x - rect.center().x) / scale, mid.1 - (pos.y - rect.center().y) / scale)
    };
    // A grid at a step that keeps lines a sensible distance apart.
    let step = [0.5f32, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0]
        .into_iter()
        .find(|s| s * scale >= 12.0)
        .unwrap_or(100.0);
    let grid = Stroke::new(1.0, p.weak.gamma_multiply(0.15));
    let (m0, m1) = (to_model(rect.left_bottom()), to_model(rect.right_top()));
    let mut x = (m0.0 / step).floor() * step;
    while x <= m1.0 {
        let s = to_screen(x, 0.0).x;
        painter.line_segment([egui::pos2(s, rect.top()), egui::pos2(s, rect.bottom())], grid);
        x += step;
    }
    let mut y = (m0.1 / step).floor() * step;
    while y <= m1.1 {
        let s = to_screen(0.0, y).y;
        painter.line_segment([egui::pos2(rect.left(), s), egui::pos2(rect.right(), s)], grid);
        y += step;
    }
    let axis = Stroke::new(1.0, p.accent.gamma_multiply(if revolve { 0.9 } else { 0.4 }));
    let o = to_screen(0.0, 0.0);
    painter.line_segment([egui::pos2(o.x, rect.top()), egui::pos2(o.x, rect.bottom())], axis);
    painter.line_segment(
        [egui::pos2(rect.left(), o.y), egui::pos2(rect.right(), o.y)],
        Stroke::new(1.0, p.accent.gamma_multiply(0.4)),
    );
    // The outline, filled.
    let pts: Vec<egui::Pos2> = outline.iter().map(|&(x, y)| to_screen(x, y)).collect();
    if pts.len() >= 3 {
        let mut closed = pts.clone();
        closed.push(pts[0]);
        painter.add(egui::Shape::line(closed, Stroke::new(2.0, p.accent)));
    }
    painter.text(
        rect.left_top() + egui::vec2(6.0, 4.0),
        egui::Align2::LEFT_TOP,
        format!("grid {step} mm"),
        egui::FontId::proportional(10.0),
        p.weak,
    );
    // Dimensions: each straight edge's length beside it, and the overall size.
    let n_pts = sketch.points.len();
    for k in 0..n_pts {
        let (Some(a), Some(b)) = (corners[k], corners[(k + 1) % n_pts]) else { continue };
        if !sketch.points[k].bulge.trim().is_empty() {
            continue;
        }
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        if len * scale < 28.0 {
            continue;
        }
        let m = to_screen((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        // Outside the outline: along the edge's normal, away from the middle.
        let normal = egui::vec2(b.1 - a.1, a.0 - b.0).normalized();
        let outward = if (m - to_screen(mid.0, mid.1)).dot(egui::vec2(normal.x, -normal.y)) >= 0.0 {
            1.0
        } else {
            -1.0
        };
        let at = m + egui::vec2(normal.x, -normal.y) * 9.0 * outward;
        painter.text(
            at,
            egui::Align2::CENTER_CENTER,
            format!("{len:.1}"),
            egui::FontId::monospace(9.5),
            p.weak,
        );
    }
    if lo.0 < hi.0 {
        painter.text(
            rect.right_bottom() + egui::vec2(-6.0, -4.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("{:.1} × {:.1} mm", hi.0 - lo.0, hi.1 - lo.1),
            egui::FontId::monospace(10.0),
            p.weak,
        );
    }
    // Corner handles and edge midpoints.
    let pointer = response.interact_pointer_pos().or(response.hover_pos());
    let near = |a: egui::Pos2| pointer.is_some_and(|q| q.distance(a) < 7.0);
    let n = sketch.points.len();
    let mut hovered_corner = None;
    for (k, c) in corners.iter().enumerate() {
        let Some((x, y)) = *c else { continue };
        let at = to_screen(x, y);
        let hot = near(at) || *dragging == Some(k);
        if near(at) {
            hovered_corner = Some(k);
        }
        painter.circle(
            at,
            if hot { 5.5 } else { 4.0 },
            if hot { p.accent } else { p.panel },
            Stroke::new(1.5, p.accent),
        );
    }
    let mut hovered_edge = None;
    if hovered_corner.is_none() {
        for k in 0..n {
            let (Some(a), Some(b)) = (corners[k], corners[(k + 1) % n]) else { continue };
            if !sketch.points[k].bulge.trim().is_empty() {
                continue;
            }
            let m = to_screen((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            if near(m) {
                hovered_edge = Some(k);
                painter.circle_stroke(m, 4.0, Stroke::new(1.5, p.go));
                painter.text(
                    m + egui::vec2(6.0, -6.0),
                    egui::Align2::LEFT_BOTTOM,
                    "+",
                    egui::FontId::proportional(13.0),
                    p.go,
                );
            }
        }
    }
    let snap = |v: f32| (v * 2.0).round() / 2.0;
    let fmt = |v: f32| {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    if response.drag_started() {
        *dragging = hovered_corner;
    }
    if let (Some(k), Some(pos)) = (*dragging, response.interact_pointer_pos()) {
        if response.dragged() {
            let (x, y) = to_model(pos);
            let x = if revolve { x.max(0.0) } else { x };
            sketch.points[k].x = fmt(snap(x));
            sketch.points[k].y = fmt(snap(y));
        }
    }
    if response.drag_stopped() && dragging.take().is_some() {
        done = true;
    }
    if response.clicked() {
        if let Some(k) = hovered_edge {
            let (a, b) = (corners[k].expect("checked"), corners[(k + 1) % n].expect("checked"));
            let mid = SketchPoint {
                x: fmt(snap((a.0 + b.0) / 2.0)),
                y: fmt(snap((a.1 + b.1) / 2.0)),
                bulge: String::new(),
            };
            sketch.points.insert(k + 1, mid);
            done = true;
        }
    }
    if response.secondary_clicked() {
        if let Some(k) = hovered_corner.filter(|_| n > 3) {
            sketch.points.remove(k);
            done = true;
        }
    }
    response
        .on_hover_text("Drag a corner; click a + to add one; right-click a corner to remove it");
    // The size and each edge's length and direction, to type in.
    egui::CollapsingHeader::new("Dimensions").default_open(true).show(ui, |ui| {
        if lo.0 < hi.0 {
            let (mut w, mut h) = (hi.0 - lo.0, hi.1 - lo.1);
            ui.horizontal(|ui| {
                ui.label("Size");
                let a = ui.add(
                    egui::DragValue::new(&mut w).range(0.1..=10000.0).speed(0.1).suffix(" mm"),
                );
                ui.label("×");
                let b = ui.add(
                    egui::DragValue::new(&mut h).range(0.1..=10000.0).speed(0.1).suffix(" mm"),
                );
                if a.changed() || b.changed() {
                    set_size(sketch, &corners, (lo, hi), (w, h));
                    done = true;
                }
            });
        }
        egui::Grid::new("sketch-edges").num_columns(3).show(ui, |ui| {
            ui.label(RichText::new("edge").small());
            ui.label(RichText::new("length").small());
            ui.label(RichText::new("direction").small());
            ui.end_row();
            let n = sketch.points.len();
            for k in 0..n {
                let (Some(a), Some(b)) = (corners[k], corners[(k + 1) % n]) else { continue };
                ui.label(RichText::new(format!("{} → {}", k + 1, (k + 1) % n + 1)).small());
                if !sketch.points[k].bulge.trim().is_empty() {
                    ui.label(RichText::new("arc").small().weak());
                    ui.label("");
                    ui.end_row();
                    continue;
                }
                let mut len = (b.0 - a.0).hypot(b.1 - a.1);
                let mut angle = (b.1 - a.1).atan2(b.0 - a.0).to_degrees();
                let l = ui.add(
                    egui::DragValue::new(&mut len).range(0.01..=10000.0).speed(0.1).suffix(" mm"),
                );
                let d = ui.add(
                    egui::DragValue::new(&mut angle).range(-180.0..=180.0).speed(1.0).suffix("°"),
                );
                if l.changed() || d.changed() {
                    set_edge(sketch, &corners, k, len, angle);
                    done = true;
                }
                ui.end_row();
            }
        });
    });
    // The corners as numbers.
    egui::CollapsingHeader::new("Corners").default_open(false).show(ui, |ui| {
        egui::Grid::new("sketch-corners").num_columns(3).show(ui, |ui| {
            ui.label(RichText::new("x").small());
            ui.label(RichText::new("y").small());
            ui.label(RichText::new("arc to next (bulge)").small());
            ui.end_row();
            for q in &mut sketch.points {
                for v in [&mut q.x, &mut q.y, &mut q.bulge] {
                    done |= ui
                        .add(
                            egui::TextEdit::singleline(v)
                                .desired_width(58.0)
                                .font(egui::TextStyle::Monospace),
                        )
                        .lost_focus();
                }
                ui.end_row();
            }
        });
    });
    done
}

/// Gives edge `k` (corner `k` to the next) this length and direction, moving the
/// corners after it with its end, so the rest of the outline keeps its shape; the
/// closing edge takes up the change.
pub fn set_edge(
    sketch: &mut Sketch,
    corners: &[Option<(f32, f32)>],
    k: usize,
    len: f32,
    degrees: f32,
) {
    let n = sketch.points.len();
    let (Some(a), Some(b)) = (corners[k], corners[(k + 1) % n]) else { return };
    let r = degrees.to_radians();
    let to = (a.0 + len * r.cos(), a.1 + len * r.sin());
    let delta = (to.0 - b.0, to.1 - b.1);
    let fmt = |v: f32| {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    // Corner k+1 and those after it, up to the last (corner 0 stays: it starts the
    // outline).
    for (q, c) in sketch.points.iter_mut().zip(corners).skip(k + 1) {
        if let Some(c) = c {
            q.x = fmt(c.0 + delta.0);
            q.y = fmt(c.1 + delta.1);
        }
    }
}

/// Scales the outline about its middle to `size` (width, height), from its bounds now.
pub fn set_size(
    sketch: &mut Sketch,
    corners: &[Option<(f32, f32)>],
    (lo, hi): ((f32, f32), (f32, f32)),
    size: (f32, f32),
) {
    let (w, h) = (hi.0 - lo.0, hi.1 - lo.1);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let (sx, sy) = (size.0 / w, size.1 / h);
    let c = ((lo.0 + hi.0) / 2.0, (lo.1 + hi.1) / 2.0);
    let fmt = |v: f32| {
        let s = format!("{v:.3}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    };
    for (q, corner) in sketch.points.iter_mut().zip(corners) {
        if let Some((x, y)) = *corner {
            q.x = fmt(c.0 + (x - c.0) * sx);
            q.y = fmt(c.1 + (y - c.1) * sy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sketches_take_dimensions() {
        let mut s = Sketch::rectangle(20.0, 10.0);
        let corners = |s: &Sketch| -> Vec<Option<(f32, f32)>> {
            s.points.iter().map(|q| Some((q.x.parse().unwrap(), q.y.parse().unwrap()))).collect()
        };
        // The bottom edge (corner 1 → 2) made 30 long: the right side moves with it.
        let c = corners(&s);
        set_edge(&mut s, &c, 0, 30.0, 0.0);
        let c = corners(&s);
        assert_eq!(c[1], Some((20.0, -5.0)));
        assert_eq!(c[2], Some((20.0, 5.0)), "the right side kept its shape");
        assert_eq!(c[0], Some((-10.0, -5.0)), "the start stays");
        // Then the whole made 40 × 20 about its middle.
        let c = corners(&s);
        set_size(&mut s, &c, ((-10.0, -5.0), (20.0, 5.0)), (40.0, 20.0));
        let c = corners(&s);
        assert_eq!(c[0], Some((-15.0, -10.0)));
        assert_eq!(c[2], Some((25.0, 10.0)));
    }

    /// A volume from a closed mesh's triangles.
    fn volume(m: &circuiter_3d::Mesh) -> f32 {
        m.indices
            .chunks(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| m.vertices[i as usize].pos);
                (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                    + a[2] * (b[0] * c[1] - b[1] * c[0]))
                    / 6.0
            })
            .sum()
    }

    /// A drawn plate with a hole becomes a body with the hole through it; a symbol's
    /// box a body with its markings raised.
    #[test]
    fn bodies_come_from_drawings_and_symbols() {
        let svg = r#"<svg width="40mm" height="20mm" viewBox="0 0 40 20"><rect x="0" y="0" width="40" height="20"/><circle cx="10" cy="10" r="3"/></svg>"#;
        let loops = circuiter_3d::import2d::outlines("plate.svg", svg).unwrap();
        let fs = from_loops(&loops, &[], "plate", "5", false);
        assert_eq!(fs.len(), 2);
        assert_eq!(fs[1].operation, Operation::Cut);
        let built = circuiter_3d::cad::evaluate(&fs, &|_| None);
        assert!(built.errors.is_empty(), "{:?}", built.errors);
        let want = (800.0 - std::f32::consts::PI * 9.0) * 5.0;
        assert!(
            (volume(&built.mesh) - want).abs() < want * 0.01,
            "{} vs {want}",
            volume(&built.mesh)
        );
        let (lo, hi) = built.mesh.bounds().unwrap();
        assert!((lo[0] + 20.0).abs() < 1e-3 && (hi[0] - 20.0).abs() < 1e-3, "centred");

        let lib = circuiter_stdlib::library();
        let chip = lib.cell("ne555").unwrap().symbol.as_ref().unwrap();
        let loops = symbol_loops(chip);
        assert!(!loops.is_empty());
        let fs = from_loops(&loops, &[], "Symbol", "3", true);
        let built = circuiter_3d::cad::evaluate(&fs, &|_| None);
        assert!(built.errors.is_empty() && !built.mesh.vertices.is_empty());

        // A board's outline from a drawing: top-left at the origin, Y down.
        let outline = super::super::pcb::board_outline(&loops_of("plate.svg", svg)[0]);
        let xs: Vec<f64> = outline.iter().map(|p| p.x.mm()).collect();
        let ys: Vec<f64> = outline.iter().map(|p| p.y.mm()).collect();
        assert!(
            xs.iter().all(|&x| x > -1e-9 && x < 40.0 + 1e-9)
                && ys.iter().all(|&y| y > -1e-9 && y < 20.0 + 1e-9)
        );
    }

    fn loops_of(name: &str, text: &str) -> Vec<circuiter_3d::import2d::Loop> {
        circuiter_3d::import2d::outlines(name, text).unwrap()
    }

    #[test]
    fn new_features_get_unique_names_and_patterns_copy_the_operation() {
        let mut fs = vec![new_feature("Box", &[], None)];
        fs.push(new_feature("Box", &fs, None));
        assert_eq!(fs[1].name, "Box 2");
        fs.push(new_feature("Hole", &fs, None));
        let ring = new_feature("Ring", &fs, fs.last());
        assert_eq!(ring.operation, Operation::Cut);
        assert!(matches!(&ring.shape, Shape::Ring { of, .. } if of == "Hole"));
    }
}
