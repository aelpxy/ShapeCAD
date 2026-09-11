//! The implicit node tree: `ShapeCAD`'s document representation of geometry.
//!
//! This is a DAG, not a tree: subtrees may be shared. It is the authoritative
//! description of a model; evaluation backends (CPU, WGSL, `fidget`) all consume
//! it and none of them own it.

use crate::math::Transform;
use crate::profile::Profile;
use crate::sdf::{AssetId, Grid};
use glam::{Vec2, Vec3};
use std::sync::Arc;

/// Signed area of a polygon, by the shoelace formula. Negative for a clockwise
/// winding, and the profile's own distance function does not care which.
///
/// Used only to reject degenerate profiles: three collinear points enclose
/// nothing and would give the field no inside.
#[must_use]
pub fn polygon_area(points: &[Vec2]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f32>()
        * 0.5
}

/// A stable handle to a node.
///
/// Ids are never reused, even after deletion. That stability is what lets the UI
/// hold a selection, and an AI agent hold a reference, across arbitrary edits,
/// and it is why the topological naming problem that plagues B-rep kernels does
/// not arise here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct NodeId(pub u32);

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// One operation in the implicit DAG.
///
/// Primitives are defined at the origin in a canonical orientation; placement is
/// always expressed with an enclosing [`Node::Transform`]. Keeping primitives
/// canonical means there is exactly one way to express a placement, which keeps
/// both the undo log and the agent-facing representation unambiguous.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Node {
    /// Sphere centred on the origin.
    Sphere {
        /// Radius. Must be positive.
        radius: f32,
    },
    /// Axis-aligned box with corner rounding. `half` is the half-extent of the
    /// *outer* surface, so increasing `round` does not change overall size.
    Box {
        /// Half-extent along each axis.
        half: Vec3,
        /// Corner radius, from zero up to the smallest half-extent.
        round: f32,
    },
    /// Capped cylinder along +Z.
    Cylinder {
        /// Radius of the outer surface.
        radius: f32,
        /// Half the overall height.
        half_height: f32,
        /// Edge rounding where the cap meets the wall.
        round: f32,
    },
    /// Torus in the XY plane, axis along Z.
    Torus {
        /// Distance from the origin to the centre of the tube.
        major: f32,
        /// Tube radius. Must be smaller than `major`.
        minor: f32,
    },
    /// Half-space. Unbounded; useful for cutting.
    Plane {
        /// Outward normal. Normalised on evaluation, so it need not be a unit
        /// vector here.
        normal: Vec3,
        /// Signed distance from the origin to the plane along `normal`.
        offset: f32,
    },
    /// An imported triangle mesh, voxelized to a signed distance grid at import.
    ///
    /// The grid is carried in the node rather than looked up from an asset table
    /// during evaluation. [`eval`](crate::eval::eval) takes a point and an arena and
    /// nothing else, and threading an asset context through evaluation, bounds,
    /// hashing, picking and codegen would change every caller in the workspace
    /// for the sake of one node kind. The document resolves [`AssetId`] into a
    /// grid once, at load, and the kernel stays context-free.
    Mesh {
        /// Which grid, as stored beside the document.
        asset: AssetId,
        /// The grid itself. Shared, because a grid is megabytes and a node is
        /// cloned freely: by the tree view, by every undo entry, by codegen.
        ///
        /// Deserializes to [`Grid::default`], the placeholder, since the samples
        /// are not in the document; the loader fills it in. A placeholder fails
        /// [`Node::is_valid`], so an unresolved asset is refused by the arena
        /// rather than rendering as empty space.
        #[cfg_attr(feature = "serde", serde(skip))]
        grid: Arc<Grid>,
    },

    /// Union of two solids.
    ///
    /// `smooth` is a blend radius in model units. Zero gives a hard union; any
    /// positive value gives a fillet. In an implicit kernel a fillet is a
    /// parameter, not a fallible operation on boundary topology.
    Union {
        /// First operand.
        a: NodeId,
        /// Second operand.
        b: NodeId,
        /// Blend radius. Zero for a hard edge.
        smooth: f32,
    },
    /// `a` minus `b`.
    Difference {
        /// The solid being cut.
        a: NodeId,
        /// The tool removed from `a`.
        b: NodeId,
        /// Blend radius applied to the resulting interior edge.
        smooth: f32,
    },
    /// The volume common to both operands.
    Intersection {
        /// First operand.
        a: NodeId,
        /// Second operand.
        b: NodeId,
        /// Blend radius applied to the resulting edge.
        smooth: f32,
    },

    /// Places a subtree in space.
    Transform {
        /// The subtree being placed.
        child: NodeId,
        /// Rigid transform plus uniform scale.
        xform: Transform,
        /// The feature this placement was derived from, if any.
        ///
        /// Provenance, not geometry. It records that `xform` was computed from
        /// another node's face, so that moving that face can move this
        /// placement with it. Evaluation, bounds, code generation, picking and
        /// hashing all ignore it, which is why a derivation never changes the
        /// shape a document hashes to.
        ///
        /// The placement is stored resolved rather than recomputed while
        /// evaluating: evaluation runs millions of times per mesh and cannot
        /// afford a walk up the tree per sample.
        ///
        /// Defaulted on load so version 2 files, written before this field
        /// existed, still open.
        #[cfg_attr(feature = "serde", serde(default))]
        on: Option<NodeId>,
    },
    /// Grows (positive) or shrinks (negative) the solid by `distance`.
    /// This is how clearance fits for printing are expressed.
    Offset {
        /// The subtree being offset.
        child: NodeId,
        /// Distance to grow by; negative shrinks.
        distance: f32,
    },
    /// A closed profile swept along +Z: the implicit equivalent of a pad.
    ///
    /// The profile is parametric, so a rectangle stays a width and a height and
    /// can be re-dimensioned long after it was drawn. The solid runs from z = 0
    /// to z = `depth`, so a sketch sits on its own plane rather than straddling
    /// it; place it with an enclosing [`Node::Transform`].
    Extrude {
        /// The region being swept.
        profile: Profile,
        /// Distance swept along +Z. Must be positive.
        depth: f32,
    },
    /// A closed profile swept without end along Z: a cut that goes all the way
    /// through, however the thing it cuts changes afterwards.
    ///
    /// "Through all" is a standard end condition in parametric CAD, and this is
    /// the honest way to express it in an implicit kernel. A pocket sized from
    /// the model at the moment it was cut is frozen: grow the base and the
    /// through hole quietly becomes a blind recess. A prism has no extent to go
    /// stale.
    ///
    /// Infinite along Z, so it is only meaningful as the tool of a
    /// [`Node::Difference`] or [`Node::Intersection`], whose bounds come from
    /// the other operand. [`Node::Plane`] is unbounded for the same reason and
    /// on the same terms.
    Prism {
        /// The region swept.
        profile: Profile,
    },
    /// Hollows the solid inward, preserving the outer surface.
    Shell {
        /// The subtree being hollowed.
        child: NodeId,
        /// Wall thickness. Must be positive.
        thickness: f32,
    },
}

/// Written out rather than derived so that a mesh compares by identity.
///
/// A derived implementation would compare two grids sample by sample. Node
/// equality is not a rare operation: `sc-doc` compares effects when it decides
/// whether a drag coalesces into one undo entry, and doing that at tens of
/// megabytes per keystroke would be felt. Identity is also the right answer and
/// not merely the cheap one. A grid is immutable once imported and resolved
/// exactly once per asset, so two nodes sharing an [`Arc`] are the same import,
/// while two separate allocations holding identical samples are two imports the
/// user made and can edit apart. [`Grid`] itself compares by value for anyone
/// who does want that.
///
/// One arm per variant, matched on `self` alone, so the compiler names a new
/// node kind here instead of letting it fall through a catch-all as unequal to
/// itself. A pair-wise match cannot be exhaustive without writing out every
/// mismatched combination, and that is exactly how [`Node::Prism`] once ended
/// up comparing false against a copy of itself.
impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        match self {
            Node::Sphere { radius } => {
                matches!(other, Node::Sphere { radius: r } if radius == r)
            }
            Node::Box { half, round } => {
                matches!(other, Node::Box { half: h, round: r } if half == h && round == r)
            }
            Node::Cylinder {
                radius,
                half_height,
                round,
            } => matches!(
                other,
                Node::Cylinder { radius: r, half_height: hh, round: rd }
                    if radius == r && half_height == hh && round == rd
            ),
            Node::Torus { major, minor } => {
                matches!(other, Node::Torus { major: j, minor: n } if major == j && minor == n)
            }
            Node::Plane { normal, offset } => {
                matches!(other, Node::Plane { normal: n, offset: o } if normal == n && offset == o)
            }
            Node::Mesh { asset, grid } => matches!(
                other,
                Node::Mesh { asset: a, grid: g } if asset == a && Arc::ptr_eq(grid, g)
            ),
            Node::Union { a, b, smooth } => matches!(
                other,
                Node::Union { a: a2, b: b2, smooth: s } if a == a2 && b == b2 && smooth == s
            ),
            Node::Difference { a, b, smooth } => matches!(
                other,
                Node::Difference { a: a2, b: b2, smooth: s } if a == a2 && b == b2 && smooth == s
            ),
            Node::Intersection { a, b, smooth } => matches!(
                other,
                Node::Intersection { a: a2, b: b2, smooth: s } if a == a2 && b == b2 && smooth == s
            ),
            // Provenance counts. Two placements that sit in the same spot but
            // are derived from different faces will part company the next time
            // either face moves, so they are not the same node.
            Node::Transform { child, xform, on } => matches!(
                other,
                Node::Transform { child: c, xform: x, on: o }
                    if child == c && xform == x && on == o
            ),
            Node::Offset { child, distance } => matches!(
                other,
                Node::Offset { child: c, distance: d } if child == c && distance == d
            ),
            Node::Extrude { profile, depth } => matches!(
                other,
                Node::Extrude { profile: p, depth: d } if profile == p && depth == d
            ),
            Node::Prism { profile } => {
                matches!(other, Node::Prism { profile: p } if profile == p)
            }
            Node::Shell { child, thickness } => matches!(
                other,
                Node::Shell { child: c, thickness: t } if child == c && thickness == t
            ),
        }
    }
}

impl Node {
    /// An imported mesh node wrapping an already-resolved grid.
    #[must_use]
    pub fn mesh(asset: AssetId, grid: Arc<Grid>) -> Self {
        Node::Mesh { asset, grid }
    }

    /// The grid behind a [`Node::Mesh`], or `None` for any other kind.
    #[must_use]
    pub fn mesh_grid(&self) -> Option<&Arc<Grid>> {
        match self {
            Node::Mesh { grid, .. } => Some(grid),
            Node::Sphere { .. }
            | Node::Box { .. }
            | Node::Cylinder { .. }
            | Node::Torus { .. }
            | Node::Plane { .. }
            | Node::Union { .. }
            | Node::Difference { .. }
            | Node::Intersection { .. }
            | Node::Transform { .. }
            | Node::Offset { .. }
            | Node::Extrude { .. }
            | Node::Prism { .. }
            | Node::Shell { .. } => None,
        }
    }

    /// Voxel resolution of a [`Node::Mesh`], or `None` for any other kind.
    ///
    /// Read-only on purpose. Resolution is fixed when the mesh is voxelized, so
    /// exposing it through [`Node::params`] would put a slider in the property
    /// panel that changes nothing: re-voxelizing needs the original triangles,
    /// which the kernel does not keep.
    #[must_use]
    pub fn mesh_resolution(&self) -> Option<[u32; 3]> {
        self.mesh_grid().map(|g| g.dims)
    }

    /// Stable machine-readable tag. Part of the agent-facing vocabulary, so
    /// treat these strings as API.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Node::Sphere { .. } => "sphere",
            Node::Box { .. } => "box",
            Node::Cylinder { .. } => "cylinder",
            Node::Torus { .. } => "torus",
            Node::Plane { .. } => "plane",
            Node::Mesh { .. } => "mesh",
            Node::Union { .. } => "union",
            Node::Difference { .. } => "difference",
            Node::Intersection { .. } => "intersection",
            Node::Transform { .. } => "transform",
            Node::Offset { .. } => "offset",
            Node::Extrude { .. } => "extrude",
            Node::Prism { .. } => "prism",
            Node::Shell { .. } => "shell",
        }
    }

    /// The node this one's placement was derived from, if any.
    ///
    /// Deliberately separate from [`Node::children`]: the arena treats it as a
    /// reference to protect, not as an operand to evaluate.
    #[must_use]
    pub fn derived_from(&self) -> Option<NodeId> {
        match *self {
            Node::Transform { on, .. } => on,
            Node::Sphere { .. }
            | Node::Box { .. }
            | Node::Cylinder { .. }
            | Node::Torus { .. }
            | Node::Plane { .. }
            | Node::Mesh { .. }
            | Node::Union { .. }
            | Node::Difference { .. }
            | Node::Intersection { .. }
            | Node::Offset { .. }
            | Node::Extrude { .. }
            | Node::Prism { .. }
            | Node::Shell { .. } => None,
        }
    }

    /// The node's direct child references, in declaration order.
    ///
    /// A [`Node::Transform`]'s `on` is not among them, and must not be: it is a
    /// reference rather than a geometric child, and including it would make
    /// bounds and the generated shader treat the base as a second operand,
    /// evaluating and bounding it twice over.
    pub fn children(&self) -> impl Iterator<Item = NodeId> + '_ {
        // Written out rather than closed with a wildcard: a new kind that holds
        // a subtree has to be named here, or its child is invisible to bounds,
        // to the generated shader, to the cycle check and to the reference
        // count that stops a live node being deleted.
        let (a, b) = match *self {
            Node::Union { a, b, .. }
            | Node::Difference { a, b, .. }
            | Node::Intersection { a, b, .. } => (Some(a), Some(b)),
            Node::Transform { child, .. }
            | Node::Offset { child, .. }
            | Node::Shell { child, .. } => (Some(child), None),
            Node::Sphere { .. }
            | Node::Box { .. }
            | Node::Cylinder { .. }
            | Node::Torus { .. }
            | Node::Plane { .. }
            | Node::Mesh { .. }
            | Node::Extrude { .. }
            | Node::Prism { .. } => (None, None),
        };
        a.into_iter().chain(b)
    }

    /// Rewrite child references in place. Used by cycle-safe edits and by
    /// compaction, which renumbers ids.
    ///
    /// Leaves a derivation alone, for the reason given on [`Node::children`]:
    /// rerouting a feature's operands must not silently re-point the face it
    /// was built on.
    pub fn map_children(&mut self, mut f: impl FnMut(NodeId) -> NodeId) {
        match self {
            Node::Union { a, b, .. }
            | Node::Difference { a, b, .. }
            | Node::Intersection { a, b, .. } => {
                *a = f(*a);
                *b = f(*b);
            }
            Node::Transform { child, .. }
            | Node::Offset { child, .. }
            | Node::Shell { child, .. } => *child = f(*child),
            // Exhaustive for the same reason as [`Node::children`]: a kind that
            // is silently unrewritable keeps pointing at the node an edit just
            // replaced.
            Node::Sphere { .. }
            | Node::Box { .. }
            | Node::Cylinder { .. }
            | Node::Torus { .. }
            | Node::Plane { .. }
            | Node::Mesh { .. }
            | Node::Extrude { .. }
            | Node::Prism { .. } => {}
        }
    }

    /// Named scalar parameters, in a stable order.
    ///
    /// This exists so the agent layer and the property panel can both drive
    /// edits generically instead of matching on every variant.
    #[must_use]
    pub fn params(&self) -> Vec<(&'static str, f32)> {
        // Matched by reference first: a profile is not `Copy`, and its own
        // dimensions are part of the feature's.
        if let Node::Extrude { profile, depth } = self {
            let mut out = profile.params();
            out.push(("depth", *depth));
            return out;
        }
        // A prism has no depth to report: that is the whole point of it.
        if let Node::Prism { profile } = self {
            return profile.params();
        }
        match *self {
            // Handled above; it cannot be bound here because a profile is not
            // `Copy`.
            Node::Extrude { .. } | Node::Prism { .. } => {
                unreachable!("swept profiles are handled before the copy match")
            }
            Node::Sphere { radius } => vec![("radius", radius)],
            Node::Box { half, round } => vec![
                ("half_x", half.x),
                ("half_y", half.y),
                ("half_z", half.z),
                ("round", round),
            ],
            Node::Cylinder {
                radius,
                half_height,
                round,
            } => vec![
                ("radius", radius),
                ("half_height", half_height),
                ("round", round),
            ],
            Node::Torus { major, minor } => vec![("major", major), ("minor", minor)],
            // Empty on purpose. The only number a mesh has is its voxel
            // resolution, which is fixed when the triangles are voxelized and
            // cannot be changed without them. A slider that silently does
            // nothing is worse than no slider, so it is read-only through
            // [`Node::mesh_resolution`] instead.
            Node::Mesh { .. } => Vec::new(),
            Node::Plane { normal, offset } => vec![
                ("normal_x", normal.x),
                ("normal_y", normal.y),
                ("normal_z", normal.z),
                ("offset", offset),
            ],
            Node::Union { smooth, .. }
            | Node::Difference { smooth, .. }
            | Node::Intersection { smooth, .. } => vec![("smooth", smooth)],
            Node::Transform { xform, .. } => vec![
                ("x", xform.translation.x),
                ("y", xform.translation.y),
                ("z", xform.translation.z),
                ("scale", xform.scale),
            ],
            Node::Offset { distance, .. } => vec![("distance", distance)],

            Node::Shell { thickness, .. } => vec![("thickness", thickness)],
        }
    }

    /// Set a named parameter. Returns false if the name is not valid for this
    /// node kind, leaving the node untouched.
    pub fn set_param(&mut self, name: &str, v: f32) -> bool {
        if let Node::Prism { profile } = self {
            return profile.set_param(name, v);
        }
        if let Node::Extrude { profile, depth } = self {
            if name == "depth" {
                *depth = v;
                return true;
            }
            return profile.set_param(name, v);
        }
        match self {
            Node::Sphere { radius } if name == "radius" => *radius = v,
            Node::Box { half, round } => match name {
                "half_x" => half.x = v,
                "half_y" => half.y = v,
                "half_z" => half.z = v,
                "round" => *round = v,
                _ => return false,
            },
            Node::Cylinder {
                radius,
                half_height,
                round,
            } => match name {
                "radius" => *radius = v,
                "half_height" => *half_height = v,
                "round" => *round = v,
                _ => return false,
            },
            Node::Torus { major, minor } => match name {
                "major" => *major = v,
                "minor" => *minor = v,
                _ => return false,
            },
            Node::Plane { normal, offset } => match name {
                "normal_x" => normal.x = v,
                "normal_y" => normal.y = v,
                "normal_z" => normal.z = v,
                "offset" => *offset = v,
                _ => return false,
            },
            Node::Union { smooth, .. }
            | Node::Difference { smooth, .. }
            | Node::Intersection { smooth, .. }
                if name == "smooth" =>
            {
                *smooth = v;
            }
            // A derived placement belongs to whatever regenerates it, so an edit
            // here would be rewritten the next time its face moved. Refused
            // rather than silently reverted. `params` still reports the values,
            // because they are the node's data and the geometry hash is built
            // from them: hiding them would make two placements on different
            // parts of the same face hash alike.
            Node::Transform { on: Some(_), .. } => return false,
            Node::Transform { xform, .. } => match name {
                "x" => xform.translation.x = v,
                "y" => xform.translation.y = v,
                "z" => xform.translation.z = v,
                "scale" => xform.scale = v,
                _ => return false,
            },
            Node::Offset { distance, .. } if name == "distance" => *distance = v,

            Node::Shell { thickness, .. } if name == "thickness" => *thickness = v,
            _ => return false,
        }
        true
    }

    /// Structural and numeric sanity. Does not check child ids; the arena does
    /// that, because only the arena knows which ids are live.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let finite = self.params().iter().all(|(_, v)| v.is_finite());
        if !finite {
            return false;
        }
        if let Node::Extrude { profile, depth } = self {
            return *depth > 0.0 && depth.is_finite() && profile.is_valid();
        }
        if let Node::Prism { profile } = self {
            return profile.is_valid();
        }
        match *self {
            Node::Sphere { radius } => radius > 0.0,
            Node::Box { half, round } => {
                half.cmpgt(Vec3::ZERO).all() && round >= 0.0 && round <= half.min_element()
            }
            Node::Cylinder {
                radius,
                half_height,
                round,
            } => {
                radius > 0.0
                    && half_height > 0.0
                    && round >= 0.0
                    && round <= radius.min(half_height)
            }
            Node::Torus { major, minor } => major > 0.0 && minor > 0.0 && minor < major,
            // Both bounds matter. Too short and there is no direction to
            // normalise; long enough to overflow the square and `normalize`
            // hands back the zero vector, which turns the half-space into the
            // constant field `-offset`: everywhere solid or everywhere empty,
            // and nothing on screen to say which.
            Node::Plane { normal, .. } => {
                let l2 = normal.length_squared();
                l2 > 1e-12 && l2.is_finite()
            }
            // Rejects the placeholder an unresolved asset deserializes to, so
            // a document referring to a missing sidecar fails at load instead
            // of rendering as a part with a hole where the import should be.
            Node::Mesh { ref grid, .. } => grid.is_valid(),
            Node::Union { smooth, .. }
            | Node::Difference { smooth, .. }
            | Node::Intersection { smooth, .. } => smooth >= 0.0,
            Node::Transform { xform, .. } => xform.is_valid(),
            Node::Offset { .. } => true,
            Node::Shell { thickness, .. } => thickness > 0.0,
            // Matched by reference: the profile is not `Copy`.
            Node::Extrude { .. } | Node::Prism { .. } => {
                unreachable!("swept profiles are handled before the copy match")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Node, NodeId};
    use glam::Vec3;

    /// A normal big enough that squaring it overflows `f32` normalises to the
    /// zero vector, and the plane's field collapses to the constant `-offset`:
    /// a half-space that is either everywhere solid or everywhere empty, with
    /// nothing on screen to say so. Testing `length_squared() > 1e-12` alone
    /// waves it through, because infinity is greater than 1e-12.
    #[test]
    fn a_plane_normal_too_large_to_normalise_is_rejected() {
        let normal = Vec3::new(1.0e30, 0.0, 0.0);
        assert!(
            normal.is_finite() && !normal.length_squared().is_finite(),
            "the premise of this test is that 1e30 is finite but its square is not"
        );
        assert_eq!(
            normal.normalize_or_zero(),
            Vec3::ZERO,
            "such a normal has no direction left to give"
        );
        assert!(
            !Node::Plane {
                normal,
                offset: 0.0
            }
            .is_valid(),
            "a plane with no usable normal was accepted"
        );
    }

    /// The bound above must not catch an ordinary small normal: a sketch drawn
    /// in metres and a normal left unnormalised are both perfectly legal.
    #[test]
    fn an_ordinary_normal_is_still_accepted() {
        for n in [Vec3::Z, Vec3::splat(1.0e-3), Vec3::new(0.0, 1.0e6, 0.0)] {
            assert!(
                Node::Plane {
                    normal: n,
                    offset: 2.0,
                }
                .is_valid(),
                "{n:?} was refused"
            );
        }
    }

    /// Every kind that names a subtree has to report it. A child nothing can
    /// see is not bounded, not emitted into the shader, not protected from
    /// deletion and not checked for cycles.
    #[test]
    fn every_kind_that_holds_a_subtree_reports_it_as_a_child() {
        let one = NodeId(7);
        let two = NodeId(9);
        let cases: Vec<(Node, Vec<NodeId>)> = vec![
            (
                Node::Union {
                    a: one,
                    b: two,
                    smooth: 0.0,
                },
                vec![one, two],
            ),
            (
                Node::Difference {
                    a: one,
                    b: two,
                    smooth: 0.0,
                },
                vec![one, two],
            ),
            (
                Node::Intersection {
                    a: one,
                    b: two,
                    smooth: 0.0,
                },
                vec![one, two],
            ),
            (
                Node::Transform {
                    child: one,
                    xform: crate::Transform::IDENTITY,
                    on: Some(two),
                },
                vec![one],
            ),
            (
                Node::Offset {
                    child: one,
                    distance: 1.0,
                },
                vec![one],
            ),
            (
                Node::Shell {
                    child: one,
                    thickness: 1.0,
                },
                vec![one],
            ),
        ];

        for (node, want) in cases {
            let got: Vec<NodeId> = node.children().collect();
            assert_eq!(got, want, "a {} lost a child", node.kind());

            let mut rewired = node.clone();
            rewired.map_children(|_| NodeId(0));
            let after: Vec<NodeId> = rewired.children().collect();
            assert_eq!(
                after,
                vec![NodeId(0); want.len()],
                "a {} ignored map_children",
                node.kind()
            );
            assert_eq!(
                rewired.derived_from(),
                node.derived_from(),
                "map_children moved a {}'s derivation",
                node.kind()
            );
        }
    }
}
