//! Datum planes: the surfaces a sketch can be drawn on.
//!
//! A sketch is always drawn in two dimensions. The plane decides how those two
//! dimensions sit in the model, and an extrusion runs along its normal. This is
//! the same idea as `FreeCAD`'s origin planes, and it is why a new document shows
//! three of them rather than nothing at all.

use sc_geom::glam::{Mat3, Quat, Vec2, Vec3};
use sc_geom::Transform;

/// One of the three planes through the origin.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SketchPlane {
    /// The build plate. Sketches here sit flat on the bed.
    #[default]
    Xy,
    /// Vertical, facing along -Y.
    Xz,
    /// Vertical, facing along +X.
    Yz,
}

impl SketchPlane {
    /// All three, in the order they are shown.
    pub(crate) const ALL: [Self; 3] = [Self::Xy, Self::Xz, Self::Yz];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Xy => "XY",
            Self::Xz => "XZ",
            Self::Yz => "YZ",
        }
    }

    /// The in-plane axes and the normal, as a right-handed frame.
    ///
    /// Sketch coordinates are expressed against `u` and `v`; an extrusion runs
    /// along `n`.
    pub(crate) fn frame(self) -> (Vec3, Vec3, Vec3) {
        match self {
            // u cross v gives n in every case, so the frame never mirrors the
            // sketch, which would silently reverse a profile's winding.
            Self::Xy => (Vec3::X, Vec3::Y, Vec3::Z),
            Self::Xz => (Vec3::X, Vec3::Z, -Vec3::Y),
            Self::Yz => (Vec3::Y, Vec3::Z, Vec3::X),
        }
    }

    pub(crate) fn normal(self) -> Vec3 {
        self.frame().2
    }

    /// Lifts a sketch coordinate into the model.
    pub(crate) fn to_world(self, point: Vec2) -> Vec3 {
        let (u, v, _) = self.frame();
        u * point.x + v * point.y
    }

    /// Drops a model point onto the plane's coordinates.
    pub(crate) fn to_plane(self, point: Vec3) -> Vec2 {
        let (u, v, _) = self.frame();
        Vec2::new(point.dot(u), point.dot(v))
    }

    /// The transform that carries geometry built in sketch coordinates into the
    /// model.
    ///
    /// An `Extrude` is defined in its own XY plane sweeping along +Z, so placing
    /// one is exactly a rotation from that frame onto this one.
    pub(crate) fn placement(self) -> Transform {
        let (u, v, n) = self.frame();
        Transform::from_rotation(Quat::from_mat3(&Mat3::from_cols(u, v, n)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_is_right_handed() {
        // A mirrored frame would reverse a profile's winding on that plane.
        for plane in SketchPlane::ALL {
            let (u, v, n) = plane.frame();
            assert!(
                (u.cross(v) - n).length() < 1.0e-6,
                "{} is left-handed: {u:?} x {v:?} != {n:?}",
                plane.name()
            );
        }
    }

    #[test]
    fn sketch_coordinates_survive_the_round_trip() {
        for plane in SketchPlane::ALL {
            for point in [Vec2::new(3.0, -7.0), Vec2::ZERO, Vec2::new(-12.5, 40.0)] {
                let back = plane.to_plane(plane.to_world(point));
                assert!(
                    (back - point).length() < 1.0e-5,
                    "{} mangled {point:?} into {back:?}",
                    plane.name()
                );
            }
        }
    }

    #[test]
    fn placement_matches_the_frame() {
        // The placement must carry the extrusion's own axes onto the plane's,
        // or a pad comes out facing the wrong way.
        for plane in SketchPlane::ALL {
            let (u, v, n) = plane.frame();
            let placed = plane.placement();
            for (local, expected) in [(Vec3::X, u), (Vec3::Y, v), (Vec3::Z, n)] {
                let got = placed.rotation * local;
                assert!(
                    (got - expected).length() < 1.0e-5,
                    "{}: {local:?} became {got:?}, expected {expected:?}",
                    plane.name()
                );
            }
        }
    }

    #[test]
    fn the_build_plate_is_the_identity() {
        assert_eq!(SketchPlane::Xy.normal(), Vec3::Z);
        assert_eq!(
            SketchPlane::Xy.to_world(Vec2::new(5.0, 6.0)),
            Vec3::new(5.0, 6.0, 0.0)
        );
    }
}
