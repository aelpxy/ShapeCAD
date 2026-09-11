<p align="center">
  <img src="assets/logo.svg" width="104" height="104" alt="">
</p>

<h1 align="center">ShapeCAD</h1>

<p align="center">
  An experimental desktop CAD app for designing parts to 3D print.<br>
  Written in Rust, with a GPU-rendered viewport and STL export.
</p>

<p align="center">
  <img src="assets/screenshot.png" width="900" alt="The ShapeCAD window: a design tree and tool list on the left, a bracket in the 3D viewport with its upright wall selected, and its dimensions on the right.">
</p>

---

ShapeCAD is an early prototype and isn't usable yet. Basic modeling tools are
in place, but sketching, feature dependencies and mesh export still need work.

The code was written with OpenAI Codex and Claude Code, with me directing and
reviewing the work. I haven't hand-written the code.

## What you can do

- Draw a closed polygon on the XY, XZ or YZ plane, snap points to a grid,
  and extrude it.
- Add rectangular, circular or hexagonal pads and edit their dimensions.
  Change the hexagon's side count to make other regular polygons.
- Add spheres, boxes and cylinders, or cut rectangular, circular and hexagonal
  pockets through a part.
- Start a polygon sketch or a pocket on an existing pad's top surface.
- Select bodies and boolean joints in the viewport or design tree, then edit
  dimensions and blend radii in the property panel.
- Save and open designs as readable JSON, and export meshes as STL.

Numeric edits update the viewport through a GPU parameter buffer. They don't
recompile the shader unless the generated shader code changes.

## What's still rough

- Sketches have no constraint solver or arc tools. The shape buttons create
  pads immediately; there's no separate sketch editor for dimensioning a
  profile before using it in several features.
- There are no push/pull handles or direct movement tools in the viewport.
  Dimensions are edited in the side panel.
- Completed features keep a fixed placement. Changing the depth of a supporting
  pad won't move a sketch extrusion built on its top surface, and a through-cut
  won't automatically extend to follow a thicker part.
- Shell, offset and move currently affect the visible model only when applied
  to its root. Applying them to a child creates a modifier without reconnecting
  it to the model.
- Undo and redo work per command. Creating a feature or dragging a value can
  leave several undo steps for one action.
- Some exports have non-manifold edges. The CLI can still report these as
  manifold, so its success message isn't a guarantee. See
  [the meshing notes](docs/meshing.md).

## Run it

You'll need Rust, a C linker and a GPU driver. The Rust version is pinned in
[rust-toolchain.toml](rust-toolchain.toml). System packages and WSL setup are
covered in [the build guide](docs/building.md).

```sh
cargo run --release -p sc-app
```

Use a release build for the viewport and mesh export.

Click to select, drag to orbit, right-drag or Shift-drag to pan, and scroll to
zoom. Right-click for actions on the item under the pointer. Double-click to
focus, or press `F` to fit the model.

To try a simple part, choose **XY**, add a **Rectangle**, and set its width,
height and depth in the right panel. Use **Hole** to cut through it, then change
the hole's radius in the same panel.

To draw your own outline, choose a plane and press **Sketch a profile**. Click
points, then press Enter to close and extrude it. Backspace removes the last
point; Escape cancels.

The app has been tested on Linux, including WSL. Windows and macOS are untested.

The headless CLI can inspect and export the built-in reference part:

```sh
cargo run -p sc-cli -- demo
cargo run --release -p sc-cli -- export bracket.stl --resolution 192
cargo run -p sc-cli -- selftest
```

## How it works

ShapeCAD stores geometry as a directed acyclic graph of implicit operations. The viewport
evaluates that tree on the GPU, and a dual contouring mesher turns it into
triangles for export.

This makes it possible to combine shapes and blend their joins without managing
surface topology at each edit. It also comes with tradeoffs: there's no exact
NURBS geometry or STEP export, and the exported mesh depends on the meshing
resolution. The reasoning is in [the kernel design decision](docs/adr/0001-implicit-kernel.md).

The workspace has six crates:

| Crate | Purpose |
| --- | --- |
| `sc-geom` | Geometry tree, evaluation, bounds, hashing and shader generation |
| `sc-doc` | Documents, commands, undo and the file format |
| `sc-mesh` | Dual contouring and STL export |
| `sc-render` | Viewport rendering and camera controls |
| `sc-app` | Desktop interface |
| `sc-cli` | Headless commands |

Document edits go through `Document::apply`, which keeps the command history
and undo behavior consistent. Commands are atomic: a rejected edit leaves the
document unchanged. This command interface is also the basis for planned agent
support.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
cargo run -p sc-cli -- selftest
```

Tests cover geometry evaluation, bounds, undo, meshing and rendering. Property
tests exercise randomly generated models, and the CLI's `selftest` checks a
reference part against its saved geometry hash.

See [CONTRIBUTING.md](CONTRIBUTING.md) for development conventions and
[docs/](docs/) for architecture and implementation notes.

## License

ShapeCAD is licensed under the [Apache License 2.0](LICENSE).

The bundled Inter font uses the [SIL Open Font License](crates/sc-app/assets/fonts/LICENSE.txt).
