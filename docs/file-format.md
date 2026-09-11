# The `.shapecad` format

A document is JSON: the node arena, the root, and the names. Plain text rather
than a container, because it diffs cleanly in version control and is directly
legible to a language model, which matters for a tool whose premise is that
agents can read and edit designs.

## Shape

```json
{
  "format": 1,
  "generator": "ShapeCAD 0.0.1",
  "units": "mm",
  "arena": {
    "slots": [
      { "Box": { "half": [30.0, 20.0, 3.0], "round": 1.0 } },
      { "Cylinder": { "radius": 2.5, "half_height": 10.0, "round": 0.0 } },
      null,
      { "Union": { "a": 0, "b": 1, "smooth": 4.0 } }
    ]
  },
  "root": 3,
  "names": [[3, "bracket"]]
}
```

| Field | Meaning |
|---|---|
| `format` | Version. A file from the future is refused rather than misread |
| `generator` | Informational |
| `units` | Always `"mm"` today. Recorded so a change is detectable rather than silently reinterpreting every dimension |
| `arena.slots` | Nodes by index. `null` is a tombstone, a deleted node whose id is never reused |
| `root` | The node the document renders and exports, or `null` |
| `names` | `[id, name]` pairs, sorted by id so the file diffs cleanly |

A slot's index **is** its `NodeId`. Ids survive a round trip, which is asserted by
a test: renumbering them on save would invalidate every stored selection and
every agent-held reference.

## What is not saved

**The edit log.** It grows without bound while the arena is proportional to the
model, and undo history is a property of a session rather than of a part. An
opened document has no undo history, which a test pins down.

**Tombstones are kept**, so a file carries a little dead weight after deletions.
Compacting would renumber live nodes and break the stability above; if it ever
becomes worth doing it needs an explicit id-remapping step.

## Loading

`sc_doc::file::open` refuses a file whose `format` is newer than the build
understands, and validates that the root and every named id refer to live nodes
before constructing a `Document`. A file that parses but describes an
inconsistent model is an error, not a partially-loaded document.

Failures are reported, never partial: a failed open leaves the existing document
untouched.

## Versioning

Bump `FORMAT_VERSION` for any change that an older build would misread. Adding a
node kind is such a change, because an older build cannot evaluate it, so it needs a
bump. Adding an *optional* field with a serde default does not.
