//! Transform types for the implicit kernel.

use glam::{Quat, Vec3};

/// A rigid transform plus *uniform* scale.
///
/// Non-uniform scale is deliberately unrepresentable. Applying it to a signed
/// distance field degrades the field from an exact distance to a mere bound on
/// distance, which silently breaks sphere tracing, offsets, shells and smooth
/// blends everywhere downstream. If a user wants a stretched box they get a box
/// with different half-extents, not a scaled one.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Transform {
    /// Position of the child's origin in parent space.
    pub translation: Vec3,
    /// Orientation of the child frame. Expected to be normalised.
    pub rotation: Quat,
    /// Uniform scale factor. Must be finite and strictly positive.
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    /// The transform that changes nothing.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: 1.0,
    };

    /// A pure translation.
    #[must_use]
    pub fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    /// A pure rotation about the origin.
    #[must_use]
    pub fn from_rotation(rotation: Quat) -> Self {
        Self {
            rotation,
            ..Self::IDENTITY
        }
    }

    /// A pure uniform scale about the origin.
    #[must_use]
    pub fn from_scale(scale: f32) -> Self {
        Self {
            scale,
            ..Self::IDENTITY
        }
    }

    /// Whether this transform can be applied to a distance field without
    /// corrupting it.
    ///
    /// Rejects zero, negative, NaN and infinite scale, non-finite translation,
    /// and any rotation that is not a unit quaternion.
    pub fn is_valid(&self) -> bool {
        self.scale.is_finite()
            && self.scale > 0.0
            && self.translation.is_finite()
            && self.rotation.is_finite()
            && self.rotation.is_normalized()
    }

    /// Maps a parent-space point into the child's local frame.
    #[must_use]
    pub fn inverse_point(&self, p: Vec3) -> Vec3 {
        (self.rotation.inverse() * (p - self.translation)) / self.scale
    }

    /// Maps a child-space point out into parent space.
    #[must_use]
    pub fn apply_point(&self, p: Vec3) -> Vec3 {
        self.rotation * (p * self.scale) + self.translation
    }

    /// Converts a distance measured in the child's frame into parent units.
    ///
    /// Forgetting this is the classic way to turn a valid distance field into
    /// one that merely bounds distance, so it is a named operation rather than
    /// a bare multiply at the call site.
    #[must_use]
    pub fn apply_distance(&self, d: f32) -> f32 {
        d * self.scale
    }
}
