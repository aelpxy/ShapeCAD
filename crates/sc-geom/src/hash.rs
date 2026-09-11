//! Deterministic geometry hashing.
//!
//! This is the verification primitive the whole project leans on. It lets a test
//! — or an AI agent that just edited a model — ask "is this still the same
//! shape?" without rendering pixels or diffing meshes.
//!
//! FNV-1a is used rather than [`std::hash::DefaultHasher`] because the standard
//! hasher makes no stability guarantee across Rust releases, and a golden value
//! checked into `tests/corpus` has to mean the same thing in five years.

use crate::arena::Arena;
use crate::bounds::bounds;
use crate::eval::eval;
use crate::node::NodeId;
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

/// Default sampling resolution per axis. 24^3 is 13,824 evaluations — cheap
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
            if let crate::node::Node::Transform { xform, .. } = node {
                for c in xform.rotation.to_array() {
                    h.write_f32(c);
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
