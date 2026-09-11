# Developer API reference

This is the entry point for developers using or extending ShapeCAD. It covers
all explicitly public functions, inherent methods, types and constants in the
four library crates, plus the crate-visible interfaces of the desktop binary.
Signatures and the catalog summaries were checked against source on 2026-09-12.
Standard trait methods and private implementation helpers are not duplicated.

This is an experimental API, with no compatibility guarantee yet. Read
[architecture.md](architecture.md) before changing it. For navigable Rust type
links, trait implementations and complete rustdoc, run:

```sh
cargo doc --workspace --no-deps --all-features
```

Open `target/doc/sc_doc/index.html` in a browser. To include desktop internals,
use `cargo doc -p sc-app --no-deps --document-private-items`.

## Contents

- [Choosing an API](#choosing-an-api)
- [Conventions and contracts](#conventions-and-contracts)
- [Examples](#examples)
- [Editable parameter names](#editable-parameter-names)
- [Library API catalog](#library-api-catalog)
  - [sc-doc](#sc-doc)
  - [sc-geom](#sc-geom)
  - [sc-mesh](#sc-mesh)
  - [sc-render](#sc-render)
- [Desktop internal interfaces](#desktop-internal-interfaces)
- [Command-line interfaces](#command-line-interfaces)
- [Keeping this reference current](#keeping-this-reference-current)

## Choosing an API

| Crate | Use it for | Entry points |
| --- | --- | --- |
| `sc-doc` | Creating and editing designs, history, persistence | `Document`, `Command`, `file` |
| `sc-geom` | Field evaluation, bounds, node definitions, picking, shader generation | `Node`, `Profile`, `eval`, `bounds`, `wgsl` |
| `sc-mesh` | Turning a rooted field into triangles and STL | `contour`, `Settings`, `Mesh`, `stl::write` |
| `sc-render` | Cameras, GPU rendering and image capture | `OrbitCamera`, `CameraRig`, `Renderer`, `snapshot` |
| `sc-app` | Desktop interaction and UI implementation | Internal `AppState`, `Chrome`, `SketchPlane` |
| `sc-cli` | Running the built-in reference part headlessly | `demo`, `export`, `selftest` |

Rust imports use underscores: `sc_doc`, `sc_geom`, `sc_mesh`, `sc_render`.
`sc-app` and `sc-cli` are binaries, not libraries or network APIs. There is no
public HTTP endpoint or implemented external agent tool protocol.

`sc-geom` has no default features; `serde` enables serialization, including glam
values. `sc-doc` enables its `serde` feature by default; disabling it removes
`file`, `Document::snapshot` and `Document::from_snapshot`. `sc-mesh` and
`sc-render` have no crate feature switches. `sc_geom::glam` re-exports the math
library so callers can use matching `Vec2`, `Vec3` and `Quat` types.

## Conventions and contracts

- Lengths are millimetres; angles used by camera and quaternion APIs are radians.
  Profiles lie in local XY. Extrusions run from Z=0 to Z=`depth`.
- `NodeId` is local to an arena. Keep IDs through edits; discard references when
  replacing a document. Tombstones preserve IDs across normal undo/save/load.
- Application edits go through `Document::apply`. `Arena` and `Builder` mutation
  APIs are for kernel construction and tests, not shortcuts around document history.
- `Document::begin_step` / `end_step` group undo actions. Grouping several calls
  does not make them one atomic transaction: each `apply` can succeed or fail
  independently. Balance the pair even when a caller encounters an error.
- `Transform` supports rigid motion and positive uniform scale. Rotation must be
  a normalized quaternion. To change proportions, edit primitive dimensions.
- `Node::Transform::on` records a derived placement. It is separate from geometry
  `children()`. `Document::apply` regenerates derived placements and logs their
  updates. See [ADR 0005](adr/0005-derived-placements.md).
- A derived placement is an output. `params()` still reports its `x`, `y`, `z` and
  `scale`, because they are the node's data and the geometry hash is built from
  them, but `set_param` refuses: regeneration would rewrite the edit at the next
  change to the face. Move such a feature by composing a placement *beneath* it,
  in the feature's own frame, which is what `AppState::move_selection` does; clear
  `on` to fix it in place, which is what `AppState::detach_selection` does.
- `Node` equality compares a mesh by `asset` and `Arc::ptr_eq`, never elementwise:
  a grid is megabytes and `Effect` values are compared routinely. A `Transform`
  compares its `on` too, because two placements in the same spot derived from
  different faces part company the moment either face moves.
- `Node::Prism` is unbounded along Z and is only meaningful as the tool of a
  `Difference` or `Intersection`, both of which take their bounds from the other
  operand. It is how a through cut is expressed: an end condition rather than a
  measurement, so growing the part cannot turn a through hole into a blind
  recess. `Node::Plane` is unbounded on the same terms.
- A mesh is a voxel signed distance grid, not triangles. `Node::Mesh` carries an
  `Arc<Grid>` inline rather than an id to look up, because `eval` takes no asset
  context. Its `Deserialize` therefore yields an empty placeholder that
  `is_valid` rejects, and `Document::attach_assets` is what fills it in.
  See [ADR 0006](adr/0006-sidecar-assets.md).
- `Grid` carries a precondition its producer owns: the zero level set must sit at
  least `Grid::REQUIRED_CLEARANCE` voxels inside every face. `sample` relies on it
  to stay a lower bound on true distance outside the lattice, and sphere tracing
  relies on that in turn. `Grid::boundary_clearance` measures it.
- A shader consists of `Generated::source` **and** `Generated::params`. Upload both
  initially. `Renderer::update` returns whether shader source changed and forced
  a pipeline rebuild; ordinary parameter changes usually only upload values.
- `Result` means the owning crate's error alias. File operations use `FileError`;
  STL and PNG writes return `std::io::Result`. Optional queries use `Option`.

### Current implementation limits

The catalog is an API inventory, not a guarantee that every documented invariant
has been established. General min/max CSG fields are not exact Euclidean signed
distances everywhere. Offsets and bounds on composed fields need particular care.
`Aabb::finite_or` currently clamps finite coordinates as well as infinities, and
export uses it with 1000 mm. Do not treat it as a lossless bounds conversion.

`Topology::is_printable` checks boundary edges and winding inconsistencies;
it permits non-manifold edges. `is_manifold` checks only the non-manifold edge
count. Neither predicate proves a nonempty, valid printable solid on its own.
Inspect triangle count and the report, and read [meshing.md](meshing.md).

`Node::Mesh` has no GPU path yet. `wgsl::generate` emits an empty field for one
and records it in `Generated::unsupported`; `try_generate` is the same call as a
`Result` for callers that can refuse. Check `Generated::is_complete` before
trusting a shader to show the whole model.

`Grid::sample` is the only field in the kernel that is an approximation rather
than a distance. Trilinear interpolation of exact samples is not itself exact,
and it is not 1-Lipschitz: the gradient reaches `sqrt(3)` at a kink, such as the
medial axis inside a solid. Sphere tracing can therefore overstep by up to 73
percent of a step within one voxel of such a kink. `Node::Mesh` is excluded from
the kernel's strict Lipschitz property for the same reason a smooth blend is, and
the violation is pinned rather than hidden. Outside the lattice the reading is a
lower bound and safe to trace against.

Native loading currently checks the root and labels but does not fully validate
the deserialized arena. Saving includes tombstoned labels that loading rejects.
The current effect log also cannot reproduce the allocator high-water mark after
all newly allocated nodes have been undone. Do not use replay as a complete
session snapshot or treat native loading as an untrusted-input validator.

## Examples

### Command vocabulary

| Command | Effect on the document |
| --- | --- |
| `Add { node }` | Allocate a node and return its ID. Does not automatically set the root or union it into the model. |
| `Replace { id, node }` | Replace a live node while preserving its ID; validate parameters and references. |
| `SetParam { id, name, value }` | Replace one scalar parameter on a live node; reject unknown keys. |
| `Delete { id }` | Delete an unreferenced node; refuse the current root and nodes still referenced by geometry or derivation. |
| `SetRoot { root }` | Choose the rendered/exported node; `None` clears the root without deleting nodes. |
| `SetName { id, name }` | Set a label; `None` clears it. |

`Effect` is the resolved history record: `Create` names the allocated ID,
`Replace` stores a replacement node, `Destroy` names a deletion, and `SetRoot`
and `SetName` record the corresponding changes. Consumers can inspect
`Document::log`; they should submit `Command` values for new edits.

`GeomError` distinguishes unknown IDs, deleted IDs, invalid parameters, cycles,
remaining references and occupied slots. `DocError` wraps geometry errors and
adds unknown parameter names and attempts to delete the root. `FileError`
distinguishes I/O, JSON parsing, unsupported versions and inconsistent documents.

### Create, edit, group undo, and save a pad

```rust
use sc_doc::{Command, Document};
use sc_geom::{Node, Profile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut doc = Document::new();
    doc.begin_step();
    let pad = sc_doc::add(&mut doc, Node::Extrude {
        profile: Profile::Rect { width: 60.0, height: 40.0 },
        depth: 5.0,
    })?;
    doc.apply(Command::SetRoot { root: Some(pad) })?;
    doc.end_step();

    doc.apply(Command::SetParam {
        id: pad,
        name: "width".into(),
        value: 80.0,
    })?;
    let edited = doc.hash();
    assert!(doc.undo()?);
    assert!(doc.redo()?);
    assert_eq!(doc.hash(), edited);

    sc_doc::file::save(&doc, std::path::Path::new("pad.shapecad"))?;
    Ok(())
}
```

The example exits on error. A long-lived UI must also close an open undo group
on the error path. File loading starts a new session without undo history.

### Mesh an existing document

```rust
fn export(doc: &sc_doc::Document, path: &std::path::Path)
    -> Result<(), Box<dyn std::error::Error>>
{
    let root = doc.root().ok_or("document has no root")?;
    let bounds = doc.bounds().ok_or("document has no bounds")?;
    if !bounds.is_finite() || bounds.is_empty() {
        return Err("export requires finite, nonempty bounds".into());
    }
    let mesh = sc_mesh::contour(doc.arena(), root, sc_mesh::Settings {
        resolution: 128,
        refinement: 2,
    });
    let topology = mesh.topology();
    if mesh.triangle_count() == 0
        || !topology.is_printable()
        || !topology.is_manifold()
    {
        return Err("mesh failed export checks".into());
    }
    sc_mesh::stl::write(&mesh, path)?;
    Ok(())
}
```

Use release builds for meshing. Resolution is cells along the longest axis,
not millimetres per cell. Grid memory grows roughly with the cube of resolution.
STL carries no unit metadata; ShapeCAD coordinates are in millimetres.

### Capture a document without a window

```rust
fn capture(doc: &sc_doc::Document, path: &std::path::Path)
    -> Result<(), Box<dyn std::error::Error>>
{
    let bounds = doc.bounds().ok_or("document has no root")?;
    let camera = sc_render::OrbitCamera::framing(bounds);
    let image = sc_render::snapshot::try_render(
        &doc.wgsl(), &camera, 800, 600,
        sc_render::gpu::Preference::Software,
    ).ok_or("no usable adapter")?;
    sc_render::snapshot::write_png(&image, path)?;
    Ok(())
}
```

`try_render` returns `None` when no adapter is available, but device creation or
GPU validation can still panic. If a renderer already owns a device, use
`render_with` or `capture` rather than creating another GPU instance. See
[building.md](building.md) for WSL driver and main-thread requirements.

## Editable parameter names

Use `node.params()` to discover supported scalar fields. Apply changes through
`Command::SetParam`; `Node::set_param` only changes a detached node value.
Validation happens when the document or arena accepts that node.

| Node/profile | Keys | Meaning |
| --- | --- | --- |
| Sphere | `radius` | Radius |
| Box | `half_x`, `half_y`, `half_z`, `round` | Half-extents of the outer box; rounding |
| Cylinder | `radius`, `half_height`, `round` | Radius, half-height, rounding |
| Torus | `major`, `minor` | Ring radius and tube radius |
| Plane | `normal_x`, `normal_y`, `normal_z`, `offset` | Normal components and signed plane offset |
| Union, Difference, Intersection | `smooth` | Blend parameter; zero is hard CSG |
| Transform | `x`, `y`, `z`, `scale` | Translation and uniform scale |
| Transform with `on` set | Reported but not settable | Position is regenerated from the attached face; `set_param` returns `false` |
| Mesh | None | The grid is fixed at import; resolution is read-only via `Node::mesh_resolution` |
| Prism | Profile keys only | No depth: an unbounded sweep has none. This is how "through all" is expressed |
| Pattern, linear | `count`, `step_x`, `step_y`, `step_z` | Copies and the vector between neighbours |
| Pattern, circular | `count`, `sweep` | Copies and the total swept angle, in degrees |
| Offset | `distance` | Positive grows; negative shrinks |
| Shell | `thickness` | Inward wall thickness |
| Extrude | Profile keys plus `depth` | Extrusion depth |
| Rect profile | `width`, `height` | Full dimensions |
| Circle profile | `radius` | Radius |
| RegularPolygon profile | `sides`, `radius` | Side count and circumradius, not across-flats size |
| Path profile | None | Points require replacement of the containing node |

`Pattern::count` edits round and clamp to 2..=`node::MAX_INSTANCES` (200); the
ceiling exists because every instance costs a loop iteration in the shader.
`RegularPolygon::sides` edits round and clamp to 3..64. Paths allow 3..256 finite
points with nonzero signed area. Rotation, child links and profile point arrays
are not scalar parameters; use `Command::Replace` while preserving the node ID.

## Library API catalog

Each module links to its source. Types below show public fields or enum variants;
method rows show signatures without bodies. `Self` refers to the type in the
method column. Imported short names use the imports in the linked module.
Summaries condense rustdoc; consult the contracts above for known limitations.

### sc-doc

#### sc_doc::command

Source: [crates/sc-doc/src/command.rs](../crates/sc-doc/src/command.rs).

```rust,ignore
pub enum Command {
    Add {
        node: Node,
    },
    Replace {
        id: NodeId,
        node: Node,
    },
    SetParam {
        id: NodeId,
        name: String,
        value: f32,
    },
    Delete {
        id: NodeId,
    },
    SetRoot {
        root: Option<NodeId>,
    },
    SetName {
        id: NodeId,
        name: Option<String>,
    },
}
```

```rust,ignore
pub enum Effect {
    Create {
        id: NodeId,
        node: Node,
    },
    Replace {
        id: NodeId,
        node: Node,
    },
    Destroy {
        id: NodeId,
    },
    SetRoot {
        root: Option<NodeId>,
    },
    SetName {
        id: NodeId,
        name: Option<String>,
    },
}
```

#### sc_doc::asset

Source: [crates/sc-doc/src/asset.rs](../crates/sc-doc/src/asset.rs).

Grids are megabytes of binary, so they live in a sidecar directory beside the
JSON rather than in it. See [ADR 0006](adr/0006-sidecar-assets.md).

```
part.shapecad     the document, JSON as before
part.assets/      one 00000000.scsdf per grid, named after its AssetId
```

The file is `SCSDF\0\0\0`, then little-endian `u32` version, `[u32; 3]` dims,
`[f32; 3]` origin, `f32` spacing, then `dims.product()` samples. 40-byte header.
Decoding checks magic, version and that the declared dims match the byte length
**before allocating**, so a corrupt header cannot drive a huge allocation.

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `encode_grid` | `pub fn encode_grid(grid: &Grid) -> Vec<u8>` | Serializes one grid. |
| `decode_grid` | `pub fn decode_grid(bytes: &[u8]) -> Result<Grid, GridError>` | `GridError::{Header, BadMagic, UnsupportedVersion, SizeMismatch, TooLarge}`. |
| `grid_path` | `pub fn grid_path(dir: &Path, id: AssetId) -> PathBuf` | Zero-padded so a listing sorts in id order. |
| `AssetStore::insert` | `pub fn insert(&mut self, grid: Grid) -> AssetId` | Allocates monotonically. Ids are never reissued. |
| `AssetStore::get` | `pub fn get(&self, id: AssetId) -> Option<&Arc<Grid>>` | |
| `AssetStore::write_dir` | `pub fn write_dir(&self, dir: &Path, referenced: &BTreeSet<AssetId>) -> Result<(), AssetError>` | Writes only referenced grids. |
| `AssetStore::prune_dir` | `pub fn prune_dir(dir: &Path, referenced: &BTreeSet<AssetId>) -> Result<(), AssetError>` | Called after the JSON is written, never before, so a save that fails part way cannot leave the previous document referring to a grid that has just been deleted. |
| `AssetStore::read_dir` | `pub fn read_dir(dir: &Path, required: &BTreeSet<AssetId>) -> Result<Self, AssetError>` | Fails if a required grid is missing. |

#### sc_doc::error

Source: [crates/sc-doc/src/error.rs](../crates/sc-doc/src/error.rs).

```rust,ignore
pub enum DocError {
    Geom(GeomError),
    UnknownParam {
        id: NodeId,
        name: String,
    },
    IsRoot(NodeId),
}
```

```rust,ignore
pub type Result<T> = std::result::Result<T, DocError>;
```

#### sc_doc::file

Source: [crates/sc-doc/src/file.rs](../crates/sc-doc/src/file.rs).

```rust,ignore
pub struct DocumentFile {
    pub format: u32,
    pub generator: String,
    pub units: String,
    pub arena: Arena,
    pub root: Option<NodeId>,
    pub names: Vec<(NodeId, String)>,
}
```

```rust,ignore
pub enum FileError {
    Io(std::io::Error),
    Parse(serde_json::Error),
    UnsupportedVersion(u32),
    Invalid(DocError),
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| Constant | `pub const FORMAT_VERSION: u32 = 4;` | Bumped whenever the on-disk shape changes incompatibly. Version 2 made extrusion profiles parametric; version 3 added mesh nodes and the sidecar asset directory; version 4 added `Node::Prism`. Older files still load: they contain neither node kind, so there is no sidecar to find and its absence is not an error. |
| `sidecar_dir` | `pub fn sidecar_dir(path: &Path) -> PathBuf` | The `.assets` directory beside a document. |
| Constant | `pub const EXTENSION: &str = "shapecad";` | Conventional file extension, without the dot. |
| `Document::snapshot` | `pub fn snapshot(&self) -> DocumentFile` | Captures the document as its serialisable form. |
| `Document::from_snapshot` | `pub fn from_snapshot(file: DocumentFile) -> Result<Self, DocError>` | Rebuilds a document from a snapshot. Errors: `DocError::Geom` if the root or any label refers to a node that is not live, which would mean the file is internally inconsistent. |
| `save` | `pub fn save(doc: &Document, path: &Path) -> Result<(), FileError>` | Writes a document to `path` as pretty-printed JSON. Errors: `FileError::Io` or `FileError::Parse` on failure. |
| `open` | `pub fn open(path: &Path) -> Result<Document, FileError>` | Reads a document from `path`. Errors: `FileError` for any I/O, parse, version or consistency problem. |

#### sc_doc::crate root

Source: [crates/sc-doc/src/lib.rs](../crates/sc-doc/src/lib.rs).

```rust,ignore
pub mod command;
pub mod error;
pub mod file;
pub mod samples;
pub use command::{Command, Effect};
pub use error::{DocError, Result};
```

```rust,ignore
pub struct Document {
    // Fields are private.
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Document::new` | `pub fn new() -> Self` | An empty document with no root. |
| `Document::arena` | `pub fn arena(&self) -> &Arena` | Read-only access to the geometry storage. |
| `Document::root` | `pub fn root(&self) -> Option<NodeId>` | The node the document currently renders and exports. |
| `Document::name` | `pub fn name(&self, id: NodeId) -> Option<&str>` | The label attached to a node, if any. |
| `Document::bounds` | `pub fn bounds(&self) -> Option<Aabb>` | Conservative bounds of the rooted model. |
| `Document::hash` | `pub fn hash(&self) -> Option<GeometryHash>` | Deterministic digest of the rooted model, for regression testing. |
| `Document::wgsl` | `pub fn wgsl(&self) -> wgsl::Generated` | The generated shader source for the current model. |
| `Document::log` | `pub fn log(&self) -> impl Iterator<Item = &Effect>` | The effects that produced the current state, oldest first. Excludes anything sitting in the redo tail. |
| `Document::log_len` | `pub fn log_len(&self) -> usize` | Number of applied mutations. |
| `Document::replay` | `pub fn replay(ops: impl IntoIterator<Item = Effect>) -> Result<Self>` | Replay resolved effects into a new document. Returns document errors for rejected effects; see replay limitations above. Undo grouping is not preserved by the plain effect iterator. |
| `Document::can_undo` | `pub fn can_undo(&self) -> bool` | Whether there is an applied command to undo. |
| `Document::can_redo` | `pub fn can_redo(&self) -> bool` | Whether there is an undone command to reapply. |
| `Document::apply` | `pub fn apply(&mut self, command: Command) -> Result<Option<NodeId>>` | Apply one command, regenerate derived placements and log effects in one undo step. Returns the created/touched ID, or None when clearing the root. Returns DocError on rejection. |
| `Document::begin_step` | `pub fn begin_step(&mut self)` | Starts grouping: every command applied until the matching `Document::end_step` undoes and redoes as one. |
| `Document::end_step` | `pub fn end_step(&mut self)` | Closes the step opened by `Document::begin_step`. |
| `Document::undo` | `pub fn undo(&mut self) -> Result<bool>` | Reverses the most recent command. Returns false if there is nothing to undo. Errors: `DocError::Geom` only if the arena rejects the inverse edit, which would indicate a bug in inverse construction rather than user error. |
| `Document::redo` | `pub fn redo(&mut self) -> Result<bool>` | Reapplies the most recently undone command. Returns false if there is nothing to redo. Errors: `DocError::Geom` only if the arena rejects the replayed edit. |
| `Document::outline` | `pub fn outline(&self) -> String` | A compact, human- and model-readable rendering of the tree. |
| `add` | `pub fn add(doc: &mut Document, node: Node) -> Result<NodeId>` | Convenience: add a node and return its id. Errors: Propagates any `DocError` from `Document::apply`. Panics: If `Command::Add` ever returns without an id, which would be a bug in `Document::apply` rather than a condition a caller can trigger. |

#### sc_doc::mesh and asset methods on Document

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Document::assets` | `pub fn assets(&self) -> &AssetStore` | |
| `Document::import_grid` | `pub fn import_grid(&mut self, grid: Grid) -> AssetId` | Stores a grid and returns its id. Deliberately **not** routed through `apply`: it touches no node and is not a mutation. The `Command::Add` that follows is. |
| `Document::add_mesh` | `pub fn add_mesh(&mut self, grid: Grid) -> Result<NodeId>` | Imports and adds a `Node::Mesh` in one step. |
| `Document::referenced_assets` | `pub fn referenced_assets(&self) -> BTreeSet<AssetId>` | Ids named by live nodes. What gets written on save. |
| `Document::attach_assets` | `pub fn attach_assets(&mut self, store: AssetStore) -> Result<(), AttachError>` | Fills in every placeholder grid. `AttachError::{Unresolved, Invalid}`. Construction rather than an edit, so it is not logged: an opened file has no undo history. |

Two reachability rules, because disk and memory are reachable from different
places. **Disk:** a grid is written if a live node references it. **Memory:** live
nodes plus every node embedded in the log, across the whole of it and not just
the applied prefix, because a `Destroy` entry's inverse carries the whole node and
undo has to be able to put the geometry back. Collection therefore runs at exactly
one moment, when a fresh edit discards the redo tail.

Saving to a new path always writes a full copy of the sidecar from memory. A
document referring back to grids somewhere else breaks the moment either copy
moves. The cost is that a save-as rewrites every grid.

#### sc_doc::samples

Source: [crates/sc-doc/src/samples.rs](../crates/sc-doc/src/samples.rs).

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `bracket` | `pub fn bracket() -> Document` | A small printable bracket, built entirely through the command log. Panics: If any command is rejected, which would mean the kernel's validation rules have changed underneath this fixture. |

### sc-geom

#### sc_geom::arena

Source: [crates/sc-geom/src/arena.rs](../crates/sc-geom/src/arena.rs).

```rust,ignore
pub struct Arena {
    // Fields are private.
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Arena::new` | `pub fn new() -> Self` | An empty arena. |
| `Arena::len` | `pub fn len(&self) -> usize` | Number of live nodes. |
| `Arena::is_empty` | `pub fn is_empty(&self) -> bool` | Whether there are no live nodes. |
| `Arena::capacity` | `pub fn capacity(&self) -> usize` | Total ids ever issued, live or dead. |
| `Arena::get` | `pub fn get(&self, id: NodeId) -> Option<&Node>` | The node at `id`, or `None` if it is unknown or deleted. |
| `Arena::try_get` | `pub fn try_get(&self, id: NodeId) -> Result<&Node>` | Like `Arena::get` but distinguishes "never existed" from "deleted". Errors: `GeomError::UnknownNode` if the id was never issued, `GeomError::DeadNode` if it has been deleted. |
| `Arena::is_alive` | `pub fn is_alive(&self, id: NodeId) -> bool` | Whether `id` refers to a live node. |
| `Arena::live_ids` | `pub fn live_ids(&self) -> impl Iterator<Item = NodeId> + '_` | Every live id, in ascending order. |
| `Arena::next_id` | `pub fn next_id(&self) -> NodeId` | The id `Arena::insert` would hand out next. |
| `Arena::create_at` | `pub fn create_at(&mut self, id: NodeId, node: Node) -> Result<()>` | Recreate a node at a specific id. Errors: `GeomError::SlotOccupied` if the id is currently live, `GeomError::InvalidNode` if parameters are out of range, `GeomError::UnknownNode` / `GeomError::DeadNode` if a child or the node it is derived from is gone, or `GeomError::Cycle` for a derivation that points into the node's own subtree. |
| `Arena::insert` | `pub fn insert(&mut self, node: Node) -> Result<NodeId>` | Adds a node and returns its freshly issued id. Errors: `GeomError::InvalidNode` if parameters are out of range, `GeomError::UnknownNode` / `GeomError::DeadNode` if a child or the node it is derived from is gone, or `GeomError::Cycle` for a derivation that points into the node's own subtree. |
| `Arena::replace` | `pub fn replace(&mut self, id: NodeId, node: Node) -> Result<Node>` | Overwrite a node in place, returning the previous value. Errors: `GeomError::Cycle` if the new children would close a loop, or if the new derivation points inside this node's own subtree, plus the same validation errors as `Arena::insert`. Panics: Never in practice; the liveness of the slot is checked first. |
| `Arena::remove` | `pub fn remove(&mut self, id: NodeId) -> Result<Node>` | Deletes a node. Errors: `GeomError::StillReferenced` if any live node points at it, as a child or as the feature its placement is derived from, so the DAG can never contain a dangling pointer of either kind. Panics: Never in practice; the liveness of the slot is checked first. |
| `Arena::parents_of` | `pub fn parents_of(&self, id: NodeId) -> Vec<NodeId>` | Every live node that names `id` as a direct child. |
| `Arena::reaches` | `pub fn reaches(&self, from: NodeId, target: NodeId) -> bool` | Whether `target` is reachable by walking children from `from`. |
| `Arena::reachable` | `pub fn reachable(&self, root: NodeId) -> HashSet<NodeId>` | Every node reachable from `root`, including `root` itself. |

#### sc_geom::bounds

Source: [crates/sc-geom/src/bounds.rs](../crates/sc-geom/src/bounds.rs).

```rust,ignore
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| Constant | `pub const EMPTY: Self = Self { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY), };` | The empty box, which absorbs into any union without affecting it. |
| Constant | `pub const INFINITE: Self = Self { min: Vec3::splat(f32::NEG_INFINITY), max: Vec3::splat(f32::INFINITY), };` | The box covering all of space. |
| `Aabb::from_half` | `pub fn from_half(h: Vec3) -> Self` | A box centred on the origin with the given half-extents. |
| `Aabb::is_empty` | `pub fn is_empty(&self) -> bool` | Whether the box contains no points. |
| `Aabb::is_finite` | `pub fn is_finite(&self) -> bool` | Whether both corners are finite. |
| `Aabb::center` | `pub fn center(&self) -> Vec3` | Midpoint of the box. |
| `Aabb::size` | `pub fn size(&self) -> Vec3` | Extent along each axis, clamped at zero for an empty box. |
| `Aabb::union` | `pub fn union(self, o: Self) -> Self` | The smallest box containing both inputs. |
| `Aabb::intersection` | `pub fn intersection(self, o: Self) -> Self` | The overlap of two boxes, possibly empty. |
| `Aabb::expand` | `pub fn expand(self, d: f32) -> Self` | Grows the box by `d` on every side. An empty box stays empty. |
| `Aabb::transformed` | `pub fn transformed(self, t: &Transform) -> Self` | The axis-aligned box enclosing this box after `t` is applied, computed from its eight corners. |
| `Aabb::finite_or` | `pub fn finite_or(self, fallback_half: f32) -> Self` | Clamp bounds to the origin-centered fallback box; empty bounds become that box. Currently also clips finite bounds. |
| `bounds` | `pub fn bounds(arena: &Arena, id: NodeId) -> Aabb` | Conservative bounds of the solid rooted at `id`. |

#### sc_geom::error

Source: [crates/sc-geom/src/error.rs](../crates/sc-geom/src/error.rs).

```rust,ignore
pub enum GeomError {
    UnknownNode(NodeId),
    DeadNode(NodeId),
    InvalidNode {
        kind: &'static str,
        reason: String,
    },
    Cycle {
        at: NodeId,
        via: NodeId,
    },
    StillReferenced {
        node: NodeId,
        by: NodeId,
    },
    SlotOccupied(NodeId),
}
```

```rust,ignore
pub type Result<T> = std::result::Result<T, GeomError>;
```

#### sc_geom::eval

Source: [crates/sc-geom/src/eval.rs](../crates/sc-geom/src/eval.rs).

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `smin` | `pub fn smin(a: f32, b: f32, k: f32) -> f32` | Polynomial smooth minimum. `k` is a blend radius in model units. |
| `smax` | `pub fn smax(a: f32, b: f32, k: f32) -> f32` | Polynomial smooth maximum, the dual of `smin`. |
| `eval` | `pub fn eval(arena: &Arena, id: NodeId, p: Vec3) -> f32` | Evaluate the field at p; negative is inside, positive outside. Missing nodes return infinity. General CSG results are not exact Euclidean distances everywhere. |
| `sd_polygon` | `pub fn sd_polygon(p: Vec2, verts: &[Vec2]) -> f32` | Exact signed distance from a point to a closed polygon, negative inside. |
| `normal` | `pub fn normal(arena: &Arena, id: NodeId, p: Vec3, eps: f32) -> Vec3` | Surface normal by the tetrahedron finite-difference trick: four evaluations instead of six, and no axis bias. |

#### sc_geom::hash

Source: [crates/sc-geom/src/hash.rs](../crates/sc-geom/src/hash.rs).

```rust,ignore
pub struct StableHasher(u64);
```

```rust,ignore
pub struct GeometryHash {
    pub structure: u64,
    pub field: u64,
    pub bounds: u64,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| Constant | `pub const QUANTUM: f32 = 1e-4;` | Quantum for parameter and sample quantization: 0.1 micrometres. Far below any printer's resolution, far above f32 evaluation noise. |
| `StableHasher::new` | `pub fn new() -> Self` | A hasher primed with the FNV-1a offset basis. |
| `StableHasher::write_bytes` | `pub fn write_bytes(&mut self, bytes: &[u8])` | Folds raw bytes into the digest. |
| `StableHasher::write_u64` | `pub fn write_u64(&mut self, v: u64)` | Folds an unsigned integer in little-endian order. |
| `StableHasher::write_i64` | `pub fn write_i64(&mut self, v: i64)` | Folds a signed integer in little-endian order. |
| `StableHasher::write_str` | `pub fn write_str(&mut self, s: &str)` | Folds a string, terminated so that concatenations cannot collide. |
| `StableHasher::write_f32` | `pub fn write_f32(&mut self, v: f32)` | Folds a float after quantising it via `quantize`. |
| `StableHasher::finish` | `pub fn finish(&self) -> u64` | The digest so far. |
| `quantize` | `pub fn quantize(v: f32) -> i64` | Snap a float to a lattice so that harmless last-bit differences between two evaluation backends do not change the hash. Non-finite values map to distinct sentinels rather than collapsing together. |
| `GeometryHash::combined` | `pub fn combined(&self) -> u64` | A single digest folding all three components together. |
| `GeometryHash::short` | `pub fn short(&self) -> String` | The combined digest as 16 hex characters. |
| Constant | `pub const DEFAULT_GRID: u32 = 24;` | Default sampling resolution per axis. 24^3 is 13,824 evaluations, cheap enough to run on every test, dense enough to catch a sub-millimetre edit. |
| `geometry_hash` | `pub fn geometry_hash(arena: &Arena, root: NodeId) -> GeometryHash` | All three digests of the model rooted at `root`, at the default resolution. |
| `geometry_hash_with` | `pub fn geometry_hash_with(arena: &Arena, root: NodeId, grid: u32) -> GeometryHash` | As `geometry_hash`, with an explicit sampling resolution. |
| `structure_hash` | `pub fn structure_hash(arena: &Arena, root: NodeId) -> u64` | Hashes the DAG by structure, not by id, so compaction and renumbering do not perturb it. |
| `bounds_hash` | `pub fn bounds_hash(arena: &Arena, root: NodeId) -> u64` | Digest of the model's quantised bounding box. |
| `field_hash` | `pub fn field_hash(arena: &Arena, root: NodeId, grid: u32) -> u64` | Samples the field on a regular grid spanning the model's bounds, inflated so the grid straddles the surface rather than sitting exactly on it. |

#### sc_geom::crate root

Source: [crates/sc-geom/src/lib.rs](../crates/sc-geom/src/lib.rs).

```rust,ignore
pub mod arena;
pub mod bounds;
pub mod error;
pub mod eval;
pub mod hash;
pub mod math;
pub mod node;
pub mod ops;
pub mod pick;
pub mod profile;
pub mod wgsl;
pub use arena::Arena;
pub use bounds::{bounds, Aabb};
pub use error::{GeomError, Result};
pub use eval::{eval, normal, smax, smin};
pub use hash::{geometry_hash, GeometryHash};
pub use math::Transform;
pub use node::{Node, NodeId};
pub use ops::Builder;
pub use pick::{pick, Hit};
pub use profile::Profile;
pub use glam;
```

#### sc_geom::math

Source: [crates/sc-geom/src/math.rs](../crates/sc-geom/src/math.rs).

```rust,ignore
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: f32,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| Constant | `pub const IDENTITY: Self = Self { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: 1.0, };` | The transform that changes nothing. |
| `Transform::from_translation` | `pub fn from_translation(translation: Vec3) -> Self` | A pure translation. |
| `Transform::from_rotation` | `pub fn from_rotation(rotation: Quat) -> Self` | A pure rotation about the origin. |
| `Transform::from_scale` | `pub fn from_scale(scale: f32) -> Self` | A pure uniform scale about the origin. |
| `Transform::is_valid` | `pub fn is_valid(&self) -> bool` | Whether this transform can be applied to a distance field without corrupting it. |
| `Transform::inverse_point` | `pub fn inverse_point(&self, p: Vec3) -> Vec3` | Maps a parent-space point into the child's local frame. |
| `Transform::apply_point` | `pub fn apply_point(&self, p: Vec3) -> Vec3` | Maps a child-space point out into parent space. |
| `Transform::then` | `pub fn then(&self, outer: &Self) -> Self` | Composes two transforms: this one applied first, then `outer`. |
| `Transform::inverse` | `pub fn inverse(&self) -> Self` | The transform that undoes this one. |
| `Transform::apply_distance` | `pub fn apply_distance(&self, d: f32) -> f32` | Converts a distance measured in the child's frame into parent units. |

#### sc_geom::node

Source: [crates/sc-geom/src/node.rs](../crates/sc-geom/src/node.rs).

```rust,ignore
pub struct NodeId(pub u32);
```

```rust,ignore
pub enum Node {
    Sphere {
        radius: f32,
    },
    Box {
        half: Vec3,
        round: f32,
    },
    Cylinder {
        radius: f32,
        half_height: f32,
        round: f32,
    },
    Torus {
        major: f32,
        minor: f32,
    },
    Plane {
        normal: Vec3,
        offset: f32,
    },
    Union {
        a: NodeId,
        b: NodeId,
        smooth: f32,
    },
    Difference {
        a: NodeId,
        b: NodeId,
        smooth: f32,
    },
    Intersection {
        a: NodeId,
        b: NodeId,
        smooth: f32,
    },
    Transform {
        child: NodeId,
        xform: Transform,
        on: Option<NodeId>,
    },
    Offset {
        child: NodeId,
        distance: f32,
    },
    Extrude {
        profile: Profile,
        depth: f32,
    },
    Shell {
        child: NodeId,
        thickness: f32,
    },
    Mesh {
        asset: AssetId,      // which grid, as stored beside the document
        grid: Arc<Grid>,     // the grid itself; `serde(skip)`, filled in on load
    },
    Prism {
        profile: Profile,    // swept without end along Z
    },
    Pattern {
        child: NodeId,
        kind: Repeat,        // how one instance is placed relative to the last
        count: u32,          // total instances, including the original
    },
}

pub const MAX_INSTANCES: u32 = 200;

pub enum Repeat {
    Linear { step: Vec3 },   // the vector between neighbouring instances
    Circular { sweep: f32 }, // total swept angle in radians, about Z
}
```

A pattern is a union of `count` placements of one child, evaluated as a minimum
over instances rather than as `count` nodes in the arena. The count stays a
single editable number, and the child stays a single node: editing the child
edits every copy. Instance zero is always the identity, so adding a pattern
never moves what was already there.

`Repeat::Circular` divides a full turn by `count` and a partial sweep by
`count - 1`, which is what makes a 360 degree pattern of 6 place bosses every 60
degrees while a 90 degree pattern of 3 places them at 0, 45 and 90.

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Repeat::spans` | `pub fn spans(self, count: u32) -> u32` | How many gaps a circular sweep is divided into: `count` for a full turn, `count - 1` otherwise. A linear step is already a per-instance offset, so it does not divide. |
| `Repeat::placement` | `pub fn placement(self, i: u32, count: u32) -> Transform` | Where instance `i` sits. Identity at `i == 0`. |

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `polygon_area` | `pub fn polygon_area(points: &[Vec2]) -> f32` | Twice the signed area of a polygon, by the shoelace formula. |
| `Node::kind` | `pub fn kind(&self) -> &'static str` | Stable machine-readable tag. Part of the agent-facing vocabulary, so treat these strings as API. |
| `Node::derived_from` | `pub fn derived_from(&self) -> Option<NodeId>` | The node this one's placement was derived from, if any. Separate from `children()` on purpose: a reference to protect, not an operand to evaluate. |
| `Node::mesh` | `pub fn mesh(asset: AssetId, grid: Arc<Grid>) -> Node` | Constructs a mesh node. |
| `Node::mesh_grid` | `pub fn mesh_grid(&self) -> Option<&Arc<Grid>>` | |
| `Node::mesh_resolution` | `pub fn mesh_resolution(&self) -> Option<[u32; 3]>` | Read-only. The grid is fixed at import, so resolution is not a parameter; offering one would be a control that silently does nothing. |
| `Node::children` | `pub fn children(&self) -> impl Iterator<Item = NodeId> + '_` | The node's direct child references, in declaration order. |
| `Node::map_children` | `pub fn map_children(&mut self, mut f: impl FnMut(NodeId) -> NodeId)` | Rewrite child references in place. Used by cycle-safe edits and by compaction, which renumbers ids. |
| `Node::params` | `pub fn params(&self) -> Vec<(&'static str, f32)>` | Named scalar parameters, in a stable order. |
| `Node::set_param` | `pub fn set_param(&mut self, name: &str, v: f32) -> bool` | Set a named parameter. Returns false if the name is not valid for this node kind, leaving the node untouched. |
| `Node::is_valid` | `pub fn is_valid(&self) -> bool` | Structural and numeric sanity. Does not check child ids; the arena does that, because only the arena knows which ids are live. |

#### sc_geom::ops

Source: [crates/sc-geom/src/ops.rs](../crates/sc-geom/src/ops.rs).

```rust,ignore
pub struct Builder {
    pub arena: Arena,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Builder::new` | `pub fn new() -> Self` | An empty builder. |
| `Builder::into_arena` | `pub fn into_arena(self) -> Arena` | Consumes the builder, yielding the arena. |
| `Builder::sphere` | `pub fn sphere(&mut self, radius: f32) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::cuboid` | `pub fn cuboid(&mut self, half: Vec3) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::rounded_cuboid` | `pub fn rounded_cuboid(&mut self, half: Vec3, round: f32) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::cube` | `pub fn cube(&mut self, half: f32) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::cylinder` | `pub fn cylinder(&mut self, radius: f32, half_height: f32) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::torus` | `pub fn torus(&mut self, major: f32, minor: f32) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::plane` | `pub fn plane(&mut self, normal: Vec3, offset: f32) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::union` | `pub fn union(&mut self, a: NodeId, b: NodeId) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::smooth_union` | `pub fn smooth_union(&mut self, a: NodeId, b: NodeId, smooth: f32) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::difference` | `pub fn difference(&mut self, a: NodeId, b: NodeId) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::smooth_difference` | `pub fn smooth_difference(&mut self, a: NodeId, b: NodeId, smooth: f32) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::intersection` | `pub fn intersection(&mut self, a: NodeId, b: NodeId) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::transform` | `pub fn transform(&mut self, child: NodeId, xform: Transform) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::translate` | `pub fn translate(&mut self, child: NodeId, v: Vec3) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::rotate` | `pub fn rotate(&mut self, child: NodeId, q: Quat) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::offset` | `pub fn offset(&mut self, child: NodeId, distance: f32) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::extrude` | `pub fn extrude(&mut self, profile: crate::Profile, depth: f32) -> Result<NodeId>` | A closed profile swept along +Z from z = 0. Errors: Propagates validation failures from `Arena::insert`. |
| `Builder::shell` | `pub fn shell(&mut self, child: NodeId, thickness: f32) -> Result<NodeId>` |  Errors: Propagates validation failures from `Arena::insert`. |

#### sc_geom::pick

Source: [crates/sc-geom/src/pick.rs](../crates/sc-geom/src/pick.rs).

```rust,ignore
pub enum Hit {
    Face(NodeId),
    Seam {
        boolean: NodeId,
        between: (NodeId, NodeId),
    },
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Hit::node` | `pub fn node(self) -> NodeId` | The node a selection should land on. |
| `pick` | `pub fn pick(arena: &Arena, root: NodeId, world: Vec3, tolerance: f32) -> Option<Hit>` | Finds what lies under `world`, within `tolerance`. |
| `placement_of` | `pub fn placement_of(arena: &Arena, root: NodeId, target: NodeId) -> Option<crate::Transform>` | The transform carrying `target` from its own frame into the model. |
| `face_placement` | `pub fn face_placement(arena: &Arena, root: NodeId, target: NodeId) -> Option<crate::Transform>` | Where a sketch drawn on `target`'s far face sits in the model. |

#### sc_geom::profile

Source: [crates/sc-geom/src/profile.rs](../crates/sc-geom/src/profile.rs).

```rust,ignore
pub enum Profile {
    Rect {
        width: f32,
        height: f32,
    },
    Circle {
        radius: f32,
    },
    RegularPolygon {
        sides: u32,
        radius: f32,
    },
    Path {
        points: Vec<Vec2>,
    },
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| Constant | `pub const MAX_PATH_POINTS: usize = 256;` | Largest number of points a freehand path may have. |
| `Profile::kind` | `pub fn kind(&self) -> &'static str` | Machine-readable tag, part of the agent-facing vocabulary. |
| `Profile::distance` | `pub fn distance(&self, p: Vec2) -> f32` | Exact signed distance in the sketch plane, negative inside. |
| `Profile::polygon` | `pub fn polygon(&self) -> Vec<Vec2>` | The profile as a closed polygon. |
| `Profile::params` | `pub fn params(&self) -> Vec<(&'static str, f32)>` | Return editable scalar dimensions; Path returns no scalar parameters. |
| `Profile::set_param` | `pub fn set_param(&mut self, name: &str, v: f32) -> bool` | Sets a named dimension. False if the name does not apply. |
| `Profile::is_valid` | `pub fn is_valid(&self) -> bool` | Whether this profile encloses an area the kernel can work with. |
| `Profile::bounds` | `pub fn bounds(&self) -> (Vec2, Vec2)` | The profile's extent in the sketch plane, as minimum and maximum. |

#### sc_geom::sdf

Source: [crates/sc-geom/src/sdf.rs](../crates/sc-geom/src/sdf.rs).

```rust,ignore
pub struct AssetId(pub u32);

pub struct Grid {
    pub dims: [u32; 3],   // sample counts along x, y, z
    pub origin: Vec3,     // model-space position of sample (0, 0, 0)
    pub spacing: f32,     // uniform distance between adjacent samples
    pub data: Vec<f32>,   // x fastest, then y, then z
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Grid::REQUIRED_CLEARANCE` | `pub const REQUIRED_CLEARANCE: u32 = 2` | Voxels of padding the producer must leave between the zero level set and every face. A precondition, not something `Grid` enforces. |
| `Grid::from_fn` | `pub fn from_fn(dims: [u32; 3], origin: Vec3, spacing: f32, f: impl FnMut(Vec3) -> f32) -> Self` | Builds a grid by sampling a field analytically. Used by tests to get ground truth. |
| `Grid::sample` | `pub fn sample(&self, p: Vec3) -> f32` | Trilinear inside the lattice, which is an approximation carrying the discretization error. Outside, a lower bound on true distance, safe for sphere tracing. Returns `f32::INFINITY` for a placeholder or malformed grid rather than panicking. |
| `Grid::bounds` | `pub fn bounds(&self) -> Aabb` | Conservative bounds of the sampled region. |
| `Grid::digest` | `pub fn digest(&self) -> u64` | Deterministic across platforms and runs. Hashes raw sample bits with `-0.0` and NaN normalized first, rather than quantizing, so a single moved voxel is visible. |
| `Grid::is_valid` | `pub fn is_valid(&self) -> bool` | Dims at least 2 per axis, `data` length matching, usable spacing and origin. Does **not** scan samples for finiteness: that is O(n) on megabytes for a defect only the voxelizer can create, so it is a documented precondition instead. |
| `Grid::is_placeholder` | `pub fn is_placeholder(&self) -> bool` | True for the empty grid that `Deserialize` produces before assets are attached. |
| `Grid::boundary_clearance` | `pub fn boundary_clearance(&self) -> f32` | Smallest sample on the six faces. Compare against `REQUIRED_CLEARANCE * spacing` to check the padding precondition. |
| `Grid::voxel_count` | `pub fn voxel_count(&self) -> usize` | Product of `dims`. |

#### sc_geom::wgsl

Source: [crates/sc-geom/src/wgsl.rs](../crates/sc-geom/src/wgsl.rs).

```rust,ignore
pub struct Generated {
    pub source: String,
    pub params: Vec<f32>,
    pub unsupported: Vec<NodeId>,   // nodes the shader could not represent
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `generate` | `pub fn generate(arena: &Arena, root: Option<NodeId>) -> Generated` | Emits a module exposing `sc_sdf` and `sc_selected`, plus the values they read. Infallible by design: the viewport calls it on every structural edit and has nowhere to put a `Result`. A node it cannot emit becomes empty space and is listed in `unsupported`. |
| `try_generate` | `pub fn try_generate(arena: &Arena, root: Option<NodeId>) -> Result<Generated>` | The same call for callers that can refuse. `GeomError::NotInShader` naming the first unsupported node. |
| `Generated::is_complete` | `pub fn is_complete(&self) -> bool` | Whether the shader represents the whole model. False means part of it is silently missing from the viewport, which today means a `Node::Mesh`. |
| `generate_with_selection` | `pub fn generate_with_selection( arena: &Arena, root: Option<NodeId>, selected: Option<NodeId>, ) -> Generated` | Generate model and selected-subtree field functions with their parameter buffer. The selected subtree currently omits ancestor placement. |

### sc-mesh

#### sc_mesh::dual_contour

Source: [crates/sc-mesh/src/dual_contour.rs](../crates/sc-mesh/src/dual_contour.rs).

```rust,ignore
pub struct Settings {
    pub resolution: u32,
    pub refinement: u32,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `contour` | `pub fn contour(arena: &Arena, root: NodeId, settings: Settings) -> Mesh` | Meshes the solid rooted at `root`. Panics: If the model produces more than `u32::MAX` vertices, which at any sane resolution would exhaust memory long before it was reached. |

#### sc_mesh::crate root

Source: [crates/sc-mesh/src/lib.rs](../crates/sc-mesh/src/lib.rs).

```rust,ignore
pub mod bvh;
pub mod dual_contour;
pub mod error;
pub mod import;
pub mod mesh;
pub mod obj;
pub mod qef;
pub mod stl;
pub mod voxelize;
pub use bvh::{Bvh, Nearest, Triangle};
pub use dual_contour::{contour, Settings};
pub use error::{MeshError, Result};
pub use import::Format;
pub use mesh::{Mesh, Topology};
pub use sc_geom::sdf::Grid;
pub use voxelize::voxelize;
```

Sampling a grid is `Grid::sample`, in the kernel. `sc-mesh` deliberately does not
define a second sampler: two readings of one field would eventually disagree, and
the exterior bound is exactly where a disagreement is unsafe.

#### sc_mesh::mesh

Source: [crates/sc-mesh/src/mesh.rs](../crates/sc-mesh/src/mesh.rs).

```rust,ignore
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub indices: Vec<[u32; 3]>,
}
```

```rust,ignore
pub struct Topology {
    pub boundary_edges: usize,
    pub non_manifold_edges: usize,
    pub inconsistent_edges: usize,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Topology::is_printable` | `pub fn is_printable(&self) -> bool` | True when boundary_edges and inconsistent_edges are zero. Allows non-manifold edges and empty meshes. |
| `Topology::is_manifold` | `pub fn is_manifold(&self) -> bool` | True when non_manifold_edges is zero. Does not also check boundary edges or winding. |
| `Mesh::triangle_count` | `pub fn triangle_count(&self) -> usize` | Number of triangles. |
| `Mesh::bounds` | `pub fn bounds(&self) -> Aabb` | Bounding box of the vertices. |
| `Mesh::volume` | `pub fn volume(&self) -> f32` | Enclosed volume in cubic millimetres, by the divergence theorem. |
| `Mesh::topology` | `pub fn topology(&self) -> Topology` | Classifies every edge. |

#### sc_mesh::qef

Source: [crates/sc-mesh/src/qef.rs](../crates/sc-mesh/src/qef.rs).

```rust,ignore
pub struct Qef {
    // Fields are private.
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Qef::new` | `pub fn new() -> Self` | A solver with no constraints. |
| `Qef::add` | `pub fn add(&mut self, position: Vec3, normal: Vec3)` | Adds the tangent plane at a surface crossing. |
| `Qef::count` | `pub fn count(&self) -> u32` | Number of constraints added. |
| `Qef::mass_point` | `pub fn mass_point(&self) -> Vec3` | The average of the crossing points. |
| `Qef::solve` | `pub fn solve(&self, min: Vec3, max: Vec3) -> Vec3` | Solves for the vertex, clamped into `min..max`. |

#### sc_mesh::stl

Source: [crates/sc-mesh/src/stl.rs](../crates/sc-mesh/src/stl.rs).

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `write` | `pub fn write(mesh: &Mesh, path: &std::path::Path) -> std::io::Result<()>` | Writes `mesh` as binary STL. Errors: Any I/O failure, or a mesh with more than `u32::MAX` triangles. |

#### sc_mesh::bvh

Source: [crates/sc-mesh/src/bvh.rs](../crates/sc-mesh/src/bvh.rs).

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Bvh::build` | `pub fn build(triangles: Vec<Triangle>) -> Self` | Median split on the widest axis, `LEAF_TRIANGLES` per leaf. Deterministic. |
| `Bvh::from_mesh` | `pub fn from_mesh(mesh: &Mesh) -> Self` | Builds over a mesh's triangles. |
| `Bvh::nearest` | `pub fn nearest(&self, p: Vec3) -> Option<Nearest>` | Closest point on the closest triangle. `None` for an empty mesh. |
| `Bvh::distance` | `pub fn distance(&self, p: Vec3) -> f32` | Unsigned distance to the surface. |
| `Bvh::winding_number` | `pub fn winding_number(&self, p: Vec3) -> f32` | Generalized winding number, dipole-approximated per node. Above 0.5 means inside. Chosen over ray parity because downloaded meshes are frequently not watertight and parity gives garbage on them, whereas this degrades gracefully. |
| `Bvh::winding_number_exact` | `pub fn winding_number_exact(&self, p: Vec3) -> f32` | Solid angle summed over every triangle. Reference for testing the approximation. |
| `Triangle::closest_point` | `pub fn closest_point(&self, p: Vec3) -> Vec3` | Closest point on the triangle, including its edges and vertices. |
| `Triangle::solid_angle` | `pub fn solid_angle(&self, p: Vec3) -> f64` | Signed solid angle subtended at `p`. `f64` because the winding number sums many near-cancelling terms. |

#### sc_mesh::voxelize

Source: [crates/sc-mesh/src/voxelize.rs](../crates/sc-mesh/src/voxelize.rs).

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `voxelize` | `pub fn voxelize(mesh: &Mesh, resolution: u32) -> Result<Grid>` | Signed distance grid from a triangle mesh. `resolution` is the voxel count along the longest axis. Pads by three voxels a side, one more than `Grid::REQUIRED_CLEARANCE`, because a surface can touch the mesh bounding box exactly. `MeshError::EmptyMesh` for a mesh with no triangles. |
| `voxelize_bvh` | `pub fn voxelize_bvh(bvh: &Bvh, resolution: u32) -> Result<Grid>` | As above, reusing a BVH already built. |
| `inside` | `pub fn inside(bvh: &Bvh, p: Vec3) -> bool` | Winding number test against the 0.5 threshold. |
| `sample_box` | `pub fn sample_box(grid: &Grid) -> (Vec3, Vec3)` | Corners of the region the samples actually cover. |
| `voxel_index`, `voxel_center`, `voxel_value` | see source | Index arithmetic over `Grid::data`, x fastest. |

#### sc_mesh::import, sc_mesh::stl, sc_mesh::obj

Sources: [import.rs](../crates/sc-mesh/src/import.rs), [stl.rs](../crates/sc-mesh/src/stl.rs), [obj.rs](../crates/sc-mesh/src/obj.rs).

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `import::read` | `pub fn read(path: &Path) -> Result<Mesh>` | Dispatches on extension. `MeshError::UnknownFormat` otherwise. |
| `stl::read` | `pub fn read(path: &Path) -> Result<Mesh>` | Binary or ASCII. |
| `stl::is_binary` | `pub fn is_binary(bytes: &[u8]) -> bool` | Checks the declared triangle count against the file length rather than trusting the `solid` prefix, which binary files sometimes carry. |
| `stl::write`, `stl::write_ascii` | `pub fn write(mesh: &Mesh, path: &Path) -> std::io::Result<()>` | Binary and ASCII output. |
| `obj::read`, `obj::read_str` | `pub fn read(path: &Path) -> Result<Mesh>` | Vertices and faces only; polygons are fan-triangulated. Materials, normals, texture coordinates and groups are ignored. |

`MeshError` distinguishes `Io`, `UnknownFormat`, `TriangleCountOverrunsFile`,
`Syntax`, `VertexOutOfRange`, `NonFiniteCoordinate` and `EmptyMesh`. Malformed
input is always a typed error, never a panic and never a partial mesh.

### sc-render

#### sc_render::camera

Source: [crates/sc-render/src/camera.rs](../crates/sc-render/src/camera.rs).

```rust,ignore
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
}
```

```rust,ignore
pub struct CameraRig {
    pub current: OrbitCamera,
    pub goal: OrbitCamera,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `OrbitCamera::framing` | `pub fn framing(bounds: Aabb) -> Self` | Frames a model so it comfortably fills the view. |
| `OrbitCamera::direction` | `pub fn direction(&self) -> Vec3` | Unit vector from the target toward the eye. |
| `OrbitCamera::eye` | `pub fn eye(&self) -> Vec3` | Eye position in world space. |
| `OrbitCamera::basis` | `pub fn basis(&self) -> (Vec3, Vec3, Vec3)` | Orthonormal view basis as `(right, up, forward)`. |
| `OrbitCamera::orbit` | `pub fn orbit(&mut self, delta_yaw: f32, delta_pitch: f32)` | Rotates the camera. Inputs are in radians. |
| `OrbitCamera::orbit_pixels` | `pub fn orbit_pixels(&mut self, dx: f32, dy: f32, viewport: Vec2)` | Rotates by a pointer movement in pixels. |
| `OrbitCamera::zoom` | `pub fn zoom(&mut self, factor: f32)` | Moves the eye toward or away from the target. `factor` is multiplicative, so zooming feels the same at every scale. |
| `OrbitCamera::zoom_towards` | `pub fn zoom_towards(&mut self, factor: f32, anchor: Vec3)` | Zooms while keeping `anchor` fixed on screen. |
| `OrbitCamera::world_per_pixel` | `pub fn world_per_pixel(&self, viewport_height: f32) -> f32` | World units covered by one screen pixel at the target's depth. |
| `OrbitCamera::pan_pixels` | `pub fn pan_pixels(&mut self, dx: f32, dy: f32, viewport_height: f32)` | Slides the target across the view plane by a pointer movement in pixels. |
| `OrbitCamera::ray` | `pub fn ray(&self, ndc: Vec2, aspect: f32) -> (Vec3, Vec3)` | A world-space ray through a point in normalised device coordinates, where x and y run from -1 to 1 and y points up. |
| `OrbitCamera::project` | `pub fn project(&self, world: Vec3, aspect: f32) -> Option<Vec2>` | Where a world point lands in normalised device coordinates. |
| `OrbitCamera::plate_hit` | `pub fn plate_hit(&self, ndc: Vec2, aspect: f32) -> Option<Vec3>` | Where a ray through `ndc` meets the build plate at z = 0. |
| `OrbitCamera::plane_hit` | `pub fn plane_hit(&self, ndc: Vec2, aspect: f32, origin: Vec3, normal: Vec3) -> Option<Vec3>` | Where a ray through `ndc` meets an arbitrary plane. |
| `OrbitCamera::look_along` | `pub fn look_along(&mut self, normal: Vec3)` | Points the camera straight down `normal`, keeping its distance. |
| `OrbitCamera::far` | `pub fn far(&self) -> f32` | How far a ray may travel before being treated as a miss. |
| `OrbitCamera::pixel_angle` | `pub fn pixel_angle(&self, height: u32) -> f32` | Angular size of one pixel, in radians, for a viewport `height` pixels tall. |
| `CameraRig::new` | `pub fn new(camera: OrbitCamera) -> Self` | A rig sitting still at `camera`. |
| `CameraRig::snap_to` | `pub fn snap_to(&mut self, camera: OrbitCamera)` | Jumps both to `camera`, with no easing. |
| `CameraRig::advance` | `pub fn advance(&mut self, dt: f32) -> bool` | Eases toward the goal. Returns true while still moving, so the caller knows to ask for another frame. |

#### sc_render::renderer

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `ScenePalette` | `pub struct ScenePalette { sky, haze, plate, grid: [f32; 3] }` | The viewport's colours. **Linear, not sRGB**: the shader writes into a linear target and the swapchain encodes on the way out, so a hex colour dropped in here comes out about two and a half times too light. `#1F1F23` is `0.0137`. |
| `Renderer::set_scene` | `pub fn set_scene(&mut self, scene: ScenePalette)` | Takes effect on the next draw. Only the uniform changes, so switching appearance does not rebuild the pipeline. |

#### sc_render::gpu

Source: [crates/sc-render/src/gpu.rs](../crates/sc-render/src/gpu.rs).

```rust,ignore
pub enum Preference {
    Hardware,
    Software,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `instance` | `pub fn instance() -> wgpu::Instance` | Creates an instance configured for the platforms we care about. |
| `try_adapter` | `pub fn try_adapter( instance: &wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, preference: Preference, ) -> Option<wgpu::Adapter>` | Picks the best available adapter, optionally one able to present to `surface`. |
| `adapter` | `pub fn adapter( instance: &wgpu::Instance, surface: Option<&wgpu::Surface<'_>>, preference: Preference, ) -> wgpu::Adapter` | Picks the best available adapter. Panics: If no adapter is available at all. |
| `device` | `pub fn device(adapter: &wgpu::Adapter) -> (wgpu::Device, wgpu::Queue)` | Acquires a device and queue with default limits. Panics: If the adapter refuses to create a device. |

#### sc_render::crate root

Source: [crates/sc-render/src/lib.rs](../crates/sc-render/src/lib.rs).

```rust,ignore
pub mod camera;
pub mod gpu;
pub mod renderer;
pub mod shader;
pub mod snapshot;
pub use camera::{CameraRig, OrbitCamera};
pub use renderer::Renderer;
```

#### sc_render::renderer

Source: [crates/sc-render/src/renderer.rs](../crates/sc-render/src/renderer.rs).

```rust,ignore
pub struct Renderer {
    // Fields are private.
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Renderer::new` | `pub fn new( device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, field: &Generated, ) -> Self` | Builds a renderer for a given surface format and generated field. |
| `Renderer::update` | `pub fn update( &mut self, device: &wgpu::Device, queue: &wgpu::Queue, field: &Generated, ) -> bool` | Applies a newly generated field. |
| `Renderer::draw` | `pub fn draw( &self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, camera: &OrbitCamera, width: u32, height: u32, )` | Records a draw covering the whole target. |
| `Renderer::draw_in` | `pub fn draw_in( &self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, camera: &OrbitCamera, rect: [f32; 4], )` | Records a draw confined to `rect`, given in physical pixels as `[x, y, width, height]`. |

#### sc_render::shader

Source: [crates/sc-render/src/shader.rs](../crates/sc-render/src/shader.rs).

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| Constant | `pub const MAX_STEPS: u32 = 192;` | Maximum sphere-tracing steps per pixel. |
| `compose` | `pub fn compose(field_wgsl: &str) -> String` | Returns a complete WGSL module: the generated field plus the renderer. |

#### sc_render::snapshot

Source: [crates/sc-render/src/snapshot.rs](../crates/sc-render/src/snapshot.rs).

```rust,ignore
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `try_render` | `pub fn try_render( field: &sc_geom::wgsl::Generated, camera: &OrbitCamera, width: u32, height: u32, preference: crate::gpu::Preference, ) -> Option<Image>` | Renders `field` from `camera` into an image, or `None` if the machine has no usable GPU. |
| `render` | `pub fn render( field: &sc_geom::wgsl::Generated, camera: &OrbitCamera, width: u32, height: u32, ) -> Image` | Renders `field` from `camera` into an image. Panics: If no GPU adapter is available. |
| `render_with` | `pub fn render_with( device: &wgpu::Device, queue: &wgpu::Queue, field: &sc_geom::wgsl::Generated, camera: &OrbitCamera, width: u32, height: u32, ) -> Image` | Renders using a caller-supplied device. Panics: If the device rejects the work or the readback buffer cannot be mapped. Both indicate a broken driver rather than a recoverable condition. |
| Constant | `pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;` | The texture format offscreen capture renders into. |
| `capture` | `pub fn capture( device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32, record: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView), ) -> Image` | Renders whatever `record` draws into an offscreen target and reads it back. Panics: If the device rejects the work or the readback buffer cannot be mapped. |
| `write_png` | `pub fn write_png(image: &Image, path: &std::path::Path) -> std::io::Result<()>` | Writes an `Image` to a PNG file. Errors: Any I/O or encoding failure. |

## Desktop internal interfaces

`sc-app` exposes no public Rust library. The following types and methods are
crate-visible integration points for desktop contributors. UI events should
call `AppState`; document mutations still go through `Document::apply`.
See [interface.md](interface.md) for widget and pointer ownership rules.

### sc-app

#### sc_app::dialog

Source: [crates/sc-app/src/dialog.rs](../crates/sc-app/src/dialog.rs).

```rust,ignore
pub(crate) enum Purpose {
    Open,
    SaveAs,
    ExportStl,
}
```

```rust,ignore
pub(crate) enum Outcome {
    Pending,
    Cancelled,
    Chosen(PathBuf),
}
```

```rust,ignore
pub(crate) struct FileBrowser {
    pub(crate) purpose: Purpose,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `FileBrowser::new` | `pub(crate) fn new(purpose: Purpose, start: &Path, filename: String) -> Self` | Internal helper; see source for behavior. |
| `FileBrowser::show` | `pub(crate) fn show(&mut self, ctx: &egui::Context) -> Outcome` | Internal helper; see source for behavior. |

#### sc_app::icon

Source: [crates/sc-app/src/icon.rs](../crates/sc-app/src/icon.rs).

```rust,ignore
pub(crate) enum Icon {
    Pen,
    Square,
    Circle,
    Hexagon,
    Cube,
    Sphere,
    Cylinder,
    Slot,
    Hole,
    HexHole,
    Shell,
    Offset,
    Move,
    Undo,
    Redo,
    File,
    Folder,
    Save,
    Download,
    Frame,
    Cursor,
    Layers,
    Plane,
    Union,
    Subtract,
    Intersect,
    Torus,
    Extrude,
    Pattern,
    Trash,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `draw` | `pub(crate) fn draw(painter: &Painter, rect: Rect, icon: Icon, color: Color32)` | Draws `icon` centred in `rect`. |
| `for_kind` | `pub(crate) fn for_kind(kind: &str) -> Icon` | The glyph that stands for a node kind in the design tree. |

#### sc_app::plane

Source: [crates/sc-app/src/plane.rs](../crates/sc-app/src/plane.rs).

```rust,ignore
pub(crate) enum SketchPlane {
    Xy,
    Xz,
    Yz,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| Constant | `pub(crate) const ALL: [Self; 3] = [Self::Xy, Self::Xz, Self::Yz];` | All three, in the order they are shown. |
| `SketchPlane::name` | `pub(crate) fn name(self) -> &'static str` | Internal helper; see source for behavior. |
| `SketchPlane::describe` | `pub(crate) fn describe(self) -> &'static str` | What sketching on this plane means, in the terms a printed part is thought about: which way is up, and which way a pad grows. |
| `SketchPlane::frame` | `pub(crate) fn frame(self) -> (Vec3, Vec3, Vec3)` | The in-plane axes and the normal, as a right-handed frame. |
| `SketchPlane::to_world` | `pub(crate) fn to_world(self, point: Vec2) -> Vec3` | Lifts a sketch coordinate into the model. |
| `SketchPlane::placement` | `pub(crate) fn placement(self) -> Transform` | The transform that carries geometry built in sketch coordinates into the model. |
| `SketchPlane::placement_at` | `pub(crate) fn placement_at(self, offset: f32) -> Transform` | As `SketchPlane::placement`, shifted along the normal. |

#### sc_app::settings

Source: [crates/sc-app/src/settings.rs](../crates/sc-app/src/settings.rs).

```rust,ignore
pub(crate) struct Settings {
    pub(crate) ui_scale: Option<f32>,
    pub(crate) display: Option<String>,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Settings::load` | `pub(crate) fn load() -> Self` | Reads preferences, falling back to defaults on any problem. |
| `Settings::save` | `pub(crate) fn save(&self)` | Writes preferences, ignoring failure. |
| `auto_scale` | `pub(crate) fn auto_scale(monitor_width_px: u32, native_points_per_pixel: f32) -> f32` | Interface zoom to use when the user has not chosen one. |

#### sc_app::snapshot

Source: [crates/sc-app/src/snapshot.rs](../crates/sc-app/src/snapshot.rs).

```rust,ignore
pub(crate) enum Scene {
    Empty,
    Dialog,
    Sample,
    Hover,
    Menu,
    Showcase,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `write` | `pub(crate) fn write(path: &std::path::Path, width: u32, height: u32, scene: Scene, scale: f32)` | Renders one frame of the application to a PNG. Panics: If no GPU is available or the image cannot be written. |

#### sc_app::state

Source: [crates/sc-app/src/state.rs](../crates/sc-app/src/state.rs).

```rust,ignore
pub(crate) type Selection = Option<NodeId>;
```

```rust,ignore
pub(crate) enum MenuTarget {
    Node(NodeId),
    Empty,
    Sketch,
}
```

```rust,ignore
pub(crate) struct ContextMenu {
    pub at: (f32, f32),
    pub target: MenuTarget,
}
```

```rust,ignore
pub(crate) struct AppState {
    pub doc: Document,
    pub rig: CameraRig,
    pub selected: Selection,
    pub field_dirty: bool,
    pub status: String,
    pub last_edit_ms: f32,
    pub last_edit_rebuilt: bool,
    pub tab: usize,
    pub tool: usize,
    pub sketch: Option<Vec<Vec2>>,
    pub extrude_height: f32,
    pub plane: SketchPlane,
    pub attached_to: Option<NodeId>,
    pub grid: f32,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub browser: Option<FileBrowser>,
    pub ui_scale: f32,
    pub settings: Settings,
    pub menu: Option<ContextMenu>,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| Constant | `pub(crate) const TOOL_SELECT: usize = 0;` | Index of the pointer tool in the viewport tool bar. |
| Constant | `pub(crate) const TOOL_SKETCH: usize = 1;` | Index of the sketch tool. |
| `AppState::new` | `pub(crate) fn new() -> Self` | Internal helper; see source for behavior. |
| `AppState::open_menu` | `pub(crate) fn open_menu(&mut self, at: (f32, f32), target: MenuTarget)` | Opens the context menu at `at`, in interface points. |
| `AppState::close_menu` | `pub(crate) fn close_menu(&mut self) -> bool` | Closes the context menu, if one is open. True if there was one. |
| `AppState::frame_node` | `pub(crate) fn frame_node(&mut self, id: NodeId)` | Frames one node rather than the whole model. |
| `AppState::set_display` | `pub(crate) fn set_display(&mut self, name: Option<String>)` | Chooses the display to open on, and remembers it. |
| `AppState::set_ui_scale` | `pub(crate) fn set_ui_scale(&mut self, scale: f32)` | Changes interface zoom and remembers the choice. |
| `AppState::title` | `pub(crate) fn title(&self) -> String` | The document name for the title bar, with a marker for unsaved changes. |
| `AppState::new_document` | `pub(crate) fn new_document(&mut self)` | Replaces the document with an empty one. |
| `AppState::load_sample` | `pub(crate) fn load_sample(&mut self)` | Loads the built-in reference part. |
| `AppState::look_along` | `pub(crate) fn look_along(&mut self, normal: sc_geom::glam::Vec3)` | Points the camera down `normal` without moving what it is looking at. |
| `AppState::camera` | `pub(crate) fn camera(&self) -> OrbitCamera` | The camera the viewport should draw this frame. |
| `AppState::frame_model` | `pub(crate) fn frame_model(&mut self)` | Frames the whole model, easing rather than cutting. |
| `AppState::pick_world` | `pub(crate) fn pick_world(&self, ndc: sc_geom::glam::Vec2, aspect: f32) -> Vec3` | The world point under a viewport position. |
| `AppState::trace` | `pub(crate) fn trace(&self, ndc: sc_geom::glam::Vec2, aspect: f32) -> Option<Vec3>` | Sphere-traces the model, returning the surface point if the ray hits it. |
| `AppState::select_at` | `pub(crate) fn select_at(&mut self, ndc: sc_geom::glam::Vec2, aspect: f32, tolerance: f32)` | Selects whatever lies under a viewport position, or clears the selection. |
| `AppState::open_path` | `pub(crate) fn open_path(&mut self, path: &Path)` | Internal helper; see source for behavior. |
| `AppState::save_to` | `pub(crate) fn save_to(&mut self, path: &Path)` | Internal helper; see source for behavior. |
| `AppState::save` | `pub(crate) fn save(&mut self)` | Saves in place, or asks where to put it if the document has no path yet. |
| `AppState::browse` | `pub(crate) fn browse(&mut self, purpose: Purpose)` | Opens the file browser, starting wherever the document lives. |
| `AppState::finish_browse` | `pub(crate) fn finish_browse(&mut self, purpose: Purpose, path: &Path)` | Applies whatever the browser returned. |
| `AppState::apply` | `pub(crate) fn apply(&mut self, command: Command) -> Option<NodeId>` | Applies a command, reporting failures to the status bar rather than panicking. A rejected command leaves the document untouched. |
| `AppState::undo` | `pub(crate) fn undo(&mut self)` | Internal helper; see source for behavior. |
| `AppState::redo` | `pub(crate) fn redo(&mut self)` | Internal helper; see source for behavior. |
| `AppState::add_pad` | `pub(crate) fn add_pad(&mut self, profile: sc_geom::Profile, label: &str)` | Adds a pad built from a parametric profile on the current plane. |
| `AppState::add_pocket` | `pub(crate) fn add_pocket(&mut self, profile: sc_geom::Profile, label: &str)` | Cuts a profile through the model from the current plane. |
| `AppState::wrap_selection` | `pub(crate) fn wrap_selection(&mut self, make: impl FnOnce(NodeId) -> Node, label: &str)` | Wraps the selection in a modifier and puts the wrapper where the selection used to sit. |
| `AppState::add_body` | `pub(crate) fn add_body(&mut self, node: Node, label: &str)` | Adds a primitive and unions it onto the current root, so a new body shows up immediately instead of sitting orphaned in the tree. |
| `AppState::wgsl` | `pub(crate) fn wgsl(&self) -> sc_geom::wgsl::Generated` | Internal helper; see source for behavior. |
| `AppState::start_sketch` | `pub(crate) fn start_sketch(&mut self)` | Begins a new profile on the current plane. |
| `AppState::sketch_frame` | `pub(crate) fn sketch_frame(&self) -> Transform` | The frame a sketch is drawn in: datum, or the face it is attached to. |
| `AppState::plane_origin` | `pub(crate) fn plane_origin(&self) -> Vec3` | Where the sketch plane sits in the model. |
| `AppState::plane_normal` | `pub(crate) fn plane_normal(&self) -> Vec3` | The sketch plane's outward normal. |
| `AppState::to_world` | `pub(crate) fn to_world(&self, point: Vec2) -> Vec3` | Lifts a sketch coordinate into the model. |
| `AppState::to_plane` | `pub(crate) fn to_plane(&self, point: Vec3) -> Vec2` | Drops a model point onto the sketch plane's coordinates. |
| `AppState::attach_to_selection` | `pub(crate) fn attach_to_selection(&mut self)` | Attaches the sketch plane to the selected feature's far face. Looks through single-child wrappers (offset, shell, transform) to the pad underneath, because a user selects the finished feature rather than the bare extrude. Refuses on a boolean: both sides have a face and nothing can say which was meant. |
| `AppState::attachable_face` | `pub(crate) fn attachable_face(&self, id: NodeId) -> Option<NodeId>` | The pad a given selection would attach to, or `None`. |
| `AppState::move_selection` | `pub(crate) fn move_selection(&mut self, delta: Vec3)` | Moves the selection. An attached feature keeps its attachment: the move goes into a placement **beneath** the derived one, in the feature's own frame, so it slides across the face in x and y and lifts off it in z, and still follows that face when the base changes. Repeated moves accumulate into one offset rather than stacking a node each. Anything unattached is wrapped in a placement of its own. |
| `AppState::duplicate_selection` | `pub(crate) fn duplicate_selection(&mut self)` | Copies the selected subtree, places the copy four grid steps along X so it lands beside the original rather than inside it, unions it onto the model and selects it. The copy is independent: editing it does not touch the source. A node referenced twice inside the subtree is copied once, so shared structure stays shared. One undo step. |
| `AppState::clone_subtree` | `fn clone_subtree(&mut self, id: NodeId) -> Option<NodeId>` | The copy itself, children first so a child id always exists before its parent references it. Memoized per source node, which is what keeps a shared child shared. |
| `AppState::repeat_selection` | `pub(crate) fn repeat_selection(&mut self, kind: Repeat)` | Wraps the selection in a `Node::Pattern` with a count of 4, rewiring the parent so the pattern takes the selection's place, and selects the pattern so the count is immediately editable. A linear step is recomputed from the selection's bounds rather than taken from `kind`, so instances land beside each other at any scale; a circular sweep is used as given. |
| `AppState::detach_selection` | `pub(crate) fn detach_selection(&mut self)` | Clears the derivation, keeping the resolved placement so nothing jumps. The only way to stop a feature following its face, and deliberately explicit. |
| `AppState::selection_is_attached` | `pub(crate) fn selection_is_attached(&self) -> bool` | Whether the selection is a placement that follows a face. Gates the Detach action and the property panel. |
| `AppState::detach_plane` | `pub(crate) fn detach_plane(&mut self)` | Returns the sketch plane to a datum. |
| `AppState::set_plane` | `pub(crate) fn set_plane(&mut self, plane: SketchPlane)` | Chooses the plane the next sketch will be drawn on. |
| `AppState::cancel_sketch` | `pub(crate) fn cancel_sketch(&mut self)` | Internal helper; see source for behavior. |
| `AppState::snap` | `pub(crate) fn snap(&self, point: Vec2) -> Vec2` | Rounds a plate position to the sketch grid. |
| `AppState::add_sketch_point` | `pub(crate) fn add_sketch_point(&mut self, point: Vec2)` | Adds a point to the profile in progress. |
| `AppState::undo_sketch_point` | `pub(crate) fn undo_sketch_point(&mut self)` | Internal helper; see source for behavior. |
| `AppState::finish_sketch` | `pub(crate) fn finish_sketch(&mut self)` | Turns the profile in progress into an extruded solid. |
| `AppState::select` | `pub(crate) fn select(&mut self, id: Option<NodeId>)` | Selects a node and refreshes the viewport highlight. |
| `AppState::export_stl` | `pub(crate) fn export_stl(&mut self, path: &Path, resolution: u32)` | Meshes the model and writes it as STL. |

#### sc_app::settings

Source: [crates/sc-app/src/settings.rs](../crates/sc-app/src/settings.rs).

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Appearance` | `enum Appearance { System, Light, Dark }` | What the user asked for. A request, not a result: `System` becomes a `Scheme` only once the window system has been asked. Serialized with a serde default, so a settings file written before this existed loads as `System`. |
| `Appearance::resolve` | `pub(crate) fn resolve(self, system: Option<Scheme>) -> Scheme` | `None` for `system` is a normal answer, not a failure: several Wayland compositors never report a colour scheme. `System` then resolves to light, the scheme the interface was designed in. |

#### sc_app::theme

Source: [crates/sc-app/src/theme.rs](../crates/sc-app/src/theme.rs).

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `Scheme` | `enum Scheme { Light, Dark }` | Which palette is in force. Resolved, never "follow the system". |
| `Palette` | struct of `Color32` fields plus `sky`/`haze`/`plate`/`grid` as `[f32; 3]` | Every colour the interface uses, so a scheme is one value rather than `if dark` scattered across the call sites. |
| `palette` | `pub(crate) fn palette() -> Palette` | The colours in force right now. Read this rather than naming a colour at a call site. |
| `scene` | `pub(crate) fn scene() -> sc_render::ScenePalette` | The viewport's share of the palette, in the form the renderer wants. |
| `set_scheme` | `pub(crate) fn set_scheme(scheme: Scheme)` | Process-wide, because a desktop application has one appearance at a time. Takes effect next frame. Callers must also call `apply` (egui caches style colours) and set `field_dirty` (the viewport's colours live in a shader uniform). |
| `apply` | `pub(crate) fn apply(ctx: &egui::Context)` | Installs fonts, metrics and the palette. Idempotent, so it can be called again on every scheme change. Pins egui to its light base in both schemes: every visible colour comes from `palette()`, and letting egui swap underneath would give a scheme assembled from two sources. |
| Constant | `pub(crate) const CANVAS: Color32 = Color32::from_rgb(0xFA, 0xFA, 0xFA);` | Application background, behind the cards. |
| Constant | `pub(crate) const SURFACE: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);` | Card and panel fill. |
| Constant | `pub(crate) const SURFACE_ALT: Color32 = Color32::from_rgb(0xF4, 0xF4, 0xF5);` | Inset and hover fill. |
| Constant | `pub(crate) const BORDER: Color32 = Color32::from_rgb(0xE4, 0xE4, 0xE7);` | Hairline borders. |
| Constant | `pub(crate) const TEXT: Color32 = Color32::from_rgb(0x09, 0x09, 0x0B);` | Primary text. |
| Constant | `pub(crate) const TEXT_DIM: Color32 = Color32::from_rgb(0x71, 0x71, 0x7A);` | Secondary text: units, hints, metadata. |
| Constant | `pub(crate) const ACCENT: Color32 = Color32::from_rgb(0x25, 0x63, 0xEB);` | Reserved for selected geometry, in the viewport and in the tree. |
| Constant | `pub(crate) const ACCENT_SOFT: Color32 = Color32::from_rgb(0xEF, 0xF6, 0xFF);` | Selection background. |
| Constant | `pub(crate) const DANGER: Color32 = Color32::from_rgb(0xDC, 0x26, 0x26);` | Near-black, for the single primary action. Destructive actions. The only other hue in the interface. |
| Constant | `pub(crate) const INK: Color32 = Color32::from_rgb(0x18, 0x18, 0x1B);` | Near-black, used for the single primary button and the application mark. |
| `semibold` | `pub(crate) fn semibold() -> FontFamily` | Family used for headings and emphasis. |
| `install_fonts` | `pub(crate) fn install_fonts(ctx: &egui::Context)` | Installs Inter and makes it the proportional face. |
| `apply` | `pub(crate) fn apply(ctx: &egui::Context)` | Installs the palette and metrics. |
| `card` | `pub(crate) fn card() -> egui::Frame` | A white card: the unit the whole layout is built from. |
| `floating` | `pub(crate) fn floating() -> egui::Frame` | A card that floats over the 3D view, so it needs a shadow to separate. |
| `bar` | `pub(crate) fn bar(bottom_border: bool) -> egui::Frame` | A flat bar with a hairline on one edge, for the top and bottom chrome. |
| `section` | `pub(crate) fn section(ui: &mut egui::Ui, text: &str)` | Section label inside a card: small, muted, widely tracked. |
| `row` | `pub(crate) fn row( ui: &mut egui::Ui, icon: crate::icon::Icon, label: &str, active: bool, enabled: bool, ) -> egui::Response` | A sidebar row: icon, label, full width, no chrome until hovered. |
| `tool_button` | `pub(crate) fn tool_button( ui: &mut egui::Ui, icon: crate::icon::Icon, label: &str, active: bool, enabled: bool, ) -> egui::Response` | A compact icon-and-label button. Used in the top bar and the floating toolbar, where a full-width row would be wrong but a bare label is mute. |
| `icon_button` | `pub(crate) fn icon_button( ui: &mut egui::Ui, icon: crate::icon::Icon, enabled: bool, ) -> egui::Response` | Icon only, square. For actions whose glyph is universal: undo, redo. |
| `tree_row` | `pub(crate) fn tree_row( ui: &mut egui::Ui, depth: usize, icon: crate::icon::Icon, title: &str, note: Option<&str>, selected: bool, ) -> egui::Response` | A design tree row: indent guide, kind glyph, name, and a muted kind note. |
| `hint` | `pub(crate) fn hint( response: egui::Response, title: &str, body: &str, shortcut: Option<&str>, ) -> egui::Response` | Attaches an explanatory tooltip to a control. |
| `menu` | `pub(crate) fn menu() -> egui::Frame` | The frame a context menu is drawn in. |
| `menu_item` | `pub(crate) fn menu_item( ui: &mut egui::Ui, icon: crate::icon::Icon, label: &str, shortcut: Option<&str>, enabled: bool, destructive: bool, ) -> egui::Response` | One item in a context menu: icon, label, and an optional shortcut. |
| `menu_title` | `pub(crate) fn menu_title(ui: &mut egui::Ui, title: &str, note: &str)` | A menu heading: what the menu is acting on. |
| `menu_separator` | `pub(crate) fn menu_separator(ui: &mut egui::Ui)` | A hairline between groups of menu items. |
| `danger_row` | `pub(crate) fn danger_row( ui: &mut egui::Ui, icon: crate::icon::Icon, label: &str, ) -> egui::Response` | A ghost row for a destructive action. The only red in the interface. |
| `empty_state` | `pub(crate) fn empty_state(ui: &mut egui::Ui, icon: crate::icon::Icon, title: &str, hint: &str)` | A centred placeholder for a panel with nothing in it. |
| `primary_button` | `pub(crate) fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response` | The single dark call-to-action. |
| `primary_button_with_icon` | `pub(crate) fn primary_button_with_icon( ui: &mut egui::Ui, icon: crate::icon::Icon, text: &str, ) -> egui::Response` | The same, with a leading glyph. |
| `segmented` | `pub(crate) fn segmented(ui: &mut egui::Ui, current: &mut usize, labels: &[(&str, &str)]) -> bool` | A segmented control. Returns true if the choice changed. |
| `logo` | `pub(crate) fn logo(ui: &mut egui::Ui, size: f32)` | The application mark: an isometric solid with its corners filleted. |

#### sc_app::ui

Source: [crates/sc-app/src/ui.rs](../crates/sc-app/src/ui.rs).

```rust,ignore
pub(crate) struct Chrome {
    pub(crate) viewport: egui::Rect,
    pub(crate) overlays: Vec<egui::Rect>,
}
```

| Item | Signature | Behavior / failure conditions |
| --- | --- | --- |
| `draw` | `pub(crate) fn draw(ui: &mut egui::Ui, state: &mut AppState) -> Chrome` | Draws the whole shell and reports where everything landed. |
| `ndc_of` | `pub(crate) fn ndc_of(pos: egui::Pos2, viewport: egui::Rect) -> GVec2` | Maps a point in the viewport to normalised device coordinates. |

## Command-line interfaces

Run from the repository root. Both commands below use Cargo package names;
the built binaries are `shapecad` (desktop) and `shapecad-cli` (headless).

### Headless runner

Source: [sc-cli/src/main.rs](../crates/sc-cli/src/main.rs).

| Invocation after `cargo run --release -p sc-cli --` | Behavior |
| --- | --- |
| `demo` or no command | Build the reference bracket; print its outline, bounds and hash |
| `demo --wgsl` | Also print generated WGSL source; the parameter buffer is separate |
| `export part.stl --resolution 128` | Mesh and export the reference bracket; default resolution is 128 |
| `selftest` | Compare the reference hash with `tests/corpus/bracket.hash` |
| `help`, `--help`, `-h` | Print usage |

The CLI does not load arbitrary `.shapecad` files or accept the document command
vocabulary. Use `sc-doc` from Rust for those workflows. Unknown commands exit 2;
selftest failures and export errors exit 1. Export's current manifold success
message is stronger than the topology predicate it checks.

### Desktop and snapshots

Source: [sc-app/src/main.rs](../crates/sc-app/src/main.rs).

| Flag after `cargo run --release -p sc-app --` | Behavior |
| --- | --- |
| No flags | Open the desktop application |
| `--displays` | List available displays through the window event loop |
| `--display NAME` | Choose and remember a display |
| `--display auto` | Return display choice to the window system |
| `--snapshot PATH` | Write an interface PNG without opening the desktop window |
| `--width N`, `--height N` | Snapshot pixel size; defaults 1500 by 940 |
| `--scale N` | Snapshot UI scale; default 1.0 |
| `--sample` | Snapshot with the built-in bracket |
| `--dialog` | Snapshot demonstrating the file browser |
| `--hover` | Snapshot demonstrating a tooltip |
| `--menu` | Snapshot demonstrating a context menu |
| `--showcase` | Snapshot demonstrating the modeling workflow |

Scene flags apply only with `--snapshot`. With none, capture the empty document.
If several are supplied, precedence is dialog, sample, hover, menu, showcase.

```sh
cargo run --release -p sc-app -- --snapshot out.png --sample --width 2400 --height 1500 --scale 1.5
```

## Keeping this reference current

When changing an exported method, type, parameter key, feature gate or CLI flag,
update this document in the same change. Check the source-linked module for the
full contract and trait implementations. Rustdoc remains the compiler-generated
reference for exact paths and signatures.

Run the repository checks described in [AGENTS.md](../AGENTS.md), and build docs
with `cargo doc --workspace --no-deps --all-features`. New geometry needs the
example, property and golden checks described in [kernel.md](kernel.md).
