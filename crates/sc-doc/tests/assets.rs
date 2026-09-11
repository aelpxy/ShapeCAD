//! The sidecar directory: what gets written, what gets read back, and what is
//! refused.
//!
//! Grids are the one part of a document that is not in the JSON, so they are the
//! one part whose integrity nothing else checks. Everything here is about a
//! corrupt or absent grid being a typed failure rather than a part that opens
//! looking subtly wrong.

use sc_doc::asset::{
    decode_grid, encode_grid, grid_path, AssetError, AssetStore, GridError, GRID_VERSION,
};
use sc_doc::{file, samples, AssetId, Grid};
use sc_geom::glam::Vec3;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

/// A distinct scratch directory per test, so a failure in one does not leave
/// another looking at its debris.
fn scratch(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("shapecad-assets-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&p).ok();
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn grid(dims: [u32; 3], seed: f32) -> Grid {
    let n = (dims[0] * dims[1] * dims[2]) as usize;
    Grid {
        dims,
        origin: Vec3::new(-2.0, 1.0, 0.5),
        spacing: 0.25,
        data: (0..n).map(|i| seed + i as f32 * 0.5).collect(),
    }
}

fn set(ids: &[AssetId]) -> BTreeSet<AssetId> {
    ids.iter().copied().collect()
}

/// The two halves of a save, in the order `file::save` runs them.
fn save(store: &AssetStore, dir: &std::path::Path, referenced: &BTreeSet<AssetId>) {
    store.write_dir(dir, referenced).unwrap();
    AssetStore::prune_dir(dir, referenced).unwrap();
}

#[test]
fn a_grid_survives_a_trip_through_the_sidecar_directory_byte_for_byte() {
    let dir = scratch("roundtrip");
    let mut store = AssetStore::new();
    let a = store.insert(grid([5, 4, 3], 1.0));
    let b = store.insert(grid([2, 2, 2], -7.5));

    save(&store, &dir, &set(&[a, b]));
    let back = AssetStore::read_dir(&dir, &set(&[a, b])).unwrap();

    assert_eq!(back.len(), 2);
    for id in [a, b] {
        assert_eq!(
            **back.get(id).unwrap(),
            **store.get(id).unwrap(),
            "grid {} changed on the way to disk and back",
            id.0
        );
    }
    assert_eq!(
        encode_grid(back.get(a).unwrap()),
        encode_grid(store.get(a).unwrap()),
        "re-encoding after a load was not byte identical"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn ids_are_the_same_after_a_round_trip() {
    let dir = scratch("ids");
    let mut store = AssetStore::new();
    store.insert(grid([2, 2, 2], 0.0));
    let second = store.insert(grid([2, 2, 2], 1.0));
    let ids: Vec<_> = store.ids().collect();

    save(&store, &dir, &set(&ids));
    let back = AssetStore::read_dir(&dir, &set(&ids)).unwrap();

    assert_eq!(back.ids().collect::<Vec<_>>(), ids, "ids were renumbered");
    // The allocator must not rewind either, or the next import would overwrite
    // geometry that is already in the document.
    let mut back = back;
    assert!(back.alloc().0 > second.0, "an id could be handed out twice");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_unreferenced_grid_is_not_written() {
    let dir = scratch("unreferenced");
    let mut store = AssetStore::new();
    let live = store.insert(grid([3, 3, 3], 0.0));
    let dead = store.insert(grid([3, 3, 3], 9.0));

    save(&store, &dir, &set(&[live]));

    assert!(grid_path(&dir, live).exists());
    assert!(
        !grid_path(&dir, dead).exists(),
        "an unreachable grid was written beside the document"
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

/// A grid that stops being referenced has to stop existing on disk too, or
/// saving over the same path forever accumulates every mesh the part ever had.
#[test]
fn a_grid_that_stops_being_referenced_is_deleted_on_the_next_save() {
    let dir = scratch("stale");
    let mut store = AssetStore::new();
    let a = store.insert(grid([2, 2, 2], 0.0));
    let b = store.insert(grid([2, 2, 2], 1.0));

    save(&store, &dir, &set(&[a, b]));
    assert!(grid_path(&dir, b).exists());

    save(&store, &dir, &set(&[a]));
    assert!(grid_path(&dir, a).exists());
    assert!(!grid_path(&dir, b).exists(), "a stale grid survived a save");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_directory_with_nothing_left_in_it_is_removed() {
    let dir = scratch("empty").join("part.assets");
    let mut store = AssetStore::new();
    let a = store.insert(grid([2, 2, 2], 0.0));

    save(&store, &dir, &set(&[a]));
    assert!(dir.exists());

    save(&store, &dir, &BTreeSet::new());
    assert!(
        !dir.exists(),
        "an empty sidecar directory was left behind, so a part with no meshes \
         is no longer a single file"
    );

    std::fs::remove_dir_all(dir.parent().unwrap()).ok();
}

/// Saving to a new path has to take the geometry with it. A document whose
/// grids stayed behind is a document that cannot be opened.
#[test]
fn saving_to_a_new_directory_takes_the_grids_along() {
    let root = scratch("saveas");
    let first = root.join("original.assets");
    let second = root.join("copy.assets");

    let mut store = AssetStore::new();
    let a = store.insert(grid([4, 4, 4], 3.0));
    save(&store, &first, &set(&[a]));
    save(&store, &second, &set(&[a]));

    let from_copy = AssetStore::read_dir(&second, &set(&[a])).unwrap();
    assert_eq!(**from_copy.get(a).unwrap(), **store.get(a).unwrap());
    assert!(
        grid_path(&first, a).exists(),
        "the original's sidecar was moved rather than copied, breaking it"
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_missing_sidecar_directory_is_an_error_not_an_empty_store() {
    let dir = scratch("absent").join("never-written.assets");
    let err = AssetStore::read_dir(&dir, &set(&[AssetId(0)])).unwrap_err();
    assert!(
        matches!(err, AssetError::Missing { asset, .. } if asset == AssetId(0)),
        "expected a missing-asset error, got {err}"
    );
    std::fs::remove_dir_all(dir.parent().unwrap()).ok();
}

#[test]
fn an_incomplete_sidecar_directory_is_an_error() {
    let dir = scratch("incomplete");
    let mut store = AssetStore::new();
    let a = store.insert(grid([2, 2, 2], 0.0));
    let b = store.insert(grid([2, 2, 2], 1.0));
    save(&store, &dir, &set(&[a, b]));
    std::fs::remove_file(grid_path(&dir, b)).unwrap();

    let err = AssetStore::read_dir(&dir, &set(&[a, b])).unwrap_err();
    assert!(
        matches!(err, AssetError::Missing { asset, .. } if asset == b),
        "expected the missing asset to be named, got {err}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_truncated_sidecar_file_is_rejected() {
    let dir = scratch("truncated");
    let mut store = AssetStore::new();
    let a = store.insert(grid([4, 4, 4], 0.0));
    save(&store, &dir, &set(&[a]));

    let path = grid_path(&dir, a);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 16]).unwrap();

    let err = AssetStore::read_dir(&dir, &set(&[a])).unwrap_err();
    assert!(
        matches!(
            err,
            AssetError::Malformed {
                cause: GridError::SizeMismatch { .. },
                ..
            }
        ),
        "expected a size mismatch, got {err}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_sidecar_file_with_the_wrong_magic_is_rejected() {
    let dir = scratch("magic");
    let mut store = AssetStore::new();
    let a = store.insert(grid([2, 2, 2], 0.0));
    save(&store, &dir, &set(&[a]));

    // Long enough to have a header, so it is the magic being wrong that is
    // caught rather than the length.
    let path = grid_path(&dir, a);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[..8].copy_from_slice(b"PLYnPLYn");
    std::fs::write(&path, &bytes).unwrap();

    let err = AssetStore::read_dir(&dir, &set(&[a])).unwrap_err();
    assert!(
        matches!(
            err,
            AssetError::Malformed {
                cause: GridError::BadMagic,
                ..
            }
        ),
        "expected bad magic, got {err}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_sidecar_file_from_a_newer_build_is_rejected() {
    let dir = scratch("gridversion");
    let mut store = AssetStore::new();
    let a = store.insert(grid([2, 2, 2], 0.0));
    save(&store, &dir, &set(&[a]));

    let path = grid_path(&dir, a);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[8..12].copy_from_slice(&(GRID_VERSION + 1).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let err = AssetStore::read_dir(&dir, &set(&[a])).unwrap_err();
    assert!(
        matches!(
            err,
            AssetError::Malformed {
                cause: GridError::UnsupportedVersion(_),
                ..
            }
        ),
        "expected an unsupported grid version, got {err}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// Whatever the store does in memory, the bytes are the contract. A grid read
/// from a file another build wrote has to compare equal to the one that went in.
#[test]
fn the_encoding_is_stable_across_the_store() {
    let g = grid([3, 1, 2], 0.75);
    let bytes = encode_grid(&g);
    assert_eq!(decode_grid(&bytes).unwrap(), g);

    let mut store = AssetStore::new();
    store.insert_at(AssetId(4), Arc::new(g.clone()));
    let dir = scratch("stable");
    save(&store, &dir, &set(&[AssetId(4)]));
    assert_eq!(std::fs::read(grid_path(&dir, AssetId(4))).unwrap(), bytes);

    std::fs::remove_dir_all(&dir).ok();
}

/// The promise the format makes to a part with no imported meshes: it is still
/// one file, and nothing new appears next to it.
#[test]
fn a_document_with_no_assets_is_still_a_single_file() {
    let dir = scratch("plain");
    let path = dir.join("bracket.shapecad");
    let doc = samples::bracket();
    assert!(doc.referenced_assets().is_empty());

    file::save(&doc, &path).unwrap();

    assert!(
        !file::sidecar_dir(&path).exists(),
        "an empty sidecar was made"
    );
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        1,
        "saving produced more than one file"
    );

    std::fs::remove_dir_all(&dir).ok();
}
