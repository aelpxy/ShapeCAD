//! Turning a triangle mesh into a signed distance grid.
//!
//! An imported STL has to become a field function before the kernel can boolean
//! it against modelled geometry, and a voxel grid is the representation that
//! makes that possible without reintroducing a boundary representation. The
//! grid is sampled once on import; from then on it is just another node.
//!
//! Distance comes from the nearest point on the nearest triangle, found with
//! [`crate::bvh::Bvh`]. The sign is the interesting half. See [`inside`].

use crate::bvh::Bvh;
use crate::error::{MeshError, Result};
use crate::mesh::Mesh;
use rayon::prelude::*;
use sc_geom::glam::Vec3;
use sc_geom::sdf::Grid;

/// Voxels of padding added outside the mesh bounding box on every side.
///
/// [`Grid`] requires the zero level set to be at least two voxels clear of
/// every face. The surface can touch the bounding box exactly, so padding by
/// two would put the zero set on the boundary of the region that is supposed to
/// be clear. The third voxel buys the margin that makes the invariant strict
/// rather than marginal, and costs a few percent of the sample count.
pub const PAD_VOXELS: u32 = 3;

/// Voxels of clearance between the zero level set and every face of the grid
/// that [`Grid`] promises its readers.
///
/// [`PAD_VOXELS`] is what the voxelizer adds; this is what the contract says,
/// and it is deliberately the smaller of the two.
pub const CLEARANCE_VOXELS: f32 = 2.0;

/// Smallest spacing that will be used, in millimetres.
///
/// A mesh whose bounding box has zero extent on every axis (one degenerate
/// triangle, or a single repeated point) would otherwise divide by zero.
const MIN_SPACING: f32 = 1.0e-5;

/// Winding number above which a point counts as inside.
///
/// Exactly one inside a closed surface and exactly zero outside, so any
/// threshold strictly between the two would do for a perfect mesh. A half is
/// the choice that puts the maximum margin on both sides, which is what buys
/// the tolerance to holes.
const INSIDE_THRESHOLD: f32 = 0.5;

/// Whether `p` is inside the solid the mesh describes.
///
/// Uses the generalized winding number: the sum over triangles of the signed
/// solid angle each subtends at `p`, divided by four pi.
///
/// The obvious alternative, casting a ray and counting crossings for parity, is
/// rejected because it is brittle in exactly the conditions this code exists to
/// handle. Parity is a discrete count, so a single missing triangle flips the
/// answer for the entire ray behind the hole, a ray that grazes a shared edge
/// double-counts or misses, and coplanar or self-intersecting geometry has no
/// defined parity at all. Downloaded STLs have all three problems routinely.
/// The winding number is instead a continuous function that equals one inside
/// and zero outside a closed surface and degrades smoothly near a defect, so a
/// hole perturbs the answer only within roughly its own diameter, and a
/// half-way threshold still classifies everything else correctly.
#[must_use]
pub fn inside(bvh: &Bvh, p: Vec3) -> bool {
    bvh.winding_number(p) > INSIDE_THRESHOLD
}

/// Voxelizes `mesh` at `resolution` voxels along its longest axis.
///
/// # Errors
/// [`MeshError::EmptyMesh`] if there is nothing to voxelize.
pub fn voxelize(mesh: &Mesh, resolution: u32) -> Result<Grid> {
    if mesh.indices.is_empty() {
        return Err(MeshError::EmptyMesh);
    }
    voxelize_bvh(&Bvh::from_mesh(mesh), resolution)
}

/// Voxelizes a tree that has already been built.
///
/// Saves rebuilding the hierarchy when the same mesh is sampled at more than
/// one resolution, which the resolution property test does.
///
/// # Errors
/// [`MeshError::EmptyMesh`] if the tree holds no triangles.
pub fn voxelize_bvh(bvh: &Bvh, resolution: u32) -> Result<Grid> {
    if bvh.triangles().is_empty() {
        return Err(MeshError::EmptyMesh);
    }
    let (dims, origin, spacing) = layout(bvh.bounds().min, bvh.bounds().max, resolution);

    let plane = dims[0] as usize * dims[1] as usize;
    let mut data = vec![0.0f32; plane * dims[2] as usize];
    // One slab per z, which is the axis the layout makes slowest, so each task
    // walks contiguous memory and the tree queries stay in a small region.
    data.par_chunks_mut(plane)
        .enumerate()
        .for_each(|(k, slab)| fill_slab(bvh, dims, origin, spacing, k as u32, slab));

    Ok(Grid {
        dims,
        origin,
        spacing,
        data,
    })
}

/// Chooses the grid dimensions, origin and spacing for a mesh spanning
/// `min..max`.
fn layout(min: Vec3, max: Vec3, resolution: u32) -> ([u32; 3], Vec3, f32) {
    let size = (max - min).max(Vec3::ZERO);
    let spacing = (size.max_element() / resolution.max(1) as f32).max(MIN_SPACING);

    let core = |extent: f32| (extent / spacing).ceil() as u32 + 1;
    let dims = [
        core(size.x) + 2 * PAD_VOXELS,
        core(size.y) + 2 * PAD_VOXELS,
        core(size.z) + 2 * PAD_VOXELS,
    ];
    let origin = min - Vec3::splat(spacing * PAD_VOXELS as f32);
    (dims, origin, spacing)
}

/// Fills one constant-z slab of the grid.
fn fill_slab(bvh: &Bvh, dims: [u32; 3], origin: Vec3, spacing: f32, k: u32, slab: &mut [f32]) {
    for j in 0..dims[1] {
        for i in 0..dims[0] {
            let p = origin + spacing * Vec3::new(i as f32, j as f32, k as f32);
            let d = bvh.distance(p);
            // Every padding voxel sits at least one full voxel outside the
            // mesh bounding box, so no part of the input surface can reach it.
            // Taking the sign as positive there rather than asking the winding
            // number keeps the grid invariant true even when the input is torn
            // badly enough that the winding number misbehaves far from the
            // surface.
            let negative = !in_padding(dims, i, j, k) && inside(bvh, p);
            slab[(j * dims[0] + i) as usize] = if negative { -d } else { d };
        }
    }
}

/// Whether the voxel lies in the padding shell rather than over the mesh.
fn in_padding(dims: [u32; 3], i: u32, j: u32, k: u32) -> bool {
    [(i, dims[0]), (j, dims[1]), (k, dims[2])]
        .iter()
        .any(|&(v, n)| v < PAD_VOXELS || v + PAD_VOXELS >= n)
}

/// Offset of voxel `(i, j, k)` in [`Grid::data`].
///
/// A free function rather than a method because [`Grid`] belongs to the kernel.
#[must_use]
pub fn voxel_index(grid: &Grid, i: u32, j: u32, k: u32) -> usize {
    ((k * grid.dims[1] + j) * grid.dims[0] + i) as usize
}

/// World position of the centre of voxel `(i, j, k)`.
#[must_use]
pub fn voxel_center(grid: &Grid, i: u32, j: u32, k: u32) -> Vec3 {
    grid.origin + grid.spacing * Vec3::new(i as f32, j as f32, k as f32)
}

/// The value stored at voxel `(i, j, k)`.
#[must_use]
pub fn voxel_value(grid: &Grid, i: u32, j: u32, k: u32) -> f32 {
    grid.data[voxel_index(grid, i, j, k)]
}

/// The two opposite corners of the region the samples cover.
#[must_use]
pub fn sample_box(grid: &Grid) -> (Vec3, Vec3) {
    let extent = Vec3::new(
        (grid.dims[0] - 1) as f32,
        (grid.dims[1] - 1) as f32,
        (grid.dims[2] - 1) as f32,
    );
    (grid.origin, grid.origin + grid.spacing * extent)
}

#[cfg(test)]
#[allow(clippy::many_single_char_names)]
mod tests {
    use super::*;
    use crate::testing::{box_sdf, sphere_sdf, unit_box, uv_sphere, with_triangles_removed};

    /// A deterministic scatter of points inside the sample box. A fixed
    /// sequence rather than a random one so a failure is reproducible without
    /// a seed to carry around.
    fn probes(grid: &Grid, count: usize) -> Vec<Vec3> {
        let (lo, hi) = sample_box(grid);
        let size = hi - lo;
        (0..count)
            .map(|n| {
                let t = n as f32;
                let u = Vec3::new(
                    (t * 0.7351).fract(),
                    (t * 0.4327).fract(),
                    (t * 0.9137).fract(),
                );
                lo + size * u
            })
            .collect()
    }

    #[test]
    fn grid_agrees_with_the_analytic_sphere_within_one_voxel() {
        let radius = 10.0;
        let mesh = uv_sphere(radius, 48, 32);
        let grid = voxelize(&mesh, 24).unwrap();

        let mut worst = 0.0f32;
        for p in probes(&grid, 500) {
            let got = grid.sample(p);
            let want = sphere_sdf(radius, p);
            worst = worst.max((got - want).abs());
        }
        assert!(
            worst <= grid.spacing,
            "worst error {worst} exceeds the spacing {}",
            grid.spacing
        );
    }

    #[test]
    fn grid_agrees_with_the_analytic_box_within_one_voxel() {
        // A box exercises flat faces, edges and corners, and the medial axis
        // kinks inside it, none of which a sphere reaches.
        let half = Vec3::new(8.0, 5.0, 3.0);
        let grid = voxelize(&unit_box(half), 24).unwrap();

        let mut worst = 0.0f32;
        for p in probes(&grid, 500) {
            let got = grid.sample(p);
            let want = box_sdf(half, p);
            worst = worst.max((got - want).abs());
        }
        assert!(
            worst <= grid.spacing,
            "worst error {worst} exceeds the spacing {}",
            grid.spacing
        );
    }

    #[test]
    fn voxel_centres_are_nearly_exact_on_a_sphere() {
        // Trilinear interpolation carries most of the error in the tests above.
        // At the sample points themselves only the tessellation is in the way,
        // so the bar is much tighter.
        let radius = 10.0;
        let grid = voxelize(&uv_sphere(radius, 64, 40), 20).unwrap();
        let mut worst = 0.0f32;
        for k in 0..grid.dims[2] {
            for j in 0..grid.dims[1] {
                for i in 0..grid.dims[0] {
                    if in_padding(grid.dims, i, j, k) {
                        continue;
                    }
                    let p = voxel_center(&grid, i, j, k);
                    worst = worst.max((voxel_value(&grid, i, j, k) - sphere_sdf(radius, p)).abs());
                }
            }
        }
        // One facet of a 64 by 40 sphere of radius 10 has a sagitta under
        // 0.03mm, and the mesh is inscribed, so the grid reads slightly large.
        assert!(worst < 0.05, "worst sample error {worst}");
    }

    #[test]
    fn the_zero_set_is_at_least_two_voxels_clear_of_every_face() {
        for grid in [
            voxelize(&uv_sphere(4.0, 24, 16), 16).unwrap(),
            voxelize(&unit_box(Vec3::new(3.0, 1.0, 7.0)), 16).unwrap(),
        ] {
            let corner = voxel_value(&grid, 0, 0, 0);
            assert!(corner > 0.0, "the corner voxel must be outside");
            for k in 0..grid.dims[2] {
                for j in 0..grid.dims[1] {
                    for i in 0..grid.dims[0] {
                        let near_face = i < 2
                            || j < 2
                            || k < 2
                            || i + 2 >= grid.dims[0]
                            || j + 2 >= grid.dims[1]
                            || k + 2 >= grid.dims[2];
                        if !near_face {
                            continue;
                        }
                        let v = voxel_value(&grid, i, j, k);
                        assert!(
                            v > 0.0,
                            "voxel ({i},{j},{k}) is within two of a face but reads {v}, \
                             the same side as the corner voxel {corner} is required"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_grid_covers_the_mesh_with_padding_on_every_side() {
        let mesh = uv_sphere(6.0, 20, 14);
        let grid = voxelize(&mesh, 12).unwrap();
        let b = mesh.bounds();
        let (lo, hi) = sample_box(&grid);
        let pad = grid.spacing * PAD_VOXELS as f32;
        assert!(
            lo.cmple(b.min - pad + Vec3::splat(1e-4)).all(),
            "{lo} vs {}",
            b.min
        );
        assert!(
            hi.cmpge(b.max + pad - Vec3::splat(1e-4)).all(),
            "{hi} vs {}",
            b.max
        );
        assert_eq!(
            grid.data.len(),
            grid.dims[0] as usize * grid.dims[1] as usize * grid.dims[2] as usize
        );
    }

    #[test]
    fn the_winding_number_keeps_the_sign_right_on_a_sphere_with_triangles_deleted() {
        // This is the case that rejects ray parity and justifies the whole
        // approach. A downloaded STL is very often a few triangles short of
        // watertight. Parity would invert the sign of every point behind the
        // hole, all the way across the solid; the winding number is perturbed
        // only in the neighbourhood of the hole itself.
        let radius = 10.0;
        let whole = uv_sphere(radius, 32, 20);
        // The first `segments` triangles are the north polar cap, so removing
        // six of them tears a hole around the pole rather than a slit.
        let torn = with_triangles_removed(&whole, 0, 6);
        assert!(
            !torn.topology().is_printable(),
            "the test mesh must have a hole"
        );

        let bvh = Bvh::from_mesh(&torn);
        // Everything except a cone around the hole, which is at +z.
        let hole = Vec3::new(0.0, 0.0, radius);
        let mut checked = 0;
        for n in 0..2000 {
            let t = n as f32;
            let p = Vec3::new(
                (t * 0.7351).fract() * 2.4 * radius - 1.2 * radius,
                (t * 0.4327).fract() * 2.4 * radius - 1.2 * radius,
                (t * 0.9137).fract() * 2.4 * radius - 1.2 * radius,
            );
            let want = sphere_sdf(radius, p);
            // Skip points near the surface, where the sign is meaningless at
            // this tessellation, and points near the hole, where the field
            // genuinely is not the sphere's any more.
            if want.abs() < 0.3 || (p - hole).length() < 0.5 * radius {
                continue;
            }
            checked += 1;
            assert_eq!(
                inside(&bvh, p),
                want < 0.0,
                "at {p}, analytic distance {want}, winding {}",
                bvh.winding_number(p)
            );
        }
        assert!(checked > 500, "only {checked} points were actually tested");
    }

    #[test]
    fn voxelizing_a_mesh_with_no_triangles_is_rejected() {
        assert_eq!(
            voxelize(&Mesh::default(), 8).unwrap_err(),
            MeshError::EmptyMesh
        );
    }

    #[test]
    fn a_resolution_of_zero_still_produces_a_usable_grid() {
        let grid = voxelize(&unit_box(Vec3::splat(1.0)), 0).unwrap();
        assert!(grid.dims.iter().all(|&d| d >= 2 * PAD_VOXELS));
        assert!(grid.spacing > 0.0);
    }

    #[test]
    fn outside_the_sampled_region_the_reading_never_over_estimates_the_distance() {
        // This is what the padding invariant buys: a tracer that reaches past
        // the grid still gets a step it can take safely.
        let radius = 5.0;
        let mesh = uv_sphere(radius, 32, 20);
        let bvh = Bvh::from_mesh(&mesh);
        let grid = voxelize(&mesh, 16).unwrap();
        let (lo, hi) = sample_box(&grid);

        let mut outside = 0;
        for n in 1..400 {
            let t = n as f32;
            let dir = Vec3::new(t.sin(), (t * 1.7).cos(), (t * 0.3).sin()).normalize();
            let p = dir * (radius + 1.0 + t * 0.2);
            if p.cmpge(lo).all() && p.cmple(hi).all() {
                continue;
            }
            outside += 1;
            let got = grid.sample(p);
            assert!(got > 0.0, "at {p}: grid says {got}, which is inside");
            let truth = bvh.distance(p);
            assert!(got <= truth, "at {p}: grid {got} over-estimated {truth}");
            // And against the shape the mesh approximates, which for an
            // inscribed tessellation is nearer still.
            let analytic = sphere_sdf(radius, p);
            assert!(
                got <= analytic,
                "at {p}: grid {got} over-estimated {analytic}"
            );
        }
        assert!(outside > 100, "only {outside} points were outside the grid");
    }
}
