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
fn version_6_is_what_this_build_writes() {
    assert_eq!(FORMAT_VERSION, 6);
    assert_eq!(samples::bracket().snapshot().format, 6);
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

/// Deleting a node deliberately leaves its label behind, so that undoing the
/// deletion brings the two back together. Writing that label out produced a
/// file this build's own loader refused, which is the one thing a save must
/// never do: a saved file that cannot be opened is not a saved file.
#[test]
fn a_label_left_on_a_deleted_node_does_not_break_the_file() {
    let mut doc = Document::new();
    let keep = sc_doc::add(&mut doc, Node::Sphere { radius: 4.0 }).unwrap();
    let scrap = sc_doc::add(&mut doc, Node::Sphere { radius: 2.0 }).unwrap();
    doc.apply(Command::SetRoot { root: Some(keep) }).unwrap();
    doc.apply(Command::SetName {
        id: scrap,
        name: Some("offcut".into()),
    })
    .unwrap();
    doc.apply(Command::Delete { id: scrap }).unwrap();
    assert_eq!(doc.name(scrap), Some("offcut"), "undo lost the label");

    assert!(
        doc.snapshot().names.iter().all(|(id, _)| *id != scrap),
        "a tombstoned label was written to the file"
    );

    let path = scratch("deadlabel");
    file::save(&doc, &path).unwrap();
    let loaded = file::open(&path).expect("a saved document must reopen");
    assert_eq!(loaded.hash(), doc.hash());
    assert_eq!(loaded.name(scrap), None);

    std::fs::remove_file(&path).ok();
}

/// Everything below writes an arena by hand, because these are the states no
/// sequence of commands can produce: a file is the one way into the kernel that
/// nothing else checks.
fn load_arena(name: &str, slots: &str, root: &str) -> Result<Document, FileError> {
    let json = format!(
        r#"{{"format":4,"generator":"test","units":"mm",
            "arena":{{"slots":[{slots}]}},"root":{root},"names":[]}}"#
    );
    let path = scratch(name);
    std::fs::write(&path, json).unwrap();
    let out = file::open(&path);
    std::fs::remove_file(&path).ok();
    out
}

/// A child pointing at a tombstone evaluates as empty space rather than as an
/// error, so the part opens looking like a part with a piece missing.
#[test]
fn a_child_pointing_at_a_tombstone_is_refused() {
    let err = load_arena(
        "dangling",
        r#"null,{"Union":{"a":0,"b":0,"smooth":0.0}}"#,
        "1",
    )
    .expect_err("a dangling child loaded");
    assert!(matches!(err, FileError::Invalid(_)), "got {err}");
}

/// A negative radius bounds to an inverted box, `min` above `max`. Bounds may
/// over-report and never under-report, and an inverted one clips the whole part
/// out of an export without anything saying so.
#[test]
fn an_out_of_range_parameter_is_refused() {
    let err = load_arena("badparam", r#"{"Sphere":{"radius":-5.0}}"#, "0")
        .expect_err("an invalid node loaded");
    assert!(matches!(err, FileError::Invalid(_)), "got {err}");
}

/// The arena is a DAG. A loop in it is not a shape, it is a walk that never
/// finishes: evaluation recurses until the stack runs out.
#[test]
fn a_cyclic_arena_is_refused() {
    let err = load_arena(
        "cyclic",
        r#"{"Union":{"a":1,"b":1,"smooth":0.0}},{"Union":{"a":0,"b":0,"smooth":0.0}}"#,
        "0",
    )
    .expect_err("a cyclic arena loaded");
    assert!(matches!(err, FileError::Invalid(_)), "got {err}");
}

/// The arena refuses to delete a node a placement is derived from, so a file
/// holding that pair is a file no session produced. Regeneration would silently
/// leave the placement where it was found.
#[test]
fn a_placement_derived_from_a_dead_node_is_refused() {
    let err = load_arena(
        "deadbase",
        r#"{"Sphere":{"radius":2.0}},null,
           {"Transform":{"child":0,"xform":{"translation":[0.0,0.0,0.0],
            "rotation":[0.0,0.0,0.0,1.0],"scale":1.0},"on":1}}"#,
        "2",
    )
    .expect_err("a derivation on a dead node loaded");
    assert!(matches!(err, FileError::Invalid(_)), "got {err}");
}

/// A placement derived from something inside its own subtree would be defined
/// in terms of its own result. The arena rejects the edit that would make one;
/// loading has to reject the file that already contains one.
#[test]
fn a_placement_derived_from_its_own_subtree_is_refused() {
    let err = load_arena(
        "selfbase",
        r#"{"Sphere":{"radius":2.0}},
           {"Transform":{"child":0,"xform":{"translation":[0.0,0.0,1.0],
            "rotation":[0.0,0.0,0.0,1.0],"scale":1.0},"on":0}}"#,
        "1",
    )
    .expect_err("a self-derived placement loaded");
    assert!(matches!(err, FileError::Invalid(_)), "got {err}");
}

/// Validation must not cost a real document its ability to open. The engine is
/// the deepest model in the repo and every node in it is legal.
#[test]
fn validation_still_lets_the_samples_through() {
    for doc in [samples::bracket(), samples::engine()] {
        let snapshot = doc.snapshot();
        let back = Document::from_snapshot(snapshot).expect("a sample failed validation");
        assert_eq!(back.hash(), doc.hash());
    }
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
