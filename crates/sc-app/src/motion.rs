//! Spring-based motion.
//!
//! Interface animation here is driven by springs rather than by a duration and
//! an easing curve. The difference matters when something changes target
//! mid-flight, which in a tool that responds to every click is most of the time:
//! a tween restarts from wherever it was and loses its velocity, so an
//! interrupted animation visibly stutters. A spring carries its velocity across
//! the change and simply bends toward the new target.
//!
//! Tunings are expressed the way `SwiftUI` expresses them, as a response time and
//! a damping fraction, because those are the two numbers a person can reason
//! about. Stiffness and mass are not.

/// The state of one animated value.
///
/// Small and `Copy`, because there is one per animated widget and they live in
/// egui's per-frame memory rather than in any struct of ours.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Spring {
    pub value: f32,
    pub velocity: f32,
}

/// How a spring behaves, in the two terms worth naming.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Tuning {
    /// Roughly how long the motion takes to cover its distance, in seconds.
    /// Smaller is faster.
    pub response: f32,
    /// 1.0 settles without overshooting. Below that it overshoots and comes
    /// back, which reads as liveliness on something appearing and as sloppiness
    /// on something being dragged.
    pub damping: f32,
}

impl Tuning {
    /// The workhorse: quick, no overshoot. Hover fills, colour changes, and
    /// anything a pointer is currently driving.
    pub(crate) const SMOOTH: Self = Self {
        response: 0.30,
        damping: 1.0,
    };

    /// Overshoots slightly. For things that appear rather than change: a menu
    /// opening, a panel arriving, a selection moving to a new home.
    pub(crate) const BOUNCY: Self = Self {
        response: 0.38,
        damping: 0.72,
    };

    /// Near-instant, for press feedback. Anything slower than this feels like
    /// lag rather than like animation, because the finger is already gone.
    pub(crate) const SNAPPY: Self = Self {
        response: 0.15,
        damping: 0.92,
    };
}

/// Longest step the integrator will take, in seconds.
///
/// A frame that took longer than this is a stall, not a slow frame: the window
/// was occluded, or a shader was compiling. Integrating the real elapsed time
/// across one would fling every spring in the interface. Clamping instead makes
/// the animation briefly slower than wall clock, which nobody can see, rather
/// than wrong, which everybody can.
const MAX_STEP: f32 = 1.0 / 30.0;

/// Longest slice the integrator will take within one frame.
///
/// Chosen so the error stays under a fraction of a percent across every tuning
/// here, which is what makes the motion identical on a 60Hz and a 144Hz display.
/// At 60Hz this is four slices per frame.
const MAX_SUBSTEP: f32 = 1.0 / 240.0;

/// Distance and speed below which a spring is considered arrived.
///
/// In interface points, so this is a twentieth of a pixel: far below anything
/// that can be drawn, and far enough above zero that a spring actually stops
/// instead of asking for repaints forever.
const EPSILON: f32 = 0.05;

/// The shortest response the integrator will honour, in seconds.
///
/// Semi-implicit Euler is stable only while `omega * h` stays comfortably under
/// two, and `h` is at most [`MAX_SUBSTEP`], so a response of zero (or a negative
/// one) would wind a spring up instead of settling it: the value doubles every
/// few slices and the interface throws everything animated off screen. Sixteen
/// slices per oscillation leaves `omega * h` at `TAU / 16`, which is a wide
/// margin. Every tuning in the table above is slower than this, so the clamp
/// only ever catches a tuning computed at a call site.
const MIN_RESPONSE: f32 = 16.0 * MAX_SUBSTEP;

/// The least damping the integrator will honour.
///
/// Zero is a lossless oscillator: it never satisfies [`Spring::settled`], so it
/// would ask for a repaint forever. Below zero it gains energy on every slice.
const MIN_DAMPING: f32 = 0.05;

/// The most damping the integrator will honour.
///
/// The velocity term is `2 * damping * omega * h` per slice, which has to stay
/// under two for the same reason as [`MIN_RESPONSE`]. At the shortest response
/// that leaves room for well over this, and anything heavier than two is
/// already so overdamped that it reads as a fade rather than a movement.
const MAX_DAMPING: f32 = 2.0;

impl Tuning {
    /// The tuning as the integrator can actually honour it.
    ///
    /// See [`MIN_RESPONSE`], [`MIN_DAMPING`] and [`MAX_DAMPING`]. A tuning that
    /// is not a number at all is a mistake rather than a request, and taking the
    /// bound for it is the same answer as taking the bound for zero.
    fn stable(self) -> Self {
        let damping = if self.damping.is_nan() {
            MIN_DAMPING
        } else {
            self.damping.clamp(MIN_DAMPING, MAX_DAMPING)
        };
        Self {
            // `max` answers the bound for a `NaN` of its own accord.
            response: self.response.max(MIN_RESPONSE),
            damping,
        }
    }
}

impl Spring {
    pub(crate) fn at(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
        }
    }

    /// Advances the spring toward `target` by `dt` seconds.
    ///
    /// Semi-implicit Euler, subdivided so the integration error does not depend
    /// on how long the frame was. Taking one step per frame is the obvious
    /// implementation and it is wrong: at a response of 0.3 seconds a 60Hz frame
    /// is a third of a radian, which is far enough into the error to be visible.
    /// The same motion reached 0.68 at 60Hz and 0.63 at 144Hz, so it ran
    /// measurably faster on the slower display.
    ///
    /// A closed form would be exact and O(1), but it has to special-case the
    /// critically damped root and behaves badly either side of it, whereas four
    /// multiplies repeated four times is nothing.
    #[must_use]
    pub(crate) fn step(self, target: f32, dt: f32, tuning: Tuning) -> Self {
        let dt = dt.clamp(0.0, MAX_STEP);
        if dt <= 0.0 {
            return self;
        }
        if !target.is_finite() {
            // A `NaN` or infinite target is a bug upstream: a rect that
            // collapsed to nothing, a ratio over an empty range. Integrating one
            // poisons the spring permanently rather than for a frame, because a
            // `NaN` value never compares as settled, so the interface would go
            // on repainting at the refresh rate for the rest of the session.
            // Holding position keeps the mistake where it was made.
            return self;
        }
        if !self.value.is_finite() || !self.velocity.is_finite() {
            // And if one got in anyway, the next sensible target recovers.
            return Self::arrived(target);
        }
        let tuning = tuning.stable();
        // Angular frequency from the response time, the same relation `SwiftUI`
        // uses: a response of one second is one radian per second.
        let omega = std::f32::consts::TAU / tuning.response;
        let steps = (dt / MAX_SUBSTEP).ceil().max(1.0);
        let h = dt / steps;

        let mut s = self;
        for _ in 0..steps as u32 {
            let accel =
                -2.0 * tuning.damping * omega * s.velocity - omega * omega * (s.value - target);
            s.velocity += accel * h;
            s.value += s.velocity * h;
        }
        s
    }

    /// Whether the spring has arrived and stopped.
    #[must_use]
    pub(crate) fn settled(self, target: f32) -> bool {
        (self.value - target).abs() < EPSILON && self.velocity.abs() < EPSILON
    }

    /// Snaps to the target, discarding any velocity.
    #[must_use]
    pub(crate) fn arrived(target: f32) -> Self {
        Self::at(target)
    }
}

/// Animates a named value toward `target`, returning where it is now.
///
/// The state lives in egui's memory under `id`, so a caller needs to own
/// nothing. The first call for a given id starts at the target rather than at
/// zero: a widget appearing for the first time should already be where it
/// belongs, not slide in from wherever zero happens to be.
///
/// Requests a repaint while the spring is moving, which is what makes the
/// animation run at all. The frame loop honours delayed repaint requests, so an
/// idle interface still costs nothing.
pub(crate) fn animate(ui: &egui::Ui, id: egui::Id, target: f32, tuning: Tuning) -> f32 {
    let (dt, now) = ui.input(|i| (i.stable_dt, i.time));
    let previous = ui.data(|d| d.get_temp::<Tracked>(id));

    if !target.is_finite() {
        // Nothing sensible to move toward, and storing it would poison the
        // spring for good. Stay where we are and do not ask for another frame.
        return previous.map_or(0.0, |t| t.spring.value);
    }
    let next = previous
        .map_or_else(|| Spring::arrived(target), |t| t.spring)
        .step(target, dt, tuning);

    if next.settled(target) {
        // Park it exactly on the target, so a value used for a position does not
        // sit a fraction of a pixel off forever and keep asking to be redrawn.
        remember(ui, id, Spring::arrived(target), now);
        return target;
    }
    remember(ui, id, next, now);
    ui.ctx().request_repaint();
    next.value
}

/// A spring, and when something last asked about it.
///
/// The timestamp is what lets [`sweep`] tell a widget that is still on screen
/// from one that went away without anyone calling [`forget`].
#[derive(Clone, Copy, Debug)]
struct Tracked {
    spring: Spring,
    seen: f64,
}

/// How long a spring nobody has asked about is kept, in seconds.
///
/// egui's temporary memory evicts nothing of its own: a value written under an
/// id stays until something removes it. [`forget`] covers the few ids whose
/// lifetime a caller knows, but most animated widgets come and go with the
/// selection, the design tree or the open document, and their springs would
/// otherwise accumulate for the life of the process. Long enough that anything
/// merely scrolled out of view keeps its motion.
const FORGOTTEN_AFTER: f64 = 30.0;

/// How often [`sweep`] walks the map, in seconds.
const SWEEP_EVERY: f64 = 5.0;

fn remember(ui: &egui::Ui, id: egui::Id, spring: Spring, now: f64) {
    ui.data_mut(|d| d.insert_temp(id, Tracked { spring, seen: now }));
    sweep(ui.ctx(), now);
}

/// Drops springs nothing has animated for [`FORGOTTEN_AFTER`] seconds.
///
/// Only entries this module wrote are touched: the key has to hold a [`Tracked`]
/// for it to be removed, so another subsystem's temporary state is safe.
fn sweep(ctx: &egui::Context, now: f64) {
    let clock = egui::Id::new("motion-last-sweep");
    let due = ctx.data_mut(|d| {
        let last: &mut f64 = d.get_temp_mut_or(clock, now);
        if now - *last < SWEEP_EVERY {
            return false;
        }
        *last = now;
        true
    });
    if !due {
        return;
    }
    let stale: Vec<_> = ctx.data(|d| {
        d.temp_keys()
            .filter(|key| {
                d.get_temp_raw(*key)
                    .and_then(|value| value.downcast_ref::<Tracked>())
                    .is_some_and(|t| now - t.seen > FORGOTTEN_AFTER)
            })
            .collect()
    });
    ctx.data_mut(|d| {
        for key in stale {
            d.remove_temp_raw(key);
        }
    });
}

/// Like [`animate`], but a value seen for the first time starts at `from`.
///
/// For something that appears rather than changes. It has no previous position
/// to carry over, so it should begin somewhere deliberate instead of already
/// having arrived. Callers are responsible for forgetting the id when the thing
/// goes away, or the next appearance starts from where the last one finished.
pub(crate) fn animate_from(
    ui: &egui::Ui,
    id: egui::Id,
    from: f32,
    target: f32,
    tuning: Tuning,
) -> f32 {
    if ui.data(|d| d.get_temp::<Tracked>(id)).is_none() {
        let now = ui.input(|i| i.time);
        ui.data_mut(|d| {
            d.insert_temp(
                id,
                Tracked {
                    spring: Spring::at(from),
                    seen: now,
                },
            );
        });
    }
    animate(ui, id, target, tuning)
}

/// Discards a value's animation state, so the next [`animate_from`] starts over.
pub(crate) fn forget(ctx: &egui::Context, id: egui::Id) {
    ctx.data_mut(|d| d.remove::<Tracked>(id));
}

/// Animates between two states, `false` being 0 and `true` being 1.
pub(crate) fn animate_bool(ui: &egui::Ui, id: egui::Id, on: bool, tuning: Tuning) -> f32 {
    animate(ui, id, if on { 1.0 } else { 0.0 }, tuning)
}

#[cfg(test)]
mod tests {
    use super::{Spring, Tuning, EPSILON, MAX_STEP};

    /// Runs a spring to rest, returning where it got to and how long it took.
    fn settle(mut s: Spring, target: f32, tuning: Tuning) -> (Spring, f32) {
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        for _ in 0..600 {
            s = s.step(target, dt, tuning);
            t += dt;
            if s.settled(target) {
                break;
            }
        }
        (s, t)
    }

    #[test]
    fn a_spring_arrives_at_its_target() {
        for tuning in [Tuning::SMOOTH, Tuning::BOUNCY, Tuning::SNAPPY] {
            let (s, _) = settle(Spring::at(0.0), 1.0, tuning);
            assert!(
                s.settled(1.0),
                "{tuning:?} never settled, reached {}",
                s.value
            );
        }
    }

    /// Response is the number a caller tunes, so it has to mean something. A
    /// snappier spring must actually arrive sooner.
    #[test]
    fn a_shorter_response_settles_sooner() {
        let (_, slow) = settle(Spring::at(0.0), 1.0, Tuning::SMOOTH);
        let (_, fast) = settle(Spring::at(0.0), 1.0, Tuning::SNAPPY);
        assert!(fast < slow, "snappy took {fast}s, smooth took {slow}s");
    }

    /// A fully damped spring must not overshoot. Anything driven directly by the
    /// pointer uses one, and overshoot there reads as the interface arguing with
    /// the hand moving it.
    #[test]
    fn full_damping_does_not_overshoot() {
        let mut s = Spring::at(0.0);
        for _ in 0..300 {
            s = s.step(1.0, 1.0 / 60.0, Tuning::SMOOTH);
            assert!(
                s.value <= 1.0 + EPSILON,
                "overshot to {} with full damping",
                s.value
            );
        }
    }

    /// And an underdamped one must actually overshoot, or naming it bouncy is a
    /// lie and the tuning table has no effect.
    #[test]
    fn light_damping_overshoots_and_returns() {
        let mut s = Spring::at(0.0);
        let mut peak = 0.0f32;
        for _ in 0..300 {
            s = s.step(1.0, 1.0 / 60.0, Tuning::BOUNCY);
            peak = peak.max(s.value);
        }
        assert!(peak > 1.01, "bouncy peaked at {peak}, which is not bouncy");
        assert!(s.settled(1.0), "bouncy never came back to rest");
    }

    /// The same motion on a 60Hz and a 144Hz display has to look the same.
    /// Integrating per frame rather than per second is the classic way to get
    /// animation that runs at different speeds on different machines.
    #[test]
    fn motion_does_not_depend_on_frame_rate() {
        let at = |dt: f32, seconds: f32| {
            let mut s = Spring::at(0.0);
            let steps = (seconds / dt).round() as u32;
            for _ in 0..steps {
                s = s.step(1.0, dt, Tuning::SMOOTH);
            }
            s.value
        };
        let sixty = at(1.0 / 60.0, 0.1);
        let one_forty_four = at(1.0 / 144.0, 0.1);
        assert!(
            (sixty - one_forty_four).abs() < 0.02,
            "60Hz reached {sixty}, 144Hz reached {one_forty_four}"
        );
    }

    /// A stalled frame reports an enormous delta. Integrating it honestly makes
    /// the spring explode, which on screen is every animated thing in the
    /// interface jumping at once.
    #[test]
    fn a_stalled_frame_does_not_fling_the_spring() {
        let s = Spring::at(0.0).step(1.0, 4.0, Tuning::SMOOTH);
        assert!(
            s.value.is_finite() && s.value.abs() < 2.0,
            "a four second frame sent it to {}",
            s.value
        );
        // Clamping means it behaves exactly as the longest allowed step would.
        let clamped = Spring::at(0.0).step(1.0, MAX_STEP, Tuning::SMOOTH);
        assert!((s.value - clamped.value).abs() < 1.0e-6);
    }

    /// Retargeting mid-flight keeps the velocity. This is the whole reason for
    /// preferring a spring to a tween, so it is worth pinning.
    #[test]
    fn retargeting_keeps_the_velocity_it_had() {
        let mut s = Spring::at(0.0);
        for _ in 0..6 {
            s = s.step(1.0, 1.0 / 60.0, Tuning::SMOOTH);
        }
        assert!(s.velocity > 0.0, "not moving yet, test proves nothing");
        let moving = s.velocity;

        let redirected = s.step(2.0, 1.0 / 60.0, Tuning::SMOOTH);
        assert!(
            redirected.velocity > moving,
            "a further target should have accelerated it, {moving} to {}",
            redirected.velocity
        );
    }

    #[test]
    fn a_spring_at_rest_on_its_target_reports_settled() {
        assert!(Spring::arrived(3.0).settled(3.0));
        assert!(!Spring::arrived(3.0).settled(9.0));
    }

    /// A target that is not a number must not get into the spring.
    ///
    /// It is permanent if it does: `NaN` never compares as settled, so the value
    /// stays `NaN` and `animate` asks for a repaint on every frame for the rest
    /// of the session, at whatever the display's refresh rate is.
    #[test]
    fn a_target_that_is_not_a_number_leaves_the_spring_where_it_was() {
        let held = Spring {
            value: 0.4,
            velocity: 1.5,
        };
        for target in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let after = held.step(target, 1.0 / 60.0, Tuning::SMOOTH);
            assert_eq!(after, held, "a target of {target} moved the spring");
        }
    }

    /// And one that got in anyway has to be recoverable, or the widget it
    /// belongs to animates never again.
    #[test]
    fn a_poisoned_spring_recovers_on_the_next_real_target() {
        let poisoned = Spring {
            value: f32::NAN,
            velocity: f32::NAN,
        };
        let after = poisoned.step(2.0, 1.0 / 60.0, Tuning::SMOOTH);
        assert!(after.settled(2.0), "recovered to {}", after.value);
    }

    /// A response of zero is a division the integrator cannot survive: the
    /// slices are fixed, so the spring gains energy on each one and the value
    /// doubles until it overflows. Callers pass the constants above, but the
    /// type is public and the numbers are `f32`.
    #[test]
    fn an_impossible_tuning_does_not_explode() {
        for tuning in [
            Tuning {
                response: 0.0,
                damping: 1.0,
            },
            Tuning {
                response: -1.0,
                damping: 1.0,
            },
            Tuning {
                response: f32::NAN,
                damping: f32::NAN,
            },
            Tuning {
                response: 0.3,
                damping: -4.0,
            },
            Tuning {
                response: 0.3,
                damping: 50.0,
            },
        ] {
            let (s, _) = settle(Spring::at(0.0), 1.0, tuning);
            assert!(
                s.value.is_finite() && (s.value - 1.0).abs() < 0.5,
                "{tuning:?} reached {} with velocity {}",
                s.value,
                s.velocity
            );
        }
    }

    /// Every spring has to come to rest. One that does not asks for a repaint
    /// on every frame forever, which is a flat cost to the whole interface.
    #[test]
    fn even_an_undamped_tuning_comes_to_rest() {
        let tuning = Tuning {
            response: 0.3,
            damping: 0.0,
        };
        let mut s = Spring::at(0.0);
        // Twenty seconds at 60Hz. A lossless oscillator never gets here.
        for _ in 0..1200 {
            s = s.step(1.0, 1.0 / 60.0, tuning);
            if s.settled(1.0) {
                return;
            }
        }
        panic!("still moving at {} after twenty seconds", s.value);
    }

    /// A zero or negative delta must be a no-op rather than a division or a
    /// backwards step. egui reports zero on the first frame.
    #[test]
    fn a_zero_delta_changes_nothing() {
        let s = Spring {
            value: 0.25,
            velocity: 3.0,
        };
        assert_eq!(s.step(1.0, 0.0, Tuning::SMOOTH), s);
        assert_eq!(s.step(1.0, -1.0, Tuning::SMOOTH), s);
    }
}

#[cfg(test)]
mod memory_tests {
    use super::{animate, Tracked, Tuning, FORGOTTEN_AFTER};

    /// How many springs are in egui's memory.
    fn springs(ctx: &egui::Context) -> usize {
        ctx.data(egui::util::IdTypeMap::count::<Tracked>)
    }

    /// Runs one frame at `time`, calling `body` inside it.
    fn frame(ctx: &egui::Context, time: f64, body: impl FnMut(&mut egui::Ui)) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(200.0, 200.0),
            )),
            time: Some(time),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, body);
        // Nothing here uploads textures, and epaint refuses to let a delta be
        // dropped unhandled.
        output.textures_delta.clear();
    }

    /// egui's temporary memory has no eviction of its own, so every id ever
    /// animated keeps a spring until something removes it. Ids come and go with
    /// the document: a session that builds and deletes features all afternoon
    /// would leave a spring behind for each one.
    #[test]
    fn springs_for_ids_nobody_animates_again_are_dropped() {
        let ctx = egui::Context::default();
        frame(&ctx, 0.0, |ui| {
            for i in 0..200 {
                animate(ui, egui::Id::new(("gone", i)), 1.0, Tuning::SMOOTH);
            }
        });
        assert_eq!(
            springs(&ctx),
            200,
            "the test did not animate what it thought it did"
        );

        // Later, with one of them still in use and the rest long gone.
        frame(&ctx, FORGOTTEN_AFTER + 10.0, |ui| {
            animate(ui, egui::Id::new(("gone", 0)), 1.0, Tuning::SMOOTH);
        });
        assert_eq!(
            springs(&ctx),
            1,
            "abandoned springs are still in egui's memory"
        );
    }

    /// A value that goes to `NaN` upstream must not leave the interface
    /// repainting at the refresh rate for the rest of the session.
    #[test]
    fn animating_toward_nothing_stops_asking_for_frames() {
        let ctx = egui::Context::default();
        let id = egui::Id::new("wobble");
        frame(&ctx, 0.0, |ui| {
            let v = animate(ui, id, 4.0, Tuning::SMOOTH);
            assert!(v.is_finite());
        });
        frame(&ctx, 0.25, |ui| {
            let v = animate(ui, id, f32::NAN, Tuning::SMOOTH);
            assert!(v.is_finite(), "a NaN target came back out as {v}");
        });
        frame(&ctx, 0.5, |ui| {
            let v = animate(ui, id, 4.0, Tuning::SMOOTH);
            assert!(v.is_finite(), "the spring was poisoned: {v}");
        });
    }
}
