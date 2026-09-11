//! Visual language for the application.
//!
//! One palette and one set of metrics, applied to egui's style once at startup.
//! Widgets then inherit it, so a new panel looks right without restating colours
//! or corner radii at the call site.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2};

/// Which of the two palettes is in use.
///
/// Resolved, not requested: "follow the system" is a preference, and by the time
/// anything is painted it has become one of these.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Scheme {
    #[default]
    Light,
    Dark,
}

/// Every colour the interface uses, so a scheme is one value rather than a
/// scattering of `if dark` branches at the call sites.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Palette {
    /// Application background, behind the cards.
    pub canvas: Color32,
    /// Card and panel fill.
    pub surface: Color32,
    /// Inset and hover fill.
    pub surface_alt: Color32,
    /// Hairline borders.
    pub border: Color32,
    /// Primary text.
    pub text: Color32,
    /// Secondary text: units, hints, metadata.
    pub text_dim: Color32,
    /// Reserved for selected geometry, in the viewport and in the tree.
    pub accent: Color32,
    /// Selection background.
    pub accent_soft: Color32,
    /// Destructive actions. The only other hue in the interface.
    pub danger: Color32,
    /// The single primary button and the application mark.
    pub ink: Color32,
    /// Text on top of `ink`.
    pub on_ink: Color32,
    /// X, Y and Z, for the axis legend. The one place the interface borrows the
    /// scene's colours, so the legend reads as naming the lines behind it.
    pub axis: [Color32; 3],
    /// Viewport sky, at the top of the sweep.
    pub sky: [f32; 3],
    /// Viewport sky, at the horizon.
    pub haze: [f32; 3],
    /// The build plate.
    pub plate: [f32; 3],
    /// Grid lines on the plate.
    pub grid: [f32; 3],
}

// Apple's system colours, which is what makes an interface read as native on a
// platform that has them and as deliberate on one that does not. A grouped
// background behind raised content, one system blue, one system red, and labels
// at two levels of emphasis.

const LIGHT: Palette = Palette {
    // systemGroupedBackground: the interface sits *in* something, rather than
    // floating on white. This is what gives grouped lists their edges without
    // needing a border around each one.
    canvas: Color32::from_rgb(0xF2, 0xF2, 0xF7),
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    // tertiarySystemFill, the pressed and selected state of a control.
    surface_alt: Color32::from_rgb(0xE7, 0xE7, 0xEC),
    // separator, opaque. Hairlines are the only structure between list rows.
    border: Color32::from_rgb(0xD8, 0xD8, 0xDD),
    // label and secondaryLabel.
    text: Color32::from_rgb(0x1C, 0x1C, 0x1E),
    text_dim: Color32::from_rgb(0x8A, 0x8A, 0x8E),
    // systemBlue.
    accent: Color32::from_rgb(0x00, 0x7A, 0xFF),
    accent_soft: Color32::from_rgb(0xE4, 0xEF, 0xFF),
    // systemRed.
    danger: Color32::from_rgb(0xFF, 0x3B, 0x30),
    // A filled primary button is tinted, not black: on this platform the single
    // most important action is the accent colour, and everything else is plain.
    ink: Color32::from_rgb(0x00, 0x7A, 0xFF),
    on_ink: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    axis: [
        Color32::from_rgb(0xC7, 0x5C, 0x5C),
        Color32::from_rgb(0x66, 0x9E, 0x66),
        Color32::from_rgb(0x5C, 0x7F, 0xC7),
    ],
    sky: [0.700, 0.735, 0.790],
    haze: [0.930, 0.943, 0.962],
    plate: [0.895, 0.910, 0.930],
    grid: [0.66, 0.69, 0.73],
};

// The dark system palette. Surfaces lift as they come forward, which is the
// reverse of the light scheme, because on a dark background a raised card reads
// by being lighter than what is behind it.
//
// The canvas is pure black on purpose: that is what the platform does, and it is
// what lets the raised surfaces read as raised at all.
const DARK: Palette = Palette {
    canvas: Color32::from_rgb(0x00, 0x00, 0x00),
    // secondarySystemGroupedBackground and tertiary, the two raised levels.
    surface: Color32::from_rgb(0x1C, 0x1C, 0x1E),
    surface_alt: Color32::from_rgb(0x2C, 0x2C, 0x2E),
    border: Color32::from_rgb(0x38, 0x38, 0x3A),
    text: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    text_dim: Color32::from_rgb(0x98, 0x98, 0x9F),
    // systemBlue and systemRed, dark variants: both are lifted, because the
    // light ones do not carry against a dark surface.
    accent: Color32::from_rgb(0x0A, 0x84, 0xFF),
    accent_soft: Color32::from_rgb(0x0A, 0x25, 0x40),
    danger: Color32::from_rgb(0xFF, 0x45, 0x3A),
    ink: Color32::from_rgb(0x0A, 0x84, 0xFF),
    on_ink: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    // Lifted, like the accent and the red: the light tints carry too little
    // against a dark surface to be read at eleven points.
    axis: [
        Color32::from_rgb(0xE8, 0x84, 0x84),
        Color32::from_rgb(0x84, 0xC7, 0x84),
        Color32::from_rgb(0x84, 0xA6, 0xE8),
    ],
    // Linear, not sRGB: the shader writes into a linear target and the swapchain
    // encodes on the way out, so a value picked to look right as a hex colour
    // comes out roughly two and a half times too light. These are the zinc ramp
    // converted, and they match the chrome beside them:
    // #0B0B0E, #18181B, #1F1F23, #3F3F46.
    sky: [0.0033, 0.0033, 0.0044],
    haze: [0.0091, 0.0091, 0.0110],
    plate: [0.0137, 0.0137, 0.0168],
    grid: [0.0497, 0.0497, 0.0612],
};

/// The active scheme, as a `u8` so the palette can be read from anywhere that
/// paints without threading it through every function that draws a label.
///
/// A desktop application has exactly one appearance at a time, so this is a
/// property of the process rather than of any one widget tree.
static ACTIVE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Switches the palette. Takes effect on the next frame.
pub(crate) fn set_scheme(scheme: Scheme) {
    ACTIVE.store(scheme as u8, std::sync::atomic::Ordering::Relaxed);
}

#[must_use]
pub(crate) fn scheme() -> Scheme {
    if ACTIVE.load(std::sync::atomic::Ordering::Relaxed) == Scheme::Dark as u8 {
        Scheme::Dark
    } else {
        Scheme::Light
    }
}

/// The viewport's share of the palette, in the form the renderer wants.
#[must_use]
pub(crate) fn scene() -> sc_render::ScenePalette {
    let p = palette();
    sc_render::ScenePalette {
        sky: p.sky,
        haze: p.haze,
        plate: p.plate,
        grid: p.grid,
    }
}

/// The tint for one named world axis.
///
/// The legend, the gizmo and the locked-axis line all name the axis rather than
/// index it, so there is one place that decides X is red and nowhere for the
/// three of them to disagree.
#[must_use]
pub(crate) fn axis_tint(name: &str) -> Color32 {
    let p = palette();
    match name {
        "X" => p.axis[0],
        "Y" => p.axis[1],
        _ => p.axis[2],
    }
}

/// The colours in force right now.
#[must_use]
pub(crate) fn palette() -> Palette {
    match scheme() {
        Scheme::Light => LIGHT,
        Scheme::Dark => DARK,
    }
}

/// Corner radii, on the platform's scale rather than the web's.
///
/// A grouped list is 10, a card or sheet is larger, and a control inside one is
/// smaller than the thing containing it or the two corners fight.
const CARD_RADIUS: u8 = 14;
const WIDGET_RADIUS: u8 = 9;
/// A pill: a segmented control's track and the indicator that slides along it.
const PILL_RADIUS: u8 = 8;
/// How wide a tooltip is allowed to run. A hover label the width of the screen
/// is unreadable, so it is capped near the width of a sidebar card.
const TOOLTIP_WIDTH: f32 = 250.0;
/// A grouped list. Smaller than a card, because a group sits inside one.
const GROUP_RADIUS: u8 = 10;
/// Where a grouped row's label starts, measured from the group's edge.
const GROUP_INSET: f32 = 6.0;

/// How far a control shrinks while it is held down.
///
/// Pressing something should move it. A tint alone reads as a state change,
/// which is a different message from "this is responding to you".
const PRESS_SHRINK: f32 = 0.045;

/// Shrinks a rect about its centre.
fn scaled(rect: egui::Rect, factor: f32) -> egui::Rect {
    egui::Rect::from_center_size(rect.center(), rect.size() * factor)
}

/// The factor to draw a control at, given whether it is currently held.
///
/// Springs, so releasing rebounds rather than snapping back.
fn press_scale(ui: &egui::Ui, response: &egui::Response) -> f32 {
    let held = response.is_pointer_button_down_on();
    let t = crate::motion::animate_bool(
        ui,
        response.id.with("press"),
        held,
        crate::motion::Tuning::SNAPPY,
    );
    1.0 - PRESS_SHRINK * t
}

/// How far into its hover state a widget is, from 0 to 1.
fn hover_fade(ui: &egui::Ui, response: &egui::Response) -> f32 {
    crate::motion::animate_bool(
        ui,
        response.id.with("hover"),
        response.hovered(),
        crate::motion::Tuning::SMOOTH,
    )
}

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
    // Pinned to egui's light style regardless of the palette in force. Every
    // colour that shows is set from `palette()` below, and letting egui swap its
    // own base underneath would change the handful this does not name, giving a
    // scheme assembled from two sources instead of one. Idempotent, so it can be
    // called again whenever the palette changes.
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
    // Tooltips and menus share this margin.
    spacing.menu_margin = Margin::symmetric(10, 8);
    spacing.tooltip_width = TOOLTIP_WIDTH;
    // egui fades the edge of a scroll area towards the background colour it
    // finds on the Ui stack. Inside a white card that resolves to a mid grey,
    // so the fade paints a dull band over the last row instead of disappearing
    // into it. The cards are solid and bounded by a border already.
    spacing.scroll.fade.strength = 0.0;

    let v = &mut style.visuals;
    v.dark_mode = false;
    v.panel_fill = palette().canvas;
    v.window_fill = palette().surface;
    v.window_stroke = Stroke::new(1.0, palette().border);
    v.window_corner_radius = CornerRadius::same(CARD_RADIUS);
    v.extreme_bg_color = palette().surface_alt;
    v.faint_bg_color = palette().surface_alt;
    v.override_text_color = Some(palette().text);
    v.weak_text_color = Some(palette().text_dim);
    v.selection.bg_fill = palette().accent_soft;
    v.selection.stroke = Stroke::new(1.0, palette().accent);
    v.hyperlink_color = palette().accent;
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
        s.fg_stroke = Stroke::new(1.0, palette().text);
        s.expansion = 0.0;
    }
    w.noninteractive.bg_fill = palette().surface;
    w.noninteractive.weak_bg_fill = palette().surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, palette().border);

    w.inactive.bg_fill = palette().surface_alt;
    w.inactive.weak_bg_fill = palette().surface_alt;
    w.inactive.bg_stroke = Stroke::new(1.0, palette().border);

    // Two steps along the same ramp the hairlines come from. Named colours here
    // outlined every field in the property panel in light grey under the
    // pointer once the dark palette was in force.
    w.hovered.bg_fill = palette().surface_alt;
    w.hovered.weak_bg_fill = palette().surface_alt;
    w.hovered.bg_stroke = Stroke::new(1.0, palette().border);

    w.active.bg_fill = palette().surface_alt;
    w.active.weak_bg_fill = palette().surface_alt;
    w.active.bg_stroke = Stroke::new(1.0, palette().text_dim);
    // Not the accent: `Visuals::strong_text_color` reads this, so tinting it
    // would turn every bold label in the application blue.
    w.active.fg_stroke = Stroke::new(1.0, palette().text);

    w.open.bg_fill = palette().surface_alt;
    w.open.weak_bg_fill = palette().surface_alt;
    w.open.bg_stroke = Stroke::new(1.0, palette().border);

    ctx.set_style_of(egui::Theme::Light, style.clone());
    ctx.set_style_of(egui::Theme::Dark, style);
}

/// A white card: the unit the whole layout is built from.
pub(crate) fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(palette().surface)
        .stroke(Stroke::new(1.0, palette().border))
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

/// A flat bar for the top and bottom chrome.
///
/// No border of its own: the canvas gutter around the panels and the viewport
/// between them are each a different colour in either scheme, and that edge is
/// what separates the bar from what it sits against.
pub(crate) fn bar() -> egui::Frame {
    egui::Frame::new()
        .fill(palette().surface)
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
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        // Indented to the same place a row's label starts, so the header reads
        // as belonging to the group under it rather than to the panel.
        ui.add_space(GROUP_INSET);
        ui.label(
            egui::RichText::new(spaced)
                .size(10.5)
                .color(palette().text_dim)
                .family(semibold()),
        );
    });
    ui.add_space(5.0);
}

/// A grouped list: a run of rows on one rounded surface.
///
/// This is the shape a settings or tool list takes on the platform. Rows do not
/// carry their own chrome; the group does, and the rows inside it are separated
/// by hairlines rather than by gaps. Read as one object with parts, instead of
/// as a column of independent buttons.
pub(crate) fn group() -> egui::Frame {
    egui::Frame::new()
        .fill(palette().surface)
        .corner_radius(CornerRadius::same(GROUP_RADIUS))
        .inner_margin(Margin::symmetric(6, 4))
}

/// Builds the rows of a grouped list, putting the hairlines in for you.
///
/// The separator belongs between rows and nowhere else, which is fiddly to get
/// right by hand in a list whose length depends on what is selected. Asking for
/// rows and inserting them here means it cannot be got wrong.
pub(crate) struct Rows<'a> {
    ui: &'a mut egui::Ui,
    placed: bool,
}

impl Rows<'_> {
    pub(crate) fn row(
        &mut self,
        icon: crate::icon::Icon,
        label: &str,
        active: bool,
        enabled: bool,
    ) -> egui::Response {
        self.divide();
        row(self.ui, icon, label, active, enabled)
    }

    /// A row that is not one of ours, drawn with the separator still handled.
    pub(crate) fn custom<T>(&mut self, add: impl FnOnce(&mut egui::Ui) -> T) -> T {
        self.divide();
        add(self.ui)
    }

    fn divide(&mut self) {
        if self.placed {
            group_separator(self.ui);
        }
        self.placed = true;
    }
}

/// A grouped list of rows on one rounded surface.
pub(crate) fn grouped(ui: &mut egui::Ui, add: impl FnOnce(&mut Rows<'_>)) {
    group().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        add(&mut Rows { ui, placed: false });
    });
}

/// The hairline between two rows of a group.
///
/// Inset from the left so it starts under the label rather than under the icon,
/// which is what stops a list of rows looking like a table.
pub(crate) fn group_separator(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), egui::Sense::hover());
    ui.painter().hline(
        (rect.left() + 33.0)..=rect.right(),
        rect.center().y,
        Stroke::new(1.0, palette().border),
    );
}

/// A large navigation title.
///
/// Sits above its content at a size nothing else in the panel comes near, which
/// is what makes a panel read as a place rather than as a region.
/// Truncated rather than wrapped: a title is one line, and a node can be given
/// a name longer than the panel it is shown in.
pub(crate) fn large_title(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(21.0)
                .family(semibold())
                .color(palette().text),
        )
        .truncate(),
    )
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

    // Selected sits at full strength, hover at a little over half, so the two
    // are distinguishable while the pointer is on the selected row.
    let hovered = enabled && response.hovered();
    let target = if active {
        1.0
    } else if hovered {
        0.6
    } else {
        0.0
    };
    let fade = crate::motion::animate(
        ui,
        response.id.with("fill"),
        target,
        crate::motion::Tuning::SMOOTH,
    );
    let fill = palette().surface_alt.gamma_multiply(fade);
    let tint = if enabled {
        if active {
            palette().text
        } else {
            palette().text_dim
        }
    } else {
        palette().text_dim.gamma_multiply(0.45)
    };

    let painter = ui.painter();
    if fade > 0.0 {
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
            palette().text
        } else {
            palette().text_dim.gamma_multiply(0.6)
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
    let painter = ui.painter();
    // The fill fades and the whole control shrinks under the pointer. Both are
    // springs, so an interrupted press rebounds from wherever it had got to
    // rather than restarting.
    let hover = if enabled {
        hover_fade(ui, response)
    } else {
        0.0
    };
    let held = if enabled {
        press_scale(ui, response)
    } else {
        1.0
    };
    let rect = scaled(rect, held);
    let fill = if active {
        palette().accent_soft
    } else {
        palette().surface_alt.gamma_multiply(hover)
    };
    if active || hover > 0.0 {
        painter.rect_filled(rect, CornerRadius::same(WIDGET_RADIUS), fill);
    }

    let tint = if !enabled {
        palette().text_dim.gamma_multiply(0.45)
    } else if active {
        palette().accent
    } else {
        palette().text_dim
    };
    let text = if !enabled {
        palette().text_dim.gamma_multiply(0.6)
    } else if active {
        palette().accent
    } else {
        palette().text
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

/// A small toggle: the plane picker, and anything else that is a word rather
/// than a glyph and too small to be a row.
///
/// No chrome until it is active, so a strip of them reads as one choice rather
/// than as a row of buttons.
pub(crate) fn chip(ui: &mut egui::Ui, label: &str, active: bool, enabled: bool) -> egui::Response {
    let colour = if !enabled {
        palette().text_dim.gamma_multiply(0.45)
    } else if active {
        palette().text
    } else {
        palette().text_dim
    };
    let button = egui::Button::new(egui::RichText::new(label).size(11.5).color(colour))
        .fill(if active {
            palette().surface_alt
        } else {
            Color32::TRANSPARENT
        })
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(38.0, 24.0));
    ui.add(button)
}

/// One letter of the axis legend, tinted to match the line it names.
pub(crate) fn axis_chip(ui: &mut egui::Ui, label: &str, tint: Color32) -> egui::Response {
    let button = egui::Button::new(egui::RichText::new(label).size(11.5).strong().color(tint))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(26.0, 24.0));
    ui.add(button)
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
        painter.rect_filled(
            rect,
            CornerRadius::same(WIDGET_RADIUS),
            palette().accent_soft,
        );
    } else if response.hovered() {
        painter.rect_filled(
            rect,
            CornerRadius::same(WIDGET_RADIUS),
            palette().surface_alt,
        );
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
            Stroke::new(1.0, palette().border),
        );
    }

    let tint = if selected {
        palette().accent
    } else {
        palette().text_dim
    };
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
        if selected {
            palette().accent
        } else {
            palette().text
        },
    );
    x += width + 6.0;

    if let Some(note) = note {
        painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            note,
            egui::FontId::proportional(10.5),
            palette().text_dim,
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
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(title).size(12.5).family(semibold()));
            if let Some(shortcut) = shortcut {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(shortcut)
                        .size(11.0)
                        .color(palette().text_dim)
                        .background_color(palette().surface_alt),
                );
            }
        });
        if !body.is_empty() {
            ui.label(
                egui::RichText::new(body)
                    .size(11.5)
                    .color(palette().text_dim),
            );
        }
    })
}

/// The frame a context menu is drawn in.
pub(crate) fn menu() -> egui::Frame {
    egui::Frame::new()
        .fill(palette().surface)
        .stroke(Stroke::new(1.0, palette().border))
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
            palette().danger.gamma_multiply(0.08)
        } else {
            palette().surface_alt
        };
        painter.rect_filled(rect, CornerRadius::same(WIDGET_RADIUS - 2), tint);
    }

    let (glyph_tint, text_tint) = match (enabled, destructive) {
        (false, _) => (
            palette().text_dim.gamma_multiply(0.45),
            palette().text_dim.gamma_multiply(0.6),
        ),
        (true, true) => (palette().danger, palette().danger),
        (true, false) => (palette().text_dim, palette().text),
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
            palette()
                .text_dim
                .gamma_multiply(if enabled { 1.0 } else { 0.5 }),
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
        palette().text,
    );
    if !note.is_empty() {
        painter.text(
            egui::pos2(rect.left() + 15.0 + width, rect.center().y),
            egui::Align2::LEFT_CENTER,
            note,
            egui::FontId::proportional(10.5),
            palette().text_dim,
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
        Stroke::new(1.0, palette().border),
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
            palette().danger.gamma_multiply(0.08),
        );
    }
    let glyph = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 17.0, rect.center().y),
        Vec2::splat(15.0),
    );
    crate::icon::draw(painter, glyph, icon, palette().danger);
    painter.text(
        egui::pos2(rect.left() + 33.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.5),
        palette().danger,
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
        crate::icon::draw(
            ui.painter(),
            rect,
            icon,
            palette().text_dim.gamma_multiply(0.55),
        );
        ui.add_space(10.0);
        ui.label(egui::RichText::new(title).size(13.0).family(semibold()));
        ui.add_space(4.0);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        // A panel is a share of the window, so this cannot insist on a width
        // the card it sits in may not have.
        ui.set_max_width(220.0_f32.min(ui.available_width()));
        ui.label(
            egui::RichText::new(hint)
                .size(11.5)
                .color(palette().text_dim),
        );
        ui.add_space(28.0);
    });
}

/// The single dark call-to-action.
pub(crate) fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let button = egui::Button::new(egui::RichText::new(text).color(palette().on_ink).size(13.0))
        .fill(palette().ink)
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
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(13.0),
        palette().on_ink,
    );
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + 38.0, 32.0),
        egui::Sense::click(),
    );
    let hover = hover_fade(ui, &response);
    let rect = scaled(rect, press_scale(ui, &response));
    let painter = ui.painter();
    // Brightening on hover and shrinking on press, rather than a border or a
    // shadow. The fill is the accent colour, so it is already the loudest thing
    // in the bar and does not need more emphasis.
    let fill = palette().ink.lerp_to_gamma(palette().text, hover * 0.12);
    painter.rect_filled(rect, CornerRadius::same(WIDGET_RADIUS), fill);
    let glyph = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 17.0, rect.center().y),
        Vec2::splat(15.0),
    );
    crate::icon::draw(painter, glyph, icon, palette().on_ink);
    let at = egui::pos2(rect.left() + 29.0, rect.center().y - galley.size().y * 0.5);
    painter.galley(at, galley, palette().on_ink);
    response
}

/// A segmented control. Returns true if the choice changed.
///
/// Each entry is a label and the tooltip that explains it.
/// Space between a segmented control's track and the segments inside it.
const SEGMENT_INSET: f32 = 3.0;

/// The size of a track holding `count` segments.
fn track_size(segment: Vec2, count: usize) -> Vec2 {
    Vec2::new(
        segment.x * count as f32 + SEGMENT_INSET * 2.0,
        segment.y + SEGMENT_INSET * 2.0,
    )
}

/// Where segment `i` sits inside a track.
///
/// Separated out because it is the only part of a segmented control with an
/// answer that can be checked: the segments have to tile the track exactly, and
/// the indicator has to land on one of them.
fn segment_slot(track: egui::Rect, segment: Vec2, i: usize) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(
            track.left() + SEGMENT_INSET + segment.x * i as f32,
            track.top() + SEGMENT_INSET,
        ),
        segment,
    )
}

/// One choice in a segmented control.
///
/// A glyph, a label, or both. The label is always present even when it is not
/// drawn, because it is what the tooltip says and what a reader of this code
/// needs to know which segment is which.
pub(crate) struct Segment<'a> {
    pub glyph: Option<crate::icon::Icon>,
    pub label: &'a str,
    pub help: &'a str,
}

/// A segmented control whose selection slides between segments.
///
/// The sliding indicator is the point. Swapping a fill from one segment to
/// another is a state change; moving it is the same information plus where it
/// came from, which is what makes the control feel like an object rather than a
/// set of independent buttons.
///
/// The whole strip is allocated as one rect and the segments are measured inside
/// it. Laying each segment out as its own widget would work, but then their
/// positions are only known after the fact and there is nothing to animate
/// between. It also sidesteps layout direction entirely: the top bar lays its
/// right-hand group out right to left, and a nested horizontal inherits that,
/// which silently reverses a row of independently placed widgets.
fn segmented_core(
    ui: &mut egui::Ui,
    id_source: &str,
    current: usize,
    segments: &[Segment<'_>],
    segment_size: Vec2,
) -> Option<usize> {
    let count = segments.len();
    if count == 0 {
        return None;
    }
    let (track, _) = ui.allocate_exact_size(track_size(segment_size, count), egui::Sense::hover());
    let id = ui.id().with(id_source);

    let painter = ui.painter();
    painter.rect_filled(
        track,
        CornerRadius::same(PILL_RADIUS + SEGMENT_INSET as u8),
        palette().surface_alt,
    );

    let slot = |i: usize| segment_slot(track, segment_size, i);

    // The indicator is animated by its left edge rather than by an index, so
    // that a click three segments away travels the whole distance instead of
    // jumping two and animating one.
    let settled = slot(current.min(count - 1));
    let x = crate::motion::animate(
        ui,
        id.with("slide"),
        settled.left(),
        crate::motion::Tuning::BOUNCY,
    );
    let indicator = egui::Rect::from_min_size(egui::pos2(x, settled.top()), segment_size);
    ui.painter().rect_filled(
        indicator,
        CornerRadius::same(PILL_RADIUS),
        palette().surface,
    );

    let mut picked = None;
    for (i, segment) in segments.iter().enumerate() {
        let rect = slot(i);
        let response = ui.interact(rect, id.with(i), egui::Sense::click());
        let active = i == current;
        // Measured from the indicator, not from the index, so a segment lights
        // up as the indicator arrives rather than the instant it is clicked.
        let nearness = 1.0 - ((indicator.left() - rect.left()).abs() / segment_size.x).min(1.0);
        let tint = palette().text_dim.lerp_to_gamma(
            palette().text,
            nearness.max(hover_fade(ui, &response) * 0.4),
        );

        if let Some(glyph) = segment.glyph {
            let box_ = egui::Rect::from_center_size(rect.center(), Vec2::splat(16.0));
            crate::icon::draw(ui.painter(), box_, glyph, tint);
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                segment.label,
                egui::FontId::new(13.0, FontFamily::Proportional),
                tint,
            );
        }
        if hint(response, segment.label, segment.help, None).clicked() && !active {
            picked = Some(i);
        }
    }
    picked
}

/// A segmented control of icons, for a setting that has to share the top bar.
pub(crate) fn icon_segmented(
    ui: &mut egui::Ui,
    current: usize,
    options: &[(crate::icon::Icon, &str, &str)],
) -> Option<usize> {
    let segments: Vec<Segment<'_>> = options
        .iter()
        .map(|(glyph, label, help)| Segment {
            glyph: Some(*glyph),
            label,
            help,
        })
        .collect();
    segmented_core(ui, "icons", current, &segments, Vec2::new(30.0, 26.0))
}

/// A segmented control of words, for the workspace tabs.
pub(crate) fn segmented(ui: &mut egui::Ui, current: &mut usize, labels: &[(&str, &str)]) -> bool {
    let segments: Vec<Segment<'_>> = labels
        .iter()
        .map(|(label, help)| Segment {
            glyph: None,
            label,
            help,
        })
        .collect();
    match segmented_core(ui, "words", *current, &segments, Vec2::new(84.0, 26.0)) {
        Some(picked) => {
            *current = picked;
            true
        }
        None => false,
    }
}

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

/// Face tints for the mark, lightest first.
///
/// Shaded from `ink`, the fill of the one primary button, rather than named
/// here: those are the two places the interface is allowed to be loud, and they
/// should be loud in the same hue.
fn mark_faces() -> [Color32; 3] {
    let ink = palette().ink;
    [
        ink.lerp_to_gamma(Color32::WHITE, 0.42),
        ink,
        ink.lerp_to_gamma(Color32::BLACK, 0.42),
    ]
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
    let [top_face, body, right_face] = mark_faces();

    // Body: the whole silhouette, every outer corner rounded.
    let silhouette = [apex, upper_r, lower_r, base, lower_l, upper_l];
    painter.add(egui::Shape::convex_polygon(
        rounded_polygon(&silhouette, &[round; 6]),
        body,
        no_edge,
    ));

    // Lit faces. The shared centre stays sharp; rounding it would open a notch
    // where the three faces meet.
    let top = [upper_l, apex, upper_r, middle];
    painter.add(egui::Shape::convex_polygon(
        rounded_polygon(&top, &[round, round, round, 0.0]),
        top_face,
        no_edge,
    ));

    let right = [middle, upper_r, lower_r, base];
    painter.add(egui::Shape::convex_polygon(
        rounded_polygon(&right, &[0.0, round, round, round]),
        right_face,
        no_edge,
    ));
}

#[cfg(test)]
mod tests {
    use super::{
        palette, scene, scheme, segment_slot, set_scheme, track_size, Scheme, DARK, LIGHT,
        SEGMENT_INSET,
    };
    use egui::Vec2;

    /// The segments have to tile the track exactly. A gap between two of them is
    /// a dead strip that swallows clicks, and an overlap gives two segments the
    /// same pixel.
    #[test]
    fn segments_tile_their_track_without_gaps_or_overlap() {
        let segment = Vec2::new(30.0, 26.0);
        for count in 1..=5 {
            let track =
                egui::Rect::from_min_size(egui::pos2(10.0, 4.0), track_size(segment, count));
            let slots: Vec<egui::Rect> = (0..count)
                .map(|i| segment_slot(track, segment, i))
                .collect();

            for pair in slots.windows(2) {
                assert!(
                    (pair[1].left() - pair[0].right()).abs() < 1.0e-4,
                    "gap or overlap between segments: {pair:?}"
                );
            }
            let first = slots.first().expect("at least one segment");
            let last = slots.last().expect("at least one segment");
            assert!((first.left() - track.left() - SEGMENT_INSET).abs() < 1.0e-4);
            assert!((track.right() - last.right() - SEGMENT_INSET).abs() < 1.0e-4);
            assert!(
                track.contains_rect(*first) && track.contains_rect(*last),
                "a segment escaped its track"
            );
        }
    }

    /// The sliding indicator is animated toward a segment's left edge, so those
    /// edges have to be ordered and evenly spaced or it would move unevenly.
    #[test]
    fn segment_edges_are_evenly_spaced() {
        let segment = Vec2::new(84.0, 26.0);
        let track = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), track_size(segment, 3));
        let lefts: Vec<f32> = (0..3)
            .map(|i| segment_slot(track, segment, i).left())
            .collect();
        assert!((lefts[1] - lefts[0] - segment.x).abs() < 1.0e-4);
        assert!((lefts[2] - lefts[1] - segment.x).abs() < 1.0e-4);
    }

    /// Rough perceptual weight, enough to tell one end of a ramp from the other.
    fn luma(c: egui::Color32) -> f32 {
        0.2126 * f32::from(c.r()) + 0.7152 * f32::from(c.g()) + 0.0722 * f32::from(c.b())
    }

    #[test]
    fn the_dark_scheme_inverts_the_light_one() {
        assert!(
            luma(DARK.canvas) < luma(LIGHT.canvas),
            "the dark canvas is not darker"
        );
        assert!(
            luma(DARK.text) > luma(LIGHT.text),
            "dark text is not lighter"
        );
        // A raised surface reads by contrast with what is behind it, which runs
        // the opposite way in each scheme.
        assert!(luma(LIGHT.surface) > luma(LIGHT.canvas));
        assert!(luma(DARK.surface) > luma(DARK.canvas));
    }

    /// The primary button fills with `ink` and writes on it with `on_ink`. If
    /// those ever land on the same side of the ramp the label disappears, which
    /// is exactly what happened the first time the dark palette was added.
    #[test]
    fn the_primary_button_label_contrasts_with_its_fill() {
        for p in [LIGHT, DARK] {
            assert!(
                (luma(p.ink) - luma(p.on_ink)).abs() > 120.0,
                "ink and on_ink are too close: {:?} on {:?}",
                p.on_ink,
                p.ink
            );
        }
    }

    /// The viewport is drawn into a linear target, so these are linear values
    /// and are much smaller than the hex colours they correspond to. A dark
    /// plate written as if it were sRGB comes out mid grey.
    #[test]
    fn the_dark_scene_is_actually_dark() {
        // Const blocks, so getting this wrong fails the build rather than
        // waiting for someone to run the tests. The values are literals in this
        // file; there is nothing to evaluate at run time.
        const {
            assert!(
                DARK.plate[0] < 0.05,
                "the dark plate is an sRGB value in a linear slot, and will \
                 render as mid grey"
            );
        }
        const { assert!(LIGHT.plate[0] > 0.5) }
        // The grid has to be visible against the plate it sits on, which runs
        // the opposite way in each scheme.
        const { assert!(DARK.grid[0] > DARK.plate[0]) }
        const { assert!(LIGHT.grid[0] < LIGHT.plate[0]) }
    }

    /// The scheme is one value for the whole process, so the tests that switch
    /// it take turns rather than reading each other's half-applied palette.
    static SCHEME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Every colour an egui widget is drawn with has to come from the scheme in
    /// force. The two outline colours were named at this call site instead, so
    /// in the dark scheme every field in the property panel was outlined in
    /// light grey the moment the pointer went near it.
    ///
    /// The rule is that an outline is a hairline: it sits between the surface it
    /// bounds and the quietest text in the palette, and never further from that
    /// surface than the quietest text is.
    #[test]
    fn widget_outlines_follow_the_scheme() {
        let _guard = SCHEME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let ctx = egui::Context::default();

        for (scheme, palette) in [(Scheme::Light, LIGHT), (Scheme::Dark, DARK)] {
            set_scheme(scheme);
            super::apply(&ctx);
            let widgets = ctx.style_of(egui::Theme::Light).visuals.widgets.clone();
            let room = (luma(palette.text_dim) - luma(palette.surface)).abs();
            for (state, style) in [
                ("inactive", &widgets.inactive),
                ("hovered", &widgets.hovered),
                ("active", &widgets.active),
            ] {
                let outline = style.bg_stroke.color;
                let reach = (luma(outline) - luma(palette.surface)).abs();
                assert!(
                    reach <= room + 0.5,
                    "the {state} outline {outline:?} is louder than the dimmest text in the \
                     {scheme:?} scheme"
                );
            }
        }
        set_scheme(Scheme::Light);
    }

    #[test]
    fn switching_scheme_switches_the_palette() {
        let _guard = SCHEME
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set_scheme(Scheme::Dark);
        assert_eq!(scheme(), Scheme::Dark);
        assert_eq!(palette().canvas, DARK.canvas);
        // A copy, not a computation: comparing the floats exactly is the point,
        // since a mismatch would mean the wrong palette rather than rounding.
        assert!(scene().plate.iter().eq(DARK.plate.iter()));

        set_scheme(Scheme::Light);
        assert_eq!(scheme(), Scheme::Light);
        assert_eq!(palette().canvas, LIGHT.canvas);
        assert!(scene().plate.iter().eq(LIGHT.plate.iter()));
    }
}
