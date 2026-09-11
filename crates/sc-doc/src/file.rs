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

use crate::{DocError, Document};
use sc_geom::{Arena, NodeId};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Bumped whenever the on-disk shape changes incompatibly.
///
/// Version 2 made an extrusion's profile parametric. A rectangle is now a width
/// and a height rather than four points, which an older build cannot read.
pub const FORMAT_VERSION: u32 = 2;

/// Conventional file extension, without the dot.
pub const EXTENSION: &str = "shapecad";

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
        }
    }
}

impl std::error::Error for FileError {}

impl Document {
    /// Captures the document as its serialisable form.
    #[must_use]
    pub fn snapshot(&self) -> DocumentFile {
        let mut names: Vec<(NodeId, String)> =
            self.names.iter().map(|(id, n)| (*id, n.clone())).collect();
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
    /// [`DocError::Geom`] if the root or any label refers to a node that is not
    /// live, which would mean the file is internally inconsistent.
    pub fn from_snapshot(file: DocumentFile) -> Result<Self, DocError> {
        let arena = file.arena;
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
            entries: Vec::new(),
            cursor: 0,
            step: 0,
            open: 0,
        })
    }
}

/// Writes a document to `path` as pretty-printed JSON.
///
/// # Errors
/// [`FileError::Io`] or [`FileError::Parse`] on failure.
pub fn save(doc: &Document, path: &Path) -> Result<(), FileError> {
    let text = serde_json::to_string_pretty(&doc.snapshot()).map_err(FileError::Parse)?;
    std::fs::write(path, text).map_err(FileError::Io)
}

/// Reads a document from `path`.
///
/// # Errors
/// [`FileError`] for any I/O, parse, version or consistency problem.
pub fn open(path: &Path) -> Result<Document, FileError> {
    let text = std::fs::read_to_string(path).map_err(FileError::Io)?;
    let file: DocumentFile = serde_json::from_str(&text).map_err(FileError::Parse)?;
    if file.format > FORMAT_VERSION {
        return Err(FileError::UnsupportedVersion(file.format));
    }
    Document::from_snapshot(file).map_err(FileError::Invalid)
}
