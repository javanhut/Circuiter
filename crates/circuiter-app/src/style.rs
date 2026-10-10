//! The look of the window around the sheet: colours, fonts, spacing, and the few
//! widgets every panel shares (icon buttons, tab strips, collapsible sections,
//! property rows).

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, Response, RichText,
    Sense, Stroke, TextStyle, Vec2,
};
pub use egui_phosphor::regular as icon;

/// The window's colours, for the dark and light themes.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Behind everything: the gaps between panels.
    pub base: Color32,
    /// Panels.
    pub panel: Color32,
    /// Headers, tab strips and toolbars on panels.
    pub raised: Color32,
    /// Text fields and other wells.
    pub well: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    /// Behind selected tabs and items.
    pub accent_soft: Color32,
    pub go: Color32,
    pub go_soft: Color32,
    pub warning: Color32,
    pub error: Color32,
}

pub const DARK: Palette = Palette {
    base: Color32::from_rgb(0x0B, 0x10, 0x18),
    panel: Color32::from_rgb(0x12, 0x19, 0x24),
    raised: Color32::from_rgb(0x17, 0x20, 0x2D),
    well: Color32::from_rgb(0x0C, 0x13, 0x1D),
    border: Color32::from_rgb(0x25, 0x31, 0x43),
    text: Color32::from_rgb(0xD5, 0xDE, 0xEA),
    weak: Color32::from_rgb(0x84, 0x94, 0xA8),
    accent: Color32::from_rgb(0x3B, 0x8B, 0xF6),
    accent_soft: Color32::from_rgb(0x1B, 0x35, 0x5A),
    go: Color32::from_rgb(0x34, 0xD3, 0x7A),
    go_soft: Color32::from_rgb(0x12, 0x2E, 0x22),
    warning: Color32::from_rgb(0xF2, 0xB1, 0x3C),
    error: Color32::from_rgb(0xF2, 0x5F, 0x5C),
};

pub const LIGHT: Palette = Palette {
    base: Color32::from_rgb(0xD9, 0xDE, 0xE5),
    panel: Color32::from_rgb(0xF4, 0xF6, 0xF9),
    raised: Color32::from_rgb(0xE9, 0xED, 0xF2),
    well: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    border: Color32::from_rgb(0xC9, 0xD1, 0xDC),
    text: Color32::from_rgb(0x1C, 0x25, 0x33),
    weak: Color32::from_rgb(0x5E, 0x6B, 0x7D),
    accent: Color32::from_rgb(0x1F, 0x6F, 0xE0),
    accent_soft: Color32::from_rgb(0xD3, 0xE3, 0xFA),
    go: Color32::from_rgb(0x15, 0x8A, 0x48),
    go_soft: Color32::from_rgb(0xDB, 0xF2, 0xE4),
    warning: Color32::from_rgb(0xB8, 0x74, 0x00),
    error: Color32::from_rgb(0xC8, 0x2E, 0x2A),
};

/// The palette in use, from the visuals.
pub fn palette(ui: &egui::Ui) -> &'static Palette {
    palette_for(ui.visuals().dark_mode)
}

pub fn palette_for(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

/// Fonts: the default ones plus the Phosphor icons, so `icon::PLAY` and friends can be
/// written in any label.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
}

/// Sets egui's look from the palette.
pub fn apply(ctx: &egui::Context, dark: bool) {
    let p = palette_for(dark);
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.5, FontFamily::Proportional)),
        (TextStyle::Heading, FontId::new(17.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.5, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = Vec2::new(7.0, 5.0);
    style.spacing.button_padding = Vec2::new(8.0, 3.0);
    style.spacing.interact_size.y = 24.0;
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.window_margin = Margin::same(14);

    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.panel;
    v.window_fill = p.raised;
    v.extreme_bg_color = p.well;
    v.faint_bg_color = if dark {
        Color32::from_rgb(0x15, 0x1E, 0x2A)
    } else {
        Color32::from_rgb(0xEC, 0xF0, 0xF5)
    };
    v.code_bg_color = p.well;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warning;
    v.error_fg_color = p.error;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    let radius = CornerRadius::same(4);
    let widget = |fill: Color32, stroke: Color32, fg: Color32| egui::style::WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: Stroke::new(1.0, stroke),
        corner_radius: radius,
        fg_stroke: Stroke::new(1.0, fg),
        expansion: 0.0,
    };
    let (rest, hover, press) = if dark {
        (
            Color32::from_rgb(0x1A, 0x24, 0x33),
            Color32::from_rgb(0x22, 0x30, 0x45),
            Color32::from_rgb(0x2A, 0x3B, 0x55),
        )
    } else {
        (
            Color32::from_rgb(0xE6, 0xEB, 0xF1),
            Color32::from_rgb(0xDA, 0xE2, 0xEC),
            Color32::from_rgb(0xCD, 0xD8, 0xE6),
        )
    };
    v.widgets.noninteractive = widget(p.panel, p.border, p.text);
    v.widgets.inactive = widget(rest, Color32::TRANSPARENT, p.text);
    v.widgets.hovered = widget(hover, p.border, p.text);
    v.widgets.active = widget(press, p.accent, p.text);
    v.widgets.open = widget(press, p.border, p.text);
    style.visuals = v;
    ctx.set_global_style(style);
}

/// A toolbar button showing an icon, with a tooltip. `selected` marks the active tool.
pub fn icon_button(
    ui: &mut egui::Ui,
    glyph: &str,
    tip: &str,
    enabled: bool,
    selected: bool,
) -> Response {
    let p = palette(ui);
    let size = Vec2::new(30.0, 28.0);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, r) = ui.allocate_exact_size(size, sense);
    // Named by its tooltip, for screen readers (and tests).
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tip));
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            p.accent_soft
        } else if enabled && r.hovered() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, 5.0, fill);
        if selected {
            ui.painter().rect_stroke(
                rect,
                5.0,
                Stroke::new(1.0, p.accent.gamma_multiply(0.7)),
                egui::StrokeKind::Inside,
            );
        }
        let color = match (enabled, selected) {
            (false, _) => p.weak.gamma_multiply(0.45),
            (true, true) if ui.visuals().dark_mode => Color32::WHITE,
            (true, true) => p.accent,
            (true, false) => p.text,
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(17.0),
            color,
        );
    }
    let r = r.on_hover_text(tip);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tip));
    r
}

/// A thin vertical rule between toolbar groups.
pub fn toolbar_gap(ui: &mut egui::Ui) {
    let p = palette(ui);
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(1.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border);
    ui.add_space(4.0);
}

/// A row of tabs, the active one marked by an accent bar along its bottom edge.
/// Returns the tab clicked.
pub fn tab_strip(ui: &mut egui::Ui, labels: &[&str], active: usize) -> Option<usize> {
    let p = palette(ui);
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (i, label) in labels.iter().enumerate() {
            let on = i == active;
            let text = RichText::new(*label).size(13.0).color(if on { p.text } else { p.weak });
            let galley = egui::WidgetText::from(text).into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                TextStyle::Button,
            );
            let size = Vec2::new(galley.size().x + 26.0, 32.0);
            let (rect, r) = ui.allocate_exact_size(size, Sense::click());
            let fill = if on {
                p.accent_soft.gamma_multiply(0.75)
            } else if r.hovered() {
                ui.visuals().widgets.hovered.weak_bg_fill
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 0.0, fill);
            if on {
                let bar = egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom() - 2.0),
                    rect.right_bottom(),
                );
                ui.painter().rect_filled(bar, 0.0, p.accent);
            }
            let at = rect.center() - galley.size() / 2.0;
            ui.painter().galley(at, galley, p.text);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, *label));
            if r.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// A strip across the top of a panel holding tabs or a title, on the raised colour.
pub fn strip<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let p = palette(ui);
    let out = egui::Frame::new()
        .fill(p.raised)
        .inner_margin(Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner;
    let rect = ui.min_rect();
    let y = ui.cursor().top();
    ui.painter().hline(rect.x_range(), y, Stroke::new(1.0, p.border));
    out
}

/// A collapsible section of a panel with a bold title, open unless closed.
pub fn section<R>(
    ui: &mut egui::Ui,
    title: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    let p = palette(ui);
    ui.add_space(4.0);
    let r = egui::CollapsingHeader::new(RichText::new(title).strong().color(p.text).size(13.5))
        .id_salt(("section", title))
        .default_open(true)
        .show(ui, |ui| {
            ui.add_space(2.0);
            add(ui)
        });
    r.body_returned
}

/// The grid that holds a section's rows: names on the left, values on the right.
pub fn properties<R>(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Grid::new(id).num_columns(2).spacing([12.0, 6.0]).min_col_width(110.0).show(ui, add).inner
}

/// A property's name, in the left column.
pub fn name(ui: &mut egui::Ui, text: &str) -> Response {
    let p = palette(ui);
    ui.label(RichText::new(text).color(p.weak))
}

/// A read-only value with its unit, right-aligned in a well like the editable ones.
pub fn value(ui: &mut egui::Ui, text: &str, width: f32) -> Response {
    let p = palette(ui);
    let (rect, r) = ui.allocate_exact_size(Vec2::new(width, 24.0), Sense::hover());
    ui.painter().rect(rect, 4.0, p.well, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    ui.painter().text(
        rect.left_center() + Vec2::new(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::monospace(12.5),
        p.text,
    );
    r
}

/// The Circuiter mark: a ring broken on the right, like a C, around a dot.
pub fn logo(ui: &mut egui::Ui, size: f32) -> Response {
    let p = palette(ui);
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let c = rect.center();
    let radius = size * 0.4;
    let stroke = Stroke::new(size * 0.11, p.accent);
    let arc: Vec<egui::Pos2> = (0..=40)
        .map(|k| {
            let a = 0.75 + (std::f32::consts::TAU - 1.5) * k as f32 / 40.0;
            c + Vec2::angled(a) * radius
        })
        .collect();
    ui.painter().add(egui::Shape::line(arc, stroke));
    let inner: Vec<egui::Pos2> = (0..=30)
        .map(|k| {
            let a = 0.9 + (std::f32::consts::TAU - 1.8) * k as f32 / 30.0;
            c + Vec2::angled(a) * radius * 0.55
        })
        .collect();
    ui.painter().add(egui::Shape::line(inner, Stroke::new(size * 0.08, p.accent)));
    ui.painter().circle_filled(c + Vec2::new(radius * 0.55, 0.0), size * 0.07, p.accent);
    r
}

/// Right-to-left layout for the trailing end of a bar.
pub fn trailing<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.with_layout(Layout::right_to_left(Align::Center), add).inner
}

/// Buttons joined into one control, one of them chosen: view switchers. Disabled
/// entries can't be picked. Returns the one clicked.
pub fn segmented(ui: &mut egui::Ui, labels: &[(&str, bool)], active: usize) -> Option<usize> {
    let p = palette(ui);
    let mut clicked = None;
    let frame = egui::Frame::new()
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(6)
        .inner_margin(Margin::same(2));
    frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (i, &(label, enabled)) in labels.iter().enumerate() {
                let on = i == active;
                let color = match (enabled, on) {
                    (false, _) => p.weak.gamma_multiply(0.5),
                    (true, true) => p.text,
                    (true, false) => p.weak,
                };
                let galley = egui::WidgetText::from(RichText::new(label).color(color)).into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    TextStyle::Button,
                );
                let size = Vec2::new(galley.size().x + 22.0, 24.0);
                let sense = if enabled { Sense::click() } else { Sense::hover() };
                let (rect, r) = ui.allocate_exact_size(size, sense);
                let fill = if on {
                    p.accent_soft
                } else if enabled && r.hovered() {
                    ui.visuals().widgets.hovered.weak_bg_fill
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 4.0, fill);
                ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label)
                });
                if r.clicked() && !on {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}
