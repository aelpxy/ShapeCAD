//! Round-tripping documents through the native format.

use sc_doc::file::{self, DocumentFile, FileError, FORMAT_VERSION};
use sc_doc::{samples, Command, Document};
use sc_geom::Node;

fn scratch(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "shapecad-test-{name}-{}.shapecad",
        std::process::id()
    ));
    p
}

#[test]
fn a_saved_document_reloads_identically() {
    let doc = samples::bracket();
    let path = scratch("roundtrip");
    file::save(&doc, &path).unwrap();

    let loaded = file::open(&path).unwrap();
    assert_eq!(
        loaded.hash(),
        doc.hash(),
        "geometry changed across save and load"
    );
    assert_eq!(loaded.outline(), doc.outline(), "tree or names changed");
    assert_eq!(loaded.root(), doc.root(), "root id changed");

    std::fs::remove_file(&path).ok();
}

#[test]
fn opening_a_document_starts_a_fresh_history() {
    // Undo history belongs to a session, not to a part. Reopening a file must
    // not offer to undo edits made before it was saved.
    let mut doc = Document::new();
    let a = sc_doc::add(&mut doc, Node::Sphere { radius: 4.0 }).unwrap();
    doc.apply(Command::SetRoot { root: Some(a) }).unwrap();
    assert!(doc.can_undo());

    let path = scratch("history");
    file::save(&doc, &path).unwrap();
    let loaded = file::open(&path).unwrap();

    assert!(!loaded.can_undo());
    assert!(!loaded.can_redo());
    assert_eq!(loaded.log_len(), 0);

    std::fs::remove_file(&path).ok();
}

#[test]
fn ids_survive_a_round_trip() {
    // Stable ids are the kernel's central promise. A save that renumbered them
    // would invalidate every stored selection and agent-held reference.
    let doc = samples::bracket();
    let ids: Vec<_> = doc.arena().live_ids().collect();

    let path = scratch("ids");
    file::save(&doc, &path).unwrap();
    let loaded = file::open(&path).unwrap();

    assert_eq!(loaded.arena().live_ids().collect::<Vec<_>>(), ids);
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_newer_format_is_refused_rather_than_misread() {
    let mut snapshot = samples::bracket().snapshot();
    snapshot.format = FORMAT_VERSION + 1;
    let path = scratch("version");
    std::fs::write(&path, serde_json::to_string(&snapshot).unwrap()).unwrap();

    assert!(matches!(
        file::open(&path),
        Err(FileError::UnsupportedVersion(_))
    ));
    std::fs::remove_file(&path).ok();
}

/// A version 2 file predates both mesh nodes and prisms, so it can contain
/// neither, which means it has no sidecar directory and the absence of one must
/// not be read as a missing asset.
#[test]
fn a_version_2_document_still_loads() {
    let doc = samples::bracket();
    let mut snapshot = doc.snapshot();
    snapshot.format = 2;
    let path = scratch("version2");
    std::fs::write(&path, serde_json::to_string(&snapshot).unwrap()).unwrap();
    assert!(!file::sidecar_dir(&path).exists());

    let loaded = file::open(&path).unwrap();
    assert_eq!(loaded.hash(), doc.hash(), "an older file read differently");

    std::fs::remove_file(&path).ok();
}

#[test]
fn version_4_is_what_this_build_writes() {
    assert_eq!(FORMAT_VERSION, 4);
    assert_eq!(samples::bracket().snapshot().format, 4);
}

#[test]
fn a_corrupt_file_fails_cleanly() {
    let path = scratch("corrupt");
    std::fs::write(&path, "{ this is not a document").unwrap();
    assert!(matches!(file::open(&path), Err(FileError::Parse(_))));
    std::fs::remove_file(&path).ok();
}

#[test]
fn a_snapshot_naming_a_dead_node_is_rejected() {
    let doc = samples::bracket();
    let mut snapshot = doc.snapshot();
    snapshot.root = Some(sc_geom::NodeId(9_999));
    assert!(
        Document::from_snapshot(snapshot).is_err(),
        "dangling root was accepted"
    );
}

#[test]
fn the_format_is_readable_json() {
    // The file being legible to a person - and to a language model - is a
    // deliberate property of the format, not an accident of the encoder.
    let doc = samples::bracket();
    let text = serde_json::to_string_pretty(&doc.snapshot()).unwrap();
    assert!(text.contains("\"units\": \"mm\""), "{text}");
    assert!(
        text.contains("\"Cylinder\""),
        "node kinds should be named, not tagged numerically"
    );

    let parsed: DocumentFile = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed.format, FORMAT_VERSION);
}

/// A version 2 file was written before a placement could name the feature it
/// was derived from. Adding that field must not strand every document already
/// on disk, which is why it defaults rather than bumping the format version.
#[test]
fn a_version_2_document_without_a_derivation_still_loads() {
    // Written by hand exactly as an older build would have: the transform has a
    // child and an xform and nothing else.
    let legacy = r#"{
      "format": 2,
      "generator": "ShapeCAD 0.0.1",
      "units": "mm",
      "arena": {
        "slots": [
          { "Extrude": { "profile": { "Rect": { "width": 20.0, "height": 10.0 } }, "depth": 5.0 } },
          {
            "Transform": {
              "child": 0,
              "xform": {
                "translation": [0.0, 0.0, 5.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": 1.0
              }
            }
          }
        ]
      },
      "root": 1,
      "names": []
    }"#;

    let path = scratch("legacy-v2");
    std::fs::write(&path, legacy).unwrap();
    let doc = file::open(&path).unwrap();

    assert_eq!(doc.root(), Some(sc_geom::NodeId(1)));
    let placed = doc.arena().try_get(sc_geom::NodeId(1)).unwrap();
    assert_eq!(
        placed.derived_from(),
        None,
        "an older file gained a derivation out of nowhere"
    );
    let bounds = doc.bounds().expect("rooted");
    assert!((bounds.max.z - 10.0).abs() < 0.01, "{bounds:?}");

    std::fs::remove_file(&path).ok();
}
