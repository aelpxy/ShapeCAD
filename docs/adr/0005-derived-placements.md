# 5. A placement records what it was derived from, and is regenerated on edit

Status: accepted

## Context

A sketch can be attached to the far face of an existing feature, and a pad or a
pocket drawn there is placed on that face. Until now the attachment lasted only
as long as the sketch: `AppState::place` baked the resolved transform into the
tree and nothing recorded where the number had come from.

Change the base from 10mm to 4mm afterwards and the boss stayed at z = 10,
hovering 6mm above the part it was supposed to sit on. The model became two
disconnected pieces, it still meshed, it still hashed, and nothing in the
interface said so. That is the worst class of defect this project has: a part
that looks right and prints wrong.

## Decision

`Node::Transform` carries a third field, `on: Option<NodeId>`, naming the
feature the placement was derived from. `Document::apply` recomputes every
derived placement after the edit it was asked to make, and logs each one that
moved as an ordinary `Effect::Replace`.

Three things fall out of that sentence, and each was a decision of its own.

**Provenance, not a new node kind.** A `DerivedTransform` variant would need
identical handling in `eval.rs`, `bounds.rs`, `wgsl.rs`, `pick.rs` and `hash.rs`,
all of it doing exactly what `Transform` already does. The dependency is not
geometry; it is a note about where a number came from. So `on` is excluded from
`children()` and from `map_children()`, and every consumer of the tree ignores
it. A derivation cannot change the shape a document hashes to, which
`cargo run -p sc-cli -- selftest` checks on every run.

**Stored resolved, not computed during evaluation.** The obvious alternative is
to leave the placement implicit and resolve it while evaluating the field. It is
not affordable. Evaluation runs millions of times per mesh and tens of millions
per frame, and resolving a placement means walking the tree from the root to find
the face it names. The resolved transform is a cache with an explicit
invalidation point: the end of `Document::apply`, which runs once per command
rather than once per sample.

**Regeneration emits real effects.** Fixing up the arena quietly would leave the
log describing a document that no longer exists, and undo would take back the
edit while leaving its consequences standing. Instead the moves are `Replace`
effects like any other, bracketed into the same step as the edit that caused
them, so one press of undo takes back the base's new depth *and* the boss that
followed it. `Document::replay` applies logged effects straight through `run` and
must never regenerate on top of them; it would be deriving placements from a
model it is only half way through rebuilding.

Derived placements chain: a boss on a boss on a base. Recomputing one moves the
face the next sits on, so regeneration iterates to a fixed point, bounded by the
number of derived placements in the document.

## Consequences

The arena now protects two kinds of reference rather than one. `Arena::remove`
refuses to delete a node a derived placement names, the same way it refuses to
delete one that is still a child, and a derivation pointing into the placement's
own subtree is rejected as a cycle, because its position would then depend on
its own result.

The file format did not change version. `on` defaults to `None`, so every
version 2 document already on disk still opens, and one saved without a
derivation is exactly what it was before.

A derived placement is owned by its derivation. Editing its `x`, `y` or `z` in
the property panel is overwritten by the next regeneration, which is correct in
the sense that the placement is not an independent fact, but it means the panel
offers an edit that does not stick. Detaching a feature from its face, so it can
be nudged freely, is not implemented yet and is the obvious next step.

A pocket's *depth* is still fixed at the moment it is cut, sized from the bounds
of the model at that time. Regeneration moves the cut with the face it was cut
from; it does not re-measure how far it has to reach. A base that grows can
therefore leave a blind recess where there used to be a through hole. This
predates the change, and is why `add_pocket` keeps its overshoot in a placement
of its own rather than folding it into the derived one.
