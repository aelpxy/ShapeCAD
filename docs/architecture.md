# Architecture

## Crates

```
sc-geom    implicit kernel: node DAG, evaluation, bounds, hashing, WGSL codegen
  ^
  |
sc-doc     document model: command log, undo/redo, file format, sample parts
  ^   ^
  |   |
  |  sc-mesh    dual contouring to printable triangle meshes, STL output
  |   ^
sc-render  sphere-traced viewport, camera, offscreen capture
  ^   ^
  |   |
sc-app     the desktop application
sc-cli     headless runner
```

Dependencies point downward only. `sc-geom` knows nothing about documents;
`sc-doc` knows nothing about rendering; neither knows anything about the
application.

### What each one owns

**`sc-geom`** owns the definition of geometry. A model is a DAG of implicit
operations. This crate evaluates that field on the CPU, computes conservative
bounds, hashes a model deterministically, and generates the WGSL that evaluates
the same field on a GPU. It has one dependency, `glam`, and `serde` is optional.

**`sc-doc`** owns *change*. It wraps an arena in a `Document` and exposes exactly
one way to modify it. It also defines the `.shapecad` file format and the sample
parts used by tests, the CLI and the application.

**`sc-mesh`** owns the export path. It is the only place in the project where a
mesh exists at all.

**`sc-render`** owns what appears on screen: GPU selection, the camera, the
shader that sphere-traces a field, and offscreen capture.

**`sc-app`** owns interaction: panels, tools, input, files. It holds no geometry
logic; everything it does is a command applied to a `Document`.

## Data flow

```
        Command ──► Document::apply ──► Effect ──► Arena
                          │                          │
                          │                          ├──► wgsl::generate ──► Renderer ──► screen
                          │                          │
                          │                          └──► mesh::contour ──► stl::write ──► printer
                          │
                          └──► log ──► undo / redo / replay / save
```

A `Command` is what a caller *expresses*. An `Effect` is what *happened*, with
every node id made explicit. Only effects are stored, because only they can be
replayed faithfully. See [ADR 0003](adr/0003-command-log.md).

## The two rules

**Every mutation goes through `Document::apply`.** The interface, the CLI and (in
future) the agent layer share one command vocabulary. An agent can only do what a
user could, and anything a user does is replayable, undoable and legible to an
agent. Reaching into `Arena` from application code defeats all three.

**Commands are all-or-nothing.** Validation happens before any mutation, so a
rejected edit leaves the document byte-identical and unlogged. Anything that can
fail halfway breaks undo, replay, and any caller that edits speculatively.

## Editing a node that something else points at

The arena is a DAG, not a tree: a node can be reached down more than one path.
Anything that puts a new node where an old one used to sit has to rewire every
parent, which is what `Arena::parents_of` is for.

Forgetting is a quiet failure rather than a loud one. Creating the node succeeds,
the design tree shows it, and the model renders exactly as before, because the
root still reaches the original. `AppState::wrap_selection` had this bug: a
modifier applied to anything below the root produced a byte-identical model.
`a_modifier_applied_below_the_root_changes_the_model` pins it, by hash rather
than by inspecting the tree.

Read the parents before creating the wrapper. Afterwards the wrapper is one of
them, and rewiring it points it at itself.

## Features built on other features

A sketch attached to the far face of a feature produces a placement that is
*derived*: `Node::Transform` records the node it was resolved from in its `on`
field, and `Document::apply` recomputes every derived placement after the edit it
was asked to make. Shorten a base and the boss on top follows it down, in the
same undo step, with the moves logged as ordinary `Replace` effects so the
history stays replayable.

`on` is provenance, not geometry. It is not among the node's `children()`, and
evaluation, bounds, code generation, picking and hashing all ignore it, so a
derivation can never change the shape a document hashes to. What it does buy is
protection: the arena refuses to delete a node a placement is derived from, and
rejects a derivation that points inside the placement's own subtree.

The placement is stored resolved rather than worked out while evaluating.
Evaluation runs millions of times per mesh; resolving a placement means walking
the tree from the root. The stored transform is a cache, and the end of
`Document::apply` is its one invalidation point. See
[ADR 0005](adr/0005-derived-placements.md).

## Why there is no scene graph

The node DAG *is* the scene. There is no separate render representation to keep
in sync: the viewport generates a shader from the same arena the document holds,
and regenerates it when the geometry changes. The only derived artefact is the
exported mesh, produced on demand.

This is why selection is cheap to show: the highlight is a second field function
emitted from the same tree, not a separate pass or a picking buffer.

## Testing layers

| Layer | Where | What it establishes |
|---|---|---|
| Example tests | `#[test]` in `src/` | Specific claims, readable as documentation |
| Property tests | `tests/properties.rs` | Invariants across thousands of generated models |
| Golden geometry | `cargo run -p sc-cli -- selftest` | A reference part still comes out the same shape |
| Shader validation | `naga` in `sc-geom` tests | Generated WGSL parses and type-checks without a GPU |
| GPU smoke test | `sc-render/tests/smoke.rs` | The whole render path produces an image; skips where there is no GPU |

The property tests are where the real guarantees live. See
[kernel.md](kernel.md#invariants).
