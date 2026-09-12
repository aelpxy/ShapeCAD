# The `.shapecad` format

A document is JSON: the node arena, the root, and the names. Plain text rather
than a container, because it diffs cleanly in version control and is directly
legible to a language model, which matters for a tool whose premise is that
agents can read and edit designs.

One kind of data is too big for that and lives beside the document instead: the
voxel grid behind an imported mesh. See [Imported geometry](#imported-geometry).

## Shape

```json
{
  "format": 4,
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

## Imported geometry

An imported triangle mesh is stored as a voxel signed distance grid, so it
behaves like any other field in the kernel. A grid is megabytes of binary, which
is exactly what this format is not, so grids live beside the document rather than
in it and the JSON references them by id. [ADR 0006](adr/0006-sidecar-assets.md)
covers the decision and what it costs.

```
part.shapecad     the JSON above, with mesh nodes naming an asset id
part.assets/      one binary grid per asset
  00000000.scsdf
  00000001.scsdf
```

The directory name is the document's path with its extension replaced, so
renaming or moving the pair keeps them associated. Nothing in the file records
where the assets are; an absolute path in a document would survive exactly one
move.

A `Mesh` node in the JSON looks like this, and carries no samples:

```json
{ "Mesh": { "asset": 0 } }
```

### Binary grid format

Little endian throughout. Native endianness would be a portability bug waiting
to happen: the file would read back as plausible noise on the other kind of
machine rather than failing.

| Offset | Size | Field |
|---|---|---|
| 0 | 8 | Magic, `SCSDF\0\0\0` |
| 8 | 4 | Version, `u32`, currently 1 |
| 12 | 12 | `dims`, three `u32`: samples along x, y, z |
| 24 | 12 | `origin`, three `f32`: model-space position of sample (0, 0, 0) |
| 36 | 4 | `spacing`, `f32`: distance between adjacent samples |
| 40 | 4 x product of dims | Samples, `f32`, x varying fastest, then y, then z |

The grid version is separate from the document version: the encoding can change
without touching the JSON, and usually will not.

Reading checks the magic, the version, and that the declared dimensions agree
with the actual byte length, in that order and all before a single sample is
allocated. A truncated or mismatched file is a typed error, never a panic, never
a partial grid, and never an allocation taken on the header's word.

### Saving

Only assets a live node references are written. A tombstoned node is not written
to the file, so its grid cannot be reached from the file either, and writing it
would put dead megabytes beside every part that has ever had a mesh deleted from
it. Files for assets that are no longer referenced are deleted, and the directory
itself is removed once it holds nothing, so **a part with no imported meshes is
still exactly one file**.

The order is: add the new grids, write the document, then delete the dead grids.
Nothing the document on disk refers to is removed before its replacement is in
place, so a save that fails part way through leaves the previous document
openable rather than pointing into a half-rewritten directory.

**Saving to a new path takes the assets with it**, as a full copy. A document
that pointed back at grids somewhere else would break the moment either copy
moved, and a saved file that cannot be opened on its own is not a saved file. The
cost is that a save-as rewrites every grid, which for a part carrying large
imports is the slowest thing the application does.

### Garbage collection

Two different reachability rules, because disk and memory are reachable from
different places.

**On disk**: an asset is written if and only if a live node in the saved arena
references it. That is exactly what reopening the file will ask for.

**In memory**: an asset is kept if a live node references it *or* any node stored
in the undo log does, including the redo tail. Deleting a mesh node leaves its
grid unreferenced by the arena, but the node itself lives on inside the
reversing effect, so undo can put it back; dropping the grid would make an
ordinary press of undo resurrect a node with no geometry in it.

Collection therefore runs at one moment only: when a fresh edit discards the redo
tail. Undo and redo move a cursor without shortening the log, so nothing becomes
unreachable there. Getting this wrong in either direction is expensive: too
eager and redo is broken, too lazy and every save carries dead megabytes.

### Opening a document that has assets

A document with no mesh nodes asks for nothing, so the absence of a sidecar
directory is not an error. That is what keeps version 2 files, which cannot
contain a mesh node, loading unchanged.

A mesh node's `Deserialize` cannot produce a real grid, because the JSON does not
carry one, so it produces an empty placeholder that `is_valid` rejects. Loading
fills those in from the sidecar directory and fails if it cannot. A document that
finished loading with a placeholder still in it would render and export as
silently empty geometry, which is worse than not opening at all, so a missing,
incomplete or corrupt sidecar directory is reported with the offending asset
named.

Only the assets the document asks for are read. A stale file left in the
directory by an older save is ignored, and removed by the next one.

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

| Version | What it added |
|---|---|
| 1 | The original arena, root and names |
| 2 | Parametric extrusion profiles. A rectangle became a width and a height rather than four points, which an older build cannot read |
| 3 | Mesh nodes, and with them the sidecar directory. An older build cannot evaluate a mesh node and would not know to look for the directory either |
| 4 | `Node::Prism`, an unbounded sweep. It is how a through cut is expressed, so that a hole sized against the part as it stood cannot silently become a blind recess when the part grows |
| 5 | `Node::Pattern`, the same subtree repeated in a line or about an axis. The count is one number rather than a copy per instance, so an older build would read four holes where the file says six |
| 6 | `Node::Revolve`, a profile spun about the Z axis. An older build cannot evaluate it, and a turning it dropped would leave a part that looks finished and is not |

Older files still load unchanged. A version 2 or 3 file cannot contain a prism,
and a version 2 file cannot contain a mesh node either, so it asks for no assets
and the sidecar directory it does not have is not missed.
