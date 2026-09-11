//! Reading a triangle mesh off disk.
//!
//! The formats that matter for 3D printing are STL, which everything writes,
//! and OBJ, which is what a scan or a sculpt tends to arrive as. Both are read
//! into the same [`Mesh`] and are then indistinguishable.
//!
//! Which format a file is comes from its extension, because the two are not
//! confusable in practice and an OBJ named `.stl` is a mislabelled file rather
//! than something to guess at. Which *variant* of STL a file is does not come
//! from the extension, because there is only one extension for both and the
//! usual content test is unreliable. See [`crate::stl::is_binary`].

use crate::error::{MeshError, Result};
use crate::mesh::Mesh;
use std::path::Path;

/// A mesh file format this crate can read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Stereolithography, either binary or ASCII.
    Stl,
    /// Wavefront OBJ.
    Obj,
}

impl Format {
    /// The format implied by a path's extension, case-insensitively.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "stl" => Some(Format::Stl),
            "obj" => Some(Format::Obj),
            _ => None,
        }
    }
}

/// Reads a mesh file, choosing the reader from the extension.
///
/// # Errors
/// [`MeshError::UnknownFormat`] if the extension is not one of the supported
/// formats, and otherwise whatever the format's reader reports.
pub fn read(path: &Path) -> Result<Mesh> {
    match Format::from_path(path) {
        Some(Format::Stl) => crate::stl::read(path),
        Some(Format::Obj) => crate::obj::read(path),
        None => Err(MeshError::UnknownFormat {
            path: path.to_path_buf(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_chooses_the_reader_regardless_of_case() {
        assert_eq!(Format::from_path(Path::new("a/b.STL")), Some(Format::Stl));
        assert_eq!(Format::from_path(Path::new("a/b.Obj")), Some(Format::Obj));
        assert_eq!(Format::from_path(Path::new("a/b.3mf")), None);
        assert_eq!(Format::from_path(Path::new("bare")), None);
    }

    #[test]
    fn an_unsupported_extension_is_refused_rather_than_guessed_at() {
        let err = read(Path::new("part.step")).unwrap_err();
        assert!(matches!(err, MeshError::UnknownFormat { .. }));
    }

    #[test]
    fn a_missing_file_reports_io_rather_than_a_parse_failure() {
        let err = read(Path::new("definitely-not-here.stl")).unwrap_err();
        assert!(matches!(
            err,
            MeshError::Io {
                kind: std::io::ErrorKind::NotFound,
                ..
            }
        ));
    }
}
