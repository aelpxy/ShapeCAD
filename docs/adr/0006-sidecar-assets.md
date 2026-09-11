# 6. Imported geometry lives in sidecar binaries, not in the document

Status: accepted

## Context

An imported triangle mesh becomes a voxel signed distance grid, so that it
behaves like any other field in the kernel rather than as a second, parallel
representation. A grid useful for printing is a few hundred samples on a side,
which is tens of millions of floats: hundreds of megabytes in the worst case and
several in the ordinary one.

A `.shapecad` file is JSON, and that is a decision rather than a default. Plain
text diffs cleanly in version control and is directly legible to a language
model, which matters for a tool whose premise is that agents can read and edit
designs.

Both of the obvious ways to put a grid inside the JSON destroy that. Base64
turns a part into a single multi-megabyte line: no meaningful diff, and a model
cannot read it and would burn its entire context trying. An array of numbers is
worse still, because it diffs line by line and a one-voxel change rewrites a
million lines. Moving to a binary container instead, a zip with the JSON inside,
keeps the size down but gives up the legibility and the diff, which is the thing
the format was chosen for.

## Decision

Grids are stored beside the document, one file per asset, in a directory named
after it. The JSON references them by id and carries no sample data at all.

```
part.shapecad     JSON, as before, with mesh nodes naming an asset id
part.assets/      one binary grid per asset
  00000000.scsdf
  00000001.scsdf
```

The directory name is derived from the document's path rather than recorded in
the file, so renaming or moving the pair keeps them associated and no absolute
path is ever baked into a document.

Saving to a new path writes a complete copy of the sidecar directory there. A
document that pointed back at grids somewhere else would break the moment either
copy moved, and a saved file that cannot be opened on its own is not a saved
file.

Only grids a live node references are written. Assets outside that set are
unreachable from the saved file, so writing them would put dead megabytes beside
every part that has ever had a mesh deleted from it. Memory keeps a wider set
than disk does, because the undo log holds nodes too; the rule is spelled out in
`docs/file-format.md`.

## Consequences

**A document is no longer one file.** This is the whole cost, and it is paid
every time a part leaves the machine it was made on.

- Copying a part by dragging the `.shapecad` file produces something that will
  not open. It fails loudly, with the missing asset named, rather than opening
  with the mesh silently empty, but it still fails.
- Emailing or attaching a part means attaching a folder, or zipping first.
- In version control the JSON diffs as beautifully as it always did, and the
  sidecar files are binary blobs that do not diff at all and will bloat history
  if they change often. A project holding large imports wants them in
  `.gitattributes` as LFS, or out of the repository entirely.

The alternative costs were judged worse. Making the format opaque would be
permanent and would apply to every part, including the overwhelming majority
that contain no imported mesh at all. The sidecar cost applies only to parts that
do: a part with no imports produces exactly one file, exactly as before, and is
unaffected by any of this.

If the single-file property is ever wanted back, the way to get it is an
explicit bundle format for interchange, written and read on demand, rather than
changing what the editor saves.
