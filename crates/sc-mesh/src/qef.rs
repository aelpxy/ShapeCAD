//! Quadratic error function solving.
//!
//! Dual contouring places one vertex per cell. Where that vertex goes is the
//! whole game: at the centroid of the surface crossings you get rounded-off
//! corners, but at the point that best satisfies the tangent planes of those
//! crossings you get a sharp corner reproduced exactly. The latter is what makes
//! a milled-looking box come out of a mesher rather than a soap bar.

use sc_geom::glam::{Mat3, Vec3};

/// Accumulates plane constraints and solves for the best-fitting point.
///
/// Note the hand-written [`Default`]: deriving it would initialise `ata` with
/// `Mat3::default()`, which in glam is the *identity*, not zero. That seeds every
/// cell with a phantom unit constraint pulling its vertex toward the origin, and
/// the resulting meshes come out uniformly undersized, a bias small enough to
/// look plausible in a render and quite wrong in a printed part.
#[derive(Clone, Copy, Debug)]
pub struct Qef {
    /// Normal equations: sum of n nᵀ.
    ata: Mat3,
    /// Right-hand side: sum of n (n · p).
    atb: Vec3,
    /// Sum of constraint points, for the fallback and the regularisation target.
    mass: Vec3,
    count: u32,
}

/// Pull toward the mass point. Without it the normal equations are singular
/// whenever the constraints are degenerate (a flat face, or an edge, where the
/// surface does not pin down all three axes) and the solution runs off to
/// infinity.
const REGULARISATION: f32 = 1.0e-3;

impl Default for Qef {
    fn default() -> Self {
        Self::new()
    }
}

impl Qef {
    /// A solver with no constraints.
    #[must_use]
    pub fn new() -> Self {
        Self {
            ata: Mat3::ZERO,
            atb: Vec3::ZERO,
            mass: Vec3::ZERO,
            count: 0,
        }
    }

    /// Adds the tangent plane at a surface crossing.
    pub fn add(&mut self, position: Vec3, normal: Vec3) {
        let n = normal.normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        self.ata += Mat3::from_cols(n * n.x, n * n.y, n * n.z);
        self.atb += n * n.dot(position);
        self.mass += position;
        self.count += 1;
    }

    /// Number of constraints added.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The average of the crossing points.
    #[must_use]
    pub fn mass_point(&self) -> Vec3 {
        if self.count == 0 {
            Vec3::ZERO
        } else {
            self.mass / self.count as f32
        }
    }

    /// Solves for the vertex, clamped into `min..max`.
    ///
    /// Clamping is not cosmetic. An ill-conditioned system can put the minimiser
    /// far outside its own cell, which produces long spikes and self-intersecting
    /// triangles, geometry a slicer will reject.
    #[must_use]
    pub fn solve(&self, min: Vec3, max: Vec3) -> Vec3 {
        let mass = self.mass_point();
        if self.count == 0 {
            return mass;
        }

        let a = self.ata + Mat3::IDENTITY * REGULARISATION;
        let b = self.atb + mass * REGULARISATION;

        // A near-zero determinant means the constraints do not pin down all
        // three axes; fall back to the centroid rather than amplifying noise.
        let solved = if a.determinant().abs() > 1.0e-8 {
            a.inverse() * b
        } else {
            mass
        };

        if solved.is_finite() {
            solved.clamp(min, max)
        } else {
            mass.clamp(min, max)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_solver_accumulates_from_zero() {
        // Guards the glam footgun: `Mat3::default()` is the identity.
        assert_eq!(Qef::new().ata, Mat3::ZERO);
    }

    #[test]
    fn a_single_plane_puts_the_vertex_on_that_plane() {
        let mut q = Qef::new();
        for p in [
            Vec3::new(-10.0, 0.0, 0.0),
            Vec3::new(-10.0, 2.5, 0.0),
            Vec3::new(-10.0, 0.0, 2.5),
            Vec3::new(-10.0, 2.5, 2.5),
        ] {
            q.add(p, Vec3::new(-1.0, 0.0, 0.0));
        }
        let v = q.solve(Vec3::new(-10.0, 0.0, 0.0), Vec3::new(-7.5, 2.5, 2.5));
        assert!((v.x + 10.0).abs() < 1.0e-3, "vertex left the plane: {v:?}");
        // Unconstrained axes fall back to the centroid of the crossings.
        assert!((v.y - 1.25).abs() < 1.0e-3, "{v:?}");
    }

    #[test]
    fn three_planes_reconstruct_a_sharp_corner() {
        let mut q = Qef::new();
        let corner = Vec3::new(10.0, 10.0, 10.0);
        q.add(corner, Vec3::X);
        q.add(corner, Vec3::Y);
        q.add(corner, Vec3::Z);
        let v = q.solve(Vec3::splat(9.0), Vec3::splat(11.0));
        assert!(
            (v - corner).length() < 1.0e-2,
            "corner reconstructed at {v:?} instead of {corner:?}"
        );
    }

    #[test]
    fn a_degenerate_system_falls_back_instead_of_exploding() {
        // A single constraint pins one axis only.
        let mut q = Qef::new();
        q.add(Vec3::new(1.0, 5.0, 5.0), Vec3::X);
        let v = q.solve(Vec3::splat(0.0), Vec3::splat(10.0));
        assert!(v.is_finite(), "{v:?}");
        assert!((v.x - 1.0).abs() < 1.0e-2, "{v:?}");
    }
}
