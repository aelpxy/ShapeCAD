//! Errors from reading a mesh off disk and turning it into a grid.
//!
//! Every variant is returned instead of a partial mesh. A reader that gives
//! back the triangles it managed to parse before the file went wrong is worse
//! than one that refuses, because the caller has no way to tell the difference
//! between a small part and half a large one.

use std::path::PathBuf;

/// Why a mesh could not be read or voxelized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MeshError {
    /// The file could not be opened or read.
    ///
    /// `std::io::Error` is neither `Clone` nor `PartialEq`, so the kind and the
    /// message are captured instead of the error itself.
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The underlying failure category.
        kind: std::io::ErrorKind,
        /// The operating system's description.
        detail: String,
    },
    /// The extension is not one of the formats this crate reads.
    UnknownFormat {
        /// The path that was offered.
        path: PathBuf,
    },
    /// A binary STL header declares more triangles than the file can hold.
    ///
    /// Also covers a truncated file, where the declared count is plausible but
    /// the triangle records run past the end.
    TriangleCountOverrunsFile {
        /// Count taken from bytes 80..84 of the header.
        declared: u32,
        /// Bytes the header and that many triangle records would occupy.
        needed: u64,
        /// Bytes the file actually holds.
        actual: u64,
    },
    /// A text format did not say what was expected at this line.
    Syntax {
        /// One-based line number.
        line: usize,
        /// What was wrong with it.
        reason: String,
    },
    /// A face referred to a vertex the file never defined.
    VertexOutOfRange {
        /// One-based line number of the face.
        line: usize,
        /// The index as written, before resolving negatives.
        index: i64,
        /// How many vertices had been defined at that point.
        defined: usize,
    },
    /// A binary triangle record held a `NaN` or an infinity.
    NonFiniteCoordinate {
        /// Zero-based index of the offending triangle record.
        triangle: usize,
    },
    /// Voxelization was asked for a grid of a mesh with no triangles.
    EmptyMesh,
    /// The mesh spans a range that cannot be sampled in 32-bit floating point.
    ///
    /// Coordinates are checked for finiteness as they are read, but the
    /// difference of two finite `f32`s need not be finite and the square of a
    /// finite one need not be either. The grid's spacing and origin come from
    /// that difference and the tree compares squared distances, so a mesh this
    /// wide would produce a grid of infinities and `NaN`s that breaks every
    /// precondition `Grid` documents, while reporting success.
    BoundsNotRepresentable {
        /// The bounding box, already formatted, because the error type is
        /// `Eq` and a float is not. Same reasoning as `Io::detail`.
        extent: String,
    },
}

impl MeshError {
    /// Attaches a path to an I/O failure.
    pub(crate) fn io(path: &std::path::Path, e: &std::io::Error) -> Self {
        MeshError::Io {
            path: path.to_path_buf(),
            kind: e.kind(),
            detail: e.to_string(),
        }
    }
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshError::Io { path, detail, .. } => {
                write!(f, "cannot read {}: {detail}", path.display())
            }
            MeshError::UnknownFormat { path } => {
                write!(f, "{}: not a recognised mesh format", path.display())
            }
            MeshError::TriangleCountOverrunsFile {
                declared,
                needed,
                actual,
            } => write!(
                f,
                "binary STL declares {declared} triangles, needing {needed} bytes, \
                 but the file holds {actual}"
            ),
            MeshError::Syntax { line, reason } => write!(f, "line {line}: {reason}"),
            MeshError::VertexOutOfRange {
                line,
                index,
                defined,
            } => write!(
                f,
                "line {line}: face uses vertex {index} but only {defined} are defined"
            ),
            MeshError::NonFiniteCoordinate { triangle } => {
                write!(f, "triangle {triangle} has a coordinate that is not finite")
            }
            MeshError::EmptyMesh => write!(f, "mesh has no triangles"),
            MeshError::BoundsNotRepresentable { extent } => write!(
                f,
                "mesh spans {extent}, which is too wide to sample in 32-bit floating point"
            ),
        }
    }
}

impl std::error::Error for MeshError {}

/// Result alias for fallible mesh import and voxelization.
pub type Result<T> = std::result::Result<T, MeshError>;
