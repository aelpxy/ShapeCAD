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
///
/// Deserialization is permissive by design: it writes straight into `slots`
/// and checks nothing. A file is the one way in that did not go through
/// [`Arena::insert`], [`Arena::create_at`] or [`Arena::replace`], so it has to
/// be checked, but checking it *here* costs the caller the ability to say what
/// went wrong. Serde would fold a dangling child into a parse failure, and the
/// loader would report a well-formed file as not a `ShapeCAD` document.
///
/// So the check lives in [`Arena::check_structure`] and the loader calls it.
/// See `sc_doc::file::open`.
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

    /// Validate a node's parameters, that all of its children are live, and
    /// that any node it derives its placement from is live and outside its own
    /// subtree.
    ///
    /// `id` is the id the node will occupy, which the derivation check needs:
    /// a placement built on itself would be defined in terms of its own result.
    fn check(&self, id: NodeId, node: &Node) -> Result<()> {
        if !node.is_valid() {
            return Err(GeomError::InvalidNode {
                kind: node.kind(),
                reason: "parameters out of range or non-finite".to_string(),
            });
        }
        // An id the arena has never issued cannot be reached from anywhere, so
        // a node taking one cannot be closing a loop. Worth the test: without
        // it every `insert` walks its whole new subtree to learn nothing, which
        // makes building a model quadratic in its own depth.
        let issued = (id.0 as usize) < self.slots.len();
        for c in node.children() {
            self.try_get(c)?;
            // Guarded here rather than in `replace` alone, so that every way
            // into the arena shares one predicate instead of relying on an
            // argument about which caller can produce which id.
            if issued && (c == id || self.reaches(c, id)) {
                return Err(GeomError::Cycle { at: id, via: c });
            }
        }
        // A derivation is not a child, so it is checked here rather than by the
        // loop above. Both halves matter: the base has to exist, and it has to
        // sit outside the subtree this node places, or regenerating the
        // placement would chase its own tail.
        if let Some(on) = node.derived_from() {
            self.try_get(on)?;
            if on == id || node.children().any(|c| self.reaches(c, on)) {
                return Err(GeomError::Cycle { at: id, via: on });
            }
            // And the derivations themselves must not close a loop. Two
            // placements each deriving from the other pass both tests above:
            // neither contains the other, so neither chases its own tail
            // through children. Regeneration still cannot settle, because
            // moving one moves the other right back. It terminates, having
            // silently left a placement stale.
            if self.derives_from(on, id) {
                return Err(GeomError::Cycle { at: id, via: on });
            }
        }
        Ok(())
    }

    /// Whether `from` reaches `target` by following derivations.
    ///
    /// Bounded by the number of slots rather than by a visited set: a chain of
    /// derivations is a path, not a tree, so it either ends or repeats, and it
    /// cannot be longer than the arena.
    fn derives_from(&self, from: NodeId, target: NodeId) -> bool {
        let mut at = from;
        for _ in 0..self.slots.len() {
            if at == target {
                return true;
            }
            match self.get(at).and_then(Node::derived_from) {
                Some(next) => at = next,
                None => return false,
            }
        }
        true
    }

    /// Every structural invariant the arena keeps, checked against slots that
    /// arrived from somewhere other than a mutation.
    ///
    /// Every child of a live node is live, no node is its own descendant, and
    /// any derived placement names a live node outside its own subtree. This is
    /// the same predicate [`Arena::check`] applies to one node, run over all of
    /// them.
    ///
    /// Deliberately not [`Node::is_valid`]. A [`Node::Mesh`] deserializes with
    /// a placeholder grid that the document resolves afterwards, so parameter
    /// validation here would refuse every file containing an import. Parameters
    /// are the loader's business; structure is the arena's.
    ///
    /// Quadratic in the worst case, because each child edge may walk a whole
    /// subtree. It runs once per file opened, not per frame.
    ///
    /// # Errors
    /// [`GeomError::UnknownNode`] or [`GeomError::DeadNode`] for a reference
    /// that leads nowhere, and [`GeomError::Cycle`] for a loop, whether through
    /// children or through a derivation.
    pub fn check_structure(&self) -> Result<()> {
        for id in self.live_ids() {
            let node = self.try_get(id)?;
            for c in node.children() {
                self.try_get(c)?;
                if c == id || self.reaches(c, id) {
                    return Err(GeomError::Cycle { at: id, via: c });
                }
            }
            if let Some(on) = node.derived_from() {
                self.try_get(on)?;
                if on == id
                    || node.children().any(|c| self.reaches(c, on))
                    || self.derives_from(on, id)
                {
                    return Err(GeomError::Cycle { at: id, via: on });
                }
            }
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
    /// [`GeomError::InvalidNode`] if parameters are out of range,
    /// [`GeomError::UnknownNode`] / [`GeomError::DeadNode`] if a child or the
    /// node it is derived from is gone, or [`GeomError::Cycle`] for a
    /// derivation that points into the node's own subtree.
    pub fn create_at(&mut self, id: NodeId, node: Node) -> Result<()> {
        self.check(id, &node)?;
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
    /// [`GeomError::InvalidNode`] if parameters are out of range,
    /// [`GeomError::UnknownNode`] / [`GeomError::DeadNode`] if a child or the
    /// node it is derived from is gone, or [`GeomError::Cycle`] for a
    /// derivation that points into the node's own subtree.
    pub fn insert(&mut self, node: Node) -> Result<NodeId> {
        let id = self.next_id();
        self.check(id, &node)?;
        self.slots.push(Some(node));
        Ok(id)
    }

    /// Overwrite a node in place, returning the previous value.
    ///
    /// Keeping the id lets a selection or an agent's reference survive a
    /// parameter change, which is the whole point of stable ids.
    ///
    /// # Errors
    /// [`GeomError::Cycle`] if the new children would close a loop, or if the
    /// new derivation points inside this node's own subtree, plus the same
    /// validation errors as [`Arena::insert`].
    ///
    /// # Panics
    /// Never in practice; the liveness of the slot is checked first.
    pub fn replace(&mut self, id: NodeId, node: Node) -> Result<Node> {
        self.try_get(id)?;
        self.check(id, &node)?;
        Ok(self.slots[id.0 as usize]
            .replace(node)
            .expect("checked live"))
    }

    /// Deletes a node.
    ///
    /// # Errors
    /// [`GeomError::StillReferenced`] if any live node points at it, as a child
    /// or as the feature its placement is derived from, so the DAG can never
    /// contain a dangling pointer of either kind.
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

    /// Anything that would dangle if `id` went away: a parent, or a placement
    /// derived from it. Deletion consults this, so both kinds of reference keep
    /// a node alive.
    fn referrer_of(&self, id: NodeId) -> Option<NodeId> {
        self.live_ids()
            .find(|&other| self.is_parent(other, id) || self.is_derived_from(other, id))
    }

    fn is_derived_from(&self, node: NodeId, base: NodeId) -> bool {
        node != base && self.get(node).and_then(Node::derived_from) == Some(base)
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

    /// A feature built on another names it, and that reference has to keep the
    /// base alive just as a child reference does. Deleting it would leave a
    /// placement derived from a node that no longer exists.
    #[test]
    fn a_node_a_derived_placement_is_built_on_cannot_be_deleted() {
        let mut arena = Arena::new();
        let base = sphere(&mut arena, 1.0);
        let boss = sphere(&mut arena, 2.0);
        let placed = arena
            .insert(Node::Transform {
                child: boss,
                xform: crate::Transform::from_translation(glam::Vec3::Z),
                on: Some(base),
            })
            .expect("valid placement");

        assert!(
            matches!(
                arena.remove(base),
                Err(GeomError::StillReferenced { node, by }) if node == base && by == placed
            ),
            "the base a feature is built on was deleted out from under it"
        );
        assert!(arena.is_alive(base));
    }

    /// A placement derived from something inside its own subtree would be
    /// defined in terms of itself, and regenerating it would never settle.
    #[test]
    fn a_circular_derivation_is_rejected() {
        let mut arena = Arena::new();
        let child = sphere(&mut arena, 1.0);

        // Built on the very node it places.
        assert!(
            matches!(
                arena.insert(Node::Transform {
                    child,
                    xform: crate::Transform::IDENTITY,
                    on: Some(child),
                }),
                Err(GeomError::Cycle { .. })
            ),
            "a placement was allowed to derive from its own child"
        );

        // And on itself, reached by replacing an existing placement.
        let placed = arena
            .insert(Node::Transform {
                child,
                xform: crate::Transform::IDENTITY,
                on: None,
            })
            .expect("valid placement");
        assert!(
            matches!(
                arena.replace(
                    placed,
                    Node::Transform {
                        child,
                        xform: crate::Transform::IDENTITY,
                        on: Some(placed),
                    }
                ),
                Err(GeomError::Cycle { .. })
            ),
            "a placement was allowed to derive from itself"
        );
    }

    /// A derivation must be a live node, even though it is not a child.
    #[test]
    fn a_derivation_naming_a_dead_node_is_refused() {
        let mut arena = Arena::new();
        let child = sphere(&mut arena, 1.0);
        let gone = sphere(&mut arena, 2.0);
        arena.remove(gone).expect("nothing points at it");

        assert!(matches!(
            arena.insert(Node::Transform {
                child,
                xform: crate::Transform::IDENTITY,
                on: Some(gone),
            }),
            Err(GeomError::DeadNode(_))
        ));
    }

    /// Serialised form of a two-node arena, ready to be corrupted the way a
    /// hand-edited or truncated `.shapecad` file would be. Slot 0 is a sphere,
    /// slot 1 a union that names it twice.
    #[cfg(feature = "serde")]
    fn saved_pair() -> serde_json::Value {
        let mut arena = Arena::new();
        let a = sphere(&mut arena, 1.0);
        arena
            .insert(Node::Union {
                a,
                b: a,
                smooth: 0.0,
            })
            .expect("valid union");
        serde_json::to_value(&arena).expect("an arena serialises")
    }

    /// Loading a document deserializes straight into the slot vector, so none
    /// of the checks `insert` runs have ever seen the contents. A file whose
    /// sphere has gone missing leaves the union pointing into a tombstone,
    /// which evaluates as empty space: the part comes back a piece short with
    /// no error anywhere.
    #[cfg(feature = "serde")]
    #[test]
    fn a_file_with_a_dangling_child_is_refused() {
        let mut json = saved_pair();
        json["slots"][0] = serde_json::Value::Null;

        assert!(
            serde_json::from_value::<Arena>(json)
                .expect("deserialisation is permissive")
                .check_structure()
                .is_err(),
            "a union kept a child that is not there"
        );
    }

    /// The same hole, with worse consequences: a file describing a loop makes
    /// `eval` recurse until the stack runs out, which is not a panic a caller
    /// can catch.
    #[cfg(feature = "serde")]
    #[test]
    fn a_file_containing_a_cycle_is_refused() {
        let mut json = saved_pair();
        json["slots"][0] = serde_json::to_value(Node::Transform {
            child: NodeId(1),
            xform: crate::Transform::IDENTITY,
            on: None,
        })
        .expect("a node serialises");

        assert!(
            serde_json::from_value::<Arena>(json)
                .expect("deserialisation is permissive")
                .check_structure()
                .is_err(),
            "a file described a node that is its own descendant"
        );
    }

    /// And the check must not reject anything a save actually writes.
    #[cfg(feature = "serde")]
    #[test]
    fn a_well_formed_arena_still_round_trips() {
        let json = saved_pair();
        let back: Arena = serde_json::from_value(json).expect("a saved arena reloads");

        assert_eq!(back.len(), 2);
        assert_eq!(back.parents_of(NodeId(0)), vec![NodeId(1)]);
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
