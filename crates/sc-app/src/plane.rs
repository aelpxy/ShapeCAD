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

    /// A few sketch coordinates to carry through, including the degenerate one.
    const POINTS: [Vec2; 4] = [
        Vec2::ZERO,
        Vec2::new(3.0, -7.0),
        Vec2::new(-12.5, 40.0),
        Vec2::new(0.0, 1.0),
    ];

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

    /// Right-handed is not enough on its own. Axes that are not unit length, or
    /// not square to each other, would scale or shear a sketch on the way into
    /// the model: a 40mm rectangle would come out some other size, and the field
    /// under it would no longer be a distance.
    #[test]
    fn every_frame_is_orthonormal() {
        for plane in SketchPlane::ALL {
            let (u, v, n) = plane.frame();
            for (name, axis) in [("u", u), ("v", v), ("n", n)] {
                assert!(
                    (axis.length() - 1.0).abs() < 1.0e-6,
                    "{}: {name} is {} long",
                    plane.name(),
                    axis.length()
                );
            }
            for (a, b, pair) in [(u, v, "u,v"), (v, n, "v,n"), (n, u, "n,u")] {
                assert!(
                    a.dot(b).abs() < 1.0e-6,
                    "{}: {pair} are not square, dot is {}",
                    plane.name(),
                    a.dot(b)
                );
            }
        }
    }

    /// The three are meant to be three different planes, each described in the
    /// terms a printed part is thought about.
    #[test]
    fn the_three_planes_are_distinct_and_described() {
        for (i, plane) in SketchPlane::ALL.into_iter().enumerate() {
            for other in SketchPlane::ALL.into_iter().skip(i + 1) {
                assert_ne!(plane, other);
                assert_ne!(plane.name(), other.name());
                assert_ne!(plane.describe(), other.describe());
                assert!(
                    (plane.frame().2 - other.frame().2).length() > 1.0e-6,
                    "{} and {} face the same way",
                    plane.name(),
                    other.name()
                );
            }
            assert!(plane.describe().len() > 40, "{} says nothing", plane.name());
            assert!(
                !plane.describe().contains('\u{2014}'),
                "{} has an em dash in it",
                plane.name()
            );
        }
        assert!(SketchPlane::ALL.contains(&SketchPlane::default()));
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

    /// Through the real `to_world`, not a copy of it: the placement and the lift
    /// are two routes to the same point, and the interface draws a sketch by one
    /// and builds it by the other.
    #[test]
    fn a_placement_carries_sketch_coordinates_into_the_model() {
        for plane in SketchPlane::ALL {
            for p in POINTS {
                let through_placement = plane.placement().apply_point(Vec3::new(p.x, p.y, 0.0));
                assert!(
                    (through_placement - plane.to_world(p)).length() < 1.0e-5,
                    "{} placed {p:?} at {through_placement:?}, lifted it to {:?}",
                    plane.name(),
                    plane.to_world(p)
                );
            }
        }
    }

    /// A pocket is placed behind its plane and swept through, so the offset has
    /// to run along the normal on every plane, not only on the one that was
    /// tried first. On XZ the normal is negative, which is where a sign gets
    /// lost.
    #[test]
    fn an_offset_placement_moves_along_the_normal() {
        for plane in SketchPlane::ALL {
            let (.., n) = plane.frame();
            for offset in [-3.0, 0.0, 12.5] {
                let placed = plane.placement_at(offset);
                assert!(
                    (placed.translation - n * offset).length() < 1.0e-6,
                    "{} at {offset} moved to {:?}, expected {:?}",
                    plane.name(),
                    placed.translation,
                    n * offset
                );
                // Only the translation moves: an offset plane is the same plane.
                assert_eq!(placed.rotation, plane.placement().rotation);
                assert!((placed.scale - 1.0).abs() < f32::EPSILON);
            }
            assert_eq!(plane.placement(), plane.placement_at(0.0));
        }
    }

    /// And the offset lands where the sketch frame says it should, which is what
    /// a pocket depends on to start outside the material.
    #[test]
    fn an_offset_sketch_point_lands_off_the_plane() {
        for plane in SketchPlane::ALL {
            let (.., n) = plane.frame();
            for p in POINTS {
                let placed = plane
                    .placement_at(-2.0)
                    .apply_point(Vec3::new(p.x, p.y, 0.0));
                assert!(
                    (placed - (plane.to_world(p) - n * 2.0)).length() < 1.0e-5,
                    "{} put {p:?} at {placed:?}",
                    plane.name()
                );
            }
        }
    }

    #[test]
    fn the_build_plate_is_the_identity() {
        assert_eq!(SketchPlane::Xy.placement(), Transform::IDENTITY);
        assert_eq!(
            SketchPlane::Xy.to_world(Vec2::new(5.0, 6.0)),
            Vec3::new(5.0, 6.0, 0.0)
        );
    }
}
