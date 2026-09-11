//! Orbit camera.
//!
//! Z-up, matching the modelling convention: primitives such as a cylinder run
//! along +Z, and a printer's build plate is the XY plane.

use sc_geom::glam::{Vec2, Vec3};
use sc_geom::Aabb;

/// A camera that rotates about a target point.
#[derive(Clone, Copy, Debug)]
pub struct OrbitCamera {
    /// The point being orbited.
    pub target: Vec3,
    /// Distance from `target` to the eye.
    pub distance: f32,
    /// Rotation about the world Z axis, in radians.
    pub yaw: f32,
    /// Elevation above the XY plane, in radians. Clamped short of vertical.
    pub pitch: f32,
    /// Vertical field of view, in radians.
    pub fov_y: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 100.0,
            yaw: 0.7,
            pitch: 0.5,
            fov_y: 0.8,
        }
    }
}

/// How close to straight up or down the camera may tilt. Stopping short keeps
/// the view direction away from world up, where the pitch stops distinguishing
/// one orientation from another.
const PITCH_LIMIT: f32 = 1.553; // ~89 degrees

/// Closest the eye may sit to the target.
///
/// Zero is the value that has to be excluded: [`OrbitCamera::zoom_towards`]
/// divides by the distance it started from, and a zero there makes the applied
/// ratio infinite and leaves the target `NaN`, which nothing downstream
/// recovers from.
const MIN_DISTANCE: f32 = 0.01;

/// Furthest the eye may sit from the target.
const MAX_DISTANCE: f32 = 1.0e6;

/// Smallest aspect ratio the projection will work with.
///
/// A viewport squeezed to nothing reports width / height as zero, and dividing
/// by that in [`OrbitCamera::project`] gives infinity, or `NaN` for a point on
/// the view axis. Callers read `Some` as "on screen" and place a handle at the
/// result, so a `NaN` there is drawn rather than discarded.
const MIN_ASPECT: f32 = 1.0e-6;

/// Brings a distance back into range, `NaN` included.
fn sane_distance(distance: f32) -> f32 {
    if distance.is_nan() {
        MIN_DISTANCE
    } else {
        distance.clamp(MIN_DISTANCE, MAX_DISTANCE)
    }
}

/// Floors an aspect ratio away from zero.
///
/// [`OrbitCamera::ray`] and [`OrbitCamera::project`] must apply this identically
/// or they stop being exact inverses of one another. `f32::max` yields the other
/// operand for `NaN`, so this also floors a `NaN` aspect.
fn sane_aspect(aspect: f32) -> f32 {
    aspect.max(MIN_ASPECT)
}

impl OrbitCamera {
    /// Frames a model so it comfortably fills the view.
    #[must_use]
    pub fn framing(bounds: Aabb) -> Self {
        let b = bounds.finite_or(1000.0);
        let radius = (b.size().length() * 0.5).max(1.0);
        let fov_y: f32 = 0.8;
        Self {
            target: b.center(),
            // `radius / sin(half_fov)` is the distance at which the bounding
            // sphere exactly fills the frustum; the margin keeps the part off
            // the edges, and covers the viewport being narrower than the window
            // once the side panels take their share.
            distance: radius / (fov_y * 0.5).sin() * 1.2,
            yaw: 0.9,
            pitch: 0.45,
            fov_y,
        }
    }

    /// Unit vector from the target toward the eye.
    #[must_use]
    pub fn direction(&self) -> Vec3 {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        Vec3::new(cp * cy, cp * sy, sp)
    }

    /// Eye position in world space.
    #[must_use]
    pub fn eye(&self) -> Vec3 {
        self.target + self.direction() * self.distance
    }

    /// Orthonormal view basis as `(right, up, forward)`.
    ///
    /// `right` comes from the yaw alone, which is what it reduces to: the
    /// normalised `forward × Z` is exactly `(-sin yaw, cos yaw, 0)` for any
    /// pitch short of vertical. Taking the cross product instead shrinks toward
    /// the zero vector as the pitch approaches the pole and then normalises
    /// float noise, which flips the whole frame through 180 degrees within a
    /// hair of the limit instead of degrading smoothly.
    #[must_use]
    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let forward = -self.direction();
        let (sy, cy) = self.yaw.sin_cos();
        let right = Vec3::new(-sy, cy, 0.0);
        let up = right.cross(forward).normalize();
        (right, up, forward)
    }

    /// Rotates the camera. Inputs are in radians.
    pub fn orbit(&mut self, delta_yaw: f32, delta_pitch: f32) {
        self.yaw -= delta_yaw;
        self.pitch = (self.pitch + delta_pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Rotates by a pointer movement in pixels.
    ///
    /// Tied to the viewport rather than to a fixed radians-per-pixel constant,
    /// so the same physical drag turns the model by the same amount whatever the
    /// window size or display density. A drag across the full width is half a
    /// turn.
    pub fn orbit_pixels(&mut self, dx: f32, dy: f32, viewport: Vec2) {
        let w = viewport.x.max(1.0);
        let h = viewport.y.max(1.0);
        self.orbit(
            dx / w * std::f32::consts::PI,
            dy / h * std::f32::consts::PI * 0.5,
        );
    }

    /// Moves the eye toward or away from the target. `factor` is multiplicative,
    /// so zooming feels the same at every scale.
    pub fn zoom(&mut self, factor: f32) {
        self.distance = sane_distance(self.distance * factor);
    }

    /// Zooms while keeping `anchor` fixed on screen.
    ///
    /// Scaling the target's offset from the anchor by the same factor as the
    /// distance leaves the anchor on the same ray from the eye, so whatever is
    /// under the pointer stays under it. This is the single thing that makes a
    /// wheel feel like it is pulling you toward what you are looking at rather
    /// than toward wherever the camera happens to be aimed.
    pub fn zoom_towards(&mut self, factor: f32, anchor: Vec3) {
        // Bring the distance into range before reading it. Dividing by a
        // distance of zero further down makes `applied` infinite, and the
        // target comes out `NaN` with no way back.
        self.distance = sane_distance(self.distance);
        let before = self.distance;
        self.zoom(factor);
        // Use the factor actually applied, in case the distance clamped.
        let applied = self.distance / before;
        self.target = anchor + (self.target - anchor) * applied;
    }

    /// World units covered by one screen pixel at the target's depth.
    #[must_use]
    pub fn world_per_pixel(&self, viewport_height: f32) -> f32 {
        2.0 * (self.fov_y * 0.5).tan() * self.distance / viewport_height.max(1.0)
    }

    /// Slides the target across the view plane by a pointer movement in pixels.
    ///
    /// Exactly 1:1 at the target's depth: the point you grabbed stays under the
    /// cursor instead of drifting, which is most of what makes panning feel
    /// direct rather than vague.
    pub fn pan_pixels(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let (right, up, _) = self.basis();
        let scale = self.world_per_pixel(viewport_height);
        self.target += (-right * dx + up * dy) * scale;
    }

    /// A world-space ray through a point in normalised device coordinates,
    /// where x and y run from -1 to 1 and y points up.
    ///
    /// This is the same construction the shader performs, kept in step so that
    /// what the user clicks is what they see.
    #[must_use]
    pub fn ray(&self, ndc: Vec2, aspect: f32) -> (Vec3, Vec3) {
        let (right, up, forward) = self.basis();
        let tan_half = (self.fov_y * 0.5).tan();
        let aspect = sane_aspect(aspect);
        let dir = (forward + right * ndc.x * tan_half * aspect + up * ndc.y * tan_half).normalize();
        (self.eye(), dir)
    }

    /// Where a world point lands in normalised device coordinates.
    ///
    /// `None` when the point is behind the camera, where a projection would
    /// otherwise fold it back into view.
    ///
    /// The exact inverse of [`OrbitCamera::ray`] for the same aspect, which is
    /// what lets a handle be drawn and hit-tested through one projection.
    #[must_use]
    pub fn project(&self, world: Vec3, aspect: f32) -> Option<Vec2> {
        let (right, up, forward) = self.basis();
        let v = world - self.eye();
        let depth = v.dot(forward);
        if depth <= 1.0e-4 {
            return None;
        }
        let tan_half = (self.fov_y * 0.5).tan();
        let aspect = sane_aspect(aspect);
        Some(Vec2::new(
            v.dot(right) / (depth * tan_half * aspect),
            v.dot(up) / (depth * tan_half),
        ))
    }

    /// Where a ray through `ndc` meets the build plate at z = 0.
    ///
    /// `None` when the ray is parallel to the plate or points away from it,
    /// which is what happens when the user clicks the sky.
    #[must_use]
    pub fn plate_hit(&self, ndc: Vec2, aspect: f32) -> Option<Vec3> {
        self.plane_hit(ndc, aspect, Vec3::ZERO, Vec3::Z)
    }

    /// Where a ray through `ndc` meets an arbitrary plane.
    ///
    /// `None` when the ray runs parallel to the plane or meets it behind the
    /// camera. Sketching on a datum plane is this, with the plane's own normal.
    #[must_use]
    pub fn plane_hit(&self, ndc: Vec2, aspect: f32, origin: Vec3, normal: Vec3) -> Option<Vec3> {
        let (eye, dir) = self.ray(ndc, aspect);
        // Normalised so the parallel test is about the geometry rather than
        // about how long the caller's normal happens to be: scaling a normal
        // scales `denom` without changing the answer, so an unnormalised one
        // reads as parallel when it is nothing of the kind. A zero normal
        // normalises to zero and is rejected, which is right for a plane that
        // does not have one.
        let normal = normal.normalize_or_zero();
        let denom = dir.dot(normal);
        if denom.abs() < 1.0e-6 {
            return None;
        }
        let t = (origin - eye).dot(normal) / denom;
        (t > 0.0).then(|| eye + dir * t)
    }

    /// Points the camera straight down `normal`, keeping its distance.
    ///
    /// Used when a sketch begins: drawing in two dimensions only makes sense if
    /// the plane is facing you.
    pub fn look_along(&mut self, normal: Vec3) {
        let dir = normal.normalize_or_zero();
        if dir == Vec3::ZERO {
            return;
        }
        self.pitch = dir
            .z
            .clamp(-1.0, 1.0)
            .asin()
            .clamp(-PITCH_LIMIT, PITCH_LIMIT);
        // A normal along Z leaves the yaw undetermined; keep the current one
        // rather than snapping to an arbitrary direction.
        if dir.x.abs() > 1.0e-4 || dir.y.abs() > 1.0e-4 {
            self.yaw = dir.y.atan2(dir.x);
        }
    }

    /// How far a ray may travel before being treated as a miss.
    #[must_use]
    pub fn far(&self) -> f32 {
        self.distance * 20.0
    }

    /// Angular size of one pixel, in radians, for a viewport `height` pixels tall.
    ///
    /// Tracing tolerances are derived from this rather than from the camera
    /// distance. A surface is "hit" once the ray is within about half a pixel of
    /// it, which is the point past which more precision cannot change the image.
    /// Scaling a tolerance by ray distance *as well* over-blurs badly: at a
    /// hundred millimetres out it turns a 0.02mm threshold into millimetres,
    /// which rounds off every edge in the scene.
    #[must_use]
    pub fn pixel_angle(&self, height: u32) -> f32 {
        2.0 * (self.fov_y * 0.5).tan() / height.max(1) as f32
    }
}

/// A camera that eases toward where it is being sent.
///
/// Input moves the goal; the visible camera chases it. Without this, every
/// gesture lands as a hard cut, which reads as clunky even when the mapping is
/// correct.
#[derive(Clone, Copy, Debug, Default)]
pub struct CameraRig {
    /// What the viewport draws.
    pub current: OrbitCamera,
    /// What input manipulates.
    pub goal: OrbitCamera,
}

/// Fraction of the remaining distance closed per second.
///
/// High enough that dragging feels attached to the pointer rather than elastic,
/// low enough to take the edge off a wheel click.
const SMOOTHING_RATE: f32 = 26.0;

impl CameraRig {
    /// A rig sitting still at `camera`.
    #[must_use]
    pub fn new(camera: OrbitCamera) -> Self {
        Self {
            current: camera,
            goal: camera,
        }
    }

    /// Jumps both to `camera`, with no easing.
    pub fn snap_to(&mut self, camera: OrbitCamera) {
        self.current = camera;
        self.goal = camera;
    }

    /// Eases toward the goal. Returns true while still moving, so the caller
    /// knows to ask for another frame.
    pub fn advance(&mut self, dt: f32) -> bool {
        // Exponential decay, framed so the result does not depend on frame rate.
        let t = 1.0 - (-SMOOTHING_RATE * dt.clamp(0.0, 0.1)).exp();

        let c = &mut self.current;
        let g = self.goal;
        c.target += (g.target - c.target) * t;
        c.distance += (g.distance - c.distance) * t;
        c.yaw += (g.yaw - c.yaw) * t;
        c.pitch += (g.pitch - c.pitch) * t;
        c.fov_y = g.fov_y;

        // Positional thresholds scale with the distance, because that is the
        // size the camera is working at. The floor matters: a goal distance of
        // zero would make both of them zero, nothing would ever be close
        // enough, and the rig would ask for another frame for ever.
        let scale = sane_distance(g.distance) * 1.0e-4;
        let moving = (c.target - g.target).length() > scale
            || (c.distance - g.distance).abs() > scale
            || (c.yaw - g.yaw).abs() > 1.0e-4
            || (c.pitch - g.pitch).abs() > 1.0e-4;
        if !moving {
            // Settle exactly, so the shader is not fed a drifting camera.
            *c = g;
        }
        moving
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_geom::glam::Vec2;

    #[test]
    fn projection_inverts_the_ray_construction() {
        let cam = OrbitCamera {
            distance: 80.0,
            ..OrbitCamera::default()
        };
        let aspect = 1.6;
        for ndc in [
            Vec2::ZERO,
            Vec2::new(0.5, -0.25),
            Vec2::new(-0.8, 0.7),
            Vec2::new(1.0, 1.0),
        ] {
            let (origin, dir) = cam.ray(ndc, aspect);
            let point = origin + dir * 50.0;
            let back = cam.project(point, aspect).expect("in front of the camera");
            assert!(
                (back - ndc).length() < 1.0e-3,
                "{ndc:?} projected back as {back:?}"
            );
        }
    }

    /// Cameras worth putting through the projection, each awkward in its own
    /// way: at the pitch limit either side, wound round past a full turn, at
    /// both ends of the distance clamp, and with a field of view narrow enough
    /// and wide enough to strain the tangent.
    fn awkward_cameras() -> Vec<(&'static str, OrbitCamera)> {
        vec![
            ("default", OrbitCamera::default()),
            (
                "at the pitch limit",
                OrbitCamera {
                    pitch: PITCH_LIMIT,
                    ..OrbitCamera::default()
                },
            ),
            (
                "at the lower pitch limit",
                OrbitCamera {
                    pitch: -PITCH_LIMIT,
                    ..OrbitCamera::default()
                },
            ),
            (
                "wound past a full turn",
                OrbitCamera {
                    yaw: 13.7,
                    pitch: -1.2,
                    ..OrbitCamera::default()
                },
            ),
            (
                "as close as it goes",
                OrbitCamera {
                    distance: MIN_DISTANCE,
                    ..OrbitCamera::default()
                },
            ),
            (
                "as far as it goes",
                OrbitCamera {
                    distance: MAX_DISTANCE,
                    ..OrbitCamera::default()
                },
            ),
            (
                "a narrow field of view",
                OrbitCamera {
                    fov_y: 0.02,
                    ..OrbitCamera::default()
                },
            ),
            (
                "a wide field of view",
                OrbitCamera {
                    fov_y: 2.8,
                    ..OrbitCamera::default()
                },
            ),
        ]
    }

    #[test]
    fn projection_inverts_the_ray_construction_in_the_awkward_cases() {
        // Portrait through to letterbox, at the corners where the error is
        // largest, on every camera that is awkward for a different reason.
        for (name, cam) in awkward_cameras() {
            for aspect in [0.05f32, 0.2, 1.0, 4.0, 40.0] {
                for ndc in [
                    Vec2::ZERO,
                    Vec2::new(1.0, 1.0),
                    Vec2::new(-1.0, -1.0),
                    Vec2::new(0.73, -0.91),
                ] {
                    let (origin, dir) = cam.ray(ndc, aspect);
                    let point = origin + dir * cam.distance;
                    let back = cam
                        .project(point, aspect)
                        .expect("a point at the target's depth is in front");
                    assert!(
                        (back - ndc).length() < 1.0e-3,
                        "{name} at aspect {aspect}: {ndc:?} projected back as {back:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_collapsed_viewport_does_not_project_to_nan() {
        // A viewport with no width reports an aspect of zero, and the division
        // in `project` then returns infinity, or NaN for a point sitting on the
        // view axis. Callers read `Some` as "this is on screen" and draw a
        // handle at the result, so a NaN there is painted rather than skipped.
        let cam = OrbitCamera::default();
        for aspect in [0.0f32, -2.0, f32::NAN] {
            for world in [
                cam.target,
                cam.target + Vec3::new(1.0, 2.0, 3.0),
                cam.target + Vec3::new(-7.0, 0.5, -2.0),
            ] {
                let ndc = cam.project(world, aspect).expect("in front of the camera");
                assert!(
                    ndc.is_finite(),
                    "aspect {aspect} projected {world:?} to {ndc:?}"
                );
                let (_, dir) = cam.ray(ndc.clamp(Vec2::splat(-1.0), Vec2::ONE), aspect);
                assert!(dir.is_finite(), "aspect {aspect} gave the ray {dir:?}");
            }
        }
    }

    #[test]
    fn the_basis_does_not_flip_at_the_pole() {
        // `forward.cross(Z)` collapses here, and normalising what is left of it
        // turns the view upside down within a thousandth of a radian of the
        // limit the orbit already allows.
        let at_limit = OrbitCamera {
            pitch: PITCH_LIMIT,
            ..OrbitCamera::default()
        };
        let at_pole = OrbitCamera {
            pitch: std::f32::consts::FRAC_PI_2,
            ..OrbitCamera::default()
        };
        let (r0, u0, _) = at_limit.basis();
        let (r1, u1, _) = at_pole.basis();
        assert!(
            r0.dot(r1) > 0.999,
            "right flipped from {r0:?} to {r1:?} across the pole"
        );
        assert!(
            u0.dot(u1) > 0.999,
            "up flipped from {u0:?} to {u1:?} across the pole"
        );
    }

    #[test]
    fn the_basis_stays_orthonormal_everywhere() {
        for (name, cam) in awkward_cameras() {
            for pitch in [
                -std::f32::consts::FRAC_PI_2,
                0.0,
                std::f32::consts::FRAC_PI_2,
            ] {
                let cam = OrbitCamera { pitch, ..cam };
                let (r, u, f) = cam.basis();
                for (label, v) in [("right", r), ("up", u), ("forward", f)] {
                    assert!(
                        (v.length() - 1.0).abs() < 1.0e-4,
                        "{name} at pitch {pitch}: {label} is {v:?}"
                    );
                }
                assert!(r.dot(u).abs() < 1.0e-4, "{name} at pitch {pitch}: r.u");
                assert!(r.dot(f).abs() < 1.0e-4, "{name} at pitch {pitch}: r.f");
                assert!(u.dot(f).abs() < 1.0e-4, "{name} at pitch {pitch}: u.f");
            }
        }
    }

    #[test]
    fn zooming_from_a_zero_distance_leaves_a_usable_camera() {
        // The distance is a public field, so nothing stops a caller arriving
        // here with a zero in it. Dividing by it used to leave the target NaN,
        // which then spreads to the eye, the basis and every projection.
        let mut cam = OrbitCamera {
            distance: 0.0,
            ..OrbitCamera::default()
        };
        cam.zoom_towards(2.0, Vec3::new(5.0, -3.0, 1.0));
        assert!(
            cam.target.is_finite() && cam.distance.is_finite(),
            "target {:?} distance {}",
            cam.target,
            cam.distance
        );
        assert!(cam.distance >= MIN_DISTANCE);
        assert!(cam.eye().is_finite());
    }

    #[test]
    fn a_plane_hit_does_not_depend_on_the_normal_s_length() {
        let cam = OrbitCamera {
            target: Vec3::ZERO,
            ..OrbitCamera::default()
        };
        let ndc = Vec2::new(0.2, -0.4);
        let reference = cam
            .plane_hit(ndc, 1.5, Vec3::ZERO, Vec3::Z)
            .expect("the plate is in view");
        for scale in [1.0e-7f32, 1.0e-3, 1.0, 1.0e3] {
            let hit = cam
                .plane_hit(ndc, 1.5, Vec3::ZERO, Vec3::Z * scale)
                .unwrap_or_else(|| {
                    panic!("a normal scaled by {scale} is not parallel to anything")
                });
            assert!(
                (hit - reference).length() < 1.0e-2,
                "normal scaled by {scale} hit {hit:?} rather than {reference:?}"
            );
        }
        assert!(
            cam.plane_hit(ndc, 1.5, Vec3::ZERO, Vec3::ZERO).is_none(),
            "a plane with no normal has no intersection"
        );
    }

    #[test]
    fn a_point_behind_the_camera_does_not_project() {
        let cam = OrbitCamera::default();
        let behind = cam.eye() + (cam.eye() - cam.target).normalize() * 10.0;
        assert!(cam.project(behind, 1.0).is_none());
    }

    #[test]
    fn the_centre_ray_meets_the_plate_under_the_target() {
        // Looking at the origin, the middle of the screen should land on it.
        let cam = OrbitCamera {
            target: Vec3::ZERO,
            ..OrbitCamera::default()
        };
        let hit = cam.plate_hit(Vec2::ZERO, 1.5).expect("plate is in view");
        assert!(hit.length() < 1.0e-2, "centre ray hit the plate at {hit:?}");
        assert!(hit.z.abs() < 1.0e-4, "not on the plate: {hit:?}");
    }

    #[test]
    fn zooming_keeps_the_anchor_under_the_pointer() {
        let mut cam = OrbitCamera {
            distance: 100.0,
            ..OrbitCamera::default()
        };
        let aspect = 1.5;
        let ndc = Vec2::new(0.4, -0.3);
        let anchor = cam.plate_hit(ndc, aspect).expect("plate in view");

        for factor in [0.5f32, 0.8, 1.25, 2.0] {
            let mut zoomed = cam;
            zoomed.zoom_towards(factor, anchor);
            let after = zoomed.project(anchor, aspect).expect("still in front");
            assert!(
                (after - ndc).length() < 1.0e-3,
                "anchor moved from {ndc:?} to {after:?} at factor {factor}"
            );
        }
        cam.zoom_towards(0.5, anchor);
        assert!((cam.distance - 50.0).abs() < 1.0e-3);
    }

    #[test]
    fn panning_moves_the_scene_one_to_one_with_the_pointer() {
        let mut cam = OrbitCamera {
            distance: 120.0,
            ..OrbitCamera::default()
        };
        let (aspect, height) = (1.6, 800.0);
        let point = cam.target;
        let before = cam.project(point, aspect).expect("in view");

        cam.pan_pixels(80.0, -40.0, height);
        let after = cam.project(point, aspect).expect("in view");

        // 80px right and 40px up, expressed in NDC for this viewport.
        let expected_x = before.x + 80.0 / (height * aspect * 0.5);
        let expected_y = before.y + 40.0 / (height * 0.5);
        assert!(
            (after.x - expected_x).abs() < 2.0e-3,
            "x: {after:?} vs {expected_x}"
        );
        assert!(
            (after.y - expected_y).abs() < 2.0e-3,
            "y: {after:?} vs {expected_y}"
        );
    }

    #[test]
    fn orbiting_is_independent_of_viewport_size() {
        // The same fraction of the window should turn the model equally.
        let mut small = OrbitCamera::default();
        let mut large = OrbitCamera::default();
        small.orbit_pixels(200.0, 0.0, Vec2::new(800.0, 600.0));
        large.orbit_pixels(400.0, 0.0, Vec2::new(1600.0, 1200.0));
        assert!((small.yaw - large.yaw).abs() < 1.0e-5);
    }

    #[test]
    fn the_rig_settles_exactly_on_its_goal() {
        let mut rig = CameraRig::new(OrbitCamera::default());
        rig.goal.distance = 250.0;
        rig.goal.yaw = 2.0;

        let mut frames = 0;
        while rig.advance(1.0 / 60.0) {
            frames += 1;
            assert!(frames < 600, "the rig never settled");
        }
        assert!((rig.current.distance - 250.0).abs() < f32::EPSILON);
        assert!((rig.current.yaw - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_rig_settles_on_a_goal_at_zero_distance() {
        // Every positional threshold is a fraction of the goal distance, so a
        // zero there made nothing ever close enough and the rig asked for
        // another frame for ever.
        let mut rig = CameraRig::new(OrbitCamera::default());
        rig.goal.distance = 0.0;
        let mut frames = 0;
        while rig.advance(1.0 / 60.0) {
            frames += 1;
            assert!(frames < 600, "the rig never settled");
        }
        assert!(rig.current.distance.abs() < f32::EPSILON);
    }

    #[test]
    fn a_ray_meets_an_arbitrary_plane() {
        let cam = OrbitCamera {
            target: Vec3::ZERO,
            ..OrbitCamera::default()
        };
        // The YZ plane through the origin, seen from the default viewpoint.
        let hit = cam
            .plane_hit(Vec2::ZERO, 1.5, Vec3::ZERO, Vec3::X)
            .expect("plane is in view");
        assert!(hit.x.abs() < 1.0e-3, "not on the plane: {hit:?}");
    }

    #[test]
    fn looking_along_a_normal_faces_the_plane() {
        let mut cam = OrbitCamera {
            target: Vec3::ZERO,
            ..OrbitCamera::default()
        };
        for normal in [Vec3::X, Vec3::Y, -Vec3::X, -Vec3::Y] {
            cam.look_along(normal);
            // The eye should sit on the normal, so the plane is face-on.
            let toward_eye = (cam.eye() - cam.target).normalize();
            assert!(
                toward_eye.dot(normal) > 0.999,
                "looking along {normal:?} put the eye at {toward_eye:?}"
            );
        }
    }

    #[test]
    fn a_ray_into_the_sky_misses_the_plate() {
        // Eye above the plate and nearly level with it, looking at the top of
        // the screen: the ray tilts upward and never comes back down.
        let cam = OrbitCamera {
            pitch: 0.1,
            ..OrbitCamera::default()
        };
        assert!(cam.eye().z > 0.0, "eye should be above the plate");
        assert!(cam.plate_hit(Vec2::new(0.0, 1.0), 1.5).is_none());
    }
}
