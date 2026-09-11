# Working on ShapeCAD

Guidance for AI coding agents. This file is the source of truth; `CLAUDE.md`
points here so the two cannot drift apart.

ShapeCAD is a CAD program for 3D printing. Geometry is a DAG of signed distance
functions, not a boundary representation. Read [docs/architecture.md](docs/architecture.md)
before your first change, and [docs/kernel.md](docs/kernel.md) before touching
geometry. The four decision records in [docs/adr/](docs/adr/) explain why the
design is what it is.

## Non-negotiables

These are load-bearing. Breaking one is a bug even if the tests pass.

1. **Every mutation goes through `Document::apply`.** The interface, the CLI and
   the agent layer share one command vocabulary. Do not reach into `Arena` from
   application code.
2. **Commands are all-or-nothing.** Validate before mutating. A rejected edit
   must leave the document byte-identical and unlogged.
3. **Node ids are stable and never reused.** A selection or a stored reference
   must survive arbitrary edits, including undo and a save/load round trip.
4. **The field stays an exact signed distance, not a bound.** If you can only
   bound it, say so in the doc comment, because everything downstream assumes
   exactness. This is why `Transform` carries uniform scale only.
5. **Bounds may over-report, never under-report.** Too-small bounds silently
   clip geometry out of an export.

## Repo map

| Path | What lives there |
|---|---|
| `crates/sc-geom` | The kernel: node DAG, evaluation, bounds, hashing, WGSL generation |
| `crates/sc-doc` | Documents: command log, undo, `.shapecad` format, sample parts |
| `crates/sc-mesh` | Dual contouring to printable triangles, STL output |
| `crates/sc-render` | Viewport, camera, GPU selection, offscreen capture |
| `crates/sc-app` | The desktop application |
| `crates/sc-cli` | Headless runner |
| `docs/` | Reference documentation and decision records |
| `tests/corpus/` | Golden geometry hashes |

Dependencies point one way only, from `sc-app` down to `sc-geom`. Do not add an
edge that points back up.

## Commands

```sh
cargo test --workspace                  # 90 tests
cargo clippy --workspace --all-targets --all-features
cargo fmt --all
cargo run --release -p sc-app           # the application
cargo run -p sc-cli -- selftest         # golden geometry check
```

Anything that renders or meshes needs `--release`; debug is roughly 20x slower.

Verify visually without a display by capturing the whole interface:

```sh
cargo run --release -p sc-app -- --snapshot out.png --width 2400 --height 1500 --scale 1.5
```

## Before you say you are done

1. `cargo fmt --all`
2. `cargo clippy --workspace --all-targets --all-features` reports **zero**
   warnings. The configuration is deliberately strict: `clippy::pedantic`,
   `missing_docs`, `unsafe_code = "forbid"`.
3. `cargo test --workspace` passes.
4. `cargo run -p sc-cli -- selftest` passes. If the hash moved, either you
   changed geometry on purpose, in which case update `tests/corpus/bracket.hash`
   and say why, or you broke something.
5. If you changed anything visual, render a snapshot and actually look at it.

## Testing expectations

Three layers, and new work is expected to carry the right ones.

**Example tests** in `src/` pin down specific claims. Name them as the claim they
make: `shell_hollows_inward_and_keeps_the_outer_surface`, not `test_shell`.

**Property tests** in `tests/properties.rs` pin down invariants across thousands
of generated models. This is where the real guarantees live. Adding a node kind
means adding it to the generator, which is the step that gets skipped and the one
that actually tests the node.

**Golden geometry** catches shape changes that look fine in a render.

Verify geometry by hashing, never by eye. A render can be completely convincing
while the part is several percent the wrong size. `geometry_hash` returns three
separate digests, structure, field and bounds, so a failure says what moved.

If a property test fails, proptest writes the shrunken counterexample to
`tests/proptest-regressions/`. Commit that file.

## Adding a node kind

1. `node.rs`: the variant, then `kind()`, `params()`, `set_param()`,
   `is_valid()`, `children()`, `map_children()`.
2. `eval.rs`: evaluate it.
3. `bounds.rs`: bound it conservatively.
4. `wgsl.rs`: emit it. Anything needing a loop or an array goes in a helper
   function via `Emitter::helpers`, not inline.
5. `tests/properties.rs` in both `sc-geom` and `sc-mesh`: add it to the generator.

The property panel and the agent interface both work off `params()`, so you get
an editor for free.

## Traps that will cost you an hour

- **`Mat3::default()` in glam is the identity, not zero.** Deriving `Default` on
  anything accumulating a matrix seeds it with a phantom constraint. This made
  every mesh 6 to 11 percent undersized while still watertight and plausible.
- **naga rejects `vec3<bool>` constructors.** Use scalar booleans in generated
  WGSL. Generated shaders are validated by `naga` in the kernel's tests.
- **egui reports pointer events as consumed whenever the cursor is merely over
  one of its areas**, and the viewport is a panel. Honouring that flag means the
  3D view never receives a click. `viewport_owns_pointer` decides instead.
- **Tracing tolerances come from the pixel footprint, not the ray distance.**
  Scaling by distance as well turns a 0.02mm threshold into millimetres and
  rounds every edge in the scene into a curve.
- **In headless capture, `RawInput::screen_rect` is in points, not pixels**, and
  several frames must be run with time advancing, applying texture deltas from
  every frame.
- **Under WSL the only hardware GPU path reports itself as non-conformant** and
  crashes if driven off the main thread. See [docs/building.md](docs/building.md).

## Style

**No em dashes.** Anywhere: prose, documentation, code comments, commit messages.
Use a comma, colon, semicolon, parentheses, or a new sentence. Do not substitute a
hyphen doing an em dash's job.

Comments should explain why, not what. The reader can see what the code does. If
a constant was chosen for a reason, or an obvious-looking alternative is wrong,
that is worth a line. Restating the signature is not.

Match the density and idiom of the surrounding code.

## Things not to do

- **Do not reintroduce B-rep concepts.** There are no faces, edges or vertices,
  and no topological naming. If you find yourself wanting them, you are
  contradicting [ADR 0001](docs/adr/0001-implicit-kernel.md); write the next ADR
  instead of quietly diverging.
- **Do not loosen a test threshold until it passes.** If a property fails, either
  the code is wrong or the assertion was. Work out which and say so. If the
  algorithm genuinely cannot make the guarantee, document the limitation against
  a reproducing case rather than weakening the test into meaninglessness. See
  `known_limitation_two_sheets_in_one_cell_are_non_manifold`.
- **Do not add interface elements that do nothing.** A button that lies is worse
  than no button.
- **Do not suppress a lint without a written reason.**
- **Do not claim something works without running it.** Compiling is not passing,
  and passing tests is not the same as having looked at the output.

## Commits

Single-line conventional commit subjects: `feat:`, `fix:`, `docs:`, `test:`,
`refactor:`, `chore:`. Lowercase, imperative, no body, no trailers, no
attribution or co-author lines.

The repository signs commits. If signing is not available in your environment,
say so rather than committing unsigned.
