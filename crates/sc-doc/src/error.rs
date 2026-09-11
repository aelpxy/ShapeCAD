//! Errors from document-level edits.

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

/// Result alias for fallible document operations.
pub type Result<T> = std::result::Result<T, DocError>;
