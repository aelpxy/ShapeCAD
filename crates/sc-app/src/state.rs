//! Application state.
//!
//! Holds the document, the camera and the current selection. Every edit goes
//! through [`AppState::apply`], which is the same `Document::apply` the CLI and
//! (later) the agent layer use — the UI gets no privileged path into the model.

use crate::dialog::{FileBrowser, Purpose};
use crate::plane::SketchPlane;
use crate::settings::Settings;
use sc_doc::{file, samples, Command, Document};
use sc_geom::glam::Vec2;
use sc_geom::glam::Vec3;
use sc_geom::{Node, NodeId};
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
    /// Height the next extrusion will be given, in millimetres.
    pub extrude_height: f32,
    /// The plane the next sketch will be drawn on.
    pub plane: SketchPlane,
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
            sketch: None,
            extrude_height: 10.0,
            grid: 1.0,
            path: None,
            dirty: false,
            browser: None,
            // Replaced once the window exists and the display can be measured.
            ui_scale: 1.0,
            settings: Settings::load(),
        }
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

    /// Adds a primitive and unions it onto the current root, so a new body shows
    /// up immediately instead of sitting orphaned in the tree.
    pub(crate) fn add_body(&mut self, node: Node, label: &str) {
        let Some(id) = self.apply(Command::Add { node }) else {
            return;
        };
        self.apply(Command::SetName {
            id,
            name: Some(label.to_string()),
        });

        if let Some(root) = self.doc.root() {
            let join = Node::Union {
                a: root,
                b: id,
                smooth: 0.0,
            };
            if let Some(union) = self.apply(Command::Add { node: join }) {
                self.apply(Command::SetRoot { root: Some(union) });
            }
        } else {
            self.apply(Command::SetRoot { root: Some(id) });
        }
        self.select(Some(id));
        self.status = format!("Added {label}");
    }

    /// Wraps the selection in a modifier, rerouting the root if needed.
    pub(crate) fn wrap_selection(&mut self, make: impl FnOnce(NodeId) -> Node, label: &str) {
        let Some(target) = self.selected else {
            self.status = "Nothing selected".to_string();
            return;
        };
        let was_root = self.doc.root() == Some(target);
        let Some(wrapped) = self.apply(Command::Add { node: make(target) }) else {
            return;
        };
        if was_root {
            self.apply(Command::SetRoot {
                root: Some(wrapped),
            });
        }
        self.select(Some(wrapped));
        self.status = format!("Applied {label}");
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
        self.rig.goal.look_along(self.plane.normal());
        self.rig.goal.target = Vec3::ZERO;
        self.status = format!("Sketching on {}, click to place points", self.plane.name());
    }

    /// Chooses the plane the next sketch will be drawn on.
    pub(crate) fn set_plane(&mut self, plane: SketchPlane) {
        self.plane = plane;
        self.status = format!("{} plane", plane.name());
        if self.sketch.is_some() {
            // Switching mid-sketch would leave points on the old plane.
            self.sketch = Some(Vec::new());
            self.rig.goal.look_along(plane.normal());
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
        let plane = self.plane;
        let node = Node::Extrude {
            profile: points,
            height,
        };
        if !node.is_valid() {
            self.status = "That profile encloses no area".to_string();
            return;
        }

        let Some(extrude) = self.apply(Command::Add { node }) else {
            return;
        };

        // An extrusion is defined in its own XY plane sweeping along +Z, so
        // placing it on a datum plane is exactly the rotation between the two
        // frames. The build plate needs none, and an identity transform in the
        // tree is just noise.
        let id = if plane == SketchPlane::Xy {
            extrude
        } else {
            let placed = Command::Add {
                node: Node::Transform {
                    child: extrude,
                    xform: plane.placement(),
                },
            };
            match self.apply(placed) {
                Some(id) => id,
                None => return,
            }
        };

        self.apply(Command::SetName {
            id,
            name: Some(format!("Pad on {}", plane.name())),
        });

        if let Some(root) = self.doc.root() {
            let join = Node::Union {
                a: root,
                b: id,
                smooth: 0.0,
            };
            if let Some(union) = self.apply(Command::Add { node: join }) {
                self.apply(Command::SetRoot { root: Some(union) });
            }
        } else {
            self.apply(Command::SetRoot { root: Some(id) });
        }

        self.select(Some(id));
        self.tool = TOOL_SELECT;
        self.status = format!("Extruded {height:.1} mm - adjust the height on the right");
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
            name: "height".into(),
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
            name: "height".into(),
            value: 18.0,
        });
        assert_eq!(
            state.wgsl().source,
            before_source,
            "a height edit rebuilt the shader"
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
            assert!(
                toward_eye.dot(plane.normal()) > 0.99,
                "{} faces {toward_eye:?}, expected {:?}",
                plane.name(),
                plane.normal()
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
}
