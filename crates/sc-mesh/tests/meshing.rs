//! Correctness of the mesher.
//!
//! "Watertight by construction" is a claim about the algorithm. These tests
//! measure it on real output, because a hole in an exported mesh is the single
//! defect that makes a part unprintable, and it is invisible in a render.

use sc_geom::glam::Vec3;
use sc_geom::{Builder, NodeId};
use sc_mesh::{contour, Mesh, Settings};

fn mesh_of(build: impl FnOnce(&mut Builder) -> NodeId, resolution: u32) -> Mesh {
    let mut b = Builder::new();
    let root = build(&mut b);
    contour(
        &b.arena,
        root,
        Settings {
            resolution,
            ..Settings::default()
        },
    )
}

fn assert_printable(mesh: &Mesh, what: &str) {
    let t = mesh.topology();
    assert!(
        t.is_manifold(),
        "{what} has {} non-manifold edges",
        t.non_manifold_edges
    );
    assert!(
        t.is_printable(),
        "{what} is not printable: {} boundary edges (holes), {} non-manifold, {} inconsistent winding",
        t.boundary_edges,
        t.non_manifold_edges,
        t.inconsistent_edges
    );
}

#[test]
fn a_sphere_is_watertight_and_the_right_size() {
    let mesh = mesh_of(|b| b.sphere(10.0).unwrap(), 64);
    assert!(mesh.triangle_count() > 0, "nothing was meshed");
    assert_printable(&mesh, "sphere");

    let expected = 4.0 / 3.0 * std::f32::consts::PI * 1000.0;
    let actual = mesh.volume();
    // A positive volume also proves the winding is outward; inside-out geometry
    // would come out negative.
    assert!(
        (actual - expected).abs() / expected < 0.02,
        "sphere volume {actual} differs from {expected} by more than 2%"
    );
}

#[test]
fn a_cube_keeps_its_corners() {
    let mesh = mesh_of(|b| b.cube(10.0).unwrap(), 48);
    assert_printable(&mesh, "cube");

    let actual = mesh.volume();
    assert!(
        (actual - 8000.0).abs() / 8000.0 < 0.02,
        "cube volume {actual} differs from 8000 by more than 2%"
    );

    // Dual contouring should reproduce the corners exactly. Marching cubes would
    // round them off and pull the bounds inward.
    let b = mesh.bounds();
    for (axis, v) in [("x", b.max.x), ("y", b.max.y), ("z", b.max.z)] {
        assert!(
            (v - 10.0).abs() < 0.3,
            "{axis} extent {v} is not the sharp corner at 10.0"
        );
    }
}

#[test]
fn a_hollow_shell_is_watertight_on_both_surfaces() {
    // Two nested surfaces is where a mesher that mishandles orientation shows
    // it: the inner surface must wind the opposite way.
    let mesh = mesh_of(
        |b| {
            let c = b.cube(10.0).unwrap();
            b.shell(c, 2.0).unwrap()
        },
        64,
    );
    assert_printable(&mesh, "shell");

    let solid = 20.0f32.powi(3);
    let cavity = 16.0f32.powi(3);
    let expected = solid - cavity;
    let actual = mesh.volume();
    assert!(
        (actual - expected).abs() / expected < 0.05,
        "shell volume {actual} differs from {expected} by more than 5%"
    );
}

#[test]
fn a_part_with_holes_through_it_is_watertight() {
    let mesh = mesh_of(
        |b| {
            let body = b.cuboid(Vec3::new(20.0, 12.0, 4.0)).unwrap();
            let drill = b.cylinder(3.0, 10.0).unwrap();
            let left = b.translate(drill, Vec3::new(-10.0, 0.0, 0.0)).unwrap();
            let right = b.translate(drill, Vec3::new(10.0, 0.0, 0.0)).unwrap();
            let cut = b.difference(body, left).unwrap();
            b.difference(cut, right).unwrap()
        },
        96,
    );
    assert_printable(&mesh, "drilled plate");

    let expected = 40.0 * 24.0 * 8.0 - 2.0 * std::f32::consts::PI * 9.0 * 8.0;
    let actual = mesh.volume();
    assert!(
        (actual - expected).abs() / expected < 0.03,
        "drilled plate volume {actual} differs from {expected} by more than 3%"
    );
}

#[test]
fn a_filleted_joint_is_watertight() {
    let mesh = mesh_of(
        |b| {
            let plate = b.cuboid(Vec3::new(20.0, 12.0, 3.0)).unwrap();
            let wall = b.cuboid(Vec3::new(20.0, 3.0, 10.0)).unwrap();
            let wall = b.translate(wall, Vec3::new(0.0, 9.0, 10.0)).unwrap();
            b.smooth_union(plate, wall, 4.0).unwrap()
        },
        96,
    );
    assert_printable(&mesh, "filleted joint");
    assert!(mesh.volume() > 0.0, "winding is inside out");
}

#[test]
fn resolution_converges_toward_the_true_volume() {
    let exact = 4.0 / 3.0 * std::f32::consts::PI * 1000.0;
    let coarse = (mesh_of(|b| b.sphere(10.0).unwrap(), 24).volume() - exact).abs();
    let fine = (mesh_of(|b| b.sphere(10.0).unwrap(), 96).volume() - exact).abs();
    assert!(
        fine < coarse,
        "refining the grid did not improve accuracy: {coarse} then {fine}"
    );
}

#[test]
fn an_empty_region_meshes_to_nothing_rather_than_panicking() {
    // An intersection of two disjoint solids encloses no volume at all.
    let mut b = Builder::new();
    let a = b.sphere(1.0).unwrap();
    let far = b.sphere(1.0).unwrap();
    let far = b.translate(far, Vec3::new(100.0, 0.0, 0.0)).unwrap();
    let empty = b.intersection(a, far).unwrap();
    let mesh = contour(
        &b.arena,
        empty,
        Settings {
            resolution: 16,
            ..Settings::default()
        },
    );
    assert_eq!(mesh.triangle_count(), 0);
}

/// Documents a real limitation rather than asserting it away.
///
/// Uniform dual contouring places one vertex per cell. Where two sheets of the
/// surface pass through the same cell, that vertex serves both and the edges
/// around it end up shared by four triangles. On this model at this resolution
/// roughly four percent of edges are affected, which is not a stray cell but a structural
/// property of the algorithm.
///
/// The mesh is still closed and consistently wound, so it slices; most tools
/// repair the rest silently. The real fix is Manifold Dual Contouring: detect
/// that a cell's crossings form more than one connected component and emit a
/// vertex per component.
///
/// If this test starts failing because the count reached zero, that work has
/// landed, delete the test and tighten `assert_printable` to `is_manifold`.
#[test]
fn known_limitation_two_sheets_in_one_cell_are_non_manifold() {
    let mesh = mesh_of(
        |b| {
            let body = b.cylinder(5.76, 2.0).unwrap();
            let big = b.sphere(5.07).unwrap();
            let small = b.sphere(2.0).unwrap();
            let tool = b.union(big, small).unwrap();
            b.smooth_difference(body, tool, 1.18).unwrap()
        },
        24,
    );

    let t = mesh.topology();
    assert!(t.is_printable(), "closed and consistently wound regardless");
    assert!(
        !t.is_manifold(),
        "no longer produces non-manifold edges - see the doc comment above"
    );

    // Refining the grid must not make it worse; the defect is grid-relative.
    let finer = mesh_of(
        |b| {
            let body = b.cylinder(5.76, 2.0).unwrap();
            let big = b.sphere(5.07).unwrap();
            let small = b.sphere(2.0).unwrap();
            let tool = b.union(big, small).unwrap();
            b.smooth_difference(body, tool, 1.18).unwrap()
        },
        96,
    );
    let coarse_share = t.non_manifold_edges as f32 / mesh.triangle_count() as f32;
    let fine_share = finer.topology().non_manifold_edges as f32 / finer.triangle_count() as f32;
    assert!(
        fine_share <= coarse_share,
        "refining made it proportionally worse: {coarse_share} then {fine_share}"
    );
}
