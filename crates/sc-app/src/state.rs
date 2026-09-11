//! Application state.
//!
//! Holds the document, the camera and the current selection. Every edit goes
//! through [`AppState::apply`], which is the same `Document::apply` the CLI and
//! (later) the agent layer use — the UI gets no privileged path into the model.

use crate::dialog::{FileBrowser, Purpose};
use crate::settings::Settings;
use sc_doc::{file, samples, Command, Document};
use sc_geom::glam::Vec2;
use sc_geom::glam::Vec3;
use sc_geom::{Node, NodeId};
use sc_render::{CameraRig, OrbitCamera};
use std::path::{Path, PathBuf};

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
    /// Milliseconds spent on the last shader rebuild, shown in the status bar
    /// because it is the cost that will drive moving parameters into a uniform
    /// buffer later.
    pub last_rebuild_ms: f32,
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
        let doc = samples::bracket();
        let camera = OrbitCamera::framing(doc.bounds().expect("sample is rooted"));
        Self {
            selected: doc.root(),
            doc,
            rig: CameraRig::new(camera),
            field_dirty: true,
            status: "Ready".to_string(),
            last_rebuild_ms: 0.0,
            tab: 0,
            tool: 0,
            sketch: None,
            extrude_height: 10.0,
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
        if let Some(bounds) = self.doc.bounds() {
            let framed = OrbitCamera::framing(bounds);
            // Keep the current orientation; only the target and distance move,
            // so framing does not disorient by spinning the part as well.
            self.rig.goal.target = framed.target;
            self.rig.goal.distance = framed.distance;
            self.status = "Framed the model".to_string();
        }
    }

    fn frame_camera(&mut self) {
        if let Some(bounds) = self.doc.bounds() {
            self.rig.snap_to(OrbitCamera::framing(bounds));
        }
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
        let (origin, dir) = cam.ray(ndc, aspect);

        if let Some(root) = self.doc.root() {
            let epsilon = (cam.distance * 1.0e-4).max(1.0e-4);
            let mut travelled = 0.0f32;
            for _ in 0..192 {
                let point = origin + dir * travelled;
                let d = sc_geom::eval(self.doc.arena(), root, point);
                if d < epsilon {
                    return point;
                }
                travelled += d.max(epsilon);
                if travelled > cam.far() {
                    break;
                }
            }
        }
        cam.plate_hit(ndc, aspect).unwrap_or(cam.target)
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

    pub(crate) fn wgsl(&self) -> String {
        sc_geom::wgsl::generate_with_selection(self.doc.arena(), self.doc.root(), self.selected)
    }

    /// Begins a new profile on the build plate.
    pub(crate) fn start_sketch(&mut self) {
        self.sketch = Some(Vec::new());
        self.tool = TOOL_SKETCH;
        self.status = "Click on the build plate to place points".to_string();
    }

    pub(crate) fn cancel_sketch(&mut self) {
        if self.sketch.take().is_some() {
            self.status = "Sketch cancelled".to_string();
        }
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
        points.push(point);
        let n = points.len();
        self.status = if n < 3 {
            format!("{n} of at least 3 points")
        } else {
            format!("{n} points - Enter to extrude, Esc to cancel")
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
        let node = Node::Extrude {
            profile: points,
            height,
        };
        if !node.is_valid() {
            self.status = "That profile encloses no area".to_string();
            return;
        }

        let Some(id) = self.apply(Command::Add { node }) else {
            return;
        };
        self.apply(Command::SetName {
            id,
            name: Some("Pad".to_string()),
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
    fn a_new_document_is_empty_and_clean() {
        let mut state = AppState::new();
        state.new_document();
        assert!(state.doc.root().is_none());
        assert!(!state.dirty);
        assert!(state.path.is_none());
        assert_eq!(state.title(), "Untitled");
    }
}
