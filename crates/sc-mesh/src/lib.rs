//! `ShapeCAD` meshing.
//!
//! Converts the implicit field into triangles for export. This is the only place
//! in the application where a mesh exists at all: the viewport traces the field
//! directly, so tessellation happens once, on the way to a printer.

pub mod dual_contour;
pub mod mesh;
pub mod qef;
pub mod stl;

pub use dual_contour::{contour, Settings};
pub use mesh::{Mesh, Topology};
