//! Application state.
//!
//! Holds the document, the camera and the current selection. Every edit goes
//! through [`AppState::apply`], which is the same `Document::apply` the CLI and
//! (later) the agent layer use. The UI gets no privileged path into the model.

use crate::dialog::{FileBrowser, Purpose};
use crate::plane::SketchPlane;
use crate::settings::{Appearance, Settings};
use sc_doc::{file, samples, Command, Document};
use sc_geom::glam::Vec2;
use sc_geom::glam::Vec3;
use sc_geom::{Node, NodeId, Transform};
use sc_render::{CameraRig, OrbitCamera};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What the camera frames when there is nothing in the document.
///
/// Roughly a 120mm cube about the origin, so the datum planes and a first sketch
/// are both comfortably in view.
const EMPTY_VIEW: sc_geom::Aabb = sc_geom::Aabb {
    min: Vec3::new(-60.0, -60.0, -60.0),
    max: Vec3::new(60.0, 60.0, 60.0),
};

/// Index of the pointer tool in the viewport tool bar.
pub(crate) const TOOL_SELECT: usize = 0;
/// Index of the sketch tool.
pub(crate) const TOOL_SKETCH: usize = 1;

/// What the property panel is editing.
pub(crate) type Selection = Option<NodeId>;

/// What a right click landed on, which decides what the menu offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuTarget {
    /// A node, picked in the viewport or clicked in the design tree.
    Node(NodeId),
    /// Empty space in the viewport.
    Empty,
    /// Anywhere, while a sketch is in progress. The sketch is the only thing
    /// worth offering at that point.
    Sketch,
}

/// An open context menu.
///
/// Held on the state rather than in the widget tree because the click that
/// opens it arrives from winit, one layer below egui, and because it has to
/// survive until the next frame draws it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ContextMenu {
    /// Top left corner, in interface points.
    pub at: (f32, f32),
    pub target: MenuTarget,
}

/// One of the selection's dimensions, ready to be drawn and grabbed.
///
/// In world space, unlike [`crate::handle::Handle`], which is in the node's own
/// frame. `tip` is one local unit along the direction the parameter grows, and
/// exists so that a caller can recover both the screen direction and the screen
/// scale from two projections. That is what makes the drag track the pointer
/// under perspective, and under a transform that scales.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Grip {
    pub param: &'static str,
    pub value: f32,
    pub at: Vec3,
    pub tip: Vec3,
    pub gain: f32,
}

/// Whether an armed feature adds material or takes it away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Placing {
    Pad,
    Pocket,
}

/// A feature armed and waiting for a click to say where it goes.
///
/// Every add and cut tool used to drop its feature at the plane's origin, which
/// meant every hole landed in the middle of the part and then had to be moved
/// with numbers. Arming instead, and letting the next click on the plane place
/// it, is the same operation with the position supplied by the hand that already
/// knows where it wants it.
#[derive(Clone, Debug)]
pub(crate) struct Armed {
    pub kind: Placing,
    pub profile: sc_geom::Profile,
    pub label: &'static str,
}

/// A free drag of the selection in progress.
///
/// Kept here rather than in the event loop because it owns an open undo step.
/// A step left open merges everything the user does afterwards into one entry,
/// and the winit match has several ways out of a gesture: a release, a release
/// that never arrives because the window lost focus, and a document replaced
/// from under it. They can only be made to agree if one place owns the state.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MoveDrag {
    /// The placement being written into.
    pub node: NodeId,
    /// Where the pointer first met the drag plane.
    pub grabbed: Vec3,
    /// Where the feature was then. A move is the difference between the two, so
    /// the feature travels with the pointer rather than jumping its centre to
    /// it.
    pub from: Vec3,
    /// The one direction motion is allowed in, if the drag has been constrained.
    ///
    /// Unconstrained dragging slides across a plane, which is two degrees of
    /// freedom from a pointer that only has two, so every move changes two
    /// coordinates at once whether that was wanted or not. Locking an axis is
    /// how you move something ten millimetres to the right and nowhere else.
    pub axis: Option<Vec3>,
    /// Where the feature's own faces and centre sit relative to its origin.
    ///
    /// Measured once, when the gesture starts. A move changes the origin and
    /// nothing else, so these do not change, and measuring them per frame would
    /// let a rounding wobble in the bounds make the snap targets breathe.
    pub extent: crate::snap::Extent,
    /// How far a snap line pulls from, in world units.
    ///
    /// Taken from the camera at the start of the gesture rather than per frame,
    /// so the pull does not change under a zoom made mid-drag.
    pub reach: f32,
    /// Set when the constraint has just changed, so the next pointer sample
    /// becomes the new anchor.
    ///
    /// Changing the constraint changes the plane the pointer is measured
    /// against, so the old anchor is a point on a plane that no longer exists.
    /// Re-anchoring on the next sample is what stops the feature jumping the
    /// moment a key is pressed, and it needs no pointer position here, which is
    /// what lets the key be handled where the keys are.
    pub reanchor: bool,
}

/// The three world axes, as a person names them.
pub(crate) const AXES: [(&str, Vec3); 3] = [("X", Vec3::X), ("Y", Vec3::Y), ("Z", Vec3::Z)];

/// What to call a direction.
#[must_use]
pub(crate) fn axis_name(axis: Vec3) -> &'static str {
    AXES.iter()
        .find(|(_, a)| a.dot(axis).abs() > 0.5)
        .map_or("an axis", |(name, _)| name)
}

/// One arm of the move gizmo, in world space.
///
/// Dragging an arm moves along that axis and nothing else, which is the whole
/// reason it exists: a plane drag has two degrees of freedom and a pointer has
/// two, so every free move changes two coordinates whether or not that was
/// wanted.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MoveArm {
    pub name: &'static str,
    pub axis: Vec3,
    /// The gizmo's centre, on the feature.
    pub tail: Vec3,
    /// Where the arm's grab target sits.
    pub head: Vec3,
}

/// A push/pull drag in progress.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Drag {
    pub node: NodeId,
    pub param: &'static str,
    /// The value when the drag began, so it can be put back.
    pub from: f32,
    /// Where it is now, for the readout.
    pub value: f32,
    /// Screen direction the parameter grows in. Unit length, in points.
    pub axis: Vec2,
    /// Parameter units per point of travel along `axis`.
    pub gain: f32,
    /// Pointer position when the drag began, in points.
    pub origin: Vec2,
}

pub(crate) struct AppState {
    pub doc: Document,
    /// Camera, eased toward wherever input sends it.
    pub rig: CameraRig,
    pub selected: Selection,
    /// Set when the geometry changed and the shader needs regenerating.
    pub field_dirty: bool,
    pub status: String,
    /// Milliseconds the last edit took to reach the GPU.
    pub last_edit_ms: f32,
    /// Whether that edit needed a pipeline rebuild or was only a buffer upload.
    pub last_edit_rebuilt: bool,
    /// Which viewport tool is armed.
    pub tool: usize,
    /// Profile points placed so far, in build-plate coordinates.
    ///
    /// `Some` means a sketch is in progress; the viewport draws it and the
    /// next click extends it.
    pub sketch: Option<Vec<Vec2>>,
    /// Depth the next feature will be given, in millimetres.
    pub extrude_height: f32,
    /// The datum plane the next sketch uses when nothing is attached.
    pub plane: SketchPlane,
    /// When set, the sketch plane rides on the far face of this feature.
    ///
    /// Held as a node id rather than a coordinate, so changing the feature's
    /// depth moves the plane. It needs no topological naming because ids are
    /// stable.
    ///
    /// This is the plane the *next* feature will be drawn on, and nothing more.
    /// What keeps features already built on that face attached to it is the
    /// derivation recorded in their placement, which [`Document::apply`]
    /// regenerates; this field is forgotten as soon as the plane is detached.
    pub attached_to: Option<NodeId>,
    /// Sketch points snap to this spacing, in millimetres.
    ///
    /// Without it a profile is whatever the pixel under the cursor happened to
    /// be, which is not a dimension anybody can reproduce.
    pub grid: f32,
    /// Where the document came from, if it has been saved or opened.
    pub path: Option<PathBuf>,
    /// Unsaved changes since the last save or open.
    pub dirty: bool,
    /// The file browser, when one is open.
    pub browser: Option<FileBrowser>,
    /// Interface zoom, multiplied onto the display's own scale factor.
    pub ui_scale: f32,
    /// Persisted preferences.
    pub settings: Settings,
    /// The context menu, while one is open.
    pub menu: Option<ContextMenu>,
    /// What the window system says the desktop's colour scheme is, if it says.
    pub system_scheme: Option<crate::theme::Scheme>,
    /// The dimension currently being pushed or pulled, if any.
    pub drag: Option<Drag>,
    /// The free drag of the selection in progress, if any.
    pub moving: Option<MoveDrag>,
    /// Coordinates the rest of the model offers the move in flight.
    ///
    /// Gathered once per gesture, indexed by axis. Empty when nothing is being
    /// dragged, which is what makes an ordinary edit fall back to the grid.
    pub snap_lines: [Vec<crate::snap::Line>; 3],
    /// Where a guide should be drawn to on each axis, in world coordinates.
    ///
    /// Set every time the feature is placed, so it follows the drag. `None` on
    /// an axis that only rounded to the grid: a line on screen through the whole
    /// of every drag says nothing.
    pub guides: [Option<Vec3>; 3],
    /// A number being typed during a gesture, if one is.
    pub entry: Option<crate::entry::Entry>,
    /// A feature waiting to be placed by the next click, if any.
    pub armed: Option<Armed>,
    /// The tutorial, while it is running.
    pub tutorial: Option<crate::tutorial::Tutorial>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub(crate) fn new() -> Self {
        // An empty document, as every other CAD tool opens. The datum planes
        // give the viewport something to show and somewhere to start.
        let doc = Document::new();
        Self {
            selected: None,
            doc,
            rig: CameraRig::new(OrbitCamera::framing(EMPTY_VIEW)),
            field_dirty: true,
            status: "Ready".to_string(),
            last_edit_ms: 0.0,
            last_edit_rebuilt: false,
            tool: 0,
            plane: SketchPlane::default(),
            attached_to: None,
            sketch: None,
            extrude_height: 10.0,
            grid: 1.0,
            path: None,
            dirty: false,
            browser: None,
            // Replaced once the window exists and the display can be measured.
            ui_scale: 1.0,
            settings: Settings::load(),
            menu: None,
            system_scheme: None,
            drag: None,
            moving: None,
            snap_lines: [Vec::new(), Vec::new(), Vec::new()],
            guides: [None; 3],
            entry: None,
            armed: None,
            tutorial: None,
        }
    }

    /// Opens the context menu at `at`, in interface points.
    ///
    /// Selecting the node first is deliberate: the menu acts on the selection,
    /// so what the menu does and what the panels show can never disagree.
    pub(crate) fn open_menu(&mut self, at: (f32, f32), target: MenuTarget) {
        if let MenuTarget::Node(id) = target {
            self.select(Some(id));
        }
        self.menu = Some(ContextMenu { at, target });
    }

    /// Closes the context menu, if one is open. True if there was one.
    pub(crate) fn close_menu(&mut self) -> bool {
        self.menu.take().is_some()
    }

    /// Frames one node rather than the whole model.
    pub(crate) fn frame_node(&mut self, id: NodeId) {
        if self.doc.arena().get(id).is_none() {
            return;
        }
        let framed = OrbitCamera::framing(sc_geom::bounds(self.doc.arena(), id));
        self.rig.goal.target = framed.target;
        self.rig.goal.distance = framed.distance;
        self.status = format!("Framed {id}");
    }

    /// Chooses the display to open on, and remembers it.
    pub(crate) fn set_display(&mut self, name: Option<String>) {
        self.settings.display = name;
        self.settings.save();
    }

    /// Chooses light, dark, or the desktop's own setting, and remembers it.
    ///
    /// Sets `field_dirty` so the viewport picks up the new scene colours: they
    /// live in the shader's uniform, so nothing repaints them until the frame is
    /// rebuilt.
    pub(crate) fn set_appearance(&mut self, appearance: Appearance) {
        self.settings.appearance = appearance;
        self.settings.save();
        self.apply_appearance();
    }

    /// Resolves the preference against what the window system reports and puts
    /// the resulting palette in force.
    pub(crate) fn apply_appearance(&mut self) {
        crate::theme::set_scheme(self.settings.appearance.resolve(self.system_scheme));
        self.field_dirty = true;
    }

    /// Records what the desktop says its colour scheme is.
    ///
    /// `None` where the window system declines to answer, which on some Wayland
    /// compositors it does. Following a setting nobody will state is not an
    /// error, so the preference simply resolves to light there.
    pub(crate) fn set_system_scheme(&mut self, scheme: Option<crate::theme::Scheme>) {
        if self.system_scheme == scheme {
            return;
        }
        self.system_scheme = scheme;
        self.apply_appearance();
    }

    /// Changes interface zoom and remembers the choice.
    pub(crate) fn set_ui_scale(&mut self, scale: f32) {
        let scale = (scale * 4.0).round() / 4.0;
        self.ui_scale = scale.clamp(0.75, 3.0);
        self.settings.ui_scale = Some(self.ui_scale);
        self.settings.save();
        self.status = format!("Interface scale {:.0}%", self.ui_scale * 100.0);
    }

    /// The document name for the title bar, with a marker for unsaved changes.
    pub(crate) fn title(&self) -> String {
        let name = self.path.as_ref().map_or("Untitled", |p| {
            p.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled")
        });
        if self.dirty {
            format!("{name} \u{2022}")
        } else {
            name.to_string()
        }
    }

    /// Replaces the document with an empty one.
    pub(crate) fn new_document(&mut self) {
        self.abandon_gestures();
        self.doc = Document::new();
        self.selected = None;
        self.path = None;
        self.dirty = false;
        self.field_dirty = true;
        self.status = "New document".to_string();
    }

    /// Loads the built-in reference part.
    pub(crate) fn load_sample(&mut self) {
        self.abandon_gestures();
        self.doc = samples::bracket();
        self.selected = self.doc.root();
        self.frame_camera();
        self.path = None;
        self.dirty = false;
        self.field_dirty = true;
        self.status = "Loaded sample bracket".to_string();
    }

    /// The placement that a free drag of the selection should write into,
    /// creating one if the selection does not already have one.
    ///
    /// A feature dragged twice must not leave two placements behind, so this
    /// reuses the transform it made the first time. A derived placement is left
    /// alone and its local offset used instead, which is what keeps a feature
    /// attached to its face while being slid around on it.
    fn movable(&mut self) -> Option<NodeId> {
        let target = self.selected?;
        match self.doc.arena().get(target) {
            Some(Node::Transform { on: None, .. }) => return Some(target),
            Some(Node::Transform {
                child, on: Some(_), ..
            }) => {
                let child = *child;
                if matches!(
                    self.doc.arena().get(child),
                    Some(Node::Transform { on: None, .. })
                ) {
                    return Some(child);
                }
                self.as_one_step(|s| s.slide_under(target, child, Vec3::ZERO));
                return match self.doc.arena().get(target) {
                    Some(Node::Transform { child, .. }) => Some(*child),
                    _ => None,
                };
            }
            _ => {}
        }
        // Nothing to write into yet: give it a placement of its own, once.
        self.wrap_selection(
            |child| Node::Transform {
                child,
                xform: Transform::IDENTITY,
                on: None,
            },
            "Move",
        );
        self.selected
    }

    /// Where the selection sits in the world, if it is somewhere.
    ///
    /// `placement_of` stops short of the node's own transform, because the
    /// question it answers is which frame the node sits in. A placement's own
    /// translation is exactly what the gizmo writes, so it has to be added back
    /// here: the first drag makes the placement the selection, and without this
    /// the arms would stay at the parent's origin from then on while the feature
    /// they move walked away from them.
    #[must_use]
    pub(crate) fn selection_origin(&self) -> Option<Vec3> {
        let id = self.selected?;
        let root = self.doc.root()?;
        let outer = sc_geom::pick::placement_of(self.doc.arena(), root, id)?;
        let frame = match self.doc.arena().get(id) {
            Some(Node::Transform { xform, .. }) => xform.then(&outer),
            _ => outer,
        };
        Some(frame.apply_point(Vec3::ZERO))
    }

    /// The move gizmo for the selection, or empty if there is nothing to move.
    ///
    /// The arms are sized in world units from the camera so they stay the same
    /// length on screen however far away the part is. A gizmo that shrinks with
    /// the model is one you cannot grab on a large part and one that swallows a
    /// small one.
    #[must_use]
    pub(crate) fn move_arms(&self, viewport: [f32; 4]) -> Vec<MoveArm> {
        /// Arm length, in interface points. Long enough to aim at without
        /// covering the feature it belongs to.
        const ARM: f32 = 72.0;

        if self.sketch.is_some() || self.armed.is_some() || self.tool != TOOL_SELECT {
            return Vec::new();
        }
        let Some(tail) = self.selection_origin() else {
            return Vec::new();
        };
        let height = viewport[3].max(1.0);
        let reach = self.camera().world_per_pixel(height) * ARM;
        AXES.iter()
            .map(|(name, axis)| MoveArm {
                name,
                axis: *axis,
                tail,
                head: tail + *axis * reach,
            })
            .collect()
    }

    /// Locks a free drag to one world axis, or releases it back to the plane.
    ///
    /// Takes effect from where the feature is now rather than from where the
    /// drag began, so pressing X part way through does not fling it back along
    /// the path it has already travelled.
    pub(crate) fn constrain_move(&mut self, axis: Option<Vec3>) {
        let Some(mut drag) = self.moving else {
            return;
        };
        if drag.axis == axis {
            return;
        }
        // Measured from where the feature is now, not from where the drag
        // began, so locking an axis part way through does not fling it back
        // along the path it has already travelled.
        drag.from = self.placement_of(drag.node).unwrap_or(drag.from);
        drag.axis = axis;
        drag.reanchor = true;
        self.moving = Some(drag);
        self.status = match axis {
            Some(a) => format!("Locked to {}, press it again to let go", axis_name(a)),
            None => "Free to slide".to_string(),
        };
    }

    /// Whether the move in flight is locked, and to what.
    #[must_use]
    pub(crate) fn move_axis(&self) -> Option<Vec3> {
        self.moving.and_then(|d| d.axis)
    }

    /// Where a placement currently sits, in its own parent's frame.
    fn placement_of(&self, id: NodeId) -> Option<Vec3> {
        match self.doc.arena().get(id) {
            Some(Node::Transform { xform, .. }) => Some(xform.translation),
            _ => None,
        }
    }

    /// Starts dragging the selection around, returning the node that will move.
    ///
    /// `grabbed` is where the pointer met the drag plane, which is what makes
    /// the feature travel with the pointer instead of jumping its centre there.
    /// `viewport_height` sizes the snap pull, which is a screen distance so that
    /// it feels the same at any zoom.
    pub(crate) fn begin_move(&mut self, grabbed: Vec3, viewport_height: f32) -> Option<NodeId> {
        // One gesture at a time. A second begin would open a second step that
        // only one release could ever close.
        if self.moving.is_some() {
            return None;
        }
        // Opened before the placement is made, not after. Creating one is part
        // of the same thing the user did, and leaving it outside the step means
        // undo takes back the movement and leaves the placement behind.
        self.doc.begin_step();
        let Some(node) = self.movable() else {
            self.doc.end_step();
            return None;
        };
        let from = match self.doc.arena().get(node) {
            Some(Node::Transform { xform, .. }) => xform.translation,
            _ => Vec3::ZERO,
        };
        // Gathered after `movable`, because that is what creates the placement
        // this drag writes into, and the lines have to exclude it.
        self.snap_lines = self.snap_lines_excluding(node);
        let per_pixel = self.camera().world_per_pixel(viewport_height.max(1.0));
        self.moving = Some(MoveDrag {
            node,
            grabbed,
            from,
            extent: self.extent_of(node),
            reach: crate::snap::reach(per_pixel, self.grid),
            axis: None,
            reanchor: false,
        });
        Some(node)
    }

    /// Coordinates the rest of the model offers, indexed by axis.
    ///
    /// Only leaf features contribute. A boolean's bounding box is the box around
    /// both of its operands, which is a number nobody drew and nobody wants to
    /// line anything up with, and the root's is the whole model. Lining up with
    /// a hole, a pad or a boss is what people actually mean.
    ///
    /// Measured in the frame the placement's own translation lives in, since
    /// that is what the drag writes. Where an ancestor rotates that frame, an
    /// axis-aligned box becomes a larger axis-aligned box, which is conservative
    /// rather than wrong: it is the extent the feature appears to occupy.
    #[must_use]
    fn snap_lines_excluding(&self, ignore: NodeId) -> [Vec<crate::snap::Line>; 3] {
        use crate::snap::{Edge, Line};

        let mut lines: [Vec<Line>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let arena = self.doc.arena();
        let Some(root) = self.doc.root() else {
            return lines;
        };
        let Some(parent) = sc_geom::pick::placement_of(arena, root, ignore) else {
            return lines;
        };
        let into = parent.inverse();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            // The whole subtree, not just the node: its own children move with
            // it, so they would be snapping the drag to itself.
            if id == ignore {
                continue;
            }
            let Some(node) = arena.get(id) else {
                continue;
            };
            let mut kids = node.children().peekable();
            if kids.peek().is_some() {
                stack.extend(node.children());
                continue;
            }
            let Some(to_world) = sc_geom::pick::placement_of(arena, root, id) else {
                continue;
            };
            let box3 = sc_geom::bounds(arena, id).transformed(&to_world.then(&into));
            // A half-space is unbounded and an empty box has no coordinates. A
            // line at infinity would swallow every drag that came near it.
            if box3.is_empty() || !box3.is_finite() {
                continue;
            }
            let centre = box3.center();
            for (axis, out) in lines.iter_mut().enumerate() {
                for (edge, at) in [
                    (Edge::Min, box3.min[axis]),
                    (Edge::Centre, centre[axis]),
                    (Edge::Max, box3.max[axis]),
                ] {
                    out.push(Line {
                        at,
                        edge,
                        from: centre,
                    });
                }
            }
        }
        lines
    }

    /// Where a placement's own box sits relative to the origin it is moved by.
    ///
    /// Zero offsets when the feature has no finite box, which degrades to
    /// snapping the origin alone rather than refusing to snap.
    #[must_use]
    fn extent_of(&self, node: NodeId) -> crate::snap::Extent {
        let origin = self.placement_of(node).unwrap_or(Vec3::ZERO);
        // A placement's bounds already carry its own transform, so this is
        // measured in the same frame as the translation being written.
        let box3 = sc_geom::bounds(self.doc.arena(), node);
        if box3.is_empty() || !box3.is_finite() {
            return crate::snap::Extent::default();
        }
        crate::snap::Extent {
            offsets: [box3.min - origin, box3.center() - origin, box3.max - origin],
        }
    }

    /// Slides the feature to wherever the pointer has reached on the drag plane.
    pub(crate) fn move_to_plane(&mut self, now: Vec3) {
        let Some(mut drag) = self.moving else {
            return;
        };
        // The feature can go out from under the gesture: Delete acts on the
        // selection, and the selection can change while the button is still
        // down. Ending the move is the only answer that closes the step;
        // carrying on would put a rejected command on the status bar on every
        // frame and never say why the part stopped responding.
        if !self.doc.arena().is_alive(drag.node) {
            self.finish_move();
            self.status = "That feature is gone".to_string();
            return;
        }
        // The first sample after a constraint change is the new anchor: the old
        // one was a point on a plane the drag is no longer measured against.
        if drag.reanchor {
            drag.grabbed = now;
            drag.reanchor = false;
            self.moving = Some(drag);
        }
        let mut delta = now - drag.grabbed;
        // Constrained, only the component along the axis survives. The pointer
        // still moves in two dimensions; the feature does not.
        if let Some(axis) = drag.axis {
            delta = axis * delta.dot(axis);
        }
        self.move_to(drag.node, drag.from + delta);
    }

    /// The plane a free drag slides across, as a normal.
    ///
    /// Unconstrained, the world axis most nearly facing the camera, so looking
    /// down drags across the plate and looking from the side drags up and
    /// along. Constrained, the plane that contains the locked axis and faces
    /// the camera as squarely as it can: hitting a plane the axis lies in is
    /// what keeps the projection onto that axis well conditioned, where using
    /// the view-facing plane would make a pixel of pointer travel worth metres
    /// whenever the axis pointed away.
    #[must_use]
    pub(crate) fn drag_plane(&self) -> Vec3 {
        if let Some(axis) = self.moving.and_then(|d| d.axis) {
            return self.plane_for_axis(axis);
        }
        let (_, _, view) = self.camera().basis();
        let a = view.abs();
        if a.x >= a.y && a.x >= a.z {
            Vec3::X
        } else if a.y >= a.z {
            Vec3::Y
        } else {
            Vec3::Z
        }
    }

    /// The plane containing `axis` that faces the camera most squarely.
    ///
    /// Hitting a plane the axis lies in is what keeps the projection onto that
    /// axis well conditioned. Using the view-facing plane instead would make a
    /// pixel of pointer travel worth metres whenever the axis pointed away.
    #[must_use]
    pub(crate) fn plane_for_axis(&self, axis: Vec3) -> Vec3 {
        let (_, _, view) = self.camera().basis();
        let across = view - axis * view.dot(axis);
        across.try_normalize().unwrap_or_else(|| {
            // The axis points straight at the camera, so no plane containing it
            // faces the viewer at all. Any perpendicular will do: the drag is
            // unusable at this angle whatever we pick, and orbiting fixes it.
            axis.any_orthonormal_vector()
        })
    }

    /// Moves a placement to `to`, snapped, in world coordinates.
    ///
    /// Snapping is against the other features in the model as well as the grid
    /// while a drag is in flight, and against the grid alone otherwise: an edit
    /// made from a menu was not aimed at anything, so there is nothing for it to
    /// latch onto.
    pub(crate) fn move_to(&mut self, id: NodeId, to: Vec3) {
        let (extent, reach) = match self.moving {
            Some(drag) if drag.node == id => (drag.extent, drag.reach),
            _ => (crate::snap::Extent::default(), 0.0),
        };
        let (snapped, latched) =
            crate::snap::position(to, extent, &self.snap_lines, reach, self.grid);
        self.place_at(id, snapped);

        // The guides are drawn in the world, and the snap was measured in the
        // placement's own parent frame, so they have to be carried back out.
        let out = self
            .doc
            .root()
            .and_then(|root| sc_geom::pick::placement_of(self.doc.arena(), root, id))
            .unwrap_or(Transform::IDENTITY);
        for (guide, latch) in self.guides.iter_mut().zip(latched) {
            *guide = latch.guide().map(|at| out.apply_point(at));
        }

        let mut readout = format!("{:.1}, {:.1}, {:.1} mm", snapped.x, snapped.y, snapped.z);
        // Named, because "snapped" is not information. Knowing it was centre to
        // centre is what lets you tell a wanted alignment from an accident.
        for (axis, latch) in AXES.iter().zip(latched) {
            if let Some(label) = latch.label() {
                let _ = write!(readout, " \u{b7} {} {label}", axis.0);
            }
        }
        self.status = readout;
    }

    /// Writes a placement's translation, exactly as given.
    ///
    /// The one path that does not snap, because a number that was typed is
    /// already the number that was meant. Rounding it to the grid afterwards
    /// would make typing 12.5 on a 1mm grid produce 13 and say nothing.
    pub(crate) fn place_at(&mut self, id: NodeId, to: Vec3) {
        for (name, value) in [("x", to.x), ("y", to.y), ("z", to.z)] {
            self.apply(Command::SetParam {
                id,
                name: name.to_string(),
                value,
            });
        }
    }

    /// Ends a free drag, keeping where it got to.
    ///
    /// Does nothing if no move is in flight, so a second release, or a release
    /// arriving after the gesture was abandoned, cannot close a step that
    /// something else opened.
    pub(crate) fn finish_move(&mut self) {
        if self.moving.take().is_some() {
            self.clear_snap();
            self.doc.end_step();
            self.status = "Ready".to_string();
        }
    }

    /// Drops everything that only meant something inside a gesture.
    ///
    /// The lines name nodes in this document and the guides are positions in it,
    /// so leaving either behind draws a guide to a feature nothing is being
    /// aligned with, and snaps the next drag to a model that has since changed.
    fn clear_snap(&mut self) {
        self.snap_lines = [Vec::new(), Vec::new(), Vec::new()];
        self.guides = [None; 3];
        self.entry = None;
    }

    /// Drops every pointer gesture in flight, closing the undo steps they hold.
    ///
    /// Called before the document underneath them is replaced. A drag carries a
    /// node id and the value that node started at, and an id only means
    /// something within one document: left in flight across a load, the next
    /// pointer movement drives whatever node happens to wear that id in the
    /// part that was just opened, changing a model the user has not touched
    /// with nothing on screen to say why.
    ///
    /// Nothing is put back, because there is nothing left to put it back into.
    /// The step is closed against the document that opened it, which is still
    /// this one at the point this runs.
    fn abandon_gestures(&mut self) {
        if self.drag.take().is_some() {
            self.doc.end_step();
        }
        if self.moving.take().is_some() {
            self.doc.end_step();
        }
        self.clear_snap();
        self.armed = None;
        // A profile in progress is drawn in the old plane's coordinates, and
        // `attached_to` is another id belonging to the document being replaced.
        if self.sketch.take().is_some() {
            self.tool = TOOL_SELECT;
        }
        self.attached_to = None;
    }

    /// Starts the tutorial from the beginning.
    pub(crate) fn start_tutorial(&mut self) {
        self.tutorial = Some(crate::tutorial::Tutorial::start(self));
    }

    /// Closes the tutorial and remembers not to open it again unasked.
    pub(crate) fn end_tutorial(&mut self) {
        self.tutorial = None;
        if !self.settings.tutorial_seen {
            self.settings.tutorial_seen = true;
            self.settings.save();
        }
    }

    /// Lets the tutorial look at what just happened. True if it moved on.
    ///
    /// Called once a frame rather than from each action, so that a step is
    /// satisfied by the state the user put the application in and not by the
    /// particular route they took to get there.
    pub(crate) fn poll_tutorial(&mut self) -> bool {
        let Some(mut tutorial) = self.tutorial else {
            return false;
        };
        let moved = tutorial.advance(self);
        self.tutorial = Some(tutorial);
        moved
    }

    /// Arms a feature, to be placed by the next click on the plane.
    ///
    /// Clicking the same tool again disarms it, so the tool row is a toggle and
    /// there is always a way out that does not involve knowing about Escape.
    pub(crate) fn arm(&mut self, armed: Armed) {
        if self
            .armed
            .as_ref()
            .is_some_and(|a| a.label == armed.label && a.kind == armed.kind)
        {
            self.disarm();
            return;
        }
        if self.sketch.is_some() {
            self.cancel_sketch();
        }
        let what = armed.label;
        self.armed = Some(armed);
        self.tool = TOOL_SELECT;
        self.status = format!("Click where the {} goes", what.to_lowercase());
    }

    pub(crate) fn disarm(&mut self) {
        if self.armed.take().is_some() {
            self.status = "Ready".to_string();
        }
    }

    /// Places the armed feature at a point on the active plane.
    ///
    /// `at` is in the plane's own coordinates and is snapped, so a feature
    /// placed by eye still lands on a number somebody can reproduce.
    pub(crate) fn place_armed(&mut self, at: Vec2) {
        let Some(armed) = self.armed.take() else {
            return;
        };
        let at = self.snap(at);
        match armed.kind {
            Placing::Pad => self.add_pad_at(armed.profile, armed.label, at),
            Placing::Pocket => self.add_pocket_at(armed.profile, armed.label, at),
        }
    }

    /// The selection's draggable dimensions, in world space.
    ///
    /// Empty when nothing is selected, when the selected node has no dimension
    /// with a direction, or when it is not reachable from the root, since a node
    /// that is not in the model has nowhere to put a grip.
    #[must_use]
    pub(crate) fn grips(&self) -> Vec<Grip> {
        let Some(id) = self.selected else {
            return Vec::new();
        };
        let Some(root) = self.doc.root() else {
            return Vec::new();
        };
        let Some(placement) = sc_geom::pick::placement_of(self.doc.arena(), root, id) else {
            return Vec::new();
        };
        let Some(node) = self.doc.arena().get(id) else {
            return Vec::new();
        };
        let values = node.params();
        crate::handle::handles(self.doc.arena(), id)
            .into_iter()
            .filter_map(|h| {
                let value = values.iter().find(|(n, _)| *n == h.param)?.1;
                Some(Grip {
                    param: h.param,
                    value,
                    at: placement.apply_point(h.at),
                    tip: placement.apply_point(h.at + h.along),
                    gain: h.gain,
                })
            })
            .collect()
    }

    /// Where a point in the model lands on screen, in interface points.
    ///
    /// One definition, shared by everything that draws into the viewport and
    /// everything that hit-tests against it. Two would drift, and the symptom
    /// would be a handle that cannot be grabbed where it appears.
    #[must_use]
    pub(crate) fn world_to_screen(&self, world: Vec3, viewport: [f32; 4]) -> Option<Vec2> {
        let [left, top, width, height] = viewport;
        let ndc = self.camera().project(world, width / height.max(1.0))?;
        Some(Vec2::new(
            left + (ndc.x * 0.5 + 0.5) * width,
            top + (0.5 - ndc.y * 0.5) * height,
        ))
    }

    /// Where a grip is on screen, and how fast its parameter moves there.
    ///
    /// `None` when the grip is behind the camera, or when its direction is so
    /// close to head-on that a pixel of pointer travel would be worth metres.
    /// Refusing a grip in that state is what stops a drag from flying off: the
    /// user can orbit a little and grab it from a workable angle.
    #[must_use]
    pub(crate) fn grip_on_screen(
        &self,
        grip: &Grip,
        viewport: [f32; 4],
    ) -> Option<(Vec2, Vec2, f32)> {
        /// Screen points a grip's direction must cover per local unit before it
        /// is considered grabbable.
        const MIN_FORESHORTENING: f32 = 2.0;

        let at = self.world_to_screen(grip.at, viewport)?;
        let tip = self.world_to_screen(grip.tip, viewport)?;
        let span = tip - at;
        let points = span.length();
        if points < MIN_FORESHORTENING {
            return None;
        }
        // Parameter units per point: the gain per local unit, divided by how
        // many points a local unit currently covers. Perspective and any scale
        // in the placement are both already in that number.
        Some((at, span / points, grip.gain / points))
    }

    /// Starts pushing or pulling one dimension.
    ///
    /// Opens an undo step that stays open for the whole gesture, so the hundreds
    /// of parameter changes a drag makes take one press of undo to take back.
    /// Every path out of a drag closes it again.
    pub(crate) fn begin_drag(&mut self, drag: Drag) {
        if self.drag.is_some() {
            return;
        }
        self.doc.begin_step();
        self.drag = Some(drag);
    }

    /// Moves the dimension to wherever the pointer has got to.
    pub(crate) fn drag_to(&mut self, pointer: Vec2) {
        let Some(mut drag) = self.drag else {
            return;
        };
        let travel = (pointer - drag.origin).dot(drag.axis);
        let value = drag.from + travel * drag.gain;

        // A drag that would make the node invalid stops at the limit instead of
        // being refused. A radius cannot pass through zero, and reporting that
        // as an error on every frame of a drag would be noise rather than
        // information.
        let Some(node) = self.doc.arena().get(drag.node) else {
            return;
        };
        let mut probe = node.clone();
        if !probe.set_param(drag.param, value) || !probe.is_valid() {
            return;
        }

        self.apply(Command::SetParam {
            id: drag.node,
            name: drag.param.to_string(),
            value,
        });
        drag.value = value;
        self.drag = Some(drag);
        self.status = format!("{} {value:.2} mm", drag.param);
    }

    /// Ends the drag, keeping where it got to.
    pub(crate) fn finish_drag(&mut self) {
        if self.drag.take().is_some() {
            self.entry = None;
            self.doc.end_step();
            self.status = "Ready".to_string();
        }
    }

    /// Takes one typed character into the number being entered.
    ///
    /// Returns whether it was used, so a key that means something else is left
    /// for whatever else is listening rather than swallowed.
    pub(crate) fn type_number(&mut self, c: char) -> bool {
        if self.drag.is_none() && self.moving.is_none() {
            return false;
        }
        let mut entry = self.entry.clone().unwrap_or_default();
        if !entry.push(c) {
            return false;
        }
        self.status = self.entry_readout(&entry);
        self.entry = Some(entry);
        true
    }

    /// Removes the last character typed, ending the entry when it empties.
    pub(crate) fn entry_backspace(&mut self) {
        let Some(mut entry) = self.entry.take() else {
            return;
        };
        if entry.backspace() {
            self.status = self.entry_readout(&entry);
            self.entry = Some(entry);
        } else {
            self.status = "Typing cancelled, keep dragging".to_string();
        }
    }

    /// Abandons the number, leaving the gesture in flight.
    ///
    /// Escape means "not that" rather than "not any of this": the drag is still
    /// wanted, it was only the number that was wrong.
    pub(crate) fn cancel_entry(&mut self) {
        if self.entry.take().is_some() {
            self.status = "Typing cancelled, keep dragging".to_string();
        }
    }

    /// What the status bar says while a number is being typed.
    fn entry_readout(&self, entry: &crate::entry::Entry) -> String {
        let typed = entry.text();
        if let Some(drag) = self.drag {
            return format!("{} = {typed}, Enter to apply", drag.param);
        }
        match self.move_axis() {
            Some(axis) => format!("{typed} mm along {}, Enter to apply", axis_name(axis)),
            None => format!("{typed} mm, press X, Y or Z to say which way"),
        }
    }

    /// Applies the typed number and ends the gesture it belonged to.
    ///
    /// A dimension takes it as the dimension, because "twelve" means a radius of
    /// twelve. A move takes it as a distance along the locked axis, because
    /// "twelve" means twelve millimetres that way, not twelve from the origin.
    /// Both are what the word means in that context, and the readout says which
    /// is being read while it is still being typed.
    pub(crate) fn commit_entry(&mut self) {
        let Some(entry) = self.entry.clone() else {
            return;
        };
        let Some(value) = entry.value() else {
            self.status = format!("{} is not a number", entry.text());
            return;
        };
        if let Some(drag) = self.drag {
            let Some(node) = self.doc.arena().get(drag.node) else {
                self.finish_drag();
                return;
            };
            // Refused rather than clamped. A drag stops at the limit because it
            // is a continuous gesture passing through; a typed number is a
            // statement, and silently applying a different one is worse than
            // saying no.
            let mut probe = node.clone();
            if !probe.set_param(drag.param, value) || !probe.is_valid() {
                self.status = format!("{} cannot be {value}", drag.param);
                return;
            }
            self.apply(Command::SetParam {
                id: drag.node,
                name: drag.param.to_string(),
                value,
            });
            self.finish_drag();
            return;
        }
        let Some(drag) = self.moving else {
            return;
        };
        let Some(axis) = drag.axis.or_else(|| self.travelled_axis(&drag)) else {
            self.status = "Press X, Y or Z to say which way first".to_string();
            return;
        };
        self.place_at(drag.node, drag.from + axis * value);
        self.finish_move();
        self.status = format!("Moved {value} mm along {}", axis_name(axis));
    }

    /// The axis a free drag has mostly travelled along, if it has travelled.
    ///
    /// A typed number needs a direction. Locking one is the explicit way to say
    /// it, but somebody who has already dragged a hand's width to the right has
    /// said it too, and making them press X as well would be pedantry. Refusing
    /// when nothing has moved is not: there is genuinely no answer then.
    fn travelled_axis(&self, drag: &MoveDrag) -> Option<Vec3> {
        /// How far the drag must have gone before its direction is taken as
        /// meant rather than as a wobble.
        const MEANT: f32 = 0.5;

        let now = self.placement_of(drag.node)?;
        let delta = now - drag.from;
        let (name, axis) = AXES
            .iter()
            .max_by(|a, b| delta.dot(a.1).abs().total_cmp(&delta.dot(b.1).abs()))?;
        let _ = name;
        (delta.dot(*axis).abs() >= self.grid.max(0.01) * MEANT).then_some(*axis)
    }

    /// Ends the drag and puts the dimension back where it started.
    pub(crate) fn cancel_drag(&mut self) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        self.apply(Command::SetParam {
            id: drag.node,
            name: drag.param.to_string(),
            value: drag.from,
        });
        self.entry = None;
        self.doc.end_step();
        self.status = "Cancelled".to_string();
    }

    /// Loads the engine example: a deeper model than the bracket, for seeing how
    /// the tree and the property panel behave on something with real depth.
    pub(crate) fn load_engine(&mut self) {
        self.abandon_gestures();
        self.doc = samples::engine();
        self.selected = self.doc.root();
        self.frame_camera();
        self.path = None;
        self.dirty = false;
        self.field_dirty = true;
        self.status = "Loaded the engine example".to_string();
    }

    /// Points the camera down `normal` without moving what it is looking at.
    ///
    /// The rig smooths towards the new orientation rather than cutting to it,
    /// so a standard view still reads as the same model from a new angle.
    pub(crate) fn look_along(&mut self, normal: sc_geom::glam::Vec3) {
        self.rig.goal.look_along(normal);
    }

    /// The camera the viewport should draw this frame.
    #[must_use]
    pub(crate) fn camera(&self) -> OrbitCamera {
        self.rig.current
    }

    /// Frames the whole model, easing rather than cutting.
    pub(crate) fn frame_model(&mut self) {
        {
            let framed = OrbitCamera::framing(self.doc.bounds().unwrap_or(EMPTY_VIEW));
            // Keep the current orientation; only the target and distance move,
            // so framing does not disorient by spinning the part as well.
            self.rig.goal.target = framed.target;
            self.rig.goal.distance = framed.distance;
            self.status = "Framed the model".to_string();
        }
    }

    fn frame_camera(&mut self) {
        let bounds = self.doc.bounds().unwrap_or(EMPTY_VIEW);
        self.rig.snap_to(OrbitCamera::framing(bounds));
    }

    /// The world point under a viewport position.
    ///
    /// Traces the model first so the wheel zooms toward whatever is actually
    /// beneath the pointer, falling back to the build plate and then to the
    /// orbit target. Sphere tracing a single ray on the CPU is cheap; it is the
    /// per-pixel case that needs a GPU.
    #[must_use]
    pub(crate) fn pick_world(&self, ndc: sc_geom::glam::Vec2, aspect: f32) -> Vec3 {
        let cam = self.camera();
        self.trace(ndc, aspect)
            .or_else(|| cam.plate_hit(ndc, aspect))
            .unwrap_or(cam.target)
    }

    /// Sphere-traces the model, returning the surface point if the ray hits it.
    ///
    /// Unlike [`AppState::pick_world`] this reports a miss, which is what
    /// selection needs: clicking empty space should deselect rather than pick
    /// whatever happens to be near the build plate.
    #[must_use]
    pub(crate) fn trace(&self, ndc: sc_geom::glam::Vec2, aspect: f32) -> Option<Vec3> {
        let root = self.doc.root()?;
        let cam = self.camera();
        let (origin, dir) = cam.ray(ndc, aspect);

        let epsilon = (cam.distance * 1.0e-4).max(1.0e-4);
        let mut travelled = 0.0f32;
        for _ in 0..192 {
            let point = origin + dir * travelled;
            let d = sc_geom::eval(self.doc.arena(), root, point);
            if d < epsilon {
                return Some(point);
            }
            travelled += d.max(epsilon);
            if travelled > cam.far() {
                return None;
            }
        }
        None
    }

    /// Selects whatever lies under a viewport position, or clears the selection.
    ///
    /// `tolerance` should be a few pixels' worth of world units at that depth.
    /// What is under the pointer, without selecting it.
    ///
    /// Shared with `select_at` so that "is the press on the selection" and "what
    /// would this click select" can never give different answers.
    #[must_use]
    pub(crate) fn hit_at(&self, ndc: Vec2, aspect: f32, tolerance: f32) -> Option<NodeId> {
        let root = self.doc.root()?;
        let point = self.trace(ndc, aspect)?;
        sc_geom::pick(self.doc.arena(), root, point, tolerance).map(sc_geom::Hit::node)
    }

    pub(crate) fn select_at(&mut self, ndc: sc_geom::glam::Vec2, aspect: f32, tolerance: f32) {
        let Some(root) = self.doc.root() else { return };
        let Some(point) = self.trace(ndc, aspect) else {
            self.select(None);
            self.status = "Nothing there".to_string();
            return;
        };

        let Some(hit) = sc_geom::pick(self.doc.arena(), root, point, tolerance) else {
            self.select(None);
            self.status = "Nothing there".to_string();
            return;
        };

        let id = hit.node();
        self.select(Some(id));
        let kind = self.doc.arena().get(id).map_or("node", sc_geom::Node::kind);
        self.status = match hit {
            sc_geom::Hit::Seam { .. } => {
                format!("Selected the {kind} at this joint, drag its blend to fillet")
            }
            sc_geom::Hit::Face(_) => format!("Selected {kind} {id}"),
        };
    }

    pub(crate) fn open_path(&mut self, path: &Path) {
        match file::open(path) {
            Ok(doc) => {
                self.abandon_gestures();
                self.doc = doc;
                self.selected = self.doc.root();
                self.frame_camera();
                self.path = Some(path.to_path_buf());
                self.dirty = false;
                self.field_dirty = true;
                self.status = format!("Opened {}", path.display());
            }
            Err(e) => self.status = format!("Could not open {}: {e}", path.display()),
        }
    }

    pub(crate) fn save_to(&mut self, path: &Path) {
        match file::save(&self.doc, path) {
            Ok(()) => {
                self.path = Some(path.to_path_buf());
                self.dirty = false;
                self.status = format!("Saved {}", path.display());
            }
            Err(e) => self.status = format!("Could not save {}: {e}", path.display()),
        }
    }

    /// Saves in place, or asks where to put it if the document has no path yet.
    pub(crate) fn save(&mut self) {
        if let Some(path) = self.path.clone() {
            self.save_to(&path);
        } else {
            self.browse(Purpose::SaveAs);
        }
    }

    /// Opens the file browser, starting wherever the document lives.
    pub(crate) fn browse(&mut self, purpose: Purpose) {
        let start = self
            .path
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("untitled")
            .to_string();
        let filename = match purpose {
            Purpose::Open => String::new(),
            Purpose::SaveAs => format!("{stem}.{}", sc_doc::file::EXTENSION),
            Purpose::ExportStl => format!("{stem}.stl"),
        };
        self.browser = Some(FileBrowser::new(purpose, &start, filename));
    }

    /// Applies whatever the browser returned.
    pub(crate) fn finish_browse(&mut self, purpose: Purpose, path: &Path) {
        match purpose {
            Purpose::Open => self.open_path(path),
            Purpose::SaveAs => self.save_to(path),
            Purpose::ExportStl => self.export_stl(path, 128),
        }
    }

    /// Applies a command, reporting failures to the status bar rather than
    /// panicking. A rejected command leaves the document untouched.
    pub(crate) fn apply(&mut self, command: Command) -> Option<NodeId> {
        match self.doc.apply(command) {
            Ok(id) => {
                self.field_dirty = true;
                self.dirty = true;
                // A delete can take the selection with it. A selection on a
                // dead id is a property panel editing nothing and a grip
                // hanging in space over the gap where the feature was, so the
                // check belongs on every applied command rather than only on
                // undo. Ids are stable, so liveness is the whole of it.
                self.prune_selection();
                self.status = "Ready".to_string();
                id
            }
            Err(e) => {
                self.status = e.to_string();
                None
            }
        }
    }

    /// Runs `f` as a single undo step, however many commands it applies.
    ///
    /// Nearly every tool is several commands underneath. A pad is an extrusion,
    /// a placement, a name and a boolean, and a user who wants it gone should
    /// press undo once, not four times.
    fn as_one_step<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        self.doc.begin_step();
        let out = f(self);
        self.doc.end_step();
        out
    }

    /// Unions a new body onto the model, or makes it the model if there is none.
    /// Copies the selection and joins the copy to the model.
    ///
    /// The whole point of a CAD tool is making the same thing more than once,
    /// and until now every repeat meant performing the entire gesture again and
    /// then matching the numbers by hand. The copy lands offset by one grid step
    /// so it is visible rather than hidden inside the original, and arrives
    /// selected, so the gizmo is already on it and it can be dragged straight to
    /// where it belongs.
    pub(crate) fn duplicate_selection(&mut self) {
        let Some(target) = self.selected else {
            self.status = "Nothing selected".to_string();
            return;
        };
        self.as_one_step(|s| {
            let Some(copy) = s.clone_subtree(target) else {
                s.status = "That cannot be copied".to_string();
                return;
            };
            let step = s.grid.max(0.01) * 4.0;
            let Some(placed) = s.place_plain(copy, Transform::from_translation(Vec3::X * step))
            else {
                return;
            };
            if let Some(name) = s.doc.name(target).map(ToString::to_string) {
                s.apply(Command::SetName {
                    id: copy,
                    name: Some(name),
                });
            }
            s.join_to_model(placed);
            s.select(Some(placed));
            s.status = "Copied, drag it where you want it".to_string();
        });
    }

    /// Repeats the selection along a line, or about the Z axis of its own frame.
    ///
    /// Wraps rather than copies. The child is evaluated once per instance, so
    /// twenty instances cost one subtree and twenty point transforms, and the
    /// count stays a single number: raise it and there are more of them.
    pub(crate) fn repeat_selection(&mut self, kind: sc_geom::node::Repeat) {
        let Some(target) = self.selected else {
            self.status = "Nothing selected".to_string();
            return;
        };
        // Spaced off the selection's own size, so the instances land beside each
        // other rather than inside each other. A fixed step would bury them in a
        // large feature and scatter them across the room for a small one.
        let span = sc_geom::bounds(self.doc.arena(), target).size();
        let kind = match kind {
            sc_geom::node::Repeat::Linear { .. } => sc_geom::node::Repeat::Linear {
                step: Vec3::X * (span.x.max(1.0) * 1.5),
            },
            circular @ sc_geom::node::Repeat::Circular { .. } => circular,
        };
        self.wrap_selection(
            |child| Node::Pattern {
                child,
                kind,
                count: 4,
            },
            "Repeat",
        );
        self.status = "Repeated. Change the count on the right".to_string();
    }

    /// Deep-copies a subtree, returning the new root.
    ///
    /// Deep rather than shared, because an edit to the copy must not change the
    /// original: that is what the word means to everyone who has used it. The
    /// arena would happily let two parents name one node, and for a pattern that
    /// is exactly right, but not for this.
    ///
    /// A derivation is remapped only when the face it names is inside the copy.
    /// Copying a boss on its own leaves it attached to the face it was already
    /// on; copying a boss together with the pad it sits on gives the copy its own
    /// pad to follow.
    fn clone_subtree(&mut self, id: NodeId) -> Option<NodeId> {
        let order = self.post_order(id);
        let mut copied: std::collections::HashMap<NodeId, NodeId> =
            std::collections::HashMap::new();
        for old in order {
            let mut node = self.doc.arena().get(old)?.clone();
            node.map_children(|c| copied.get(&c).copied().unwrap_or(c));
            if let Node::Transform { on: Some(base), .. } = &mut node {
                if let Some(inside) = copied.get(base) {
                    *base = *inside;
                }
            }
            let new = self.apply(Command::Add { node })?;
            copied.insert(old, new);
        }
        copied.get(&id).copied()
    }

    #[cfg(test)]
    pub(crate) fn post_order_for_test(&self, id: NodeId) -> Vec<NodeId> {
        self.post_order(id)
    }

    /// Every node under `id`, children before parents, each once.
    ///
    /// A shared node is reached twice and copied once, so the copy keeps the
    /// sharing the original had rather than quietly doubling in size.
    fn post_order(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        self.walk_post(id, &mut seen, &mut out);
        out
    }

    fn walk_post(
        &self,
        id: NodeId,
        seen: &mut std::collections::HashSet<NodeId>,
        out: &mut Vec<NodeId>,
    ) {
        if !seen.insert(id) {
            return;
        }
        let Some(node) = self.doc.arena().get(id) else {
            return;
        };
        for child in node.children() {
            self.walk_post(child, seen, out);
        }
        out.push(id);
    }

    fn join_to_model(&mut self, id: NodeId) {
        let Some(root) = self.doc.root() else {
            self.apply(Command::SetRoot { root: Some(id) });
            return;
        };
        let join = Node::Union {
            a: root,
            b: id,
            smooth: 0.0,
        };
        if let Some(union) = self.apply(Command::Add { node: join }) {
            self.apply(Command::SetRoot { root: Some(union) });
        }
    }

    pub(crate) fn undo(&mut self) {
        if self.doc.undo().unwrap_or(false) {
            self.field_dirty = true;
            self.dirty = true;
            self.prune_selection();
            self.status = "Undo".to_string();
        }
    }

    pub(crate) fn redo(&mut self) {
        if self.doc.redo().unwrap_or(false) {
            self.field_dirty = true;
            self.dirty = true;
            self.prune_selection();
            self.status = "Redo".to_string();
        }
    }

    /// Undo can delete the selected node. Ids are stable, so this only has to
    /// check liveness rather than re-resolve anything.
    fn prune_selection(&mut self) {
        if let Some(id) = self.selected {
            if !self.doc.arena().is_alive(id) {
                let root = self.doc.root();
                self.select(root);
            }
        }
    }

    /// Adds a pad built from a parametric profile on the current plane.
    ///
    /// Created at a default size and then re-dimensioned in the panel, because
    /// the profile stays parametric: a rectangle is a width and a height for as
    /// long as it exists.
    pub(crate) fn add_pad(&mut self, profile: sc_geom::Profile, label: &str) {
        self.add_pad_at(profile, label, Vec2::ZERO);
    }

    /// As [`AppState::add_pad`], at a chosen spot on the active plane.
    ///
    /// `at` is in the plane's own coordinates, which is what a click on the
    /// plane gives. Kept as a placement of its own beneath the frame rather than
    /// folded into it, so that an attached pad can still be regenerated when the
    /// face it sits on moves.
    pub(crate) fn add_pad_at(&mut self, profile: sc_geom::Profile, label: &str, at: Vec2) {
        self.as_one_step(|s| {
            // The sketch frame, not the datum plane: a pad dropped on an
            // attached face belongs on that face, exactly like one drawn there
            // by hand.
            let frame = s.sketch_frame();
            let node = Node::Extrude {
                profile,
                depth: s.extrude_height,
            };
            let Some(extrude) = s.apply(Command::Add { node }) else {
                return;
            };

            let local = Transform::from_translation(Vec3::new(at.x, at.y, 0.0));
            let Some(id) = s.place_offset(extrude, local, frame) else {
                return;
            };
            s.apply(Command::SetName {
                id,
                name: Some(label.to_string()),
            });
            s.join_to_model(id);
            s.select(Some(extrude));
            s.status = format!("Added {label}, set its dimensions on the right");
        });
    }

    /// Cuts a profile through the model from the current plane.
    ///
    /// The cut is placed behind the plane and swept past the far side, so the
    /// default is a hole all the way through rather than a blind recess. Its
    /// depth and position stay editable afterwards like any other feature.
    pub(crate) fn add_pocket(&mut self, profile: sc_geom::Profile, label: &str) {
        self.add_pocket_at(profile, label, Vec2::ZERO);
    }

    /// As [`AppState::add_pocket`], at a chosen spot on the active plane.
    pub(crate) fn add_pocket_at(&mut self, profile: sc_geom::Profile, label: &str, at: Vec2) {
        let Some(root) = self.doc.root() else {
            self.status = "Nothing to cut into yet".to_string();
            return;
        };

        self.as_one_step(|s| {
            // A prism rather than an extrusion sized to the model. "Through all"
            // is an end condition, not a measurement: a cut sized from the part
            // as it stands today silently becomes a blind recess the first time
            // the part grows.
            let node = Node::Prism { profile };
            let Some(cut) = s.apply(Command::Add { node }) else {
                return;
            };
            let frame = s.sketch_frame();
            let local = Transform::from_translation(Vec3::new(at.x, at.y, 0.0));
            let Some(placed) = s.place_offset(cut, local, frame) else {
                return;
            };

            let carve = Node::Difference {
                a: root,
                b: placed,
                smooth: 0.0,
            };
            let Some(result) = s.apply(Command::Add { node: carve }) else {
                return;
            };
            s.apply(Command::SetRoot { root: Some(result) });
            s.apply(Command::SetName {
                id: cut,
                name: Some(label.to_string()),
            });

            s.select(Some(cut));
            s.status = format!("Cut {label} through the part");
        });
    }

    /// Wraps the selection in a modifier and puts the wrapper where the
    /// selection used to sit.
    ///
    /// Rerouting is the whole job. Creating the modifier node is not enough:
    /// until every parent points at the wrapper instead of at the original, the
    /// model still evaluates the unmodified node and the tool looks like it did
    /// nothing at all.
    pub(crate) fn wrap_selection(&mut self, make: impl FnOnce(NodeId) -> Node, label: &str) {
        let Some(target) = self.selected else {
            self.status = "Nothing selected".to_string();
            return;
        };
        self.as_one_step(|s| {
            let was_root = s.doc.root() == Some(target);
            // Read the parents before the wrapper exists, otherwise it is found
            // as one of them and rewired into a loop.
            let parents = s.doc.arena().parents_of(target);
            let Some(wrapped) = s.apply(Command::Add { node: make(target) }) else {
                return;
            };
            for parent in parents {
                let Some(mut node) = s.doc.arena().get(parent).cloned() else {
                    continue;
                };
                node.map_children(|c| if c == target { wrapped } else { c });
                s.apply(Command::Replace { id: parent, node });
            }
            if was_root {
                s.apply(Command::SetRoot {
                    root: Some(wrapped),
                });
            }
            s.select(Some(wrapped));
            s.status = format!("Applied {label}");
        });
    }

    /// Adds a primitive and unions it onto the current root, so a new body shows
    /// up immediately instead of sitting orphaned in the tree.
    pub(crate) fn add_body(&mut self, node: Node, label: &str) {
        self.as_one_step(|s| {
            let Some(id) = s.apply(Command::Add { node }) else {
                return;
            };
            s.apply(Command::SetName {
                id,
                name: Some(label.to_string()),
            });
            s.join_to_model(id);
            s.select(Some(id));
            s.status = format!("Added {label}");
        });
    }

    pub(crate) fn wgsl(&self) -> sc_geom::wgsl::Generated {
        sc_geom::wgsl::generate_with_selection(self.doc.arena(), self.doc.root(), self.selected)
    }

    /// Begins a new profile on the current plane.
    ///
    /// Turns the camera to face it: drawing in two dimensions on a plane seen
    /// edge-on is guesswork.
    pub(crate) fn start_sketch(&mut self) {
        // The opposite of what `arm` does, and for the same reason: two
        // pending gestures both want the next click, and the armed one is
        // checked first, so a sketch started with a tool still armed would
        // spend its first point placing a pad somewhere nobody asked for.
        self.disarm();
        self.sketch = Some(Vec::new());
        self.tool = TOOL_SKETCH;
        self.rig.goal.look_along(self.plane_normal());
        self.rig.goal.target = self.plane_origin();
        self.status = if self.attached_to.is_some() {
            "Sketching on the attached face, click to place points".to_string()
        } else {
            format!("Sketching on {}, click to place points", self.plane.name())
        };
    }

    /// The frame a sketch is drawn in: datum, or the face it is attached to.
    #[must_use]
    pub(crate) fn sketch_frame(&self) -> Transform {
        let attached = self.attached_to.and_then(|id| {
            let root = self.doc.root()?;
            sc_geom::pick::face_placement(self.doc.arena(), root, id)
        });
        attached.unwrap_or_else(|| self.plane.placement())
    }

    /// Where the sketch plane sits in the model.
    #[must_use]
    pub(crate) fn plane_origin(&self) -> Vec3 {
        self.sketch_frame().apply_point(Vec3::ZERO)
    }

    /// The sketch plane's outward normal.
    #[must_use]
    pub(crate) fn plane_normal(&self) -> Vec3 {
        self.sketch_frame().rotation * Vec3::Z
    }

    /// Lifts a sketch coordinate into the model.
    #[must_use]
    pub(crate) fn to_world(&self, point: Vec2) -> Vec3 {
        self.sketch_frame()
            .apply_point(Vec3::new(point.x, point.y, 0.0))
    }

    /// Drops a model point onto the sketch plane's coordinates.
    #[must_use]
    pub(crate) fn to_plane(&self, point: Vec3) -> Vec2 {
        let local = self.sketch_frame().inverse_point(point);
        Vec2::new(local.x, local.y)
    }

    /// Wraps a node in a placement on the current sketch frame.
    fn place(&mut self, node: NodeId, frame: Transform) -> Option<NodeId> {
        self.place_offset(node, Transform::IDENTITY, frame)
    }

    /// Wraps a node in a placement, unless the placement does nothing, and
    /// records the face the frame came from.
    ///
    /// `local` is the feature's own offset within the sketch frame, which a
    /// pocket uses to begin its cut outside the material. It is kept in a
    /// placement of its own rather than folded into the frame, because
    /// regeneration rewrites the derived placement wholesale: composed into one
    /// node, the offset would be erased the next time the face moved.
    ///
    /// An identity transform in the tree is noise, so the build plate adds
    /// none. A derived placement is kept even so: dropping it would drop the
    /// dependency with it, and the feature would stop following its face.
    fn place_offset(&mut self, node: NodeId, local: Transform, frame: Transform) -> Option<NodeId> {
        let Some(on) = self.attached_to else {
            // Nothing to regenerate, so the two halves can collapse into the
            // single placement this has always produced.
            return self.place_plain(node, local.then(&frame));
        };
        let inner = self.place_plain(node, local)?;
        self.apply(Command::Add {
            node: Node::Transform {
                child: inner,
                xform: frame,
                on: Some(on),
            },
        })
    }

    /// A placement with no dependency behind it, skipped when it would be the
    /// identity.
    fn place_plain(&mut self, node: NodeId, frame: Transform) -> Option<NodeId> {
        if frame == Transform::IDENTITY {
            return Some(node);
        }
        self.apply(Command::Add {
            node: Node::Transform {
                child: node,
                xform: frame,
                on: None,
            },
        })
    }

    /// Attaches the sketch plane to the selected feature's far face.
    pub(crate) fn attach_to_selection(&mut self) {
        let Some(id) = self.selected else {
            self.status = "Select a pad to sketch on first".to_string();
            return;
        };
        let Some(pad) = self.attachable_face(id) else {
            self.status = "That selection has no single pad face to sketch on".to_string();
            return;
        };
        self.attached_to = Some(pad);
        self.status = "Sketching on the face of that pad".to_string();
    }

    /// Moves the selection, keeping any attachment it has.
    ///
    /// A derived placement belongs to regeneration, which rewrites it whole the
    /// next time its face moves. So the move goes underneath it instead, in the
    /// feature's own frame, which is the same place a pocket keeps the overshoot
    /// that opens its bore. The feature still follows its face; it simply sits
    /// somewhere else on it.
    ///
    /// That frame is also the one a user means. A pad attached to a face slides
    /// across that face in x and y and lifts off it in z, rather than moving
    /// along the world axes, which on an angled face would be unusable.
    ///
    /// Anything not attached is wrapped in a placement of its own, as before.
    pub(crate) fn move_selection(&mut self, delta: Vec3) {
        let Some(target) = self.selected else {
            self.status = "Nothing selected".to_string();
            return;
        };
        let derived = match self.doc.arena().get(target) {
            Some(Node::Transform {
                child, on: Some(_), ..
            }) => Some(*child),
            _ => None,
        };
        let Some(child) = derived else {
            self.wrap_selection(
                |child| Node::Transform {
                    child,
                    xform: Transform::from_translation(delta),
                    on: None,
                },
                "Move",
            );
            return;
        };
        self.as_one_step(|s| {
            s.slide_under(target, child, delta);
            s.status = "Moved along the face it is attached to".to_string();
        });
    }

    /// Adds `delta` to the local offset beneath a derived placement, creating
    /// one if this is the first move.
    ///
    /// Accumulates rather than stacking a new node per move, so ten nudges leave
    /// one offset in the tree instead of ten.
    fn slide_under(&mut self, derived: NodeId, child: NodeId, delta: Vec3) {
        if let Some(Node::Transform {
            child: inner,
            xform,
            on: None,
        }) = self.doc.arena().get(child).cloned()
        {
            let moved = Transform {
                translation: xform.translation + delta,
                ..xform
            };
            self.apply(Command::Replace {
                id: child,
                node: Node::Transform {
                    child: inner,
                    xform: moved,
                    on: None,
                },
            });
            return;
        }
        let Some(offset) = self.apply(Command::Add {
            node: Node::Transform {
                child,
                xform: Transform::from_translation(delta),
                on: None,
            },
        }) else {
            return;
        };
        let Some(Node::Transform { xform, on, .. }) = self.doc.arena().get(derived).cloned() else {
            return;
        };
        self.apply(Command::Replace {
            id: derived,
            node: Node::Transform {
                child: offset,
                xform,
                on,
            },
        });
    }

    /// Breaks a derived placement's link to the face it was built on.
    ///
    /// Deliberately explicit. Moving an attached feature keeps the attachment,
    /// so the only way to stop one following its face is to say so, and the
    /// resolved placement is kept exactly as it stands so nothing jumps at the
    /// moment the link is cut.
    pub(crate) fn detach_selection(&mut self) {
        let Some(id) = self.selected else {
            return;
        };
        let Some(Node::Transform {
            child,
            xform,
            on: Some(_),
        }) = self.doc.arena().get(id).cloned()
        else {
            self.status = "That is not attached to a face".to_string();
            return;
        };
        self.apply(Command::Replace {
            id,
            node: Node::Transform {
                child,
                xform,
                on: None,
            },
        });
        self.status = "Detached; it will stay where it is now".to_string();
    }

    /// Whether the selection is a placement that follows a face.
    #[must_use]
    pub(crate) fn selection_is_attached(&self) -> bool {
        self.selected
            .and_then(|id| self.doc.arena().get(id))
            .and_then(Node::derived_from)
            .is_some()
    }

    /// The pad whose face a sketch would land on, given what is selected.
    ///
    /// A user selects what they can see, and what they can see is the finished
    /// feature: the offset, shell or transform wrapping a pad rather than the
    /// pad itself. Those each have exactly one child, so there is no question
    /// which pad is underneath, and refusing to look through them greys the Face
    /// button out while the pointer is on the very face it describes.
    ///
    /// Stops at a boolean. Both sides of a union have a face and nothing here
    /// can tell which one was meant, so the honest answer is none.
    #[must_use]
    pub(crate) fn attachable_face(&self, id: NodeId) -> Option<NodeId> {
        let mut at = id;
        // Terminates because the arena refuses a cycle, so the walk is bounded
        // by the depth of the tree.
        loop {
            let node = self.doc.arena().get(at)?;
            if matches!(node, Node::Extrude { .. }) {
                return Some(at);
            }
            let mut children = node.children();
            let only = children.next()?;
            if children.next().is_some() {
                return None;
            }
            at = only;
        }
    }

    /// Returns the sketch plane to a datum.
    pub(crate) fn detach_plane(&mut self) {
        self.attached_to = None;
    }

    /// Chooses the plane the next sketch will be drawn on.
    pub(crate) fn set_plane(&mut self, plane: SketchPlane) {
        self.plane = plane;
        self.attached_to = None;
        self.status = format!("{} plane", plane.name());
        if self.sketch.is_some() {
            // Switching mid-sketch would leave points on the old plane.
            self.sketch = Some(Vec::new());
            self.rig.goal.look_along(self.plane_normal());
        }
    }

    pub(crate) fn cancel_sketch(&mut self) {
        if self.sketch.take().is_some() {
            self.status = "Sketch cancelled".to_string();
        }
    }

    /// Rounds a plate position to the sketch grid.
    #[must_use]
    pub(crate) fn snap(&self, point: Vec2) -> Vec2 {
        if self.grid <= 0.0 {
            return point;
        }
        Vec2::new(
            (point.x / self.grid).round() * self.grid,
            (point.y / self.grid).round() * self.grid,
        )
    }

    /// Adds a point to the profile in progress.
    pub(crate) fn add_sketch_point(&mut self, point: Vec2) {
        let Some(points) = self.sketch.as_mut() else {
            return;
        };
        // Ignore a repeat of the last point; a double click should not create a
        // zero-length edge that makes the profile degenerate.
        if points.last().is_some_and(|p| (*p - point).length() < 0.01) {
            return;
        }
        // Report the edge just closed, so the profile can be built to a size
        // rather than to a feel.
        let edge = points.last().map(|prev| (point - *prev).length());
        points.push(point);
        let n = points.len();
        self.status = match (n, edge) {
            (_, Some(len)) => format!("{n} points, last edge {len:.1} mm"),
            _ => format!("{n} of at least 3 points"),
        };
    }

    pub(crate) fn undo_sketch_point(&mut self) {
        if let Some(points) = self.sketch.as_mut() {
            points.pop();
        }
    }

    /// Turns the profile in progress into an extruded solid.
    pub(crate) fn finish_sketch(&mut self) {
        let Some(points) = self.sketch.take() else {
            return;
        };
        if points.len() < 3 {
            self.status = "A profile needs at least 3 points".to_string();
            self.sketch = Some(points);
            return;
        }

        let height = self.extrude_height;
        let frame = self.sketch_frame();
        let node = Node::Extrude {
            profile: sc_geom::Profile::Path { points },
            depth: height,
        };
        if !node.is_valid() {
            self.status = "That profile encloses no area".to_string();
            return;
        }

        self.as_one_step(|s| {
            let Some(extrude) = s.apply(Command::Add { node }) else {
                return;
            };

            // An extrusion is defined in its own XY plane sweeping along +Z, so
            // placing it on a datum plane is exactly the rotation between the
            // two frames. The build plate needs none, and an identity transform
            // in the tree is just noise.
            let Some(id) = s.place(extrude, frame) else {
                return;
            };

            let where_ = if s.attached_to.is_some() {
                "face".to_string()
            } else {
                s.plane.name().to_string()
            };
            s.apply(Command::SetName {
                id,
                name: Some(format!("Pad on {where_}")),
            });
            s.join_to_model(id);

            s.select(Some(id));
            s.tool = TOOL_SELECT;
            s.status = format!("Extruded {height:.1} mm - adjust the height on the right");
        });
    }

    /// Selects a node and refreshes the viewport highlight.
    pub(crate) fn select(&mut self, id: Option<NodeId>) {
        if self.selected != id {
            self.selected = id;
            // The highlight is part of the generated field, so it has to be
            // rebuilt like any other geometry change.
            self.field_dirty = true;
        }
    }

    /// Meshes the model and writes it as STL.
    ///
    /// Reports the topology in the status bar rather than assuming it: a hole is
    /// invisible on screen and fatal to a print, so the user is told either way.
    pub(crate) fn export_stl(&mut self, path: &Path, resolution: u32) {
        let Some(root) = self.doc.root() else {
            self.status = "Nothing to export".to_string();
            return;
        };

        let started = std::time::Instant::now();
        let mesh = sc_mesh::contour(
            self.doc.arena(),
            root,
            sc_mesh::Settings {
                resolution,
                refinement: 2,
            },
        );
        let topology = mesh.topology();
        if !topology.is_printable() {
            self.status = format!(
                "NOT printable: {} holes, {} non-manifold",
                topology.boundary_edges, topology.non_manifold_edges
            );
            return;
        }

        self.status = match sc_mesh::stl::write(&mesh, path) {
            Ok(()) => format!(
                "Wrote {} - {} triangles, watertight, {:.0} mm^3, {:.1?}",
                path.display(),
                mesh.triangle_count(),
                mesh.volume(),
                started.elapsed()
            ),
            Err(e) => format!("Could not write {}: {e}", path.display()),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "shapecad-app-{name}-{}.shapecad",
            std::process::id()
        ));
        p
    }

    #[test]
    fn saving_clears_the_dirty_flag_and_names_the_document() {
        let mut state = AppState::new();
        state.load_sample();
        state.apply(Command::SetName {
            id: state.doc.root().unwrap(),
            name: Some("part".into()),
        });
        assert!(state.dirty, "an edit should mark the document dirty");
        assert_eq!(state.title(), "Untitled \u{2022}");

        let path = scratch("save");
        state.save_to(&path);
        assert!(!state.dirty, "saving should clear the dirty flag");
        assert_eq!(state.title(), path.file_stem().unwrap().to_str().unwrap());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn opening_restores_the_same_geometry() {
        let mut state = AppState::new();
        state.load_sample();
        let before = state.doc.hash();
        let path = scratch("open");
        state.save_to(&path);

        let mut other = AppState::new();
        other.new_document();
        assert_eq!(other.doc.hash(), None, "a new document is empty");

        other.open_path(&path);
        assert_eq!(other.doc.hash(), before, "reopened geometry differs");
        assert!(!other.dirty);
        assert!(other.path.is_some());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_failed_open_leaves_the_document_alone() {
        let mut state = AppState::new();
        state.load_sample();
        let before = state.doc.hash();
        state.open_path(std::path::Path::new("/nonexistent/nope.shapecad"));
        assert_eq!(
            state.doc.hash(),
            before,
            "a failed open replaced the document"
        );
        assert!(state.status.contains("Could not open"), "{}", state.status);
    }

    #[test]
    fn sketching_a_square_produces_an_extruded_solid() {
        let mut state = AppState::new();
        state.new_document();
        state.start_sketch();
        for (x, y) in [(-10.0, -10.0), (10.0, -10.0), (10.0, 10.0), (-10.0, 10.0)] {
            state.add_sketch_point(Vec2::new(x, y));
        }
        state.extrude_height = 5.0;
        state.finish_sketch();

        assert!(state.sketch.is_none(), "the sketch should be consumed");
        let root = state.doc.root().expect("the pad became the document root");
        let bounds = state.doc.bounds().unwrap();
        assert!((bounds.size().x - 20.0).abs() < 0.01, "{bounds:?}");
        assert!((bounds.size().z - 5.0).abs() < 0.01, "{bounds:?}");
        // A pad sits on the plate it was drawn on.
        assert!(
            bounds.min.z.abs() < 0.01,
            "pad is not on the build plate: {bounds:?}"
        );

        // Solid inside, empty above.
        assert!(
            sc_geom::eval(
                state.doc.arena(),
                root,
                sc_geom::glam::Vec3::new(0.0, 0.0, 2.5)
            ) < 0.0
        );
        assert!(
            sc_geom::eval(
                state.doc.arena(),
                root,
                sc_geom::glam::Vec3::new(0.0, 0.0, 9.0)
            ) > 0.0
        );
    }

    /// The whole modelling loop, end to end, in the order a person does it.
    ///
    /// Every step here has been verified in isolation; this checks they survive
    /// being done one after another, which is the part that actually matters and
    /// the part unit tests miss.
    #[test]
    fn the_full_workflow_survives_being_done_in_order() {
        let mut state = AppState::new();
        state.new_document();

        // 1. Sketch a dimensioned rectangle: 40 by 30 on the build plate.
        state.start_sketch();
        for (x, y) in [(-20.0, -15.0), (20.0, -15.0), (20.0, 15.0), (-20.0, 15.0)] {
            state.add_sketch_point(Vec2::new(x, y));
        }

        // 2. Extrude it 12mm.
        state.extrude_height = 12.0;
        state.finish_sketch();
        let pad = state.selected.expect("the pad is selected after extruding");

        let bounds = state.doc.bounds().expect("rooted");
        assert!((bounds.size().x - 40.0).abs() < 0.01, "{bounds:?}");
        assert!((bounds.size().y - 30.0).abs() < 0.01, "{bounds:?}");
        assert!((bounds.size().z - 12.0).abs() < 0.01, "{bounds:?}");
        assert!(
            bounds.min.z.abs() < 0.01,
            "the pad should sit on the plate: {bounds:?}"
        );

        // 3. Select it by clicking the middle of the view, then edit it.
        state.frame_model();
        state.rig.snap_to(state.rig.goal);
        state.select(None);
        state.select_at(sc_geom::glam::Vec2::ZERO, 1.5, 0.5);
        assert_eq!(
            state.selected,
            Some(pad),
            "clicking the part selected {:?} instead: {}",
            state.selected,
            state.status
        );
        state.apply(Command::SetParam {
            id: pad,
            name: "depth".into(),
            value: 20.0,
        });
        let edited = state.doc.bounds().expect("rooted");
        assert!(
            (edited.size().z - 20.0).abs() < 0.01,
            "the edit did not take: {edited:?}"
        );

        // Editing a value must not have forced a shader rebuild.
        let before_source = state.wgsl().source;
        state.apply(Command::SetParam {
            id: pad,
            name: "depth".into(),
            value: 18.0,
        });
        assert_eq!(
            state.wgsl().source,
            before_source,
            "a depth edit rebuilt the shader"
        );

        // 4. Save and reopen.
        let path = scratch("workflow");
        state.save_to(&path);
        assert!(
            !state.dirty,
            "saving left the document dirty: {}",
            state.status
        );

        let mut reopened = AppState::new();
        reopened.open_path(&path);
        assert_eq!(
            reopened.doc.hash(),
            state.doc.hash(),
            "the part changed across save and reopen: {}",
            reopened.status
        );
        assert!(
            reopened.doc.arena().is_alive(pad),
            "the pad's id did not survive the round trip"
        );

        // 5. Export a mesh a slicer would accept.
        let root = reopened.doc.root().expect("rooted");
        let mesh = sc_mesh::contour(
            reopened.doc.arena(),
            root,
            sc_mesh::Settings {
                resolution: 96,
                refinement: 2,
            },
        );
        let topology = mesh.topology();
        assert!(
            topology.is_printable(),
            "not printable: {} holes, {} inconsistent winding",
            topology.boundary_edges,
            topology.inconsistent_edges
        );
        assert!(
            topology.is_manifold(),
            "{} non-manifold edges",
            topology.non_manifold_edges
        );

        let expected = 40.0 * 30.0 * 18.0;
        let volume = mesh.volume();
        assert!(
            (volume - expected).abs() / expected < 0.02,
            "volume {volume} differs from {expected} by more than 2%"
        );

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_profile_with_too_few_points_is_refused_and_kept() {
        let mut state = AppState::new();
        state.new_document();
        state.start_sketch();
        state.add_sketch_point(Vec2::new(0.0, 0.0));
        state.add_sketch_point(Vec2::new(10.0, 0.0));
        state.finish_sketch();

        assert!(
            state.sketch.is_some(),
            "an unfinished sketch must not be thrown away"
        );
        assert!(
            state.doc.root().is_none(),
            "nothing should have been created"
        );
        assert!(state.status.contains("at least 3"), "{}", state.status);
    }

    #[test]
    fn sketch_points_land_on_the_grid() {
        let mut state = AppState::new();
        state.grid = 1.0;
        assert_eq!(state.snap(Vec2::new(10.4, -3.7)), Vec2::new(10.0, -4.0));
        assert_eq!(state.snap(Vec2::new(-0.2, 0.6)), Vec2::new(0.0, 1.0));

        state.grid = 5.0;
        assert_eq!(state.snap(Vec2::new(12.0, -12.0)), Vec2::new(10.0, -10.0));

        // Zero disables snapping rather than dividing by zero.
        state.grid = 0.0;
        assert_eq!(state.snap(Vec2::new(1.234, 5.678)), Vec2::new(1.234, 5.678));
    }

    #[test]
    fn clicking_the_model_selects_it_and_clicking_away_clears_it() {
        let mut state = AppState::new();
        state.load_sample();
        state.select(None);

        // The camera frames the sample, so the middle of the view is the part.
        state.select_at(sc_geom::glam::Vec2::ZERO, 1.5, 0.5);
        assert!(
            state.selected.is_some(),
            "centre of view selected nothing: {}",
            state.status
        );

        // A corner of the view is empty space.
        state.select_at(sc_geom::glam::Vec2::new(-0.98, 0.98), 1.5, 0.5);
        assert!(
            state.selected.is_none(),
            "empty space selected {:?}",
            state.selected
        );
        assert!(state.status.contains("Nothing"), "{}", state.status);
    }

    #[test]
    fn a_repeated_point_is_ignored() {
        // Double clicking must not create a zero-length edge.
        let mut state = AppState::new();
        state.start_sketch();
        state.add_sketch_point(Vec2::new(1.0, 1.0));
        state.add_sketch_point(Vec2::new(1.0, 1.0));
        assert_eq!(state.sketch.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn cancelling_a_sketch_changes_nothing() {
        let mut state = AppState::new();
        state.load_sample();
        let before = state.doc.hash();
        state.start_sketch();
        for (x, y) in [(0.0, 0.0), (5.0, 0.0), (5.0, 5.0)] {
            state.add_sketch_point(Vec2::new(x, y));
        }
        state.cancel_sketch();
        assert!(state.sketch.is_none());
        assert_eq!(state.doc.hash(), before);
    }

    #[test]
    fn the_application_opens_on_an_empty_document() {
        let state = AppState::new();
        assert!(
            state.doc.root().is_none(),
            "startup should not preload a part"
        );
        assert!(!state.dirty);
        assert_eq!(state.title(), "Untitled");
    }

    #[test]
    fn starting_a_sketch_turns_the_camera_to_face_the_plane() {
        // Drawing in two dimensions on a plane seen edge-on is guesswork.
        for plane in SketchPlane::ALL {
            let mut state = AppState::new();
            state.set_plane(plane);
            state.start_sketch();
            // The rig eases, so read the goal rather than the current camera.
            let toward_eye = (state.rig.goal.eye() - state.rig.goal.target).normalize();
            let normal = state.plane_normal();
            assert!(
                toward_eye.dot(normal) > 0.99,
                "{} faces {toward_eye:?}, expected {normal:?}",
                plane.name()
            );
        }
    }

    #[test]
    fn switching_plane_mid_sketch_discards_points_on_the_old_one() {
        let mut state = AppState::new();
        state.start_sketch();
        state.add_sketch_point(Vec2::new(5.0, 5.0));
        state.set_plane(SketchPlane::Yz);
        assert_eq!(
            state.sketch.as_ref().map(Vec::len),
            Some(0),
            "points from the previous plane were kept"
        );
    }

    #[test]
    fn a_rectangle_can_be_dimensioned_after_it_is_created() {
        // The worked example: draw it, then say what size it is, then change
        // one dimension and have the other survive.
        let mut state = AppState::new();
        state.extrude_height = 5.0;
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 40.0,
                height: 30.0,
            },
            "Rectangle",
        );
        let pad = state.selected.expect("the new pad is selected");

        state.apply(Command::SetParam {
            id: pad,
            name: "width".into(),
            value: 60.0,
        });
        state.apply(Command::SetParam {
            id: pad,
            name: "height".into(),
            value: 40.0,
        });

        let b = state.doc.bounds().expect("rooted");
        assert!((b.size().x - 60.0).abs() < 0.01, "{b:?}");
        assert!((b.size().y - 40.0).abs() < 0.01, "{b:?}");
        assert!((b.size().z - 5.0).abs() < 0.01, "pad depth changed: {b:?}");

        // Changing the width later leaves the height and the pad depth alone.
        state.apply(Command::SetParam {
            id: pad,
            name: "width".into(),
            value: 25.0,
        });
        let after = state.doc.bounds().expect("rooted");
        assert!((after.size().x - 25.0).abs() < 0.01, "{after:?}");
        assert!(
            (after.size().y - 40.0).abs() < 0.01,
            "height was disturbed: {after:?}"
        );
        assert!(
            (after.size().z - 5.0).abs() < 0.01,
            "depth was disturbed: {after:?}"
        );
    }

    #[test]
    fn re_dimensioning_never_rebuilds_the_shader() {
        // Parametric dimensions are only useful if they are cheap to change.
        let mut state = AppState::new();
        state.add_pad(sc_geom::Profile::Circle { radius: 10.0 }, "Circle");
        let pad = state.selected.expect("selected");
        let before = state.wgsl().source;

        state.apply(Command::SetParam {
            id: pad,
            name: "radius".into(),
            value: 22.0,
        });
        assert_eq!(
            state.wgsl().source,
            before,
            "a radius edit rebuilt the shader"
        );
    }

    #[test]
    fn a_pocket_cuts_all_the_way_through() {
        // The rest of the worked example: pad a rectangle, then bore a hole.
        let mut state = AppState::new();
        state.extrude_height = 5.0;
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 60.0,
                height: 40.0,
            },
            "Base",
        );
        state.add_pocket(sc_geom::Profile::Circle { radius: 6.0 }, "Bore");

        let root = state.doc.root().expect("rooted");
        let arena = state.doc.arena();
        let at = |x: f32, y: f32, z: f32| sc_geom::eval(arena, root, Vec3::new(x, y, z));

        // Void through the whole thickness, including right at both faces.
        assert!(at(0.0, 0.0, 0.5) > 0.0, "bottom of the bore is still solid");
        assert!(at(0.0, 0.0, 2.5) > 0.0, "middle of the bore is still solid");
        assert!(at(0.0, 0.0, 4.5) > 0.0, "top of the bore is still solid");
        // Material remains away from the hole.
        assert!(at(20.0, 0.0, 2.5) < 0.0, "the part was cut away entirely");

        // Cutting does not grow the part.
        let b = state.doc.bounds().expect("rooted");
        assert!((b.size().x - 60.0).abs() < 0.01, "{b:?}");
        assert!((b.size().z - 5.0).abs() < 0.01, "{b:?}");
    }

    #[test]
    fn a_pocket_stays_editable_after_it_is_cut() {
        let mut state = AppState::new();
        state.extrude_height = 5.0;
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 60.0,
                height: 40.0,
            },
            "Base",
        );
        state.add_pocket(sc_geom::Profile::Circle { radius: 4.0 }, "Bore");
        let bore = state.selected.expect("the cut is selected");

        let arena = state.doc.arena();
        let root = state.doc.root().unwrap();
        assert!(
            sc_geom::eval(arena, root, Vec3::new(7.0, 0.0, 2.5)) < 0.0,
            "7mm out should still be solid at radius 4"
        );

        state.apply(Command::SetParam {
            id: bore,
            name: "radius".into(),
            value: 10.0,
        });
        let arena = state.doc.arena();
        let root = state.doc.root().unwrap();
        assert!(
            sc_geom::eval(arena, root, Vec3::new(7.0, 0.0, 2.5)) > 0.0,
            "widening the bore did not reach 7mm out"
        );
    }

    /// The worked example, exactly as described: rectangle, dimension it, pad
    /// it, sketch on its top face, pocket through, then change the width and
    /// have everything else survive.
    #[test]
    fn the_worked_example_runs_end_to_end() {
        let mut state = AppState::new();

        // Rectangle, dimensioned after the fact.
        state.extrude_height = 5.0;
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 40.0,
                height: 30.0,
            },
            "Base",
        );
        let base = state.selected.expect("the pad is selected");
        state.apply(Command::SetParam {
            id: base,
            name: "width".into(),
            value: 60.0,
        });
        state.apply(Command::SetParam {
            id: base,
            name: "height".into(),
            value: 40.0,
        });
        state.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 5.0,
        });

        let b = state.doc.bounds().expect("rooted");
        assert!((b.size().x - 60.0).abs() < 0.01, "{b:?}");
        assert!((b.size().y - 40.0).abs() < 0.01, "{b:?}");

        // Attach a work plane to the pad's top face.
        state.select(Some(base));
        state.attach_to_selection();
        assert_eq!(state.attached_to, Some(base), "{}", state.status);
        let origin = state.plane_origin();
        assert!(
            (origin.z - 5.0).abs() < 0.01,
            "the work plane is not on the top face: {origin:?}"
        );

        // Pocket a circle through from that face.
        state.add_pocket(sc_geom::Profile::Circle { radius: 6.0 }, "Bore");
        let bore = state.selected.expect("the cut is selected");

        let solid_at = |s: &AppState, x: f32, y: f32, z: f32| {
            sc_geom::eval(s.doc.arena(), s.doc.root().unwrap(), Vec3::new(x, y, z)) < 0.0
        };
        assert!(
            !solid_at(&state, 0.0, 0.0, 2.5),
            "the bore did not cut through"
        );
        assert!(
            solid_at(&state, 25.0, 0.0, 2.5),
            "the part was cut away entirely"
        );

        // Change the rectangle's width; the pad depth, the other dimension and
        // the bore all survive.
        state.apply(Command::SetParam {
            id: base,
            name: "width".into(),
            value: 90.0,
        });
        let after = state.doc.bounds().expect("rooted");
        assert!(
            (after.size().x - 90.0).abs() < 0.01,
            "width did not take: {after:?}"
        );
        assert!(
            (after.size().y - 40.0).abs() < 0.01,
            "height was disturbed: {after:?}"
        );
        assert!(
            (after.size().z - 5.0).abs() < 0.01,
            "depth was disturbed: {after:?}"
        );
        assert!(!solid_at(&state, 0.0, 0.0, 2.5), "the bore was lost");
        assert!(
            solid_at(&state, 40.0, 0.0, 2.5),
            "the part did not actually widen"
        );

        // And the bore itself is still a radius that can be changed.
        state.apply(Command::SetParam {
            id: bore,
            name: "radius".into(),
            value: 12.0,
        });
        assert!(
            !solid_at(&state, 10.0, 0.0, 2.5),
            "widening the bore had no effect"
        );
    }

    #[test]
    fn a_pocket_needs_something_to_cut() {
        let mut state = AppState::new();
        state.add_pocket(sc_geom::Profile::Circle { radius: 5.0 }, "Bore");
        assert!(
            state.doc.root().is_none(),
            "a pocket created geometry from nothing"
        );
        assert!(state.status.contains("Nothing to cut"), "{}", state.status);
    }

    #[test]
    fn a_pad_on_a_vertical_plane_is_placed_upright() {
        let mut state = AppState::new();
        state.set_plane(SketchPlane::Xz);
        state.start_sketch();
        for (u, v) in [(-10.0, 0.0), (10.0, 0.0), (10.0, 20.0), (-10.0, 20.0)] {
            state.add_sketch_point(Vec2::new(u, v));
        }
        state.extrude_height = 6.0;
        state.finish_sketch();

        // Drawn 20 wide by 20 tall on XZ, swept 6 along the plane's normal (-Y).
        let b = state.doc.bounds().expect("rooted");
        assert!((b.size().x - 20.0).abs() < 0.01, "{b:?}");
        assert!((b.size().z - 20.0).abs() < 0.01, "{b:?}");
        assert!(
            (b.size().y - 6.0).abs() < 0.01,
            "swept the wrong way: {b:?}"
        );
    }

    #[test]
    fn a_new_document_is_empty_and_clean() {
        let mut state = AppState::new();
        state.load_sample();
        state.new_document();
        assert!(state.doc.root().is_none());
        assert!(!state.dirty);
        assert!(state.path.is_none());
        assert_eq!(state.title(), "Untitled");
    }
    /// Right clicking a node has to select it, or the menu and the property
    /// panel would be acting on two different nodes at once.
    #[test]
    fn opening_a_node_menu_selects_that_node() {
        let mut state = AppState::new();
        state.add_body(Node::Sphere { radius: 5.0 }, "Ball");
        let id = state.doc.root().expect("the body became the root");
        state.select(None);

        state.open_menu((10.0, 20.0), MenuTarget::Node(id));

        assert_eq!(state.selected, Some(id));
        let menu = state.menu.expect("the menu is open");
        assert_eq!(menu.target, MenuTarget::Node(id));
        assert!((menu.at.0 - 10.0).abs() < f32::EPSILON);
    }

    /// The viewport and sketch menus act on the document as a whole, so they
    /// must not disturb whatever was selected.
    #[test]
    fn opening_an_empty_menu_leaves_the_selection_alone() {
        let mut state = AppState::new();
        state.add_body(Node::Sphere { radius: 5.0 }, "Ball");
        let id = state.doc.root();

        state.open_menu((0.0, 0.0), MenuTarget::Empty);

        assert_eq!(state.selected, id);
    }

    #[test]
    fn closing_reports_whether_a_menu_was_open() {
        let mut state = AppState::new();
        assert!(!state.close_menu(), "nothing was open");

        state.open_menu((0.0, 0.0), MenuTarget::Sketch);
        assert!(state.close_menu(), "the open menu was closed");
        assert!(state.menu.is_none());
    }

    /// Framing one node has to target that node, not the whole model. A sphere
    /// pushed away from the origin is the case that tells the two apart.
    #[test]
    fn framing_a_node_targets_that_node() {
        let mut state = AppState::new();
        state.add_body(Node::Sphere { radius: 5.0 }, "Ball");
        state.wrap_selection(
            |child| Node::Transform {
                child,
                xform: Transform::from_translation(Vec3::new(100.0, 0.0, 0.0)),
                on: None,
            },
            "Move",
        );
        let moved = state.doc.root().expect("the transform became the root");
        let inner = state
            .doc
            .arena()
            .get(moved)
            .expect("the transform is in the arena")
            .children()
            .next()
            .expect("a transform has one child");

        state.frame_node(inner);
        let at_origin = state.rig.goal.target;
        state.frame_node(moved);
        let moved_away = state.rig.goal.target;

        assert!(
            at_origin.x.abs() < 1.0,
            "the untransformed sphere sits at the origin, got {at_origin}"
        );
        assert!(
            (moved_away.x - 100.0).abs() < 1.0,
            "the transformed sphere sits 100mm along X, got {moved_away}"
        );
    }

    /// A modifier applied to a node buried in the tree has to take effect. The
    /// wrapper node existing is not enough: unless its parent is rerouted to it,
    /// the model still evaluates the original and the tool silently does
    /// nothing.
    #[test]
    fn a_modifier_applied_below_the_root_changes_the_model() {
        let mut state = AppState::new();
        state.add_body(Node::Sphere { radius: 20.0 }, "Ball");
        state.add_body(
            Node::Box {
                half: Vec3::splat(10.0),
                round: 0.0,
            },
            "Block",
        );
        let block = state.selected.expect("the new body is selected");
        let root = state.doc.root().expect("the union became the root");
        assert_ne!(block, root, "the block has to sit below the root");
        let before = state.doc.hash().expect("a rooted model hashes");

        state.wrap_selection(
            |child| Node::Shell {
                child,
                thickness: 2.0,
            },
            "Shell",
        );

        assert_eq!(
            state.doc.root(),
            Some(root),
            "wrapping a child must not move the root"
        );
        assert_ne!(
            state.doc.hash().expect("still rooted"),
            before,
            "the shell was created but nothing points at it"
        );
        let shell = state.selected.expect("the wrapper is selected");
        assert_eq!(
            state.doc.arena().parents_of(block),
            vec![shell],
            "the block is still reachable around the shell"
        );
    }

    /// The same tool applied to the root reroutes the root instead.
    #[test]
    fn a_modifier_applied_to_the_root_reroutes_the_root() {
        let mut state = AppState::new();
        state.add_body(Node::Sphere { radius: 20.0 }, "Ball");
        let ball = state.doc.root().expect("the body became the root");

        state.wrap_selection(
            |child| Node::Shell {
                child,
                thickness: 2.0,
            },
            "Shell",
        );

        let root = state.doc.root().expect("still rooted");
        assert_ne!(root, ball, "the root was left pointing at the bare body");
        assert!(matches!(
            state.doc.arena().get(root),
            Some(Node::Shell { .. })
        ));
    }

    /// One thing the user did should take one press of undo to take back, even
    /// though a pad is an extrusion, a placement, a name and a boolean.
    #[test]
    fn one_undo_takes_back_a_whole_feature() {
        let mut state = AppState::new();
        state.add_body(Node::Sphere { radius: 20.0 }, "Ball");
        let before = state.doc.hash().expect("a rooted model hashes");
        let steps = state.doc.log_len();

        state.plane = SketchPlane::Xz;
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 8.0,
                height: 8.0,
            },
            "Pad",
        );
        let pad = state.selected.expect("the new pad is selected");
        assert_ne!(
            state.doc.hash().expect("still rooted"),
            before,
            "the pad did not reach the model"
        );

        state.undo();

        assert_eq!(
            state.doc.hash().expect("still rooted"),
            before,
            "undo left part of the pad in the model"
        );
        // Checking the hash alone is not enough: reversing the last command on
        // its own repoints the root, which restores the shape while leaving the
        // pad, its placement and its boolean orphaned in the arena.
        assert!(
            !state.doc.arena().is_alive(pad),
            "the pad was orphaned rather than undone"
        );
        assert_eq!(
            state.doc.log_len(),
            steps,
            "undo took back only part of the action"
        );
    }

    /// Every tool has to be one undo step, not only the ones that got a test of
    /// their own. A pocket and a finished sketch are three or four commands each,
    /// exactly like a pad.
    #[test]
    fn every_tool_is_a_single_undo_step() {
        /// A named tool, driven end to end the way the interface drives it.
        type Action = (&'static str, fn(&mut AppState));

        let actions: [Action; 3] = [
            ("pocket", |s| {
                s.add_pocket(sc_geom::Profile::Circle { radius: 3.0 }, "Hole");
            }),
            ("sketch", |s| {
                s.start_sketch();
                for (x, y) in [(-8.0, -8.0), (8.0, -8.0), (8.0, 8.0)] {
                    s.add_sketch_point(Vec2::new(x, y));
                }
                s.finish_sketch();
            }),
            ("modifier", |s| {
                s.wrap_selection(
                    |child| Node::Shell {
                        child,
                        thickness: 1.0,
                    },
                    "Shell",
                );
            }),
        ];

        for (name, run) in actions {
            let mut state = AppState::new();
            state.add_body(
                Node::Box {
                    half: Vec3::splat(20.0),
                    round: 0.0,
                },
                "Block",
            );
            let before = state.doc.hash().expect("a rooted model hashes");
            let live = state.doc.arena().live_ids().count();
            let steps = state.doc.log_len();

            run(&mut state);
            assert_ne!(
                state.doc.hash().expect("still rooted"),
                before,
                "{name} changed nothing"
            );

            state.undo();

            assert_eq!(
                state.doc.hash().expect("still rooted"),
                before,
                "{name}: one undo left it half applied"
            );
            assert_eq!(
                state.doc.arena().live_ids().count(),
                live,
                "{name}: one undo left nodes orphaned in the arena"
            );
            assert_eq!(
                state.doc.log_len(),
                steps,
                "{name}: one undo took back only part of the action"
            );
        }
    }

    /// And one press of redo has to put all of it back.
    #[test]
    fn one_redo_rebuilds_a_whole_feature() {
        let mut state = AppState::new();
        state.add_body(Node::Sphere { radius: 20.0 }, "Ball");
        state.add_body(
            Node::Box {
                half: Vec3::splat(10.0),
                round: 0.0,
            },
            "Block",
        );
        let block = state.selected.expect("the new body is selected");
        let after = state.doc.hash().expect("a rooted model hashes");
        let steps = state.doc.log_len();

        state.undo();
        assert!(
            !state.doc.arena().is_alive(block),
            "undo left the body behind"
        );

        state.redo();

        assert_eq!(
            state.doc.hash().expect("still rooted"),
            after,
            "redo rebuilt only part of the body"
        );
        assert_eq!(state.doc.name(block), Some("Block"), "the label came back");
        assert_eq!(
            state.doc.log_len(),
            steps,
            "redo replayed only part of the action"
        );
    }

    /// A 40 by 30 base pad with a square boss sketched on its top face.
    ///
    /// Returns the state, the base extrusion and the boss extrusion, which are
    /// the two nodes a dimension edit lands on.
    fn base_with_a_boss(base_depth: f32, boss_depth: f32) -> (AppState, NodeId, NodeId) {
        let mut state = AppState::new();
        state.new_document();
        state.extrude_height = base_depth;
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 40.0,
                height: 30.0,
            },
            "Base",
        );
        let base = state.selected.expect("the base pad is selected");

        state.select(Some(base));
        state.attach_to_selection();
        state.start_sketch();
        for (x, y) in [(-5.0, -5.0), (5.0, -5.0), (5.0, 5.0), (-5.0, 5.0)] {
            state.add_sketch_point(Vec2::new(x, y));
        }
        state.extrude_height = boss_depth;
        state.finish_sketch();
        let placed = state.selected.expect("the boss is selected");
        let boss = extrude_under(&state, placed);
        (state, base, boss)
    }

    /// The extrusion under a placement, which is what carries the depth.
    fn extrude_under(state: &AppState, id: NodeId) -> NodeId {
        match state.doc.arena().get(id) {
            Some(Node::Extrude { .. }) => id,
            Some(node) => node
                .children()
                .next()
                .map(|c| extrude_under(state, c))
                .expect("a placement has a child"),
            None => panic!("{id} is not in the arena"),
        }
    }

    fn solid_at_point(state: &AppState, p: Vec3) -> bool {
        let root = state.doc.root().expect("rooted");
        sc_geom::eval(state.doc.arena(), root, p) < 0.0
    }

    fn solid_at(state: &AppState, z: f32) -> bool {
        solid_at_point(state, Vec3::new(0.0, 0.0, z))
    }

    fn z_span(state: &AppState) -> (f32, f32) {
        let b = state.doc.bounds().expect("rooted");
        (b.min.z, b.max.z)
    }

    /// A feature built on a face has to follow that face. Frozen at creation it
    /// leaves the model in two disconnected pieces with a gap in between, and
    /// nothing in the interface says so.
    #[test]
    fn an_attached_pad_follows_its_base() {
        let (mut state, base, _boss) = base_with_a_boss(10.0, 5.0);
        assert_eq!(z_span(&state), (0.0, 15.0), "the boss did not start on top");

        state.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 4.0,
        });

        let (lo, hi) = z_span(&state);
        assert!(
            lo.abs() < 1.0e-3 && (hi - 9.0).abs() < 1.0e-3,
            "the boss did not follow the face: stack spans {lo}..{hi}"
        );
        for z in [1.0, 3.9, 4.1, 6.0, 8.9] {
            assert!(solid_at(&state, z), "gap in the stack at z={z}");
        }
    }

    /// Derived placements chain: a boss on a boss on a base. Moving the base
    /// moves the face the first boss sits on, which moves the face under the
    /// second.
    #[test]
    fn a_chain_of_attached_features_all_follow() {
        let (mut state, base, boss) = base_with_a_boss(10.0, 5.0);

        state.select(Some(boss));
        state.attach_to_selection();
        state.start_sketch();
        for (x, y) in [(-2.0, -2.0), (2.0, -2.0), (2.0, 2.0), (-2.0, 2.0)] {
            state.add_sketch_point(Vec2::new(x, y));
        }
        state.extrude_height = 3.0;
        state.finish_sketch();
        assert_eq!(z_span(&state), (0.0, 18.0), "the chain did not stack up");

        state.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 4.0,
        });

        let (lo, hi) = z_span(&state);
        assert!(
            lo.abs() < 1.0e-3 && (hi - 12.0).abs() < 1.0e-3,
            "the chain did not follow: stack spans {lo}..{hi}"
        );
        for z in [1.0, 4.1, 8.9, 9.1, 11.9] {
            assert!(solid_at(&state, z), "gap in the chain at z={z}");
        }
    }

    /// A pocket follows the face it was cut from, and keeps the overshoot that
    /// makes it start outside the material. Regeneration rewrites a derived
    /// placement wholesale, so an offset folded into that placement would be
    /// erased the first time the face moved, leaving a skin over the hole.
    #[test]
    fn a_pocket_follows_the_face_it_was_cut_from() {
        let mut state = AppState::new();
        state.new_document();
        state.extrude_height = 10.0;
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 40.0,
                height: 30.0,
            },
            "Base",
        );
        let base = state.selected.expect("the base pad is selected");
        state.select(Some(base));
        state.attach_to_selection();
        state.add_pocket(sc_geom::Profile::Circle { radius: 4.0 }, "Bore");

        for z in [0.5, 5.0, 9.5] {
            assert!(!solid_at(&state, z), "the bore did not open at z={z}");
        }

        state.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 6.0,
        });

        for z in [0.5, 3.0, 5.5] {
            assert!(!solid_at(&state, z), "the bore closed up at z={z}");
        }
        assert!(
            solid_at_point(&state, Vec3::new(15.0, 0.0, 3.0)),
            "the part was cut away entirely"
        );
    }

    /// Bug A: the pad tools resolved their placement from the datum plane, so a
    /// rectangle dropped on an attached face landed buried inside the base.
    #[test]
    fn the_pad_tools_honour_the_attached_face() {
        for profile in [
            sc_geom::Profile::Rect {
                width: 10.0,
                height: 10.0,
            },
            sc_geom::Profile::Circle { radius: 5.0 },
            sc_geom::Profile::RegularPolygon {
                sides: 6,
                radius: 5.0,
            },
        ] {
            let mut state = AppState::new();
            state.new_document();
            state.extrude_height = 10.0;
            state.add_pad(
                sc_geom::Profile::Rect {
                    width: 40.0,
                    height: 30.0,
                },
                "Base",
            );
            let base = state.selected.expect("the base pad is selected");
            state.select(Some(base));
            state.attach_to_selection();

            state.extrude_height = 5.0;
            state.add_pad(profile.clone(), "Boss");

            let (lo, hi) = z_span(&state);
            assert!(
                lo.abs() < 1.0e-3 && (hi - 15.0).abs() < 1.0e-3,
                "{profile:?} ignored the attached face: stack spans {lo}..{hi}"
            );
            assert!(solid_at(&state, 12.5), "{profile:?} is not on the face");
        }
    }

    /// A user selects the finished feature, which is whatever wraps the pad. The
    /// Face button was keyed on the bare extrude, so selecting the offset or the
    /// shell that a user can actually see greyed it out while pointing straight
    /// at the face it describes.
    #[test]
    fn a_face_is_found_through_the_wrappers_around_a_pad() {
        let mut state = AppState::new();
        state.new_document();
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 20.0,
                height: 20.0,
            },
            "Pad",
        );
        let pad = state.selected.expect("the pad is selected");

        for wrap in [
            Node::Offset {
                child: pad,
                distance: 1.0,
            },
            Node::Shell {
                child: pad,
                thickness: 1.0,
            },
        ] {
            let id = state
                .apply(Command::Add { node: wrap })
                .expect("valid wrapper");
            assert_eq!(
                state.attachable_face(id),
                Some(pad),
                "{} hid the pad underneath it",
                state.doc.arena().get(id).expect("just added").kind()
            );
        }

        state.select(Some(pad));
        state.attach_to_selection();
        assert_eq!(state.attached_to, Some(pad), "a bare pad still attaches");
    }

    /// Both sides of a boolean have a face, so there is no single answer and the
    /// button has to stay off rather than guess.
    #[test]
    fn a_boolean_has_no_single_face_to_attach_to() {
        let mut state = AppState::new();
        state.new_document();
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 20.0,
                height: 20.0,
            },
            "Pad",
        );
        state.add_body(Node::Sphere { radius: 5.0 }, "Ball");
        let root = state.doc.root().expect("the union became the root");

        assert_eq!(state.attachable_face(root), None);

        state.select(Some(root));
        state.attach_to_selection();
        assert_eq!(state.attached_to, None, "it guessed a face anyway");
    }

    fn attached_stack(state: &mut AppState) -> (NodeId, NodeId) {
        state.new_document();
        state.extrude_height = 10.0;
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 40.0,
                height: 40.0,
            },
            "Base",
        );
        let base = state.selected.expect("the base is selected");
        state.attach_to_selection();
        state.extrude_height = 5.0;
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 8.0,
                height: 8.0,
            },
            "Boss",
        );
        let placed = state
            .doc
            .arena()
            .parents_of(state.selected.expect("the boss is selected"))
            .into_iter()
            .find(|id| state.doc.arena().get(*id).and_then(Node::derived_from) == Some(base))
            .expect("the boss was placed on the face");
        (base, placed)
    }

    /// Moving an attached feature must not cost it the attachment. The move goes
    /// underneath the derived placement, so the boss slides across the face and
    /// still follows it when the base changes height.
    #[test]
    fn moving_an_attached_feature_keeps_it_on_its_face() {
        let mut state = AppState::new();
        let (base, placed) = attached_stack(&mut state);

        state.select(Some(placed));
        state.move_selection(Vec3::new(12.0, 0.0, 0.0));

        assert_eq!(
            state.doc.arena().get(placed).and_then(Node::derived_from),
            Some(base),
            "the move broke the attachment"
        );
        assert!(
            solid_at_point(&state, Vec3::new(12.0, 0.0, 12.0)),
            "the boss did not move to where it was sent"
        );

        state.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 4.0,
        });
        assert!(
            solid_at_point(&state, Vec3::new(12.0, 0.0, 6.0)),
            "the moved boss stopped following its face"
        );
    }

    /// Two moves leave one offset behind, not one per nudge.
    #[test]
    fn repeated_moves_accumulate_into_a_single_offset() {
        let mut state = AppState::new();
        let (_, placed) = attached_stack(&mut state);
        state.select(Some(placed));

        state.move_selection(Vec3::new(6.0, 0.0, 0.0));
        let after_one = state.doc.arena().live_ids().count();
        state.move_selection(Vec3::new(6.0, 0.0, 0.0));

        assert_eq!(
            state.doc.arena().live_ids().count(),
            after_one,
            "the second move stacked another placement instead of accumulating"
        );
        assert!(
            solid_at_point(&state, Vec3::new(12.0, 0.0, 12.0)),
            "the two moves did not add up"
        );
    }

    /// A through cut has to stay through. Sizing one from the model as it stood
    /// at the moment of the cut leaves it frozen, so growing the part turns the
    /// hole into a blind recess with a skin over the far end.
    #[test]
    fn a_through_cut_stays_open_when_the_part_grows() {
        let mut state = AppState::new();
        state.new_document();
        state.extrude_height = 10.0;
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 40.0,
                height: 40.0,
            },
            "Base",
        );
        let base = state.selected.expect("the base is selected");
        state.add_pocket(sc_geom::Profile::Circle { radius: 4.0 }, "Bore");

        for z in [0.5, 5.0, 9.5] {
            assert!(!solid_at(&state, z), "the bore did not open at z={z}");
        }

        state.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 30.0,
        });

        for z in [0.5, 15.0, 29.5] {
            assert!(
                !solid_at(&state, z),
                "the bore closed up at z={z} after the part grew"
            );
        }
        assert!(
            solid_at_point(&state, Vec3::new(15.0, 0.0, 29.5)),
            "the part did not actually grow"
        );
    }

    /// The prism is unbounded, but it is only ever a cutting tool, and a
    /// difference takes its bounds from the solid being cut. The infinity must
    /// not escape into the camera framing or the mesher.
    #[test]
    fn a_through_cut_leaves_the_model_bounded() {
        let mut state = AppState::new();
        state.new_document();
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 40.0,
                height: 40.0,
            },
            "Base",
        );
        state.add_pocket(sc_geom::Profile::Circle { radius: 4.0 }, "Bore");

        let b = state.doc.bounds().expect("a rooted model has bounds");
        assert!(
            b.min.is_finite() && b.max.is_finite(),
            "a through cut made the model unbounded: {b:?}"
        );
    }

    /// Detaching is the explicit way to stop following a face, and it must not
    /// move anything at the moment the link is cut.
    #[test]
    fn detaching_leaves_the_feature_exactly_where_it_is() {
        let mut state = AppState::new();
        let (base, placed) = attached_stack(&mut state);
        let before = state.doc.hash().expect("a rooted model hashes");

        state.select(Some(placed));
        state.detach_selection();

        assert_eq!(
            state.doc.hash().expect("still rooted"),
            before,
            "detaching moved the feature"
        );
        assert_eq!(
            state.doc.arena().get(placed).and_then(Node::derived_from),
            None,
            "the link survived the detach"
        );

        state.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 4.0,
        });
        assert!(
            !solid_at_point(&state, Vec3::new(0.0, 0.0, 6.0)),
            "a detached feature followed its old face anyway"
        );
    }

    /// A derived placement is rewritten by regeneration, so an edit to its
    /// coordinates is refused rather than silently reverted. It still reports
    /// them, because they are the node's data and the geometry hash is built
    /// from what `params` returns.
    #[test]
    fn a_derived_placement_refuses_coordinate_edits_but_still_reports_them() {
        let mut state = AppState::new();
        let (_, placed) = attached_stack(&mut state);

        let names: Vec<&str> = state
            .doc
            .arena()
            .get(placed)
            .expect("still there")
            .params()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(names.contains(&"z"), "got {names:?}");

        let before = state.doc.hash().expect("a rooted model hashes");
        state.apply(Command::SetParam {
            id: placed,
            name: "z".into(),
            value: 99.0,
        });
        assert_eq!(
            state.doc.hash().expect("still rooted"),
            before,
            "a refused edit changed the model"
        );
    }

    /// Two placements at different spots on the same face must not hash alike.
    /// Emptying `params` for a derived placement would have made them.
    #[test]
    fn moving_an_attached_feature_changes_the_hash() {
        let mut state = AppState::new();
        let (_, placed) = attached_stack(&mut state);
        let before = state.doc.hash().expect("a rooted model hashes");

        state.select(Some(placed));
        state.move_selection(Vec3::new(9.0, 0.0, 0.0));

        assert_ne!(state.doc.hash().expect("still rooted"), before);
    }

    /// A grip has to be where the pointer will look for it. The projection is
    /// shared between the drawing and the hit testing precisely so the two
    /// cannot disagree, and this checks the shared answer is sane.
    #[test]
    fn a_grip_projects_inside_the_viewport_it_belongs_to() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(
            Node::Box {
                half: Vec3::splat(10.0),
                round: 0.0,
            },
            "Block",
        );
        let viewport = [0.0, 0.0, 1200.0, 800.0];

        let grips = state.grips();
        assert_eq!(grips.len(), 3, "a box has three half extents");
        for grip in grips {
            let (at, axis, gain) = state
                .grip_on_screen(&grip, viewport)
                .unwrap_or_else(|| panic!("{} did not project", grip.param));
            assert!(
                at.x > 0.0 && at.x < 1200.0 && at.y > 0.0 && at.y < 800.0,
                "{} landed at {at:?}, outside the viewport",
                grip.param
            );
            assert!((axis.length() - 1.0).abs() < 1.0e-4, "axis is not a unit");
            assert!(gain > 0.0 && gain.is_finite(), "gain of {gain}");
        }
    }

    /// Dragging a grip in the direction it points has to make the feature
    /// bigger, and the geometry has to keep up with the pointer rather than
    /// lagging it by the gain.
    #[test]
    fn dragging_a_grip_outward_resizes_the_feature() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(
            Node::Box {
                half: Vec3::splat(10.0),
                round: 0.0,
            },
            "Block",
        );
        let viewport = [0.0, 0.0, 1200.0, 800.0];
        let grip = state
            .grips()
            .into_iter()
            .find(|g| g.param == "half_x")
            .expect("a box has a half_x grip");
        let (at, axis, gain) = state.grip_on_screen(&grip, viewport).expect("projects");

        state.begin_drag(Drag {
            node: state.selected.expect("selected"),
            param: grip.param,
            from: grip.value,
            value: grip.value,
            axis,
            gain,
            origin: at,
        });
        // Forty points along the grip's own direction.
        state.drag_to(at + axis * 40.0);
        state.finish_drag();

        let after = state
            .doc
            .arena()
            .get(state.selected.expect("selected"))
            .expect("still there")
            .params()
            .into_iter()
            .find(|(n, _)| *n == "half_x")
            .expect("half_x")
            .1;
        let expected = grip.value + 40.0 * gain;
        assert!(
            (after - expected).abs() < 0.01,
            "half_x went to {after}, expected {expected}"
        );
        assert!(after > grip.value, "dragging outward shrank it");
    }

    /// However many parameter changes a drag makes, it is one thing the user
    /// did and takes one press of undo to take back.
    #[test]
    fn a_whole_drag_is_a_single_undo_step() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 10.0 }, "Ball");
        let before = state.doc.hash().expect("rooted");
        let viewport = [0.0, 0.0, 1200.0, 800.0];
        let grip = state.grips().into_iter().next().expect("a sphere has one");
        let (at, axis, gain) = state.grip_on_screen(&grip, viewport).expect("projects");

        state.begin_drag(Drag {
            node: state.selected.expect("selected"),
            param: grip.param,
            from: grip.value,
            value: grip.value,
            axis,
            gain,
            origin: at,
        });
        for step in 1..=30 {
            state.drag_to(at + axis * step as f32);
        }
        state.finish_drag();
        assert_ne!(
            state.doc.hash().expect("rooted"),
            before,
            "the drag did nothing"
        );

        state.undo();

        assert_eq!(
            state.doc.hash().expect("rooted"),
            before,
            "one undo left part of the drag applied"
        );
    }

    /// A drag that would take a dimension through zero stops at the limit
    /// instead of being refused. Reporting an error on every frame of a drag
    /// would be noise, and the geometry would stop responding with no
    /// explanation of why.
    #[test]
    fn a_drag_stops_at_the_limit_rather_than_erroring() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 10.0 }, "Ball");
        let viewport = [0.0, 0.0, 1200.0, 800.0];
        let grip = state.grips().into_iter().next().expect("a sphere has one");
        let (at, axis, gain) = state.grip_on_screen(&grip, viewport).expect("projects");

        state.begin_drag(Drag {
            node: state.selected.expect("selected"),
            param: grip.param,
            from: grip.value,
            value: grip.value,
            axis,
            gain,
            origin: at,
        });
        // Far enough inward to take the radius well past zero.
        state.drag_to(at - axis * 10_000.0);
        state.finish_drag();

        let node = state
            .doc
            .arena()
            .get(state.selected.expect("selected"))
            .expect("the node survived");
        assert!(node.is_valid(), "the drag left an invalid node behind");
    }

    /// Escape puts it back where it was, without needing undo.
    #[test]
    fn cancelling_a_drag_restores_the_dimension() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 10.0 }, "Ball");
        let before = state.doc.hash().expect("rooted");
        let viewport = [0.0, 0.0, 1200.0, 800.0];
        let grip = state.grips().into_iter().next().expect("a sphere has one");
        let (at, axis, gain) = state.grip_on_screen(&grip, viewport).expect("projects");

        state.begin_drag(Drag {
            node: state.selected.expect("selected"),
            param: grip.param,
            from: grip.value,
            value: grip.value,
            axis,
            gain,
            origin: at,
        });
        state.drag_to(at + axis * 60.0);
        state.cancel_drag();

        assert!(state.drag.is_none(), "the drag outlived the cancel");
        assert_eq!(
            state.doc.hash().expect("rooted"),
            before,
            "cancelling left the dimension moved"
        );
    }

    /// Nothing is selected, so there is nothing to grab.
    #[test]
    fn an_empty_selection_has_no_grips() {
        let mut state = AppState::new();
        state.new_document();
        assert!(state.grips().is_empty());
        state.add_body(Node::Sphere { radius: 4.0 }, "Ball");
        state.select(None);
        assert!(state.grips().is_empty());
    }

    /// A hole goes where it was put. Every cut tool used to drop its feature at
    /// the plane's origin, which meant every hole landed in the middle of the
    /// part and then had to be moved with numbers.
    #[test]
    fn an_armed_cut_lands_where_it_is_placed() {
        let mut state = AppState::new();
        state.new_document();
        state.add_pad(
            sc_geom::Profile::Rect {
                width: 60.0,
                height: 60.0,
            },
            "Plate",
        );
        assert!(solid_at_point(&state, Vec3::new(18.0, 0.0, 5.0)));

        state.arm(Armed {
            kind: Placing::Pocket,
            profile: sc_geom::Profile::Circle { radius: 5.0 },
            label: "Hole",
        });
        state.place_armed(Vec2::new(18.0, 0.0));

        assert!(state.armed.is_none(), "the tool stayed armed after placing");
        assert!(
            !solid_at_point(&state, Vec3::new(18.0, 0.0, 5.0)),
            "the hole did not land where it was placed"
        );
        assert!(
            solid_at_point(&state, Vec3::new(0.0, 0.0, 5.0)),
            "the hole landed at the origin instead"
        );
    }

    /// Clicking the armed tool again puts it away, so there is a way out that
    /// does not require knowing about Escape.
    #[test]
    fn arming_the_same_tool_twice_disarms_it() {
        let mut state = AppState::new();
        let armed = || Armed {
            kind: Placing::Pocket,
            profile: sc_geom::Profile::Circle { radius: 5.0 },
            label: "Hole",
        };
        state.arm(armed());
        assert!(state.armed.is_some());
        state.arm(armed());
        assert!(state.armed.is_none());
    }

    /// Dragging a feature around must not leave a placement behind every time.
    /// Ten nudges should leave the tree exactly as one nudge does.
    #[test]
    fn dragging_a_feature_reuses_one_placement() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");

        let id = state
            .begin_move(Vec3::ZERO, 940.0)
            .expect("something to move");
        state.move_to(id, Vec3::new(10.0, 0.0, 0.0));
        state.finish_move();
        let after_one = state.doc.arena().live_ids().count();

        let id = state.begin_move(Vec3::ZERO, 940.0).expect("still movable");
        state.move_to(id, Vec3::new(20.0, 5.0, 0.0));
        state.finish_move();

        assert_eq!(
            state.doc.arena().live_ids().count(),
            after_one,
            "a second drag stacked another placement"
        );
        assert!(
            solid_at_point(&state, Vec3::new(20.0, 5.0, 0.0)),
            "the feature is not where it was dragged"
        );
    }

    /// A drag writes three parameters many times over, and is still one thing
    /// the user did.
    #[test]
    fn a_whole_free_drag_is_a_single_undo_step() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        let before = state.doc.hash().expect("rooted");

        let id = state.begin_move(Vec3::ZERO, 940.0).expect("movable");
        for step in 1..=20 {
            state.move_to(id, Vec3::new(step as f32, 0.0, 0.0));
        }
        state.finish_move();
        assert_ne!(state.doc.hash().expect("rooted"), before);

        state.undo();
        assert_eq!(
            state.doc.hash().expect("rooted"),
            before,
            "one undo left part of the drag applied"
        );
    }

    /// The plane a free drag moves across follows the camera, so looking down
    /// slides a part across the plate and looking from the side lifts it.
    #[test]
    fn the_drag_plane_faces_the_camera() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");

        state.look_along(Vec3::Z);
        state.rig.snap_to(state.rig.goal);
        assert_eq!(
            state.drag_plane(),
            Vec3::Z,
            "looking down should drag in XY"
        );

        state.look_along(Vec3::X);
        state.rig.snap_to(state.rig.goal);
        assert_eq!(
            state.drag_plane(),
            Vec3::X,
            "looking along X should drag in YZ"
        );
    }

    fn placed_block(state: &mut AppState) {
        state.new_document();
        state.add_body(
            Node::Box {
                half: Vec3::splat(8.0),
                round: 0.0,
            },
            "Block",
        );
    }

    /// A locked drag moves along one axis and nowhere else. Without it a plane
    /// drag spends two degrees of freedom on every move, so nudging something
    /// sideways also shifts it forwards.
    #[test]
    fn a_locked_move_changes_only_its_own_axis() {
        let mut state = AppState::new();
        placed_block(&mut state);
        let id = state.begin_move(Vec3::ZERO, 940.0).expect("movable");
        state.constrain_move(Some(Vec3::X));

        // A pointer travelling diagonally across the plane.
        state.move_to_plane(Vec3::new(0.0, 0.0, 0.0));
        state.move_to_plane(Vec3::new(20.0, 17.0, 9.0));
        state.finish_move();

        let at = state
            .doc
            .arena()
            .get(id)
            .and_then(|n| match n {
                Node::Transform { xform, .. } => Some(xform.translation),
                _ => None,
            })
            .expect("a placement");
        assert!((at.x - 20.0).abs() < 0.01, "x did not follow, got {at:?}");
        assert!(
            at.y.abs() < 0.01 && at.z.abs() < 0.01,
            "it drifted to {at:?}"
        );
    }

    /// Locking part way through must not fling the feature back along the path
    /// it has already travelled. The lock is measured from where it is now.
    #[test]
    fn locking_part_way_through_keeps_the_ground_already_covered() {
        let mut state = AppState::new();
        placed_block(&mut state);
        let id = state.begin_move(Vec3::ZERO, 940.0).expect("movable");
        state.move_to_plane(Vec3::new(0.0, 12.0, 0.0));

        state.constrain_move(Some(Vec3::X));
        state.move_to_plane(Vec3::new(5.0, 12.0, 0.0));
        // Diagonal, so a drag that is not actually locked carries y and z with
        // it and the assertions below catch that rather than agreeing with it.
        state.move_to_plane(Vec3::new(9.0, 30.0, 6.0));
        state.finish_move();

        let at = state
            .doc
            .arena()
            .get(id)
            .and_then(|n| match n {
                Node::Transform { xform, .. } => Some(xform.translation),
                _ => None,
            })
            .expect("a placement");
        assert!(
            (at.y - 12.0).abs() < 0.01,
            "locking threw away the travel already made: {at:?}"
        );
        assert!(
            (at.x - 4.0).abs() < 0.01,
            "x should have moved 4mm, got {at:?}"
        );
    }

    /// Pressing the same axis again lets go.
    #[test]
    fn locking_the_same_axis_twice_releases_it() {
        let mut state = AppState::new();
        placed_block(&mut state);
        state.begin_move(Vec3::ZERO, 940.0).expect("movable");
        state.constrain_move(Some(Vec3::Y));
        assert_eq!(state.move_axis(), Some(Vec3::Y));
        state.constrain_move(None);
        assert_eq!(state.move_axis(), None);
    }

    /// Puts one sphere at an unround coordinate and returns the placement of a
    /// second one, mid-drag, ready to be aimed near it.
    fn two_spheres(state: &mut AppState) -> NodeId {
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "First");
        let first = state
            .begin_move(Vec3::ZERO, 940.0)
            .expect("something to move");
        // Placed rather than moved, so it sits somewhere the grid would never
        // put it and a test that passes cannot be passing by rounding.
        state.place_at(first, Vec3::new(20.4, 0.0, 0.0));
        state.finish_move();

        state.add_body(Node::Sphere { radius: 6.0 }, "Second");
        state.begin_move(Vec3::ZERO, 940.0).expect("movable")
    }

    /// The gizmo has to sit on the thing it moves. Dragging a feature makes the
    /// selection its placement, and a placement's own translation is exactly
    /// what the gizmo writes, so leaving it out parks the arms at the parent's
    /// origin from the first drag onwards.
    #[test]
    fn the_gizmo_follows_the_feature_it_moves() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        let id = state.begin_move(Vec3::ZERO, 940.0).expect("movable");
        state.move_to(id, Vec3::new(10.0, 4.0, 0.0));
        state.finish_move();

        assert_eq!(
            state.selected,
            Some(id),
            "the drag did not leave the placement selected"
        );
        let at = state.selection_origin().expect("somewhere");
        assert_eq!(
            at,
            Vec3::new(10.0, 4.0, 0.0),
            "the gizmo stayed behind at {at:?}"
        );
    }

    /// The whole point. A coordinate a fraction off a neighbour's centreline
    /// has to land on the centreline, not on the nearest round number, or
    /// lining two features up by hand is impossible however carefully you drag.
    #[test]
    fn a_drag_latches_onto_another_feature() {
        let mut state = AppState::new();
        let second = two_spheres(&mut state);

        // 20.1 rounds to 20.0 on a 1mm grid, so a pass here cannot come from
        // rounding: only the neighbour's centre at 20.4 gives 20.4.
        state.move_to(second, Vec3::new(20.1, 0.0, 0.0));
        let at = state.placement_of(second).expect("a placement");
        assert!(
            (at.x - 20.4).abs() < 1.0e-4,
            "did not latch onto the neighbour, landed at {at:?}"
        );
        state.finish_move();
    }

    /// A latch has to be visible, or it reads as the part sticking rather than
    /// as the part lining up.
    #[test]
    fn a_latch_puts_a_guide_on_the_axis_it_latched() {
        let mut state = AppState::new();
        let second = two_spheres(&mut state);
        state.move_to(second, Vec3::new(20.1, 0.0, 0.0));

        assert!(
            state.guides[0].is_some(),
            "no guide on the axis that latched"
        );
        assert!(
            state.guides[1].is_none() && state.guides[2].is_none(),
            "a guide was drawn for an axis that only rounded to the grid"
        );
        state.finish_move();
    }

    /// A feature must not offer itself a coordinate. It moves with the drag, so
    /// it would offer wherever it already is, and the part would refuse to
    /// leave the spot it started from.
    #[test]
    fn a_feature_does_not_snap_to_itself() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Only");
        let id = state.begin_move(Vec3::ZERO, 940.0).expect("movable");
        state.place_at(id, Vec3::new(20.4, 0.0, 0.0));
        state.finish_move();

        // A fresh gesture, so the lines are gathered with the feature already
        // sitting at 20.4. Offering itself that coordinate would hold it there.
        let id = state.begin_move(Vec3::ZERO, 940.0).expect("still movable");
        state.move_to(id, Vec3::new(20.1, 0.0, 0.0));
        let at = state.placement_of(id).expect("a placement");
        assert!(
            (at.x - 20.0).abs() < 1.0e-4,
            "it snapped to its own last position, landing at {at:?}"
        );
        state.finish_move();
    }

    /// Typing is how you state a number rather than hunt for it. The value has
    /// to arrive exactly, which means it must not be rounded to the grid on the
    /// way in: 12.5 on a 1mm grid would come out as 13.
    #[test]
    fn a_typed_distance_moves_exactly_that_far() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        let id = state.begin_move(Vec3::ZERO, 940.0).expect("movable");
        state.constrain_move(Some(Vec3::X));

        for c in "12.5".chars() {
            assert!(state.type_number(c), "rejected {c}");
        }
        state.commit_entry();

        let at = state.placement_of(id).expect("a placement");
        assert!(
            (at.x - 12.5).abs() < 1.0e-4,
            "the typed distance was not used, landed at {at:?}"
        );
        assert!(state.moving.is_none(), "the gesture was left open");
        assert!(state.entry.is_none(), "the number was left behind");
    }

    /// A dimension takes a typed number as the dimension itself, because
    /// "twelve" means a radius of twelve, not twelve more than it was.
    #[test]
    fn a_typed_dimension_is_absolute() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        let id = state.selected.expect("selected");
        state.begin_drag(Drag {
            node: id,
            param: "radius",
            from: 6.0,
            value: 6.0,
            axis: Vec2::X,
            gain: 0.1,
            origin: Vec2::ZERO,
        });

        for c in "9".chars() {
            state.type_number(c);
        }
        state.commit_entry();

        let radius = state
            .doc
            .arena()
            .get(id)
            .and_then(|n| {
                n.params()
                    .iter()
                    .find(|(k, _)| *k == "radius")
                    .map(|(_, v)| *v)
            })
            .expect("a radius");
        assert!((radius - 9.0).abs() < 1.0e-4, "got {radius}");
        assert!(state.drag.is_none(), "the drag was left open");
    }

    /// A typed number is a statement, so a bad one is refused rather than
    /// quietly turned into a different number. A drag stops at the limit
    /// because it is a continuous gesture passing through; typing is not.
    #[test]
    fn a_typed_dimension_that_cannot_be_is_refused() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        let id = state.selected.expect("selected");
        state.begin_drag(Drag {
            node: id,
            param: "radius",
            from: 6.0,
            value: 6.0,
            axis: Vec2::X,
            gain: 0.1,
            origin: Vec2::ZERO,
        });

        for c in "-3".chars() {
            state.type_number(c);
        }
        state.commit_entry();

        assert!(state.drag.is_some(), "a refused number ended the drag");
        let radius = state
            .doc
            .arena()
            .get(id)
            .and_then(|n| {
                n.params()
                    .iter()
                    .find(|(k, _)| *k == "radius")
                    .map(|(_, v)| *v)
            })
            .expect("a radius");
        assert!(
            (radius - 6.0).abs() < 1.0e-4,
            "it was applied anyway: {radius}"
        );
        state.cancel_drag();
    }

    /// Escape while typing means "not that number", not "not any of this". The
    /// drag is still wanted; it was only the number that was wrong.
    #[test]
    fn escape_clears_the_number_and_keeps_the_drag() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        state.begin_move(Vec3::ZERO, 940.0).expect("movable");
        state.constrain_move(Some(Vec3::X));
        state.type_number('7');

        state.cancel_entry();
        assert!(state.entry.is_none(), "the number survived");
        assert!(state.moving.is_some(), "escape ended the drag as well");
        state.finish_move();
    }

    /// A typed number needs a direction. Somebody who has already dragged a
    /// long way has said which one; somebody who has not has said nothing, and
    /// guessing would move the part somewhere they did not ask for.
    #[test]
    fn a_typed_distance_with_no_direction_is_refused() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        let id = state.begin_move(Vec3::ZERO, 940.0).expect("movable");
        state.type_number('7');
        state.commit_entry();

        assert!(state.moving.is_some(), "it committed without a direction");
        let at = state.placement_of(id).expect("a placement");
        assert_eq!(at, Vec3::ZERO, "the part moved anyway, to {at:?}");

        // Having dragged, the direction is no longer in doubt.
        state.move_to(id, Vec3::new(9.0, 0.0, 0.0));
        state.commit_entry();
        let at = state.placement_of(id).expect("a placement");
        assert!((at.x - 7.0).abs() < 1.0e-4, "landed at {at:?}");
    }

    /// Zero gets a wider catchment than the grid alone gives it. A feature on an
    /// axis is something people deliberately want and then verify by reading the
    /// number back, so landing on 0.4 when aiming at 0 is a worse answer than
    /// the grid spacing suggests.
    #[test]
    fn a_position_near_zero_snaps_to_it() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        state.grid = 0.5;

        let id = state
            .begin_move(Vec3::ZERO, 940.0)
            .expect("something to move");
        // Chosen so plain rounding would not give zero: 0.3 rounds to 0.5, and
        // only the wider catchment brings it home. A value that rounds to zero
        // anyway would pass with the pull removed and prove nothing.
        state.move_to(id, Vec3::new(0.3, -0.3, 0.3));
        let pulled = state.placement_of(id).expect("a placement");
        assert_eq!(pulled, Vec3::ZERO, "zero did not pull, got {pulled:?}");

        // But not so wide that the grid line next to zero is unreachable.
        state.move_to(id, Vec3::new(0.5, 0.0, 0.0));
        let near = state.placement_of(id).expect("a placement");
        assert!(
            (near.x - 0.5).abs() < 0.01,
            "the first grid line was swallowed, got {near:?}"
        );
        state.move_to(id, Vec3::new(7.1, 0.0, 0.0));
        let far = state.placement_of(id).expect("a placement");
        assert!(
            (far.x - 7.0).abs() < 0.01,
            "ordinary rounding broke, got {far:?}"
        );
        state.finish_move();
    }

    /// The plane a locked drag is measured against has to contain the axis, or
    /// projecting onto it is ill conditioned exactly when the axis points away
    /// from the camera.
    #[test]
    fn a_locked_drag_measures_against_a_plane_containing_its_axis() {
        let mut state = AppState::new();
        placed_block(&mut state);
        for (_, axis) in crate::state::AXES {
            let normal = state.plane_for_axis(axis);
            assert!(
                normal.dot(axis).abs() < 1.0e-4,
                "the plane for {axis:?} does not contain it, normal {normal:?}"
            );
            assert!((normal.length() - 1.0).abs() < 1.0e-4);
        }
    }

    /// The gizmo stays the same size on screen however far away the part is,
    /// or it is unusable on a large one and swallows a small one.
    #[test]
    fn the_gizmo_keeps_its_size_on_screen() {
        let mut state = AppState::new();
        placed_block(&mut state);
        let viewport = [0.0, 0.0, 1200.0, 800.0];

        // Each measured against its own camera. Projecting both sets with the
        // same one measures nothing, which is what the first version of this
        // test did: it reported a fourfold change that was entirely its own.
        let measure = |state: &AppState| {
            let arms = state.move_arms(viewport);
            assert_eq!(arms.len(), 3, "one arm per axis");
            let tail = state
                .world_to_screen(arms[0].tail, viewport)
                .expect("on screen");
            let head = state
                .world_to_screen(arms[0].head, viewport)
                .expect("on screen");
            (
                (arms[0].head - arms[0].tail).length(),
                (head - tail).length(),
            )
        };

        let (near_world, near_points) = measure(&state);
        state.rig.goal.distance *= 4.0;
        state.rig.snap_to(state.rig.goal);
        let (far_world, far_points) = measure(&state);

        assert!(
            far_world > near_world * 3.5,
            "the arm did not grow with the distance: {near_world} then {far_world}"
        );
        assert!(
            (near_points - far_points).abs() < 4.0,
            "on screen it went from {near_points} to {far_points} points"
        );
    }

    /// A copy is independent. Sharing the nodes would be cheaper and is exactly
    /// what a pattern wants, but here it would mean resizing one hole resizes
    /// the other, which is not what the word means.
    #[test]
    fn a_copy_can_be_edited_without_touching_the_original() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        let original = state.selected.expect("selected");

        state.duplicate_selection();
        let copy = state
            .attachable_face(state.selected.expect("the copy is selected"))
            .or(state.selected)
            .expect("a copy");
        assert_ne!(copy, original, "the copy is the original");

        // Reach the copied sphere, whatever placement sits above it.
        let sphere = state.post_order_for_test(copy);
        let sphere = sphere
            .into_iter()
            .find(|id| matches!(state.doc.arena().get(*id), Some(Node::Sphere { .. })))
            .expect("the copy has a sphere");
        assert_ne!(sphere, original, "the copy shares the original's node");

        state.apply(Command::SetParam {
            id: sphere,
            name: "radius".into(),
            value: 20.0,
        });
        let before = state
            .doc
            .arena()
            .get(original)
            .expect("the original survived")
            .params()
            .into_iter()
            .find(|(n, _)| *n == "radius")
            .expect("radius")
            .1;
        assert!(
            (before - 6.0).abs() < 0.01,
            "editing the copy changed the original to {before}"
        );
    }

    /// And the copy is actually in the model, somewhere else.
    #[test]
    fn a_copy_lands_beside_the_original() {
        let mut state = AppState::new();
        state.new_document();
        state.grid = 1.0;
        state.add_body(Node::Sphere { radius: 3.0 }, "Ball");
        let before = state.doc.hash().expect("rooted");

        state.duplicate_selection();

        assert_ne!(
            state.doc.hash().expect("rooted"),
            before,
            "the copy never reached the model"
        );
        assert!(
            solid_at_point(&state, Vec3::ZERO),
            "the original went missing"
        );
        assert!(
            solid_at_point(&state, Vec3::new(4.0, 0.0, 0.0)),
            "the copy is not beside it"
        );
    }

    /// However many nodes a feature is made of, copying it is one thing done.
    #[test]
    fn a_copy_is_a_single_undo_step() {
        let mut state = AppState::new();
        state.load_sample();
        let before = state.doc.hash().expect("rooted");
        let nodes = state.doc.arena().len();

        state.select(state.doc.root());
        state.duplicate_selection();
        assert!(
            state.doc.arena().len() > nodes + 5,
            "nothing much was copied"
        );

        state.undo();

        assert_eq!(
            state.doc.hash().expect("rooted"),
            before,
            "one undo left part of the copy behind"
        );
    }

    /// A node reached down two branches is copied once, so the copy keeps the
    /// sharing the original had rather than quietly doubling in size.
    #[test]
    fn a_shared_node_is_copied_once() {
        let mut state = AppState::new();
        state.new_document();
        let ball = state
            .apply(Command::Add {
                node: Node::Sphere { radius: 4.0 },
            })
            .expect("valid");
        let both = state
            .apply(Command::Add {
                node: Node::Union {
                    a: ball,
                    b: ball,
                    smooth: 0.0,
                },
            })
            .expect("valid");
        state.apply(Command::SetRoot { root: Some(both) });
        state.select(Some(both));

        let before = state.doc.arena().len();
        state.duplicate_selection();
        let added = state.doc.arena().len() - before;

        // The union, the one sphere, the placement and the joining union.
        assert!(
            added <= 4,
            "copying a shared node made {added} nodes, so it was copied twice"
        );
    }

    /// Framing something that was already deleted must not move the camera.
    #[test]
    fn framing_a_missing_node_does_nothing() {
        let mut state = AppState::new();
        state.add_body(Node::Sphere { radius: 5.0 }, "Ball");
        let id = state.doc.root().expect("the body became the root");
        state.apply(Command::SetRoot { root: None });
        state.apply(Command::Delete { id });

        let before = state.rig.goal.target;
        state.frame_node(id);

        assert_eq!(state.rig.goal.target, before);
    }

    /// Two gestures both waiting for the next click means the one checked
    /// first always wins and the other never fires. `arm` already cancels a
    /// sketch; the other direction was missing, so a sketch started with a pad
    /// still armed spent its first point dropping the pad instead.
    #[test]
    fn starting_a_sketch_puts_an_armed_tool_away() {
        let mut state = AppState::new();
        state.new_document();
        state.arm(Armed {
            kind: Placing::Pad,
            profile: sc_geom::Profile::Circle { radius: 5.0 },
            label: "Boss",
        });
        assert!(state.armed.is_some(), "nothing was armed to begin with");

        state.start_sketch();

        assert!(
            state.armed.is_none(),
            "the armed feature is still there to take the sketch's first click"
        );
        assert!(state.sketch.is_some(), "the sketch did not start");
    }

    /// A drag carries a node id and the value that node started at, and an id
    /// only means something inside the document that issued it. Ctrl+O does not
    /// ask whether a button is down, so a drag can outlive the document it
    /// began in, and the next movement of the pointer then drives whatever node
    /// happens to wear that id in the part that was just opened.
    #[test]
    fn a_drag_does_not_follow_the_pointer_into_the_next_document() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(
            Node::Box {
                half: Vec3::splat(8.0),
                round: 0.0,
            },
            "Block",
        );
        let viewport = [0.0, 0.0, 1200.0, 800.0];
        let grip = state.grips().into_iter().next().expect("a box has three");
        let (at, axis, gain) = state.grip_on_screen(&grip, viewport).expect("projects");
        state.begin_drag(Drag {
            node: state.selected.expect("the body is selected"),
            param: grip.param,
            from: grip.value,
            value: grip.value,
            axis,
            gain,
            origin: at,
        });

        // The sample's first node is a box too, so the id the drag is holding
        // names something that would accept every one of its parameters.
        state.load_sample();
        let loaded = state.doc.hash().expect("rooted");

        for step in 1..=30 {
            state.drag_to(at + axis * step as f32);
        }
        state.finish_drag();

        assert_eq!(
            state.doc.hash().expect("rooted"),
            loaded,
            "the drag went on re-dimensioning the document that replaced its own"
        );
        assert!(
            !state.dirty,
            "a freshly loaded document was edited by a gesture nobody aimed at it"
        );
    }

    /// The same for a free drag, which writes three parameters rather than one.
    #[test]
    fn a_move_does_not_follow_the_pointer_into_the_next_document() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        state.begin_move(Vec3::ZERO, 940.0).expect("movable");

        state.load_sample();
        assert!(
            state.moving.is_none(),
            "the move survived into a document that knows nothing about it"
        );

        let loaded = state.doc.hash().expect("rooted");
        state.move_to_plane(Vec3::new(25.0, 25.0, 0.0));
        state.finish_move();

        assert_eq!(
            state.doc.hash().expect("rooted"),
            loaded,
            "the move went on sliding a feature in the document that replaced its own"
        );
    }

    /// The feature can go out from under a free drag: Delete acts on the
    /// selection, and the selection can change while the button is still down.
    /// Carrying on would put a rejected command on the status bar on every
    /// frame, and leave the step open until a release that means nothing.
    #[test]
    fn a_move_stops_when_its_feature_is_deleted() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 6.0 }, "Ball");
        let id = state.begin_move(Vec3::ZERO, 940.0).expect("movable");

        state.apply(Command::SetRoot { root: None });
        state.apply(Command::Delete { id });
        state.move_to_plane(Vec3::new(10.0, 0.0, 0.0));

        assert!(
            state.moving.is_none(),
            "the move is still writing into a node that is gone"
        );
    }

    /// A selection on a dead id is a property panel editing nothing and a grip
    /// hanging in space over the gap where the feature was. Undo already pruned
    /// it; a delete is the other way a node stops existing.
    #[test]
    fn deleting_the_selection_lets_go_of_it() {
        let mut state = AppState::new();
        state.new_document();
        state.add_body(Node::Sphere { radius: 5.0 }, "Ball");
        let id = state.selected.expect("the body is selected");

        state.apply(Command::SetRoot { root: None });
        state.apply(Command::Delete { id });

        assert_ne!(
            state.selected,
            Some(id),
            "the selection still names a node that was deleted"
        );
    }
}
