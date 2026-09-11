//! Property-based tests for the implicit kernel.
//!
//! The unit tests in `lib.rs` pin down cases that were thought of in advance.
//! These pin down the ones that were not: invariants that must hold for *every*
//! well-formed tree, checked against thousands of randomly generated models.
//!
//! The most important of these is [`field_is_lipschitz`]. Sphere tracing is only
//! correct if the field never grows faster than distance itself; a primitive
//! that violates it produces a renderer that quietly steps through surfaces.

// Geometry code names points and distances `p`, `d`, `a`, `b` by long convention;
// spelling them out in tests hurts readability more than it helps.
#![allow(clippy::many_single_char_names)]

use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use sc_geom::glam::{Quat, Vec3};
use sc_geom::{bounds, eval, Arena, Builder, NodeId, Transform};

/// A symbolic shape, generated first and then materialised into an [`Arena`].
///
/// Generating a tree rather than a flat command list keeps every produced model
/// valid by construction, so the properties test real geometry rather than
/// spending their budget on rejected inputs.
#[derive(Clone, Debug)]
enum Shape {
    Sphere(f32),
    Cuboid([f32; 3], f32),
    Cylinder(f32, f32, f32),
    Torus(f32, f32),
    Extrude(usize, f32, f32),
    Union(Box<Shape>, Box<Shape>, f32),
    Difference(Box<Shape>, Box<Shape>, f32),
    Intersection(Box<Shape>, Box<Shape>, f32),
    Translate(Box<Shape>, [f32; 3]),
    Rotate(Box<Shape>, [f32; 3]),
    Scale(Box<Shape>, f32),
    Offset(Box<Shape>, f32),
    Shell(Box<Shape>, f32),
}

fn materialize(s: &Shape, b: &mut Builder) -> sc_geom::Result<NodeId> {
    match s {
        Shape::Sphere(r) => b.sphere(*r),
        Shape::Cuboid(h, round) => {
            let half = Vec3::from_array(*h);
            b.rounded_cuboid(half, round.min(half.min_element() * 0.99))
        }
        Shape::Cylinder(r, hh, round) => {
            let node = sc_geom::Node::Cylinder {
                radius: *r,
                half_height: *hh,
                round: round.min(r.min(*hh) * 0.99),
            };
            b.arena.insert(node)
        }
        Shape::Torus(major, minor) => b.torus(*major, minor.min(major * 0.9)),
        Shape::Extrude(sides, radius, height) => {
            b.extrude(Builder::regular_polygon(*sides, *radius), *height)
        }
        Shape::Union(x, y, k) => {
            let (a, c) = (materialize(x, b)?, materialize(y, b)?);
            b.smooth_union(a, c, *k)
        }
        Shape::Difference(x, y, k) => {
            let (a, c) = (materialize(x, b)?, materialize(y, b)?);
            b.smooth_difference(a, c, *k)
        }
        Shape::Intersection(x, y, k) => {
            let (a, c) = (materialize(x, b)?, materialize(y, b)?);
            b.arena.insert(sc_geom::Node::Intersection {
                a,
                b: c,
                smooth: *k,
            })
        }
        Shape::Translate(x, t) => {
            let a = materialize(x, b)?;
            b.translate(a, Vec3::from_array(*t))
        }
        Shape::Rotate(x, e) => {
            let a = materialize(x, b)?;
            b.rotate(
                a,
                Quat::from_euler(sc_geom::glam::EulerRot::XYZ, e[0], e[1], e[2]),
            )
        }
        Shape::Scale(x, s) => {
            let a = materialize(x, b)?;
            b.transform(a, Transform::from_scale(*s))
        }
        Shape::Offset(x, d) => {
            let a = materialize(x, b)?;
            b.offset(a, *d)
        }
        Shape::Shell(x, t) => {
            let a = materialize(x, b)?;
            b.shell(a, *t)
        }
    }
}

fn build(s: &Shape) -> (Arena, NodeId) {
    let mut b = Builder::new();
    let id = materialize(s, &mut b).expect("generated shapes are valid by construction");
    (b.arena, id)
}

/// Leaf primitives with parameters kept in a range where a 3D printer could
/// plausibly reproduce them.
fn arb_leaf() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (0.5f32..10.0).prop_map(Shape::Sphere),
        ((0.5f32..8.0, 0.5f32..8.0, 0.5f32..8.0), 0.0f32..2.0)
            .prop_map(|((x, y, z), r)| Shape::Cuboid([x, y, z], r)),
        (0.5f32..8.0, 0.5f32..8.0, 0.0f32..1.0).prop_map(|(r, h, o)| Shape::Cylinder(r, h, o)),
        (1.0f32..8.0, 0.2f32..3.0).prop_map(|(a, b)| Shape::Torus(a, b)),
        ((3usize..10), 1.0f32..6.0, 1.0f32..8.0).prop_map(|(n, r, h)| Shape::Extrude(n, r, h)),
    ]
}

/// Random trees. `smooth` controls whether blend radii may be non-zero, because
/// a polynomial smooth-minimum is deliberately not an exact distance field and
/// so is excluded from the strict Lipschitz property.
fn arb_shape(smooth: bool) -> impl Strategy<Value = Shape> {
    let blend: BoxedStrategy<f32> = if smooth {
        (0.0f32..2.0).boxed()
    } else {
        Just(0.0f32).boxed()
    };
    arb_leaf().prop_recursive(4, 24, 2, move |inner| {
        let blend = blend.clone();
        prop_oneof![
            (inner.clone(), inner.clone(), blend.clone()).prop_map(|(a, b, k)| Shape::Union(
                Box::new(a),
                Box::new(b),
                k
            )),
            (inner.clone(), inner.clone(), blend.clone()).prop_map(|(a, b, k)| Shape::Difference(
                Box::new(a),
                Box::new(b),
                k
            )),
            (inner.clone(), inner.clone(), blend).prop_map(|(a, b, k)| Shape::Intersection(
                Box::new(a),
                Box::new(b),
                k
            )),
            (inner.clone(), (-8.0f32..8.0, -8.0f32..8.0, -8.0f32..8.0))
                .prop_map(|(a, t)| Shape::Translate(Box::new(a), [t.0, t.1, t.2])),
            (inner.clone(), (-3.0f32..3.0, -3.0f32..3.0, -3.0f32..3.0))
                .prop_map(|(a, e)| Shape::Rotate(Box::new(a), [e.0, e.1, e.2])),
            (inner.clone(), 0.3f32..3.0).prop_map(|(a, s)| Shape::Scale(Box::new(a), s)),
            (inner.clone(), -1.0f32..2.0).prop_map(|(a, d)| Shape::Offset(Box::new(a), d)),
            (inner, 0.2f32..2.0).prop_map(|(a, t)| Shape::Shell(Box::new(a), t)),
        ]
    })
}

fn arb_points() -> impl Strategy<Value = Vec<[f32; 3]>> {
    proptest::collection::vec((-20.0f32..20.0, -20.0f32..20.0, -20.0f32..20.0), 8..40)
        .prop_map(|v| v.into_iter().map(|(x, y, z)| [x, y, z]).collect())
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/proptest-regressions/properties.txt",
        ))),
        ..ProptestConfig::default()
    })]

    /// The field must never be NaN. A single NaN poisons every downstream
    /// `min`/`max`, so a whole model can vanish from one bad evaluation.
    #[test]
    fn field_is_never_nan(s in arb_shape(true), pts in arb_points()) {
        let (arena, root) = build(&s);
        for p in pts {
            let d = eval(&arena, root, Vec3::from_array(p));
            prop_assert!(!d.is_nan(), "NaN at {p:?} in {s:?}");
        }
    }

    /// The defining property of a signed distance field: moving a distance `t`
    /// can change the reported distance by at most `t`.
    ///
    /// Sphere tracing steps by exactly the reported distance, so any violation
    /// means the renderer can step straight through a surface. Restricted to
    /// hard booleans, since polynomial blends intentionally under-report.
    #[test]
    fn field_is_lipschitz(s in arb_shape(false), pts in arb_points()) {
        let (arena, root) = build(&s);
        for w in pts.windows(2) {
            let (p, q) = (Vec3::from_array(w[0]), Vec3::from_array(w[1]));
            let (dp, dq) = (eval(&arena, root, p), eval(&arena, root, q));
            if !dp.is_finite() || !dq.is_finite() {
                continue;
            }
            let moved = (p - q).length();
            let changed = (dp - dq).abs();
            // Tolerance absorbs f32 cancellation on large coordinates.
            prop_assert!(
                changed <= moved * 1.001 + 1e-3,
                "Lipschitz violated: moved {moved} but distance changed {changed} in {s:?}"
            );
        }
    }

    /// Bounds may over-report but must never exclude solid material, or the
    /// mesher will silently clip geometry out of the exported part.
    #[test]
    fn bounds_never_exclude_solid_material(s in arb_shape(true), pts in arb_points()) {
        let (arena, root) = build(&s);
        let bb = bounds(&arena, root);
        for p in pts {
            let v = Vec3::from_array(p);
            if eval(&arena, root, v) < -1e-3 {
                prop_assert!(
                    v.cmpge(bb.min).all() && v.cmple(bb.max).all(),
                    "solid point {v:?} lies outside bounds {bb:?} of {s:?}"
                );
            }
        }
    }

    /// A blend adds material; it never removes any.
    #[test]
    fn smooth_union_never_removes_material(
        a in arb_leaf(), b in arb_leaf(), k in 0.1f32..3.0, pts in arb_points()
    ) {
        let hard = Shape::Union(Box::new(a.clone()), Box::new(b.clone()), 0.0);
        let soft = Shape::Union(Box::new(a), Box::new(b), k);
        let (ha, hr) = build(&hard);
        let (sa, sr) = build(&soft);
        for p in pts {
            let v = Vec3::from_array(p);
            prop_assert!(eval(&sa, sr, v) <= eval(&ha, hr, v) + 1e-4);
        }
    }

    /// Cutting can only ever remove material.
    #[test]
    fn difference_never_adds_material(a in arb_leaf(), b in arb_leaf(), pts in arb_points()) {
        let (aa, ar) = build(&a);
        let diff = Shape::Difference(Box::new(a), Box::new(b), 0.0);
        let (da, dr) = build(&diff);
        for p in pts {
            let v = Vec3::from_array(p);
            prop_assert!(eval(&da, dr, v) >= eval(&aa, ar, v) - 1e-4);
        }
    }

    /// A transform must scale distances exactly, or the field stops being metric
    /// and every downstream offset and blend is subtly wrong.
    #[test]
    fn transform_preserves_the_metric(
        s in arb_leaf(), scale in 0.3f32..3.0, t in (-5.0f32..5.0, -5.0f32..5.0, -5.0f32..5.0),
        pts in arb_points()
    ) {
        let (base_arena, base) = build(&s);
        let mut b = Builder::new();
        let child = materialize(&s, &mut b).unwrap();
        let xform = Transform {
            translation: Vec3::new(t.0, t.1, t.2),
            rotation: Quat::from_rotation_y(0.7),
            scale,
        };
        let placed = b.transform(child, xform).unwrap();

        for p in pts {
            let local = Vec3::from_array(p);
            let world = xform.apply_point(local);
            let expected = eval(&base_arena, base, local) * scale;
            let actual = eval(&b.arena, placed, world);
            prop_assert!(
                (expected - actual).abs() <= 1e-2 * expected.abs().max(1.0),
                "expected {expected}, got {actual} for {s:?}"
            );
        }
    }

    /// An inward shell is always a subset of the solid it hollows.
    #[test]
    fn shell_is_a_subset_of_its_child(s in arb_leaf(), t in 0.2f32..3.0, pts in arb_points()) {
        let (ca, cr) = build(&s);
        let shelled = Shape::Shell(Box::new(s), t);
        let (sa, sr) = build(&shelled);
        for p in pts {
            let v = Vec3::from_array(p);
            prop_assert!(eval(&sa, sr, v) >= eval(&ca, cr, v) - 1e-4);
        }
    }

    /// Generated shaders must compile for every tree the kernel can represent,
    /// not just the ones in the hand-written examples.
    #[test]
    fn every_tree_generates_valid_wgsl(s in arb_shape(true)) {
        let (arena, root) = build(&s);
        let generated = sc_geom::wgsl::generate(&arena, Some(root));
        let src = &generated.source;
        // No model value appears in the source, so a bad number cannot reach the
        // shader as a literal. It has to be finite in the buffer instead.
        prop_assert!(
            generated.params.iter().all(|v| v.is_finite()),
            "non-finite parameter for {s:?}"
        );
        let module = naga::front::wgsl::parse_str(src)
            .unwrap_or_else(|e| panic!("WGSL parse failed: {e:?}\n{src}"));
        let mut v = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        prop_assert!(v.validate(&module).is_ok(), "WGSL validation failed for {s:?}");
    }
}
