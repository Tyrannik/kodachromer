//! Remjet's look: a strictly neutral grey workspace (so nothing around the
//! photo tints how its colour is judged), with the library drawn as a strip
//! of slide film whose edge print is the one place colour is used.

use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2};
use std::sync::Arc;

/// Behind the photo: neutral, a little lighter than the panels, like the
/// grey surround recommended for judging colour on screen.
pub const SURROUND: Color32 = Color32::from_gray(38);
/// Side panels and top bar.
pub const PANEL: Color32 = Color32::from_gray(28);
/// Buttons, combo boxes, value boxes.
pub const CONTROL: Color32 = Color32::from_gray(50);
pub const CONTROL_HOVER: Color32 = Color32::from_gray(62);
/// Hairlines between panels.
pub const LINE: Color32 = Color32::from_gray(18);
pub const TEXT: Color32 = Color32::from_gray(214);
pub const TEXT_WEAK: Color32 = Color32::from_gray(138);
/// Unexposed slide film: the rebate of a reversal film is black.
pub const FILM_BASE: Color32 = Color32::from_gray(6);
/// Kodak's edge print: the frame numbers along the rebate.
pub const EDGE_PRINT: Color32 = Color32::from_rgb(240, 168, 48);
/// Warnings (not from an M9, failed export); kept apart from the edge print.
pub const WARN: Color32 = Color32::from_rgb(232, 110, 96);

/// Condensed face for section titles and the edge print.
pub fn condensed() -> FontFamily {
    FontFamily::Name("condensed".into())
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("barlow".into(), Arc::new(FontData::from_static(include_bytes!("../../../assets/fonts/Barlow-Regular.ttf"))));
    fonts
        .font_data
        .insert("barlow-condensed".into(), Arc::new(FontData::from_static(include_bytes!("../../../assets/fonts/BarlowCondensed-SemiBold.ttf"))));
    // egui's own fonts stay behind Barlow as fallbacks for arrows and symbols.
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "barlow".into());
    let mut cond = vec!["barlow-condensed".to_string()];
    cond.extend(fallbacks);
    fonts.families.insert(condensed(), cond);
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.style_mut_of(egui::Theme::Dark, style);
}

fn style(s: &mut egui::Style) {
    use TextStyle::*;
    s.text_styles = [
        (Small, FontId::proportional(11.5)),
        (Body, FontId::proportional(14.0)),
        (Button, FontId::proportional(14.0)),
        (Heading, FontId::new(19.0, condensed())),
        (Monospace, FontId::monospace(13.0)),
    ]
    .into();

    let sp = &mut s.spacing;
    sp.item_spacing = Vec2::new(8.0, 6.0);
    sp.button_padding = Vec2::new(10.0, 4.0);
    sp.interact_size = Vec2::new(44.0, 24.0);
    sp.slider_rail_height = 3.0;
    sp.window_margin = Margin::same(14);
    sp.combo_width = 180.0;

    let v = &mut s.visuals;
    v.dark_mode = true;
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, Color32::from_gray(60));
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(4);
    v.extreme_bg_color = Color32::from_gray(20);
    v.faint_bg_color = Color32::from_gray(32);
    v.code_bg_color = CONTROL;
    v.hyperlink_color = EDGE_PRINT;
    v.warn_fg_color = WARN;
    v.error_fg_color = WARN;
    // Selected buttons and slider fills stay neutral: colour is kept for the
    // photo and the film strip.
    v.selection.bg_fill = Color32::from_gray(96);
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };
    v.striped = false;

    let r = CornerRadius::same(3);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.weak_bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = CONTROL;
    w.inactive.weak_bg_fill = CONTROL;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = r;

    w.hovered.bg_fill = CONTROL_HOVER;
    w.hovered.weak_bg_fill = CONTROL_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_gray(90));
    w.hovered.fg_stroke = Stroke::new(1.5, Color32::from_gray(240));
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = Color32::from_gray(74);
    w.active.weak_bg_fill = Color32::from_gray(74);
    w.active.bg_stroke = Stroke::new(1.0, Color32::from_gray(120));
    w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = CONTROL_HOVER;
    w.open.weak_bg_fill = CONTROL_HOVER;
    w.open.bg_stroke = Stroke::new(1.0, Color32::from_gray(90));
    w.open.fg_stroke = Stroke::new(1.0, TEXT);
    w.open.corner_radius = r;
}

/// A section title in the edit panel.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(14.0);
    ui.label(egui::RichText::new(title).font(FontId::new(19.0, condensed())).color(TEXT));
    ui.add_space(2.0);
}

/// One line of explanatory text under a control.
pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(TEXT_WEAK));
}

/// A small (i) that explains the setting beside it when hovered.
pub fn info(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let size = 15.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let color = if resp.hovered() { TEXT } else { TEXT_WEAK };
    let p = ui.painter();
    p.circle_stroke(rect.center(), size / 2.0 - 1.0, Stroke::new(1.0, color));
    p.text(rect.center() + Vec2::new(0.0, 0.5), egui::Align2::CENTER_CENTER, "i", FontId::new(11.0, condensed()), color);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Help);
    }
    resp.on_hover_ui(|ui| {
        ui.set_max_width(280.0);
        ui.label(text);
    })
}
