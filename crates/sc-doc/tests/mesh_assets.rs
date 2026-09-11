//! Documents that contain an imported mesh.
//!
//! A grid is not carried by the JSON, so a mesh node deserialises to a
//! placeholder and the loader has to fill it in from the sidecar. These check
//! that it does, and that a document which cannot be completed fails loudly
//! rather than opening with silently empty geometry.

use sc_doc::asset::{grid_path, AssetError};
use sc_doc::file::{self, FileError};
use sc_doc::{add, Command, Document, Grid};
use sc_geom::glam::Vec3;
use sc_geom::Node;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("shapecad-mesh-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&p).ok();
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// A small solid-ish field: the values do not have to be a real distance for
/// persistence to be testable, only to be preserved exactly.
fn grid() -> Grid {
    let dims = [4u32, 4, 4];
    let n = (dims[0] * dims[1] * dims[2]) as usize;
    Grid {
        dims,
        origin: Vec3::splat(-2.0),
        spacing: 1.0,
        data: (0..n).map(|i| i as f32 * 0.25 - 8.0).collect(),
    }
}

/// Builds a document whose root is an imported mesh, and returns the node.
fn doc_with_mesh() -> (Document, sc_geom::NodeId) {
    let mut doc = Document::new();
    let m = doc.add_mesh(grid()).unwrap();
    doc.apply(Command::SetRoot { root: Some(m) }).unwrap();
    (doc, m)
}

#[test]
fn a_document_containing_a_mesh_round_trips() {
    let dir = scratch("roundtrip");
    let path = dir.join("part.shapecad");
    let (doc, m) = doc_with_mesh();
    let asset = *doc.referenced_assets().iter().next().unwrap();

    file::save(&doc, &path).unwrap();
    assert!(
        grid_path(&file::sidecar_dir(&path), asset).exists(),
        "the grid was not written beside the document"
    );

    let loaded = file::open(&path).unwrap();
    let node = loaded.arena().try_get(m).unwrap();
    assert!(
        node.is_valid(),
        "the mesh node came back holding an empty placeholder"
    );
    assert_eq!(
        loaded.assets().get(asset).map(|g| &g.data),
        doc.assets().get(asset).map(|g| &g.data),
        "the grid changed across save and load"
    );
    assert_eq!(loaded.hash(), doc.hash(), "the field changed");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_missing_sidecar_directory_fails_the_load() {
    let dir = scratch("missing-dir");
    let path = dir.join("part.shapecad");
    let (doc, _) = doc_with_mesh();
    file::save(&doc, &path).unwrap();
    std::fs::remove_dir_all(file::sidecar_dir(&path)).unwrap();

    let err = file::open(&path).unwrap_err();
    assert!(
        matches!(err, FileError::Asset(AssetError::Missing { .. })),
        "expected the load to name the missing asset, got {err}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_incomplete_sidecar_directory_fails_the_load() {
    let dir = scratch("incomplete");
    let path = dir.join("part.shapecad");
    let (doc, _) = doc_with_mesh();
    let asset = *doc.referenced_assets().iter().next().unwrap();
    file::save(&doc, &path).unwrap();
    std::fs::remove_file(grid_path(&file::sidecar_dir(&path), asset)).unwrap();

    let err = file::open(&path).unwrap_err();
    assert!(
        matches!(err, FileError::Asset(AssetError::Missing { .. })),
        "expected a missing asset, got {err}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_corrupt_sidecar_file_fails_the_load() {
    let dir = scratch("corrupt");
    let path = dir.join("part.shapecad");
    let (doc, _) = doc_with_mesh();
    let asset = *doc.referenced_assets().iter().next().unwrap();
    file::save(&doc, &path).unwrap();
    std::fs::write(grid_path(&file::sidecar_dir(&path), asset), b"rubbish").unwrap();

    let err = file::open(&path).unwrap_err();
    assert!(
        matches!(err, FileError::Asset(AssetError::Malformed { .. })),
        "expected a malformed grid, got {err}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// The case most likely to be got wrong. If the grids stay where the document
/// was first saved, the new file is broken the moment the old one moves.
#[test]
fn saving_to_a_new_path_takes_the_assets_with_it() {
    let dir = scratch("saveas");
    let first = dir.join("original.shapecad");
    let second = dir.join("elsewhere/copy.shapecad");
    std::fs::create_dir_all(second.parent().unwrap()).unwrap();

    let (doc, m) = doc_with_mesh();
    file::save(&doc, &first).unwrap();
    file::save(&doc, &second).unwrap();

    // Remove the original outright: the copy must stand on its own.
    std::fs::remove_dir_all(file::sidecar_dir(&first)).unwrap();
    std::fs::remove_file(&first).unwrap();

    let loaded = file::open(&second).unwrap();
    assert!(loaded.arena().try_get(m).unwrap().is_valid());
    assert_eq!(loaded.hash(), doc.hash());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_deleted_mesh_is_not_written_but_undo_still_has_it() {
    let dir = scratch("deleted");
    let path = dir.join("part.shapecad");

    let mut doc = Document::new();
    let m = doc.add_mesh(grid()).unwrap();
    let s = add(&mut doc, Node::Sphere { radius: 3.0 }).unwrap();
    doc.apply(Command::SetRoot { root: Some(s) }).unwrap();
    let asset = *doc.referenced_assets().iter().next().unwrap();

    doc.apply(Command::Delete { id: m }).unwrap();
    assert!(
        doc.referenced_assets().is_empty(),
        "a tombstoned node still counted as a reference"
    );

    file::save(&doc, &path).unwrap();
    assert!(
        !file::sidecar_dir(&path).exists(),
        "megabytes of dead geometry were written beside the document"
    );

    // The grid is gone from the file but must not be gone from the session:
    // undo is still holding the node that refers to it.
    assert!(doc.assets().contains(asset), "undo was left with a hole");
    doc.undo().unwrap();
    assert!(
        doc.arena().try_get(m).unwrap().is_valid(),
        "undo restored a mesh node with no geometry in it"
    );

    file::save(&doc, &path).unwrap();
    assert!(grid_path(&file::sidecar_dir(&path), asset).exists());

    std::fs::remove_dir_all(&dir).ok();
}

/// Undo alone must not collect anything: the redo tail is still reachable.
#[test]
fn an_undone_import_survives_for_redo() {
    let mut doc = Document::new();
    let m = doc.add_mesh(grid()).unwrap();
    let asset = *doc.referenced_assets().iter().next().unwrap();

    doc.undo().unwrap();
    assert!(!doc.arena().is_alive(m));
    assert!(
        doc.assets().contains(asset),
        "the grid was collected while redo could still reach it"
    );

    doc.redo().unwrap();
    assert!(doc.arena().try_get(m).unwrap().is_valid());
}

/// Discarding the redo tail is the one moment an asset really dies.
#[test]
fn a_new_edit_after_undo_collects_the_dead_grid() {
    let mut doc = Document::new();
    doc.add_mesh(grid()).unwrap();
    let asset = *doc.referenced_assets().iter().next().unwrap();

    doc.undo().unwrap();
    assert!(doc.assets().contains(asset));

    add(&mut doc, Node::Sphere { radius: 1.0 }).unwrap();
    assert!(
        !doc.assets().contains(asset),
        "an unreachable grid stayed in memory for the rest of the session"
    );
    assert!(doc.assets().is_empty());
}
