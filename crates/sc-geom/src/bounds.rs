//! Conservative analytic bounds.
//!
//! Used to frame the camera, to seed the mesher's octree, and to bound the
//! sampling grid the geometry hash uses. Must never under-report: a bound that
//! is too small silently clips geometry out of the export.

use crate::arena::Arena;
use crate::math::Transform;
use crate::node::{Node, NodeId};
use glam::Vec3;
use std::collections::HashMap;

/// An axis-aligned bounding box.
///
/// Infinite components are permitted so unbounded primitives such as a
/// half-space can be represented honestly rather than clamped at construction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    /// Lower corner.
    pub min: Vec3,
    /// Upper corner.
    pub max: Vec3,
}

impl Aabb {
    /// The empty box, which absorbs into any union without affecting it.
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    /// The box covering all of space.
    pub const INFINITE: Self = Self {
        min: Vec3::splat(f32::NEG_INFINITY),
        max: Vec3::splat(f32::INFINITY),
    };

    /// A box centred on the origin with the given half-extents.
    #[must_use]
    pub fn from_half(h: Vec3) -> Self {
        Self { min: -h, max: h }
    }

    /// Whether the box contains no points.
    pub fn is_empty(&self) -> bool {
        self.min.cmpgt(self.max).any()
    }

    /// Whether both corners are finite.
    pub fn is_finite(&self) -> bool {
        self.min.is_finite() && self.max.is_finite()
    }

    /// Midpoint of the box.
    #[must_use]
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// Extent along each axis, clamped at zero for an empty box.
    #[must_use]
    pub fn size(&self) -> Vec3 {
        (self.max - self.min).max(Vec3::ZERO)
    }

    /// The smallest box containing both inputs.
    #[must_use]
    pub fn union(self, o: Self) -> Self {
        Self {
            min: self.min.min(o.min),
            max: self.max.max(o.max),
        }
    }

    /// The overlap of two boxes, possibly empty.
    #[must_use]
    pub fn intersection(self, o: Self) -> Self {
        Self {
            min: self.min.max(o.min),
            max: self.max.min(o.max),
        }
    }

    /// Grows the box by `d` on every side. An empty box stays empty.
    #[must_use]
    pub fn expand(self, d: f32) -> Self {
        if self.is_empty() {
            return self;
        }
        Self {
            min: self.min - Vec3::splat(d),
            max: self.max + Vec3::splat(d),
        }
    }

    /// The axis-aligned box enclosing this box after `t` is applied, computed
    /// from its eight corners.
    ///
    /// An unbounded box has no corners to enumerate and goes through
    /// [`Aabb::swept`] instead.
    #[must_use]
    pub fn transformed(self, t: &Transform) -> Self {
        if self.is_empty() {
            return self;
        }
        if !self.is_finite() {
            return self.swept(t);
        }
        let mut out = Self::EMPTY;
        for i in 0..8u32 {
            let c = Vec3::new(
                if i & 1 == 0 { self.min.x } else { self.max.x },
                if i & 2 == 0 { self.min.y } else { self.max.y },
                if i & 4 == 0 { self.min.z } else { self.max.z },
            );
            let p = t.apply_point(c);
            out = out.union(Self { min: p, max: p });
        }
        out
    }

    /// The image of an unbounded box, accumulated axis by axis.
    ///
    /// A rotation leans an unbounded axis into every world axis it has a
    /// component on, so the infinity has to move with it. Carrying it on the
    /// axis it started on is an under-report of everything the rotation turned
    /// into that direction: a through cut laid on its side is then bounded
    /// along the very axis it sweeps.
    fn swept(self, t: &Transform) -> Self {
        let mut min = t.translation;
        let mut max = t.translation;
        for (j, unit) in [Vec3::X, Vec3::Y, Vec3::Z].into_iter().enumerate() {
            let axis = (t.rotation * unit) * t.scale;
            for k in 0..3 {
                // A zero component contributes nothing, and has to be skipped
                // rather than multiplied: `0 * infinity` is NaN, not the
                // absence it stands for here.
                if axis[k] == 0.0 {
                    continue;
                }
                let (lo, hi) = (axis[k] * self.min[j], axis[k] * self.max[j]);
                min[k] += lo.min(hi);
                max[k] += lo.max(hi);
            }
        }
        Self { min, max }
    }

    /// Fills in the unbounded sides with a finite fallback, so unbounded nodes
    /// such as a bare half-space still give the camera, the mesher and the
    /// geometry hash something to work with.
    ///
    /// Only the infinite sides are substituted. Clamping the finite ones to the
    /// fallback as well would silently crop any part larger than it out of the
    /// mesh and out of the framing, which is the one thing bounds must never do.
    #[must_use]
    pub fn finite_or(self, fallback_half: f32) -> Self {
        if self.is_empty() {
            return Self::from_half(Vec3::splat(fallback_half));
        }
        let f = Vec3::splat(fallback_half);
        Self {
            min: Vec3::select(self.min.is_finite_mask(), self.min, -f),
            max: Vec3::select(self.max.is_finite_mask(), self.max, f),
        }
    }
}

/// Conservative bounds of the solid rooted at `id`.
///
/// Returns [`Aabb::EMPTY`] if the node is missing or deleted.
#[must_use]
pub fn bounds(arena: &Arena, id: NodeId) -> Aabb {
    bounds_memo(arena, id, &mut HashMap::new())
}

fn bounds_memo(arena: &Arena, id: NodeId, memo: &mut HashMap<NodeId, Aabb>) -> Aabb {
    if let Some(&b) = memo.get(&id) {
        return b;
    }
    let Some(node) = arena.get(id) else {
        return Aabb::EMPTY;
    };
    let b = match *node {
        Node::Sphere { radius } => Aabb::from_half(Vec3::splat(radius)),
        Node::Box { half, .. } => Aabb::from_half(half),
        Node::Cylinder {
            radius,
            half_height,
            ..
        } => Aabb::from_half(Vec3::new(radius, radius, half_height)),
        Node::Torus { major, minor } => {
            Aabb::from_half(Vec3::new(major + minor, major + minor, minor))
        }
        Node::Plane { .. } => Aabb::INFINITE,
        // The union of every instance's box. Transforming the child's box once
        // per instance over-reports for a rotated instance, which is the safe
        // direction: a bound may be too large and must never be too small.
        Node::Pattern { child, kind, count } => {
            let base = bounds_memo(arena, child, memo);
            let mut all = Aabb::EMPTY;
            for i in 0..count {
                all = all.union(base.transformed(&kind.placement(i, count)));
            }
            all
        }
        // The voxel footprint, which is half a voxel wider than the outermost
        // sample centres and at least two voxels wider than the surface.
        Node::Mesh { ref grid, .. } => grid.bounds(),
        // Matched by reference, because a profile is not `Copy` and so cannot
        // be bound by the surrounding match on `*node`.
        Node::Prism { .. } | Node::Extrude { .. } => sweep_bounds(node),

        // A smooth blend bulges outward near the seam. Expanding by the full
        // blend radius over-estimates (the true bulge is at most a quarter of
        // it) but stays on the safe side of never under-reporting.
        Node::Union { a, b, smooth } => bounds_memo(arena, a, memo)
            .union(bounds_memo(arena, b, memo))
            .expand(smooth),
        Node::Difference { a, smooth, .. } => bounds_memo(arena, a, memo).expand(smooth),
        Node::Intersection { a, b, smooth } => bounds_memo(arena, a, memo)
            .intersection(bounds_memo(arena, b, memo))
            .expand(smooth),

        Node::Transform { child, xform, .. } => bounds_memo(arena, child, memo).transformed(&xform),
        Node::Offset { child, distance } => {
            bounds_memo(arena, child, memo).expand(distance.max(0.0))
        }
        // An inward shell is a subset of its child.
        Node::Shell { child, .. } => bounds_memo(arena, child, memo),
    };
    memo.insert(id, b);
    b
}

/// Bounds of the two profile sweeps.
///
/// [`Node::Extrude`] is bounded by its own depth. [`Node::Prism`] is bounded
/// across the profile and unbounded along the sweep: it only ever appears as
/// the tool of a difference or an intersection, and both take their bounds from
/// the other operand, so the infinity is contained in every position the node is
/// meant to occupy.
fn sweep_bounds(node: &Node) -> Aabb {
    let (profile, depth) = match node {
        Node::Prism { profile } => (profile, None),
        Node::Extrude { profile, depth } => (profile, Some(*depth)),
        _ => return Aabb::EMPTY,
    };
    let (lo, hi) = profile.bounds();
    let (near, far) = match depth {
        // An extrusion runs from z = 0 to z = depth, and `Node::is_valid`
        // insists the depth is positive.
        Some(d) => (0.0, d),
        None => (f32::NEG_INFINITY, f32::INFINITY),
    };
    Aabb {
        min: Vec3::new(lo.x, lo.y, near),
        max: Vec3::new(hi.x, hi.y, far),
    }
}

#[cfg(test)]
mod tests {
    use super::{bounds, Aabb};
    use crate::eval::eval;
    use crate::math::Transform;
    use crate::node::Node;
    use crate::ops::Builder;
    use crate::profile::Profile;
    use glam::{Quat, Vec3};

    const QUARTER_TURN: f32 = std::f32::consts::FRAC_PI_2;

    /// `finite_or` feeds the mesher's octree and the camera's framing, so
    /// shrinking a finite bound to the fallback would crop a three metre part
    /// down to one and call the result an export.
    #[test]
    fn a_part_larger_than_the_fallback_keeps_its_own_bounds() {
        let big = Aabb::from_half(Vec3::splat(2000.0));
        assert_eq!(big.finite_or(1000.0), big);
    }

    #[test]
    fn the_fallback_fills_in_only_the_unbounded_sides() {
        let bar = Aabb {
            min: Vec3::new(-1500.0, -2.0, f32::NEG_INFINITY),
            max: Vec3::new(1500.0, 2.0, f32::INFINITY),
        };
        let filled = bar.finite_or(1000.0);
        assert_eq!(filled.min, Vec3::new(-1500.0, -2.0, -1000.0));
        assert_eq!(filled.max, Vec3::new(1500.0, 2.0, 1000.0));
    }

    #[test]
    fn an_empty_box_falls_back_to_the_whole_fallback() {
        assert_eq!(
            Aabb::EMPTY.finite_or(10.0),
            Aabb::from_half(Vec3::splat(10.0))
        );
        assert!(Aabb::EMPTY
            .transformed(&Transform::from_scale(3.0))
            .is_empty());
    }

    /// The usual case for a through cut: moved, not turned. The unbounded axis
    /// stays where it was and the other two stay as tight as they started, since
    /// a loose bound on a cut is a coarser octree for whatever it cuts.
    #[test]
    fn a_sweep_that_is_only_moved_keeps_its_cross_section() {
        let sweep = Aabb {
            min: Vec3::new(-2.0, -1.0, f32::NEG_INFINITY),
            max: Vec3::new(2.0, 1.0, f32::INFINITY),
        };
        let moved = sweep.transformed(&Transform::from_translation(Vec3::new(10.0, 20.0, 30.0)));
        assert_eq!(moved.min, Vec3::new(8.0, 19.0, f32::NEG_INFINITY));
        assert_eq!(moved.max, Vec3::new(12.0, 21.0, f32::INFINITY));
    }

    /// A sweep that is unbounded along z is unbounded along y once it is turned
    /// on its side, and bounded along z. Carrying the infinity on the axis it
    /// started on under-reports everything the rotation moved into it.
    #[test]
    fn an_unbounded_axis_follows_the_rotation() {
        let sweep = Aabb {
            min: Vec3::new(-2.0, -1.0, f32::NEG_INFINITY),
            max: Vec3::new(2.0, 1.0, f32::INFINITY),
        };
        let turned = sweep.transformed(&Transform::from_rotation(Quat::from_rotation_x(
            QUARTER_TURN,
        )));
        assert!(
            turned.min.y == f32::NEG_INFINITY && turned.max.y == f32::INFINITY,
            "the sweep now runs along y: {turned:?}"
        );
        assert!((turned.min.z + 1.0).abs() < 1e-4, "{turned:?}");
        assert!((turned.max.z - 1.0).abs() < 1e-4, "{turned:?}");
        assert!((turned.min.x + 2.0).abs() < 1e-4, "{turned:?}");
        assert!((turned.max.x - 2.0).abs() < 1e-4, "{turned:?}");
    }

    #[test]
    fn a_translated_half_space_stays_unbounded() {
        let moved = Aabb::INFINITE.transformed(&Transform::from_translation(Vec3::splat(5.0)));
        assert_eq!(moved, Aabb::INFINITE);
    }

    /// The real cost of the one above: a through cut placed on its side, whose
    /// other operand is the only thing bounding it.
    #[test]
    fn a_through_cut_on_its_side_does_not_crop_what_it_cuts() {
        let mut b = Builder::new();
        let ball = b.sphere(10.0).unwrap();
        let bar = b
            .arena
            .insert(Node::Prism {
                profile: Profile::Rect {
                    width: 4.0,
                    height: 2.0,
                },
            })
            .unwrap();
        let bar = b.rotate(bar, Quat::from_rotation_x(QUARTER_TURN)).unwrap();
        let part = b.intersection(ball, bar).unwrap();

        let bb = bounds(&b.arena, part);
        let p = Vec3::new(0.0, 9.0, 0.0);
        assert!(eval(&b.arena, part, p) < 0.0, "the sample point is solid");
        assert!(
            p.cmpge(bb.min).all() && p.cmple(bb.max).all(),
            "solid point {p:?} outside reported bounds {bb:?}"
        );
    }

    #[test]
    fn a_sweep_reports_the_axis_it_is_unbounded_on() {
        let mut b = Builder::new();
        let bar = b
            .arena
            .insert(Node::Prism {
                profile: Profile::Circle { radius: 3.0 },
            })
            .unwrap();
        let bb = bounds(&b.arena, bar);
        assert_eq!(bb.min, Vec3::new(-3.0, -3.0, f32::NEG_INFINITY));
        assert_eq!(bb.max, Vec3::new(3.0, 3.0, f32::INFINITY));
    }

    #[test]
    fn an_extrusion_runs_from_its_own_plane() {
        let mut b = Builder::new();
        let pad = b
            .extrude(
                Profile::Rect {
                    width: 10.0,
                    height: 4.0,
                },
                7.0,
            )
            .unwrap();
        let bb = bounds(&b.arena, pad);
        assert_eq!(bb.min, Vec3::new(-5.0, -2.0, 0.0));
        assert_eq!(bb.max, Vec3::new(5.0, 2.0, 7.0));
    }

    #[test]
    fn a_missing_node_has_no_bounds() {
        let b = Builder::new();
        assert!(bounds(&b.arena, crate::NodeId(7)).is_empty());
    }
}
