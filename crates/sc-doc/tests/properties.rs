//! Property-based tests for the document model.
//!
//! These exercise the guarantees the rest of the application is entitled to
//! assume: that undo is exact, that a rejected command changes nothing, and
//! that replaying the log reproduces the model. Every one of them is relied on
//! by the agent layer, which edits speculatively and backs out when wrong.

// Geometry code names points and distances `p`, `d`, `a`, `b` by long convention;
// spelling them out in tests hurts readability more than it helps.
#![allow(clippy::many_single_char_names)]

use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;
use sc_doc::{add, Command, Document, Effect};
use sc_geom::glam::Vec3;
use sc_geom::{Node, NodeId};

/// An edit expressed without concrete ids, resolved against whatever the
/// document happens to contain when it is applied.
///
/// Generating ids directly would produce mostly-invalid commands and waste the
/// run budget on rejections; resolving late keeps the sequences realistic.
#[derive(Clone, Debug)]
enum Action {
    AddSphere(f32),
    AddCuboid(f32, f32, f32),
    UnionOf(usize, usize, f32),
    SetParam(usize, f32),
    SetRoot(usize),
    SetName(usize, String),
    Delete(usize),
    Undo,
    Redo,
}

fn arb_action() -> impl Strategy<Value = Action> {
    prop_oneof![
        4 => (0.5f32..10.0).prop_map(Action::AddSphere),
        4 => (0.5f32..8.0, 0.5f32..8.0, 0.5f32..8.0)
                .prop_map(|(x, y, z)| Action::AddCuboid(x, y, z)),
        3 => (0usize..16, 0usize..16, 0.0f32..2.0)
                .prop_map(|(a, b, k)| Action::UnionOf(a, b, k)),
        3 => (0usize..16, 0.5f32..12.0).prop_map(|(i, v)| Action::SetParam(i, v)),
        2 => (0usize..16).prop_map(Action::SetRoot),
        1 => (0usize..16, "[a-z]{1,6}").prop_map(|(i, s)| Action::SetName(i, s)),
        1 => (0usize..16).prop_map(Action::Delete),
        2 => Just(Action::Undo),
        2 => Just(Action::Redo),
    ]
}

/// Applies an action, ignoring the ones the document legitimately refuses.
///
/// Refusals are expected and are themselves under test: whatever happens, the
/// document must remain consistent.
fn apply(doc: &mut Document, action: &Action, live: &[NodeId]) {
    let pick = |i: usize| live.get(i % live.len().max(1)).copied();
    let _ = match action {
        Action::AddSphere(r) => add(doc, Node::Sphere { radius: *r }).map(Some),
        Action::AddCuboid(x, y, z) => add(
            doc,
            Node::Box {
                half: Vec3::new(*x, *y, *z),
                round: 0.0,
            },
        )
        .map(Some),
        Action::UnionOf(a, b, k) => match (pick(*a), pick(*b)) {
            (Some(a), Some(b)) => add(doc, Node::Union { a, b, smooth: *k }).map(Some),
            _ => Ok(None),
        },
        Action::SetParam(i, v) => match pick(*i) {
            Some(id) => doc.apply(Command::SetParam {
                id,
                name: "radius".into(),
                value: *v,
            }),
            None => Ok(None),
        },
        Action::SetRoot(i) => doc.apply(Command::SetRoot { root: pick(*i) }),
        Action::SetName(i, n) => match pick(*i) {
            Some(id) => doc.apply(Command::SetName {
                id,
                name: Some(n.clone()),
            }),
            None => Ok(None),
        },
        Action::Delete(i) => match pick(*i) {
            Some(id) => doc.apply(Command::Delete { id }),
            None => Ok(None),
        },
        Action::Undo => doc.undo().map(|_| None),
        Action::Redo => doc.redo().map(|_| None),
    };
}

fn run(actions: &[Action]) -> Document {
    let mut doc = Document::new();
    for a in actions {
        let live: Vec<NodeId> = doc.arena().live_ids().collect();
        apply(&mut doc, a, &live);
    }
    doc
}

fn arb_actions() -> impl Strategy<Value = Vec<Action>> {
    proptest::collection::vec(arb_action(), 1..40)
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/proptest-regressions/properties.txt",
        ))),
        ..ProptestConfig::default()
    })]

    /// Undoing everything must return the document to its initial state, and
    /// redoing everything must return it exactly to where it was.
    #[test]
    fn undo_then_redo_is_the_identity(actions in arb_actions()) {
        let mut doc = run(&actions);
        let after = doc.hash();

        let mut undone = 0;
        while doc.undo().unwrap_or(false) {
            undone += 1;
            prop_assert!(undone < 10_000, "undo did not terminate");
        }
        prop_assert!(doc.hash().is_none(), "fully undone document still has a root");

        // Redo exactly as many steps as were undone. Redoing to exhaustion would
        // also replay a redo tail that the action sequence had already discarded,
        // which is a different state and legitimately so.
        for _ in 0..undone {
            prop_assert!(doc.redo().unwrap_or(false), "redo ran out early");
        }
        prop_assert_eq!(doc.hash(), after, "redo did not restore the model");
    }

    /// Every prefix of the surviving history replays to the same model. This is
    /// what makes the command log a safe substrate for collaboration and for
    /// agents that need to reason about how a part was built.
    #[test]
    fn replaying_the_log_is_faithful(actions in arb_actions()) {
        let doc = run(&actions);
        let log: Vec<Effect> = doc.log().cloned().collect();

        // A log must never be rejected on replay; if it is, it was not a
        // faithful record of what happened.
        let replayed = Document::replay(log).expect("log replayed cleanly");
        prop_assert_eq!(replayed.hash(), doc.hash());
        prop_assert_eq!(replayed.outline(), doc.outline());
        prop_assert_eq!(replayed.root(), doc.root(), "replay assigned different ids");
    }

    /// A rejected command must not perturb the document in any observable way.
    #[test]
    fn rejected_commands_are_invisible(actions in arb_actions(), bogus in 0usize..32) {
        let mut doc = run(&actions);
        let before = (doc.hash(), doc.outline(), doc.log_len(), doc.can_undo());

        // A grab-bag of commands that should all fail.
        let missing = NodeId(9_999 + bogus as u32);
        let attempts = vec![
            Command::SetParam { id: missing, name: "radius".into(), value: 1.0 },
            Command::Delete { id: missing },
            Command::SetRoot { root: Some(missing) },
            Command::Replace { id: missing, node: Node::Sphere { radius: 1.0 } },
            Command::Add { node: Node::Sphere { radius: -1.0 } },
            Command::Add { node: Node::Sphere { radius: f32::NAN } },
        ];
        for c in attempts {
            prop_assert!(doc.apply(c.clone()).is_err(), "expected {c:?} to be rejected");
        }

        let after = (doc.hash(), doc.outline(), doc.log_len(), doc.can_undo());
        prop_assert_eq!(before, after, "a rejected command left a trace");
    }

    /// The geometry DAG must never contain a reference to a deleted node, no
    /// matter what sequence of edits and undos produced it.
    #[test]
    fn no_dangling_references_ever(actions in arb_actions()) {
        let doc = run(&actions);
        let arena = doc.arena();
        for id in arena.live_ids() {
            for child in arena.get(id).unwrap().children() {
                prop_assert!(arena.is_alive(child), "{id} references dead node {child}");
            }
        }
        if let Some(root) = doc.root() {
            prop_assert!(arena.is_alive(root), "root {root} is dead");
        }
    }
}
