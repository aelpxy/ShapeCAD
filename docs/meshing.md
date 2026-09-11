# Meshing

`sc-mesh` converts the implicit field into triangles for export. This is the only
place in the project where a mesh exists.

## Dual contouring

Marching cubes places vertices on grid edges and rounds every sharp feature off.
Dual contouring places one vertex per *cell*, positioned to best satisfy the
tangent planes of that cell's surface crossings, which reproduces corners and
creases exactly. For mechanical parts that is the difference between a box and a
pillow.

1. Sample the field on a regular grid covering the model's bounds plus two cells
   of padding.
2. For each cell with a sign change on any of its twelve edges, locate the
   crossings, take the surface normal at each, and solve for one vertex.
3. For each grid edge with a sign change, emit a quad joining the vertices of the
   four cells around it.

The padding is not cosmetic: it guarantees the solid never reaches the grid
boundary. A surface clipped by the boundary leaves a hole, and a hole is the one
defect that makes a mesh unprintable.

## The QEF

Where the vertex goes is the whole game. At the centroid of the crossings you get
rounded-off corners; at the point minimising squared distance to the crossings'
tangent planes you get the corner reproduced exactly.

Two guards matter:

**Regularisation.** Pulling toward the mass point keeps the normal equations
solvable when the constraints are degenerate: a flat face, or an edge, where the
surface does not pin down all three axes and the solution would otherwise run off
to infinity.

**Clamping to the cell.** An ill-conditioned system can place the minimiser far
outside its own cell, producing long spikes and self-intersecting triangles.
Because of the clamp, the honest bound on how far a vertex can sit from the
surface is the cell *diagonal*, not the cell size.

> `Qef` has a hand-written `Default`. Deriving it would initialise the normal
> equations with `Mat3::default()`, which in glam is the **identity, not zero**,
> seeding every cell with a phantom constraint pulling its vertex toward the
> origin. Meshes came out uniformly 6–11% undersized: watertight, correctly
> wound, plausible in a render, and quite wrong in a printed part. It was caught
> by asserting meshed volume against the analytic volume of a sphere, not by any
> structural check.

## What the output guarantees

`Mesh::topology()` classifies every edge, and the distinction between these two
is deliberate:

**`is_printable()`**: no holes, no inconsistent winding. These are the defects
that actually stop a slicer: a hole leaves the solid undefined, inverted winding
turns it inside out. This always holds.

**`is_manifold()`**: additionally, no edge shared by more than two triangles.
This does **not** always hold.

Uniform dual contouring places exactly one vertex per cell. Where two separate
sheets of the surface pass through the same cell (a thin gap, or two bodies
nearly touching, relative to the grid), that vertex has to serve both, and the
edges around it end up shared by four triangles. On a cylinder cut by a smooth
union of spheres at resolution 24, roughly four percent of edges are affected.
That is a structural property of the algorithm, not a stray cell.

Most slicers repair it silently, which is why it does not fail `is_printable`.
The real fix is Manifold Dual Contouring: detect that a cell's crossings form
more than one connected component and emit a vertex per component. Refining the
grid makes it proportionally rarer, since it only occurs where a feature is finer
than a cell. `known_limitation_two_sheets_in_one_cell_are_non_manifold` documents
this against a reproducing case and will start failing if it is ever fixed.

## Choosing a resolution

`Settings::resolution` is cells along the longest axis; the CLI reports the
resulting millimetres per cell. As a rule, put the cell size at or below half the
layer height you intend to print at. Finer than that and you are paying for
detail the printer cannot express.

```
cargo run --release -p sc-cli -- export part.stl --resolution 192
```

Sampling dominates the cost and is parallelised across slices. Meshing the
reference bracket at 0.35mm per cell takes about 100ms and produces ~158k
triangles.

## Output

Binary STL today: no units, no colour, 32-bit floats, every vertex repeated per
triangle. It is written because every slicer accepts it. 3MF is the better target
with real units, colour and multi-material, and is not done yet.
