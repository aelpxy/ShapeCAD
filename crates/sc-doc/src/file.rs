//! The native document format.
//!
//! A `.shapecad` file is JSON: the node arena, the root, and the names. Plain
//! text rather than a container because it diffs cleanly in version control and
//! is directly legible to a language model, which matters for a tool whose
//! premise is that agents can read and edit designs.
//!
//! The edit log is deliberately *not* saved. It grows without bound, whereas the
//! arena is proportional to the model. Undo history is a property of a session,
//! not of a part.
//!
//! Voxel grids are the one thing that is not in the JSON. They are megabytes of
//! binary and would cost the format both of the properties it was chosen for, so
//! they live in a sidecar directory beside the document and the JSON references
//! them by id. See [`crate::asset`].

use crate::asset::{AssetError, AssetStore};
use crate::{AttachError, DocError, Document};
use sc_geom::{Arena, GeomError, NodeId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Bumped whenever the on-disk shape changes incompatibly.
///
/// Version 2 made an extrusion's profile parametric. A rectangle is now a width
/// and a height rather than four points, which an older build cannot read.
///
/// Version 3 added mesh nodes, which name a voxel grid stored in the sidecar
/// directory beside the document. An older build cannot evaluate one, and would
/// not know to look for the directory either.
///
/// Version 4 added `Node::Prism`, the unbounded sweep a through cut is
/// expressed with. An older build cannot evaluate one, and reading the file
/// without it would turn a hole sized against the whole part into nothing.
pub const FORMAT_VERSION: u32 = 5;

/// Conventional file extension, without the dot.
pub const EXTENSION: &str = "shapecad";

/// Extension of the sidecar directory, without the dot.
pub const ASSET_DIR_EXTENSION: &str = "assets";

/// The sidecar directory belonging to a document path: `part.shapecad` keeps its
/// grids in `part.assets`.
///
/// Derived from the path rather than recorded in the file, so moving or renaming
/// the pair keeps them associated and no absolute path is ever baked into a
/// document.
#[must_use]
pub fn sidecar_dir(path: &Path) -> PathBuf {
    path.with_extension(ASSET_DIR_EXTENSION)
}

/// The serialised form of a document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocumentFile {
    /// Format version, checked on load.
    pub format: u32,
    /// What wrote the file. Informational.
    pub generator: String,
    /// Always millimetres today; recorded so a future change is detectable
    /// rather than silently reinterpreting every dimension.
    pub units: String,
    /// The geometry.
    pub arena: Arena,
    /// The node the document renders and exports.
    pub root: Option<NodeId>,
    /// Node labels, sorted by id so the file diffs cleanly.
    pub names: Vec<(NodeId, String)>,
}

/// Why a document could not be read or written.
#[derive(Debug)]
pub enum FileError {
    /// The file could not be read or written.
    Io(std::io::Error),
    /// The contents are not valid JSON, or not a document.
    Parse(serde_json::Error),
    /// Written by a newer version of the application.
    UnsupportedVersion(u32),
    /// The file parsed but describes an inconsistent model.
    Invalid(DocError),
    /// The sidecar directory could not be read or written.
    Asset(AssetError),
    /// A mesh node was left without its geometry. Reported rather than handed
    /// back, because a document containing a placeholder is silently empty where
    /// it should be solid.
    Unresolved(AttachError),
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileError::Io(e) => write!(f, "{e}"),
            FileError::Parse(e) => write!(f, "not a ShapeCAD document: {e}"),
            FileError::UnsupportedVersion(v) => {
                write!(
                    f,
                    "format version {v} is newer than this build supports ({FORMAT_VERSION})"
                )
            }
            FileError::Invalid(e) => write!(f, "document is inconsistent: {e}"),
            FileError::Asset(e) => write!(f, "geometry sidecar: {e}"),
            FileError::Unresolved(e) => write!(f, "missing imported geometry: {e}"),
        }
    }
}

impl std::error::Error for FileError {}

impl Document {
    /// Captures the document as its serialisable form.
    ///
    /// Labels on tombstoned nodes are left out, for the same reason a
    /// tombstoned node's grid is: nothing in the saved file can reach them.
    /// Deleting a node deliberately keeps its label in memory so that undoing
    /// the deletion brings the two back together, but writing that label out
    /// produces a file this build's own loader refuses, because loading
    /// requires every named id to be live.
    #[must_use]
    pub fn snapshot(&self) -> DocumentFile {
        let mut names: Vec<(NodeId, String)> = self
            .names
            .iter()
            .filter(|(id, _)| self.arena.is_alive(**id))
            .map(|(id, n)| (*id, n.clone()))
            .collect();
        names.sort_by_key(|(id, _)| *id);

        DocumentFile {
            format: FORMAT_VERSION,
            generator: concat!("ShapeCAD ", env!("CARGO_PKG_VERSION")).to_string(),
            units: "mm".to_string(),
            arena: self.arena.clone(),
            root: self.root,
            names,
        }
    }

    /// Rebuilds a document from a snapshot.
    ///
    /// The result has no undo history: an opened file is a starting point, not a
    /// continuation of whatever session produced it.
    ///
    /// # Errors
    /// [`DocError::Geom`] if the arena breaks any rule [`Arena`] enforces on
    /// every edit, or if the root or a label refers to a node that is not live.
    /// A file that parses but describes an inconsistent model is an error, not a
    /// partially loaded document.
    pub fn from_snapshot(file: DocumentFile) -> Result<Self, DocError> {
        let arena = file.arena;
        validate(&arena)?;
        if let Some(root) = file.root {
            arena.try_get(root)?;
        }
        let mut names = std::collections::HashMap::new();
        for (id, name) in file.names {
            arena.try_get(id)?;
            names.insert(id, name);
        }

        Ok(Self {
            arena,
            root: file.root,
            names,
            // Every mesh node in a freshly parsed snapshot holds a placeholder;
            // `attach_assets` is what makes them real, and it brings the store
            // with it.
            assets: AssetStore::new(),
            entries: Vec::new(),
            cursor: 0,
            step: 0,
            open: 0,
        })
    }
}

/// Holds a deserialised arena to the rules [`Arena`] enforces on every edit.
///
/// `Arena`'s `Deserialize` is derived: it takes the slots exactly as the file
/// gives them, so a file is the one way into the kernel that nothing checks. A
/// document this build wrote is always consistent, but a file is untrusted
/// input, and each way it can be wrong fails late and far from the cause. A
/// child pointing at a tombstone evaluates as empty space rather than as an
/// error. An out-of-range parameter produces an inverted bound, which
/// under-reports and clips the part out of an export. A cycle recurses until
/// the stack runs out.
fn validate(arena: &Arena) -> Result<(), DocError> {
    // Structure is the arena's own invariant, so the arena checks it: children
    // live, no cycles, derivations live and outside their own subtree. Keeping
    // a second copy here would let the two drift, and the arena's is the one an
    // edit already runs.
    arena.check_structure()?;

    // Parameters are the loader's business, because only the loader knows about
    // assets. A mesh node is still the empty placeholder its `Deserialize`
    // produced at this point; `attach_assets` fills it in and validates it
    // then, and rejecting it here would stop every document with an import from
    // opening at all.
    for id in arena.live_ids() {
        let node = arena.try_get(id)?;
        if !crate::mesh::is_placeholder(node) && !node.is_valid() {
            return Err(DocError::Geom(GeomError::InvalidNode {
                kind: node.kind(),
                reason: "parameters out of range or non-finite".to_string(),
            }));
        }
    }
    Ok(())
}

/// Writes a document to `path` as pretty-printed JSON, with its voxel grids in
/// the sidecar directory beside it.
///
/// The assets always follow the document. Saving to a new path writes a full
/// copy of the sidecar directory there rather than referring back to the old
/// one, because a document that pointed at grids somewhere else would break the
/// moment either copy moved, and a saved file that cannot be opened on its own
/// is not a saved file. The cost is that a save-as rewrites every grid, which
/// for a part carrying a few large imports is the slowest thing the application
/// does.
///
/// Only assets a live node references are written, and files for assets no
/// longer referenced are removed afterwards. A document with no imported meshes
/// produces exactly one file and no directory.
///
/// The order matters: grids are added first, then the document, then the dead
/// grids are removed. Nothing the document on disk refers to is ever deleted
/// before its replacement is in place, so a save that fails part way through
/// leaves the previous document openable.
///
/// # Errors
/// [`FileError::Io`], [`FileError::Parse`] or [`FileError::Asset`] on failure.
pub fn save(doc: &Document, path: &Path) -> Result<(), FileError> {
    let text = serde_json::to_string_pretty(&doc.snapshot()).map_err(FileError::Parse)?;
    let dir = sidecar_dir(path);
    let referenced = doc.referenced_assets();

    doc.assets()
        .write_dir(&dir, &referenced)
        .map_err(FileError::Asset)?;
    std::fs::write(path, text).map_err(FileError::Io)?;
    AssetStore::prune_dir(&dir, &referenced).map_err(FileError::Asset)
}

/// Reads a document from `path`, together with any grids it references.
///
/// A file with no mesh nodes needs no sidecar directory, and the absence of one
/// is therefore not an error. That is what keeps version 2 documents, which
/// cannot contain a mesh node, loading unchanged.
///
/// # Errors
/// [`FileError`] for any I/O, parse, version or consistency problem, including
/// [`FileError::Unresolved`] if a mesh node's grid never arrived.
pub fn open(path: &Path) -> Result<Document, FileError> {
    let text = std::fs::read_to_string(path).map_err(FileError::Io)?;
    let file: DocumentFile = serde_json::from_str(&text).map_err(FileError::Parse)?;
    if file.format > FORMAT_VERSION {
        return Err(FileError::UnsupportedVersion(file.format));
    }
    let mut doc = Document::from_snapshot(file).map_err(FileError::Invalid)?;

    let required = doc.referenced_assets();
    if required.is_empty() {
        return Ok(doc);
    }
    let store = AssetStore::read_dir(&sidecar_dir(path), &required).map_err(FileError::Asset)?;
    doc.attach_assets(store).map_err(FileError::Unresolved)?;
    Ok(doc)
}
