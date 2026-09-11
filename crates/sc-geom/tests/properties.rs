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
use sc_geom::{bounds, eval, Arena, AssetId, Builder, Grid, Node, NodeId, Profile, Transform};
use std::sync::Arc;

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
    /// A closed profile swept a given depth along +Z.
    Extrude(Profile, f32),
    /// A half-space taken out of a shape.
    ///
    /// Only ever generated in this position, for the reason [`Shape::ThroughCut`]
    /// gives: a bare plane is unbounded, so the bounds properties would have
    /// nothing finite to check. Cutting with one is what the node is for, and
    /// the bounds then come from the solid being cut.
    SliceOff(Box<Shape>, [f32; 3], f32),
    /// An imported mesh: a radius, a voxel count per axis, and whether the
    /// voxelized shape is a cube rather than a sphere.
    Mesh(f32, u32, bool),
    Union(Box<Shape>, Box<Shape>, f32),
    Difference(Box<Shape>, Box<Shape>, f32),
    Intersection(Box<Shape>, Box<Shape>, f32),
    Translate(Box<Shape>, [f32; 3]),
    Rotate(Box<Shape>, [f32; 3]),
    Scale(Box<Shape>, f32),
    Offset(Box<Shape>, f32),
    Shell(Box<Shape>, f32),
    /// A feature placed on another and joined to it, its placement recording
    /// where it came from. That derivation is provenance, so every property
    /// here has to hold exactly as it would without it.
    On(Box<Shape>, Box<Shape>, [f32; 3]),
    /// A through cut: a shape with an unbounded prism taken out of it.
    ///
    /// Only ever generated in this position. A prism alone is unbounded along
    /// its sweep, so the bounds properties would have nothing finite to check
    /// and the mesher no region to work in. As the tool of a difference it is
    /// doing exactly the job it exists for, and the bounds come from the solid
    /// being cut.
    ThroughCut(Box<Shape>, Profile),
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
        Shape::Extrude(profile, depth) => b.extrude(profile.clone(), *depth),
        Shape::SliceOff(x, normal, offset) => {
            let solid = materialize(x, b)?;
            let tool = b.plane(Vec3::from_array(*normal), *offset)?;
            b.smooth_difference(solid, tool, 0.0)
        }
        Shape::Mesh(radius, dims, boxy) => {
            let (radius, dims) = (*radius, *dims);
            // Spacing is derived from the radius so that every generated grid
            // holds the padding precondition with three voxels to spare, which
            // is what makes the field outside it a lower bound on distance.
            let spacing = 2.0 * radius / (dims - 7) as f32;
            let half = (dims - 1) as f32 * spacing * 0.5;
            let field = |p: Vec3| {
                if *boxy {
                    let q = p.abs() - Vec3::splat(radius);
                    q.max(Vec3::ZERO).length() + q.max_element().min(0.0)
                } else {
                    p.length() - radius
                }
            };
            let grid = Grid::from_fn([dims; 3], Vec3::splat(-half), spacing, field);
            debug_assert!(
                grid.boundary_clearance() >= Grid::REQUIRED_CLEARANCE as f32 * spacing,
                "generated an unpadded grid: {grid:?}"
            );
            b.arena
                .insert(sc_geom::Node::mesh(AssetId(0), Arc::new(grid)))
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
        Shape::ThroughCut(x, profile) => {
            let solid = materialize(x, b)?;
            let tool = b.arena.insert(Node::Prism {
                profile: profile.clone(),
            })?;
            b.smooth_difference(solid, tool, 0.0)
        }
        Shape::On(x, base, t) => {
            let under = materialize(base, b)?;
            let child = materialize(x, b)?;
            let placed = b.arena.insert(sc_geom::Node::Transform {
                child,
                xform: Transform::from_translation(Vec3::from_array(*t)),
                on: Some(under),
            })?;
            b.union(under, placed)
        }
    }
}

/// Whether the tree contains an imported mesh anywhere.
fn contains_mesh(s: &Shape) -> bool {
    match s {
        Shape::Mesh(..) => true,
        Shape::Union(a, b, _) | Shape::Difference(a, b, _) | Shape::Intersection(a, b, _) => {
            contains_mesh(a) || contains_mesh(b)
        }
        Shape::On(a, base, _) => contains_mesh(a) || contains_mesh(base),
        Shape::Translate(a, _)
        | Shape::Rotate(a, _)
        | Shape::Scale(a, _)
        | Shape::Offset(a, _)
        | Shape::Shell(a, _)
        | Shape::SliceOff(a, ..)
        // The cutting prism is generated here, never from a mesh.
        | Shape::ThroughCut(a, _) => contains_mesh(a),
        // Matched out rather than caught by a wildcard. This function exists so
        // that a node cannot go missing from the shader quietly, and a wildcard
        // here is exactly how it would: a new shape holding a mesh would report
        // no mesh, and the property would pass by agreeing with itself.
        Shape::Sphere(..)
        | Shape::Cuboid(..)
        | Shape::Cylinder(..)
        | Shape::Torus(..)
        | Shape::Extrude(..) => false,
    }
}

fn build(s: &Shape) -> (Arena, NodeId) {
    let mut b = Builder::new();
    let id = materialize(s, &mut b).expect("generated shapes are valid by construction");
    (b.arena, id)
}

/// Every kind of closed region a swept feature can be built from.
///
/// All four, because the emitter has three different forms for them: a closed
/// rectangle, a closed circle, and a loop over a vertex buffer. Generating only
/// the regular polygon, as this did, left the two closed forms and the general
/// path unexercised by every property below.
///
/// A path is drawn as a star: one vertex per evenly spaced angle, at a radius
/// that varies. That keeps it simple and non-degenerate however it is shrunk,
/// while giving it the unequal edge lengths and reflex corners a regular polygon
/// never has.
fn arb_profile() -> impl Strategy<Value = Profile> {
    prop_oneof![
        (1.0f32..12.0, 1.0f32..12.0).prop_map(|(width, height)| Profile::Rect { width, height }),
        (0.5f32..6.0).prop_map(|radius| Profile::Circle { radius }),
        (3u32..10, 1.0f32..6.0)
            .prop_map(|(sides, radius)| Profile::RegularPolygon { sides, radius }),
        proptest::collection::vec(1.0f32..5.0, 3..12).prop_map(|radii| {
            let n = radii.len();
            let points = radii
                .into_iter()
                .enumerate()
                .map(|(i, r)| {
                    let a = i as f32 / n as f32 * std::f32::consts::TAU;
                    sc_geom::glam::Vec2::new(r * a.cos(), r * a.sin())
                })
                .collect();
            Profile::Path { points }
        }),
    ]
}

/// A unit normal, built from a direction rather than from three components so
/// that no draw can produce the near-zero vector a plane is invalid with.
fn arb_normal() -> impl Strategy<Value = [f32; 3]> {
    (0.0f32..std::f32::consts::TAU, -1.0f32..1.0).prop_map(|(theta, z)| {
        let r = (1.0 - z * z).max(0.0).sqrt();
        [r * theta.cos(), r * theta.sin(), z]
    })
}

/// Leaf primitives with parameters kept in a range where a 3D printer could
/// plausibly reproduce them.
///
/// `inexact` admits leaves whose field is not an exact distance, which today
/// means an imported mesh. Voxel counts are kept small: the properties build
/// thousands of trees and a grid is the only leaf whose cost is not constant.
fn arb_leaf(inexact: bool) -> impl Strategy<Value = Shape> {
    let exact = prop_oneof![
        (0.5f32..10.0).prop_map(Shape::Sphere),
        ((0.5f32..8.0, 0.5f32..8.0, 0.5f32..8.0), 0.0f32..2.0)
            .prop_map(|((x, y, z), r)| Shape::Cuboid([x, y, z], r)),
        (0.5f32..8.0, 0.5f32..8.0, 0.0f32..1.0).prop_map(|(r, h, o)| Shape::Cylinder(r, h, o)),
        (1.0f32..8.0, 0.2f32..3.0).prop_map(|(a, b)| Shape::Torus(a, b)),
        (arb_profile(), 1.0f32..8.0).prop_map(|(profile, depth)| Shape::Extrude(profile, depth)),
    ];
    if !inexact {
        return exact.boxed();
    }
    prop_oneof![
        5 => exact,
        1 => (1.0f32..6.0, 9u32..15, proptest::bool::ANY)
            .prop_map(|(r, n, boxy)| Shape::Mesh(r, n, boxy)),
    ]
    .boxed()
}

/// Random trees. `inexact` controls whether the tree may contain anything whose
/// field is not an exact signed distance, which is both a non-zero blend radius
/// and an imported mesh. Both are excluded from the strict Lipschitz property:
/// a polynomial smooth-minimum deliberately under-reports, and trilinear
/// interpolation of a sampled field has a gradient of up to `sqrt(3)` at a kink.
/// See `known_limitation_trilinear_sampling_is_not_lipschitz_at_a_kink`.
fn arb_shape(inexact: bool) -> impl Strategy<Value = Shape> {
    let blend: BoxedStrategy<f32> = if inexact {
        (0.0f32..2.0).boxed()
    } else {
        Just(0.0f32).boxed()
    };
    arb_leaf(inexact).prop_recursive(4, 24, 2, move |inner| {
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
            (inner.clone(), 0.2f32..2.0).prop_map(|(a, t)| Shape::Shell(Box::new(a), t)),
            (
                inner.clone(),
                inner.clone(),
                (-8.0f32..8.0, -8.0f32..8.0, -8.0f32..8.0)
            )
                .prop_map(|(a, base, t)| Shape::On(
                    Box::new(a),
                    Box::new(base),
                    [t.0, t.1, t.2]
                )),
            (inner.clone(), arb_normal(), -10.0f32..10.0)
                .prop_map(|(a, n, offset)| Shape::SliceOff(Box::new(a), n, offset)),
            (inner, arb_profile()).prop_map(|(a, profile)| Shape::ThroughCut(Box::new(a), profile)),
        ]
    })
}

/// Whether an edit moves a value onto or off one of the emitter's peepholes.
///
/// Only two numbers can: a profile's side count is the shader's loop bound, and
/// a scale of exactly one is the multiply the emitter drops. Every other
/// peephole has its threshold at zero, and scaling a zero leaves it a zero.
fn is_structural(name: &str, before: f32, after: f32) -> bool {
    name == "sides"
        || (name == "scale" && ((before - 1.0).abs() < 1e-9 || (after - 1.0).abs() < 1e-9))
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
        a in arb_leaf(true), b in arb_leaf(true), k in 0.1f32..3.0, pts in arb_points()
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
    fn difference_never_adds_material(a in arb_leaf(true), b in arb_leaf(true), pts in arb_points()) {
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
        s in arb_leaf(true), scale in 0.3f32..3.0, t in (-5.0f32..5.0, -5.0f32..5.0, -5.0f32..5.0),
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
    fn shell_is_a_subset_of_its_child(s in arb_leaf(true), t in 0.2f32..3.0, pts in arb_points()) {
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
        // A mesh has no shader form yet, and the one thing that must not happen
        // is that it goes missing without saying so.
        prop_assert_eq!(
            generated.is_complete(),
            !contains_mesh(&s),
            "unsupported nodes misreported for {:?}",
            s
        );
        let module = naga::front::wgsl::parse_str(src)
            .unwrap_or_else(|e| panic!("WGSL parse failed: {e:?}\n{src}"));
        let mut v = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        prop_assert!(v.validate(&module).is_ok(), "WGSL validation failed for {s:?}");

        // Nothing else may have been quietly emitted as empty space. The
        // emitter names that constant rather than writing a bare literal so
        // that it can be counted: one occurrence for the always-empty selection
        // field, and one for each node reported above. A node kind added to the
        // arena but forgotten in `wgsl.rs` falls through to empty space, and
        // this is what says so.
        prop_assert_eq!(
            src.matches("= SC_EMPTY;").count(),
            1 + generated.unsupported.len(),
            "a node went missing from the shader without being reported: {:?}",
            s
        );
    }

    /// Editing a number must be an upload, never a recompile.
    ///
    /// The two hand-written examples in `lib.rs` check a radius and a sketch
    /// point. This checks every parameter of every node of thousands of trees,
    /// which is the only way to notice that some new node kind bakes one of its
    /// values into the source and drops a frame every time it is dragged.
    #[test]
    fn a_value_edit_never_rebuilds_the_shader(s in arb_shape(true)) {
        let (mut arena, root) = build(&s);
        let before = sc_geom::wgsl::generate(&arena, Some(root)).source;

        for id in arena.live_ids().collect::<Vec<_>>() {
            let original = arena.get(id).expect("live").clone();
            for (name, value) in original.params() {
                let edited_value = value * 1.5;
                if is_structural(name, value, edited_value) {
                    continue;
                }
                let mut edited = original.clone();
                // A derived placement refuses edits outright, and an edit that
                // would make the node invalid is not a value edit at all.
                if !edited.set_param(name, edited_value) || arena.replace(id, edited).is_err() {
                    continue;
                }
                let after = sc_geom::wgsl::generate(&arena, Some(root)).source;
                arena.replace(id, original.clone()).expect("it was valid a moment ago");
                prop_assert_eq!(
                    after,
                    before.clone(),
                    "editing {} on {} rebuilt the shader in {:?}",
                    name,
                    id,
                    s
                );
            }
        }
    }
}
