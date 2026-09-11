//! Wavefront OBJ input, geometry only.
//!
//! Only `v` and `f` are read. Normals are recomputed from the winding, texture
//! coordinates and materials have no meaning for a solid, and groups and
//! objects are dropped because the importer produces one body. Anything else in
//! the file is skipped rather than rejected, since OBJ is an open-ended format
//! and a file full of `usemtl` lines is not malformed.
//!
//! What is rejected is a face that cannot be turned into triangles: too few
//! vertices, an index of zero, or an index past the end of the vertex list.

use crate::error::{MeshError, Result};
use crate::mesh::Mesh;
use sc_geom::glam::Vec3;
use std::path::Path;

/// Reads an OBJ file.
///
/// # Errors
/// [`MeshError::Io`] if the file cannot be read, [`MeshError::Syntax`] for a
/// malformed `v` or `f` line, and [`MeshError::VertexOutOfRange`] for a face
/// referring to a vertex the file never defined.
pub fn read(path: &Path) -> Result<Mesh> {
    let text = std::fs::read_to_string(path).map_err(|e| MeshError::io(path, &e))?;
    read_str(&text)
}

/// Reads OBJ text already in memory.
///
/// # Errors
/// As [`read`], minus the I/O failures.
pub fn read_str(text: &str) -> Result<Mesh> {
    let mut positions: Vec<Vec3> = Vec::new();
    let mut indices: Vec<[u32; 3]> = Vec::new();

    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        // A `#` starts a comment anywhere on the line.
        let body = raw.split('#').next().unwrap_or("");
        let mut words = body.split_whitespace();
        match words.next() {
            Some("v") => positions.push(read_vertex(&mut words, line)?),
            Some("f") => read_face(&mut words, line, positions.len(), &mut indices)?,
            _ => {}
        }
    }

    let mut mesh = Mesh {
        positions,
        normals: Vec::new(),
        indices,
    };
    mesh.recompute_normals();
    Ok(mesh)
}

/// Parses the three coordinates of a `v` line, ignoring any trailing `w` or
/// vertex colour.
fn read_vertex<'a>(words: &mut impl Iterator<Item = &'a str>, line: usize) -> Result<Vec3> {
    let mut p = Vec3::ZERO;
    for axis in 0..3 {
        let Some(word) = words.next() else {
            return Err(MeshError::Syntax {
                line,
                reason: "vertex needs three coordinates".to_string(),
            });
        };
        p[axis] = parse_coordinate(word, line)?;
    }
    Ok(p)
}

/// Parses one finite coordinate.
fn parse_coordinate(word: &str, line: usize) -> Result<f32> {
    match word.parse::<f32>() {
        // `parse` maps an overflowing literal to an infinity rather than to an
        // error, so finiteness has to be checked separately.
        Ok(v) if v.is_finite() => Ok(v),
        Ok(_) => Err(MeshError::Syntax {
            line,
            reason: format!("'{word}' is not a finite coordinate"),
        }),
        Err(e) => Err(MeshError::Syntax {
            line,
            reason: format!("'{word}' is not a number: {e}"),
        }),
    }
}

/// Triangulates one `f` line into `indices`.
///
/// A fan from the first vertex. That is correct for any convex polygon and is
/// what every OBJ exporter assumes; a concave polygon would need ear clipping,
/// but OBJ faces come from quad meshes in practice and a fan is what the rest
/// of the world does with them.
fn read_face<'a>(
    words: &mut impl Iterator<Item = &'a str>,
    line: usize,
    defined: usize,
    indices: &mut Vec<[u32; 3]>,
) -> Result<()> {
    let mut corners = Vec::new();
    for word in words {
        corners.push(resolve_index(word, line, defined)?);
    }
    if corners.len() < 3 {
        return Err(MeshError::Syntax {
            line,
            reason: format!(
                "face needs at least three vertices, found {}",
                corners.len()
            ),
        });
    }
    for w in 1..corners.len() - 1 {
        indices.push([corners[0], corners[w], corners[w + 1]]);
    }
    Ok(())
}

/// Turns one `v`, `v/vt`, `v//vn` or `v/vt/vn` corner into a zero-based index.
fn resolve_index(word: &str, line: usize, defined: usize) -> Result<u32> {
    let field = word.split('/').next().unwrap_or("");
    let raw: i64 = field.parse().map_err(|e| MeshError::Syntax {
        line,
        reason: format!("'{field}' is not a vertex index: {e}"),
    })?;

    // OBJ indices are one-based, and negative means counting back from the
    // most recently defined vertex, so -1 is the last one. Zero is the one
    // value the format leaves no meaning for.
    let limit = i64::try_from(defined).unwrap_or(i64::MAX);
    let zero_based = match raw.cmp(&0) {
        std::cmp::Ordering::Greater => raw - 1,
        std::cmp::Ordering::Less => limit + raw,
        std::cmp::Ordering::Equal => {
            return Err(MeshError::VertexOutOfRange {
                line,
                index: 0,
                defined,
            })
        }
    };

    if zero_based < 0 || zero_based >= limit {
        return Err(MeshError::VertexOutOfRange {
            line,
            index: raw,
            defined,
        });
    }
    Ok(u32::try_from(zero_based).expect("index is within the vertex count"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::unit_box;

    #[test]
    fn reads_vertices_and_triangles() {
        let mesh = read_str("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.indices, vec![[0, 1, 2]]);
        assert_eq!(mesh.positions[1], Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn a_quad_becomes_two_triangles_by_fanning_from_the_first_corner() {
        let mesh = read_str("v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n").unwrap();
        assert_eq!(mesh.indices, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn a_pentagon_becomes_three_triangles() {
        let text = "v 0 0 0\nv 1 0 0\nv 2 1 0\nv 1 2 0\nv 0 2 0\nf 1 2 3 4 5\n";
        assert_eq!(read_str(text).unwrap().triangle_count(), 3);
    }

    #[test]
    fn materials_normals_texture_coordinates_and_groups_are_ignored() {
        let text = "\
mtllib thing.mtl
o body
g shell
# a comment
v 0 0 0
vt 0.5 0.5
vn 0 0 1
v 1 0 0
v 0 1 0
s off
usemtl steel
f 1/1/1 2/2/1 3//1
";
        let mesh = read_str(text).unwrap();
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.indices, vec![[0, 1, 2]]);
    }

    #[test]
    fn negative_indices_count_back_from_the_last_vertex() {
        let mesh = read_str("v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\n").unwrap();
        assert_eq!(mesh.indices, vec![[0, 1, 2]]);
    }

    #[test]
    fn a_face_indexing_a_vertex_that_does_not_exist_is_rejected() {
        let err = read_str("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 9\n").unwrap_err();
        assert_eq!(
            err,
            MeshError::VertexOutOfRange {
                line: 4,
                index: 9,
                defined: 3
            }
        );
    }

    #[test]
    fn index_zero_is_rejected_because_obj_is_one_based() {
        let err = read_str("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 0 1 2\n").unwrap_err();
        assert!(matches!(err, MeshError::VertexOutOfRange { index: 0, .. }));
    }

    #[test]
    fn a_face_with_two_corners_is_rejected() {
        let err = read_str("v 0 0 0\nv 1 0 0\nf 1 2\n").unwrap_err();
        assert!(matches!(err, MeshError::Syntax { line: 3, .. }));
    }

    #[test]
    fn a_vertex_missing_a_coordinate_is_rejected() {
        let err = read_str("v 0 0\n").unwrap_err();
        assert!(matches!(err, MeshError::Syntax { line: 1, .. }));
    }

    #[test]
    fn a_coordinate_that_overflows_f32_is_rejected_rather_than_becoming_infinity() {
        let err = read_str("v 1e400 0 0\n").unwrap_err();
        assert!(matches!(err, MeshError::Syntax { line: 1, .. }));
    }

    #[test]
    fn a_box_survives_the_round_trip_through_obj_text() {
        use std::fmt::Write as _;

        let source = unit_box(Vec3::new(1.0, 2.0, 3.0));
        let mut text = String::new();
        for p in &source.positions {
            writeln!(text, "v {} {} {}", p.x, p.y, p.z).unwrap();
        }
        for &[a, b, c] in &source.indices {
            writeln!(text, "f {} {} {}", a + 1, b + 1, c + 1).unwrap();
        }
        let read_back = read_str(&text).unwrap();
        assert_eq!(read_back.positions, source.positions);
        assert_eq!(read_back.indices, source.indices);
        assert!(read_back.topology().is_printable());
    }
}
