//! STL input and output, binary and ASCII.
//!
//! STL is the lowest common denominator: no units, no colour, 32-bit floats and
//! every vertex repeated per triangle. It is written here because every slicer
//! accepts it. 3MF is the better target and comes next.
//!
//! On the reading side the format has one genuine trap, which is that there is
//! no reliable marker distinguishing the two variants. See [`is_binary`].

use crate::error::{MeshError, Result};
use crate::mesh::Mesh;
use sc_geom::glam::Vec3;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Bytes before the first triangle record in a binary file: 80 of header plus
/// the 32-bit triangle count.
const BINARY_HEADER: usize = 84;

/// Bytes per binary triangle record: twelve floats plus the attribute word.
const BINARY_TRIANGLE: usize = 50;

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

/// Writes `mesh` as ASCII STL.
///
/// Binary is what an exporter should emit: it is five times smaller and exact,
/// where ASCII rounds every coordinate through a decimal representation. This
/// exists so the reader can be tested against output this crate produced, and
/// for the rare tool that still refuses binary.
///
/// # Errors
/// Any I/O failure.
pub fn write_ascii(mesh: &Mesh, path: &Path) -> std::io::Result<()> {
    let mut out = BufWriter::new(std::fs::File::create(path)?);
    writeln!(out, "solid shapecad")?;
    for &[a, b, c] in &mesh.indices {
        let (pa, pb, pc) = (
            mesh.positions[a as usize],
            mesh.positions[b as usize],
            mesh.positions[c as usize],
        );
        let n = (pb - pa).cross(pc - pa).normalize_or_zero();
        // Nine significant digits round-trip an f32 exactly, so a file written
        // here and read back gives bit-identical vertices.
        writeln!(out, "  facet normal {:.9e} {:.9e} {:.9e}", n.x, n.y, n.z)?;
        writeln!(out, "    outer loop")?;
        for v in [pa, pb, pc] {
            writeln!(out, "      vertex {:.9e} {:.9e} {:.9e}", v.x, v.y, v.z)?;
        }
        writeln!(out, "    endloop")?;
        writeln!(out, "  endfacet")?;
    }
    writeln!(out, "endsolid shapecad")?;
    out.flush()
}

/// Reads an STL file, detecting the variant from its contents.
///
/// # Errors
/// [`MeshError::Io`] if the file cannot be read, and one of the parse variants
/// if its contents are not a well-formed STL of either kind.
pub fn read(path: &Path) -> Result<Mesh> {
    let bytes = std::fs::read(path).map_err(|e| MeshError::io(path, &e))?;
    read_bytes(&bytes)
}

/// Reads an STL already in memory.
///
/// # Errors
/// One of the parse variants if `bytes` is not a well-formed STL of either kind.
pub fn read_bytes(bytes: &[u8]) -> Result<Mesh> {
    if is_binary(bytes) {
        read_binary(bytes)
    } else {
        read_ascii(bytes)
    }
}

/// Decides which variant `bytes` is.
///
/// The usual test is whether the file starts with `solid`, and it is wrong:
/// several widely used exporters leave arbitrary text in the 80-byte binary
/// header, and some of them start it with the word "solid". The load-bearing
/// test is arithmetic instead. A binary file is exactly `84 + 50 * n` bytes for
/// the `n` declared at offset 80, which an ASCII file has no reason to be.
///
/// A file that fails the length test still has to be classified, because a
/// truncated binary file or one with a dishonest count fails it too and should
/// be reported as a malformed binary rather than handed to the text parser. For
/// those the fallback is the prefix test *and* a check that the bytes a binary
/// header would occupy read as text. A binary header is padded with NULs, which
/// no text file contains, so the pair of tests separates the cases the length
/// test leaves ambiguous.
#[must_use]
pub fn is_binary(bytes: &[u8]) -> bool {
    if bytes.len() < BINARY_HEADER {
        return false;
    }
    let declared = declared_triangles(bytes);
    if BINARY_HEADER as u64 + BINARY_TRIANGLE as u64 * u64::from(declared) == bytes.len() as u64 {
        return true;
    }
    !(starts_with_solid(bytes) && looks_like_text(&bytes[..BINARY_HEADER]))
}

/// Whether `bytes` holds no control characters other than ordinary whitespace.
///
/// Bytes at or above `0x80` are allowed through so that a solid name written in
/// UTF-8 does not get the file misread as binary.
fn looks_like_text(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|&b| b >= 0x20 && b != 0x7f || b == b'\t' || b == b'\n' || b == b'\r')
}

/// The triangle count from the binary header.
fn declared_triangles(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]])
}

/// Whether the first non-blank characters are the ASCII keyword `solid`.
fn starts_with_solid(bytes: &[u8]) -> bool {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    bytes[start..].starts_with(b"solid")
}

/// Parses the binary layout.
fn read_binary(bytes: &[u8]) -> Result<Mesh> {
    if bytes.len() < BINARY_HEADER {
        return Err(MeshError::TriangleCountOverrunsFile {
            declared: 0,
            needed: BINARY_HEADER as u64,
            actual: bytes.len() as u64,
        });
    }
    let declared = declared_triangles(bytes);
    let needed = BINARY_HEADER as u64 + BINARY_TRIANGLE as u64 * u64::from(declared);
    if needed > bytes.len() as u64 {
        return Err(MeshError::TriangleCountOverrunsFile {
            declared,
            needed,
            actual: bytes.len() as u64,
        });
    }

    let mut triangles = Vec::with_capacity(declared as usize);
    for t in 0..declared as usize {
        // Skip the stored face normal: it is advisory, frequently zero or
        // simply wrong, and the winding is the authority on orientation.
        let base = BINARY_HEADER + t * BINARY_TRIANGLE + 12;
        let mut face = [Vec3::ZERO; 3];
        for (v, slot) in face.iter_mut().enumerate() {
            let o = base + v * 12;
            *slot = Vec3::new(le_f32(bytes, o), le_f32(bytes, o + 4), le_f32(bytes, o + 8));
            if !slot.is_finite() {
                return Err(MeshError::NonFiniteCoordinate { triangle: t });
            }
        }
        triangles.push(face);
    }
    Ok(Mesh::from_triangles(&triangles))
}

/// Reads a little-endian `f32` at `offset`, which the caller has bounds-checked.
fn le_f32(bytes: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Parses the text layout.
///
/// Deliberately strict about the keyword sequence. A tolerant parser that hunts
/// for `vertex` and ignores everything else will happily read four vertices in
/// one facet as one triangle and a bit, and the caller has no way to notice.
fn read_ascii(bytes: &[u8]) -> Result<Mesh> {
    let text = std::str::from_utf8(bytes).map_err(|e| MeshError::Syntax {
        line: 1,
        reason: format!("not valid UTF-8 and not a binary STL: {e}"),
    })?;
    let tokens = tokenize(text);
    let mut cursor = 0usize;

    expect(&tokens, &mut cursor, "solid")?;
    // The solid name is optional and may contain spaces, so skip to end of line.
    let name_line = tokens[cursor - 1].line;
    while tokens.get(cursor).is_some_and(|t| t.line == name_line) {
        cursor += 1;
    }

    let mut triangles = Vec::new();
    loop {
        let Some(token) = tokens.get(cursor) else {
            return Err(MeshError::Syntax {
                line: tokens.last().map_or(1, |t| t.line),
                reason: "file ends before endsolid".to_string(),
            });
        };
        match token.text {
            "endsolid" => break,
            "facet" => triangles.push(read_ascii_facet(&tokens, &mut cursor)?),
            other => {
                return Err(MeshError::Syntax {
                    line: token.line,
                    reason: format!("expected facet or endsolid, found '{other}'"),
                })
            }
        }
    }
    Ok(Mesh::from_triangles(&triangles))
}

/// One whitespace-delimited word and the line it came from.
struct Token<'a> {
    text: &'a str,
    line: usize,
}

/// Splits `text` into words, remembering one-based line numbers for errors.
fn tokenize(text: &str) -> Vec<Token<'_>> {
    text.lines()
        .enumerate()
        .flat_map(|(i, l)| {
            l.split_whitespace().map(move |w| Token {
                text: w,
                line: i + 1,
            })
        })
        .collect()
}

/// Consumes one `facet normal ... outer loop ... endloop endfacet` block.
fn read_ascii_facet(tokens: &[Token<'_>], cursor: &mut usize) -> Result<[Vec3; 3]> {
    expect(tokens, cursor, "facet")?;
    expect(tokens, cursor, "normal")?;
    for _ in 0..3 {
        read_float(tokens, cursor)?;
    }
    expect(tokens, cursor, "outer")?;
    expect(tokens, cursor, "loop")?;

    let mut face = [Vec3::ZERO; 3];
    for slot in &mut face {
        expect(tokens, cursor, "vertex")?;
        *slot = Vec3::new(
            read_float(tokens, cursor)?,
            read_float(tokens, cursor)?,
            read_float(tokens, cursor)?,
        );
    }

    expect(tokens, cursor, "endloop")?;
    expect(tokens, cursor, "endfacet")?;
    Ok(face)
}

/// Consumes `word`, or reports what was found instead.
fn expect(tokens: &[Token<'_>], cursor: &mut usize, word: &str) -> Result<()> {
    match tokens.get(*cursor) {
        Some(t) if t.text == word => {
            *cursor += 1;
            Ok(())
        }
        Some(t) => Err(MeshError::Syntax {
            line: t.line,
            reason: format!("expected '{word}', found '{}'", t.text),
        }),
        None => Err(MeshError::Syntax {
            line: tokens.last().map_or(1, |t| t.line),
            reason: format!("expected '{word}', found end of file"),
        }),
    }
}

/// Consumes one finite decimal number.
fn read_float(tokens: &[Token<'_>], cursor: &mut usize) -> Result<f32> {
    let Some(t) = tokens.get(*cursor) else {
        return Err(MeshError::Syntax {
            line: tokens.last().map_or(1, |t| t.line),
            reason: "expected a number, found end of file".to_string(),
        });
    };
    *cursor += 1;
    // `parse` turns an overflowing literal into an infinity rather than an
    // error, so finiteness has to be checked separately.
    match t.text.parse::<f32>() {
        Ok(v) if v.is_finite() => Ok(v),
        Ok(_) => Err(MeshError::Syntax {
            line: t.line,
            reason: format!("'{}' is not a finite number", t.text),
        }),
        Err(e) => Err(MeshError::Syntax {
            line: t.line,
            reason: format!("'{}' is not a number: {e}", t.text),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{unit_box, uv_sphere};

    fn scratch(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("shapecad-stl-{name}-{}.stl", std::process::id()));
        p
    }

    /// Every triangle as three positions, in file order, for comparing two
    /// meshes whose vertex welding may differ.
    fn faces(mesh: &Mesh) -> Vec<[Vec3; 3]> {
        mesh.indices
            .iter()
            .map(|&[a, b, c]| {
                [
                    mesh.positions[a as usize],
                    mesh.positions[b as usize],
                    mesh.positions[c as usize],
                ]
            })
            .collect()
    }

    #[test]
    fn a_binary_file_written_here_reads_back_triangle_for_triangle() {
        let source = uv_sphere(7.0, 16, 10);
        let path = scratch("binary-roundtrip");
        write(&source, &path).unwrap();
        let read_back = read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(faces(&read_back), faces(&source));
        assert!(
            read_back.topology().is_printable(),
            "welding must reconnect it"
        );
    }

    #[test]
    fn an_ascii_file_written_here_reads_back_triangle_for_triangle() {
        let source = unit_box(Vec3::new(3.0, 1.25, 0.5));
        let path = scratch("ascii-roundtrip");
        write_ascii(&source, &path).unwrap();
        let read_back = read(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_eq!(faces(&read_back), faces(&source));
        assert!(read_back.topology().is_printable());
    }

    #[test]
    fn the_variant_is_detected_from_the_contents_not_the_extension() {
        let source = unit_box(Vec3::splat(2.0));
        let binary = scratch("detect-binary");
        let ascii = scratch("detect-ascii");
        write(&source, &binary).unwrap();
        write_ascii(&source, &ascii).unwrap();

        assert!(is_binary(&std::fs::read(&binary).unwrap()));
        assert!(!is_binary(&std::fs::read(&ascii).unwrap()));
        std::fs::remove_file(&binary).ok();
        std::fs::remove_file(&ascii).ok();
    }

    /// A binary file whose 80-byte header begins with the word "solid", which
    /// is what makes the prefix test on its own useless.
    fn binary_with_solid_header(triangles: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; BINARY_HEADER];
        bytes[..5].copy_from_slice(b"solid");
        bytes[80..84].copy_from_slice(&triangles.to_le_bytes());
        bytes.resize(BINARY_HEADER + BINARY_TRIANGLE * triangles as usize, 0);
        // One non-degenerate triangle so the result is not empty.
        if triangles > 0 {
            let coords: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
            for (i, v) in coords.iter().enumerate() {
                let o = BINARY_HEADER + 12 + i * 4;
                bytes[o..o + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        bytes
    }

    #[test]
    fn a_binary_file_whose_header_starts_with_solid_is_still_read_as_binary() {
        let bytes = binary_with_solid_header(1);
        assert!(is_binary(&bytes));
        let mesh = read_bytes(&bytes).unwrap();
        assert_eq!(mesh.triangle_count(), 1);
    }

    #[test]
    fn a_truncated_binary_file_is_rejected() {
        let mut bytes = binary_with_solid_header(4);
        bytes.truncate(bytes.len() - 30);
        let err = read_bytes(&bytes).unwrap_err();
        assert_eq!(
            err,
            MeshError::TriangleCountOverrunsFile {
                declared: 4,
                needed: (BINARY_HEADER + 4 * BINARY_TRIANGLE) as u64,
                actual: bytes.len() as u64,
            }
        );
    }

    #[test]
    fn a_declared_triangle_count_that_overruns_the_file_is_rejected() {
        let mut bytes = binary_with_solid_header(1);
        bytes[80..84].copy_from_slice(&1_000_000u32.to_le_bytes());
        let err = read_bytes(&bytes).unwrap_err();
        assert!(matches!(
            err,
            MeshError::TriangleCountOverrunsFile {
                declared: 1_000_000,
                ..
            }
        ));
    }

    #[test]
    fn a_file_too_short_to_hold_a_header_is_rejected() {
        assert!(read_bytes(b"\x00\x01\x02").is_err());
    }

    #[test]
    fn a_binary_coordinate_that_is_not_finite_is_rejected() {
        let mut bytes = binary_with_solid_header(1);
        let o = BINARY_HEADER + 12;
        bytes[o..o + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert_eq!(
            read_bytes(&bytes).unwrap_err(),
            MeshError::NonFiniteCoordinate { triangle: 0 }
        );
    }

    const GOOD_ASCII: &str = "\
solid part
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
endsolid part
";

    #[test]
    fn a_minimal_ascii_file_reads() {
        let mesh = read_bytes(GOOD_ASCII.as_bytes()).unwrap();
        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.positions.len(), 3);
    }

    #[test]
    fn an_ascii_solid_name_with_spaces_is_skipped() {
        let text = GOOD_ASCII.replace("solid part", "solid my favourite part");
        assert_eq!(read_bytes(text.as_bytes()).unwrap().triangle_count(), 1);
    }

    #[test]
    fn an_ascii_facet_with_four_vertices_is_rejected_rather_than_half_read() {
        let text = GOOD_ASCII.replace("    endloop", "      vertex 1 1 0\n    endloop");
        let err = read_bytes(text.as_bytes()).unwrap_err();
        assert!(matches!(err, MeshError::Syntax { .. }), "{err}");
    }

    #[test]
    fn an_ascii_file_that_stops_mid_facet_is_rejected() {
        let text = "solid part\n  facet normal 0 0 1\n    outer loop\n      vertex 0 0 0\n";
        let err = read_bytes(text.as_bytes()).unwrap_err();
        assert!(matches!(err, MeshError::Syntax { .. }), "{err}");
    }

    #[test]
    fn an_ascii_file_missing_endsolid_is_rejected() {
        let text = GOOD_ASCII.replace("endsolid part\n", "");
        let err = read_bytes(text.as_bytes()).unwrap_err();
        assert!(matches!(err, MeshError::Syntax { .. }), "{err}");
    }

    #[test]
    fn an_ascii_coordinate_that_is_not_a_number_is_rejected_with_its_line() {
        let text = GOOD_ASCII.replace("vertex 1 0 0", "vertex 1 oops 0");
        let err = read_bytes(text.as_bytes()).unwrap_err();
        assert!(matches!(err, MeshError::Syntax { line: 5, .. }), "{err}");
    }

    #[test]
    fn an_ascii_coordinate_that_overflows_f32_is_rejected() {
        let text = GOOD_ASCII.replace("vertex 1 0 0", "vertex 1e400 0 0");
        let err = read_bytes(text.as_bytes()).unwrap_err();
        assert!(matches!(err, MeshError::Syntax { .. }), "{err}");
    }

    #[test]
    fn a_file_that_is_neither_variant_is_rejected() {
        let err = read_bytes(b"this is just some text, at some length or another").unwrap_err();
        assert!(matches!(err, MeshError::Syntax { .. }), "{err}");
    }
}
