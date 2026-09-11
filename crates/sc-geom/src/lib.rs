//! `ShapeCAD` implicit geometry kernel.
//!
//! Geometry is represented as a DAG of implicit (signed-distance) operations
//! rather than a boundary representation. The consequences that matter:
//!
//! - Booleans are `min`/`max`. They cannot fail.
//! - Fillets are a blend radius on a boolean, not an operation on topology.
//! - Hollowing, clearance offsets and lattices are arithmetic on the field.
//! - Exported meshes are watertight by construction.
//! - Nodes have stable ids, so there is no topological naming problem.
//!
//! The trade is that exact NURBS surfaces and B-rep interchange are out of
//! scope. `ShapeCAD` targets 3D printing, where neither is needed.

pub mod arena;
pub mod bounds;
pub mod error;
pub mod eval;
pub mod hash;
pub mod math;
pub mod node;
pub mod ops;
pub mod pick;
pub mod profile;
pub mod wgsl;

pub use arena::Arena;
pub use bounds::{bounds, Aabb};
pub use error::{GeomError, Result};
pub use eval::{eval, normal, smax, smin};
pub use hash::{geometry_hash, GeometryHash};
pub use math::Transform;
pub use node::{Node, NodeId};
pub use ops::Builder;
pub use pick::{pick, Hit};
pub use profile::Profile;

pub use glam;

#[cfg(test)]
#[allow(clippy::many_single_char_names, clippy::similar_names)]
mod tests {
    use super::*;
    use glam::Vec3;

    const EPS: f32 = 1e-4;

    fn assert_near(a: f32, b: f32, ctx: &str) {
        assert!((a - b).abs() < EPS, "{ctx}: expected {b}, got {a}");
    }

    #[test]
    fn sphere_field_is_exact_distance() {
        let mut b = Builder::new();
        let s = b.sphere(2.0).unwrap();
        assert_near(eval(&b.arena, s, Vec3::ZERO), -2.0, "centre");
        assert_near(eval(&b.arena, s, Vec3::new(2.0, 0.0, 0.0)), 0.0, "surface");
        assert_near(eval(&b.arena, s, Vec3::new(5.0, 0.0, 0.0)), 3.0, "outside");
    }

    #[test]
    fn box_field_is_exact_outside_and_on_faces() {
        let mut b = Builder::new();
        let c = b.cube(1.0).unwrap();
        assert_near(eval(&b.arena, c, Vec3::new(1.0, 0.0, 0.0)), 0.0, "face");
        assert_near(
            eval(&b.arena, c, Vec3::new(3.0, 0.0, 0.0)),
            2.0,
            "outside face",
        );
        // Diagonal corner distance is exact for an SDF box.
        let d = eval(&b.arena, c, Vec3::splat(2.0));
        assert_near(d, (Vec3::splat(1.0)).length(), "corner");
    }

    #[test]
    fn difference_removes_material() {
        let mut b = Builder::new();
        let outer = b.cube(2.0).unwrap();
        let hole = b.cylinder(0.5, 5.0).unwrap();
        let part = b.difference(outer, hole).unwrap();
        // On the axis we are inside the cylinder, so it has been cut away.
        assert!(
            eval(&b.arena, part, Vec3::ZERO) > 0.0,
            "axis should be void"
        );
        // Off-axis, still solid.
        assert!(
            eval(&b.arena, part, Vec3::new(1.5, 1.5, 0.0)) < 0.0,
            "corner should be solid"
        );
    }

    #[test]
    fn smooth_union_only_adds_material() {
        let mut b = Builder::new();
        let s1 = b.sphere(1.0).unwrap();
        let s2 = b.sphere(1.0).unwrap();
        let moved = b.translate(s2, Vec3::new(1.5, 0.0, 0.0)).unwrap();
        let hard = b.union(s1, moved).unwrap();
        let soft = b.smooth_union(s1, moved, 0.5).unwrap();
        for i in 0..40 {
            let p = Vec3::new(i as f32 * 0.1 - 1.0, 0.35, 0.0);
            let (h, s) = (eval(&b.arena, hard, p), eval(&b.arena, soft, p));
            assert!(s <= h + EPS, "fillet removed material at {p:?}: {s} > {h}");
        }
    }

    #[test]
    fn transform_preserves_the_metric() {
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        let t = b
            .transform(
                s,
                Transform {
                    translation: Vec3::new(3.0, 0.0, 0.0),
                    rotation: glam::Quat::from_rotation_z(0.7),
                    scale: 2.0,
                },
            )
            .unwrap();
        // Scaled sphere of radius 2 centred at (3,0,0).
        assert_near(eval(&b.arena, t, Vec3::new(3.0, 0.0, 0.0)), -2.0, "centre");
        assert_near(eval(&b.arena, t, Vec3::new(5.0, 0.0, 0.0)), 0.0, "surface");
        assert_near(eval(&b.arena, t, Vec3::new(8.0, 0.0, 0.0)), 3.0, "outside");
    }

    #[test]
    fn shell_hollows_inward_and_keeps_the_outer_surface() {
        let mut b = Builder::new();
        let c = b.cube(10.0).unwrap();
        let hollow = b.shell(c, 2.0).unwrap();
        assert_near(
            eval(&b.arena, hollow, Vec3::new(10.0, 0.0, 0.0)),
            0.0,
            "outer surface kept",
        );
        assert!(
            eval(&b.arena, hollow, Vec3::new(9.0, 0.0, 0.0)) < 0.0,
            "wall is solid"
        );
        assert!(eval(&b.arena, hollow, Vec3::ZERO) > 0.0, "interior is void");
    }

    #[test]
    fn an_extruded_square_has_exact_distances() {
        let mut b = Builder::new();
        let square = vec![
            glam::Vec2::new(-5.0, -5.0),
            glam::Vec2::new(5.0, -5.0),
            glam::Vec2::new(5.0, 5.0),
            glam::Vec2::new(-5.0, 5.0),
        ];
        let e = b.extrude(Profile::Path { points: square }, 10.0).unwrap();

        // Sits on its own plane rather than straddling it.
        assert_near(eval(&b.arena, e, Vec3::new(0.0, 0.0, 0.0)), 0.0, "base");
        assert_near(eval(&b.arena, e, Vec3::new(0.0, 0.0, 10.0)), 0.0, "top");
        assert_near(eval(&b.arena, e, Vec3::new(0.0, 0.0, 5.0)), -5.0, "centre");
        assert_near(
            eval(&b.arena, e, Vec3::new(8.0, 0.0, 5.0)),
            3.0,
            "beside a face",
        );
        assert_near(
            eval(&b.arena, e, Vec3::new(0.0, 0.0, 13.0)),
            3.0,
            "above the top",
        );
        // Exact diagonal distance from a corner proves it is a distance field
        // and not merely a bound.
        assert_near(
            eval(&b.arena, e, Vec3::new(9.0, 9.0, 5.0)),
            glam::Vec2::new(4.0, 4.0).length(),
            "past a corner",
        );
    }

    #[test]
    fn profile_winding_does_not_matter() {
        // Inside-ness comes from a crossing count, so a profile drawn clockwise
        // behaves exactly like the same one drawn anticlockwise.
        let ccw = vec![
            glam::Vec2::new(-4.0, -4.0),
            glam::Vec2::new(4.0, -4.0),
            glam::Vec2::new(4.0, 4.0),
            glam::Vec2::new(-4.0, 4.0),
        ];
        let cw: Vec<_> = ccw.iter().rev().copied().collect();

        let mut a = Builder::new();
        let ea = a.extrude(Profile::Path { points: ccw }, 6.0).unwrap();
        let mut c = Builder::new();
        let ec = c.extrude(Profile::Path { points: cw }, 6.0).unwrap();

        for p in [
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::new(6.0, 0.0, 3.0),
            Vec3::new(0.0, 0.0, 9.0),
        ] {
            assert_near(
                eval(&c.arena, ec, p),
                eval(&a.arena, ea, p),
                "winding changed the field",
            );
        }
    }

    #[test]
    fn degenerate_profiles_are_rejected() {
        let mut b = Builder::new();
        assert!(
            b.extrude(
                Profile::Path {
                    points: vec![glam::Vec2::ZERO, glam::Vec2::X]
                },
                5.0
            )
            .is_err(),
            "two points"
        );
        let collinear = vec![
            glam::Vec2::new(0.0, 0.0),
            glam::Vec2::new(1.0, 0.0),
            glam::Vec2::new(2.0, 0.0),
        ];
        assert!(
            b.extrude(Profile::Path { points: collinear }, 5.0).is_err(),
            "zero area"
        );
        let square = Profile::RegularPolygon {
            sides: 4,
            radius: 5.0,
        };
        assert!(b.extrude(square, -1.0).is_err(), "negative depth");
    }

    #[test]
    fn bounds_never_under_report() {
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        let c = b.cube(0.8).unwrap();
        let moved = b.translate(c, Vec3::new(1.0, 0.5, 0.0)).unwrap();
        let m = b.smooth_union(s, moved, 0.4).unwrap();
        let bb = bounds(&b.arena, m);
        for i in 0..30 {
            for j in 0..30 {
                for k in 0..30 {
                    let p = Vec3::new(i as f32, j as f32, k as f32) / 10.0 * 6.0 - Vec3::splat(3.0);
                    if eval(&b.arena, m, p) < 0.0 {
                        assert!(
                            p.cmpge(bb.min).all() && p.cmple(bb.max).all(),
                            "solid point {p:?} outside reported bounds {bb:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ids_are_stable_across_parameter_edits() {
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        let before = hash::structure_hash(&b.arena, s);
        b.arena.replace(s, Node::Sphere { radius: 2.0 }).unwrap();
        assert!(b.arena.is_alive(s), "id survives the edit");
        assert_ne!(
            before,
            hash::structure_hash(&b.arena, s),
            "shape hash moved"
        );
    }

    #[test]
    fn cycles_are_rejected() {
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        let u = b.offset(s, 0.1).unwrap();
        let err = b
            .arena
            .replace(
                s,
                Node::Offset {
                    child: u,
                    distance: 0.1,
                },
            )
            .unwrap_err();
        assert!(matches!(err, GeomError::Cycle { .. }), "got {err:?}");
    }

    #[test]
    fn referenced_nodes_cannot_be_deleted() {
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        let _o = b.offset(s, 0.1).unwrap();
        assert!(matches!(
            b.arena.remove(s).unwrap_err(),
            GeomError::StillReferenced { .. }
        ));
    }

    #[test]
    fn invalid_parameters_are_rejected_before_mutation() {
        let mut b = Builder::new();
        assert!(b.sphere(-1.0).is_err(), "negative radius");
        assert!(b.sphere(f32::NAN).is_err(), "nan radius");
        assert_eq!(
            b.arena.capacity(),
            0,
            "rejected inserts must not consume an id"
        );
    }

    #[test]
    fn structure_hash_ignores_id_numbering() {
        let mut a = Builder::new();
        let s = a.sphere(1.0).unwrap();
        let ha = hash::structure_hash(&a.arena, s);

        let mut b = Builder::new();
        let _decoy = b.cube(5.0).unwrap();
        let s2 = b.sphere(1.0).unwrap();
        assert_ne!(s, s2, "different ids");
        assert_eq!(
            ha,
            hash::structure_hash(&b.arena, s2),
            "same shape, same hash"
        );
    }

    #[test]
    fn geometry_hash_detects_a_sub_millimetre_change() {
        let mut b = Builder::new();
        let s = b.sphere(10.0).unwrap();
        let h1 = geometry_hash(&b.arena, s);
        b.arena.replace(s, Node::Sphere { radius: 10.01 }).unwrap();
        let h2 = geometry_hash(&b.arena, s);
        assert_ne!(h1, h2, "0.01mm change went unnoticed");
    }

    #[test]
    fn wgsl_generation_covers_every_node_kind() {
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        let c = b.rounded_cuboid(Vec3::splat(1.0), 0.2).unwrap();
        let cy = b.cylinder(0.5, 2.0).unwrap();
        let t = b.torus(2.0, 0.3).unwrap();
        let pl = b.plane(Vec3::Z, 0.0).unwrap();
        let u = b.smooth_union(s, c, 0.1).unwrap();
        let d = b.difference(u, cy).unwrap();
        let i = b.intersection(d, t).unwrap();
        let tr = b.translate(i, Vec3::X).unwrap();
        let of = b.offset(tr, 0.05).unwrap();
        let sh = b.shell(of, 0.4).unwrap();
        let ex = b
            .extrude(
                Profile::RegularPolygon {
                    sides: 6,
                    radius: 1.5,
                },
                2.0,
            )
            .unwrap();
        let joined = b.union(sh, ex).unwrap();
        let root = b.union(joined, pl).unwrap();

        let generated = wgsl::generate(&b.arena, Some(root));
        assert!(generated.source.contains("fn sc_sdf(p: vec3<f32>) -> f32"));
        // Every model value lives in the buffer, so the source carries no
        // numeric literal that could be a stray NaN or infinity.
        assert!(!generated.params.is_empty(), "no parameters were bound");
        assert!(
            generated.params.iter().all(|v| v.is_finite()),
            "non-finite parameter in {:?}",
            generated.params
        );
    }

    #[test]
    fn changing_a_value_does_not_change_the_shader() {
        // The whole point of binding values to a buffer: a parameter edit must
        // be an upload, never a recompile.
        let mut b = Builder::new();
        let s = b.sphere(5.0).unwrap();
        let before = wgsl::generate(&b.arena, Some(s));

        b.arena.replace(s, Node::Sphere { radius: 9.0 }).unwrap();
        let after = wgsl::generate(&b.arena, Some(s));

        assert_eq!(
            before.source, after.source,
            "a radius edit rebuilt the shader"
        );
        assert_ne!(
            before.params, after.params,
            "the new radius never reached the buffer"
        );
        assert!(after.params.contains(&9.0), "{:?}", after.params);
    }

    #[test]
    fn moving_a_sketch_point_does_not_change_the_shader() {
        let square = |x: f32| {
            vec![
                glam::Vec2::new(-5.0, -5.0),
                glam::Vec2::new(x, -5.0),
                glam::Vec2::new(x, 5.0),
                glam::Vec2::new(-5.0, 5.0),
            ]
        };
        let mut b = Builder::new();
        let e = b
            .extrude(
                Profile::Path {
                    points: square(5.0),
                },
                10.0,
            )
            .unwrap();
        let before = wgsl::generate(&b.arena, Some(e));

        b.arena
            .replace(
                e,
                Node::Extrude {
                    profile: Profile::Path {
                        points: square(8.0),
                    },
                    depth: 10.0,
                },
            )
            .unwrap();
        let after = wgsl::generate(&b.arena, Some(e));

        assert_eq!(
            before.source, after.source,
            "dragging a point rebuilt the shader"
        );
        assert!(after.params.contains(&8.0));
    }

    #[test]
    fn changing_the_shape_of_the_model_does_change_the_shader() {
        let mut b = Builder::new();
        let s = b.sphere(5.0).unwrap();
        let before = wgsl::generate(&b.arena, Some(s));

        let c = b.cube(2.0).unwrap();
        let u = b.union(s, c).unwrap();
        let after = wgsl::generate(&b.arena, Some(u));
        assert_ne!(before.source, after.source, "adding a node must rebuild");
    }

    #[test]
    fn crossing_a_peephole_threshold_rebuilds() {
        // An identity scale emits no multiply at all. Moving off 1.0 has to
        // bring that arithmetic back, which is a source change, not a value one.
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        let t = b.transform(s, Transform::from_scale(1.0)).unwrap();
        let identity = wgsl::generate(&b.arena, Some(t));

        b.arena
            .replace(
                t,
                Node::Transform {
                    child: s,
                    xform: Transform::from_scale(2.0),
                },
            )
            .unwrap();
        let scaled = wgsl::generate(&b.arena, Some(t));

        assert_ne!(
            identity.source, scaled.source,
            "the dropped multiply never came back"
        );
    }

    #[test]
    fn wgsl_handles_an_empty_document() {
        let arena = Arena::new();
        let generated = wgsl::generate(&arena, None);
        assert!(generated.source.contains("fn sc_sdf"));
        validate_wgsl(&generated.source);
    }

    /// Parse and type-check generated WGSL the same way wgpu will at runtime.
    ///
    /// Without this the codegen is only ever checked by string matching, which
    /// would let a malformed shader reach the GPU and fail as an opaque driver
    /// error far from its cause.
    fn validate_wgsl(src: &str) {
        let module = match naga::front::wgsl::parse_str(src) {
            Ok(m) => m,
            Err(e) => panic!("generated WGSL does not parse: {e:?}\n{src}"),
        };
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        if let Err(e) = validator.validate(&module) {
            panic!("generated WGSL does not validate: {e:?}\n{src}");
        }
    }

    /// The model from `wgsl_generation_covers_every_node_kind`, reused so shader
    /// validation sees every node variant.
    fn kitchen_sink() -> (Builder, NodeId) {
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        let c = b.rounded_cuboid(Vec3::splat(1.0), 0.2).unwrap();
        let cy = b.cylinder(0.5, 2.0).unwrap();
        let t = b.torus(2.0, 0.3).unwrap();
        let pl = b.plane(Vec3::Z, 0.0).unwrap();
        let u = b.smooth_union(s, c, 0.1).unwrap();
        let d = b.difference(u, cy).unwrap();
        let i = b.intersection(d, t).unwrap();
        let rot = b
            .transform(
                i,
                Transform {
                    translation: Vec3::new(1.0, 2.0, 3.0),
                    rotation: glam::Quat::from_rotation_y(0.6),
                    scale: 1.5,
                },
            )
            .unwrap();
        let of = b.offset(rot, 0.05).unwrap();
        let sh = b.shell(of, 0.4).unwrap();
        let ex = b
            .extrude(
                Profile::RegularPolygon {
                    sides: 5,
                    radius: 1.2,
                },
                3.0,
            )
            .unwrap();
        let joined = b.smooth_union(sh, ex, 0.2).unwrap();
        let root = b.smooth_union(joined, pl, 0.25).unwrap();
        (b, root)
    }

    #[test]
    fn generated_wgsl_compiles() {
        let (b, root) = kitchen_sink();
        validate_wgsl(&wgsl::generate(&b.arena, Some(root)).source);
    }

    #[test]
    fn identity_transforms_emit_no_dead_arithmetic() {
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        // A pure translation: no rotation, no scale, no rounding.
        let t = b.translate(s, Vec3::new(5.0, 0.0, 0.0)).unwrap();
        let generated = wgsl::generate(&b.arena, Some(t));
        let src = &generated.source;

        assert!(!src.contains("* 1.0"), "identity scale survived:\n{src}");
        assert!(
            !src.split("fn sc_sdf").nth(1).unwrap().contains("sc_qrot"),
            "identity rotation survived:\n{src}"
        );
        // The translation is bound rather than inlined, so the source should only
        // show the subtraction and the value should appear in the buffer.
        assert!(src.contains(" - vec3<f32>("), "translation lost:\n{src}");
        assert!(
            generated.params.contains(&5.0),
            "translation not bound: {:?}",
            generated.params
        );
        validate_wgsl(src);
    }

    #[test]
    fn cpu_and_gpu_agree_on_structure() {
        // The CPU path is the reference. This does not execute the shader, but
        // it does pin that both backends were handed the same tree and that the
        // shader is well-formed for every node kind the evaluator supports.
        let (b, root) = kitchen_sink();
        let src = wgsl::generate(&b.arena, Some(root)).source;
        for kind in ["length", "max", "min", "sc_smin", "sc_qrot"] {
            assert!(src.contains(kind), "codegen missing {kind}:\n{src}");
        }
        assert!(eval(&b.arena, root, Vec3::ZERO).is_finite());
        validate_wgsl(&src);
    }
}
