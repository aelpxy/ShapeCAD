//! Dual contouring of the implicit field.
//!
//! Marching cubes places vertices on grid edges and rounds every sharp feature
//! off. Dual contouring places one vertex per *cell*, positioned to best satisfy
//! the tangent planes of that cell's surface crossings, which reproduces corners
//! and creases exactly. For mechanical parts that is the difference between a
//! box and a pillow.
//!
//! The output is closed by construction: the sampling grid is padded so the
//! solid never reaches its boundary, and a quad is emitted for every grid edge
//! that changes sign. [`crate::Mesh::topology`] measures that claim rather than
//! assuming it.

use crate::mesh::Mesh;
use crate::qef::Qef;
use rayon::prelude::*;
use sc_geom::eval::{eval, normal};
use sc_geom::glam::Vec3;
use sc_geom::{bounds, Arena, NodeId};

/// Meshing parameters.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Cells along the longest axis of the model.
    ///
    /// Cell size follows from this and the part's size; the mesher reports the
    /// resulting millimetres per cell so it can be compared against a layer
    /// height.
    pub resolution: u32,
    /// Bisection steps used to locate each surface crossing along a grid edge.
    ///
    /// Linear interpolation is already close for a distance field, since the
    /// field is near-linear near the surface. A couple of refinements removes
    /// the residual error where it is not.
    pub refinement: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            resolution: 128,
            refinement: 2,
        }
    }
}

/// Padding around the model, in cells.
///
/// Guarantees the solid never touches the grid boundary. A surface clipped by
/// the boundary would leave a hole, and a hole is the one defect that makes a
/// mesh unprintable.
const PAD_CELLS: u32 = 2;

/// Marks a cell that produced no vertex.
const NO_VERTEX: u32 = u32::MAX;

/// A sampled scalar field on a regular grid.
struct Grid {
    origin: Vec3,
    spacing: f32,
    /// Sample counts per axis. Cell counts are one less in each direction.
    dims: [usize; 3],
    values: Vec<f32>,
}

impl Grid {
    fn cells(&self) -> [usize; 3] {
        [self.dims[0] - 1, self.dims[1] - 1, self.dims[2] - 1]
    }

    fn position(&self, i: usize, j: usize, k: usize) -> Vec3 {
        self.origin + self.spacing * Vec3::new(i as f32, j as f32, k as f32)
    }

    fn at(&self, i: usize, j: usize, k: usize) -> f32 {
        self.values[(k * self.dims[1] + j) * self.dims[0] + i]
    }

    /// Sample the field over the model's padded bounds.
    fn sample(arena: &Arena, root: NodeId, settings: Settings) -> Self {
        let b = bounds(arena, root).finite_or(1000.0);
        let size = b.size();
        let spacing = (size.max_element() / settings.resolution.max(1) as f32).max(1.0e-5);

        let pad = spacing * PAD_CELLS as f32;
        let origin = b.min - Vec3::splat(pad);
        let span = size + Vec3::splat(2.0 * pad);

        let dims = [
            (span.x / spacing).ceil() as usize + 1,
            (span.y / spacing).ceil() as usize + 1,
            (span.z / spacing).ceil() as usize + 1,
        ];

        let plane = dims[0] * dims[1];
        let mut values = vec![0.0f32; plane * dims[2]];
        // Sampling dominates the cost and is trivially parallel over slices.
        values
            .par_chunks_mut(plane)
            .enumerate()
            .for_each(|(k, slice)| {
                for j in 0..dims[1] {
                    for i in 0..dims[0] {
                        let p = origin + spacing * Vec3::new(i as f32, j as f32, k as f32);
                        slice[j * dims[0] + i] = eval(arena, root, p);
                    }
                }
            });

        Self {
            origin,
            spacing,
            dims,
            values,
        }
    }
}

/// The twelve edges of a cell, as pairs of corner offsets.
const CELL_EDGES: [([usize; 3], [usize; 3]); 12] = [
    ([0, 0, 0], [1, 0, 0]),
    ([0, 1, 0], [1, 1, 0]),
    ([0, 0, 1], [1, 0, 1]),
    ([0, 1, 1], [1, 1, 1]),
    ([0, 0, 0], [0, 1, 0]),
    ([1, 0, 0], [1, 1, 0]),
    ([0, 0, 1], [0, 1, 1]),
    ([1, 0, 1], [1, 1, 1]),
    ([0, 0, 0], [0, 0, 1]),
    ([1, 0, 0], [1, 0, 1]),
    ([0, 1, 0], [0, 1, 1]),
    ([1, 1, 0], [1, 1, 1]),
];

/// Locate the surface along a segment whose endpoints straddle it.
fn crossing(
    arena: &Arena,
    root: NodeId,
    mut a: Vec3,
    mut da: f32,
    mut b: Vec3,
    mut db: f32,
    refinement: u32,
) -> Vec3 {
    for _ in 0..refinement {
        let t = if (da - db).abs() > f32::EPSILON {
            da / (da - db)
        } else {
            0.5
        };
        let mid = a.lerp(b, t.clamp(0.0, 1.0));
        let dm = eval(arena, root, mid);
        if (dm < 0.0) == (da < 0.0) {
            a = mid;
            da = dm;
        } else {
            b = mid;
            db = dm;
        }
    }
    let t = if (da - db).abs() > f32::EPSILON {
        da / (da - db)
    } else {
        0.5
    };
    a.lerp(b, t.clamp(0.0, 1.0))
}

/// Solves one vertex per cell, independently and in parallel.
///
/// A cell with no sign change on any of its twelve edges is not touched by the
/// surface and yields nothing.
fn solve_cells(arena: &Arena, root: NodeId, grid: &Grid, settings: Settings) -> Vec<Option<Vec3>> {
    let [ncx, ncy, ncz] = grid.cells();
    (0..ncx * ncy * ncz)
        .into_par_iter()
        .map(|index| {
            let ci = index % ncx;
            let cj = (index / ncx) % ncy;
            let ck = index / (ncx * ncy);

            let mut qef = Qef::new();
            for (lo, hi) in CELL_EDGES {
                let (i0, j0, k0) = (ci + lo[0], cj + lo[1], ck + lo[2]);
                let (i1, j1, k1) = (ci + hi[0], cj + hi[1], ck + hi[2]);
                let (d0, d1) = (grid.at(i0, j0, k0), grid.at(i1, j1, k1));
                if (d0 < 0.0) == (d1 < 0.0) {
                    continue;
                }
                let p = crossing(
                    arena,
                    root,
                    grid.position(i0, j0, k0),
                    d0,
                    grid.position(i1, j1, k1),
                    d1,
                    settings.refinement,
                );
                qef.add(p, normal(arena, root, p, grid.spacing * 0.05));
            }

            if qef.count() == 0 {
                return None;
            }
            let min = grid.position(ci, cj, ck);
            Some(qef.solve(min, min + Vec3::splat(grid.spacing)))
        })
        .collect()
}

/// Emits one quad per sign-changing grid edge, joining the four cells around it.
///
/// Winding follows the right-hand rule for the edge's axis and is reversed when
/// the solid lies on the far side, so every face ends up pointing outward.
fn emit_quads(grid: &Grid, index_of: &[u32], mesh: &mut Mesh) {
    let [ncx, ncy, ncz] = grid.cells();
    let cell = |ci: usize, cj: usize, ck: usize| index_of[(ck * ncy + cj) * ncx + ci];

    let mut quad = |v: [u32; 4], flip: bool| {
        if v.contains(&NO_VERTEX) {
            return;
        }
        let [a, b, c, d] = if flip { [v[0], v[3], v[2], v[1]] } else { v };
        mesh.indices.push([a, b, c]);
        mesh.indices.push([a, c, d]);
    };

    for k in 0..grid.dims[2] {
        for j in 0..grid.dims[1] {
            for i in 0..grid.dims[0] {
                // X edge: normal +X is Y cross Z, so wind Y then Z.
                if i < ncx && j >= 1 && k >= 1 && j < ncy && k < ncz {
                    let (d0, d1) = (grid.at(i, j, k), grid.at(i + 1, j, k));
                    if (d0 < 0.0) != (d1 < 0.0) {
                        let v = [
                            cell(i, j - 1, k - 1),
                            cell(i, j, k - 1),
                            cell(i, j, k),
                            cell(i, j - 1, k),
                        ];
                        quad(v, d0 >= 0.0);
                    }
                }
                // Y edge: normal +Y is Z cross X, so wind Z then X.
                if j < ncy && i >= 1 && k >= 1 && i < ncx && k < ncz {
                    let (d0, d1) = (grid.at(i, j, k), grid.at(i, j + 1, k));
                    if (d0 < 0.0) != (d1 < 0.0) {
                        let v = [
                            cell(i - 1, j, k - 1),
                            cell(i - 1, j, k),
                            cell(i, j, k),
                            cell(i, j, k - 1),
                        ];
                        quad(v, d0 >= 0.0);
                    }
                }
                // Z edge: normal +Z is X cross Y, so wind X then Y.
                if k < ncz && i >= 1 && j >= 1 && i < ncx && j < ncy {
                    let (d0, d1) = (grid.at(i, j, k), grid.at(i, j, k + 1));
                    if (d0 < 0.0) != (d1 < 0.0) {
                        let v = [
                            cell(i - 1, j - 1, k),
                            cell(i, j - 1, k),
                            cell(i, j, k),
                            cell(i - 1, j, k),
                        ];
                        quad(v, d0 >= 0.0);
                    }
                }
            }
        }
    }
}

/// Meshes the solid rooted at `root`.
///
/// Returns an empty mesh if the root is missing or encloses nothing.
///
/// # Panics
/// If the model produces more than `u32::MAX` vertices, which at any sane
/// resolution would exhaust memory long before it was reached.
#[must_use]
pub fn contour(arena: &Arena, root: NodeId, settings: Settings) -> Mesh {
    let grid = Grid::sample(arena, root, settings);
    let [ncx, ncy, ncz] = grid.cells();
    if ncx == 0 || ncy == 0 || ncz == 0 {
        return Mesh::default();
    }

    let cell_vertices = solve_cells(arena, root, &grid, settings);

    // Assign indices to the cells that produced a vertex.
    let mut mesh = Mesh::default();
    let mut index_of = vec![NO_VERTEX; cell_vertices.len()];
    for (cell, vertex) in cell_vertices.iter().enumerate() {
        if let Some(p) = *vertex {
            index_of[cell] = u32::try_from(mesh.positions.len()).expect("vertex count fits u32");
            mesh.positions.push(p);
        }
    }
    mesh.normals = mesh
        .positions
        .par_iter()
        .map(|&p| normal(arena, root, p, grid.spacing * 0.05))
        .collect();

    emit_quads(&grid, &index_of, &mut mesh);
    mesh
}
