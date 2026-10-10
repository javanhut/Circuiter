//! Programs: assembly, C and Rust (and HDL) files of the project, edited in tabs beside
//! the cells, checked by building them, and run on the processor on the sheet.

use super::*;
use crate::style::{self, icon};
use std::sync::mpsc::{Receiver, channel};

/// File types the editor opens.
pub const PROGRAM_EXTENSIONS: [&str; 7] = ["s", "asm", "c", "h", "rs", "v", "sv"];

/// A problem found by checking a program: its line, if known, and what's wrong.
pub type CodeProblem = (Option<usize>, String);

/// A program open in a tab.
#[derive(Debug)]
pub struct CodeTab {
    /// Relative to the project.
    pub path: PathBuf,
    pub text: String,
    saved: String,
    pub problems: Vec<CodeProblem>,
    /// A check running in the background.
    checking: Option<Receiver<Vec<CodeProblem>>>,
    /// The last check found nothing wrong.
    pub checked_ok: bool,
}

impl CodeTab {
    pub fn dirty(&self) -> bool {
        self.text != self.saved
    }

    pub fn name(&self) -> String {
        self.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }

    fn language(&self) -> &'static str {
        match self.path.extension().and_then(|e| e.to_str()).unwrap_or("") {
            "s" | "asm" => "RISC-V assembly",
            "c" | "h" => "C",
            "rs" => "Rust",
            "v" => "Verilog",
            "sv" => "SystemVerilog",
            _ => "Text",
        }
    }
}

/// The project's program files, under `programs/`, sorted.
pub fn programs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs = vec![root.join("programs")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| PROGRAM_EXTENSIONS.contains(&x))
            {
                if let Ok(rel) = p.strip_prefix(root) {
                    out.push(rel.to_path_buf());
                }
            }
        }
    }
    out.sort();
    out
}

/// Builds a program the way Run would, and reports what's wrong with it.
fn check(path: &Path) -> Vec<CodeProblem> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext == "v" || ext == "sv" {
        let Ok(source) = std::fs::read_to_string(path) else {
            return vec![(None, format!("can't read {}", path.display()))];
        };
        return match circuiter_hdl::parse::modules(&source) {
            Err(e) => vec![(Some(e.line), e.message)],
            Ok(modules) => modules
                .iter()
                .filter_map(|m| {
                    circuiter_hdl::interface(&source, Some(&m.name), &Default::default()).err()
                })
                .map(|e| {
                    let line = e
                        .strip_prefix("line ")
                        .and_then(|r| r.split(':').next())
                        .and_then(|n| n.parse().ok());
                    (line, e)
                })
                .collect(),
        };
    }
    if ext == "s" || ext == "asm" {
        let Ok(source) = std::fs::read_to_string(path) else {
            return vec![(None, format!("can't read {}", path.display()))];
        };
        return match circuiter_riscv::assemble(&source) {
            Ok(_) => Vec::new(),
            Err(errors) => errors.into_iter().map(|e| (Some(e.line), e.message)).collect(),
        };
    }
    match crate::digital::load_program(path) {
        Ok(_) => Vec::new(),
        Err(text) => {
            // Compiler messages name lines as `file:LINE:COL: error: …`.
            let name =
                path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| {
                    let line = l
                        .find(&format!("{name}:"))
                        .and_then(|i| l[i + name.len() + 1..].split(':').next())
                        .and_then(|n| n.trim().parse().ok());
                    (line, l.to_owned())
                })
                .collect()
        }
    }
}

impl CircuiterApp {
    /// Opens a program in a tab (or brings its tab to the front).
    pub(super) fn open_code(&mut self, path: &Path) {
        if let Some(i) = self.code_tabs.iter().position(|t| t.path == path) {
            self.active_code = Some(i);
            return;
        }
        let Some(ws) = &self.workspace else { return };
        let full = ws.project.root.join(path);
        match std::fs::read_to_string(&full) {
            Ok(text) => {
                self.code_tabs.push(CodeTab {
                    path: path.to_path_buf(),
                    saved: text.clone(),
                    text,
                    problems: Vec::new(),
                    checking: None,
                    checked_ok: false,
                });
                self.active_code = Some(self.code_tabs.len() - 1);
            }
            Err(e) => self.error = Some(format!("opening {}: {e}", full.display())),
        }
    }

    /// Writes every changed program to disk.
    pub(super) fn save_code(&mut self) {
        let Some(ws) = &self.workspace else { return };
        let root = ws.project.root.clone();
        for tab in self.code_tabs.iter_mut().filter(|t| t.dirty()) {
            let full = root.join(&tab.path);
            match std::fs::write(&full, &tab.text) {
                Ok(()) => tab.saved = tab.text.clone(),
                Err(e) => self.error = Some(format!("saving {}: {e}", full.display())),
            }
        }
    }

    pub(super) fn code_dirty(&self) -> bool {
        self.code_tabs.iter().any(CodeTab::dirty)
    }

    /// Closes a program's tab, saving it first if it changed.
    pub(super) fn close_code(&mut self, i: usize) {
        if self.code_tabs.get(i).is_some_and(CodeTab::dirty) {
            self.save_code();
        }
        if i < self.code_tabs.len() {
            self.code_tabs.remove(i);
        }
        self.active_code = match self.active_code {
            Some(a) if a == i => None,
            Some(a) if a > i => Some(a - 1),
            other => other,
        };
    }

    /// Makes `programs/<name>` with a starting template, and opens it.
    pub(super) fn new_program(&mut self, name: &str) -> Result<(), String> {
        let ws = self.workspace.as_ref().ok_or("no project")?;
        let name = name.trim();
        let ext = Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("");
        if !PROGRAM_EXTENSIONS.contains(&ext) {
            return Err("end the name with .s (assembly), .c, .rs, .v or .sv".into());
        }
        if name.contains("..") || name.starts_with('/') {
            return Err("a name inside programs/, like blink.s or lib/util.c".into());
        }
        let rel = Path::new("programs").join(name);
        let full = ws.project.root.join(&rel);
        if full.exists() {
            return Err(format!("{} already exists", rel.display()));
        }
        let template = match ext {
            "s" | "asm" => {
                "# RISC-V assembly. ECALL ends the program.\n\n    .text\nstart:\n    li a0, 42\n    ecall\n\n    .data\n"
            }
            "c" | "h" => {
                "/* C for the processor on the sheet; main's return value ends in a0. */\n\nint main(void) {\n    return 42;\n}\n"
            }
            "rs" => {
                "// Rust for the processor on the sheet (no_std); main's return value ends in a0.\n\n#[unsafe(no_mangle)]\npub extern \"C\" fn main() -> u32 {\n    42\n}\n"
            }
            _ => {
                "// Verilog\n\nmodule top(input wire clk, output wire y);\n  assign y = clk;\nendmodule\n"
            }
        };
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&full, template).map_err(|e| e.to_string())?;
        self.open_code(&rel);
        Ok(())
    }

    /// Sets every memory on the open sheet that loads a program to `path`.
    fn use_program(&mut self, path: &Path) -> Result<usize, String> {
        let ws = self.workspace.as_mut().ok_or("no project")?;
        let program = path.to_string_lossy().replace('\\', "/");
        let lib = ws.current().cell.library.clone();
        let targets: Vec<usize> = ws
            .schematic()
            .ok_or("open the processor's schematic first")?
            .instances
            .iter()
            .enumerate()
            .filter(|(_, i)| {
                ws.design.resolve(&i.cell, &lib).is_some_and(|(_, d)| {
                    d.digital.as_ref().is_some_and(|v| v.contents.as_deref() == Some("program"))
                })
            })
            .map(|(k, _)| k)
            .collect();
        if targets.is_empty() {
            return Err(format!("{} has no memory to load a program into", ws.current().cell));
        }
        ws.edit(|_, _, d| {
            for &k in &targets {
                crate::edit::set_param(d, k, "program", Some(program.clone()));
            }
            Ok(())
        })?;
        Ok(targets.len())
    }

    /// The editor for the program in front.
    pub(super) fn code_view(&mut self, ui: &mut egui::Ui) {
        let Some(i) = self.active_code else { return };
        let p = style::palette(ui);
        // Collect a finished check.
        if let Some(tab) = self.code_tabs.get_mut(i) {
            if let Some(rx) = &tab.checking {
                if let Ok(problems) = rx.try_recv() {
                    tab.checked_ok = problems.is_empty();
                    tab.problems = problems;
                    tab.checking = None;
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
        let sheet =
            self.workspace.as_ref().map(|ws| ws.current().cell.cell.clone()).unwrap_or_default();
        let Some(tab) = self.code_tabs.get_mut(i) else { return };
        let mut action = None;
        egui::Frame::new().fill(p.panel).inner_margin(egui::Margin::symmetric(10, 6)).show(
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(tab.path.display().to_string()).monospace().color(p.text),
                    );
                    ui.label(RichText::new(tab.language()).color(p.weak));
                    if tab.dirty() {
                        ui.label(RichText::new("● unsaved").color(p.warning).small());
                    }
                    style::trailing(ui, |ui| {
                        if tab.language().ends_with("Verilog") {
                            let make =
                                egui::Button::new(format!("{}  Make Part", icon::PUZZLE_PIECE));
                            if ui
                                .add(make)
                                .on_hover_text("A part from this module, to place on sheets")
                                .clicked()
                            {
                                action = Some("part");
                            }
                        } else {
                            let run = egui::Button::new(
                                RichText::new(format!("{}  Run on {sheet}", icon::PLAY))
                                    .color(p.go),
                            )
                            .fill(p.go_soft);
                            if ui
                            .add(run)
                            .on_hover_text(
                                "Load this program into the memories on the sheet, save, and run",
                            )
                            .clicked()
                        {
                            action = Some("run");
                        }
                        }
                        let checking = tab.checking.is_some();
                        let label = if checking {
                            format!("{}  Checking…", icon::HOURGLASS)
                        } else {
                            format!("{}  Check", icon::CHECK)
                        };
                        if ui
                            .add_enabled(!checking, egui::Button::new(label))
                            .on_hover_text("Build it and list any errors")
                            .clicked()
                        {
                            action = Some("check");
                        }
                        if ui
                            .add_enabled(
                                tab.dirty(),
                                egui::Button::new(format!("{}  Save", icon::FLOPPY_DISK)),
                            )
                            .on_hover_text("Ctrl+S")
                            .clicked()
                        {
                            action = Some("save");
                        }
                    });
                });
            },
        );
        // Problems, if any, under the header.
        let error_lines: Vec<usize> = tab.problems.iter().filter_map(|(l, _)| *l).collect();
        if !tab.problems.is_empty() || tab.checked_ok {
            egui::Frame::new().fill(p.raised).inner_margin(egui::Margin::symmetric(10, 4)).show(
                ui,
                |ui| {
                    ui.set_width(ui.available_width());
                    if tab.problems.is_empty() {
                        ui.label(
                            RichText::new(format!("{}  Builds cleanly", icon::CHECK_CIRCLE))
                                .color(p.go),
                        );
                    }
                    for (line, message) in tab.problems.iter().take(8) {
                        let at = line.map(|l| format!("line {l}: ")).unwrap_or_default();
                        ui.label(
                            RichText::new(format!("{}  {at}{message}", icon::X_CIRCLE))
                                .color(p.error),
                        );
                    }
                },
            );
        }
        let language = tab.language();
        let dark = ui.visuals().dark_mode;
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
            let mut job = highlight(text.as_str(), language, dark, &error_lines);
            job.wrap.max_width = wrap;
            ui.fonts_mut(|f| f.layout_job(job))
        };
        let well = egui::Frame::new().fill(p.well).inner_margin(egui::Margin::symmetric(4, 6));
        well.show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            egui::ScrollArea::both().id_salt(("code", i)).auto_shrink([false; 2]).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let lines =
                        tab.text.lines().count().max(1) + usize::from(tab.text.ends_with('\n'));
                    let numbers: String = (1..=lines).map(|n| format!("{n:>4}\n")).collect();
                    ui.add(egui::Label::new(
                        RichText::new(numbers).monospace().color(p.weak.gamma_multiply(0.6)),
                    ));
                    ui.add(
                        egui::TextEdit::multiline(&mut tab.text)
                            .code_editor()
                            .frame(egui::Frame::NONE)
                            .background_color(p.well)
                            .desired_width(f32::INFINITY)
                            .desired_rows(40)
                            .lock_focus(true)
                            .layouter(&mut layouter),
                    );
                });
            });
        });
        match action {
            Some("save") => self.save_code(),
            Some("check") => {
                self.save_code();
                self.start_check(i);
            }
            Some("part") => {
                self.save_code();
                let path = self.code_tabs[i].path.clone();
                if let Some(ws) = &mut self.workspace {
                    let made = crate::digital::cell_from_module(&ws.project.root, &path, None)
                        .and_then(|def| ws.add_hdl_part(def));
                    if let Err(e) = made {
                        self.error = Some(e);
                    }
                }
            }
            Some("run") => {
                let path = self.code_tabs[i].path.clone();
                self.save_code();
                match self.use_program(&path) {
                    Ok(_) => {
                        let analysis = self.workspace.as_ref().map(|ws| ws.run_analysis());
                        if let Some(a) = analysis {
                            self.show_bottom = true;
                            self.simulate(a);
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            _ => {}
        }
    }

    fn start_check(&mut self, i: usize) {
        let Some(ws) = &self.workspace else { return };
        let Some(tab) = self.code_tabs.get_mut(i) else { return };
        let full = ws.project.root.join(&tab.path);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(check(&full));
        });
        tab.checking = Some(rx);
        tab.checked_ok = false;
    }
}

/// Colours for source text: comments, strings, numbers, keywords, registers, labels;
/// lines with errors get a red background.
fn highlight(
    text: &str,
    language: &str,
    dark: bool,
    error_lines: &[usize],
) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let c = |light: (u8, u8, u8), dark_: (u8, u8, u8)| {
        let (r, g, b) = if dark { dark_ } else { light };
        Color32::from_rgb(r, g, b)
    };
    let plain = c((0x1c, 0x25, 0x33), (0xd5, 0xde, 0xea));
    let comment = c((0x6a, 0x77, 0x88), (0x6e, 0x80, 0x96));
    let string = c((0x1a, 0x7f, 0x37), (0x8f, 0xd1, 0x8a));
    let number = c((0xa3, 0x5b, 0x00), (0xf2, 0xb1, 0x3c));
    let keyword = c((0x7a, 0x3e, 0x9d), (0xc6, 0x9b, 0xf0));
    let register = c((0x0b, 0x6b, 0xb0), (0x6c, 0xb6, 0xff));
    let label = c((0xa3, 0x1f, 0x34), (0xff, 0x8a, 0x80));
    let error_bg = Color32::from_rgba_unmultiplied(240, 80, 80, if dark { 40 } else { 50 });
    let font = egui::FontId::monospace(13.0);
    let asm = language == "RISC-V assembly";
    let keywords: &[&str] = match language {
        "C" => &[
            "int", "unsigned", "char", "short", "long", "void", "static", "const", "return", "if",
            "else", "for", "while", "do", "break", "continue", "struct", "typedef", "volatile",
            "switch", "case", "default", "sizeof", "#include", "#define",
        ],
        "Rust" => &[
            "fn", "let", "mut", "pub", "extern", "unsafe", "return", "if", "else", "for", "while",
            "loop", "in", "match", "struct", "impl", "const", "static", "use", "mod", "as", "u8",
            "u16", "u32", "i32", "usize", "bool", "true", "false",
        ],
        "Verilog" | "SystemVerilog" => &[
            "module",
            "endmodule",
            "input",
            "output",
            "inout",
            "wire",
            "reg",
            "logic",
            "assign",
            "always",
            "always_ff",
            "always_comb",
            "posedge",
            "negedge",
            "begin",
            "end",
            "if",
            "else",
            "case",
            "endcase",
            "default",
            "parameter",
            "localparam",
        ],
        _ => &[],
    };
    let mut job = LayoutJob::default();
    let mut in_block = false;
    for (n, line) in text.split_inclusive('\n').enumerate() {
        let bg = if error_lines.contains(&(n + 1)) { error_bg } else { Color32::TRANSPARENT };
        let mut push = |s: &str, color: Color32| {
            if !s.is_empty() {
                job.append(
                    s,
                    0.0,
                    TextFormat {
                        font_id: font.clone(),
                        color,
                        background: bg,
                        ..Default::default()
                    },
                );
            }
        };
        // Inside a /* block comment */ (C, Rust, Verilog): comment up to its end.
        let mut line = line;
        if in_block {
            match line.find("*/") {
                Some(k) => {
                    push(&line[..k + 2], comment);
                    line = &line[k + 2..];
                    in_block = false;
                }
                None => {
                    push(line, comment);
                    continue;
                }
            }
        }
        if !asm {
            if let Some(start) = line.find("/*").filter(|&k| line.find("//").is_none_or(|c| c > k))
            {
                let (before, after) = line.split_at(start);
                let end = after.find("*/").map(|k| k + 2);
                // The code before the comment, then the comment.
                highlight_code(
                    before,
                    false,
                    keywords,
                    &mut push,
                    (plain, string, number, keyword, register, label),
                );
                match end {
                    Some(e) => {
                        push(&after[..e], comment);
                        highlight_code(
                            &after[e..],
                            false,
                            keywords,
                            &mut push,
                            (plain, string, number, keyword, register, label),
                        );
                    }
                    None => {
                        push(after, comment);
                        in_block = true;
                    }
                }
                continue;
            }
        }
        let comment_at =
            if asm { line.find(['#', ';']).or_else(|| line.find("//")) } else { line.find("//") };
        let (code, rest) = match comment_at {
            Some(k) => line.split_at(k),
            None => (line, ""),
        };
        highlight_code(
            code,
            asm,
            keywords,
            &mut push,
            (plain, string, number, keyword, register, label),
        );
        push(rest, comment);
    }
    job
}

/// Colours one stretch of code (no comments): strings, numbers, keywords, registers
/// and labels. Colours are (plain, string, number, keyword, register, label).
fn highlight_code(
    code: &str,
    asm: bool,
    keywords: &[&str],
    push: &mut dyn FnMut(&str, Color32),
    colors: (Color32, Color32, Color32, Color32, Color32, Color32),
) {
    let (plain, string, number, keyword, register, label) = colors;
    let mut first_word = true;
    let mut chars = code.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch == '"' || ch == '\'' {
            let mut end = code.len();
            while let Some((k, c2)) = chars.next() {
                if c2 == '\\' {
                    chars.next();
                } else if c2 == ch {
                    end = k + 1;
                    break;
                }
            }
            push(&code[start..end], string);
        } else if ch.is_alphanumeric() || ch == '_' || ch == '.' || ch == '#' || ch == '%' {
            let mut end = start + ch.len_utf8();
            while let Some(&(k, c2)) = chars.peek() {
                if c2.is_alphanumeric() || c2 == '_' || c2 == '.' || c2 == '$' {
                    end = k + c2.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let word = &code[start..end];
            let next = code[end..].chars().next();
            let color = if word.starts_with(|c: char| c.is_ascii_digit()) || word.starts_with('-') {
                number
            } else if asm && next == Some(':') {
                label
            } else if asm && circuiter_riscv::asm::register(word).is_some() {
                register
            } else if (asm && (first_word || word.starts_with('.'))) || keywords.contains(&word) {
                keyword
            } else {
                plain
            };
            first_word = false;
            push(word, color);
        } else {
            push(&code[start..start + ch.len_utf8()], plain);
        }
    }
}
