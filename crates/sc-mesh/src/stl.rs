//! Binary STL output.
//!
//! STL is the lowest common denominator: no units, no colour, 32-bit floats and
//! every vertex repeated per triangle. It is written here because every slicer
//! accepts it. 3MF is the better target and comes next.

use crate::mesh::Mesh;
use std::io::{BufWriter, Write};

/// Writes `mesh` as binary STL.
///
/// # Errors
/// Any I/O failure, or a mesh with more than `u32::MAX` triangles.
pub fn write(mesh: &Mesh, path: &std::path::Path) -> std::io::Result<()> {
    let mut out = BufWriter::new(std::fs::File::create(path)?);

    // 80-byte header. Deliberately not starting with "solid", which some readers
    // treat as a marker for the ASCII variant.
    let mut header = [0u8; 80];
    let tag = b"ShapeCAD binary STL";
    header[..tag.len()].copy_from_slice(tag);
    out.write_all(&header)?;

    let count = u32::try_from(mesh.indices.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "too many triangles for STL",
        )
    })?;
    out.write_all(&count.to_le_bytes())?;

    for &[a, b, c] in &mesh.indices {
        let (pa, pb, pc) = (
            mesh.positions[a as usize],
            mesh.positions[b as usize],
            mesh.positions[c as usize],
        );
        // STL stores a face normal; most slicers recompute it from the winding,
        // but writing a correct one avoids arguments with the ones that do not.
        let n = (pb - pa).cross(pc - pa).normalize_or_zero();
        for v in [n, pa, pb, pc] {
            out.write_all(&v.x.to_le_bytes())?;
            out.write_all(&v.y.to_le_bytes())?;
            out.write_all(&v.z.to_le_bytes())?;
        }
        out.write_all(&0u16.to_le_bytes())?;
    }

    out.flush()
}
