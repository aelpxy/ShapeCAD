//! Where a feature's dimensions can be grabbed.
//!
//! Every editable number in this application already has a name, because
//! `Node::params` is what the property panel and the agent layer both work
//! from. What it does not have is a place: nothing says that a box's `half_x`
//! lives on its right-hand face and grows along +X. That is what this module
//! adds, and it is the whole of what makes a dimension draggable.
//!
//! Kept separate from the drawing and the input handling because it is the only
//! part with a right answer. A grip in the wrong place is a bug that can be
//! written down as a failing test; a grip that looks slightly wrong is not.

use sc_geom::glam::Vec3;
use sc_geom::{Arena, Node, NodeId, Profile};

/// One draggable dimension, in the frame of the node that owns it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Handle {
    /// The parameter this grip drives, as `Node::params` names it.
    pub param: &'static str,
    /// Where the grip sits, in the node's own frame.
    pub at: Vec3,
    /// The direction the parameter increases in. Unit length.
    pub along: Vec3,
    /// Parameter units gained per millimetre the grip travels.
    ///
    /// One for a dimension measured from the centre, like a radius or a
    /// half-extent. Two for one measured across, like a rectangle's width,
    /// where moving one edge out by a millimetre makes the whole thing two
    /// millimetres wider.
    pub gain: f32,
}

impl Handle {
    const fn new(param: &'static str, at: Vec3, along: Vec3, gain: f32) -> Self {
        Self {
            param,
            at,
            along,
            gain,
        }
    }
}

/// The grips for a node, or empty if it has none worth showing.
///
/// Deliberately not every parameter. A rounding radius, a blend radius, an
/// offset distance and a shell thickness are all real numbers a user edits, and
/// none of them has a direction: the surface moves everywhere at once, so there
/// is nowhere honest to put a grip. Those stay in the property panel, which is
/// the right place for a scalar with no axis.
///
/// A transform's translation is also left out. Moving a feature is a different
/// gesture from resizing one, and giving them the same grips would make a
/// mis-grab change the wrong thing.
pub(crate) fn handles(arena: &Arena, id: NodeId) -> Vec<Handle> {
    let Some(node) = arena.get(id) else {
        return Vec::new();
    };
    match node {
        Node::Sphere { radius } => vec![Handle::new("radius", Vec3::X * *radius, Vec3::X, 1.0)],

        Node::Box { half, .. } => vec![
            Handle::new("half_x", Vec3::new(half.x, 0.0, 0.0), Vec3::X, 1.0),
            Handle::new("half_y", Vec3::new(0.0, half.y, 0.0), Vec3::Y, 1.0),
            Handle::new("half_z", Vec3::new(0.0, 0.0, half.z), Vec3::Z, 1.0),
        ],

        Node::Cylinder {
            radius,
            half_height,
            ..
        } => vec![
            Handle::new("radius", Vec3::new(*radius, 0.0, 0.0), Vec3::X, 1.0),
            Handle::new(
                "half_height",
                Vec3::new(0.0, 0.0, *half_height),
                Vec3::Z,
                1.0,
            ),
        ],

        Node::Torus { major, minor } => vec![
            Handle::new("major", Vec3::new(*major, 0.0, 0.0), Vec3::X, 1.0),
            // On the outside of the tube, so the two grips are a tube's width
            // apart and cannot be confused for one another.
            Handle::new("minor", Vec3::new(major + minor, 0.0, 0.0), Vec3::X, 1.0),
        ],

        // An extrusion runs from z = 0 to z = depth, so the depth grip sits on
        // the far face: the surface the user would push.
        Node::Extrude { profile, depth } => {
            let mut out = profile_handles(profile, *depth * 0.5);
            out.push(Handle::new(
                "depth",
                Vec3::new(0.0, 0.0, *depth),
                Vec3::Z,
                1.0,
            ));
            out
        }

        // A prism has no ends, so its grips sit on the plane it was drawn on.
        Node::Prism { profile } => profile_handles(profile, 0.0),

        _ => Vec::new(),
    }
}

/// Grips for a swept profile, lifted to `z`.
fn profile_handles(profile: &Profile, z: f32) -> Vec<Handle> {
    match profile {
        Profile::Rect { width, height } => vec![
            Handle::new("width", Vec3::new(width * 0.5, 0.0, z), Vec3::X, 2.0),
            Handle::new("height", Vec3::new(0.0, height * 0.5, z), Vec3::Y, 2.0),
        ],
        Profile::Circle { radius } | Profile::RegularPolygon { radius, .. } => vec![Handle::new(
            "radius",
            Vec3::new(*radius, 0.0, z),
            Vec3::X,
            1.0,
        )],
        // A path's shape is its points, not a number. Dragging those is sketch
        // editing, which is a different gesture.
        Profile::Path { .. } => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{handles, Handle};
    use sc_geom::glam::Vec3;
    use sc_geom::{Arena, Node, NodeId, Profile};

    fn arena_with(node: Node) -> (Arena, NodeId) {
        let mut arena = Arena::new();
        let id = arena.insert(node).expect("valid node");
        (arena, id)
    }

    fn grips(node: Node) -> Vec<Handle> {
        let (arena, id) = arena_with(node);
        handles(&arena, id)
    }

    fn grip(node: &Node, param: &str) -> Handle {
        grips(node.clone())
            .into_iter()
            .find(|h| h.param == param)
            .unwrap_or_else(|| panic!("{} has no {param} grip", node.kind()))
    }

    /// Every grip has to name a parameter the node will actually accept. A grip
    /// driving a name `set_param` rejects is a control that does nothing, and
    /// nothing in the drawing or the input handling would notice.
    #[test]
    fn every_grip_names_a_real_parameter() {
        let nodes = [
            Node::Sphere { radius: 5.0 },
            Node::Box {
                half: Vec3::splat(4.0),
                round: 0.5,
            },
            Node::Cylinder {
                radius: 3.0,
                half_height: 6.0,
                round: 0.0,
            },
            Node::Torus {
                major: 10.0,
                minor: 2.0,
            },
            Node::Extrude {
                profile: Profile::Rect {
                    width: 8.0,
                    height: 5.0,
                },
                depth: 4.0,
            },
            Node::Extrude {
                profile: Profile::Circle { radius: 3.0 },
                depth: 4.0,
            },
            Node::Prism {
                profile: Profile::RegularPolygon {
                    sides: 6,
                    radius: 4.0,
                },
            },
        ];
        for node in nodes {
            let names: Vec<&'static str> = node.params().into_iter().map(|(n, _)| n).collect();
            for handle in grips(node.clone()) {
                assert!(
                    names.contains(&handle.param),
                    "{} has a grip for {:?}, which is not one of {names:?}",
                    node.kind(),
                    handle.param
                );
                let mut probe = node.clone();
                assert!(
                    probe.set_param(handle.param, 1.0),
                    "{} refused to set {:?}",
                    node.kind(),
                    handle.param
                );
            }
        }
    }

    /// A grip sits on the surface it moves. That is the whole claim of direct
    /// manipulation: the thing under the pointer is the thing that changes.
    #[test]
    fn a_grip_sits_on_the_surface_it_drives() {
        let cases = [
            (Node::Sphere { radius: 7.0 }, "radius"),
            (
                Node::Box {
                    half: Vec3::new(4.0, 9.0, 2.0),
                    round: 0.0,
                },
                "half_y",
            ),
            (
                Node::Cylinder {
                    radius: 3.0,
                    half_height: 11.0,
                    round: 0.0,
                },
                "half_height",
            ),
        ];
        for (node, param) in cases {
            let (arena, id) = arena_with(node.clone());
            let handle = grip(&node, param);
            let d = sc_geom::eval(&arena, id, handle.at);
            assert!(
                d.abs() < 1.0e-3,
                "{}'s {param} grip is {d}mm off the surface",
                node.kind()
            );
        }
    }

    /// Pushing a grip outward has to make the feature bigger. A sign error here
    /// would make every drag fight the hand doing it.
    #[test]
    fn pushing_a_grip_outward_grows_the_feature() {
        for node in [
            Node::Sphere { radius: 5.0 },
            Node::Box {
                half: Vec3::splat(4.0),
                round: 0.0,
            },
            Node::Cylinder {
                radius: 3.0,
                half_height: 6.0,
                round: 0.0,
            },
            Node::Extrude {
                profile: Profile::Rect {
                    width: 8.0,
                    height: 5.0,
                },
                depth: 4.0,
            },
        ] {
            for handle in grips(node.clone()) {
                let (arena, id) = arena_with(node.clone());
                let before = sc_geom::bounds(&arena, id);

                let mut grown = node.clone();
                let current = node
                    .params()
                    .into_iter()
                    .find(|(n, _)| *n == handle.param)
                    .expect("the grip names a parameter")
                    .1;
                assert!(grown.set_param(handle.param, current + 2.0 * handle.gain));

                let (arena, id) = arena_with(grown);
                let after = sc_geom::bounds(&arena, id);
                let along = after.size() - before.size();
                let grew = along.x + along.y + along.z;
                assert!(
                    grew > 0.0,
                    "{} shrank when {} was pushed out",
                    node.kind(),
                    handle.param
                );
            }
        }
    }

    /// A width is measured across, a half-extent from the centre. Dragging one
    /// edge of a rectangle a millimetre makes it two millimetres wider, and the
    /// gain is what stops the geometry moving at half the speed of the pointer.
    #[test]
    fn gain_matches_how_the_dimension_is_measured() {
        let across = grip(
            &Node::Extrude {
                profile: Profile::Rect {
                    width: 8.0,
                    height: 5.0,
                },
                depth: 4.0,
            },
            "width",
        );
        assert!((across.gain - 2.0).abs() < 1.0e-6, "got {}", across.gain);

        let from_centre = grip(
            &Node::Box {
                half: Vec3::splat(4.0),
                round: 0.0,
            },
            "half_x",
        );
        assert!(
            (from_centre.gain - 1.0).abs() < 1.0e-6,
            "got {}",
            from_centre.gain
        );
    }

    /// The two grips on a torus must not land on top of each other, or one of
    /// them is unreachable.
    #[test]
    fn a_torus_keeps_its_two_grips_apart() {
        let node = Node::Torus {
            major: 10.0,
            minor: 2.0,
        };
        let major = grip(&node, "major");
        let minor = grip(&node, "minor");
        assert!(
            (major.at - minor.at).length() > 1.0,
            "grips at {:?} and {:?}",
            major.at,
            minor.at
        );
    }

    /// Nodes with no directional dimension get no grips, deliberately. An
    /// arbitrary grip position would be a control that lies about what it does.
    #[test]
    fn nodes_without_a_direction_have_no_grips() {
        let plain = [
            Node::Offset {
                child: NodeId(0),
                distance: 1.0,
            },
            Node::Shell {
                child: NodeId(0),
                thickness: 1.0,
            },
        ];
        for node in plain {
            let mut arena = Arena::new();
            let child = arena.insert(Node::Sphere { radius: 4.0 }).expect("valid");
            assert_eq!(child, NodeId(0));
            let id = arena.insert(node).expect("valid wrapper");
            assert!(handles(&arena, id).is_empty());
        }
    }

    #[test]
    fn a_missing_node_has_no_grips() {
        let arena = Arena::new();
        assert!(handles(&arena, NodeId(7)).is_empty());
    }
}
