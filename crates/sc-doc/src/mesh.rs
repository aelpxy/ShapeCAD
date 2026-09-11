//! Every place the document model has to know what a mesh node is.
//!
//! A `Node::Mesh` carries its grid inline rather than looking it up, because
//! `eval` takes no asset context and must not grow one. That has a consequence
//! for persistence: the node's `Deserialize` cannot conjure several megabytes
//! of samples out of the JSON, so it produces an empty placeholder that
//! `is_valid` rejects, and loading has to fill the placeholder in from the
//! sidecar directory. The four functions here are the whole of that knowledge,
//! kept in one module so the rest of the crate never matches on the variant.
//!
//! # Temporary
//!
//! `Node::Mesh` is landing in the kernel separately. Until it does, the
//! `pending` module below stands in: it answers "not a mesh" for every node, so
//! Kept in one module so that the question "what counts as a mesh node" has a
//! single answer, rather than the same match arm appearing in the asset store,
//! the loader and the saver.

use crate::asset::{AssetId, Grid};
use sc_geom::Node;
use std::sync::Arc;

pub(crate) use real::*;

mod real {
    use super::{Arc, AssetId, Grid, Node};

    pub(crate) fn asset_of(node: &Node) -> Option<AssetId> {
        match node {
            Node::Mesh { asset, .. } => Some(*asset),
            _ => None,
        }
    }

    pub(crate) fn grid_of(node: &Node) -> Option<&Arc<Grid>> {
        match node {
            Node::Mesh { grid, .. } => Some(grid),
            _ => None,
        }
    }

    pub(crate) fn with_grid(node: &Node, grid: Arc<Grid>) -> Option<Node> {
        match node {
            Node::Mesh { asset, .. } => Some(Node::Mesh {
                asset: *asset,
                grid,
            }),
            _ => None,
        }
    }

    pub(crate) fn mesh_node(asset: AssetId, grid: Arc<Grid>) -> Node {
        Node::Mesh { asset, grid }
    }
}

/// Whether a node is a mesh still holding the empty grid its `Deserialize`
/// produced. A document that finishes loading with one of these in it has
/// silently empty geometry, which is a bug rather than a shape.
pub(crate) fn is_placeholder(node: &Node) -> bool {
    grid_of(node).is_some_and(|g| g.data.is_empty())
}
