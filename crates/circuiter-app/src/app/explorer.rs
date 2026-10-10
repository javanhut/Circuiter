//! The left side of the window: the project and libraries as trees of folders and
//! cells, and the components to build circuits from.

use super::*;
use crate::style::{self, icon};

/// A folder's title in the trees: its icon and name.
pub fn folder_label(name: &str) -> String {
    format!("{}  {name}", icon::FOLDER_SIMPLE)
}

impl CircuiterApp {
    pub(super) fn explorer(&mut self, ui: &mut egui::Ui) {
        let p = style::palette(ui);
        egui::Panel::bottom("components")
            .resizable(true)
            .default_size(380.0)
            .min_size(120.0)
            .frame(egui::Frame::new().fill(p.panel))
            .show(ui, |ui| {
                ui.expand_to_include_rect(ui.max_rect());
                self.components(ui)
            });
        let tab = style::strip(ui, |ui| {
            ui.horizontal(|ui| {
                let picked = style::tab_strip(ui, &["Project", "Libraries"], self.left_tab);
                if self.left_tab == 0 {
                    style::trailing(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let Some(ws) = &self.workspace else { return };
                        let lib = ws.project.name().to_owned();
                        if style::icon_button(ui, icon::FOLDER_PLUS, "New folder", true, false)
                            .clicked()
                        {
                            self.prompt = Some(Prompt::Folder {
                                lib,
                                parent: String::new(),
                                renaming: None,
                                name: String::new(),
                                error: None,
                            });
                        }
                        if style::icon_button(
                            ui,
                            icon::CUBE,
                            "New 3D part: a body to model, as in a CAD program",
                            true,
                            false,
                        )
                        .clicked()
                        {
                            self.prompt = Some(new_cell_prompt(ws, true));
                        }
                        if style::icon_button(
                            ui,
                            icon::FILE_PLUS,
                            "New cell  (Ctrl+N)",
                            true,
                            false,
                        )
                        .clicked()
                        {
                            self.prompt = Some(new_cell_prompt(ws, false));
                        }
                    });
                }
                picked
            })
            .inner
        });
        if let Some(t) = tab {
            self.left_tab = t;
        }
        let Some(ws) = &mut self.workspace else { return };
        let project = ws.project.name().to_owned();
        let mut order: Vec<String> = Vec::new();
        if self.left_tab == 0 {
            order.push(project.clone());
        } else {
            order.extend(ws.project.manifest.libraries.keys().cloned());
            order.push(circuiter_model::STD_LIBRARY.to_owned());
        }
        let current = ws.current().cell.clone();
        let mut action = None;
        let programs =
            if self.left_tab == 0 { super::code::programs(&ws.project.root) } else { Vec::new() };
        let showing = self.active_code.and_then(|i| self.code_tabs.get(i)).map(|t| t.path.clone());
        let (mut open_program, mut new_program) = (None, false);
        egui::ScrollArea::vertical().id_salt("explorer").auto_shrink([false; 2]).show(ui, |ui| {
            ui.add_space(4.0);
            for lib_name in &order {
                let Some(lib) = ws.design.library(lib_name) else { continue };
                let tree = Folder::of(lib);
                let mut browse = Browse {
                    ws: &*ws,
                    lib: lib_name,
                    editable: ws.library_editable(lib_name),
                    project: *lib_name == project,
                    current: &current,
                    action: None,
                };
                let glyph = if *lib_name == project { icon::CUBE } else { icon::BOOK_OPEN };
                let title = RichText::new(format!("{glyph}  {lib_name}")).strong();
                let r = egui::CollapsingHeader::new(title)
                    .id_salt(lib_name)
                    .default_open(true)
                    .show(ui, |ui| browse.folder(ui, &tree, ""));
                browse.folder_header(&r.header_response, "");
                if browse.action.is_some() {
                    action = browse.action;
                }
            }
            if self.left_tab == 0 {
                let (open, new) = programs_tree(ui, &programs, showing.as_deref());
                open_program = open;
                new_program = new;
            }
        });
        if let Some(path) = open_program {
            self.open_code(&path);
        }
        if new_program {
            self.prompt = Some(Prompt::NewProgram { name: String::new(), error: None });
        }
        let Some(ws) = &mut self.workspace else { return };
        match action {
            Some(BrowserAction::Open(id)) => ws.open_cell(id),
            Some(BrowserAction::Place(id)) => ws.start_placing(&id),
            Some(BrowserAction::Move(id, folder)) => {
                if let Err(e) = ws.move_cell(&id, &folder) {
                    self.error = Some(e);
                }
            }
            Some(BrowserAction::DeleteFolder { lib, folder }) => {
                if let Err(e) = ws.delete_folder(&lib, &folder) {
                    self.error = Some(e);
                }
            }
            Some(BrowserAction::Prompt(p)) => self.prompt = Some(p),
            None => {}
        }
    }

    /// Parts to build with, by kind: click one to place it, or drag it onto the sheet.
    fn components(&mut self, ui: &mut egui::Ui) {
        let p = style::palette(ui);
        style::strip(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_height(32.0);
                ui.add_space(8.0);
                ui.label(RichText::new("Components").strong().color(p.text));
            });
        });
        let Some(ws) = &self.workspace else { return };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.component_query)
                    .hint_text(format!("{}  Search components…", icon::MAGNIFYING_GLASS))
                    .desired_width(ui.available_width() - 6.0),
            );
        });
        ui.add_space(4.0);
        // The parts, by folder: the standard library's, then this project's own.
        let q = self.component_query.trim().to_lowercase();
        let mut groups: Vec<(String, Vec<CellId>)> = Vec::new();
        // This project's parts first: they're the ones being built.
        let mut libs: Vec<String> = vec![ws.project.name().to_owned()];
        libs.extend(ws.project.manifest.libraries.keys().cloned());
        libs.push(circuiter_model::STD_LIBRARY.to_owned());
        for lib_name in &libs {
            let Some(lib) = ws.design.library(lib_name) else { continue };
            for def in lib.cells.values() {
                if def.symbol.is_none() {
                    continue;
                }
                let hay =
                    format!("{} {} {}", def.name, def.folder, def.cell.description).to_lowercase();
                if !q.is_empty() && !hay.contains(&q) {
                    continue;
                }
                let group = if *lib_name == ws.project.name() {
                    "This project".to_owned()
                } else {
                    let top = def.folder.split('/').next().unwrap_or_default();
                    let mut name = if top.is_empty() { lib_name.clone() } else { top.to_owned() };
                    if let Some(first) = name.get_mut(..1) {
                        first.make_ascii_uppercase();
                    }
                    name
                };
                let id = CellId::new(lib_name, &def.name);
                match groups.iter_mut().find(|(g, _)| *g == group) {
                    Some((_, ids)) => ids.push(id),
                    None => groups.push((group, vec![id])),
                }
            }
        }
        let wanted: Vec<CellId> = groups.iter().flat_map(|(_, ids)| ids.iter().cloned()).collect();
        self.cache_icons(&wanted);
        let Some(ws) = &mut self.workspace else { return };
        let searching = !q.is_empty();
        let mut place = None;
        egui::ScrollArea::vertical().id_salt("components").auto_shrink([false; 2]).show(ui, |ui| {
            for (group, ids) in &groups {
                let header = RichText::new(folder_label(group)).color(p.text);
                egui::CollapsingHeader::new(header)
                    .id_salt(("components", group))
                    .default_open(searching || group == "This project")
                    .open(searching.then_some(true))
                    .show(ui, |ui| {
                        for id in ids {
                            let def = ws.design.cell(id).expect("listed");
                            let placeable = ws.placeable(id);
                            let (rect, r) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 30.0),
                                egui::Sense::click_and_drag(),
                            );
                            if r.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    4.0,
                                    ui.visuals().widgets.hovered.weak_bg_fill,
                                );
                            }
                            let glyph = egui::Rect::from_min_size(
                                rect.min + egui::vec2(4.0, 4.0),
                                egui::vec2(34.0, 22.0),
                            );
                            if let Some(scene) = self.icons.get(id) {
                                canvas::draw_icon(
                                    ui.painter(),
                                    glyph,
                                    scene,
                                    p.text.gamma_multiply(0.85),
                                );
                            }
                            ui.painter().text(
                                egui::pos2(glyph.right() + 10.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                &def.name,
                                egui::FontId::proportional(13.5),
                                p.text,
                            );
                            let tip = if placeable {
                                format!(
                                    "{}\n\nClick, then click the sheet · or drag it on",
                                    def.cell.description
                                )
                            } else {
                                format!(
                                    "{}\n\nOpen a schematic to place parts on it",
                                    def.cell.description
                                )
                            };
                            let r = r.on_hover_text(tip);
                            let label = format!("Add {}", id.cell);
                            r.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    placeable,
                                    &label,
                                )
                            });
                            if placeable {
                                r.dnd_set_drag_payload(DraggedCell(id.clone()));
                                if r.clicked() {
                                    place = Some(id.clone());
                                }
                            }
                        }
                    });
            }
            if groups.is_empty() {
                ui.label(RichText::new("No matching parts").color(p.weak));
            }
        });
        if let Some(id) = place {
            ws.start_placing(&id);
        }
    }

    /// Makes sure each cell's symbol is ready to draw as an icon.
    pub(super) fn cache_icons(&mut self, ids: &[CellId]) {
        let Some(ws) = &self.workspace else { return };
        for id in ids {
            if !self.icons.contains_key(id) {
                if let Some(scene) = circuiter_scene::Scene::symbol_sheet(&ws.design, id) {
                    self.icons.insert(id.clone(), scene);
                }
            }
        }
    }
}

/// A library's cells arranged by folder, for the browser.
#[derive(Default)]
struct Folder<'a> {
    folders: std::collections::BTreeMap<&'a str, Folder<'a>>,
    cells: Vec<&'a circuiter_model::CellDef>,
}

impl<'a> Folder<'a> {
    fn of(lib: &'a circuiter_model::Library) -> Self {
        let mut root = Folder::default();
        for folder in &lib.folders {
            root.walk(folder);
        }
        for def in lib.cells.values() {
            root.walk(&def.folder).cells.push(def);
        }
        root
    }

    /// The folder at `path`, made if missing.
    fn walk(&mut self, path: &'a str) -> &mut Folder<'a> {
        let mut at = self;
        for part in path.split('/').filter(|p| !p.is_empty()) {
            at = at.folders.entry(part).or_default();
        }
        at
    }

    fn count(&self) -> usize {
        self.cells.len() + self.folders.values().map(Folder::count).sum::<usize>()
    }
}

/// A cell being dragged out of the library panel: onto a folder to move it, or onto
/// the sheet to place it.
#[derive(Clone)]
pub struct DraggedCell(pub CellId);

/// What was done in the library panel.
enum BrowserAction {
    Open(CellId),
    Place(CellId),
    Move(CellId, String),
    DeleteFolder { lib: String, folder: String },
    Prompt(Prompt),
}

/// Drawing one library in the browser, and what was clicked.
struct Browse<'w> {
    ws: &'w Workspace,
    lib: &'w str,
    /// Cells and folders can be changed (not the built-in library).
    editable: bool,
    /// The project's own library, where new cells go.
    project: bool,
    current: &'w CellId,
    action: Option<BrowserAction>,
}

impl Browse<'_> {
    /// Sub-folders first, collapsed until opened (or holding the open cell), then cells.
    fn folder(&mut self, ui: &mut egui::Ui, folder: &Folder<'_>, path: &str) {
        for (name, sub) in &folder.folders {
            let sub_path =
                if path.is_empty() { name.to_string() } else { format!("{path}/{name}") };
            let holds_current = self.current.library == self.lib
                && self
                    .ws
                    .design
                    .cell(self.current)
                    .is_some_and(|d| circuiter_model::library::in_folder(&d.folder, &sub_path));
            let title = RichText::new(folder_label(name));
            let r = egui::CollapsingHeader::new(title)
                .id_salt((self.lib, &sub_path))
                .default_open(holds_current)
                .show(ui, |ui| {
                    self.folder(ui, sub, &sub_path);
                    if sub.count() == 0 {
                        ui.label(RichText::new("empty").weak().italics().small());
                    }
                });
            let header = r.header_response.on_hover_text(match sub.count() {
                1 => "1 cell".to_owned(),
                n => format!("{n} cells"),
            });
            self.folder_header(&header, &sub_path);
        }
        for def in &folder.cells {
            self.cell(ui, def);
        }
    }

    /// A folder's (or, for `path` "", the library's) header: a drop target for cells
    /// and a menu of folder actions.
    fn folder_header(&mut self, header: &egui::Response, path: &str) {
        if !self.editable {
            return;
        }
        let ours = |d: &DraggedCell| d.0.library == self.lib;
        if header.dnd_hover_payload::<DraggedCell>().is_some_and(|d| ours(&d)) {
            header.ctx.debug_painter().rect_stroke(
                header.rect.expand(1.0),
                3.0,
                egui::Stroke::new(
                    1.5,
                    style::palette_for(header.ctx.global_style().visuals.dark_mode).accent,
                ),
                egui::StrokeKind::Outside,
            );
        }
        if let Some(d) = header.dnd_release_payload::<DraggedCell>().filter(|d| ours(d)) {
            self.action = Some(BrowserAction::Move(d.0.clone(), path.to_owned()));
        }
        let lib = self.lib.to_owned();
        header.context_menu(|ui| {
            if ui.button("New Folder…").clicked() {
                self.action = Some(BrowserAction::Prompt(Prompt::Folder {
                    lib: lib.clone(),
                    parent: path.to_owned(),
                    renaming: None,
                    name: String::new(),
                    error: None,
                }));
            }
            if self.project && ui.button("New Cell Here…").clicked() {
                self.action = Some(BrowserAction::Prompt(Prompt::NewCell {
                    name: String::new(),
                    folder: path.to_owned(),
                    error: None,
                    part3d: false,
                }));
            }
            if self.project && ui.button("New 3D Part Here…").clicked() {
                self.action = Some(BrowserAction::Prompt(Prompt::NewCell {
                    name: String::new(),
                    folder: path.to_owned(),
                    error: None,
                    part3d: true,
                }));
            }
            if !path.is_empty() {
                ui.separator();
                let (parent, leaf) = path.rsplit_once('/').unwrap_or(("", path));
                if ui.button("Rename…").clicked() {
                    self.action = Some(BrowserAction::Prompt(Prompt::Folder {
                        lib: lib.clone(),
                        parent: parent.to_owned(),
                        renaming: Some(path.to_owned()),
                        name: leaf.to_owned(),
                        error: None,
                    }));
                }
                let empty = self.ws.design.library(&lib).is_some_and(|l| !l.folder_has_cells(path));
                let delete = ui.add_enabled(empty, egui::Button::new("Delete Folder"));
                if delete.on_disabled_hover_text("Move its cells out first").clicked() {
                    self.action =
                        Some(BrowserAction::DeleteFolder { lib, folder: path.to_owned() });
                }
            }
        });
    }

    fn cell(&mut self, ui: &mut egui::Ui, def: &circuiter_model::CellDef) {
        let id = CellId::new(self.lib, &def.name);
        let placeable = self.ws.placeable(&id);
        let p = style::palette(ui);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let glyph = if def.schematic.is_some() { icon::CIRCUITRY } else { icon::CPU };
            ui.label(RichText::new(glyph).color(if def.schematic.is_some() {
                p.accent
            } else {
                p.weak
            }));
            let r = ui.selectable_label(id == *self.current, &def.name);
            // Drag it onto a folder to move it, or onto the sheet to place it.
            let r = r.interact(egui::Sense::drag());
            if self.editable || placeable {
                r.dnd_set_drag_payload(DraggedCell(id.clone()));
            }
            let kind = if def.schematic.is_some() { "Schematic" } else { "Part" };
            let hint = match (self.editable, placeable) {
                (true, true) => {
                    "Click to open · drag onto the sheet to place, or onto a folder to move"
                }
                (true, false) => "Click to open · drag onto a folder to move",
                (false, true) => "Click to open · drag onto the sheet to place",
                (false, false) => "Click to open",
            };
            let r = if def.cell.description.is_empty() {
                r.on_hover_text(format!("{kind}\n\n{hint}"))
            } else {
                r.on_hover_text(format!("{}\n\n{hint}", def.cell.description))
            };
            if r.clicked() {
                self.action = Some(BrowserAction::Open(id.clone()));
            }
            r.context_menu(|ui| {
                if ui.button("Open").clicked() {
                    self.action = Some(BrowserAction::Open(id.clone()));
                }
                if placeable && ui.button("Place on This Sheet").clicked() {
                    self.action = Some(BrowserAction::Place(id.clone()));
                }
                if self.editable && ui.button("Move to Folder…").clicked() {
                    self.action = Some(BrowserAction::Prompt(Prompt::MoveCell {
                        id: id.clone(),
                        folder: def.folder.clone(),
                        error: None,
                    }));
                }
            });
            if placeable {
                style::trailing(ui, |ui| {
                    let plus =
                        egui::Button::new(RichText::new(icon::PLUS).color(p.weak)).frame(false);
                    if ui.add(plus).on_hover_text("Place on this sheet").clicked() {
                        self.action = Some(BrowserAction::Place(id.clone()));
                    }
                });
            }
        });
    }
}

/// The project's programs, under the cells: (the one clicked, New Program asked for).
fn programs_tree(
    ui: &mut egui::Ui,
    files: &[PathBuf],
    showing: Option<&Path>,
) -> (Option<PathBuf>, bool) {
    let p = style::palette(ui);
    let mut open = None;
    let mut new = false;
    ui.add_space(4.0);
    let r = egui::CollapsingHeader::new(
        RichText::new(format!("{}  programs", icon::FILE_TEXT)).strong(),
    )
    .id_salt("programs")
    .default_open(true)
    .show(ui, |ui| {
        for f in files {
            let shown = f.strip_prefix("programs").unwrap_or(f).display().to_string();
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::FILE_TEXT).color(p.weak));
                if ui.selectable_label(showing == Some(f.as_path()), shown).clicked() {
                    open = Some(f.clone());
                }
            });
        }
        if files.is_empty() {
            ui.label(RichText::new("No programs yet").weak().italics().small());
        }
        if ui.small_button(format!("{}  New Program…", icon::PLUS)).clicked() {
            new = true;
        }
    });
    r.header_response.context_menu(|ui| {
        if ui.button("New Program…").clicked() {
            new = true;
            ui.close();
        }
    });
    (open, new)
}
