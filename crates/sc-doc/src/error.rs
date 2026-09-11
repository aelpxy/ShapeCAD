//! Errors from document-level edits.

use crate::asset::AssetId;
use sc_geom::{GeomError, NodeId};

/// Why a command was refused.
#[derive(Clone, Debug, PartialEq)]
pub enum DocError {
    /// The underlying geometry edit was rejected.
    Geom(GeomError),
    /// Node kind has no such parameter.
    UnknownParam {
        /// The node addressed.
        id: NodeId,
        /// The parameter name that does not exist on that node kind.
        name: String,
    },
    /// Refusing to delete the node the document is rooted at; clear the root first.
    IsRoot(NodeId),
}

impl From<GeomError> for DocError {
    fn from(e: GeomError) -> Self {
        DocError::Geom(e)
    }
}

impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DocError::Geom(e) => write!(f, "{e}"),
            DocError::UnknownParam { id, name } => {
                write!(f, "node {id} has no parameter '{name}'")
            }
            DocError::IsRoot(id) => {
                write!(f, "cannot delete {id}: it is the document root")
            }
        }
    }
}

impl std::error::Error for DocError {}

/// Why a loaded document's mesh nodes could not be given their geometry.
///
/// A mesh node deserialises to an empty placeholder that `is_valid` rejects, so
/// either of these means the document would otherwise have finished loading with
/// geometry that is silently empty rather than merely wrong.
#[derive(Clone, Debug, PartialEq)]
pub enum AttachError {
    /// The sidecar directory held no grid for the asset this node names.
    Unresolved {
        /// The mesh node left holding a placeholder.
        node: NodeId,
        /// The asset it references.
        asset: AssetId,
    },
    /// The grid arrived but does not describe a usable field.
    Invalid {
        /// The mesh node it was attached to.
        node: NodeId,
        /// The asset it came from.
        asset: AssetId,
        /// What the kernel objected to.
        cause: GeomError,
    },
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachError::Unresolved { node, asset } => write!(
                f,
                "node {node} references asset {} but no grid was found for it",
                asset.0
            ),
            AttachError::Invalid { node, asset, cause } => write!(
                f,
                "node {node}: asset {} is not a usable grid: {cause}",
                asset.0
            ),
        }
    }
}

impl std::error::Error for AttachError {}

/// Result alias for fallible document operations.
pub type Result<T> = std::result::Result<T, DocError>;
