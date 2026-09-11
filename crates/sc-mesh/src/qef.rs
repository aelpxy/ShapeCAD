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
    /// Sum of crossing points, for the fallback and the regularisation target.
    mass: Vec3,
    /// Crossings seen, whether or not they contributed a tangent plane.
    crossings: u32,
    /// Crossings that did contribute one. Fewer than three leaves the system
    /// rank deficient and reliant on the regularisation.
    planes: u32,
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
            crossings: 0,
            planes: 0,
        }
    }

    /// Records a surface crossing and, where there is one, its tangent plane.
    ///
    /// A crossing whose `normal` is zero or non-finite still counts. The field
    /// is flat there (or too flat for the sampling to resolve), so it pins down
    /// no direction, but the cell is on the surface either way and still owes
    /// the mesher a vertex. Dropping it entirely is how a cell ends up with no
    /// vertex while the quads around it expect one, which is a hole.
    ///
    /// A non-finite `position` is ignored outright: there is no point on the
    /// surface to report, and averaging it in would take the whole cell with it.
    pub fn add(&mut self, position: Vec3, normal: Vec3) {
        if !position.is_finite() {
            return;
        }
        self.mass += position;
        self.crossings += 1;

        let n = normal.normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        self.ata += Mat3::from_cols(n * n.x, n * n.y, n * n.z);
        self.atb += n * n.dot(position);
        self.planes += 1;
    }

    /// Number of crossings added, including any that carried no usable normal.
    ///
    /// This is the count a mesher must branch on: zero means the cell is not on
    /// the surface at all, and anything else means it owes a vertex.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.crossings
    }

    /// The average of the crossing points.
    #[must_use]
    pub fn mass_point(&self) -> Vec3 {
        if self.crossings == 0 {
            Vec3::ZERO
        } else {
            self.mass / self.crossings as f32
        }
    }

    /// Solves for the vertex. The result is always inside `min..max`.
    ///
    /// Clamping is not cosmetic. An ill-conditioned system can put the minimiser
    /// far outside its own cell, which produces long spikes and self-intersecting
    /// triangles, geometry a slicer will reject.
    #[must_use]
    pub fn solve(&self, min: Vec3, max: Vec3) -> Vec3 {
        // With nothing at all to go on, the honest answer is the middle of the
        // cell. The mass point of no crossings is the origin, which belongs to
        // one cell out of the whole grid and is a spike in all the others.
        if self.crossings == 0 {
            return (min + max) * 0.5;
        }
        let mass = self.mass_point();
        if self.planes == 0 {
            return mass.clamp(min, max);
        }

        let a = self.ata + Mat3::IDENTITY * REGULARISATION;
        let b = self.atb + mass * REGULARISATION;

        // The regularisation already keeps the smallest eigenvalue at or above
        // `REGULARISATION`, so this catches what it cannot: a determinant that
        // is not a number, which no comparison is true of.
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

    /// `min..max` is a cell, and a vertex outside it is a spike.
    fn assert_inside(v: Vec3, min: Vec3, max: Vec3) {
        assert!(
            v.cmpge(min).all() && v.cmple(max).all(),
            "{v:?} is outside {min:?}..{max:?}"
        );
    }

    #[test]
    fn a_solver_with_no_crossings_still_answers_inside_its_cell() {
        // `solve` promises a point in `min..max`. Returning the mass point of
        // nothing is the origin, which for any cell away from the origin is a
        // vertex somewhere else entirely.
        let (min, max) = (Vec3::new(10.0, 10.0, 10.0), Vec3::new(11.0, 11.0, 11.0));
        assert_inside(Qef::new().solve(min, max), min, max);
    }

    #[test]
    fn a_crossing_with_a_degenerate_normal_still_places_a_vertex() {
        // The field is flat at the crossing, so there is no tangent plane to
        // constrain with. The crossing is still a crossing: the cell is on the
        // surface and owes the mesher a vertex, or the quads around it are
        // dropped and the mesh has a hole.
        let mut q = Qef::new();
        let p = Vec3::new(10.25, 10.5, 10.75);
        q.add(p, Vec3::ZERO);
        assert_eq!(q.count(), 1, "the crossing was forgotten");
        let (min, max) = (Vec3::splat(10.0), Vec3::splat(11.0));
        let v = q.solve(min, max);
        assert_inside(v, min, max);
        assert!((v - p).length() < 1.0e-3, "{v:?} is not the crossing {p:?}");
    }

    #[test]
    fn parallel_normals_do_not_run_the_vertex_off_to_infinity() {
        // Two faces of a thin sheet passing through one cell. The normals are
        // anti-parallel, so the system is rank one and the unconstrained axes
        // are pinned only by the regularisation.
        let (min, max) = (Vec3::splat(0.0), Vec3::splat(1.0));
        let mut q = Qef::new();
        q.add(Vec3::new(0.4, 0.5, 0.5), Vec3::X);
        q.add(Vec3::new(0.6, 0.5, 0.5), Vec3::NEG_X);
        let v = q.solve(min, max);
        assert_inside(v, min, max);
        assert!(
            (v.x - 0.5).abs() < 1.0e-2,
            "{v:?} is not between the sheets"
        );
    }

    #[test]
    fn an_ill_conditioned_corner_is_clamped_back_into_its_cell() {
        // Two almost parallel planes meet in a line a long way off. Without the
        // clamp the vertex lands there and the cell grows a spike.
        let (min, max) = (Vec3::splat(0.0), Vec3::splat(1.0));
        let mut q = Qef::new();
        q.add(Vec3::new(0.5, 0.5, 0.5), Vec3::X);
        q.add(
            Vec3::new(0.5, 0.5, 0.5) + Vec3::new(0.0, 1.0e-4, 0.0),
            Vec3::new(1.0, 1.0e-4, 0.0),
        );
        assert_inside(q.solve(min, max), min, max);
    }

    #[test]
    fn a_non_finite_crossing_does_not_poison_the_vertex() {
        let (min, max) = (Vec3::splat(0.0), Vec3::splat(1.0));
        let mut q = Qef::new();
        q.add(Vec3::new(0.25, 0.5, 0.5), Vec3::X);
        q.add(Vec3::new(f32::NAN, 0.5, 0.5), Vec3::Y);
        let v = q.solve(min, max);
        assert!(v.is_finite(), "{v:?}");
        assert_inside(v, min, max);
        assert!(
            (v.x - 0.25).abs() < 1.0e-2,
            "the good constraint was lost: {v:?}"
        );
    }
}
