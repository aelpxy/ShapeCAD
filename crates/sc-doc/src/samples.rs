//! Reference models.
//!
//! Shared by the CLI, the viewer, the benchmarks and the golden-geometry test,
//! so that all of them are talking about the same part. A change to a sample is
//! a change to a test fixture and will move its hash.

use crate::{add, Command, Document};
use sc_geom::glam::{Quat, Vec3};
use sc_geom::{Node, NodeId, Transform};

/// A small printable bracket, built entirely through the command log.
///
/// Deliberately exercises the operations that are hardest in a boundary
/// representation and trivial here: a 4mm fillet along an interior corner, and
/// holes cut clean through. Dimensions are millimetres.
///
/// # Panics
/// If any command is rejected, which would mean the kernel's validation rules
/// have changed underneath this fixture.
#[must_use]
pub fn bracket() -> Document {
    let mut d = Document::new();

    let plate = add(
        &mut d,
        Node::Box {
            half: Vec3::new(30.0, 20.0, 3.0),
            round: 1.0,
        },
    )
    .unwrap();

    let wall = add(
        &mut d,
        Node::Box {
            half: Vec3::new(30.0, 3.0, 15.0),
            round: 1.0,
        },
    )
    .unwrap();
    let wall = add(
        &mut d,
        Node::Transform {
            child: wall,
            xform: Transform::from_translation(Vec3::new(0.0, 17.0, 15.0)),
            on: None,
        },
    )
    .unwrap();

    // The blend radius *is* the fillet. No topology is consulted, so this cannot
    // fail the way a B-rep fillet on an interior corner routinely does.
    let body = add(
        &mut d,
        Node::Union {
            a: plate,
            b: wall,
            smooth: 4.0,
        },
    )
    .unwrap();

    let mut part = body;
    for x in [-20.0f32, 20.0] {
        let drill = add(
            &mut d,
            Node::Cylinder {
                radius: 2.5,
                half_height: 10.0,
                round: 0.0,
            },
        )
        .unwrap();
        // Named, like every feature. The design tree labels a boolean by the
        // feature it applied, so an unnamed cut reads as "Difference" and says
        // nothing about what it did.
        d.apply(Command::SetName {
            id: drill,
            name: Some("Bolt hole".into()),
        })
        .unwrap();
        let drill = add(
            &mut d,
            Node::Transform {
                child: drill,
                xform: Transform::from_translation(Vec3::new(x, 0.0, 0.0)),
                on: None,
            },
        )
        .unwrap();
        part = add(
            &mut d,
            Node::Difference {
                a: part,
                b: drill,
                smooth: 0.0,
            },
        )
        .unwrap();
    }

    // Sit the part on the build plate. Everything above is modelled about the
    // origin; this lifts it so z = 0 is the underside, which is where a printer
    // will actually put it.
    let part = add(
        &mut d,
        Node::Transform {
            child: part,
            xform: Transform::from_translation(Vec3::new(0.0, 0.0, 3.0)),
            on: None,
        },
    )
    .unwrap();

    d.apply(Command::SetRoot { root: Some(part) }).unwrap();
    d.apply(Command::SetName {
        id: body,
        name: Some("body".into()),
    })
    .unwrap();
    d.apply(Command::SetName {
        id: part,
        name: Some("bracket".into()),
    })
    .unwrap();
    d
}

/// Places a node, skipping the transform when it would be the identity.
fn at(d: &mut Document, child: NodeId, position: Vec3) -> NodeId {
    if position == Vec3::ZERO {
        return child;
    }
    add(
        d,
        Node::Transform {
            child,
            xform: Transform::from_translation(position),
            on: None,
        },
    )
    .unwrap()
}

/// Places a node and turns it, for the parts of an engine that do not stand
/// upright: a port lies along X, a cylinder bore stands along Z.
fn at_turned(d: &mut Document, child: NodeId, position: Vec3, rotation: Quat) -> NodeId {
    add(
        d,
        Node::Transform {
            child,
            xform: Transform {
                translation: position,
                rotation,
                scale: 1.0,
            },
            on: None,
        },
    )
    .unwrap()
}

fn joined(d: &mut Document, a: NodeId, b: NodeId, smooth: f32) -> NodeId {
    add(d, Node::Union { a, b, smooth }).unwrap()
}

fn carved(d: &mut Document, a: NodeId, b: NodeId) -> NodeId {
    add(d, Node::Difference { a, b, smooth: 0.0 }).unwrap()
}

/// A finite bore: a circle swept a known distance, for a hole that stops.
///
/// Named, like every other feature here. The design tree shows a boolean by the
/// feature it applied, so an unnamed cut reads as "difference" and a part with
/// eleven of them reads as nothing at all.
fn bore(
    d: &mut Document,
    name: &str,
    radius: f32,
    depth: f32,
    position: Vec3,
    rotation: Quat,
) -> NodeId {
    let cut = add(
        d,
        Node::Extrude {
            profile: sc_geom::Profile::Circle { radius },
            depth,
        },
    )
    .unwrap();
    named(d, cut, name);
    at_turned(d, cut, position, rotation)
}

/// Labels a node, so the tree has something to call it.
fn named(d: &mut Document, id: NodeId, name: &str) {
    d.apply(Command::SetName {
        id,
        name: Some(name.into()),
    })
    .unwrap();
}

/// The crankcase: a rounded box with its chamber taken out of the inside.
fn crankcase(d: &mut Document) -> (NodeId, NodeId) {
    let case = add(
        d,
        Node::Box {
            half: Vec3::new(48.0, 38.0, 27.0),
            round: 6.0,
        },
    )
    .unwrap();
    let case = at(d, case, Vec3::new(0.0, 0.0, 27.0));

    let chamber = add(
        d,
        Node::Box {
            half: Vec3::new(40.0, 30.0, 18.0),
            round: 4.0,
        },
    )
    .unwrap();
    let chamber = at(d, chamber, Vec3::new(0.0, 0.0, 26.0));
    (case, chamber)
}

/// The barrel and the fin stack around it.
///
/// Air cooling is the reason an engine looks like an engine, and it is a good
/// test of the kernel: eight discs unioned onto a cylinder is a shape a boundary
/// representation has to think about and this one does not.
fn barrel(d: &mut Document) -> NodeId {
    /// Fins, their spacing, and where the stack starts.
    const FINS: u32 = 8;
    const PITCH: f32 = 7.0;
    const FIRST: f32 = 58.0;

    let mut stack = add(
        d,
        Node::Cylinder {
            radius: 27.0,
            half_height: 28.0,
            round: 1.0,
        },
    )
    .unwrap();
    stack = at(d, stack, Vec3::new(0.0, 0.0, 82.0));

    for i in 0..FINS {
        let fin = add(
            d,
            Node::Cylinder {
                radius: 36.0,
                half_height: 2.0,
                round: 0.8,
            },
        )
        .unwrap();
        let z = FIRST + PITCH * i as f32;
        let fin = at(d, fin, Vec3::new(0.0, 0.0, z));
        stack = joined(d, stack, fin, 1.5);
    }
    stack
}

/// The head, its plug boss, and the two ports either side.
fn head(d: &mut Document) -> NodeId {
    let block = add(
        d,
        Node::Box {
            half: Vec3::new(42.0, 36.0, 13.0),
            round: 5.0,
        },
    )
    .unwrap();
    let mut head = at(d, block, Vec3::new(0.0, 0.0, 123.0));

    let boss = add(
        d,
        Node::Cylinder {
            radius: 8.0,
            half_height: 8.0,
            round: 1.0,
        },
    )
    .unwrap();
    let boss = at(d, boss, Vec3::new(0.0, 28.0, 132.0));
    head = joined(d, head, boss, 2.5);

    // A cylinder stands along Z, so a port lying along X is a quarter turn about
    // Y. The same rotation serves for the boss and for the bore through it.
    let lying = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    for side in [-1.0f32, 1.0] {
        let port = add(
            d,
            Node::Cylinder {
                radius: 11.0,
                half_height: 12.0,
                round: 1.0,
            },
        )
        .unwrap();
        let port = at_turned(d, port, Vec3::new(46.0 * side, 0.0, 123.0), lying);
        head = joined(d, head, port, 2.5);
    }
    head
}

/// The two mounting feet the engine bolts down through.
fn feet(d: &mut Document, body: NodeId) -> NodeId {
    let mut body = body;
    for side in [-1.0f32, 1.0] {
        let foot = add(
            d,
            Node::Box {
                half: Vec3::new(12.0, 38.0, 5.0),
                round: 2.0,
            },
        )
        .unwrap();
        let foot = at(d, foot, Vec3::new(54.0 * side, 0.0, 5.0));
        body = joined(d, body, foot, 3.0);
    }
    body
}

/// Every hole in the engine, cut after the body is whole.
///
/// Order matters: a bore taken out before the fins were unioned on would be
/// filled straight back in by them.
fn drillings(d: &mut Document, body: NodeId, chamber: NodeId) -> NodeId {
    named(d, chamber, "Crank chamber");
    let mut body = carved(d, body, chamber);

    // The cylinder bore is a through cut, so it is a prism rather than a
    // measured extrusion: it has no depth to fall out of date when the barrel or
    // the head changes height.
    let cylinder = add(
        d,
        Node::Prism {
            profile: sc_geom::Profile::Circle { radius: 19.0 },
        },
    )
    .unwrap();
    named(d, cylinder, "Cylinder bore");
    body = carved(d, body, cylinder);

    let upright = Quat::IDENTITY;
    for x in [-33.0f32, 33.0] {
        for y in [-28.0f32, 28.0] {
            let hole = bore(d, "Head bolt", 4.0, 34.0, Vec3::new(x, y, 104.0), upright);
            body = carved(d, body, hole);
        }
    }

    let plug = bore(
        d,
        "Plug thread",
        4.5,
        30.0,
        Vec3::new(0.0, 28.0, 116.0),
        upright,
    );
    body = carved(d, body, plug);

    let lying = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    for side in [-1.0f32, 1.0] {
        // Started outside the casting and swept inward, so the port opens on the
        // outer face rather than leaving a skin over it.
        let name = if side < 0.0 {
            "Intake port"
        } else {
            "Exhaust port"
        };
        let port = bore(
            d,
            name,
            7.0,
            34.0,
            Vec3::new(60.0 * side, 0.0, 123.0),
            lying,
        );
        body = carved(d, body, port);
    }

    for x in [-54.0f32, 54.0] {
        for y in [-26.0f32, 26.0] {
            let hole = bore(
                d,
                "Mounting bolt",
                4.5,
                18.0,
                Vec3::new(x, y, -2.0),
                upright,
            );
            body = carved(d, body, hole);
        }
    }
    body
}

/// A single cylinder air-cooled engine block, built through the command log.
///
/// Deliberately larger and messier than [`bracket`]: a crankcase with its
/// chamber hollowed out, a finned barrel, a head with a plug boss and two ports,
/// mounting feet, and eleven holes of three different kinds. It exists to be
/// opened and pulled apart, and to be the case that shows whether the tree, the
/// property panel and the mesher hold up on something with real depth to it.
///
/// # Panics
/// If any command is rejected, which would mean the kernel's validation rules
/// have changed underneath this fixture.
#[must_use]
pub fn engine() -> Document {
    let mut d = Document::new();

    let (case, chamber) = crankcase(&mut d);
    let barrel = barrel(&mut d);
    let head = head(&mut d);

    // Cast in one piece, so the joins are blended rather than butted.
    let mut body = joined(&mut d, case, barrel, 5.0);
    body = joined(&mut d, body, head, 3.0);
    body = feet(&mut d, body);
    let part = drillings(&mut d, body, chamber);

    d.apply(Command::SetRoot { root: Some(part) }).unwrap();
    for (id, name) in [
        (case, "crankcase"),
        (barrel, "barrel"),
        (head, "head"),
        (part, "engine"),
    ] {
        d.apply(Command::SetName {
            id,
            name: Some(name.into()),
        })
        .unwrap();
    }
    d
}
