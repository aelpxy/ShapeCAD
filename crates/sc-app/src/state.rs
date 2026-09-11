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
    /// Which workspace the top bar has selected.
    pub tab: usize,
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
            tab: 0,
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
        self.doc = Document::new();
        self.selected = None;
        self.path = None;
        self.dirty = false;
        self.field_dirty = true;
        self.status = "New document".to_string();
    }

    /// Loads the built-in reference part.
    pub(crate) fn load_sample(&mut self) {
        self.doc = samples::bracket();
        self.selected = self.doc.root();
        self.frame_camera();
        self.path = None;
        self.dirty = false;
        self.field_dirty = true;
        self.status = "Loaded sample bracket".to_string();
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
                self.selected = self.doc.root();
            }
        }
    }

    /// Adds a pad built from a parametric profile on the current plane.
    ///
    /// Created at a default size and then re-dimensioned in the panel, because
    /// the profile stays parametric: a rectangle is a width and a height for as
    /// long as it exists.
    pub(crate) fn add_pad(&mut self, profile: sc_geom::Profile, label: &str) {
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

            let Some(id) = s.place(extrude, frame) else {
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
            let Some(placed) = s.place_offset(cut, Transform::IDENTITY, frame) else {
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
}
