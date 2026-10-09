//! Application state and window layout.

use crate::Args;
use crate::canvas::{self, Camera, CanvasEvent, Highlight};
use anyhow::{Context as _, Result};
use circuiter_model::check::{Diagnostic, Severity, check_libraries};
use circuiter_model::connectivity::{Item, NetNameSource};
use circuiter_model::schematic::Schematic;
use circuiter_model::{CellDef, CellId, Design, Project, ViewKind};
use circuiter_scene::{Owner, Pt, Scene, Theme, param_display, to_svg};
use eframe::egui::{self, Align, Color32, Key, Layout, RichText, ViewportCommand};
use std::path::{Path, PathBuf};

pub struct CircuiterApp {
    workspace: Option<Workspace>,
    error: Option<String>,
    theme: &'static Theme,
    applied_theme: Option<&'static str>,
    show_grid: bool,
    cursor: Option<Pt>,
    hovered: Option<Owner>,
    screenshot: Option<PathBuf>,
    frames: u32,
}

/// What the canvas is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Schematic,
    /// A library part without a schematic: show its symbol.
    Symbol,
}

/// One step of the hierarchy path being viewed.
struct Level {
    cell: CellId,
    /// The instance in the parent level that was descended into.
    via: Option<String>,
    camera: Option<Camera>,
}

struct Workspace {
    project: Project,
    design: Design,
    diagnostics: Vec<Diagnostic>,
    levels: Vec<Level>,
    mode: Mode,
    scene: Scene,
    selection: Option<Owner>,
    net: Option<usize>,
}

impl Workspace {
    fn open(path: &Path) -> Result<Workspace> {
        let root = Project::find_root(path)
            .with_context(|| format!("no Circuiter project found at {}", path.display()))?;
        let project = Project::load(&root)?;
        let design = Design::for_project(&project, circuiter_stdlib::load()?)?;
        let mut libraries = vec![project.name()];
        libraries.extend(project.manifest.libraries.keys().map(String::as_str));
        let diagnostics = check_libraries(&design, &libraries);
        let top = CellId::new(project.name(), &project.manifest.project.top);
        let scene = Scene::build(&design, &top)
            .or_else(|| Scene::symbol_preview(&design, &top))
            .with_context(|| format!("top cell {top} has nothing to show"))?;
        let mode = if design.cell(&top).is_some_and(|d| d.schematic.is_some()) {
            Mode::Schematic
        } else {
            Mode::Symbol
        };
        Ok(Workspace {
            project,
            design,
            diagnostics,
            levels: vec![Level { cell: top, via: None, camera: None }],
            mode,
            scene,
            selection: None,
            net: None,
        })
    }

    fn current(&self) -> &Level {
        self.levels.last().expect("at least one level")
    }

    fn current_def(&self) -> Option<&CellDef> {
        self.design.cell(&self.current().cell)
    }

    fn schematic(&self) -> Option<&Schematic> {
        match self.mode {
            Mode::Schematic => self.current_def()?.schematic.as_ref(),
            Mode::Symbol => None,
        }
    }

    fn rebuild_scene(&mut self) {
        let id = self.current().cell.clone();
        let (scene, mode) = match Scene::build(&self.design, &id) {
            Some(s) => (Some(s), Mode::Schematic),
            None => (Scene::symbol_preview(&self.design, &id), Mode::Symbol),
        };
        if let Some(scene) = scene {
            self.scene = scene;
            self.mode = mode;
        }
        self.selection = None;
        self.net = None;
    }

    fn open_cell(&mut self, id: CellId) {
        self.levels = vec![Level { cell: id, via: None, camera: None }];
        self.rebuild_scene();
    }

    fn descend(&mut self, instance: usize) {
        let Some(schematic) = self.schematic() else { return };
        let inst = &schematic.instances[instance];
        let Some((child, _)) = self.design.resolve(&inst.cell, &self.current().cell.library) else {
            return;
        };
        let via = Some(inst.name.clone());
        self.levels.push(Level { cell: child, via, camera: None });
        self.rebuild_scene();
    }

    fn ascend_to(&mut self, depth: usize) {
        if depth < self.levels.len() {
            let child = self.levels[depth].via.clone();
            self.levels.truncate(depth);
            self.rebuild_scene();
            // Keep the instance we came out of selected, as a landmark.
            if let (Some(name), Some(schematic)) = (child, self.schematic()) {
                self.selection =
                    schematic.instances.iter().position(|i| i.name == name).map(Owner::Instance);
            }
        }
    }

    fn select(&mut self, owner: Option<Owner>) {
        let conn = &self.scene.connectivity;
        self.net = match owner {
            Some(Owner::Wire(i)) => conn.net_of(Item::Wire(i)),
            Some(Owner::Label(i)) => conn.net_of(Item::Label(i)),
            Some(Owner::Port(i)) => conn.net_of(Item::Port(i)),
            _ => None,
        };
        self.selection = owner;
    }

    fn select_instance_named(&mut self, name: &str) {
        let found = self
            .schematic()
            .and_then(|s| s.instances.iter().position(|i| i.name == name))
            .map(Owner::Instance);
        self.select(found);
    }

    fn problems(&self) -> Vec<(Severity, String)> {
        let id = self.current().cell.to_string();
        let mut out: Vec<(Severity, String)> = self
            .diagnostics
            .iter()
            .filter(|d| d.location == id || d.location.starts_with(&format!("{id}/")))
            .map(|d| (d.severity, d.message.clone()))
            .collect();
        if let Some(schematic) = self.schematic() {
            let conn = &self.scene.connectivity;
            for &i in &conn.dangling_pins {
                let pin = &conn.pins[i];
                let inst = &schematic.instances[pin.instance].name;
                out.push((Severity::Warning, format!("pin {inst}.{} is not connected", pin.port)));
            }
            for p in &conn.dangling_wire_ends {
                out.push((Severity::Warning, format!("wire end at {p} is not connected")));
            }
            for net in &conn.nets {
                if net.names.len() > 1 {
                    out.push((
                        Severity::Warning,
                        format!("net has conflicting names: {}", net.names.join(", ")),
                    ));
                }
            }
        }
        out
    }
}

impl CircuiterApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, args: Args) -> Self {
        let theme = match args.theme.as_deref() {
            Some(t) => Theme::ALL.into_iter().find(|th| th.name.eq_ignore_ascii_case(t)),
            None => None,
        }
        .unwrap_or(&Theme::BENCH);
        let mut app = CircuiterApp {
            workspace: None,
            error: None,
            theme,
            applied_theme: None,
            show_grid: true,
            cursor: None,
            hovered: None,
            screenshot: args.screenshot,
            frames: 0,
        };
        if let Some(path) = args.project {
            app.open(&path);
        }
        if let Some(ws) = &mut app.workspace {
            if let Some(cell) = args.cell {
                match cell.parse::<circuiter_model::CellRef>() {
                    Ok(r) => match ws.design.resolve(&r, ws.project.name()) {
                        Some((id, _)) => ws.open_cell(id),
                        None => app.error = Some(format!("cell `{cell}` not found")),
                    },
                    Err(e) => app.error = Some(e),
                }
            }
            if let Some(name) = args.select {
                ws.select_instance_named(&name);
            }
        }
        app
    }

    fn open(&mut self, path: &Path) {
        match Workspace::open(path) {
            Ok(ws) => {
                self.workspace = Some(ws);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    fn open_dialog(&mut self) {
        if let Some(dir) = rfd::FileDialog::new().set_title("Open Circuiter project").pick_folder()
        {
            self.open(&dir);
        }
    }

    fn reload(&mut self) {
        let Some(ws) = &self.workspace else { return };
        let root = ws.project.root.clone();
        let path: Vec<(CellId, Option<String>)> =
            ws.levels.iter().map(|l| (l.cell.clone(), l.via.clone())).collect();
        self.open(&root);
        // Restore the hierarchy path as far as it still exists.
        if let Some(ws) = &mut self.workspace {
            if let Some((first, _)) = path.first() {
                if ws.design.cell(first).is_some() {
                    ws.open_cell(first.clone());
                    for (_, via) in path.iter().skip(1) {
                        let found = via.as_ref().and_then(|name| {
                            ws.schematic()?.instances.iter().position(|i| &i.name == name)
                        });
                        match found {
                            Some(i) => ws.descend(i),
                            None => break,
                        }
                    }
                }
            }
        }
    }

    fn export_svg(&mut self) {
        let Some(ws) = &self.workspace else { return };
        let name = format!("{}.svg", ws.current().cell.cell);
        if let Some(path) =
            rfd::FileDialog::new().set_file_name(name).add_filter("SVG", &["svg"]).save_file()
        {
            if let Err(e) = std::fs::write(&path, to_svg(&ws.scene, self.theme)) {
                self.error = Some(format!("writing {}: {e}", path.display()));
            }
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.applied_theme == Some(self.theme.name) {
            return;
        }
        let mut visuals =
            if self.theme.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        let accent = canvas::color(self.theme.highlight);
        visuals.selection.bg_fill = accent.gamma_multiply(0.6);
        visuals.hyperlink_color = accent;
        ctx.set_visuals(visuals);
        self.applied_theme = Some(self.theme.name);
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        self.frames += 1;
        ctx.request_repaint();
        if self.frames == 10 {
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Err(e) = image::save_buffer(
                &path,
                &bytes,
                w as u32,
                h as u32,
                image::ExtendedColorType::Rgba8,
            ) {
                eprintln!("error: saving screenshot to {}: {e}", path.display());
            }
            self.screenshot = None;
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (fit, up, escape, reload, open) = ctx.input(|i| {
            (
                i.key_pressed(Key::F) || i.key_pressed(Key::Home),
                i.key_pressed(Key::Backspace) || (i.modifiers.alt && i.key_pressed(Key::ArrowUp)),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::F5),
                i.modifiers.command && i.key_pressed(Key::O),
            )
        });
        if open {
            self.open_dialog();
        }
        if reload {
            self.reload();
        }
        if let Some(ws) = &mut self.workspace {
            if fit {
                ws.levels.last_mut().expect("level").camera = None;
            }
            if up && ws.levels.len() > 1 {
                ws.ascend_to(ws.levels.len() - 1);
            }
            if escape {
                ws.select(None);
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open Project…").clicked() {
                    self.open_dialog();
                }
                let has_ws = self.workspace.is_some();
                if ui.add_enabled(has_ws, egui::Button::new("Reload")).clicked() {
                    self.reload();
                }
                ui.separator();
                if ui.add_enabled(has_ws, egui::Button::new("Export SVG…")).clicked() {
                    self.export_svg();
                }
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                if ui.button("Fit to Window").clicked() {
                    if let Some(ws) = &mut self.workspace {
                        ws.levels.last_mut().expect("level").camera = None;
                    }
                }
                ui.checkbox(&mut self.show_grid, "Grid");
                ui.separator();
                for theme in Theme::ALL {
                    if ui.radio(std::ptr::eq(self.theme, theme), theme.name).clicked() {
                        self.theme = theme;
                    }
                }
            });
            ui.menu_button("Navigate", |ui| {
                let Some(ws) = &mut self.workspace else { return };
                if ui.add_enabled(ws.levels.len() > 1, egui::Button::new("Up One Level")).clicked()
                {
                    ws.ascend_to(ws.levels.len() - 1);
                }
                if ui.add_enabled(ws.levels.len() > 1, egui::Button::new("Top Level")).clicked() {
                    ws.ascend_to(1);
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let Some(ws) = &mut self.workspace else {
                ui.label(RichText::new("No project open").weak());
                return;
            };
            let mut go_to = None;
            for (depth, level) in ws.levels.iter().enumerate() {
                if depth > 0 {
                    ui.label(RichText::new("›").weak());
                }
                let label = match &level.via {
                    Some(inst) => format!("{inst}  ({})", level.cell.cell),
                    None => level.cell.to_string(),
                };
                let current = depth + 1 == ws.levels.len();
                let text = RichText::new(label).monospace();
                if ui
                    .selectable_label(current, if current { text.strong() } else { text })
                    .clicked()
                    && !current
                {
                    go_to = Some(depth + 1);
                }
            }
            if ws.mode == Mode::Symbol {
                ui.label(RichText::new("symbol").weak().italics());
            }
            if let Some(depth) = go_to {
                ws.ascend_to(depth);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                for theme in Theme::ALL.into_iter().rev() {
                    if ui.selectable_label(std::ptr::eq(self.theme, theme), theme.name).clicked() {
                        self.theme = theme;
                    }
                }
            });
        });
    }

    fn browser(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &mut self.workspace else { return };
        ui.add_space(4.0);
        ui.heading(ws.project.name());
        if !ws.project.manifest.project.description.is_empty() {
            ui.label(RichText::new(&ws.project.manifest.project.description).weak().small());
        }
        ui.separator();
        let current = ws.current().cell.clone();
        let mut open = None;
        let mut order: Vec<&str> = vec![ws.project.name()];
        order.extend(ws.project.manifest.libraries.keys().map(String::as_str));
        order.push(circuiter_model::STD_LIBRARY);
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            for lib_name in order {
                let Some(lib) = ws.design.library(lib_name) else { continue };
                egui::CollapsingHeader::new(RichText::new(lib_name).strong())
                    .id_salt(lib_name)
                    .default_open(true)
                    .show(ui, |ui| {
                        for def in lib.cells.values() {
                            let id = CellId::new(lib_name, &def.name);
                            let kind = if def.schematic.is_some() { "schematic" } else { "part" };
                            ui.horizontal(|ui| {
                                let r = ui.selectable_label(
                                    id == current,
                                    RichText::new(&def.name).monospace(),
                                );
                                ui.label(RichText::new(kind).weak().small());
                                let r = if def.cell.description.is_empty() {
                                    r
                                } else {
                                    r.on_hover_text(&def.cell.description)
                                };
                                if r.clicked() {
                                    open = Some(id);
                                }
                            });
                        }
                    });
            }
        });
        if let Some(id) = open {
            ws.open_cell(id);
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui) {
        let Some(ws) = &mut self.workspace else { return };
        ui.add_space(4.0);
        let mut action: Option<InspectorAction> = None;
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| match ws.selection {
            Some(Owner::Instance(i)) => action = instance_panel(ui, ws, i),
            Some(Owner::Wire(_) | Owner::Label(_) | Owner::Port(_)) => {
                action = ws.net.and_then(|n| net_panel(ui, ws, n));
            }
            _ => cell_panel(ui, ws),
        });
        match action {
            Some(InspectorAction::Descend(i)) => ws.descend(i),
            Some(InspectorAction::SelectNet(n)) => {
                ws.selection = None;
                ws.net = Some(n);
                // Select a wire on the net so the inspector shows it.
                let wire = ws.scene.connectivity.nets[n].items.iter().find_map(|it| match it {
                    Item::Wire(w) => Some(Owner::Wire(*w)),
                    Item::Label(l) => Some(Owner::Label(*l)),
                    Item::Port(p) => Some(Owner::Port(*p)),
                    Item::Pin(_) => None,
                });
                ws.selection = wire;
            }
            Some(InspectorAction::SelectInstance(i)) => ws.select(Some(Owner::Instance(i))),
            None => {}
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match self.cursor {
                Some(p) => {
                    let mil = |mm: f64| (mm / 0.0254).round() as i64;
                    ui.label(
                        RichText::new(format!(
                            "X {:>8.2}  Y {:>8.2} mm   ({}, {} mil)",
                            p.x,
                            p.y,
                            mil(p.x),
                            mil(p.y)
                        ))
                        .monospace(),
                    );
                }
                None => {
                    ui.label(RichText::new("X        –  Y        – mm").monospace().weak());
                }
            }
            ui.separator();
            if let (Some(ws), Some(owner)) = (&self.workspace, self.hovered) {
                ui.label(describe(ws, owner));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(ws) = &self.workspace {
                    if let Some(cam) = ws.current().camera {
                        ui.label(
                            RichText::new(format!("{:.1} px/mm", cam.scale)).monospace().weak(),
                        );
                        ui.separator();
                    }
                    let problems = ws.problems();
                    let errors = problems.iter().filter(|(s, _)| *s == Severity::Error).count();
                    let text = match (errors, problems.len()) {
                        (_, 0) => {
                            RichText::new("✔ no problems").color(Color32::from_rgb(90, 190, 120))
                        }
                        (0, n) => RichText::new(format!("⚠ {n} warning(s)"))
                            .color(Color32::from_rgb(230, 170, 60)),
                        (e, n) => RichText::new(format!("✖ {e} error(s), {} warning(s)", n - e))
                            .color(Color32::from_rgb(230, 80, 70)),
                    };
                    ui.label(text);
                }
            });
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.label(RichText::new("Circuiter").size(40.0).strong());
            ui.label(RichText::new("Design, simulate and build hardware").weak());
            ui.add_space(24.0);
            if ui.button(RichText::new("Open Project…").size(16.0)).clicked() {
                self.open_dialog();
            }
            let example = Path::new("examples/cmos_inverter");
            if example.join(circuiter_model::project::MANIFEST_FILE).is_file() {
                ui.add_space(8.0);
                if ui.link("Open the CMOS inverter example").clicked() {
                    self.open(example);
                }
            }
        });
    }
}

enum InspectorAction {
    Descend(usize),
    SelectNet(usize),
    SelectInstance(usize),
}

fn describe(ws: &Workspace, owner: Owner) -> String {
    let conn = &ws.scene.connectivity;
    let net_name = |item| conn.net_of(item).map(|n| conn.nets[n].name.clone()).unwrap_or_default();
    let Some(schematic) = ws.schematic() else {
        return ws.current().cell.to_string();
    };
    match owner {
        Owner::Instance(i) => {
            let inst = &schematic.instances[i];
            let hint = if ws.scene.descendable[i] { "  (double-click to open)" } else { "" };
            format!("{}  {}{hint}", inst.name, inst.cell)
        }
        Owner::Wire(i) => format!("wire on net {}", net_name(Item::Wire(i))),
        Owner::Label(i) => format!("label on net {}", net_name(Item::Label(i))),
        Owner::Port(i) => format!("port {}", schematic.ports[i].port),
        Owner::Derived => String::new(),
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title.to_uppercase()).small().strong().weak());
    ui.add_space(2.0);
}

fn instance_panel(ui: &mut egui::Ui, ws: &Workspace, index: usize) -> Option<InspectorAction> {
    let schematic = ws.schematic()?;
    let inst = &schematic.instances[index];
    let resolved = ws.design.resolve(&inst.cell, &ws.current().cell.library);
    let mut action = None;

    ui.label(RichText::new(&inst.name).size(22.0).strong().monospace());
    let Some((id, def)) = resolved else {
        ui.colored_label(Color32::from_rgb(230, 80, 70), format!("unknown cell {}", inst.cell));
        return None;
    };
    ui.label(RichText::new(id.to_string()).monospace().weak());
    if !def.cell.description.is_empty() {
        ui.label(&def.cell.description);
    }

    section(ui, "Model");
    let view = ws.design.select_view(def, inst.view);
    let model = match view {
        Some(ViewKind::Schematic) => "Schematic (hierarchical)",
        Some(ViewKind::Spice) => "SPICE model",
        None if def.cell.global_net.is_some() => "Global net",
        None => "No simulation view",
    };
    ui.label(model);
    if ws.scene.descendable[index] && ui.button("Open schematic  ⏎").clicked() {
        action = Some(InspectorAction::Descend(index));
    }

    if !def.cell.params.is_empty() {
        section(ui, "Parameters");
        egui::Grid::new("params").num_columns(2).striped(true).spacing([12.0, 4.0]).show(
            ui,
            |ui| {
                for p in &def.cell.params {
                    let overridden = inst.params.contains_key(&p.name);
                    ui.label(RichText::new(&p.name).monospace()).on_hover_text(&p.description);
                    let value = RichText::new(param_display(inst, def, &p.name)).monospace();
                    ui.label(if overridden { value.strong() } else { value.weak() })
                        .on_hover_text(if overridden { "Set on this instance" } else { "Default" });
                    ui.end_row();
                }
            },
        );
    }

    let conn = &ws.scene.connectivity;
    let pins: Vec<(usize, &str)> = conn
        .pins
        .iter()
        .enumerate()
        .filter(|(_, p)| p.instance == index)
        .map(|(i, p)| (i, p.port.as_str()))
        .collect();
    if !pins.is_empty() {
        section(ui, "Connections");
        egui::Grid::new("pins").num_columns(2).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
            for (pin, port) in pins {
                ui.label(RichText::new(port).monospace());
                match conn.net_of(Item::Pin(pin)) {
                    Some(n) if conn.dangling_pins.contains(&pin) => {
                        let _ = n;
                        ui.colored_label(Color32::from_rgb(230, 90, 70), "not connected");
                    }
                    Some(n) => {
                        if ui.link(RichText::new(&conn.nets[n].name).monospace()).clicked() {
                            action = Some(InspectorAction::SelectNet(n));
                        }
                    }
                    None => {
                        ui.label("—");
                    }
                }
                ui.end_row();
            }
        });
    }
    action
}

fn net_panel(ui: &mut egui::Ui, ws: &Workspace, net: usize) -> Option<InspectorAction> {
    let schematic = ws.schematic()?;
    let conn = &ws.scene.connectivity;
    let n = &conn.nets[net];
    let mut action = None;
    ui.label(RichText::new("Net").weak());
    ui.label(RichText::new(&n.name).size(22.0).strong().monospace());
    let source = match n.source {
        NetNameSource::Global => "Global net",
        NetNameSource::Port => "Named by a port of this cell",
        NetNameSource::Label => "Named by a net label",
        NetNameSource::Auto => "Unnamed (automatic name)",
    };
    ui.label(RichText::new(source).weak());
    if n.names.len() > 1 {
        ui.colored_label(
            Color32::from_rgb(230, 170, 60),
            format!("⚠ conflicting names: {}", n.names.join(", ")),
        );
    }
    section(ui, "Connected pins");
    for item in &n.items {
        match *item {
            Item::Pin(p) => {
                let pin = &conn.pins[p];
                let inst = &schematic.instances[pin.instance];
                let text = RichText::new(format!("{}.{}", inst.name, pin.port)).monospace();
                if ui.link(text).on_hover_text(inst.cell.to_string()).clicked() {
                    action = Some(InspectorAction::SelectInstance(pin.instance));
                }
            }
            Item::Port(p) => {
                ui.label(RichText::new(format!("port {}", schematic.ports[p].port)).monospace());
            }
            _ => {}
        }
    }
    action
}

fn cell_panel(ui: &mut egui::Ui, ws: &Workspace) {
    let Some(def) = ws.current_def() else { return };
    let id = &ws.current().cell;
    ui.label(RichText::new(&def.name).size(22.0).strong().monospace());
    ui.label(RichText::new(id.to_string()).monospace().weak());
    if !def.cell.description.is_empty() {
        ui.label(&def.cell.description);
    }

    if !def.cell.ports.is_empty() {
        section(ui, "Ports");
        egui::Grid::new("ports").num_columns(2).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
            for p in &def.cell.ports {
                ui.label(RichText::new(&p.name).monospace());
                ui.label(RichText::new(format!("{:?}", p.kind)).weak());
                ui.end_row();
            }
        });
    }

    if let Some(schematic) = ws.schematic() {
        let conn = &ws.scene.connectivity;
        section(ui, "Contents");
        ui.label(format!(
            "{} instances · {} nets · {} junctions",
            schematic.instances.len(),
            conn.nets.len(),
            conn.junctions.len()
        ));
    } else if !def.cell.params.is_empty() {
        section(ui, "Parameters");
        egui::Grid::new("cell-params").num_columns(2).striped(true).spacing([12.0, 4.0]).show(
            ui,
            |ui| {
                for p in &def.cell.params {
                    ui.label(RichText::new(&p.name).monospace()).on_hover_text(&p.description);
                    ui.label(RichText::new(p.default.as_deref().unwrap_or("required")).monospace());
                    ui.end_row();
                }
            },
        );
    }

    section(ui, "Problems");
    let problems = ws.problems();
    if problems.is_empty() {
        ui.label(RichText::new("✔ None").color(Color32::from_rgb(90, 190, 120)));
    }
    for (severity, message) in problems {
        let (icon, c) = match severity {
            Severity::Error => ("✖", Color32::from_rgb(230, 80, 70)),
            Severity::Warning => ("⚠", Color32::from_rgb(230, 170, 60)),
        };
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(c, icon);
            ui.label(message);
        });
    }
}

impl eframe::App for CircuiterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        self.handle_screenshot(&ctx);
        self.shortcuts(&ctx);

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        if self.workspace.is_some() {
            egui::Panel::left("browser")
                .resizable(true)
                .default_size(250.0)
                .show(ui, |ui| self.browser(ui));
            egui::Panel::right("inspector")
                .resizable(true)
                .default_size(300.0)
                .show(ui, |ui| self.inspector(ui));
        }
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if let Some(err) = &self.error {
                ui.colored_label(Color32::from_rgb(230, 80, 70), err);
            }
            let theme = self.theme;
            let show_grid = self.show_grid;
            match &mut self.workspace {
                None => {
                    self.cursor = None;
                    self.hovered = None;
                    self.welcome(ui);
                }
                Some(ws) => {
                    let highlight = Highlight { selection: ws.selection, net: ws.net };
                    let level = ws.levels.last_mut().expect("level");
                    let out =
                        canvas::show(ui, &ws.scene, &mut level.camera, theme, highlight, show_grid);
                    self.cursor = out.cursor;
                    self.hovered = out.hovered;
                    for event in out.events {
                        match event {
                            CanvasEvent::Select(owner) => ws.select(owner),
                            CanvasEvent::Descend(i) => ws.descend(i),
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Workspace {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cmos_inverter");
        Workspace::open(&path).unwrap()
    }

    fn instance(ws: &Workspace, name: &str) -> usize {
        ws.schematic().unwrap().instances.iter().position(|i| i.name == name).unwrap()
    }

    #[test]
    fn opens_the_top_cell_without_problems() {
        let ws = example();
        assert_eq!(ws.current().cell, CellId::new("cmos_inverter", "testbench"));
        assert_eq!(ws.mode, Mode::Schematic);
        assert!(ws.problems().is_empty(), "{:?}", ws.problems());
    }

    #[test]
    fn descends_into_an_instance_and_back_out() {
        let mut ws = example();
        let u1 = instance(&ws, "U1");
        assert!(ws.scene.descendable[u1]);
        ws.descend(u1);
        assert_eq!(ws.levels.len(), 2);
        assert_eq!(ws.current().cell, CellId::new("cmos_inverter", "inverter"));
        assert_eq!(ws.current().via.as_deref(), Some("U1"));
        assert_eq!(ws.schematic().unwrap().instances.len(), 2);

        ws.ascend_to(1);
        assert_eq!(ws.levels.len(), 1);
        assert_eq!(ws.selection, Some(Owner::Instance(u1)), "the instance we left is selected");
    }

    #[test]
    fn primitives_are_not_descendable_and_open_as_symbols() {
        let mut ws = example();
        let vdd = instance(&ws, "VDD");
        assert!(!ws.scene.descendable[vdd]);
        ws.descend(vdd);
        assert_eq!(ws.levels.len(), 2, "descend still records the level");
        assert_eq!(ws.mode, Mode::Symbol);

        ws.open_cell(CellId::new("std", "nmos"));
        assert_eq!(ws.levels.len(), 1);
        assert_eq!(ws.mode, Mode::Symbol);
    }

    #[test]
    fn selecting_a_wire_highlights_its_net() {
        let mut ws = example();
        let out_wire = ws
            .schematic()
            .unwrap()
            .wires
            .iter()
            .position(|w| w.points.first() == Some(&circuiter_model::Point::mm(40.64, 0.0)))
            .unwrap();
        ws.select(Some(Owner::Wire(out_wire)));
        let net = ws.net.expect("net selected");
        assert_eq!(ws.scene.connectivity.nets[net].name, "OUT");
        ws.select(None);
        assert_eq!(ws.net, None);
    }

    #[test]
    fn reload_restores_the_hierarchy_path() {
        let mut app = CircuiterApp {
            workspace: Some(example()),
            error: None,
            theme: &Theme::BENCH,
            applied_theme: None,
            show_grid: true,
            cursor: None,
            hovered: None,
            screenshot: None,
            frames: 0,
        };
        let ws = app.workspace.as_mut().unwrap();
        let u1 = instance(ws, "U1");
        ws.descend(u1);
        app.reload();
        let ws = app.workspace.as_ref().unwrap();
        assert!(app.error.is_none(), "{:?}", app.error);
        assert_eq!(ws.levels.len(), 2);
        assert_eq!(ws.current().cell.cell, "inverter");
    }
}
