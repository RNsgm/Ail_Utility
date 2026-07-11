//! Общая тема и переиспользуемые UI-компоненты.

use egui::{
    Color32, Context, CornerRadius, Frame, Margin, RichText, Shadow, Stroke, Ui, Vec2,
};

pub const NAV_WIDTH: f32 = 200.0;
pub const CARD_RADIUS: f32 = 10.0;
pub const THUMB_RADIUS: f32 = 10.0;
pub const CONTENT_PAD: f32 = 28.0;
pub const SECTION_GAP: f32 = 16.0;
pub const CARD_INNER_X: i8 = 22;
pub const CARD_INNER_Y: i8 = 18;

pub fn bg() -> Color32 {
    Color32::from_rgb(16, 18, 24)
}

pub fn panel() -> Color32 {
    Color32::from_rgb(22, 25, 33)
}

pub fn card() -> Color32 {
    Color32::from_rgb(30, 34, 44)
}

pub fn card_hover() -> Color32 {
    Color32::from_rgb(40, 45, 58)
}

pub fn accent() -> Color32 {
    Color32::from_rgb(72, 175, 220)
}

pub fn accent_soft() -> Color32 {
    Color32::from_rgba_unmultiplied(72, 175, 220, 45)
}

pub fn accent_bg() -> Color32 {
    Color32::from_rgb(45, 95, 130)
}

pub fn text() -> Color32 {
    Color32::from_rgb(232, 234, 240)
}

pub fn text_muted() -> Color32 {
    Color32::from_rgb(130, 136, 152)
}

pub fn text_hint() -> Color32 {
    Color32::from_rgb(210, 175, 90)
}

pub fn border() -> Color32 {
    Color32::from_rgb(48, 54, 68)
}

pub fn success() -> Color32 {
    Color32::from_rgb(95, 200, 130)
}

pub fn error() -> Color32 {
    Color32::from_rgb(235, 110, 110)
}

pub fn log_ok() -> Color32 {
    Color32::from_rgb(120, 210, 150)
}

pub fn log_err() -> Color32 {
    Color32::from_rgb(240, 120, 120)
}

pub fn apply(ctx: &Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(12.0, 12.0);
    style.spacing.button_padding = Vec2::new(14.0, 9.0);
    style.spacing.indent = 20.0;
    style.spacing.slider_width = 160.0;
    style.spacing.window_margin = Margin::same(16);
    style.spacing.menu_margin = Margin::same(10);

    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(text());
    visuals.panel_fill = panel();
    visuals.window_fill = bg();
    visuals.extreme_bg_color = Color32::from_rgb(12, 14, 18);
    visuals.faint_bg_color = card();
    visuals.code_bg_color = Color32::from_rgb(26, 29, 38);
    visuals.window_shadow = Shadow {
        offset: [0, 6],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(80),
    };
    visuals.popup_shadow = visuals.window_shadow;
    visuals.selection.bg_fill = accent_soft();
    visuals.selection.stroke = Stroke::new(1.0, accent());
    visuals.hyperlink_color = accent();
    visuals.warn_fg_color = text_hint();
    visuals.error_fg_color = error();

    let w = &mut visuals.widgets;
    w.noninteractive.bg_fill = card();
    w.noninteractive.fg_stroke = Stroke::new(1.0, text_muted());
    w.inactive.bg_fill = Color32::from_rgb(36, 40, 52);
    w.inactive.fg_stroke = Stroke::new(1.0, text());
    w.hovered.bg_fill = card_hover();
    w.hovered.fg_stroke = Stroke::new(1.2, text());
    w.active.bg_fill = accent_bg();
    w.active.fg_stroke = Stroke::new(1.2, text());
    w.open.bg_fill = card_hover();

    style.visuals = visuals;
    ctx.set_style(style);
}

pub fn card_frame() -> Frame {
    Frame::new()
        .fill(card())
        .corner_radius(CornerRadius::same(CARD_RADIUS as u8))
        .inner_margin(Margin::symmetric(CARD_INNER_X, CARD_INNER_Y))
        .stroke(Stroke::new(1.0, border()))
}

pub fn section_card<R>(ui: &mut Ui, title: &str, add_body: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(SECTION_GAP);
    card_frame().show(ui, |ui| {
        ui.label(RichText::new(title).strong().size(15.0).color(text()));
        ui.add_space(12.0);
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing = Vec2::new(spacing.x, 12.0);
        let result = add_body(ui);
        ui.spacing_mut().item_spacing = spacing;
        result
    })
    .inner
}

pub fn inset_content<R>(ui: &mut Ui, add_body: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.add_space(CONTENT_PAD);
        let result = ui.vertical(add_body).inner;
        ui.add_space(CONTENT_PAD);
        result
    })
    .inner
}

pub fn row_gap(ui: &mut Ui, gap: f32) {
    ui.add_space(gap);
}

pub fn page_header(ui: &mut Ui, title: &str, subtitle: Option<&str>) {
    ui.label(RichText::new(title).size(22.0).strong().color(text()));
    if let Some(sub) = subtitle {
        ui.add_space(6.0);
        ui.label(RichText::new(sub).color(text_muted()));
    }
    ui.add_space(18.0);
}

pub fn toolbar_frame() -> Frame {
    Frame::new()
        .fill(panel())
        .inner_margin(Margin::symmetric(12, 8))
        .stroke(Stroke::new(1.0, border()))
}

pub fn media_bar_frame() -> Frame {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(10, 12, 16, 220))
        .inner_margin(Margin::symmetric(14, 10))
        .corner_radius(CornerRadius::same(8))
}

pub fn nav_button(ui: &mut Ui, active: bool, icon: &str, label: &str) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 42.0), egui::Sense::click());

    let bg = if active {
        accent_soft()
    } else if response.hovered() {
        card_hover()
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 8.0, bg);
    }
    if active {
        let bar = egui::Rect::from_min_size(rect.left_top(), Vec2::new(3.0, rect.height()));
        ui.painter().rect_filled(bar, 2.0, accent());
    }

    let text_color = if active { text() } else { text_muted() };
    let pos = rect.left_center() + Vec2::new(14.0, 0.0);
    ui.painter().text(
        pos,
        egui::Align2::LEFT_CENTER,
        format!("{icon}  {label}"),
        egui::FontId::proportional(14.0),
        text_color,
    );

    response.clicked()
}

pub fn primary_button(ui: &mut Ui, label: &str, enabled: bool) -> egui::Response {
    let fill = if enabled { accent_bg() } else { card() };
    let stroke = if enabled {
        Stroke::new(1.0, accent())
    } else {
        Stroke::new(1.0, border())
    };
    Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(18, 10))
        .stroke(stroke)
        .show(ui, |ui| {
            ui.add_enabled(
                enabled,
                egui::Button::new(RichText::new(label).color(text()).size(15.0))
                    .frame(false)
                    .min_size(Vec2::new(200.0, 28.0)),
            )
        })
        .inner
}

pub fn empty_state(ui: &mut Ui, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(80.0);
        card_frame().show(ui, |ui| {
            ui.set_width(360.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(title).size(18.0).strong().color(text()));
                ui.add_space(6.0);
                ui.label(RichText::new(hint).color(text_muted()));
            });
        });
    });
}

pub fn log_panel(ui: &mut Ui, max_height: f32, logs: &[(String, bool)]) {
    ui.add_space(16.0);
    ui.label(RichText::new("Лог").strong().color(text_muted()));
    ui.add_space(8.0);
    Frame::new()
        .fill(Color32::from_rgb(14, 16, 20))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(14, 12))
        .stroke(Stroke::new(1.0, border()))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .min_scrolled_height(72.0)
                .show(ui, |ui| {
                    if logs.is_empty() {
                        ui.label(RichText::new("Пока пусто").italics().color(text_muted()));
                    } else {
                        for (msg, is_err) in logs.iter().rev() {
                            let color = if *is_err { log_err() } else { log_ok() };
                            ui.label(RichText::new(msg).color(color).monospace().size(12.0));
                        }
                    }
                });
        });
}

pub fn thumb_colors(selected: bool, hovered: bool) -> (Color32, Option<Stroke>) {
    let bg = if selected {
        accent_bg()
    } else if hovered {
        card_hover()
    } else {
        card()
    };
    let stroke = if selected {
        Some(Stroke::new(2.0, accent()))
    } else if hovered {
        Some(Stroke::new(1.0, border()))
    } else {
        None
    };
    (bg, stroke)
}
