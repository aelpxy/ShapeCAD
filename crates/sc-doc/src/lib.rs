//! `ShapeCAD` document model.
//!
//! A document is an implicit geometry DAG plus an append-only log of the
//! commands that produced it. Undo/redo, replay-based testing, and the AI agent
//! interface are all the same mechanism viewed from different angles, which is
//! why the log is built before any UI exists.

/// Voxel grid assets and the sidecar directory they are stored in.
pub mod asset;
/// The command vocabulary shared by the UI, the CLI and the agent layer.
pub mod command;
/// Document-level error types.
pub mod error;
#[cfg(feature = "serde")]
pub mod file;
mod mesh;
pub mod samples;

pub use asset::{AssetId, AssetStore, Grid};
pub use command::{Command, Effect};
pub use error::{AttachError, DocError, Result};

use sc_geom::{bounds, wgsl, Aabb, Arena, GeometryHash, Node, NodeId};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;

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
    /// The voxel grids this document's mesh nodes refer to. Not part of the
    /// model: the nodes carry their own grids, and this is the index that
    /// makes them findable by id when saving and loading.
    assets: AssetStore,
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

    /// The voxel grids this document's mesh nodes refer to.
    #[must_use]
    pub fn assets(&self) -> &AssetStore {
        &self.assets
    }

    /// Registers a grid and returns the id a mesh node should reference it by.
    ///
    /// This is not a mutation of the model, so it deliberately does not go
    /// through [`Document::apply`] and is not logged: the arena is untouched and
    /// the document still hashes exactly as it did. The mutation is the
    /// [`Command::Add`] of the node that references the grid, which must follow.
    /// An id that no node ever references holds nothing alive: it is collected
    /// at the next garbage collection and is never written to disk.
    pub fn import_grid(&mut self, grid: Grid) -> AssetId {
        self.assets.insert(grid)
    }

    /// Imports a grid and adds the mesh node that uses it.
    ///
    /// # Errors
    /// Propagates any [`DocError`] from [`Document::apply`], which for a mesh
    /// means the grid did not describe a usable field.
    ///
    /// # Panics
    /// Never in practice; the grid is inserted immediately before it is read
    /// back.
    pub fn add_mesh(&mut self, grid: Grid) -> Result<NodeId> {
        let asset = self.assets.insert(grid);
        let stored = Arc::clone(
            self.assets
                .get(asset)
                .expect("the grid was just inserted under this id"),
        );
        add(self, mesh::mesh_node(asset, stored))
    }

    /// The assets a saved copy of this document would need: those named by a
    /// node that is still live in the arena.
    ///
    /// This is the write rule. A tombstoned node is not written to the file, so
    /// its grid cannot be reached from the file either, and writing it would put
    /// megabytes of unreachable binary beside every part that has ever had a
    /// mesh deleted from it.
    #[must_use]
    pub fn referenced_assets(&self) -> BTreeSet<AssetId> {
        self.arena
            .live_ids()
            .filter_map(|id| self.arena.get(id).and_then(mesh::asset_of))
            .collect()
    }

    /// The assets this session must keep in memory.
    ///
    /// Wider than [`Document::referenced_assets`] by exactly the undo log. A
    /// deleted mesh node lives on inside the `Destroy` entry's inverse, so undo
    /// can put it back, and dropping its grid would make a perfectly ordinary
    /// press of undo resurrect a node with no geometry in it. The whole of
    /// `entries` counts, not just the applied prefix, because the redo tail is
    /// reachable too.
    fn reachable_assets(&self) -> BTreeSet<AssetId> {
        let mut keep = self.referenced_assets();
        for entry in &self.entries {
            for effect in [&entry.effect, &entry.inverse] {
                if let Some(asset) = effect.node().and_then(mesh::asset_of) {
                    keep.insert(asset);
                }
            }
        }
        keep
    }

    /// Drops grids nothing can reach any more.
    ///
    /// Only worth running when the redo tail has just been discarded, because
    /// that is the one moment an asset stops being reachable. Undo and redo move
    /// the cursor without shortening `entries`, so nothing dies there.
    fn collect_assets(&mut self) {
        if self.assets.is_empty() {
            return;
        }
        let keep = self.reachable_assets();
        self.assets.retain(&keep);
    }

    /// Fills in the grid of every mesh node from `store`, and adopts it.
    ///
    /// A mesh node deserialises to an empty placeholder, because its grid is
    /// megabytes of binary that the JSON does not carry. This is the step that
    /// makes such a document real, and it is part of construction rather than an
    /// edit, so it is deliberately not routed through [`Document::apply`]: an
    /// opened file has no undo history, and filling a hole is not something a
    /// user did.
    ///
    /// # Errors
    /// [`AttachError`] if a node's grid is absent or unusable. Failing here is
    /// the point: a document that finished loading with a placeholder still in
    /// it would render and export as silently empty geometry.
    pub fn attach_assets(&mut self, store: AssetStore) -> std::result::Result<(), AttachError> {
        self.assets = store;
        for id in self.arena.live_ids().collect::<Vec<_>>() {
            let Some(node) = self.arena.get(id) else {
                continue;
            };
            let Some(asset) = mesh::asset_of(node) else {
                continue;
            };
            if !mesh::is_placeholder(node) {
                continue;
            }
            // An empty grid is not a grid; accepting one would put the
            // placeholder back under a different name.
            let grid = match self.assets.get(asset) {
                Some(g) if !g.data.is_empty() => Arc::clone(g),
                _ => return Err(AttachError::Unresolved { node: id, asset }),
            };
            let Some(filled) = mesh::with_grid(node, grid) else {
                continue;
            };
            self.arena
                .replace(id, filled)
                .map_err(|cause| AttachError::Invalid {
                    node: id,
                    asset,
                    cause,
                })?;
        }
        Ok(())
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
        // Set the redo tail aside. A fresh edit invalidates it, but a rejected
        // one has to leave it exactly where it was.
        let tail = self.entries.split_off(self.cursor);
        let mark = self.cursor;

        // The edit and whatever it drags along with it are one action. A user
        // who shortens a base and watches the boss on top follow it down has
        // done one thing, and should press undo once to take it back.
        self.begin_step();
        let out = self.commit(effect).and_then(|out| {
            self.regenerate()?;
            Ok(out)
        });
        self.end_step();

        // An edit is no longer a single effect: regeneration follows it. If that
        // fails the edit has already landed, so it has to be taken back out
        // again, or a rejected command leaves behind exactly the half-edited
        // model the guarantee above exists to rule out.
        if out.is_err() {
            self.rewind(mark);
            self.entries.extend(tail);
        } else if !tail.is_empty() {
            // The edit stuck, so the redo tail is gone for good, and with it the
            // only thing that was keeping the grids named in it reachable.
            self.collect_assets();
        }
        out
    }

    /// Unwinds applied entries back to `mark`, discarding them.
    ///
    /// Every entry carries its own inverse, so this is undo without the history:
    /// the model returns to where it was and no record is left that it ever
    /// moved.
    fn rewind(&mut self, mark: usize) {
        while self.cursor > mark {
            let inverse = self.entries[self.cursor - 1].inverse.clone();
            // An inverse built from state that existed a moment ago can only be
            // rejected by a bug in inverse construction, and there is no better
            // recovery available here than to keep unwinding and leave the model
            // as close to untouched as it can be.
            let _ = self.run(inverse);
            self.cursor -= 1;
        }
        self.entries.truncate(self.cursor);
    }

    /// Runs an effect and logs it under the step currently open.
    ///
    /// Every mutation [`Document::apply`] makes goes through here, the
    /// regeneration that follows an edit included, so nothing can change the
    /// model without leaving a replayable record and an inverse behind.
    fn commit(&mut self, effect: Effect) -> Result<Option<NodeId>> {
        let inverse = self.invert(&effect)?;
        let out = self.run(effect.clone())?;

        // A fresh edit invalidates the redo tail. Collecting the grids it was
        // keeping alive is not done here: `apply` sets the tail aside so it can
        // be put back if the edit fails, so by this point there is nothing left
        // to truncate and nothing has actually been given up yet.
        self.entries.truncate(self.cursor);
        self.entries.push(Entry {
            effect,
            inverse,
            step: self.step,
        });
        self.cursor += 1;
        Ok(out)
    }

    /// Puts every derived placement back on the face it was built on.
    ///
    /// Run at the end of [`Document::apply`], inside the same step. The moves
    /// are emitted as ordinary [`Effect::Replace`]s rather than applied behind
    /// the log's back, so history stays the whole story: [`Document::replay`]
    /// feeds those effects straight to `run` and must not regenerate again on
    /// top of them, or it would be deriving from a model it is only half way
    /// through rebuilding.
    ///
    /// Derived placements chain, and moving one moves the face the next sits
    /// on, so this iterates. Each pass settles at least the shallowest
    /// placement that is still stale, which bounds the work by the number of
    /// derived placements and stops a malformed document spinning here.
    fn regenerate(&mut self) -> Result<()> {
        let Some(root) = self.root else {
            return Ok(());
        };
        let derived: Vec<NodeId> = self
            .arena
            .live_ids()
            .filter(|&id| self.arena.get(id).and_then(Node::derived_from).is_some())
            .collect();

        for _ in 0..derived.len() {
            let mut moved = false;
            for &id in &derived {
                if let Some(effect) = self.regenerated(root, id) {
                    self.commit(effect)?;
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        Ok(())
    }

    /// The effect that puts one derived placement back on its face, or `None`
    /// if it is already there.
    ///
    /// Also `None` when either end has no position: a feature detached from the
    /// model is not wrong, it is simply nowhere, and guessing a placement for it
    /// would move it the moment it was joined back on.
    fn regenerated(&self, root: NodeId, id: NodeId) -> Option<Effect> {
        let node = self.arena.get(id)?;
        let on = node.derived_from()?;
        let Node::Transform { child, xform, .. } = node else {
            return None;
        };

        let face = sc_geom::pick::face_placement(&self.arena, root, on)?;
        // A placement is expressed in its parent's frame rather than the
        // world's, so anything above it has to be divided back out. Skipping
        // this double-applies an enclosing move.
        let above = sc_geom::pick::placement_of(&self.arena, root, id)?;
        let wanted = face.then(&above.inverse());
        if settled(xform, &wanted) {
            return None;
        }

        Some(Effect::Replace {
            id,
            node: Node::Transform {
                child: *child,
                xform: wanted,
                on: Some(on),
            },
        })
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
        let mut at = self.cursor;
        let mut effects = Vec::new();
        while at > 0 && self.entries[at - 1].step == step {
            effects.push(self.entries[at - 1].inverse.clone());
            at -= 1;
        }
        self.run_together(effects)?;
        self.cursor = at;
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
        let mut at = self.cursor;
        let mut effects = Vec::new();
        while at < self.entries.len() && self.entries[at].step == step {
            effects.push(self.entries[at].effect.clone());
            at += 1;
        }
        self.run_together(effects)?;
        self.cursor = at;
        Ok(true)
    }

    /// Runs a whole step's worth of effects as one, putting back whatever
    /// landed if any of them is refused.
    ///
    /// Undo and redo move over a whole step, so a refusal part way through
    /// would leave the user looking at a state their action never passed
    /// through, with the cursor claiming the step was consumed. The reversing
    /// effects are read one at a time, because an inverse can only be built
    /// from the state the effect is about to overwrite.
    ///
    /// Nothing reachable today can make this fail: an effect that ran once ran
    /// against the same arena it is being replayed against. It exists because
    /// the alternative to unwinding is a half-applied step, and an all-or-
    /// nothing guarantee with one path out of it is not a guarantee.
    fn run_together(&mut self, effects: Vec<Effect>) -> Result<()> {
        let mut landed = Vec::with_capacity(effects.len());
        for effect in effects {
            let back = match self.invert(&effect) {
                Ok(back) => back,
                Err(e) => return Err(self.unwind(landed, e)),
            };
            if let Err(e) = self.run(effect) {
                return Err(self.unwind(landed, e));
            }
            landed.push(back);
        }
        Ok(())
    }

    /// Reverses effects that have already run, newest first, and hands back the
    /// error that stopped them.
    ///
    /// Failures here are ignored for the same reason [`Document::rewind`]
    /// ignores them: an inverse built from state that existed a moment ago can
    /// only be refused by a bug in inverse construction, and there is no better
    /// recovery than to keep unwinding and report the original cause.
    fn unwind(&mut self, mut landed: Vec<Effect>, cause: DocError) -> DocError {
        while let Some(back) = landed.pop() {
            let _ = self.run(back);
        }
        cause
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
                Self::register(&mut self.assets, &self.arena, id);
                Some(id)
            }
            Effect::Replace { id, node } => {
                self.arena.replace(id, node)?;
                Self::register(&mut self.assets, &self.arena, id);
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

    /// Indexes the grid a node carries, if it carries one.
    ///
    /// The store is maintained from the nodes rather than alongside them, so a
    /// caller that builds a mesh node by hand and applies [`Command::Add`] gets
    /// a saveable document without having to know the store exists. Takes the
    /// two fields rather than `&mut self` because it reads one and writes the
    /// other.
    fn register(assets: &mut AssetStore, arena: &Arena, id: NodeId) {
        let Some(node) = arena.get(id) else {
            return;
        };
        let (Some(asset), Some(grid)) = (mesh::asset_of(node), mesh::grid_of(node)) else {
            return;
        };
        // A node straight out of the JSON carries an empty placeholder. Priming
        // the store with the hole is exactly what would stop a load noticing
        // that the real grid never arrived.
        if grid.data.is_empty() || assets.contains(asset) {
            return;
        }
        assets.insert_at(asset, Arc::clone(grid));
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

/// Whether a recomputed placement is close enough to the one in the tree to
/// leave alone.
///
/// Compared at the hashing quantum, a tenth of a micrometre: below that the
/// document cannot tell the two apart anyway, and logging the difference would
/// put an effect in the history for every command that changed nothing.
fn settled(current: &sc_geom::Transform, wanted: &sc_geom::Transform) -> bool {
    let close = |a: f32, b: f32| (a - b).abs() <= sc_geom::hash::QUANTUM;
    close(current.translation.x, wanted.translation.x)
        && close(current.translation.y, wanted.translation.y)
        && close(current.translation.z, wanted.translation.z)
        && close(current.scale, wanted.scale)
        && current
            .rotation
            .to_array()
            .iter()
            .zip(wanted.rotation.to_array())
            .all(|(a, b)| close(*a, b))
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

    /// Regeneration runs after the edit that caused it, so a failure there has
    /// to take the edit back out with it. The unwinding is tested directly
    /// because the failure that would trigger it is not reachable today, and an
    /// untested recovery path is one that stops working without anyone noticing.
    #[test]
    fn unwinding_a_step_restores_both_the_model_and_the_log() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(5.0)).unwrap();
        doc.apply(Command::SetRoot { root: Some(a) }).unwrap();
        let before = doc.hash().unwrap();
        let mark = doc.log_len();

        doc.begin_step();
        let b = add(&mut doc, sphere(2.0)).unwrap();
        let u = add(&mut doc, Node::Union { a, b, smooth: 0.0 }).unwrap();
        doc.apply(Command::SetRoot { root: Some(u) }).unwrap();
        doc.end_step();
        assert_ne!(
            doc.hash().unwrap(),
            before,
            "the edit did not reach the model"
        );

        doc.rewind(mark);

        assert_eq!(
            doc.hash().unwrap(),
            before,
            "unwinding left the model changed"
        );
        assert_eq!(doc.log_len(), mark, "unwinding left entries applied");
        assert!(!doc.arena().is_alive(b), "unwinding left a node behind");
        assert!(!doc.arena().is_alive(u), "unwinding left a node behind");
        assert!(
            !doc.can_redo(),
            "an unwound entry stayed behind as a redo candidate"
        );
    }

    /// Undo and redo move over a whole step, so a refusal part way through one
    /// must not leave half of it applied. Tested directly, because no sequence
    /// of commands can produce an effect the arena would refuse to replay, and
    /// an untested recovery path is one that stops working without anyone
    /// noticing.
    #[test]
    fn a_refused_effect_leaves_the_rest_of_the_step_unapplied() {
        let mut doc = Document::new();
        let a = add(&mut doc, sphere(5.0)).unwrap();
        doc.apply(Command::SetRoot { root: Some(a) }).unwrap();
        let before = doc.hash().unwrap();

        // The second create lands on the slot the first one just filled, so it
        // is refused and the first has to come back out with it.
        let fresh = doc.arena().next_id();
        let out = doc.run_together(vec![
            Effect::SetName {
                id: a,
                name: Some("Body".into()),
            },
            Effect::Create {
                id: fresh,
                node: sphere(1.0),
            },
            Effect::Create {
                id: fresh,
                node: sphere(2.0),
            },
        ]);

        assert!(out.is_err(), "a duplicate create was accepted");
        assert_eq!(doc.hash().unwrap(), before, "the model was left changed");
        assert_eq!(doc.name(a), None, "a label from the step survived");
        assert!(
            !doc.arena().is_alive(fresh),
            "a node from the step survived"
        );
        // Unwinding leaves a tombstone, as any deletion does. What it must not
        // do is hand the id out again, or a reference held across the failure
        // would come to mean something else.
        assert_ne!(
            doc.arena().next_id(),
            fresh,
            "an unwound id was offered again"
        );
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

    /// A base pad, a boss placed on its far face, and the two joined.
    ///
    /// Returns the document, the base extrusion that carries the depth, and the
    /// derived placement regeneration has to move.
    fn base_and_boss(base_depth: f32) -> (Document, NodeId, NodeId) {
        use sc_geom::{Profile, Transform};
        let mut doc = Document::new();
        let base = add(
            &mut doc,
            Node::Extrude {
                profile: Profile::Rect {
                    width: 20.0,
                    height: 20.0,
                },
                depth: base_depth,
            },
        )
        .unwrap();
        let body = add(
            &mut doc,
            Node::Extrude {
                profile: Profile::Rect {
                    width: 6.0,
                    height: 6.0,
                },
                depth: 5.0,
            },
        )
        .unwrap();
        let boss = add(
            &mut doc,
            Node::Transform {
                child: body,
                xform: Transform::from_translation(Vec3::new(0.0, 0.0, base_depth)),
                on: Some(base),
            },
        )
        .unwrap();
        let joined = add(
            &mut doc,
            Node::Union {
                a: base,
                b: boss,
                smooth: 0.0,
            },
        )
        .unwrap();
        doc.apply(Command::SetRoot { root: Some(joined) }).unwrap();
        (doc, base, boss)
    }

    fn placement_z(doc: &Document, id: NodeId) -> f32 {
        match doc.arena().get(id) {
            Some(Node::Transform { xform, .. }) => xform.translation.z,
            other => panic!("{id} is not a placement: {other:?}"),
        }
    }

    /// A derived placement follows the face it names when that face moves.
    #[test]
    fn a_derived_placement_follows_the_face_it_names() {
        let (mut doc, base, boss) = base_and_boss(10.0);
        assert!((placement_z(&doc, boss) - 10.0).abs() < 1.0e-4);

        doc.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 4.0,
        })
        .unwrap();

        assert!(
            (placement_z(&doc, boss) - 4.0).abs() < 1.0e-4,
            "the boss stayed at {} after the base shrank",
            placement_z(&doc, boss)
        );
    }

    /// The edit and the regeneration it causes are one user action, so one
    /// press of undo takes back both. Checking the hash alone would not catch a
    /// missing bracket: undoing the parameter would restore the base's size
    /// while leaving the boss hanging where the regeneration put it.
    #[test]
    fn a_base_edit_and_its_regeneration_are_one_undo_step() {
        let (mut doc, base, boss) = base_and_boss(10.0);
        let before = doc.hash().unwrap();
        let steps = doc.log_len();

        doc.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 4.0,
        })
        .unwrap();
        assert_eq!(
            doc.log_len(),
            steps + 2,
            "the regeneration was not logged as a real effect"
        );

        assert!(doc.undo().unwrap());
        assert_eq!(
            doc.log_len(),
            steps,
            "undo took back only half of the action"
        );
        assert!(
            (placement_z(&doc, boss) - 10.0).abs() < 1.0e-4,
            "the boss was left where regeneration put it"
        );
        assert_eq!(
            doc.hash().unwrap(),
            before,
            "undo did not restore the model"
        );

        assert!(doc.redo().unwrap());
        assert!(
            (placement_z(&doc, boss) - 4.0).abs() < 1.0e-4,
            "redo replayed only part of the action"
        );
    }

    /// A feature the root cannot reach is not wrong, it is nowhere, and there
    /// is no face to resolve a placement against. Regeneration has to leave it
    /// alone rather than guess, or rerooting a document would move every
    /// detached feature to the origin on the next edit.
    #[test]
    fn a_derivation_the_root_cannot_reach_is_left_where_it_is() {
        let (mut doc, base, boss) = base_and_boss(10.0);
        let whole = doc.root().expect("rooted");

        // Point the document at the base alone. The boss is still in the arena
        // but nothing reaches it.
        doc.apply(Command::SetRoot { root: Some(base) }).unwrap();
        doc.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 4.0,
        })
        .unwrap();
        assert!(
            (placement_z(&doc, boss) - 10.0).abs() < 1.0e-4,
            "an unreachable placement was moved to {}",
            placement_z(&doc, boss)
        );

        // Put it back in the model and it catches up with the next edit.
        doc.apply(Command::SetRoot { root: Some(whole) }).unwrap();
        assert!(
            (placement_z(&doc, boss) - 4.0).abs() < 1.0e-4,
            "the placement stayed stale once it was reachable again"
        );
    }

    /// Regeneration emits real effects, so the log stays the whole story. Replay
    /// applies them directly and must not regenerate again on top.
    #[test]
    fn a_regenerated_document_replays_exactly() {
        let (mut doc, base, boss) = base_and_boss(10.0);
        doc.apply(Command::SetParam {
            id: base,
            name: "depth".into(),
            value: 4.0,
        })
        .unwrap();

        let log: Vec<Effect> = doc.log().cloned().collect();
        let replayed = Document::replay(log).unwrap();
        assert_eq!(replayed.hash(), doc.hash(), "replaying the log diverged");
        assert_eq!(replayed.log_len(), doc.log_len(), "replay grew the log");
        assert!((placement_z(&replayed, boss) - 4.0).abs() < 1.0e-4);
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
