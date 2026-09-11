//! Closed 2D regions that a feature is built from.
//!
//! A profile is *parametric*: a rectangle is a width and a height, not four
//! points that happen to form one. That is what makes "set it to 60 by 40" mean
//! something later, and it is why changing a width updates the model instead of
//! requiring the sketch to be redrawn.
//!
//! Profiles live on the feature node rather than in the arena. Entities are two
//! dimensional and [`crate::eval()`] is three dimensional, so giving them node
//! ids would introduce a kind of node that cannot be evaluated, and a 2D/3D
//! split running through the kernel. The cost of keeping them here is that one
//! profile cannot yet be shared between two features.

use glam::Vec2;

/// Points used to approximate a circle when one is needed as a polygon.
///
/// Only for outline drawing; the field itself is exact.
const CIRCLE_SEGMENTS: usize = 64;

/// Largest number of points a freehand path may have.
///
/// Path vertices are bound into the shader's parameter buffer and its loop
/// bound is a literal, so an unbounded path would generate unbounded code.
pub const MAX_PATH_POINTS: usize = 256;

/// A closed region in the sketch plane.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Profile {
    /// Axis-aligned rectangle centred on the sketch origin.
    Rect {
        /// Extent along the plane's first axis.
        width: f32,
        /// Extent along the plane's second axis.
        height: f32,
    },
    /// Circle centred on the sketch origin.
    Circle {
        /// Radius.
        radius: f32,
    },
    /// Regular polygon centred on the sketch origin, first vertex on +u.
    ///
    /// A hexagon is this with six sides, and `radius` is across the corners.
    RegularPolygon {
        /// Number of sides. At least three.
        sides: u32,
        /// Distance from the centre to a vertex.
        radius: f32,
    },
    /// An arbitrary closed polyline, as drawn.
    Path {
        /// Vertices in order. The closing edge is implied.
        points: Vec<Vec2>,
    },
}

impl Profile {
    /// Machine-readable tag, part of the agent-facing vocabulary.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Profile::Rect { .. } => "rectangle",
            Profile::Circle { .. } => "circle",
            Profile::RegularPolygon { .. } => "polygon",
            Profile::Path { .. } => "path",
        }
    }

    /// Exact signed distance in the sketch plane, negative inside.
    #[must_use]
    pub fn distance(&self, p: Vec2) -> f32 {
        match self {
            Profile::Rect { width, height } => {
                let q = p.abs() - Vec2::new(width * 0.5, height * 0.5);
                q.max(Vec2::ZERO).length() + q.max_element().min(0.0)
            }
            Profile::Circle { radius } => p.length() - radius,
            // A drawn path already is its polygon. Going through `polygon()`
            // would clone up to `MAX_PATH_POINTS` vertices onto the heap for
            // every sample of every voxel.
            Profile::Path { points } => crate::eval::sd_polygon(p, points),
            Profile::RegularPolygon { .. } => crate::eval::sd_polygon(p, &self.polygon()),
        }
    }

    /// The profile as a closed polygon.
    ///
    /// Exact for everything but a circle, which is sampled. Backs the field for
    /// the two kinds that have no closed form, and is what the outline drawing
    /// and the generated shaders walk.
    #[must_use]
    pub fn polygon(&self) -> Vec<Vec2> {
        match self {
            Profile::Rect { width, height } => {
                let (w, h) = (width * 0.5, height * 0.5);
                vec![
                    Vec2::new(-w, -h),
                    Vec2::new(w, -h),
                    Vec2::new(w, h),
                    Vec2::new(-w, h),
                ]
            }
            Profile::Circle { radius } => (0..CIRCLE_SEGMENTS)
                .map(|i| {
                    let a = i as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
                    Vec2::new(radius * a.cos(), radius * a.sin())
                })
                .collect(),
            Profile::RegularPolygon { sides, radius } => {
                let n = (*sides).max(3);
                (0..n)
                    .map(|i| {
                        let a = i as f32 / n as f32 * std::f32::consts::TAU;
                        Vec2::new(radius * a.cos(), radius * a.sin())
                    })
                    .collect()
            }
            Profile::Path { points } => points.clone(),
        }
    }

    /// Named dimensions, editable after the fact. This is the whole point.
    #[must_use]
    pub fn params(&self) -> Vec<(&'static str, f32)> {
        match *self {
            Profile::Rect { width, height } => vec![("width", width), ("height", height)],
            Profile::Circle { radius } => vec![("radius", radius)],
            Profile::RegularPolygon { sides, radius } => {
                vec![("sides", sides as f32), ("radius", radius)]
            }
            // A freehand path has no dimension worth naming; its points are
            // edited by dragging them.
            Profile::Path { .. } => Vec::new(),
        }
    }

    /// Sets a named dimension. False if the name does not apply.
    pub fn set_param(&mut self, name: &str, v: f32) -> bool {
        match self {
            Profile::Rect { width, height } => match name {
                "width" => *width = v,
                "height" => *height = v,
                _ => return false,
            },
            Profile::Circle { radius } if name == "radius" => *radius = v,
            Profile::RegularPolygon { sides, radius } => match name {
                // Rounded rather than truncated, so dragging through 5.9 gives
                // six sides rather than five.
                "sides" => *sides = v.round().clamp(3.0, 64.0) as u32,
                "radius" => *radius = v,
                _ => return false,
            },
            _ => return false,
        }
        true
    }

    /// Whether this profile encloses an area the kernel can work with.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        match self {
            Profile::Rect { width, height } => {
                width.is_finite() && height.is_finite() && *width > 0.0 && *height > 0.0
            }
            Profile::Circle { radius } => radius.is_finite() && *radius > 0.0,
            Profile::RegularPolygon { sides, radius } => {
                *sides >= 3 && *sides <= 64 && radius.is_finite() && *radius > 0.0
            }
            Profile::Path { points } => {
                points.len() >= 3
                    && points.len() <= MAX_PATH_POINTS
                    && points.iter().all(|p| p.is_finite())
                    && crate::node::polygon_area(points).abs() > 1.0e-6
            }
        }
    }

    /// The profile's extent in the sketch plane, as minimum and maximum.
    ///
    /// Returned as a box rather than half-extents: a drawn path need not be
    /// centred on the sketch origin, and assuming it is makes the model's bounds
    /// far larger than the part.
    #[must_use]
    pub fn bounds(&self) -> (Vec2, Vec2) {
        match self {
            Profile::Rect { width, height } => {
                let half = Vec2::new(width * 0.5, height * 0.5);
                (-half, half)
            }
            Profile::Circle { radius } | Profile::RegularPolygon { radius, .. } => {
                (Vec2::splat(-radius), Vec2::splat(*radius))
            }
            Profile::Path { points } => points.iter().fold(
                (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
                |(lo, hi), p| (lo.min(*p), hi.max(*p)),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32, what: &str) {
        assert!((a - b).abs() < 1.0e-3, "{what}: expected {b}, got {a}");
    }

    #[test]
    fn a_rectangle_is_a_width_and_a_height() {
        // The property that makes "set it to 60 by 40" mean something later.
        let mut r = Profile::Rect {
            width: 60.0,
            height: 40.0,
        };
        near(r.distance(Vec2::ZERO), -20.0, "centre");
        near(r.distance(Vec2::new(30.0, 0.0)), 0.0, "edge");
        near(r.distance(Vec2::new(35.0, 0.0)), 5.0, "outside");

        assert!(r.set_param("width", 80.0));
        near(r.distance(Vec2::new(40.0, 0.0)), 0.0, "edge after the edit");
        // Editing the width left the height alone.
        near(
            r.distance(Vec2::new(0.0, 20.0)),
            0.0,
            "height was disturbed",
        );
    }

    #[test]
    fn a_circle_is_exact_rather_than_faceted() {
        let c = Profile::Circle { radius: 10.0 };
        for angle in [0.0f32, 0.7, 2.1, 4.9] {
            let p = Vec2::new(angle.cos(), angle.sin()) * 15.0;
            near(c.distance(p), 5.0, "distance around the circle");
        }
    }

    #[test]
    fn a_hexagon_has_six_corners_at_its_radius() {
        let h = Profile::RegularPolygon {
            sides: 6,
            radius: 10.0,
        };
        assert_eq!(h.polygon().len(), 6);
        near(h.distance(Vec2::new(10.0, 0.0)), 0.0, "first vertex");
        // Across the flats is radius * cos(30).
        near(h.distance(Vec2::new(0.0, 10.0 * 0.866_025)), 0.0, "flat");
    }

    #[test]
    fn dimensions_survive_being_read_back() {
        for profile in [
            Profile::Rect {
                width: 12.0,
                height: 34.0,
            },
            Profile::Circle { radius: 7.0 },
            Profile::RegularPolygon {
                sides: 6,
                radius: 9.0,
            },
        ] {
            for (name, value) in profile.params() {
                let mut edited = profile.clone();
                assert!(edited.set_param(name, value), "{name} was rejected");
                assert_eq!(edited, profile, "{name} round trip changed the profile");
            }
        }
    }

    #[test]
    fn side_counts_round_rather_than_truncate() {
        let mut p = Profile::RegularPolygon {
            sides: 6,
            radius: 5.0,
        };
        p.set_param("sides", 5.9);
        assert_eq!(
            p,
            Profile::RegularPolygon {
                sides: 6,
                radius: 5.0
            }
        );
        p.set_param("sides", 1.0);
        assert_eq!(
            p,
            Profile::RegularPolygon {
                sides: 3,
                radius: 5.0
            },
            "clamped to a triangle"
        );
    }

    #[test]
    fn a_path_is_bounded_where_it_actually_is() {
        // An off-centre path must not report bounds mirrored about the origin.
        let path = Profile::Path {
            points: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(10.0, 0.0),
                Vec2::new(10.0, 20.0),
            ],
        };
        let (lo, hi) = path.bounds();
        assert_eq!(lo, Vec2::new(0.0, 0.0));
        assert_eq!(hi, Vec2::new(10.0, 20.0));
    }

    /// A path is drawn by hand, so it arrives with whatever the pointer gave it.
    /// A vertex clicked twice is a zero length edge, and an edge with no
    /// direction must not be allowed to move the surface or flip the inside.
    #[test]
    fn a_repeated_point_does_not_change_the_region() {
        let square = |points: Vec<Vec2>| Profile::Path { points };
        let clean = square(vec![
            Vec2::ZERO,
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
        ]);
        let stuttered = square(vec![
            Vec2::ZERO,
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
        ]);
        assert!(stuttered.is_valid(), "a stutter is not a reason to refuse");
        for p in [
            Vec2::new(5.0, 5.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(13.0, 0.0),
            Vec2::new(-2.0, 5.0),
            Vec2::new(9.5, 0.5),
        ] {
            near(
                stuttered.distance(p),
                clean.distance(p),
                "the stutter moved the field",
            );
        }
    }

    /// A path that crosses itself and comes back encloses nothing: the two lobes
    /// wind opposite ways, so there is no inside for the field to have.
    #[test]
    fn a_path_that_crosses_itself_and_cancels_is_refused() {
        let bowtie = Profile::Path {
            points: vec![
                Vec2::ZERO,
                Vec2::new(10.0, 10.0),
                Vec2::new(10.0, 0.0),
                Vec2::new(0.0, 10.0),
            ],
        };
        assert!(!bowtie.is_valid());
    }

    #[test]
    fn a_polygon_needs_three_sides_to_enclose_anything() {
        for sides in [0, 1, 2] {
            assert!(
                !Profile::RegularPolygon { sides, radius: 5.0 }.is_valid(),
                "{sides} sides"
            );
        }
        assert!(Profile::RegularPolygon {
            sides: 3,
            radius: 5.0
        }
        .is_valid());
        // Sixty-four is the cap `set_param` clamps a drag to, and validation
        // has to agree with it or an agent could set what a drag cannot.
        assert!(!Profile::RegularPolygon {
            sides: 65,
            radius: 5.0
        }
        .is_valid());
    }

    #[test]
    fn a_polygon_is_bounded_by_its_circumradius() {
        let p = Profile::RegularPolygon {
            sides: 5,
            radius: 8.0,
        };
        let (lo, hi) = p.bounds();
        assert_eq!((lo, hi), (Vec2::splat(-8.0), Vec2::splat(8.0)));
        for v in p.polygon() {
            assert!(v.length() <= 8.0 + 1.0e-4, "{v:?} is outside the bound");
            assert!(p.distance(v).abs() < 1.0e-3, "{v:?} is not on the outline");
        }
    }

    #[test]
    fn a_path_keeps_the_points_it_was_drawn_with() {
        let points = vec![Vec2::ZERO, Vec2::new(4.0, 0.0), Vec2::new(0.0, 3.0)];
        let path = Profile::Path {
            points: points.clone(),
        };
        assert_eq!(path.polygon(), points);
        assert!(path.params().is_empty(), "a drawn path has no dimensions");
        assert!(!path.clone().set_param("width", 5.0), "and none to set");
        // Inside the triangle, and the nearest edge is the hypotenuse.
        assert!(path.distance(Vec2::new(0.5, 0.5)) < 0.0);
        assert!(path.distance(Vec2::new(4.0, 4.0)) > 0.0);
    }

    #[test]
    fn degenerate_profiles_are_rejected() {
        assert!(!Profile::Rect {
            width: 0.0,
            height: 10.0
        }
        .is_valid());
        assert!(!Profile::Circle { radius: -1.0 }.is_valid());
        assert!(!Profile::RegularPolygon {
            sides: 2,
            radius: 5.0
        }
        .is_valid());
        assert!(!Profile::Path {
            points: vec![Vec2::ZERO, Vec2::X]
        }
        .is_valid());
    }
}
