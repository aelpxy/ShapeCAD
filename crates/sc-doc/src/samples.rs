//! Reference models.
//!
//! Shared by the CLI, the viewer, the benchmarks and the golden-geometry test,
//! so that all of them are talking about the same part. A change to a sample is
//! a change to a test fixture and will move its hash.

use crate::{add, Command, Document};
use sc_geom::glam::Vec3;
use sc_geom::{Node, Transform};

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
        let drill = add(
            &mut d,
            Node::Transform {
                child: drill,
                xform: Transform::from_translation(Vec3::new(x, 0.0, 0.0)),
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
