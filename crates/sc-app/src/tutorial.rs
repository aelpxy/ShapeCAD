//! A tutorial that watches instead of narrating.
//!
//! Every step here advances when the user actually does the thing, not when
//! they press Next. That is the whole difference between a tutorial and a
//! slideshow: a slideshow can be clicked through without learning anything,
//! and it cannot tell whether it worked.
//!
//! The cost of that is a condition per step, and conditions are the part that
//! rots. A step that can never be satisfied traps a beginner in the one part of
//! the application they cannot leave, so every one of them is driven through a
//! real `AppState` in the tests below.

use crate::state::AppState;
use sc_geom::GeometryHash;

/// What the document looked like when the current step began.
///
/// Steps ask "has something changed" far more often than "is something true",
/// because most of what is being taught is an action rather than a state. A pad
/// already in the document should not tick off "add a shape".
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mark {
    nodes: usize,
    log: usize,
    cuts: usize,
    yaw: f32,
    pitch: f32,
    hash: Option<GeometryHash>,
}

impl Mark {
    fn of(state: &AppState) -> Self {
        Self {
            nodes: state.doc.arena().len(),
            log: state.doc.log_len(),
            cuts: cuts(state),
            // The goal rather than the visible camera. The rig eases toward it,
            // so a camera still in flight when the step began would go on
            // turning by itself and tick the step off with nobody touching
            // anything. The goal only moves when the user moves it.
            yaw: state.rig.goal.yaw,
            pitch: state.rig.goal.pitch,
            hash: state.doc.hash(),
        }
    }

    /// Keeps the mark reachable from where the document now is.
    ///
    /// A step asks for more nodes, or more cuts, than there were when it began.
    /// The document underneath can be replaced while the card is showing: File
    /// then New, opening a file, loading a sample. That leaves a mark describing
    /// a document that no longer exists, and a beginner holding a card that can
    /// never be ticked off, in the one part of the application they cannot
    /// leave. Following the document down whenever it has fewer of something
    /// than the mark does costs nothing when it has not, and can never skip a
    /// step, because no condition asks for *fewer* than the mark.
    ///
    /// The log goes the other way: the last step asks for it to shrink, so the
    /// mark follows it up instead, and a document loaded mid-step can still be
    /// undone out of.
    fn rebase(&mut self, state: &AppState) {
        let now = Self::of(state);
        self.nodes = self.nodes.min(now.nodes);
        self.cuts = self.cuts.min(now.cuts);
        self.log = self.log.max(now.log);
    }
}

/// How many cuts are in the document.
fn cuts(state: &AppState) -> usize {
    state
        .doc
        .arena()
        .live_ids()
        .filter(|id| {
            state
                .doc
                .arena()
                .get(*id)
                .is_some_and(|n| n.kind() == "difference")
        })
        .count()
}

/// One thing to learn.
pub(crate) struct Step {
    pub title: &'static str,
    pub body: &'static str,
    /// Whether it has been done, given where the step started.
    done: fn(Mark, &AppState) -> bool,
}

/// How far the camera has to turn before orbiting counts as learned.
///
/// A third of a radian is about twenty degrees: unmistakably deliberate, and
/// reached in the first flick of a drag.
const TURNED: f32 = 0.33;

/// The steps, in order.
///
/// Five, and each one is a thing you cannot use the application without.
/// Anything discoverable from a tooltip is left to the tooltip.
///
/// There is deliberately no "now select it" step. Adding a solid selects it
/// already, so the condition would be satisfied before the user had done
/// anything, and a step that ticks itself off teaches nothing while looking like
/// it taught something. Selection is explained on the cards either side of where
/// it would have been.
pub(crate) const STEPS: [Step; 5] = [
    Step {
        title: "Add a solid",
        body: "Click Box under ADD, on the left. Everything here starts as a solid you then cut and reshape, and whatever you add is selected for you: its dimensions are in the panel on the right.",
        done: |mark, state| state.doc.arena().len() > mark.nodes,
    },
    Step {
        title: "Look around it",
        body: "Drag in the viewport to orbit, scroll to zoom, and right-drag to pan. The X, Y and Z buttons above snap you back to a square-on view.",
        done: |mark, state| {
            (state.rig.goal.yaw - mark.yaw).abs() > TURNED
                || (state.rig.goal.pitch - mark.pitch).abs() > TURNED
        },
    },
    Step {
        title: "Resize it by hand",
        body: "Drag one of the blue dots on the solid. Each sits on the face it moves, and the value follows your pointer. Click anything else to select it instead, and its own dots appear.",
        done: |mark, state| state.doc.hash() != mark.hash,
    },
    Step {
        title: "Cut a hole",
        body: "Pick Hole under CUT, then click where you want it. The outline follows your pointer until you do, and the cut goes all the way through.",
        done: |mark, state| cuts(state) > mark.cuts,
    },
    Step {
        title: "Take it back",
        body: "Press Ctrl+Z. One press undoes one thing you did, however many operations it took underneath.",
        // A shorter log alone would also be true of a document that was
        // replaced, which would tick this off without anything being undone.
        // Undoing always leaves something to redo, and starting or opening a
        // document never does, so the two together say an undo happened.
        done: |mark, state| state.doc.log_len() < mark.log && state.doc.can_redo(),
    },
];

/// A tutorial in progress.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tutorial {
    /// Which step is showing. Equal to `STEPS.len()` on the closing card.
    pub at: usize,
    mark: Mark,
}

impl Tutorial {
    pub(crate) fn start(state: &AppState) -> Self {
        Self {
            at: 0,
            mark: Mark::of(state),
        }
    }

    /// The step showing now, or `None` on the closing card.
    #[must_use]
    pub(crate) fn step(&self) -> Option<&'static Step> {
        STEPS.get(self.at)
    }

    /// Whether the tutorial has run out of steps.
    ///
    /// The interface asks `step()` instead, since it needs the step anyway. This
    /// exists because "did it finish" is what the tests are actually asserting,
    /// and spelling that as `step().is_none()` reads backwards.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn finished(&self) -> bool {
        self.at >= STEPS.len()
    }

    /// Checks the current step and moves on if it is done. True if it advanced.
    ///
    /// Only ever one step per call, even when an action satisfies several. A
    /// beginner who happens to do two things at once should still see the card
    /// for each: skipping one is skipping the explanation that goes with it.
    pub(crate) fn advance(&mut self, state: &AppState) -> bool {
        let Some(step) = self.step() else {
            return false;
        };
        self.mark.rebase(state);
        if !(step.done)(self.mark, state) {
            return false;
        }
        self.at += 1;
        self.mark = Mark::of(state);
        true
    }

    /// Moves on without doing the step, for a user who already knows.
    pub(crate) fn skip_step(&mut self, state: &AppState) {
        self.at = (self.at + 1).min(STEPS.len());
        self.mark = Mark::of(state);
    }
}

#[cfg(test)]
mod tests {
    use super::{Tutorial, STEPS};
    use crate::state::{AppState, Armed, Placing};
    use sc_geom::glam::{Vec2, Vec3};
    use sc_geom::Node;

    /// What a step asks for, named so the table below reads as one thing.
    type Action = (&'static str, fn(&mut AppState));

    fn started() -> (AppState, Tutorial) {
        let mut state = AppState::new();
        state.new_document();
        let tutorial = Tutorial::start(&state);
        (state, tutorial)
    }

    /// Does what each step asks, in order, and checks the tutorial keeps up.
    ///
    /// This is the test that matters. A step whose condition cannot be satisfied
    /// traps a beginner in the one part of the application they cannot leave,
    /// and nothing else in the suite would notice.
    #[test]
    fn doing_what_each_step_asks_finishes_the_tutorial() {
        let (mut state, mut tutorial) = started();

        let add = |state: &mut AppState| {
            state.add_body(
                Node::Box {
                    half: Vec3::splat(10.0),
                    round: 0.0,
                },
                "Block",
            );
        };
        let orbit = |state: &mut AppState| {
            state.rig.goal.yaw += 1.0;
            state.rig.snap_to(state.rig.goal);
        };
        let resize = |state: &mut AppState| {
            let id = state.selected.expect("selected by now");
            state.apply(sc_doc::Command::SetParam {
                id,
                name: "half_x".into(),
                value: 18.0,
            });
        };
        let cut = |state: &mut AppState| {
            state.arm(Armed {
                kind: Placing::Pocket,
                profile: sc_geom::Profile::Circle { radius: 3.0 },
                label: "Hole",
            });
            state.place_armed(Vec2::ZERO);
        };
        let undo = AppState::undo;

        let actions: [Action; 5] = [
            ("add", add),
            ("orbit", orbit),
            ("resize", resize),
            ("cut", cut),
            ("undo", undo),
        ];

        for (i, (name, act)) in actions.into_iter().enumerate() {
            assert_eq!(
                tutorial.at, i,
                "expected to be on step {i} before {name}, on {}",
                tutorial.at
            );
            assert!(
                !tutorial.advance(&state),
                "step {i} ({}) was already satisfied before {name}",
                STEPS[i].title
            );
            act(&mut state);
            assert!(
                tutorial.advance(&state),
                "doing {name} did not satisfy step {i} ({})",
                STEPS[i].title
            );
        }
        assert!(tutorial.finished(), "ran out of actions before steps");
        assert!(tutorial.step().is_none());
    }

    /// A document that already has something in it must not tick off "add a
    /// solid" before the user has added anything. The steps measure change, not
    /// state, and this is the case that tells the two apart.
    #[test]
    fn a_document_that_is_not_empty_still_starts_at_the_first_step() {
        let mut state = AppState::new();
        state.load_sample();
        let mut tutorial = Tutorial::start(&state);

        assert!(!tutorial.advance(&state), "a loaded sample skipped a step");
        assert_eq!(tutorial.at, 0);

        state.add_body(Node::Sphere { radius: 4.0 }, "Ball");
        assert!(tutorial.advance(&state));
    }

    /// One step per check, even when an action satisfies several at once.
    /// Adding a body also selects it, which would otherwise skip a card and the
    /// explanation on it.
    #[test]
    fn advancing_never_skips_a_card() {
        let (mut state, mut tutorial) = started();
        state.add_body(Node::Sphere { radius: 5.0 }, "Ball");

        assert!(tutorial.advance(&state));
        assert_eq!(tutorial.at, 1, "one action moved more than one step");
    }

    /// Someone who already knows can move on without performing the step.
    #[test]
    fn a_step_can_be_skipped() {
        let (state, mut tutorial) = started();
        for expected in 1..=STEPS.len() {
            tutorial.skip_step(&state);
            assert_eq!(tutorial.at, expected);
        }
        tutorial.skip_step(&state);
        assert_eq!(tutorial.at, STEPS.len(), "skipping ran off the end");
        assert!(tutorial.finished());
    }

    /// Moves to the step at `index` the way a user who knows it all would.
    fn at_step(state: &AppState, index: usize) -> Tutorial {
        let mut tutorial = Tutorial::start(state);
        for _ in 0..index {
            tutorial.skip_step(state);
        }
        assert_eq!(tutorial.at, index);
        tutorial
    }

    /// The document can be replaced while a card is showing: File then New,
    /// opening a file, loading the sample. The mark then describes a document
    /// that no longer exists, and "more nodes than there were" is a bar the new
    /// one may never clear. That leaves a beginner holding a card that cannot be
    /// ticked off, in the one part of the application they cannot leave.
    #[test]
    fn starting_a_new_document_cannot_strand_the_first_step() {
        let mut state = AppState::new();
        state.load_sample();
        let mut tutorial = Tutorial::start(&state);

        state.new_document();
        assert!(!tutorial.advance(&state), "an empty document added nothing");

        state.add_body(Node::Sphere { radius: 4.0 }, "Ball");
        assert!(
            tutorial.advance(&state),
            "a solid was added and the step could not see it"
        );
    }

    /// The same thing one card further on, where the bar is a count of cuts.
    #[test]
    fn starting_a_new_document_cannot_strand_the_cut_step() {
        let mut state = AppState::new();
        state.load_sample();
        let mut tutorial = at_step(&state, 3);
        assert_eq!(STEPS[3].title, "Cut a hole");

        state.new_document();
        state.add_body(
            Node::Box {
                half: Vec3::splat(10.0),
                round: 0.0,
            },
            "Block",
        );
        assert!(!tutorial.advance(&state), "nothing has been cut yet");

        state.arm(Armed {
            kind: Placing::Pocket,
            profile: sc_geom::Profile::Circle { radius: 3.0 },
            label: "Hole",
        });
        state.place_armed(Vec2::ZERO);
        assert!(
            tutorial.advance(&state),
            "a hole was cut and the step could not see it"
        );
    }

    /// And on the last card, where the log is the measure. A document loaded
    /// here has a log of its own, longer or shorter than the one the mark was
    /// taken from, and neither may be undone out of.
    #[test]
    fn loading_a_document_neither_satisfies_nor_strands_the_undo_step() {
        let mut state = AppState::new();
        state.new_document();
        let mut tutorial = at_step(&state, 4);
        assert_eq!(STEPS[4].title, "Take it back");

        state.load_sample();
        assert!(
            !tutorial.advance(&state),
            "loading a document is not undoing anything"
        );

        state.undo();
        assert!(
            tutorial.advance(&state),
            "the user undid something and the step could not see it"
        );
    }

    /// The camera is a rig: input moves the goal and the visible camera eases
    /// after it. A step that watches the visible camera ticks itself off while
    /// the pointer is nowhere near, because a camera still in flight when the
    /// card appeared goes on turning by itself.
    #[test]
    fn a_camera_still_easing_is_not_the_user_looking_around() {
        let mut state = AppState::new();
        state.new_document();
        state.rig.goal.yaw += 1.0;
        let mut tutorial = at_step(&state, 1);
        assert_eq!(STEPS[1].title, "Look around it");

        // Two seconds of frames with nobody touching anything.
        for _ in 0..120 {
            state.rig.advance(1.0 / 60.0);
        }
        assert!(
            !tutorial.advance(&state),
            "the camera arrived on its own and the step counted it"
        );

        state.rig.goal.yaw += 1.0;
        assert!(tutorial.advance(&state), "orbiting did not count");
    }

    /// Every card has something on it. An empty step is a dead end that looks
    /// like a bug to whoever hits it.
    #[test]
    fn every_step_says_something() {
        for step in &STEPS {
            assert!(!step.title.is_empty());
            assert!(
                step.body.len() > 40,
                "{} has nothing useful on it",
                step.title
            );
            assert!(
                !step.body.contains('—'),
                "{} has an em dash in it",
                step.title
            );
        }
    }
}
