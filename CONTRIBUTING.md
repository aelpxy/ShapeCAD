# Contributing to ShapeCAD

## Getting set up

```sh
cargo test --workspace     # everything
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

You need a C linker (`cc`) even though there is no C in the project: Rust
requires one. On Arch: `pacman -S gcc`.

## The two rules that matter

**Every mutation goes through `Document::apply`.** The UI, the CLI and the agent
layer share one command vocabulary. If you find yourself reaching into `Arena`
from application code to change something, stop. That path exists for the
document model and for tests, not for features. See [ADR 0003](docs/adr/0003-command-log.md).

**Commands are all-or-nothing.** Validate before you mutate. A command that can
fail halfway breaks undo, replay and every agent that edits speculatively.

## Testing standard

The lint configuration is strict on purpose: `clippy::pedantic`, `missing_docs`,
and `unsafe_code = "forbid"` workspace-wide. CI treats warnings as errors. Public
items need real documentation, not restatements of their signature.

Three kinds of test, and new work is expected to carry the right ones:

**Example tests** (`#[test]` in `src/`) pin down specific behaviour and read as
documentation. Name them as the claim they make, such as `shell_hollows_inward_and_keeps_the_outer_surface`,
not `test_shell`.

**Property tests** (`tests/properties.rs`) pin down invariants that must hold for
*every* input. This is where the kernel's real guarantees live: that the field is
never NaN, that it is 1-Lipschitz, that bounds never exclude solid material, that
undo is exact. Adding a node kind means checking it against the existing
properties, not just adding an example.

If a property test fails, proptest writes the shrunken counterexample to
`tests/proptest-regressions/`. **Commit that file.** It becomes a permanent
regression case.

**Golden geometry** (`cargo run -p sc-cli -- selftest`) hashes a reference part.
A change here means the part came out a different shape. If the change is
intended, update `tests/corpus/bracket.hash` in the same commit and say why.

## Verifying geometry

Do not eyeball renders to check correctness. Use `sc_geom::geometry_hash`, which
returns three separate digests (structure, field and bounds), so a failure tells
you *what* moved rather than just that something did.

The headless CLI exists so that tests and AI agents can drive and verify the
application without a window. Keep it capable of everything the GUI can do.

## Commits

Single-line conventional commit subjects: `feat:`, `fix:`, `docs:`, `test:`,
`refactor:`, `chore:`. Lowercase, imperative, no body.

## Architecture decisions

Significant choices are recorded in [`docs/adr/`](docs/adr/). If you are about to
contradict one, write the next ADR rather than quietly diverging.
