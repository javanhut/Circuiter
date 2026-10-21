//! The Diagram view: the sheet laid out afresh by the layout engine, signals flowing
//! left to right, as an RTL viewer shows a design. It is made from the sheet whenever
//! the sheet changes, and only looked at: click a wire to follow its net, double-click
//! a part to go inside it. A Verilog file's modules are shown the same way, as boxes
//! for their registers, logic and instances.

use super::*;
use circuiter_diagram::rtl;
use circuiter_model::schematic::Schematic;
use circuiter_scene::Scene;

#[derive(Debug, Default)]
pub(super) struct DiagramView {
    /// What it was made from (the cell and the design's revision) and the drawing.
    made: Option<(CellId, u64, Schematic, Scene)>,
    pub camera: Option<canvas::Camera>,
    pub canvas: CanvasState,
    /// The net clicked, numbered as the diagram's own nets are.
    net: Option<usize>,
}

impl DiagramView {
    /// The sheet as laid out, if it was made from `id` at `revision`.
    pub fn laid_out(&self, id: &CellId, revision: u64) -> Option<Schematic> {
        match &self.made {
            Some((c, r, sheet, _)) if c == id && *r == revision => Some(sheet.clone()),
            _ => None,
        }
    }

    /// Where part `name` is on screen.
    #[cfg(test)]
    pub fn part_on_screen(&self, name: &str) -> Option<egui::Pos2> {
        let (_, _, sheet, scene) = self.made.as_ref()?;
        let i = sheet.instances.iter().position(|inst| inst.name == name)?;
        Some(self.camera?.to_screen(self.canvas.view?, scene.instance_bodies[i].center()))
    }
}

impl CircuiterApp {
    pub(super) fn diagram_view(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &mut self.workspace else { return };
        let id = ws.current().cell.clone();
        let fresh = matches!(&self.diagram.made, Some((c, r, ..)) if *c == id && *r == ws.revision);
        if !fresh {
            let Some(def) = ws.design.cell(&id) else { return };
            match circuiter_diagram::arrange(&ws.design, &id) {
                Ok(sheet) => {
                    let scene = Scene::build_from(&ws.design, &id, &def.cell, &sheet);
                    let same_cell = self.diagram.made.as_ref().is_some_and(|(c, ..)| *c == id);
                    if !same_cell {
                        self.diagram.camera = None;
                        self.diagram.net = None;
                    }
                    self.diagram.made = Some((id, ws.revision, sheet, scene));
                }
                Err(e) => {
                    self.error = Some(e);
                    return;
                }
            }
        }
        let Some((_, _, sheet, scene)) = &self.diagram.made else { return };
        let highlight = Highlight { selection: None, net: self.diagram.net };
        let out = canvas::show(
            ui,
            scene,
            &mut self.diagram.camera,
            &mut self.diagram.canvas,
            self.theme,
            highlight,
            &canvas::Overlay::default(),
            self.show_grid,
        );
        self.cursor = out.cursor;
        // The diagram's parts and wires aren't the sheet's: nothing to inspect there.
        self.hovered = None;
        let conn = &scene.connectivity;
        let net_of = |owner: Owner| match owner {
            Owner::Wire(i) => conn.net_of(Item::Wire(i)),
            Owner::Label(i) => conn.net_of(Item::Label(i)),
            Owner::Port(i) => conn.net_of(Item::Port(i)),
            _ => None,
        };
        let mut open = None;
        for g in out.gestures {
            match g {
                canvas::Gesture::Click { at, .. } => {
                    self.diagram.net = scene.hit_test(at, out.tolerance).and_then(net_of);
                }
                canvas::Gesture::DoubleClick { at } => {
                    if let Some(Owner::Instance(i)) = scene.hit_test(at, out.tolerance) {
                        open = Some(sheet.instances[i].name.clone());
                    }
                }
                _ => {}
            }
        }
        // Parts keep their names in the diagram, so the sheet's own is found by name.
        if let Some(name) = open {
            let index =
                ws.schematic().and_then(|s| s.instances.iter().position(|i| i.name == name));
            if let Some(i) = index {
                ws.descend(i);
            }
        }
    }
}

/// A Verilog file's RTL diagram, in place of its text.
#[derive(Debug, Default)]
pub(super) struct RtlView {
    pub shown: bool,
    /// The module shown, when it isn't the file's last.
    module: Option<String>,
    /// What it was made from (the text and module), and the drawing, or why there's none.
    made: Option<(String, Option<String>, Result<Made, String>)>,
    camera: Option<canvas::Camera>,
    canvas: CanvasState,
    net: Option<usize>,
}

#[derive(Debug)]
struct Made {
    scene: Scene,
    sheet: Schematic,
    modules: Vec<String>,
    kinds: std::collections::BTreeMap<String, String>,
}

/// Draws `view` of the Verilog in `text`, made afresh when the text changes.
pub(super) fn rtl_pane(
    ui: &mut egui::Ui,
    view: &mut RtlView,
    text: &str,
    theme: &'static Theme,
    grid: bool,
) {
    let fresh = matches!(&view.made, Some((t, m, _)) if t == text && *m == view.module);
    if !fresh {
        let made = rtl::diagram(text, view.module.as_deref()).map(|d| {
            let def = d.design.cell(&d.id).expect("made");
            let scene = Scene::build_from(&d.design, &d.id, &def.cell, &d.sheet);
            Made { scene, sheet: d.sheet, modules: d.modules, kinds: d.kinds }
        });
        view.made = Some((text.to_owned(), view.module.clone(), made));
    }
    let p = style::palette(ui);
    let Some((_, _, made)) = &view.made else { return };
    let made = match made {
        Ok(m) => m,
        Err(e) => {
            ui.add_space(12.0);
            ui.label(RichText::new(format!("{}  {e}", style::icon::X_CIRCLE)).color(p.error));
            return;
        }
    };
    // Which module: the file's last unless another is chosen.
    let mut pick = None;
    egui::Frame::new().fill(p.raised).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Module").color(p.weak));
            let shown =
                view.module.clone().or_else(|| made.modules.last().cloned()).unwrap_or_default();
            egui::ComboBox::from_id_salt("rtl-module").selected_text(&shown).show_ui(ui, |ui| {
                for m in &made.modules {
                    if ui.selectable_label(*m == shown, m).clicked() {
                        pick = Some(m.clone());
                    }
                }
            });
            ui.label(
                RichText::new("click a wire to follow it · double-click an instance to go inside")
                    .small()
                    .color(p.weak),
            );
        });
    });
    let highlight = Highlight { selection: None, net: view.net };
    let out = canvas::show(
        ui,
        &made.scene,
        &mut view.camera,
        &mut view.canvas,
        theme,
        highlight,
        &canvas::Overlay::default(),
        grid,
    );
    let conn = &made.scene.connectivity;
    for g in out.gestures {
        match g {
            canvas::Gesture::Click { at, .. } => {
                view.net = match made.scene.hit_test(at, out.tolerance) {
                    Some(Owner::Wire(i)) => conn.net_of(Item::Wire(i)),
                    Some(Owner::Label(i)) => conn.net_of(Item::Label(i)),
                    Some(Owner::Port(i)) => conn.net_of(Item::Port(i)),
                    _ => None,
                };
            }
            canvas::Gesture::DoubleClick { at } => {
                if let Some(Owner::Instance(i)) = made.scene.hit_test(at, out.tolerance) {
                    let kind = made.kinds.get(&made.sheet.instances[i].name);
                    pick = kind.filter(|k| made.modules.contains(k)).cloned().or(pick);
                }
            }
            _ => {}
        }
    }
    if let Some(m) = pick {
        view.module = Some(m);
        view.camera = None;
        view.net = None;
    }
}
