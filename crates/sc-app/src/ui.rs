//! The application shell.
//!
//! The central region is deliberately left unpainted: the sphere-traced viewport
//! is drawn into that rectangle first and this chrome composites on top, so the
//! floating toolbar and the axis legend sit over live 3D with no copy and no
//! intermediate texture.

use crate::dialog::{Outcome, Purpose};
use crate::icon::Icon;
use crate::plane::SketchPlane;
use crate::settings::Appearance;
use crate::state::{AppState, Armed, MenuTarget, Placing, TOOL_SELECT, TOOL_SKETCH};
use crate::theme;
use egui::{Align, Layout, Margin, RichText, Vec2};
use sc_doc::Command;
// egui has its own `Vec2`; the kernel's is aliased to keep both readable.
use sc_geom::glam::{Vec2 as GVec2, Vec3};
use sc_geom::{Node, NodeId, Profile};

const WORKSPACES: [(&str, &str); 2] = [
    (
        "Model",
        "Build the part: sketch, pad, pocket and edit dimensions.",
    ),
    (
        "Print",
        "Checks for printability such as wall thickness and overhangs. Not built yet.",
    ),
];
/// Only tools that do something. A button that does nothing is worse than
/// no button.
const TOOLS: [(Icon, &str, &str); 2] = [
    (
        Icon::Cursor,
        "Select",
        "Click the model to select the feature under the pointer, right-click it for the actions menu. Drag to orbit, scroll to zoom.",
    ),
    (
        Icon::Pen,
        "Sketch",
        "Click points on the active plane to draw a closed profile, then press Enter to pad it. Right-click to finish, step back or cancel.",
    ),
];

/// Where the chrome ended up, so the application can tell which pointer events
/// belong to the 3D view.
#[derive(Clone, Debug)]
pub(crate) struct Chrome {
    /// The region the 3D view should fill.
    pub(crate) viewport: egui::Rect,
    /// Interactive chrome floating over the viewport. A click inside one of
    /// these belongs to the interface, not to the model.
    pub(crate) overlays: Vec<egui::Rect>,
}

impl Default for Chrome {
    fn default() -> Self {
        Self {
            viewport: egui::Rect::ZERO,
            overlays: Vec::new(),
        }
    }
}

/// Draws the whole shell and reports where everything landed.
///
/// Panels shrink the parent `Ui`, so whatever space remains afterwards is
/// exactly the region the 3D view should fill.
pub(crate) fn draw(ui: &mut egui::Ui, state: &mut AppState) -> Chrome {
    // Once a frame, before anything is laid out, so a step satisfied by the last
    // frame's input shows its next card in this one.
    if state.poll_tutorial() {
        ui.ctx().request_repaint();
    }
    shortcuts(ui.ctx(), state);
    top_bar(ui, state);
    status_bar(ui, state);
    left_panel(ui, state);
    right_panel(ui, state);

    // Claim the leftover space explicitly rather than reading the parent's
    // remaining rect: a floating layer such as the file browser perturbs that,
    // and the field would then be drawn across the whole window with the panels
    // painted over the top of it.
    let mut layout = egui::CentralPanel::no_frame()
        .show(ui, |ui| {
            let rect = ui.max_rect();
            let mut layout = Chrome {
                viewport: rect,
                overlays: Vec::new(),
            };
            datum_planes(ui, state, rect, &mut layout.overlays);
            overlays(ui, state, rect, &mut layout.overlays);
            sketch_overlay(ui, state, rect, &mut layout.overlays);
            layout
        })
        .inner;

    // The menu is drawn last so it sits over every panel, and its rect is
    // claimed so a click inside it never also reaches the model behind it.
    if let Some(rect) = context_menu(ui.ctx(), state) {
        layout.overlays.push(rect);
    }

    // While a modal is up the whole window belongs to it.
    if state.browser.is_some() {
        layout.overlays.push(egui::Rect::EVERYTHING);
    }
    file_browser(ui.ctx(), state);
    layout
}

/// Width of the context menu, in points.
const MENU_WIDTH: f32 = 218.0;

/// Draws the context menu, if one is open, and reports the rect it took.
///
/// Every item closes the menu, including the ones that do nothing, so there is
/// no way to leave it hanging over the model.
fn context_menu(ctx: &egui::Context, state: &mut AppState) -> Option<egui::Rect> {
    let grow = egui::Id::new("context-menu-grow");
    let Some(open) = state.menu else {
        // Forgotten on close, so the next menu grows again rather than appearing
        // already finished.
        crate::motion::forget(ctx, grow);
        return None;
    };
    let screen = ctx.viewport_rect();

    // Flip the menu back inside the window rather than letting it run off the
    // edge, which is where a right click near the bottom right would put it.
    let estimate = Vec2::new(MENU_WIDTH, menu_height(state, open.target));
    let mut at = egui::pos2(open.at.0, open.at.1);
    at.x =
        at.x.min(screen.right() - estimate.x - 6.0)
            .max(screen.left() + 6.0);
    at.y =
        at.y.min(screen.bottom() - estimate.y - 6.0)
            .max(screen.top() + 6.0);

    let response = egui::Area::new(egui::Id::new("context-menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            // Grows out of the corner it was summoned from rather than simply
            // being there. The corner matters: a menu that expands from the
            // pointer reads as a consequence of the click, where one that fades
            // in centred reads as a separate window that happened to appear.
            let t = crate::motion::animate_from(ui, grow, 0.0, 1.0, crate::motion::Tuning::BOUNCY);
            ui.set_opacity(t.clamp(0.0, 1.0));
            ui.ctx().set_transform_layer(
                ui.layer_id(),
                egui::emath::TSTransform::from_translation(at.to_vec2())
                    * egui::emath::TSTransform::from_scaling(0.90 + 0.10 * t)
                    * egui::emath::TSTransform::from_translation(-at.to_vec2()),
            );
            ui.set_width(MENU_WIDTH);
            theme::menu().show(ui, |ui| {
                ui.set_width(MENU_WIDTH);
                ui.spacing_mut().item_spacing.y = 1.0;
                match open.target {
                    MenuTarget::Sketch => sketch_menu(ui, state),
                    MenuTarget::Node(id) => node_menu(ui, state, id),
                    MenuTarget::Empty => empty_menu(ui, state),
                }
            });
        });

    let rect = response.response.rect;

    // Escape dismisses it, the same key that cancels everything else here.
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.close_menu();
    }

    // A primary press anywhere else dismisses it. Only the primary button is
    // checked: the secondary press is the one that opened the menu, and it
    // lands on the menu's own corner.
    let dismissed = ctx.input(|i| {
        i.pointer.button_pressed(egui::PointerButton::Primary)
            && i.pointer
                .interact_pos()
                .is_some_and(|pos| !rect.contains(pos))
    });
    if dismissed {
        state.close_menu();
    }

    Some(rect)
}

/// Roughly how tall the menu will be, used only to keep it on screen.
///
/// An estimate is enough: being a few points out shifts the menu slightly, it
/// does not clip anything, and the alternative is laying the menu out twice.
fn menu_height(state: &AppState, target: MenuTarget) -> f32 {
    let items = match target {
        MenuTarget::Sketch => 3.0,
        MenuTarget::Empty => 9.0,
        MenuTarget::Node(id) => {
            let extrude = state
                .doc
                .arena()
                .get(id)
                .is_some_and(|n| n.kind() == "extrude");
            if extrude {
                11.0
            } else {
                10.0
            }
        }
    };
    items * 27.0 + 46.0
}

/// The menu while a sketch is in progress. Nothing else is worth offering
/// until the profile is closed or abandoned.
fn sketch_menu(ui: &mut egui::Ui, state: &mut AppState) {
    let points = state.sketch.as_ref().map_or(0, Vec::len);
    theme::menu_title(ui, "Sketch", &format!("{points} points"));
    theme::menu_separator(ui);

    if theme::menu_item(
        ui,
        Icon::Pen,
        "Finish and pad",
        Some("Enter"),
        points >= 3,
        false,
    )
    .clicked()
    {
        state.finish_sketch();
        state.close_menu();
    }
    if theme::menu_item(
        ui,
        Icon::Undo,
        "Remove last point",
        Some("Backspace"),
        points > 0,
        false,
    )
    .clicked()
    {
        state.undo_sketch_point();
        state.close_menu();
    }
    if theme::menu_item(ui, Icon::Trash, "Cancel sketch", Some("Esc"), true, true).clicked() {
        state.cancel_sketch();
        state.tool = TOOL_SELECT;
        state.close_menu();
    }
}

/// The menu for a node: what can be done to the thing that was clicked.
fn node_menu(ui: &mut egui::Ui, state: &mut AppState, id: NodeId) {
    let Some(node) = state.doc.arena().get(id).cloned() else {
        state.close_menu();
        return;
    };
    let name = state
        .doc
        .name(id)
        .map_or_else(|| capitalise(node.kind()), ToString::to_string);
    theme::menu_title(ui, &name, &format!("{id}"));
    theme::menu_separator(ui);

    if node.kind() == "extrude"
        && theme::menu_item(ui, Icon::Plane, "Sketch on this face", None, true, false).clicked()
    {
        state.attach_to_selection();
        state.start_sketch();
        state.close_menu();
    }

    let cuts: [(Icon, &str, Profile); 2] = [
        (Icon::Hole, "Cut a hole", Profile::Circle { radius: 5.0 }),
        (
            Icon::Slot,
            "Cut a slot",
            Profile::Rect {
                width: 20.0,
                height: 10.0,
            },
        ),
    ];
    for (glyph, label, profile) in cuts {
        if theme::menu_item(ui, glyph, label, None, true, false).clicked() {
            state.add_pocket(profile, label);
            state.close_menu();
        }
    }

    theme::menu_separator(ui);
    if theme::menu_item(ui, Icon::Shell, "Shell", None, true, false).clicked() {
        state.wrap_selection(
            |child| Node::Shell {
                child,
                thickness: 2.0,
            },
            "Shell",
        );
        state.close_menu();
    }
    if theme::menu_item(ui, Icon::Offset, "Offset", None, true, false).clicked() {
        state.wrap_selection(
            |child| Node::Offset {
                child,
                distance: 1.0,
            },
            "Offset",
        );
        state.close_menu();
    }
    if theme::menu_item(ui, Icon::Move, "Move", None, true, false).clicked() {
        state.move_selection(Vec3::new(10.0, 0.0, 0.0));
        state.close_menu();
    }
    if state.selection_is_attached()
        && theme::menu_item(ui, Icon::Plane, "Detach from face", None, true, false).clicked()
    {
        state.detach_selection();
        state.close_menu();
    }

    theme::menu_separator(ui);
    if theme::menu_item(ui, Icon::Frame, "Frame this", Some("F"), true, false).clicked() {
        state.frame_node(id);
        state.close_menu();
    }
    let is_root = state.doc.root() == Some(id);
    if theme::menu_item(ui, Icon::Layers, "Make root", None, !is_root, false).clicked() {
        state.apply(Command::SetRoot { root: Some(id) });
        state.close_menu();
    }
    if theme::menu_item(ui, Icon::Trash, "Delete", None, true, true).clicked() {
        state.apply(Command::Delete { id });
        state.selected = state.doc.root();
        state.close_menu();
    }
}

/// The menu over empty space: start something, or move the camera.
fn empty_menu(ui: &mut egui::Ui, state: &mut AppState) {
    theme::menu_title(ui, "Viewport", state.plane.name());
    theme::menu_separator(ui);

    if theme::menu_item(ui, Icon::Pen, "Sketch a profile", None, true, false).clicked() {
        state.start_sketch();
        state.close_menu();
    }

    let pads: [(Icon, &str, Profile); 2] = [
        (
            Icon::Square,
            "Rectangle",
            Profile::Rect {
                width: 40.0,
                height: 30.0,
            },
        ),
        (Icon::Circle, "Circle", Profile::Circle { radius: 15.0 }),
    ];
    for (glyph, label, profile) in pads {
        if theme::menu_item(ui, glyph, label, None, true, false).clicked() {
            state.add_pad(profile, label);
            state.close_menu();
        }
    }
    if theme::menu_item(ui, Icon::Cube, "Box", None, true, false).clicked() {
        state.add_body(
            Node::Box {
                half: Vec3::splat(10.0),
                round: 0.0,
            },
            "Box",
        );
        state.close_menu();
    }

    theme::menu_separator(ui);
    if theme::menu_item(ui, Icon::Frame, "Fit in view", Some("F"), true, false).clicked() {
        state.frame_model();
        state.close_menu();
    }
    for (axis, _, normal, label) in AXES {
        if theme::menu_item(ui, Icon::Plane, label, Some(axis), true, false).clicked() {
            state.look_along(normal);
            state.close_menu();
        }
    }
}

/// Keyboard shortcuts, consumed before any widget sees them.
fn shortcuts(ctx: &egui::Context, state: &mut AppState) {
    use egui::{Key, KeyboardShortcut, Modifiers};

    const NEW: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::N);
    const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::O);
    const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::S);
    const SAVE_AS: KeyboardShortcut =
        KeyboardShortcut::new(Modifiers::CTRL.plus(Modifiers::SHIFT), Key::S);
    const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Z);
    const REDO: KeyboardShortcut =
        KeyboardShortcut::new(Modifiers::CTRL.plus(Modifiers::SHIFT), Key::Z);
    const ZOOM_IN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Plus);
    const ZOOM_IN_EQ: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Equals);
    const ZOOM_OUT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Minus);
    const ZOOM_RESET: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Num0);
    const FIT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::F);

    // Escape abandons a drag and puts the dimension back. Checked before
    // anything else that consumes Escape, so that a drag started by mistake can
    // always be taken back without looking for undo.
    if state.drag.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        state.cancel_drag();
        return;
    }

    // Escape also puts an armed tool away, the same key that cancels everything.
    if state.armed.is_some() && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        state.disarm();
        return;
    }

    if state.sketch.is_some() {
        let (finish, cancel, back) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::Backspace),
            )
        });
        if finish {
            state.finish_sketch();
        } else if cancel {
            state.cancel_sketch();
            state.tool = TOOL_SELECT;
        } else if back {
            state.undo_sketch_point();
        }
    }

    // Save As is checked first: Ctrl+Shift+S also matches Ctrl+S otherwise.
    ctx.input_mut(|i| {
        if i.consume_shortcut(&SAVE_AS) {
            state.browse(Purpose::SaveAs);
        } else if i.consume_shortcut(&SAVE) {
            state.save();
        } else if i.consume_shortcut(&OPEN) {
            state.browse(Purpose::Open);
        } else if i.consume_shortcut(&NEW) {
            state.new_document();
        } else if i.consume_shortcut(&REDO) {
            state.redo();
        } else if i.consume_shortcut(&UNDO) {
            state.undo();
        } else if i.consume_shortcut(&ZOOM_IN) || i.consume_shortcut(&ZOOM_IN_EQ) {
            state.set_ui_scale(state.ui_scale + 0.25);
        } else if i.consume_shortcut(&ZOOM_OUT) {
            state.set_ui_scale(state.ui_scale - 0.25);
        } else if i.consume_shortcut(&ZOOM_RESET) {
            state.set_ui_scale(1.0);
        } else if i.consume_shortcut(&FIT) {
            state.frame_model();
        }
    });
}

fn file_browser(ctx: &egui::Context, state: &mut AppState) {
    // Taken out of the state so the browser can borrow it mutably while acting
    // on the document.
    let Some(mut browser) = state.browser.take() else {
        return;
    };
    match browser.show(ctx) {
        Outcome::Pending => state.browser = Some(browser),
        Outcome::Cancelled => {}
        Outcome::Chosen(path) => state.finish_browse(browser.purpose, &path),
    }
}

fn top_bar(ui: &mut egui::Ui, state: &mut AppState) {
    egui::Panel::top("top")
        .exact_size(62.0)
        .frame(theme::bar(true))
        .show(ui, |ui| {
            let full = ui.max_rect();

            ui.horizontal_centered(|ui| {
                theme::logo(ui, 30.0);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    ui.add_space(9.0);
                    ui.label(RichText::new("ShapeCAD").size(15.5).strong());
                    ui.label(
                        RichText::new(state.title())
                            .size(10.5)
                            .color(theme::palette().text_dim),
                    );
                });

                ui.add_space(16.0);
                ui.spacing_mut().item_spacing.x = 2.0;
                let new = theme::tool_button(ui, Icon::File, "New", false, true);
                if theme::hint(
                    new,
                    "New document",
                    "Starts an empty document with the three origin planes. Unsaved changes are discarded.",
                    Some("Ctrl+N"),
                )
                .clicked()
                {
                    state.new_document();
                }

                let open = theme::tool_button(ui, Icon::Folder, "Open", false, true);
                if theme::hint(
                    open,
                    "Open document",
                    "Loads a .shape file. The command log inside it is replayed, so the whole history comes back with it.",
                    Some("Ctrl+O"),
                )
                .clicked()
                {
                    state.browse(Purpose::Open);
                }

                let sample = theme::tool_button(ui, Icon::Layers, "Sample", false, true);
                if theme::hint(
                    sample,
                    "Load the sample",
                    "A small bracket built from the same operations the tool panel offers. Handy for seeing what the property panel does.",
                    None,
                )
                .clicked()
                {
                    state.load_sample();
                }

                let can_save = state.dirty || state.path.is_none();
                let save = theme::tool_button(ui, Icon::Save, "Save", false, can_save);
                let help = if can_save {
                    "Writes the document to disk. Ctrl+Shift+S saves it somewhere new."
                } else {
                    "Nothing has changed since the last save."
                };
                if theme::hint(save, "Save", help, Some("Ctrl+S")).clicked() {
                    state.save();
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    top_bar_actions(ui, state);
                });
            });

            // Centred exactly, which a horizontal layout cannot do on its own.
            let width = 190.0;
            let centre = egui::Rect::from_center_size(full.center(), Vec2::new(width, 34.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(centre), |ui| {
                theme::segmented(ui, &mut state.tab, &WORKSPACES);
            });
        });
}

fn status_bar(ui: &mut egui::Ui, state: &AppState) {
    egui::Panel::bottom("status")
        .exact_size(30.0)
        .frame(theme::bar(false))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                let status = ui.label(
                    RichText::new(&state.status)
                        .size(11.5)
                        .color(theme::palette().text_dim),
                );
                theme::hint(
                    status,
                    "Status",
                    "The result of the last thing you did, including why an operation was refused.",
                    None,
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let unit = ui.label(RichText::new("mm").size(11.5).color(theme::palette().text_dim));
                    theme::hint(
                        unit,
                        "Millimetres",
                        "Every dimension in the document is in millimetres. There is no other unit.",
                        None,
                    );
                    ui.add_space(10.0);
                    if let Some(b) = state.doc.bounds() {
                        let s = b.size();
                        let size = ui.label(
                            RichText::new(format!("{:.1} x {:.1} x {:.1}", s.x, s.y, s.z))
                                .size(11.5)
                                .color(theme::palette().text_dim),
                        );
                        theme::hint(
                            size,
                            "Bounding box",
                            "How much space the model occupies along X, Y and Z. Compare it against your printer's build volume.",
                            None,
                        );
                        ui.add_space(10.0);
                    }
                    let (how, why) = if state.last_edit_rebuilt {
                        (
                            "rebuild",
                            "The last edit changed the shape of the model, so the shader was regenerated and recompiled.",
                        )
                    } else {
                        (
                            "upload",
                            "The last edit only changed a number, so it was uploaded to the GPU without recompiling anything.",
                        )
                    };
                    let timing = ui.label(
                        RichText::new(format!("{how} {:.1} ms", state.last_edit_ms))
                            .size(11.5)
                            .color(theme::palette().text_dim),
                    );
                    theme::hint(timing, "Last edit", why, None);
                });
            });
        });
}

fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::palette().canvas)
        .inner_margin(Margin::same(12))
}

fn left_panel(ui: &mut egui::Ui, state: &mut AppState) {
    egui::Panel::left("design")
        .exact_size(292.0)
        .frame(panel_frame())
        .show(ui, |ui| {
            // The whole column scrolls: on a short window the tool list would
            // otherwise be cut off with no way to reach it.
            let tree_height = (ui.available_height() * 0.38).clamp(120.0, 420.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let title = theme::large_title(ui, "Design");
                    theme::hint(
                        title,
                        "Design tree",
                        "Every operation in the document, newest at the top. Click a row to select it and edit its dimensions.",
                        None,
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let count = ui.label(
                            RichText::new(format!("{} nodes", state.doc.arena().len()))
                                .size(11.0)
                                .color(theme::palette().text_dim),
                        );
                        theme::hint(
                            count,
                            "Node count",
                            "How many nodes the kernel is holding, including any that are no longer reachable from the root.",
                            None,
                        );
                    });
                });
                ui.add_space(8.0);

                egui::ScrollArea::vertical()
                    .id_salt("tree")
                    .max_height(tree_height)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if let Some(root) = state.doc.root() {
                            let mut seen = Vec::new();
                            tree_node(ui, state, root, 0, &mut seen);
                        } else {
                            let empty = ui.label(
                                RichText::new("Empty document").color(theme::palette().text_dim),
                            );
                            theme::hint(
                                empty,
                                "Empty document",
                                "Pick a plane, sketch a profile, and pad it. Or start from one of the shapes below.",
                                None,
                            );
                        }
                    });
            });

            ui.add_space(12.0);

            // Deliberately not in a card. Each section below is its own raised
            // group, and a group only reads as raised against the canvas. Nested
            // inside a card they would be surface on surface and the separators
            // would be doing all the work on their own.
            tools(ui, state);
            ui.add_space(4.0);
                });
        });
}

/// One row of the design tree.
fn tree_node(
    ui: &mut egui::Ui,
    state: &mut AppState,
    id: NodeId,
    depth: usize,
    seen: &mut Vec<NodeId>,
) {
    let Some(node) = state.doc.arena().get(id).cloned() else {
        return;
    };
    let selected = state.selected == Some(id);

    let (title, subtitle) = label_for(state, id, &node);

    let row = theme::tree_row(
        ui,
        depth,
        crate::icon::for_kind(node.kind()),
        &title,
        subtitle.as_deref(),
        selected,
    );
    let row = theme::hint(row, &title, describe_node(&node), None);
    if row.clicked() {
        state.select(Some(id));
    }
    // The same menu the viewport offers, so the tree is not a second-class way
    // to reach a node.
    if row.secondary_clicked() {
        let at = row
            .interact_pointer_pos()
            .unwrap_or_else(|| row.rect.right_center());
        state.open_menu((at.x, at.y), MenuTarget::Node(id));
    }

    if seen.contains(&id) {
        ui.horizontal(|ui| {
            ui.add_space((depth + 1) as f32 * 14.0 + 8.0);
            ui.label(
                RichText::new("shared")
                    .size(10.0)
                    .color(theme::palette().text_dim),
            );
        });
        return;
    }
    seen.push(id);

    // A stack of cuts is a list, not a staircase. A boolean whose first operand
    // is another of the same kind is one more step in the same chain, so it is
    // drawn at the same depth instead of one further in: the engine example's
    // eleven holes produced eleven levels of indent, marching off the side of
    // the panel.
    //
    // The operand comes first and the rest of the chain after, so reading down
    // gives the newest cut, the thing it cut with, the one before it, and the
    // body at the bottom.
    match chained_boolean(state, &node) {
        Some((rest, operand)) => {
            tree_node(ui, state, operand, depth + 1, seen);
            tree_node(ui, state, rest, depth, seen);
        }
        None => {
            for child in node.children() {
                tree_node(ui, state, child, depth + 1, seen);
            }
        }
    }
}

/// What to call a node, and what to say under it.
///
/// One place decides. The tree and the property panel were each working it out
/// and could disagree about the same node.
///
/// An unnamed boolean is titled by what it did and subtitled by the feature it
/// did it with: "Difference" eleven times over says nothing about a part with
/// eleven holes in it, and the row below it is that feature, so the two read as
/// the different things they are. A bare placement is called what it is for
/// rather than what it is made of.
fn label_for(state: &AppState, id: NodeId, node: &Node) -> (String, Option<String>) {
    if let Some(name) = state.doc.name(id) {
        return (name.to_owned(), Some(node.kind().to_owned()));
    }
    if let (Some(verb), Some(feature)) = (operation(node), applied_feature(state, node)) {
        return (verb.to_owned(), Some(feature));
    }
    if matches!(node, Node::Transform { .. }) {
        return ("Position".to_owned(), Some(node.kind().to_owned()));
    }
    (capitalise(node.kind()), None)
}

/// What a boolean does, in a word.
///
/// The kind names are the agent-facing vocabulary and stay as they are; these
/// are for the one place a person reads them down a list.
fn operation(node: &Node) -> Option<&'static str> {
    match *node {
        Node::Union { .. } => Some("Join"),
        Node::Difference { .. } => Some("Cut"),
        Node::Intersection { .. } => Some("Keep"),
        _ => None,
    }
}

/// The `(rest, operand)` of a boolean that continues a chain of its own kind.
fn chained_boolean(state: &AppState, node: &Node) -> Option<(NodeId, NodeId)> {
    let (Node::Union { a, b, .. }
    | Node::Difference { a, b, .. }
    | Node::Intersection { a, b, .. }) = *node
    else {
        return None;
    };
    let same = state
        .doc
        .arena()
        .get(a)
        .is_some_and(|inner| inner.kind() == node.kind());
    same.then_some((a, b))
}

/// The name of the feature a boolean applied, if it has one.
///
/// Walks down the second operand through single-child wrappers, which is where
/// the name ends up: a pocket names the profile it cut with, and the placement
/// around it is unnamed plumbing.
fn applied_feature(state: &AppState, node: &Node) -> Option<String> {
    let (Node::Union { b, .. } | Node::Difference { b, .. } | Node::Intersection { b, .. }) = *node
    else {
        return None;
    };
    let mut at = b;
    for _ in 0..8 {
        if let Some(name) = state.doc.name(at) {
            return Some(name.to_owned());
        }
        let inner = state.doc.arena().get(at)?;
        let mut children = inner.children();
        let only = children.next()?;
        if children.next().is_some() {
            return None;
        }
        at = only;
    }
    None
}

/// Where the next sketch goes: a datum plane, or the face of a selected pad.
fn plane_row(ui: &mut egui::Ui, state: &mut AppState) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let (glyph, _) = ui.allocate_exact_size(Vec2::new(30.0, 24.0), egui::Sense::hover());
        crate::icon::draw(
            ui.painter(),
            egui::Rect::from_center_size(glyph.center(), Vec2::splat(15.0)),
            Icon::Plane,
            theme::palette().text_dim,
        );
        for plane in SketchPlane::ALL {
            let active = state.plane == plane && state.attached_to.is_none();
            let response = ui.add(chip(plane.name(), active, true));
            if theme::hint(
                response,
                plane.name(),
                plane.describe(),
                None,
            )
            .clicked()
            {
                state.set_plane(plane);
            }
        }

        // Attaching to a face answers the same question a datum plane does:
        // where does the next sketch go.
        let can_attach = state
            .selected
            .is_some_and(|id| state.attachable_face(id).is_some());
        let attached = state.attached_to.is_some();
        let response = ui.add(chip("Face", attached, can_attach || attached));
        let help = if can_attach || attached {
            "Puts the next sketch on the far face of the selected pad, so a hole lands on the surface you can see rather than at the origin."
        } else {
            "Select a padded body first. A face comes from the pad that made it, and the selection has to lead down to a single pad, not to both sides of a boolean."
        };
        if theme::hint(response, "Sketch on a face", help, None).clicked() {
            if attached {
                state.detach_plane();
            } else {
                state.attach_to_selection();
            }
        }
    });
}

/// The top bar's right-hand group: export, history, appearance and the guide.
///
/// Laid out right to left, which anything nested inside inherits. Split out
/// because the bar was doing two unrelated jobs in one function.
fn top_bar_actions(ui: &mut egui::Ui, state: &mut AppState) {
    let export = theme::primary_button_with_icon(ui, Icon::Download, "Export STL");
    if theme::hint(
        export,
        "Export STL",
        "Meshes the model with dual contouring and writes a binary STL ready to slice.",
        None,
    )
    .clicked()
    {
        state.browse(Purpose::ExportStl);
    }
    ui.add_space(6.0);

    appearance_switch(ui, state);
    ui.add_space(6.0);

    let running = state.tutorial.is_some();
    let help = theme::tool_button(ui, Icon::Help, "Guide", running, true);
    if theme::hint(
                    help,
                    "Guided tour",
                    "Five steps covering everything you need to make a part. It watches what you do rather than asking you to click through it.",
                    None,
                )
                .clicked()
                {
                    if running {
                        state.end_tutorial();
                    } else {
                        state.start_tutorial();
                    }
                }
    ui.add_space(10.0);

    let redo = theme::icon_button(ui, Icon::Redo, state.doc.can_redo());
    if theme::hint(
        redo,
        "Redo",
        "Replays the last undone operation.",
        Some("Ctrl+Shift+Z"),
    )
    .clicked()
    {
        state.redo();
    }
    let undo = theme::icon_button(ui, Icon::Undo, state.doc.can_undo());
    if theme::hint(
        undo,
        "Undo",
        "Steps back through the command log.",
        Some("Ctrl+Z"),
    )
    .clicked()
    {
        state.undo();
    }
}

/// Light, dark, or follow the desktop.
fn appearance_switch(ui: &mut egui::Ui, state: &mut AppState) {
    const OPTIONS: [(Icon, &str, &str); 3] = [
        (
            Icon::Monitor,
            "System",
            "Follows the desktop's own light or dark setting, and changes with it. Some Wayland compositors do not report one, in which case this is light.",
        ),
        (Icon::Sun, "Light", "The light palette, whatever the desktop is set to."),
        (Icon::Moon, "Dark", "The dark palette, whatever the desktop is set to."),
    ];

    let current = Appearance::ALL
        .iter()
        .position(|a| *a == state.settings.appearance)
        .unwrap_or(0);
    if let Some(picked) = theme::icon_segmented(ui, current, &OPTIONS) {
        state.set_appearance(Appearance::ALL[picked]);
        theme::apply(ui.ctx());
    }
}

/// A small toggle used for the plane picker and the workspace tabs.
fn chip(label: &str, active: bool, enabled: bool) -> egui::Button<'static> {
    let colour = if !enabled {
        theme::palette().text_dim.gamma_multiply(0.45)
    } else if active {
        theme::palette().text
    } else {
        theme::palette().text_dim
    };
    egui::Button::new(RichText::new(label.to_owned()).size(11.5).color(colour))
        .fill(if active {
            theme::palette().surface_alt
        } else {
            egui::Color32::TRANSPARENT
        })
        .stroke(egui::Stroke::NONE)
        .min_size(Vec2::new(38.0, 24.0))
}

/// The tool list: one ghost row per operation, grouped.
fn tools(ui: &mut egui::Ui, state: &mut AppState) {
    sketch_tools(ui, state);
    add_tools(ui, state);
    cut_tools(ui, state);
    modify_tools(ui, state);
}

/// Where the next profile is drawn, and how to start drawing it.
fn sketch_tools(ui: &mut egui::Ui, state: &mut AppState) {
    theme::section(ui, "SKETCH");
    let drawing = state.sketch.is_some();
    theme::grouped(ui, |rows| {
        let row = rows.row(Icon::Pen, "Sketch a profile", drawing, true);
        if theme::hint(
            row,
            "Sketch a profile",
            "Click points on the active plane to draw a closed outline. Enter pads it into a solid, Backspace removes the last point, Escape cancels.",
            Some("S"),
        )
        .clicked()
        {
            state.start_sketch();
        }
        rows.custom(|ui| plane_row(ui, state));
    });
}

/// Material added to the model: padded profiles and primitive bodies.
fn add_tools(ui: &mut egui::Ui, state: &mut AppState) {
    theme::section(ui, "ADD");
    let pads: [(Icon, &str, &str, Profile); 3] = [
        (
            Icon::Square,
            "Rectangle",
            "Pads a 40 by 30 mm rectangle off the active plane. Edit its width, height and depth afterwards in the property panel.",
            Profile::Rect {
                width: 40.0,
                height: 30.0,
            },
        ),
        (
            Icon::Circle,
            "Circle",
            "Pads a 15 mm radius disc off the active plane, giving a cylinder you can re-dimension later.",
            Profile::Circle { radius: 15.0 },
        ),
        (
            Icon::Hexagon,
            "Hexagon",
            "Pads a six sided prism off the active plane. The side count is a parameter, so the same node can become any regular polygon.",
            Profile::RegularPolygon {
                sides: 6,
                radius: 15.0,
            },
        ),
    ];
    theme::grouped(ui, |rows| {
        for (glyph, label, help, profile) in pads {
            let armed = state.armed.as_ref().is_some_and(|a| a.label == label);
            let row = rows.row(glyph, label, armed, true);
            if theme::hint(row, label, help, None).clicked() {
                state.arm(Armed {
                    kind: Placing::Pad,
                    profile,
                    label,
                });
            }
        }
    });
    ui.add_space(8.0);

    let bodies: [(Icon, &str, &str, Node); 3] = [
        (
            Icon::Sphere,
            "Sphere",
            "Drops a 10 mm radius ball at the origin. Unions with whatever is already in the document.",
            Node::Sphere { radius: 10.0 },
        ),
        (
            Icon::Cube,
            "Box",
            "Drops a 20 mm cube at the origin. Its round parameter turns the edges into fillets without any edge selection.",
            Node::Box {
                half: Vec3::splat(10.0),
                round: 0.0,
            },
        ),
        (
            Icon::Cylinder,
            "Cylinder",
            "Drops a 6 mm radius, 24 mm tall cylinder at the origin, standing on the Z axis.",
            Node::Cylinder {
                radius: 6.0,
                half_height: 12.0,
                round: 0.0,
            },
        ),
    ];
    theme::grouped(ui, |rows| {
        for (glyph, label, help, node) in bodies {
            let row = rows.row(glyph, label, false, true);
            if theme::hint(row, label, help, None).clicked() {
                state.add_body(node, label);
            }
        }
    });
}

/// Material removed from the model. Nothing to cut from an empty document.
fn cut_tools(ui: &mut egui::Ui, state: &mut AppState) {
    theme::section(ui, "CUT");
    let can_cut = state.doc.root().is_some();
    let cuts: [(Icon, &str, &str, Profile); 3] = [
        (
            Icon::Slot,
            "Slot",
            "Cuts a 20 by 10 mm slot straight through the model from the active plane.",
            Profile::Rect {
                width: 20.0,
                height: 10.0,
            },
        ),
        (
            Icon::Hole,
            "Hole",
            "Cuts a 10 mm bore straight through the model from the active plane. Select a pad first and pick Face to drill from its top surface.",
            Profile::Circle { radius: 5.0 },
        ),
        (
            Icon::HexHole,
            "Hex hole",
            "Cuts a hexagonal pocket through the model, sized for a captive nut.",
            Profile::RegularPolygon {
                sides: 6,
                radius: 5.0,
            },
        ),
    ];
    theme::grouped(ui, |rows| {
        for (glyph, label, help, profile) in cuts {
            let armed = state.armed.as_ref().is_some_and(|a| a.label == label);
            let row = rows.row(glyph, label, armed, can_cut);
            let help = if can_cut {
                help
            } else {
                "There is nothing to cut into yet. Add a body first."
            };
            if theme::hint(row, label, help, None).clicked() {
                state.arm(Armed {
                    kind: Placing::Pocket,
                    profile,
                    label,
                });
            }
        }
    });
}

/// Shown by every modify tool when there is nothing to apply it to.
const NO_SELECTION: &str = "Select a node in the design tree or the viewport first. This wraps the selection rather than adding beside it.";

/// Operations that wrap the selection rather than adding to it.
fn modify_tools(ui: &mut egui::Ui, state: &mut AppState) {
    theme::section(ui, "MODIFY");
    let has_selection = state.selected.is_some();
    let attached = state.selection_is_attached();

    theme::grouped(ui, |rows| {
        let row = rows.row(Icon::Shell, "Shell", false, has_selection);
        let help = if has_selection {
            "Hollows the selection out, leaving a 2 mm wall. Useful for making a printed part lighter."
        } else {
            NO_SELECTION
        };
        if theme::hint(row, "Shell", help, None).clicked() {
            state.wrap_selection(
                |child| Node::Shell {
                    child,
                    thickness: 2.0,
                },
                "Shell",
            );
        }

        let row = rows.row(Icon::Offset, "Offset", false, has_selection);
        let help = if has_selection {
            "Grows the selection outwards by 1 mm, or shrinks it with a negative distance. Rounds convex corners as it goes."
        } else {
            NO_SELECTION
        };
        if theme::hint(row, "Offset", help, None).clicked() {
            state.wrap_selection(
                |child| Node::Offset {
                    child,
                    distance: 1.0,
                },
                "Offset",
            );
        }

        let row = rows.row(Icon::Move, "Move", false, has_selection);
        let help = if !has_selection {
            NO_SELECTION
        } else if attached {
            "Slides the selection across the face it is attached to. It keeps following that face; use Detach from face to stop it."
        } else {
            "Gives the selection a placement of its own and nudges it 10 mm along X. Dragging it in the viewport does the same thing and is usually quicker; this is here for when you want to type the numbers."
        };
        if theme::hint(row, "Move", help, None).clicked() {
            state.move_selection(Vec3::new(10.0, 0.0, 0.0));
        }

        if attached {
            let row = rows.row(Icon::Plane, "Detach from face", false, true);
            if theme::hint(
                row,
                "Detach from face",
                "Stops this following the face it was built on. It stays exactly where it is now; only the link is cut.",
                None,
            )
            .clicked()
            {
                state.detach_selection();
            }
        }
    });
}

fn right_panel(ui: &mut egui::Ui, state: &mut AppState) {
    egui::Panel::right("properties")
        .exact_size(320.0)
        .frame(panel_frame())
        .show(ui, |ui| {
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());

                let Some(id) = state.selected else {
                    theme::empty_state(
                        ui,
                        Icon::Cursor,
                        "Nothing selected",
                        "Click a body in the viewport or a row in the design tree. Its dimensions appear here, and blue grips appear on the surfaces they move.",
                    );
                    return;
                };
                let Some(node) = state.doc.arena().get(id).cloned() else {
                    theme::empty_state(
                        ui,
                        Icon::Layers,
                        "Selection was deleted",
                        "That node is no longer part of the document.",
                    );
                    return;
                };

                properties(ui, state, id, &node);
            });
        });
}

/// The body of the property panel for one selected node.
/// Shown instead of coordinates on a placement that follows a face.
fn derived_note(ui: &mut egui::Ui) {
    theme::hint(
        ui.label(
            RichText::new("Positioned by the face it is attached to.")
                .size(12.0)
                .color(theme::palette().text_dim),
        ),
        "Derived placement",
        "This follows the face it was built on, so its position is recomputed whenever that face moves. Use Move to slide it across the face, or Detach from face to fix it in place.",
        None,
    );
}

/// The selected node's icon, name and id, across the top of the panel.
fn properties_header(ui: &mut egui::Ui, state: &AppState, id: NodeId, node: &Node) {
    let (title, subtitle) = label_for(state, id, node);
    ui.horizontal(|ui| {
        let (glyph, _) = ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
        crate::icon::draw(
            ui.painter(),
            glyph,
            crate::icon::for_kind(node.kind()),
            theme::palette().text,
        );
        ui.add_space(2.0);
        theme::large_title(ui, &title);
        if let Some(subtitle) = subtitle {
            ui.add_space(5.0);
            ui.label(
                RichText::new(subtitle)
                    .size(11.0)
                    .color(theme::palette().text_dim),
            );
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{id}"))
                    .size(11.0)
                    .color(theme::palette().text_dim),
            );
        });
    });
}

fn properties(ui: &mut egui::Ui, state: &mut AppState, id: NodeId, node: &Node) {
    properties_header(ui, state, id, node);

    theme::section(ui, "DIMENSIONS");

    // A derived placement is recomputed from the face it sits on, so its
    // coordinates are an output rather than an input. They are still the node's
    // data and still hashed; they are simply not something to type into, and
    // offering a box that snaps back is worse than offering none.
    if node.derived_from().is_some() {
        derived_note(ui);
        return;
    }

    // Driven entirely by `Node::params`, so a new node kind gets a property
    // panel without any UI code being written for it.
    let mut edit: Option<(String, f32)> = None;
    let mut any = false;
    for (name, value) in node.params() {
        any = true;
        ui.horizontal(|ui| {
            ui.add_space(2.0);
            ui.label(
                RichText::new(pretty(name))
                    .size(12.5)
                    .color(theme::palette().text_dim),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut v = value;
                let speed = if name == "smooth" { 0.05 } else { 0.1 };
                let field = egui::DragValue::new(&mut v)
                    .speed(speed)
                    .fixed_decimals(2)
                    .suffix(unit_for(name));
                let response = ui.add_sized(Vec2::new(112.0, 26.0), field);
                if theme::hint(response, &pretty(name), describe_param(name), None).changed() {
                    edit = Some((name.to_string(), v));
                }
            });
        });
    }
    if !any {
        ui.label(
            RichText::new("This node has no editable dimensions.")
                .size(11.5)
                .color(theme::palette().text_dim),
        );
    }
    if let Some((name, value)) = edit {
        state.apply(Command::SetParam { id, name, value });
    }

    // Filleting is a parameter on a boolean, not an operation on edges. The
    // panel says so rather than borrowing B-rep language the kernel cannot
    // honour, and stays quiet when the note would not apply.
    if matches!(node, Node::Union { .. } | Node::Difference { .. }) {
        theme::section(ui, "BLEND");
        ui.label(
            RichText::new("Smooth is the fillet radius of this joint. It applies wherever the two shapes meet.")
                .size(11.5)
                .color(theme::palette().text_dim),
        );
    }

    theme::section(ui, "NODE");
    if state.selection_is_attached() {
        let row = theme::row(ui, Icon::Plane, "Detach from face", false, true);
        if theme::hint(
            row,
            "Detach from face",
            "Stops this following the face it was built on. It stays exactly where it is now; only the link is cut.",
            None,
        )
        .clicked()
        {
            state.detach_selection();
        }
    }
    if state.doc.root() != Some(id) {
        let row = theme::row(ui, Icon::Layers, "Make root", false, true);
        if theme::hint(
            row,
            "Make root",
            "Renders this node instead of the current root. Everything above it in the tree is hidden without being deleted.",
            None,
        )
        .clicked()
        {
            state.apply(Command::SetRoot { root: Some(id) });
        }
    }
    let row = theme::danger_row(ui, Icon::Trash, "Delete");
    if theme::hint(
        row,
        "Delete",
        "Removes this node. Refused while another node still references it, so a tree can never be left with a dangling child.",
        None,
    )
    .clicked()
    {
        state.apply(Command::Delete { id });
        state.selected = state.doc.root();
    }
}

/// Chrome that floats over the 3D view.
fn overlays(
    ui: &mut egui::Ui,
    state: &mut AppState,
    viewport: egui::Rect,
    claimed: &mut Vec<egui::Rect>,
) {
    grips(ui, state, viewport);
    armed_preview(ui, state, viewport);
    tutorial_card(ui, state, viewport, claimed);

    // Axis legend and Fit, top right.
    let legend = egui::Rect::from_min_size(
        egui::pos2(viewport.max.x - 124.0, viewport.min.y + 14.0),
        Vec2::new(110.0, 30.0),
    );
    claimed.push(legend);
    ui.scope_builder(egui::UiBuilder::new().max_rect(legend), |ui| {
        theme::floating().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                // The legend doubles as the standard views. Clicking an axis
                // looks down it, which is the only reliable way back to a
                // square-on view after orbiting.
                for (axis, colour, normal, help) in AXES {
                    let response = ui.add(axis_chip(axis, colour));
                    if theme::hint(response, help, "Looks straight down this axis.", None).clicked()
                    {
                        state.look_along(normal);
                    }
                }
            });
        });
    });

    // How to move around, stated rather than assumed.
    let hint = egui::Rect::from_min_size(
        egui::pos2(viewport.min.x + 16.0, viewport.min.y + 14.0),
        Vec2::new(520.0, 30.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(hint), |ui| {
        ui.label(
            RichText::new(
                "Drag to orbit · Right-drag to pan · Scroll to zoom · Right-click for actions",
            )
            .size(10.5)
            .color(theme::palette().text_dim),
        );
    });

    // Tool bar, bottom centre.
    let bar = egui::Rect::from_center_size(
        egui::pos2(viewport.center().x, viewport.max.y - 36.0),
        Vec2::new(380.0, 44.0),
    );
    claimed.push(bar);
    ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
        theme::floating().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (i, (glyph, label, help)) in TOOLS.iter().enumerate() {
                    let active = state.tool == i;
                    let response = theme::tool_button(ui, *glyph, label, active, true);
                    if theme::hint(response, label, help, None).clicked() && state.tool != i {
                        state.tool = i;
                        if i == TOOL_SKETCH {
                            state.start_sketch();
                        } else {
                            state.cancel_sketch();
                            state.status = format!("{label} tool");
                        }
                    }
                }
                ui.add_space(4.0);
                let fit = theme::icon_button(ui, Icon::Frame, true);
                if theme::hint(
                    fit,
                    "Fit in view",
                    "Frames the whole model in the viewport. Double-clicking the model focuses on that point instead.",
                    Some("F"),
                )
                .clicked()
                {
                    state.frame_model();
                }
            });
        });
    });
}

/// The axis legend, which doubles as the standard view buttons.
const AXES: [(&str, egui::Color32, Vec3, &str); 3] = [
    (
        "X",
        egui::Color32::from_rgb(0xC7, 0x5C, 0x5C),
        Vec3::X,
        "Right view",
    ),
    (
        "Y",
        egui::Color32::from_rgb(0x66, 0x9E, 0x66),
        Vec3::Y,
        "Front view",
    ),
    (
        "Z",
        egui::Color32::from_rgb(0x5C, 0x7F, 0xC7),
        Vec3::Z,
        "Top view",
    ),
];

/// One letter of the axis legend, as a button.
fn axis_chip(axis: &str, colour: egui::Color32) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(axis.to_owned())
            .size(11.5)
            .strong()
            .color(colour),
    )
    .fill(egui::Color32::TRANSPARENT)
    .stroke(egui::Stroke::NONE)
    .min_size(Vec2::new(26.0, 24.0))
}

/// Half-width of a datum plane as drawn, in millimetres.
const PLANE_EXTENT: f32 = 45.0;

/// Draws the three origin planes and lets one be picked.
///
/// Shown while the document is empty or a sketch is in progress, which is when
/// the choice is live. Once there is a model they would only be in the way, and
/// the panel still offers them.
fn datum_planes(
    ui: &mut egui::Ui,
    state: &mut AppState,
    viewport: egui::Rect,
    claimed: &mut Vec<egui::Rect>,
) {
    if state.doc.root().is_some() && state.sketch.is_none() {
        return;
    }

    let aspect = viewport.width() / viewport.height().max(1.0);
    let camera = state.camera();
    let painter = ui.painter_at(viewport);
    let mut picked = None;

    for plane in SketchPlane::ALL {
        let corners: Option<Vec<egui::Pos2>> = [
            GVec2::new(-PLANE_EXTENT, -PLANE_EXTENT),
            GVec2::new(PLANE_EXTENT, -PLANE_EXTENT),
            GVec2::new(PLANE_EXTENT, PLANE_EXTENT),
            GVec2::new(-PLANE_EXTENT, PLANE_EXTENT),
        ]
        .iter()
        .map(|c| {
            camera.project(plane.to_world(*c), aspect).map(|ndc| {
                egui::pos2(
                    viewport.min.x + (ndc.x * 0.5 + 0.5) * viewport.width(),
                    viewport.min.y + (0.5 - ndc.y * 0.5) * viewport.height(),
                )
            })
        })
        .collect();
        let Some(corners) = corners else { continue };

        let active = state.plane == plane;
        let hovered = ui
            .ctx()
            .pointer_latest_pos()
            .is_some_and(|p| viewport.contains(p) && contains(&corners, p));

        let fill = if active {
            theme::palette().accent.gamma_multiply(0.16)
        } else if hovered {
            theme::palette().accent.gamma_multiply(0.10)
        } else {
            theme::palette().text_dim.gamma_multiply(0.05)
        };
        let edge = if active || hovered {
            theme::palette().accent
        } else {
            theme::palette().border
        };
        painter.add(egui::Shape::convex_polygon(
            corners.clone(),
            fill,
            egui::Stroke::new(if active { 2.0 } else { 1.0 }, edge),
        ));

        // Label the corner nearest the top left of its own quad.
        let anchor = corners
            .iter()
            .copied()
            .min_by(|a, b| (a.x + a.y).total_cmp(&(b.x + b.y)))
            .unwrap_or(viewport.center());
        label(&painter, anchor + Vec2::new(18.0, 12.0), plane.name());

        if hovered {
            let bbox = egui::Rect::from_points(&corners);
            claimed.push(bbox);
            let response = ui.interact(
                bbox,
                egui::Id::new(("datum", plane.name())),
                egui::Sense::click(),
            );
            if response.clicked() {
                picked = Some(plane);
            }
        }
    }

    if let Some(plane) = picked {
        state.set_plane(plane);
    }
}

/// Point in convex polygon, by consistent turn direction.
fn contains(polygon: &[egui::Pos2], point: egui::Pos2) -> bool {
    let mut positive = false;
    let mut negative = false;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        let cross = (b.x - a.x) * (point.y - a.y) - (b.y - a.y) * (point.x - a.x);
        if cross > 0.0 {
            positive = true;
        } else if cross < 0.0 {
            negative = true;
        }
        if positive && negative {
            return false;
        }
    }
    true
}

/// A small readout chip, legible over whatever the viewport is showing.
fn label(painter: &egui::Painter, at: egui::Pos2, text: &str) {
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(11.0),
        theme::palette().text,
    );
    let rect = egui::Rect::from_center_size(at, galley.size() + Vec2::new(10.0, 6.0));
    painter.rect_filled(rect, egui::CornerRadius::same(5), theme::palette().surface);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(5),
        egui::Stroke::new(1.0, theme::palette().border),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        rect.center() - galley.size() * 0.5,
        galley,
        theme::palette().text,
    );
}

/// Maps a point in the viewport to normalised device coordinates.
pub(crate) fn ndc_of(pos: egui::Pos2, viewport: egui::Rect) -> GVec2 {
    GVec2::new(
        ((pos.x - viewport.min.x) / viewport.width()) * 2.0 - 1.0,
        1.0 - ((pos.y - viewport.min.y) / viewport.height()) * 2.0,
    )
}

/// One step's card: where you are, what to do, and how far along.
fn tutorial_step_card(ui: &mut egui::Ui, step: &crate::tutorial::Step, at: usize) -> bool {
    let total = crate::tutorial::STEPS.len();
    ui.label(
        RichText::new(format!("STEP {} OF {total}", at + 1))
            .size(10.0)
            .family(theme::semibold())
            .color(theme::palette().accent),
    );
    ui.add_space(2.0);
    ui.label(RichText::new(step.title).size(15.0).strong());
    ui.add_space(4.0);
    ui.label(
        RichText::new(step.body)
            .size(12.0)
            .color(theme::palette().text_dim),
    );
    ui.add_space(8.0);

    let mut skip = false;
    ui.horizontal(|ui| {
        // A progress track rather than a Next button. There is nothing to press:
        // the step advances when the thing it asked for happens.
        for i in 0..total {
            let (dot, _) = ui.allocate_exact_size(Vec2::new(14.0, 4.0), egui::Sense::hover());
            ui.painter().rect_filled(
                dot.shrink2(Vec2::new(2.0, 0.0)),
                egui::CornerRadius::same(2),
                if i <= at {
                    theme::palette().accent
                } else {
                    theme::palette().border
                },
            );
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button(RichText::new("Skip").size(11.0)).clicked() {
                skip = true;
            }
        });
    });
    skip
}

/// The card after the last step. True if it was dismissed.
fn tutorial_closing_card(ui: &mut egui::Ui) -> bool {
    ui.label(RichText::new("That is the whole of it").size(15.0).strong());
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Everything else explains itself: rest the pointer on any control and it will tell you what it does. Export STL when the part is ready to print.",
        )
        .size(12.0)
        .color(theme::palette().text_dim),
    );
    ui.add_space(8.0);
    theme::primary_button(ui, "Done").clicked()
}

/// The tutorial card, bottom left of the viewport.
///
/// Deliberately not modal and not anchored to the control it is talking about.
/// A card that blocks the interface teaches people to dismiss it, and one that
/// points at a button teaches the button rather than the tool. This sits out of
/// the way and advances when the thing it asked for actually happens.
fn tutorial_card(
    ui: &mut egui::Ui,
    state: &mut AppState,
    viewport: egui::Rect,
    claimed: &mut Vec<egui::Rect>,
) {
    /// Wide enough for three lines of body text at this size, and tall enough
    /// for the longest of them plus the progress track.
    const WIDTH: f32 = 310.0;
    const HEIGHT: f32 = 196.0;
    /// Clear of the floating tool bar along the bottom of the viewport.
    const ABOVE_TOOLBAR: f32 = 74.0;

    let Some(tutorial) = state.tutorial else {
        return;
    };
    let card = egui::Rect::from_min_size(
        egui::pos2(
            viewport.min.x + 16.0,
            viewport.max.y - HEIGHT - ABOVE_TOOLBAR,
        ),
        Vec2::new(WIDTH, HEIGHT),
    );
    claimed.push(card);

    let mut close = false;
    let mut skip = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
        theme::floating().show(ui, |ui| {
            ui.set_width(WIDTH - 28.0);
            if let Some(step) = tutorial.step() {
                skip |= tutorial_step_card(ui, step, tutorial.at);
            } else {
                close = tutorial_closing_card(ui);
            }
        });
    });

    if skip {
        let mut t = tutorial;
        t.skip_step(state);
        state.tutorial = Some(t);
    }
    if close {
        state.end_tutorial();
    }
}

/// Draws the outline of an armed feature where it would land.
///
/// The preview is the whole point of arming rather than dropping: a hole placed
/// by eye is only better than one placed at the origin if you can see where the
/// eye is aiming before committing to it.
fn armed_preview(ui: &egui::Ui, state: &AppState, viewport: egui::Rect) {
    let Some(armed) = state.armed.as_ref() else {
        return;
    };
    let Some(cursor) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    if !viewport.contains(cursor) {
        return;
    }
    let aspect = viewport.width() / viewport.height().max(1.0);
    let Some(hit) = state.camera().plane_hit(
        ndc_of(cursor, viewport),
        aspect,
        state.plane_origin(),
        state.plane_normal(),
    ) else {
        return;
    };
    let at = state.snap(state.to_plane(hit));

    let to_screen = |world: Vec3| -> Option<egui::Pos2> {
        let ndc = state.camera().project(world, aspect)?;
        Some(egui::pos2(
            viewport.min.x + (ndc.x * 0.5 + 0.5) * viewport.width(),
            viewport.min.y + (0.5 - ndc.y * 0.5) * viewport.height(),
        ))
    };
    let outline: Vec<egui::Pos2> = armed
        .profile
        .polygon()
        .into_iter()
        .filter_map(|p| to_screen(state.to_world(at + p)))
        .collect();
    if outline.len() < 3 {
        return;
    }

    // A cut is drawn in red and an addition in the accent colour, so which of
    // the two is about to happen is visible without reading the status bar.
    let colour = if armed.kind == Placing::Pocket {
        theme::palette().danger
    } else {
        theme::palette().accent
    };
    let painter = ui.painter_at(viewport);
    painter.add(egui::Shape::convex_polygon(
        outline.clone(),
        colour.gamma_multiply(0.18),
        egui::Stroke::new(2.0, colour),
    ));

    let Some(centre) = to_screen(state.to_world(at)) else {
        return;
    };
    label(
        &painter,
        centre + Vec2::new(0.0, -20.0),
        &format!("{} at {:.0}, {:.0}", armed.label, at.x, at.y),
    );
}

/// Radius of a grip, in points. Large enough to hit without aiming, small
/// enough that a feature with three of them still looks like a feature.
const GRIP_RADIUS: f32 = 6.0;
/// How close the pointer has to be to grab one.
pub(crate) const GRIP_REACH: f32 = 11.0;

/// Draws the selection's draggable dimensions.
///
/// One dot per dimension, sitting on the surface it moves, with a stub pointing
/// the way it grows. The dot is the whole affordance: a full arrow gizmo at every
/// dimension of every feature would bury the model it is meant to be editing.
fn grips(ui: &egui::Ui, state: &AppState, viewport: egui::Rect) {
    // Nothing while sketching. The pointer means "place a point" then, and a
    // grip under it would be two meanings for one click.
    if state.sketch.is_some() || state.tool != TOOL_SELECT {
        return;
    }
    let rect = [
        viewport.min.x,
        viewport.min.y,
        viewport.width(),
        viewport.height(),
    ];
    let painter = ui.painter_at(viewport);
    let cursor = ui.ctx().pointer_latest_pos();
    let dragging = state.drag.map(|d| d.param);

    for grip in state.grips() {
        let Some((at, axis, _)) = state.grip_on_screen(&grip, rect) else {
            continue;
        };
        let at = egui::pos2(at.x, at.y);
        let held = dragging == Some(grip.param);
        let near = dragging.is_none()
            && cursor.is_some_and(|p| (p - at).length() < GRIP_REACH && viewport.contains(p));

        // Full strength even at rest. A grip drawn faintly over shaded geometry
        // is one nobody finds, and these are the only thing telling a user that
        // the model can be edited by touching it.
        let colour = theme::palette().accent;
        // The stub only appears once the grip is worth grabbing, so a selected
        // feature reads as a few dots rather than as a diagram.
        if held || near {
            let along = egui::vec2(axis.x, axis.y);
            painter.line_segment(
                [at + along * 8.0, at + along * 20.0],
                egui::Stroke::new(2.0, colour),
            );
        }
        let radius = if held || near {
            GRIP_RADIUS + 1.5
        } else {
            GRIP_RADIUS
        };
        // A dark halo under the ring, so the grip reads against a light face and
        // a shadowed one alike. The viewport is not a surface whose colour this
        // code gets to choose.
        painter.circle_filled(at, radius + 1.0, egui::Color32::from_black_alpha(40));
        painter.circle(
            at,
            radius,
            theme::palette().surface,
            egui::Stroke::new(2.5, colour),
        );

        if held || near {
            label(
                &painter,
                at + Vec2::new(14.0, -16.0),
                &format!("{} {:.2} mm", pretty(grip.param), grip.value),
            );
        }
    }
}

/// Draws the profile being sketched, and the hint telling you how to finish.
/// The line from the last placed point to the pointer, with the length it would
/// add and the coordinate it would land on.
///
/// A profile is only dimensioned if the dimension is visible while it is being
/// placed. Reading it off afterwards is measuring, not drawing.
fn rubber_band(
    ui: &egui::Ui,
    state: &AppState,
    viewport: egui::Rect,
    painter: &egui::Painter,
    screen: &[egui::Pos2],
    points: &[GVec2],
    to_screen: &impl Fn(Vec3) -> Option<egui::Pos2>,
) {
    let aspect = viewport.width() / viewport.height().max(1.0);
    let Some(cursor) = ui.ctx().pointer_latest_pos() else {
        return;
    };
    if !viewport.contains(cursor) {
        return;
    }
    let Some(hit) = state.camera().plane_hit(
        ndc_of(cursor, viewport),
        aspect,
        state.plane_origin(),
        state.plane_normal(),
    ) else {
        return;
    };
    let snapped = state.snap(state.to_plane(hit));
    let Some(preview) = to_screen(state.to_world(snapped)) else {
        return;
    };

    if let (Some(&last), Some(prev)) = (screen.last(), points.last()) {
        painter.line_segment(
            [last, preview],
            egui::Stroke::new(1.5, theme::palette().accent.gamma_multiply(0.45)),
        );
        let length = (snapped - *prev).length();
        label(painter, last.lerp(preview, 0.5), &format!("{length:.1} mm"));
    }
    // The snapped position itself, so a point can be placed at a known
    // coordinate rather than wherever the pixel happened to land.
    painter.circle_stroke(
        preview,
        3.5,
        egui::Stroke::new(1.5, theme::palette().accent.gamma_multiply(0.7)),
    );
    label(
        painter,
        preview + Vec2::new(12.0, 14.0),
        &format!("{:.0}, {:.0}", snapped.x, snapped.y),
    );
}

fn sketch_overlay(
    ui: &mut egui::Ui,
    state: &AppState,
    viewport: egui::Rect,
    claimed: &mut Vec<egui::Rect>,
) {
    let Some(points) = state.sketch.as_ref() else {
        return;
    };
    let aspect = viewport.width() / viewport.height().max(1.0);

    let to_screen = |world: Vec3| -> Option<egui::Pos2> {
        let ndc = state.camera().project(world, aspect)?;
        Some(egui::pos2(
            viewport.min.x + (ndc.x * 0.5 + 0.5) * viewport.width(),
            viewport.min.y + (0.5 - ndc.y * 0.5) * viewport.height(),
        ))
    };

    let painter = ui.painter_at(viewport);
    let stroke = egui::Stroke::new(2.0, theme::palette().accent);
    // Sketch coordinates are in the plane's own frame, not always on XY. Lifting
    // them with `Vec3::new(p.x, p.y, 0.0)` draws a profile on XZ or YZ in
    // entirely the wrong place.
    let screen: Vec<egui::Pos2> = points
        .iter()
        .filter_map(|p| to_screen(state.to_world(*p)))
        .collect();

    for pair in screen.windows(2) {
        painter.line_segment([pair[0], pair[1]], stroke);
    }
    // Closing edge, so the enclosed area is visible before committing.
    if screen.len() >= 3 {
        painter.line_segment(
            [screen[screen.len() - 1], screen[0]],
            egui::Stroke::new(1.5, theme::palette().accent.gamma_multiply(0.5)),
        );
    }

    rubber_band(ui, state, viewport, &painter, &screen, points, &to_screen);

    for (i, point) in screen.iter().enumerate() {
        let first = i == 0;
        painter.circle(
            *point,
            if first { 5.0 } else { 4.0 },
            if first {
                theme::palette().accent
            } else {
                theme::palette().surface
            },
            stroke,
        );
    }

    // Hint, anchored below the tool bar.
    let hint = egui::Rect::from_center_size(
        egui::pos2(viewport.center().x, viewport.max.y - 96.0),
        Vec2::new(430.0, 40.0),
    );
    claimed.push(hint);
    ui.scope_builder(egui::UiBuilder::new().max_rect(hint), |ui| {
        theme::floating().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} points", points.len()))
                        .size(12.0)
                        .family(theme::semibold()),
                );
                ui.label(
                    RichText::new(format!(
                        "{:.0} mm grid · Enter to extrude · Backspace undo · Esc cancel",
                        state.grid
                    ))
                    .size(11.5)
                    .color(theme::palette().text_dim),
                );
            });
        });
    });
}

/// One line on what a node contributes to the model, for its tree tooltip.
fn describe_node(node: &Node) -> &'static str {
    match node {
        Node::Sphere { .. } => "A ball. Radius is its only dimension.",
        Node::Box { .. } => "A rectangular block. Round fillets every edge at once.",
        Node::Cylinder { .. } => "A cylinder standing on the Z axis.",
        Node::Torus { .. } => {
            "A ring. The major radius is the circle it follows, the minor radius its thickness."
        }
        Node::Plane { .. } => "A half space. Everything on one side of a plane is solid.",
        Node::Prism { .. } => {
            "A profile swept without end, used to cut all the way through. It has no depth to \
             go stale, so the hole stays open however the part around it changes."
        }
        Node::Mesh { .. } => {
            "An imported mesh, resampled onto a voxel grid so it can be cut and joined like \
             anything else."
        }
        Node::Union { .. } => "Both children, joined. Smooth fillets the joint between them.",
        Node::Difference { .. } => {
            "The first child with the second cut out of it. Smooth fillets the cut."
        }
        Node::Intersection { .. } => "Only where both children overlap.",
        Node::Transform { .. } => "Moves and rotates its child without changing its shape.",
        Node::Offset { .. } => {
            "Grows its child outwards by the distance, or shrinks it if negative."
        }
        Node::Extrude { .. } => "A 2D profile swept into a solid. Depth is how far it runs.",
        Node::Shell { .. } => "Hollows its child out, leaving a wall of the given thickness.",
    }
}

/// One line on what a parameter controls, for its field tooltip.
///
/// Keyed on the kernel's own parameter names, not the prettified labels, so a
/// renamed parameter fails the coverage test rather than silently losing its
/// description.
fn describe_param(name: &str) -> &'static str {
    match name {
        "radius" => "Measured from the centre outwards.",
        "depth" => "How far the profile is swept. Drag the field or type a number.",
        "width" | "height" => "The full size of the profile, not the half size.",
        "sides" => "How many sides the regular polygon has. Three or more.",
        "thickness" => {
            "Wall thickness left behind. Keep it above two extrusion widths for a printable part."
        }
        "distance" => "Positive grows the shape, negative shrinks it.",
        "round" => "Fillet radius on the edges. Zero leaves them sharp.",
        "smooth" => "Fillet radius of this joint. Zero gives a sharp crease.",
        "half_x" | "half_y" | "half_z" => "Half the box's size along this axis.",
        "half_height" => "Half the cylinder's total height.",
        "major" => "The radius of the circle the ring follows.",
        "minor" => "The thickness of the ring itself.",
        "normal_x" | "normal_y" | "normal_z" => {
            "One component of the plane's normal. Everything behind the plane is solid."
        }
        "offset" => "How far the plane sits from the origin along its normal.",
        "x" | "y" | "z" => "Translation along this axis, from wherever the child already sits.",
        "scale" => "Uniform scale factor. One leaves the child at its authored size.",
        _ => "",
    }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| {
        f.to_uppercase().collect::<String>() + c.as_str()
    })
}

fn pretty(name: &str) -> String {
    name.replace('_', " ")
}

/// Millimetres for lengths, nothing for ratios and directions. Labelling a
/// scale factor "mm" is the kind of small lie that erodes trust in the numbers.
fn unit_for(name: &str) -> &'static str {
    match name {
        // A count, a ratio and a direction are not lengths. Everything else in
        // the kernel is a distance in millimetres.
        "sides" | "scale" | "normal_x" | "normal_y" | "normal_z" => "",
        _ => " mm",
    }
}

#[cfg(test)]
mod tests {
    use super::{describe_param, pretty, unit_for};
    use sc_geom::glam::Vec3;
    use sc_geom::{Node, Profile, Transform};

    /// One node of every kind, so the tests below cannot miss one silently.
    fn every_kind() -> Vec<Node> {
        let child = sc_geom::NodeId(0);
        vec![
            Node::Sphere { radius: 1.0 },
            Node::Box {
                half: Vec3::ONE,
                round: 0.0,
            },
            Node::Cylinder {
                radius: 1.0,
                half_height: 1.0,
                round: 0.0,
            },
            Node::Torus {
                major: 2.0,
                minor: 0.5,
            },
            Node::Plane {
                normal: Vec3::Z,
                offset: 0.0,
            },
            Node::Union {
                a: child,
                b: child,
                smooth: 0.0,
            },
            Node::Difference {
                a: child,
                b: child,
                smooth: 0.0,
            },
            Node::Intersection {
                a: child,
                b: child,
                smooth: 0.0,
            },
            Node::Transform {
                child,
                xform: Transform::IDENTITY,
                on: None,
            },
            Node::Offset {
                child,
                distance: 1.0,
            },
            Node::Extrude {
                profile: Profile::Circle { radius: 1.0 },
                depth: 1.0,
            },
            Node::Prism {
                profile: Profile::Rect {
                    width: 2.0,
                    height: 1.0,
                },
            },
            Node::mesh(
                sc_geom::AssetId(0),
                std::sync::Arc::new(sc_geom::Grid::default()),
            ),
            Node::Shell {
                child,
                thickness: 1.0,
            },
        ]
    }

    /// The list above has to be every kind, or the tests built on it check a
    /// shrinking subset while looking like they check all of it.
    #[test]
    fn every_kind_really_is_every_kind() {
        let mut kinds: Vec<&'static str> = every_kind().iter().map(Node::kind).collect();
        kinds.sort_unstable();
        assert_eq!(
            kinds,
            [
                "box",
                "cylinder",
                "difference",
                "extrude",
                "intersection",
                "mesh",
                "offset",
                "plane",
                "prism",
                "shell",
                "sphere",
                "torus",
                "transform",
                "union",
            ],
            "a node kind was added to the kernel and not to this fixture"
        );
    }

    /// Every field in the property panel carries a hover explanation. A new
    /// parameter with no description would otherwise ship as a bare number.
    #[test]
    fn every_parameter_is_described() {
        for node in every_kind() {
            for (name, _) in node.params() {
                assert!(
                    !describe_param(name).is_empty(),
                    "parameter `{name}` on a {} has no description",
                    node.kind()
                );
            }
        }
    }

    /// Parameter names reach the panel through `pretty`, which is what the
    /// tooltip title shows, so the two have to agree on the same string.
    #[test]
    fn parameter_labels_are_readable() {
        for node in every_kind() {
            for (name, _) in node.params() {
                let label = pretty(name);
                assert!(!label.is_empty(), "parameter `{name}` has an empty label");
                assert!(
                    !label.contains('_'),
                    "parameter `{name}` shows an underscore in the panel"
                );
            }
        }
    }

    /// Lengths are millimetres and counts are not. Getting this wrong labels a
    /// side count as a distance.
    #[test]
    fn only_lengths_carry_a_unit() {
        assert_eq!(unit_for("radius"), " mm");
        assert_eq!(unit_for("depth"), " mm");
        assert_eq!(unit_for("sides"), "");
        assert_eq!(unit_for("scale"), "");
    }
    /// Hovering has to actually produce a tooltip.
    ///
    /// egui gates a tooltip on the pointer having rested, and it only learns
    /// that the pointer is still by running frames in which it did not move.
    /// Those frames are the ones it asks for with a delay, so this drives the
    /// loop the way the application does, through `next_frame`. With a rule
    /// that drops delayed requests the loop reaches `Wait` and no tooltip is
    /// ever drawn, which is exactly what happened in the running application.
    #[test]
    fn a_hinted_row_shows_its_tooltip() {
        use crate::{icon::Icon, theme};
        use std::time::{Duration, Instant};

        const BODY: &str = "Click points on the active plane.";

        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 260.0));

        let mut time = 0.0_f64;
        let mut moved = false;
        let mut shown = false;

        for _ in 0..40 {
            let mut input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            // Moved once and then left alone. egui measures the delay from the
            // last movement, so repeating it would reset the timer forever.
            if !moved {
                input
                    .events
                    .push(egui::Event::PointerMoved(egui::pos2(80.0, 14.0)));
                moved = true;
            }

            let mut output = ctx.run_ui(input, |ui| {
                let row = theme::row(ui, Icon::Pen, "Sketch a profile", false, true);
                theme::hint(row, "Sketch a profile", BODY, Some("S"));
            });
            // Nothing here uploads textures, and epaint refuses to let a delta
            // be dropped unhandled.
            output.textures_delta.clear();

            if output.shapes.iter().any(|s| draws_text(&s.shape, BODY)) {
                shown = true;
                break;
            }

            let delay = output
                .viewport_output
                .values()
                .map(|v| v.repaint_delay)
                .min()
                .unwrap_or(Duration::MAX);
            match crate::next_frame(false, delay, Instant::now()) {
                crate::NextFrame::Now => time += 1.0 / 60.0,
                crate::NextFrame::At(_) => time += delay.as_secs_f64(),
                crate::NextFrame::Wait => break,
            }
        }

        assert!(
            shown,
            "the pointer rested on the row and no tooltip appeared"
        );
    }

    /// Whether a shape, or anything nested in it, draws exactly this string.
    fn draws_text(shape: &egui::Shape, text: &str) -> bool {
        match shape {
            egui::Shape::Text(t) => t.galley.text() == text,
            egui::Shape::Vec(shapes) => shapes.iter().any(|s| draws_text(s, text)),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tree_tests {
    use super::{applied_feature, chained_boolean, operation};
    use crate::state::AppState;
    use sc_doc::samples;
    use sc_geom::{Node, NodeId};

    /// Walks the tree the way `tree_node` does, returning the depth each node
    /// would be drawn at.
    fn depths(state: &AppState) -> Vec<(NodeId, usize)> {
        fn walk(state: &AppState, id: NodeId, depth: usize, out: &mut Vec<(NodeId, usize)>) {
            let Some(node) = state.doc.arena().get(id).cloned() else {
                return;
            };
            out.push((id, depth));
            match chained_boolean(state, &node) {
                Some((rest, operand)) => {
                    walk(state, operand, depth + 1, out);
                    walk(state, rest, depth, out);
                }
                None => {
                    for child in node.children() {
                        walk(state, child, depth + 1, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = state.doc.root() {
            walk(state, root, 0, &mut out);
        }
        out
    }

    /// A stack of cuts is a list, not a staircase. The engine has eleven of
    /// them, and nesting each one a level deeper marched the tree off the side
    /// of the panel.
    #[test]
    fn a_chain_of_cuts_does_not_indent_forever() {
        let mut state = AppState::new();
        state.doc = samples::engine();
        state.selected = state.doc.root();

        let deepest = depths(&state)
            .into_iter()
            .map(|(_, depth)| depth)
            .max()
            .expect("the engine has nodes");
        assert!(
            deepest <= 6,
            "the tree reaches depth {deepest}, which will not fit in the panel"
        );
    }

    /// Every node still gets exactly one row. Flattening must not drop any, or a
    /// feature becomes unreachable from the tree.
    #[test]
    fn flattening_still_shows_every_node_once() {
        let mut state = AppState::new();
        state.doc = samples::engine();

        let rows = depths(&state);
        let mut ids: Vec<NodeId> = rows.iter().map(|(id, _)| *id).collect();
        let before = ids.len();
        ids.sort_by_key(|id| id.0);
        ids.dedup();
        assert_eq!(before, ids.len(), "a node was drawn twice");

        let reachable = state
            .doc
            .arena()
            .reachable(state.doc.root().expect("rooted"))
            .len();
        assert_eq!(ids.len(), reachable, "a node was dropped from the tree");
    }

    /// A cut is followed by the thing it cut with, so reading down the panel
    /// gives the newest cut, its feature, then the one before it.
    #[test]
    fn a_cut_is_followed_by_what_it_cut_with() {
        let mut state = AppState::new();
        state.doc = samples::engine();
        let rows = depths(&state);

        let (first, depth) = rows[0];
        assert_eq!(depth, 0);
        let node = state.doc.arena().get(first).expect("there").clone();
        let (_, operand) = chained_boolean(&state, &node).expect("the root is a chain");
        assert_eq!(rows[1].0, operand, "the operand does not follow its cut");
        assert_eq!(rows[1].1, 1, "the operand is not nested under it");
    }

    /// An unnamed boolean is labelled by what it did it with, or eleven rows all
    /// read "Difference" and the panel says nothing.
    #[test]
    fn an_unnamed_cut_borrows_the_name_of_its_feature() {
        let mut state = AppState::new();
        state.doc = samples::engine();

        let named: Vec<String> = depths(&state)
            .into_iter()
            .filter_map(|(id, _)| {
                let node = state.doc.arena().get(id)?;
                (state.doc.name(id).is_none()).then(|| applied_feature(&state, node))?
            })
            .collect();
        assert!(
            named.iter().any(|n| n == "Mounting bolt"),
            "no cut found its feature name, got {named:?}"
        );
        assert!(
            named.iter().any(|n| n == "Cylinder bore"),
            "the through cut lost its name, got {named:?}"
        );
    }

    /// A boolean reads as a verb. Nothing else does.
    #[test]
    fn only_booleans_have_an_operation() {
        assert_eq!(
            operation(&Node::Difference {
                a: NodeId(0),
                b: NodeId(1),
                smooth: 0.0
            }),
            Some("Cut")
        );
        assert_eq!(
            operation(&Node::Union {
                a: NodeId(0),
                b: NodeId(1),
                smooth: 0.0
            }),
            Some("Join")
        );
        assert_eq!(operation(&Node::Sphere { radius: 1.0 }), None);
    }
}
