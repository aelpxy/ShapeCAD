# 4. Selection and filleting work on nodes and seams, not edges

Status: accepted

## Context

A mockup of the intended UI showed a conventional CAD fillet workflow: select
three edges, type a radius, enable tangent propagation, apply.

The kernel has no edges. It has no faces or vertices either, only a field. This
is the direct consequence of [ADR 0001](0001-implicit-kernel.md), and it is also
why the topological naming problem does not exist here.

## Decision

Selection is by ray picking into the field, resolved to a node.

Trace a ray to the surface, then evaluate candidate primitives at the hit point
and find which are at (near) zero distance:

- **One primitive at zero**: the user clicked a face. Select that body.
- **Two primitives at zero**: the user clicked a seam. Select the boolean node
  that combines them; dragging then edits its blend radius.

A fillet is therefore a parameter on a boolean, not an operation on edges.

## Consequences

The interaction reads much like the mockup (click a joint, drag a radius), and
it is real-time, cannot fail, and needs no rebuild.

What is given up is filleting an arbitrary subset of edges. One blend radius
applies wherever those two shapes meet: if a clamp meets a body in two places,
both round together. Per-region control would require masking a blend with
helper geometry, which was considered and deferred as too much new vocabulary
for the user to carry.

The UI must not borrow B-rep language it cannot honour. "3 edges selected" would
be a lie the first time someone tried to fillet one edge of a box. Tangent
propagation is meaningless here and should not appear at all.
