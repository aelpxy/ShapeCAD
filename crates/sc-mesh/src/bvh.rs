//! A bounding volume hierarchy over triangles.
//!
//! Voxelizing an imported mesh asks two questions at every one of hundreds of
//! thousands of sample points: how far is the nearest triangle, and is the point
//! inside. Both are linear in the triangle count without an acceleration
//! structure, and the product of the two counts is what makes the naive version
//! unusable rather than merely slow.
//!
//! The tree is a plain binary BVH: split the widest axis of the current range at
//! the median centroid, stop at [`LEAF_TRIANGLES`]. A surface area heuristic
//! would build a better tree, but the median split is a few lines, is
//! deterministic, and the queries here are point queries rather than rays, where
//! the difference is small.
//!
//! Every node also carries the aggregate dipole of its subtree, which is what
//! makes the generalized winding number affordable. See
//! [`Bvh::winding_number`].

// Geometry names points, vertices and edge dot products `p`, `a`, `b`, `d1` by
// long convention, and the closest-point and solid-angle formulae are far
// easier to check against their published forms with those names intact.
#![allow(clippy::many_single_char_names)]

use crate::mesh::Mesh;
use sc_geom::glam::{DVec3, Vec3};
use sc_geom::Aabb;

/// Triangles per leaf.
///
/// Small enough that a leaf's triangles are nearly always a better answer than
/// another level of bounds test, large enough that the tree is not mostly
/// interior nodes.
pub const LEAF_TRIANGLES: usize = 4;

/// Ratio of query distance to node radius at which a subtree's winding number
/// contribution is approximated rather than summed triangle by triangle.
///
/// This is the `beta` of Barill et al., "Fast Winding Numbers for Soups and
/// Clouds". They report that 2.0 is safe for their higher order expansion; only
/// the leading (point dipole) term is used here, so the test is tightened to
/// keep the truncated terms below the accuracy the sign decision needs.
const FAR_FIELD_RATIO: f32 = 4.0;

/// A triangle with its vertices resolved, wound counter-clockwise from outside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    /// First vertex.
    pub a: Vec3,
    /// Second vertex.
    pub b: Vec3,
    /// Third vertex.
    pub c: Vec3,
}

impl Triangle {
    /// The tightest axis-aligned box containing the triangle.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        Aabb {
            min: self.a.min(self.b).min(self.c),
            max: self.a.max(self.b).max(self.c),
        }
    }

    /// The mean of the three vertices, used as the split key.
    #[must_use]
    pub fn centroid(&self) -> Vec3 {
        (self.a + self.b + self.c) / 3.0
    }

    /// Twice the area times the unit normal, so a degenerate triangle
    /// contributes nothing rather than a division by zero.
    #[must_use]
    pub fn area_normal(&self) -> Vec3 {
        (self.b - self.a).cross(self.c - self.a) * 0.5
    }

    /// The point of the triangle nearest `p`.
    ///
    /// Voronoi region test from Ericson, *Real-Time Collision Detection*,
    /// section 5.1.5. It is branch-heavy but exact on the edges and corners,
    /// which is where a projection-and-clamp version goes wrong.
    ///
    /// A triangle with a repeated corner is a segment and is answered as one.
    #[must_use]
    pub fn closest_point(&self, p: Vec3) -> Vec3 {
        let (a, b, c) = (self.a, self.b, self.c);
        let (ab, ac, ap) = (b - a, c - a, p - a);

        // Ericson's region tests assume a triangle with area. Without it the
        // barycentric denominators vanish and the region tests stop
        // partitioning space: a coincident pair makes the a-b region match
        // every query and collapse the answer onto `a`, and three collinear
        // corners misclassify by up to the length of the sliver. Neither is
        // exotic. Welding in `Mesh::from_triangles` turns any facet naming a
        // vertex twice into the first, and the second is what a badly
        // triangulated face from a CAD exporter looks like.
        //
        // A triangle with no area is a segment, so answer it as the segment it
        // is: the two corners furthest apart, since the third lies between
        // them.
        // Relative, not absolute. The cross product's magnitude scales with the
        // triangle's size, so an absolute threshold either misses a large
        // sliver or swallows a small honest triangle. Dividing by the edge
        // lengths leaves the squared sine of the corner angle, which is the
        // shape rather than the scale.
        let normal = ab.cross(ac);
        let scale = ab.length_squared() * ac.length_squared();
        if scale <= 0.0 || normal.length_squared() <= FLAT_SINE_SQUARED * scale {
            let (u, v) = longest_edge(a, b, c);
            return closest_on_segment(u, v, p);
        }

        let d1 = ab.dot(ap);
        let d2 = ac.dot(ap);
        if d1 <= 0.0 && d2 <= 0.0 {
            return a;
        }

        let bp = p - b;
        let d3 = ab.dot(bp);
        let d4 = ac.dot(bp);
        if d3 >= 0.0 && d4 <= d3 {
            return b;
        }

        let vc = d1 * d4 - d3 * d2;
        if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
            let v = safe_ratio(d1, d1 - d3);
            return a + ab * v;
        }

        let cp = p - c;
        let d5 = ab.dot(cp);
        let d6 = ac.dot(cp);
        if d6 >= 0.0 && d5 <= d6 {
            return c;
        }

        let vb = d5 * d2 - d1 * d6;
        if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
            let w = safe_ratio(d2, d2 - d6);
            return a + ac * w;
        }

        let va = d3 * d6 - d5 * d4;
        if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
            let w = safe_ratio(d4 - d3, (d4 - d3) + (d5 - d6));
            return b + (c - b) * w;
        }

        let denom = va + vb + vc;
        if denom == 0.0 {
            // Fully degenerate: the three vertices are collinear and every
            // Voronoi test above fell through. Any vertex is as good as another.
            return a;
        }
        let v = vb / denom;
        let w = vc / denom;
        a + ab * v + ac * w
    }

    /// Signed solid angle the triangle subtends at `p`, in steradians.
    ///
    /// Van Oosterom and Strackee's formula. Computed in double precision
    /// because the numerator is a triple product of differences, which loses
    /// most of its significant digits when `p` is far from the triangle
    /// relative to the triangle's size, and those are exactly the terms that
    /// have to cancel to zero outside a closed surface.
    #[must_use]
    pub fn solid_angle(&self, p: Vec3) -> f64 {
        let q = p.as_dvec3();
        let (a, b, c): (DVec3, DVec3, DVec3) = (
            self.a.as_dvec3() - q,
            self.b.as_dvec3() - q,
            self.c.as_dvec3() - q,
        );
        let (la, lb, lc) = (a.length(), b.length(), c.length());

        let num = a.dot(b.cross(c));
        let den = la * lb * lc + a.dot(b) * lc + b.dot(c) * la + c.dot(a) * lb;
        if num == 0.0 && den == 0.0 {
            // `p` coincides with a vertex. The solid angle is undefined there;
            // zero keeps the sum finite and the point is on the surface anyway.
            return 0.0;
        }
        2.0 * num.atan2(den)
    }
}

/// Guards the parametric divides in the Voronoi test against a zero-length edge.
fn safe_ratio(num: f32, den: f32) -> f32 {
    if den == 0.0 {
        0.0
    } else {
        (num / den).clamp(0.0, 1.0)
    }
}

/// The point of the segment `a..b` nearest `p`, or `a` if the segment is a
/// point.
/// Squared sine of the corner angle below which a triangle is treated as a
/// segment.
///
/// A hundredth of a degree squared, give or take: a triangle this thin is a
/// line as far as anything downstream is concerned, and answering it as one is
/// wrong by at most its own width. On a 100mm triangle that is a thousandth of
/// a millimetre, well under the tracer's tolerance and far under anything a
/// printer resolves. Against the barycentric path, which divides two quantities
/// both at the f32 noise floor at this shape, it is the better answer: a
/// measured counterexample was off by 0.04mm.
const FLAT_SINE_SQUARED: f32 = 1.0e-10;

/// The two corners furthest apart, for a triangle that has collapsed to a line.
fn longest_edge(a: Vec3, b: Vec3, c: Vec3) -> (Vec3, Vec3) {
    let (ab, bc, ca) = (
        (b - a).length_squared(),
        (c - b).length_squared(),
        (a - c).length_squared(),
    );
    if ab >= bc && ab >= ca {
        (a, b)
    } else if bc >= ca {
        (b, c)
    } else {
        (c, a)
    }
}

fn closest_on_segment(a: Vec3, b: Vec3, p: Vec3) -> Vec3 {
    let ab = b - a;
    a + ab * safe_ratio(ab.dot(p - a), ab.length_squared())
}

/// One node of the tree.
#[derive(Clone, Copy, Debug)]
struct BvhNode {
    bounds: Aabb,
    /// Sum of `area * unit normal` over the subtree. The dipole moment of the
    /// subtree seen as a sheet of sources.
    dipole: Vec3,
    /// Area-weighted mean position of the subtree's triangles.
    dipole_origin: Vec3,
    /// Distance from `dipole_origin` to the farthest corner of `bounds`.
    radius: f32,
    /// Leaf: index of the first triangle. Interior: index of the right child.
    /// The left child of an interior node always follows it immediately.
    first: u32,
    /// Triangles in the leaf, or zero for an interior node.
    count: u32,
}

/// A binary BVH over a fixed set of triangles.
#[derive(Clone, Debug)]
pub struct Bvh {
    triangles: Vec<Triangle>,
    nodes: Vec<BvhNode>,
}

/// The result of a nearest-triangle query.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Nearest {
    /// The nearest point on the mesh.
    pub point: Vec3,
    /// Unsigned distance from the query point to `point`.
    pub distance: f32,
    /// Index into [`Bvh::triangles`], which is not the caller's triangle order.
    pub triangle: usize,
}

impl Bvh {
    /// Builds a tree over `triangles`.
    ///
    /// The input order is not preserved; [`Nearest::triangle`] indexes
    /// [`Bvh::triangles`].
    #[must_use]
    pub fn build(triangles: Vec<Triangle>) -> Self {
        let mut bvh = Self {
            triangles,
            nodes: Vec::new(),
        };
        if bvh.triangles.is_empty() {
            return bvh;
        }
        // A median split halves the range each level, so the node count is
        // bounded by twice the leaf count.
        let leaves = bvh.triangles.len().div_ceil(LEAF_TRIANGLES);
        bvh.nodes.reserve(2 * leaves + 1);
        let len = bvh.triangles.len();
        bvh.build_range(0, len);
        bvh
    }

    /// Builds a tree over the triangles of `mesh`.
    #[must_use]
    pub fn from_mesh(mesh: &Mesh) -> Self {
        let triangles = mesh
            .indices
            .iter()
            .map(|&[a, b, c]| Triangle {
                a: mesh.positions[a as usize],
                b: mesh.positions[b as usize],
                c: mesh.positions[c as usize],
            })
            .collect();
        Self::build(triangles)
    }

    /// The triangles, in tree order.
    #[must_use]
    pub fn triangles(&self) -> &[Triangle] {
        &self.triangles
    }

    /// Number of nodes, which is a proxy for tree shape in tests.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Bounding box of every triangle in the tree, empty if there are none.
    #[must_use]
    pub fn bounds(&self) -> Aabb {
        self.nodes.first().map_or(Aabb::EMPTY, |n| n.bounds)
    }

    /// Recursively builds the node covering `triangles[start..end]`, returning
    /// its index. Partitions the slice in place as it goes.
    fn build_range(&mut self, start: usize, end: usize) -> u32 {
        let index = u32::try_from(self.nodes.len()).expect("node count fits in u32");
        let node = summarize(&self.triangles[start..end], start);
        self.nodes.push(node);

        if end - start <= LEAF_TRIANGLES {
            return index;
        }

        let axis = widest_axis(node.bounds);
        let mid = start + (end - start) / 2;
        // Tie-break on the remaining axes so the partition is a total order and
        // the tree does not depend on the unspecified ordering that
        // `select_nth_unstable_by` leaves behind.
        self.triangles[start..end].select_nth_unstable_by(mid - start, |x, y| {
            split_key(x, axis)
                .partial_cmp(&split_key(y, axis))
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let left = self.build_range(start, mid);
        debug_assert_eq!(left, index + 1, "left child must follow its parent");
        let right = self.build_range(mid, end);
        self.nodes[index as usize].first = right;
        self.nodes[index as usize].count = 0;
        index
    }

    /// The nearest point on any triangle, or `None` for an empty tree.
    #[must_use]
    pub fn nearest(&self, p: Vec3) -> Option<Nearest> {
        if self.nodes.is_empty() {
            return None;
        }
        let mut best = Nearest {
            point: Vec3::ZERO,
            distance: f32::INFINITY,
            triangle: 0,
        };
        let mut best_sq = f32::INFINITY;
        let mut stack = vec![0u32];
        while let Some(index) = stack.pop() {
            let node = self.nodes[index as usize];
            if box_distance_sq(node.bounds, p) >= best_sq {
                continue;
            }
            if node.count > 0 {
                for t in node.first as usize..node.first as usize + node.count as usize {
                    let q = self.triangles[t].closest_point(p);
                    let d = (q - p).length_squared();
                    if d < best_sq {
                        best_sq = d;
                        best = Nearest {
                            point: q,
                            distance: 0.0,
                            triangle: t,
                        };
                    }
                }
                continue;
            }
            // Descend into the nearer child first so the other one is more
            // likely to be culled outright when it is popped.
            let (l, r) = (index + 1, node.first);
            let dl = box_distance_sq(self.nodes[l as usize].bounds, p);
            let dr = box_distance_sq(self.nodes[r as usize].bounds, p);
            if dl <= dr {
                stack.push(r);
                stack.push(l);
            } else {
                stack.push(l);
                stack.push(r);
            }
        }
        best.distance = best_sq.sqrt();
        Some(best)
    }

    /// Unsigned distance to the nearest triangle, infinite for an empty tree.
    #[must_use]
    pub fn distance(&self, p: Vec3) -> f32 {
        self.nearest(p).map_or(f32::INFINITY, |n| n.distance)
    }

    /// The generalized winding number at `p`.
    ///
    /// One inside a closed outward-wound surface, zero outside, and varying
    /// smoothly in between where the surface is not closed. Subtrees far enough
    /// from `p` are approximated by their aggregate dipole, so a query costs
    /// roughly the log of the triangle count rather than all of it.
    #[must_use]
    pub fn winding_number(&self, p: Vec3) -> f32 {
        if self.nodes.is_empty() {
            return 0.0;
        }
        let mut total = 0.0f64;
        let mut stack = vec![0u32];
        while let Some(index) = stack.pop() {
            let node = self.nodes[index as usize];
            let offset = node.dipole_origin - p;
            let dist = offset.length();
            if dist > FAR_FIELD_RATIO * node.radius {
                total += f64::from(node.dipole.dot(offset)) / f64::from(dist).powi(3);
                continue;
            }
            if node.count > 0 {
                for t in node.first as usize..node.first as usize + node.count as usize {
                    total += self.triangles[t].solid_angle(p);
                }
            } else {
                stack.push(node.first);
                stack.push(index + 1);
            }
        }
        (total / (4.0 * std::f64::consts::PI)) as f32
    }

    /// The generalized winding number summed over every triangle.
    ///
    /// Ground truth for the approximation in [`Bvh::winding_number`], and the
    /// reason that approximation can be tested rather than trusted.
    #[must_use]
    pub fn winding_number_exact(&self, p: Vec3) -> f32 {
        let total: f64 = self.triangles.iter().map(|t| t.solid_angle(p)).sum();
        (total / (4.0 * std::f64::consts::PI)) as f32
    }
}

/// Orders triangles along `axis`, breaking ties on the other two axes so that
/// no two distinct centroids compare equal.
fn split_key(t: &Triangle, axis: usize) -> [f32; 3] {
    let c = t.centroid();
    [c[axis], c[(axis + 1) % 3], c[(axis + 2) % 3]]
}

/// Builds a leaf node summarizing `span`, whose triangles begin at `start`.
fn summarize(span: &[Triangle], start: usize) -> BvhNode {
    let mut bounds = Aabb::EMPTY;
    let mut dipole = Vec3::ZERO;
    let mut weighted = Vec3::ZERO;
    let mut area = 0.0f32;
    for t in span {
        bounds = bounds.union(t.bounds());
        let an = t.area_normal();
        dipole += an;
        let a = an.length();
        area += a;
        weighted += t.centroid() * a;
    }
    // Zero total area means every triangle is degenerate, in which case the
    // dipole is zero too and the far field test can use the box centre.
    let dipole_origin = if area > 0.0 {
        weighted / area
    } else {
        bounds.center()
    };
    let radius = (bounds.max - dipole_origin)
        .max(dipole_origin - bounds.min)
        .length();
    BvhNode {
        bounds,
        dipole,
        dipole_origin,
        radius,
        first: u32::try_from(start).expect("triangle count fits in u32"),
        count: u32::try_from(span.len()).expect("triangle count fits in u32"),
    }
}

/// Index of the longest axis of `b`.
fn widest_axis(b: Aabb) -> usize {
    let s = b.size();
    if s.x >= s.y && s.x >= s.z {
        0
    } else if s.y >= s.z {
        1
    } else {
        2
    }
}

/// Squared distance from `p` to the box, zero if inside.
fn box_distance_sq(b: Aabb, p: Vec3) -> f32 {
    (b.min - p).max(p - b.max).max(Vec3::ZERO).length_squared()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{unit_box, uv_sphere};

    fn tri(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> Triangle {
        Triangle {
            a: Vec3::from_array(a),
            b: Vec3::from_array(b),
            c: Vec3::from_array(c),
        }
    }

    #[test]
    fn closest_point_lands_in_the_face_edge_and_corner_regions() {
        let t = tri([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        // Above the interior: projects straight down.
        assert_eq!(
            t.closest_point(Vec3::new(0.25, 0.25, 3.0)),
            Vec3::new(0.25, 0.25, 0.0)
        );
        // Past the a-b edge: clamps onto the edge.
        assert_eq!(
            t.closest_point(Vec3::new(0.5, -2.0, 0.0)),
            Vec3::new(0.5, 0.0, 0.0)
        );
        // Past the a corner: clamps onto the corner.
        assert_eq!(t.closest_point(Vec3::new(-3.0, -3.0, 0.0)), Vec3::ZERO);
    }

    #[test]
    fn nearest_matches_brute_force_on_every_triangle() {
        let mesh = uv_sphere(7.0, 16, 10);
        let bvh = Bvh::from_mesh(&mesh);
        let tris = bvh.triangles().to_vec();
        for i in 0..40 {
            let a = i as f32 * 0.79;
            let p = Vec3::new(a.sin() * 11.0, a.cos() * 4.0, (a * 1.7).sin() * 9.0);
            let brute = tris
                .iter()
                .map(|t| (t.closest_point(p) - p).length())
                .fold(f32::INFINITY, f32::min);
            let got = bvh.distance(p);
            assert!(
                (got - brute).abs() < 1e-4,
                "at {p}: bvh {got} vs brute force {brute}"
            );
        }
    }

    #[test]
    fn every_triangle_is_reachable_and_appears_once() {
        let mesh = uv_sphere(3.0, 13, 9);
        let bvh = Bvh::from_mesh(&mesh);
        let mut seen = vec![0u32; bvh.triangles().len()];
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let n = bvh.nodes[i as usize];
            if n.count > 0 {
                for t in n.first..n.first + n.count {
                    seen[t as usize] += 1;
                }
            } else {
                stack.push(i + 1);
                stack.push(n.first);
            }
        }
        assert!(
            seen.iter().all(|&c| c == 1),
            "leaf ranges must tile the triangles"
        );
    }

    #[test]
    fn build_is_deterministic() {
        let mesh = uv_sphere(2.0, 15, 11);
        let a = Bvh::from_mesh(&mesh);
        let b = Bvh::from_mesh(&mesh);
        assert_eq!(a.node_count(), b.node_count());
        assert_eq!(a.triangles(), b.triangles());
    }

    #[test]
    fn winding_number_is_one_inside_a_closed_sphere_and_zero_outside() {
        let bvh = Bvh::from_mesh(&uv_sphere(5.0, 32, 20));
        assert!((bvh.winding_number_exact(Vec3::ZERO) - 1.0).abs() < 1e-4);
        assert!((bvh.winding_number_exact(Vec3::new(2.0, 1.0, -1.5)) - 1.0).abs() < 1e-4);
        assert!(bvh.winding_number_exact(Vec3::new(0.0, 0.0, 40.0)).abs() < 1e-4);
        assert!(bvh.winding_number_exact(Vec3::new(-9.0, 9.0, 9.0)).abs() < 1e-4);
    }

    #[test]
    fn winding_number_is_one_inside_a_box() {
        let bvh = Bvh::from_mesh(&unit_box(Vec3::new(2.0, 3.0, 1.0)));
        assert!((bvh.winding_number_exact(Vec3::new(0.5, -1.0, 0.25)) - 1.0).abs() < 1e-4);
        assert!(bvh.winding_number_exact(Vec3::new(0.5, -1.0, 5.0)).abs() < 1e-4);
    }

    #[test]
    fn the_dipole_approximation_tracks_the_exact_sum() {
        let bvh = Bvh::from_mesh(&uv_sphere(5.0, 40, 24));
        let mut worst = 0.0f32;
        for i in 0..200 {
            let a = i as f32 * 0.61;
            let r = 0.5 + (i % 37) as f32 * 0.55;
            let p = Vec3::new(a.sin() * r, a.cos() * r, (a * 2.3).sin() * r);
            let d = (bvh.winding_number(p) - bvh.winding_number_exact(p)).abs();
            worst = worst.max(d);
        }
        // The sign decision compares against 0.5, so anything well under that
        // margin cannot flip a voxel that is not already on the surface.
        assert!(worst < 0.02, "dipole approximation drifted by {worst}");
    }

    #[test]
    fn a_repeated_first_vertex_does_not_hide_the_rest_of_the_triangle() {
        // Reduced from a fuzz run against a sampled reference. With `a == b`
        // the two edge dot products that decide the a-b region are identically
        // zero, so that region matches every query in space and the answer
        // collapses onto `a` however far away `c` is. Here the truth is eight
        // times nearer than what comes back.
        let a = Vec3::new(9.982, 0.321, 9.66);
        let c = Vec3::new(-8.684, -9.176, -9.678);
        let p = Vec3::new(-11.7285, -10.794, -11.9595);
        let t = Triangle { a, b: a, c };

        let truth = (c - p).length();
        let got = (t.closest_point(p) - p).length();
        assert!(
            (got - truth).abs() < 1e-3,
            "closest point is {got} from the query, but `c` itself is {truth}"
        );
        let bvh = Bvh::build(vec![t]);
        assert!((bvh.distance(p) - truth).abs() < 1e-3);
    }

    #[test]
    fn a_degenerate_triangle_is_answered_as_the_segment_it_is() {
        // Two of the three corners coinciding leaves a segment, whichever pair
        // it is. Exporters emit these, and welding bit-identical corners in
        // `Mesh::from_triangles` produces them from any file that repeats a
        // vertex within one facet.
        let near = Vec3::new(9.0, 0.0, 9.0);
        let far = Vec3::new(-9.0, -9.0, -9.0);
        let p = Vec3::new(-11.0, -11.0, -11.0);
        let truth = (far - p).length();
        for t in [
            Triangle {
                a: near,
                b: near,
                c: far,
            },
            Triangle {
                a: near,
                b: far,
                c: near,
            },
            Triangle {
                a: near,
                b: far,
                c: far,
            },
            Triangle {
                a: far,
                b: near,
                c: near,
            },
        ] {
            let got = (t.closest_point(p) - p).length();
            assert!(
                (got - truth).abs() < 1e-3,
                "{got} rather than {truth} for {t:?}"
            );
        }
        // All three the same point is the one case with nothing to choose.
        let point = Triangle {
            a: far,
            b: far,
            c: far,
        };
        assert_eq!(point.closest_point(p), far);
    }

    #[test]
    fn the_build_terminates_when_the_split_key_cannot_separate_anything() {
        // The median split is on position within the slice, not on the key, so
        // a set of triangles sharing one centroid still halves at every level.
        // Splitting on the key's value instead would recurse forever here.
        let mut tris = Vec::new();
        for i in 0..64 {
            let s = 1.0 + i as f32;
            tris.push(tri([-s, -s, 0.0], [s, 0.0, 0.0], [0.0, s, 0.0]));
        }
        assert!(
            tris.iter().all(|t| t.centroid() == tris[0].centroid()),
            "the fixture must actually share a centroid"
        );
        let bvh = Bvh::build(tris.clone());
        assert_eq!(bvh.triangles().len(), 64);
        for n in 0..30 {
            let a = n as f32 * 0.83;
            let p = Vec3::new(a.sin() * 40.0, a.cos() * 25.0, (a * 1.3).sin() * 30.0);
            let brute = tris
                .iter()
                .map(|t| (t.closest_point(p) - p).length())
                .fold(f32::INFINITY, f32::min);
            assert!((bvh.distance(p) - brute).abs() < 1e-3, "at {p}");
        }
    }

    #[test]
    fn a_tree_of_coincident_triangles_still_answers() {
        let t = tri([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let bvh = Bvh::build(vec![t; 64]);
        assert_eq!(bvh.triangles().len(), 64);
        assert!((bvh.distance(Vec3::new(0.25, 0.25, 3.0)) - 3.0).abs() < 1e-5);
        assert!(bvh.winding_number(Vec3::new(0.25, 0.25, 3.0)).is_finite());
    }

    #[test]
    fn a_tree_of_one_triangle_is_a_single_leaf_that_answers_exactly() {
        let t = tri([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let bvh = Bvh::build(vec![t]);
        assert_eq!(bvh.node_count(), 1);
        let n = bvh.nearest(Vec3::new(0.25, 0.25, 2.0)).unwrap();
        assert_eq!(n.triangle, 0);
        assert!((n.distance - 2.0).abs() < 1e-6);
    }

    #[test]
    fn nearest_is_exact_at_the_corners_and_edges_where_leaves_meet() {
        // Leaf boundaries fall on the triangles themselves, because the split
        // key is the centroid. A pruning test that culled a node at a tie would
        // show up here and nowhere else.
        for mesh in [uv_sphere(5.0, 17, 11), unit_box(Vec3::new(3.0, 1.0, 7.0))] {
            let bvh = Bvh::from_mesh(&mesh);
            let tris = bvh.triangles().to_vec();
            let mut probes = Vec::new();
            for t in &tris {
                probes.push(t.a);
                probes.push((t.a + t.b) * 0.5);
                probes.push(t.centroid());
                probes.push(t.centroid() + Vec3::splat(1.0e-3));
                probes.push(t.centroid() * 1.0001);
            }
            for p in probes {
                let brute = tris
                    .iter()
                    .map(|t| (t.closest_point(p) - p).length())
                    .fold(f32::INFINITY, f32::min);
                let got = bvh.distance(p);
                assert!(
                    (got - brute).abs() <= 1.0e-5,
                    "at {p}: tree says {got}, every triangle says {brute}"
                );
            }
        }
    }

    #[test]
    fn the_dipole_approximation_holds_on_open_surfaces_too() {
        // A closed mesh makes the far field easy: the aggregate dipole of a
        // closed subtree cancels, so the approximated term is near zero anyway.
        // An open one does not cancel, which is the case the ratio has to be
        // chosen for, and it is also what a torn download actually is.
        let sheet = {
            let mut tris = Vec::new();
            for i in 0..16 {
                for j in 0..16 {
                    let (x, y) = (-10.0 + i as f32 * 1.25, -10.0 + j as f32 * 1.25);
                    let at = |dx: f32, dy: f32| [x + dx, y + dy, 0.0];
                    tris.push(tri(at(0.0, 0.0), at(1.25, 0.0), at(1.25, 1.25)));
                    tris.push(tri(at(0.0, 0.0), at(1.25, 1.25), at(0.0, 1.25)));
                }
            }
            Bvh::build(tris)
        };
        let torn = {
            let mut mesh = uv_sphere(10.0, 32, 20);
            mesh.indices.drain(0..6);
            mesh.recompute_normals();
            Bvh::from_mesh(&mesh)
        };

        let mut worst = 0.0f32;
        for bvh in [&sheet, &torn] {
            for n in 0..1500 {
                let t = n as f32;
                let dir = Vec3::new(
                    (t * 0.7351).fract() - 0.5,
                    (t * 0.4327).fract() - 0.5,
                    (t * 0.9137).fract() - 0.5,
                );
                if dir.length() < 1.0e-6 {
                    continue;
                }
                let p = dir.normalize() * (0.5 + (n % 40) as f32 * 1.7);
                let a = bvh.winding_number(p);
                let e = bvh.winding_number_exact(p);
                worst = worst.max((a - e).abs());
                assert_eq!(
                    a > 0.5,
                    e > 0.5,
                    "the approximation flipped the inside test at {p}: {a} against {e}"
                );
            }
        }
        // An order of magnitude below the half the threshold sits at, which is
        // the margin the comment on `FAR_FIELD_RATIO` claims to buy.
        assert!(worst < 0.05, "dipole approximation drifted by {worst}");
    }

    #[test]
    fn an_empty_tree_answers_rather_than_panicking() {
        let bvh = Bvh::build(Vec::new());
        assert!(bvh.nearest(Vec3::ZERO).is_none());
        assert!(bvh.winding_number(Vec3::ZERO).abs() < 1e-9);
        assert_eq!(bvh.node_count(), 0);
    }
}
