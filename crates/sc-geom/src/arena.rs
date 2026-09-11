//! Storage for the implicit DAG.
//!
//! Append-only with tombstones: ids are handed out monotonically and never
//! recycled. Deletion marks a slot dead rather than compacting, so a stale
//! [`NodeId`] always resolves to a clear "this was deleted" rather than
//! silently aliasing an unrelated node.

use crate::error::{GeomError, Result};
use crate::node::{Node, NodeId};
use std::collections::HashSet;

/// Storage for an implicit geometry DAG.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Arena {
    slots: Vec<Option<Node>>,
}

impl Arena {
    /// An empty arena.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of live nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Whether there are no live nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total ids ever issued, live or dead.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// The node at `id`, or `None` if it is unknown or deleted.
    #[must_use]
    pub fn get(&self, id: NodeId) -> Option<&Node> {
        self.slots.get(id.0 as usize).and_then(|s| s.as_ref())
    }

    /// Like [`Arena::get`] but distinguishes "never existed" from "deleted".
    ///
    /// # Errors
    /// [`GeomError::UnknownNode`] if the id was never issued,
    /// [`GeomError::DeadNode`] if it has been deleted.
    pub fn try_get(&self, id: NodeId) -> Result<&Node> {
        match self.slots.get(id.0 as usize) {
            None => Err(GeomError::UnknownNode(id)),
            Some(None) => Err(GeomError::DeadNode(id)),
            Some(Some(n)) => Ok(n),
        }
    }

    /// Whether `id` refers to a live node.
    #[must_use]
    pub fn is_alive(&self, id: NodeId) -> bool {
        self.get(id).is_some()
    }

    /// Every live id, in ascending order.
    pub fn live_ids(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|_| NodeId(i as u32)))
    }

    /// Validate a node's parameters and that all of its children are live.
    fn check(&self, node: &Node) -> Result<()> {
        if !node.is_valid() {
            return Err(GeomError::InvalidNode {
                kind: node.kind(),
                reason: "parameters out of range or non-finite".to_string(),
            });
        }
        for c in node.children() {
            self.try_get(c)?;
        }
        Ok(())
    }

    /// The id [`Arena::insert`] would hand out next.
    #[must_use]
    pub fn next_id(&self) -> NodeId {
        NodeId(self.slots.len() as u32)
    }

    /// Recreate a node at a specific id.
    ///
    /// Exists so that undoing a deletion restores the node under its original
    /// id. Without it, undo would resurrect the geometry under a fresh id and
    /// quietly invalidate every selection and agent-held reference pointing at
    /// it, which would defeat the purpose of stable ids.
    ///
    /// # Errors
    /// [`GeomError::SlotOccupied`] if the id is currently live,
    /// [`GeomError::InvalidNode`] if parameters are out of range, or
    /// [`GeomError::UnknownNode`] / [`GeomError::DeadNode`] if a child is gone.
    pub fn create_at(&mut self, id: NodeId, node: Node) -> Result<()> {
        self.check(&node)?;
        let idx = id.0 as usize;
        if idx < self.slots.len() && self.slots[idx].is_some() {
            return Err(GeomError::SlotOccupied(id));
        }
        if idx >= self.slots.len() {
            self.slots.resize(idx + 1, None);
        }
        self.slots[idx] = Some(node);
        Ok(())
    }

    /// Adds a node and returns its freshly issued id.
    ///
    /// # Errors
    /// [`GeomError::InvalidNode`] if parameters are out of range, or
    /// [`GeomError::UnknownNode`] / [`GeomError::DeadNode`] if a child is gone.
    pub fn insert(&mut self, node: Node) -> Result<NodeId> {
        self.check(&node)?;
        let id = NodeId(self.slots.len() as u32);
        self.slots.push(Some(node));
        Ok(id)
    }

    /// Overwrite a node in place, returning the previous value.
    ///
    /// Keeping the id lets a selection or an agent's reference survive a
    /// parameter change, which is the whole point of stable ids.
    ///
    /// # Errors
    /// [`GeomError::Cycle`] if the new children would close a loop, plus the
    /// same validation errors as [`Arena::insert`].
    ///
    /// # Panics
    /// Never in practice; the liveness of the slot is checked first.
    pub fn replace(&mut self, id: NodeId, node: Node) -> Result<Node> {
        self.try_get(id)?;
        self.check(&node)?;
        for c in node.children() {
            if c == id || self.reaches(c, id) {
                return Err(GeomError::Cycle { at: id, via: c });
            }
        }
        Ok(self.slots[id.0 as usize]
            .replace(node)
            .expect("checked live"))
    }

    /// Deletes a node.
    ///
    /// # Errors
    /// [`GeomError::StillReferenced`] if any live node points at it, so the DAG
    /// can never contain a dangling child pointer.
    ///
    /// # Panics
    /// Never in practice; the liveness of the slot is checked first.
    pub fn remove(&mut self, id: NodeId) -> Result<Node> {
        self.try_get(id)?;
        if let Some(by) = self.referrer_of(id) {
            return Err(GeomError::StillReferenced { node: id, by });
        }
        Ok(self.slots[id.0 as usize].take().expect("checked live"))
    }

    fn referrer_of(&self, id: NodeId) -> Option<NodeId> {
        self.live_ids().find(|&other| {
            other != id
                && self
                    .get(other)
                    .is_some_and(|n| n.children().any(|c| c == id))
        })
    }

    /// Whether `target` is reachable by walking children from `from`.
    #[must_use]
    pub fn reaches(&self, from: NodeId, target: NodeId) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![from];
        while let Some(cur) = stack.pop() {
            if cur == target {
                return true;
            }
            if !seen.insert(cur) {
                continue;
            }
            if let Some(n) = self.get(cur) {
                stack.extend(n.children());
            }
        }
        false
    }

    /// Every node reachable from `root`, including `root` itself.
    #[must_use]
    pub fn reachable(&self, root: NodeId) -> HashSet<NodeId> {
        let mut seen = HashSet::new();
        let mut stack = vec![root];
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur) {
                continue;
            }
            if let Some(n) = self.get(cur) {
                stack.extend(n.children());
            }
        }
        seen
    }
}
