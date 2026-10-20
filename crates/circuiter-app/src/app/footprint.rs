//! Choosing a cell's footprint in the inspector: a package, its parameters, which pad
//! each port goes to, and a drawing of the result with every pad's number and port.

use crate::style::{self, icon};
use crate::workspace::Workspace;
use circuiter_model::board::{FootprintView, Graphic, Layer, PadKind, PadShape};
use circuiter_pcb::footprints::{self, PACKAGES};
use eframe::egui::{self, Color32, RichText, Stroke};

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

/// The cell's footprint section: chosen, changed, checked and drawn.
pub fn section(ui: &mut egui::Ui, ws: &mut Workspace) {
    let p = style::palette(ui);
    let Some(def) = ws.current_def().cloned() else { return };
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
                painter.circle_filled(c, w.min(h) / 2.0, copper);
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
