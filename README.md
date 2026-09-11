# ShapeCAD

ShapeCAD is an experimental desktop CAD app for designing parts to 3D print.
It's written in Rust, with a GPU-rendered viewport and STL export.

It's still early. Basic modeling works, but selection, sketching and meshing
need more work before I'd trust it for everyday use.

The code was written with OpenAI Codex and Claude Code, with me directing and
reviewing the work. I haven't hand-written the code.

## What you can do

- Draw a closed polygon on the build plate and extrude it.
- Add spheres, boxes, cylinders, tori and planes.
- Combine or subtract shapes, adjust blend radii, and apply shells, offsets
  and transforms.
- Edit objects through the design tree and property panel, with undo and redo.
- Save and open designs as readable JSON, and export meshes as STL.

Selection is currently through the tree. You can't click a body in the viewport
to select or move it yet. Sketches only support polygons on the build plate;
there are no constraints, arcs, circles or snapping.

Parameter edits recompile the viewport shader, so dragging values can be choppy.
The mesher can also produce non-manifold edges on some models. See
[the meshing notes](docs/meshing.md) for the details.

## Run it

You'll need Rust, a C linker and a GPU driver. The Rust version is pinned in
[rust-toolchain.toml](rust-toolchain.toml). System packages and WSL setup are
covered in [the build guide](docs/building.md).

```sh
cargo run --release -p sc-app
```

Use a release build for the viewport and mesh export.

Drag to orbit, right-drag or Shift-drag to pan, and scroll to zoom. Double-click
to focus, or press `F` to fit the model. To draw a profile, press **Sketch a
profile**, click points on the plate, then press Enter.

The app has been tested on Linux, including WSL. Windows and macOS are untested.

The headless CLI can inspect and export the built-in reference part:

```sh
cargo run -p sc-cli -- demo
cargo run --release -p sc-cli -- export bracket.stl --resolution 192
cargo run -p sc-cli -- selftest
```

## How it works

ShapeCAD stores geometry as a tree of signed distance functions. The viewport
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
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Tests cover geometry evaluation, bounds, undo, meshing and rendering. Property
tests exercise randomly generated models, and the CLI's `selftest` checks a
reference part against its saved geometry hash.

See [CONTRIBUTING.md](CONTRIBUTING.md) for development conventions and
[docs/](docs/) for architecture and implementation notes.

## License

ShapeCAD is licensed under the [Apache License 2.0](LICENSE).

The bundled Inter font uses the [SIL Open Font License](crates/sc-app/assets/fonts/LICENSE.txt).
