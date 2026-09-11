//! Visual language for the application.
//!
//! One palette and one set of metrics, applied to egui's style once at startup.
//! Widgets then inherit it, so a new panel looks right without restating colours
//! or corner radii at the call site.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2};

/// Application background, behind the cards.
pub(crate) const CANVAS: Color32 = Color32::from_rgb(0xED, 0xEF, 0xF2);
/// Card and panel fill.
pub(crate) const SURFACE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
/// Inset fill: text fields, number fields, segment tracks.
pub(crate) const SURFACE_ALT: Color32 = Color32::from_rgb(0xF3, 0xF5, 0xF7);
/// Hairline borders.
pub(crate) const BORDER: Color32 = Color32::from_rgb(0xE2, 0xE5, 0xEA);
/// Primary text.
pub(crate) const TEXT: Color32 = Color32::from_rgb(0x14, 0x17, 0x1C);
/// Secondary text: units, hints, metadata.
pub(crate) const TEXT_DIM: Color32 = Color32::from_rgb(0x6C, 0x73, 0x7F);
/// Selection and active state.
pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x2B, 0x6C, 0xF0);
/// Selection background.
pub(crate) const ACCENT_SOFT: Color32 = Color32::from_rgb(0xE8, 0xF0, 0xFE);
/// Near-black, for the single primary action.
pub(crate) const INK: Color32 = Color32::from_rgb(0x16, 0x19, 0x1E);

const CARD_RADIUS: u8 = 12;
const WIDGET_RADIUS: u8 = 8;

/// Family used for headings and emphasis.
///
/// egui does not synthesise bold, so weight has to come from a second face.
pub(crate) fn semibold() -> FontFamily {
    FontFamily::Name("inter-semibold".into())
}

/// Installs Inter and makes it the proportional face.
///
/// The font is vendored rather than loaded from the system: a machine may have
/// no fonts installed at all — this one does not — and falling back to whatever
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
    spacing.item_spacing = Vec2::new(8.0, 8.0);
    spacing.button_padding = Vec2::new(11.0, 6.0);
    spacing.interact_size.y = 30.0;
    spacing.indent = 14.0;
    spacing.window_margin = Margin::ZERO;

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
    v.window_shadow = Shadow {
        offset: [0, 2],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(18),
    };
    v.popup_shadow = v.window_shadow;

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

    w.hovered.bg_fill = Color32::from_rgb(0xE9, 0xEC, 0xF1);
    w.hovered.weak_bg_fill = Color32::from_rgb(0xE9, 0xEC, 0xF1);
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0xC9, 0xD0, 0xDA));

    w.active.bg_fill = ACCENT_SOFT;
    w.active.weak_bg_fill = ACCENT_SOFT;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
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

/// Section label inside a card.
pub(crate) fn section(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(12.0)
            .color(TEXT_DIM)
            .strong(),
    );
    ui.add_space(6.0);
}

/// The single dark call-to-action.
pub(crate) fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let button = egui::Button::new(egui::RichText::new(text).color(SURFACE).size(13.0))
        .fill(INK)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(WIDGET_RADIUS));
    ui.add(button)
}

/// A segmented control. Returns true if the choice changed.
pub(crate) fn segmented(ui: &mut egui::Ui, current: &mut usize, labels: &[&str]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(SURFACE_ALT)
        .corner_radius(CornerRadius::same(WIDGET_RADIUS + 2))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (i, label) in labels.iter().enumerate() {
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
                    if ui.add(button).clicked() && !active {
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
/// claim — in this kernel a fillet is a parameter that cannot fail, which is the
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
