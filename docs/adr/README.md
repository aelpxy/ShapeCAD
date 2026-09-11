# Architecture decision records

Significant choices and the reasoning behind them, so that a decision can be
revisited on its merits rather than rediscovered by accident.

| # | Decision | Status |
|---|---|---|
| [0001](0001-implicit-kernel.md) | Use an implicit (SDF) kernel rather than B-rep | accepted |
| [0002](0002-rust.md) | Write everything in Rust | accepted |
| [0003](0003-command-log.md) | All mutation flows through an id-explicit command log | accepted |
| [0004](0004-selection-by-seam.md) | Selection and filleting work on nodes and seams, not edges | accepted |
| [0005](0005-derived-placements.md) | A placement records what it was derived from, and is regenerated on edit | accepted |
| [0006](0006-sidecar-assets.md) | Imported geometry lives in sidecar binaries, not in the document | accepted |

0001 is the load-bearing one; nearly everything else follows from it. If you are
about to contradict a record, write the next one rather than quietly diverging.
