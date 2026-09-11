# The implicit kernel

`sc-geom` represents a model as a DAG of implicit operations. Every node defines
a function from a point to a signed distance: negative inside the solid, zero on
its surface, positive outside.

There is no boundary representation. No faces, no edges, no vertices, which is
why the topological naming problem does not arise here, and why there is nothing
to select called an "edge". See [ADR 0001](adr/0001-implicit-kernel.md) and
[ADR 0004](adr/0004-selection-by-seam.md).

## Nodes

Primitives are defined at the origin in a canonical orientation. Placement is
always an enclosing `Transform`, so there is exactly one way to express it.

| Node | Parameters | Notes |
|---|---|---|
| `Sphere` | `radius` | Centred on the origin |
| `Box` | `half`, `round` | `half` is the half-extent of the *outer* surface, so rounding does not change overall size |
| `Cylinder` | `radius`, `half_height`, `round` | Capped, along +Z |
| `Torus` | `major`, `minor` | In the XY plane, axis along Z |
| `Plane` | `normal`, `offset` | Half-space. Unbounded |
| `Extrude` | `profile`, `height` | Closed polygon in local XY swept from z = 0 to z = `height` |
| `Union` | `a`, `b`, `smooth` | `smooth` is a blend radius in millimetres |
| `Difference` | `a`, `b`, `smooth` | `a` minus `b` |
| `Intersection` | `a`, `b`, `smooth` | |
| `Transform` | `child`, `xform` | Rigid plus *uniform* scale |
| `Offset` | `child`, `distance` | Grows or shrinks the solid. How clearance fits are expressed |
| `Shell` | `child`, `thickness` | Hollows inward, keeping the outer surface |

### A fillet is a parameter

`smooth` on a boolean is the fillet radius. It is arithmetic on distances, a
polynomial smooth minimum, not surgery on boundary topology, so it cannot fail,
needs no rebuild, and can be dragged in real time.

The consequence is that a blend applies wherever the two shapes meet. You cannot
fillet three of a box's twelve edges; there are no edges to select.

### Uniform scale only

`Transform` carries a single scale factor, and non-uniform scale is
*unrepresentable* rather than validated against. Applying it degrades the field
from an exact distance to a mere bound on distance, which silently breaks sphere
tracing, offsets, shells and blends everywhere downstream. A stretched box gets
different half-extents instead.

Distances measured in a child's frame must be multiplied by the scale on the way
out. `Transform::apply_distance` exists so this is a named operation rather than
a bare multiply somebody forgets.

## Invariants

These hold for every well-formed tree and are enforced by property tests in
`crates/sc-geom/tests/properties.rs`.

**The field is never NaN.** One NaN poisons every downstream `min` and `max`, so
a single bad evaluation can make a whole model vanish.

**The field is 1-Lipschitz.** Moving a distance `t` changes the reported distance
by at most `t`. This is the defining property of a signed distance field and the
reason sphere tracing terminates: the renderer steps by exactly the distance the
field reports, so a violation lets a ray step straight through a surface. The
strict test excludes smooth blends, which deliberately under-report.

**Bounds never under-report.** `bounds()` may be loose but must never exclude
solid material, because too-small bounds silently clip geometry out of an export.
Smooth blends expand their bounds by the full blend radius; the true bulge is at
most a quarter of that, so this is conservative on purpose.

**Ids are stable.** `NodeId`s are never reused, even after deletion, so a
selection or an agent's reference stays valid across arbitrary edits. The arena
is append-only with tombstones. Deleting a node that something still references
is refused, so a dangling child pointer is unrepresentable, and rewiring is
checked for cycles.

**Generated WGSL always compiles.** Every tree the kernel can express produces a
shader that parses and type-checks, verified with `naga` and no GPU.

## Verifying geometry

Do not eyeball renders. `geometry_hash` returns three independent digests
(structure, field and bounds), so a failure says *what* changed rather than just
that something did:

- structure alone changed: the tree was rebuilt
- field alone changed: a parameter moved
- bounds alone changed: it was resized

It uses FNV-1a rather than `DefaultHasher`, which makes no stability guarantee
across Rust releases; a golden value checked into `tests/corpus` has to mean the
same thing in five years.

## Adding a node kind

1. `node.rs`: add the variant, then `kind()`, `params()`, `set_param()`,
   `is_valid()`, `children()` and `map_children()`. Missing one of these is a
   silent bug, not a compile error, for the ones that match on `_`.
2. `eval.rs`: evaluate it. Keep the result an *exact* distance if you can; if you
   can only bound it, say so in the doc comment, because everything downstream
   assumes exactness.
3. `bounds.rs`: bound it conservatively.
4. `wgsl.rs`: emit it. Anything needing a loop or an array goes in a helper
   function via `Emitter::helpers` rather than inline in the body.
5. `tests/properties.rs`: add it to the generator. This is the step people skip,
   and it is the one that actually tests the node. The invariants above will
   exercise it against thousands of random trees.
6. If it has a profile or other bulk data, check `MAX_PROFILE_POINTS`: bulk data
   is currently compiled into the shader as literals.

The property panel and the agent interface both work off `params()`, so a new
node kind gets an editor with no interface code.

## Known limitations

**Parameters are compiled into the shader as literals**, so every edit triggers a
shader rebuild. Fine for structural edits, too slow for dragging a value at 60fps.
The numeric leaves need to move into a uniform buffer, leaving only topology
changes to force recompilation. `wgsl.rs` is written to make that swap local.

**Profiles are capped at 256 points** for the same reason.

**CPU evaluation is not memoised.** A DAG with heavy sharing is evaluated once per
path rather than once per node. The GPU path does not have this problem because
the emitter memoises by (node, point expression).
