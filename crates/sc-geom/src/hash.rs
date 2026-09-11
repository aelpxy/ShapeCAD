//! Deterministic geometry hashing.
//!
//! This is the verification primitive the whole project leans on. It lets a test
//! (or an AI agent that just edited a model) ask "is this still the same
//! shape?" without rendering pixels or diffing meshes.
//!
//! FNV-1a is used rather than [`std::hash::DefaultHasher`] because the standard
//! hasher makes no stability guarantee across Rust releases, and a golden value
//! checked into `tests/corpus` has to mean the same thing in five years.

use crate::arena::Arena;
use crate::bounds::bounds;
use crate::eval::eval;
use crate::node::{Node, NodeId};
use crate::profile::Profile;
use glam::Vec3;
use std::collections::HashMap;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

/// Quantum for parameter and sample quantization: 0.1 micrometres.
/// Far below any printer's resolution, far above f32 evaluation noise.
pub const QUANTUM: f32 = 1e-4;

/// An FNV-1a hasher with a stability guarantee.
///
/// Unlike [`std::hash::DefaultHasher`], the digest produced here is fixed for
/// all time, which is what makes a golden value checked into the repository
/// meaningful across Rust releases.
#[derive(Clone, Copy, Debug)]
pub struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl StableHasher {
    /// A hasher primed with the FNV-1a offset basis.
    #[must_use]
    pub fn new() -> Self {
        Self(FNV_OFFSET)
    }

    /// Folds raw bytes into the digest.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }

    /// Folds an unsigned integer in little-endian order.
    pub fn write_u64(&mut self, v: u64) {
        self.write_bytes(&v.to_le_bytes());
    }

    /// Folds a signed integer in little-endian order.
    pub fn write_i64(&mut self, v: i64) {
        self.write_bytes(&v.to_le_bytes());
    }

    /// Folds a string, terminated so that concatenations cannot collide.
    pub fn write_str(&mut self, s: &str) {
        self.write_bytes(s.as_bytes());
        self.write_bytes(&[0xff]);
    }

    /// Folds a float after quantising it via [`quantize`].
    pub fn write_f32(&mut self, v: f32) {
        self.write_i64(quantize(v));
    }

    /// The digest so far.
    #[must_use]
    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// Snap a float to a lattice so that harmless last-bit differences between two
/// evaluation backends do not change the hash. Non-finite values map to
/// distinct sentinels rather than collapsing together.
#[must_use]
pub fn quantize(v: f32) -> i64 {
    if v.is_nan() {
        return i64::MIN;
    }
    if v == f32::INFINITY {
        return i64::MAX;
    }
    if v == f32::NEG_INFINITY {
        return i64::MIN + 1;
    }
    (v / QUANTUM).round() as i64
}

/// Three independent digests of a model.
///
/// Keeping them separate makes a failing test diagnostic instead of just red:
/// structure alone changing means the tree was rebuilt, field alone changing
/// means a parameter moved, bounds alone changing means it got resized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GeometryHash {
    /// Shape of the DAG plus every parameter. Invariant under id renumbering.
    pub structure: u64,
    /// The field itself, sampled on a grid. Catches cases where a different
    /// tree produces the same solid, and vice versa.
    pub field: u64,
    /// Quantized bounding box.
    pub bounds: u64,
}

impl GeometryHash {
    /// A single digest folding all three components together.
    #[must_use]
    pub fn combined(&self) -> u64 {
        let mut h = StableHasher::new();
        h.write_u64(self.structure);
        h.write_u64(self.field);
        h.write_u64(self.bounds);
        h.finish()
    }

    /// The combined digest as 16 hex characters.
    #[must_use]
    pub fn short(&self) -> String {
        format!("{:016x}", self.combined())
    }
}

impl std::fmt::Display for GeometryHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (structure {:08x} field {:08x} bounds {:08x})",
            self.short(),
            self.structure as u32,
            self.field as u32,
            self.bounds as u32
        )
    }
}

/// Default sampling resolution per axis. 24^3 is 13,824 evaluations, cheap
/// enough to run on every test, dense enough to catch a sub-millimetre edit.
pub const DEFAULT_GRID: u32 = 24;

/// All three digests of the model rooted at `root`, at the default resolution.
#[must_use]
pub fn geometry_hash(arena: &Arena, root: NodeId) -> GeometryHash {
    geometry_hash_with(arena, root, DEFAULT_GRID)
}

/// As [`geometry_hash`], with an explicit sampling resolution.
#[must_use]
pub fn geometry_hash_with(arena: &Arena, root: NodeId, grid: u32) -> GeometryHash {
    GeometryHash {
        structure: structure_hash(arena, root),
        field: field_hash(arena, root, grid),
        bounds: bounds_hash(arena, root),
    }
}

/// Hashes the DAG by structure, not by id, so compaction and renumbering do
/// not perturb it.
#[must_use]
pub fn structure_hash(arena: &Arena, root: NodeId) -> u64 {
    let mut memo = HashMap::new();
    structure_of(arena, root, &mut memo)
}

fn structure_of(arena: &Arena, id: NodeId, memo: &mut HashMap<NodeId, u64>) -> u64 {
    if let Some(&h) = memo.get(&id) {
        return h;
    }
    let mut h = StableHasher::new();
    match arena.get(id) {
        None => h.write_str("<dead>"),
        Some(node) => {
            h.write_str(node.kind());
            for (name, value) in node.params() {
                h.write_str(name);
                h.write_f32(value);
            }
            // Rotation is not exposed through `params`, so fold it in explicitly.
            if let Node::Transform { xform, .. } = node {
                for c in xform.rotation.to_array() {
                    h.write_f32(c);
                }
            }
            // Nor is a voxel grid: a mesh has no parameters at all, so without
            // this every import would hash the same as every other one.
            if let Node::Mesh { asset, grid } = node {
                h.write_u64(u64::from(asset.0));
                h.write_u64(grid.digest());
            }
            // Nor are the vertices of a freehand path, for the same reason:
            // `Profile::params` names the dimensions of a parametric profile,
            // and a drawn one has none, so its shape is entirely in its points.
            if let Some(Profile::Path { points }) = swept_profile(node) {
                h.write_u64(points.len() as u64);
                for p in points {
                    h.write_f32(p.x);
                    h.write_f32(p.y);
                }
            }
            let child_hashes: Vec<u64> = node
                .children()
                .collect::<Vec<_>>()
                .into_iter()
                .map(|c| structure_of(arena, c, memo))
                .collect();
            for ch in child_hashes {
                h.write_u64(ch);
            }
        }
    }
    let out = h.finish();
    memo.insert(id, out);
    out
}

/// The region a node sweeps, for the two node kinds built from one.
fn swept_profile(node: &Node) -> Option<&Profile> {
    match node {
        Node::Extrude { profile, .. } | Node::Prism { profile } => Some(profile),
        _ => None,
    }
}

/// Digest of the model's quantised bounding box.
#[must_use]
pub fn bounds_hash(arena: &Arena, root: NodeId) -> u64 {
    let b = bounds(arena, root);
    let mut h = StableHasher::new();
    for v in [b.min, b.max] {
        h.write_f32(v.x);
        h.write_f32(v.y);
        h.write_f32(v.z);
    }
    h.finish()
}

/// Samples the field on a regular grid spanning the model's bounds, inflated so
/// the grid straddles the surface rather than sitting exactly on it.
#[must_use]
pub fn field_hash(arena: &Arena, root: NodeId, grid: u32) -> u64 {
    let b = bounds(arena, root).finite_or(1000.0);
    let size = b.size();
    let pad = (size.max_element() * 0.1).max(1.0);
    let lo = b.min - Vec3::splat(pad);
    let hi = b.max + Vec3::splat(pad);

    let mut h = StableHasher::new();
    h.write_u64(u64::from(grid));
    let n = grid.max(2);
    let step = (hi - lo) / (n - 1) as f32;
    for iz in 0..n {
        for iy in 0..n {
            for ix in 0..n {
                let p = lo + step * Vec3::new(ix as f32, iy as f32, iz as f32);
                h.write_f32(eval(arena, root, p));
            }
        }
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::{bounds_hash, field_hash, geometry_hash, quantize, structure_hash};
    use crate::node::Node;
    use crate::ops::Builder;
    use crate::profile::Profile;
    use glam::{Quat, Vec2, Vec3};

    /// Two paths with the same number of points and no named dimensions between
    /// them. Nothing but the coordinates tells them apart, so a hash that does
    /// not read the coordinates cannot tell them apart at all.
    #[test]
    fn two_different_drawn_paths_do_not_hash_alike() {
        let triangle = |apex: Vec2| Profile::Path {
            points: vec![Vec2::ZERO, Vec2::new(10.0, 0.0), apex],
        };
        let mut a = Builder::new();
        let one = a.extrude(triangle(Vec2::new(10.0, 6.0)), 4.0).unwrap();
        let mut b = Builder::new();
        let other = b.extrude(triangle(Vec2::new(10.0, 9.0)), 4.0).unwrap();

        assert_ne!(
            structure_hash(&a.arena, one),
            structure_hash(&b.arena, other),
            "a redrawn path hashes as the same structure"
        );
    }

    #[test]
    fn a_through_cut_follows_the_path_it_was_drawn_from() {
        let path = |points: Vec<Vec2>| Node::Prism {
            profile: Profile::Path { points },
        };
        let square = vec![
            Vec2::ZERO,
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(0.0, 4.0),
        ];
        let mut clipped = square.clone();
        clipped.pop();

        let mut a = Builder::new();
        let one = a.arena.insert(path(square)).unwrap();
        let mut b = Builder::new();
        let other = b.arena.insert(path(clipped)).unwrap();
        assert_ne!(
            structure_hash(&a.arena, one),
            structure_hash(&b.arena, other),
            "a prism has no parameters of its own, so its profile is all there is"
        );
    }

    #[test]
    fn the_same_path_drawn_twice_hashes_the_same() {
        let points = vec![Vec2::ZERO, Vec2::new(3.0, 0.0), Vec2::new(0.0, 7.0)];
        let mut a = Builder::new();
        let one = a
            .extrude(
                Profile::Path {
                    points: points.clone(),
                },
                2.0,
            )
            .unwrap();
        let mut b = Builder::new();
        let other = b.extrude(Profile::Path { points }, 2.0).unwrap();
        assert_eq!(geometry_hash(&a.arena, one), geometry_hash(&b.arena, other));
    }

    /// A rotation is not one of a transform's named parameters, so it is folded
    /// in by hand, and this is what says the hand is still there.
    #[test]
    fn turning_a_feature_changes_the_hash() {
        let mut b = Builder::new();
        let bar = b.cuboid(Vec3::new(10.0, 2.0, 2.0)).unwrap();
        let placed = b.rotate(bar, Quat::IDENTITY).unwrap();
        let before = structure_hash(&b.arena, placed);
        b.arena
            .replace(
                placed,
                Node::Transform {
                    child: bar,
                    xform: crate::Transform::from_rotation(Quat::from_rotation_z(0.4)),
                    on: None,
                },
            )
            .unwrap();
        assert_ne!(before, structure_hash(&b.arena, placed));
    }

    /// The field grid spans the model's own bounds, and those used to be
    /// clamped to a thousand millimetres. Anything further out than that was
    /// never sampled, so a part with a feature beyond the clamp hashed exactly
    /// like the same part without it.
    #[test]
    fn a_feature_beyond_the_sampling_fallback_is_still_sampled() {
        let mut b = Builder::new();
        let hub = b.sphere(1200.0).unwrap();
        let outrigger = b.cube(50.0).unwrap();
        let outrigger = b.translate(outrigger, Vec3::new(2500.0, 0.0, 0.0)).unwrap();
        let whole = b.union(hub, outrigger).unwrap();
        assert_ne!(field_hash(&b.arena, hub, 8), field_hash(&b.arena, whole, 8));
    }

    #[test]
    fn the_three_digests_separate_what_moved() {
        let mut b = Builder::new();
        let s = b.sphere(5.0).unwrap();
        let before = geometry_hash(&b.arena, s);
        b.arena.replace(s, Node::Sphere { radius: 6.0 }).unwrap();
        let after = geometry_hash(&b.arena, s);
        assert_ne!(before.structure, after.structure);
        assert_ne!(before.field, after.field);
        assert_ne!(before.bounds, after.bounds);
        assert_ne!(before.combined(), after.combined());
        assert_eq!(after.short().len(), 16);
    }

    #[test]
    fn an_unbounded_model_still_hashes() {
        let mut b = Builder::new();
        let cut = b.plane(Vec3::Z, 0.0).unwrap();
        let h = geometry_hash(&b.arena, cut);
        assert_eq!(h, geometry_hash(&b.arena, cut), "the same model twice");
        assert_ne!(bounds_hash(&b.arena, cut), 0);
    }

    #[test]
    fn quantization_keeps_the_non_finite_values_apart() {
        assert_ne!(quantize(f32::INFINITY), quantize(f32::NEG_INFINITY));
        assert_ne!(quantize(f32::NAN), quantize(f32::INFINITY));
        assert_eq!(quantize(0.0), quantize(-0.0));
        assert_eq!(quantize(1.0), 10_000);
    }
}
