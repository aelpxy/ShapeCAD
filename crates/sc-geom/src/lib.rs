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
pub mod sdf;
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
pub use sdf::{AssetId, Grid};

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

    /// A sphere of radius 3 sampled on a 25^3 grid of 0.5mm voxels, which leaves
    /// the surface six voxels clear of every face.
    fn sphere_mesh_node(asset: u32) -> Node {
        let grid = Grid::from_fn([25; 3], Vec3::splat(-6.0), 0.5, |p| p.length() - 3.0);
        assert!(
            grid.boundary_clearance() >= Grid::REQUIRED_CLEARANCE as f32 * grid.spacing,
            "test fixture does not meet the padding precondition"
        );
        Node::mesh(AssetId(asset), std::sync::Arc::new(grid))
    }

    #[test]
    fn an_imported_mesh_booleans_against_modelled_geometry() {
        // The whole point of voxelizing on import: the boolean does not know or
        // care that one side of it came from a triangle mesh.
        let mut b = Builder::new();
        let imported = b.arena.insert(sphere_mesh_node(1)).unwrap();
        let block = b.cuboid(Vec3::new(4.0, 1.0, 1.0)).unwrap();
        let moved = b.translate(block, Vec3::new(5.0, 0.0, 0.0)).unwrap();
        let joined = b.union(imported, moved).unwrap();

        let analytic_union = |p: Vec3| {
            let sphere = p.length() - 3.0;
            let q = (p - Vec3::new(5.0, 0.0, 0.0)).abs() - Vec3::new(4.0, 1.0, 1.0);
            let block = q.max(Vec3::ZERO).length() + q.max_element().min(0.0);
            sphere.min(block)
        };

        for i in 0..80 {
            let t = i as f32 / 79.0;
            for p in [
                Vec3::new(t * 12.0 - 4.0, 0.0, 0.0),
                Vec3::new(t * 12.0 - 4.0, 0.7, -0.4),
                Vec3::new(2.0, t * 8.0 - 4.0, 1.0),
            ] {
                let field = eval(&b.arena, joined, p);
                let expected = analytic_union(p);
                assert!(
                    (field - expected).abs() <= 0.5,
                    "at {p:?}: union field {field}, analytic {expected}"
                );
                // Inside a voxel of the surface the two may legitimately
                // disagree on which side a point is; anywhere else they must
                // not.
                if expected.abs() > 0.5 {
                    assert_eq!(
                        field < 0.0,
                        expected < 0.0,
                        "at {p:?}: solid disagrees, {field} against {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_mesh_node_is_a_leaf_with_no_editable_parameters() {
        let node = sphere_mesh_node(4);
        assert_eq!(node.kind(), "mesh");
        assert!(node.params().is_empty(), "a mesh has nothing to drag");
        assert_eq!(node.children().count(), 0);
        assert_eq!(node.mesh_resolution(), Some([25, 25, 25]));
        let mut copy = node.clone();
        assert!(
            !copy.set_param("resolution", 64.0),
            "resolution is read-only"
        );
        assert!(!copy.set_param("radius", 1.0));
    }

    #[test]
    fn meshes_compare_by_identity_not_by_contents() {
        // Two nodes over one import are the same node. Two imports that happen
        // to hold identical samples are not, and neither comparison reads a
        // single voxel.
        let shared = std::sync::Arc::new(Grid::from_fn([5; 3], Vec3::splat(-2.0), 1.0, |p| {
            p.length() - 1.0
        }));
        let one = Node::mesh(AssetId(1), shared.clone());
        assert_eq!(one, Node::mesh(AssetId(1), shared.clone()));
        assert_ne!(one, Node::mesh(AssetId(2), shared.clone()));

        let identical_copy = std::sync::Arc::new((*shared).clone());
        assert_eq!(*identical_copy, *shared, "the grids really are equal");
        assert_ne!(
            one,
            Node::mesh(AssetId(1), identical_copy),
            "equal contents must not make two imports one node"
        );
    }

    #[test]
    fn every_node_kind_equals_a_copy_of_itself() {
        // `PartialEq` for `Node` is written out rather than derived, so a new
        // variant can be forgotten and fall into the catch-all as unequal to
        // itself. Nothing else in the kernel would notice.
        let mut b = Builder::new();
        let leaf = b.sphere(1.0).unwrap();
        let profile = Profile::Circle { radius: 1.0 };
        let nodes = vec![
            Node::Sphere { radius: 1.0 },
            Node::Box {
                half: Vec3::ONE,
                round: 0.1,
            },
            Node::Cylinder {
                radius: 1.0,
                half_height: 2.0,
                round: 0.0,
            },
            Node::Torus {
                major: 2.0,
                minor: 0.5,
            },
            Node::Plane {
                normal: Vec3::Z,
                offset: 1.0,
            },
            sphere_mesh_node(9),
            Node::Union {
                a: leaf,
                b: leaf,
                smooth: 0.2,
            },
            Node::Difference {
                a: leaf,
                b: leaf,
                smooth: 0.0,
            },
            Node::Intersection {
                a: leaf,
                b: leaf,
                smooth: 0.3,
            },
            Node::Transform {
                child: leaf,
                xform: Transform::from_scale(2.0),
                on: None,
            },
            Node::Offset {
                child: leaf,
                distance: 0.5,
            },
            Node::Extrude {
                profile,
                depth: 3.0,
            },
            Node::Shell {
                child: leaf,
                thickness: 1.0,
            },
        ];
        for node in &nodes {
            assert_eq!(
                node,
                &node.clone(),
                "a {} is not equal to itself",
                node.kind()
            );
        }
        for (i, a) in nodes.iter().enumerate() {
            for other in &nodes[i + 1..] {
                assert_ne!(a, other, "{} equals a {}", a.kind(), other.kind());
            }
        }
    }

    #[test]
    fn an_unresolved_mesh_is_refused_by_the_arena() {
        let mut b = Builder::new();
        let placeholder = Node::mesh(AssetId(0), std::sync::Arc::new(Grid::default()));
        assert!(!placeholder.is_valid());
        assert!(
            b.arena.insert(placeholder).is_err(),
            "an unresolved asset must not reach the renderer as empty space"
        );
    }

    #[test]
    fn the_geometry_hash_sees_the_voxels() {
        let mut b = Builder::new();
        let node = b.arena.insert(sphere_mesh_node(1)).unwrap();
        let before = geometry_hash(&b.arena, node);

        // Same asset id, same dimensions, one voxel different.
        let Node::Mesh { asset, grid } = b.arena.get(node).unwrap().clone() else {
            unreachable!("just inserted a mesh")
        };
        let mut edited = (*grid).clone();
        edited.data[7000] -= 1.0;
        b.arena
            .replace(node, Node::mesh(asset, std::sync::Arc::new(edited)))
            .unwrap();

        let after = geometry_hash(&b.arena, node);
        assert_ne!(
            before.structure, after.structure,
            "a changed voxel left the structure hash alone"
        );
    }

    #[test]
    fn codegen_reports_a_mesh_rather_than_quietly_dropping_it() {
        let mut b = Builder::new();
        let imported = b.arena.insert(sphere_mesh_node(3)).unwrap();
        let solid = b.cube(1.0).unwrap();
        let root = b.union(imported, solid).unwrap();

        let generated = wgsl::generate(&b.arena, Some(root));
        assert!(!generated.is_complete(), "the mesh was emitted silently");
        assert_eq!(generated.unsupported, vec![imported]);
        // Still a shader that compiles: a module that fails to build reaches the
        // user as an opaque driver error, which is worse than a reported gap.
        validate_wgsl(&generated.source);
        assert!(generated.source.contains("no GPU path yet"));

        match wgsl::try_generate(&b.arena, Some(root)) {
            Err(GeomError::NotInShader { node, kind }) => {
                assert_eq!(node, imported);
                assert_eq!(kind, "mesh");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }

        // A model without a mesh is unaffected.
        assert!(wgsl::generate(&b.arena, Some(solid)).is_complete());
        assert!(wgsl::try_generate(&b.arena, Some(solid)).is_ok());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn a_deserialized_mesh_is_a_placeholder_that_fails_validation() {
        // The samples are not in the document, so what comes back is a stub the
        // loader is expected to fill in from the sidecar asset.
        let node = sphere_mesh_node(12);
        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("\"asset\""), "{json}");
        assert!(
            !json.contains("grid"),
            "the grid was written into the document: {json}"
        );

        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind(), "mesh");
        assert_eq!(back.mesh_resolution(), Some([0, 0, 0]));
        assert!(back.mesh_grid().unwrap().is_placeholder());
        assert!(
            !back.is_valid(),
            "an unresolved mesh passed validation and would render as nothing"
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn a_grid_survives_a_round_trip_of_its_own() {
        // Not through the document, which stores grids beside it, but the sidecar
        // writer needs some serialisation and this pins that it is lossless.
        let grid = Grid::from_fn([5, 6, 7], Vec3::splat(-2.0), 0.75, |p| p.length() - 1.0);
        let back: Grid = serde_json::from_str(&serde_json::to_string(&grid).unwrap()).unwrap();
        assert_eq!(grid, back);
        assert_eq!(grid.digest(), back.digest());
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
                    on: None,
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
