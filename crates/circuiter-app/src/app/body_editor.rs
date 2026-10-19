//! Modelling a part's body: the feature list beside the 3D view, each feature's
//! dimensions and placement, and a sketch editor for extruded and revolved outlines.
//! Edits are made on a working copy and committed (undoably) when a field is left or a
//! control changes.

use super::CircuiterApp;
use crate::style::{self, icon};
use circuiter_3d::packages::PACKAGES;
use circuiter_3d::sketch::{self as sk, P, Prim, Snap};
use circuiter_model::CellId;
use circuiter_model::model3d::{
    Axis, Constraint, Curve, Drawing, Ends, Feature, Model3d, Operation, Shape, Sketch, SketchPoint,
};
use eframe::egui::{self, Color32, RichText, Stroke};

/// The editor's state.
#[derive(Debug, Default)]
pub struct BodyEditor {
    /// The working copy, the cell it belongs to, and the stored model it started from.
    draft: Option<(CellId, Model3d, Model3d)>,
    pub selected: Option<usize>,
    /// The sketch open for drawing, and the drawing tools' state.
    pub pad: Pad,
}

impl BodyEditor {
    /// A feature drawn from a sketch is open and the sketch is to have most of the view.
    pub fn sketching(&self) -> bool {
        let feature = self.selected.and_then(|k| self.draft.as_ref()?.1.features.get(k));
        !self.pad.narrow
            && feature.is_some_and(|f| {
                matches!(
                    f.shape,
                    Shape::Extrude { .. }
                        | Shape::Revolve { .. }
                        | Shape::Sweep { .. }
                        | Shape::Loft { .. }
                )
            })
    }
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
        "Extrude" => {
            let mut f = Feature::new(
                "Extrude",
                Shape::Extrude { sketch: Sketch::default(), height: "5".into() },
            );
            let mut d = Drawing::default();
            sk::rectangle(&mut d, [-10.0, -5.0], [10.0, 5.0]);
            f.drawing = Some(d);
            f
        }
        "Revolve" => {
            let mut f = Feature::new(
                "Revolve",
                Shape::Revolve { sketch: Sketch::default(), angle: "360".into() },
            );
            f.drawing = Some(revolve_profile());
            f
        }
        "Sweep" => {
            let mut f = Feature::new(
                "Sweep",
                Shape::Sweep { sketch: Sketch::default(), path: bend_path() },
            );
            let mut d = Drawing::default();
            let c = sk::add_point(&mut d, [0.0, 0.0]);
            d.curves.push(Curve::Circle { center: c, radius: "2".into() });
            f.drawing = Some(d);
            f
        }
        "Loft" => {
            let mut to = Drawing::default();
            let c = sk::add_point(&mut to, [0.0, 0.0]);
            to.curves.push(Curve::Circle { center: c, radius: "5".into() });
            let mut f = Feature::new(
                "Loft",
                Shape::Loft { sketch: Sketch::default(), to, height: "15".into() },
            );
            let mut d = Drawing::default();
            sk::rectangle(&mut d, [-10.0, -10.0], [10.0, 10.0]);
            f.drawing = Some(d);
            f
        }
        "Shell" => {
            let mut f = Feature::new(
                "Shell",
                Shape::Shell { of: of.clone(), thickness: "2".into(), open: Some(Ends::Top) },
            );
            f.operation = Operation::Cut;
            f
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
        // The cell's parameters, for expressions in sketches.
        let params: Vec<(String, f64)> = ws.current_def().map_or_else(Vec::new, |d| {
            d.cell
                .params
                .iter()
                .filter_map(|p| {
                    let v =
                        circuiter_model::units::parse_quantity(p.default.as_ref()?, None).ok()?;
                    Some((p.name.clone(), v))
                })
                .collect()
        });
        let vars = |n: &str| params.iter().find(|p| p.0 == n).map(|p| p.1);
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
        let sketching = ed.sketching();
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
            // With a feature open, the list scrolls on its own so the editor stays in view;
            // a few rows of it while sketching, so the drawing has the room.
            let list_height = match (sketching, ed.selected.is_some()) {
                (true, _) => 110.0,
                (false, true) => 230.0,
                (false, false) => f32::INFINITY,
            };
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
                    commit |=
                        feature_editor(ui, &mut draft.features[k], &names, k, &mut ed.pad, &vars);
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
        // Undo and redo typed or pressed in the sketch's command line.
        if let (Some(redo), Some(ws)) =
            (self.viewer3d.editor.pad.history.take(), &mut self.workspace)
        {
            if redo { ws.redo() } else { ws.undo() }
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
            for kind in [
                "Box",
                "Rounded box",
                "Cylinder",
                "Cone",
                "Sphere",
                "Extrude",
                "Revolve",
                "Sweep",
                "Loft",
                "Hole",
            ] {
                if ui.button(kind).clicked() {
                    let f = new_feature(kind, &model.features, None);
                    model.features.push(f);
                    *selected = Some(model.features.len() - 1);
                    commit = true;
                }
            }
            ui.separator();
            let has_any = !model.features.is_empty();
            for kind in ["Copies", "Ring", "Mirror", "Shell"] {
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

/// One feature's settings (it is feature `k`); true when an edit is finished.
fn feature_editor(
    ui: &mut egui::Ui,
    f: &mut Feature,
    earlier: &[String],
    k: usize,
    pad: &mut Pad,
    vars: &dyn Fn(&str) -> Option<f64>,
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
    let placed_by_others = matches!(
        f.shape,
        Shape::Copies { .. } | Shape::Ring { .. } | Shape::Mirror { .. } | Shape::Shell { .. }
    );
    // A sweep or loft has two sketches, shown one at a time.
    let two = matches!(f.shape, Shape::Sweep { .. } | Shape::Loft { .. });
    let slot = if two { pad.slot.min(1) } else { 0 };
    if pad.target != Some((k, slot)) {
        *pad = Pad {
            slot,
            snap: pad.snap,
            grid: pad.grid,
            ortho: pad.ortho,
            polar: pad.polar,
            polar_step: pad.polar_step,
            radius: pad.radius,
            chamfer: pad.chamfer,
            distance: pad.distance,
            sides: pad.sides,
            narrow: pad.narrow,
            ..Pad::default()
        };
        pad.target = Some((k, slot));
    }
    // What the other sketch looks like, drawn faintly beneath a loft's.
    let reference = match &f.shape {
        Shape::Loft { sketch, to, .. } => {
            if slot == 0 {
                sk::profile(to, vars).unwrap_or_default()
            } else {
                circuiter_3d::cad::profile(f, sketch, vars).unwrap_or_default()
            }
        }
        _ => Vec::new(),
    };
    let fillet = &mut f.fillet;
    let chamfer = &mut f.chamfer;
    let ends = &mut f.edges;
    let drawing = &mut f.drawing;
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
            done |= edges_editor(ui, fillet, chamfer, ends);
        }
        Shape::Cylinder { radius, height } => {
            ui.horizontal(|ui| {
                done |= field(ui, "radius", radius, 120.0);
                done |= field(ui, "height", height, 120.0);
            });
            done |= edges_editor(ui, fillet, chamfer, ends);
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
            done |= edges_editor(ui, fillet, chamfer, ends);
            done |= profile_editor(ui, pad, drawing, sketch, Kind::Profile, &reference, vars);
        }
        Shape::Revolve { sketch, angle } => {
            ui.horizontal(|ui| done |= field(ui, "angle (°)", angle, 120.0));
            ui.label(RichText::new("The outline turns about the vertical axis on the left: x is the distance from it.").small());
            done |= profile_editor(ui, pad, drawing, sketch, Kind::Revolve, &reference, vars);
        }
        Shape::Sweep { sketch, path } => {
            slot_picker(ui, pad, ["Profile", "Path"]);
            if slot == 0 {
                ui.label(
                    RichText::new(
                        "The profile stands square to the path at its start, its origin on it.",
                    )
                    .small(),
                );
                done |= profile_editor(ui, pad, drawing, sketch, Kind::Profile, &reference, vars);
            } else {
                done |= sketch_editor(ui, pad, path, Kind::Path, &reference, vars).1;
            }
        }
        Shape::Loft { sketch, to, height } => {
            ui.horizontal(|ui| done |= field(ui, "height", height, 120.0));
            slot_picker(ui, pad, ["Bottom", "Top"]);
            if slot == 0 {
                done |= profile_editor(ui, pad, drawing, sketch, Kind::Profile, &reference, vars);
            } else {
                done |= sketch_editor(ui, pad, to, Kind::Profile, &reference, vars).1;
            }
        }
        Shape::Shell { of, thickness, open } => {
            done |= of_picker(ui, of);
            ui.horizontal(|ui| {
                done |= field(ui, "wall thickness", thickness, 70.0);
                ui.label(RichText::new("open").small());
                for (o, label) in [
                    (None, "closed"),
                    (Some(Ends::Top), "top"),
                    (Some(Ends::Bottom), "bottom"),
                    (Some(Ends::Both), "both"),
                ] {
                    if ui.selectable_label(*open == o, label).clicked() {
                        *open = o;
                        done = true;
                    }
                }
            });
            ui.label(
                RichText::new("The inside of a box, cylinder, cone, sphere, extrusion or revolve; cut, it hollows it out.")
                    .small(),
            );
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

/// Which of a feature's two sketches to draw in.
fn slot_picker(ui: &mut egui::Ui, pad: &mut Pad, names: [&str; 2]) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Sketch").strong());
        for (k, n) in names.iter().enumerate() {
            if ui.selectable_label(pad.slot == k as u8, *n).clicked() {
                pad.slot = k as u8;
            }
        }
    });
}

/// The rounding or bevel of a shape's top and bottom edges; true when an edit is
/// finished.
fn edges_editor(
    ui: &mut egui::Ui,
    fillet: &mut String,
    chamfer: &mut String,
    ends: &mut Ends,
) -> bool {
    let mut done = false;
    ui.horizontal_wrapped(|ui| {
        done |= field(ui, "edge fillet", fillet, 50.0);
        done |= field(ui, "or chamfer", chamfer, 50.0);
        for (e, label) in [(Ends::Both, "both ends"), (Ends::Top, "top"), (Ends::Bottom, "bottom")] {
            if ui.selectable_label(*ends == e, label).clicked() {
                *ends = e;
                done = true;
            }
        }
    })
    .response
    .on_hover_text("Rounds or bevels the edges round the top and bottom faces; the radius must be small beside the outline's features");
    done
}

/// The profile to draw: the feature's drawing, or its sketch of corners taken into one
/// when first changed. True when an edit is finished.
fn profile_editor(
    ui: &mut egui::Ui,
    pad: &mut Pad,
    drawing: &mut Option<Drawing>,
    sketch: &mut Sketch,
    kind: Kind,
    reference: &[Vec<(f32, f32)>],
    vars: &dyn Fn(&str) -> Option<f64>,
) -> bool {
    let mut d = drawing.clone().unwrap_or_else(|| sk::from_sketch(sketch, vars));
    let (changed, done) = sketch_editor(ui, pad, &mut d, kind, reference, vars);
    if changed || done {
        *drawing = Some(d);
        sketch.points.clear();
    }
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

/// The sketch editor's state: the tool, what it has picked so far, the view, and the
/// drawing aids' settings.
#[derive(Debug)]
pub struct Pad {
    /// Which sketch is open: the feature, and 0 for its profile or 1 for a loft's far
    /// end or a sweep's path.
    target: Option<(usize, u8)>,
    /// Which of a feature's sketches to show.
    slot: u8,
    tool: Tool,
    /// Points picked for the tool so far.
    picks: Vec<Pick>,
    /// Where a run of lines started, to close on.
    first: Option<usize>,
    /// For tools that pick in two steps (offset, mirror), whether the first is done.
    stage: bool,
    selection: Vec<Sel>,
    command: String,
    message: String,
    /// The model point at the middle of the view and pixels per millimetre.
    view: Option<(P, f32)>,
    /// Object snaps (F3), grid snap (F9), ortho (F8), and polar tracking's step in
    /// degrees, 0 for off (F10).
    snap: bool,
    grid: bool,
    ortho: bool,
    polar: f64,
    polar_step: f64,
    radius: f64,
    chamfer: f64,
    distance: f64,
    sides: usize,
    /// The sketch kept small beside the feature list, rather than given most of the
    /// view.
    pub narrow: bool,
    /// A dimension waiting for its value.
    pending: Option<Constraint>,
    dragging: Option<usize>,
    /// Undo (false) or redo (true) asked for while typing.
    pub history: Option<bool>,
    /// Where the pointer was in the sketch.
    cursor: P,
}

impl Default for Pad {
    fn default() -> Pad {
        Pad {
            target: None,
            slot: 0,
            tool: Tool::Select,
            picks: Vec::new(),
            first: None,
            stage: false,
            selection: Vec::new(),
            command: String::new(),
            message: String::new(),
            view: None,
            snap: true,
            grid: true,
            ortho: false,
            polar: 0.0,
            polar_step: 15.0,
            radius: 2.0,
            chamfer: 1.0,
            distance: 2.0,
            sides: 6,
            narrow: false,
            pending: None,
            dragging: None,
            history: None,
            cursor: [0.0, 0.0],
        }
    }
}

/// A point picked for a tool: where, the drawing's point there, and whether it lies on
/// a curve (so the curve is split there to join it).
#[derive(Debug, Clone, Copy)]
struct Pick {
    at: P,
    point: Option<usize>,
    on_curve: bool,
}

/// Something selected in a sketch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sel {
    Point(usize),
    Curve(usize),
    Constraint(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Select,
    Line,
    Arc3,
    ArcCenter,
    Circle,
    Rect,
    Polygon,
    Guide,
    Trim,
    Extend,
    Offset,
    Mirror,
    Fillet,
    Chamfer,
    Dimension,
}

/// Each tool: its button, the commands that start it, and what it does.
const TOOLS: &[(Tool, &str, &[&str], &str)] = &[
    (
        Tool::Select,
        icon::CURSOR,
        &["select", "s"],
        "Select (Esc): click curves, points or constraints (Shift adds); drag points; constrain what's selected",
    ),
    (
        Tool::Line,
        icon::LINE_SEGMENT,
        &["l", "line"],
        "Line (L): click or type points; C closes, Enter or a right-click ends",
    ),
    (
        Tool::Arc3,
        icon::ARROW_ARC_RIGHT,
        &["a", "arc"],
        "Arc through three points (A): start, a point on it, end",
    ),
    (
        Tool::ArcCenter,
        icon::CIRCLE_HALF,
        &["ac", "arcc"],
        "Arc from its centre (AC): centre, start, then end, counter-clockwise",
    ),
    (
        Tool::Circle,
        icon::CIRCLE,
        &["c", "circle"],
        "Circle (C): centre, then a point on it or a radius",
    ),
    (
        Tool::Rect,
        icon::SQUARE,
        &["rec", "rect", "rectangle"],
        "Rectangle (REC): two corners (type @20,10 for the second)",
    ),
    (Tool::Polygon, icon::HEXAGON, &["pol", "polygon"], "Polygon (POL): centre, then a corner"),
    (
        Tool::Guide,
        icon::LINE_VERTICAL,
        &["xl", "guide"],
        "Guide line (XL): construction, for mirrors and constraints; not part of the outline",
    ),
    (
        Tool::Trim,
        icon::SCISSORS,
        &["tr", "trim"],
        "Trim (TR): click the piece to take away, back to where curves cross it",
    ),
    (
        Tool::Extend,
        icon::ARROW_LINE_RIGHT,
        &["ex", "extend"],
        "Extend (EX): click near a line's end to lengthen it to the next curve",
    ),
    (
        Tool::Offset,
        icon::ARROWS_OUT_LINE_HORIZONTAL,
        &["o", "offset"],
        "Offset (O): type a distance, click a curve, then click the side to copy it to",
    ),
    (
        Tool::Mirror,
        icon::FLIP_HORIZONTAL,
        &["mi", "mirror"],
        "Mirror (MI): click the curves (or select them first), Enter, then click the line to mirror across",
    ),
    (
        Tool::Fillet,
        icon::BEZIER_CURVE,
        &["f", "fillet"],
        "Fillet (F): type a radius, then click a corner where two lines meet",
    ),
    (
        Tool::Chamfer,
        icon::FRAME_CORNERS,
        &["cha", "chamfer"],
        "Chamfer (CHA): type a distance, then click a corner where two lines meet",
    ),
    (
        Tool::Dimension,
        icon::RULER,
        &["dim", "d"],
        "Dimension (DIM): click a line (length), an arc or circle (radius), or two points (distance); then type its value or an expression",
    ),
];

impl Pad {
    /// Starts `tool`, dropping what the last one had picked.
    fn start(&mut self, tool: Tool) {
        self.tool = tool;
        self.picks.clear();
        self.first = None;
        self.pending = None;
        self.stage =
            tool == Tool::Mirror && self.selection.iter().any(|s| matches!(s, Sel::Curve(_)));
        if tool != Tool::Mirror && tool != Tool::Select {
            self.selection.clear();
        }
        self.message.clear();
    }

    /// Ends the tool's current run (Enter, a right-click).
    fn finish(&mut self) {
        if self.tool == Tool::Mirror && !self.stage {
            self.stage = true;
        } else if self.picks.is_empty() && self.pending.is_none() {
            self.start(Tool::Select);
        } else {
            self.picks.clear();
            self.first = None;
            self.pending = None;
            self.stage = false;
        }
    }

    /// What the command line asks for.
    fn prompt(&self, d: &Drawing, g: &sk::Geometry) -> String {
        if let Some(c) = &self.pending {
            return format!(
                "{} <{}>: type a value or expression, Enter to keep",
                dimension_name(c),
                fmt_num(measure(d, g, c))
            );
        }
        let n = self.picks.len();
        match self.tool {
            Tool::Select => {
                "Select, or type a command (L, A, C, REC, POL, TR, EX, O, MI, F, CHA, DIM)".into()
            }
            Tool::Line | Tool::Guide if n == 0 => "first point:".into(),
            Tool::Line => {
                "next point (@dx,dy  @len<angle  or a length; C closes, Enter ends):".into()
            }
            Tool::Guide => "second point:".into(),
            Tool::Arc3 => ["start point:", "a point on the arc:", "end point:"][n.min(2)].into(),
            Tool::ArcCenter => {
                ["centre:", "start point (sets the radius):", "end point (counter-clockwise):"]
                    [n.min(2)]
                .into()
            }
            Tool::Circle if n == 0 => "centre:".into(),
            Tool::Circle => "a point on it, or a radius:".into(),
            Tool::Rect if n == 0 => "first corner:".into(),
            Tool::Rect => "other corner (@width,height):".into(),
            Tool::Polygon if n == 0 => format!("centre ({} sides):", self.sides),
            Tool::Polygon => "a corner, or the radius:".into(),
            Tool::Trim => "click the piece to trim away:".into(),
            Tool::Extend => "click near the end of a line to extend:".into(),
            Tool::Offset if self.stage => "click the side to offset to:".into(),
            Tool::Offset => {
                format!("distance <{}>, or click the curve to offset:", fmt_num(self.distance))
            }
            Tool::Mirror if self.stage => "click the line to mirror across:".into(),
            Tool::Mirror => "click curves to mirror, then Enter:".into(),
            Tool::Fillet => format!("radius <{}>, or click a corner:", fmt_num(self.radius)),
            Tool::Chamfer => format!("distance <{}>, or click a corner:", fmt_num(self.chamfer)),
            Tool::Dimension if n == 1 => "the second point:".into(),
            Tool::Dimension => "click a line, an arc or circle, or a point:".into(),
        }
    }
}

fn fmt_num(v: f64) -> String {
    circuiter_model::model3d::fmt(v)
}

fn dimension_name(c: &Constraint) -> &'static str {
    match c {
        Constraint::Length(..) => "Length",
        Constraint::Distance(..) => "Distance",
        Constraint::DistanceX(..) => "Across",
        Constraint::DistanceY(..) => "Up",
        Constraint::Radius(..) => "Radius",
        Constraint::Angle(..) => "Angle",
        _ => "Value",
    }
}

/// What a dimension measures now.
fn measure(d: &Drawing, g: &sk::Geometry, c: &Constraint) -> f64 {
    let p = |i: usize| g.points[i];
    let ends = |k: usize| match d.curves[k] {
        Curve::Line(a, b) | Curve::Guide(a, b) => Some((p(a), p(b))),
        _ => None,
    };
    match *c {
        Constraint::Length(k, _) => ends(k).map_or(0.0, |(a, b)| (b[0] - a[0]).hypot(b[1] - a[1])),
        Constraint::Distance(a, b, _) => (p(b)[0] - p(a)[0]).hypot(p(b)[1] - p(a)[1]),
        Constraint::DistanceX(a, b, _) => (p(b)[0] - p(a)[0]).abs(),
        Constraint::DistanceY(a, b, _) => (p(b)[1] - p(a)[1]).abs(),
        Constraint::Radius(k, _) => match sk::prim(d, g, k) {
            Prim::Arc { r, .. } => r,
            Prim::Seg(..) => 0.0,
        },
        Constraint::Angle(a, b, _) => match (ends(a), ends(b)) {
            (Some((s1, e1)), Some((s2, e2))) => {
                let a1 = (e1[1] - s1[1]).atan2(e1[0] - s1[0]);
                let a2 = (e2[1] - s2[1]).atan2(e2[0] - s2[0]);
                let mut x = (a2 - a1).to_degrees();
                while x > 180.0 {
                    x -= 360.0;
                }
                while x <= -180.0 {
                    x += 360.0;
                }
                x
            }
            _ => 0.0,
        },
        _ => 0.0,
    }
}

/// The dimension's text, and a copy with `text` in its place.
fn dimension_value(c: &mut Constraint) -> Option<&mut String> {
    match c {
        Constraint::Length(_, v)
        | Constraint::Distance(_, _, v)
        | Constraint::DistanceX(_, _, v)
        | Constraint::DistanceY(_, _, v)
        | Constraint::Radius(_, v)
        | Constraint::Angle(_, _, v) => Some(v),
        _ => None,
    }
}

/// What a sketch is for, which changes what the editor shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// An outline to extrude or loft.
    Profile,
    /// An outline to turn about the vertical axis at x = 0.
    Revolve,
    /// A sweep's path, in the XZ plane.
    Path,
}

/// A point of the drawing for a pick: the one there, or a new one (splitting the curves
/// it lies on, so they join).
fn resolve(d: &mut Drawing, g: &mut sk::Geometry, p: Pick) -> usize {
    if let Some(k) = p.point.filter(|k| *k < d.points.len()) {
        return k;
    }
    if p.on_curve {
        return sk::join_at(d, g, p.at, None);
    }
    sk::point_at(g, p.at).unwrap_or_else(|| {
        g.points.push(p.at);
        sk::add_point(d, p.at)
    })
}

/// The ready-made drawings to start from.
fn presets(kind: Kind) -> Vec<(&'static str, Drawing)> {
    let none = |_: &str| None;
    let mut rect = Drawing::default();
    sk::rectangle(&mut rect, [-10.0, -5.0], [10.0, 5.0]);
    let mut circle = Drawing::default();
    let c = sk::add_point(&mut circle, [0.0, 0.0]);
    circle.curves.push(Curve::Circle { center: c, radius: "5".into() });
    let mut hexagon = Drawing::default();
    sk::polygon(&mut hexagon, [0.0, 0.0], [5.0, 0.0], 6);
    match kind {
        Kind::Profile => vec![
            ("Rectangle", rect),
            ("Circle", circle),
            ("Slot", sk::from_sketch(&Sketch::slot(12.0, 4.0), &none)),
            ("Hexagon", hexagon),
            ("Clear", Drawing::default()),
        ],
        Kind::Revolve => vec![("Profile", revolve_profile()), ("Clear", Drawing::default())],
        Kind::Path => vec![
            ("Straight", straight_path()),
            ("Bend", bend_path()),
            ("Clear", Drawing::default()),
        ],
    }
}

fn revolve_profile() -> Drawing {
    let s = Sketch {
        points: vec![
            SketchPoint::new(0.0, 0.0),
            SketchPoint::new(6.0, 0.0),
            SketchPoint::new(6.0, 2.0),
            SketchPoint::new(3.0, 8.0),
            SketchPoint::new(0.0, 8.0),
        ],
    };
    sk::from_sketch(&s, &|_| None)
}

fn straight_path() -> Drawing {
    let mut d = Drawing::default();
    let (a, b) = (sk::add_point(&mut d, [0.0, 0.0]), sk::add_point(&mut d, [0.0, 20.0]));
    d.curves.push(Curve::Line(a, b));
    d
}

/// Up, round a bend of radius 10, and across.
fn bend_path() -> Drawing {
    let mut d = Drawing::default();
    let p = [[0.0, 0.0], [0.0, 15.0], [10.0, 25.0], [25.0, 25.0], [10.0, 15.0]]
        .map(|q| sk::add_point(&mut d, q));
    d.curves.push(Curve::Line(p[0], p[1]));
    d.curves.push(Curve::Arc { center: p[4], from: p[2], to: p[1] });
    d.curves.push(Curve::Line(p[2], p[3]));
    d.constraints.extend([
        Constraint::Vertical(0),
        Constraint::Tangent(0, 1),
        Constraint::Tangent(2, 1),
        Constraint::Horizontal(2),
    ]);
    d
}

/// Solves `d` (holding `pinned`) and writes the result back; a message when the
/// constraints can't all be met.
fn settle(d: &mut Drawing, vars: &dyn Fn(&str) -> Option<f64>, pinned: &[usize]) -> Option<String> {
    match sk::solve(d, vars, pinned) {
        Ok((g, miss)) => {
            sk::store(d, &g);
            (miss > 1e-4).then(|| {
                "the constraints can't all be met: remove one or change a dimension".into()
            })
        }
        Err(e) => Some(e),
    }
}

/// A sketch to draw, as in a 2D CAD program: tools for lines, arcs, circles, rectangles
/// and polygons, trim, extend, offset, mirror, fillet and chamfer; object snaps, grid,
/// ortho and polar tracking; a command line for commands and typed points; constraints
/// and driving dimensions. `reference` is drawn faintly underneath. Returns whether the
/// drawing changed and whether an edit is finished.
fn sketch_editor(
    ui: &mut egui::Ui,
    pad: &mut Pad,
    d: &mut Drawing,
    kind: Kind,
    reference: &[Vec<(f32, f32)>],
    vars: &dyn Fn(&str) -> Option<f64>,
) -> (bool, bool) {
    let p = style::palette(ui);
    let mut changed = false;
    let mut done = false;
    // Toolbar: the tools, then the drawing aids.
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (tool, label, _, hint) in TOOLS {
            if ui
                .selectable_label(pad.tool == *tool, RichText::new(*label).size(15.0))
                .on_hover_text(*hint)
                .clicked()
            {
                pad.start(*tool);
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.toggle_value(&mut pad.snap, format!("{} Snap", icon::MAGNET)).on_hover_text("Object snaps (F3): ends, middles, centres, crossings, nearest");
        ui.toggle_value(&mut pad.grid, format!("{} Grid", icon::GRID_FOUR)).on_hover_text("Snap to the grid (F9)");
        if ui.toggle_value(&mut pad.ortho, "Ortho").on_hover_text("Ortho (F8): lines run horizontally or vertically").changed() && pad.ortho {
            pad.polar = 0.0;
        }
        let mut polar = pad.polar > 0.0;
        if ui.toggle_value(&mut polar, format!("Polar {}°", fmt_num(pad.polar_step))).on_hover_text("Polar tracking (F10): lines hold to multiples of the angle; type polar 30 for another").changed() {
            pad.polar = if polar { pad.polar_step } else { 0.0 };
            pad.ortho &= !polar;
        }
        ui.menu_button("Start from", |ui| {
            for (name, preset) in presets(kind) {
                if ui.button(name).clicked() {
                    *d = preset;
                    pad.view = None;
                    pad.selection.clear();
                    changed = true;
                    done = true;
                    ui.close();
                }
            }
        });
        if ui.small_button(icon::CORNERS_OUT).on_hover_text("Fit the view (Z)").clicked() {
            pad.view = None;
        }
        let label = if pad.narrow { format!("{}  Wide", icon::ARROWS_OUT) } else { format!("{}  Narrow", icon::ARROWS_IN) };
        if ui.button(label).on_hover_text("Give the sketch most of the view, or keep it beside the 3D model").clicked() {
            pad.narrow = !pad.narrow;
        }
    });
    // The tool's settings.
    ui.horizontal(|ui| {
        let drag = |ui: &mut egui::Ui, label: &str, v: &mut f64| {
            ui.label(RichText::new(label).small());
            ui.add(egui::DragValue::new(v).range(0.01..=10000.0).speed(0.1).suffix(" mm"));
        };
        match pad.tool {
            Tool::Offset => drag(ui, "distance", &mut pad.distance),
            Tool::Fillet => drag(ui, "radius", &mut pad.radius),
            Tool::Chamfer => drag(ui, "distance", &mut pad.chamfer),
            Tool::Polygon => {
                ui.label(RichText::new("sides").small());
                ui.add(egui::DragValue::new(&mut pad.sides).range(3..=64));
            }
            _ => {}
        }
    });
    let (g, miss) = match sk::solve(d, vars, pad.dragging.as_slice()) {
        Ok(x) => x,
        Err(e) => {
            ui.label(RichText::new(format!("Can't draw the sketch: {e}")).color(p.error));
            done |= points_table(ui, d);
            return (done, done);
        }
    };
    let loops = sk::loops(d, &g);
    // The view: kept between frames, fitted when first shown.
    // Wide, the drawing reaches the bottom of the panel, leaving room for the command line.
    let tall = (ui.clip_rect().bottom() - ui.cursor().top() - 110.0).max(360.0);
    let size = egui::vec2(ui.available_width().max(200.0), if pad.narrow { 300.0 } else { tall });
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let (mid, scale) = *pad.view.get_or_insert_with(|| {
        let mut lo = [f64::MAX; 2];
        let mut hi = [f64::MIN; 2];
        let pts = g.points.iter().copied().chain(loops.iter().flatten().copied());
        let pts = pts.chain(reference.iter().flatten().map(|q| [q.0 as f64, q.1 as f64]));
        for q in pts.chain(matches!(kind, Kind::Revolve | Kind::Path).then_some([0.0, 0.0])) {
            lo = [lo[0].min(q[0]), lo[1].min(q[1])];
            hi = [hi[0].max(q[0]), hi[1].max(q[1])];
        }
        if lo[0] > hi[0] {
            lo = [-10.0, -10.0];
            hi = [10.0, 10.0];
        }
        let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(2.0) * 1.3;
        (
            [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0],
            (rect.width().min(rect.height()) as f64 / span) as f32,
        )
    });
    let s = scale as f64;
    let to_screen = |q: P| {
        rect.center() + egui::vec2(((q[0] - mid[0]) * s) as f32, (-(q[1] - mid[1]) * s) as f32)
    };
    let to_model = |pos: egui::Pos2| -> P {
        [
            mid[0] + (pos.x - rect.center().x) as f64 / s,
            mid[1] - (pos.y - rect.center().y) as f64 / s,
        ]
    };
    // Zoom about the pointer, pan with a middle or right drag (or a left drag on
    // nothing while selecting).
    if response.hovered() {
        let wheel = ui.input_mut(|i| std::mem::take(&mut i.smooth_scroll_delta).y);
        let pinch = ui.input(|i| i.zoom_delta());
        let factor = (wheel as f64 * 0.002).exp() * pinch as f64;
        if (factor - 1.0).abs() > 1e-6 {
            if let Some(at) = response.hover_pos() {
                let q = to_model(at);
                let ns = (s * factor).clamp(0.01, 5000.0);
                let c = [
                    q[0] - (at.x - rect.center().x) as f64 / ns,
                    q[1] + (at.y - rect.center().y) as f64 / ns,
                ];
                pad.view = Some((c, ns as f32));
            }
        }
    }
    // A left drag from a point moves it.
    if response.drag_started_by(egui::PointerButton::Primary) && pad.tool == Tool::Select {
        pad.dragging = response
            .interact_pointer_pos()
            .and_then(|pos| sk::nearest_point(&g, to_model(pos), 8.0 / s));
    }
    let panning = response.dragged_by(egui::PointerButton::Middle)
        || response.dragged_by(egui::PointerButton::Secondary)
        || (response.dragged_by(egui::PointerButton::Primary)
            && pad.tool == Tool::Select
            && pad.dragging.is_none());
    if panning {
        let dv = response.drag_delta();
        if let Some((c, sc)) = &mut pad.view {
            c[0] -= dv.x as f64 / *sc as f64;
            c[1] += dv.y as f64 / *sc as f64;
        }
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, p.well);
    // A grid at a step that keeps lines a sensible distance apart.
    let step = [0.1f64, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0]
        .into_iter()
        .find(|st| st * s >= 14.0)
        .unwrap_or(1000.0);
    let grid = Stroke::new(1.0, p.weak.gamma_multiply(0.13));
    let (m0, m1) = (to_model(rect.left_bottom()), to_model(rect.right_top()));
    let mut x = (m0[0] / step).floor() * step;
    while x <= m1[0] {
        let sx = to_screen([x, 0.0]).x;
        painter.line_segment([egui::pos2(sx, rect.top()), egui::pos2(sx, rect.bottom())], grid);
        x += step;
    }
    let mut y = (m0[1] / step).floor() * step;
    while y <= m1[1] {
        let sy = to_screen([0.0, y]).y;
        painter.line_segment([egui::pos2(rect.left(), sy), egui::pos2(rect.right(), sy)], grid);
        y += step;
    }
    let o = to_screen([0.0, 0.0]);
    let axis =
        |strong: bool| Stroke::new(1.0, p.accent.gamma_multiply(if strong { 0.9 } else { 0.35 }));
    painter.line_segment(
        [egui::pos2(o.x, rect.top()), egui::pos2(o.x, rect.bottom())],
        axis(kind == Kind::Revolve),
    );
    painter
        .line_segment([egui::pos2(rect.left(), o.y), egui::pos2(rect.right(), o.y)], axis(false));
    // What will be solid, filled.
    let fill = p.accent.gamma_multiply(0.16);
    let shade = |rings: &[Vec<(f32, f32)>], color: Color32| {
        if rings.is_empty() || rings[0].len() < 3 {
            return;
        }
        let (pts, tris) = circuiter_3d::triangulate_holes(&rings[0], &rings[1..]);
        let mut mesh = egui::Mesh::default();
        for q in &pts {
            mesh.colored_vertex(to_screen([q.0 as f64, q.1 as f64]), color);
        }
        for t in tris {
            mesh.add_triangle(t[0], t[1], t[2]);
        }
        painter.add(egui::Shape::mesh(mesh));
    };
    shade(reference, p.weak.gamma_multiply(0.12));
    for r in reference {
        let mut line: Vec<egui::Pos2> =
            r.iter().map(|q| to_screen([q.0 as f64, q.1 as f64])).collect();
        line.extend(line.first().copied());
        painter.add(egui::Shape::dashed_line(&line, Stroke::new(1.0, p.weak), 4.0, 3.0));
    }
    if kind != Kind::Path {
        let rings: Vec<Vec<(f32, f32)>> =
            loops.iter().map(|l| l.iter().map(|q| (q[0] as f32, q[1] as f32)).collect()).collect();
        shade(&rings, fill);
    }
    // The pointer, snapped.
    let pointer = response.hover_pos().or(response.interact_pointer_pos());
    let reach = 8.0 / s;
    let from = pad.picks.last().map(|k| k.at);
    let snapped = pointer.map(|pos| snap(pad, d, &g, to_model(pos), reach, from, step));
    if let Some((q, ..)) = snapped {
        pad.cursor = q;
    }
    // Hovered things, for selecting and for the tools that click curves and points.
    let hover_point = pointer.and_then(|pos| sk::nearest_point(&g, to_model(pos), reach));
    let hover_curve =
        pointer.and_then(|pos| sk::nearest_curve(d, &g, to_model(pos), reach)).map(|c| c.0);
    // Curves.
    let selected = |sel: Sel| pad.selection.contains(&sel);
    let mut label_rects: Vec<(egui::Rect, usize)> = Vec::new();
    for k in 0..d.curves.len() {
        let pr = sk::prim(d, &g, k);
        let pts: Vec<egui::Pos2> = pr.points().into_iter().map(to_screen).collect();
        let hot = hover_curve == Some(k) && hover_point.is_none() && pad.tool != Tool::Line;
        let color = if selected(Sel::Curve(k)) {
            p.warning
        } else if hot {
            p.go
        } else {
            p.accent
        };
        let width = if hot || selected(Sel::Curve(k)) { 2.6 } else { 1.8 };
        if matches!(d.curves[k], Curve::Guide(..)) {
            painter.add(egui::Shape::dashed_line(
                &pts,
                Stroke::new(1.2, color.gamma_multiply(0.8)),
                6.0,
                4.0,
            ));
        } else {
            painter.add(egui::Shape::line(pts, Stroke::new(width, color)));
        }
    }
    // Points: curve ends, and centres as crosses.
    for (k, q) in g.points.iter().enumerate() {
        let at = to_screen(*q);
        let center = d.curves.iter().any(|c| matches!(*c, Curve::Arc { center, .. } | Curve::Circle { center, .. } if center == k));
        let hot = hover_point == Some(k) || pad.dragging == Some(k);
        let color = if selected(Sel::Point(k)) {
            p.warning
        } else if hot {
            p.go
        } else {
            p.accent
        };
        if center {
            let r = 3.5;
            painter.line_segment(
                [at - egui::vec2(r, 0.0), at + egui::vec2(r, 0.0)],
                Stroke::new(1.2, color),
            );
            painter.line_segment(
                [at - egui::vec2(0.0, r), at + egui::vec2(0.0, r)],
                Stroke::new(1.2, color),
            );
        } else {
            painter.circle(
                at,
                if hot { 4.5 } else { 2.8 },
                if hot { color } else { p.panel },
                Stroke::new(1.3, color),
            );
        }
    }
    // Constraints and dimensions, labelled where they apply.
    let font = egui::FontId::proportional(10.5);
    for (k, c) in d.constraints.iter().enumerate() {
        let curve_mid = |j: usize| {
            let pr = sk::prim(d, &g, j);
            pr.at(pr.end() / 2.0)
        };
        let normal_off = |j: usize, px: f32| -> egui::Vec2 {
            match sk::prim(d, &g, j) {
                Prim::Seg(a, b) => {
                    let v = egui::vec2((b[1] - a[1]) as f32, (b[0] - a[0]) as f32).normalized();
                    v * px
                }
                Prim::Arc { .. } => egui::vec2(px, -px),
            }
        };
        let (at, text) = match c {
            Constraint::Coincident(a, _) => {
                (to_screen(g.points[*a]) + egui::vec2(6.0, -8.0), "●".to_owned())
            }
            Constraint::OnCurve(a, _) => {
                (to_screen(g.points[*a]) + egui::vec2(6.0, -8.0), "◦".to_owned())
            }
            Constraint::Fixed(a) => {
                (to_screen(g.points[*a]) + egui::vec2(8.0, 8.0), icon::LOCK.to_owned())
            }
            Constraint::Horizontal(a) => {
                (to_screen(curve_mid(*a)) + normal_off(*a, -9.0), "H".to_owned())
            }
            Constraint::Vertical(a) => {
                (to_screen(curve_mid(*a)) + normal_off(*a, -9.0), "V".to_owned())
            }
            Constraint::Parallel(a, _) => (
                to_screen(curve_mid(*a)) + normal_off(*a, -9.0) + egui::vec2(9.0, 0.0),
                "∥".to_owned(),
            ),
            Constraint::Perpendicular(a, _) => (
                to_screen(curve_mid(*a)) + normal_off(*a, -9.0) + egui::vec2(9.0, 0.0),
                "⊥".to_owned(),
            ),
            Constraint::Tangent(_, b) => {
                (to_screen(curve_mid(*b)) + egui::vec2(-8.0, -8.0), "T".to_owned())
            }
            Constraint::Equal(a, _) => (
                to_screen(curve_mid(*a)) + normal_off(*a, -9.0) - egui::vec2(9.0, 0.0),
                "=".to_owned(),
            ),
            Constraint::Length(a, v) => {
                (to_screen(curve_mid(*a)) + normal_off(*a, 12.0), label(v, measure(d, &g, c)))
            }
            Constraint::Distance(a, b, v)
            | Constraint::DistanceX(a, b, v)
            | Constraint::DistanceY(a, b, v) => {
                let (pa, pb) = (g.points[*a], g.points[*b]);
                let corner = match c {
                    Constraint::DistanceX(..) => [pb[0], pa[1]],
                    Constraint::DistanceY(..) => [pa[0], pb[1]],
                    _ => pb,
                };
                painter.add(egui::Shape::dashed_line(
                    &[to_screen(pa), to_screen(corner)],
                    Stroke::new(1.0, p.weak),
                    3.0,
                    3.0,
                ));
                let m = [(pa[0] + corner[0]) / 2.0, (pa[1] + corner[1]) / 2.0];
                let arrow = match c {
                    Constraint::DistanceX(..) => "↔ ",
                    Constraint::DistanceY(..) => "↕ ",
                    _ => "",
                };
                (
                    to_screen(m) + egui::vec2(0.0, -8.0),
                    format!("{arrow}{}", label(v, measure(d, &g, c))),
                )
            }
            Constraint::Radius(a, v) => (
                to_screen(curve_mid(*a)) + egui::vec2(10.0, -10.0),
                format!("R{}", label(v, measure(d, &g, c))),
            ),
            Constraint::Angle(a, _, v) => (
                to_screen(curve_mid(*a)) + normal_off(*a, 14.0),
                format!("{}°", label(v, measure(d, &g, c))),
            ),
        };
        let color = if selected(Sel::Constraint(k)) {
            p.warning
        } else if dimension_name(c) != "Value" {
            p.text
        } else {
            p.weak
        };
        let r = painter.text(at, egui::Align2::CENTER_CENTER, text, font.clone(), color);
        label_rects.push((r.expand(2.0), k));
    }
    let hover_label =
        pointer.and_then(|pos| label_rects.iter().find(|(r, _)| r.contains(pos)).map(|x| x.1));
    // What the tool would make, following the pointer.
    let ghost = Stroke::new(1.3, p.go);
    if let Some((q, ..)) = snapped {
        let at = |i: usize| pad.picks[i].at;
        let line = |a: P, b: P| {
            painter.line_segment([to_screen(a), to_screen(b)], ghost);
        };
        let circle = |c: P, r: f64| {
            painter.circle_stroke(to_screen(c), (r * s) as f32, ghost);
        };
        let arc = |c: P, r: f64, a0: f64, sweep: f64| {
            let pr = Prim::Arc { c, r, a0, sweep };
            painter.add(egui::Shape::line(pr.points().into_iter().map(to_screen).collect(), ghost));
        };
        let ang = |c: P, q: P| (q[1] - c[1]).atan2(q[0] - c[0]);
        match (pad.tool, pad.picks.len()) {
            (Tool::Line | Tool::Guide, 1..) => line(at(pad.picks.len() - 1), q),
            (Tool::Rect, 1) => {
                let a = at(0);
                for (u, v) in
                    [(a, [q[0], a[1]]), ([q[0], a[1]], q), (q, [a[0], q[1]]), ([a[0], q[1]], a)]
                {
                    line(u, v);
                }
            }
            (Tool::Circle, 1) => circle(at(0), (q[0] - at(0)[0]).hypot(q[1] - at(0)[1])),
            (Tool::Polygon, 1) => {
                let mut tmp = Drawing::default();
                sk::polygon(&mut tmp, at(0), q, pad.sides);
                let tg = sk::geometry(&tmp, vars).unwrap_or_default();
                for k in 0..tmp.curves.len() {
                    if let Prim::Seg(a, b) = sk::prim(&tmp, &tg, k) {
                        line(a, b);
                    }
                }
            }
            (Tool::Arc3, 1) | (Tool::ArcCenter, 1) => line(at(0), q),
            (Tool::Arc3, 2) => {
                if let Some(c) = sk::circle_through(at(0), at(1), q) {
                    let r = (at(0)[0] - c[0]).hypot(at(0)[1] - c[1]);
                    let ccw = (at(1)[0] - at(0)[0]) * (q[1] - at(0)[1])
                        - (at(1)[1] - at(0)[1]) * (q[0] - at(0)[0])
                        > 0.0;
                    let (s0, e0) = if ccw { (at(0), q) } else { (q, at(0)) };
                    arc(
                        c,
                        r,
                        ang(c, s0),
                        (ang(c, e0) - ang(c, s0)).rem_euclid(std::f64::consts::TAU),
                    );
                }
            }
            (Tool::ArcCenter, 2) => {
                let c = at(0);
                let r = (at(1)[0] - c[0]).hypot(at(1)[1] - c[1]);
                arc(
                    c,
                    r,
                    ang(c, at(1)),
                    (ang(c, q) - ang(c, at(1))).rem_euclid(std::f64::consts::TAU),
                );
            }
            _ => {}
        }
    }
    // The snap marker and the tracking line.
    let drawing_tool = matches!(
        pad.tool,
        Tool::Line
            | Tool::Guide
            | Tool::Arc3
            | Tool::ArcCenter
            | Tool::Circle
            | Tool::Rect
            | Tool::Polygon
    ) || pad.dragging.is_some();
    if let (Some((q, kind_of, _, _, tracked)), true) = (snapped, drawing_tool) {
        let at = to_screen(q);
        let mark = Stroke::new(1.5, p.warning);
        let r = 5.0;
        match kind_of {
            Some(Snap::End) => {
                painter.rect_stroke(
                    egui::Rect::from_center_size(at, egui::vec2(2.0 * r, 2.0 * r)),
                    0.0,
                    mark,
                    egui::StrokeKind::Middle,
                );
            }
            Some(Snap::Mid) => {
                let pts = vec![
                    at + egui::vec2(0.0, -r),
                    at + egui::vec2(r, r * 0.8),
                    at + egui::vec2(-r, r * 0.8),
                ];
                painter.add(egui::Shape::closed_line(pts, mark));
            }
            Some(Snap::Center) => {
                painter.circle_stroke(at, r, mark);
            }
            Some(Snap::Intersection) => {
                painter.line_segment([at + egui::vec2(-r, -r), at + egui::vec2(r, r)], mark);
                painter.line_segment([at + egui::vec2(-r, r), at + egui::vec2(r, -r)], mark);
            }
            Some(Snap::Nearest) => {
                let pts = vec![
                    at + egui::vec2(-r, -r),
                    at + egui::vec2(r, -r),
                    at + egui::vec2(-r, r),
                    at + egui::vec2(r, r),
                ];
                painter.add(egui::Shape::closed_line(pts, mark));
            }
            Some(Snap::Grid) => {
                painter.line_segment(
                    [at + egui::vec2(-r, 0.0), at + egui::vec2(r, 0.0)],
                    Stroke::new(1.0, p.weak),
                );
                painter.line_segment(
                    [at + egui::vec2(0.0, -r), at + egui::vec2(0.0, r)],
                    Stroke::new(1.0, p.weak),
                );
            }
            None => {}
        }
        if let (true, Some(f)) = (tracked, from) {
            let (a, b) = (to_screen(f), at);
            let far = a + (b - a).normalized() * rect.width().max(rect.height()) * 2.0;
            painter.add(egui::Shape::dashed_line(
                &[a, far],
                Stroke::new(1.0, p.warning.gamma_multiply(0.6)),
                5.0,
                4.0,
            ));
        }
        // Dynamic input: where, or how far and at what angle from the last point.
        let text = match from {
            Some(f) => {
                let (dx, dy) = (q[0] - f[0], q[1] - f[1]);
                format!("{}  ∠{}°", fmt2(dx.hypot(dy)), fmt2(dy.atan2(dx).to_degrees()))
            }
            None => format!("{}, {}", fmt2(q[0]), fmt2(q[1])),
        };
        let label = kind_of.map_or(String::new(), |k| format!("  {}", k.label()));
        painter.text(
            at + egui::vec2(12.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!("{text}{label}"),
            egui::FontId::monospace(10.5),
            p.text,
        );
    }
    painter.text(
        rect.left_top() + egui::vec2(6.0, 4.0),
        egui::Align2::LEFT_TOP,
        format!("grid {} mm", fmt_num(step)),
        egui::FontId::proportional(10.0),
        p.weak,
    );
    if kind == Kind::Path {
        painter.text(
            rect.left_bottom() + egui::vec2(6.0, -4.0),
            egui::Align2::LEFT_BOTTOM,
            "the path, in the XZ plane: x across, y up",
            egui::FontId::proportional(10.0),
            p.weak,
        );
    }
    if let Some(first) = loops.first() {
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for q in first {
            lo = [lo[0].min(q[0]), lo[1].min(q[1])];
            hi = [hi[0].max(q[0]), hi[1].max(q[1])];
        }
        painter.text(
            rect.right_bottom() + egui::vec2(-6.0, -4.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("{} × {} mm", fmt2(hi[0] - lo[0]), fmt2(hi[1] - lo[1])),
            egui::FontId::monospace(10.0),
            p.weak,
        );
    }
    if miss > 1e-4 {
        painter.text(
            rect.center_top() + egui::vec2(0.0, 6.0),
            egui::Align2::CENTER_TOP,
            "the constraints can't all be met",
            egui::FontId::proportional(11.0),
            p.error,
        );
    }
    // Clicks and drags.
    let edit = |d: &mut Drawing| {
        sk::store(d, &g);
    };
    let shift = ui.input(|i| i.modifiers.shift);
    if let (Some(k), Some((q, ..))) = (pad.dragging, snapped) {
        if response.dragged() && k < d.points.len() {
            let [x, y] = &mut d.points[k];
            let plain = |t: &str| t.trim().parse::<f64>().is_ok();
            if plain(x) {
                *x = fmt_num(q[0]);
            }
            if plain(y) {
                *y = fmt_num(q[1]);
            }
            if let Some(m) = settle(d, vars, &[k]) {
                pad.message = m;
            }
            changed = true;
        }
    }
    if response.drag_stopped() && pad.dragging.take().is_some() {
        pad.message = settle(d, vars, &[]).unwrap_or_default();
        changed = true;
        done = true;
    }
    if response.secondary_clicked() {
        pad.finish();
    }
    if response.clicked() {
        if let Some((q, kind_of, point, on_curve, _)) = snapped {
            let pick = Pick {
                at: q,
                point: point
                    .filter(|_| kind_of.is_some_and(|k| matches!(k, Snap::End | Snap::Center))),
                on_curve,
            };
            match pad.tool {
                Tool::Select => {
                    let hit = hover_label
                        .map(Sel::Constraint)
                        .or(hover_point.map(Sel::Point))
                        .or(hover_curve.map(Sel::Curve));
                    if !shift {
                        pad.selection.clear();
                    }
                    if let Some(h) = hit {
                        if let Some(i) = pad.selection.iter().position(|x| *x == h) {
                            pad.selection.remove(i);
                        } else {
                            pad.selection.push(h);
                        }
                    }
                }
                _ => {
                    edit(d);
                    let at = to_model(pointer.expect("clicked"));
                    if click(pad, d, &g, pick, at, hover_point, hover_curve) {
                        pad.message = settle(d, vars, &[]).unwrap_or_default();
                        changed = true;
                        done = true;
                    }
                }
            }
        }
    }
    response.on_hover_text_at_pointer(pad.prompt(d, &g));
    // The command line: commands, typed points, values. While the pointer is over the
    // drawing, what's typed goes here, as in AutoCAD.
    let id = ui.make_persistent_id("sketch-command");
    let focused = ui.memory(|m| m.has_focus(id));
    let (enter, escape, del) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::Delete),
        )
    });
    let pointer_in = pointer.is_some_and(|q| rect.contains(q));
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} ", icon::TERMINAL_WINDOW)).color(p.weak));
        let prompt = pad.prompt(d, &g);
        let r = ui.add(
            egui::TextEdit::singleline(&mut pad.command)
                .id(id)
                .hint_text(prompt)
                .desired_width(ui.available_width())
                .font(egui::TextStyle::Monospace),
        );
        if pointer_in && ui.memory(|m| m.focused().is_none()) {
            r.request_focus();
        }
        if r.lost_focus() && enter {
            let text = std::mem::take(&mut pad.command);
            edit(d);
            let (ch, dn) = command(pad, d, &g, text.trim(), vars);
            if ch {
                pad.message = settle(d, vars, &[]).unwrap_or_default();
            }
            changed |= ch;
            done |= dn;
            r.request_focus();
        }
    });
    if focused || pointer_in {
        let keys = ui.input(|i| {
            (
                i.key_pressed(egui::Key::F3),
                i.key_pressed(egui::Key::F8),
                i.key_pressed(egui::Key::F9),
                i.key_pressed(egui::Key::F10),
                i.modifiers.command && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                i.modifiers.command
                    && (i.key_pressed(egui::Key::Y)
                        || (i.modifiers.shift && i.key_pressed(egui::Key::Z))),
            )
        });
        pad.snap ^= keys.0;
        if keys.1 {
            pad.ortho = !pad.ortho;
            pad.polar = 0.0;
        }
        pad.grid ^= keys.2;
        if keys.3 {
            pad.polar = if pad.polar > 0.0 { 0.0 } else { pad.polar_step };
            pad.ortho = false;
        }
        if focused && keys.4 {
            pad.history = Some(false);
        }
        if focused && keys.5 {
            pad.history = Some(true);
        }
        if escape {
            if pad.picks.is_empty() && pad.pending.is_none() && !pad.stage {
                pad.start(Tool::Select);
                pad.selection.clear();
            } else {
                pad.finish();
            }
        }
        if del && pad.command.is_empty() && !pad.selection.is_empty() {
            edit(d);
            delete(pad, d);
            pad.message = settle(d, vars, &[]).unwrap_or_default();
            changed = true;
            done = true;
        }
    }
    if !pad.message.is_empty() {
        ui.label(RichText::new(&pad.message).small().color(p.warning));
    }
    // Constraints for what's selected.
    let (ch, dn) = selection_actions(ui, pad, d, &g, vars);
    changed |= ch;
    done |= dn;
    // The constraints and dimensions, to change or remove.
    egui::CollapsingHeader::new(format!("Constraints and dimensions ({})", d.constraints.len()))
        .id_salt("sketch-constraints")
        .default_open(false)
        .show(ui, |ui| {
            let mut gone = None;
            egui::Grid::new("sketch-constraint-list").num_columns(3).show(ui, |ui| {
                for k in 0..d.constraints.len() {
                    let name = describe(d, &d.constraints[k]);
                    let sel = pad.selection.contains(&Sel::Constraint(k));
                    if ui.selectable_label(sel, RichText::new(name).small()).clicked() {
                        pad.selection = vec![Sel::Constraint(k)];
                    }
                    if let Some(v) = dimension_value(&mut d.constraints[k]) {
                        if ui
                            .add(
                                egui::TextEdit::singleline(v)
                                    .desired_width(80.0)
                                    .font(egui::TextStyle::Monospace),
                            )
                            .lost_focus()
                        {
                            pad.message = settle(d, vars, &[]).unwrap_or_default();
                            changed = true;
                            done = true;
                        }
                    } else {
                        ui.label("");
                    }
                    if ui.small_button(icon::X).on_hover_text("Remove").clicked() {
                        gone = Some(k);
                    }
                    ui.end_row();
                }
            });
            if let Some(k) = gone {
                d.constraints.remove(k);
                pad.selection.clear();
                changed = true;
                done = true;
            }
        });
    if points_table(ui, d) {
        pad.message = settle(d, vars, &[]).unwrap_or_default();
        changed = true;
        done = true;
    }
    (changed, done)
}

/// A value to show: the number, or the expression and what it comes to.
fn label(text: &str, now: f64) -> String {
    if text.trim().parse::<f64>().is_ok() {
        fmt2(now)
    } else {
        format!("{} = {}", text.trim(), fmt2(now))
    }
}

fn fmt2(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_owned() }
}

/// A constraint in words.
fn describe(d: &Drawing, c: &Constraint) -> String {
    let curve = |k: usize| match d.curves.get(k) {
        Some(Curve::Line(..)) => format!("line {}", k + 1),
        Some(Curve::Guide(..)) => format!("guide {}", k + 1),
        Some(Curve::Arc { .. }) => format!("arc {}", k + 1),
        Some(Curve::Circle { .. }) => format!("circle {}", k + 1),
        None => format!("curve {}", k + 1),
    };
    let point = |k: usize| format!("point {}", k + 1);
    use Constraint::*;
    match c {
        Coincident(a, b) => format!("Coincident {} {}", point(*a), point(*b)),
        OnCurve(a, b) => format!("{} on {}", point(*a), curve(*b)),
        Horizontal(a) => format!("Horizontal {}", curve(*a)),
        Vertical(a) => format!("Vertical {}", curve(*a)),
        Parallel(a, b) => format!("Parallel {}, {}", curve(*a), curve(*b)),
        Perpendicular(a, b) => format!("Perpendicular {}, {}", curve(*a), curve(*b)),
        Tangent(a, b) => format!("Tangent {}, {}", curve(*a), curve(*b)),
        Equal(a, b) => format!("Equal {}, {}", curve(*a), curve(*b)),
        Fixed(a) => format!("Fixed {}", point(*a)),
        Length(a, _) => format!("Length of {}", curve(*a)),
        Distance(a, b, _) => format!("Distance {} to {}", point(*a), point(*b)),
        DistanceX(a, b, _) => format!("Across {} to {}", point(*a), point(*b)),
        DistanceY(a, b, _) => format!("Up {} to {}", point(*a), point(*b)),
        Radius(a, _) => format!("Radius of {}", curve(*a)),
        Angle(a, b, _) => format!("Angle {} to {} (°)", curve(*a), curve(*b)),
    }
}

/// The points as numbers or expressions; true when one is changed.
fn points_table(ui: &mut egui::Ui, d: &mut Drawing) -> bool {
    let mut done = false;
    egui::CollapsingHeader::new(format!("Points ({})", d.points.len())).id_salt("sketch-points").default_open(false).show(ui, |ui| {
        egui::Grid::new("sketch-point-list").num_columns(3).show(ui, |ui| {
            for (k, q) in d.points.iter_mut().enumerate() {
                ui.label(RichText::new(format!("{}", k + 1)).small());
                for v in q.iter_mut() {
                    done |= ui.add(egui::TextEdit::singleline(v).desired_width(70.0).font(egui::TextStyle::Monospace)).lost_focus();
                }
                ui.end_row();
            }
        });
        ui.label(RichText::new("A point given as an expression (w/2) stays there; constraints move the others.").small().weak());
    });
    done
}

/// Where the pointer lands: on a snap (with the point there, and whether it's on a
/// curve), held by ortho or polar tracking from `from`, or on the grid.
fn snap(
    pad: &Pad,
    d: &Drawing,
    g: &sk::Geometry,
    q: P,
    reach: f64,
    from: Option<P>,
    step: f64,
) -> (P, Option<Snap>, Option<usize>, bool, bool) {
    let dist = |a: P, b: P| (a[0] - b[0]).hypot(a[1] - b[1]);
    let skip = pad.dragging;
    if pad.snap {
        let best = sk::snap_points(d, g)
            .into_iter()
            .filter(|(at, _)| dist(*at, q) <= reach)
            .filter(|(at, _)| skip.is_none_or(|k| dist(g.points[k], *at) > sk::TOL))
            .min_by(|a, b| {
                let rank = |s: Snap| {
                    if matches!(s, Snap::End | Snap::Center | Snap::Intersection) {
                        0.0
                    } else {
                        reach * 0.5
                    }
                };
                (dist(a.0, q) + rank(a.1)).total_cmp(&(dist(b.0, q) + rank(b.1)))
            });
        if let Some((at, kind)) = best {
            let point = sk::point_at(g, at);
            return (at, Some(kind), point, matches!(kind, Snap::Mid | Snap::Intersection), false);
        }
    }
    if let Some(f) = from {
        let (t, held) = sk::track(f, q, pad.ortho, (pad.polar > 0.0).then_some(pad.polar));
        if held {
            let (dx, dy) = (t[0] - f[0], t[1] - f[1]);
            let l = dx.hypot(dy);
            let l2 = if pad.grid { (l / step).round() * step } else { l };
            let k = if l > 1e-12 { l2 / l } else { 1.0 };
            return ([f[0] + dx * k, f[1] + dy * k], None, None, false, true);
        }
    }
    if pad.snap && skip.is_none() {
        if let Some((_, at)) = sk::nearest_curve(d, g, q, reach * 0.6) {
            return (at, Some(Snap::Nearest), sk::point_at(g, at), true, false);
        }
    }
    if pad.grid {
        return (
            [(q[0] / step).round() * step, (q[1] / step).round() * step],
            Some(Snap::Grid),
            None,
            false,
            false,
        );
    }
    (q, None, None, false, false)
}

/// The tool's response to a click (or a typed point) at `pick`; `at` is the pointer,
/// unsnapped, and what's under it. True when the drawing changed.
#[allow(clippy::too_many_arguments)]
fn click(
    pad: &mut Pad,
    d: &mut Drawing,
    g: &sk::Geometry,
    pick: Pick,
    at: P,
    hover_point: Option<usize>,
    hover_curve: Option<usize>,
) -> bool {
    let mut g = g.clone();
    let g = &mut g;
    let dist = |a: P, b: P| (a[0] - b[0]).hypot(a[1] - b[1]);
    match pad.tool {
        Tool::Select => false,
        Tool::Line | Tool::Guide => {
            let Some(prev) = pad.picks.last().copied() else {
                pad.picks.push(pick);
                return false;
            };
            let a = resolve(d, g, prev);
            let b = resolve(d, g, pick);
            if a == b {
                return false;
            }
            d.curves.push(if pad.tool == Tool::Guide {
                Curve::Guide(a, b)
            } else {
                Curve::Line(a, b)
            });
            if pad.first.is_none() {
                pad.first = Some(a);
            }
            if pad.tool == Tool::Guide || Some(b) == pad.first {
                pad.picks.clear();
                pad.first = None;
            } else {
                pad.picks = vec![Pick { at: g.points[b], point: Some(b), on_curve: false }];
            }
            true
        }
        Tool::Rect | Tool::Circle | Tool::Polygon if pad.picks.is_empty() => {
            pad.picks.push(pick);
            false
        }
        Tool::Rect => {
            let a = pad.picks[0].at;
            pad.picks.clear();
            if (a[0] - pick.at[0]).abs() < sk::TOL || (a[1] - pick.at[1]).abs() < sk::TOL {
                return false;
            }
            sk::rectangle(d, a, pick.at);
            true
        }
        Tool::Circle => {
            let c = pad.picks[0];
            pad.picks.clear();
            let r = dist(c.at, pick.at);
            if r < sk::TOL {
                return false;
            }
            let center = resolve(d, g, c);
            d.curves.push(Curve::Circle { center, radius: fmt_num(r) });
            true
        }
        Tool::Polygon => {
            let c = pad.picks[0].at;
            pad.picks.clear();
            if dist(c, pick.at) < sk::TOL {
                return false;
            }
            sk::polygon(d, c, pick.at, pad.sides);
            true
        }
        Tool::Arc3 | Tool::ArcCenter if pad.picks.len() < 2 => {
            pad.picks.push(pick);
            false
        }
        Tool::Arc3 => {
            let (p0, p1) = (pad.picks[0], pad.picks[1]);
            pad.picks.clear();
            let Some(c) = sk::circle_through(p0.at, p1.at, pick.at) else {
                pad.message = "those three points are in a line".into();
                return false;
            };
            let ccw = (p1.at[0] - p0.at[0]) * (pick.at[1] - p0.at[1])
                - (p1.at[1] - p0.at[1]) * (pick.at[0] - p0.at[0])
                > 0.0;
            let (s, e) = (resolve(d, g, p0), resolve(d, g, pick));
            let center = sk::add_point(d, c);
            g.points.push(c);
            d.curves.push(if ccw {
                Curve::Arc { center, from: s, to: e }
            } else {
                Curve::Arc { center, from: e, to: s }
            });
            true
        }
        Tool::ArcCenter => {
            let (c, s0) = (pad.picks[0], pad.picks[1]);
            pad.picks.clear();
            let r = dist(c.at, s0.at);
            let l = dist(c.at, pick.at);
            if r < sk::TOL || l < sk::TOL {
                return false;
            }
            let end = [
                c.at[0] + (pick.at[0] - c.at[0]) * r / l,
                c.at[1] + (pick.at[1] - c.at[1]) * r / l,
            ];
            let center = resolve(d, g, c);
            let from = resolve(d, g, s0);
            let to = resolve(d, g, Pick { at: end, point: None, on_curve: pick.on_curve });
            d.curves.push(Curve::Arc { center, from, to });
            true
        }
        Tool::Trim => hover_curve.is_some_and(|k| sk::trim(d, g, k, at)),
        Tool::Extend => {
            let ok = hover_curve.is_some_and(|k| sk::extend(d, g, k, at));
            if !ok {
                pad.message = "nothing to extend to (lines only)".into();
            }
            ok
        }
        Tool::Offset => {
            if !pad.stage {
                if let Some(k) = hover_curve {
                    pad.selection = vec![Sel::Curve(k)];
                    pad.stage = true;
                }
                return false;
            }
            pad.stage = false;
            let k = pad
                .selection
                .iter()
                .find_map(|s| if let Sel::Curve(k) = s { Some(*k) } else { None });
            pad.selection.clear();
            let ok = k.is_some_and(|k| k < d.curves.len() && sk::offset(d, g, k, pad.distance, at));
            if !ok {
                pad.message = "can't offset that by so much".into();
            }
            ok
        }
        Tool::Mirror => {
            let Some(k) = hover_curve else { return false };
            if !pad.stage {
                let s = Sel::Curve(k);
                match pad.selection.iter().position(|x| *x == s) {
                    Some(i) => {
                        pad.selection.remove(i);
                    }
                    None => pad.selection.push(s),
                }
                return false;
            }
            let curves: Vec<usize> = pad
                .selection
                .iter()
                .filter_map(|s| if let Sel::Curve(k) = s { Some(*k) } else { None })
                .collect();
            pad.selection.clear();
            pad.stage = false;
            let ok = sk::mirror(d, g, &curves, k);
            if !ok {
                pad.message = "mirror across a line or guide".into();
            }
            ok
        }
        Tool::Fillet | Tool::Chamfer => {
            let Some(q) = hover_point else { return false };
            let ok = if pad.tool == Tool::Fillet {
                sk::fillet(d, g, q, pad.radius)
            } else {
                sk::chamfer(d, g, q, pad.chamfer)
            };
            if !ok {
                pad.message = "click a corner where two lines meet, with room for it".into();
            }
            ok
        }
        Tool::Dimension => {
            if let Some(q) = hover_point {
                if let Some(Sel::Point(first)) = pad.selection.first().copied() {
                    pad.selection.clear();
                    if first != q {
                        pad.pending = Some(Constraint::Distance(first, q, String::new()));
                    }
                } else {
                    pad.selection = vec![Sel::Point(q)];
                    pad.picks = vec![pick];
                }
                return false;
            }
            if let Some(k) = hover_curve {
                pad.pending = Some(match d.curves[k] {
                    Curve::Line(..) | Curve::Guide(..) => Constraint::Length(k, String::new()),
                    _ => Constraint::Radius(k, String::new()),
                });
                pad.picks.clear();
            }
            false
        }
    }
}

/// Runs a typed command, value or point. Returns whether the drawing changed and whether
/// that is an edit to keep.
fn command(
    pad: &mut Pad,
    d: &mut Drawing,
    g: &sk::Geometry,
    text: &str,
    vars: &dyn Fn(&str) -> Option<f64>,
) -> (bool, bool) {
    let lower = text.to_ascii_lowercase();
    // A dimension waiting for its value.
    if let Some(mut c) = pad.pending.take() {
        let value = if text.is_empty() { fmt_num(measure(d, g, &c)) } else { text.to_owned() };
        if let Err(e) = circuiter_3d::cad::value(&value, vars) {
            pad.message = e;
            pad.pending = Some(c);
            return (false, false);
        }
        *dimension_value(&mut c).expect("a dimension") = value;
        d.constraints.push(c);
        pad.selection.clear();
        pad.picks.clear();
        return (true, true);
    }
    if text.is_empty() {
        pad.finish();
        return (false, false);
    }
    // C closes a run of lines back to its start.
    if let (Tool::Line, "c" | "close", Some(f)) = (pad.tool, lower.as_str(), pad.first) {
        let pick = Pick { at: g.points[f], point: Some(f), on_curve: false };
        let ch = click(pad, d, g, pick, pick.at, None, None);
        return (ch, ch);
    }
    if let Some(&(tool, ..)) = TOOLS.iter().find(|t| t.2.contains(&lower.as_str())) {
        pad.start(tool);
        return (false, false);
    }
    let words: Vec<&str> = lower.split_whitespace().collect();
    match words.as_slice() {
        ["z" | "zoom" | "fit"] => {
            pad.view = None;
            return (false, false);
        }
        ["u" | "undo"] => {
            pad.history = Some(false);
            return (false, false);
        }
        ["redo"] => {
            pad.history = Some(true);
            return (false, false);
        }
        ["ortho"] => {
            pad.ortho = !pad.ortho;
            pad.polar = 0.0;
            return (false, false);
        }
        ["snap" | "osnap"] => {
            pad.snap = !pad.snap;
            return (false, false);
        }
        ["grid"] => {
            pad.grid = !pad.grid;
            return (false, false);
        }
        ["polar"] => {
            pad.polar = if pad.polar > 0.0 { 0.0 } else { pad.polar_step };
            pad.ortho = false;
            return (false, false);
        }
        ["polar", a] => {
            if let Ok(a) = a.parse::<f64>() {
                pad.polar_step = a.abs();
                pad.polar = a.abs();
                pad.ortho = false;
            }
            return (false, false);
        }
        _ => {}
    }
    // A number for the tools that take one.
    if !text.contains([',', '<', '@']) {
        if let Ok(v) = circuiter_3d::cad::value(text, vars) {
            let v = f64::from(v);
            match pad.tool {
                Tool::Offset if !pad.stage => {
                    pad.distance = v.abs();
                    return (false, false);
                }
                Tool::Fillet => {
                    pad.radius = v.abs();
                    return (false, false);
                }
                Tool::Chamfer => {
                    pad.chamfer = v.abs();
                    return (false, false);
                }
                Tool::Polygon if pad.picks.is_empty() && v.fract() == 0.0 && v >= 3.0 => {
                    pad.sides = v as usize;
                    return (false, false);
                }
                _ => {}
            }
        }
    }
    // A point.
    let last = pad.picks.last().map(|p| p.at);
    match sk::typed(text, last, pad.cursor, vars) {
        Ok(q) => {
            let pick = Pick { at: q, point: sk::point_at(g, q), on_curve: false };
            let ch = if pad.tool == Tool::Select {
                pad.message = "start a tool first (L, C, REC…)".into();
                false
            } else {
                click(pad, d, g, pick, q, None, None)
            };
            (ch, ch)
        }
        Err(e) => {
            pad.message = format!("not a command or a point: {e}");
            (false, false)
        }
    }
}

/// Removes what's selected: constraints, curves, and the curves through selected points.
fn delete(pad: &mut Pad, d: &mut Drawing) {
    let mut cons: Vec<usize> = pad
        .selection
        .iter()
        .filter_map(|s| if let Sel::Constraint(k) = s { Some(*k) } else { None })
        .collect();
    cons.sort_unstable();
    for k in cons.into_iter().rev() {
        if k < d.constraints.len() {
            d.constraints.remove(k);
        }
    }
    let points: Vec<usize> = pad
        .selection
        .iter()
        .filter_map(|s| if let Sel::Point(k) = s { Some(*k) } else { None })
        .collect();
    let curves: Vec<usize> = (0..d.curves.len())
        .filter(|k| {
            pad.selection.contains(&Sel::Curve(*k))
                || sk::refs(&d.curves[*k]).iter().any(|p| points.contains(p))
        })
        .collect();
    d.constraints.retain(|c| !matches!(c, Constraint::Fixed(p) | Constraint::Coincident(p, _) | Constraint::OnCurve(p, _) if points.contains(p)));
    sk::remove_curves(d, &curves);
    pad.selection.clear();
}

/// Buttons that constrain what's selected. Returns whether the drawing changed and
/// whether that's an edit to keep.
fn selection_actions(
    ui: &mut egui::Ui,
    pad: &mut Pad,
    d: &mut Drawing,
    g: &sk::Geometry,
    vars: &dyn Fn(&str) -> Option<f64>,
) -> (bool, bool) {
    let curves: Vec<usize> = pad
        .selection
        .iter()
        .filter_map(|s| if let Sel::Curve(k) = s { Some(*k) } else { None })
        .filter(|k| *k < d.curves.len())
        .collect();
    let points: Vec<usize> = pad
        .selection
        .iter()
        .filter_map(|s| if let Sel::Point(k) = s { Some(*k) } else { None })
        .filter(|k| *k < d.points.len())
        .collect();
    if curves.is_empty() && points.is_empty() && pad.selection.is_empty() {
        return (false, false);
    }
    let is_line = |k: usize| matches!(d.curves[k], Curve::Line(..) | Curve::Guide(..));
    let lines = curves.iter().all(|k| is_line(*k));
    let round = curves.iter().all(|k| !is_line(*k));
    let two = curves.len() == 2 && points.is_empty();
    let mut add: Vec<Constraint> = Vec::new();
    let mut dimension = None;
    let mut guide = false;
    let mut remove = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label(RichText::new("Constrain").small().weak());
        let button = |ui: &mut egui::Ui, on: bool, text: &str, hint: &str| {
            ui.add_enabled(on, egui::Button::new(text).small()).on_hover_text(hint).clicked()
        };
        let only_lines = !curves.is_empty() && lines && points.is_empty();
        if button(ui, only_lines, "H", "Horizontal") {
            add.extend(curves.iter().map(|k| Constraint::Horizontal(*k)));
        }
        if button(ui, only_lines, "V", "Vertical") {
            add.extend(curves.iter().map(|k| Constraint::Vertical(*k)));
        }
        if button(ui, two && lines, "∥", "Parallel") {
            add.push(Constraint::Parallel(curves[0], curves[1]));
        }
        if button(ui, two && lines, "⊥", "Perpendicular") {
            add.push(Constraint::Perpendicular(curves[0], curves[1]));
        }
        if button(
            ui,
            two && !lines,
            "T",
            "Tangent: a line and an arc or circle, or two arcs or circles",
        ) {
            add.push(Constraint::Tangent(curves[0], curves[1]));
        }
        if button(
            ui,
            two && (lines || round),
            "=",
            "Equal: lines of one length, or arcs and circles of one radius",
        ) {
            add.push(Constraint::Equal(curves[0], curves[1]));
        }
        let coincide =
            (points.len() == 2 && curves.is_empty()) || (points.len() == 1 && curves.len() == 1);
        if button(ui, coincide, "●", "Coincident: two points in one place, or a point on a curve")
        {
            add.push(if curves.is_empty() {
                Constraint::Coincident(points[0], points[1])
            } else {
                Constraint::OnCurve(points[0], curves[0])
            });
        }
        if button(
            ui,
            !points.is_empty() && curves.is_empty(),
            icon::LOCK,
            "Fixed: the points stay where they are",
        ) {
            add.extend(points.iter().map(|k| Constraint::Fixed(*k)));
        }
        ui.separator();
        ui.label(RichText::new("Dimension").small().weak());
        let one = curves.len() == 1 && points.is_empty();
        if button(ui, one && lines, "Length", "The line's length") {
            dimension = Some(Constraint::Length(curves[0], String::new()));
        }
        if button(ui, one && round, "Radius", "The arc's or circle's radius") {
            dimension = Some(Constraint::Radius(curves[0], String::new()));
        }
        if button(ui, two && lines, "Angle", "From the first line to the second, counter-clockwise")
        {
            dimension = Some(Constraint::Angle(curves[0], curves[1], String::new()));
        }
        let pair = points.len() == 2 && curves.is_empty();
        if button(ui, pair, "Distance", "Between the two points") {
            dimension = Some(Constraint::Distance(points[0], points[1], String::new()));
        }
        if button(ui, pair, "↔", "Across: the horizontal distance between the points") {
            dimension = Some(Constraint::DistanceX(points[0], points[1], String::new()));
        }
        if button(ui, pair, "↕", "Up: the vertical distance between the points") {
            dimension = Some(Constraint::DistanceY(points[0], points[1], String::new()));
        }
        ui.separator();
        if button(
            ui,
            !curves.is_empty() && lines && points.is_empty(),
            "Guide",
            "Turn lines into guides (construction) or back",
        ) {
            guide = true;
        }
        if button(ui, true, icon::TRASH, "Delete what's selected (Delete)") {
            remove = true;
        }
    });
    let mut changed = false;
    if remove {
        delete(pad, d);
        changed = true;
    }
    if guide {
        for k in &curves {
            d.curves[*k] = match d.curves[*k] {
                Curve::Line(a, b) => Curve::Guide(a, b),
                Curve::Guide(a, b) => Curve::Line(a, b),
                ref c => c.clone(),
            };
        }
        changed = true;
    }
    if !add.is_empty() {
        sk::store(d, g);
        d.constraints.extend(add);
        changed = true;
    }
    if let Some(c) = dimension {
        pad.pending = Some(c);
        pad.command.clear();
        pad.message = "type the value (or an expression) and press Enter".into();
    }
    if changed {
        pad.selection.clear();
        pad.message = settle(d, vars, &[]).unwrap_or_default();
    }
    (changed, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Commands and typed points, as at AutoCAD's command line: a rectangle by its
    /// corners, lines closed with C, a fillet at a clicked corner, a typed dimension.
    #[test]
    fn the_command_line_draws_constrains_and_dimensions() {
        let none = |_: &str| None;
        let mut pad = Pad::default();
        let mut d = Drawing::default();
        let run = |pad: &mut Pad, d: &mut Drawing, text: &str| {
            let g = sk::solve(d, &none, &[]).unwrap().0;
            let (changed, _) = command(pad, d, &g, text, &none);
            if changed {
                assert_eq!(settle(d, &none, &[]), None);
            }
        };
        for line in ["rec", "0,0", "@20,10"] {
            run(&mut pad, &mut d, line);
        }
        assert_eq!(d.curves.len(), 4);
        let area = |d: &Drawing| {
            let g = sk::geometry(d, &none).unwrap();
            let l = &sk::loops(d, &g)[0];
            (0..l.len())
                .map(|k| {
                    let (a, b) = (l[k], l[(k + 1) % l.len()]);
                    (a[0] * b[1] - b[0] * a[1]) / 2.0
                })
                .sum::<f64>()
        };
        assert!((area(&d) - 200.0).abs() < 1e-9);
        // A triangle beside it: polar entry, a bare length along the pointer, then C.
        pad.cursor = [40.0, 0.0];
        for line in ["l", "30,0", "@10<0", "@10<90", "c"] {
            run(&mut pad, &mut d, line);
        }
        assert_eq!(d.curves.len(), 7, "{:?}", d.curves);
        assert_eq!(pad.picks.len(), 0, "closing ends the run");
        // Fillet radius 2, then a click on the rectangle's corner at the origin.
        run(&mut pad, &mut d, "f");
        run(&mut pad, &mut d, "2");
        let g = sk::geometry(&d, &none).unwrap();
        let corner = sk::point_at(&g, [0.0, 0.0]).unwrap();
        let pick = Pick { at: [0.0, 0.0], point: Some(corner), on_curve: false };
        assert!(click(&mut pad, &mut d, &g, pick, [0.0, 0.0], Some(corner), None));
        assert!(d.curves.iter().any(|c| matches!(c, Curve::Arc { .. })));
        // Dimension the rectangle's top edge 30 long: the rectangle follows.
        run(&mut pad, &mut d, "dim");
        let g = sk::geometry(&d, &none).unwrap();
        let top = sk::nearest_curve(&d, &g, [10.0, 10.0], 0.1).unwrap().0;
        click(&mut pad, &mut d, &g, pick, [10.0, 10.0], None, Some(top));
        assert!(matches!(pad.pending, Some(Constraint::Length(k, _)) if k == top));
        run(&mut pad, &mut d, "30");
        let g = sk::geometry(&d, &none).unwrap();
        assert!((measure(&d, &g, d.constraints.last().unwrap()) - 30.0).abs() < 1e-6);
        // Unknown words say so.
        run(&mut pad, &mut d, "frobnicate");
        assert!(pad.message.starts_with("not a command"), "{}", pad.message);
    }

    /// The editor drawn for real, a rectangle typed at its command line.
    #[test]
    fn the_sketch_editor_takes_typed_commands() {
        use egui_kittest::kittest::Queryable;
        let mut h = egui_kittest::Harness::new_ui_state(
            |ui, (pad, d): &mut (Pad, Drawing)| {
                sketch_editor(ui, pad, d, Kind::Profile, &[], &|_| None);
            },
            (Pad::default(), Drawing::default()),
        );
        h.run();
        for line in ["rec", "-5,-5", "@10,10"] {
            h.get_by_role(egui::accesskit::Role::TextInput).focus();
            h.run();
            h.get_by_role(egui::accesskit::Role::TextInput).type_text(line);
            h.run();
            h.key_press(egui::Key::Enter);
            h.run();
        }
        let (_, d) = h.state();
        assert_eq!(d.curves.len(), 4, "{:?}", d);
        assert_eq!(d.constraints.len(), 4, "held square");
    }
}
