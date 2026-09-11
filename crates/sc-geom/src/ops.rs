//! Ergonomic construction of implicit DAGs.
//!
//! Thin sugar over [`Arena`], with no logic of its own, so it cannot drift from the
//! kernel. Used by tests, the CLI and (later) the agent layer's tool handlers.

use crate::arena::Arena;
use crate::error::Result;
use crate::math::Transform;
use crate::node::{Node, NodeId};
use glam::{Quat, Vec3};

/// Fluent constructor for implicit DAGs.
#[derive(Clone, Debug, Default)]
pub struct Builder {
    /// The arena being built into.
    pub arena: Arena,
}

impl Builder {
    /// An empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes the builder, yielding the arena.
    #[must_use]
    pub fn into_arena(self) -> Arena {
        self.arena
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn sphere(&mut self, radius: f32) -> Result<NodeId> {
        self.arena.insert(Node::Sphere { radius })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn cuboid(&mut self, half: Vec3) -> Result<NodeId> {
        self.arena.insert(Node::Box { half, round: 0.0 })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn rounded_cuboid(&mut self, half: Vec3, round: f32) -> Result<NodeId> {
        self.arena.insert(Node::Box { half, round })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn cube(&mut self, half: f32) -> Result<NodeId> {
        self.cuboid(Vec3::splat(half))
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn cylinder(&mut self, radius: f32, half_height: f32) -> Result<NodeId> {
        self.arena.insert(Node::Cylinder {
            radius,
            half_height,
            round: 0.0,
        })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn torus(&mut self, major: f32, minor: f32) -> Result<NodeId> {
        self.arena.insert(Node::Torus { major, minor })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn plane(&mut self, normal: Vec3, offset: f32) -> Result<NodeId> {
        self.arena.insert(Node::Plane { normal, offset })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn union(&mut self, a: NodeId, b: NodeId) -> Result<NodeId> {
        self.smooth_union(a, b, 0.0)
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn smooth_union(&mut self, a: NodeId, b: NodeId, smooth: f32) -> Result<NodeId> {
        self.arena.insert(Node::Union { a, b, smooth })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn difference(&mut self, a: NodeId, b: NodeId) -> Result<NodeId> {
        self.smooth_difference(a, b, 0.0)
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn smooth_difference(&mut self, a: NodeId, b: NodeId, smooth: f32) -> Result<NodeId> {
        self.arena.insert(Node::Difference { a, b, smooth })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn intersection(&mut self, a: NodeId, b: NodeId) -> Result<NodeId> {
        self.arena.insert(Node::Intersection { a, b, smooth: 0.0 })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn transform(&mut self, child: NodeId, xform: Transform) -> Result<NodeId> {
        self.arena.insert(Node::Transform {
            child,
            xform,
            on: None,
        })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn translate(&mut self, child: NodeId, v: Vec3) -> Result<NodeId> {
        self.transform(child, Transform::from_translation(v))
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn rotate(&mut self, child: NodeId, q: Quat) -> Result<NodeId> {
        self.transform(child, Transform::from_rotation(q))
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn offset(&mut self, child: NodeId, distance: f32) -> Result<NodeId> {
        self.arena.insert(Node::Offset { child, distance })
    }

    /// A closed profile swept along +Z from z = 0.
    ///
    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn extrude(&mut self, profile: crate::Profile, depth: f32) -> Result<NodeId> {
        self.arena.insert(Node::Extrude { profile, depth })
    }

    /// # Errors
    /// Propagates validation failures from [`Arena::insert`].
    pub fn shell(&mut self, child: NodeId, thickness: f32) -> Result<NodeId> {
        self.arena.insert(Node::Shell { child, thickness })
    }
}
