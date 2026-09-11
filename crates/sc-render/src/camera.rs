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

/// How close to straight up or down the camera may tilt. Stopping short avoids
/// the degenerate case where the view direction is parallel to world up and the
/// right vector is undefined.
const PITCH_LIMIT: f32 = 1.553; // ~89 degrees

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
    #[must_use]
    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let forward = -self.direction();
        let right = forward.cross(Vec3::Z).normalize();
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
        self.distance = (self.distance * factor).clamp(0.01, 1.0e6);
    }

    /// Zooms while keeping `anchor` fixed on screen.
    ///
    /// Scaling the target's offset from the anchor by the same factor as the
    /// distance leaves the anchor on the same ray from the eye, so whatever is
    /// under the pointer stays under it. This is the single thing that makes a
    /// wheel feel like it is pulling you toward what you are looking at rather
    /// than toward wherever the camera happens to be aimed.
    pub fn zoom_towards(&mut self, factor: f32, anchor: Vec3) {
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
        let dir = (forward + right * ndc.x * tan_half * aspect + up * ndc.y * tan_half).normalize();
        (self.eye(), dir)
    }

    /// Where a world point lands in normalised device coordinates.
    ///
    /// `None` when the point is behind the camera, where a projection would
    /// otherwise fold it back into view.
    #[must_use]
    pub fn project(&self, world: Vec3, aspect: f32) -> Option<Vec2> {
        let (right, up, forward) = self.basis();
        let v = world - self.eye();
        let depth = v.dot(forward);
        if depth <= 1.0e-4 {
            return None;
        }
        let tan_half = (self.fov_y * 0.5).tan();
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
        let (origin, dir) = self.ray(ndc, aspect);
        if dir.z.abs() < 1.0e-6 {
            return None;
        }
        let t = -origin.z / dir.z;
        (t > 0.0).then(|| origin + dir * t)
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
    /// hundred millimetres out it turns a 0.1mm threshold into several
    /// millimetres, which rounds off every edge in the scene.
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

        let moving = (c.target - g.target).length() > g.distance * 1.0e-4
            || (c.distance - g.distance).abs() > g.distance * 1.0e-4
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
