//! The implicit node tree: `ShapeCAD`'s document representation of geometry.
//!
//! This is a DAG, not a tree — subtrees may be shared. It is the authoritative
//! description of a model; evaluation backends (CPU, WGSL, `fidget`) all consume
//! it and none of them own it.

use crate::math::Transform;
use glam::{Vec2, Vec3};

/// Upper bound on profile complexity.
///
/// Profiles are currently emitted into the shader as literal arrays, so an
/// unbounded one would produce an unusable amount of generated code. Lifting
/// this means moving profile data into a storage buffer.
pub const MAX_PROFILE_POINTS: usize = 256;

/// Twice the signed area of a polygon, by the shoelace formula.
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
/// hold a selection, and an AI agent hold a reference, across arbitrary edits —
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
#[derive(Clone, Debug, PartialEq)]
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
    },
    /// Grows (positive) or shrinks (negative) the solid by `distance`.
    /// This is how clearance fits for printing are expressed.
    Offset {
        /// The subtree being offset.
        child: NodeId,
        /// Distance to grow by; negative shrinks.
        distance: f32,
    },
    /// A closed 2D profile swept along +Z: the implicit equivalent of a pad.
    ///
    /// The profile lives in the local XY plane and the solid runs from z = 0 to
    /// z = `height`, so a sketch sits on its own plane rather than straddling
    /// it. Place it in space with an enclosing [`Node::Transform`].
    Extrude {
        /// Closed polygon, in order. The winding may be either way; the field
        /// determines inside-ness by crossing count, not by orientation.
        profile: Vec<Vec2>,
        /// Distance swept along +Z. Must be positive.
        height: f32,
    },
    /// Hollows the solid inward, preserving the outer surface.
    Shell {
        /// The subtree being hollowed.
        child: NodeId,
        /// Wall thickness. Must be positive.
        thickness: f32,
    },
}

impl Node {
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
            Node::Union { .. } => "union",
            Node::Difference { .. } => "difference",
            Node::Intersection { .. } => "intersection",
            Node::Transform { .. } => "transform",
            Node::Offset { .. } => "offset",
            Node::Extrude { .. } => "extrude",
            Node::Shell { .. } => "shell",
        }
    }

    /// The node's direct child references, in declaration order.
    pub fn children(&self) -> impl Iterator<Item = NodeId> + '_ {
        let (a, b) = match *self {
            Node::Union { a, b, .. }
            | Node::Difference { a, b, .. }
            | Node::Intersection { a, b, .. } => (Some(a), Some(b)),
            Node::Transform { child, .. }
            | Node::Offset { child, .. }
            | Node::Shell { child, .. } => (Some(child), None),
            _ => (None, None),
        };
        a.into_iter().chain(b)
    }

    /// Rewrite child references in place. Used by cycle-safe edits and by
    /// compaction, which renumbers ids.
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
            _ => {}
        }
    }

    /// Named scalar parameters, in a stable order.
    ///
    /// This exists so the agent layer and the property panel can both drive
    /// edits generically instead of matching on every variant.
    #[must_use]
    pub fn params(&self) -> Vec<(&'static str, f32)> {
        match *self {
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
            Node::Extrude { height, .. } => vec![("height", height)],
            Node::Shell { thickness, .. } => vec![("thickness", thickness)],
        }
    }

    /// Set a named parameter. Returns false if the name is not valid for this
    /// node kind, leaving the node untouched.
    pub fn set_param(&mut self, name: &str, v: f32) -> bool {
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
            Node::Transform { xform, .. } => match name {
                "x" => xform.translation.x = v,
                "y" => xform.translation.y = v,
                "z" => xform.translation.z = v,
                "scale" => xform.scale = v,
                _ => return false,
            },
            Node::Offset { distance, .. } if name == "distance" => *distance = v,
            Node::Extrude { height, .. } if name == "height" => *height = v,
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
        if let Node::Extrude { profile, height } = self {
            return *height > 0.0
                && profile.len() >= 3
                && profile.len() <= MAX_PROFILE_POINTS
                && profile.iter().all(|p| p.is_finite())
                && polygon_area(profile).abs() > 1.0e-6;
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
            Node::Plane { normal, .. } => normal.length_squared() > 1e-12,
            Node::Union { smooth, .. }
            | Node::Difference { smooth, .. }
            | Node::Intersection { smooth, .. } => smooth >= 0.0,
            Node::Transform { xform, .. } => xform.is_valid(),
            Node::Offset { .. } => true,
            Node::Shell { thickness, .. } => thickness > 0.0,
            // Matched by reference: the profile is not `Copy`.
            Node::Extrude { .. } => unreachable!("handled before the copy match"),
        }
    }
}
