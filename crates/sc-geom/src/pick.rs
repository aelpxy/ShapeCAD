//! Resolving a point in space to the node that put it there.
//!
//! There are no faces or edges to select, so picking works on the tree instead.
//! At a surface point, ask which primitives are at zero distance: one means the
//! user clicked a face, two means they clicked the seam where those two shapes
//! meet, and the node to select is then the boolean that joins them.
//!
//! This is the mechanism described in ADR 0004, and it is why a fillet is
//! adjusted by clicking a joint rather than by selecting edges.

use crate::arena::Arena;
use crate::eval::eval;
use crate::node::{Node, NodeId};
use glam::Vec3;

/// What was found under a point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// A single primitive owns this point: a face.
    Face(NodeId),
    /// Two primitives meet here. `boolean` is the node that joins them, and is
    /// what carries the blend radius.
    Seam {
        /// The boolean joining the two shapes.
        boolean: NodeId,
        /// The primitives that meet.
        between: (NodeId, NodeId),
    },
}

impl Hit {
    /// The node a selection should land on.
    #[must_use]
    pub fn node(self) -> NodeId {
        match self {
            Hit::Face(id) => id,
            Hit::Seam { boolean, .. } => boolean,
        }
    }
}

/// A primitive found at the query point, with the path taken to reach it.
struct Found {
    id: NodeId,
    /// Distance from the query point to this primitive's surface, in world
    /// units, so that entries gathered under different placements compare.
    distance: f32,
    /// The query point in the primitive's own frame. Two entries for one node
    /// at one local point are two readings of a single surface, not two shapes
    /// that meet.
    local: Vec3,
    path: Vec<NodeId>,
}

/// Finds what lies under `world`, within `tolerance`.
///
/// `tolerance` should be roughly the size of a pixel at the clicked depth. Too
/// tight and a click on a curved surface misses; too loose and everything reads
/// as a seam.
#[must_use]
pub fn pick(arena: &Arena, root: NodeId, world: Vec3, tolerance: f32) -> Option<Hit> {
    let mut found = Vec::new();
    let mut path = Vec::new();
    gather(arena, root, world, tolerance, 1.0, &mut path, &mut found);

    // Closest first, so a near-exact face wins over a grazing neighbour.
    found.sort_by(|a, b| a.distance.total_cmp(&b.distance));
    let first = found.first()?;

    // A second surface within the same tolerance means the click landed where
    // two shapes meet rather than in the middle of a face. The tree is a DAG, so
    // the runner-up can be the same primitive reached down another branch: that
    // is a second surface only if it was reached at a different point in the
    // primitive's own frame, as two placements of one part are. The same point
    // on the same primitive twice is one face, and calling it a seam would offer
    // a blend on an edge that is not there.
    let second = found
        .iter()
        .find(|f| f.id != first.id || f.local.distance_squared(first.local) > 0.0);
    if let Some(second) = second {
        if let Some(boolean) = common_ancestor(&first.path, &second.path) {
            if matches!(
                arena.get(boolean),
                Some(Node::Union { .. } | Node::Difference { .. } | Node::Intersection { .. })
            ) {
                return Some(Hit::Seam {
                    boolean,
                    between: (first.id, second.id),
                });
            }
        }
    }
    Some(Hit::Face(first.id))
}

/// Walks the tree, carrying the point into each node's own frame.
///
/// A child under a transform is tested in its local coordinates, and `scale`
/// carries the accumulated placement so the distance it reports comes back out
/// in world units. Converting the distance is what makes `tolerance` mean the
/// same thing at every depth; dividing the tolerance as well would apply the
/// scale twice.
fn gather(
    arena: &Arena,
    id: NodeId,
    p: Vec3,
    tolerance: f32,
    scale: f32,
    path: &mut Vec<NodeId>,
    out: &mut Vec<Found>,
) {
    let Some(node) = arena.get(id) else { return };
    path.push(id);

    match *node {
        Node::Transform { child, xform, .. } => {
            gather(
                arena,
                child,
                xform.inverse_point(p),
                tolerance,
                scale * xform.scale,
                path,
                out,
            );
        }
        Node::Union { a, b, .. }
        | Node::Difference { a, b, .. }
        | Node::Intersection { a, b, .. } => {
            gather(arena, a, p, tolerance, scale, path, out);
            gather(arena, b, p, tolerance, scale, path, out);
        }
        // Everything else answers for itself, including the modifiers: an offset
        // or a shell moves the surface, so its child is not at zero where the
        // result is, and the modifier is the node worth selecting anyway.
        _ => {
            let d = eval(arena, id, p).abs() * scale;
            if d <= tolerance {
                out.push(Found {
                    id,
                    distance: d,
                    local: p,
                    path: path.clone(),
                });
            }
        }
    }

    path.pop();
}

/// The deepest node both paths pass through.
fn common_ancestor(a: &[NodeId], b: &[NodeId]) -> Option<NodeId> {
    a.iter()
        .zip(b)
        .take_while(|(x, y)| x == y)
        .map(|(x, _)| *x)
        .last()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Transform;
    use crate::ops::Builder;

    #[test]
    fn a_click_on_a_face_selects_that_primitive() {
        let mut b = Builder::new();
        let s = b.sphere(10.0).unwrap();
        let hit = pick(&b.arena, s, Vec3::new(10.0, 0.0, 0.0), 0.1).unwrap();
        assert_eq!(hit, Hit::Face(s));
        assert_eq!(hit.node(), s);
    }

    #[test]
    fn a_click_away_from_the_surface_finds_nothing() {
        let mut b = Builder::new();
        let s = b.sphere(10.0).unwrap();
        assert!(pick(&b.arena, s, Vec3::new(30.0, 0.0, 0.0), 0.1).is_none());
        assert!(pick(&b.arena, s, Vec3::ZERO, 0.1).is_none());
    }

    #[test]
    fn a_transform_is_seen_through() {
        // The point is in world space; the sphere is not at the origin.
        let mut b = Builder::new();
        let s = b.sphere(4.0).unwrap();
        let moved = b.translate(s, Vec3::new(50.0, 0.0, 0.0)).unwrap();
        let hit = pick(&b.arena, moved, Vec3::new(54.0, 0.0, 0.0), 0.1).unwrap();
        assert_eq!(hit.node(), s);
    }

    #[test]
    fn a_click_on_a_seam_selects_the_boolean() {
        // Two boxes meeting at x = 0; the shared plane is the seam.
        let mut b = Builder::new();
        let left = b.cuboid(Vec3::new(10.0, 10.0, 10.0)).unwrap();
        let left = b.translate(left, Vec3::new(-10.0, 0.0, 0.0)).unwrap();
        let right = b.cuboid(Vec3::new(10.0, 10.0, 10.0)).unwrap();
        let right = b.translate(right, Vec3::new(10.0, 0.0, 0.0)).unwrap();
        let joint = b.union(left, right).unwrap();

        let hit = pick(&b.arena, joint, Vec3::new(0.0, 0.0, 10.0), 0.2).unwrap();
        match hit {
            Hit::Seam { boolean, .. } => assert_eq!(boolean, joint),
            Hit::Face(id) => panic!("expected the seam, got face {id}"),
        }
        assert_eq!(hit.node(), joint, "selecting a seam selects the blend");
    }

    /// The tolerance is a world-space distance, and it has to stay one all the
    /// way down. A scaled placement used to apply the scale twice, once to the
    /// distance and once to the tolerance, so a click missed on anything
    /// enlarged and caught thin air on anything shrunk.
    #[test]
    fn tolerance_is_the_same_distance_under_a_scaled_placement() {
        let mut b = Builder::new();
        let s = b.sphere(4.0).unwrap();

        let enlarged = b.transform(s, Transform::from_scale(2.0)).unwrap();
        // The surface is at r = 8 in the world, and this is 0.08 outside it.
        assert!(
            pick(&b.arena, enlarged, Vec3::new(8.08, 0.0, 0.0), 0.1).is_some(),
            "a click 0.08 from the surface missed a 0.1 tolerance"
        );

        let shrunk = b.transform(s, Transform::from_scale(0.5)).unwrap();
        // The surface is at r = 2, and this is 0.15 outside it.
        assert!(
            pick(&b.arena, shrunk, Vec3::new(2.15, 0.0, 0.0), 0.1).is_none(),
            "a click 0.15 from the surface was caught by a 0.1 tolerance"
        );
    }

    /// The tree is a DAG, so one primitive can be reached down two branches. Two
    /// readings of the same surface are still one surface: calling them a seam
    /// offers a fillet on an edge that is not there.
    #[test]
    fn one_surface_reached_down_two_branches_is_not_a_seam() {
        let mut b = Builder::new();
        let s = b.sphere(10.0).unwrap();
        let here = b.translate(s, Vec3::new(4.0, 0.0, 0.0)).unwrap();
        let also_here = b.translate(s, Vec3::new(4.0, 0.0, 0.0)).unwrap();
        let both = b.union(here, also_here).unwrap();

        let hit = pick(&b.arena, both, Vec3::new(14.0, 0.0, 0.0), 0.1).unwrap();
        assert_eq!(hit, Hit::Face(s), "one surface read twice is not an edge");
    }

    /// The other half of the one above: two placements of one primitive that
    /// genuinely meet do make a seam, and the boolean joining them is what the
    /// blend radius lives on.
    #[test]
    fn two_placements_of_one_primitive_seam_where_they_touch() {
        let mut b = Builder::new();
        let s = b.sphere(10.0).unwrap();
        let left = b.translate(s, Vec3::new(-10.0, 0.0, 0.0)).unwrap();
        let right = b.translate(s, Vec3::new(10.0, 0.0, 0.0)).unwrap();
        let joint = b.union(left, right).unwrap();

        let hit = pick(&b.arena, joint, Vec3::ZERO, 0.1).unwrap();
        assert_eq!(hit.node(), joint, "the two copies meet at the origin");
        assert!(matches!(hit, Hit::Seam { .. }), "got {hit:?}");
    }

    #[test]
    fn a_modifier_answers_for_the_surface_it_moved() {
        let mut b = Builder::new();
        let s = b.sphere(10.0).unwrap();
        let grown = b.offset(s, 2.0).unwrap();
        // The offset surface is at 12; the sphere's own is nowhere near.
        let hit = pick(&b.arena, grown, Vec3::new(12.0, 0.0, 0.0), 0.1).unwrap();
        assert_eq!(hit, Hit::Face(grown));
        assert!(pick(&b.arena, grown, Vec3::new(10.0, 0.0, 0.0), 0.1).is_none());
    }

    #[test]
    fn picking_an_empty_model_finds_nothing() {
        let b = Builder::new();
        assert!(pick(&b.arena, crate::NodeId(3), Vec3::ZERO, 1.0).is_none());
    }

    #[test]
    fn the_middle_of_a_face_is_not_mistaken_for_a_seam() {
        let mut b = Builder::new();
        let left = b.cuboid(Vec3::new(10.0, 10.0, 10.0)).unwrap();
        let left = b.translate(left, Vec3::new(-10.0, 0.0, 0.0)).unwrap();
        let right = b.cuboid(Vec3::new(10.0, 10.0, 10.0)).unwrap();
        let right = b.translate(right, Vec3::new(10.0, 0.0, 0.0)).unwrap();
        let joint = b.union(left, right).unwrap();

        // Well away from x = 0, on top of the right-hand box.
        let hit = pick(&b.arena, joint, Vec3::new(15.0, 0.0, 10.0), 0.2).unwrap();
        assert!(
            matches!(hit, Hit::Face(_)),
            "got {hit:?} in the middle of a face"
        );
    }
}

/// The transform carrying `target` from its own frame into the model.
///
/// Walks the tree accumulating placements. Returns `None` if the node is not
/// reachable from `root`, which is how a feature that has been removed from the
/// model reports that it no longer has a position. A node shared by two
/// branches sits in two places at once, and this reports the first one found,
/// depth first with the left operand of a boolean taken first.
///
/// This is what lets a work plane be attached to a feature rather than pinned to
/// a coordinate: the plane is re-derived from the node each time, so changing
/// the feature underneath moves everything built on it.
#[must_use]
pub fn placement_of(arena: &Arena, root: NodeId, target: NodeId) -> Option<crate::Transform> {
    fn walk(
        arena: &Arena,
        id: NodeId,
        target: NodeId,
        acc: crate::Transform,
    ) -> Option<crate::Transform> {
        if id == target {
            return Some(acc);
        }
        let node = arena.get(id)?;
        if let Node::Transform { child, xform, .. } = node {
            return walk(arena, *child, target, xform.then(&acc));
        }
        node.children().find_map(|c| walk(arena, c, target, acc))
    }
    walk(arena, root, target, crate::Transform::IDENTITY)
}

/// Where a sketch drawn on `target`'s far face sits in the model.
///
/// This is [`placement_of`] with the face's own offset applied: an [`Extrude`]
/// runs from z = 0 to z = `depth` in its own frame, so its far face is a lift of
/// `depth` along the local +Z. `None` if the node is not reachable from `root`,
/// or is not a kind of feature with a face to sketch on.
///
/// One definition, used both when a sketch is being drawn and when a feature
/// already built on that face is regenerated. Two would drift, and a boss that
/// lands correctly and then moves the first time anything else is edited is
/// worse than one that is simply in the wrong place.
///
/// [`Extrude`]: crate::Node::Extrude
#[must_use]
pub fn face_placement(arena: &Arena, root: NodeId, target: NodeId) -> Option<crate::Transform> {
    let placement = placement_of(arena, root, target)?;
    let depth = match arena.get(target)? {
        Node::Extrude { depth, .. } => *depth,
        _ => return None,
    };
    Some(crate::Transform::from_translation(Vec3::new(0.0, 0.0, depth)).then(&placement))
}

#[cfg(test)]
mod placement_tests {
    use super::*;
    use crate::ops::Builder;

    #[test]
    fn a_nested_node_reports_its_world_placement() {
        let mut b = Builder::new();
        let s = b.sphere(1.0).unwrap();
        let inner = b.translate(s, Vec3::new(10.0, 0.0, 0.0)).unwrap();
        let outer = b.translate(inner, Vec3::new(0.0, 5.0, 0.0)).unwrap();

        let placed = placement_of(&b.arena, outer, s).expect("reachable");
        let world = placed.apply_point(Vec3::ZERO);
        assert!(
            (world - Vec3::new(10.0, 5.0, 0.0)).length() < 1.0e-4,
            "sphere sits at {world:?}"
        );
    }

    #[test]
    fn a_node_under_a_boolean_is_still_found() {
        let mut b = Builder::new();
        let a = b.sphere(1.0).unwrap();
        let c = b.cube(1.0).unwrap();
        let moved = b.translate(c, Vec3::new(4.0, 0.0, 0.0)).unwrap();
        let u = b.union(a, moved).unwrap();

        let placed = placement_of(&b.arena, u, c).expect("reachable");
        assert!((placed.apply_point(Vec3::ZERO) - Vec3::new(4.0, 0.0, 0.0)).length() < 1.0e-4);
    }

    #[test]
    fn a_face_placement_sits_on_the_far_end_of_the_extrusion() {
        let mut b = Builder::new();
        let pad = b
            .extrude(
                crate::Profile::Rect {
                    width: 10.0,
                    height: 10.0,
                },
                7.0,
            )
            .unwrap();
        let lifted = b.translate(pad, Vec3::new(0.0, 0.0, 2.0)).unwrap();

        let face = face_placement(&b.arena, lifted, pad).expect("reachable");
        let origin = face.apply_point(Vec3::ZERO);
        assert!(
            (origin - Vec3::new(0.0, 0.0, 9.0)).length() < 1.0e-4,
            "the face is at {origin:?}, expected z = 9"
        );
    }

    /// Only a feature with a face to sketch on has one. A sphere reports none
    /// rather than quietly returning its centre.
    #[test]
    fn a_node_with_no_face_has_no_face_placement() {
        let mut b = Builder::new();
        let s = b.sphere(3.0).unwrap();
        assert!(face_placement(&b.arena, s, s).is_none());
    }

    #[test]
    fn an_unreachable_node_has_no_placement() {
        let mut b = Builder::new();
        let a = b.sphere(1.0).unwrap();
        let orphan = b.cube(1.0).unwrap();
        assert!(placement_of(&b.arena, a, orphan).is_none());
    }
}
