//! CPU evaluation of the implicit field.
//!
//! The reference implementation. The WGSL backend in [`crate::wgsl`] must agree
//! with this to within float tolerance; the corpus tests pin that down.

use crate::arena::Arena;
use crate::node::{Node, NodeId};
use glam::{Vec2, Vec3};

/// Polynomial smooth minimum. `k` is a blend radius in model units.
///
/// This one function is why fillets cannot fail in this kernel: blending is
/// arithmetic on distances, not surgery on boundary topology. A `k` of zero or
/// less degrades to a plain [`f32::min`].
#[inline]
#[must_use]
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    (b * (1.0 - h) + a * h) - k * h * (1.0 - h)
}

/// Polynomial smooth maximum, the dual of [`smin`].
#[inline]
#[must_use]
pub fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

/// Signed distance from `p` to the surface of the solid rooted at `id`.
/// Negative inside, positive outside.
///
/// A missing or deleted node evaluates as empty space rather than panicking, so
/// a partially-edited document still renders.
#[must_use]
pub fn eval(arena: &Arena, id: NodeId, p: Vec3) -> f32 {
    let Some(node) = arena.get(id) else {
        return f32::INFINITY;
    };
    match *node {
        Node::Sphere { radius } => p.length() - radius,

        Node::Box { half, round } => {
            let q = p.abs() - (half - Vec3::splat(round));
            q.max(Vec3::ZERO).length() + q.max_element().min(0.0) - round
        }

        Node::Cylinder {
            radius,
            half_height,
            round,
        } => {
            let d = Vec2::new(
                Vec2::new(p.x, p.y).length() - (radius - round),
                p.z.abs() - (half_height - round),
            );
            d.max(Vec2::ZERO).length() + d.max_element().min(0.0) - round
        }

        Node::Torus { major, minor } => {
            let q = Vec2::new(Vec2::new(p.x, p.y).length() - major, p.z);
            q.length() - minor
        }

        Node::Plane { normal, offset } => p.dot(normal.normalize()) - offset,

        Node::Union { a, b, smooth } => smin(eval(arena, a, p), eval(arena, b, p), smooth),

        Node::Difference { a, b, smooth } => smax(eval(arena, a, p), -eval(arena, b, p), smooth),

        Node::Intersection { a, b, smooth } => smax(eval(arena, a, p), eval(arena, b, p), smooth),

        Node::Transform { child, xform } => {
            xform.apply_distance(eval(arena, child, xform.inverse_point(p)))
        }

        Node::Offset { child, distance } => eval(arena, child, p) - distance,

        // A profile swept along +Z. Combining the in-plane distance with the
        // slab distance this way keeps the result exact rather than merely
        // bounding, which matters for offsets and blends applied on top.
        Node::Extrude {
            ref profile,
            height,
        } => {
            let plane = sd_polygon(Vec2::new(p.x, p.y), profile);
            let slab = (-p.z).max(p.z - height);
            plane.max(slab).min(0.0) + Vec2::new(plane.max(0.0), slab.max(0.0)).length()
        }

        // Inward shell: keep the outer surface, hollow everything deeper than
        // `thickness`. This is the operation 3D printing actually wants.
        Node::Shell { child, thickness } => {
            let d = eval(arena, child, p);
            d.max(-(d + thickness))
        }
    }
}

/// Exact signed distance from a point to a closed polygon, negative inside.
///
/// Distance is the minimum over all edges; the sign comes from a crossing count
/// rather than from winding order, so a profile drawn either way behaves the
/// same.
#[must_use]
pub fn sd_polygon(p: Vec2, verts: &[Vec2]) -> f32 {
    let n = verts.len();
    if n < 3 {
        return f32::INFINITY;
    }
    let mut d = (p - verts[0]).length_squared();
    let mut sign = 1.0f32;

    for i in 0..n {
        let prev = (i + n - 1) % n;
        let edge = verts[prev] - verts[i];
        let offset = p - verts[i];
        let edge_len2 = edge.length_squared();

        // A repeated point gives a zero-length edge; fall back to the vertex.
        let nearest = if edge_len2 > 1.0e-20 {
            offset - edge * (offset.dot(edge) / edge_len2).clamp(0.0, 1.0)
        } else {
            offset
        };
        d = d.min(nearest.length_squared());

        // Crossing count: flip the sign each time the ray from `p` passes an
        // edge. Independent of winding order, so a profile drawn either way
        // gives the same solid.
        let crossings = [
            p.y >= verts[i].y,
            p.y < verts[prev].y,
            edge.x * offset.y > edge.y * offset.x,
        ];
        if crossings.iter().all(|&c| c) || crossings.iter().all(|&c| !c) {
            sign = -sign;
        }
    }
    sign * d.sqrt()
}

/// Surface normal by the tetrahedron finite-difference trick: four evaluations
/// instead of six, and no axis bias.
///
/// `eps` should be comfortably above float noise but well below the smallest
/// feature of interest. Returns a zero vector where the field has no gradient.
#[must_use]
pub fn normal(arena: &Arena, id: NodeId, p: Vec3, eps: f32) -> Vec3 {
    const K: [Vec3; 4] = [
        Vec3::new(1.0, -1.0, -1.0),
        Vec3::new(-1.0, -1.0, 1.0),
        Vec3::new(-1.0, 1.0, -1.0),
        Vec3::new(1.0, 1.0, 1.0),
    ];
    let n: Vec3 = K.iter().map(|&k| k * eval(arena, id, p + k * eps)).sum();
    n.normalize_or_zero()
}
