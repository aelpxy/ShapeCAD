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

    /// What sketching on this plane means, in the terms a printed part is
    /// thought about: which way is up, and which way a pad grows.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Self::Xy => "The build plate. Sketches lie flat and pads grow upwards, which is how most printed parts start.",
            Self::Xz => "The front elevation. Sketches stand upright and pads grow towards you along Y.",
            Self::Yz => "The side elevation. Sketches stand upright and pads grow to the right along X.",
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

    /// Lifts a sketch coordinate into the model.
    pub(crate) fn to_world(self, point: Vec2) -> Vec3 {
        let (u, v, _) = self.frame();
        u * point.x + v * point.y
    }

    /// The transform that carries geometry built in sketch coordinates into the
    /// model.
    ///
    /// An `Extrude` is defined in its own XY plane sweeping along +Z, so placing
    /// one is exactly a rotation from that frame onto this one.
    pub(crate) fn placement(self) -> Transform {
        self.placement_at(0.0)
    }

    /// As [`SketchPlane::placement`], shifted along the normal.
    ///
    /// A pocket has to begin outside the material it cuts, so it is placed
    /// behind the plane and swept far enough to emerge.
    pub(crate) fn placement_at(self, offset: f32) -> Transform {
        let (u, v, n) = self.frame();
        Transform {
            translation: n * offset,
            rotation: Quat::from_mat3(&Mat3::from_cols(u, v, n)),
            scale: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything else derives from the frame, so it is what the tests pin down.
    fn to_world(plane: SketchPlane, p: Vec2) -> Vec3 {
        let (u, v, _) = plane.frame();
        u * p.x + v * p.y
    }

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
    fn placement_matches_the_frame() {
        // The placement must carry the extrusion's own axes onto the plane's, or
        // a pad comes out facing the wrong way.
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
    fn a_placement_carries_sketch_coordinates_into_the_model() {
        for plane in SketchPlane::ALL {
            for p in [Vec2::new(3.0, -7.0), Vec2::ZERO, Vec2::new(-12.5, 40.0)] {
                let through_placement = plane.placement().apply_point(Vec3::new(p.x, p.y, 0.0));
                assert!(
                    (through_placement - to_world(plane, p)).length() < 1.0e-5,
                    "{} placed {p:?} at {through_placement:?}",
                    plane.name()
                );
            }
        }
    }

    #[test]
    fn an_offset_placement_moves_along_the_normal() {
        let placed = SketchPlane::Xy.placement_at(-3.0);
        assert!((placed.translation - Vec3::new(0.0, 0.0, -3.0)).length() < 1.0e-6);
    }

    #[test]
    fn the_build_plate_is_the_identity() {
        assert_eq!(SketchPlane::Xy.placement(), Transform::IDENTITY);
        assert_eq!(
            to_world(SketchPlane::Xy, Vec2::new(5.0, 6.0)),
            Vec3::new(5.0, 6.0, 0.0)
        );
    }
}
