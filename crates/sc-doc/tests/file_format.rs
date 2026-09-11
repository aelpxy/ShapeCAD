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
