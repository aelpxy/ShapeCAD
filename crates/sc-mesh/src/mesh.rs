//! Triangle meshes and the checks that decide whether one is printable.

use sc_geom::glam::Vec3;
use sc_geom::Aabb;
use std::collections::HashMap;

/// An indexed triangle mesh.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    /// Vertex positions, in millimetres.
    pub positions: Vec<Vec3>,
    /// Per-vertex normals, parallel to `positions`.
    pub normals: Vec<Vec3>,
    /// Triangles, wound counter-clockwise seen from outside.
    pub indices: Vec<[u32; 3]>,
}

/// What a slicer needs to know about a mesh before it will accept it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Topology {
    /// Number of triangles classified.
    pub triangles: usize,
    /// Edges used by exactly one triangle. Any of these means a hole.
    pub boundary_edges: usize,
    /// Edges used by three or more triangles.
    pub non_manifold_edges: usize,
    /// Edges the triangles around them traverse more often one way than the
    /// other, meaning they disagree about which side is outside.
    ///
    /// On an edge shared by exactly two triangles this is the familiar flipped
    /// face. On one shared by more it is the same disagreement, counted the
    /// same way, because a surface that closes traverses every edge as often in
    /// each direction however many triangles meet there.
    pub inconsistent_edges: usize,
}

impl Topology {
    /// No holes and no disagreement about which side is outside, among the
    /// triangles that are present.
    ///
    /// These are the two defects that actually stop a slicer: a hole leaves the
    /// solid undefined, and inconsistent winding inverts it. Non-manifold edges
    /// are deliberately not included. See [`Topology::is_manifold`].
    ///
    /// Every one of those checks is vacuous on a mesh with no triangles, so
    /// this is true of an empty export. It is not a claim that there is
    /// anything to print: `triangles` is, and [`Topology::is_manifold`] folds it
    /// in.
    #[must_use]
    pub fn is_printable(&self) -> bool {
        self.boundary_edges == 0 && self.inconsistent_edges == 0
    }

    /// A solid: everything [`Topology::is_printable`] checks, on at least one
    /// triangle, and no edge shared by more than two of them.
    ///
    /// Uniform dual contouring places exactly one vertex per cell. Where two
    /// separate sheets of the surface pass through the same cell (a thin gap,
    /// or two bodies almost touching, relative to the grid) that single vertex
    /// has to serve both, and the edges around it end up shared by four
    /// triangles rather than two.
    ///
    /// Most slicers repair this silently, which is why it does not fail
    /// [`Topology::is_printable`], but it is a genuine defect. Fixing it
    /// properly means Manifold Dual Contouring: detecting that a cell's
    /// crossings form more than one connected component and emitting a vertex
    /// for each. Raising the resolution also makes it rarer, since it only
    /// occurs where a feature is finer than a cell.
    ///
    /// The other two conditions are not decoration. Read on its own the
    /// non-manifold count is satisfied by a mesh full of holes, and by an empty
    /// one, neither of which is a solid.
    #[must_use]
    pub fn is_manifold(&self) -> bool {
        self.triangles > 0 && self.is_printable() && self.non_manifold_edges == 0
    }
}

impl Mesh {
    /// Builds an indexed mesh from a triangle soup, welding bit-identical
    /// vertices and recomputing normals.
    ///
    /// STL has no vertex sharing at all, so a file of `n` triangles arrives as
    /// `3n` positions of which roughly `n/2` are distinct. Welding them is what
    /// lets [`Mesh::topology`] say anything useful about an imported file, and
    /// it shrinks the BVH the voxelizer builds over the result.
    ///
    /// Welding is exact rather than tolerance-based. A tolerance would need a
    /// spatial structure and would silently collapse genuinely thin features;
    /// every writer that emits a shared vertex emits the same bits for it.
    ///
    /// # Panics
    /// If welding produces more than `u32::MAX` distinct vertices, which needs
    /// an input of over four billion triangles.
    #[must_use]
    pub fn from_triangles(triangles: &[[Vec3; 3]]) -> Self {
        let mut mesh = Self {
            positions: Vec::new(),
            normals: Vec::new(),
            indices: Vec::with_capacity(triangles.len()),
        };
        let mut seen: HashMap<[u32; 3], u32> = HashMap::new();
        for tri in triangles {
            let mut face = [0u32; 3];
            for (slot, &p) in face.iter_mut().zip(tri.iter()) {
                // Negative zero and positive zero are the same point but not
                // the same bits, so fold one onto the other before hashing.
                let p = p + Vec3::ZERO;
                let key = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
                *slot = *seen.entry(key).or_insert_with(|| {
                    mesh.positions.push(p);
                    u32::try_from(mesh.positions.len() - 1).expect("vertex count fits in u32")
                });
            }
            mesh.indices.push(face);
        }
        mesh.recompute_normals();
        mesh
    }

    /// Number of triangles.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.indices.len()
    }

    /// Replaces `normals` with the area-weighted average of the incident face
    /// normals.
    ///
    /// Imported files either carry no normals at all (OBJ without `vn`) or
    /// carry per-face normals that a writer may have got wrong, so an importer
    /// recomputes rather than trusts. Weighting by the cross product length
    /// rather than normalizing first means a sliver triangle does not pull a
    /// vertex normal around as hard as a large one.
    pub fn recompute_normals(&mut self) {
        self.normals = vec![Vec3::ZERO; self.positions.len()];
        for &[a, b, c] in &self.indices {
            let (pa, pb, pc) = (
                self.positions[a as usize],
                self.positions[b as usize],
                self.positions[c as usize],
            );
            let n = (pb - pa).cross(pc - pa);
            for i in [a, b, c] {
                self.normals[i as usize] += n;
            }
        }
        for n in &mut self.normals {
            *n = n.normalize_or_zero();
        }
    }

    /// Bounding box of the vertices.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        self.positions
            .iter()
            .fold(Aabb::EMPTY, |b, &p| b.union(Aabb { min: p, max: p }))
    }

    /// Enclosed volume in cubic millimetres, by the divergence theorem.
    ///
    /// Only meaningful for a closed mesh. A negative result means the winding is
    /// inside out, which is why the mesher's tests assert on the sign.
    #[must_use]
    pub fn volume(&self) -> f32 {
        self.indices
            .iter()
            .map(|&[a, b, c]| {
                let (a, b, c) = (
                    self.positions[a as usize],
                    self.positions[b as usize],
                    self.positions[c as usize],
                );
                a.dot(b.cross(c)) / 6.0
            })
            .sum()
    }

    /// Classifies every edge.
    ///
    /// This is the check that matters. "Watertight by construction" is a claim
    /// about the algorithm; this measures it on the actual output.
    #[must_use]
    pub fn topology(&self) -> Topology {
        // Keyed by the unordered pair, counting each direction separately.
        let mut edges: HashMap<(u32, u32), (u32, u32)> = HashMap::new();
        for &[a, b, c] in &self.indices {
            for (from, to) in [(a, b), (b, c), (c, a)] {
                let key = (from.min(to), from.max(to));
                let slot = edges.entry(key).or_insert((0, 0));
                if from < to {
                    slot.0 += 1;
                } else {
                    slot.1 += 1;
                }
            }
        }

        let mut report = Topology {
            triangles: self.indices.len(),
            boundary_edges: 0,
            non_manifold_edges: 0,
            inconsistent_edges: 0,
        };
        for (forward, backward) in edges.values().copied() {
            match forward + backward {
                0 | 1 => report.boundary_edges += 1,
                2 => {}
                _ => report.non_manifold_edges += 1,
            }
            // A closed, consistently wound surface traverses every edge as
            // often one way as the other. Checking that, rather than only the
            // healthy (1, 1) case, catches the same defect on an edge shared by
            // more than two triangles, where the old test simply did not look: a
            // triangle emitted twice with the same winding left three edges at
            // (2, 1) and the mesh still called itself printable.
            if forward != backward {
                report.inconsistent_edges += 1;
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{unit_box, with_triangles_removed};

    #[test]
    fn a_closed_box_passes_every_check() {
        let t = unit_box(Vec3::splat(1.0)).topology();
        assert_eq!(t.triangles, 12);
        assert_eq!(t.boundary_edges, 0);
        assert_eq!(t.non_manifold_edges, 0);
        assert_eq!(t.inconsistent_edges, 0);
        assert!(t.is_printable() && t.is_manifold(), "{t:?}");
    }

    #[test]
    fn a_mesh_with_a_hole_is_neither_printable_nor_manifold() {
        // `is_manifold` says "additionally", so it has to include what
        // `is_printable` checks. On its own it called a mesh with three open
        // edges manifold, which is true of the edge count and useless as an
        // answer to "can this be printed".
        let torn = with_triangles_removed(&unit_box(Vec3::splat(1.0)), 0, 1);
        let t = torn.topology();
        assert_eq!(t.boundary_edges, 3, "{t:?}");
        assert!(!t.is_printable(), "{t:?}");
        assert!(!t.is_manifold(), "{t:?}");
    }

    #[test]
    fn a_triangle_emitted_twice_is_not_printable() {
        // Duplicating a face leaves a zero-thickness flap: three edges shared
        // by three triangles, two traversals one way and one the other. The
        // winding disagrees, and counting that only on two-triangle edges
        // missed it entirely.
        let mut mesh = unit_box(Vec3::splat(1.0));
        mesh.indices.push(mesh.indices[0]);
        let t = mesh.topology();
        assert_eq!(t.non_manifold_edges, 3, "{t:?}");
        assert_eq!(t.inconsistent_edges, 3, "{t:?}");
        assert!(!t.is_printable(), "{t:?}");
    }

    #[test]
    fn a_single_flipped_triangle_is_caught() {
        let mut mesh = unit_box(Vec3::splat(1.0));
        mesh.indices[0].swap(1, 2);
        let t = mesh.topology();
        assert_eq!(t.inconsistent_edges, 3, "{t:?}");
        assert!(!t.is_printable(), "{t:?}");
    }

    #[test]
    fn an_empty_mesh_encloses_nothing_and_is_not_manifold() {
        // Every edge check passes vacuously on a mesh with no edges. That is
        // why the triangle count is part of the report: a slicer handed an
        // empty file has no solid, whatever the edge counts say.
        let t = Mesh::default().topology();
        assert_eq!(t.triangles, 0);
        assert!(!t.is_manifold(), "{t:?}");
    }
}
