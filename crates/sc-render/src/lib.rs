//! `ShapeCAD` viewport.
//!
//! The field is sphere-traced directly on the GPU: there is no tessellation
//! anywhere in the display path. Meshing happens only on export, which means
//! what you see is the actual implicit surface rather than an approximation of
//! it, at any zoom level.

pub mod camera;
pub mod gpu;
pub mod renderer;
pub mod shader;
pub mod snapshot;

pub use camera::{CameraRig, OrbitCamera};
pub use renderer::Renderer;
