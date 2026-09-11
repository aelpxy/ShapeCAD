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
use sc_mesh::{contour, Settings};

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
}

fn build(s: &Shape, b: &mut Builder) -> NodeId {
    match s {
        Shape::Sphere(r) => b.sphere(*r).unwrap(),
        Shape::Cuboid(h) => b.cuboid(Vec3::from_array(*h)).unwrap(),
        Shape::Cylinder(r, hh) => b.cylinder(*r, *hh).unwrap(),
        Shape::Extrude(sides, radius, height) => b
            .extrude(Builder::regular_polygon(*sides, *radius), *height)
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
            (inner, 1.0f32..3.0).prop_map(|(a, t)| Shape::Shell(Box::new(a), t)),
        ]
    })
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
    /// honest case is the cell diagonal — a surface that only clips one corner.
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
}
