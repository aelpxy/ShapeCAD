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

    /// Composes two transforms: this one applied first, then `outer`.
    ///
    /// Needed to answer "where in the world is this node", which is how a work
    /// plane attached to a face stays attached when the feature under it moves.
    #[must_use]
    pub fn then(&self, outer: &Self) -> Self {
        Self {
            translation: outer.rotation * (self.translation * outer.scale) + outer.translation,
            rotation: outer.rotation * self.rotation,
            scale: outer.scale * self.scale,
        }
    }

    /// The transform that undoes this one.
    ///
    /// Needed to express a placement in its parent's frame when what is known is
    /// where it must end up in the world: divide out everything above it.
    #[must_use]
    pub fn inverse(&self) -> Self {
        let rotation = self.rotation.inverse();
        let scale = 1.0 / self.scale;
        Self {
            translation: (rotation * -self.translation) * scale,
            rotation,
            scale,
        }
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

#[cfg(test)]
mod tests {
    use super::Transform;
    use glam::{Quat, Vec3};

    #[test]
    fn composition_matches_applying_them_in_turn() {
        let inner = Transform {
            translation: Vec3::new(3.0, -1.0, 2.0),
            rotation: Quat::from_rotation_z(0.7),
            scale: 2.0,
        };
        let outer = Transform {
            translation: Vec3::new(-5.0, 4.0, 1.0),
            rotation: Quat::from_rotation_x(-0.4),
            scale: 0.5,
        };
        let combined = inner.then(&outer);

        for p in [
            Vec3::ZERO,
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(-7.0, 0.5, 9.0),
        ] {
            let stepwise = outer.apply_point(inner.apply_point(p));
            let composed = combined.apply_point(p);
            assert!(
                (stepwise - composed).length() < 1.0e-4,
                "{p:?}: {stepwise:?} vs {composed:?}"
            );
        }
    }

    #[test]
    fn the_inverse_undoes_the_transform() {
        let t = Transform {
            translation: Vec3::new(3.0, -1.0, 2.0),
            rotation: Quat::from_rotation_z(0.7),
            scale: 2.0,
        };
        let back = t.inverse();
        for p in [
            Vec3::ZERO,
            Vec3::new(1.0, 2.0, 3.0),
            Vec3::new(-7.0, 0.5, 9.0),
        ] {
            let round_trip = back.apply_point(t.apply_point(p));
            assert!(
                (round_trip - p).length() < 1.0e-4,
                "{p:?} came back as {round_trip:?}"
            );
            assert!((back.apply_point(p) - t.inverse_point(p)).length() < 1.0e-4);
        }
        let identity = t.then(&back);
        assert!(identity.translation.length() < 1.0e-4, "{identity:?}");
        assert!((identity.scale - 1.0).abs() < 1.0e-4, "{identity:?}");
    }

    #[test]
    fn composing_with_the_identity_changes_nothing() {
        let t = Transform {
            translation: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::from_rotation_y(1.1),
            scale: 3.0,
        };
        let both = t.then(&Transform::IDENTITY);
        assert!((both.translation - t.translation).length() < 1.0e-5);
        assert!((both.scale - t.scale).abs() < 1.0e-5);
    }
}
