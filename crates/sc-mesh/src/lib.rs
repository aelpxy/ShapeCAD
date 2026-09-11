//! `ShapeCAD` meshing.
//!
//! Two directions, sharing one [`Mesh`] type.
//!
//! Outward, [`contour`] converts the implicit field into triangles for export.
//! That is the only place in the application where a mesh of modelled geometry
//! exists at all: the viewport traces the field directly, so tessellation
//! happens once, on the way to a printer.
//!
//! Inward, [`import::read`] reads a triangle mesh someone else made and
//! [`voxelize::voxelize`] turns it into a signed distance grid, at which point
//! it is a field function like any other and the kernel can boolean against it
//! without knowing where it came from.

pub mod bvh;
pub mod dual_contour;
pub mod error;
pub mod import;
pub mod mesh;
pub mod obj;
pub mod qef;
pub mod stl;
pub mod voxelize;

#[cfg(test)]
pub(crate) mod testing;

pub use bvh::{Bvh, Nearest, Triangle};
pub use dual_contour::{contour, Settings};
pub use error::{MeshError, Result};
pub use import::Format;
pub use mesh::{Mesh, Topology};
pub use sc_geom::sdf::Grid;
pub use voxelize::voxelize;
