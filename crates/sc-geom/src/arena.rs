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

    /// Every live node that names `id` as a direct child.
    ///
    /// The store is a DAG rather than a tree, so a node can have more than one
    /// parent. Anything that reroutes a node has to rewire all of them: leaving
    /// one behind splits the model in two, with the edit visible down one path
    /// and the original still standing down the other.
    ///
    /// Linear in the size of the arena. Callers that need this per frame should
    /// cache it; the editing commands run it once per action.
    #[must_use]
    pub fn parents_of(&self, id: NodeId) -> Vec<NodeId> {
        self.live_ids()
            .filter(|&other| self.is_parent(other, id))
            .collect()
    }

    fn referrer_of(&self, id: NodeId) -> Option<NodeId> {
        self.live_ids().find(|&other| self.is_parent(other, id))
    }

    fn is_parent(&self, parent: NodeId, child: NodeId) -> bool {
        parent != child
            && self
                .get(parent)
                .is_some_and(|n| n.children().any(|c| c == child))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Node;

    fn sphere(arena: &mut Arena, radius: f32) -> NodeId {
        arena.insert(Node::Sphere { radius }).expect("valid sphere")
    }

    #[test]
    fn a_node_nothing_points_at_has_no_parents() {
        let mut arena = Arena::new();
        let a = sphere(&mut arena, 1.0);
        assert_eq!(arena.parents_of(a), Vec::new());
    }

    #[test]
    fn both_sides_of_a_boolean_name_it_as_their_parent() {
        let mut arena = Arena::new();
        let a = sphere(&mut arena, 1.0);
        let b = sphere(&mut arena, 2.0);
        let union = arena
            .insert(Node::Union { a, b, smooth: 0.0 })
            .expect("valid union");

        assert_eq!(arena.parents_of(a), vec![union]);
        assert_eq!(arena.parents_of(b), vec![union]);
        assert_eq!(arena.parents_of(union), Vec::new());
    }

    /// The store is a DAG, so one node can be reached down two paths. Reporting
    /// only the first is what lets an edit rewire half a model and leave the
    /// other half pointing at the original.
    #[test]
    fn a_shared_node_reports_every_parent() {
        let mut arena = Arena::new();
        let shared = sphere(&mut arena, 1.0);
        let other = sphere(&mut arena, 2.0);
        let left = arena
            .insert(Node::Union {
                a: shared,
                b: other,
                smooth: 0.0,
            })
            .expect("valid union");
        let right = arena
            .insert(Node::Difference {
                a: other,
                b: shared,
                smooth: 0.0,
            })
            .expect("valid difference");

        let mut parents = arena.parents_of(shared);
        parents.sort_by_key(|id| id.0);
        assert_eq!(parents, vec![left, right]);
    }

    /// A union of a node with itself names it twice. Callers rewrite children by
    /// mapping over them, so the parent must appear once, not once per edge.
    #[test]
    fn a_parent_that_names_a_node_twice_is_reported_once() {
        let mut arena = Arena::new();
        let a = sphere(&mut arena, 1.0);
        let twice = arena
            .insert(Node::Union {
                a,
                b: a,
                smooth: 0.0,
            })
            .expect("valid union");

        assert_eq!(arena.parents_of(a), vec![twice]);
    }

    #[test]
    fn a_tombstoned_parent_is_forgotten() {
        let mut arena = Arena::new();
        let a = sphere(&mut arena, 1.0);
        let b = sphere(&mut arena, 2.0);
        let union = arena
            .insert(Node::Union { a, b, smooth: 0.0 })
            .expect("valid union");
        arena.remove(union).expect("nothing points at the union");

        assert_eq!(arena.parents_of(a), Vec::new());
    }
}
