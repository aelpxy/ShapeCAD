//! `ShapeCAD` document model.
//!
//! A document is an implicit geometry DAG plus an append-only log of the
//! commands that produced it. Undo/redo, replay-based testing, and the AI agent
//! interface are all the same mechanism viewed from different angles, which is
//! why the log is built before any UI exists.

/// The command vocabulary shared by the UI, the CLI and the agent layer.
pub mod command;
/// Document-level error types.
pub mod error;
#[cfg(feature = "serde")]
pub mod file;
pub mod samples;

pub use command::{Command, Effect};
pub use error::{DocError, Result};

use sc_geom::{bounds, wgsl, Aabb, Arena, GeometryHash, Node, NodeId};
use std::collections::HashMap;
use std::fmt::Write as _;

/// One applied mutation plus the effect that reverses it.
///
/// Only effects are retained. The originating [`Command`] is deliberately not
/// stored: it carries strictly less information, and keeping both would allow
/// the two to drift.
#[derive(Debug)]
struct Entry {
    effect: Effect,
    inverse: Effect,
    /// Which user action produced this. Entries sharing a step undo together.
    step: u64,
}

/// An implicit model plus the log of commands that produced it.
#[derive(Debug, Default)]
pub struct Document {
    arena: Arena,
    root: Option<NodeId>,
    names: HashMap<NodeId, String>,
    entries: Vec<Entry>,
    /// Number of entries currently applied. Everything at or past this index is
    /// a redo candidate.
    cursor: usize,
    /// The step entries are currently being recorded under.
    step: u64,
    /// How many [`Document::begin_step`] calls are open. Zero means every
    /// command is its own undo step.
    open: u32,
}

impl Document {
    /// An empty document with no root.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read-only access to the geometry storage.
    #[must_use]
    pub fn arena(&self) -> &Arena {
        &self.arena
    }

    /// The node the document currently renders and exports.
    #[must_use]
    pub fn root(&self) -> Option<NodeId> {
        self.root
    }

    /// The label attached to a node, if any.
    #[must_use]
    pub fn name(&self, id: NodeId) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    /// Conservative bounds of the rooted model.
    #[must_use]
    pub fn bounds(&self) -> Option<Aabb> {
        self.root.map(|r| bounds(&self.arena, r))
    }

    /// Deterministic digest of the rooted model, for regression testing.
    #[must_use]
    pub fn hash(&self) -> Option<GeometryHash> {
        self.root.map(|r| sc_geom::geometry_hash(&self.arena, r))
    }

    /// The generated shader source for the current model.
    #[must_use]
    pub fn wgsl(&self) -> wgsl::Generated {
        wgsl::generate(&self.arena, self.root)
    }

    /// The effects that produced the current state, oldest first. Excludes
    /// anything sitting in the redo tail.
    ///
    /// This is the durable, replayable form of the document's history; feed it
    /// to [`Document::replay`] to reconstruct an identical model.
    pub fn log(&self) -> impl Iterator<Item = &Effect> {
        self.entries[..self.cursor].iter().map(|e| &e.effect)
    }

    /// Number of applied mutations.
    #[must_use]
    pub fn log_len(&self) -> usize {
        self.cursor
    }

    /// Rebuilds a document by replaying a log produced by [`Document::log`].
    ///
    /// Because effects name every id explicitly, the result is identical to the
    /// original, including id assignment, which abstract commands cannot
    /// guarantee once undo has left tombstones in the arena.
    ///
    /// # Errors
    /// [`DocError::Geom`] if the log is not self-consistent, which would mean it
    /// did not come from a real document.
    pub fn replay(ops: impl IntoIterator<Item = Effect>) -> Result<Self> {
        let mut doc = Self::new();
        for effect in ops {
            let inverse = doc.invert(&effect)?;
            doc.run(effect.clone())?;
            doc.step += 1;
            doc.entries.push(Entry {
                effect,
                inverse,
                step: doc.step,
            });
            doc.cursor += 1;
        }
        Ok(doc)
    }

    /// Whether there is an applied command to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    /// Whether there is an undone command to reapply.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.cursor < self.entries.len()
    }

    /// Apply a command, returning the node it created or touched.
    ///
    /// On error nothing is mutated and nothing is logged: the document is
    /// exactly as it was. Commands are all-or-nothing so a failed agent action
    /// or a rejected drag can never leave a half-edited model behind.
    ///
    /// # Errors
    /// [`DocError::UnknownParam`] for an unknown parameter name,
    /// [`DocError::IsRoot`] when deleting the root, or [`DocError::Geom`] for
    /// any validation failure in the underlying geometry edit.
    pub fn apply(&mut self, command: Command) -> Result<Option<NodeId>> {
        let effect = self.lower(command)?;
        let inverse = self.invert(&effect)?;
        let out = self.run(effect.clone())?;

        // A fresh edit invalidates the redo tail.
        self.entries.truncate(self.cursor);
        if self.open == 0 {
            self.step += 1;
        }
        self.entries.push(Entry {
            effect,
            inverse,
            step: self.step,
        });
        self.cursor += 1;
        Ok(out)
    }

    /// Starts grouping: every command applied until the matching
    /// [`Document::end_step`] undoes and redoes as one.
    ///
    /// One thing a user did should take one press of undo to take back. Most
    /// editing actions are several commands underneath, because a feature is a
    /// node plus its placement plus the boolean that joins it to the model, and
    /// making the user unwind those one at a time would expose an implementation
    /// detail as an interface.
    ///
    /// Calls nest. Only the outermost pair opens a step, so a helper that groups
    /// internally still merges into a larger action that calls it.
    pub fn begin_step(&mut self) {
        if self.open == 0 {
            self.step += 1;
        }
        self.open += 1;
    }

    /// Closes the step opened by [`Document::begin_step`].
    ///
    /// Unbalanced calls are ignored rather than panicking: the worst outcome of
    /// a missing `begin_step` is finer-grained undo, which is not worth aborting
    /// a user's session over.
    pub fn end_step(&mut self) {
        self.open = self.open.saturating_sub(1);
    }

    /// Reverses the most recent command. Returns false if there is nothing to undo.
    ///
    /// # Errors
    /// [`DocError::Geom`] only if the arena rejects the inverse edit, which
    /// would indicate a bug in inverse construction rather than user error.
    pub fn undo(&mut self) -> Result<bool> {
        if !self.can_undo() {
            return Ok(false);
        }
        // Walk back over every entry the same action produced. Applied in
        // reverse, because a step builds a node before referring to it.
        let step = self.entries[self.cursor - 1].step;
        while self.cursor > 0 && self.entries[self.cursor - 1].step == step {
            let inverse = self.entries[self.cursor - 1].inverse.clone();
            self.run(inverse)?;
            self.cursor -= 1;
        }
        Ok(true)
    }

    /// Reapplies the most recently undone command. Returns false if there is
    /// nothing to redo.
    ///
    /// # Errors
    /// [`DocError::Geom`] only if the arena rejects the replayed edit.
    pub fn redo(&mut self) -> Result<bool> {
        if !self.can_redo() {
            return Ok(false);
        }
        let step = self.entries[self.cursor].step;
        while self.cursor < self.entries.len() && self.entries[self.cursor].step == step {
            let effect = self.entries[self.cursor].effect.clone();
            self.run(effect)?;
            self.cursor += 1;
        }
        Ok(true)
    }

    /// Resolves a command into a concrete effect, validating as we go.
    ///
    /// Consumes the command so nodes and names are moved into the effect rather
    /// than cloned.
    fn lower(&self, command: Command) -> Result<Effect> {
        Ok(match command {
            Command::Add { node } => Effect::Create {
                id: self.arena.next_id(),
                node,
            },
            Command::Replace { id, node } => Effect::Replace { id, node },
            Command::SetParam { id, name, value } => {
                let mut node = self.arena.try_get(id)?.clone();
                if !node.set_param(&name, value) {
                    return Err(DocError::UnknownParam { id, name });
                }
                Effect::Replace { id, node }
            }
            Command::Delete { id } => {
                if self.root == Some(id) {
                    return Err(DocError::IsRoot(id));
                }
                self.arena.try_get(id)?;
                Effect::Destroy { id }
            }
            Command::SetRoot { root } => {
                if let Some(r) = root {
                    self.arena.try_get(r)?;
                }
                Effect::SetRoot { root }
            }
            Command::SetName { id, name } => {
                self.arena.try_get(id)?;
                Effect::SetName { id, name }
            }
        })
    }

    /// Build the effect that undoes `effect`, reading the state it is about to
    /// overwrite. Must be called before the effect is run.
    fn invert(&self, effect: &Effect) -> Result<Effect> {
        Ok(match effect {
            Effect::Create { id, .. } => Effect::Destroy { id: *id },
            Effect::Replace { id, .. } => Effect::Replace {
                id: *id,
                node: self.arena.try_get(*id)?.clone(),
            },
            Effect::Destroy { id } => Effect::Create {
                id: *id,
                node: self.arena.try_get(*id)?.clone(),
            },
            Effect::SetRoot { .. } => Effect::SetRoot { root: self.root },
            Effect::SetName { id, .. } => Effect::SetName {
                id: *id,
                name: self.names.get(id).cloned(),
            },
        })
    }

    fn run(&mut self, effect: Effect) -> Result<Option<NodeId>> {
        Ok(match effect {
            Effect::Create { id, node } => {
                self.arena.create_at(id, node)?;
                Some(id)
            }
            Effect::Replace { id, node } => {
                self.arena.replace(id, node)?;
                Some(id)
            }
            Effect::Destroy { id } => {
                // The name is deliberately left behind. Ids are never recycled
                // except by undoing this very deletion, in which case the name
                // coming back with the node is exactly what you want.
                self.arena.remove(id)?;
                Some(id)
            }
            Effect::SetRoot { root } => {
                self.root = root;
                root
            }
            Effect::SetName { id, name } => {
                match name {
                    Some(n) => self.names.insert(id, n),
                    None => self.names.remove(&id),
                };
                Some(id)
            }
        })
    }

    /// A compact, human- and model-readable rendering of the tree.
    ///
    /// This is the representation the agent layer will hand to a model: the
    /// whole design in a few hundred tokens, with the stable ids it needs to
    /// address anything it wants to change.
    pub fn outline(&self) -> String {
        let mut out = String::new();
        match self.root {
            None => out.push_str("(empty document)\n"),
            Some(r) => {
                let mut seen = Vec::new();
                self.outline_node(r, 0, &mut seen, &mut out);
            }
        }
        out
    }

    fn outline_node(&self, id: NodeId, depth: usize, seen: &mut Vec<NodeId>, out: &mut String) {
        for _ in 0..depth {
            out.push_str("  ");
        }
        let Some(node) = self.arena.get(id) else {
            let _ = writeln!(out, "{id} <deleted>");
            return;
        };

        let _ = write!(out, "{id} {}", node.kind());
        if let Some(n) = self.name(id) {
            let _ = write!(out, " \"{n}\"");
        }
        for (k, v) in node.params() {
            let _ = write!(out, " {k}={v}");
        }

        // A shared subtree is printed once; later references are marked so the
        // outline stays linear in the size of the DAG, not its unrolling.
        if seen.contains(&id) {
            out.push_str(" (shared, shown above)\n");
            return;
        }
        seen.push(id);
        out.push('\n');

        for c in node.children() {
            self.outline_node(c, depth + 1, seen, out);
        }
    }
}

/// Convenience: add a node and return its id.
///
/// # Errors
/// Propagates any [`DocError`] from [`Document::apply`].
///
/// # Panics
/// If [`Command::Add`] ever returns without an id, which would be a bug in
/// [`Document::apply`] rather than a condition a caller can trigger.
pub fn add(doc: &mut Document, node: Node) -> Result<NodeId> {
    Ok(doc
        .apply(Command::Add { node })?
        .expect("Add always yields an id"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sc_geom::glam::Vec3;
    use sc_geom::Node;

    fn sphere(r: f32) -> Node {
        Node::Sphere { radius: r }
    }

    /// A feature is several commands underneath, so one press of undo has to
    /// take back all of them. Without grouping the user is left holding the
    /// half-built model the tool passed through on its way.
    #[test]
    fn a_step_undoes_and_redoes_as_one() {
        let mut doc = Document::new();
        let base = add(&mut doc, sphere(5.0)).unwrap();
        doc.apply(Command::SetRoot { root: Some(base) }).unwrap();
        let before = doc.hash().unwrap();

        doc.begin_step();
        let extra = add(&mut doc, sphere(2.0)).unwrap();
        let union = add(
            &mut doc,
            Node::Union {
                a: base,
                b: extra,
                smooth: 0.0,
            },
        )
        .unwrap();
        doc.apply(Command::SetRoot { root: Some(union) }).unwrap();
        doc.apply(Command::SetName {
            id: extra,
            name: Some("Boss".into()),
        })
        .unwrap();
        doc.end_step();
        let after = doc.hash().unwrap();

        assert!(doc.undo().unwrap());
        assert_eq!(
            doc.hash().unwrap(),
            before,
            "one undo left part of the step applied"
        );
        assert!(!doc.arena().is_alive(union), "the union survived the undo");
        assert!(!doc.arena().is_alive(extra), "the body survived the undo");

        assert!(doc.redo().unwrap());
        assert_eq!(
            doc.hash().unwrap(),
            after,
            "one redo rebuilt only part of it"
        );
        assert_eq!(doc.name(extra), Some("Boss"), "the label was not restored");
    }

    /// Commands applied outside a step keep undoing one at a time, so a dragged
    /// parameter is still its own step.
    #[test]
    fn ungrouped_commands_stay_separate() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(5.0)).unwrap();
        doc.apply(Command::SetRoot { root: Some(a) }).unwrap();
        assert_eq!(doc.log_len(), 2);

        assert!(doc.undo().unwrap());
        assert_eq!(doc.log_len(), 1, "two separate commands undid together");
    }

    /// Steps nest, so a helper that groups internally merges into the larger
    /// action that calls it rather than ending it early.
    #[test]
    fn nested_steps_stay_one_step() {
        let mut doc = Document::new();
        doc.begin_step();
        let a = add(&mut doc, sphere(1.0)).unwrap();
        doc.begin_step();
        add(&mut doc, sphere(2.0)).unwrap();
        doc.end_step();
        doc.apply(Command::SetRoot { root: Some(a) }).unwrap();
        doc.end_step();

        assert!(doc.undo().unwrap());
        assert_eq!(doc.log_len(), 0, "the inner step split the outer one");
    }

    /// An unbalanced close must not swallow everything that follows into one
    /// permanent step.
    #[test]
    fn an_unmatched_end_step_does_not_leave_grouping_on() {
        let mut doc = Document::new();
        doc.end_step();
        let a = add(&mut doc, sphere(1.0)).unwrap();
        doc.apply(Command::SetRoot { root: Some(a) }).unwrap();

        assert!(doc.undo().unwrap());
        assert_eq!(doc.log_len(), 1, "later commands were grouped together");
    }

    /// A step started and abandoned without any command must not merge with the
    /// next one.
    #[test]
    fn an_empty_step_does_not_absorb_the_next_command() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(1.0)).unwrap();
        doc.begin_step();
        doc.end_step();
        doc.apply(Command::SetRoot { root: Some(a) }).unwrap();

        assert!(doc.undo().unwrap());
        assert_eq!(doc.log_len(), 1, "an empty step merged two commands");
    }

    #[test]
    fn undo_redo_restores_exact_geometry() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(5.0)).unwrap();
        doc.apply(Command::SetRoot { root: Some(a) }).unwrap();
        let before = doc.hash().unwrap();

        doc.apply(Command::SetParam {
            id: a,
            name: "radius".into(),
            value: 9.0,
        })
        .unwrap();
        assert_ne!(doc.hash().unwrap(), before);

        assert!(doc.undo().unwrap());
        assert_eq!(
            doc.hash().unwrap(),
            before,
            "undo did not restore the shape"
        );

        assert!(doc.redo().unwrap());
        assert_ne!(doc.hash().unwrap(), before, "redo did not reapply the edit");
    }

    #[test]
    fn undoing_a_delete_restores_the_same_id() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(1.0)).unwrap();
        doc.apply(Command::Delete { id: a }).unwrap();
        assert!(!doc.arena().is_alive(a));

        doc.undo().unwrap();
        assert!(
            doc.arena().is_alive(a),
            "id must come back, not be reissued"
        );
        assert_eq!(doc.arena().try_get(a).unwrap(), &sphere(1.0));
    }

    #[test]
    fn redoing_an_add_reuses_the_original_id() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(1.0)).unwrap();
        doc.undo().unwrap();
        doc.redo().unwrap();
        assert!(doc.arena().is_alive(a), "redo must not reissue a fresh id");
        assert_eq!(doc.arena().capacity(), 1, "no id was burned");
    }

    #[test]
    fn a_new_edit_discards_the_redo_tail() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(1.0)).unwrap();
        doc.apply(Command::SetParam {
            id: a,
            name: "radius".into(),
            value: 2.0,
        })
        .unwrap();
        doc.undo().unwrap();
        assert!(doc.can_redo());

        doc.apply(Command::SetParam {
            id: a,
            name: "radius".into(),
            value: 3.0,
        })
        .unwrap();
        assert!(!doc.can_redo(), "stale redo tail survived a new edit");
    }

    #[test]
    fn failed_commands_leave_no_trace() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(1.0)).unwrap();
        let before = doc.hash();
        let history_len = doc.log_len();

        assert!(doc
            .apply(Command::SetParam {
                id: a,
                name: "bogus".into(),
                value: 1.0
            })
            .is_err());
        assert!(doc
            .apply(Command::Replace {
                id: a,
                node: sphere(-5.0)
            })
            .is_err());

        assert_eq!(doc.hash(), before, "document mutated despite failure");
        assert_eq!(doc.log_len(), history_len, "failed command was logged");
    }

    #[test]
    fn the_root_cannot_be_deleted_out_from_under_the_document() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(1.0)).unwrap();
        doc.apply(Command::SetRoot { root: Some(a) }).unwrap();
        assert!(matches!(
            doc.apply(Command::Delete { id: a }),
            Err(DocError::IsRoot(_))
        ));
    }

    #[test]
    fn replay_of_the_log_reproduces_the_model() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(4.0)).unwrap();
        let b = add(
            &mut doc,
            Node::Box {
                half: Vec3::splat(3.0),
                round: 0.0,
            },
        )
        .unwrap();
        let u = add(&mut doc, Node::Union { a, b, smooth: 0.8 }).unwrap();
        doc.apply(Command::SetRoot { root: Some(u) }).unwrap();
        doc.apply(Command::SetName {
            id: u,
            name: Some("body".into()),
        })
        .unwrap();

        let log: Vec<Effect> = doc.log().cloned().collect();
        let replayed = Document::replay(log).unwrap();
        assert_eq!(replayed.hash(), doc.hash(), "replaying the log diverged");
        assert_eq!(replayed.outline(), doc.outline());
    }

    /// Regression: undo leaves a tombstone, which advances the id counter. A log
    /// of abstract commands would re-allocate different ids on replay and every
    /// later reference would point at the wrong node. Found by property testing.
    #[test]
    fn replay_survives_tombstones_left_by_undo() {
        let mut doc = Document::new();
        let _a = add(&mut doc, sphere(1.0)).unwrap();
        let b = add(&mut doc, sphere(2.0)).unwrap();
        doc.undo().unwrap(); // destroys `b`, but the id counter stays advanced
        let c = add(&mut doc, sphere(3.0)).unwrap();
        assert_ne!(b, c, "id counter must not rewind");
        doc.apply(Command::SetRoot { root: Some(c) }).unwrap();

        let replayed = Document::replay(doc.log().cloned().collect::<Vec<_>>()).unwrap();
        assert_eq!(replayed.root(), doc.root(), "root id diverged on replay");
        assert_eq!(replayed.hash(), doc.hash());
    }

    #[test]
    fn outline_is_compact_and_marks_shared_subtrees() {
        let mut doc = Document::new();
        let s = add(&mut doc, sphere(1.0)).unwrap();
        let u = add(
            &mut doc,
            Node::Union {
                a: s,
                b: s,
                smooth: 0.0,
            },
        )
        .unwrap();
        doc.apply(Command::SetRoot { root: Some(u) }).unwrap();
        let text = doc.outline();
        assert!(text.contains("union"), "{text}");
        assert_eq!(text.matches("sphere").count(), 2, "{text}");
        assert!(text.contains("shared"), "{text}");
    }
}
