//! The reference models, end to end.
//!
//! `sc-doc` and `sc-mesh` are siblings: both sit on the kernel and neither
//! depends on the other. This crate is where they meet, so it is the only place
//! a test can take a sample document all the way to a printable mesh.

use sc_doc::samples;
use sc_mesh::{contour, Mesh, Settings};

fn assert_printable(mesh: &Mesh, what: &str) {
    let t = mesh.topology();
    assert!(
        t.is_printable(),
        "{what} is not printable: {} boundary edges (holes), {} non-manifold, {} inconsistent winding",
        t.boundary_edges,
        t.non_manifold_edges,
        t.inconsistent_edges
    );
}

/// The engine example is the deepest model in the repo: a hollowed crankcase, a
/// finned barrel, and eleven holes of three different kinds. If the mesher is
/// going to produce something unprintable, it will be here rather than on a
/// bracket made of two boxes.
#[test]
fn the_engine_example_meshes_to_a_printable_solid() {
    let doc = samples::engine();
    let root = doc.root().expect("a sample is always rooted");
    let mesh = contour(
        doc.arena(),
        root,
        Settings {
            resolution: 128,
            refinement: 2,
        },
    );

    assert!(
        mesh.triangle_count() > 20_000,
        "only {} triangles for a model this size, so something collapsed",
        mesh.triangle_count()
    );
    assert_printable(&mesh, "the engine example");
}

/// The cylinder bore is a through cut, so it has to be open along its whole
/// length: from the top of the head, through the barrel, into the crankcase.
/// A prism has no depth to fall out of date, which is the point of using one.
#[test]
fn the_engine_bore_is_open_from_end_to_end() {
    let doc = samples::engine();
    let root = doc.root().expect("rooted");
    let bounds = doc.bounds().expect("rooted");

    for z in [bounds.max.z - 2.0, 120.0, 82.0, 56.0] {
        let d = sc_geom::eval(doc.arena(), root, sc_geom::glam::Vec3::new(0.0, 0.0, z));
        assert!(d > 0.0, "the bore is blocked at z={z}, field reads {d}");
    }
}

/// It is meant to be a deeper model than the bracket. If it ever stops being
/// one, it has stopped earning its place.
#[test]
fn the_engine_example_is_substantially_deeper_than_the_bracket() {
    let engine = samples::engine();
    let bracket = samples::bracket();
    assert!(
        engine.arena().len() > bracket.arena().len() * 3,
        "engine has {} nodes against the bracket's {}",
        engine.arena().len(),
        bracket.arena().len()
    );
}

/// Samples are fixtures, so they have to be reproducible. A sample whose
/// geometry drifts between runs would make every test built on it meaningless.
#[test]
fn the_engine_example_is_deterministic() {
    assert_eq!(samples::engine().hash(), samples::engine().hash());
}

/// It exists to be opened, so it has to survive being written and read back.
#[test]
fn the_engine_example_round_trips_through_the_file_format() {
    let doc = samples::engine();
    let mut path = std::env::temp_dir();
    path.push(format!("shapecad-engine-{}.shapecad", std::process::id()));

    sc_doc::file::save(&doc, &path).expect("saved");
    let loaded = sc_doc::file::open(&path).expect("opened");

    assert_eq!(
        loaded.hash(),
        doc.hash(),
        "geometry changed on the way back"
    );
    assert_eq!(loaded.outline(), doc.outline(), "the tree changed");
    std::fs::remove_file(&path).ok();
}
