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
    /// The same subtree repeated, in a line or about an axis.
    ///
    /// A part with four bolt holes is four holes, and until this existed the
    /// only way to say so was to make four and then keep four sets of numbers in
    /// step by hand. Here the count is one parameter: change it and the part
    /// changes.
    ///
    /// The child is evaluated once per instance rather than copied, which is
    /// what makes it cheap. A hundred instances is one subtree and a hundred
    /// point transforms, not a hundred subtrees.
    Pattern {
        /// The subtree being repeated. Instance zero is it, where it already is.
        child: NodeId,
        /// How the instances are laid out.
        kind: Repeat,
        /// How many there are in total, counting the original.
        count: u32,
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
            Node::Pattern { child, kind, count } => matches!(
                other,
                Node::Pattern { child: c, kind: k, count: n }
                    if child == c && kind == k && count == n
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

/// The most instances a pattern will hold.
///
/// The shader unrolls nothing: it loops, and the loop bound is baked into the
/// source, so a count is structural and changing it rebuilds the pipeline. That
/// is the right trade for a number somebody types occasionally, and the wrong
/// one for a number somebody drags, which is why the cap exists at all. Two
/// hundred is far past any bolt circle and still a loop a tracer can afford.
pub const MAX_INSTANCES: u32 = 200;

/// How a [`Node::Pattern`] lays its instances out.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Repeat {
    /// Evenly spaced along a direction. The length of `step` is the spacing and
    /// its direction is the line.
    Linear {
        /// Offset from one instance to the next.
        step: Vec3,
    },
    /// Evenly spaced about the Z axis of the pattern's own frame, spanning
    /// `sweep` radians in total.
    ///
    /// About Z rather than an arbitrary axis, for the same reason an extrusion
    /// sweeps along Z: the frame is what gets rotated, so a pattern about any
    /// other axis is this one with a placement above it. One axis here means one
    /// loop in the shader rather than a general rotation per instance.
    Circular {
        /// Total angle covered, in radians.
        sweep: f32,
    },
}

impl Repeat {
    /// How many gaps the sweep is divided into.
    ///
    /// A full turn lands the last instance back on the first, so there the
    /// spacing divides by the count. A partial sweep puts the last one at the
    /// far end, which is what an arc of holes wants and what anybody typing
    /// ninety degrees expects.
    #[must_use]
    pub fn spans(self, count: u32) -> u32 {
        match self {
            Self::Linear { .. } => count.max(1),
            Self::Circular { sweep } => {
                if (sweep.abs() - std::f32::consts::TAU).abs() < 1.0e-4 {
                    count.max(1)
                } else {
                    count.max(2) - 1
                }
            }
        }
    }

    /// Where instance `i` of `count` sits, relative to instance zero.
    ///
    /// Evaluation needs the inverse of this, since a field is sampled by moving
    /// the point rather than the shape, but forwards is how anyone reading it
    /// will think about it. Instance zero is always the identity, which is what
    /// lets the shader seed its loop from the child itself.
    #[must_use]
    pub fn placement(self, i: u32, count: u32) -> crate::Transform {
        match self {
            Self::Linear { step } => crate::Transform::from_translation(step * i as f32),
            Self::Circular { sweep } => {
                let spans = Self::Circular { sweep }.spans(count);
                crate::Transform {
                    translation: Vec3::ZERO,
                    rotation: glam::Quat::from_rotation_z(sweep * i as f32 / spans as f32),
                    scale: 1.0,
                }
            }
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
            Node::Pattern { .. }
            | Node::Sphere { .. }
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
            Node::Pattern { .. } => "pattern",
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
            Node::Pattern { .. }
            | Node::Sphere { .. }
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
            Node::Pattern { child, .. }
            | Node::Transform { child, .. }
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
            Node::Pattern { child, .. }
            | Node::Transform { child, .. }
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
            // The count is the number worth editing, and the one that makes a
            // pattern worth having: four holes become six by changing it.
            Node::Pattern { kind, count, .. } => {
                let mut out = vec![("count", count as f32)];
                match kind {
                    Repeat::Linear { step } => {
                        out.push(("step_x", step.x));
                        out.push(("step_y", step.y));
                        out.push(("step_z", step.z));
                    }
                    Repeat::Circular { sweep } => out.push(("sweep", sweep.to_degrees())),
                }
                out
            }
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
            Node::Pattern { kind, count, .. } => match (name, &mut *kind) {
                // Rounded and floored at two, because one instance is not a
                // pattern and a fraction of one is not a number of things. The
                // cap keeps a dragged count from asking the shader for a loop
                // nobody meant.
                ("count", _) => {
                    *count = (v.round() as i64).clamp(2, i64::from(MAX_INSTANCES)) as u32;
                }
                ("step_x", Repeat::Linear { step }) => step.x = v,
                ("step_y", Repeat::Linear { step }) => step.y = v,
                ("step_z", Repeat::Linear { step }) => step.z = v,
                ("sweep", Repeat::Circular { sweep }) => *sweep = v.to_radians(),
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
            // A step of nothing stacks every instance in one place, which is a
            // hundred copies of one shape and a hundred times the work for a
            // part that looks unchanged.
            Node::Pattern { kind, count, .. } => {
                (2..=MAX_INSTANCES).contains(&count)
                    && match kind {
                        Repeat::Linear { step } => step.length_squared() > 1.0e-12,
                        Repeat::Circular { sweep } => sweep.is_finite() && sweep.abs() > 1.0e-6,
                    }
            }
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

#[cfg(test)]
mod pattern_tests {
    use super::{Node, Repeat, MAX_INSTANCES};
    use crate::glam::Vec3;
    use crate::{eval, Arena, NodeId};

    fn four_in_a_row(step: Vec3, count: u32) -> (Arena, NodeId) {
        let mut arena = Arena::new();
        let ball = arena.insert(Node::Sphere { radius: 1.0 }).expect("valid");
        let pattern = arena
            .insert(Node::Pattern {
                child: ball,
                kind: Repeat::Linear { step },
                count,
            })
            .expect("valid pattern");
        (arena, pattern)
    }

    /// The whole promise: there is something at every instance and nothing in
    /// between. Without it a pattern is a shape drawn once with extra nodes.
    #[test]
    fn every_instance_is_there_and_the_gaps_are_not() {
        let (arena, id) = four_in_a_row(Vec3::X * 10.0, 4);
        for i in 0..4 {
            let at = Vec3::X * 10.0 * i as f32;
            assert!(
                eval(&arena, id, at) < 0.0,
                "instance {i} is missing at {at:?}"
            );
        }
        // Halfway between two, well outside a one millimetre ball.
        assert!(eval(&arena, id, Vec3::X * 5.0) > 0.0, "the gap filled in");
        // And it stops after the last one.
        assert!(eval(&arena, id, Vec3::X * 40.0) > 0.0, "it went on forever");
    }

    /// Changing the count is one number, which is the entire reason this node
    /// exists rather than four copies of a feature.
    #[test]
    fn the_count_is_one_parameter() {
        let (mut arena, id) = four_in_a_row(Vec3::X * 10.0, 4);
        assert!(eval(&arena, id, Vec3::X * 40.0) > 0.0);

        let mut node = arena.get(id).expect("there").clone();
        assert!(node.set_param("count", 6.0), "count is not settable");
        arena.replace(id, node).expect("still valid");

        assert!(
            eval(&arena, id, Vec3::X * 40.0) < 0.0,
            "raising the count did not add an instance"
        );
    }

    /// A circular pattern closes on itself for a full turn and spans end to end
    /// for anything less, which is what an arc of holes needs.
    #[test]
    fn a_full_turn_does_not_double_up_on_itself() {
        let full = Repeat::Circular {
            sweep: std::f32::consts::TAU,
        };
        assert_eq!(full.spans(4), 4, "a full turn should divide by the count");
        let last = full.placement(3, 4).rotation;
        let first = full.placement(0, 4).rotation;
        assert!(
            (last * Vec3::X - first * Vec3::X).length() > 0.5,
            "the last instance landed on the first"
        );

        let quarter = Repeat::Circular {
            sweep: std::f32::consts::FRAC_PI_2,
        };
        assert_eq!(quarter.spans(3), 2, "a partial sweep divides by the gaps");
        let end = quarter.placement(2, 3).rotation * Vec3::X;
        assert!(
            (end - Vec3::Y).length() < 1.0e-4,
            "a quarter sweep should end on Y, ended on {end:?}"
        );
    }

    /// A pattern that goes nowhere stacks every instance in one place: a hundred
    /// copies of one shape, a hundred times the work, and a part that looks
    /// unchanged while the tracer crawls.
    #[test]
    fn a_pattern_that_goes_nowhere_is_refused() {
        let mut arena = Arena::new();
        let ball = arena.insert(Node::Sphere { radius: 1.0 }).expect("valid");
        for kind in [
            Repeat::Linear { step: Vec3::ZERO },
            Repeat::Circular { sweep: 0.0 },
        ] {
            assert!(
                arena
                    .insert(Node::Pattern {
                        child: ball,
                        kind,
                        count: 4,
                    })
                    .is_err(),
                "{kind:?} was accepted"
            );
        }
    }

    /// One instance is not a pattern, and the cap is what stops a dragged count
    /// asking the shader for a loop nobody meant.
    #[test]
    fn the_count_is_held_between_two_and_the_cap() {
        let mut node = Node::Pattern {
            child: NodeId(0),
            kind: Repeat::Linear { step: Vec3::X },
            count: 4,
        };
        for (asked, expected) in [(1.0, 2), (0.0, 2), (-5.0, 2), (1.0e9, MAX_INSTANCES)] {
            assert!(node.set_param("count", asked));
            let got = node
                .params()
                .into_iter()
                .find(|(n, _)| *n == "count")
                .expect("count")
                .1;
            assert_eq!(got as u32, expected, "asking for {asked} gave {got}");
        }
    }

    /// The bounds have to hold every instance. A bound that is too small crops
    /// the part out of the mesh and out of the camera framing.
    #[test]
    fn the_bounds_hold_every_instance() {
        let (arena, id) = four_in_a_row(Vec3::X * 10.0, 4);
        let b = crate::bounds(&arena, id);
        assert!(
            b.min.x <= -1.0 + 1.0e-3,
            "the first instance is outside {b:?}"
        );
        assert!(
            b.max.x >= 31.0 - 1.0e-3,
            "the last instance is outside {b:?}"
        );
    }
}
