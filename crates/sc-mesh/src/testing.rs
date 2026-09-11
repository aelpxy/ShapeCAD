//! Meshes with a known exact signed distance, and the distance functions to
//! compare against.
//!
//! Voxelization is checked against analytic ground truth rather than against a
//! previous run of itself, so the shapes here are generated with their vertices
//! exactly on the true surface. That makes the tessellation error a quantity
//! the tests can reason about (the sagitta of one facet) instead of an unknown.

use crate::mesh::Mesh;
use sc_geom::glam::Vec3;

/// A latitude/longitude sphere with every vertex exactly on the surface.
///
/// `segments` is the number of divisions around the equator and `rings` the
/// number of bands from pole to pole. The result is closed and wound outward.
pub(crate) fn uv_sphere(radius: f32, segments: u32, rings: u32) -> Mesh {
    let segments = segments.max(3);
    let rings = rings.max(2);

    let mut positions = vec![Vec3::new(0.0, 0.0, radius)];
    for i in 1..rings {
        let theta = std::f32::consts::PI * i as f32 / rings as f32;
        for j in 0..segments {
            let phi = std::f32::consts::TAU * j as f32 / segments as f32;
            positions.push(
                radius
                    * Vec3::new(
                        theta.sin() * phi.cos(),
                        theta.sin() * phi.sin(),
                        theta.cos(),
                    ),
            );
        }
    }
    positions.push(Vec3::new(0.0, 0.0, -radius));

    let north = 0u32;
    let south = u32::try_from(positions.len() - 1).expect("vertex count fits in u32");
    let at = |i: u32, j: u32| 1 + (i - 1) * segments + j % segments;

    let mut indices = Vec::new();
    for j in 0..segments {
        indices.push([north, at(1, j), at(1, j + 1)]);
    }
    for i in 1..rings - 1 {
        for j in 0..segments {
            let (a, b) = (at(i, j), at(i, j + 1));
            let (d, c) = (at(i + 1, j), at(i + 1, j + 1));
            indices.push([a, d, c]);
            indices.push([a, c, b]);
        }
    }
    for j in 0..segments {
        indices.push([south, at(rings - 1, j + 1), at(rings - 1, j)]);
    }

    let mut mesh = Mesh {
        positions,
        normals: Vec::new(),
        indices,
    };
    mesh.recompute_normals();
    mesh
}

/// A box centred on the origin, as twelve triangles wound outward.
pub(crate) fn unit_box(half: Vec3) -> Mesh {
    let (x, y, z) = (half.x, half.y, half.z);
    let positions = vec![
        Vec3::new(-x, -y, -z),
        Vec3::new(x, -y, -z),
        Vec3::new(x, y, -z),
        Vec3::new(-x, y, -z),
        Vec3::new(-x, -y, z),
        Vec3::new(x, -y, z),
        Vec3::new(x, y, z),
        Vec3::new(-x, y, z),
    ];
    let indices = vec![
        [0, 3, 2],
        [0, 2, 1], // -z
        [4, 5, 6],
        [4, 6, 7], // +z
        [0, 1, 5],
        [0, 5, 4], // -y
        [3, 7, 6],
        [3, 6, 2], // +y
        [0, 4, 7],
        [0, 7, 3], // -x
        [1, 2, 6],
        [1, 6, 5], // +x
    ];
    let mut mesh = Mesh {
        positions,
        normals: Vec::new(),
        indices,
    };
    mesh.recompute_normals();
    mesh
}

/// Exact signed distance to a sphere of `radius` centred on the origin.
pub(crate) fn sphere_sdf(radius: f32, p: Vec3) -> f32 {
    p.length() - radius
}

/// Exact signed distance to a box of the given half-extents, centred on the
/// origin.
pub(crate) fn box_sdf(half: Vec3, p: Vec3) -> f32 {
    let q = p.abs() - half;
    q.max(Vec3::ZERO).length() + q.max_element().min(0.0)
}

/// Deletes `count` triangles starting at `from`, leaving a hole.
///
/// Models the downloaded STL that is nearly but not quite watertight.
pub(crate) fn with_triangles_removed(mesh: &Mesh, from: usize, count: usize) -> Mesh {
    let mut out = mesh.clone();
    let end = (from + count).min(out.indices.len());
    out.indices.drain(from..end);
    out.recompute_normals();
    out
}
