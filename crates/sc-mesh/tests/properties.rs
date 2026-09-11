//! Property-based tests for the mesher.
//!
//! Meshing is where a defect becomes a failed print rather than a visual
//! artefact, and holes are invisible in a render. These check the guarantee that
//! matters on arbitrary geometry rather than on the shapes that occurred to me.

// Geometry code names points and distances `p`, `d`, `a`, `b` by long convention.
#![allow(clippy::many_single_char_names)]

use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use sc_geom::glam::Vec3;
use sc_geom::{Builder, NodeId};
use sc_mesh::voxelize::{sample_box, voxel_value, voxelize_bvh};
use sc_mesh::{contour, stl, Bvh, Mesh, Settings};

#[derive(Clone, Debug)]
enum Shape {
    Sphere(f32),
    Cuboid([f32; 3]),
    Cylinder(f32, f32),
    Extrude(usize, f32, f32),
    Union(Box<Shape>, Box<Shape>, f32),
    Difference(Box<Shape>, Box<Shape>, f32),
    Translate(Box<Shape>, [f32; 3]),
    Shell(Box<Shape>, f32),
    /// A feature placed on another and joined to it, carrying the derivation an
    /// attached pad records. The mesher must not see any difference.
    On(Box<Shape>, Box<Shape>, [f32; 3]),
}

fn build(s: &Shape, b: &mut Builder) -> NodeId {
    match s {
        Shape::Sphere(r) => b.sphere(*r).unwrap(),
        Shape::Cuboid(h) => b.cuboid(Vec3::from_array(*h)).unwrap(),
        Shape::Cylinder(r, hh) => b.cylinder(*r, *hh).unwrap(),
        Shape::Extrude(sides, radius, height) => b
            .extrude(
                sc_geom::Profile::RegularPolygon {
                    sides: *sides as u32,
                    radius: *radius,
                },
                *height,
            )
            .unwrap(),
        Shape::Union(x, y, k) => {
            let (a, c) = (build(x, b), build(y, b));
            b.smooth_union(a, c, *k).unwrap()
        }
        Shape::Difference(x, y, k) => {
            let (a, c) = (build(x, b), build(y, b));
            b.smooth_difference(a, c, *k).unwrap()
        }
        Shape::Translate(x, t) => {
            let a = build(x, b);
            b.translate(a, Vec3::from_array(*t)).unwrap()
        }
        Shape::Shell(x, t) => {
            let a = build(x, b);
            b.shell(a, *t).unwrap()
        }
        Shape::On(x, base, t) => {
            let under = build(base, b);
            let child = build(x, b);
            let placed = b
                .arena
                .insert(sc_geom::Node::Transform {
                    child,
                    xform: sc_geom::Transform::from_translation(Vec3::from_array(*t)),
                    on: Some(under),
                })
                .unwrap();
            b.union(under, placed).unwrap()
        }
    }
}

fn arb_leaf() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (2.0f32..8.0).prop_map(Shape::Sphere),
        (2.0f32..8.0, 2.0f32..8.0, 2.0f32..8.0).prop_map(|(x, y, z)| Shape::Cuboid([x, y, z])),
        (2.0f32..6.0, 2.0f32..8.0).prop_map(|(r, h)| Shape::Cylinder(r, h)),
        ((3usize..9), 2.0f32..6.0, 2.0f32..8.0).prop_map(|(n, r, h)| Shape::Extrude(n, r, h)),
    ]
}

fn arb_shape() -> impl Strategy<Value = Shape> {
    arb_leaf().prop_recursive(3, 10, 2, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone(), 0.0f32..2.0).prop_map(|(a, b, k)| Shape::Union(
                Box::new(a),
                Box::new(b),
                k
            )),
            (inner.clone(), inner.clone(), 0.0f32..2.0).prop_map(|(a, b, k)| Shape::Difference(
                Box::new(a),
                Box::new(b),
                k
            )),
            (inner.clone(), (-6.0f32..6.0, -6.0f32..6.0, -6.0f32..6.0))
                .prop_map(|(a, t)| Shape::Translate(Box::new(a), [t.0, t.1, t.2])),
            (inner.clone(), 1.0f32..3.0).prop_map(|(a, t)| Shape::Shell(Box::new(a), t)),
            (
                inner.clone(),
                inner,
                (-6.0f32..6.0, -6.0f32..6.0, -6.0f32..6.0)
            )
                .prop_map(|(a, base, t)| Shape::On(
                    Box::new(a),
                    Box::new(base),
                    [t.0, t.1, t.2]
                )),
        ]
    })
}

/// Meshes the shape at a resolution coarse enough that voxelizing the result
/// stays a unit test rather than a benchmark.
fn mesh_of(s: &Shape) -> Mesh {
    let mut b = Builder::new();
    let root = build(s, &mut b);
    contour(
        &b.arena,
        root,
        Settings {
            resolution: 16,
            refinement: 1,
        },
    )
}

/// Every triangle as three positions, in file order, so two meshes can be
/// compared without depending on how their vertices happen to be welded.
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

fn scratch(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("shapecad-prop-{name}-{}.stl", std::process::id()));
    p
}

/// A deterministic scatter of points in the box, so a counterexample replays.
fn probes(lo: Vec3, hi: Vec3, count: usize) -> Vec<Vec3> {
    (0..count)
        .map(|n| {
            let t = n as f32;
            let u = Vec3::new(
                (t * 0.7351).fract(),
                (t * 0.4327).fract(),
                (t * 0.9137).fract(),
            );
            lo + (hi - lo) * u
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Meshing is far more expensive than field evaluation, so this runs a
        // smaller sample than the kernel's properties do.
        cases: 48,
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/proptest-regressions/properties.txt",
        ))),
        ..ProptestConfig::default()
    })]

    /// Whatever the model, the exported mesh must be closed and consistently
    /// wound. A hole leaves the solid undefined and inverted winding turns it
    /// inside out; both are fatal, and neither is visible in a render.
    #[test]
    fn every_model_meshes_to_closed_geometry(s in arb_shape()) {
        let mut b = Builder::new();
        let root = build(&s, &mut b);
        let mesh = contour(&b.arena, root, Settings { resolution: 24, refinement: 1 });

        let t = mesh.topology();
        prop_assert!(
            t.is_printable(),
            "not printable ({} holes, {} inconsistent winding) for {s:?}",
            t.boundary_edges, t.inconsistent_edges
        );
    }


    /// A mesh that encloses anything must enclose a positive volume. A negative
    /// one means the surface is wound inside out, which slicers read as a hole
    /// the size of the part.
    #[test]
    fn enclosed_volume_is_never_negative(s in arb_shape()) {
        let mut b = Builder::new();
        let root = build(&s, &mut b);
        let mesh = contour(&b.arena, root, Settings { resolution: 24, refinement: 1 });
        prop_assert!(mesh.volume() >= -1.0e-3, "inside-out mesh for {s:?}");
    }

    /// Every vertex must sit within its own cell of the surface.
    ///
    /// The QEF clamps each vertex into the cell that produced it, so the worst
    /// honest case is the cell diagonal: a surface that only clips one corner.
    /// Anything beyond that means the solver escaped its cell, which is how a
    /// mesher silently emits spikes and self-intersecting triangles.
    #[test]
    fn vertices_lie_on_the_surface(s in arb_shape()) {
        let mut b = Builder::new();
        let root = build(&s, &mut b);
        let settings = Settings { resolution: 24, refinement: 2 };
        let mesh = contour(&b.arena, root, settings);

        let size = sc_geom::bounds(&b.arena, root).finite_or(1000.0).size().max_element();
        let spacing = size / settings.resolution as f32;
        // Cell diagonal, with a little slack for the crossing refinement.
        let limit = spacing * 3.0f32.sqrt() * 1.15;
        for p in &mesh.positions {
            let d = sc_geom::eval(&b.arena, root, *p).abs();
            prop_assert!(
                d <= limit,
                "vertex {p:?} is {d} from the surface, beyond the {limit} cell diagonal, for {s:?}"
            );
        }
    }

    /// Sampling the same mesh twice as finely must describe the same surface.
    ///
    /// Neither grid is ground truth, so this cannot check accuracy; what it
    /// checks is consistency, which catches the whole family of off-by-one
    /// errors in the layout arithmetic. A grid whose origin, spacing or
    /// dimensions disagree by one voxel produces a field shifted by a voxel,
    /// and that shows up here as soon as the two resolutions differ.
    ///
    /// The bound is the coarser spacing: that is the resolution at which the
    /// coarse grid can represent anything at all, so two grids that agree to
    /// within it are describing the same shape.
    #[test]
    fn voxelizing_at_two_resolutions_agrees_within_the_coarser_spacing(s in arb_shape()) {
        let mesh = mesh_of(&s);
        prop_assume!(mesh.triangle_count() > 0);
        let bvh = Bvh::from_mesh(&mesh);

        let coarse = voxelize_bvh(&bvh, 10).unwrap();
        let fine = voxelize_bvh(&bvh, 20).unwrap();

        // Only inside the fine grid's box, which is the smaller of the two;
        // outside its own samples `trilinear` returns a conservative bound
        // rather than an estimate, and the two are not comparable.
        let (lo, hi) = sample_box(&fine);
        for p in probes(lo, hi, 240) {
            let a = coarse.sample(p);
            let b = fine.sample(p);
            prop_assert!(
                (a - b).abs() <= coarse.spacing,
                "at {p:?} the grids read {a} and {b}, further apart than the \
                 coarse spacing {}, for {s:?}",
                coarse.spacing
            );
        }
    }

    /// The padding precondition on [`sc_mesh::Grid`] holds for any mesh.
    ///
    /// The kernel's sampler treats it as given, and sphere tracing treats the
    /// sampler's answer as a safe step, so a grid that breaks it produces
    /// rays that pass through solid material rather than an obvious error.
    #[test]
    fn no_voxel_within_two_of_a_face_is_ever_inside(s in arb_shape()) {
        let mesh = mesh_of(&s);
        prop_assume!(mesh.triangle_count() > 0);
        let grid = voxelize_bvh(&Bvh::from_mesh(&mesh), 12).unwrap();

        let corner = voxel_value(&grid, 0, 0, 0);
        prop_assert!(corner > 0.0, "the corner voxel reads {corner} for {s:?}");
        for k in 0..grid.dims[2] {
            for j in 0..grid.dims[1] {
                for i in 0..grid.dims[0] {
                    let near_face = i < 2 || j < 2 || k < 2
                        || i + 2 >= grid.dims[0]
                        || j + 2 >= grid.dims[1]
                        || k + 2 >= grid.dims[2];
                    if !near_face {
                        continue;
                    }
                    let v = voxel_value(&grid, i, j, k);
                    prop_assert!(
                        v > 0.0,
                        "voxel ({i},{j},{k}) is within two of a face but reads {v}, \
                         against the corner's {corner}, for {s:?}"
                    );
                }
            }
        }
    }

    /// Whatever the model, writing it and reading it back gives the same
    /// triangles, in both STL variants.
    #[test]
    fn stl_round_trips_through_both_variants(s in arb_shape()) {
        let mesh = mesh_of(&s);
        prop_assume!(mesh.triangle_count() > 0);

        let binary = scratch("binary");
        stl::write(&mesh, &binary).unwrap();
        let from_binary = stl::read(&binary).unwrap();
        std::fs::remove_file(&binary).ok();
        prop_assert_eq!(faces(&from_binary), faces(&mesh), "binary differs for {:?}", s);

        let ascii = scratch("ascii");
        stl::write_ascii(&mesh, &ascii).unwrap();
        let from_ascii = stl::read(&ascii).unwrap();
        std::fs::remove_file(&ascii).ok();
        prop_assert_eq!(faces(&from_ascii), faces(&mesh), "ascii differs for {:?}", s);
    }
}
