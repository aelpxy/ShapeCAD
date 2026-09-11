# 1. Use an implicit (SDF) kernel rather than B-rep

Status: accepted

## Context

ShapeCAD targets 3D printing exclusively. The conventional choice for a CAD
kernel is a boundary representation over NURBS surfaces: OpenCASCADE if open
source, Parasolid or ACIS if commercial. That is what Shapr3D, Onshape and
SolidWorks use.

Two constraints rule out the commercial options outright: the project is
permanently open source, and there is no business behind it to fund a licence.
That leaves OpenCASCADE, whose boolean and fillet operations fail often enough
on real input to be the dominant source of user-visible defects in FreeCAD.

## Decision

Represent geometry as a DAG of implicit signed-distance operations, evaluated on
the GPU.

## Consequences

Gained:

- Booleans are `min`/`max` and cannot fail.
- A fillet is a blend radius parameter, not an operation on boundary topology.
- Hollowing, clearance offsets and lattices are arithmetic on the field.
- Meshed output is watertight and manifold by construction, so there is no mesh
  repair step and no unsliceable exports.
- Nodes have permanently stable ids, so the topological naming problem, the
  chronic wound in history-based B-rep modellers, does not arise.
- A typed parameter tree is far more legible to a language model than opaque
  B-rep face ids, which directly serves the AI-agent goal.

Given up:

- No exact NURBS surfaces and no true STEP export. Printing does not need them;
  handing a part to a machinist would.
- Selection is by tree node, not by face. Direct manipulation has to be designed
  rather than copied from existing tools.
- Sketch constraints must be written rather than borrowed from `planegcs`.
- Evaluation cost must be managed by interval pruning and GPU evaluation.

If the 3D-printing-only scope is ever abandoned, this decision must be revisited
first, because nearly everything else follows from it.
