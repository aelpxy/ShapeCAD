# ShapeCAD

Read [AGENTS.md](AGENTS.md). It holds the project's conventions, invariants,
commands and traps, and applies here in full. This file exists only so the
guidance is not duplicated in two places that drift apart.

Worth repeating, because these are the ones most often missed:

- Every mutation goes through `Document::apply`. Nothing reaches into `Arena`
  from application code.
- Zero clippy warnings before you call anything done. The configuration is
  strict on purpose.
- Verify geometry by hashing, not by looking at a render.
- No em dashes, anywhere.
- Do not loosen a failing assertion to make it pass.
