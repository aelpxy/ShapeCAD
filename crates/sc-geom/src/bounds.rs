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
    #[must_use]
    pub fn transformed(self, t: &Transform) -> Self {
        if !self.is_finite() {
            return self;
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

    /// Clamps to a finite box so unbounded nodes, such as a bare half-space,
    /// still give the camera and the geometry hash something to work with.
    #[must_use]
    pub fn finite_or(self, fallback_half: f32) -> Self {
        if self.is_empty() {
            return Self::from_half(Vec3::splat(fallback_half));
        }
        let f = Vec3::splat(fallback_half);
        Self {
            min: self.min.max(-f),
            max: self.max.min(f),
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
        // The voxel footprint, which is half a voxel wider than the outermost
        // sample centres and at least two voxels wider than the surface.
        Node::Mesh { ref grid, .. } => grid.bounds(),
        // Bounded across the profile, unbounded along the sweep. A prism only
        // ever appears as the tool of a difference or an intersection, and both
        // take their bounds from the other operand, so the infinity is contained
        // in every position the node is meant to occupy.
        Node::Prism { .. } => {
            let Some(Node::Prism { profile }) = arena.get(id) else {
                return Aabb::EMPTY;
            };
            let (lo, hi) = profile.bounds();
            Aabb {
                min: Vec3::new(lo.x, lo.y, f32::NEG_INFINITY),
                max: Vec3::new(hi.x, hi.y, f32::INFINITY),
            }
        }

        Node::Extrude { .. } => {
            // Re-fetched by reference: a profile is not `Copy`, so it cannot be
            // bound by the surrounding match on `*node`.
            let Some(Node::Extrude { profile, depth }) = arena.get(id) else {
                return Aabb::EMPTY;
            };
            let (lo, hi) = profile.bounds();
            Aabb {
                min: Vec3::new(lo.x, lo.y, 0.0),
                max: Vec3::new(hi.x, hi.y, *depth),
            }
        }

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
