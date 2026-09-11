# ShapeCAD documentation

Reference material for working on ShapeCAD. For what the project is and how to run
it, see the [top-level README](../README.md); for how to contribute, see
[CONTRIBUTING](../CONTRIBUTING.md).

| Document | Covers |
|---|---|
| [architecture.md](architecture.md) | How the crates fit together and how data flows between them |
| [kernel.md](kernel.md) | The implicit geometry kernel: node semantics, invariants, adding a node kind |
| [rendering.md](rendering.md) | Sphere tracing, shader generation, GPU selection |
| [meshing.md](meshing.md) | Dual contouring, the QEF, and what the mesher does and does not guarantee |
| [file-format.md](file-format.md) | The `.shapecad` document format |
| [interface.md](interface.md) | The design language, the icon set, and the rule that every control explains itself |
| [building.md](building.md) | Toolchain, system dependencies, and platform-specific traps |
| [adr/](adr/) | Architecture decision records: why things are the way they are |

## Where to start

Read [architecture.md](architecture.md) first. It is short, and everything else
assumes it. If you are changing geometry, read [kernel.md](kernel.md) next; if you
are changing what appears on screen, read [rendering.md](rendering.md) for the
3D view and [interface.md](interface.md) for the chrome around it.

Two decisions explain most of the design and are worth reading before disagreeing
with anything: [ADR 0001](adr/0001-implicit-kernel.md) on why geometry is implicit
rather than a boundary representation, and [ADR 0003](adr/0003-command-log.md) on
why every edit goes through a command log.
