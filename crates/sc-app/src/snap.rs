//! Where a drag lands.
//!
//! A grid is the floor of precision, not the whole of it. Most of the time the
//! number somebody wants is not a round one: it is the number that puts this
//! boss on the same centreline as that one, or this wall flush with that face.
//! Those are relationships between features, and a grid cannot express them.
//!
//! So a drag is offered lines from the other features in the model and latches
//! onto the nearest one, falling back to the grid when nothing is close. The
//! module is pure arithmetic on one axis at a time, which is what makes it
//! testable without a document, a camera or a pointer.
//!
//! One axis at a time is also what makes it compose with axis locking: a locked
//! drag simply has nothing to contribute on the two axes it cannot move along.

use sc_geom::glam::Vec3;

/// Which part of a feature a coordinate belongs to.
///
/// Kept alongside the number because the readout has to say what was matched.
/// "Snapped" on its own is not information; "centre" tells you why the part
/// stopped where it did, and lets you tell a wanted alignment from an accident.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Edge {
    /// The low side of the feature's bounding box on this axis.
    Min,
    /// Its midpoint.
    Centre,
    /// The high side.
    Max,
}

impl Edge {
    /// The three of them, in the order the offsets in [`Extent`] are stored.
    pub(crate) const ALL: [Self; 3] = [Self::Min, Self::Centre, Self::Max];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Min => "min",
            Self::Centre => "centre",
            Self::Max => "max",
        }
    }
}

/// A coordinate another feature offers on one axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Line {
    /// Where it is, in the frame the drag is measured in.
    pub at: f32,
    /// What part of that feature it is.
    pub edge: Edge,
    /// The centre of the feature offering it.
    ///
    /// Carried so the guide can be drawn between the two things that are now
    /// aligned. A line with no visible end says that something snapped; a line
    /// reaching the feature it snapped to says what to.
    pub from: Vec3,
}

/// Where the moving feature's own three coordinates sit relative to its origin.
///
/// Constant for the whole drag, because a move changes the origin and nothing
/// else. Computed once when the gesture starts rather than per frame, so a
/// rounding wobble in the bounds cannot make the offsets breathe.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Extent {
    /// Min, centre and max offsets, in the same order as [`Edge::ALL`].
    pub offsets: [Vec3; 3],
}

impl Extent {
    /// The three offsets on one axis, `axis` being 0, 1 or 2.
    fn on(self, axis: usize) -> [f32; 3] {
        [
            self.offsets[0][axis],
            self.offsets[1][axis],
            self.offsets[2][axis],
        ]
    }
}

/// What a snapped coordinate latched onto.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Latch {
    /// Rounded to the grid. The ordinary case, and not worth announcing.
    Grid,
    /// The origin.
    Zero,
    /// A coordinate shared with another feature.
    Feature {
        /// Where the shared coordinate is.
        at: f32,
        /// The centre of the feature it came from, so the guide has two ends.
        from: Vec3,
        /// The moving feature's own edge that landed on it.
        mine: Edge,
        /// The other feature's edge it landed on.
        theirs: Edge,
    },
}

impl Latch {
    /// Where to draw a guide to, when this is worth drawing one for.
    pub(crate) fn guide(self) -> Option<Vec3> {
        match self {
            Self::Feature { from, .. } => Some(from),
            Self::Grid | Self::Zero => None,
        }
    }

    /// How the readout describes this, or nothing when there is nothing to say.
    pub(crate) fn label(self) -> Option<String> {
        match self {
            Self::Zero => Some("zero".to_string()),
            Self::Feature { mine, theirs, .. } => {
                Some(format!("{} to {}", mine.label(), theirs.label()))
            }
            Self::Grid => None,
        }
    }

    /// Preference between two candidates the same distance away.
    ///
    /// Exact ties are not a curiosity here: a symmetric model offers the same
    /// coordinate from both sides of itself, and centring one feature on
    /// another makes min-to-min and max-to-max tie with centre-to-centre. The
    /// centre is what was meant, so it is what gets reported.
    fn rank(self) -> u8 {
        match self {
            Self::Zero => 0,
            Self::Feature {
                mine: Edge::Centre,
                theirs: Edge::Centre,
                ..
            } => 1,
            Self::Feature { .. } => 2,
            Self::Grid => 3,
        }
    }
}

/// How many grid steps either side of zero snap to it.
///
/// Zero is not just another grid line. A feature on an axis, or centred on the
/// plate, is a thing people deliberately want and then check by reading the
/// number back, so it gets a wider catchment than the grid spacing alone would
/// give it.
const ZERO_PULL: f32 = 0.75;

/// The smallest grid spacing the arithmetic will work with.
///
/// A zero step would divide by zero, and a step below this is finer than the
/// numbers are displayed to anyway.
const MIN_GRID: f32 = 0.01;

/// Snap one coordinate.
///
/// `want` is where the pointer put the feature's origin on this axis, `extent`
/// its own three offsets, and `lines` the coordinates other features offer.
/// `reach` is how far a feature line pulls from, in the same units; see
/// [`reach`] for why it is a screen distance rather than a fixed one.
///
/// Returns the coordinate to use and what it latched onto. Nothing within reach
/// means the grid, which is the behaviour of an empty document and of a part
/// being dragged somewhere nothing else is.
pub(crate) fn axis(
    want: f32,
    extent: [f32; 3],
    lines: &[Line],
    reach: f32,
    grid: f32,
) -> (f32, Latch) {
    let grid = grid.max(MIN_GRID);

    // Every way this feature could line up with that one: any of my three
    // coordinates onto any of theirs. Matching only centres would miss flush
    // faces, which is most of what anyone aligns by hand.
    let mut best: Option<(f32, f32, Latch)> = None;
    let offer = |at: f32, limit: f32, latch: Latch, best: &mut Option<(f32, f32, Latch)>| {
        let d = (at - want).abs();
        if d > limit {
            return;
        }
        let better = match best {
            None => true,
            Some((bd, _, blatch)) => match d.total_cmp(bd) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Equal => latch.rank() < blatch.rank(),
                std::cmp::Ordering::Greater => false,
            },
        };
        if better {
            *best = Some((d, at, latch));
        }
    };

    for line in lines {
        for (i, mine) in Edge::ALL.into_iter().enumerate() {
            // The origin has to go where it puts my edge on their line, which
            // is their coordinate less my offset, not their coordinate.
            let at = line.at - extent[i];
            offer(
                at,
                reach,
                Latch::Feature {
                    at: line.at,
                    from: line.from,
                    mine,
                    theirs: line.edge,
                },
                &mut best,
            );
        }
    }
    offer(0.0, grid * ZERO_PULL, Latch::Zero, &mut best);

    if let Some((_, at, latch)) = best {
        return (at, latch);
    }
    ((want / grid).round() * grid, Latch::Grid)
}

/// How far a feature line pulls from, in world units.
///
/// A screen distance rather than a world one, so the pull feels the same
/// whatever the zoom: a snap you have to fight at one zoom and cannot escape at
/// another is worse than none. `per_pixel` comes from the camera.
///
/// Clamped against the grid at both ends. Zoomed far out a few points of screen
/// can be tens of millimetres, which would have parts snapping to things on the
/// other side of the plate; zoomed far in it can be microns, which is no snap
/// at all.
pub(crate) fn reach(per_pixel: f32, grid: f32) -> f32 {
    /// Screen points a feature line pulls from.
    const POINTS: f32 = 8.0;

    let grid = grid.max(MIN_GRID);
    (per_pixel * POINTS).clamp(grid * 0.5, grid * 4.0)
}

/// Snap a whole position, one axis at a time.
///
/// `lines` holds the coordinates offered on X, Y and Z in that order.
pub(crate) fn position(
    want: Vec3,
    extent: Extent,
    lines: &[Vec<Line>; 3],
    reach: f32,
    grid: f32,
) -> (Vec3, [Latch; 3]) {
    let mut at = Vec3::ZERO;
    let mut latches = [Latch::Grid; 3];
    for i in 0..3 {
        let (v, latch) = axis(want[i], extent.on(i), &lines[i], reach, grid);
        at[i] = v;
        latches[i] = latch;
    }
    (at, latches)
}

#[cfg(test)]
mod tests {
    use super::{axis, position, reach, Edge, Extent, Latch, Line};
    use sc_geom::glam::Vec3;

    /// A feature with no thickness, so tests can talk about one coordinate.
    const POINT: [f32; 3] = [0.0, 0.0, 0.0];

    /// Nothing to snap to is the empty document, and it still has to round.
    #[test]
    fn with_no_lines_it_rounds_to_the_grid() {
        let (at, latch) = axis(7.1, POINT, &[], 0.1, 1.0);
        assert!((at - 7.0).abs() < 1.0e-6, "{at}");
        assert_eq!(latch, Latch::Grid);
    }

    /// The whole point of the feature. Without it, a coordinate a fraction off
    /// a neighbour's centreline rounds to the grid and stays a fraction off.
    #[test]
    fn a_feature_line_beats_the_grid() {
        let lines = [Line {
            at: 7.3,
            edge: Edge::Centre,
            from: Vec3::ZERO,
        }];
        let (at, latch) = axis(7.1, POINT, &lines, 0.5, 1.0);
        assert!(
            (at - 7.3).abs() < 1.0e-6,
            "rounded to the grid instead: {at}"
        );
        assert!(
            matches!(
                latch,
                Latch::Feature {
                    theirs: Edge::Centre,
                    ..
                }
            ),
            "{latch:?}"
        );
    }

    /// Out of reach is out of reach. A snap that reaches across the plate is
    /// not precision, it is the part refusing to go where it was put.
    #[test]
    fn a_line_out_of_reach_is_ignored() {
        let lines = [Line {
            at: 7.3,
            edge: Edge::Centre,
            from: Vec3::ZERO,
        }];
        let (at, latch) = axis(7.1, POINT, &lines, 0.05, 1.0);
        assert!((at - 7.0).abs() < 1.0e-6, "{at}");
        assert_eq!(latch, Latch::Grid);
    }

    /// Flush faces, which is most of what anyone aligns by hand. The origin has
    /// to land two millimetres short of their face so that my own face, two
    /// millimetres out from my origin, lands on it.
    #[test]
    fn my_max_can_land_on_their_max() {
        let lines = [Line {
            at: 10.0,
            edge: Edge::Max,
            from: Vec3::ZERO,
        }];
        let (at, latch) = axis(8.1, [-2.0, 0.0, 2.0], &lines, 0.5, 1.0);
        assert!((at - 8.0).abs() < 1.0e-6, "{at}");
        assert_eq!(
            latch,
            Latch::Feature {
                at: 10.0,
                from: Vec3::ZERO,
                mine: Edge::Max,
                theirs: Edge::Max,
            }
        );
    }

    /// A symmetric feature offers min-to-min and max-to-max at the same
    /// distance as centre-to-centre. Centring is what was meant, and the
    /// readout has to say so rather than pick whichever came first.
    #[test]
    fn a_tie_reports_the_centre() {
        // Away from the origin, because at the origin zero is the better
        // explanation and rightly wins.
        let lines = [
            Line {
                at: 18.0,
                edge: Edge::Min,
                from: Vec3::ZERO,
            },
            Line {
                at: 20.0,
                edge: Edge::Centre,
                from: Vec3::ZERO,
            },
            Line {
                at: 22.0,
                edge: Edge::Max,
                from: Vec3::ZERO,
            },
        ];
        let (at, latch) = axis(20.05, [-2.0, 0.0, 2.0], &lines, 0.5, 10.0);
        assert!((at - 20.0).abs() < 1.0e-6, "{at}");
        assert!(
            matches!(
                latch,
                Latch::Feature {
                    mine: Edge::Centre,
                    theirs: Edge::Centre,
                    ..
                }
            ),
            "{latch:?}"
        );
    }

    /// Zero keeps the wider catchment it had before features could be snapped
    /// to, and still wins against a grid line it is nearer than.
    #[test]
    fn zero_still_pulls_wider_than_the_grid() {
        let (at, latch) = axis(0.6, POINT, &[], 0.1, 1.0);
        assert!(at.abs() < 1.0e-6, "{at}");
        assert_eq!(latch, Latch::Zero);
        // Beyond the pull it rounds like anything else.
        let (at, latch) = axis(0.8, POINT, &[], 0.1, 1.0);
        assert!((at - 1.0).abs() < 1.0e-6, "{at}");
        assert_eq!(latch, Latch::Grid);
    }

    /// Axes do not leak into each other. A line on X must not move Y.
    #[test]
    fn each_axis_is_snapped_on_its_own() {
        let lines = [
            vec![Line {
                at: 3.0,
                edge: Edge::Centre,
                from: Vec3::ZERO,
            }],
            Vec::new(),
            Vec::new(),
        ];
        let (at, latches) = position(
            Vec3::new(3.1, 7.1, 7.1),
            Extent::default(),
            &lines,
            0.5,
            1.0,
        );
        assert!((at.x - 3.0).abs() < 1.0e-6, "{at}");
        assert!((at.y - 7.0).abs() < 1.0e-6, "{at}");
        assert!(matches!(latches[0], Latch::Feature { .. }));
        assert_eq!(latches[1], Latch::Grid);
    }

    /// The pull is a screen distance, so zooming must change it, and the clamp
    /// must stop it running away at either extreme.
    #[test]
    fn the_pull_is_screen_sized_and_bounded() {
        let near = reach(0.01, 1.0);
        let far = reach(0.2, 1.0);
        assert!(far > near, "zooming out did not widen the pull");
        assert!(reach(100.0, 1.0) <= 4.0, "unbounded when zoomed far out");
        assert!(reach(0.0, 1.0) >= 0.5, "vanished when zoomed far in");
    }

    /// Only a feature latch gets a guide. Drawing one for every grid rounding
    /// would put a line on screen through the whole of every drag.
    #[test]
    fn only_a_feature_latch_draws_a_guide() {
        assert_eq!(Latch::Grid.guide(), None);
        assert_eq!(Latch::Zero.guide(), None);
        assert_eq!(
            Latch::Feature {
                at: 4.0,
                from: Vec3::X,
                mine: Edge::Min,
                theirs: Edge::Max,
            }
            .guide(),
            Some(Vec3::X)
        );
    }
}
