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
/// less degrades to a plain [`f32::min`], and so does an operand that is not
/// finite.
#[inline]
#[must_use]
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    // The interpolation below multiplies an operand by a weight that saturates
    // at zero, so an infinite operand yields `inf * 0`, which is NaN rather
    // than the other operand. [`eval`] returns `+inf` for a node it cannot
    // find, and a transform small enough to overflow its own division returns
    // it too, so this is not a theoretical input: without the guard one absent
    // operand blanks the whole model instead of just itself.
    if k <= 0.0 || !a.is_finite() || !b.is_finite() {
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

        // Trilinear inside the grid, and a lower bound on the true distance
        // outside it. See [`Grid::sample`](crate::sdf::Grid::sample): this is
        // the one node whose field is an approximation rather than an exact
        // distance, bounded by the voxel spacing chosen at import.
        Node::Mesh { ref grid, .. } => grid.sample(p),

        Node::Union { a, b, smooth } => smin(eval(arena, a, p), eval(arena, b, p), smooth),

        Node::Difference { a, b, smooth } => smax(eval(arena, a, p), -eval(arena, b, p), smooth),

        Node::Intersection { a, b, smooth } => smax(eval(arena, a, p), eval(arena, b, p), smooth),

        Node::Transform { child, xform, .. } => {
            xform.apply_distance(eval(arena, child, xform.inverse_point(p)))
        }

        Node::Offset { child, distance } => eval(arena, child, p) - distance,

        // A profile swept along +Z. Combining the in-plane distance with the
        // slab distance this way keeps the result exact rather than merely
        // bounding, which matters for offsets and blends applied on top.
        // The nearest instance wins, which is a union over the whole set. Each
        // instance is the child sampled at a point moved into that instance's
        // frame, so the subtree is evaluated `count` times and stored once.
        Node::Pattern { child, kind, count } => {
            let mut best = f32::INFINITY;
            for i in 0..count {
                let at = kind.placement(i, count).inverse_point(p);
                best = best.min(eval(arena, child, at));
            }
            best
        }

        // No z term at all: that is what "without end" means, and it makes the
        // prism the cheapest node in the kernel rather than the dearest.
        Node::Prism { ref profile } => profile.distance(Vec2::new(p.x, p.y)),

        Node::Extrude { ref profile, depth } => {
            let plane = profile.distance(Vec2::new(p.x, p.y));
            let slab = (-p.z).max(p.z - depth);
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

#[cfg(test)]
mod tests {
    use super::{smax, smin};

    fn assert_near(got: f32, want: f32, ctx: &str) {
        assert!(
            (got - want).abs() < 1.0e-6,
            "{ctx}: expected {want}, got {got}"
        );
    }

    /// [`eval`] hands back `+inf` for a node it cannot find, so that a document
    /// in the middle of an edit still renders. A positive blend radius used to
    /// turn that sentinel into NaN, and NaN is not "empty space": it is a model
    /// that disappears everywhere at once, with no error to say why.
    #[test]
    fn a_blend_against_an_absent_operand_is_the_operand_that_is_there() {
        for k in [0.0, 0.5, 4.0] {
            assert_near(smin(2.0, f32::INFINITY, k), 2.0, &format!("smin, k = {k}"));
            assert_near(
                smin(f32::INFINITY, 2.0, k),
                2.0,
                &format!("smin reversed, k = {k}"),
            );
            assert_near(
                smax(2.0, f32::NEG_INFINITY, k),
                2.0,
                &format!("smax, k = {k}"),
            );
            assert_near(
                smax(f32::NEG_INFINITY, 2.0, k),
                2.0,
                &format!("smax reversed, k = {k}"),
            );
        }
    }

    /// The other side of the same guard. An operand that is infinitely deep
    /// wins the blend outright rather than poisoning it.
    #[test]
    fn a_blend_against_an_unbounded_operand_keeps_its_sign() {
        let inside = smin(2.0, f32::NEG_INFINITY, 1.0);
        assert!(
            inside.is_infinite() && inside.is_sign_negative(),
            "smin gave {inside}"
        );
        let outside = smax(2.0, f32::INFINITY, 1.0);
        assert!(
            outside.is_infinite() && outside.is_sign_positive(),
            "smax gave {outside}"
        );
    }

    /// Blending two ordinary distances is untouched by the guard above.
    #[test]
    fn a_blend_of_finite_distances_is_unchanged() {
        assert!((smin(1.0, 3.0, 0.0) - 1.0).abs() < 1e-6);
        assert!(smin(1.0, 1.0, 2.0) < 1.0, "a blend should round the corner");
        assert!((smax(1.0, 3.0, 0.0) - 3.0).abs() < 1e-6);
    }
}
