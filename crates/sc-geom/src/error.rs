//! Errors from geometry-tree manipulation.
//!
//! Every one of these is returned *before* any mutation happens, so a rejected
//! edit leaves the arena byte-identical. That is what makes commands in
//! `sc-doc` transactional.

use crate::node::NodeId;

/// Why an edit to the geometry DAG was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GeomError {
    /// The id was never issued by this arena.
    UnknownNode(NodeId),
    /// The id was issued but the node has since been deleted.
    DeadNode(NodeId),
    /// Node parameters are out of range, such as a negative radius or a scale
    /// of zero.
    InvalidNode {
        /// The node kind that failed validation.
        kind: &'static str,
        /// Human-readable explanation.
        reason: String,
    },
    /// The edit would make the DAG cyclic.
    Cycle {
        /// The node being edited.
        at: NodeId,
        /// The child reference that would close the loop.
        via: NodeId,
    },
    /// Refusing to delete a node that something still points at.
    StillReferenced {
        /// The node that was to be deleted.
        node: NodeId,
        /// A live node still referencing it.
        by: NodeId,
    },
    /// Tried to recreate a node at an id that is currently live.
    SlotOccupied(NodeId),
}

impl std::fmt::Display for GeomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeomError::UnknownNode(id) => write!(f, "no such node {id}"),
            GeomError::DeadNode(id) => write!(f, "node {id} has been deleted"),
            GeomError::InvalidNode { kind, reason } => {
                write!(f, "invalid {kind} node: {reason}")
            }
            GeomError::Cycle { at, via } => {
                write!(f, "edit to {at} would create a cycle through {via}")
            }
            GeomError::StillReferenced { node, by } => {
                write!(f, "cannot delete {node}: still referenced by {by}")
            }
            GeomError::SlotOccupied(id) => write!(f, "node {id} is already live"),
        }
    }
}

impl std::error::Error for GeomError {}

/// Result alias for fallible geometry operations.
pub type Result<T> = std::result::Result<T, GeomError>;
