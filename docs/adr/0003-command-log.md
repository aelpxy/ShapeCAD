# 3. All mutation flows through an id-explicit command log

Status: accepted

## Context

The UI, the headless CLI and the AI agent layer all need to edit documents.
Exposing a separate API to each would let them drift, and would mean an agent
could reach states a user could not.

## Decision

Every mutation goes through `Document::apply`. Commands are transactional: a
rejected command leaves the document byte-identical and unlogged.

The log stores `Effect`, not `Command`. A command is what a caller *expresses*;
an effect is what *happened*, with every node id made explicit.

## Consequences

Undo/redo, replay-based testing, crash recovery and the agent interface are all
the same mechanism seen from different angles, which is why the log was built
before any UI existed.

The command/effect split is not academic. `Command::Add` carries no id and is
assigned one at apply time. Undo leaves a tombstone in the arena, which advances
the id counter, so replaying a sequence of abstract *commands* allocates
different ids than the original run, and every later reference by id then points
at the wrong node or at nothing.

This was found by property testing (`replaying_the_log_is_faithful`) rather than
by review, and is pinned by the regression test
`replay_survives_tombstones_left_by_undo`.

## Undo steps

One command is the wrong unit for undo. Almost every tool is several commands
underneath: a pad is an extrusion, a placement, a label and the boolean that
joins it to the model. Undoing those one at a time walks the user backwards
through states the tool only passed through, which exposes an implementation
detail as an interface.

So entries carry a step number. `Document::begin_step` and `end_step` bracket an
action, `undo` and `redo` consume whole steps, and calls nest so a helper that
groups internally still merges into the larger action that called it. Commands
applied outside a step are their own step, which keeps a dragged parameter
undoable on its own.

Grouping is a property of the session, not of the model. The step number never
reaches the log, because the log is not saved: it is replayable history, and
replay assigns one step per effect. Nothing about the file format changes.

`end_step` without a matching `begin_step` is ignored rather than treated as an
error. The worst outcome of an unbalanced pair is finer-grained undo, which is
not worth ending a user's session over.
