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
    distance: f32,
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

    // A second primitive at comparable distance means this is a seam. Requiring
    // it to be genuinely close, rather than merely within tolerance, keeps a
    // click in the middle of a face from being read as an edge.
    if let Some(second) = found.get(1) {
        if second.distance <= tolerance {
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
    }
    Some(Hit::Face(first.id))
}

/// Walks the tree, carrying the point into each node's own frame.
///
/// A child under a transform must be tested in its local coordinates, and the
/// tolerance has to be divided by the same scale so it still means the same
/// distance in world units.
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
        Node::Transform { child, xform } => {
            gather(
                arena,
                child,
                xform.inverse_point(p),
                tolerance / xform.scale,
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
        Node::Offset { child, .. } | Node::Shell { child, .. } => {
            // Modifiers move the surface, so the child is not at zero where the
            // result is. Report the modifier itself, which is also the node
            // worth selecting.
            let d = eval(arena, id, p).abs() * scale;
            if d <= tolerance {
                out.push(Found {
                    id,
                    distance: d,
                    path: path.clone(),
                });
            }
            let _ = child;
        }
        _ => {
            let d = eval(arena, id, p).abs() * scale;
            if d <= tolerance {
                out.push(Found {
                    id,
                    distance: d,
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
