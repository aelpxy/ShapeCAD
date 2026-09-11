//! Visual language for the application.
//!
//! One palette and one set of metrics, applied to egui's style once at startup.
//! Widgets then inherit it, so a new panel looks right without restating colours
//! or corner radii at the call site.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2};

// A neutral scale, kept deliberately colourless so the only saturated thing on
// screen is the selected geometry. Values follow the zinc ramp that shadcn/ui
// defaults to.

/// Application background, behind the cards.
pub(crate) const CANVAS: Color32 = Color32::from_rgb(0xFA, 0xFA, 0xFA);
/// Card and panel fill.
pub(crate) const SURFACE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
/// Inset and hover fill.
pub(crate) const SURFACE_ALT: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF5);
/// Hairline borders.
pub(crate) const BORDER: Color32 = Color32::from_rgb(0xE4, 0xE4, 0xE7);
/// Primary text.
pub(crate) const TEXT: Color32 = Color32::from_rgb(0x09, 0x09, 0x0B);
/// Secondary text: units, hints, metadata.
pub(crate) const TEXT_DIM: Color32 = Color32::from_rgb(0x71, 0x71, 0x7A);
/// Reserved for selected geometry, in the viewport and in the tree.
pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x25, 0x63, 0xEB);
/// Selection background.
pub(crate) const ACCENT_SOFT: Color32 = Color32::from_rgb(0xEF, 0xF6, 0xFF);
/// Near-black, for the single primary action.
/// Destructive actions. The only other hue in the interface.
pub(crate) const DANGER: Color32 = Color32::from_rgb(0xDC, 0x26, 0x26);
/// Near-black, used for the single primary button and the application mark.
pub(crate) const INK: Color32 = Color32::from_rgb(0x18, 0x18, 0x1B);

const CARD_RADIUS: u8 = 8;
const WIDGET_RADIUS: u8 = 6;

/// Family used for headings and emphasis.
///
/// egui does not synthesise bold, so weight has to come from a second face.
pub(crate) fn semibold() -> FontFamily {
    FontFamily::Name("inter-semibold".into())
}

/// Installs Inter and makes it the proportional face.
///
/// The font is vendored rather than loaded from the system: a machine may have
/// no fonts installed at all (this one does not) and falling back to whatever
/// happens to be present would make the interface look different everywhere.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    use std::sync::Arc;

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-semibold".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );

    // Inter first, egui's bundled faces after it so symbols and emoji still
    // resolve when Inter has no glyph.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "inter".to_owned());
    fonts.families.insert(
        semibold(),
        vec!["inter-semibold".to_owned(), "inter".to_owned()],
    );

    ctx.set_fonts(fonts);
}

/// Installs the palette and metrics.
pub(crate) fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    // The application is light-themed by design, so the light style is both
    // themes: egui would otherwise swap palettes with the desktop preference and
    // undo everything below.
    ctx.set_theme(egui::ThemePreference::Light);
    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();

    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(15.5, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
    ]
    .into();

    let spacing = &mut style.spacing;
    spacing.item_spacing = Vec2::new(6.0, 5.0);
    spacing.button_padding = Vec2::new(9.0, 5.0);
    spacing.interact_size.y = 28.0;
    spacing.indent = 12.0;
    spacing.window_margin = Margin::ZERO;
    // Tooltips and menus share this margin and this width. A hover label that
    // runs the width of the screen is unreadable, so it is capped near the
    // width of a sidebar card.
    spacing.menu_margin = Margin::symmetric(10, 8);
    spacing.tooltip_width = 270.0;
    // egui fades the edge of a scroll area towards the background colour it
    // finds on the Ui stack. Inside a white card that resolves to a mid grey,
    // so the fade paints a dull band over the last row instead of disappearing
    // into it. The cards are solid and bounded by a border already.
    spacing.scroll.fade.strength = 0.0;

    let v = &mut style.visuals;
    v.dark_mode = false;
    v.panel_fill = CANVAS;
    v.window_fill = SURFACE;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    v.extreme_bg_color = SURFACE_ALT;
    v.faint_bg_color = SURFACE_ALT;
    v.override_text_color = Some(TEXT);
    v.weak_text_color = Some(TEXT_DIM);
    v.selection.bg_fill = ACCENT_SOFT;
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    // Barely there. A card is separated by its border, not by a drop shadow.
    v.window_shadow = Shadow {
        offset: [0, 1],
        blur: 3,
        spread: 0,
        color: Color32::from_black_alpha(12),
    };
    // Tooltips and menus float, so they get a little more separation than a
    // card, which is held apart by its border alone.
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(26),
    };
    v.menu_corner_radius = CornerRadius::same(CARD_RADIUS);

    let w = &mut v.widgets;
    for s in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        s.corner_radius = CornerRadius::same(WIDGET_RADIUS);
        s.fg_stroke = Stroke::new(1.0, TEXT);
        s.expansion = 0.0;
    }
    w.noninteractive.bg_fill = SURFACE;
    w.noninteractive.weak_bg_fill = SURFACE;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);

    w.inactive.bg_fill = SURFACE_ALT;
    w.inactive.weak_bg_fill = SURFACE_ALT;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);

    w.hovered.bg_fill = SURFACE_ALT;
    w.hovered.weak_bg_fill = SURFACE_ALT;
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xD4, 0xD4, 0xD8));

    w.active.bg_fill = SURFACE_ALT;
    w.active.weak_bg_fill = SURFACE_ALT;
    w.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xA1, 0xA1, 0xAA));
    // Not the accent: `Visuals::strong_text_color` reads this, so tinting it
    // would turn every bold label in the application blue.
    w.active.fg_stroke = Stroke::new(1.0, TEXT);

    w.open.bg_fill = SURFACE_ALT;
    w.open.weak_bg_fill = SURFACE_ALT;
    w.open.bg_stroke = Stroke::new(1.0, BORDER);

    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// A white card: the unit the whole layout is built from.
pub(crate) fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .inner_margin(Margin::symmetric(14, 12))
}

/// A card that floats over the 3D view, so it needs a shadow to separate.
pub(crate) fn floating() -> egui::Frame {
    card().inner_margin(Margin::symmetric(8, 6)).shadow(Shadow {
        offset: [0, 3],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(38),
    })
}

/// A flat bar with a hairline on one edge, for the top and bottom chrome.
pub(crate) fn bar(bottom_border: bool) -> egui::Frame {
    let _ = bottom_border;
    egui::Frame::new()
        .fill(SURFACE)
        .inner_margin(Margin::symmetric(14, 0))
}

/// Section label inside a card: small, muted, widely tracked.
pub(crate) fn section(ui: &mut egui::Ui, text: &str) {
    // egui has no letter-spacing, so the tracking is put in by hand. At this
    // size it is the difference between a label and a heading.
    let spaced: String = text
        .chars()
        .flat_map(|c| [c, '\u{2009}'])
        .collect::<String>()
        .trim_end()
        .to_owned();
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(spaced)
            .size(10.5)
            .color(TEXT_DIM)
            .family(semibold()),
    );
    ui.add_space(2.0);
}

/// A sidebar row: icon, label, full width, no chrome until hovered.
///
/// The building block of the tool list. Ghost by default, so a column of them
/// reads as a list rather than a wall of buttons.
pub(crate) fn row(
    ui: &mut egui::Ui,
    icon: crate::icon::Icon,
    label: &str,
    active: bool,
    enabled: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 28.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );

    let hovered = enabled && response.hovered();
    let fill = if active || hovered {
        SURFACE_ALT
    } else {
        Color32::TRANSPARENT
    };
    let tint = if enabled {
        if active {
            TEXT
        } else {
            TEXT_DIM
        }
    } else {
        TEXT_DIM.gamma_multiply(0.45)
    };

    let painter = ui.painter();
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, CornerRadius::same(WIDGET_RADIUS), fill);
    }

    let glyph = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 17.0, rect.center().y),
        Vec2::splat(15.0),
    );
    crate::icon::draw(painter, glyph, icon, tint);

    painter.text(
        egui::pos2(rect.left() + 33.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.5),
        if enabled {
            TEXT
        } else {
            TEXT_DIM.gamma_multiply(0.6)
        },
    );

    response
}

/// A compact icon-and-label button. Used in the top bar and the floating
/// toolbar, where a full-width row would be wrong but a bare label is mute.
pub(crate) fn tool_button(
    ui: &mut egui::Ui,
    icon: crate::icon::Icon,
    label: &str,
    active: bool,
    enabled: bool,
) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(12.5),
        Color32::PLACEHOLDER,
    );
    let width = galley.size().x + 34.0;
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(width, 30.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    paint_button(
        ui,
        rect,
        &response,
        Some(icon),
        Some(galley),
        active,
        enabled,
    );
    response
}

/// Icon only, square. For actions whose glyph is universal: undo, redo.
///
/// The caller is expected to wrap the result in [`hint`], since an icon with no
/// label has to explain itself on hover.
pub(crate) fn icon_button(
    ui: &mut egui::Ui,
    icon: crate::icon::Icon,
    enabled: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::splat(30.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    paint_button(ui, rect, &response, Some(icon), None, false, enabled);
    response
}

/// Shared painting for the ghost buttons above.
fn paint_button(
    ui: &egui::Ui,
    rect: egui::Rect,
    response: &egui::Response,
    icon: Option<crate::icon::Icon>,
    galley: Option<std::sync::Arc<egui::Galley>>,
    active: bool,
    enabled: bool,
) {
    let hovered = enabled && response.hovered();
    let painter = ui.painter();
    let fill = if active {
        ACCENT_SOFT
    } else if hovered {
        SURFACE_ALT
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, CornerRadius::same(WIDGET_RADIUS), fill);
    }

    let tint = if !enabled {
        TEXT_DIM.gamma_multiply(0.45)
    } else if active {
        ACCENT
    } else {
        TEXT_DIM
    };
    let text = if !enabled {
        TEXT_DIM.gamma_multiply(0.6)
    } else if active {
        ACCENT
    } else {
        TEXT
    };

    match galley {
        None => {
            if let Some(icon) = icon {
                let glyph = egui::Rect::from_center_size(rect.center(), Vec2::splat(15.0));
                crate::icon::draw(painter, glyph, icon, tint);
            }
        }
        Some(galley) => {
            if let Some(icon) = icon {
                let glyph = egui::Rect::from_center_size(
                    egui::pos2(rect.left() + 16.0, rect.center().y),
                    Vec2::splat(15.0),
                );
                crate::icon::draw(painter, glyph, icon, tint);
            }
            let at = egui::pos2(rect.left() + 27.0, rect.center().y - galley.size().y * 0.5);
            painter.galley(at, galley, text);
        }
    }
}

/// A design tree row: indent guide, kind glyph, name, and a muted kind note.
///
/// Selection is a filled ghost row rather than a highlighted label, so the whole
/// width is the hit target and a deep tree still reads as a single column.
pub(crate) fn tree_row(
    ui: &mut egui::Ui,
    depth: usize,
    icon: crate::icon::Icon,
    title: &str,
    note: Option<&str>,
    selected: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), egui::Sense::click());
    let indent = depth as f32 * 14.0;
    let painter = ui.painter();

    if selected {
        painter.rect_filled(rect, CornerRadius::same(WIDGET_RADIUS), ACCENT_SOFT);
    } else if response.hovered() {
        painter.rect_filled(rect, CornerRadius::same(WIDGET_RADIUS), SURFACE_ALT);
    }

    // One hairline per level of nesting, so parentage is visible without
    // disclosure triangles on a tree that is never collapsed.
    for level in 0..depth {
        let x = rect.left() + 14.0 + level as f32 * 14.0;
        painter.line_segment(
            [
                egui::pos2(x, rect.top() + 1.0),
                egui::pos2(x, rect.bottom() - 1.0),
            ],
            Stroke::new(1.0, BORDER),
        );
    }

    let tint = if selected { ACCENT } else { TEXT_DIM };
    let glyph = egui::Rect::from_center_size(
        egui::pos2(rect.left() + indent + 16.0, rect.center().y),
        Vec2::splat(14.0),
    );
    crate::icon::draw(painter, glyph, icon, tint);

    let mut x = rect.left() + indent + 30.0;
    let text = painter.layout_no_wrap(
        title.to_owned(),
        egui::FontId::new(12.5, semibold()),
        Color32::PLACEHOLDER,
    );
    let width = text.size().x;
    painter.galley(
        egui::pos2(x, rect.center().y - text.size().y * 0.5),
        text,
        if selected { ACCENT } else { TEXT },
    );
    x += width + 6.0;

    if let Some(note) = note {
        painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            note,
            egui::FontId::proportional(10.5),
            TEXT_DIM,
        );
    }

    response
}

/// Attaches an explanatory tooltip to a control.
///
/// A bold first line saying what the control does, a muted second line saying
/// how it behaves or why it is unavailable, and an optional keyboard shortcut
/// set off to the right. Every control in the interface carries one, because a
/// line icon on its own does not teach anyone what a tool is for.
pub(crate) fn hint(
    response: egui::Response,
    title: &str,
    body: &str,
    shortcut: Option<&str>,
) -> egui::Response {
    response.on_hover_ui(|ui| {
        ui.set_max_width(250.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(title).size(12.5).family(semibold()));
            if let Some(shortcut) = shortcut {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(shortcut)
                        .size(11.0)
                        .color(TEXT_DIM)
                        .background_color(SURFACE_ALT),
                );
            }
        });
        if !body.is_empty() {
            ui.label(egui::RichText::new(body).size(11.5).color(TEXT_DIM));
        }
    })
}

/// The frame a context menu is drawn in.
pub(crate) fn menu() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(CARD_RADIUS))
        .shadow(Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_black_alpha(34),
        })
        .inner_margin(Margin::symmetric(5, 5))
}

/// One item in a context menu: icon, label, and an optional shortcut.
///
/// Narrower and tighter than [`row`], because a menu is read at the pointer
/// rather than scanned down a panel.
pub(crate) fn menu_item(
    ui: &mut egui::Ui,
    icon: crate::icon::Icon,
    label: &str,
    shortcut: Option<&str>,
    enabled: bool,
    destructive: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 26.0),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );

    let hovered = enabled && response.hovered();
    let painter = ui.painter();
    if hovered {
        let tint = if destructive {
            DANGER.gamma_multiply(0.08)
        } else {
            SURFACE_ALT
        };
        painter.rect_filled(rect, CornerRadius::same(WIDGET_RADIUS - 2), tint);
    }

    let (glyph_tint, text_tint) = match (enabled, destructive) {
        (false, _) => (TEXT_DIM.gamma_multiply(0.45), TEXT_DIM.gamma_multiply(0.6)),
        (true, true) => (DANGER, DANGER),
        (true, false) => (TEXT_DIM, TEXT),
    };

    let glyph = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 15.0, rect.center().y),
        Vec2::splat(14.0),
    );
    crate::icon::draw(painter, glyph, icon, glyph_tint);
    painter.text(
        egui::pos2(rect.left() + 29.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.5),
        text_tint,
    );
    if let Some(shortcut) = shortcut {
        painter.text(
            egui::pos2(rect.right() - 9.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            shortcut,
            egui::FontId::proportional(11.0),
            TEXT_DIM.gamma_multiply(if enabled { 1.0 } else { 0.5 }),
        );
    }

    response
}

/// A menu heading: what the menu is acting on.
pub(crate) fn menu_title(ui: &mut egui::Ui, title: &str, note: &str) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), egui::Sense::hover());
    let painter = ui.painter();
    let galley = painter.layout_no_wrap(
        title.to_owned(),
        egui::FontId::new(11.5, semibold()),
        Color32::PLACEHOLDER,
    );
    let width = galley.size().x;
    painter.galley(
        egui::pos2(rect.left() + 9.0, rect.center().y - galley.size().y * 0.5),
        galley,
        TEXT,
    );
    if !note.is_empty() {
        painter.text(
            egui::pos2(rect.left() + 15.0 + width, rect.center().y),
            egui::Align2::LEFT_CENTER,
            note,
            egui::FontId::proportional(10.5),
            TEXT_DIM,
        );
    }
}

/// A hairline between groups of menu items.
pub(crate) fn menu_separator(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 7.0), egui::Sense::hover());
    ui.painter().line_segment(
        [
            egui::pos2(rect.left() + 5.0, rect.center().y),
            egui::pos2(rect.right() - 5.0, rect.center().y),
        ],
        Stroke::new(1.0, BORDER),
    );
}

/// A ghost row for a destructive action. The only red in the interface.
pub(crate) fn danger_row(
    ui: &mut egui::Ui,
    icon: crate::icon::Icon,
    label: &str,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), egui::Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(
            rect,
            CornerRadius::same(WIDGET_RADIUS),
            DANGER.gamma_multiply(0.08),
        );
    }
    let glyph = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 17.0, rect.center().y),
        Vec2::splat(15.0),
    );
    crate::icon::draw(painter, glyph, icon, DANGER);
    painter.text(
        egui::pos2(rect.left() + 33.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.5),
        DANGER,
    );
    response
}

/// A centred placeholder for a panel with nothing in it.
///
/// An empty panel that says nothing reads as broken. This says what would go
/// there and how to make it appear.
pub(crate) fn empty_state(ui: &mut egui::Ui, icon: crate::icon::Icon, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(28.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::hover());
        crate::icon::draw(ui.painter(), rect, icon, TEXT_DIM.gamma_multiply(0.55));
        ui.add_space(10.0);
        ui.label(egui::RichText::new(title).size(13.0).family(semibold()));
        ui.add_space(4.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        ui.set_max_width(220.0);
        ui.label(egui::RichText::new(hint).size(11.5).color(TEXT_DIM));
        ui.add_space(28.0);
    });
}

/// The single dark call-to-action.
pub(crate) fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let button = egui::Button::new(egui::RichText::new(text).color(SURFACE).size(13.0))
        .fill(INK)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(WIDGET_RADIUS));
    ui.add(button)
}

/// The same, with a leading glyph.
pub(crate) fn primary_button_with_icon(
    ui: &mut egui::Ui,
    icon: crate::icon::Icon,
    text: &str,
) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(13.0), SURFACE);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + 38.0, 32.0),
        egui::Sense::click(),
    );
    let painter = ui.painter();
    let fill = if response.hovered() {
        INK.gamma_multiply(0.88)
    } else {
        INK
    };
    painter.rect_filled(rect, CornerRadius::same(WIDGET_RADIUS), fill);
    let glyph = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 17.0, rect.center().y),
        Vec2::splat(15.0),
    );
    crate::icon::draw(painter, glyph, icon, SURFACE);
    let at = egui::pos2(rect.left() + 29.0, rect.center().y - galley.size().y * 0.5);
    painter.galley(at, galley, SURFACE);
    response
}

/// A segmented control. Returns true if the choice changed.
///
/// Each entry is a label and the tooltip that explains it.
pub(crate) fn segmented(ui: &mut egui::Ui, current: &mut usize, labels: &[(&str, &str)]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(SURFACE_ALT)
        .corner_radius(CornerRadius::same(WIDGET_RADIUS + 2))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (i, (label, help)) in labels.iter().enumerate() {
                    let active = *current == i;
                    let text = egui::RichText::new(*label).size(13.0).color(if active {
                        TEXT
                    } else {
                        TEXT_DIM
                    });
                    let button = egui::Button::new(text)
                        .fill(if active {
                            SURFACE
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(WIDGET_RADIUS))
                        .min_size(Vec2::new(84.0, 26.0));
                    if hint(ui.add(button), label, help, None).clicked() && !active {
                        *current = i;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Face tints for the mark, lightest first.
const MARK_TOP: Color32 = Color32::from_rgb(0x86, 0xAD, 0xF8);
const MARK_BODY: Color32 = Color32::from_rgb(0x2B, 0x6C, 0xF0);
const MARK_RIGHT: Color32 = Color32::from_rgb(0x17, 0x3F, 0x9B);

/// Replaces a sharp corner with an arc tangent to both edges.
///
/// The corner is used as the control point of a quadratic, which is tangency by
/// construction.
fn fillet(prev: egui::Pos2, corner: egui::Pos2, next: egui::Pos2, radius: f32) -> Vec<egui::Pos2> {
    /// Arc subdivisions. Eight is smooth well past the sizes the mark is drawn at.
    const SEGMENTS: usize = 8;

    if radius <= 0.0 {
        return vec![corner];
    }
    let into = (corner - prev).normalized();
    let out = (next - corner).normalized();
    // Never round away more than half an edge, or adjacent arcs overlap.
    let limit = 0.5 * (corner - prev).length().min((next - corner).length());
    let radius = radius.min(limit);

    let start = corner - into * radius;
    let end = corner + out * radius;

    (0..=SEGMENTS)
        .map(|i| {
            let t = i as f32 / SEGMENTS as f32;
            let inv = 1.0 - t;
            egui::pos2(
                inv * inv * start.x + 2.0 * inv * t * corner.x + t * t * end.x,
                inv * inv * start.y + 2.0 * inv * t * corner.y + t * t * end.y,
            )
        })
        .collect()
}

/// Rounds each vertex of a polygon by its own radius. Zero leaves it sharp.
fn rounded_polygon(points: &[egui::Pos2], radii: &[f32]) -> Vec<egui::Pos2> {
    let n = points.len();
    (0..n)
        .flat_map(|i| {
            let prev = points[(i + n - 1) % n];
            let next = points[(i + 1) % n];
            fillet(prev, points[i], next, radii[i])
        })
        .collect()
}

/// The application mark: an isometric solid with its corners filleted.
///
/// A plain cube would say "3D" and nothing more. The rounding is the specific
/// claim. In this kernel a fillet is a parameter that cannot fail, which is the
/// reason the whole thing is built the way it is.
///
/// The silhouette is drawn first as one rounded body, with the lit faces laid
/// over it. Drawing three separate faces leaves hairline seams where their
/// antialiased edges meet.
pub(crate) fn logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let c = rect.center();
    let r = size * 0.46;
    let round = r * 0.24;

    // Isometric projection at 30 degrees, so the half-width is cos(30).
    let half_w = r * 0.866;
    let apex = egui::pos2(c.x, c.y - r);
    let upper_l = egui::pos2(c.x - half_w, c.y - r * 0.5);
    let upper_r = egui::pos2(c.x + half_w, c.y - r * 0.5);
    let middle = egui::pos2(c.x, c.y);
    let lower_l = egui::pos2(c.x - half_w, c.y + r * 0.5);
    let lower_r = egui::pos2(c.x + half_w, c.y + r * 0.5);
    let base = egui::pos2(c.x, c.y + r);

    let painter = ui.painter();
    let no_edge = Stroke::NONE;

    // Body: the whole silhouette, every outer corner rounded.
    let silhouette = [apex, upper_r, lower_r, base, lower_l, upper_l];
    painter.add(egui::Shape::convex_polygon(
        rounded_polygon(&silhouette, &[round; 6]),
        MARK_BODY,
        no_edge,
    ));

    // Lit faces. The shared centre stays sharp; rounding it would open a notch
    // where the three faces meet.
    let top = [upper_l, apex, upper_r, middle];
    painter.add(egui::Shape::convex_polygon(
        rounded_polygon(&top, &[round, round, round, 0.0]),
        MARK_TOP,
        no_edge,
    ));

    let right = [middle, upper_r, lower_r, base];
    painter.add(egui::Shape::convex_polygon(
        rounded_polygon(&right, &[0.0, round, round, round]),
        MARK_RIGHT,
        no_edge,
    ));
}
