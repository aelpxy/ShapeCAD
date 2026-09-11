//! The command vocabulary.
//!
//! Every mutation of a document goes through exactly one of these. The UI, the
//! CLI and the AI agent layer all speak this same vocabulary, so there is no
//! second, parallel path into the geometry. That is deliberate: an agent can
//! only ever do things a user could also do, and anything a user does is
//! automatically replayable, undoable and visible to an agent.

use sc_geom::{Node, NodeId};

/// A single user-expressible mutation.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Creates a node. The new id is returned from `apply`.
    Add {
        /// The node to create.
        node: Node,
    },
    /// Overwrites a node in place, keeping its id.
    Replace {
        /// The node to overwrite.
        id: NodeId,
        /// Its replacement.
        node: Node,
    },
    /// Changes one named scalar. The common case for dragging a value.
    SetParam {
        /// The node to edit.
        id: NodeId,
        /// Parameter name, as reported by [`Node::params`].
        name: String,
        /// The new value.
        value: f32,
    },
    /// Deletes a node. Refused if it is the root or is still referenced.
    Delete {
        /// The node to delete.
        id: NodeId,
    },
    /// Points the document at a different root, or at nothing.
    SetRoot {
        /// The new root.
        root: Option<NodeId>,
    },
    /// Attaches or clears a human-readable label.
    SetName {
        /// The node to label.
        id: NodeId,
        /// The new label, or `None` to clear it.
        name: Option<String>,
    },
}

/// A fully-resolved mutation: the durable record of what actually happened.
///
/// Commands are what callers *express*; effects are what actually *happened*,
/// with every id made explicit. Keeping them separate is what makes the log
/// replayable.
///
/// The distinction is not academic. [`Command::Add`] does not name an id, so it
/// gets one at apply time. Undo leaves a tombstone behind, which advances the id
/// counter, so replaying a sequence of abstract commands can allocate different
/// ids than the original run did, and every later reference by id then points
/// at the wrong node or at nothing. The log therefore stores effects, and
/// [`crate::Document::replay`] reproduces a model exactly.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Bring a node into existence at a specific id.
    Create {
        /// The id to create at.
        id: NodeId,
        /// The node to store there.
        node: Node,
    },
    /// Overwrite the node at an id.
    Replace {
        /// The id to overwrite.
        id: NodeId,
        /// Its replacement.
        node: Node,
    },
    /// Tombstone the node at an id.
    Destroy {
        /// The id to destroy.
        id: NodeId,
    },
    /// Repoint the document root.
    SetRoot {
        /// The new root.
        root: Option<NodeId>,
    },
    /// Set or clear a node's label.
    SetName {
        /// The node to label.
        id: NodeId,
        /// The new label, or `None` to clear it.
        name: Option<String>,
    },
}
