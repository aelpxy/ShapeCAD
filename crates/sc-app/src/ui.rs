//! The application shell.
//!
//! The central region is deliberately left unpainted: the sphere-traced viewport
//! is drawn into that rectangle first and this chrome composites on top, so the
//! floating toolbar and the axis legend sit over live 3D with no copy and no
//! intermediate texture.

use crate::dialog::{Outcome, Purpose};
use crate::plane::SketchPlane;
use crate::state::{AppState, TOOL_SELECT, TOOL_SKETCH};
use crate::theme;
use egui::{Align, Layout, Margin, RichText, Vec2};
use sc_doc::Command;
// egui has its own `Vec2`; the kernel's is aliased to keep both readable.
use sc_geom::glam::{Vec2 as GVec2, Vec3};
use sc_geom::{Node, NodeId};

const WORKSPACES: [&str; 2] = ["Model", "Print"];
/// Only tools that do something. A button that does nothing is worse than
/// no button.
const TOOLS: [&str; 2] = ["Select", "Sketch"];

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

    // While a modal is up the whole window belongs to it.
    if state.browser.is_some() {
        layout.overlays.push(egui::Rect::EVERYTHING);
    }
    file_browser(ui.ctx(), state);
    layout
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
                            .color(theme::TEXT_DIM),
                    );
                });

                ui.add_space(14.0);
                if ui.button("New").clicked() {
                    state.new_document();
                }
                if ui.button("Open").clicked() {
                    state.browse(Purpose::Open);
                }
                if ui.button("Sample").clicked() {
                    state.load_sample();
                }
                let can_save = state.dirty || state.path.is_none();
                if ui
                    .add_enabled(can_save, egui::Button::new("Save"))
                    .clicked()
                {
                    state.save();
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button(ui, "Export STL").clicked() {
                        state.browse(Purpose::ExportStl);
                    }
                    ui.add_space(4.0);
                    if ui
                        .add_enabled(state.doc.can_redo(), egui::Button::new("Redo"))
                        .clicked()
                    {
                        state.redo();
                    }
                    if ui
                        .add_enabled(state.doc.can_undo(), egui::Button::new("Undo"))
                        .clicked()
                    {
                        state.undo();
                    }
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
                ui.label(
                    RichText::new(&state.status)
                        .size(11.5)
                        .color(theme::TEXT_DIM),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("mm").size(11.5).color(theme::TEXT_DIM));
                    ui.add_space(10.0);
                    if let Some(b) = state.doc.bounds() {
                        let s = b.size();
                        ui.label(
                            RichText::new(format!("{:.1} x {:.1} x {:.1}", s.x, s.y, s.z))
                                .size(11.5)
                                .color(theme::TEXT_DIM),
                        );
                        ui.add_space(10.0);
                    }
                    let how = if state.last_edit_rebuilt {
                        "rebuild"
                    } else {
                        "upload"
                    };
                    ui.label(
                        RichText::new(format!("{how} {:.1} ms", state.last_edit_ms))
                            .size(11.5)
                            .color(theme::TEXT_DIM),
                    );
                });
            });
        });
}

fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::CANVAS)
        .inner_margin(Margin::same(12))
}

fn left_panel(ui: &mut egui::Ui, state: &mut AppState) {
    egui::Panel::left("design")
        .exact_size(292.0)
        .frame(panel_frame())
        .show(ui, |ui| {
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Design").heading());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{} nodes", state.doc.arena().len()))
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                        );
                    });
                });
                ui.add_space(8.0);

                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if let Some(root) = state.doc.root() {
                            let mut seen = Vec::new();
                            tree_node(ui, state, root, 0, &mut seen);
                        } else {
                            ui.label(RichText::new("Empty document").color(theme::TEXT_DIM));
                        }
                    });
            });

            ui.add_space(12.0);

            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());
                theme::section(ui, "ADD");
                tool_grid(ui, state, true);
                ui.add_space(12.0);
                theme::section(ui, "MODIFY");
                tool_grid(ui, state, false);
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

    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 13.0);
        let name = state.doc.name(id).map(ToString::to_string);
        let title = name.clone().unwrap_or_else(|| node.kind().to_string());

        let mut text = RichText::new(title).size(13.0);
        if name.is_some() {
            text = text.strong();
        }
        let response = ui.selectable_label(selected, text);
        if response.clicked() {
            state.select(Some(id));
        }
        if name.is_some() {
            ui.label(RichText::new(node.kind()).size(10.5).color(theme::TEXT_DIM));
        }
    });

    if seen.contains(&id) {
        ui.horizontal(|ui| {
            ui.add_space((depth + 1) as f32 * 13.0);
            ui.label(RichText::new("shared").size(10.0).color(theme::TEXT_DIM));
        });
        return;
    }
    seen.push(id);

    for child in node.children() {
        tree_node(ui, state, child, depth + 1, seen);
    }
}

fn tool_grid(ui: &mut egui::Ui, state: &mut AppState, primitives: bool) {
    let gap = 6.0;
    ui.spacing_mut().item_spacing = Vec2::new(gap, gap);

    // Three columns, sized from the card's actual width. A fourth button did
    // not fit and silently clipped its label.
    let width = (ui.available_width() - gap * 2.0) / 3.0;
    let size = Vec2::new(width, 34.0);

    if primitives {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Plane").size(12.0).color(theme::TEXT_DIM));
            for plane in SketchPlane::ALL {
                let active = state.plane == plane;
                let text = RichText::new(plane.name()).size(12.5).color(if active {
                    theme::ACCENT
                } else {
                    theme::TEXT_DIM
                });
                let button = egui::Button::new(text)
                    .fill(if active {
                        theme::ACCENT_SOFT
                    } else {
                        egui::Color32::TRANSPARENT
                    })
                    .stroke(egui::Stroke::NONE)
                    .min_size(Vec2::new(40.0, 26.0));
                if ui.add(button).clicked() {
                    state.set_plane(plane);
                }
            }
        });
        ui.add_space(4.0);

        // Sketching is the main way to make something, so it gets its own row
        // rather than competing with the primitives for space.
        let full = Vec2::new(ui.available_width(), 34.0);
        if ui
            .add_sized(full, egui::Button::new("Sketch a profile"))
            .clicked()
        {
            state.start_sketch();
        }
        ui.horizontal(|ui| {
            if ui.add_sized(size, egui::Button::new("Sphere")).clicked() {
                state.add_body(Node::Sphere { radius: 10.0 }, "Sphere");
            }
            if ui.add_sized(size, egui::Button::new("Box")).clicked() {
                state.add_body(
                    Node::Box {
                        half: Vec3::splat(10.0),
                        round: 0.0,
                    },
                    "Box",
                );
            }
            if ui.add_sized(size, egui::Button::new("Cylinder")).clicked() {
                state.add_body(
                    Node::Cylinder {
                        radius: 6.0,
                        half_height: 12.0,
                        round: 0.0,
                    },
                    "Cylinder",
                );
            }
        });
    } else {
        ui.horizontal(|ui| {
            if ui.add_sized(size, egui::Button::new("Shell")).clicked() {
                state.wrap_selection(
                    |child| Node::Shell {
                        child,
                        thickness: 2.0,
                    },
                    "Shell",
                );
            }
            if ui.add_sized(size, egui::Button::new("Offset")).clicked() {
                state.wrap_selection(
                    |child| Node::Offset {
                        child,
                        distance: 1.0,
                    },
                    "Offset",
                );
            }
            if ui.add_sized(size, egui::Button::new("Move")).clicked() {
                state.wrap_selection(
                    |child| Node::Transform {
                        child,
                        xform: sc_geom::Transform::from_translation(Vec3::new(10.0, 0.0, 0.0)),
                    },
                    "Move",
                );
            }
        });
    }
}

fn right_panel(ui: &mut egui::Ui, state: &mut AppState) {
    egui::Panel::right("properties")
        .exact_size(320.0)
        .frame(panel_frame())
        .show(ui, |ui| {
            theme::card().show(ui, |ui| {
                ui.set_width(ui.available_width());

                let Some(id) = state.selected else {
                    ui.label(RichText::new("Nothing selected").color(theme::TEXT_DIM));
                    return;
                };
                let Some(node) = state.doc.arena().get(id).cloned() else {
                    ui.label(RichText::new("Selection was deleted").color(theme::TEXT_DIM));
                    return;
                };

                ui.horizontal(|ui| {
                    ui.label(RichText::new(capitalise(node.kind())).heading());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("{id}"))
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                        );
                    });
                });
                ui.add_space(10.0);

                // Driven entirely by `Node::params`, so a new node kind gets a
                // property panel without any UI code being written for it.
                let mut edit: Option<(String, f32)> = None;
                for (name, value) in node.params() {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(pretty(name))
                                .size(12.5)
                                .color(theme::TEXT_DIM),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let mut v = value;
                            let speed = if name == "smooth" { 0.05 } else { 0.1 };
                            let field = egui::DragValue::new(&mut v)
                                .speed(speed)
                                .fixed_decimals(2)
                                .suffix(unit_for(name));
                            if ui.add_sized(Vec2::new(110.0, 26.0), field).changed() {
                                edit = Some((name.to_string(), v));
                            }
                        });
                    });
                }
                if let Some((name, value)) = edit {
                    state.apply(Command::SetParam { id, name, value });
                }

                ui.add_space(12.0);

                // Filleting is a parameter on a boolean, not an operation on edges.
                // The panel says so rather than borrowing B-rep language the kernel
                // cannot honour.
                if matches!(node, Node::Union { .. } | Node::Difference { .. }) {
                    theme::section(ui, "BLEND");
                    ui.label(
                    RichText::new(
                        "The fillet radius of this joint. Applies wherever the two shapes meet.",
                    )
                    .size(11.5)
                    .color(theme::TEXT_DIM),
                );
                } else {
                    ui.label(
                        RichText::new("Select a union or difference to adjust its fillet.")
                            .size(11.5)
                            .color(theme::TEXT_DIM),
                    );
                }

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if state.doc.root() != Some(id) && ui.button("Make root").clicked() {
                        state.apply(Command::SetRoot { root: Some(id) });
                    }
                    if ui.button("Delete").clicked() {
                        state.apply(Command::Delete { id });
                        state.selected = state.doc.root();
                    }
                });
            });
        });
}

/// Chrome that floats over the 3D view.
fn overlays(
    ui: &mut egui::Ui,
    state: &mut AppState,
    viewport: egui::Rect,
    claimed: &mut Vec<egui::Rect>,
) {
    // Axis legend and Fit, top right.
    let legend = egui::Rect::from_min_size(
        egui::pos2(viewport.max.x - 152.0, viewport.min.y + 14.0),
        Vec2::new(138.0, 30.0),
    );
    claimed.push(legend);
    ui.scope_builder(egui::UiBuilder::new().max_rect(legend), |ui| {
        theme::floating().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                for (axis, colour) in [
                    ("X", egui::Color32::from_rgb(0xC7, 0x5C, 0x5C)),
                    ("Y", egui::Color32::from_rgb(0x66, 0x9E, 0x66)),
                    ("Z", egui::Color32::from_rgb(0x5C, 0x7F, 0xC7)),
                ] {
                    ui.label(RichText::new(axis).size(11.5).strong().color(colour));
                }
                ui.separator();
                if ui
                    .button(RichText::new("Fit").size(11.5))
                    .on_hover_text("Frame the model (F)")
                    .clicked()
                {
                    state.frame_model();
                }
            });
        });
    });

    // How to move around, stated rather than assumed.
    let hint = egui::Rect::from_min_size(
        egui::pos2(viewport.min.x + 16.0, viewport.min.y + 14.0),
        Vec2::new(460.0, 30.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(hint), |ui| {
        ui.label(
            RichText::new("Drag to orbit · Right-drag or Shift-drag to pan · Scroll to zoom · Double-click to focus")
                .size(10.5)
                .color(theme::TEXT_DIM),
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
                for (i, label) in TOOLS.iter().enumerate() {
                    let active = state.tool == i;
                    let text = RichText::new(*label).size(12.5).color(if active {
                        theme::ACCENT
                    } else {
                        theme::TEXT_DIM
                    });
                    let button = egui::Button::new(text)
                        .fill(if active {
                            theme::ACCENT_SOFT
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .stroke(egui::Stroke::NONE)
                        .min_size(Vec2::new(82.0, 30.0));
                    if ui.add(button).clicked() && state.tool != i {
                        state.tool = i;
                        if i == TOOL_SKETCH {
                            state.start_sketch();
                        } else {
                            state.cancel_sketch();
                            state.status = format!("{label} tool");
                        }
                    }
                }
            });
        });
    });
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
            theme::ACCENT.gamma_multiply(0.16)
        } else if hovered {
            theme::ACCENT.gamma_multiply(0.10)
        } else {
            theme::TEXT_DIM.gamma_multiply(0.05)
        };
        let edge = if active || hovered {
            theme::ACCENT
        } else {
            theme::BORDER
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
        theme::TEXT,
    );
    let rect = egui::Rect::from_center_size(at, galley.size() + Vec2::new(10.0, 6.0));
    painter.rect_filled(rect, egui::CornerRadius::same(5), theme::SURFACE);
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(5),
        egui::Stroke::new(1.0, theme::BORDER),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.center() - galley.size() * 0.5, galley, theme::TEXT);
}

/// Maps a point in the viewport to normalised device coordinates.
pub(crate) fn ndc_of(pos: egui::Pos2, viewport: egui::Rect) -> GVec2 {
    GVec2::new(
        ((pos.x - viewport.min.x) / viewport.width()) * 2.0 - 1.0,
        1.0 - ((pos.y - viewport.min.y) / viewport.height()) * 2.0,
    )
}

/// Draws the profile being sketched, and the hint telling you how to finish.
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
    let stroke = egui::Stroke::new(2.0, theme::ACCENT);
    // Sketch coordinates are in the plane's own frame, not always on XY. Lifting
    // them with `Vec3::new(p.x, p.y, 0.0)` draws a profile on XZ or YZ in
    // entirely the wrong place.
    let plane = state.plane;
    let screen: Vec<egui::Pos2> = points
        .iter()
        .filter_map(|p| to_screen(plane.to_world(*p)))
        .collect();

    for pair in screen.windows(2) {
        painter.line_segment([pair[0], pair[1]], stroke);
    }
    // Closing edge, so the enclosed area is visible before committing.
    if screen.len() >= 3 {
        painter.line_segment(
            [screen[screen.len() - 1], screen[0]],
            egui::Stroke::new(1.5, theme::ACCENT.gamma_multiply(0.5)),
        );
    }

    // Rubber band to wherever the pointer is on the plate, labelled with the
    // length it would add. A profile is only dimensioned if you can see the
    // dimension while you place it.
    if let Some(cursor) = ui.ctx().pointer_latest_pos() {
        if viewport.contains(cursor) {
            if let Some(hit) = state.camera().plane_hit(
                ndc_of(cursor, viewport),
                aspect,
                Vec3::ZERO,
                plane.normal(),
            ) {
                let snapped = state.snap(plane.to_plane(hit));
                let world = plane.to_world(snapped);
                if let Some(preview) = to_screen(world) {
                    if let Some(&last) = screen.last() {
                        painter.line_segment(
                            [last, preview],
                            egui::Stroke::new(1.5, theme::ACCENT.gamma_multiply(0.45)),
                        );
                        let length =
                            (snapped - *points.last().expect("screen is non-empty")).length();
                        label(
                            &painter,
                            last.lerp(preview, 0.5),
                            &format!("{length:.1} mm"),
                        );
                    }
                    // The snapped position itself, so a point can be placed at a
                    // known coordinate rather than wherever the pixel landed.
                    painter.circle_stroke(
                        preview,
                        3.5,
                        egui::Stroke::new(1.5, theme::ACCENT.gamma_multiply(0.7)),
                    );
                    label(
                        &painter,
                        preview + Vec2::new(12.0, 14.0),
                        &format!("{:.0}, {:.0}", snapped.x, snapped.y),
                    );
                }
            }
        }
    }

    for (i, point) in screen.iter().enumerate() {
        let first = i == 0;
        painter.circle(
            *point,
            if first { 5.0 } else { 4.0 },
            if first { theme::ACCENT } else { theme::SURFACE },
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
                    .color(theme::TEXT_DIM),
                );
            });
        });
    });
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
        "scale" | "normal_x" | "normal_y" | "normal_z" => "",
        _ => " mm",
    }
}
