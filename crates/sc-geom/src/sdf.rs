//! Sampled signed distance fields: imported geometry as a voxel grid.
//!
//! An STL someone downloaded, or a scan, has no closed form. Voxelizing it once
//! at import turns it into just another field function, so booleans, shells,
//! offsets and smooth blends all work on it without any of them knowing that a
//! triangle mesh was ever involved. The trade is resolution: a grid resolves
//! nothing finer than its spacing, and unlike every other node in this kernel
//! its field is an approximation rather than an exact distance.
//!
//! The grid itself is produced by the voxelizer in `sc-mesh` and stored beside
//! the document rather than inside it, because it is megabytes of samples that
//! no one will ever hand-edit. [`AssetId`] is the handle the document keeps.

use crate::bounds::Aabb;
use crate::hash::StableHasher;
use glam::Vec3;
use std::fmt;

/// Identifies a voxel grid stored beside the document.
///
/// The samples live in a sidecar file, so the document holds only this handle
/// and the loader resolves it into a [`Grid`] on the way in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AssetId(pub u32);

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "asset:{}", self.0)
    }
}

/// A signed distance field sampled on a regular grid, interpolated trilinearly.
///
/// # Preconditions
///
/// These are the producer's responsibility. The voxelizer must hold to them and
/// [`Grid::is_valid`] checks the cheap ones:
///
/// - `data.len()` equals the product of `dims`, laid out with x varying
///   fastest, then y, then z.
/// - `spacing` is finite and strictly positive, and `origin` is finite.
/// - Every sample is finite. This is not checked, because scanning megabytes on
///   every arena insert and every log replay would be paid over and over for a
///   defect that can only come from the voxelizer. One NaN poisons every `min`
///   and `max` above it, so check it where the samples are produced.
/// - **The zero level set is at least [`Grid::REQUIRED_CLEARANCE`] voxels clear
///   of every face of the grid.** Padding is what makes [`Grid::sample`] a lower
///   bound on distance outside the grid, and a lower bound is what keeps sphere
///   tracing from stepping through a surface. Grow the box at import rather than
///   trimming it to the model. [`Grid::boundary_clearance`] measures it.
///
/// Note that the field a grid carries is an approximation, not an exact
/// distance, which makes it the one exception in this kernel. It agrees with the
/// true distance to within the interpolation error, which is a fraction of a
/// voxel for a surface the spacing can resolve, and its gradient can reach
/// `sqrt(3)` at a kink. Offsets, shells and blends applied above a mesh inherit
/// that error.
///
/// Two grids compare equal when every sample matches, which is the right
/// meaning for a value type but the wrong one for a node: see the note on
/// [`PartialEq`](crate::node::Node) for [`Node::Mesh`](crate::node::Node::Mesh).
#[derive(Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Grid {
    /// Number of samples along x, y and z.
    pub dims: [u32; 3],
    /// World position of the centre of voxel (0, 0, 0).
    pub origin: Vec3,
    /// World edge length of one voxel, uniform on all three axes.
    pub spacing: f32,
    /// The samples, x fastest, then y, then z.
    pub data: Vec<f32>,
}

impl fmt::Debug for Grid {
    /// Prints the shape of the grid and not its contents.
    ///
    /// A derived `Debug` would dump every sample into a panic message or a
    /// proptest counterexample, which is megabytes of noise around the four
    /// numbers that actually identify the grid.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Grid")
            .field("dims", &self.dims)
            .field("origin", &self.origin)
            .field("spacing", &self.spacing)
            .field("samples", &self.data.len())
            .finish_non_exhaustive()
    }
}

impl Default for Grid {
    /// The unresolved placeholder, which fails [`Grid::is_valid`] by
    /// construction.
    fn default() -> Self {
        Self {
            dims: [0; 3],
            origin: Vec3::ZERO,
            spacing: 0.0,
            data: Vec::new(),
        }
    }
}

impl Grid {
    /// How far the surface must stay from every face of the grid, in voxels.
    ///
    /// Two rather than one: trilinear interpolation in the outermost cell reads
    /// the boundary samples themselves, so one voxel of clearance would let the
    /// surface influence the value returned at the face, and the distance to the
    /// grid box would stop bounding the distance to the surface from below.
    pub const REQUIRED_CLEARANCE: u32 = 2;

    /// Samples `f` at the centre of every voxel.
    ///
    /// This is how a field with a closed form becomes a grid, which is what the
    /// tests use to check the sampler against an analytic answer. Importing a
    /// triangle mesh goes through the voxelizer in `sc-mesh` instead.
    #[must_use]
    pub fn from_fn(
        dims: [u32; 3],
        origin: Vec3,
        spacing: f32,
        mut f: impl FnMut(Vec3) -> f32,
    ) -> Self {
        let count = Self::count(dims);
        let mut data = Vec::with_capacity(count);
        for iz in 0..dims[2] {
            for iy in 0..dims[1] {
                for ix in 0..dims[0] {
                    let at = origin + Vec3::new(ix as f32, iy as f32, iz as f32) * spacing;
                    data.push(f(at));
                }
            }
        }
        Self {
            dims,
            origin,
            spacing,
            data,
        }
    }

    /// Number of samples implied by `dims`.
    #[must_use]
    pub fn voxel_count(&self) -> usize {
        Self::count(self.dims)
    }

    fn count(dims: [u32; 3]) -> usize {
        dims.iter().map(|&d| d as usize).product()
    }

    /// Whether this is the placeholder a document deserializes to before its
    /// sidecar asset has been resolved.
    #[must_use]
    pub fn is_placeholder(&self) -> bool {
        self.dims == [0; 3] && self.data.is_empty()
    }

    /// Whether the grid is structurally sound: two or more samples on every
    /// axis, a matching sample count, and a usable origin and spacing.
    ///
    /// Two samples per axis is the minimum trilinear interpolation can work
    /// with. A real imported grid has far more, since the clearance requirement
    /// alone costs four.
    ///
    /// This deliberately does not scan `data`; see the preconditions on
    /// [`Grid`].
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.dims.iter().all(|&d| d >= 2)
            && self.data.len() == self.voxel_count()
            && self.spacing.is_finite()
            && self.spacing > 0.0
            && self.origin.is_finite()
    }

    /// Whether the samples can be read at all: enough of them, and a spacing
    /// that places them somewhere.
    ///
    /// Weaker than [`Grid::is_valid`], which is the contract a node has to meet.
    /// This is only what the readers below need in order to return an answer
    /// instead of a NaN, since they are reachable on a hand-built grid.
    fn is_readable(&self) -> bool {
        !self.data.is_empty()
            && self.data.len() == self.voxel_count()
            && self.spacing.is_finite()
            && self.spacing > 0.0
    }

    /// The smallest sample anywhere on the six boundary faces, or
    /// [`f32::NEG_INFINITY`] for a grid with no samples.
    ///
    /// The padding precondition holds when this is at least
    /// `REQUIRED_CLEARANCE * spacing`. It is O(n^2/3) rather than O(n), but it
    /// is still a scan, so it belongs at import next to the voxelizer and not on
    /// the path of every edit.
    #[must_use]
    pub fn boundary_clearance(&self) -> f32 {
        if !self.is_readable() {
            return f32::NEG_INFINITY;
        }
        let [nx, ny, nz] = self.dims;
        let mut worst = f32::INFINITY;
        for iz in 0..nz {
            let on_z = iz == 0 || iz == nz - 1;
            for iy in 0..ny {
                let on_y = iy == 0 || iy == ny - 1;
                // Interior rows touch the boundary only at their two ends, so
                // stepping by the full row skips the bulk of the grid.
                let step = if on_z || on_y { 1 } else { (nx - 1).max(1) };
                for ix in (0..nx).step_by(step as usize) {
                    worst = worst.min(self.data[self.index(ix, iy, iz)]);
                }
            }
        }
        worst
    }

    /// The world-space box the samples cover, one half voxel beyond the outermost
    /// sample centres.
    ///
    /// Over-reports, as bounds in this kernel always must: the surface is
    /// required to sit at least [`Grid::REQUIRED_CLEARANCE`] voxels inside the
    /// sample lattice, so the true extent of the solid is comfortably smaller.
    /// Returns [`Aabb::EMPTY`] for a placeholder.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        if !self.is_readable() {
            return Aabb::EMPTY;
        }
        let (lo, hi) = self.lattice();
        let half = Vec3::splat(self.spacing * 0.5);
        Aabb {
            min: lo - half,
            max: hi + half,
        }
    }

    /// Signed distance at `p`, negative inside.
    ///
    /// Inside the sample lattice this is trilinear interpolation of the eight
    /// surrounding voxels. That is an approximation, not an exact distance: it
    /// reproduces a linear field exactly, but near a convex corner or the medial
    /// axis its gradient can reach `sqrt(3)`, so the value can exceed the true
    /// distance by a fraction of a voxel. Everything downstream that assumes an
    /// exact field is therefore approximate on a mesh node to within the
    /// spacing, which is the resolution the user chose at import.
    ///
    /// Outside the lattice the result is
    /// `hypot(distance to the lattice box, sample at the nearest point on it)`.
    /// Neither term alone is good enough. The box distance on its own collapses
    /// to zero at the face and says nothing about how far away the surface
    /// actually is, which makes tracing crawl along the outside of every
    /// imported part. The clamped edge sample on its own ignores the distance
    /// travelled to get there and stops growing as the ray recedes. Combining
    /// them in quadrature is not a compromise between the two but the exact
    /// answer for the worst case: the nearest point on the box is the
    /// orthogonal projection of `p`, so for any surface point `s` inside the box
    /// the two legs are perpendicular and `|p - s|^2 >= box^2 + |c - s|^2`.
    /// Since the surface is required to lie inside the box, that makes the
    /// result a lower bound on the true distance, which is what sphere tracing
    /// needs to avoid stepping through the surface. It is also continuous with
    /// the interior, since the box distance is zero at the face.
    ///
    /// The one qualification: the bound is exact in the value it is given, but
    /// the value it is given is the interpolated boundary sample, which
    /// over-reports a curved surface by a fraction of a voxel like any other
    /// reading of the grid. So the exterior is conservative to within the
    /// interior's own accuracy and no worse. Pinned by
    /// `exterior_samples_never_exceed_the_true_distance`.
    ///
    /// A placeholder or a malformed grid evaluates as empty space rather than
    /// panicking, matching a missing node, though
    /// [`Node::is_valid`](crate::node::Node::is_valid) is meant to have stopped
    /// one reaching here.
    #[must_use]
    pub fn sample(&self, p: Vec3) -> f32 {
        if !self.is_readable() {
            return f32::INFINITY;
        }
        let (lo, hi) = self.lattice();
        let nearest = p.clamp(lo, hi);
        let inside = self.trilinear(nearest);
        let outside = (p - nearest).length();
        if outside == 0.0 {
            return inside;
        }
        // The two legs are genuinely perpendicular, so the hypotenuse is a valid
        // lower bound: writing the step from the nearest lattice point to the
        // nearest surface point as an inward part `a` and a tangential part `t`,
        // the true distance squared is `(outside + a)^2 + |t|^2`, which is at
        // least `outside^2 + a^2 + |t|^2`.
        //
        // The reading is shaded down by a voxel first. `inside` is interpolated,
        // and interpolation over-reports a curved surface by a fraction of a
        // voxel, which is enough to turn the bound into an estimate: measured
        // 5.809 against a true 5.800 on a voxelized sphere. Outside the lattice
        // is the one place where over-reporting is unsafe, because a tracer
        // takes the reading as a step length and walks through the surface.
        //
        // A whole voxel is far more slack than the error has ever measured, and
        // it costs almost nothing: once either leg is longer than the spacing,
        // which is the resolution the user asked for, the shading is lost in the
        // hypotenuse. Subtracting nothing is unsafe and subtracting the padding
        // instead throws the reading away, which collapses the bound to a
        // fraction of the truth out towards the corners of the lattice.
        let shaded = (inside - self.spacing).max(0.0);
        shaded.hypot(outside)
    }

    /// Deterministic digest of the whole grid.
    ///
    /// Folded into the geometry hash, so it has to mean the same thing on every
    /// platform and in five years. Samples are hashed by their raw bits rather
    /// than through [`quantize`](crate::hash::quantize), because a grid is
    /// machine-produced from one fixed input and is expected to be reproduced
    /// bit for bit; quantizing would hide a voxel that moved by less than the
    /// quantum. Zero and NaN are normalised first, since those are the two
    /// values with more than one bit pattern and a producer may emit either.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut h = StableHasher::new();
        for d in self.dims {
            h.write_u64(u64::from(d));
        }
        for v in [self.origin.x, self.origin.y, self.origin.z, self.spacing] {
            h.write_u64(u64::from(canonical_bits(v)));
        }
        h.write_u64(self.data.len() as u64);
        for &v in &self.data {
            h.write_u64(u64::from(canonical_bits(v)));
        }
        h.finish()
    }

    /// Corners of the sample lattice: the centres of the first and last voxels.
    fn lattice(&self) -> (Vec3, Vec3) {
        let last = Vec3::new(
            self.dims[0].saturating_sub(1) as f32,
            self.dims[1].saturating_sub(1) as f32,
            self.dims[2].saturating_sub(1) as f32,
        );
        (self.origin, self.origin + last * self.spacing)
    }

    fn index(&self, x: u32, y: u32, z: u32) -> usize {
        let [nx, ny] = [self.dims[0] as usize, self.dims[1] as usize];
        (z as usize * ny + y as usize) * nx + x as usize
    }

    /// The two sample indices bracketing `rel` on one axis, and the weight
    /// between them. `rel` is in voxels from the origin.
    fn bracket(dim: u32, rel: f32) -> (u32, u32, f32) {
        let last = dim.saturating_sub(1);
        if last == 0 {
            return (0, 0, 0.0);
        }
        let clamped = rel.clamp(0.0, last as f32);
        // `last - 1` rather than `last`, so that at the far face the pair is the
        // final cell with a weight of one instead of an empty cell past the end.
        let lo = (clamped.floor() as u32).min(last - 1);
        (lo, lo + 1, clamped - lo as f32)
    }

    /// Trilinear interpolation at a point already clamped into the lattice.
    fn trilinear(&self, p: Vec3) -> f32 {
        let rel = (p - self.origin) / self.spacing;
        let (x0, x1, tx) = Self::bracket(self.dims[0], rel.x);
        let (y0, y1, ty) = Self::bracket(self.dims[1], rel.y);
        let (z0, z1, tz) = Self::bracket(self.dims[2], rel.z);

        let at = |x, y, z| self.data[self.index(x, y, z)];
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;

        let y00 = lerp(at(x0, y0, z0), at(x1, y0, z0), tx);
        let y10 = lerp(at(x0, y1, z0), at(x1, y1, z0), tx);
        let y01 = lerp(at(x0, y0, z1), at(x1, y0, z1), tx);
        let y11 = lerp(at(x0, y1, z1), at(x1, y1, z1), tx);

        lerp(lerp(y00, y10, ty), lerp(y01, y11, ty), tz)
    }
}

/// Collapses the two bit patterns that mean the same number.
///
/// Negative zero, and the many bit patterns that are all NaN, differ between one
/// producer and another while denoting the same value, and a bitwise digest
/// would call those grids different.
fn canonical_bits(v: f32) -> u32 {
    if v == 0.0 {
        return 0;
    }
    if v.is_nan() {
        return f32::NAN.to_bits();
    }
    v.to_bits()
}

#[cfg(test)]
mod tests {
    use super::{AssetId, Grid};
    use glam::Vec3;

    /// A sphere of radius `r` centred at the origin, sampled over a box with
    /// plenty of clearance on every face.
    fn sphere_grid(radius: f32, dims: u32, spacing: f32) -> Grid {
        let half = (dims - 1) as f32 * spacing * 0.5;
        Grid::from_fn([dims; 3], Vec3::splat(-half), spacing, |p| {
            p.length() - radius
        })
    }

    /// A reproducible stream in `[0, 1)`. Written out rather than pulled in: a
    /// fixed seed makes a failure here reproducible without a shrinker, and the
    /// generated models belong in `tests/properties.rs`.
    fn unit_stream(seed: u32) -> impl FnMut() -> f32 {
        let mut state = seed;
        move || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / 16_777_216.0
        }
    }

    #[test]
    fn interior_samples_track_the_analytic_field() {
        let grid = sphere_grid(4.0, 33, 0.5);
        let half = (33 - 1) as f32 * 0.5 * 0.5;
        let mut rand = unit_stream(0x1234_5678);
        for _ in 0..4000 {
            let p = (Vec3::new(rand(), rand(), rand()) - Vec3::splat(0.5)) * (2.0 * half);
            let analytic = p.length() - 4.0;
            let sampled = grid.sample(p);
            assert!(
                (sampled - analytic).abs() <= grid.spacing,
                "at {p:?}: sampled {sampled}, analytic {analytic}"
            );
        }
    }

    /// The property that keeps sphere tracing sound: outside the grid the value
    /// returned must not be further than the surface really is, or a ray takes a
    /// step that lands inside the solid and the renderer punches a hole through
    /// it.
    ///
    /// "Must not exceed" is measured against the interpolated edge sample rather
    /// than against the analytic answer, and that is not a loosening: the
    /// extrapolation is exactly conservative, but the value it extrapolates from
    /// is a trilinear reading of the boundary, which already over-reports a
    /// curved surface by a fraction of a voxel. Requiring the exterior to beat
    /// an error the interior is entitled to would be asking it to be more
    /// accurate than the grid it reads. What is pinned here is that the
    /// extrapolation adds nothing of its own.
    #[test]
    fn exterior_samples_never_exceed_the_true_distance() {
        let grid = sphere_grid(3.0, 25, 0.5);
        let half = (25 - 1) as f32 * 0.5 * 0.5;
        let mut rand = unit_stream(0x0bad_c0de);
        let mut tightest = f32::INFINITY;
        let mut worst_edge_error = 0.0f32;
        let mut checked = 0;
        for _ in 0..20_000 {
            let p = (Vec3::new(rand(), rand(), rand()) - Vec3::splat(0.5)) * (8.0 * half);
            // Only the outside is under test; the interior has its own.
            if p.abs().max_element() <= half {
                continue;
            }
            checked += 1;
            let analytic = p.length() - 3.0;
            let sampled = grid.sample(p);

            // How much the grid over-reports at the point the extrapolation
            // starts from, which is the nearest point on the sample lattice.
            let nearest = p.clamp(Vec3::splat(-half), Vec3::splat(half));
            let edge_error = (grid.sample(nearest) - (nearest.length() - 3.0)).max(0.0);
            worst_edge_error = worst_edge_error.max(edge_error);

            assert!(
                sampled <= analytic + edge_error + 1e-4,
                "overshoot at {p:?}: sampled {sampled} > true {analytic} \
                 by more than the {edge_error} the edge sample was already out by"
            );
            tightest = tightest.min(sampled / analytic);
        }
        assert!(checked > 1000, "the exterior was barely sampled");
        // The error being allowed for is a small fraction of a voxel, not a
        // licence to be wrong by any amount.
        assert!(
            worst_edge_error < grid.spacing * 0.05,
            "edge samples are out by {worst_edge_error}, more than expected of \
             trilinear interpolation at this spacing"
        );
        // A bound of zero would also never overshoot and would make tracing
        // crawl. Pin that the combination is actually worth something.
        assert!(
            tightest > 0.5,
            "the bound collapsed to {tightest} of the true distance"
        );
    }

    #[test]
    fn the_placeholder_is_not_a_valid_grid() {
        let empty = Grid::default();
        assert!(empty.is_placeholder());
        assert!(!empty.is_valid());
        assert!(empty.bounds().is_empty());
        assert!(empty.sample(Vec3::ZERO) > 0.0, "a placeholder is not solid");
        assert!(empty.sample(Vec3::ZERO).is_infinite());
    }

    #[test]
    fn bounds_cover_every_sample_and_then_some() {
        let grid = sphere_grid(2.0, 9, 1.0);
        let b = grid.bounds();
        assert_eq!(b.min, Vec3::splat(-4.5));
        assert_eq!(b.max, Vec3::splat(4.5));
    }

    #[test]
    fn clearance_reports_the_padding_actually_present() {
        let grid = sphere_grid(2.0, 17, 0.5);
        // Corner sample: the nearest face is 4 voxels from the surface, but the
        // minimum over the whole boundary is at a face centre.
        let expected = 4.0 - 2.0;
        assert!(
            (grid.boundary_clearance() - expected).abs() < 1e-5,
            "{}",
            grid.boundary_clearance()
        );
        assert!(grid.boundary_clearance() >= Grid::REQUIRED_CLEARANCE as f32 * grid.spacing);
    }

    /// The largest `|sample(p + h*axis) - sample(p)| / h` found over a scan.
    fn steepest_gradient(grid: &Grid, samples: u32) -> (f32, Vec3) {
        let extent = (grid.dims[0] - 1) as f32 * grid.spacing;
        let step = extent / samples as f32;
        let h = grid.spacing * 0.01;
        let mut worst = (0.0f32, Vec3::ZERO);
        for iz in 0..samples {
            for iy in 0..samples {
                for ix in 0..samples {
                    let p = grid.origin + Vec3::new(ix as f32, iy as f32, iz as f32) * step;
                    let here = grid.sample(p);
                    for axis in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::ONE.normalize()] {
                        let slope = (grid.sample(p + axis * h) - here).abs() / h;
                        if slope > worst.0 {
                            worst = (slope, p);
                        }
                    }
                }
            }
        }
        worst
    }

    /// Trilinear interpolation of an exact distance field is not itself an exact
    /// distance field, and this is the price.
    ///
    /// Each axis of the interpolant has slope at most one, because its samples
    /// came from a 1-Lipschitz function, but the three combine, so the gradient
    /// can reach `sqrt(3)` where the field kinks: the medial axis inside a
    /// solid, or a convex corner outside one. The reproducing case is the centre
    /// of a sphere, where `|p| - r` has its minimum and the three axis slopes
    /// all reach one at the same sample.
    ///
    /// The consequence is that sphere tracing can overstep by up to 73 percent
    /// of a step within one voxel of such a kink. It is bounded by the voxel
    /// spacing, which is the resolution the user asked for, and paying it back
    /// would mean scaling every sample down by `sqrt(3)` and making every trace
    /// of every imported part 1.7 times slower. So it is documented and pinned
    /// here rather than hidden, and [`Node::Mesh`](crate::node::Node::Mesh) is
    /// excluded from the kernel's strict Lipschitz property for the same reason
    /// a smooth blend is.
    #[test]
    fn known_limitation_trilinear_sampling_is_not_lipschitz_at_a_kink() {
        let grid = sphere_grid(2.0, 17, 0.5);
        let (worst, at) = steepest_gradient(&grid, 60);
        assert!(
            worst > 1.5,
            "the known violation went away at {at:?} ({worst}); if the sampler \
             now holds the bound, tighten this test rather than deleting it"
        );
        assert!(
            worst <= 3.0f32.sqrt() + 1e-3,
            "gradient {worst} at {at:?} exceeds the sqrt(3) bound trilinear \
             interpolation of 1-Lipschitz samples can reach"
        );
        assert!(
            at.length() < grid.spacing,
            "the violation should sit at the medial axis, not at {at:?}"
        );
    }

    /// Away from the medial axis the interpolant behaves like the field it came
    /// from, which is why the error stays sub-voxel in the interior test above.
    #[test]
    fn gradient_is_near_unit_away_from_the_medial_axis() {
        let grid = sphere_grid(2.0, 17, 0.5);
        let h = grid.spacing * 0.01;
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            let p = axis * 3.0;
            let slope = (grid.sample(p + axis * h) - grid.sample(p)).abs() / h;
            assert!((slope - 1.0).abs() < 0.02, "slope {slope} along {axis:?}");
        }
    }

    #[test]
    fn the_same_grid_built_twice_digests_the_same() {
        let a = sphere_grid(2.0, 9, 0.5);
        let b = sphere_grid(2.0, 9, 0.5);
        assert_eq!(a.digest(), b.digest());
        // Fixed for all time, like every other digest in this kernel. If this
        // moves, a golden hash somewhere else has moved with it.
        assert_eq!(format!("{:016x}", a.digest()), "208dad057b59bb5e");
    }

    #[test]
    fn one_voxel_changes_the_digest() {
        let base = sphere_grid(2.0, 9, 0.5);
        for (index, delta) in [(0usize, 1e-6f32), (17, -1.0), (728, 0.5)] {
            let mut moved = base.clone();
            moved.data[index] += delta;
            assert_ne!(
                base.digest(),
                moved.digest(),
                "voxel {index} moved by {delta} unnoticed"
            );
        }
    }

    #[test]
    fn placement_is_part_of_the_digest() {
        let base = sphere_grid(2.0, 9, 0.5);
        let mut moved = base.clone();
        moved.origin.x += 0.25;
        assert_ne!(base.digest(), moved.digest(), "origin");
        let mut finer = base.clone();
        finer.spacing = 0.6;
        assert_ne!(base.digest(), finer.digest(), "spacing");
    }

    #[test]
    fn negative_zero_does_not_perturb_the_digest() {
        // Two voxelizers can disagree on the sign of an exact zero while
        // describing the same surface. A bitwise digest would call those two
        // different parts.
        let mut plus = sphere_grid(2.0, 5, 1.0);
        plus.data[3] = 0.0;
        let mut minus = plus.clone();
        minus.data[3] = -0.0;
        assert_eq!(plus.digest(), minus.digest());
    }

    #[test]
    fn asset_ids_order_and_print_stably() {
        assert!(AssetId(1) < AssetId(2));
        assert_eq!(AssetId(7).to_string(), "asset:7");
    }
}
